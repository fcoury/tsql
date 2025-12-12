//! Highlighted editor widget that combines tui-textarea editing with tui-syntax highlighting.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use tui_syntax::{Highlighter, sql, themes};
use tui_textarea::TextArea;

/// A widget that renders a TextArea with syntax highlighting.
///
/// This widget:
/// 1. Takes pre-computed highlighted lines
/// 2. Overlays cursor position and selection from the TextArea
pub struct HighlightedTextArea<'a> {
    textarea: &'a TextArea<'a>,
    highlighted_lines: Vec<Line<'static>>,
    block: Option<Block<'a>>,
    cursor_style: Style,
    selection_style: Style,
}

impl<'a> HighlightedTextArea<'a> {
    pub fn new(textarea: &'a TextArea<'a>, highlighted_lines: Vec<Line<'static>>) -> Self {
        Self {
            textarea,
            highlighted_lines,
            block: None,
            cursor_style: Style::default().add_modifier(Modifier::REVERSED),
            selection_style: Style::default().bg(Color::Blue),
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn cursor_style(mut self, style: Style) -> Self {
        self.cursor_style = style;
        self
    }

    pub fn selection_style(mut self, style: Style) -> Self {
        self.selection_style = style;
        self
    }
}

impl Widget for HighlightedTextArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Get the inner area (accounting for block borders)
        let inner_area = if let Some(ref block) = self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        if inner_area.width == 0 || inner_area.height == 0 {
            return;
        }

        // Get cursor position and selection from textarea
        let (cursor_row, cursor_col) = self.textarea.cursor();
        let selection = self.textarea.selection_range();

        // Build the final lines with cursor and selection applied
        let mut final_lines: Vec<Line<'static>> = Vec::with_capacity(self.highlighted_lines.len());

        for (row_idx, line) in self.highlighted_lines.into_iter().enumerate() {
            let is_cursor_line = row_idx == cursor_row;

            // Convert Line to mutable spans for manipulation
            let mut line_spans: Vec<Span<'static>> = line.spans;

            // Apply selection highlighting if this line is in the selection range
            if let Some(((start_row, start_col), (end_row, end_col))) = selection {
                if row_idx >= start_row && row_idx <= end_row {
                    line_spans = apply_selection_to_spans(
                        line_spans,
                        row_idx,
                        start_row,
                        start_col,
                        end_row,
                        end_col,
                        self.selection_style,
                    );
                }
            }

            // Apply cursor highlighting
            if is_cursor_line {
                line_spans =
                    apply_cursor_to_spans(line_spans, cursor_col, self.cursor_style);
            }

            let result_line = Line::from(line_spans);
            final_lines.push(result_line);
        }

        // Handle empty editor - show cursor on empty line
        if final_lines.is_empty() {
            let cursor_span = Span::styled(" ", self.cursor_style);
            final_lines.push(Line::from(vec![cursor_span]));
        }

        // Render the highlighted text as a Paragraph
        let paragraph = Paragraph::new(final_lines);
        paragraph.render(inner_area, buf);
    }
}

/// Apply selection highlighting to spans in a line.
fn apply_selection_to_spans(
    spans: Vec<Span<'static>>,
    row_idx: usize,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
    selection_style: Style,
) -> Vec<Span<'static>> {
    // Determine the column range for selection on this line
    let line_start = if row_idx == start_row { start_col } else { 0 };
    let line_end = if row_idx == end_row {
        end_col
    } else {
        usize::MAX
    };

    apply_style_to_range(spans, line_start, line_end, selection_style)
}

/// Apply cursor highlighting to spans at a specific column.
fn apply_cursor_to_spans(
    spans: Vec<Span<'static>>,
    cursor_col: usize,
    cursor_style: Style,
) -> Vec<Span<'static>> {
    // Apply cursor style to a single character at cursor_col
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut current_col = 0;
    let mut cursor_applied = false;

    for span in spans {
        let span_text: String = span.content.to_string();
        let span_len = span_text.chars().count();
        let span_end = current_col + span_len;

        if !cursor_applied && cursor_col >= current_col && cursor_col < span_end {
            // Cursor is in this span
            let char_offset = cursor_col - current_col;
            let chars: Vec<char> = span_text.chars().collect();

            // Before cursor
            if char_offset > 0 {
                let before: String = chars[..char_offset].iter().collect();
                result.push(Span::styled(before, span.style));
            }

            // Cursor character
            if char_offset < chars.len() {
                let cursor_char: String = chars[char_offset..char_offset + 1].iter().collect();
                result.push(Span::styled(cursor_char, cursor_style));
            } else {
                // Cursor at end of span - add a space as cursor
                result.push(Span::styled(" ", cursor_style));
            }

            // After cursor
            if char_offset + 1 < chars.len() {
                let after: String = chars[char_offset + 1..].iter().collect();
                result.push(Span::styled(after, span.style));
            }

            cursor_applied = true;
        } else {
            result.push(span);
        }

        current_col = span_end;
    }

    // If cursor is past all spans (at end of line), add a cursor space
    if !cursor_applied {
        result.push(Span::styled(" ", cursor_style));
    }

    result
}

/// Apply a style to a range of columns within spans.
fn apply_style_to_range(
    spans: Vec<Span<'static>>,
    start_col: usize,
    end_col: usize,
    style: Style,
) -> Vec<Span<'static>> {
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut current_col = 0;

    for span in spans {
        let span_text: String = span.content.to_string();
        let span_len = span_text.chars().count();
        let span_end = current_col + span_len;

        if span_end <= start_col || current_col >= end_col {
            // Span is completely outside the selection range
            result.push(span);
        } else if current_col >= start_col && span_end <= end_col {
            // Span is completely inside the selection range
            result.push(Span::styled(span_text, style));
        } else {
            // Span partially overlaps with selection
            let chars: Vec<char> = span_text.chars().collect();

            // Part before selection
            if current_col < start_col {
                let before_end = start_col - current_col;
                let before: String = chars[..before_end].iter().collect();
                result.push(Span::styled(before, span.style));
            }

            // Selected part
            let sel_start = start_col.saturating_sub(current_col);
            let sel_end = (end_col - current_col).min(chars.len());
            if sel_start < sel_end {
                let selected: String = chars[sel_start..sel_end].iter().collect();
                result.push(Span::styled(selected, style));
            }

            // Part after selection
            if span_end > end_col {
                let after_start = end_col - current_col;
                let after: String = chars[after_start..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }

        current_col = span_end;
    }

    result
}

/// Creates a pre-configured highlighter for SQL.
pub fn create_sql_highlighter() -> Highlighter {
    let mut highlighter = Highlighter::new(themes::one_dark());
    // Ignore errors - SQL should always register successfully
    let _ = highlighter.register_language(sql());
    highlighter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_sql_highlighter() {
        let _highlighter = create_sql_highlighter();
        // Just verify it creates without panicking
    }

    #[test]
    fn test_apply_cursor_to_spans() {
        let spans = vec![Span::raw("SELECT")];
        let cursor_style = Style::default().add_modifier(Modifier::REVERSED);

        let result = apply_cursor_to_spans(spans, 0, cursor_style);

        // First char should have cursor style
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content.as_ref(), "S");
        assert_eq!(result[1].content.as_ref(), "ELECT");
    }

    #[test]
    fn test_apply_cursor_at_end() {
        let spans = vec![Span::raw("SELECT")];
        let cursor_style = Style::default().add_modifier(Modifier::REVERSED);

        let result = apply_cursor_to_spans(spans, 6, cursor_style);

        // Cursor at end should add a space
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content.as_ref(), "SELECT");
        assert_eq!(result[1].content.as_ref(), " ");
    }
}
