//! PostgreSQL session-local TEMP-table refinement provider.

use std::time::Instant;

use tokio_postgres::SimpleQueryMessage;

use super::app::{QueryResult, SharedClient};
use super::execution::ExecutionContext;
use super::refinement::{
    RefinementProviderId, ResultVersion, RetainedResult, RetainedResultHandle,
};
use crate::util::format_pg_error;

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
    pub source_snapshot: Option<PgTempSnapshot>,
}

pub(crate) async fn execute(
    client: SharedClient,
    request: PgSnapshotRequest,
) -> Result<PgSnapshotResult, String> {
    let started = Instant::now();
    let source = snapshot_source(&request.query)?;
    let mut guard = client.lock().await;
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

    if let Some(expected) = &request.source_snapshot {
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

    let statement = transaction
        .prepare(&source)
        .await
        .map_err(|error| pg_error("snapshot preflight failed", &error))?;
    if statement.columns().is_empty() {
        return Err("statement has no row result to retain".to_string());
    }
    if statement
        .columns()
        .iter()
        .any(|column| matches!(column.type_().name(), "record" | "void"))
    {
        return Err("result contains a PostgreSQL pseudo-type".to_string());
    }

    let physical_name = format!(
        "__tsql_nb_{}_{}",
        request.context.id.0, request.context.source_revision
    );
    let ordinal_name = format!("__tsql_row_ordinal_{}", request.context.id.0);
    let output_names =
        normalize_column_names(statement.columns().iter().map(|column| column.name()));
    let mut table_columns = Vec::with_capacity(output_names.len() + 1);
    table_columns.push(quote_identifier(&ordinal_name));
    table_columns.extend(output_names.iter().map(|name| quote_identifier(name)));
    let create_sql = format!(
        "CREATE TEMP TABLE {} ({}) ON COMMIT PRESERVE ROWS AS \
         SELECT row_number() OVER (), __tsql_source.* FROM ({}) AS __tsql_source \
         LIMIT {}",
        quote_identifier(&physical_name),
        table_columns.join(", "),
        source,
        request.max_rows.saturating_add(1)
    );
    transaction
        .batch_execute(&create_sql)
        .await
        .map_err(|error| pg_error("snapshot materialization failed", &error))?;

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

    let select_columns = output_names
        .iter()
        .map(|name| quote_identifier(name))
        .collect::<Vec<_>>()
        .join(", ");
    let display_sql = format!(
        "SELECT {select_columns} FROM {qualified_name} \
         ORDER BY {} LIMIT {}",
        quote_identifier(&ordinal_name),
        request.display_max_rows
    );
    let messages = transaction
        .simple_query(&display_sql)
        .await
        .map_err(|error| pg_error("failed to read snapshot result", &error))?;
    let (headers, rows) = display_rows(messages, request.display_max_rows);
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
            command_tag: Some(format!("SELECT {row_count}")),
            truncated: !complete || row_count > request.display_max_rows,
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

fn snapshot_source(query: &str) -> Result<String, String> {
    let trimmed = query.trim();
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    let mut terminal_semicolon = None;
    let mut comment_ranges = Vec::new();

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                if terminal_semicolon.is_some() {
                    return Err(
                        "multiple statements are not eligible for a session snapshot".to_string(),
                    );
                }
                let delimiter = bytes[index];
                index += 1;
                loop {
                    let Some(offset) = trimmed[index..].find(delimiter as char) else {
                        return Err("unterminated SQL literal".to_string());
                    };
                    index += offset + 1;
                    if bytes.get(index) == Some(&delimiter) {
                        index += 1;
                    } else {
                        break;
                    }
                }
            }
            b'$' => {
                let Some(tag_end_offset) = trimmed[index + 1..].find('$') else {
                    if terminal_semicolon.is_some() {
                        return Err(
                            "multiple statements are not eligible for a session snapshot"
                                .to_string(),
                        );
                    }
                    index += 1;
                    continue;
                };
                let tag_end = index + 1 + tag_end_offset;
                let tag = &trimmed[index + 1..tag_end];
                let mut characters = tag.chars();
                let valid_tag = characters.next().is_none_or(|character| {
                    (character.is_ascii_alphabetic() || character == '_')
                        && characters
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                });
                if !valid_tag {
                    if terminal_semicolon.is_some() {
                        return Err(
                            "multiple statements are not eligible for a session snapshot"
                                .to_string(),
                        );
                    }
                    index += 1;
                    continue;
                }
                if terminal_semicolon.is_some() {
                    return Err(
                        "multiple statements are not eligible for a session snapshot".to_string(),
                    );
                }
                let delimiter = &trimmed[index..=tag_end];
                let content_start = tag_end + 1;
                let Some(close_offset) = trimmed[content_start..].find(delimiter) else {
                    return Err("unterminated dollar-quoted SQL literal".to_string());
                };
                index = content_start + close_offset + delimiter.len();
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                let start = index;
                index = trimmed[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset + 1);
                comment_ranges.push((start, index));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                let mut depth = 1;
                index += 2;
                while index < bytes.len() && depth > 0 {
                    if bytes.get(index..index + 2) == Some(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes.get(index..index + 2) == Some(b"*/") {
                        depth -= 1;
                        index += 2;
                    } else {
                        index += trimmed[index..].chars().next().unwrap().len_utf8();
                    }
                }
                if depth > 0 {
                    return Err("unterminated SQL comment".to_string());
                }
                comment_ranges.push((start, index));
            }
            b';' => {
                if terminal_semicolon.is_some() {
                    return Err(
                        "multiple statements are not eligible for a session snapshot".to_string(),
                    );
                }
                terminal_semicolon = Some(index);
                index += 1;
            }
            _ => {
                let character = trimmed[index..].chars().next().unwrap();
                if terminal_semicolon.is_some() && !character.is_whitespace() {
                    return Err(
                        "multiple statements are not eligible for a session snapshot".to_string(),
                    );
                }
                index += character.len_utf8();
            }
        }
    }

    let source = terminal_semicolon.map_or(trimmed, |index| trimmed[..index].trim_end());
    let mut parser_source = source.to_string();
    for (start, end) in comment_ranges.into_iter().rev() {
        if start < parser_source.len() {
            parser_source.replace_range(start..end.min(parser_source.len()), " ");
        }
    }
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_sequel::LANGUAGE.into())
        .map_err(|error| format!("failed to initialize SQL parser: {error}"))?;
    let tree = parser
        .parse(&parser_source, None)
        .ok_or_else(|| "failed to parse SQL statement".to_string())?;
    let root = tree.root_node();
    if root.has_error() || root.named_child_count() != 1 {
        return Err("statement shape is not eligible for a session snapshot".to_string());
    }
    let first = parser_source
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if !matches!(first.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE") {
        return Err("statement is not eligible for a session snapshot".to_string());
    }
    Ok(source.to_string())
}

pub(crate) fn is_snapshot_candidate(query: &str) -> bool {
    snapshot_source(query).is_ok()
}

fn normalize_column_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    names
        .enumerate()
        .map(|(index, name)| {
            let base = if name.is_empty() {
                format!("column_{}", index + 1)
            } else {
                name.chars().take(50).collect()
            };
            let mut candidate = base.clone();
            let mut suffix = 2;
            while !used.insert(candidate.clone()) {
                candidate = format!("{base}_{suffix}");
                suffix += 1;
            }
            candidate
        })
        .collect()
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn display_rows(
    messages: Vec<SimpleQueryMessage>,
    max_rows: usize,
) -> (Vec<String>, Vec<Vec<String>>) {
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    for message in messages {
        if let SimpleQueryMessage::Row(row) = message {
            if headers.is_empty() {
                headers = row
                    .columns()
                    .iter()
                    .map(|column| column.name().to_string())
                    .collect();
            }
            if rows.len() < max_rows {
                rows.push(
                    (0..row.len())
                        .map(|index| row.get(index).unwrap_or("NULL").to_string())
                        .collect(),
                );
            }
        }
    }
    (headers, rows)
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
            normalize_column_names(["id", "id", ""].into_iter()),
            ["id", "id_2", "column_3"]
        );
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
            source_snapshot: None,
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
                source_snapshot: Some(source_snapshot),
            },
        )
        .await
        .unwrap();
        assert_eq!(refined.query_result.rows.len(), 2);
        assert_eq!(refined.query_result.headers, ["value", "text_value"]);
        assert_eq!(refined.retained.unwrap().rows, 2);
    }
}
