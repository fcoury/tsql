//! Sidebar component with connections list and schema tree.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use super::mouse_util::{is_inside, MOUSE_SCROLL_LINES};
use super::{zone_block, zone_label, UiTheme};
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
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        connections: &ConnectionsFile,
        current_connection: Option<&str>,
        schema_items: &[TreeItem<'static, String>],
        schema_connected: bool,
        schema_loading: bool,
        schema_error: Option<&str>,
        focused_section: SidebarSection,
        has_focus: bool,
        theme: &UiTheme,
    ) {
        // Size connections to its content (label row + one row per entry,
        // or the empty-state hint), capped so schema keeps most of the space.
        let connection_count = connections.sorted().len();
        let connections_height = if connection_count == 0 {
            6
        } else {
            connection_count as u16 + 1
        };
        let connections_cap = (area.height * 2 / 5).max(3);
        let chunks = Layout::vertical([
            Constraint::Length(connections_height.min(connections_cap)),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

        // Store areas for mouse hit testing
        self.connections_area = Some(chunks[0]);
        self.schema_area = Some(chunks[2]);

        frame.render_widget(
            Paragraph::new("").style(Style::default().fg(theme.text).bg(theme.bg_panel)),
            chunks[1],
        );

        self.render_connections(
            frame,
            chunks[0],
            connections,
            current_connection,
            has_focus && focused_section == SidebarSection::Connections,
            theme,
        );
        self.render_schema(
            frame,
            chunks[2],
            schema_items,
            schema_connected,
            schema_loading,
            schema_error,
            has_focus && focused_section == SidebarSection::Schema,
            theme,
        );
    }

    fn render_connections(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        connections: &ConnectionsFile,
        current: Option<&str>,
        focused: bool,
        theme: &UiTheme,
    ) {
        let sorted = connections.sorted();
        let details = if sorted.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(
                format!(" · {}", sorted.len()),
                theme.text_muted,
            )]
        };
        let block = zone_block(
            zone_label("CONNECTIONS", details, focused, theme.accent, theme),
            theme.bg_panel,
            theme.text,
            focused,
            theme.accent,
        );
        if sorted.is_empty() {
            let empty = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No saved connections yet",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::raw("Press "),
                    Span::styled("a", Style::default().fg(theme.warning)),
                    Span::raw(" to add one, or"),
                ]),
                Line::from(vec![
                    Span::styled("Ctrl+Shift+C", Style::default().fg(theme.warning)),
                    Span::raw(" for the full manager."),
                ]),
            ])
            .block(block)
            .wrap(Wrap { trim: true });
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
                        .fg(theme.success)
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
            theme.selection
        } else {
            Style::default().fg(theme.accent)
        };

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, area, &mut self.connections_state);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_schema(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        schema_items: &[TreeItem<'static, String>],
        connected: bool,
        loading: bool,
        error: Option<&str>,
        focused: bool,
        theme: &UiTheme,
    ) {
        let table_count: usize = schema_items.iter().map(|item| item.children().len()).sum();
        let details = if table_count == 0 {
            Vec::new()
        } else {
            vec![Span::styled(
                format!(" · {table_count} tables"),
                theme.text_muted,
            )]
        };
        let block = zone_block(
            zone_label("SCHEMA", details, focused, theme.accent, theme),
            theme.bg_panel,
            theme.text,
            focused,
            theme.accent,
        );

        // Handle loading state
        if loading {
            let loading_text = Paragraph::new("Loading schema...")
                .block(block)
                .style(Style::default().fg(theme.warning));
            frame.render_widget(loading_text, area);
            return;
        }

        // Handle error state
        if let Some(err) = error {
            let error_text = Paragraph::new(format!("Error: {}\nPress 'r' to retry", err))
                .block(block)
                .style(Style::default().fg(theme.error));
            frame.render_widget(error_text, area);
            return;
        }

        // Distinguish a disconnected workspace from a connected database with no relations.
        if schema_items.is_empty() {
            let message = if connected {
                "No schema items · r refresh"
            } else {
                "Connect to view schema"
            };
            let empty = Paragraph::new(message)
                .block(block)
                .style(Style::default().fg(theme.text_muted));
            frame.render_widget(empty, area);
            return;
        }

        let highlight_style = if focused {
            theme.selection
        } else {
            Style::default().fg(theme.accent)
        };

        match Tree::new(schema_items) {
            Ok(tree) => {
                let tree = tree
                    .block(block)
                    .highlight_style(highlight_style)
                    .highlight_symbol("› ");
                frame.render_stateful_widget(tree, area, &mut self.schema_state);
            }
            Err(e) => {
                let err =
                    Paragraph::new(format!("Schema tree build failed: {}\n(retry with `r`)", e))
                        .block(block)
                        .style(Style::default().fg(theme.error));
                frame.render_widget(err, area);
            }
        }
    }

    /// Move selection up in connections list by the specified amount.
    ///
    /// # Arguments
    /// * `total_count` - Total number of connections (for bounds checking)
    /// * `amount` - Number of items to move up (default 1)
    pub fn connections_up_by(&mut self, total_count: usize, amount: usize) {
        if let Some(selected) = self.connections_state.selected() {
            let new_selected = selected.saturating_sub(amount);
            self.connections_state.select(Some(new_selected));
            self.selected_connection = Some(new_selected);
        } else if total_count > 0 {
            self.connections_state.select(Some(0));
            self.selected_connection = Some(0);
        }
    }

    /// Move selection up in connections list by 1.
    pub fn connections_up(&mut self, total_count: usize) {
        self.connections_up_by(total_count, 1);
    }

    /// Move selection down in connections list by the specified amount.
    ///
    /// # Arguments
    /// * `total_count` - Total number of connections (for bounds checking)
    /// * `amount` - Number of items to move down (default 1)
    pub fn connections_down_by(&mut self, total_count: usize, amount: usize) {
        if total_count == 0 {
            return;
        }
        if let Some(selected) = self.connections_state.selected() {
            let new_selected = (selected + amount).min(total_count - 1);
            self.connections_state.select(Some(new_selected));
            self.selected_connection = Some(new_selected);
        } else {
            self.connections_state.select(Some(0));
            self.selected_connection = Some(0);
        }
    }

    /// Move selection down in connections list by 1.
    pub fn connections_down(&mut self, total_count: usize) {
        self.connections_down_by(total_count, 1);
    }

    /// Select the first connection in the list
    pub fn select_first_connection(&mut self) {
        self.connections_state.select(Some(0));
        self.selected_connection = Some(0);
    }

    /// Get selected connection name
    pub fn get_selected_connection<'a>(
        &self,
        connections: &'a ConnectionsFile,
    ) -> Option<&'a ConnectionEntry> {
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

    /// Select the first item in the schema tree if nothing is selected
    pub fn select_first_schema_if_empty(&mut self) {
        if self.schema_state.selected().is_empty() {
            self.schema_state.select_first();
        }
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
                    if is_inside(x, y, conn_area) {
                        return self.handle_connections_click(y, conn_area, connections);
                    }
                }

                // Check schema area
                if let Some(schema_area) = self.schema_area {
                    if is_inside(x, y, schema_area) {
                        return self.handle_schema_click(x, y, schema_area);
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_over_connections(x, y) {
                    let total_count = connections.sorted().len();
                    self.connections_up_by(total_count, MOUSE_SCROLL_LINES);
                    return (None, Some(SidebarSection::Connections));
                }
                if self.is_over_schema(x, y) {
                    for _ in 0..MOUSE_SCROLL_LINES {
                        self.schema_up();
                    }
                    return (None, Some(SidebarSection::Schema));
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_over_connections(x, y) {
                    let total_count = connections.sorted().len();
                    self.connections_down_by(total_count, MOUSE_SCROLL_LINES);
                    return (None, Some(SidebarSection::Connections));
                }
                if self.is_over_schema(x, y) {
                    for _ in 0..MOUSE_SCROLL_LINES {
                        self.schema_down();
                    }
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
        // Calculate visual row within the list (subtract 1 for border)
        let visual_row = y.saturating_sub(conn_area.y + 1) as usize;

        // Add scroll offset to get actual index into the list
        let scroll_offset = self.connections_state.offset();
        let actual_index = scroll_offset + visual_row;

        let sorted = connections.sorted();

        if actual_index < sorted.len() {
            self.connections_state.select(Some(actual_index));
            self.selected_connection = Some(actual_index);
            let name = sorted[actual_index].name.clone();
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
        x: u16,
        y: u16,
        _schema_area: Rect,
    ) -> (Option<SidebarAction>, Option<SidebarSection>) {
        // TreeState's click_at expects absolute screen coordinates, not relative ones.
        // The tree widget stores absolute y positions in last_rendered_identifiers
        // during render, so we pass the mouse coordinates directly.
        use ratatui::layout::Position;
        self.schema_state.click_at(Position::new(x, y));

        (None, Some(SidebarSection::Schema))
    }

    /// Check if mouse is over the connections section
    fn is_over_connections(&self, x: u16, y: u16) -> bool {
        self.connections_area
            .map(|area| is_inside(x, y, area))
            .unwrap_or(false)
    }

    /// Check if mouse is over the schema section
    fn is_over_schema(&self, x: u16, y: u16) -> bool {
        self.schema_area
            .map(|area| is_inside(x, y, area))
            .unwrap_or(false)
    }

    /// Get all currently expanded node paths from the schema tree.
    /// Returns a Vec of identifier paths for serialization.
    pub fn get_expanded_nodes(&self) -> Vec<Vec<String>> {
        self.schema_state.opened().iter().cloned().collect()
    }

    /// Restore expanded nodes from saved state.
    /// Opens each node path in the tree.
    pub fn restore_expanded_nodes(&mut self, paths: &[Vec<String>]) {
        for path in paths {
            self.schema_state.open(path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;

    #[test]
    fn render_uses_stable_zones_and_excludes_spacer_from_hit_areas() {
        let backend = TestBackend::new(32, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut sidebar = Sidebar::new();
        let connections = ConnectionsFile::new();
        let theme = UiTheme::fallback();

        terminal
            .draw(|frame| {
                sidebar.render(
                    frame,
                    frame.area(),
                    &connections,
                    None,
                    &[],
                    true,
                    false,
                    None,
                    SidebarSection::Connections,
                    true,
                    &theme,
                );
            })
            .unwrap();

        let connections_area = sidebar.connections_area.unwrap();
        let schema_area = sidebar.schema_area.unwrap();
        let spacer_y = connections_area.bottom();
        let buffer = terminal.backend().buffer();

        let first_row = row_text(buffer, 0);
        assert!(first_row.contains("CONNECTIONS"));
        assert!(row_text(buffer, schema_area.y).contains("SCHEMA"));
        assert!(row_text(buffer, schema_area.y + 1).contains("No schema items"));
        assert_eq!(
            buffer.cell((0, connections_area.y + 1)).unwrap().fg,
            theme.accent
        );
        assert_eq!(
            buffer.cell((0, schema_area.y + 1)).unwrap().fg,
            theme.bg_panel
        );
        assert_eq!(buffer.cell((5, spacer_y)).unwrap().bg, theme.bg_panel);
        assert!(!sidebar.is_over_connections(5, spacer_y));
        assert!(!sidebar.is_over_schema(5, spacer_y));
    }

    #[test]
    fn disconnected_schema_prompts_for_connection() {
        let backend = TestBackend::new(32, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut sidebar = Sidebar::new();
        let connections = ConnectionsFile::new();
        let theme = UiTheme::fallback();

        terminal
            .draw(|frame| {
                sidebar.render(
                    frame,
                    frame.area(),
                    &connections,
                    None,
                    &[],
                    false,
                    false,
                    None,
                    SidebarSection::Schema,
                    true,
                    &theme,
                );
            })
            .unwrap();

        let schema_area = sidebar.schema_area.unwrap();
        assert!(row_text(terminal.backend().buffer(), schema_area.y + 1)
            .contains("Connect to view schema"));
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        (buffer.area.x..buffer.area.right())
            .map(|x| buffer.cell((x, y)).unwrap().symbol())
            .collect()
    }
}
