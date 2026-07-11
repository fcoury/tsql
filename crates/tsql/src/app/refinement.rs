//! Database-neutral retained-result contract for notebook refinement.

use super::execution::{CellId, ExecutionId};

/// Immutable identity of one executed cell result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResultVersion {
    pub source_cell: CellId,
    pub source_execution: ExecutionId,
    pub source_revision: u64,
}

/// Opaque key into a backend-specific retained-resource registry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RetainedResultHandle(pub(crate) u64);

/// User-visible semantics of a refinement provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefinementProviderId {
    PostgresTemp,
}

/// Published retained result. Backend resource details remain in its manager.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedResult {
    pub provider: RefinementProviderId,
    pub handle: RetainedResultHandle,
    pub version: ResultVersion,
    pub rows: usize,
    pub retained_bytes: u64,
    pub connection_generation: u64,
}

/// Why a cell cannot currently be refined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefinementUnavailableReason {
    SnapshotDisabled,
    UnsupportedBackend,
    TransactionNotIdle,
    IneligibleStatement,
    ResultIncomplete,
    SnapshotEvicted,
    ConnectionChanged,
}

/// Current refinement state rendered for one output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefinementAvailability {
    Available(RetainedResult),
    Unavailable(RefinementUnavailableReason),
}

/// Logical result selected by a structural notebook reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogicalResultReference {
    Latest,
    Cell(CellId),
}

fn visit_logical_result_references(
    source: &str,
    mut visit: impl FnMut(&str, usize, usize) -> Result<(), String>,
) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                let delimiter = bytes[index];
                index += 1;
                loop {
                    let Some(offset) = source[index..].find(delimiter as char) else {
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
                let Some(tag_end_offset) = source[index + 1..].find('$') else {
                    index += 1;
                    continue;
                };
                let tag_end = index + 1 + tag_end_offset;
                let tag = &source[index + 1..tag_end];
                let mut characters = tag.chars();
                let valid_tag = characters.next().is_none_or(|character| {
                    (character.is_ascii_alphabetic() || character == '_')
                        && characters
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                });
                if !valid_tag {
                    index += 1;
                    continue;
                }
                let delimiter = &source[index..=tag_end];
                let content_start = tag_end + 1;
                let Some(close_offset) = source[content_start..].find(delimiter) else {
                    return Err("unterminated dollar-quoted SQL literal".to_string());
                };
                index = content_start + close_offset + delimiter.len();
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = source[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset + 1);
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
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
                        index += source[index..].chars().next().unwrap().len_utf8();
                    }
                }
                if depth > 0 {
                    return Err("unterminated SQL comment".to_string());
                }
            }
            b'@' if source[index..].starts_with("@result")
                && source[..index].chars().next_back().is_none_or(|character| {
                    !character.is_alphanumeric() && character != '_' && character != '$'
                }) =>
            {
                let start = index;
                index += "@result".len();
                while let Some(character) = source[index..].chars().next() {
                    if !character.is_alphanumeric() && character != '_' && character != '$' {
                        break;
                    }
                    index += character.len_utf8();
                }
                visit(&source[start..index], start, index)?;
            }
            _ => index += source[index..].chars().next().unwrap().len_utf8(),
        }
    }

    Ok(())
}

/// Returns the single structural result reference used by a notebook query.
pub(crate) fn logical_result_reference(
    source: &str,
) -> Result<Option<LogicalResultReference>, String> {
    let mut found = None;
    visit_logical_result_references(source, |reference, _, _| {
        let current = if reference == "@result" {
            LogicalResultReference::Latest
        } else if let Some(cell) = reference.strip_prefix("@result_") {
            let id = cell
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| format!("invalid logical result reference: {reference}"))?;
            LogicalResultReference::Cell(CellId(id))
        } else {
            return Err(format!("invalid logical result reference: {reference}"));
        };

        if found.is_some_and(|found| found != current) {
            return Err("a cell can reference only one result source".to_string());
        }
        found = Some(current);
        Ok(())
    })?;
    Ok(found)
}

/// Compiles one structurally bound logical result reference outside SQL literals/comments.
pub(crate) fn compile_logical_reference(
    source: &str,
    version: ResultVersion,
    physical_name: &str,
    public_columns: &[String],
) -> Result<String, String> {
    match logical_result_reference(source)? {
        Some(LogicalResultReference::Latest) => {}
        Some(LogicalResultReference::Cell(cell)) if cell == version.source_cell => {}
        Some(LogicalResultReference::Cell(_)) => {
            return Err("logical result reference does not match cell dependency".to_string());
        }
        None => {
            return Err("dependent cell is missing its logical result reference".to_string());
        }
    }
    let columns = public_columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let replacement = format!(
        "(SELECT {columns} FROM pg_temp.\"{}\") AS \"__tsql_result_{}\"",
        physical_name.replace('"', "\"\""),
        version.source_cell.0
    );
    let mut compiled = String::with_capacity(source.len() + replacement.len());
    let mut copied_until = 0;
    visit_logical_result_references(source, |_, start, end| {
        compiled.push_str(&source[copied_until..start]);
        compiled.push_str(&replacement);
        copied_until = end;
        Ok(())
    })?;
    compiled.push_str(&source[copied_until..]);

    Ok(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_only_structurally_bound_reference() {
        let version = ResultVersion {
            source_cell: CellId(3),
            source_execution: ExecutionId(7),
            source_revision: 2,
        };
        let compiled = compile_logical_reference(
            "select '@result_3', $$@result_3$$, * from @result_3 -- @result_3",
            version,
            "snapshot_3",
            &["value".to_string()],
        )
        .unwrap();
        assert_eq!(
            compiled,
            "select '@result_3', $$@result_3$$, * from (SELECT \"value\" FROM pg_temp.\"snapshot_3\") AS \"__tsql_result_3\" -- @result_3"
        );
    }

    #[test]
    fn preserves_utf8_and_ignores_nested_comments_and_quoted_references() {
        let version = ResultVersion {
            source_cell: CellId(3),
            source_execution: ExecutionId(7),
            source_revision: 2,
        };
        let source = "SELECT 'café @result_3', \"Δ@result_3\", $tag$🙂 @result_3$tag$ /* outer /* @result_3 */ still comment */ FROM @result_3 -- naïve @result_3";

        let compiled =
            compile_logical_reference(source, version, "snapshot_3", &["value".to_string()])
                .unwrap();

        assert_eq!(
            compiled,
            "SELECT 'café @result_3', \"Δ@result_3\", $tag$🙂 @result_3$tag$ /* outer /* @result_3 */ still comment */ FROM (SELECT \"value\" FROM pg_temp.\"snapshot_3\") AS \"__tsql_result_3\" -- naïve @result_3"
        );
    }

    #[test]
    fn rejects_prefix_and_malformed_reference_mismatches() {
        let version = ResultVersion {
            source_cell: CellId(1),
            source_execution: ExecutionId(7),
            source_revision: 2,
        };

        assert_eq!(
            compile_logical_reference(
                "SELECT * FROM @result_10",
                version,
                "snapshot_1",
                &["value".to_string()]
            ),
            Err("logical result reference does not match cell dependency".to_string())
        );
        for reference in ["@result_1suffix", "@result_", "@result_0", "@resulting"] {
            let source = format!("SELECT * FROM {reference}");
            assert_eq!(
                logical_result_reference(&source),
                Err(format!("invalid logical result reference: {reference}"))
            );
        }
    }

    #[test]
    fn detects_latest_and_numbered_structural_references() {
        assert_eq!(
            logical_result_reference("SELECT * FROM @result"),
            Ok(Some(LogicalResultReference::Latest))
        );
        assert_eq!(
            logical_result_reference("SELECT * FROM @result_12"),
            Ok(Some(LogicalResultReference::Cell(CellId(12))))
        );
        assert_eq!(
            logical_result_reference(
                "SELECT '@result', \"@result_2\", $$@result_3$$ /* @result_4 */ -- @result_5"
            ),
            Ok(None)
        );
        assert_eq!(
            logical_result_reference("SELECT 1 /* outer /* @result_1 */ still comment */"),
            Ok(None)
        );
        assert_eq!(
            logical_result_reference("SELECT prefix@result, prefix@result_12"),
            Ok(None)
        );
    }

    #[test]
    fn compiles_latest_reference_and_rejects_mixed_sources() {
        let version = ResultVersion {
            source_cell: CellId(4),
            source_execution: ExecutionId(9),
            source_revision: 3,
        };

        assert_eq!(
            compile_logical_reference(
                "SELECT * FROM @result UNION ALL SELECT * FROM @result",
                version,
                "snapshot_4",
                &["value".to_string()]
            ),
            Ok("SELECT * FROM (SELECT \"value\" FROM pg_temp.\"snapshot_4\") AS \"__tsql_result_4\" UNION ALL SELECT * FROM (SELECT \"value\" FROM pg_temp.\"snapshot_4\") AS \"__tsql_result_4\"".to_string())
        );
        assert_eq!(
            logical_result_reference("SELECT * FROM @result JOIN @result_4 USING (value)"),
            Err("a cell can reference only one result source".to_string())
        );
    }
}
