//! Non-interactive query execution for scripts and agents.
//!
//! `tsql exec` runs one PostgreSQL statement and exits. It does not start the
//! TUI. `--read-only` reuses [`classify_postgres`] — the same check the TUI
//! uses before write confirmation — and also opens a read-only transaction.
//! `--timeout` is applied with PostgreSQL `statement_timeout`.

mod output;
mod session;

use std::io::{self, IsTerminal, Read};
use std::time::Duration;

use crate::app::{classify_postgres, QuerySafety};

pub use output::OutputFormat;

const HELP: &str = r#"tsql exec - run one PostgreSQL query without the TUI

Usage:
  tsql exec [OPTIONS] [CONNECTION_URL]

CONNECTION_URL is a postgres:// or postgresql:// URL, or a libpq keyword
string (host=... dbname=...). When it is omitted, DATABASE_URL is used.

Options:
  -e, --execute <SQL>       SQL to run
  -f, --format <FORMAT>     table (default), csv, or json
      --json                Shorthand for --format json
      --csv                 Shorthand for --format csv
      --table               Shorthand for --format table
      --read-only           Refuse non-read SQL and enforce a read-only
                            transaction
      --timeout <DURATION>  Server-side statement_timeout (30s, 500ms, 2m,
                            or a bare number of seconds)
  -h, --help                Show this help

SQL is read from stdin when -e/--execute is omitted. Result data is written
to stdout. Refusals, timeouts, and database errors are written to stderr.

JSON cells are PostgreSQL's text representation. SQL NULL is JSON null.
CSV renders NULL as an empty field.

Exit codes:
  0  the statement finished
  1  usage error, or PostgreSQL rejected the statement
  2  read-only mode refused the statement
  3  could not connect
  4  the statement timed out

Read-only mode classifies the statement with the same query-safety check the
TUI uses for write confirmation and saved read-only connections. Writes,
destructive statements, and anything that check cannot prove is a read are
refused before a connection is opened. Statements that do run are wrapped in
BEGIN READ ONLY, with default_transaction_read_only enabled, so a statement
the classifier sees as a read (for example SELECT of a writing function)
still cannot mutate data.

--timeout sets statement_timeout on the server: SET LOCAL inside the
read-only transaction, or a session SET when writes are allowed. PostgreSQL
cancels the statement and tsql exits 4.

Examples:
  tsql exec postgres://localhost/app --read-only --timeout 30s --format json \
    -e "SELECT id, name FROM users"
  printf '%s\n' 'SELECT 1' | tsql exec "$DATABASE_URL" --format csv
"#;

/// What to run. Built by the CLI or by a caller that already has the SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub url: String,
    pub sql: String,
    pub read_only: bool,
    pub timeout: Option<Duration>,
    pub format: OutputFormat,
}

/// Failure of one headless execution. The message is a single user-facing error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// The invocation itself is invalid.
    Usage(String),
    /// `--read-only` rejected the statement, or the server rejected a write.
    Refused(String),
    /// The server could not be reached.
    Connect(String),
    /// PostgreSQL rejected the statement for a reason other than timeout or
    /// read-only enforcement.
    Query(String),
    /// `statement_timeout` fired, or the client backstop gave up.
    Timeout(String),
}

impl ExecError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::Query(_) => 1,
            Self::Refused(_) => 2,
            Self::Connect(_) => 3,
            Self::Timeout(_) => 4,
        }
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message)
            | Self::Refused(message)
            | Self::Connect(message)
            | Self::Query(message)
            | Self::Timeout(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ExecError {}

#[derive(Debug)]
enum CliOutput {
    Help(String),
    Query(String),
}

#[derive(Debug, PartialEq, Eq)]
enum ExecCommand {
    Help,
    Run(ParsedRun),
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedRun {
    url: String,
    sql: SqlInput,
    read_only: bool,
    timeout: Option<Duration>,
    format: OutputFormat,
}

#[derive(Debug, PartialEq, Eq)]
enum SqlInput {
    Text(String),
    Stdin,
}

/// Parse `argv` and run one query. Returns a process exit code.
///
/// Data is printed to stdout. Errors are printed to stderr as `tsql: error: ...`.
pub fn run(args: &[String]) -> i32 {
    match execute_cli(args) {
        Ok(CliOutput::Help(text)) => {
            print!("{text}");
            0
        }
        Ok(CliOutput::Query(text)) => {
            print!("{text}");
            0
        }
        Err(error) => {
            eprintln!("tsql: error: {error}");
            error.exit_code()
        }
    }
}

/// Connect, apply the safety and timeout policy, and return the formatted result.
pub async fn run_query(request: ExecRequest) -> Result<String, ExecError> {
    let sql = nonempty(request.sql)?;
    let url = request.url.trim().to_string();
    validate_connection(&url)?;

    if request.read_only {
        if let Some(reason) = read_only_refusal(&sql) {
            return Err(ExecError::Refused(reason));
        }
    }

    let sets = session::execute(&url, &sql, request.read_only, request.timeout).await?;
    Ok(output::render(request.format, &sets))
}

fn execute_cli(args: &[String]) -> Result<CliOutput, ExecError> {
    let database_url = std::env::var("DATABASE_URL").ok();
    match parse_args(args, database_url.as_deref())? {
        ExecCommand::Help => Ok(CliOutput::Help(HELP.to_string())),
        ExecCommand::Run(parsed) => {
            let sql = load_sql(parsed.sql)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    ExecError::Query(format!("failed to start the async runtime: {error}"))
                })?;
            let text = runtime.block_on(run_query(ExecRequest {
                url: parsed.url,
                sql,
                read_only: parsed.read_only,
                timeout: parsed.timeout,
                format: parsed.format,
            }))?;
            Ok(CliOutput::Query(text))
        }
    }
}

fn parse_args(args: &[String], database_url: Option<&str>) -> Result<ExecCommand, ExecError> {
    if args.get(1).map(String::as_str) != Some("exec") {
        return Err(ExecError::Usage(
            "usage: tsql exec [OPTIONS] [CONNECTION_URL]".to_string(),
        ));
    }

    let mut url = None;
    let mut sql = None;
    let mut read_only = false;
    let mut timeout = None;
    let mut format = OutputFormat::Table;
    let mut end_of_options = false;
    let mut index = 2;

    while index < args.len() {
        let arg = args[index].as_str();
        if end_of_options || arg == "-" || !arg.starts_with('-') {
            push_url(&mut url, arg)?;
            index += 1;
            continue;
        }
        if arg == "--" {
            end_of_options = true;
            index += 1;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            return Ok(ExecCommand::Help);
        }
        if arg == "--read-only" {
            read_only = true;
            index += 1;
            continue;
        }
        if arg == "--json" {
            format = OutputFormat::Json;
            index += 1;
            continue;
        }
        if arg == "--csv" {
            format = OutputFormat::Csv;
            index += 1;
            continue;
        }
        if arg == "--table" {
            format = OutputFormat::Table;
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--execute=") {
            sql = Some(SqlInput::Text(value.to_string()));
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--timeout=") {
            timeout = Some(parse_timeout(value)?);
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--format=") {
            format = parse_format(value)?;
            index += 1;
            continue;
        }
        if arg == "-e" || arg == "--execute" {
            let value = next_value(args, &mut index, "--execute")?;
            sql = Some(SqlInput::Text(value));
            index += 1;
            continue;
        }
        if arg == "--timeout" {
            let value = next_value(args, &mut index, "--timeout")?;
            timeout = Some(parse_timeout(&value)?);
            index += 1;
            continue;
        }
        if arg == "-f" || arg == "--format" {
            let value = next_value(args, &mut index, "--format")?;
            format = parse_format(&value)?;
            index += 1;
            continue;
        }
        return Err(ExecError::Usage(format!("unknown option '{arg}'")));
    }

    let url = match url {
        Some(url) => url,
        None => database_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                ExecError::Usage(
                    "connection URL is required; pass one or set DATABASE_URL".to_string(),
                )
            })?
            .to_string(),
    };

    Ok(ExecCommand::Run(ParsedRun {
        url,
        sql: sql.unwrap_or(SqlInput::Stdin),
        read_only,
        timeout,
        format,
    }))
}

fn push_url(url: &mut Option<String>, arg: &str) -> Result<(), ExecError> {
    if url.is_some() {
        return Err(ExecError::Usage(format!("unexpected argument '{arg}'")));
    }
    *url = Some(arg.to_string());
    Ok(())
}

fn next_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, ExecError> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| ExecError::Usage(format!("{flag} requires a value")))
}

fn parse_format(value: &str) -> Result<OutputFormat, ExecError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(OutputFormat::Json),
        "csv" => Ok(OutputFormat::Csv),
        "table" => Ok(OutputFormat::Table),
        other => Err(ExecError::Usage(format!(
            "unknown format '{other}' (expected table, csv, or json)"
        ))),
    }
}

/// Parse a timeout such as `30s`, `500ms`, `2m`, or a bare number of seconds.
pub fn parse_timeout(raw: &str) -> Result<Duration, ExecError> {
    let raw = raw.trim();
    let split = raw
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(raw.len());
    if split == 0 || raw.is_empty() {
        return Err(invalid_timeout(raw));
    }
    let (number, unit) = raw.split_at(split);
    let value: u64 = number.parse().map_err(|_| invalid_timeout(raw))?;
    if value == 0 {
        return Err(ExecError::Usage(
            "timeout must be greater than zero".to_string(),
        ));
    }
    let duration = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => Duration::from_secs(value),
        "ms" | "msec" | "millis" | "millisecond" | "milliseconds" => Duration::from_millis(value),
        "m" | "min" | "mins" | "minute" | "minutes" => {
            let seconds = value.checked_mul(60).ok_or_else(|| {
                ExecError::Usage("timeout is too large for PostgreSQL statement_timeout".into())
            })?;
            Duration::from_secs(seconds)
        }
        _ => return Err(invalid_timeout(raw)),
    };
    let millis = duration.as_millis();
    if millis == 0 || millis > i32::MAX as u128 {
        return Err(ExecError::Usage(
            "timeout is too large for PostgreSQL statement_timeout".to_string(),
        ));
    }
    Ok(duration)
}

fn invalid_timeout(raw: &str) -> ExecError {
    ExecError::Usage(format!(
        "invalid timeout '{raw}' (expected 30s, 500ms, 2m, or a number of seconds)"
    ))
}

pub(crate) fn format_timeout(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis.is_multiple_of(1000) {
        format!("{}s", millis / 1000)
    } else {
        format!("{millis}ms")
    }
}

fn validate_connection(conn: &str) -> Result<(), ExecError> {
    if conn.is_empty() {
        return Err(ExecError::Usage("connection URL is empty".to_string()));
    }
    if let Some((scheme, _)) = conn.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Ok(()),
            "mongodb" | "mongodb+srv" => Err(ExecError::Usage(
                "tsql exec supports PostgreSQL connections; open mongodb:// URLs in the TUI"
                    .to_string(),
            )),
            other => Err(ExecError::Usage(format!(
                "unsupported connection scheme '{other}'"
            ))),
        };
    }
    if conn.contains('=') {
        return Ok(());
    }
    Err(ExecError::Usage(
        "expected a postgres:// URL or a libpq keyword string".to_string(),
    ))
}

/// Refuse anything [`classify_postgres`] does not accept as a read.
///
/// `Unknown` is refused on purpose: the TUI treats it as requiring write
/// access (`QuerySafety::requires_write_access`).
fn read_only_refusal(sql: &str) -> Option<String> {
    let safety = classify_postgres(sql);
    if !safety.requires_write_access() {
        return None;
    }
    let kind = match safety {
        QuerySafety::Write => "write",
        QuerySafety::Destructive => "destructive",
        QuerySafety::Unknown => "not a known read",
        QuerySafety::ReadOnly => return None,
    };
    Some(format!(
        "read-only mode refused this statement ({kind}): {}",
        preview_sql(sql)
    ))
}

fn preview_sql(sql: &str) -> String {
    let one_line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 180;
    if one_line.chars().count() <= MAX_CHARS {
        one_line
    } else {
        let truncated: String = one_line.chars().take(MAX_CHARS).collect();
        format!("{truncated}…")
    }
}

fn load_sql(input: SqlInput) -> Result<String, ExecError> {
    match input {
        SqlInput::Text(sql) => nonempty(sql),
        SqlInput::Stdin => {
            let terminal = io::stdin().is_terminal();
            if terminal {
                return read_stdin_sql(true, "");
            }
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf).map_err(|error| {
                ExecError::Usage(format!("failed to read SQL from stdin: {error}"))
            })?;
            read_stdin_sql(false, &buf)
        }
    }
}

fn read_stdin_sql(is_terminal: bool, stdin: &str) -> Result<String, ExecError> {
    if is_terminal {
        return Err(ExecError::Usage(
            "no SQL provided; pass -e/--execute or pipe SQL on stdin".to_string(),
        ));
    }
    nonempty(stdin.to_string())
}

fn nonempty(sql: String) -> Result<String, ExecError> {
    if sql.trim().is_empty() {
        Err(ExecError::Usage(
            "SQL is empty; pass -e/--execute or pipe a statement on stdin".to_string(),
        ))
    } else {
        Ok(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn parsed(args: &[&str]) -> ParsedRun {
        match parse_args(&argv(args), None).unwrap() {
            ExecCommand::Run(parsed) => parsed,
            ExecCommand::Help => panic!("expected a run command"),
        }
    }

    #[test]
    fn parses_the_agent_command_shape() {
        let parsed = parsed(&[
            "tsql",
            "exec",
            "postgres://localhost/app",
            "--read-only",
            "--timeout",
            "30s",
            "--format",
            "json",
            "-e",
            "SELECT id FROM users",
        ]);
        assert_eq!(parsed.url, "postgres://localhost/app");
        assert_eq!(
            parsed.sql,
            SqlInput::Text("SELECT id FROM users".to_string())
        );
        assert!(parsed.read_only);
        assert_eq!(parsed.timeout, Some(Duration::from_secs(30)));
        assert_eq!(parsed.format, OutputFormat::Json);
    }

    #[test]
    fn options_may_precede_the_url_and_use_equals_form() {
        let parsed = parsed(&[
            "tsql",
            "exec",
            "--format=csv",
            "--timeout=500ms",
            "--execute=SELECT 1",
            "host=localhost dbname=app",
        ]);
        assert_eq!(parsed.url, "host=localhost dbname=app");
        assert_eq!(parsed.sql, SqlInput::Text("SELECT 1".to_string()));
        assert_eq!(parsed.timeout, Some(Duration::from_millis(500)));
        assert_eq!(parsed.format, OutputFormat::Csv);
        assert!(!parsed.read_only);
    }

    #[test]
    fn stdin_is_the_default_sql_source_and_database_url_fills_the_connection() {
        let command =
            parse_args(&argv(&["tsql", "exec", "--table"]), Some(" postgres://db ")).unwrap();
        match command {
            ExecCommand::Run(parsed) => {
                assert_eq!(parsed.url, "postgres://db");
                assert_eq!(parsed.sql, SqlInput::Stdin);
                assert_eq!(parsed.format, OutputFormat::Table);
            }
            ExecCommand::Help => panic!("expected a run command"),
        }
    }

    #[test]
    fn help_wins_over_a_partial_command() {
        let command = parse_args(&argv(&["tsql", "exec", "--help"]), None).unwrap();
        assert!(matches!(command, ExecCommand::Help));
    }

    #[test]
    fn rejects_unknown_flags_missing_values_and_a_second_positional() {
        let error = parse_args(&argv(&["tsql", "exec", "--limit", "1"]), None).unwrap_err();
        assert_eq!(error.exit_code(), 1);
        assert!(error.to_string().contains("unknown option"));

        let error = parse_args(&argv(&["tsql", "exec", "-e"]), None).unwrap_err();
        assert!(error.to_string().contains("--execute requires a value"));

        let error = parse_args(
            &argv(&[
                "tsql",
                "exec",
                "postgres://a",
                "postgres://b",
                "-e",
                "SELECT 1",
            ]),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unexpected argument"));
    }

    #[test]
    fn parses_timeout_units_and_rejects_zero_or_garbage() {
        assert_eq!(parse_timeout("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_timeout("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_timeout("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_timeout("500ms").unwrap(), Duration::from_millis(500));
        assert!(parse_timeout("0s").is_err());
        assert!(parse_timeout("soon").is_err());
        assert!(parse_timeout("1.5s").is_err());
        assert!(parse_timeout("999999999m").is_err());
    }

    #[test]
    fn read_only_refusal_reuses_the_tui_classifier() {
        assert!(read_only_refusal("SELECT 1").is_none());
        assert!(read_only_refusal("WITH value AS (SELECT 1) SELECT * FROM value").is_none());
        assert!(read_only_refusal("SELECT 'DROP TABLE users'").is_none());
        assert!(read_only_refusal("EXPLAIN SELECT 1").is_none());
        assert!(read_only_refusal("SELECT sneak_delete()").is_none());

        let delete = read_only_refusal("DELETE FROM users").unwrap();
        assert!(delete.contains("destructive"), "{delete}");
        assert!(delete.contains("DELETE FROM users"), "{delete}");

        let update = read_only_refusal("UPDATE users SET active = true WHERE id = 1").unwrap();
        assert!(update.contains("write"), "{update}");

        let block = read_only_refusal("DO $$ BEGIN DELETE FROM users; END $$").unwrap();
        assert!(block.contains("not a known read"), "{block}");

        let multi = read_only_refusal("SELECT 1; SELECT 2").unwrap();
        assert!(multi.contains("not a known read"), "{multi}");

        let explain = read_only_refusal("EXPLAIN ANALYZE DELETE FROM users WHERE id = 1").unwrap();
        assert!(explain.contains("write"), "{explain}");
    }

    #[test]
    fn refusal_does_not_connect() {
        let error = tokio_test_refusal();
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("refused"));
    }

    fn tokio_test_refusal() -> ExecError {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(run_query(ExecRequest {
                url: "postgres://127.0.0.1:1/definitely-closed".to_string(),
                sql: "DELETE FROM agents".to_string(),
                read_only: true,
                timeout: Some(Duration::from_secs(30)),
                format: OutputFormat::Json,
            }))
            .unwrap_err()
    }

    #[test]
    fn cli_refusal_exits_2_without_printing_a_result() {
        let code = run(&argv(&[
            "tsql",
            "exec",
            "postgres://127.0.0.1:1/definitely-closed",
            "--read-only",
            "--format",
            "json",
            "-e",
            "DELETE FROM agents",
        ]));
        assert_eq!(code, 2);
    }

    #[test]
    fn stdin_rules_and_empty_sql() {
        let error = read_stdin_sql(true, "SELECT 1").unwrap_err();
        assert!(error.to_string().contains("pipe SQL on stdin"));
        assert_eq!(read_stdin_sql(false, "SELECT 1\n").unwrap(), "SELECT 1\n");
        assert!(read_stdin_sql(false, "  \n").is_err());
        assert!(nonempty("   ".to_string()).is_err());
    }

    #[test]
    fn mongodb_urls_are_rejected_before_connect() {
        let error = validate_connection("mongodb://localhost/app").unwrap_err();
        assert!(error.to_string().contains("PostgreSQL"));
        assert!(validate_connection("postgres://localhost/app").is_ok());
        assert!(validate_connection("host=localhost dbname=app").is_ok());
    }

    #[test]
    fn help_text_documents_the_exit_codes() {
        assert!(HELP.contains("Exit codes:"));
        assert!(HELP.contains("--read-only"));
        assert!(HELP.contains("--timeout"));
    }
}
