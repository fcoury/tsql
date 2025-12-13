//! HTML language support using tree-sitter-html.

use super::Language;

/// Returns the HTML language configuration.
///
/// Uses the tree-sitter-html grammar for HTML syntax highlighting.
pub fn html() -> Language {
    Language {
        name: "html",
        ts_language: tree_sitter_html::LANGUAGE.into(),
        highlights_query: tree_sitter_html::HIGHLIGHTS_QUERY,
        injections_query: tree_sitter_html::INJECTIONS_QUERY,
        locals_query: "",
    }
}
