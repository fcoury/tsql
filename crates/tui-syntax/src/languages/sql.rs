//! SQL language support using tree-sitter-sequel.

use super::Language;

/// Returns the SQL language configuration.
///
/// Uses the tree-sitter-sequel grammar which supports PostgreSQL syntax.
pub fn sql() -> Language {
    Language {
        name: "sql",
        ts_language: tree_sitter_sequel::LANGUAGE.into(),
        highlights_query: tree_sitter_sequel::HIGHLIGHTS_QUERY,
        injections_query: "",
        locals_query: "",
    }
}
