//! Language definitions for syntax highlighting.
//!
//! Each language provides a tree-sitter grammar and highlight queries.

mod sql;

pub use sql::sql;

use tree_sitter::Language as TsLanguage;

/// Error configuring a language.
#[derive(Debug)]
pub enum LanguageError {
    /// Failed to create highlight configuration
    HighlightConfig(String),
}

impl std::fmt::Display for LanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LanguageError::HighlightConfig(msg) => write!(f, "Highlight config error: {}", msg),
        }
    }
}

impl std::error::Error for LanguageError {}

/// A language configuration for syntax highlighting.
pub struct Language {
    /// Language name (e.g., "sql", "rust", "python")
    pub name: &'static str,
    /// Tree-sitter language grammar
    pub ts_language: TsLanguage,
    /// Highlight queries (tree-sitter query syntax)
    pub highlights_query: &'static str,
    /// Injection queries (for embedded languages, optional)
    pub injections_query: &'static str,
    /// Locals queries (for local variable scoping, optional)
    pub locals_query: &'static str,
}
