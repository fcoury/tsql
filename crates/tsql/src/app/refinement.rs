//! Database-neutral retained-result contract for notebook refinement.

use super::execution::{CellId, ExecutionId};
use super::sql_lexer::{self, SqlSegmentKind};

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LogicalResultReference {
    Latest,
    Cell(CellId),
    Named(String),
}

/// Maps PostgreSQL character positions in expanded notebook SQL back to the cell source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SqlSourceMap {
    source: String,
    segments: Vec<SqlSourceMapSegment>,
    compiled_chars: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqlSourceMapSegment {
    compiled_start: usize,
    compiled_end: usize,
    source_start: usize,
    source_end: usize,
    replacement: bool,
}

impl SqlSourceMap {
    pub(crate) fn identity(source: &str) -> Self {
        let chars = source.chars().count();
        Self {
            source: source.to_string(),
            segments: vec![SqlSourceMapSegment {
                compiled_start: 0,
                compiled_end: chars,
                source_start: 0,
                source_end: chars,
                replacement: false,
            }],
            compiled_chars: chars,
        }
    }

    /// Returns the original one-based position, line, and column for a generated position.
    ///
    /// `generated_prefix` is the SQL inserted immediately before the expanded query, such as
    /// the CTAS or preview-cursor wrapper. Positions within generated substitutions point to
    /// the beginning of the corresponding logical result reference.
    pub(crate) fn map_position(
        &self,
        generated_position: u32,
        generated_prefix: &str,
    ) -> Option<(u32, usize, usize)> {
        let prefix_chars = generated_prefix.chars().count();
        let compiled_offset = usize::try_from(generated_position)
            .ok()?
            .checked_sub(prefix_chars.checked_add(1)?)?;
        if compiled_offset > self.compiled_chars {
            return None;
        }

        let source_offset = self
            .segments
            .iter()
            .find(|segment| {
                compiled_offset >= segment.compiled_start && compiled_offset < segment.compiled_end
            })
            .map(|segment| {
                if segment.replacement {
                    segment.source_start
                } else {
                    (segment.source_start + compiled_offset - segment.compiled_start)
                        .min(segment.source_end)
                }
            })
            .unwrap_or_else(|| self.source.chars().count());
        let source_position = u32::try_from(source_offset.checked_add(1)?).ok()?;
        let (line, column) = self.source.chars().take(source_offset).fold(
            (1usize, 1usize),
            |(line, column), character| {
                if character == '\n' {
                    (line + 1, 1)
                } else {
                    (line, column + 1)
                }
            },
        );
        Some((source_position, line, column))
    }
}

pub(crate) struct CompiledLogicalSql {
    pub(crate) sql: String,
    pub(crate) source_map: SqlSourceMap,
}

fn visit_logical_result_references(
    source: &str,
    mut visit: impl FnMut(&str, usize, usize) -> Result<(), String>,
) -> Result<(), String> {
    for segment in sql_lexer::scan(source)? {
        if segment.kind != SqlSegmentKind::Code {
            continue;
        }
        let mut index = segment.range.start;
        while index < segment.range.end {
            let Some(offset) = source[index..segment.range.end].find("@result") else {
                break;
            };
            let start = index + offset;
            index = start + "@result".len();
            if source[..start]
                .chars()
                .next_back()
                .is_some_and(|character| {
                    character.is_alphanumeric() || character == '_' || character == '$'
                })
            {
                continue;
            }
            while index < segment.range.end {
                let Some(character) = source[index..].chars().next() else {
                    break;
                };
                if !character.is_alphanumeric() && character != '_' && character != '$' {
                    break;
                }
                index += character.len_utf8();
            }
            visit(&source[start..index], start, index)?;
        }
    }

    Ok(())
}

/// Returns all distinct structural result references used by a notebook query.
pub(crate) fn logical_result_references(
    source: &str,
) -> Result<Vec<LogicalResultReference>, String> {
    let mut found = Vec::new();
    visit_logical_result_references(source, |reference, _, _| {
        let current = parse_logical_reference(reference)?;

        if !found.contains(&current) {
            found.push(current);
        }
        Ok(())
    })?;
    Ok(found)
}

/// Returns the single structural result reference used by a notebook query.
pub(crate) fn logical_result_reference(
    source: &str,
) -> Result<Option<LogicalResultReference>, String> {
    let references = logical_result_references(source)?;
    match references.as_slice() {
        [] => Ok(None),
        [reference] => Ok(Some(reference.clone())),
        _ => Err("a cell can reference only one result source".to_string()),
    }
}

/// Compiles one structurally bound logical result reference outside SQL literals/comments.
#[cfg(test)]
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
        Some(LogicalResultReference::Named(_)) => {
            return Err("named result references require an explicit binding".to_string());
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
    let relation = format!(
        "(SELECT {columns} FROM pg_temp.\"{}\")",
        physical_name.replace('"', "\"\"")
    );
    let generated_alias = format!(" AS \"__tsql_result_{}\"", version.source_cell.0);
    let mut compiled = String::with_capacity(source.len() + relation.len() + generated_alias.len());
    let mut copied_until = 0;
    visit_logical_result_references(source, |_, start, end| {
        compiled.push_str(&source[copied_until..start]);
        compiled.push_str(&relation);
        if !has_user_alias(source, end)? {
            compiled.push_str(&generated_alias);
        }
        copied_until = end;
        Ok(())
    })?;
    compiled.push_str(&source[copied_until..]);

    Ok(compiled)
}

/// Compiles multiple structurally bound references for dependency joins.
#[cfg(test)]
pub(crate) fn compile_logical_references(
    source: &str,
    bindings: &[(LogicalResultReference, ResultVersion, String, Vec<String>)],
) -> Result<String, String> {
    compile_logical_references_mapped(source, bindings).map(|compiled| compiled.sql)
}

/// Compiles structural references and records the source locations hidden by each expansion.
pub(crate) fn compile_logical_references_mapped(
    source: &str,
    bindings: &[(LogicalResultReference, ResultVersion, String, Vec<String>)],
) -> Result<CompiledLogicalSql, String> {
    let mut compiled = String::with_capacity(source.len());
    let mut copied_until = 0;
    let mut compiled_chars = 0;
    let mut segments = Vec::new();
    visit_logical_result_references(source, |token, start, end| {
        let reference = parse_logical_reference(token)?;
        let (_, version, physical_name, public_columns) = bindings
            .iter()
            .find(|(candidate, _, _, _)| candidate == &reference)
            .ok_or_else(|| format!("logical result reference is not bound: {token}"))?;
        if matches!(&reference, LogicalResultReference::Cell(cell) if *cell != version.source_cell)
        {
            return Err("logical result reference does not match cell dependency".to_string());
        }
        let columns = public_columns
            .iter()
            .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", ");
        let copied = &source[copied_until..start];
        let copied_chars = copied.chars().count();
        let source_start = source[..copied_until].chars().count();
        let reference_start = source[..start].chars().count();
        let reference_end = source[..end].chars().count();
        if copied_chars > 0 {
            segments.push(SqlSourceMapSegment {
                compiled_start: compiled_chars,
                compiled_end: compiled_chars + copied_chars,
                source_start,
                source_end: reference_start,
                replacement: false,
            });
        }
        compiled.push_str(copied);
        compiled_chars += copied_chars;

        let mut replacement = format!(
            "(SELECT {columns} FROM pg_temp.\"{}\")",
            physical_name.replace('"', "\"\"")
        );
        if !has_user_alias(source, end)? {
            replacement.push_str(&format!(" AS \"__tsql_result_{}\"", version.source_cell.0));
        }
        let replacement_chars = replacement.chars().count();
        compiled.push_str(&replacement);
        segments.push(SqlSourceMapSegment {
            compiled_start: compiled_chars,
            compiled_end: compiled_chars + replacement_chars,
            source_start: reference_start,
            source_end: reference_end,
            replacement: true,
        });
        compiled_chars += replacement_chars;
        copied_until = end;
        Ok(())
    })?;
    let copied = &source[copied_until..];
    let copied_chars = copied.chars().count();
    let source_start = source[..copied_until].chars().count();
    let source_end = source.chars().count();
    compiled.push_str(copied);
    if copied_chars > 0 {
        segments.push(SqlSourceMapSegment {
            compiled_start: compiled_chars,
            compiled_end: compiled_chars + copied_chars,
            source_start,
            source_end,
            replacement: false,
        });
    }
    compiled_chars += copied_chars;
    Ok(CompiledLogicalSql {
        sql: compiled,
        source_map: SqlSourceMap {
            source: source.to_string(),
            segments,
            compiled_chars,
        },
    })
}

fn parse_logical_reference(reference: &str) -> Result<LogicalResultReference, String> {
    if reference == "@result" {
        return Ok(LogicalResultReference::Latest);
    }
    if let Some(cell) = reference.strip_prefix("@result_") {
        if cell.chars().all(|character| character.is_ascii_digit()) {
            let id = cell
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| format!("invalid logical result reference: {reference}"))?;
            return Ok(LogicalResultReference::Cell(CellId(id)));
        }
        let name = normalize_result_name(cell)
            .ok_or_else(|| format!("invalid logical result reference: {reference}"))?;
        return Ok(LogicalResultReference::Named(name));
    }
    Err(format!("invalid logical result reference: {reference}"))
}

/// Normalizes a user-facing result name to a stable SQL-safe token suffix.
pub(crate) fn normalize_result_name(name: &str) -> Option<String> {
    let name = name.trim();
    let mut characters = name.chars();
    let first = characters.next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    Some(name.to_ascii_lowercase())
}

fn has_user_alias(source: &str, reference_end: usize) -> Result<bool, String> {
    let suffix = &source[reference_end..];
    for segment in sql_lexer::scan(suffix)? {
        match segment.kind {
            SqlSegmentKind::LineComment | SqlSegmentKind::BlockComment => continue,
            SqlSegmentKind::DoubleQuoted => return Ok(true),
            SqlSegmentKind::Code => {
                let code = suffix[segment.range].trim_start();
                if code.is_empty() {
                    continue;
                }
                let Some(first) = code.chars().next() else {
                    continue;
                };
                if !first.is_ascii_alphabetic() && first != '_' {
                    return Ok(false);
                }
                let word = code
                    .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                return Ok(!matches!(
                    word.as_str(),
                    "WHERE"
                        | "GROUP"
                        | "ORDER"
                        | "LIMIT"
                        | "OFFSET"
                        | "FETCH"
                        | "FOR"
                        | "JOIN"
                        | "INNER"
                        | "LEFT"
                        | "RIGHT"
                        | "FULL"
                        | "CROSS"
                        | "NATURAL"
                        | "ON"
                        | "USING"
                        | "UNION"
                        | "INTERSECT"
                        | "EXCEPT"
                        | "WINDOW"
                        | "HAVING"
                        | "QUALIFY"
                        | "RETURNING"
                ));
            }
            _ => return Ok(false),
        }
    }
    Ok(false)
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
            logical_result_reference("SELECT * FROM @result_Recent_Users"),
            Ok(Some(LogicalResultReference::Named(
                "recent_users".to_string()
            )))
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
        assert_eq!(
            logical_result_reference("SELECT E'it\\'s @result_1; -- literal' AS note"),
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
        assert_eq!(
            logical_result_references(
                "SELECT * FROM @result_1 a JOIN @result_2 b USING (value) JOIN @result_1 c USING (value)"
            ),
            Ok(vec![
                LogicalResultReference::Cell(CellId(1)),
                LogicalResultReference::Cell(CellId(2))
            ])
        );
    }

    #[test]
    fn preserves_explicit_shorthand_and_quoted_aliases() {
        let version = ResultVersion {
            source_cell: CellId(2),
            source_execution: ExecutionId(9),
            source_revision: 1,
        };

        assert_eq!(
            compile_logical_reference(
                "SELECT a.value, b.value FROM @result_2 AS a JOIN @result_2 b USING (value)",
                version,
                "snapshot_2",
                &["value".to_string()]
            ),
            Ok("SELECT a.value, b.value FROM (SELECT \"value\" FROM pg_temp.\"snapshot_2\") AS a JOIN (SELECT \"value\" FROM pg_temp.\"snapshot_2\") b USING (value)".to_string())
        );
        assert_eq!(
            compile_logical_reference(
                "SELECT r.value FROM @result_2 /* alias */ \"r\" WHERE r.value > 1",
                version,
                "snapshot_2",
                &["value".to_string()]
            ),
            Ok("SELECT r.value FROM (SELECT \"value\" FROM pg_temp.\"snapshot_2\") /* alias */ \"r\" WHERE r.value > 1".to_string())
        );
    }

    #[test]
    fn compiles_multiple_numbered_sources_for_a_join() {
        let first = ResultVersion {
            source_cell: CellId(1),
            source_execution: ExecutionId(11),
            source_revision: 2,
        };
        let second = ResultVersion {
            source_cell: CellId(2),
            source_execution: ExecutionId(12),
            source_revision: 3,
        };
        let bindings = vec![
            (
                LogicalResultReference::Cell(CellId(1)),
                first,
                "snapshot_1".to_string(),
                vec!["value".to_string()],
            ),
            (
                LogicalResultReference::Cell(CellId(2)),
                second,
                "snapshot_2".to_string(),
                vec!["value".to_string()],
            ),
        ];

        assert_eq!(
            compile_logical_references(
                "SELECT a.value, b.value FROM @result_1 AS a JOIN @result_2 b USING (value)",
                &bindings
            ),
            Ok("SELECT a.value, b.value FROM (SELECT \"value\" FROM pg_temp.\"snapshot_1\") AS a JOIN (SELECT \"value\" FROM pg_temp.\"snapshot_2\") b USING (value)".to_string())
        );
        assert_eq!(
            compile_logical_references("SELECT * FROM @result_3", &bindings),
            Err("logical result reference is not bound: @result_3".to_string())
        );

        let named = vec![(
            LogicalResultReference::Named("recent_users".to_string()),
            first,
            "snapshot_1".to_string(),
            vec!["value".to_string()],
        )];
        assert_eq!(
            compile_logical_references("SELECT r.value FROM @result_recent_users AS r", &named),
            Ok(
                "SELECT r.value FROM (SELECT \"value\" FROM pg_temp.\"snapshot_1\") AS r"
                    .to_string()
            )
        );
    }

    #[test]
    fn maps_expanded_positions_back_to_unicode_cell_source_and_wrappers() {
        let source = "  SELECT 'é' AS note\nFROM @result_1\nWHERE missing_value = 1";
        let binding = vec![(
            LogicalResultReference::Cell(CellId(1)),
            ResultVersion {
                source_cell: CellId(1),
                source_execution: ExecutionId(7),
                source_revision: 2,
            },
            "snapshot_1".to_string(),
            vec!["value".to_string()],
        )];
        let compiled = compile_logical_references_mapped(source, &binding).unwrap();
        let generated_position = u32::try_from(
            compiled.sql[..compiled.sql.find("missing_value").unwrap()]
                .chars()
                .count()
                + 1,
        )
        .unwrap();
        let source_position = u32::try_from(
            source[..source.find("missing_value").unwrap()]
                .chars()
                .count()
                + 1,
        )
        .unwrap();
        assert_eq!(
            compiled.source_map.map_position(generated_position, ""),
            Some((source_position, 3, 7))
        );

        let relation_position = u32::try_from(
            compiled.sql[..compiled.sql.find("snapshot_1").unwrap()]
                .chars()
                .count()
                + 1,
        )
        .unwrap();
        let reference_position =
            u32::try_from(source[..source.find("@result_1").unwrap()].chars().count() + 1).unwrap();
        assert_eq!(
            compiled.source_map.map_position(relation_position, ""),
            Some((reference_position, 2, 6))
        );

        let wrapper = "CREATE TEMP TABLE generated AS SELECT * FROM (\n";
        let wrapped_position = generated_position + u32::try_from(wrapper.chars().count()).unwrap();
        assert_eq!(
            compiled.source_map.map_position(wrapped_position, wrapper),
            Some((source_position, 3, 7))
        );
        assert_eq!(compiled.source_map.map_position(1, wrapper), None);
    }

    #[test]
    fn identity_source_map_uses_postgres_character_positions() {
        let source = "SELECT 'é' AS note\nWHERE broken";
        let position =
            u32::try_from(source[..source.find("broken").unwrap()].chars().count() + 1).unwrap();
        assert_eq!(
            SqlSourceMap::identity(source).map_position(position, ""),
            Some((position, 2, 7))
        );
    }
}
