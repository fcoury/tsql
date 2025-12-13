//! A generic fuzzy picker widget for ratatui.
//!
//! Provides an interactive popup for selecting items from a list with
//! fuzzy matching and highlighted results.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

/// Result of handling a key event in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction<T> {
    /// Continue showing the picker.
    Continue,
    /// User selected an item.
    Selected(T),
    /// User cancelled (Escape).
    Cancelled,
}

/// A filtered item with match information.
#[derive(Debug, Clone)]
pub struct FilteredItem<T> {
    /// The original item.
    pub item: T,
    /// Original index in the source list.
    pub original_index: usize,
    /// Match score (higher is better).
    pub score: u32,
    /// Character indices that matched (for highlighting).
    pub indices: Vec<u32>,
}

/// A generic fuzzy picker widget.
///
/// Type parameter `T` must implement:
/// - `Clone` for returning selected items
/// - `AsRef<str>` for fuzzy matching
/// - `Display` or provide a custom display function
pub struct FuzzyPicker<T> {
    /// All items in the picker.
    items: Vec<T>,
    /// Filtered items after fuzzy matching.
    filtered: Vec<FilteredItem<T>>,
    /// Current search query.
    query: String,
    /// Cursor position in the query string.
    cursor: usize,
    /// Selected item index in filtered list.
    selected: usize,
    /// Scroll offset for the list.
    scroll_offset: usize,
    /// Title for the popup.
    title: String,
    /// The fuzzy matcher.
    matcher: Matcher,
    /// Function to get display text from item.
    display_fn: fn(&T) -> String,
}

impl<T: Clone + AsRef<str>> FuzzyPicker<T> {
    /// Create a new fuzzy picker with default display (uses AsRef<str>).
    pub fn new(items: Vec<T>, title: impl Into<String>) -> Self {
        Self::with_display(items, title, |item| item.as_ref().to_string())
    }
}

impl<T: Clone> FuzzyPicker<T> {
    /// Create a new fuzzy picker with a custom display function.
    pub fn with_display(
        items: Vec<T>,
        title: impl Into<String>,
        display_fn: fn(&T) -> String,
    ) -> Self {
        let mut picker = Self {
            items,
            filtered: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            scroll_offset: 0,
            title: title.into(),
            matcher: Matcher::new(Config::DEFAULT),
            display_fn,
        };
        picker.update_filtered();
        picker
    }

    /// Get the current query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Get the number of filtered items.
    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    /// Get the total number of items.
    pub fn total_count(&self) -> usize {
        self.items.len()
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> PickerAction<T> {
        match (key.code, key.modifiers) {
            // Cancel.
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                PickerAction::Cancelled
            }

            // Select.
            (KeyCode::Enter, _) => {
                if let Some(item) = self.filtered.get(self.selected) {
                    PickerAction::Selected(item.item.clone())
                } else {
                    PickerAction::Cancelled
                }
            }

            // Navigation.
            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.move_up();
                PickerAction::Continue
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.move_down();
                PickerAction::Continue
            }
            (KeyCode::PageUp, _) => {
                for _ in 0..10 {
                    self.move_up();
                }
                PickerAction::Continue
            }
            (KeyCode::PageDown, _) => {
                for _ in 0..10 {
                    self.move_down();
                }
                PickerAction::Continue
            }
            (KeyCode::Home, KeyModifiers::CONTROL) => {
                self.selected = 0;
                self.scroll_offset = 0;
                PickerAction::Continue
            }
            (KeyCode::End, KeyModifiers::CONTROL) => {
                if !self.filtered.is_empty() {
                    self.selected = self.filtered.len() - 1;
                }
                PickerAction::Continue
            }

            // Query editing.
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.query.insert(self.cursor, c);
                self.cursor += 1;
                self.update_filtered();
                PickerAction::Continue
            }
            (KeyCode::Backspace, _) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.query.remove(self.cursor);
                    self.update_filtered();
                }
                PickerAction::Continue
            }
            (KeyCode::Delete, _) => {
                if self.cursor < self.query.len() {
                    self.query.remove(self.cursor);
                    self.update_filtered();
                }
                PickerAction::Continue
            }
            (KeyCode::Left, _) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                PickerAction::Continue
            }
            (KeyCode::Right, _) => {
                if self.cursor < self.query.len() {
                    self.cursor += 1;
                }
                PickerAction::Continue
            }
            (KeyCode::Home, _) => {
                self.cursor = 0;
                PickerAction::Continue
            }
            (KeyCode::End, _) => {
                self.cursor = self.query.len();
                PickerAction::Continue
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.query.clear();
                self.cursor = 0;
                self.update_filtered();
                PickerAction::Continue
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                // Delete word backwards.
                while self.cursor > 0 && self.query.chars().nth(self.cursor - 1) == Some(' ') {
                    self.cursor -= 1;
                    self.query.remove(self.cursor);
                }
                while self.cursor > 0 && self.query.chars().nth(self.cursor - 1) != Some(' ') {
                    self.cursor -= 1;
                    self.query.remove(self.cursor);
                }
                self.update_filtered();
                PickerAction::Continue
            }

            _ => PickerAction::Continue,
        }
    }

    /// Render the picker as a centered popup.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate popup size based on content.
        let max_width = (area.width as usize * 80 / 100).clamp(40, 100) as u16;
        let max_height = (area.height as usize * 70 / 100).clamp(10, 30) as u16;

        // Calculate actual width needed.
        let content_width = self
            .filtered
            .iter()
            .map(|item| (self.display_fn)(&item.item).len())
            .max()
            .unwrap_or(20)
            .max(self.title.len())
            .max(30) as u16
            + 4; // Padding.

        let width = content_width.min(max_width);
        let height = (self.filtered.len() as u16 + 5).min(max_height); // +5 for borders, input, status.

        let popup = centered_rect(width, height, area);

        // Clear the background.
        frame.render_widget(Clear, popup);

        // Create the block.
        let block = Block::default()
            .title(format!(" {} ", self.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // Layout: input line, separator, list, status.
        let chunks = Layout::vertical([
            Constraint::Length(1), // Input.
            Constraint::Length(1), // Separator.
            Constraint::Min(1),    // List.
            Constraint::Length(1), // Status.
        ])
        .split(inner);

        // Render input line.
        self.render_input(frame, chunks[0]);

        // Render separator.
        let sep = Paragraph::new("─".repeat(chunks[1].width as usize))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(sep, chunks[1]);

        // Render list.
        self.render_list(frame, chunks[2]);

        // Render status.
        self.render_status(frame, chunks[3]);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let mut spans = vec![Span::styled("> ", Style::default().fg(Color::Yellow))];

        // Query text with cursor.
        let query_before: String = self.query.chars().take(self.cursor).collect();
        let cursor_char = self.query.chars().nth(self.cursor).unwrap_or(' ');
        let query_after: String = self.query.chars().skip(self.cursor + 1).collect();

        spans.push(Span::raw(query_before));
        spans.push(Span::styled(
            cursor_char.to_string(),
            Style::default().bg(Color::White).fg(Color::Black),
        ));
        spans.push(Span::raw(query_after));

        let input = Paragraph::new(Line::from(spans));
        frame.render_widget(input, area);
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        let visible_height = area.height as usize;

        // Adjust scroll to keep selected visible.
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected - visible_height + 1;
        }

        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible_height)
            .map(|(i, filtered_item)| {
                let text = (self.display_fn)(&filtered_item.item);
                let is_selected = i == self.selected;

                let line = if filtered_item.indices.is_empty() {
                    // No highlighting needed.
                    Line::from(text)
                } else {
                    // Highlight matched characters.
                    highlight_matches(&text, &filtered_item.indices)
                };

                let style = if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                ListItem::new(line).style(style)
            })
            .collect();

        let list = List::new(items);
        let mut state = ListState::default();

        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let status = format!(
            " {}/{} ",
            if self.filtered.is_empty() {
                0
            } else {
                self.selected + 1
            },
            self.filtered.len()
        );

        let status_widget = Paragraph::new(status).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status_widget, area);
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected < self.filtered.len() - 1 {
            self.selected += 1;
        }
    }

    fn update_filtered(&mut self) {
        self.filtered.clear();

        if self.query.is_empty() {
            // Show all items in reverse order (most recent first for history).
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .rev()
                .map(|(i, item)| FilteredItem {
                    item: item.clone(),
                    original_index: i,
                    score: 0,
                    indices: Vec::new(),
                })
                .collect();
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);

            let mut matches: Vec<FilteredItem<T>> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    let text = (self.display_fn)(item);
                    let mut indices = Vec::new();
                    let mut buf = Vec::new();
                    let haystack = Utf32Str::new(&text, &mut buf);

                    pattern
                        .indices(haystack, &mut self.matcher, &mut indices)
                        .map(|score| FilteredItem {
                            item: item.clone(),
                            original_index: i,
                            score,
                            indices,
                        })
                })
                .collect();

            // Sort by score descending.
            matches.sort_by(|a, b| b.score.cmp(&a.score));
            self.filtered = matches;
        }

        // Reset selection.
        self.selected = 0;
        self.scroll_offset = 0;
    }
}

/// Highlight matched characters in a string.
fn highlight_matches(text: &str, indices: &[u32]) -> Line<'static> {
    let indices_set: std::collections::HashSet<usize> =
        indices.iter().map(|&i| i as usize).collect();

    let mut spans = Vec::new();
    let mut current_span = String::new();
    let mut current_is_match = false;

    for (i, c) in text.chars().enumerate() {
        let is_match = indices_set.contains(&i);

        if is_match != current_is_match && !current_span.is_empty() {
            // Flush current span.
            let style = if current_is_match {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            spans.push(Span::styled(current_span.clone(), style));
            current_span.clear();
        }

        current_span.push(c);
        current_is_match = is_match;
    }

    // Flush remaining.
    if !current_span.is_empty() {
        let style = if current_is_match {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(current_span, style));
    }

    Line::from(spans)
}

/// Helper to create a centered rectangle.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;

    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_picker_creation() {
        let items = vec!["apple", "banana", "cherry"];
        let picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        assert_eq!(picker.total_count(), 3);
        assert_eq!(picker.filtered_count(), 3);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn test_picker_filtering() {
        let items = vec![
            "SELECT * FROM users".to_string(),
            "SELECT * FROM orders".to_string(),
            "INSERT INTO logs".to_string(),
        ];
        let mut picker = FuzzyPicker::new(items, "Test");

        // Initially shows all.
        assert_eq!(picker.filtered_count(), 3);

        // Type to filter.
        picker.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

        assert_eq!(picker.query(), "user");
        // Should match "users".
        assert!(picker.filtered_count() >= 1);
    }

    #[test]
    fn test_picker_navigation() {
        let items = vec!["a", "b", "c"];
        let mut picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        // Initially at 0.
        assert_eq!(picker.selected, 0);

        // Move down.
        picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(picker.selected, 1);

        picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(picker.selected, 2);

        // Can't go past end.
        picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(picker.selected, 2);

        // Move up.
        picker.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn test_picker_selection() {
        let items = vec!["first", "second", "third"];
        let mut picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        // Move to second item.
        picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        // Select.
        let action = picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // Items are shown in reverse order (most recent first), so index 1 is "second".
        match action {
            PickerAction::Selected(item) => {
                assert_eq!(item, "second");
            }
            _ => panic!("Expected Selected action"),
        }
    }

    #[test]
    fn test_picker_cancel() {
        let items = vec!["a", "b"];
        let mut picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        let action = picker.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, PickerAction::Cancelled);
    }

    #[test]
    fn test_picker_backspace() {
        let items = vec!["test"];
        let mut picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        picker.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(picker.query(), "ab");

        picker.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(picker.query(), "a");
    }

    #[test]
    fn test_picker_clear_query() {
        let items = vec!["test"];
        let mut picker: FuzzyPicker<&str> = FuzzyPicker::new(items, "Test");

        picker.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert_eq!(picker.query(), "abc");

        // Ctrl+U clears.
        picker.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn test_highlight_matches() {
        let text = "SELECT * FROM users";
        let indices = vec![0, 1, 2, 14, 15, 16, 17, 18]; // "SEL" and "users"

        let line = highlight_matches(text, &indices);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_empty_picker_selection() {
        let items: Vec<String> = vec![];
        let mut picker = FuzzyPicker::new(items, "Test");

        let action = picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, PickerAction::Cancelled);
    }
}
