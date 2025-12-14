//! Text editor modal for editing cell values with syntax highlighting.
//!
//! This modal provides:
//! - Auto-detected syntax highlighting (JSON, HTML, SQL, plain text)
//! - Vim-like keybindings (Normal/Insert/Visual modes) via unified VimHandler
//! - JSON validation with error display
//! - Auto-formatting on open for JSON content
//! - Virtual scrolling for large content

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use tui_textarea::{CursorMove, TextArea};

use tui_syntax::{html, json, themes, Highlighter};

use crate::ui::HighlightedTextArea;
use crate::util::{
    detect_content_type, is_json_column_type, is_valid_json, try_format_json, ContentType,
};
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
    /// Cancel editing and close the editor (no unsaved changes).
    Cancel,
    /// Request to close with unsaved changes (needs confirmation).
    RequestClose { row: usize, col: usize },
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
    /// Command mode active
    command_active: bool,
    /// Command buffer
    command_buffer: String,
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
        // Replace tabs with spaces to avoid rendering issues in the terminal
        let lines: Vec<String> = formatted_value
            .lines()
            .map(|s| s.replace('\t', "    "))
            .collect();
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        let mut textarea = TextArea::new(lines);

        // Configure textarea
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

        // Create highlighter with JSON and HTML support
        let mut highlighter = Highlighter::new(themes::one_dark());
        let _ = highlighter.register_language(json());
        let _ = highlighter.register_language(html());

        // Create vim handler with JSON editor config (double-Esc to cancel, no search)
        let vim_handler = VimHandler::new(VimConfig::json_editor());

        Self {
            textarea,
            highlighter,
            column_name,
            column_type,
            original_value: formatted_value,
            row,
            col,
            is_valid_json: is_valid,
            scroll_offset: (0, 0),
            mode: VimMode::Normal, // Start in normal mode (vim default)
            vim_handler,
            command_active: false,
            command_buffer: String::new(),
        }
    }

    /// Get the current content as a string.
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Check if the content has been modified from the original.
    pub fn is_modified(&self) -> bool {
        self.content() != self.original_value
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
        // Handle command mode separately
        if self.command_active {
            return self.handle_command_mode_key(key);
        }

        // Handle q and Esc in Normal mode to close/request close
        if self.mode == VimMode::Normal {
            match (key.code, key.modifiers) {
                // q in Normal mode closes/requests close
                (KeyCode::Char('q'), KeyModifiers::NONE) => {
                    return self.request_close();
                }
                // Esc in Normal mode closes/requests close
                (KeyCode::Esc, KeyModifiers::NONE) => {
                    return self.request_close();
                }
                _ => {}
            }
        }

        let command = self.vim_handler.handle_key(key, self.mode);
        self.execute_command(command, key)
    }

    /// Request to close the editor, checking for unsaved changes.
    fn request_close(&self) -> JsonEditorAction {
        if self.is_modified() {
            JsonEditorAction::RequestClose {
                row: self.row,
                col: self.col,
            }
        } else {
            JsonEditorAction::Cancel
        }
    }

    /// Handle key events in command mode (after pressing ':').
    fn handle_command_mode_key(&mut self, key: KeyEvent) -> JsonEditorAction {
        match (key.code, key.modifiers) {
            // Escape cancels command mode
            (KeyCode::Esc, KeyModifiers::NONE) => {
                self.command_active = false;
                self.command_buffer.clear();
                JsonEditorAction::Continue
            }
            // Enter executes command
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let result = self.execute_ex_command();
                self.command_active = false;
                self.command_buffer.clear();
                result
            }
            // Backspace
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                if self.command_buffer.is_empty() {
                    self.command_active = false;
                } else {
                    self.command_buffer.pop();
                }
                JsonEditorAction::Continue
            }
            // Character input
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.command_buffer.push(c);
                JsonEditorAction::Continue
            }
            _ => JsonEditorAction::Continue,
        }
    }

    /// Execute an ex command (like :format, :w, :q).
    fn execute_ex_command(&mut self) -> JsonEditorAction {
        let cmd = self.command_buffer.trim().to_lowercase();

        match cmd.as_str() {
            // Format JSON
            "format" | "fmt" => {
                self.format_json();
                JsonEditorAction::Continue
            }
            // Save (write)
            "w" | "write" => self.try_save(),
            // Quit (cancel)
            "q" | "quit" => JsonEditorAction::Cancel,
            // Save and quit
            "wq" | "x" => match self.try_save() {
                JsonEditorAction::Error(e) => JsonEditorAction::Error(e),
                _ => JsonEditorAction::Save {
                    value: self.content(),
                    row: self.row,
                    col: self.col,
                },
            },
            // Unknown command
            _ => JsonEditorAction::Error(format!("Unknown command: {}", cmd)),
        }
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
                "command" => {
                    self.command_active = true;
                    self.command_buffer.clear();
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

        // Build title with [+] indicator if modified
        let modified_indicator = if self.is_modified() { " [+]" } else { "" };
        let title = format!(
            " Edit: {} ({}){} - {} ",
            self.column_name,
            self.column_type,
            modified_indicator,
            self.mode.label()
        );

        // Detect content type and determine border color
        let content = self.content();
        let content_type = detect_content_type(&content);

        // Border color: green for valid JSON (if JSON column), yellow otherwise, red for invalid JSON in JSON column
        let border_color = if self.is_json_column() {
            if self.is_valid_json {
                Color::Green
            } else {
                Color::Red
            }
        } else {
            // Non-JSON column: show detected type in title color
            match content_type {
                ContentType::Json => Color::Green,
                ContentType::Html => Color::Cyan,
                ContentType::Sql => Color::Yellow,
                ContentType::Plain => Color::White,
            }
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color));

        // Apply syntax highlighting based on detected content type
        let highlighted_lines = if let Some(lang) = content_type.language_name() {
            self.highlighter
                .highlight(lang, &content)
                .unwrap_or_else(|_| content.lines().map(|l| Line::from(l.to_string())).collect())
        } else {
            // Plain text - no highlighting
            content.lines().map(|l| Line::from(l.to_string())).collect()
        };

        // Render highlighted textarea
        let highlighted_textarea = HighlightedTextArea::new(&self.textarea, highlighted_lines)
            .block(block.clone())
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

        // Render scrollbar if content exceeds visible area
        let total_lines = self.textarea.lines().len();
        if total_lines > inner_height && inner_height > 0 {
            let inner_area = block.inner(editor_area);
            let scrollbar_area = Rect {
                x: inner_area.x + inner_area.width.saturating_sub(1),
                y: inner_area.y,
                width: 1,
                height: inner_area.height,
            };

            let scrollbar = if scrollbar_area.height >= 7 {
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("▲"))
                    .end_symbol(Some("▼"))
                    .thumb_symbol("█")
                    .track_symbol(Some("░"))
            } else {
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .thumb_symbol("█")
                    .track_symbol(Some("│"))
            };

            let mut scrollbar_state =
                ScrollbarState::new(total_lines).position(self.scroll_offset.0 as usize);

            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }

        // Render status bar
        let (cursor_row, cursor_col) = self.textarea.cursor();
        let line_count = self.textarea.lines().len();

        // Show content type indicator
        let type_span = match content_type {
            ContentType::Json => {
                if self.is_valid_json {
                    Span::styled(" ✓ JSON ", Style::default().fg(Color::Green))
                } else {
                    Span::styled(" ✗ JSON ", Style::default().fg(Color::Red))
                }
            }
            ContentType::Html => Span::styled(" HTML ", Style::default().fg(Color::Cyan)),
            ContentType::Sql => Span::styled(" SQL ", Style::default().fg(Color::Yellow)),
            ContentType::Plain => Span::styled(" TEXT ", Style::default().fg(Color::White)),
        };

        // For JSON columns, also show validation status
        let validity_span = if self.is_json_column() && !self.is_valid_json {
            Some(Span::styled(
                " (invalid for JSONB) ",
                Style::default().fg(Color::Red),
            ))
        } else {
            None
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

        // If command mode is active, show command prompt instead of help
        if self.command_active {
            let command_line = Line::from(vec![
                Span::styled(":", Style::default().fg(Color::Yellow)),
                Span::raw(&self.command_buffer),
                Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
            ]);
            let status = Paragraph::new(command_line).style(Style::default().bg(Color::DarkGray));
            frame.render_widget(status, status_area);
        } else {
            let help_span = match self.mode {
                VimMode::Normal => Span::styled(
                    " i:insert  v:visual  :format  Ctrl+S:save  q/Esc:close ",
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

            let mut spans = vec![mode_span, type_span];
            if let Some(v) = validity_span {
                spans.push(v);
            }
            spans.push(pos_span);
            spans.push(help_span);

            let status_line = Line::from(spans);

            let status = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));

            frame.render_widget(status, status_area);
        }
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
        // JSON content should be detected as JSON type
        let content_type = detect_content_type(&editor.content());
        assert_eq!(content_type, ContentType::Json);
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
        // Plain text should be detected as Plain type
        let content_type = detect_content_type(&editor.content());
        assert_eq!(content_type, ContentType::Plain);
    }

    #[test]
    fn test_json_editor_with_html_content() {
        // HTML content should be detected as HTML type
        let html = "<html><head><title>Test</title></head><body><p>Hello</p></body></html>";
        let editor = JsonEditorModal::new(
            html.to_string(),
            "content".to_string(),
            "text".to_string(),
            0,
            0,
        );
        assert!(!editor.is_valid_json);
        let content_type = detect_content_type(&editor.content());
        assert_eq!(content_type, ContentType::Html);
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

        // Should be detected as HTML, not JSON
        assert!(!editor.is_valid_json);
        let content_type = detect_content_type(&editor.content());
        assert_eq!(content_type, ContentType::Html);

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

    #[test]
    fn test_json_editor_command_mode_format() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = JsonEditorModal::new(
            r#"{"key":"value"}"#.to_string(), // Not formatted
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );

        // Press ':' to enter command mode
        let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
        editor.handle_key(colon);
        assert!(editor.command_active);

        // Type "format"
        for c in "format".chars() {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            editor.handle_key(key);
        }
        assert_eq!(editor.command_buffer, "format");

        // Press Enter to execute
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        editor.handle_key(enter);

        // Command mode should be closed
        assert!(!editor.command_active);
        assert!(editor.command_buffer.is_empty());

        // Content should be formatted (multi-line)
        let content = editor.content();
        assert!(
            content.contains('\n'),
            "Content should be formatted with newlines"
        );
    }

    #[test]
    fn test_json_editor_command_mode_escape() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = JsonEditorModal::new(
            r#"{"key": "value"}"#.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );

        // Press ':' to enter command mode
        let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
        editor.handle_key(colon);
        assert!(editor.command_active);

        // Type something
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        editor.handle_key(key);

        // Press Escape to cancel
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        editor.handle_key(esc);

        // Command mode should be closed and buffer cleared
        assert!(!editor.command_active);
        assert!(editor.command_buffer.is_empty());
    }

    // ========== Change Tracking Tests ==========

    #[test]
    fn test_json_editor_not_modified_initially() {
        // Note: JSON content gets formatted on creation, so we use pre-formatted content
        let formatted = "{\n  \"key\": \"value\"\n}";
        let editor = JsonEditorModal::new(
            formatted.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );

        // For formatted JSON, content equals original so not modified
        // This test verifies the is_modified logic works
        assert_eq!(
            editor.content(),
            editor.original_value,
            "Formatted content should match original"
        );
        assert!(
            !editor.is_modified(),
            "Editor should not be modified initially when content matches original"
        );
    }

    #[test]
    fn test_json_editor_not_modified_after_auto_formatting() {
        // Unformatted JSON gets pretty-printed on open, but should NOT be marked as modified
        // The original_value should be stored as the formatted value, not the input value
        let unformatted = r#"{"key":"value"}"#;
        let editor = JsonEditorModal::new(
            unformatted.to_string(),
            "data".to_string(),
            "jsonb".to_string(),
            0,
            0,
        );

        // After formatting, content differs from the input
        assert!(
            editor.content() != unformatted,
            "Formatted content should differ from unformatted input"
        );
        // But the editor should NOT be marked as modified since no user changes were made
        assert!(
            !editor.is_modified(),
            "Editor should NOT be modified on first open, even after auto-formatting"
        );
    }

    #[test]
    fn test_json_editor_plain_text_not_modified() {
        // Plain text shouldn't be formatted
        let plain = "hello world";
        let editor = JsonEditorModal::new(
            plain.to_string(),
            "notes".to_string(),
            "text".to_string(),
            0,
            0,
        );

        assert_eq!(editor.content(), plain);
        assert!(
            !editor.is_modified(),
            "Plain text editor should not be modified"
        );
    }

    // ========== Esc/q Close Behavior Tests ==========

    #[test]
    fn test_json_editor_esc_no_changes_returns_cancel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // Plain text that won't be formatted
        let mut editor = JsonEditorModal::new(
            "hello".to_string(),
            "notes".to_string(),
            "text".to_string(),
            0,
            0,
        );

        // Press Esc in Normal mode with no changes
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = editor.handle_key(esc);

        assert!(
            matches!(result, JsonEditorAction::Cancel),
            "Esc with no changes should return Cancel"
        );
    }

    #[test]
    fn test_json_editor_esc_with_changes_returns_request_close() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = JsonEditorModal::new(
            "hello".to_string(),
            "notes".to_string(),
            "text".to_string(),
            0,
            0,
        );

        // Should start unmodified
        assert!(!editor.is_modified());

        // Enter insert mode and make a change
        let i_key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        editor.handle_key(i_key);
        assert_eq!(editor.mode, VimMode::Insert);

        // Type some text
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        editor.handle_key(x_key);

        // Exit insert mode
        let esc1 = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        editor.handle_key(esc1);
        assert_eq!(editor.mode, VimMode::Normal);

        // Now verify it's modified
        assert!(editor.is_modified());

        // Press Esc in Normal mode with changes
        let esc2 = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = editor.handle_key(esc2);

        match result {
            JsonEditorAction::RequestClose { row, col } => {
                assert_eq!(row, 0);
                assert_eq!(col, 0);
            }
            _ => panic!(
                "Expected RequestClose, got {:?}",
                std::any::type_name_of_val(&result)
            ),
        }
    }

    #[test]
    fn test_json_editor_q_no_changes_returns_cancel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = JsonEditorModal::new(
            "hello".to_string(),
            "notes".to_string(),
            "text".to_string(),
            0,
            0,
        );

        // Press 'q' in Normal mode
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = editor.handle_key(q);

        assert!(
            matches!(result, JsonEditorAction::Cancel),
            "'q' with no changes should return Cancel"
        );
    }

    #[test]
    fn test_json_editor_q_with_changes_returns_request_close() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = JsonEditorModal::new(
            "hello".to_string(),
            "notes".to_string(),
            "text".to_string(),
            0,
            0,
        );

        // Should start unmodified
        assert!(!editor.is_modified());

        // Enter insert mode and make a change
        let i_key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        editor.handle_key(i_key);

        // Type some text
        let x_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        editor.handle_key(x_key);

        // Exit insert mode
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        editor.handle_key(esc);

        // Now verify it's modified
        assert!(editor.is_modified());

        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = editor.handle_key(q);

        assert!(
            matches!(result, JsonEditorAction::RequestClose { .. }),
            "'q' with changes should return RequestClose"
        );
    }

    // ========== Rendering Tests ==========

    #[test]
    fn test_json_editor_modal_with_large_html_file() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        // Load the actual problematic HTML file or create synthetic content
        let html_content = std::fs::read_to_string("tests/fixtures/large_html.html")
            .unwrap_or_else(|_| {
                // Fallback: create synthetic content with a very long line (like minified JS)
                let long_line = "x".repeat(60000);
                format!(
                    "<!DOCTYPE html>\n<html>\n<head>\n<script>{}</script>\n</head>\n</html>",
                    long_line
                )
            });

        // Create a terminal with test backend
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        // First, fill the terminal buffer with "garbage" to simulate underlying grid content
        terminal
            .draw(|frame| {
                let area = frame.area();
                let buf = frame.buffer_mut();
                for y in 0..area.height {
                    for x in 0..area.width {
                        // Simulate grid row numbers and content like "64|15<data..."
                        let pattern = format!("{:02}|{:02}", y, x % 100);
                        if let Some(ch) = pattern.chars().nth((x % 5) as usize) {
                            buf[(x, y)].set_symbol(&ch.to_string());
                        }
                    }
                }
            })
            .unwrap();

        // Verify garbage is in the buffer
        {
            let backend = terminal.backend();
            let buf = backend.buffer();
            // Check that garbage pattern exists at various positions
            assert_ne!(buf[(10, 10)].symbol(), " ");
            assert_ne!(buf[(50, 20)].symbol(), " ");
        }

        // Now render the JSON editor modal with the large HTML content
        let mut editor = JsonEditorModal::new(
            html_content,
            "raw_contents".to_string(),
            "text".to_string(),
            0,
            0,
        );

        terminal
            .draw(|frame| {
                editor.render(frame, frame.area());
            })
            .unwrap();

        // Check that the modal area is properly cleared
        let backend = terminal.backend();
        let buf = backend.buffer();

        // The modal is 80% of 120 = 96 wide, centered at x = (120-96)/2 = 12
        // The modal is 80% of 40 = 32 tall, centered at y = (40-32)/2 = 4
        let modal_x = 12;
        let modal_y = 4;
        let inner_x = modal_x + 1;
        let inner_y = modal_y + 1;

        // First character should be '<' from "<!DOCTYPE html>"
        assert_eq!(
            buf[(inner_x, inner_y)].symbol(),
            "<",
            "First char should be '<' from HTML content, not garbage. Got: '{}'",
            buf[(inner_x, inner_y)].symbol()
        );

        // Check multiple positions across the inner area - they should NOT have garbage
        // Test middle of a row that should have content cleared
        let test_positions = [
            (inner_x + 20, inner_y),     // After "<!DOCTYPE html>" on first line
            (inner_x + 50, inner_y),     // Further right on first line
            (inner_x + 5, inner_y + 5),  // On a later line
            (inner_x + 30, inner_y + 5), // Middle of a later line
            (inner_x + 70, inner_y + 5), // Right side of a later line
        ];

        for (x, y) in test_positions {
            let symbol = buf[(x, y)].symbol();
            // Should NOT be the garbage pattern (digits followed by pipe)
            let is_garbage = symbol
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && buf
                    .cell((x + 1, y))
                    .map(|c| c.symbol() == "|")
                    .unwrap_or(false);
            assert!(
                !is_garbage,
                "Position ({}, {}) should not have garbage pattern, got: '{}'",
                x, y, symbol,
            );
        }
    }
}
