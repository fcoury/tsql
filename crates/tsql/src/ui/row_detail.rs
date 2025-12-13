//! Row detail view modal for displaying all columns of a single row.
//!
//! This modal provides:
//! - Scrollable list of column name: value pairs
//! - Full values (no truncation, unlike grid)
//! - Syntax highlighting for JSON/HTML content
//! - Vim-like navigation (j/k scroll, q/Esc close)

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use tui_syntax::{html, json, themes, Highlighter};

use crate::util::{detect_content_type, ContentType};

/// The result of handling a key event in the row detail view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowDetailAction {
    /// Continue viewing, nothing special happened.
    Continue,
    /// Close the detail view.
    Close,
    /// Open editor for the current field.
    Edit { col: usize },
}

/// A modal view showing all columns for a single row.
pub struct RowDetailModal {
    /// Column headers
    headers: Vec<String>,
    /// Row values
    values: Vec<String>,
    /// Column types (for display info)
    col_types: Vec<String>,
    /// Row index in the grid (for display)
    row_index: usize,
    /// Current scroll offset (in lines)
    scroll_offset: usize,
    /// Currently selected field index
    selected_field: usize,
    /// Total number of fields
    field_count: usize,
    /// Visible height (set during render)
    visible_height: usize,
    /// Syntax highlighter
    highlighter: Highlighter,
}

impl RowDetailModal {
    /// Create a new row detail modal.
    pub fn new(
        headers: Vec<String>,
        values: Vec<String>,
        col_types: Vec<String>,
        row_index: usize,
    ) -> Self {
        let field_count = headers.len();

        // Create highlighter with JSON and HTML support
        let mut highlighter = Highlighter::new(themes::one_dark());
        let _ = highlighter.register_language(json());
        let _ = highlighter.register_language(html());

        Self {
            headers,
            values,
            col_types,
            row_index,
            scroll_offset: 0,
            selected_field: 0,
            field_count,
            visible_height: 10,
            highlighter,
        }
    }

    /// Get the currently selected column index.
    pub fn selected_col(&self) -> usize {
        self.selected_field
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> RowDetailAction {
        match (key.code, key.modifiers) {
            // Close view
            (KeyCode::Esc, KeyModifiers::NONE)
            | (KeyCode::Char('q'), KeyModifiers::NONE) => RowDetailAction::Close,

            // Scroll down / next field
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.select_next();
                RowDetailAction::Continue
            }

            // Scroll up / previous field
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.select_prev();
                RowDetailAction::Continue
            }

            // Half page down (Ctrl-d)
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                let amount = self.visible_height / 2;
                for _ in 0..amount {
                    self.select_next();
                }
                RowDetailAction::Continue
            }

            // Half page up (Ctrl-u)
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                let amount = self.visible_height / 2;
                for _ in 0..amount {
                    self.select_prev();
                }
                RowDetailAction::Continue
            }

            // Full page down (Ctrl-f or PageDown)
            (KeyCode::Char('f'), KeyModifiers::CONTROL) | (KeyCode::PageDown, KeyModifiers::NONE) => {
                let amount = self.visible_height.saturating_sub(2);
                for _ in 0..amount {
                    self.select_next();
                }
                RowDetailAction::Continue
            }

            // Full page up (Ctrl-b or PageUp)
            (KeyCode::Char('b'), KeyModifiers::CONTROL) | (KeyCode::PageUp, KeyModifiers::NONE) => {
                let amount = self.visible_height.saturating_sub(2);
                for _ in 0..amount {
                    self.select_prev();
                }
                RowDetailAction::Continue
            }

            // Top (gg)
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.selected_field = 0;
                self.scroll_offset = 0;
                RowDetailAction::Continue
            }

            // Bottom (G)
            (KeyCode::Char('G'), KeyModifiers::SHIFT) | (KeyCode::Char('G'), KeyModifiers::NONE) => {
                if self.field_count > 0 {
                    self.selected_field = self.field_count - 1;
                    self.ensure_selected_visible();
                }
                RowDetailAction::Continue
            }

            // Edit current field (e or Enter)
            (KeyCode::Char('e'), KeyModifiers::NONE) | (KeyCode::Enter, KeyModifiers::NONE) => {
                RowDetailAction::Edit {
                    col: self.selected_field,
                }
            }

            _ => RowDetailAction::Continue,
        }
    }

    fn select_next(&mut self) {
        if self.field_count > 0 && self.selected_field < self.field_count - 1 {
            self.selected_field += 1;
            self.ensure_selected_visible();
        }
    }

    fn select_prev(&mut self) {
        if self.selected_field > 0 {
            self.selected_field -= 1;
            self.ensure_selected_visible();
        }
    }

    fn ensure_selected_visible(&mut self) {
        // Each field takes at least 2 lines (header line + value line)
        // But we track by field index, not line count
        if self.visible_height == 0 {
            return;
        }

        // Approximate lines per field (header + at least 1 value line + separator)
        let fields_per_page = self.visible_height / 3;
        let fields_per_page = fields_per_page.max(1);

        // If selected field is before visible area, scroll up
        if self.selected_field < self.scroll_offset {
            self.scroll_offset = self.selected_field;
        }

        // If selected field is after visible area, scroll down
        if self.selected_field >= self.scroll_offset + fields_per_page {
            self.scroll_offset = self.selected_field.saturating_sub(fields_per_page - 1);
        }
    }

    /// Render the row detail modal.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate modal size (80% of screen)
        let modal_width = (area.width as f32 * 0.85) as u16;
        let modal_height = (area.height as f32 * 0.85) as u16;
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

        // Build title
        let title = format!(" Row {} Details ({} columns) ", self.row_index + 1, self.field_count);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Create layout: content area + footer
        let chunks = Layout::vertical([
            Constraint::Min(1),    // Content
            Constraint::Length(1), // Footer
        ])
        .split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];

        // Update visible height for scrolling calculations
        self.visible_height = content_area.height as usize;

        // Render content with scrolling
        self.render_content(frame, content_area);

        // Render footer with navigation hints
        self.render_footer(frame, footer_area);

        // Render scrollbar if content overflows
        if self.field_count > self.visible_height / 3 {
            self.render_scrollbar(frame, content_area);
        }
    }

    fn render_content(&mut self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        // Calculate which fields to show based on scroll offset
        let fields_per_page = (area.height as usize / 3).max(1);
        let start_field = self.scroll_offset;
        let end_field = (start_field + fields_per_page + 1).min(self.field_count);

        let max_value_width = area.width.saturating_sub(4) as usize; // Leave room for borders/padding

        for field_idx in start_field..end_field {
            if field_idx >= self.headers.len() || field_idx >= self.values.len() {
                break;
            }

            let header = self.headers[field_idx].clone();
            let value = self.values[field_idx].clone();
            let col_type = self.col_types.get(field_idx).cloned().unwrap_or_default();

            let is_selected = field_idx == self.selected_field;

            // Field header line with type info
            let header_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            };

            let type_info = if col_type.is_empty() {
                String::new()
            } else {
                format!(" ({})", col_type)
            };

            let selector = if is_selected { "> " } else { "  " };

            lines.push(Line::from(vec![
                Span::styled(selector, Style::default().fg(Color::Cyan)),
                Span::styled(header, header_style),
                Span::styled(type_info, Style::default().fg(Color::DarkGray)),
            ]));

            // Value line(s) with syntax highlighting
            let content_type = detect_content_type(&value);
            let value_style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            // Handle multi-line values
            let value_lines: Vec<&str> = value.lines().collect();
            let value_lines_count = value_lines.len();
            let max_lines = 10; // Limit displayed lines per field

            if value_lines.is_empty() || value.is_empty() {
                // Empty or NULL value
                let display_value = if value.is_empty() { "(empty)".to_string() } else { value.clone() };
                lines.push(Line::from(vec![
                    Span::styled("    ", value_style),
                    Span::styled(display_value, value_style.fg(Color::DarkGray)),
                ]));
            } else if value_lines_count == 1 {
                // Single line value
                let truncated = truncate_for_display(&value, max_value_width);
                let highlighted = self.highlight_value(&truncated, content_type);
                
                let mut spans = vec![Span::styled("    ", value_style)];
                for span in highlighted {
                    spans.push(if is_selected {
                        Span::styled(span.content.to_string(), span.style.bg(Color::DarkGray))
                    } else {
                        span
                    });
                }
                lines.push(Line::from(spans));
            } else {
                // Multi-line value - collect the lines first
                let value_lines_owned: Vec<String> = value_lines.iter().map(|s| s.to_string()).collect();
                for (i, line) in value_lines_owned.iter().take(max_lines).enumerate() {
                    let truncated = truncate_for_display(line, max_value_width);
                    let highlighted = self.highlight_value(&truncated, content_type);
                    
                    let mut spans = vec![Span::styled("    ", value_style)];
                    for span in highlighted {
                        spans.push(if is_selected {
                            Span::styled(span.content.to_string(), span.style.bg(Color::DarkGray))
                        } else {
                            span
                        });
                    }
                    lines.push(Line::from(spans));

                    // Show truncation indicator
                    if i == max_lines - 1 && value_lines_count > max_lines {
                        lines.push(Line::from(vec![
                            Span::styled("    ", value_style),
                            Span::styled(
                                format!("... ({} more lines)", value_lines_count - max_lines),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                    }
                }
            }

            // Separator line (except for last field)
            if field_idx < end_field - 1 {
                lines.push(Line::from(""));
            }
        }

        let content = Paragraph::new(lines);
        frame.render_widget(content, area);
    }

    fn highlight_value(&mut self, value: &str, content_type: ContentType) -> Vec<Span<'static>> {
        if let Some(lang) = content_type.language_name() {
            if let Ok(highlighted_lines) = self.highlighter.highlight(lang, value) {
                // Flatten all spans from highlighted lines
                let mut spans = Vec::new();
                for line in highlighted_lines {
                    for span in line.spans {
                        spans.push(Span::styled(span.content.to_string(), span.style));
                    }
                }
                if spans.is_empty() {
                    return vec![Span::raw(value.to_string())];
                }
                return spans;
            }
        }
        
        // Plain text - no highlighting
        vec![Span::raw(value.to_string())]
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let footer = Line::from(vec![
            Span::styled(" j/k ", Style::default().fg(Color::Yellow)),
            Span::styled("navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("e/Enter ", Style::default().fg(Color::Yellow)),
            Span::styled("edit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("g/G ", Style::default().fg(Color::Yellow)),
            Span::styled("top/bottom  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q/Esc ", Style::default().fg(Color::Yellow)),
            Span::styled("close  ", Style::default().fg(Color::DarkGray)),
            Span::raw(" ".repeat(area.width.saturating_sub(60) as usize)),
            Span::styled(
                format!("{}/{}", self.selected_field + 1, self.field_count),
                Style::default().fg(Color::Cyan),
            ),
        ]);
        frame.render_widget(Paragraph::new(footer), area);
    }

    fn render_scrollbar(&self, frame: &mut Frame, area: Rect) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        let mut scrollbar_state = ScrollbarState::new(self.field_count)
            .position(self.selected_field);

        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Truncate a string for display, adding ellipsis if needed.
fn truncate_for_display(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        return s.to_string();
    }
    
    if max_width <= 3 {
        return s.chars().take(max_width).collect();
    }
    
    let truncated: String = s.chars().take(max_width - 1).collect();
    format!("{}…", truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_modal() -> RowDetailModal {
        RowDetailModal::new(
            vec!["id".to_string(), "name".to_string(), "data".to_string()],
            vec!["1".to_string(), "Alice".to_string(), r#"{"key": "value"}"#.to_string()],
            vec!["int4".to_string(), "text".to_string(), "jsonb".to_string()],
            0,
        )
    }

    #[test]
    fn test_row_detail_creation() {
        let modal = create_test_modal();
        assert_eq!(modal.field_count, 3);
        assert_eq!(modal.selected_field, 0);
        assert_eq!(modal.row_index, 0);
    }

    #[test]
    fn test_navigation_j_k() {
        let mut modal = create_test_modal();
        
        // Initial state
        assert_eq!(modal.selected_field, 0);
        
        // Press j to move down
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let result = modal.handle_key(key);
        assert_eq!(result, RowDetailAction::Continue);
        assert_eq!(modal.selected_field, 1);
        
        // Press j again
        modal.handle_key(key);
        assert_eq!(modal.selected_field, 2);
        
        // Press j at bottom - should stay at 2
        modal.handle_key(key);
        assert_eq!(modal.selected_field, 2);
        
        // Press k to move up
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        modal.handle_key(key);
        assert_eq!(modal.selected_field, 1);
    }

    #[test]
    fn test_close_actions() {
        let mut modal = create_test_modal();
        
        // Esc closes
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(modal.handle_key(key), RowDetailAction::Close);
        
        // q closes
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(modal.handle_key(key), RowDetailAction::Close);
    }

    #[test]
    fn test_edit_action() {
        let mut modal = create_test_modal();
        modal.selected_field = 1;
        
        // e opens editor
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!(modal.handle_key(key), RowDetailAction::Edit { col: 1 });
        
        // Enter also opens editor
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(modal.handle_key(key), RowDetailAction::Edit { col: 1 });
    }

    #[test]
    fn test_go_to_top_bottom() {
        let mut modal = create_test_modal();
        modal.selected_field = 1;
        
        // g goes to top
        let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        modal.handle_key(key);
        assert_eq!(modal.selected_field, 0);
        
        // G goes to bottom
        let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        modal.handle_key(key);
        assert_eq!(modal.selected_field, 2);
    }

    #[test]
    fn test_selected_col() {
        let mut modal = create_test_modal();
        modal.selected_field = 2;
        assert_eq!(modal.selected_col(), 2);
    }

    #[test]
    fn test_truncate_for_display() {
        assert_eq!(truncate_for_display("hello", 10), "hello");
        assert_eq!(truncate_for_display("hello world", 8), "hello w…");
        assert_eq!(truncate_for_display("hi", 2), "hi");
    }
}
