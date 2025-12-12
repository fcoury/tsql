//! JSON language support using tree-sitter-json.

use super::Language;

/// Returns the JSON language configuration.
///
/// Uses the tree-sitter-json grammar for JSON syntax highlighting.
pub fn json() -> Language {
    Language {
        name: "json",
        ts_language: tree_sitter_json::LANGUAGE.into(),
        highlights_query: tree_sitter_json::HIGHLIGHTS_QUERY,
        injections_query: "",
        locals_query: "",
    }
}
