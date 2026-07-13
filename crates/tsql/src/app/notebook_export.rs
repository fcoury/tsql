//! Bounded, atomic exports for retained PostgreSQL notebook results.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::TryStreamExt;
use tokio::fs::{self, File};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_postgres::{Client, SimpleQueryMessage};
use uuid::Uuid;

use super::pg_snapshot::{validate_snapshot_identity, PgTempSnapshot};
use super::refinement::ResultVersion;
use crate::ui::quote_identifier;

/// File format for a streamed notebook-result export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NotebookExportFormat {
    Csv,
    Json,
    Tsv,
    Sql { table: String },
}

impl NotebookExportFormat {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Json => "JSON",
            Self::Tsv => "TSV",
            Self::Sql { .. } => "SQL",
        }
    }
}

/// Stream a retained snapshot to a temporary sibling file, then atomically replace the target.
///
/// Rows are formatted and written as they arrive; a full retained result is never held in memory.
pub(crate) async fn export_snapshot(
    client: &Client,
    snapshot: &PgTempSnapshot,
    version: ResultVersion,
    path: &Path,
    format: &NotebookExportFormat,
    cancelled: &Arc<AtomicBool>,
) -> Result<usize, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("export cancelled".to_string());
    }
    validate_snapshot_identity(client, snapshot).await?;

    let qualified = format!(
        "pg_temp.\"{}\"",
        snapshot.physical_name.replace('"', "\"\"")
    );
    let columns = snapshot
        .public_columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let ordinal = format!("__tsql_row_ordinal_{:016x}", version.source_execution.0);
    let query = format!("SELECT {columns} FROM {qualified} ORDER BY \"{ordinal}\"");

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("invalid export path: {}", path.display()))?;
    let temporary = path.with_file_name(format!(".{file_name}.{}.part", Uuid::new_v4().simple()));
    let result = write_snapshot(client, &query, snapshot, &temporary, format, cancelled).await;
    match result {
        Ok(rows) => {
            if let Err(error) = fs::rename(&temporary, path).await {
                let _ = fs::remove_file(&temporary).await;
                return Err(format!("failed to replace {}: {error}", path.display()));
            }
            Ok(rows)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            Err(error)
        }
    }
}

async fn write_snapshot(
    client: &Client,
    query: &str,
    snapshot: &PgTempSnapshot,
    path: &PathBuf,
    format: &NotebookExportFormat,
    cancelled: &Arc<AtomicBool>,
) -> Result<usize, String> {
    let file = File::create(path)
        .await
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    write_header(&mut writer, &snapshot.public_columns, format).await?;

    let stream = client
        .simple_query_raw(query)
        .await
        .map_err(|error| format!("failed to read retained result: {error}"))?;
    tokio::pin!(stream);
    let mut rows = 0usize;
    while let Some(message) = stream
        .try_next()
        .await
        .map_err(|error| format!("failed to read retained result: {error}"))?
    {
        if cancelled.load(Ordering::Acquire) {
            return Err("export cancelled".to_string());
        }
        if let SimpleQueryMessage::Row(row) = message {
            let values = (0..row.len())
                .map(|index| row.get(index))
                .collect::<Vec<_>>();
            write_row(&mut writer, &snapshot.public_columns, &values, format, rows).await?;
            rows = rows.saturating_add(1);
        }
    }
    if matches!(format, NotebookExportFormat::Json) {
        writer
            .write_all(if rows == 0 { b"[]\n" } else { b"\n]\n" })
            .await
            .map_err(|error| format!("failed to finish JSON export: {error}"))?;
    }
    writer
        .flush()
        .await
        .map_err(|error| format!("failed to flush export: {error}"))?;
    Ok(rows)
}

async fn write_header(
    writer: &mut BufWriter<File>,
    headers: &[String],
    format: &NotebookExportFormat,
) -> Result<(), String> {
    let line = match format {
        NotebookExportFormat::Csv => format!(
            "{}\n",
            headers
                .iter()
                .map(|header| escape_delimited(header, ','))
                .collect::<Vec<_>>()
                .join(",")
        ),
        NotebookExportFormat::Tsv => format!(
            "{}\n",
            headers
                .iter()
                .map(|header| escape_tsv(header))
                .collect::<Vec<_>>()
                .join("\t")
        ),
        NotebookExportFormat::Json | NotebookExportFormat::Sql { .. } => String::new(),
    };
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("failed to write export header: {error}"))
}

async fn write_row(
    writer: &mut BufWriter<File>,
    headers: &[String],
    values: &[Option<&str>],
    format: &NotebookExportFormat,
    row_index: usize,
) -> Result<(), String> {
    let line = match format {
        NotebookExportFormat::Csv => format!(
            "{}\n",
            values
                .iter()
                .map(|value| escape_delimited(value.unwrap_or_default(), ','))
                .collect::<Vec<_>>()
                .join(",")
        ),
        NotebookExportFormat::Tsv => format!(
            "{}\n",
            values
                .iter()
                .map(|value| escape_tsv(value.unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\t")
        ),
        NotebookExportFormat::Json => {
            let pairs = headers
                .iter()
                .zip(values.iter())
                .map(|(header, value)| match value {
                    Some(value) => {
                        format!("\"{}\": \"{}\"", escape_json(header), escape_json(value))
                    }
                    None => format!("\"{}\": null", escape_json(header)),
                })
                .collect::<Vec<_>>()
                .join(", ");
            if row_index == 0 {
                format!("[\n  {{{pairs}}}")
            } else {
                format!(",\n  {{{pairs}}}")
            }
        }
        NotebookExportFormat::Sql { table } => {
            let table = table
                .split('.')
                .map(quote_identifier)
                .collect::<Vec<_>>()
                .join(".");
            let columns = headers
                .iter()
                .map(|header| quote_identifier(header))
                .collect::<Vec<_>>()
                .join(", ");
            let values = values
                .iter()
                .map(|value| {
                    value.map_or_else(
                        || "NULL".to_string(),
                        |value| format!("'{}'", value.replace('\'', "''")),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {table} ({columns}) VALUES ({values});\n")
        }
    };
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("failed to write export row: {error}"))
}

fn escape_delimited(value: &str, delimiter: char) -> String {
    if value.contains([delimiter, '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn escape_tsv(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{0008}' => escaped.push_str("\\b"),
            '\u{000c}' => escaped.push_str("\\f"),
            control if control < ' ' => escaped.push_str(&format!("\\u{:04x}", control as u32)),
            printable => escaped.push(printable),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::{escape_delimited, escape_json, escape_tsv, export_snapshot, NotebookExportFormat};
    use crate::app::execution::{CellId, ExecutionId};
    use crate::app::pg_snapshot::PgTempSnapshot;
    use crate::app::refinement::ResultVersion;

    #[test]
    fn delimited_fields_quote_special_characters() {
        assert_eq!(escape_delimited("plain", ','), "plain");
        assert_eq!(escape_delimited("a,b", ','), "\"a,b\"");
        assert_eq!(escape_delimited("say \"hi\"", ','), "\"say \"\"hi\"\"\"");
        assert_eq!(escape_delimited("two\nlines", ','), "\"two\nlines\"");
    }

    #[test]
    fn json_and_tsv_fields_keep_row_boundaries_intact() {
        assert_eq!(escape_tsv("a\tb\nc\\d"), "a\\tb\\nc\\\\d");
        assert_eq!(escape_json("a\"b\nc\\d\t"), "a\\\"b\\nc\\\\d\\t");
        assert_eq!(
            escape_json("back\u{0008} form\u{000c} control\u{0001}"),
            "back\\b form\\f control\\u0001"
        );
    }

    #[test]
    fn format_labels_are_stable() {
        assert_eq!(NotebookExportFormat::Csv.label(), "CSV");
        assert_eq!(NotebookExportFormat::Json.label(), "JSON");
        assert_eq!(NotebookExportFormat::Tsv.label(), "TSV");
        assert_eq!(
            NotebookExportFormat::Sql {
                table: "result".to_string()
            }
            .label(),
            "SQL"
        );
    }

    #[tokio::test]
    async fn postgres_snapshot_exports_all_formats_in_ordinal_order_and_preserves_values() {
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
            .batch_execute(
                r#"
                CREATE TEMP TABLE "export ""snapshot""" (
                    "id" text,
                    "odd, ""name""" text,
                    "notes" text,
                    "missing" text,
                    "__tsql_row_ordinal_000000000000002a" bigint
                );
                INSERT INTO "export ""snapshot""" VALUES
                    ('2', '', 'NULL', 'value', 2),
                    ('1', 'O''Reilly, "hello"', E'line one\nline two\t\\tail', NULL, 1);
                "#,
            )
            .await
            .unwrap();
        let identity = client
            .query_one(
                r#"SELECT pg_backend_pid(), 'pg_temp."export ""snapshot"""'::regclass::oid"#,
                &[],
            )
            .await
            .unwrap();
        let snapshot = PgTempSnapshot {
            physical_name: "export \"snapshot\"".to_string(),
            public_columns: vec![
                "id".to_string(),
                "odd, \"name\"".to_string(),
                "notes".to_string(),
                "missing".to_string(),
            ],
            relation_oid: identity.get(1),
            connection_generation: 4,
            backend_pid: identity.get(0),
            row_count: 2,
            byte_size: 256,
        };
        let version = ResultVersion {
            source_cell: CellId(3),
            source_execution: ExecutionId(42),
            source_revision: 1,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let directory = tempfile::tempdir().unwrap();

        let csv = directory.path().join("result.csv");
        tokio::fs::write(&csv, "stale").await.unwrap();
        assert_eq!(
            export_snapshot(
                &client,
                &snapshot,
                version,
                &csv,
                &NotebookExportFormat::Csv,
                &cancelled,
            )
            .await
            .unwrap(),
            2
        );
        assert_eq!(
            tokio::fs::read_to_string(&csv).await.unwrap(),
            concat!(
                "id,\"odd, \"\"name\"\"\",notes,missing\n",
                "1,\"O'Reilly, \"\"hello\"\"\",\"line one\nline two\t\\tail\",\n",
                "2,,NULL,value\n"
            )
        );

        let tsv = directory.path().join("result.tsv");
        assert_eq!(
            export_snapshot(
                &client,
                &snapshot,
                version,
                &tsv,
                &NotebookExportFormat::Tsv,
                &cancelled,
            )
            .await
            .unwrap(),
            2
        );
        assert_eq!(
            tokio::fs::read_to_string(&tsv).await.unwrap(),
            concat!(
                "id\todd, \"name\"\tnotes\tmissing\n",
                "1\tO'Reilly, \"hello\"\tline one\\nline two\\t\\\\tail\t\n",
                "2\t\tNULL\tvalue\n"
            )
        );

        let json = directory.path().join("result.json");
        assert_eq!(
            export_snapshot(
                &client,
                &snapshot,
                version,
                &json,
                &NotebookExportFormat::Json,
                &cancelled,
            )
            .await
            .unwrap(),
            2
        );
        let json = serde_json::from_str::<serde_json::Value>(
            &tokio::fs::read_to_string(&json).await.unwrap(),
        )
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {
                    "id": "1",
                    "odd, \"name\"": "O'Reilly, \"hello\"",
                    "notes": "line one\nline two\t\\tail",
                    "missing": null
                },
                {"id": "2", "odd, \"name\"": "", "notes": "NULL", "missing": "value"}
            ])
        );

        let sql = directory.path().join("result.sql");
        assert_eq!(
            export_snapshot(
                &client,
                &snapshot,
                version,
                &sql,
                &NotebookExportFormat::Sql {
                    table: "reporting.user \"copy\"".to_string()
                },
                &cancelled,
            )
            .await
            .unwrap(),
            2
        );
        assert_eq!(
            tokio::fs::read_to_string(&sql).await.unwrap(),
            concat!(
                "INSERT INTO reporting.\"user \"\"copy\"\"\" ",
                "(id, \"odd, \"\"name\"\"\", notes, missing) ",
                "VALUES ('1', 'O''Reilly, \"hello\"', 'line one\nline two\t\\tail', NULL);\n",
                "INSERT INTO reporting.\"user \"\"copy\"\"\" ",
                "(id, \"odd, \"\"name\"\"\", notes, missing) ",
                "VALUES ('2', '', 'NULL', 'value');\n"
            )
        );

        let destination_directory = directory.path().join("cannot-replace");
        tokio::fs::create_dir(&destination_directory).await.unwrap();
        let error = export_snapshot(
            &client,
            &snapshot,
            version,
            &destination_directory,
            &NotebookExportFormat::Csv,
            &cancelled,
        )
        .await
        .unwrap_err();
        assert!(error.contains("failed to replace"), "{error}");
        assert!(destination_directory.is_dir());

        let mut entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            [
                std::ffi::OsString::from("cannot-replace"),
                std::ffi::OsString::from("result.csv"),
                std::ffi::OsString::from("result.json"),
                std::ffi::OsString::from("result.sql"),
                std::ffi::OsString::from("result.tsv")
            ]
        );
    }

    #[tokio::test]
    async fn cancelled_export_preserves_existing_target_and_leaves_no_partial_file() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("result.csv");
        tokio::fs::write(&target, "keep me").await.unwrap();
        let snapshot = PgTempSnapshot {
            physical_name: "missing".to_string(),
            public_columns: vec!["value".to_string()],
            relation_oid: 0,
            connection_generation: 1,
            backend_pid: 0,
            row_count: 0,
            byte_size: 0,
        };
        let version = ResultVersion {
            source_cell: CellId(1),
            source_execution: ExecutionId(1),
            source_revision: 1,
        };
        let cancelled = Arc::new(AtomicBool::new(true));

        let error = export_snapshot(
            &client,
            &snapshot,
            version,
            &target,
            &NotebookExportFormat::Csv,
            &cancelled,
        )
        .await
        .unwrap_err();

        assert_eq!(error, "export cancelled");
        assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "keep me");
        let entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, [std::ffi::OsString::from("result.csv")]);
    }
}
