//! Text, CSV, and JSON renderers for headless query results.

use std::collections::HashMap;

use serde_json::{Map, Value};
use tokio_postgres::SimpleQueryMessage;
use unicode_width::UnicodeWidthStr;

/// How `tsql exec` prints a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Table,
    Csv,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResultSet {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<Option<String>>>,
    pub(crate) rows_affected: u64,
}

pub(crate) fn messages_to_sets(messages: Vec<SimpleQueryMessage>) -> Vec<ResultSet> {
    let mut sets = Vec::new();
    let mut columns: Option<Vec<String>> = None;
    let mut rows = Vec::new();

    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(described) => {
                columns = Some(
                    described
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect(),
                );
                rows.clear();
            }
            SimpleQueryMessage::Row(row) => {
                if columns.is_none() {
                    columns = Some(
                        row.columns()
                            .iter()
                            .map(|column| column.name().to_string())
                            .collect(),
                    );
                }
                let cells = (0..row.len())
                    .map(|index| row.get(index).map(str::to_string))
                    .collect();
                rows.push(cells);
            }
            SimpleQueryMessage::CommandComplete(rows_affected) => {
                sets.push(ResultSet {
                    columns: columns.take().unwrap_or_default(),
                    rows: std::mem::take(&mut rows),
                    rows_affected,
                });
            }
            _ => {}
        }
    }

    if columns.is_some() || !rows.is_empty() {
        let rows_affected = u64::try_from(rows.len()).unwrap_or(u64::MAX);
        sets.push(ResultSet {
            columns: columns.unwrap_or_default(),
            rows,
            rows_affected,
        });
    }

    sets
}

pub(crate) fn render(format: OutputFormat, sets: &[ResultSet]) -> String {
    match format {
        OutputFormat::Table => render_table(sets),
        OutputFormat::Csv => render_csv(sets),
        OutputFormat::Json => render_json(sets),
    }
}

fn render_table(sets: &[ResultSet]) -> String {
    if sets.is_empty() {
        return "rows affected: 0\n".to_string();
    }
    let mut out = String::new();
    for (index, set) in sets.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if set.columns.is_empty() {
            out.push_str(&format!("rows affected: {}\n", set.rows_affected));
            continue;
        }
        let displayed: Vec<Vec<String>> = set
            .rows
            .iter()
            .map(|row| {
                (0..set.columns.len())
                    .map(|index| table_cell(row.get(index).and_then(Option::as_ref)))
                    .collect()
            })
            .collect();
        let mut widths: Vec<usize> = set
            .columns
            .iter()
            .map(|column| UnicodeWidthStr::width(column.as_str()))
            .collect();
        for row in &displayed {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
        out.push_str(&join_cells(
            &set.columns
                .iter()
                .enumerate()
                .map(|(index, column)| pad(column, widths[index]))
                .collect::<Vec<_>>(),
        ));
        out.push('\n');
        out.push_str(
            &widths
                .iter()
                .map(|width| "-".repeat(width + 2))
                .collect::<Vec<_>>()
                .join("+"),
        );
        out.push('\n');
        for row in &displayed {
            let padded = row
                .iter()
                .enumerate()
                .map(|(index, cell)| pad(cell, widths[index]))
                .collect::<Vec<_>>();
            out.push_str(&join_cells(&padded));
            out.push('\n');
        }
        let noun = if set.rows.len() == 1 { "row" } else { "rows" };
        out.push_str(&format!("({} {noun})\n", set.rows.len()));
    }
    out
}

fn join_cells(cells: &[String]) -> String {
    cells
        .iter()
        .map(|cell| format!(" {cell} "))
        .collect::<Vec<_>>()
        .join("|")
}

fn pad(text: &str, width: usize) -> String {
    let mut padded = text.to_string();
    let displayed = UnicodeWidthStr::width(text);
    if displayed < width {
        padded.push_str(&" ".repeat(width - displayed));
    }
    padded
}

fn table_cell(value: Option<&String>) -> String {
    match value {
        Some(text) => text
            .replace('\t', "\\t")
            .replace('\r', "\\r")
            .replace('\n', "\\n"),
        None => "NULL".to_string(),
    }
}

fn render_csv(sets: &[ResultSet]) -> String {
    if sets.is_empty() {
        return "rows_affected\n0\n".to_string();
    }
    let mut out = String::new();
    for (index, set) in sets.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if set.columns.is_empty() {
            out.push_str("rows_affected\n");
            out.push_str(&set.rows_affected.to_string());
            out.push('\n');
            continue;
        }
        out.push_str(
            &set.columns
                .iter()
                .map(|column| escape_csv(column))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
        for row in &set.rows {
            let fields = (0..set.columns.len())
                .map(|index| match row.get(index).and_then(Option::as_ref) {
                    Some(text) => escape_csv(text),
                    None => String::new(),
                })
                .collect::<Vec<_>>();
            out.push_str(&fields.join(","));
            out.push('\n');
        }
    }
    out
}

fn escape_csv(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn render_json(sets: &[ResultSet]) -> String {
    let value = match sets {
        [] => command_value(0),
        [only] if only.columns.is_empty() => command_value(only.rows_affected),
        [only] => rows_value(only),
        many => Value::Array(many.iter().map(set_value).collect()),
    };
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".to_string());
    text.push('\n');
    text
}

fn command_value(rows_affected: u64) -> Value {
    let mut object = Map::new();
    object.insert("rows_affected".to_string(), Value::from(rows_affected));
    Value::Object(object)
}

fn rows_value(set: &ResultSet) -> Value {
    let names = unique_column_names(&set.columns);
    Value::Array(
        set.rows
            .iter()
            .map(|row| {
                let mut object = Map::new();
                for (index, name) in names.iter().enumerate() {
                    let value = match row.get(index).and_then(Option::as_ref) {
                        Some(text) => Value::String(text.clone()),
                        None => Value::Null,
                    };
                    object.insert(name.clone(), value);
                }
                Value::Object(object)
            })
            .collect(),
    )
}

fn set_value(set: &ResultSet) -> Value {
    let mut object = Map::new();
    object.insert(
        "columns".to_string(),
        Value::Array(set.columns.iter().cloned().map(Value::String).collect()),
    );
    object.insert("rows".to_string(), rows_value(set));
    object.insert("rows_affected".to_string(), Value::from(set.rows_affected));
    Value::Object(object)
}

fn unique_column_names(names: &[String]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut unique = Vec::with_capacity(names.len());
    for name in names {
        let count = seen.entry(name.as_str()).or_insert(0);
        *count += 1;
        if *count == 1 {
            unique.push(name.clone());
        } else {
            unique.push(format!("{name}_{count}"));
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResultSet {
        ResultSet {
            columns: vec!["id".to_string(), "note".to_string()],
            rows: vec![
                vec![Some("1".to_string()), None],
                vec![
                    Some("2".to_string()),
                    Some("say \"hi\", friend".to_string()),
                ],
            ],
            rows_affected: 2,
        }
    }

    #[test]
    fn json_preserves_nulls_and_text() {
        let text = render(OutputFormat::Json, &[sample()]);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value.is_array());
        assert_eq!(value[0]["id"], "1");
        assert!(value[0]["note"].is_null());
        assert_eq!(value[1]["note"], "say \"hi\", friend");
        assert!(text.contains('\n'));
    }

    #[test]
    fn json_command_and_multiple_sets() {
        let command = render(
            OutputFormat::Json,
            &[ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: 3,
            }],
        );
        let value: Value = serde_json::from_str(&command).unwrap();
        assert_eq!(value["rows_affected"], 3);

        let combined = render(OutputFormat::Json, &[sample(), sample()]);
        let value: Value = serde_json::from_str(&combined).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2);
        assert!(value[0]["columns"].is_array());
    }

    #[test]
    fn json_disambiguates_duplicate_column_names() {
        let set = ResultSet {
            columns: vec!["id".to_string(), "id".to_string()],
            rows: vec![vec![Some("1".to_string()), Some("2".to_string())]],
            rows_affected: 1,
        };
        let value: Value = serde_json::from_str(&render(OutputFormat::Json, &[set])).unwrap();
        assert_eq!(value[0]["id"], "1");
        assert_eq!(value[0]["id_2"], "2");
    }

    #[test]
    fn csv_quotes_special_characters_and_leaves_nulls_empty() {
        let text = render(OutputFormat::Csv, &[sample()]);
        assert_eq!(text, "id,note\n1,\n2,\"say \"\"hi\"\", friend\"\n");
    }

    #[test]
    fn table_aligns_columns_and_names_null() {
        let text = render(OutputFormat::Table, &[sample()]);
        assert_eq!(
            text,
            " id | note             \n----+------------------\n 1  | NULL             \n 2  | say \"hi\", friend \n(2 rows)\n"
        );
    }

    #[test]
    fn table_escapes_newlines_and_reports_commands() {
        let set = ResultSet {
            columns: vec!["body".to_string()],
            rows: vec![vec![Some("a\nb".to_string())]],
            rows_affected: 1,
        };
        let text = render(OutputFormat::Table, &[set]);
        assert!(text.contains("a\\nb"), "{text}");
        assert!(text.contains("(1 row)"), "{text}");

        let command = render(
            OutputFormat::Table,
            &[ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: 4,
            }],
        );
        assert_eq!(command, "rows affected: 4\n");
    }
}
