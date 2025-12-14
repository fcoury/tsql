//! A styled help popup widget with sections, keybinding highlighting, and scrolling.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use super::mouse_util::{is_inside, MOUSE_SCROLL_LINES};

/// Result of handling a key event in the help popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpAction {
    /// Continue showing the help popup.
    Continue,
    /// Close the help popup.
    Close,
}

/// A single keybinding entry.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    /// The key or key combination (e.g., "j/k", "Ctrl-r", "Space").
    pub keys: &'static str,
    /// Description of what the key does.
    pub description: &'static str,
}

impl KeyBinding {
    pub const fn new(keys: &'static str, description: &'static str) -> Self {
        Self { keys, description }
    }
}

/// A section in the help popup.
#[derive(Debug, Clone)]
pub struct HelpSection {
    /// Section title (e.g., "Navigation", "Editing").
    pub title: &'static str,
    /// Keybindings in this section.
    pub bindings: &'static [KeyBinding],
}

impl HelpSection {
    pub const fn new(title: &'static str, bindings: &'static [KeyBinding]) -> Self {
        Self { title, bindings }
    }
}

/// The help popup widget with scrolling support.
pub struct HelpPopup {
    /// All help sections.
    sections: &'static [HelpSection],
    /// Current scroll offset (in lines).
    scroll_offset: usize,
    /// Total number of renderable lines.
    total_lines: usize,
    /// Visible height (set during render).
    visible_height: usize,
    /// Popup area (set during render, used for mouse hit testing).
    popup_area: Option<Rect>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Help Content Definition
// ─────────────────────────────────────────────────────────────────────────────

const GLOBAL: HelpSection = HelpSection::new(
    "Global",
    &[
        KeyBinding::new("Tab", "Switch focus (Query/Grid)"),
        KeyBinding::new("Esc", "Return to normal mode / close popup"),
        KeyBinding::new("q", "Quit application"),
        KeyBinding::new("?", "Toggle this help"),
        KeyBinding::new("Ctrl+O", "Open connection picker"),
        KeyBinding::new("Ctrl+Shift+C", "Open connection manager"),
        KeyBinding::new("Ctrl+B", "Toggle sidebar"),
    ],
);

const GOTO: HelpSection = HelpSection::new(
    "Go To (g prefix)",
    &[
        KeyBinding::new("gg", "Go to first row / document start"),
        KeyBinding::new("ge", "Go to editor"),
        KeyBinding::new("gc", "Go to connections sidebar"),
        KeyBinding::new("gt", "Go to tables/schema sidebar"),
        KeyBinding::new("gr", "Go to results grid"),
    ],
);

const SIDEBAR_SCHEMA: HelpSection = HelpSection::new(
    "Sidebar - Schema",
    &[
        KeyBinding::new("h/j/k/l or arrows", "Navigate tree"),
        KeyBinding::new("Space", "Toggle node expand/collapse"),
        KeyBinding::new("Enter (schema)", "Toggle schema expand/collapse"),
        KeyBinding::new("Enter (column)", "Insert column name"),
        KeyBinding::new("Enter (table) then s", "Insert SELECT template"),
        KeyBinding::new("Enter (table) then i", "Insert INSERT template"),
        KeyBinding::new("Enter (table) then u", "Insert UPDATE template"),
        KeyBinding::new("Enter (table) then d", "Insert DELETE template"),
        KeyBinding::new("Enter (table) then n", "Insert table name"),
        KeyBinding::new("r", "Refresh schema"),
    ],
);

const QUERY_NAVIGATION: HelpSection = HelpSection::new(
    "Query Editor - Navigation",
    &[
        KeyBinding::new("h/j/k/l", "Move cursor left/down/up/right"),
        KeyBinding::new("w/b/e", "Word forward/backward/end"),
        KeyBinding::new("0 / $", "Line start/end"),
        KeyBinding::new("gg / G", "Document start/end"),
        KeyBinding::new("Ctrl-d/u", "Scroll half page down/up"),
    ],
);

const QUERY_EDITING: HelpSection = HelpSection::new(
    "Query Editor - Editing",
    &[
        KeyBinding::new("i/a", "Insert before/after cursor"),
        KeyBinding::new("I/A", "Insert at line start/end"),
        KeyBinding::new("o/O", "Open line below/above"),
        KeyBinding::new("x/X", "Delete char forward/backward"),
        KeyBinding::new("dd/cc", "Delete/change entire line"),
        KeyBinding::new("dw/cw", "Delete/change word"),
        KeyBinding::new("D/C", "Delete/change to end of line"),
        KeyBinding::new("u", "Undo"),
        KeyBinding::new("Ctrl-r", "Redo"),
    ],
);

const QUERY_VISUAL: HelpSection = HelpSection::new(
    "Query Editor - Visual Mode",
    &[
        KeyBinding::new("v", "Enter visual mode"),
        KeyBinding::new("h/j/k/l", "Extend selection"),
        KeyBinding::new("y", "Yank (copy) selection"),
        KeyBinding::new("d", "Delete selection"),
        KeyBinding::new("c", "Change selection"),
        KeyBinding::new("Esc", "Cancel visual mode"),
    ],
);

const QUERY_OTHER: HelpSection = HelpSection::new(
    "Query Editor - Other",
    &[
        KeyBinding::new("yy", "Yank (copy) line"),
        KeyBinding::new("p/P", "Paste after/before cursor"),
        KeyBinding::new("/", "Search in editor"),
        KeyBinding::new("n/N", "Next/previous search match"),
        KeyBinding::new("Enter / Ctrl+E", "Execute query"),
        KeyBinding::new("Ctrl-p/n", "Previous/next history"),
        KeyBinding::new("Ctrl-r", "Fuzzy history search"),
        KeyBinding::new("Tab", "Trigger completion"),
        KeyBinding::new(":", "Open command prompt"),
        KeyBinding::new("?", "Toggle this help"),
    ],
);

const GRID_NAVIGATION: HelpSection = HelpSection::new(
    "Results Grid - Navigation",
    &[
        KeyBinding::new("j/k", "Move down/up one row"),
        KeyBinding::new("h/l", "Move left/right one column"),
        KeyBinding::new("H/L", "Scroll left/right"),
        KeyBinding::new("gg / G", "First/last row"),
        KeyBinding::new("0 / $", "First/last column"),
        KeyBinding::new("Ctrl-d/u", "Page down/up"),
    ],
);

const GRID_SELECTION: HelpSection = HelpSection::new(
    "Results Grid - Selection",
    &[
        KeyBinding::new("Space", "Toggle row selection"),
        KeyBinding::new("a", "Select all rows"),
        KeyBinding::new("Esc", "Clear selection"),
    ],
);

const GRID_ACTIONS: HelpSection = HelpSection::new(
    "Results Grid - Actions",
    &[
        KeyBinding::new("c", "Copy cell to clipboard"),
        KeyBinding::new("y", "Yank (copy) row as TSV"),
        KeyBinding::new("Y", "Yank row with headers"),
        KeyBinding::new("e / Enter", "Edit cell"),
        KeyBinding::new("o", "Open row detail view"),
        KeyBinding::new("/", "Search in results"),
        KeyBinding::new("n/N", "Next/previous match"),
    ],
);

const GRID_COLUMNS: HelpSection = HelpSection::new(
    "Results Grid - Columns",
    &[
        KeyBinding::new("+ / >", "Widen column"),
        KeyBinding::new("- / <", "Narrow column"),
        KeyBinding::new("=", "Auto-fit column width"),
    ],
);

const COMMANDS: HelpSection = HelpSection::new(
    "Commands",
    &[
        KeyBinding::new(":connect <url>", "Connect to database"),
        KeyBinding::new(":disconnect", "Disconnect from database"),
        KeyBinding::new(":export <fmt> <path>", "Export results (csv/json/tsv)"),
        KeyBinding::new(":gen <type>", "Generate SQL (update/delete/insert)"),
        KeyBinding::new(":history", "Open history picker"),
        KeyBinding::new(":q / :quit", "Quit application"),
        KeyBinding::new(":help / :?", "Show this help"),
    ],
);

const SCHEMA_COMMANDS: HelpSection = HelpSection::new(
    "Schema Commands (psql-style)",
    &[
        KeyBinding::new(":\\dt", "List tables"),
        KeyBinding::new(":\\d <table>", "Describe table"),
        KeyBinding::new(":\\dn", "List schemas"),
        KeyBinding::new(":\\di", "List indexes"),
        KeyBinding::new(":\\dv", "List views"),
        KeyBinding::new(":\\df", "List functions"),
        KeyBinding::new(":\\l", "List databases"),
        KeyBinding::new(":\\du", "List roles/users"),
        KeyBinding::new(":\\conninfo", "Show connection info"),
    ],
);

const ALL_SECTIONS: &[HelpSection] = &[
    GLOBAL,
    GOTO,
    SIDEBAR_SCHEMA,
    QUERY_NAVIGATION,
    QUERY_EDITING,
    QUERY_VISUAL,
    QUERY_OTHER,
    GRID_NAVIGATION,
    GRID_SELECTION,
    GRID_ACTIONS,
    GRID_COLUMNS,
    COMMANDS,
    SCHEMA_COMMANDS,
];

// ─────────────────────────────────────────────────────────────────────────────
// Implementation
// ─────────────────────────────────────────────────────────────────────────────

impl Default for HelpPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpPopup {
    /// Create a new help popup with default content.
    pub fn new() -> Self {
        let total_lines = Self::calculate_total_lines(ALL_SECTIONS);
        Self {
            sections: ALL_SECTIONS,
            scroll_offset: 0,
            total_lines,
            visible_height: 0,
            popup_area: None,
        }
    }

    /// Calculate total lines needed to render all sections.
    fn calculate_total_lines(sections: &[HelpSection]) -> usize {
        let mut lines = 0;
        for (i, section) in sections.iter().enumerate() {
            // Section header + separator
            lines += 2;
            // Bindings
            lines += section.bindings.len();
            // Blank line after section (except last)
            if i < sections.len() - 1 {
                lines += 1;
            }
        }
        lines
    }

    /// Handle a key event, returning the action to take.
    pub fn handle_key(&mut self, key: KeyEvent) -> HelpAction {
        match key.code {
            // Close help
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => HelpAction::Close,

            // Scroll down
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_down(1);
                HelpAction::Continue
            }

            // Scroll up
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_up(1);
                HelpAction::Continue
            }

            // Half page down (Ctrl-d)
            KeyCode::Char('d')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_down(self.visible_height / 2);
                HelpAction::Continue
            }

            // Full page down (Ctrl-f or PageDown)
            KeyCode::Char('f')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_down(self.visible_height.saturating_sub(2));
                HelpAction::Continue
            }
            KeyCode::PageDown => {
                self.scroll_down(self.visible_height.saturating_sub(2));
                HelpAction::Continue
            }

            // Half page up (Ctrl-u)
            KeyCode::Char('u')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_up(self.visible_height / 2);
                HelpAction::Continue
            }

            // Full page up (Ctrl-b or PageUp)
            KeyCode::Char('b')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_up(self.visible_height.saturating_sub(2));
                HelpAction::Continue
            }
            KeyCode::PageUp => {
                self.scroll_up(self.visible_height.saturating_sub(2));
                HelpAction::Continue
            }

            // Top
            KeyCode::Char('g') => {
                self.scroll_offset = 0;
                HelpAction::Continue
            }

            // Bottom
            KeyCode::Char('G') => {
                self.scroll_to_bottom();
                HelpAction::Continue
            }

            _ => HelpAction::Continue,
        }
    }

    /// Handle a mouse event, returning the action to take.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> HelpAction {
        let (x, y) = (mouse.column, mouse.row);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if click is outside popup - close if so
                if let Some(popup) = self.popup_area {
                    if !is_inside(x, y, popup) {
                        return HelpAction::Close;
                    }
                }
                HelpAction::Continue
            }
            MouseEventKind::ScrollUp => {
                // Only scroll if mouse is inside popup
                if self.is_mouse_inside(x, y) {
                    self.scroll_up(MOUSE_SCROLL_LINES);
                }
                HelpAction::Continue
            }
            MouseEventKind::ScrollDown => {
                // Only scroll if mouse is inside popup
                if self.is_mouse_inside(x, y) {
                    self.scroll_down(MOUSE_SCROLL_LINES);
                }
                HelpAction::Continue
            }
            _ => HelpAction::Continue,
        }
    }

    /// Check if mouse coordinates are inside the popup.
    fn is_mouse_inside(&self, x: u16, y: u16) -> bool {
        self.popup_area
            .map(|popup| is_inside(x, y, popup))
            .unwrap_or(false)
    }

    fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.total_lines.saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    fn scroll_to_bottom(&mut self) {
        let max_scroll = self.total_lines.saturating_sub(self.visible_height);
        self.scroll_offset = max_scroll;
    }

    /// Render the help popup centered on the screen.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate popup size (80% width, 80% height, with min/max)
        let width = (area.width * 80 / 100).clamp(60, 100);
        let height = (area.height * 85 / 100).clamp(20, 50);

        let popup = centered_rect(width, height, area);

        // Store popup area for mouse hit testing
        self.popup_area = Some(popup);

        // Clear background
        frame.render_widget(Clear, popup);

        // Create the outer block
        let block = Block::default()
            .title(" Help ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // Calculate layout: header, separator, content, footer
        let chunks = Layout::vertical([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Separator
            Constraint::Min(1),    // Content
            Constraint::Length(1), // Footer
        ])
        .split(inner);

        // Render header
        self.render_header(frame, chunks[0]);

        // Render separator
        let sep = Paragraph::new("─".repeat(chunks[1].width as usize))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(sep, chunks[1]);

        // Update visible height for scrolling calculations
        self.visible_height = chunks[2].height as usize;

        // Render content with scrolling
        self.render_content(frame, chunks[2]);

        // Render footer with scroll indicator
        self.render_footer(frame, chunks[3]);

        // Render scrollbar if content overflows
        if self.total_lines > self.visible_height {
            self.render_scrollbar(frame, chunks[2]);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header = Line::from(vec![
            Span::styled(
                "tsql",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" - PostgreSQL CLI  "),
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" or ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to close", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn render_content(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (section_idx, section) in self.sections.iter().enumerate() {
            // Section header
            lines.push(Line::from(vec![Span::styled(
                format!(" {} ", section.title),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));

            // Separator under header
            lines.push(Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(Color::DarkGray),
            )));

            // Keybindings
            for binding in section.bindings.iter() {
                lines.push(self.render_keybinding(binding, area.width as usize));
            }

            // Blank line between sections (except last)
            if section_idx < self.sections.len() - 1 {
                lines.push(Line::from(""));
            }
        }

        // Apply scroll offset
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.scroll_offset)
            .take(area.height as usize)
            .collect();

        let content = Paragraph::new(visible_lines);
        frame.render_widget(content, area);
    }

    fn render_keybinding(&self, binding: &KeyBinding, _width: usize) -> Line<'static> {
        // Fixed width for keys column
        let key_width = 20;
        let keys = format!("{:width$}", binding.keys, width = key_width);

        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                keys,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                binding.description.to_string(),
                Style::default().fg(Color::White),
            ),
        ])
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let scroll_info = if self.total_lines > self.visible_height {
            let percent = if self.total_lines == 0 {
                100
            } else {
                ((self.scroll_offset + self.visible_height) * 100 / self.total_lines).min(100)
            };
            format!("{}%", percent)
        } else {
            "All".to_string()
        };

        let footer = Line::from(vec![
            Span::styled(" j/k ", Style::default().fg(Color::Yellow)),
            Span::styled("scroll  ", Style::default().fg(Color::DarkGray)),
            Span::styled(" g/G ", Style::default().fg(Color::Yellow)),
            Span::styled("top/bottom  ", Style::default().fg(Color::DarkGray)),
            Span::styled(" PgUp/PgDn ", Style::default().fg(Color::Yellow)),
            Span::styled("page  ", Style::default().fg(Color::DarkGray)),
            Span::raw(" ".repeat(area.width.saturating_sub(50) as usize)),
            Span::styled(scroll_info, Style::default().fg(Color::Cyan)),
        ]);
        frame.render_widget(Paragraph::new(footer), area);
    }

    fn render_scrollbar(&self, frame: &mut Frame, area: Rect) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        // The scrollbar needs to know the max scroll position (total - visible)
        // and the current scroll position
        let max_scroll = self.total_lines.saturating_sub(self.visible_height);
        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(self.scroll_offset);

        // Render scrollbar in a slightly inset area
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Create a centered rectangle of the given size within the area.
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
