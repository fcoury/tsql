//! Highlighted editor widget that combines tui-textarea editing with tui-syntax highlighting.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use tui_syntax::{sql, themes, Highlighter};
use tui_textarea::TextArea;

/// The shape of the cursor to display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// Block cursor (full character highlight, default for normal mode)
    /// This is rendered by highlighting the character under the cursor.
    #[default]
    Block,
    /// Bar cursor (thin vertical line before character, default for insert mode)
    /// This should use the terminal's native cursor via frame.set_cursor_position().
    Bar,
    /// Underline cursor (line under the character)
    /// This should use the terminal's native cursor via frame.set_cursor_position().
    Underline,
}

/// A widget that renders a TextArea with syntax highlighting.
///
/// This widget:
/// 1. Takes pre-computed highlighted lines
/// 2. Overlays cursor position and selection from the TextArea
/// 3. Supports horizontal and vertical scrolling to keep cursor visible
pub struct HighlightedTextArea<'a> {
    textarea: &'a TextArea<'a>,
    highlighted_lines: Vec<Line<'static>>,
    block: Option<Block<'a>>,
    cursor_style: Style,
    selection_style: Style,
    /// Current scroll offset (row, col). Updated during render.
    scroll_offset: (u16, u16),
    /// Whether to show the cursor. Defaults to true.
    show_cursor: bool,
    /// The shape of the cursor. Defaults to Block.
    cursor_shape: CursorShape,
}

impl<'a> HighlightedTextArea<'a> {
    pub fn new(textarea: &'a TextArea<'a>, highlighted_lines: Vec<Line<'static>>) -> Self {
        Self {
            textarea,
            highlighted_lines,
            block: None,
            cursor_style: Style::default().add_modifier(Modifier::REVERSED),
            selection_style: Style::default().bg(Color::Blue),
            scroll_offset: (0, 0),
            show_cursor: true,
            cursor_shape: CursorShape::Block,
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

    /// Set the scroll offset (row, col).
    pub fn scroll(mut self, offset: (u16, u16)) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Set whether to show the cursor. Defaults to true.
    pub fn show_cursor(mut self, show: bool) -> Self {
        self.show_cursor = show;
        self
    }

    /// Set the cursor shape. Defaults to Block.
    pub fn cursor_shape(mut self, shape: CursorShape) -> Self {
        self.cursor_shape = shape;
        self
    }

    /// Calculate the screen position of the cursor for use with frame.set_cursor_position().
    /// Returns None if the cursor is not visible (scrolled out of view or show_cursor is false).
    /// The returned position is relative to the widget's area (including block borders).
    pub fn cursor_screen_position(&self, area: Rect) -> Option<Position> {
        if !self.show_cursor {
            return None;
        }

        // Get the inner area (accounting for block borders)
        let inner_area = if let Some(ref block) = self.block {
            block.inner(area)
        } else {
            area
        };

        if inner_area.width == 0 || inner_area.height == 0 {
            return None;
        }

        let (cursor_row, cursor_col) = self.textarea.cursor();

        // Calculate scroll offset
        let (scroll_row, scroll_col) = calculate_scroll_offset(
            cursor_row,
            cursor_col,
            self.scroll_offset,
            inner_area.height as usize,
            inner_area.width as usize,
        );

        // Calculate cursor position on screen
        let screen_row = cursor_row.saturating_sub(scroll_row);
        let screen_col = cursor_col.saturating_sub(scroll_col);

        // Check if cursor is within visible area
        if screen_row >= inner_area.height as usize || screen_col >= inner_area.width as usize {
            return None;
        }

        Some(Position {
            x: inner_area.x + screen_col as u16,
            y: inner_area.y + screen_row as u16,
        })
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

        // Calculate scroll offset to keep cursor visible
        let (scroll_row, scroll_col) = calculate_scroll_offset(
            cursor_row,
            cursor_col,
            self.scroll_offset,
            inner_area.height as usize,
            inner_area.width as usize,
        );

        // Get the actual number of lines in the textarea (may differ from highlighted_lines)
        let textarea_line_count = self.textarea.lines().len();
        let highlighted_line_count = self.highlighted_lines.len();
        let total_lines = textarea_line_count.max(highlighted_line_count).max(1);

        // Convert highlighted_lines into a vec we can index and take from
        let mut highlighted_lines = self.highlighted_lines;

        // Pad with empty lines if textarea has more lines than highlighted
        while highlighted_lines.len() < total_lines {
            highlighted_lines.push(Line::from(vec![]));
        }

        // Build the final lines with cursor and selection applied
        let mut final_lines: Vec<Line<'static>> = Vec::with_capacity(total_lines);

        for (row_idx, line) in highlighted_lines.into_iter().enumerate() {
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

            // Apply cursor highlighting only for Block cursor shape
            // Bar and Underline cursors use the terminal's native cursor
            if is_cursor_line && self.show_cursor && self.cursor_shape == CursorShape::Block {
                line_spans = apply_cursor_to_spans(line_spans, cursor_col, self.cursor_style);
            }

            let result_line = Line::from(line_spans);
            final_lines.push(result_line);
        }

        // Handle completely empty editor - show block cursor on empty line (only if cursor is visible)
        // Bar and Underline cursors use the terminal's native cursor
        if final_lines.is_empty() {
            if self.show_cursor && self.cursor_shape == CursorShape::Block {
                let cursor_span = Span::styled(" ", self.cursor_style);
                final_lines.push(Line::from(vec![cursor_span]));
            } else {
                final_lines.push(Line::from(vec![]));
            }
        }

        // Render the highlighted text as a Paragraph with scroll offset
        let paragraph = Paragraph::new(final_lines).scroll((scroll_row as u16, scroll_col as u16));
        paragraph.render(inner_area, buf);
    }
}

/// Calculate the scroll offset needed to keep cursor visible in the viewport.
fn calculate_scroll_offset(
    cursor_row: usize,
    cursor_col: usize,
    current_scroll: (u16, u16),
    viewport_height: usize,
    viewport_width: usize,
) -> (usize, usize) {
    let (mut scroll_row, mut scroll_col) = (current_scroll.0 as usize, current_scroll.1 as usize);

    // Vertical scrolling
    if viewport_height > 0 {
        // If cursor is above the viewport, scroll up
        if cursor_row < scroll_row {
            scroll_row = cursor_row;
        }
        // If cursor is below the viewport, scroll down
        let viewport_bottom = scroll_row + viewport_height;
        if cursor_row >= viewport_bottom {
            scroll_row = cursor_row.saturating_sub(viewport_height - 1);
        }
    }

    // Horizontal scrolling
    if viewport_width > 0 {
        // Leave some margin (3 chars) for context
        let margin = 3.min(viewport_width / 4);

        // If cursor is left of the viewport, scroll left
        if cursor_col < scroll_col + margin {
            scroll_col = cursor_col.saturating_sub(margin);
        }
        // If cursor is right of the viewport, scroll right
        let viewport_right = scroll_col + viewport_width;
        if cursor_col + margin >= viewport_right {
            scroll_col = (cursor_col + margin).saturating_sub(viewport_width - 1);
        }
    }

    (scroll_row, scroll_col)
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

/// Apply block cursor highlighting to spans at a specific column.
/// This highlights the character at the cursor position with the cursor style.
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

            // Block cursor: apply style to the character
            if char_offset < chars.len() {
                let cursor_char: String = chars[char_offset..char_offset + 1].iter().collect();
                result.push(Span::styled(cursor_char, cursor_style));
            } else {
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

    // If cursor is past all spans (at end of line), add a block cursor space
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

    #[test]
    fn test_apply_cursor_to_empty_spans() {
        // This simulates the bug: cursor on an empty line (e.g., after pressing Enter)
        let spans: Vec<Span<'static>> = vec![];
        let cursor_style = Style::default().add_modifier(Modifier::REVERSED);

        let result = apply_cursor_to_spans(spans, 0, cursor_style);

        // Should have a cursor space even with empty spans
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content.as_ref(), " ");
        assert!(result[0].style.add_modifier == Modifier::REVERSED);
    }

    #[test]
    fn test_cursor_on_new_line_after_text() {
        // Simulates: "SELECT\n" with cursor on line 1, col 0
        // highlighted_lines would be ["SELECT"] but cursor is on row 1
        // This test verifies our fix handles this case

        let mut textarea = TextArea::new(vec!["SELECT".to_string(), "".to_string()]);
        // Move cursor to second line
        textarea.move_cursor(tui_textarea::CursorMove::Down);

        let (cursor_row, cursor_col) = textarea.cursor();
        assert_eq!(cursor_row, 1, "cursor should be on row 1");
        assert_eq!(cursor_col, 0, "cursor should be at column 0");

        // Highlighted lines might only have one line if the second is empty
        let highlighted_lines = [Line::from("SELECT")];

        // The number of highlighted lines (1) is less than cursor_row + 1 (2)
        // This is the bug condition
        assert!(
            highlighted_lines.len() <= cursor_row,
            "Bug condition: highlighted_lines.len()={} <= cursor_row={}",
            highlighted_lines.len(),
            cursor_row
        );
    }

    #[test]
    fn test_widget_renders_cursor_on_new_line() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::Widget;

        // Create a textarea with two lines, cursor on second (empty) line
        let mut textarea = TextArea::new(vec!["SELECT".to_string(), "".to_string()]);
        textarea.move_cursor(tui_textarea::CursorMove::Down);

        // Only provide one highlighted line (simulating the bug condition)
        let highlighted_lines = vec![Line::from("SELECT")];

        let widget = HighlightedTextArea::new(&textarea, highlighted_lines);

        // Render to a buffer
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // The cursor should be visible on line 1 (second line)
        // Check that something is rendered on the second line at position (0, 1)
        let cell = buf.cell((0, 1)).unwrap();

        // The cursor should be a space with reversed style (cursor on empty line)
        assert_eq!(
            cell.symbol(),
            " ",
            "Cursor should render as space on empty line"
        );
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "Cursor should have REVERSED modifier"
        );
    }
}
