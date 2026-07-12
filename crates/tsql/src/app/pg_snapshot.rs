//! PostgreSQL session-local TEMP-table refinement provider.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio_postgres::{types::Kind, SimpleQueryMessage};

use super::app::{QueryResult, SharedClient};
use super::execution::ExecutionContext;
use super::refinement::{
    RefinementProviderId, ResultVersion, RetainedResult, RetainedResultHandle, SqlSourceMap,
};
use super::sql_lexer;
use crate::util::{format_pg_error, format_pg_error_with_position};

/// Backend resource tracked by the PostgreSQL snapshot manager.
#[derive(Clone, Debug)]
pub struct PgTempSnapshot {
    pub physical_name: String,
    pub public_columns: Vec<String>,
    pub relation_oid: u32,
    pub connection_generation: u64,
    pub backend_pid: i32,
    pub row_count: usize,
    pub byte_size: u64,
}

/// Checks that a retained relation still belongs to the expected backend session.
pub(crate) async fn validate_snapshot_identity(
    client: &tokio_postgres::Client,
    snapshot: &PgTempSnapshot,
) -> Result<(), String> {
    let qualified = format!("pg_temp.{}", quote_identifier(&snapshot.physical_name));
    let identity = client
        .query_one(
            "SELECT pg_backend_pid(), to_regclass($1)::oid",
            &[&qualified],
        )
        .await
        .map_err(|error| pg_error("failed to validate source snapshot", &error))?;
    let backend_pid: i32 = identity.get(0);
    let relation_oid: Option<u32> = identity.get(1);
    if backend_pid != snapshot.backend_pid || relation_oid != Some(snapshot.relation_oid) {
        return Err("source snapshot is no longer available on this session".to_string());
    }
    Ok(())
}

/// Drops a retained relation only while its backend PID and relation OID still match.
pub(crate) async fn drop_if_identity_matches(
    client: &tokio_postgres::Client,
    snapshot: &PgTempSnapshot,
) -> Result<(), String> {
    let qualified = format!("pg_temp.{}", quote_identifier(&snapshot.physical_name));
    let identity = client
        .query_one(
            "SELECT pg_backend_pid(), to_regclass($1)::oid",
            &[&qualified],
        )
        .await
        .map_err(|error| pg_error("failed to inspect snapshot before cleanup", &error))?;
    let backend_pid: i32 = identity.get(0);
    let relation_oid: Option<u32> = identity.get(1);
    if backend_pid != snapshot.backend_pid || relation_oid != Some(snapshot.relation_oid) {
        return Ok(());
    }
    client
        .batch_execute(&format!("DROP TABLE IF EXISTS {qualified}"))
        .await
        .map_err(|error| pg_error("failed to discard snapshot", &error))
}

pub(crate) struct PgSnapshotResult {
    pub query_result: QueryResult,
    pub retained: Option<RetainedResult>,
    pub snapshot: Option<PgTempSnapshot>,
}

pub(crate) struct PgSnapshotRequest {
    pub context: ExecutionContext,
    pub query: String,
    pub handle: RetainedResultHandle,
    pub max_rows: usize,
    pub display_max_rows: usize,
    pub max_bytes: u64,
    pub timeout_secs: u32,
    pub source_snapshots: Vec<PgTempSnapshot>,
    pub cancelled: Option<Arc<AtomicBool>>,
    pub source_map: Option<SqlSourceMap>,
}

/// Result of a side-effect-free snapshot eligibility check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotEligibility {
    Eligible(String),
    Ineligible(String),
    Invalid(String),
}

pub(crate) async fn execute(
    client: SharedClient,
    request: PgSnapshotRequest,
) -> Result<PgSnapshotResult, String> {
    let started = Instant::now();
    let mut source = snapshot_source(&request.query)?;
    let leading = &request.query[..request.query.len() - request.query.trim_start().len()];
    source.insert_str(0, leading);
    let mut guard = client.lock().await;
    if request
        .cancelled
        .as_ref()
        .is_some_and(|cancelled| cancelled.load(Ordering::Acquire))
    {
        return Err("query cancelled before snapshot execution".to_string());
    }
    let transaction = guard
        .transaction()
        .await
        .map_err(|error| pg_error("failed to start snapshot transaction", &error))?;

    if request.timeout_secs > 0 {
        transaction
            .batch_execute(&format!(
                "SET LOCAL statement_timeout = {}",
                request.timeout_secs.saturating_mul(1_000)
            ))
            .await
            .map_err(|error| pg_error("failed to set snapshot timeout", &error))?;
    }

    for expected in &request.source_snapshots {
        let qualified = format!("pg_temp.{}", quote_identifier(&expected.physical_name));
        let identity = transaction
            .query_one(
                "SELECT pg_backend_pid(), to_regclass($1)::oid",
                &[&qualified],
            )
            .await
            .map_err(|error| pg_error("failed to validate source snapshot", &error))?;
        let backend_pid: i32 = identity.get(0);
        let relation_oid: Option<u32> = identity.get(1);
        if backend_pid != expected.backend_pid || relation_oid != Some(expected.relation_oid) {
            return Err("source snapshot is no longer available on this session".to_string());
        }
    }

    let capabilities = transaction
        .query_one(
            "SELECT pg_is_in_recovery(), current_setting('transaction_read_only') = 'on', \
             has_database_privilege(current_database(), 'TEMP')",
            &[],
        )
        .await
        .map_err(|error| pg_error("failed to inspect snapshot capability", &error))?;
    let in_recovery: bool = capabilities.get(0);
    let read_only: bool = capabilities.get(1);
    let has_temp_privilege: bool = capabilities.get(2);
    if in_recovery || read_only || !has_temp_privilege {
        return execute_preview_fallback(
            transaction,
            &source,
            &request,
            Vec::new(),
            started,
            "session cannot create a PostgreSQL TEMP snapshot",
        )
        .await;
    }

    let statement = transaction.prepare(&source).await.map_err(|error| {
        pg_error_mapped(
            "snapshot preflight failed",
            &error,
            request.source_map.as_ref(),
            "",
        )
    })?;
    if statement.columns().is_empty() {
        return execute_preview_fallback(
            transaction,
            &source,
            &request,
            Vec::new(),
            started,
            "statement has no row result to retain",
        )
        .await;
    }
    if !statement.params().is_empty() {
        return Err("statement contains unbound PostgreSQL parameters".to_string());
    }
    if statement
        .columns()
        .iter()
        .any(|column| contains_pseudo_type(column.type_()))
    {
        let col_types = statement
            .columns()
            .iter()
            .map(|column| column.type_().name().to_string())
            .collect();
        return execute_preview_fallback(
            transaction,
            &source,
            &request,
            col_types,
            started,
            "result contains a PostgreSQL pseudo-type",
        )
        .await;
    }

    let physical_name = format!(
        "__tsql_nb_{}_{:016x}",
        uuid::Uuid::new_v4().simple(),
        request.context.id.0
    );
    let ordinal_name = format!("__tsql_row_ordinal_{:016x}", request.context.id.0);
    let output_names = normalize_column_names(
        statement.columns().iter().map(|column| column.name()),
        &ordinal_name,
    );
    let mut table_columns = Vec::with_capacity(output_names.len() + 1);
    table_columns.push(quote_identifier(&ordinal_name));
    table_columns.extend(output_names.iter().map(|name| quote_identifier(name)));
    let create_prefix = format!(
        "CREATE TEMP TABLE {} ({}) ON COMMIT PRESERVE ROWS AS \
         SELECT row_number() OVER (), __tsql_source.* FROM (\n",
        quote_identifier(&physical_name),
        table_columns.join(", ")
    );
    let create_sql = format!(
        "{create_prefix}{source}\n) AS __tsql_source LIMIT {}",
        request
            .max_rows
            .saturating_add(1)
            .max(request.display_max_rows)
    );
    transaction
        .batch_execute(&create_sql)
        .await
        .map_err(|error| {
            pg_error_mapped(
                "snapshot materialization failed",
                &error,
                request.source_map.as_ref(),
                &create_prefix,
            )
        })?;

    let qualified_name = format!("pg_temp.{}", quote_identifier(&physical_name));
    let count_sql = format!("SELECT count(*)::bigint FROM {qualified_name}");
    let row_count_i64: i64 = transaction
        .query_one(&count_sql, &[])
        .await
        .map_err(|error| pg_error("failed to inspect snapshot rows", &error))?
        .get(0);
    let row_count = usize::try_from(row_count_i64)
        .map_err(|_| "snapshot returned an invalid row count".to_string())?;
    let size_sql = format!("SELECT pg_total_relation_size('{qualified_name}'::regclass)");
    let byte_size_i64: i64 = transaction
        .query_one(&size_sql, &[])
        .await
        .map_err(|error| pg_error("failed to inspect snapshot size", &error))?
        .get(0);
    let byte_size = u64::try_from(byte_size_i64)
        .map_err(|_| "snapshot returned an invalid relation size".to_string())?;
    let display_max_rows =
        notebook_preview_row_limit(row_count, byte_size, request.display_max_rows);

    let select_columns = output_names
        .iter()
        .map(|name| quote_identifier(name))
        .collect::<Vec<_>>()
        .join(", ");
    let display_sql = format!(
        "SELECT {select_columns} FROM {qualified_name} \
         ORDER BY {} LIMIT {}",
        quote_identifier(&ordinal_name),
        display_max_rows
    );
    let messages = transaction
        .simple_query(&display_sql)
        .await
        .map_err(|error| pg_error("failed to read snapshot result", &error))?;
    let (mut headers, rows, null_cells) = display_rows(messages, display_max_rows);
    if headers.is_empty() {
        headers.clone_from(&output_names);
    }
    let complete = row_count <= request.max_rows && byte_size <= request.max_bytes;

    let (retained, snapshot) = if complete {
        let identity_sql = format!(
            "SELECT pg_backend_pid(), '{}'::regclass::oid",
            qualified_name.replace('\'', "''")
        );
        let identity = transaction
            .query_one(&identity_sql, &[])
            .await
            .map_err(|error| pg_error("failed to identify snapshot", &error))?;
        let backend_pid: i32 = identity.get(0);
        let relation_oid: u32 = identity.get(1);
        let version = ResultVersion {
            source_cell: match request.context.target {
                super::execution::ExecutionTarget::Notebook(cell) => cell,
                super::execution::ExecutionTarget::Classic => {
                    return Err("classic execution cannot publish notebook snapshot".to_string())
                }
            },
            source_execution: request.context.id,
            source_revision: request.context.source_revision,
        };
        (
            Some(RetainedResult {
                provider: RefinementProviderId::PostgresTemp,
                handle: request.handle,
                version,
                rows: row_count,
                retained_bytes: byte_size,
                connection_generation: request.context.connection_generation,
            }),
            Some(PgTempSnapshot {
                physical_name: physical_name.clone(),
                public_columns: output_names.clone(),
                relation_oid,
                connection_generation: request.context.connection_generation,
                backend_pid,
                row_count,
                byte_size,
            }),
        )
    } else {
        transaction
            .batch_execute(&format!("DROP TABLE {qualified_name}"))
            .await
            .map_err(|error| pg_error("failed to discard oversized snapshot", &error))?;
        (None, None)
    };

    transaction
        .commit()
        .await
        .map_err(|error| pg_error("failed to commit snapshot", &error))?;

    Ok(PgSnapshotResult {
        query_result: QueryResult {
            headers,
            rows,
            null_cells,
            command_tag: Some(format!("SELECT {row_count}")),
            truncated: !complete || row_count > display_max_rows,
            elapsed: started.elapsed(),
            source_table: None,
            primary_keys: Vec::new(),
            col_types: statement
                .columns()
                .iter()
                .map(|column| column.type_().name().to_string())
                .collect(),
        },
        retained,
        snapshot,
    })
}

fn pg_error(context: &str, error: &tokio_postgres::Error) -> String {
    format!("{context}: {}", format_pg_error(error))
}

fn pg_error_mapped(
    context: &str,
    error: &tokio_postgres::Error,
    source_map: Option<&SqlSourceMap>,
    generated_prefix: &str,
) -> String {
    format!(
        "{context}: {}",
        format_pg_error_with_position(error, |position| {
            source_map.and_then(|source_map| source_map.map_position(position, generated_prefix))
        })
    )
}

async fn execute_preview_fallback(
    transaction: tokio_postgres::Transaction<'_>,
    source: &str,
    request: &PgSnapshotRequest,
    col_types: Vec<String>,
    started: Instant,
    reason: &str,
) -> Result<PgSnapshotResult, String> {
    let cursor = format!("__tsql_nb_preview_{:016x}", request.context.id.0);
    let preview_prefix = format!(
        "DECLARE {} NO SCROLL CURSOR FOR\n",
        quote_identifier(&cursor)
    );
    transaction
        .batch_execute(&format!("{preview_prefix}{source}\n"))
        .await
        .map_err(|error| {
            pg_error_mapped(
                &format!("{reason}; preview failed"),
                &error,
                request.source_map.as_ref(),
                &preview_prefix,
            )
        })?;
    let messages = transaction
        .simple_query(&format!(
            "FETCH FORWARD {} FROM {}",
            request.display_max_rows.saturating_add(1),
            quote_identifier(&cursor)
        ))
        .await
        .map_err(|error| pg_error("failed to read notebook preview", &error))?;
    let (headers, mut rows, mut null_cells) =
        display_rows(messages, request.display_max_rows.saturating_add(1));
    let truncated = rows.len() > request.display_max_rows;
    rows.truncate(request.display_max_rows);
    null_cells.truncate(request.display_max_rows);
    transaction
        .rollback()
        .await
        .map_err(|error| pg_error("failed to close notebook preview", &error))?;

    Ok(PgSnapshotResult {
        query_result: QueryResult {
            headers,
            command_tag: Some(format!("SELECT {}", rows.len())),
            rows,
            null_cells,
            truncated,
            elapsed: started.elapsed(),
            source_table: None,
            primary_keys: Vec::new(),
            col_types,
        },
        retained: None,
        snapshot: None,
    })
}

fn snapshot_source(query: &str) -> Result<String, String> {
    match snapshot_eligibility(query) {
        SnapshotEligibility::Eligible(source) => Ok(source),
        SnapshotEligibility::Ineligible(reason) | SnapshotEligibility::Invalid(reason) => {
            Err(reason)
        }
    }
}

/// Classifies a single PostgreSQL statement without executing it.
pub(crate) fn snapshot_eligibility(query: &str) -> SnapshotEligibility {
    let source = match sql_lexer::single_statement(query) {
        Ok(source) => source,
        Err(error) => return SnapshotEligibility::Invalid(error),
    };
    let words = match sql_lexer::code_words(source, usize::MAX) {
        Ok(words) => words,
        Err(error) => return SnapshotEligibility::Invalid(error),
    };
    let Some(first) = words.first().map(String::as_str) else {
        return SnapshotEligibility::Invalid("empty SQL statement".to_string());
    };
    if !matches!(first, "SELECT" | "WITH" | "VALUES" | "TABLE") {
        return SnapshotEligibility::Ineligible(
            "statement is not eligible for a session snapshot".to_string(),
        );
    }
    if has_unbound_parameter(source) {
        return SnapshotEligibility::Ineligible(
            "statement contains unbound PostgreSQL parameters".to_string(),
        );
    }
    if has_locking_clause(&words) {
        return SnapshotEligibility::Ineligible(
            "locking queries are not eligible for a session snapshot".to_string(),
        );
    }

    // tree-sitter-sequel does not currently accept standalone VALUES or TABLE,
    // both of which are valid PostgreSQL row-producing statements.
    if matches!(first, "VALUES" | "TABLE") {
        return SnapshotEligibility::Eligible(source.to_string());
    }
    let parser_source = match parser_validation_source(source) {
        Ok(source) => source,
        Err(error) => return SnapshotEligibility::Invalid(error),
    };
    let mut parser = tree_sitter::Parser::new();
    if let Err(error) = parser.set_language(&tree_sitter_sequel::LANGUAGE.into()) {
        return SnapshotEligibility::Invalid(format!("failed to initialize SQL parser: {error}"));
    }
    let Some(tree) = parser.parse(&parser_source, None) else {
        return SnapshotEligibility::Invalid("failed to parse SQL statement".to_string());
    };
    let root = tree.root_node();
    if root.has_error() || root.named_child_count() != 1 {
        return SnapshotEligibility::Ineligible(
            "statement shape is not eligible for a session snapshot".to_string(),
        );
    }
    if tree_contains_kind(
        root,
        &[
            "insert",
            "update",
            "delete",
            "keyword_merge",
            "keyword_copy",
            "keyword_truncate",
        ],
    ) {
        return SnapshotEligibility::Ineligible(
            "statement contains a data-changing or transaction-control clause".to_string(),
        );
    }
    if tree_contains_kind(root, &["keyword_into"]) {
        return SnapshotEligibility::Ineligible(
            "SELECT INTO is not eligible for a session snapshot".to_string(),
        );
    }
    SnapshotEligibility::Eligible(source.to_string())
}

fn tree_contains_kind(root: tree_sitter::Node<'_>, kinds: &[&str]) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if kinds.contains(&node.kind()) {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    false
}

pub(crate) fn is_snapshot_candidate(query: &str) -> bool {
    matches!(
        snapshot_eligibility(query),
        SnapshotEligibility::Eligible(_)
    )
}

fn has_unbound_parameter(source: &str) -> bool {
    sql_lexer::scan(source).is_ok_and(|segments| {
        segments.into_iter().any(|segment| {
            if segment.kind != sql_lexer::SqlSegmentKind::Code {
                return false;
            }
            let code = &source[segment.range];
            code.char_indices().any(|(index, character)| {
                character == '$'
                    && code
                        .as_bytes()
                        .get(index + 1)
                        .is_some_and(u8::is_ascii_digit)
                    && code[..index]
                        .chars()
                        .next_back()
                        .is_none_or(|previous| !is_identifier_character(previous))
            })
        })
    })
}

fn parser_validation_source(source: &str) -> Result<String, String> {
    let masked = sql_lexer::mask_comments(source)?;
    let mut normalized = String::with_capacity(masked.len());
    // tree-sitter-sequel rejects PostgreSQL's valid identifier-internal `$` characters.
    for segment in sql_lexer::scan(&masked)? {
        let text = &masked[segment.range];
        if segment.kind != sql_lexer::SqlSegmentKind::Code {
            normalized.push_str(text);
            continue;
        }
        let mut previous = None;
        for character in text.chars() {
            normalized.push(
                if character == '$' && previous.is_some_and(is_identifier_character) {
                    '_'
                } else {
                    character
                },
            );
            previous = Some(character);
        }
    }
    Ok(normalized)
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphanumeric()
}

fn has_locking_clause(words: &[String]) -> bool {
    words.windows(2).any(|pair| {
        pair[0] == "FOR" && matches!(pair[1].as_str(), "UPDATE" | "SHARE" | "KEY" | "NO")
    })
}

fn contains_pseudo_type(type_: &tokio_postgres::types::Type) -> bool {
    match type_.kind() {
        Kind::Pseudo => true,
        Kind::Array(inner) | Kind::Range(inner) | Kind::Multirange(inner) | Kind::Domain(inner) => {
            contains_pseudo_type(inner)
        }
        Kind::Composite(fields) => fields
            .iter()
            .any(|field| contains_pseudo_type(field.type_())),
        _ => false,
    }
}

const POSTGRES_IDENTIFIER_BYTES: usize = 63;
const MAX_NOTEBOOK_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;

fn notebook_preview_row_limit(row_count: usize, byte_size: u64, requested: usize) -> usize {
    if row_count == 0 || byte_size <= MAX_NOTEBOOK_PREVIEW_BYTES {
        return requested;
    }
    let rows_within_budget = MAX_NOTEBOOK_PREVIEW_BYTES
        .saturating_mul(row_count as u64)
        .checked_div(byte_size)
        .unwrap_or(0)
        .max(1);
    requested.min(usize::try_from(rows_within_budget).unwrap_or(usize::MAX))
}

fn normalize_column_names<'a>(names: impl Iterator<Item = &'a str>, reserved: &str) -> Vec<String> {
    let mut used = std::collections::HashSet::from([reserved.to_string()]);
    names
        .enumerate()
        .map(|(index, name)| {
            let base = if name.is_empty() {
                format!("column_{}", index + 1)
            } else {
                name.to_string()
            };
            let mut suffix = 1usize;
            loop {
                let ending = if suffix == 1 {
                    String::new()
                } else {
                    format!("_{suffix}")
                };
                let byte_budget = POSTGRES_IDENTIFIER_BYTES.saturating_sub(ending.len());
                let mut end = base.len().min(byte_budget);
                while !base.is_char_boundary(end) {
                    end -= 1;
                }
                let candidate = format!("{}{ending}", &base[..end]);
                if used.insert(candidate.clone()) {
                    return candidate;
                }
                suffix += 1;
            }
        })
        .collect()
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn display_rows(
    messages: Vec<SimpleQueryMessage>,
    max_rows: usize,
) -> (Vec<String>, Vec<Vec<String>>, Vec<Vec<bool>>) {
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut null_cells = Vec::new();
    for message in messages {
        match message {
            SimpleQueryMessage::Row(row) => {
                if headers.is_empty() {
                    headers = row
                        .columns()
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect();
                }
                if rows.len() < max_rows {
                    let mut values = Vec::with_capacity(row.len());
                    let mut null_row = Vec::with_capacity(row.len());
                    for index in 0..row.len() {
                        let cell = row.get(index);
                        null_row.push(cell.is_none());
                        values.push(cell.unwrap_or("NULL").to_string());
                    }
                    rows.push(values);
                    null_cells.push(null_row);
                }
            }
            SimpleQueryMessage::RowDescription(columns) if headers.is_empty() => {
                headers = columns
                    .iter()
                    .map(|column| column.name().to_string())
                    .collect();
            }
            _ => {}
        }
    }
    (headers, rows, null_cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CellId, ExecutionId, ExecutionTarget};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn normalizes_empty_and_duplicate_columns() {
        assert_eq!(
            normalize_column_names(["id", "id", ""].into_iter(), "__ordinal"),
            ["id", "id_2", "column_3"]
        );

        let unicode = "á".repeat(31);
        let names = normalize_column_names(
            [unicode.as_str(), unicode.as_str(), unicode.as_str()].into_iter(),
            "__ordinal",
        );
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|name| name.len() <= 63));
        assert_eq!(
            names.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );

        let ascii = "x".repeat(63);
        let names = normalize_column_names(
            [ascii.as_str(), ascii.as_str(), "__ordinal"].into_iter(),
            "__ordinal",
        );
        assert_eq!(names[0], ascii);
        assert!(names[1].ends_with("_2"));
        assert_eq!(names[2], "__ordinal_2");
        assert!(names.iter().all(|name| name.len() <= 63));
    }

    #[test]
    fn limits_oversized_snapshot_preview_by_estimated_row_bytes() {
        assert_eq!(notebook_preview_row_limit(100, 1024, 50), 50);
        assert_eq!(notebook_preview_row_limit(100, 80 * 1024 * 1024, 100), 10);
        assert_eq!(notebook_preview_row_limit(1, 80 * 1024 * 1024, 100), 1);
        assert_eq!(notebook_preview_row_limit(0, 80 * 1024 * 1024, 100), 100);
    }

    #[test]
    fn rejects_non_row_and_multiple_statements() {
        assert!(snapshot_source("UPDATE users SET active = true").is_err());
        assert!(snapshot_source("SELECT 1; SELECT 2").is_err());
        assert_eq!(snapshot_source("SELECT ';';").unwrap(), "SELECT ';'");
    }

    #[test]
    fn ignores_semicolons_in_postgres_literals_and_comments() {
        assert_eq!(
            snapshot_source("SELECT $$one;two$$ AS value;").unwrap(),
            "SELECT $$one;two$$ AS value"
        );
        assert_eq!(
            snapshot_source("SELECT $body$one;two$body$ AS value;").unwrap(),
            "SELECT $body$one;two$body$ AS value"
        );
        assert_eq!(
            snapshot_source("SELECT /* one; /* nested; */ two; */ 1;").unwrap(),
            "SELECT /* one; /* nested; */ two; */ 1"
        );
        assert_eq!(
            snapshot_source("SELECT 1 -- ignored;\n;").unwrap(),
            "SELECT 1 -- ignored;"
        );
        assert_eq!(
            snapshot_source("SELECT 1; -- trailing; comment").unwrap(),
            "SELECT 1"
        );
        assert_eq!(
            snapshot_source("/* leading; */ SELECT 1;").unwrap(),
            "/* leading; */ SELECT 1"
        );
    }

    #[test]
    fn rejects_multiple_statements_after_dollar_literals_and_comments() {
        assert!(snapshot_source("SELECT $$one;two$$; SELECT 2").is_err());
        assert!(snapshot_source("SELECT /* ignored; */ 1; SELECT 2").is_err());
    }

    #[test]
    fn classifies_supported_and_ineligible_statement_shapes() {
        for source in [
            "SELECT 1",
            "WITH values_cte AS (SELECT 1 AS value) SELECT value FROM values_cte",
            "VALUES (1), (2)",
            "TABLE users",
            "SELECT E'it\\'s @result_1; -- literal' AS note",
            "SELECT 123 AS value -- trailing comment",
            "SELECT 1 AS update, 2 AS delete, 3 AS into, 4 AS lock",
            "WITH update AS (SELECT 1 AS delete) SELECT delete FROM update",
        ] {
            assert!(
                matches!(
                    snapshot_eligibility(source),
                    SnapshotEligibility::Eligible(_)
                ),
                "not eligible: {source}"
            );
        }
        for source in [
            "UPDATE users SET active = true",
            "SELECT * INTO TEMP copied FROM users",
            "WITH moved AS (DELETE FROM users RETURNING *) SELECT * FROM moved",
            "WITH created AS (INSERT INTO users(id) VALUES (1) RETURNING *) SELECT * FROM created",
            "WITH source AS (SELECT 1 AS value) SELECT value INTO TEMP copied FROM source",
            "SELECT * FROM users FOR UPDATE",
            "SELECT * FROM users FOR SHARE",
            "SELECT $1::integer",
        ] {
            assert!(
                matches!(
                    snapshot_eligibility(source),
                    SnapshotEligibility::Ineligible(_)
                ),
                "not rejected: {source}"
            );
        }
        assert!(matches!(
            snapshot_eligibility("SELECT E'unterminated\\'"),
            SnapshotEligibility::Invalid(_)
        ));
    }

    #[test]
    fn distinguishes_postgres_identifier_dollars_from_unbound_parameters() {
        for source in [
            "SELECT foo$1",
            "SELECT _x$2",
            "SELECT café$3",
            "SELECT '$1' AS literal",
            "SELECT \"$2\" AS quoted",
            "SELECT $$value $3$$ AS literal",
            "SELECT 1 /* $4 */ -- $5\n",
        ] {
            assert!(!has_unbound_parameter(source), "false parameter: {source}");
        }
        for source in ["SELECT $1", "SELECT ($2)", "SELECT café$3, $4"] {
            assert!(has_unbound_parameter(source), "missed parameter: {source}");
        }
    }

    #[test]
    fn normalizes_identifier_dollars_only_for_parser_validation() {
        let source = "SELECT foo$1, _x$2, café$3, '$4', \"$5\", $$value $6$$ /* $7 */ -- $8\n";

        assert_eq!(
            parser_validation_source(source).unwrap(),
            "SELECT foo_1, _x_2, café_3, '$4', \"$5\", $$value $6$$               \n"
        );
        assert_eq!(
            snapshot_source("SELECT 1 AS foo$1, 2 AS _x$2, 3 AS café$3;").unwrap(),
            "SELECT 1 AS foo$1, 2 AS _x$2, 3 AS café$3"
        );
    }

    #[tokio::test]
    async fn postgres_snapshot_retains_complete_native_result_when_configured() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let client = Arc::new(Mutex::new(client));
        let request = PgSnapshotRequest {
            context: ExecutionContext {
                id: ExecutionId(42),
                target: ExecutionTarget::Notebook(CellId(3)),
                source_revision: 2,
                connection_generation: 9,
            },
            query: "SELECT value, value::text AS text_value FROM generate_series(1, 3) value"
                .to_string(),
            handle: RetainedResultHandle(7),
            max_rows: 10,
            display_max_rows: 10,
            max_bytes: 1_000_000,
            timeout_secs: 5,
            source_snapshots: Vec::new(),
            cancelled: None,
            source_map: None,
        };

        let result = execute(client.clone(), request).await.unwrap();

        assert_eq!(result.query_result.rows.len(), 3);
        assert_eq!(result.query_result.headers, ["value", "text_value"]);
        assert_eq!(result.retained.as_ref().unwrap().rows, 3);
        assert_eq!(
            result.retained.as_ref().unwrap().version.source_cell,
            CellId(3)
        );
        let source_snapshot = result.snapshot.unwrap();
        let source_version = result.retained.unwrap().version;
        let query = crate::app::refinement::compile_logical_reference(
            "SELECT * FROM @result_3 WHERE value > 1",
            source_version,
            &source_snapshot.physical_name,
            &source_snapshot.public_columns,
        )
        .unwrap();
        let refined = execute(
            client,
            PgSnapshotRequest {
                context: ExecutionContext {
                    id: ExecutionId(43),
                    target: ExecutionTarget::Notebook(CellId(4)),
                    source_revision: 1,
                    connection_generation: 9,
                },
                query,
                handle: RetainedResultHandle(8),
                max_rows: 10,
                display_max_rows: 10,
                max_bytes: 1_000_000,
                timeout_secs: 5,
                source_snapshots: vec![source_snapshot],
                cancelled: None,
                source_map: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(refined.query_result.rows.len(), 2);
        assert_eq!(refined.query_result.headers, ["value", "text_value"]);
        assert_eq!(refined.retained.unwrap().rows, 2);
    }

    #[tokio::test]
    async fn postgres_snapshot_handles_boundaries_literals_and_preview_fallback() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let client = Arc::new(Mutex::new(client));
        let request = |id, query: String, max_rows, display_max_rows| PgSnapshotRequest {
            context: ExecutionContext {
                id: ExecutionId(id),
                target: ExecutionTarget::Notebook(CellId(id)),
                source_revision: 1,
                connection_generation: 3,
            },
            query,
            handle: RetainedResultHandle(id),
            max_rows,
            display_max_rows,
            max_bytes: 1_000_000,
            timeout_secs: 5,
            source_snapshots: Vec::new(),
            cancelled: None,
            source_map: None,
        };

        let values = execute(client.clone(), request(101, "VALUES (1), (2)".into(), 1, 5))
            .await
            .unwrap();
        assert_eq!(values.query_result.rows.len(), 2);
        assert!(values.retained.is_none());
        assert!(values.query_result.truncated);

        client
            .lock()
            .await
            .batch_execute(
                "CREATE TEMP TABLE notebook_table_source(value integer); \
                 INSERT INTO notebook_table_source VALUES (1), (2)",
            )
            .await
            .unwrap();
        let table = execute(
            client.clone(),
            request(107, "TABLE notebook_table_source".into(), 5, 5),
        )
        .await
        .unwrap();
        assert_eq!(table.query_result.headers, ["value"]);
        assert_eq!(table.query_result.rows.len(), 2);
        assert!(table.retained.is_some());

        let empty = execute(
            client.clone(),
            request(
                102,
                "SELECT value FROM generate_series(1, 1) value WHERE false".into(),
                5,
                5,
            ),
        )
        .await
        .unwrap();
        assert!(empty.query_result.rows.is_empty());
        assert_eq!(empty.query_result.headers, ["value"]);
        assert!(empty.retained.is_some());

        let literal = execute(
            client.clone(),
            request(
                103,
                "SELECT E'it\\'s @result_1; -- literal' AS note -- trailing comment".into(),
                5,
                5,
            ),
        )
        .await
        .unwrap();
        assert_eq!(literal.query_result.rows, [["it's @result_1; -- literal"]]);
        assert!(literal.retained.is_some());

        let nulls = execute(
            client.clone(),
            request(
                108,
                "SELECT NULL::text AS actual_null, 'NULL'::text AS literal_null".into(),
                5,
                5,
            ),
        )
        .await
        .unwrap();
        assert_eq!(nulls.query_result.rows, [["NULL", "NULL"]]);
        assert_eq!(nulls.query_result.null_cells, [[true, false]]);
        assert!(nulls.retained.is_some());

        let same_context_first = execute(
            client.clone(),
            request(109, "SELECT 1 AS value".into(), 5, 5),
        )
        .await
        .unwrap();
        let same_context_second = execute(
            client.clone(),
            request(109, "SELECT 2 AS value".into(), 5, 5),
        )
        .await
        .unwrap();
        let first_name = same_context_first.snapshot.unwrap().physical_name;
        let second_name = same_context_second.snapshot.unwrap().physical_name;
        assert_ne!(first_name, second_name);
        assert!(first_name.len() <= POSTGRES_IDENTIFIER_BYTES);
        assert!(second_name.len() <= POSTGRES_IDENTIFIER_BYTES);

        let unicode = "á".repeat(31);
        let ordinal = format!("__tsql_row_ordinal_{:016x}", 104u64);
        let collisions = execute(
            client.clone(),
            request(
                104,
                format!(
                    "SELECT 1 AS \"{unicode}\", 2 AS \"{unicode}\", 3 AS \"{unicode}\", 4 AS \"{ordinal}\""
                ),
                5,
                5,
            ),
        )
        .await
        .unwrap();
        assert_eq!(collisions.query_result.rows.len(), 1);
        assert_eq!(collisions.query_result.headers.len(), 4);
        assert_eq!(
            collisions
                .query_result
                .headers
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );

        let pseudo = execute(
            client.clone(),
            request(
                105,
                "SELECT ROW(1, 2) AS record_value, 'x'::cstring AS raw_value, \
                 ARRAY[ROW(1, 2), ROW(3, 4)] AS records"
                    .into(),
                5,
                5,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            pseudo.query_result.headers,
            ["record_value", "raw_value", "records"]
        );
        assert_eq!(
            pseudo.query_result.rows,
            [["(1,2)", "x", "{\"(1,2)\",\"(3,4)\"}"]]
        );
        assert!(pseudo.retained.is_none());

        client
            .lock()
            .await
            .batch_execute("SET default_transaction_read_only = on")
            .await
            .unwrap();
        let read_only = execute(
            client.clone(),
            request(108, "SELECT 1 AS value".into(), 5, 5),
        )
        .await
        .unwrap();
        assert_eq!(read_only.query_result.headers, ["value"]);
        assert_eq!(read_only.query_result.rows, [["1"]]);
        assert!(read_only.retained.is_none());
        client
            .lock()
            .await
            .batch_execute("SET default_transaction_read_only = off")
            .await
            .unwrap();

        let mut cancelled = request(106, "SELECT 1".into(), 5, 5);
        cancelled.cancelled = Some(Arc::new(AtomicBool::new(true)));
        let cancelled_error = match execute(client, cancelled).await {
            Ok(_) => panic!("cancelled snapshot query was executed"),
            Err(error) => error,
        };
        assert!(cancelled_error.contains("cancelled before snapshot execution"));
    }

    #[tokio::test]
    async fn postgres_snapshot_cleanup_preserves_a_replaced_relation() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let client = Arc::new(Mutex::new(client));
        let result = execute(
            client.clone(),
            PgSnapshotRequest {
                context: ExecutionContext {
                    id: ExecutionId(201),
                    target: ExecutionTarget::Notebook(CellId(201)),
                    source_revision: 1,
                    connection_generation: 7,
                },
                query: "SELECT 1 AS value".to_string(),
                handle: RetainedResultHandle(201),
                max_rows: 5,
                display_max_rows: 5,
                max_bytes: 1_000_000,
                timeout_secs: 5,
                source_snapshots: Vec::new(),
                cancelled: None,
                source_map: None,
            },
        )
        .await
        .unwrap();
        let snapshot = result.snapshot.unwrap();
        let guard = client.lock().await;
        validate_snapshot_identity(&guard, &snapshot).await.unwrap();
        let qualified = format!("pg_temp.{}", quote_identifier(&snapshot.physical_name));
        guard
            .batch_execute(&format!(
                "DROP TABLE {qualified}; CREATE TEMP TABLE {} (replacement integer)",
                quote_identifier(&snapshot.physical_name)
            ))
            .await
            .unwrap();

        assert!(validate_snapshot_identity(&guard, &snapshot).await.is_err());
        drop_if_identity_matches(&guard, &snapshot).await.unwrap();
        let relation_oid: Option<u32> = guard
            .query_one("SELECT to_regclass($1)::oid", &[&qualified])
            .await
            .unwrap()
            .get(0);
        assert!(relation_oid.is_some());
        assert_ne!(relation_oid, Some(snapshot.relation_oid));
    }

    #[tokio::test]
    async fn postgres_snapshot_maps_refinement_preflight_error_to_cell_source() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute("CREATE TEMP TABLE notebook_mapped_source(value integer)")
            .await
            .unwrap();

        let source = "  SELECT 'é' AS note, value\nFROM @result_1\nWHERE missing_value = 1";
        let compiled = crate::app::refinement::compile_logical_references_mapped(
            source,
            &[(
                crate::app::refinement::LogicalResultReference::Cell(CellId(1)),
                ResultVersion {
                    source_cell: CellId(1),
                    source_execution: ExecutionId(21),
                    source_revision: 1,
                },
                "notebook_mapped_source".to_string(),
                vec!["value".to_string()],
            )],
        )
        .unwrap();
        let source_position = source[..source.find("missing_value").unwrap()]
            .chars()
            .count()
            + 1;
        let error = match execute(
            Arc::new(Mutex::new(client)),
            PgSnapshotRequest {
                context: ExecutionContext {
                    id: ExecutionId(22),
                    target: ExecutionTarget::Notebook(CellId(2)),
                    source_revision: 1,
                    connection_generation: 1,
                },
                query: compiled.sql,
                handle: RetainedResultHandle(22),
                max_rows: 10,
                display_max_rows: 10,
                max_bytes: 1_000_000,
                timeout_secs: 5,
                source_snapshots: Vec::new(),
                cancelled: None,
                source_map: Some(compiled.source_map),
            },
        )
        .await
        {
            Ok(_) => panic!("invalid refinement unexpectedly executed"),
            Err(error) => error,
        };

        assert!(error.contains("snapshot preflight failed"), "{error}");
        assert!(error.contains("[42703]"), "{error}");
        assert!(
            error.contains(&format!("POSITION: {source_position} (line 3, column 7)")),
            "{error}"
        );
    }
}
