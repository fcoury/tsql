//! Connection Manager modal for browsing and managing database connections.
//!
//! This modal provides:
//! - List of saved connections with status indicators
//! - Vim-like navigation (j/k, g/G, Ctrl+d/u)
//! - Actions: connect, add, edit, delete, set favorite
//! - Visual indicators for connected state and favorites

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use ratatui::Frame;

use super::mouse_util::{is_inside, MOUSE_SCROLL_LINES};
use crate::config::{ConnectionEntry, ConnectionsFile, SortMode};

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
    /// Duplicate the selected connection and open the form pre-populated.
    Duplicate {
        /// The connection entry to duplicate (name will be changed by caller)
        entry: ConnectionEntry,
    },
    /// Yank the selected connection's URL (sanitised, no password) to the
    /// clipboard.
    YankUrl {
        /// The URL text to copy
        url: String,
    },
    /// Yank a `tsql <url>` CLI command for the selected entry.
    YankCli {
        /// The CLI command to copy
        command: String,
    },
    /// Reorder the selected connection up (-1) or down (+1).
    Reorder {
        /// The connection name to reorder
        name: String,
        /// Delta: -1 = up, +1 = down
        delta: i32,
    },
    /// Run a background smoke-test on the selected connection.
    TestConnection {
        /// The connection entry to test
        entry: ConnectionEntry,
    },
    /// User cycled the sort mode — caller should persist it so the
    /// preference survives restarts.
    SortModeChanged {
        /// The new sort mode.
        mode: SortMode,
    },
    /// Show a status message
    StatusMessage(String),
}

/// Modal for managing database connections.
pub struct ConnectionManagerModal {
    /// All connections currently being displayed (post-filter, post-sort).
    connections: Vec<ConnectionEntry>,
    /// Full unfiltered list of connections — kept in sorted order so we can
    /// apply/remove the filter without re-reading from disk.
    all_connections: Vec<ConnectionEntry>,
    /// Currently selected index (into `connections`)
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
    /// Current sort mode.
    sort_mode: SortMode,
    /// Current fuzzy filter text.
    search: String,
    /// Whether `/` search input is active (capturing keystrokes).
    search_active: bool,
    /// Transient toast message shown under the search bar for ~1 draw.
    /// Cleared by the caller via `clear_toast()` once displayed.
    toast: Option<String>,
}

impl ConnectionManagerModal {
    /// Create a new connection manager modal. Uses the last-used sort
    /// mode persisted in the file if present, otherwise the default.
    pub fn new(connections_file: &ConnectionsFile, connected_name: Option<String>) -> Self {
        let sort_mode = connections_file.last_sort_mode;
        let all_connections: Vec<ConnectionEntry> = connections_file
            .sorted_by(sort_mode)
            .into_iter()
            .cloned()
            .collect();
        let connections = all_connections.clone();

        Self {
            connections,
            all_connections,
            selected: 0,
            scroll_offset: 0,
            connected_name,
            visible_height: 10,
            modal_area: None,
            list_area: None,
            sort_mode,
            search: String::new(),
            search_active: false,
            toast: None,
        }
    }

    /// Current sort mode (for UI / external callers).
    pub fn sort_mode(&self) -> SortMode {
        self.sort_mode
    }

    /// Take the transient toast message (called by parent after draw).
    pub fn take_toast(&mut self) -> Option<String> {
        self.toast.take()
    }

    /// Push a transient toast to surface in the modal footer on next draw.
    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(msg.into());
    }

    /// Update the connections list (e.g., after add/edit/delete).
    pub fn update_connections(&mut self, connections_file: &ConnectionsFile) {
        let old_selected_name = self.connections.get(self.selected).map(|c| c.name.clone());

        self.all_connections = connections_file
            .sorted_by(self.sort_mode)
            .into_iter()
            .cloned()
            .collect();
        self.connections = self.filtered_connections();

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
        self.all_connections.is_empty()
    }

    /// Compute the visible connection list after applying the current
    /// search filter on top of the already-sorted `all_connections`.
    fn filtered_connections(&self) -> Vec<ConnectionEntry> {
        if self.search.trim().is_empty() {
            return self.all_connections.clone();
        }
        let needle = self.search.to_lowercase();
        self.all_connections
            .iter()
            .filter(|c| {
                super::super::config::ConnectionEntry::display_string(c)
                    .to_lowercase()
                    .contains(&needle)
                    || c.name.to_lowercase().contains(&needle)
                    || c.host.to_lowercase().contains(&needle)
                    || c.database.to_lowercase().contains(&needle)
                    || c.user.to_lowercase().contains(&needle)
                    || c.folder
                        .as_deref()
                        .map(|s| s.to_lowercase().contains(&needle))
                        .unwrap_or(false)
                    || c.description
                        .as_deref()
                        .map(|s| s.to_lowercase().contains(&needle))
                        .unwrap_or(false)
                    || c.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    }

    fn refresh_visible(&mut self) {
        let old_selected_name = self.connections.get(self.selected).map(|c| c.name.clone());
        self.connections = self.filtered_connections();
        if let Some(name) = old_selected_name {
            if let Some(idx) = self.connections.iter().position(|c| c.name == name) {
                self.selected = idx;
            } else if !self.connections.is_empty() {
                self.selected = self.selected.min(self.connections.len() - 1);
            } else {
                self.selected = 0;
            }
        }
        self.scroll_offset = 0;
        self.ensure_selected_visible();
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConnectionManagerAction {
        // --- Search input mode: everything typed goes into the search
        //     field until Esc / Enter closes it. ---
        if self.search_active {
            return self.handle_search_key(key);
        }

        // Handle empty state specially
        if self.all_connections.is_empty() {
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

            // Search / filter
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.search_active = true;
                ConnectionManagerAction::Continue
            }

            // Cycle sort mode
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.sort_mode = self.sort_mode.next();
                self.resort_by_current_mode();
                self.toast = Some(format!("Sort: {}", self.sort_mode.label()));
                ConnectionManagerAction::SortModeChanged {
                    mode: self.sort_mode,
                }
            }

            // Duplicate selected (shift-d)
            (KeyCode::Char('D'), _) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::Duplicate {
                        entry: entry.clone(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Yank URL (no password)
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::YankUrl {
                        url: entry.to_url(None),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Copy-as-CLI
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::YankCli {
                        command: entry.to_cli_command(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Test connection
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_connection() {
                    ConnectionManagerAction::TestConnection {
                        entry: entry.clone(),
                    }
                } else {
                    ConnectionManagerAction::Continue
                }
            }

            // Reorder up / down (Ctrl-K / Ctrl-J)
            (KeyCode::Char('K'), KeyModifiers::CONTROL)
            | (KeyCode::Char('k'), KeyModifiers::CONTROL) => self.try_reorder(-1),
            (KeyCode::Char('J'), KeyModifiers::CONTROL)
            | (KeyCode::Char('j'), KeyModifiers::CONTROL) => self.try_reorder(1),

            _ => ConnectionManagerAction::Continue,
        }
    }

    /// Emit a `Reorder` action only when the current sort mode actually
    /// tracks the `order` field. In derived modes (Recent, MostUsed,
    /// Alpha, Folder) bumping `order` would mutate hidden state without
    /// moving the visible adjacent row, which is what the user sees —
    /// so we block it and tell them to cycle sort instead.
    fn try_reorder(&mut self, delta: i32) -> ConnectionManagerAction {
        if self.sort_mode != SortMode::FavoritesAlpha {
            self.toast = Some(format!(
                "Reorder works in 'favorites' sort only (press 's' — currently: {})",
                self.sort_mode.label()
            ));
            return ConnectionManagerAction::Continue;
        }
        match self.selected_connection() {
            Some(entry) => ConnectionManagerAction::Reorder {
                name: entry.name.clone(),
                delta,
            },
            None => ConnectionManagerAction::Continue,
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> ConnectionManagerAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.search_active = false;
                self.search.clear();
                self.refresh_visible();
                ConnectionManagerAction::Continue
            }
            (KeyCode::Enter, _) => {
                self.search_active = false;
                ConnectionManagerAction::Continue
            }
            (KeyCode::Backspace, _) => {
                self.search.pop();
                self.refresh_visible();
                ConnectionManagerAction::Continue
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.search.push(c);
                self.refresh_visible();
                ConnectionManagerAction::Continue
            }
            _ => ConnectionManagerAction::Continue,
        }
    }

    fn resort_by_current_mode(&mut self) {
        // Re-sort `all_connections` and re-apply filter. We keep existing
        // clones since the underlying `ConnectionsFile` is not reachable
        // from here.
        let mut sorted = self.all_connections.clone();
        match self.sort_mode {
            SortMode::FavoritesAlpha => sorted.sort_by(|a, b| match (a.favorite, b.favorite) {
                (Some(fa), Some(fb)) => fa.cmp(&fb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a
                    .order
                    .cmp(&b.order)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            }),
            SortMode::Recent => sorted.sort_by(|a, b| {
                b.last_used_at
                    .cmp(&a.last_used_at)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
            SortMode::MostUsed => sorted.sort_by(|a, b| {
                b.use_count
                    .cmp(&a.use_count)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
            SortMode::Alpha => {
                sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }
            SortMode::Folder => sorted.sort_by(|a, b| {
                let fa = a.folder.as_deref().unwrap_or("~");
                let fb = b.folder.as_deref().unwrap_or("~");
                fa.to_lowercase()
                    .cmp(&fb.to_lowercase())
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
        }
        self.all_connections = sorted;
        self.refresh_visible();
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
                    for _ in 0..MOUSE_SCROLL_LINES {
                        self.move_up();
                    }
                }
                ConnectionManagerAction::Continue
            }
            MouseEventKind::ScrollDown => {
                // Only scroll if mouse is inside modal
                if self.is_mouse_inside(x, y) {
                    for _ in 0..MOUSE_SCROLL_LINES {
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
        // Wider modal now that we also show a detail pane. Falls back
        // gracefully on narrow terminals.
        let modal_width = ((area.width as f32 * 0.85) as u16).clamp(60, 120);
        let modal_height = ((area.height as f32 * 0.70) as u16).clamp(14, 36);
        let modal_x = area.width.saturating_sub(modal_width) / 2;
        let modal_y = area.height.saturating_sub(modal_height) / 2;

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

        // Title shows filtered vs total count and current sort mode.
        let total = self.all_connections.len();
        let visible = self.connections.len();
        let title = if visible == total {
            format!(
                " Connections ({}) · sort: {} ",
                total,
                self.sort_mode.label()
            )
        } else {
            format!(
                " Connections ({}/{}) · sort: {} ",
                visible,
                total,
                self.sort_mode.label()
            )
        };

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

        // Vertical: search bar (if active OR non-empty), body, separator, help.
        let show_search_bar = self.search_active || !self.search.is_empty();
        let mut constraints: Vec<Constraint> = Vec::new();
        if show_search_bar {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Min(1));
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));

        let vchunks = Layout::vertical(constraints).split(inner);

        let mut idx = 0;
        if show_search_bar {
            self.render_search_bar(frame, vchunks[idx]);
            idx += 1;
        }
        let body_area = vchunks[idx];
        idx += 1;
        let sep_area = vchunks[idx];
        idx += 1;
        let help_area = vchunks[idx];

        // Body: list + detail pane side by side when wide enough.
        if body_area.width >= 72 && !self.connections.is_empty() {
            let hchunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(body_area);
            self.render_list(frame, hchunks[0]);
            self.render_detail(frame, hchunks[1]);
        } else {
            self.render_list(frame, body_area);
        }

        let sep = Paragraph::new("─".repeat(sep_area.width as usize))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(sep, sep_area);

        self.render_help(frame, help_area);
    }

    fn render_search_bar(&self, frame: &mut Frame, area: Rect) {
        let mut spans = vec![
            Span::styled(
                "/",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(self.search.as_str()),
        ];
        if self.search_active {
            spans.push(Span::styled("▎", Style::default().fg(Color::Yellow)));
        }
        let p = Paragraph::new(Line::from(spans));
        frame.render_widget(p, area);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let Some(entry) = self.selected_connection() else {
            return;
        };

        let mut lines: Vec<Line<'_>> = Vec::new();

        let name_color = entry.color.to_ratatui_color().unwrap_or(Color::White);
        lines.push(Line::from(vec![Span::styled(
            entry.name.as_str(),
            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
        )]));
        let kind_label = match entry.kind {
            crate::config::DbKind::Postgres => "PostgreSQL",
            crate::config::DbKind::Mongo => "MongoDB",
        };
        lines.push(Line::from(Span::styled(
            kind_label,
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));

        lines.push(detail_row_owned("Target", entry.display_string()));
        if let Some(folder) = entry.folder.as_deref() {
            lines.push(detail_row("Folder", folder));
        }
        if !entry.tags.is_empty() {
            lines.push(detail_row_owned("Tags", entry.tags.join(", ")));
        }
        if let Some(app) = entry.application_name.as_deref() {
            lines.push(detail_row("App name", app));
        }
        if let Some(t) = entry.connect_timeout_secs {
            lines.push(detail_row_owned("Timeout", format!("{}s", t)));
        }
        if let Some(ssl) = entry.ssl_mode {
            lines.push(detail_row("SSL", ssl.as_str()));
        }
        lines.push(detail_row("Password", entry.password_source_label()));
        lines.push(detail_row_owned("Last used", entry.last_used_label()));
        lines.push(detail_row_owned("Use count", entry.use_count.to_string()));

        if let Some(desc) = entry.description.as_deref() {
            if !desc.trim().is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Notes",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::raw(desc)));
            }
        }

        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let p = Paragraph::new(lines).wrap(Wrap { trim: true });
        frame.render_widget(p, inner);
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
        if let Some(ref msg) = self.toast {
            let p = Paragraph::new(Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Yellow),
            )))
            .alignment(ratatui::layout::Alignment::Center);
            frame.render_widget(p, area);
            return;
        }
        let help_spans = vec![
            Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
            Span::raw(" connect  "),
            Span::styled("[a]", Style::default().fg(Color::Yellow)),
            Span::raw("dd "),
            Span::styled("[e]", Style::default().fg(Color::Yellow)),
            Span::raw("dit "),
            Span::styled("[D]", Style::default().fg(Color::Yellow)),
            Span::raw("up "),
            Span::styled("[d]", Style::default().fg(Color::Yellow)),
            Span::raw("el "),
            Span::styled("[f]", Style::default().fg(Color::Yellow)),
            Span::raw("av "),
            Span::styled("[t]", Style::default().fg(Color::Yellow)),
            Span::raw("est "),
            Span::styled("[/]", Style::default().fg(Color::Yellow)),
            Span::raw("find "),
            Span::styled("[s]", Style::default().fg(Color::Yellow)),
            Span::raw("ort "),
            Span::styled("[y]", Style::default().fg(Color::Yellow)),
            Span::raw("ank "),
            Span::styled("[c]", Style::default().fg(Color::Yellow)),
            Span::raw("li "),
            Span::styled("[^K/^J]", Style::default().fg(Color::Yellow)),
            Span::raw(" reorder "),
            Span::styled("[q]", Style::default().fg(Color::Yellow)),
            Span::raw(" close"),
        ];

        let help =
            Paragraph::new(Line::from(help_spans)).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(help, area);
    }
}

fn detail_row<'a>(label: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:<10}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value),
    ])
}

fn detail_row_owned(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<10}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value),
    ])
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_reorder_blocked_outside_favorites_sort() {
        // Regression: Ctrl-J/K used to emit a Reorder action regardless
        // of the current sort mode, mutating `order` invisibly when the
        // manager was sorted by Recent / MostUsed / Alpha / Folder.
        let file = create_test_connections();
        let mut m = ConnectionManagerModal::new(&file, None);
        m.sort_mode = SortMode::Recent;

        let action = m.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL));
        assert_eq!(action, ConnectionManagerAction::Continue);
        assert!(
            m.toast
                .as_deref()
                .map(|t| t.contains("favorites"))
                .unwrap_or(false),
            "manager should toast a hint instead of silently mutating order"
        );

        // Switching back to FavoritesAlpha re-enables reorder.
        m.sort_mode = SortMode::FavoritesAlpha;
        let action = m.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL));
        assert!(matches!(
            action,
            ConnectionManagerAction::Reorder { delta: 1, .. }
        ));
    }

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
