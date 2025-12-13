//! Connection Form modal for adding and editing database connections.
//!
//! This modal provides:
//! - Form fields for connection details (name, host, port, database, user, password)
//! - URL paste field that auto-expands into discrete fields
//! - Color selection
//! - Save to keychain option
//! - Test connection functionality

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::config::{ConnectionColor, ConnectionEntry};

/// Which field is currently focused in the form
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    #[default]
    Name,
    Host,
    Port,
    Database,
    User,
    Password,
    SavePassword,
    Color,
    UrlPaste,
}

impl FormField {
    /// Get the next field in tab order
    /// Order: Name → User → Password → SavePassword → Host → Port → Database → Color → UrlPaste
    pub fn next(self) -> Self {
        match self {
            FormField::Name => FormField::User,
            FormField::User => FormField::Password,
            FormField::Password => FormField::SavePassword,
            FormField::SavePassword => FormField::Host,
            FormField::Host => FormField::Port,
            FormField::Port => FormField::Database,
            FormField::Database => FormField::Color,
            FormField::Color => FormField::UrlPaste,
            FormField::UrlPaste => FormField::Name,
        }
    }

    /// Get the previous field in tab order
    pub fn prev(self) -> Self {
        match self {
            FormField::Name => FormField::UrlPaste,
            FormField::User => FormField::Name,
            FormField::Password => FormField::User,
            FormField::SavePassword => FormField::Password,
            FormField::Host => FormField::SavePassword,
            FormField::Port => FormField::Host,
            FormField::Database => FormField::Port,
            FormField::Color => FormField::Database,
            FormField::UrlPaste => FormField::Color,
        }
    }
}

/// Result of handling a key event in the connection form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionFormAction {
    /// Continue editing
    Continue,
    /// Cancel without saving (no changes or confirmed discard)
    Cancel,
    /// Request to close with unsaved changes (needs confirmation)
    RequestClose,
    /// Save the connection
    Save {
        /// The connection entry to save
        entry: ConnectionEntry,
        /// Password to store (if save_password is true)
        password: Option<String>,
        /// Whether to save password to keychain
        save_password: bool,
        /// Original name if editing (for rename detection)
        original_name: Option<String>,
    },
    /// Test the connection
    TestConnection {
        /// Connection entry to test
        entry: ConnectionEntry,
        /// Password for testing
        password: Option<String>,
    },
    /// Show a status message
    StatusMessage(String),
}

/// Modal for adding or editing a connection.
pub struct ConnectionFormModal {
    /// Current field values
    name: String,
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    save_password: bool,
    color: ConnectionColor,
    url_paste: String,

    /// Cursor positions for each text field
    name_cursor: usize,
    host_cursor: usize,
    port_cursor: usize,
    database_cursor: usize,
    user_cursor: usize,
    password_cursor: usize,
    url_paste_cursor: usize,

    /// Currently focused field
    focused: FormField,

    /// Color selection index (for cycling through colors)
    color_index: usize,

    /// Whether we're editing an existing connection (used for UI hints)
    #[allow(dead_code)]
    editing: bool,

    /// Original name when editing (for rename detection)
    original_name: Option<String>,

    /// Title for the modal
    title: String,

    /// Track if form has been modified (for unsaved changes detection)
    modified: bool,

    /// Original values for detecting changes (only set when editing)
    original_values: Option<OriginalFormValues>,
}

/// Original form values for tracking modifications
#[derive(Clone)]
struct OriginalFormValues {
    name: String,
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    save_password: bool,
    color: ConnectionColor,
}

impl ConnectionFormModal {
    /// Create a new form for adding a connection.
    pub fn new() -> Self {
        Self {
            name: String::new(),
            host: "localhost".to_string(),
            port: "5432".to_string(),
            database: String::new(),
            user: String::new(),
            password: String::new(),
            save_password: false,
            color: ConnectionColor::None,
            url_paste: String::new(),

            name_cursor: 0,
            host_cursor: 9, // "localhost".len()
            port_cursor: 4, // "5432".len()
            database_cursor: 0,
            user_cursor: 0,
            password_cursor: 0,
            url_paste_cursor: 0,

            focused: FormField::Name,
            color_index: 0,
            editing: false,
            original_name: None,
            title: "New Connection".to_string(),
            modified: false,
            original_values: None,
        }
    }

    /// Create a form for editing an existing connection.
    pub fn edit(entry: &ConnectionEntry, existing_password: Option<String>) -> Self {
        let password = existing_password.unwrap_or_default();
        let color_index = ConnectionColor::all_names()
            .iter()
            .position(|&c| c == entry.color.to_string())
            .unwrap_or(0);

        let original_values = OriginalFormValues {
            name: entry.name.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            database: entry.database.clone(),
            user: entry.user.clone(),
            password: password.clone(),
            save_password: entry.password_in_keychain,
            color: entry.color,
        };

        Self {
            name: entry.name.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            database: entry.database.clone(),
            user: entry.user.clone(),
            password: password.clone(),
            save_password: entry.password_in_keychain,
            color: entry.color,
            url_paste: String::new(),

            name_cursor: entry.name.len(),
            host_cursor: entry.host.len(),
            port_cursor: entry.port.to_string().len(),
            database_cursor: entry.database.len(),
            user_cursor: entry.user.len(),
            password_cursor: password.len(),
            url_paste_cursor: 0,

            focused: FormField::Name,
            color_index,
            editing: true,
            original_name: Some(entry.name.clone()),
            title: format!("Edit: {}", entry.name),
            modified: false,
            original_values: Some(original_values),
        }
    }

    /// Check if the form has unsaved changes.
    pub fn is_modified(&self) -> bool {
        if self.modified {
            return true;
        }

        // For new connections, check if any required field has content
        if self.original_values.is_none() {
            return !self.name.is_empty()
                || !self.user.is_empty()
                || !self.password.is_empty()
                || !self.database.is_empty()
                || self.host != "localhost"
                || self.port != "5432";
        }

        // For editing, compare with original values
        if let Some(ref orig) = self.original_values {
            return self.name != orig.name
                || self.host != orig.host
                || self.port != orig.port
                || self.database != orig.database
                || self.user != orig.user
                || self.password != orig.password
                || self.save_password != orig.save_password
                || self.color != orig.color;
        }

        false
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConnectionFormAction {
        match (key.code, key.modifiers) {
            // Cancel - check for unsaved changes
            (KeyCode::Esc, _) => {
                if self.is_modified() {
                    ConnectionFormAction::RequestClose
                } else {
                    ConnectionFormAction::Cancel
                }
            }

            // Save (Ctrl+S)
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.try_save(),

            // Test connection (Ctrl+T)
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => self.try_test(),

            // Tab - next field
            (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.focused = self.focused.next();
                ConnectionFormAction::Continue
            }

            // Shift+Tab - previous field
            (KeyCode::BackTab, _) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.focused = self.focused.prev();
                ConnectionFormAction::Continue
            }

            // Enter on URL paste field triggers paste processing
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::UrlPaste => {
                self.process_url_paste()
            }

            // Enter on save password toggles it
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::SavePassword => {
                self.save_password = !self.save_password;
                ConnectionFormAction::Continue
            }

            // Enter on color cycles to next
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::Color => {
                self.cycle_color(1);
                ConnectionFormAction::Continue
            }

            // Space toggles checkboxes
            (KeyCode::Char(' '), KeyModifiers::NONE) if self.focused == FormField::SavePassword => {
                self.save_password = !self.save_password;
                ConnectionFormAction::Continue
            }

            // Space cycles color
            (KeyCode::Char(' '), KeyModifiers::NONE) if self.focused == FormField::Color => {
                self.cycle_color(1);
                ConnectionFormAction::Continue
            }

            // Left/Right on color cycles
            (KeyCode::Left, KeyModifiers::NONE) if self.focused == FormField::Color => {
                self.cycle_color(-1);
                ConnectionFormAction::Continue
            }
            (KeyCode::Right, KeyModifiers::NONE) if self.focused == FormField::Color => {
                self.cycle_color(1);
                ConnectionFormAction::Continue
            }

            // Text input for text fields
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.insert_char(c);
                ConnectionFormAction::Continue
            }

            // Backspace
            (KeyCode::Backspace, _) => {
                self.delete_char_before();
                ConnectionFormAction::Continue
            }

            // Delete
            (KeyCode::Delete, _) => {
                self.delete_char_at();
                ConnectionFormAction::Continue
            }

            // Cursor movement
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.move_cursor_left();
                ConnectionFormAction::Continue
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.move_cursor_right();
                ConnectionFormAction::Continue
            }
            (KeyCode::Home, _) => {
                self.move_cursor_home();
                ConnectionFormAction::Continue
            }
            (KeyCode::End, _) => {
                self.move_cursor_end();
                ConnectionFormAction::Continue
            }

            // Ctrl+U - clear field
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.clear_field();
                ConnectionFormAction::Continue
            }

            _ => ConnectionFormAction::Continue,
        }
    }

    fn get_current_field_and_cursor(&mut self) -> Option<(&mut String, &mut usize)> {
        match self.focused {
            FormField::Name => Some((&mut self.name, &mut self.name_cursor)),
            FormField::Host => Some((&mut self.host, &mut self.host_cursor)),
            FormField::Port => Some((&mut self.port, &mut self.port_cursor)),
            FormField::Database => Some((&mut self.database, &mut self.database_cursor)),
            FormField::User => Some((&mut self.user, &mut self.user_cursor)),
            FormField::Password => Some((&mut self.password, &mut self.password_cursor)),
            FormField::UrlPaste => Some((&mut self.url_paste, &mut self.url_paste_cursor)),
            FormField::SavePassword | FormField::Color => None,
        }
    }

    fn insert_char(&mut self, c: char) {
        // For port field, only allow digits
        if self.focused == FormField::Port && !c.is_ascii_digit() {
            return;
        }

        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            field.insert(*cursor, c);
            *cursor += 1;
        }
    }

    fn delete_char_before(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            if *cursor > 0 {
                *cursor -= 1;
                field.remove(*cursor);
            }
        }
    }

    fn delete_char_at(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            if *cursor < field.len() {
                field.remove(*cursor);
            }
        }
    }

    fn move_cursor_left(&mut self) {
        if let Some((_, cursor)) = self.get_current_field_and_cursor() {
            if *cursor > 0 {
                *cursor -= 1;
            }
        }
    }

    fn move_cursor_right(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            if *cursor < field.len() {
                *cursor += 1;
            }
        }
    }

    fn move_cursor_home(&mut self) {
        if let Some((_, cursor)) = self.get_current_field_and_cursor() {
            *cursor = 0;
        }
    }

    fn move_cursor_end(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            *cursor = field.len();
        }
    }

    fn clear_field(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            field.clear();
            *cursor = 0;
        }
    }

    fn cycle_color(&mut self, direction: i32) {
        let colors = ConnectionColor::all_names();
        let len = colors.len() as i32;
        self.color_index = ((self.color_index as i32 + direction).rem_euclid(len)) as usize;
        self.color = colors[self.color_index]
            .parse()
            .unwrap_or(ConnectionColor::None);
    }

    fn process_url_paste(&mut self) -> ConnectionFormAction {
        if self.url_paste.is_empty() {
            return ConnectionFormAction::Continue;
        }

        // Try to parse the URL
        match ConnectionEntry::from_url("temp", &self.url_paste) {
            Ok((entry, password)) => {
                // Populate fields from parsed URL
                self.host = entry.host;
                self.port = entry.port.to_string();
                self.database = entry.database;
                self.user = entry.user;

                if let Some(pwd) = password {
                    self.password = pwd;
                    self.password_cursor = self.password.len();
                }

                // Update cursors
                self.host_cursor = self.host.len();
                self.port_cursor = self.port.len();
                self.database_cursor = self.database.len();
                self.user_cursor = self.user.len();

                // Clear URL paste field
                self.url_paste.clear();
                self.url_paste_cursor = 0;

                // Move focus to name if empty, otherwise to first empty required field
                if self.name.is_empty() {
                    self.focused = FormField::Name;
                } else {
                    self.focused = FormField::Host;
                }

                ConnectionFormAction::StatusMessage("URL parsed successfully".to_string())
            }
            Err(e) => ConnectionFormAction::StatusMessage(format!("Invalid URL: {}", e)),
        }
    }

    fn try_save(&mut self) -> ConnectionFormAction {
        // Validate required fields
        if self.name.is_empty() {
            self.focused = FormField::Name;
            return ConnectionFormAction::StatusMessage("Name is required".to_string());
        }
        if self.name.contains(char::is_whitespace) {
            self.focused = FormField::Name;
            return ConnectionFormAction::StatusMessage(
                "Name cannot contain whitespace".to_string(),
            );
        }
        if self.host.is_empty() {
            self.focused = FormField::Host;
            return ConnectionFormAction::StatusMessage("Host is required".to_string());
        }
        if self.database.is_empty() {
            self.focused = FormField::Database;
            return ConnectionFormAction::StatusMessage("Database is required".to_string());
        }
        if self.user.is_empty() {
            self.focused = FormField::User;
            return ConnectionFormAction::StatusMessage("User is required".to_string());
        }

        let port: u16 = match self.port.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                self.focused = FormField::Port;
                return ConnectionFormAction::StatusMessage("Invalid port number".to_string());
            }
        };

        let entry = ConnectionEntry {
            name: self.name.clone(),
            host: self.host.clone(),
            port,
            database: self.database.clone(),
            user: self.user.clone(),
            password_in_keychain: self.save_password && !self.password.is_empty(),
            password_env: None,
            color: self.color,
            favorite: None, // Preserve from original if editing
            ssl_mode: None,
        };

        let password = if self.password.is_empty() {
            None
        } else {
            Some(self.password.clone())
        };

        ConnectionFormAction::Save {
            entry,
            password,
            save_password: self.save_password,
            original_name: self.original_name.clone(),
        }
    }

    fn try_test(&mut self) -> ConnectionFormAction {
        // Basic validation for testing
        if self.host.is_empty() {
            return ConnectionFormAction::StatusMessage("Host is required for test".to_string());
        }
        if self.database.is_empty() {
            return ConnectionFormAction::StatusMessage(
                "Database is required for test".to_string(),
            );
        }
        if self.user.is_empty() {
            return ConnectionFormAction::StatusMessage("User is required for test".to_string());
        }

        let port: u16 = match self.port.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                return ConnectionFormAction::StatusMessage("Invalid port number".to_string());
            }
        };

        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: self.host.clone(),
            port,
            database: self.database.clone(),
            user: self.user.clone(),
            password_in_keychain: false,
            password_env: None,
            color: ConnectionColor::None,
            favorite: None,
            ssl_mode: None,
        };

        let password = if self.password.is_empty() {
            None
        } else {
            Some(self.password.clone())
        };

        ConnectionFormAction::TestConnection { entry, password }
    }

    /// Render the connection form modal.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Calculate modal size
        let modal_width = 60u16.min(area.width - 4);
        let modal_height = 18u16.min(area.height - 2);
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

        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title))
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Layout the form fields
        // Order: Name → User → Password → SavePassword → Host → Port → Database → Color → UrlPaste
        let chunks = Layout::vertical([
            Constraint::Length(1), // Name
            Constraint::Length(1), // Separator
            Constraint::Length(1), // User
            Constraint::Length(1), // Password
            Constraint::Length(1), // Save password checkbox
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Host
            Constraint::Length(1), // Port
            Constraint::Length(1), // Database
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Color
            Constraint::Length(1), // URL paste
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Help line
        ])
        .split(inner);

        // Render each field in the new order
        self.render_text_field(frame, chunks[0], "Name:", &self.name, self.name_cursor, FormField::Name);
        self.render_separator(frame, chunks[1]);
        self.render_text_field(frame, chunks[2], "User:", &self.user, self.user_cursor, FormField::User);
        self.render_password_field(frame, chunks[3]);
        self.render_checkbox(frame, chunks[4], "Save to keychain", self.save_password, FormField::SavePassword);
        self.render_separator(frame, chunks[5]);
        self.render_text_field(frame, chunks[6], "Host:", &self.host, self.host_cursor, FormField::Host);
        self.render_text_field(frame, chunks[7], "Port:", &self.port, self.port_cursor, FormField::Port);
        self.render_text_field(frame, chunks[8], "Database:", &self.database, self.database_cursor, FormField::Database);
        self.render_separator(frame, chunks[9]);
        self.render_color_field(frame, chunks[10]);
        self.render_url_paste_field(frame, chunks[11]);
        self.render_separator(frame, chunks[12]);
        self.render_help(frame, chunks[13]);
    }

    fn render_text_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: &str,
        cursor: usize,
        field: FormField,
    ) {
        let is_focused = self.focused == field;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Label
        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new(label).style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        // Value with cursor
        let value_spans = if is_focused {
            self.render_text_with_cursor(value, cursor)
        } else {
            vec![Span::raw(value)]
        };

        let value_style = if is_focused {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };

        let value_widget = Paragraph::new(Line::from(value_spans)).style(value_style);
        frame.render_widget(value_widget, chunks[1]);
    }

    fn render_password_field(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focused == FormField::Password;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Label
        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new("Password:").style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        // Masked value with cursor
        let masked: String = "•".repeat(self.password.len());
        let value_spans = if is_focused {
            self.render_text_with_cursor(&masked, self.password_cursor)
        } else {
            vec![Span::raw(masked)]
        };

        let value_style = if is_focused {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };

        let value_widget = Paragraph::new(Line::from(value_spans)).style(value_style);
        frame.render_widget(value_widget, chunks[1]);
    }

    fn render_checkbox(&self, frame: &mut Frame, area: Rect, label: &str, checked: bool, field: FormField) {
        let is_focused = self.focused == field;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Empty label space
        frame.render_widget(Paragraph::new(""), chunks[0]);

        // Checkbox
        let checkbox = if checked { "[x]" } else { "[ ]" };
        let style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        let widget = Paragraph::new(format!("{} {}", checkbox, label)).style(style);
        frame.render_widget(widget, chunks[1]);
    }

    fn render_color_field(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focused == FormField::Color;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Label
        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new("Color:").style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        // Color value with preview
        let color_name = self.color.to_string();
        let color_fg = self.color.to_ratatui_color().unwrap_or(Color::White);

        let mut spans = vec![];
        if is_focused {
            spans.push(Span::styled("◀ ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            format!("{:<8}", color_name),
            Style::default().fg(color_fg).add_modifier(Modifier::BOLD),
        ));
        if is_focused {
            spans.push(Span::styled(" ▶", Style::default().fg(Color::DarkGray)));
        }

        let widget = Paragraph::new(Line::from(spans));
        frame.render_widget(widget, chunks[1]);
    }

    fn render_url_paste_field(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focused == FormField::UrlPaste;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Label
        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new("Paste URL:").style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        // Value with cursor or placeholder
        let (value_spans, style) = if self.url_paste.is_empty() && !is_focused {
            (
                vec![Span::raw("postgres://user:pass@host/db")],
                Style::default().fg(Color::DarkGray),
            )
        } else if is_focused {
            (
                self.render_text_with_cursor(&self.url_paste, self.url_paste_cursor),
                Style::default().fg(Color::White),
            )
        } else {
            (
                vec![Span::raw(&self.url_paste)],
                Style::default().fg(Color::Gray),
            )
        };

        let value_widget = Paragraph::new(Line::from(value_spans)).style(style);
        frame.render_widget(value_widget, chunks[1]);
    }

    fn render_text_with_cursor(&self, text: &str, cursor: usize) -> Vec<Span<'static>> {
        let before: String = text.chars().take(cursor).collect();
        let cursor_char = text.chars().nth(cursor).unwrap_or(' ');
        let after: String = text.chars().skip(cursor + 1).collect();

        vec![
            Span::raw(before),
            Span::styled(
                cursor_char.to_string(),
                Style::default().bg(Color::White).fg(Color::Black),
            ),
            Span::raw(after),
        ]
    }

    fn render_separator(&self, frame: &mut Frame, area: Rect) {
        let sep = Paragraph::new("─".repeat(area.width as usize))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(sep, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_spans = vec![
            Span::styled("Tab", Style::default().fg(Color::Yellow)),
            Span::raw(" next  "),
            Span::styled("Ctrl+S", Style::default().fg(Color::Yellow)),
            Span::raw(" save  "),
            Span::styled("Ctrl+T", Style::default().fg(Color::Yellow)),
            Span::raw(" test  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ];

        let help = Paragraph::new(Line::from(help_spans))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(help, area);
    }
}

impl Default for ConnectionFormModal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form_field_navigation() {
        // New order: Name → User → Password → SavePassword → Host → Port → Database → Color → UrlPaste
        assert_eq!(FormField::Name.next(), FormField::User);
        assert_eq!(FormField::User.next(), FormField::Password);
        assert_eq!(FormField::Password.next(), FormField::SavePassword);
        assert_eq!(FormField::SavePassword.next(), FormField::Host);
        assert_eq!(FormField::UrlPaste.next(), FormField::Name);
        assert_eq!(FormField::Name.prev(), FormField::UrlPaste);
        assert_eq!(FormField::User.prev(), FormField::Name);
    }

    #[test]
    fn test_new_form_defaults() {
        let form = ConnectionFormModal::new();
        assert_eq!(form.host, "localhost");
        assert_eq!(form.port, "5432");
        assert!(!form.editing);
        assert!(form.original_name.is_none());
    }

    #[test]
    fn test_edit_form() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "db.example.com".to_string(),
            port: 5433,
            database: "mydb".to_string(),
            user: "admin".to_string(),
            color: ConnectionColor::Green,
            ..Default::default()
        };

        let form = ConnectionFormModal::edit(&entry, Some("secret".to_string()));

        assert_eq!(form.name, "test");
        assert_eq!(form.host, "db.example.com");
        assert_eq!(form.port, "5433");
        assert_eq!(form.password, "secret");
        assert!(form.editing);
        assert_eq!(form.original_name, Some("test".to_string()));
    }

    #[test]
    fn test_tab_navigation() {
        let mut form = ConnectionFormModal::new();
        assert_eq!(form.focused, FormField::Name);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::User); // New order: Name → User

        form.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(form.focused, FormField::Name);
    }

    #[test]
    fn test_text_input() {
        let mut form = ConnectionFormModal::new();
        form.name.clear();
        form.name_cursor = 0;

        form.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        assert_eq!(form.name, "test");
        assert_eq!(form.name_cursor, 4);
    }

    #[test]
    fn test_port_only_accepts_digits() {
        let mut form = ConnectionFormModal::new();
        form.focused = FormField::Port;
        form.port.clear();
        form.port_cursor = 0;

        form.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)); // Should be ignored
        form.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));

        assert_eq!(form.port, "5432");
    }

    #[test]
    fn test_backspace() {
        let mut form = ConnectionFormModal::new();
        form.name = "test".to_string();
        form.name_cursor = 4;

        form.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(form.name, "tes");
        assert_eq!(form.name_cursor, 3);
    }

    #[test]
    fn test_cursor_movement() {
        let mut form = ConnectionFormModal::new();
        form.name = "test".to_string();
        form.name_cursor = 2;

        form.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(form.name_cursor, 1);

        form.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(form.name_cursor, 2);

        form.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(form.name_cursor, 0);

        form.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(form.name_cursor, 4);
    }

    #[test]
    fn test_save_validation_empty_name() {
        let mut form = ConnectionFormModal::new();
        form.name.clear();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::StatusMessage(msg) => {
                assert!(msg.contains("Name"));
            }
            _ => panic!("Expected StatusMessage"),
        }
    }

    #[test]
    fn test_save_validation_whitespace_name() {
        let mut form = ConnectionFormModal::new();
        form.name = "my connection".to_string();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::StatusMessage(msg) => {
                assert!(msg.contains("whitespace"));
            }
            _ => panic!("Expected StatusMessage"),
        }
    }

    #[test]
    fn test_save_success() {
        let mut form = ConnectionFormModal::new();
        form.name = "myconn".to_string();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();
        form.password = "secret".to_string();
        form.save_password = true;

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::Save {
                entry,
                password,
                save_password,
                original_name,
            } => {
                assert_eq!(entry.name, "myconn");
                assert_eq!(entry.host, "localhost");
                assert_eq!(password, Some("secret".to_string()));
                assert!(save_password);
                assert!(original_name.is_none());
            }
            _ => panic!("Expected Save action"),
        }
    }

    #[test]
    fn test_url_paste() {
        let mut form = ConnectionFormModal::new();
        form.focused = FormField::UrlPaste;
        form.url_paste = "postgres://admin:secret@db.example.com:5433/production".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match action {
            ConnectionFormAction::StatusMessage(msg) => {
                assert!(msg.contains("parsed"));
            }
            _ => panic!("Expected StatusMessage"),
        }

        assert_eq!(form.host, "db.example.com");
        assert_eq!(form.port, "5433");
        assert_eq!(form.database, "production");
        assert_eq!(form.user, "admin");
        assert_eq!(form.password, "secret");
        assert!(form.url_paste.is_empty());
    }

    #[test]
    fn test_cancel() {
        let mut form = ConnectionFormModal::new();
        let action = form.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, ConnectionFormAction::Cancel);
    }

    #[test]
    fn test_color_cycling() {
        let mut form = ConnectionFormModal::new();
        form.focused = FormField::Color;
        let initial_index = form.color_index;

        form.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(form.color_index, initial_index + 1);

        form.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(form.color_index, initial_index);
    }

    #[test]
    fn test_checkbox_toggle() {
        let mut form = ConnectionFormModal::new();
        form.focused = FormField::SavePassword;
        assert!(!form.save_password);

        form.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(form.save_password);

        form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!form.save_password);
    }

    #[test]
    fn test_test_connection() {
        let mut form = ConnectionFormModal::new();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();
        form.password = "secret".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::TestConnection { entry, password } => {
                assert_eq!(entry.host, "localhost");
                assert_eq!(password, Some("secret".to_string()));
            }
            _ => panic!("Expected TestConnection action"),
        }
    }
}
