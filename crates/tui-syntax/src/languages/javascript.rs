//! JavaScript language support using tree-sitter-javascript.

use super::Language;

/// Returns the JavaScript language configuration.
///
/// Mongo shell queries use JavaScript syntax, including method calls,
/// object literals, comments, and template strings.
pub fn javascript() -> Language {
    Language {
        name: "javascript",
        ts_language: tree_sitter_javascript::LANGUAGE.into(),
        highlights_query: tree_sitter_javascript::HIGHLIGHT_QUERY,
        injections_query: "",
        locals_query: tree_sitter_javascript::LOCALS_QUERY,
    }
}
