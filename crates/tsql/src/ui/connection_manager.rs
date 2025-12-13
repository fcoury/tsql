//! Connection Manager modal for browsing and managing database connections.
//!
//! This modal provides:
//! - List of saved connections with status indicators
//! - Vim-like navigation (j/k, g/G, Ctrl+d/u)
//! - Actions: connect, add, edit, delete, set favorite
//! - Visual indicators for connected state and favorites

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Frame;

use super::mouse_util::is_inside;
use crate::config::{ConnectionEntry, ConnectionsFile};

/// Result of handling a key event in the connection manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionManagerAction {
    /// Continue viewing, nothing special happened.
    Continue,
    /// Close the manager without action.
    Close,
    /// Connect to the selected connection.
    Connect {
        /// The connection entry to connect to
        entry: ConnectionEntry,
    },
    /// Open the add connection form.
    Add,
    /// Open the edit form for the selected connection.
    Edit {
        /// The connection entry to edit
        entry: ConnectionEntry,
    },
    /// Delete the selected connection (requires confirmation).
    Delete {
        /// The connection name to delete
        name: String,
    },
    /// Set/change favorite for the selected connection.
    SetFavorite {
        /// The connection name
        name: String,
        /// Current favorite position (for display in picker)
        current: Option<u8>,
    },
    /// Show a status message
    StatusMessage(String),
}

/// Modal for managing database connections.
pub struct ConnectionManagerModal {
    /// All connections from the file
    connections: Vec<ConnectionEntry>,
    /// Currently selected index
    selected: usize,
    /// Scroll offset for the list
    scroll_offset: usize,
    /// Name of currently connected connection (if any)
    connected_name: Option<String>,
    /// Visible height (set during render)
    visible_height: usize,
    /// Modal area (set during render, used for mouse hit testing)
    modal_area: Option<Rect>,
    /// List area (set during render, used for mouse item selection)
    list_area: Option<Rect>,
}

impl ConnectionManagerModal {
    /// Create a new connection manager modal.
    pub fn new(connections_file: &ConnectionsFile, connected_name: Option<String>) -> Self {
        // Get connections sorted by favorite first, then alphabetically
        let connections: Vec<ConnectionEntry> =
            connections_file.sorted().into_iter().cloned().collect();

        Self {
            connections,
            selected: 0,
            scroll_offset: 0,
            connected_name,
            visible_height: 10,
            modal_area: None,
            list_area: None,
        }
    }

    /// Update the connections list (e.g., after add/edit/delete).
    pub fn update_connections(&mut self, connections_file: &ConnectionsFile) {
        let old_selected_name = self.connections.get(self.selected).map(|c| c.name.clone());

        self.connections = connections_file.sorted().into_iter().cloned().collect();

        // Try to preserve selection by name
        if let Some(name) = old_selected_name {
            if let Some(idx) = self.connections.iter().position(|c| c.name == name) {
                self.selected = idx;
            }
        }

        // Ensure selection is valid
        if self.selected >= self.connections.len() && !self.connections.is_empty() {
            self.selected = self.connections.len() - 1;
        }

        self.ensure_selected_visible();
    }

    /// Update the connected connection name.
    pub fn set_connected(&mut self, name: Option<String>) {
        self.connected_name = name;
    }

    /// Get the currently selected connection.
    pub fn selected_connection(&self) -> Option<&ConnectionEntry> {
        self.connections.get(self.selected)
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConnectionManagerAction {
        // Handle empty state specially
        if self.connections.is_empty() {
            return match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                    ConnectionManagerAction::Close
                }
                (KeyCode::Char('a'), KeyModifiers::NONE) => ConnectionManagerAction::Add,
                _ => ConnectionManagerAction::Continue,
            };
        }

        match (key.code, key.modifiers) {
            // Close
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                ConnectionManagerAction::Close
            }

            // Connect to selected
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::Connect {
                        entry: entry.clone(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Add new connection
            (KeyCode::Char('a'), KeyModifiers::NONE) => ConnectionManagerAction::Add,

            // Edit selected connection
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::Edit {
                        entry: entry.clone(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Delete selected connection
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::Delete {
                        name: entry.name.clone(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Set favorite
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::SetFavorite {
                        name: entry.name.clone(),
                        current: entry.favorite,
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Navigation: down
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.move_down();
                ConnectionManagerAction::Continue
            }

            // Navigation: up
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.move_up();
                ConnectionManagerAction::Continue
            }

            // Half page down (Ctrl-d)
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                let amount = self.visible_height / 2;
                for _ in 0..amount {
                    self.move_down();
                }
                ConnectionManagerAction::Continue
            }

            // Half page up (Ctrl-u)
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                let amount = self.visible_height / 2;
                for _ in 0..amount {
                    self.move_up();
                }
                ConnectionManagerAction::Continue
            }

            // Full page down
            (KeyCode::Char('f'), KeyModifiers::CONTROL)
            | (KeyCode::PageDown, KeyModifiers::NONE) => {
                let amount = self.visible_height.saturating_sub(2);
                for _ in 0..amount {
                    self.move_down();
                }
                ConnectionManagerAction::Continue
            }

            // Full page up
            (KeyCode::Char('b'), KeyModifiers::CONTROL) | (KeyCode::PageUp, KeyModifiers::NONE) => {
                let amount = self.visible_height.saturating_sub(2);
                for _ in 0..amount {
                    self.move_up();
                }
                ConnectionManagerAction::Continue
            }

            // Go to top (g)
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.selected = 0;
                self.scroll_offset = 0;
                ConnectionManagerAction::Continue
            }

            // Go to bottom (G)
            (KeyCode::Char('G'), KeyModifiers::SHIFT)
            | (KeyCode::Char('G'), KeyModifiers::NONE) => {
                if !self.connections.is_empty() {
                    self.selected = self.connections.len() - 1;
                    self.ensure_selected_visible();
                }
                ConnectionManagerAction::Continue
            }

            _ => ConnectionManagerAction::Continue,
        }
    }

    fn move_down(&mut self) {
        if !self.connections.is_empty() && self.selected < self.connections.len() - 1 {
            self.selected += 1;
            self.ensure_selected_visible();
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.ensure_selected_visible();
        }
    }

    fn ensure_selected_visible(&mut self) {
        if self.visible_height == 0 {
            return;
        }

        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }

        if self.selected >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.selected - self.visible_height + 1;
        }
    }

    /// Handle a mouse event and return the resulting action.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> ConnectionManagerAction {
        let (x, y) = (mouse.column, mouse.row);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if click is outside modal - close if so
                if let Some(modal) = self.modal_area {
                    if !is_inside(x, y, modal) {
                        return ConnectionManagerAction::Close;
                    }
                }

                // Check if click is in list area - select and connect
                if let Some(list_area) = self.list_area {
                    if is_inside(x, y, list_area) && !self.connections.is_empty() {
                        // Calculate which row was clicked
                        let row_in_list = (y - list_area.y) as usize;
                        let clicked_index = self.scroll_offset + row_in_list;

                        if clicked_index < self.connections.len() {
                            // Select the item and connect
                            self.selected = clicked_index;
                            return ConnectionManagerAction::Connect {
                                entry: self.connections[clicked_index].clone(),
                            };
                        }
                    }
                }

                ConnectionManagerAction::Continue
            }
            MouseEventKind::ScrollUp => {
                // Only scroll if mouse is inside modal
                if self.is_mouse_inside(x, y) {
                    for _ in 0..3 {
                        self.move_up();
                    }
                }
                ConnectionManagerAction::Continue
            }
            MouseEventKind::ScrollDown => {
                // Only scroll if mouse is inside modal
                if self.is_mouse_inside(x, y) {
                    for _ in 0..3 {
                        self.move_down();
                    }
                }
                ConnectionManagerAction::Continue
            }
            _ => ConnectionManagerAction::Continue,
        }
    }

    /// Check if mouse coordinates are inside the modal.
    fn is_mouse_inside(&self, x: u16, y: u16) -> bool {
        self.modal_area
            .map(|modal| is_inside(x, y, modal))
            .unwrap_or(false)
    }

    /// Render the connection manager modal.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate modal size (70% width, 60% height)
        let modal_width = ((area.width as f32 * 0.70) as u16).clamp(50, 80);
        let modal_height = ((area.height as f32 * 0.60) as u16).clamp(12, 30);
        let modal_x = (area.width - modal_width) / 2;
        let modal_y = (area.height - modal_height) / 2;

        let modal_area = Rect {
            x: modal_x,
            y: modal_y,
            width: modal_width,
            height: modal_height,
        };

        // Store modal area for mouse hit testing
        self.modal_area = Some(modal_area);

        // Clear the background
        frame.render_widget(Clear, modal_area);

        // Build title
        let title = format!(" Connections ({}) ", self.connections.len());

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Layout: list area, separator, help line
        let chunks = Layout::vertical([
            Constraint::Min(1),    // List
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Help
        ])
        .split(inner);

        // Render the list
        self.render_list(frame, chunks[0]);

        // Render separator
        let sep = Paragraph::new("─".repeat(chunks[1].width as usize))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(sep, chunks[1]);

        // Render help line
        self.render_help(frame, chunks[2]);
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        self.visible_height = area.height as usize;

        if self.connections.is_empty() {
            let empty_msg = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No connections saved",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::raw("Press "),
                    Span::styled("a", Style::default().fg(Color::Yellow)),
                    Span::raw(" to add a new connection"),
                ]),
            ])
            .alignment(ratatui::layout::Alignment::Center);
            frame.render_widget(empty_msg, area);
            return;
        }

        let total_items = self.connections.len();
        let needs_scrollbar = total_items > self.visible_height;

        // Reserve space for scrollbar if needed
        let (list_area, scrollbar_area) = if needs_scrollbar {
            let chunks =
                Layout::horizontal([Constraint::Min(1), Constraint::Length(1)]).split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Store the actual clickable list area (excluding scrollbar) for mouse hit testing
        self.list_area = Some(list_area);

        // Build list items
        let items: Vec<ListItem> = self
            .connections
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(self.visible_height)
            .map(|(i, conn)| self.render_connection_item(conn, i == self.selected))
            .collect();

        let list = List::new(items);
        frame.render_widget(list, list_area);

        // Render scrollbar if needed
        if let Some(sb_area) = scrollbar_area {
            let scrollbar = if sb_area.height >= 7 {
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

            let mut scrollbar_state = ScrollbarState::new(total_items).position(self.scroll_offset);

            frame.render_stateful_widget(scrollbar, sb_area, &mut scrollbar_state);
        }
    }

    fn render_connection_item(
        &self,
        conn: &ConnectionEntry,
        is_selected: bool,
    ) -> ListItem<'static> {
        let mut spans = Vec::new();

        // Favorite indicator (1-9 or space)
        if let Some(fav) = conn.favorite {
            spans.push(Span::styled(
                format!("{}", fav),
                Style::default().fg(Color::Yellow),
            ));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));

        // Connected indicator
        let is_connected = self
            .connected_name
            .as_ref()
            .map(|n| n == &conn.name)
            .unwrap_or(false);

        if is_connected {
            spans.push(Span::styled("●", Style::default().fg(Color::Green)));
        } else {
            spans.push(Span::styled("○", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::raw(" "));

        // Connection name with color
        let name_color = conn.color.to_ratatui_color().unwrap_or(Color::White);
        spans.push(Span::styled(
            format!("{:<16}", truncate_str(&conn.name, 16)),
            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));

        // Connection details
        let details = conn.short_display();
        spans.push(Span::styled(details, Style::default().fg(Color::DarkGray)));

        let line = Line::from(spans);

        let style = if is_selected {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };

        ListItem::new(line).style(style)
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_spans = vec![
            Span::styled("[a]", Style::default().fg(Color::Yellow)),
            Span::raw("dd "),
            Span::styled("[e]", Style::default().fg(Color::Yellow)),
            Span::raw("dit "),
            Span::styled("[d]", Style::default().fg(Color::Yellow)),
            Span::raw("elete "),
            Span::styled("[f]", Style::default().fg(Color::Yellow)),
            Span::raw("avorite "),
            Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
            Span::raw(" connect "),
            Span::styled("[q]", Style::default().fg(Color::Yellow)),
            Span::raw(" close"),
        ];

        let help =
            Paragraph::new(Line::from(help_spans)).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(help, area);
    }
}

/// Truncate a string to a maximum length, adding ellipsis if needed.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        s.chars().take(max_len).collect()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionColor;

    fn create_test_connections() -> ConnectionsFile {
        let mut file = ConnectionsFile::new();

        let entries = vec![
            ConnectionEntry {
                name: "local".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                database: "mydb".to_string(),
                user: "postgres".to_string(),
                favorite: Some(1),
                color: ConnectionColor::Green,
                ..Default::default()
            },
            ConnectionEntry {
                name: "staging".to_string(),
                host: "staging.example.com".to_string(),
                port: 5432,
                database: "staging".to_string(),
                user: "readonly".to_string(),
                favorite: Some(2),
                color: ConnectionColor::Yellow,
                ..Default::default()
            },
            ConnectionEntry {
                name: "production".to_string(),
                host: "prod.example.com".to_string(),
                port: 5432,
                database: "prod".to_string(),
                user: "admin".to_string(),
                color: ConnectionColor::Red,
                ..Default::default()
            },
        ];

        for e in entries {
            file.add(e).unwrap();
        }

        file
    }

    #[test]
    fn test_connection_manager_creation() {
        let file = create_test_connections();
        let manager = ConnectionManagerModal::new(&file, None);

        assert_eq!(manager.connections.len(), 3);
        assert_eq!(manager.selected, 0);
        assert!(!manager.is_empty());
    }

    #[test]
    fn test_connection_manager_sorted_order() {
        let file = create_test_connections();
        let manager = ConnectionManagerModal::new(&file, None);

        // Should be sorted: favorites first (by number), then alphabetical
        assert_eq!(manager.connections[0].name, "local"); // favorite 1
        assert_eq!(manager.connections[1].name, "staging"); // favorite 2
        assert_eq!(manager.connections[2].name, "production"); // no favorite
    }

    #[test]
    fn test_navigation_down() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        assert_eq!(manager.selected, 0);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Continue);
        assert_eq!(manager.selected, 1);

        manager.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(manager.selected, 2);

        // At bottom, shouldn't go further
        manager.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(manager.selected, 2);
    }

    #[test]
    fn test_navigation_up() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        manager.selected = 2;

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Continue);
        assert_eq!(manager.selected, 1);

        manager.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(manager.selected, 0);

        // At top, shouldn't go further
        manager.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(manager.selected, 0);
    }

    #[test]
    fn test_go_to_top_bottom() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);
        manager.visible_height = 10;

        // Go to bottom
        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(action, ConnectionManagerAction::Continue);
        assert_eq!(manager.selected, 2);

        // Go to top
        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Continue);
        assert_eq!(manager.selected, 0);
    }

    #[test]
    fn test_connect_action() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match action {
            ConnectionManagerAction::Connect { entry } => {
                assert_eq!(entry.name, "local");
            }
            _ => panic!("Expected Connect action"),
        }
    }

    #[test]
    fn test_add_action() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Add);
    }

    #[test]
    fn test_edit_action() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

        match action {
            ConnectionManagerAction::Edit { entry } => {
                assert_eq!(entry.name, "local");
            }
            _ => panic!("Expected Edit action"),
        }
    }

    #[test]
    fn test_delete_action() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        // Select staging (index 1)
        manager.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        match action {
            ConnectionManagerAction::Delete { name } => {
                assert_eq!(name, "staging");
            }
            _ => panic!("Expected Delete action"),
        }
    }

    #[test]
    fn test_favorite_action() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

        match action {
            ConnectionManagerAction::SetFavorite { name, current } => {
                assert_eq!(name, "local");
                assert_eq!(current, Some(1));
            }
            _ => panic!("Expected SetFavorite action"),
        }
    }

    #[test]
    fn test_close_actions() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Close);

        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Close);
    }

    #[test]
    fn test_empty_manager() {
        let file = ConnectionsFile::new();
        let mut manager = ConnectionManagerModal::new(&file, None);

        assert!(manager.is_empty());

        // Most actions should just continue or close
        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Continue);

        // But 'a' should still work
        let action = manager.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(action, ConnectionManagerAction::Add);
    }

    #[test]
    fn test_connected_indicator() {
        let file = create_test_connections();
        let manager = ConnectionManagerModal::new(&file, Some("staging".to_string()));

        // The connected name should be set
        assert_eq!(manager.connected_name, Some("staging".to_string()));
    }

    #[test]
    fn test_update_connections() {
        let file = create_test_connections();
        let mut manager = ConnectionManagerModal::new(&file, None);

        // Select staging
        manager.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(manager.selected_connection().unwrap().name, "staging");

        // Update with same file - selection should be preserved
        manager.update_connections(&file);
        assert_eq!(manager.selected_connection().unwrap().name, "staging");
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("short", 10), "short");
        assert_eq!(truncate_str("this is a long string", 10), "this is...");
        assert_eq!(truncate_str("abc", 3), "abc");
        assert_eq!(truncate_str("abcd", 3), "abc");
    }
}
