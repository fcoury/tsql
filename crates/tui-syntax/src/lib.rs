//! # tui-syntax
//!
//! Tree-sitter based syntax highlighting for TUI applications.
//!
//! This crate provides syntax highlighting that integrates with [ratatui](https://ratatui.rs),
//! returning styled `Line` and `Span` types ready for rendering.
//!
//! ## Features
//!
//! - Tree-sitter based highlighting (accurate, fast)
//! - Helix-compatible TOML theme format
//! - Built-in themes (One Dark, GitHub Light)
//! - SQL language support built-in
//! - Extensible to other languages via tree-sitter grammars
//!
//! ## Example
//!
//! ```rust
//! use tui_syntax::{Highlighter, themes, sql};
//!
//! // Create highlighter with default dark theme
//! let mut highlighter = Highlighter::new(themes::one_dark());
//!
//! // Register SQL language
//! highlighter.register_language(sql()).unwrap();
//!
//! // Highlight some SQL
//! let lines = highlighter.highlight("sql", "SELECT * FROM users WHERE id = 1;").unwrap();
//!
//! // `lines` is Vec<ratatui::text::Line> ready to render
//! ```

mod highlighter;
mod theme;
pub mod languages;
pub mod themes;

pub use highlighter::{Highlighter, HighlightError};
pub use theme::{Theme, ThemeError, Style as ThemeStyle, StyleModifier};
pub use languages::{Language, LanguageError, sql};
