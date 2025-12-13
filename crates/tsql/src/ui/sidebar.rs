//! Sidebar component with connections list and schema tree.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::app::SidebarSection;
use crate::config::{ConnectionEntry, ConnectionsFile};

/// Actions that can result from sidebar interactions
#[derive(Debug, Clone)]
pub enum SidebarAction {
    /// Connect to a connection by name
    Connect(String),
    /// Insert text into query editor (table/column name)
    InsertText(String),
    /// Open add connection modal
    OpenAddConnection,
    /// Open edit connection modal
    OpenEditConnection(String),
    /// Refresh schema
    RefreshSchema,
    /// Move focus back to editor
    FocusEditor,
}

/// State for the sidebar component
pub struct Sidebar {
    /// List state for connections (selection, scroll)
    pub connections_state: ListState,
    /// Tree state for schema navigation
    pub schema_state: TreeState<String>,
    /// Currently selected connection index
    pub selected_connection: Option<usize>,
    /// Area of the connections section (for mouse hit testing)
    connections_area: Option<Rect>,
    /// Area of the schema section (for mouse hit testing)
    schema_area: Option<Rect>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            connections_state: ListState::default(),
            schema_state: TreeState::default(),
            selected_connection: None,
            connections_area: None,
            schema_area: None,
        }
    }

    /// Render the sidebar with both connections and schema sections
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        connections: &ConnectionsFile,
        current_connection: Option<&str>,
        schema_items: &[TreeItem<'static, String>],
        schema_loading: bool,
        schema_error: Option<&str>,
        focused_section: SidebarSection,
        has_focus: bool,
    ) {
        // Split sidebar into connections (30%) and schema (70%)
        let chunks = Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .split(area);

        // Store areas for mouse hit testing
        self.connections_area = Some(chunks[0]);
        self.schema_area = Some(chunks[1]);

        self.render_connections(
            frame,
            chunks[0],
            connections,
            current_connection,
            has_focus && focused_section == SidebarSection::Connections,
        );
        self.render_schema(
            frame,
            chunks[1],
            schema_items,
            schema_loading,
            schema_error,
            has_focus && focused_section == SidebarSection::Schema,
        );
    }

    fn render_connections(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        connections: &ConnectionsFile,
        current: Option<&str>,
        focused: bool,
    ) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Connections ")
            .border_style(border_style);

        let sorted = connections.sorted();
        if sorted.is_empty() {
            let empty = Paragraph::new("No connections.\nPress 'a' to add")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, area);
            return;
        }

        let items: Vec<ListItem> = sorted
            .iter()
            .map(|conn| {
                let is_current = Some(conn.name.as_str()) == current;
                let marker = if is_current { "● " } else { "  " };

                let style = if is_current {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                ListItem::new(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(&conn.name, style),
                ]))
            })
            .collect();

        let highlight_style = if focused {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default().fg(Color::Yellow)
        };

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, area, &mut self.connections_state);
    }

    fn render_schema(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        schema_items: &[TreeItem<'static, String>],
        loading: bool,
        error: Option<&str>,
        focused: bool,
    ) {
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Schema ")
            .border_style(border_style);

        // Handle loading state
        if loading {
            let loading_text = Paragraph::new("Loading schema...")
                .block(block)
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(loading_text, area);
            return;
        }

        // Handle error state
        if let Some(err) = error {
            let error_text = Paragraph::new(format!("Error: {}\nPress 'r' to retry", err))
                .block(block)
                .style(Style::default().fg(Color::Red));
            frame.render_widget(error_text, area);
            return;
        }

        // Handle empty/not connected state
        if schema_items.is_empty() {
            let empty = Paragraph::new("Connect to view schema")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, area);
            return;
        }

        let highlight_style = if focused {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default().fg(Color::Yellow)
        };

        let tree = Tree::new(schema_items)
            .expect("valid tree items")
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(tree, area, &mut self.schema_state);
    }

    /// Move selection up in connections list
    pub fn connections_up(&mut self, count: usize) {
        if let Some(selected) = self.connections_state.selected() {
            let new_selected = selected.saturating_sub(1);
            self.connections_state.select(Some(new_selected));
            self.selected_connection = Some(new_selected);
        } else if count > 0 {
            self.connections_state.select(Some(0));
            self.selected_connection = Some(0);
        }
    }

    /// Move selection down in connections list
    pub fn connections_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(selected) = self.connections_state.selected() {
            let new_selected = (selected + 1).min(count - 1);
            self.connections_state.select(Some(new_selected));
            self.selected_connection = Some(new_selected);
        } else {
            self.connections_state.select(Some(0));
            self.selected_connection = Some(0);
        }
    }

    /// Get selected connection name
    pub fn get_selected_connection<'a>(&self, connections: &'a ConnectionsFile) -> Option<&'a ConnectionEntry> {
        let sorted = connections.sorted();
        self.connections_state
            .selected()
            .and_then(|idx| sorted.get(idx).cloned())
    }

    /// Select connection by name (for initial state sync)
    pub fn select_connection_by_name(&mut self, connections: &ConnectionsFile, name: &str) {
        let sorted = connections.sorted();
        if let Some(idx) = sorted.iter().position(|c| c.name == name) {
            self.connections_state.select(Some(idx));
            self.selected_connection = Some(idx);
        }
    }

    /// Toggle tree node expansion or get selected item name
    pub fn schema_toggle(&mut self) -> Option<String> {
        self.schema_state.toggle_selected();
        None
    }

    /// Move up in schema tree
    pub fn schema_up(&mut self) {
        self.schema_state.key_up();
    }

    /// Move down in schema tree
    pub fn schema_down(&mut self) {
        self.schema_state.key_down();
    }

    /// Expand node or move to first child
    pub fn schema_right(&mut self) {
        self.schema_state.key_right();
    }

    /// Collapse node or move to parent
    pub fn schema_left(&mut self) {
        self.schema_state.key_left();
    }

    /// Get the selected schema item identifier (for inserting into query)
    pub fn get_selected_schema_name(&self) -> Option<String> {
        self.schema_state.selected().last().cloned()
    }

    /// Handle mouse events in the sidebar
    ///
    /// Returns a tuple of (action, which_section_was_clicked)
    /// The section is returned so the caller can update focus appropriately
    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        connections: &ConnectionsFile,
    ) -> (Option<SidebarAction>, Option<SidebarSection>) {
        let (x, y) = (mouse.column, mouse.row);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check connections area
                if let Some(conn_area) = self.connections_area {
                    if Self::is_inside(x, y, conn_area) {
                        return self.handle_connections_click(y, conn_area, connections);
                    }
                }

                // Check schema area
                if let Some(schema_area) = self.schema_area {
                    if Self::is_inside(x, y, schema_area) {
                        return self.handle_schema_click(y, schema_area);
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_over_connections(x, y) {
                    let count = connections.sorted().len();
                    self.connections_up(count);
                    return (None, Some(SidebarSection::Connections));
                }
                if self.is_over_schema(x, y) {
                    self.schema_up();
                    return (None, Some(SidebarSection::Schema));
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_over_connections(x, y) {
                    let count = connections.sorted().len();
                    self.connections_down(count);
                    return (None, Some(SidebarSection::Connections));
                }
                if self.is_over_schema(x, y) {
                    self.schema_down();
                    return (None, Some(SidebarSection::Schema));
                }
            }
            _ => {}
        }

        (None, None)
    }

    /// Handle click in the connections area
    fn handle_connections_click(
        &mut self,
        y: u16,
        conn_area: Rect,
        connections: &ConnectionsFile,
    ) -> (Option<SidebarAction>, Option<SidebarSection>) {
        // Calculate which row was clicked (subtract 1 for border)
        let row = y.saturating_sub(conn_area.y + 1) as usize;
        let sorted = connections.sorted();

        if row < sorted.len() {
            self.connections_state.select(Some(row));
            self.selected_connection = Some(row);
            let name = sorted[row].name.clone();
            return (
                Some(SidebarAction::Connect(name)),
                Some(SidebarSection::Connections),
            );
        }

        // Clicked in area but not on an item - just focus
        (None, Some(SidebarSection::Connections))
    }

    /// Handle click in the schema area
    fn handle_schema_click(
        &mut self,
        y: u16,
        schema_area: Rect,
    ) -> (Option<SidebarAction>, Option<SidebarSection>) {
        // Calculate which row was clicked (subtract 1 for border)
        let row = y.saturating_sub(schema_area.y + 1) as usize;

        // Navigate to the clicked row using key_down from the current position
        // First, we need to find the currently selected row
        // TreeState doesn't expose scroll offset directly, so we'll use key navigation
        // to move to approximately the right position

        // For now, just toggle if we click anywhere in the schema
        // More sophisticated click-to-select would require tree widget changes
        // Move down 'row' times from the top (reset first)
        self.schema_state.select_first();
        for _ in 0..row {
            self.schema_state.key_down();
        }

        (None, Some(SidebarSection::Schema))
    }

    /// Check if coordinates are inside a rectangle
    fn is_inside(x: u16, y: u16, area: Rect) -> bool {
        x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
    }

    /// Check if mouse is over the connections section
    fn is_over_connections(&self, x: u16, y: u16) -> bool {
        self.connections_area
            .map(|area| Self::is_inside(x, y, area))
            .unwrap_or(false)
    }

    /// Check if mouse is over the schema section
    fn is_over_schema(&self, x: u16, y: u16) -> bool {
        self.schema_area
            .map(|area| Self::is_inside(x, y, area))
            .unwrap_or(false)
    }
}
