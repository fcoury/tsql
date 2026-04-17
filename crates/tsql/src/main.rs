use std::env;
use std::io::{self, Stdout, Write};

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use tsql::app::App;
use tsql::config;
use tsql::session::load_session;
use tsql::ui::GridModel;

/// Percent-encoding set for URL components (userinfo, path segments, query values) per RFC 3986.
/// Encodes reserved and unsafe characters while preserving unreserved chars (-, ., _, ~).
const URL_COMPONENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'+')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'|');

fn print_version() {
    println!("tsql {}", env!("CARGO_PKG_VERSION"));
}

/// Result of building a connection URL from libpq environment variables.
struct LibpqEnvResult {
    /// The connection URL, if successfully built.
    url: Option<String>,
    /// Warning message to display to the user, if any.
    warning: Option<String>,
}

/// Build a PostgreSQL connection URL from libpq-compatible environment variables.
///
/// Supports: PGHOST, PGPORT, PGDATABASE, PGUSER, PGPASSWORD, PGSSLMODE
///
/// Returns a result containing the URL (if buildable) and any warning message.
fn build_url_from_libpq_env() -> LibpqEnvResult {
    let host = env::var("PGHOST").ok();
    let port = env::var("PGPORT").ok();
    let database = env::var("PGDATABASE").ok();
    let user = env::var("PGUSER").ok();
    let password = env::var("PGPASSWORD").ok();
    let sslmode = env::var("PGSSLMODE").ok();

    // Need at least one of host or database to build a connection URL
    if host.is_none() && database.is_none() {
        return LibpqEnvResult {
            url: None,
            warning: None,
        };
    }

    // Validate port is numeric if provided
    if let Some(ref p) = port {
        if p.parse::<u16>().is_err() {
            return LibpqEnvResult {
                url: None,
                warning: Some(format!(
                    "PGPORT '{}' is not a valid port number, ignoring libpq env vars",
                    p
                )),
            };
        }
    }

    let host = host.unwrap_or_else(|| "localhost".to_string());
    let port = port.unwrap_or_else(|| "5432".to_string());

    // Build the URL
    let mut url = String::from("postgres://");

    // Add user and password if present
    if let Some(ref u) = user {
        // URL-encode special characters in username
        url.push_str(&utf8_percent_encode(u, URL_COMPONENT_ENCODE_SET).to_string());
        if let Some(ref p) = password {
            url.push(':');
            // URL-encode special characters in password
            url.push_str(&utf8_percent_encode(p, URL_COMPONENT_ENCODE_SET).to_string());
        }
        url.push('@');
    }

    // Add host and port (wrap IPv6 addresses in brackets)
    if host.contains(':') {
        url.push('[');
        url.push_str(&host);
        url.push(']');
    } else {
        url.push_str(&host);
    }
    url.push(':');
    url.push_str(&port);

    // Add database if present (percent-encode to handle special characters)
    if let Some(ref db) = database {
        url.push('/');
        url.push_str(&utf8_percent_encode(db, URL_COMPONENT_ENCODE_SET).to_string());
    }

    // Add sslmode if present (percent-encode to handle non-standard values)
    if let Some(ref ssl) = sslmode {
        url.push_str("?sslmode=");
        url.push_str(&utf8_percent_encode(ssl, URL_COMPONENT_ENCODE_SET).to_string());
    }

    LibpqEnvResult {
        url: Some(url),
        warning: None,
    }
}

fn print_usage() {
    eprintln!("tsql - A modern SQL and MongoDB CLI");
    eprintln!();
    eprintln!("Usage: tsql [OPTIONS] [CONNECTION_URL]");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  [CONNECTION_URL]  Connection URL");
    eprintln!("                    (e.g., postgres://user:pass@host:5432/dbname)");
    eprintln!("                    (or mongodb://user:pass@host:27017/dbname)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -h, --help        Print this help message");
    eprintln!("  -V, --version     Print version information");
    eprintln!("      --debug-keys  Print detected key/mouse events (for troubleshooting)");
    eprintln!("      --mouse       (with --debug-keys) Also print mouse events");
    eprintln!();
    eprintln!("Environment Variables:");
    eprintln!("  DATABASE_URL      Default connection URL if not provided as argument");
    eprintln!();
    eprintln!("  libpq-compatible variables (used if DATABASE_URL is not set):");
    eprintln!("    PGHOST          PostgreSQL server hostname (default: localhost)");
    eprintln!("    PGPORT          PostgreSQL server port (default: 5432)");
    eprintln!("    PGDATABASE      Database name");
    eprintln!("    PGUSER          Username for authentication");
    eprintln!("    PGPASSWORD      Password for authentication");
    eprintln!("    PGSSLMODE       SSL mode (disable, prefer, require, verify-ca, verify-full)");
    eprintln!();
    eprintln!("Configuration:");
    if let Some(path) = config::config_path() {
        eprintln!("  Config file: {}", path.display());
    }
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  tsql postgres://localhost/mydb");
    eprintln!("  tsql mongodb://localhost:27017/mydb");
    eprintln!("  DATABASE_URL=postgres://localhost/mydb tsql");
    eprintln!("  tsql --debug-keys");
    eprintln!("  tsql --debug-keys --mouse");
}

fn onepassword_cli_available() -> bool {
    std::process::Command::new("op")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn main() -> Result<()> {
    // Parse command-line arguments
    let args: Vec<String> = env::args().collect();

    // Check for help flag
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }

    // Check for version flag
    if args.iter().any(|a| a == "-V" || a == "--version") {
        print_version();
        return Ok(());
    }

    // Key debug mode (helps identify what the terminal actually sends)
    let debug_keys_mode = args
        .iter()
        .any(|a| a == "--debug-keys" || a == "--debug-keys-mouse")
        || args.get(1).is_some_and(|a| a == "debug-keys");
    if debug_keys_mode {
        let debug_mouse = args
            .iter()
            .any(|a| a == "--debug-keys-mouse" || a == "--mouse");
        return run_debug_keys(debug_mouse);
    }

    // --safe-mode: skip session auto-reconnect, update checks, and the
    // 1Password CLI probe. Exists so a user with a broken keychain /
    // unreachable network can always recover the app with
    // `tsql --safe-mode`.
    let safe_mode = args
        .iter()
        .any(|a| a == "--safe-mode" || a == "--no-auto-connect");

    let mut startup_warnings: Vec<String> = Vec::new();

    if let Err(err) = config::migrate_legacy_config_dir_on_startup() {
        startup_warnings.push(format!(
            "Failed to migrate legacy config directory to ~/.tsql: {}",
            err
        ));
    }

    // Load configuration from ~/.tsql/config.toml
    let cfg = config::load_config().unwrap_or_else(|e| {
        startup_warnings.push(format!("Failed to load config: {}", e));
        config::Config::default()
    });
    let onepassword_enabled = cfg.connection.enable_onepassword;

    // Load session state if persistence is enabled
    let session = if cfg.editor.persist_session {
        load_session().unwrap_or_else(|e| {
            startup_warnings.push(format!("Failed to load session: {}", e));
            Default::default()
        })
    } else {
        Default::default()
    };

    // Connection string priority: CLI arg > DATABASE_URL env var > libpq env vars > config file
    // Pick the first positional argument (one that doesn't start with
    // `-`) as the connection string, regardless of where in the argv
    // it sits. Previously we only looked at `args[1]`, which broke
    // option-first invocations like `tsql --safe-mode postgres://…`.
    let positional_url = args.iter().skip(1).find(|a| !a.starts_with('-'));
    let (conn_str, libpq_warning) = if let Some(url) = positional_url {
        (Some(url.clone()), None)
    } else if let Ok(url) = env::var("DATABASE_URL") {
        // Fall back to DATABASE_URL environment variable
        (Some(url), None)
    } else {
        // Then try libpq-compatible env vars (PGHOST, PGPORT, PGDATABASE, etc.)
        let result = build_url_from_libpq_env();
        (result.url, result.warning)
    };

    let rt = Runtime::new().context("failed to initialize tokio runtime")?;
    let (db_events_tx, db_events_rx) = mpsc::unbounded_channel();

    let mut terminal =
        init_terminal().context("failed to initialize terminal; are you running in a real TTY?")?;

    let mut app = App::with_config(
        GridModel::empty(),
        rt.handle().clone(),
        db_events_tx,
        db_events_rx,
        conn_str.clone(),
        cfg,
    );
    app.set_safe_mode(safe_mode);

    // Display startup warnings through the status line instead of
    // `eprintln!` — anything printed to stderr after we enter raw mode /
    // alt-screen would clobber the TUI on macOS Terminal.
    if let Some(warning) = libpq_warning {
        startup_warnings.push(warning);
    }
    // The 1Password CLI probe shells out to `op --version`; run it on a
    // background thread with a hard timeout so a wedged `op` never stalls
    // startup, and gate it behind safe_mode.
    if !safe_mode {
        if let Some(warning) = onepassword_startup_warning_nonblocking(onepassword_enabled) {
            startup_warnings.push(warning);
        }
    }
    if safe_mode {
        startup_warnings.push(
            "Running in safe mode: session auto-reconnect + update checks disabled".to_string(),
        );
    }
    if !startup_warnings.is_empty() {
        app.last_status = Some(startup_warnings.join(" | "));
    }

    // Apply session state (editor content, sidebar visibility, pending schema expanded)
    let session_connection = app.apply_session_state(session);

    // Queue the session auto-reconnect if one is configured. `App::run`
    // dispatches it AFTER the first draw — so the user never sees an empty
    // alt-screen while keychain / 1Password resolves.
    if conn_str.is_none() && !safe_mode {
        if let Some(conn_name) = session_connection {
            app.set_pending_startup_reconnect(Some(conn_name));
        } else {
            app.open_connection_picker();
        }
    } else if conn_str.is_none() && safe_mode {
        app.open_connection_picker();
    }

    let res = app.run(&mut terminal);

    restore_terminal(terminal)?;

    res
}

/// Non-blocking wrapper around `onepassword_startup_warning`. Returns a
/// warning on a ~750ms timeout if `op --version` never returns — that way
/// a broken `op` install can never wedge the app at startup.
fn onepassword_startup_warning_nonblocking(onepassword_enabled: bool) -> Option<String> {
    if !onepassword_enabled {
        return None;
    }
    if !cfg!(unix) {
        return Some(
            "1Password support is currently available only on Unix-like systems (Linux/macOS). \
             Disable `connection.enable_onepassword` on this platform."
                .to_string(),
        );
    }

    use std::sync::mpsc;
    use std::time::Duration;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let ok = onepassword_cli_available();
        let _ = tx.send(ok);
    });
    match rx.recv_timeout(Duration::from_millis(750)) {
        Ok(true) => None,
        Ok(false) => Some(
            "1Password support is enabled, but `op` was not found on PATH. Install/sign in via \
             1Password CLI or disable `connection.enable_onepassword`."
                .to_string(),
        ),
        Err(_) => Some(
            "1Password CLI probe timed out. Credentials may be slow to resolve — \
             use `tsql --safe-mode` to bypass."
                .to_string(),
        ),
    }
}

fn run_debug_keys(with_mouse: bool) -> Result<()> {
    struct DebugTerminalGuard {
        stdout: Stdout,
        with_mouse: bool,
    }

    impl DebugTerminalGuard {
        fn new(with_mouse: bool) -> Result<Self> {
            enable_raw_mode()?;
            let mut stdout = io::stdout();
            if with_mouse {
                execute!(stdout, EnableMouseCapture)?;
            }
            Ok(Self { stdout, with_mouse })
        }

        fn println(&mut self, line: &str) -> Result<()> {
            write!(self.stdout, "\r\n{line}")?;
            self.stdout.flush()?;
            Ok(())
        }
    }

    impl Drop for DebugTerminalGuard {
        fn drop(&mut self) {
            if self.with_mouse {
                let _ = execute!(self.stdout, DisableMouseCapture);
            }
            let _ = disable_raw_mode();
            let _ = self.stdout.flush();
        }
    }

    fn describe_key(key: KeyEvent) -> String {
        let mut parts = Vec::new();
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            parts.push("Ctrl");
        }
        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) {
            parts.push("Alt");
        }
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT)
        {
            parts.push("Shift");
        }

        let key_name = match key.code {
            KeyCode::Char(c) => format!("{c:?}"),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "BackTab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };

        if parts.is_empty() {
            key_name
        } else {
            format!("{}+{key_name}", parts.join("+"))
        }
    }

    let mut guard = DebugTerminalGuard::new(with_mouse)?;
    let mouse_msg = if with_mouse { "on" } else { "off" };
    guard.println(&format!(
        "tsql --debug-keys (mouse: {mouse_msg}) (press Esc or Ctrl+C to exit)"
    ))?;

    loop {
        let ev = event::read()?;
        match ev {
            Event::Key(key) => {
                let desc = describe_key(key);
                guard.println(&format!("Key: {desc}    raw={key:?}"))?;

                let is_ctrl_c = key.code == KeyCode::Char('c')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL);
                if key.code == KeyCode::Esc || is_ctrl_c {
                    break;
                }
            }
            Event::Mouse(mouse) => {
                if with_mouse {
                    guard.println(&format!("Mouse: {mouse:?}"))?;
                }
            }
            Event::Resize(w, h) => {
                guard.println(&format!("Resize: {w}x{h}"))?;
            }
            Event::Paste(text) => {
                guard.println(&format!("Paste: {:?} ({} bytes)", text, text.len()))?;
            }
            other => {
                guard.println(&format!("Event: {other:?}"))?;
            }
        }
    }

    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Helper to clear all libpq env vars before each test
    fn clear_libpq_env_vars() {
        for var in [
            "PGHOST",
            "PGPORT",
            "PGDATABASE",
            "PGUSER",
            "PGPASSWORD",
            "PGSSLMODE",
        ] {
            env::remove_var(var);
        }
    }

    #[test]
    #[serial]
    fn test_no_env_vars_returns_none() {
        clear_libpq_env_vars();
        let result = build_url_from_libpq_env();
        assert!(result.url.is_none());
        assert!(result.warning.is_none());
    }

    #[test]
    #[serial]
    fn test_pghost_only() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "db.example.com");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(url, "postgres://db.example.com:5432");
    }

    #[test]
    #[serial]
    fn test_pgdatabase_only() {
        clear_libpq_env_vars();
        env::set_var("PGDATABASE", "mydb");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(url, "postgres://localhost:5432/mydb");
    }

    #[test]
    #[serial]
    fn test_all_vars() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "db.example.com");
        env::set_var("PGPORT", "5433");
        env::set_var("PGDATABASE", "testdb");
        env::set_var("PGUSER", "testuser");
        env::set_var("PGPASSWORD", "secret");
        env::set_var("PGSSLMODE", "require");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(
            url,
            "postgres://testuser:secret@db.example.com:5433/testdb?sslmode=require"
        );
    }

    #[test]
    #[serial]
    fn test_user_without_password() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGUSER", "admin");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(url, "postgres://admin@localhost:5432");
    }

    #[test]
    #[serial]
    fn test_special_characters_in_password() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGUSER", "user");
        env::set_var("PGPASSWORD", "p@ss:word/test");

        let url = build_url_from_libpq_env().url.unwrap();
        // Special characters should be percent-encoded
        assert!(url.contains("p%40ss%3Aword%2Ftest"));
    }

    #[test]
    #[serial]
    fn test_special_characters_in_username() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGUSER", "user@domain");

        let url = build_url_from_libpq_env().url.unwrap();
        // @ in username should be percent-encoded
        assert!(url.contains("user%40domain@"));
    }

    #[test]
    #[serial]
    fn test_custom_port() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGPORT", "15432");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(url, "postgres://localhost:15432");
    }

    #[test]
    #[serial]
    fn test_sslmode_only_with_host() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGSSLMODE", "verify-full");

        let url = build_url_from_libpq_env().url.unwrap();
        assert_eq!(url, "postgres://localhost:5432?sslmode=verify-full");
    }

    #[test]
    #[serial]
    fn test_special_characters_in_database() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGDATABASE", "my db/test?foo");

        let url = build_url_from_libpq_env().url.unwrap();
        // Space, slash, and question mark should be percent-encoded
        assert!(url.contains("/my%20db%2Ftest%3Ffoo"));
    }

    #[test]
    #[serial]
    fn test_unreserved_chars_not_encoded() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGUSER", "user-name_test.org~");
        env::set_var("PGDATABASE", "my-db_name.test~");

        let url = build_url_from_libpq_env().url.unwrap();
        // Unreserved characters (-, _, ., ~) should NOT be percent-encoded
        assert!(url.contains("user-name_test.org~@"));
        assert!(url.contains("/my-db_name.test~"));
    }

    #[test]
    #[serial]
    fn test_ipv6_localhost() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "::1");
        env::set_var("PGDATABASE", "testdb");

        let url = build_url_from_libpq_env().url.unwrap();
        // IPv6 addresses should be wrapped in brackets
        assert_eq!(url, "postgres://[::1]:5432/testdb");
    }

    #[test]
    #[serial]
    fn test_ipv6_full_address() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "2001:db8::1");
        env::set_var("PGPORT", "5433");

        let url = build_url_from_libpq_env().url.unwrap();
        // IPv6 addresses should be wrapped in brackets
        assert_eq!(url, "postgres://[2001:db8::1]:5433");
    }

    #[test]
    #[serial]
    fn test_sslmode_with_special_chars() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGSSLMODE", "require&foo=bar");

        let url = build_url_from_libpq_env().url.unwrap();
        // Special characters in sslmode should be percent-encoded
        assert!(url.contains("?sslmode=require%26foo%3Dbar"));
    }

    #[test]
    #[serial]
    fn test_invalid_pgport_returns_warning() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGPORT", "not_a_number");

        let result = build_url_from_libpq_env();
        // Invalid port should return no URL but a warning
        assert!(result.url.is_none());
        assert!(result.warning.is_some());
        assert!(result.warning.unwrap().contains("not_a_number"));
    }

    #[test]
    #[serial]
    fn test_pgport_out_of_range_returns_warning() {
        clear_libpq_env_vars();
        env::set_var("PGHOST", "localhost");
        env::set_var("PGPORT", "99999"); // u16 max is 65535

        let result = build_url_from_libpq_env();
        // Out of range port should return no URL but a warning
        assert!(result.url.is_none());
        assert!(result.warning.is_some());
        assert!(result.warning.unwrap().contains("99999"));
    }
}
