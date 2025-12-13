//! JSON editor modal for editing JSON/JSONB cell values.
//!
//! This modal provides:
//! - Syntax-highlighted JSON editing
//! - Vim-like keybindings (Normal/Insert modes)
//! - JSON validation with error display
//! - Auto-formatting on open
//! - Virtual scrolling for large JSON

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use tui_textarea::{CursorMove, TextArea};

use tui_syntax::{json, themes, Highlighter};

use crate::ui::HighlightedTextArea;
use crate::util::{is_json_column_type, is_valid_json, try_format_json};

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

/// Editor mode for vim-like keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    /// Normal mode - navigation and commands
    Normal,
    /// Insert mode - text input
    Insert,
}

impl Default for EditorMode {
    fn default() -> Self {
        EditorMode::Insert // Start in insert mode as per user preference
    }
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
    /// Editor mode (Normal/Insert for vim bindings)
    mode: EditorMode,
    /// Whether ESC was pressed in normal mode (for double-ESC to quit)
    esc_pressed: bool,
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
            mode: EditorMode::Insert, // Start in insert mode
            esc_pressed: false,
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
        match self.mode {
            EditorMode::Normal => self.handle_normal_mode_key(key),
            EditorMode::Insert => self.handle_insert_mode_key(key),
        }
    }

    /// Handle key events in normal mode (vim navigation).
    fn handle_normal_mode_key(&mut self, key: KeyEvent) -> JsonEditorAction {
        // Reset esc_pressed flag unless we're processing Esc
        if key.code != KeyCode::Esc {
            self.esc_pressed = false;
        }

        match (key.code, key.modifiers) {
            // Exit: Esc twice or :q
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if self.esc_pressed {
                    return JsonEditorAction::Cancel;
                }
                self.esc_pressed = true;
                JsonEditorAction::Continue
            }

            // Save: Ctrl+S or :w
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.try_save(),

            // Movement: hjkl
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                self.textarea.move_cursor(CursorMove::Back);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.textarea.move_cursor(CursorMove::Down);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.textarea.move_cursor(CursorMove::Up);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                self.textarea.move_cursor(CursorMove::Forward);
                JsonEditorAction::Continue
            }

            // Word movement
            (KeyCode::Char('w'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::WordForward);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('b'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::WordBack);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::WordForward);
                JsonEditorAction::Continue
            }

            // Line movement
            (KeyCode::Char('0'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::Head);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('$'), KeyModifiers::NONE) | (KeyCode::End, _) => {
                self.textarea.move_cursor(CursorMove::End);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('^'), KeyModifiers::NONE) | (KeyCode::Home, _) => {
                self.textarea.move_cursor(CursorMove::Head);
                JsonEditorAction::Continue
            }

            // Document movement
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                // gg - go to start (simplified: just g goes to start)
                self.textarea.move_cursor(CursorMove::Top);
                JsonEditorAction::Continue
            }
            (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.textarea.move_cursor(CursorMove::Bottom);
                JsonEditorAction::Continue
            }

            // Page movement
            (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                // Move up half a page (approximate with multiple up moves)
                for _ in 0..10 {
                    self.textarea.move_cursor(CursorMove::Up);
                }
                JsonEditorAction::Continue
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                for _ in 0..10 {
                    self.textarea.move_cursor(CursorMove::Down);
                }
                JsonEditorAction::Continue
            }

            // Enter insert mode
            (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.mode = EditorMode::Insert;
                JsonEditorAction::Continue
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::Forward);
                self.mode = EditorMode::Insert;
                JsonEditorAction::Continue
            }
            (KeyCode::Char('I'), KeyModifiers::SHIFT) => {
                self.textarea.move_cursor(CursorMove::Head);
                self.mode = EditorMode::Insert;
                JsonEditorAction::Continue
            }
            (KeyCode::Char('A'), KeyModifiers::SHIFT) => {
                self.textarea.move_cursor(CursorMove::End);
                self.mode = EditorMode::Insert;
                JsonEditorAction::Continue
            }
            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                self.textarea.move_cursor(CursorMove::End);
                self.textarea.insert_newline();
                self.mode = EditorMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }
            (KeyCode::Char('O'), KeyModifiers::SHIFT) => {
                self.textarea.move_cursor(CursorMove::Head);
                self.textarea.insert_newline();
                self.textarea.move_cursor(CursorMove::Up);
                self.mode = EditorMode::Insert;
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Delete
            (KeyCode::Char('x'), KeyModifiers::NONE) => {
                self.textarea.delete_char();
                self.update_validity();
                JsonEditorAction::Continue
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                // dd - delete line (simplified: just d deletes line)
                self.textarea.move_cursor(CursorMove::Head);
                self.textarea.delete_line_by_end();
                self.textarea.delete_char(); // Delete the newline
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Format JSON
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.format_json();
                JsonEditorAction::Continue
            }

            // Undo (if supported)
            (KeyCode::Char('u'), KeyModifiers::NONE) => {
                self.textarea.undo();
                self.update_validity();
                JsonEditorAction::Continue
            }

            // Redo
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.textarea.redo();
                self.update_validity();
                JsonEditorAction::Continue
            }

            _ => JsonEditorAction::Continue,
        }
    }

    /// Handle key events in insert mode.
    fn handle_insert_mode_key(&mut self, key: KeyEvent) -> JsonEditorAction {
        match (key.code, key.modifiers) {
            // Exit insert mode
            (KeyCode::Esc, KeyModifiers::NONE) => {
                self.mode = EditorMode::Normal;
                self.esc_pressed = false;
                JsonEditorAction::Continue
            }

            // Save: Ctrl+Enter or Ctrl+S
            (KeyCode::Enter, KeyModifiers::CONTROL)
            | (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.try_save(),

            // Format JSON: Ctrl+F
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.format_json();
                JsonEditorAction::Continue
            }

            // Regular text input - pass to textarea
            _ => {
                self.textarea.input(key);
                self.update_validity();
                JsonEditorAction::Continue
            }
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
            self.column_name,
            self.column_type,
            if self.mode == EditorMode::Normal {
                "NORMAL"
            } else {
                "INSERT"
            }
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

        // Highlight the content
        let content = self.content();
        let highlighted_lines = self
            .highlighter
            .highlight("json", &content)
            .unwrap_or_else(|_| content.lines().map(|l| Line::from(l.to_string())).collect());

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

        let mode_span = if self.mode == EditorMode::Normal {
            Span::styled(
                " NORMAL ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                " INSERT ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        };

        let pos_span = Span::raw(format!(
            " Ln {}/{}, Col {} ",
            cursor_row + 1,
            line_count,
            cursor_col + 1
        ));

        let help_span = if self.mode == EditorMode::Normal {
            Span::styled(
                " i:insert  Ctrl+S:save  Esc×2:cancel  Ctrl+F:format ",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::styled(
                " Esc:normal  Ctrl+Enter:save  Ctrl+F:format ",
                Style::default().fg(Color::DarkGray),
            )
        };

        let status_line = Line::from(vec![mode_span, validity_span, pos_span, help_span]);

        let status = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));

        frame.render_widget(status, status_area);
    }
}
