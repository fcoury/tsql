//! Core syntax highlighting logic.

use std::collections::HashMap;

use ratatui::style::Style as RatatuiStyle;
use ratatui::text::{Line, Span};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

use crate::languages::Language;
use crate::theme::Theme;

/// Error during highlighting.
#[derive(Debug)]
pub enum HighlightError {
    /// Language not registered
    UnknownLanguage(String),
    /// Tree-sitter highlighting error
    Highlight(String),
    /// Language configuration error
    Config(String),
}

impl std::fmt::Display for HighlightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HighlightError::UnknownLanguage(name) => write!(f, "Unknown language: {}", name),
            HighlightError::Highlight(msg) => write!(f, "Highlight error: {}", msg),
            HighlightError::Config(msg) => write!(f, "Config error: {}", msg),
        }
    }
}

impl std::error::Error for HighlightError {}

/// Standard capture names used by tree-sitter highlight queries.
/// These are the names that themes should define styles for.
const CAPTURE_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.call",
    "function.macro",
    "function.method",
    "keyword",
    "keyword.control",
    "keyword.control.conditional",
    "keyword.control.import",
    "keyword.control.repeat",
    "keyword.control.return",
    "keyword.directive",
    "keyword.function",
    "keyword.operator",
    "keyword.special",
    "keyword.storage",
    "keyword.storage.modifier",
    "keyword.storage.type",
    "label",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "special",
    "string",
    "string.escape",
    "string.regexp",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// Configuration for a registered language.
struct LanguageConfig {
    config: HighlightConfiguration,
}

/// Syntax highlighter that produces ratatui-compatible styled text.
pub struct Highlighter {
    /// The theme to use for styling
    theme: Theme,
    /// Tree-sitter highlighter instance
    ts_highlighter: TsHighlighter,
    /// Registered languages
    languages: HashMap<String, LanguageConfig>,
}

impl Highlighter {
    /// Create a new highlighter with the given theme.
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            ts_highlighter: TsHighlighter::new(),
            languages: HashMap::new(),
        }
    }

    /// Register a language for highlighting.
    pub fn register_language(&mut self, language: Language) -> Result<(), HighlightError> {
        let mut config = HighlightConfiguration::new(
            language.ts_language,
            language.name,
            language.highlights_query,
            language.injections_query,
            language.locals_query,
        )
        .map_err(|e| HighlightError::Config(e.to_string()))?;

        // Configure the capture names
        config.configure(CAPTURE_NAMES);

        self.languages
            .insert(language.name.to_string(), LanguageConfig { config });

        Ok(())
    }

    /// Get the current theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Set a new theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Highlight source code and return styled lines.
    ///
    /// # Arguments
    /// * `language` - The language name (e.g., "sql")
    /// * `source` - The source code to highlight
    ///
    /// # Returns
    /// A vector of styled `Line`s, one per line in the source.
    pub fn highlight(
        &mut self,
        language: &str,
        source: &str,
    ) -> Result<Vec<Line<'static>>, HighlightError> {
        let lang_config = self
            .languages
            .get(language)
            .ok_or_else(|| HighlightError::UnknownLanguage(language.to_string()))?;

        let highlights = self
            .ts_highlighter
            .highlight(&lang_config.config, source.as_bytes(), None, |_| None)
            .map_err(|e| HighlightError::Highlight(e.to_string()))?;

        // Process highlight events into spans
        let mut spans: Vec<(usize, usize, RatatuiStyle)> = Vec::new();
        let mut style_stack: Vec<RatatuiStyle> = vec![RatatuiStyle::default()];

        for event in highlights {
            match event.map_err(|e| HighlightError::Highlight(e.to_string()))? {
                HighlightEvent::Source { start, end } => {
                    let current_style = *style_stack.last().unwrap_or(&RatatuiStyle::default());
                    spans.push((start, end, current_style));
                }
                HighlightEvent::HighlightStart(highlight) => {
                    let capture_name = CAPTURE_NAMES.get(highlight.0).copied().unwrap_or("text");
                    let style = self.theme.style_for(capture_name);
                    style_stack.push(style);
                }
                HighlightEvent::HighlightEnd => {
                    style_stack.pop();
                }
            }
        }

        // Convert spans to lines
        Ok(self.spans_to_lines(source, &spans))
    }

    /// Convert byte-indexed spans to line-based ratatui Lines.
    fn spans_to_lines(
        &self,
        source: &str,
        spans: &[(usize, usize, RatatuiStyle)],
    ) -> Vec<Line<'static>> {
        let lines: Vec<&str> = source.lines().collect();
        let mut result: Vec<Line<'static>> = Vec::with_capacity(lines.len());

        // Build byte offset to line mapping
        let mut line_starts: Vec<usize> = vec![0];
        for (i, c) in source.char_indices() {
            if c == '\n' {
                line_starts.push(i + 1);
            }
        }

        // If source doesn't end with newline, we still need to handle the last line
        if !source.ends_with('\n') && !source.is_empty() {
            // line_starts already has the start of the last line
        }

        for (line_idx, line_text) in lines.iter().enumerate() {
            let line_start = line_starts.get(line_idx).copied().unwrap_or(0);
            let line_end = line_start + line_text.len();

            let mut line_spans: Vec<Span<'static>> = Vec::new();
            let mut current_pos = line_start;

            // Find all spans that overlap with this line
            for &(span_start, span_end, style) in spans {
                // Skip spans before this line
                if span_end <= line_start {
                    continue;
                }
                // Stop at spans after this line
                if span_start >= line_end {
                    break;
                }

                // Clip span to line boundaries
                let clipped_start = span_start.max(line_start);
                let clipped_end = span_end.min(line_end);

                // Add unstyled text before this span if needed
                if clipped_start > current_pos {
                    let text = &source[current_pos..clipped_start];
                    line_spans.push(Span::raw(text.to_string()));
                }

                // Add the styled span
                if clipped_end > clipped_start {
                    let text = &source[clipped_start..clipped_end];
                    line_spans.push(Span::styled(text.to_string(), style));
                    current_pos = clipped_end;
                }
            }

            // Add any remaining unstyled text
            if current_pos < line_end {
                let text = &source[current_pos..line_end];
                line_spans.push(Span::raw(text.to_string()));
            }

            // Handle empty lines
            if line_spans.is_empty() {
                line_spans.push(Span::raw(String::new()));
            }

            result.push(Line::from(line_spans));
        }

        // Handle case where source is empty
        if result.is_empty() {
            result.push(Line::from(vec![Span::raw(String::new())]));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::{html, sql};
    use crate::themes;

    #[test]
    fn test_highlighter_creation() {
        let theme = themes::one_dark();
        let highlighter = Highlighter::new(theme);
        assert!(highlighter.languages.is_empty());
    }

    #[test]
    fn test_register_language() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);
        highlighter.register_language(sql()).unwrap();
        assert!(highlighter.languages.contains_key("sql"));
    }

    #[test]
    fn test_highlight_simple_sql() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);
        highlighter.register_language(sql()).unwrap();

        let lines = highlighter.highlight("sql", "SELECT * FROM users").unwrap();
        assert_eq!(lines.len(), 1);
        // Should have multiple spans with different styles
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_highlight_multiline_sql() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);
        highlighter.register_language(sql()).unwrap();

        let sql = "SELECT *\nFROM users\nWHERE id = 1";
        let lines = highlighter.highlight("sql", sql).unwrap();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_unknown_language_error() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);

        let result = highlighter.highlight("unknown", "some code");
        assert!(matches!(result, Err(HighlightError::UnknownLanguage(_))));
    }

    #[test]
    fn test_highlight_html() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);
        highlighter.register_language(html()).unwrap();

        let html_content = "<html><head><title>Test</title></head><body><p>Hello</p></body></html>";
        let lines = highlighter.highlight("html", html_content).unwrap();
        assert_eq!(lines.len(), 1);
        // Should have multiple spans with different styles
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_highlight_multiline_html() {
        let theme = themes::one_dark();
        let mut highlighter = Highlighter::new(theme);
        highlighter.register_language(html()).unwrap();

        let html_content = r#"<html>
<head>
    <title>Test</title>
</head>
<body>
    <p>Hello</p>
</body>
</html>"#;
        let lines = highlighter.highlight("html", html_content).unwrap();
        assert_eq!(lines.len(), 8);
    }
}
