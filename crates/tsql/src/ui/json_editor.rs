//! JSON editor modal for editing JSON/JSONB cell values.
//!
//! This modal provides:
//! - Syntax-highlighted JSON editing
//! - Vim-like keybindings (Normal/Insert/Visual modes) via unified VimHandler
//! - JSON validation with error display
//! - Auto-formatting on open
//! - Virtual scrolling for large JSON

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use tui_textarea::{CursorMove, TextArea};

use tui_syntax::{json, themes, Highlighter};

use crate::ui::HighlightedTextArea;
use crate::util::{is_json_column_type, is_valid_json, try_format_json};
use crate::vim::{Motion, VimCommand, VimConfig, VimHandler, VimMode};

/// The result of handling a key event in the JSON editor.
pub enum JsonEditorAction {
    /// Continue editing, nothing special happened.
    Continue,
    /// Save the value and close the editor.
    Save {
        value: String,
        row: usize,
        col: usize,
    },
    /// Cancel editing and close the editor.
    Cancel,
    /// Show an error message (e.g., invalid JSON for jsonb column).
    Error(String),
}

/// A modal editor for JSON values with syntax highlighting and vim keybindings.
pub struct JsonEditorModal<'a> {
    /// The textarea for editing
    textarea: TextArea<'a>,
    /// Syntax highlighter with JSON registered
    highlighter: Highlighter,
    /// Column name being edited
    column_name: String,
    /// Column data type (e.g., "jsonb", "text")
    column_type: String,
    /// Original value for cancel (kept for potential future "revert" feature)
    #[allow(dead_code)]
    original_value: String,
    /// Row index in the grid
    row: usize,
    /// Column index in the grid
    col: usize,
    /// Whether current content is valid JSON
    is_valid_json: bool,
    /// Scroll offset for HighlightedTextArea
    scroll_offset: (u16, u16),
    /// Current vim mode
    mode: VimMode,
    /// Vim key handler
    vim_handler: VimHandler,
}

impl<'a> JsonEditorModal<'a> {
    /// Create a new JSON editor modal.
    ///
    /// The value will be auto-formatted if it's valid JSON.
    pub fn new(
        value: String,
        column_name: String,
        column_type: String,
        row: usize,
        col: usize,
    ) -> Self {
        // Try to pretty-print the JSON
        let formatted_value = try_format_json(&value).unwrap_or_else(|| value.clone());
        let is_valid = is_valid_json(&formatted_value);

        // Create textarea with the formatted value
        let lines: Vec<String> = formatted_value.lines().map(|s| s.to_string()).collect();
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        let mut textarea = TextArea::new(lines);

        // Configure textarea
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

        // Create highlighter with JSON support
        let mut highlighter = Highlighter::new(themes::one_dark());
        let _ = highlighter.register_language(json());

        // Create vim handler with JSON editor config (double-Esc to cancel, no search)
        let vim_handler = VimHandler::new(VimConfig::json_editor());

        Self {
            textarea,
            highlighter,
            column_name,
            column_type,
            original_value: value,
            row,
            col,
            is_valid_json: is_valid,
            scroll_offset: (0, 0),
            mode: VimMode::Normal, // Start in normal mode (vim default)
            vim_handler,
        }
    }

    /// Get the current content as a string.
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Check if the column is a JSON type (json/jsonb).
    pub fn is_json_column(&self) -> bool {
        is_json_column_type(&self.column_type)
    }

    /// Update the JSON validity status.
    fn update_validity(&mut self) {
        self.is_valid_json = is_valid_json(&self.content());
    }

    /// Format the JSON content (pretty-print).
    pub fn format_json(&mut self) {
        let content = self.content();
        if let Some(formatted) = try_format_json(&content) {
            let lines: Vec<String> = formatted.lines().map(|s| s.to_string()).collect();
            let lines = if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            };
            self.textarea = TextArea::new(lines);
            self.textarea.set_cursor_line_style(Style::default());
            self.textarea
                .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
            self.is_valid_json = true;
        }
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> JsonEditorAction {
        let command = self.vim_handler.handle_key(key, self.mode);
        self.execute_command(command, key)
    }

    /// Execute a vim command and return the appropriate action.
    fn execute_command(&mut self, command: VimCommand, key: KeyEvent) -> JsonEditorAction {
        match command {
            VimCommand::None => JsonEditorAction::Continue,

            // Mode changes
            VimCommand::ChangeMode(new_mode) => {
                self.mode = new_mode;
                JsonEditorAction::Continue
            }

            // Movement
            VimCommand::Move(motion) => {
                self.apply_motion(motion);
                JsonEditorAction::Continue
            }

            // Enter insert mode at position
            VimCommand::EnterInsertAt { motion, mode } => {
                if let Some(m) = motion {
                    self.apply_motion(m);
                }
                self.mode = mode;
                JsonEditorAction::Continue
            }

            // Open new line
            VimCommand::OpenLine { above } => {
                if above {
                    self.textarea.move_cursor(CursorMove::Head);
                    self.textarea.insert_newline();
                    self.textarea.move_cursor(CursorMove::Up);
                } else {
                    self.textarea.move_cursor(CursorMove::End);
                    self.textarea.insert_newline();
                }
                self.mode = VimMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Delete operations
            VimCommand::DeleteChar => {
                self.textarea.delete_char();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::DeleteCharBefore => {
                self.textarea.delete_next_char();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::DeleteToEnd => {
                self.textarea.delete_line_by_end();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::DeleteLine => {
                self.delete_line();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::DeleteMotion(motion) => {
                self.delete_by_motion(motion);
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Change operations (delete + enter insert)
            VimCommand::ChangeToEnd => {
                self.textarea.delete_line_by_end();
                self.mode = VimMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::ChangeLine => {
                self.textarea.move_cursor(CursorMove::Head);
                self.textarea.delete_line_by_end();
                self.mode = VimMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::ChangeMotion(motion) => {
                self.delete_by_motion(motion);
                self.mode = VimMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Yank operations
            VimCommand::YankLine => {
                self.yank_line();
                JsonEditorAction::Continue
            }
            VimCommand::YankMotion(_motion) => {
                // TODO: Implement yank by motion
                // For now, just yank the whole line
                self.yank_line();
                JsonEditorAction::Continue
            }

            // Paste operations
            VimCommand::PasteAfter => {
                self.textarea.paste();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::PasteBefore => {
                // Move back one char, paste, then adjust
                self.textarea.paste();
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Undo/redo
            VimCommand::Undo => {
                self.textarea.undo();
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::Redo => {
                self.textarea.redo();
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Visual mode
            VimCommand::StartVisual => {
                self.textarea.start_selection();
                self.mode = VimMode::Visual;
                JsonEditorAction::Continue
            }
            VimCommand::CancelVisual => {
                self.textarea.cancel_selection();
                self.mode = VimMode::Normal;
                JsonEditorAction::Continue
            }
            VimCommand::VisualYank => {
                self.textarea.copy();
                self.textarea.cancel_selection();
                self.mode = VimMode::Normal;
                JsonEditorAction::Continue
            }
            VimCommand::VisualDelete => {
                self.textarea.cut();
                self.textarea.cancel_selection();
                self.mode = VimMode::Normal;
                self.update_validity();
                JsonEditorAction::Continue
            }
            VimCommand::VisualChange => {
                self.textarea.cut();
                self.textarea.cancel_selection();
                self.mode = VimMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Pass through (insert mode typing)
            VimCommand::PassThrough => {
                self.textarea.input(key);
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Custom commands
            VimCommand::Custom(cmd) => match cmd.as_str() {
                "save" => self.try_save(),
                "cancel" => JsonEditorAction::Cancel,
                "format" => {
                    self.format_json();
                    JsonEditorAction::Continue
                }
                _ => JsonEditorAction::Continue,
            },
        }
    }

    /// Apply a motion to the textarea.
    fn apply_motion(&mut self, motion: Motion) {
        match motion {
            Motion::Cursor(cm) => {
                self.textarea.move_cursor(cm);
            }
            Motion::Up(n) => {
                for _ in 0..n {
                    self.textarea.move_cursor(CursorMove::Up);
                }
            }
            Motion::Down(n) => {
                for _ in 0..n {
                    self.textarea.move_cursor(CursorMove::Down);
                }
            }
        }
    }

    /// Delete the current line.
    fn delete_line(&mut self) {
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.delete_line_by_end();
        self.textarea.delete_char(); // Delete the newline
    }

    /// Delete text by motion.
    fn delete_by_motion(&mut self, motion: Motion) {
        match motion {
            Motion::Cursor(CursorMove::WordForward) => {
                self.textarea.delete_next_word();
            }
            Motion::Cursor(CursorMove::WordEnd) => {
                self.textarea.delete_next_word();
            }
            Motion::Cursor(CursorMove::WordBack) => {
                self.textarea.delete_word();
            }
            Motion::Cursor(CursorMove::End) => {
                self.textarea.delete_line_by_end();
            }
            Motion::Cursor(CursorMove::Head) => {
                self.textarea.delete_line_by_head();
            }
            _ => {
                // For other motions, select and delete
                self.textarea.start_selection();
                self.apply_motion(motion);
                self.textarea.cut();
            }
        }
    }

    /// Yank the current line.
    fn yank_line(&mut self) {
        let (row, _) = self.textarea.cursor();
        if let Some(line) = self.textarea.lines().get(row) {
            self.textarea.set_yank_text(line.clone() + "\n");
        }
    }

    /// Try to save the content, checking validation rules.
    fn try_save(&mut self) -> JsonEditorAction {
        let content = self.content();

        // For jsonb columns, require valid JSON
        if self.is_json_column() && !is_valid_json(&content) {
            return JsonEditorAction::Error(
                "Cannot save invalid JSON to a JSONB column. Fix the JSON or press Esc twice to cancel."
                    .to_string(),
            );
        }

        JsonEditorAction::Save {
            value: content,
            row: self.row,
            col: self.col,
        }
    }

    /// Render the JSON editor modal.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate modal size (80% of screen)
        let modal_width = (area.width as f32 * 0.8) as u16;
        let modal_height = (area.height as f32 * 0.8) as u16;
        let modal_x = (area.width - modal_width) / 2;
        let modal_y = (area.height - modal_height) / 2;

        let modal_area = Rect {
            x: modal_x,
            y: modal_y,
            width: modal_width,
            height: modal_height,
        };

        // Clear the background
        frame.render_widget(Clear, modal_area);

        // Create layout: editor area + status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // Editor
                Constraint::Length(1), // Status bar
            ])
            .split(modal_area);

        let editor_area = chunks[0];
        let status_area = chunks[1];

        // Build title
        let title = format!(
            " Edit: {} ({}) - {} ",
            self.column_name, self.column_type, self.mode.label()
        );

        // Build block with border color based on validity
        let border_color = if self.is_valid_json {
            Color::Green
        } else {
            Color::Red
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color));

        // Highlight the content only if it's valid JSON
        // This avoids performance issues with large non-JSON content (like HTML)
        let content = self.content();
        let highlighted_lines = if self.should_highlight() {
            self.highlighter
                .highlight("json", &content)
                .unwrap_or_else(|_| content.lines().map(|l| Line::from(l.to_string())).collect())
        } else {
            // For non-JSON content, just return plain lines without syntax highlighting
            content.lines().map(|l| Line::from(l.to_string())).collect()
        };

        // Render highlighted textarea
        let highlighted_textarea = HighlightedTextArea::new(&self.textarea, highlighted_lines)
            .block(block)
            .scroll(self.scroll_offset);

        frame.render_widget(highlighted_textarea, editor_area);

        // Update scroll offset based on cursor
        let (cursor_row, _cursor_col) = self.textarea.cursor();
        let inner_height = editor_area.height.saturating_sub(2) as usize;
        if cursor_row >= self.scroll_offset.0 as usize + inner_height {
            self.scroll_offset.0 = (cursor_row - inner_height + 1) as u16;
        } else if cursor_row < self.scroll_offset.0 as usize {
            self.scroll_offset.0 = cursor_row as u16;
        }

        // Render status bar
        let (cursor_row, cursor_col) = self.textarea.cursor();
        let line_count = self.textarea.lines().len();

        let validity_span = if self.is_valid_json {
            Span::styled(" ✓ Valid JSON ", Style::default().fg(Color::Green))
        } else {
            Span::styled(" ✗ Invalid JSON ", Style::default().fg(Color::Red))
        };

        let mode_color = match self.mode {
            VimMode::Normal => Color::Cyan,
            VimMode::Insert => Color::Green,
            VimMode::Visual => Color::Magenta,
        };
        let mode_span = Span::styled(
            format!(" {} ", self.mode.label()),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        );

        let pos_span = Span::raw(format!(
            " Ln {}/{}, Col {} ",
            cursor_row + 1,
            line_count,
            cursor_col + 1
        ));

        let help_span = match self.mode {
            VimMode::Normal => Span::styled(
                " i:insert  v:visual  Ctrl+S:save  Esc×2:cancel ",
                Style::default().fg(Color::DarkGray),
            ),
            VimMode::Insert => Span::styled(
                " Esc:normal  Ctrl+Enter:save ",
                Style::default().fg(Color::DarkGray),
            ),
            VimMode::Visual => Span::styled(
                " y:yank  d:delete  c:change  Esc:cancel ",
                Style::default().fg(Color::DarkGray),
            ),
        };

        let status_line = Line::from(vec![mode_span, validity_span, pos_span, help_span]);

        let status = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));

        frame.render_widget(status, status_area);
    }

    /// Check if syntax highlighting should be applied.
    /// Only highlight if the content is valid JSON to avoid performance issues
    /// with large non-JSON content (like HTML).
    fn should_highlight(&self) -> bool {
        self.is_valid_json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_json_editor_with_valid_json() {
        let editor = JsonEditorModal::new(
            r#"{"key": "value"}"#.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );
        assert!(editor.is_valid_json);
        assert!(editor.should_highlight());
    }

    #[test]
    fn test_json_editor_with_invalid_json() {
        let editor = JsonEditorModal::new(
            "not json".to_string(),
            "data".to_string(),
            "text".to_string(),
            0,
            0,
        );
        assert!(!editor.is_valid_json);
        assert!(!editor.should_highlight());
    }

    #[test]
    fn test_json_editor_with_html_content() {
        // HTML content should not be treated as JSON
        let html = "<html><head><title>Test</title></head><body><p>Hello</p></body></html>";
        let editor = JsonEditorModal::new(
            html.to_string(),
            "content".to_string(),
            "text".to_string(),
            0,
            0,
        );
        assert!(!editor.is_valid_json);
        assert!(!editor.should_highlight());
    }

    #[test]
    fn test_json_editor_with_large_html_content_performance() {
        // Large HTML content should be handled quickly without freezing
        // This tests the scenario where a text column contains large HTML
        let large_html = format!(
            "<!DOCTYPE html><html><head><title>Test</title></head><body>{}</body></html>",
            "<div class=\"content\"><p>This is a paragraph with some text content.</p></div>"
                .repeat(500)
        );

        let start = Instant::now();
        let editor = JsonEditorModal::new(
            large_html.clone(),
            "html_content".to_string(),
            "text".to_string(),
            0,
            0,
        );
        let creation_time = start.elapsed();

        // Editor creation should be fast (under 100ms)
        assert!(
            creation_time < Duration::from_millis(100),
            "Editor creation took too long: {:?}",
            creation_time
        );

        // Should not be detected as JSON
        assert!(!editor.is_valid_json);
        assert!(!editor.should_highlight());

        // Content should be preserved
        assert_eq!(editor.content(), large_html);
    }

    #[test]
    fn test_json_editor_content_retrieval() {
        let json = r#"{"name": "test", "value": 123}"#;
        let editor = JsonEditorModal::new(
            json.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );

        // Content should be formatted (pretty-printed)
        let content = editor.content();
        assert!(content.contains("\"name\""));
        assert!(content.contains("\"test\""));
    }

    #[test]
    fn test_json_editor_starts_in_normal_mode() {
        let editor = JsonEditorModal::new(
            r#"{"key": "value"}"#.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );
        assert_eq!(editor.mode, VimMode::Normal);
    }
}
