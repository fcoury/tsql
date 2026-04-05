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
use url::Url;

use crate::config::{Action, ConnectionColor, ConnectionEntry, DbKind, Keymap, SslMode};

/// Which field is currently focused in the form
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    #[default]
    Name,
    Kind,
    Host,
    Port,
    Database,
    User,
    Password,
    OnePasswordRef,
    SavePassword,
    SslMode,
    Color,
    UrlPaste,
}

impl FormField {
    /// Get the next field in tab order
    /// Order: Name → Kind → User → Password → OnePasswordRef → SavePassword → SSL Mode → Host → Port → Database → Color → UrlPaste
    pub fn next(self) -> Self {
        match self {
            FormField::Name => FormField::Kind,
            FormField::Kind => FormField::User,
            FormField::User => FormField::Password,
            FormField::Password => FormField::OnePasswordRef,
            FormField::OnePasswordRef => FormField::SavePassword,
            FormField::SavePassword => FormField::SslMode,
            FormField::SslMode => FormField::Host,
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
            FormField::Kind => FormField::Name,
            FormField::User => FormField::Kind,
            FormField::Password => FormField::User,
            FormField::OnePasswordRef => FormField::Password,
            FormField::SavePassword => FormField::OnePasswordRef,
            FormField::SslMode => FormField::SavePassword,
            FormField::Host => FormField::SslMode,
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
    kind: DbKind,
    mongo_uri: Option<String>,
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    op_ref: String,
    save_password: bool,
    ssl_mode: SslMode,
    color: ConnectionColor,
    url_paste: String,

    /// Cursor positions for each text field
    name_cursor: usize,
    host_cursor: usize,
    port_cursor: usize,
    database_cursor: usize,
    user_cursor: usize,
    password_cursor: usize,
    op_ref_cursor: usize,
    url_paste_cursor: usize,

    /// Currently focused field
    focused: FormField,

    /// Color selection index (for cycling through colors)
    color_index: usize,
    /// SSL mode selection index (for cycling through modes)
    ssl_mode_index: usize,

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

    /// Keymap for form actions
    keymap: Keymap,
    /// Whether the 1Password reference field is enabled in the UI.
    onepassword_enabled: bool,
}

/// Original form values for tracking modifications
#[derive(Clone)]
struct OriginalFormValues {
    name: String,
    kind: DbKind,
    mongo_uri: Option<String>,
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    op_ref: String,
    save_password: bool,
    ssl_mode: SslMode,
    color: ConnectionColor,
}

impl ConnectionFormModal {
    /// Create a new form for adding a connection.
    pub fn new() -> Self {
        Self::with_keymap(Keymap::default_connection_form_keymap())
    }

    /// Create a new form with a custom keymap.
    pub fn with_keymap(keymap: Keymap) -> Self {
        Self::with_keymap_and_onepassword(keymap, true)
    }

    /// Create a new form with a custom keymap and explicit 1Password UI toggle.
    pub fn with_keymap_and_onepassword(keymap: Keymap, onepassword_enabled: bool) -> Self {
        Self {
            name: String::new(),
            kind: DbKind::Postgres,
            mongo_uri: None,
            host: "localhost".to_string(),
            port: "5432".to_string(),
            database: String::new(),
            user: String::new(),
            password: String::new(),
            op_ref: String::new(),
            save_password: false,
            ssl_mode: SslMode::Disable,
            color: ConnectionColor::None,
            url_paste: String::new(),

            name_cursor: 0,
            host_cursor: 9, // "localhost".len()
            port_cursor: 4, // "5432".len()
            database_cursor: 0,
            user_cursor: 0,
            password_cursor: 0,
            op_ref_cursor: 0,
            url_paste_cursor: 0,

            focused: FormField::Name,
            color_index: 0,
            ssl_mode_index: 0,
            editing: false,
            original_name: None,
            title: "New Connection".to_string(),
            modified: false,
            original_values: None,
            keymap,
            onepassword_enabled,
        }
    }

    /// Create a form for editing an existing connection.
    pub fn edit(entry: &ConnectionEntry, existing_password: Option<String>) -> Self {
        Self::edit_with_keymap(
            entry,
            existing_password,
            Keymap::default_connection_form_keymap(),
        )
    }

    /// Create a form for editing an existing connection with a custom keymap.
    pub fn edit_with_keymap(
        entry: &ConnectionEntry,
        existing_password: Option<String>,
        keymap: Keymap,
    ) -> Self {
        Self::edit_with_keymap_and_onepassword(entry, existing_password, keymap, true)
    }

    /// Create a form for editing an existing connection with explicit 1Password UI toggle.
    pub fn edit_with_keymap_and_onepassword(
        entry: &ConnectionEntry,
        existing_password: Option<String>,
        keymap: Keymap,
        onepassword_enabled: bool,
    ) -> Self {
        let password = existing_password.unwrap_or_default();
        let op_ref = entry.password_onepassword.clone().unwrap_or_default();
        let color_index = ConnectionColor::all_names()
            .iter()
            .position(|&c| c == entry.color.to_string())
            .unwrap_or(0);

        let original_values = OriginalFormValues {
            name: entry.name.clone(),
            kind: entry.kind,
            mongo_uri: entry.uri.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            database: entry.database.clone(),
            user: entry.user.clone(),
            password: password.clone(),
            op_ref: op_ref.clone(),
            save_password: entry.password_in_keychain,
            ssl_mode: entry.ssl_mode.unwrap_or(SslMode::Disable),
            color: entry.color,
        };

        let ssl_mode = entry.ssl_mode.unwrap_or(SslMode::Disable);
        let ssl_mode_index = ssl_mode.to_index();

        Self {
            name: entry.name.clone(),
            kind: entry.kind,
            mongo_uri: entry.uri.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            database: entry.database.clone(),
            user: entry.user.clone(),
            password: password.clone(),
            op_ref: op_ref.clone(),
            save_password: entry.password_in_keychain,
            ssl_mode,
            color: entry.color,
            url_paste: String::new(),

            name_cursor: entry.name.chars().count(),
            host_cursor: entry.host.chars().count(),
            port_cursor: entry.port.to_string().chars().count(),
            database_cursor: entry.database.chars().count(),
            user_cursor: entry.user.chars().count(),
            password_cursor: password.chars().count(),
            op_ref_cursor: op_ref.chars().count(),
            url_paste_cursor: 0,

            focused: FormField::Name,
            color_index,
            ssl_mode_index,
            editing: true,
            original_name: Some(entry.name.clone()),
            title: format!("Edit: {}", entry.name),
            modified: false,
            original_values: Some(original_values),
            keymap,
            onepassword_enabled,
        }
    }

    /// Create a new-connection form prefilled from an existing connection.
    pub fn duplicate_with_keymap_and_onepassword(
        entry: &ConnectionEntry,
        existing_password: Option<String>,
        duplicate_name: String,
        keymap: Keymap,
        onepassword_enabled: bool,
    ) -> Self {
        let password = existing_password.unwrap_or_default();
        let op_ref = entry.password_onepassword.clone().unwrap_or_default();
        let color_index = ConnectionColor::all_names()
            .iter()
            .position(|&c| c == entry.color.to_string())
            .unwrap_or(0);

        let ssl_mode = entry.ssl_mode.unwrap_or(SslMode::Disable);
        let ssl_mode_index = ssl_mode.to_index();

        Self {
            name_cursor: duplicate_name.chars().count(),
            host_cursor: entry.host.chars().count(),
            port_cursor: entry.port.to_string().chars().count(),
            database_cursor: entry.database.chars().count(),
            user_cursor: entry.user.chars().count(),
            password_cursor: password.chars().count(),
            op_ref_cursor: op_ref.chars().count(),

            name: duplicate_name,
            kind: entry.kind,
            mongo_uri: entry.uri.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            database: entry.database.clone(),
            user: entry.user.clone(),
            password,
            op_ref,
            save_password: entry.password_in_keychain,
            ssl_mode,
            color: entry.color,
            url_paste: String::new(),
            url_paste_cursor: 0,

            focused: FormField::Name,
            color_index,
            ssl_mode_index,
            editing: false,
            original_name: None,
            title: format!("Duplicate: {}", entry.name),
            modified: false,
            original_values: None,
            keymap,
            onepassword_enabled,
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
                || self.kind != DbKind::Postgres
                || self.mongo_uri.is_some()
                || !self.user.is_empty()
                || !self.password.is_empty()
                || !self.op_ref.is_empty()
                || !self.database.is_empty()
                || self.host != "localhost"
                || self.port != "5432"
                || self.ssl_mode != SslMode::Disable;
        }

        // For editing, compare with original values
        if let Some(ref orig) = self.original_values {
            return self.name != orig.name
                || self.kind != orig.kind
                || self.mongo_uri != orig.mongo_uri
                || self.host != orig.host
                || self.port != orig.port
                || self.database != orig.database
                || self.user != orig.user
                || self.password != orig.password
                || self.op_ref != orig.op_ref
                || self.save_password != orig.save_password
                || self.ssl_mode != orig.ssl_mode
                || self.color != orig.color;
        }

        false
    }

    /// Set the keymap for this form.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
    }

    /// Get the key binding for saving (for help display).
    pub fn save_key_display(&self) -> String {
        self.keymap
            .bindings()
            .iter()
            .find(|(_, action)| **action == Action::SaveConnection)
            .map(|(key, _)| key.to_string())
            .unwrap_or_else(|| "Ctrl+S".to_string())
    }

    /// Get the key binding for testing (for help display).
    pub fn test_key_display(&self) -> String {
        self.keymap
            .bindings()
            .iter()
            .find(|(_, action)| **action == Action::TestConnection)
            .map(|(key, _)| key.to_string())
            .unwrap_or_else(|| "Ctrl+T".to_string())
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConnectionFormAction {
        // Check keymap for configurable actions first
        if let Some(action) = self.keymap.get_action(&key) {
            match action {
                Action::SaveConnection => return self.try_save(),
                Action::TestConnection => return self.try_test(),
                Action::ClearField => {
                    self.clear_field();
                    return ConnectionFormAction::Continue;
                }
                _ => {}
            }
        }

        match (key.code, key.modifiers) {
            // Cancel - check for unsaved changes
            (KeyCode::Esc, _) => {
                if self.is_modified() {
                    ConnectionFormAction::RequestClose
                } else {
                    ConnectionFormAction::Cancel
                }
            }

            // Tab - next field
            (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.focused = self.next_focus(self.focused);
                ConnectionFormAction::Continue
            }

            // Shift+Tab - previous field
            (KeyCode::BackTab, _) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.focused = self.prev_focus(self.focused);
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

            // Enter on kind cycles
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::Kind => {
                self.cycle_kind(1);
                ConnectionFormAction::Continue
            }

            // Enter on color cycles to next
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::Color => {
                self.cycle_color(1);
                ConnectionFormAction::Continue
            }

            // Enter on ssl mode cycles
            (KeyCode::Enter, KeyModifiers::NONE) if self.focused == FormField::SslMode => {
                self.cycle_ssl_mode(1);
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

            // Space cycles kind
            (KeyCode::Char(' '), KeyModifiers::NONE) if self.focused == FormField::Kind => {
                self.cycle_kind(1);
                ConnectionFormAction::Continue
            }

            // Space cycles ssl mode
            (KeyCode::Char(' '), KeyModifiers::NONE) if self.focused == FormField::SslMode => {
                self.cycle_ssl_mode(1);
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

            // Left/Right on kind cycles
            (KeyCode::Left, KeyModifiers::NONE) if self.focused == FormField::Kind => {
                self.cycle_kind(-1);
                ConnectionFormAction::Continue
            }
            (KeyCode::Right, KeyModifiers::NONE) if self.focused == FormField::Kind => {
                self.cycle_kind(1);
                ConnectionFormAction::Continue
            }

            // Left/Right on ssl mode cycles
            (KeyCode::Left, KeyModifiers::NONE) if self.focused == FormField::SslMode => {
                self.cycle_ssl_mode(-1);
                ConnectionFormAction::Continue
            }
            (KeyCode::Right, KeyModifiers::NONE) if self.focused == FormField::SslMode => {
                self.cycle_ssl_mode(1);
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

            _ => ConnectionFormAction::Continue,
        }
    }

    fn next_focus(&self, current: FormField) -> FormField {
        let mut next = current.next();
        if !self.onepassword_enabled && next == FormField::OnePasswordRef {
            next = next.next();
        }
        next
    }

    fn prev_focus(&self, current: FormField) -> FormField {
        let mut prev = current.prev();
        if !self.onepassword_enabled && prev == FormField::OnePasswordRef {
            prev = prev.prev();
        }
        prev
    }

    fn get_current_field_and_cursor(&mut self) -> Option<(&mut String, &mut usize)> {
        match self.focused {
            FormField::Name => Some((&mut self.name, &mut self.name_cursor)),
            FormField::Host => Some((&mut self.host, &mut self.host_cursor)),
            FormField::Port => Some((&mut self.port, &mut self.port_cursor)),
            FormField::Database => Some((&mut self.database, &mut self.database_cursor)),
            FormField::User => Some((&mut self.user, &mut self.user_cursor)),
            FormField::Password => Some((&mut self.password, &mut self.password_cursor)),
            FormField::OnePasswordRef => Some((&mut self.op_ref, &mut self.op_ref_cursor)),
            FormField::UrlPaste => Some((&mut self.url_paste, &mut self.url_paste_cursor)),
            FormField::Kind | FormField::SavePassword | FormField::SslMode | FormField::Color => {
                None
            }
        }
    }

    fn char_count(text: &str) -> usize {
        text.chars().count()
    }

    fn char_to_byte_index(text: &str, char_index: usize) -> usize {
        text.char_indices()
            .nth(char_index)
            .map_or(text.len(), |(idx, _)| idx)
    }

    fn insert_char(&mut self, c: char) {
        // For port field, only allow digits
        if self.focused == FormField::Port && !c.is_ascii_digit() {
            return;
        }

        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            let char_count = Self::char_count(field);
            *cursor = (*cursor).min(char_count);
            let byte_index = Self::char_to_byte_index(field, *cursor);
            field.insert(byte_index, c);
            *cursor += 1;
        }
    }

    fn delete_char_before(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            let char_count = Self::char_count(field);
            *cursor = (*cursor).min(char_count);
            if *cursor > 0 {
                let remove_char_idx = *cursor - 1;
                let remove_byte_idx = Self::char_to_byte_index(field, remove_char_idx);
                field.remove(remove_byte_idx);
                *cursor = remove_char_idx;
            }
        }
    }

    fn delete_char_at(&mut self) {
        if let Some((field, cursor)) = self.get_current_field_and_cursor() {
            let char_count = Self::char_count(field);
            *cursor = (*cursor).min(char_count);
            if *cursor < char_count {
                let remove_byte_idx = Self::char_to_byte_index(field, *cursor);
                field.remove(remove_byte_idx);
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
            if *cursor < Self::char_count(field) {
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
            *cursor = Self::char_count(field);
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

    fn cycle_kind(&mut self, direction: i32) {
        const ORDER: [DbKind; 2] = [DbKind::Postgres, DbKind::Mongo];
        let current_idx = ORDER.iter().position(|k| *k == self.kind).unwrap_or(0) as i32;
        let next_idx = ((current_idx + direction).rem_euclid(ORDER.len() as i32)) as usize;
        self.kind = ORDER[next_idx];
        match self.kind {
            DbKind::Postgres => {
                self.mongo_uri = None;
                if self.port.is_empty() || self.port == "27017" {
                    self.port = "5432".to_string();
                    self.port_cursor = Self::char_count(&self.port);
                }
            }
            DbKind::Mongo => {
                if self.port.is_empty() || self.port == "5432" {
                    self.port = "27017".to_string();
                    self.port_cursor = Self::char_count(&self.port);
                }
                self.mongo_uri = Some(self.build_mongo_uri(None));
            }
        }
    }

    fn cycle_ssl_mode(&mut self, direction: i32) {
        if self.kind == DbKind::Mongo {
            return;
        }
        let len = SslMode::COUNT as i32;
        self.ssl_mode_index = ((self.ssl_mode_index as i32 + direction).rem_euclid(len)) as usize;
        self.ssl_mode = SslMode::from_index(self.ssl_mode_index);
    }

    fn mongo_scheme(&self) -> &'static str {
        if self
            .mongo_uri
            .as_deref()
            .is_some_and(|uri| uri.starts_with("mongodb+srv://"))
        {
            "mongodb+srv"
        } else {
            "mongodb"
        }
    }

    fn build_mongo_uri(&self, password: Option<&str>) -> String {
        let base = self.mongo_uri.as_deref().unwrap_or("mongodb://localhost");
        let mut url = Url::parse(base)
            .ok()
            .or_else(|| Url::parse("mongodb://localhost").ok())
            .expect("static Mongo URL should parse");

        let host = if self.host.trim().is_empty() {
            "localhost"
        } else {
            self.host.trim()
        };
        let _ = url.set_host(Some(host));

        let scheme = self.mongo_scheme();
        if scheme == "mongodb+srv" {
            let _ = url.set_port(None);
        } else {
            let parsed_port = self.port.parse::<u16>().ok();
            let port = parsed_port.or(url.port()).unwrap_or(27017);
            let _ = url.set_port(Some(port));
        }

        let user = self.user.trim();
        if user.is_empty() {
            let _ = url.set_username("");
            let _ = url.set_password(None);
        } else {
            let _ = url.set_username(user);
            let _ = url.set_password(password);
        }

        if self.database.trim().is_empty() {
            url.set_path("/");
        } else {
            url.set_path(&format!("/{}", self.database.trim()));
        }

        let mut built = url.to_string();
        if scheme == "mongodb+srv" && built.starts_with("mongodb://") {
            built = built.replacen("mongodb://", "mongodb+srv://", 1);
        }
        built
    }

    fn build_entry(&self, name: String, password_for_uri: Option<&str>) -> ConnectionEntry {
        let op_ref = self.op_ref.trim();
        let has_op_ref = !op_ref.is_empty();
        let no_password_required = self.password.is_empty() && !self.save_password && !has_op_ref;

        match self.kind {
            DbKind::Postgres => {
                let port = self.port.parse::<u16>().unwrap_or(5432);
                ConnectionEntry {
                    kind: DbKind::Postgres,
                    name,
                    uri: None,
                    host: self.host.clone(),
                    port,
                    database: self.database.clone(),
                    user: self.user.clone(),
                    password_in_keychain: self.save_password && !self.password.is_empty(),
                    password_env: None,
                    password_onepassword: if has_op_ref {
                        Some(op_ref.to_string())
                    } else {
                        None
                    },
                    no_password_required,
                    color: self.color,
                    favorite: None,
                    ssl_mode: match self.ssl_mode {
                        SslMode::Disable => None,
                        other => Some(other),
                    },
                }
            }
            DbKind::Mongo => ConnectionEntry {
                kind: DbKind::Mongo,
                name,
                uri: Some(self.build_mongo_uri(password_for_uri)),
                host: if self.host.trim().is_empty() {
                    "localhost".to_string()
                } else {
                    self.host.trim().to_string()
                },
                port: self.port.parse::<u16>().unwrap_or(27017),
                database: self.database.clone(),
                user: self.user.clone(),
                password_in_keychain: self.save_password && !self.password.is_empty(),
                password_env: None,
                password_onepassword: if has_op_ref {
                    Some(op_ref.to_string())
                } else {
                    None
                },
                no_password_required,
                color: self.color,
                favorite: None,
                ssl_mode: None,
            },
        }
    }

    fn process_url_paste(&mut self) -> ConnectionFormAction {
        if self.url_paste.is_empty() {
            return ConnectionFormAction::Continue;
        }

        // Try to parse the URL
        match ConnectionEntry::from_url("temp", &self.url_paste) {
            Ok((entry, password)) => {
                // Populate fields from parsed URL
                self.kind = entry.kind;
                self.mongo_uri = entry.uri.clone();
                self.host = entry.host;
                self.port = entry.port.to_string();
                self.database = entry.database;
                self.user = entry.user;
                self.ssl_mode = entry.ssl_mode.unwrap_or(SslMode::Disable);
                self.ssl_mode_index = self.ssl_mode.to_index();

                if let Some(pwd) = password {
                    self.password = pwd;
                    self.password_cursor = Self::char_count(&self.password);
                }

                // Update cursors
                self.host_cursor = Self::char_count(&self.host);
                self.port_cursor = Self::char_count(&self.port);
                self.database_cursor = Self::char_count(&self.database);
                self.user_cursor = Self::char_count(&self.user);

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
        if self.kind == DbKind::Postgres {
            if self.database.is_empty() {
                self.focused = FormField::Database;
                return ConnectionFormAction::StatusMessage("Database is required".to_string());
            }
            if self.user.is_empty() {
                self.focused = FormField::User;
                return ConnectionFormAction::StatusMessage("User is required".to_string());
            }
        }

        let needs_port = self.kind == DbKind::Postgres || self.mongo_scheme() != "mongodb+srv";
        if needs_port {
            match self.port.parse::<u16>() {
                Ok(p) if p > 0 => p,
                _ => {
                    self.focused = FormField::Port;
                    return ConnectionFormAction::StatusMessage("Invalid port number".to_string());
                }
            };
        }

        let entry = self.build_entry(self.name.clone(), None);

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
        if self.kind == DbKind::Postgres {
            if self.database.is_empty() {
                return ConnectionFormAction::StatusMessage(
                    "Database is required for test".to_string(),
                );
            }
            if self.user.is_empty() {
                return ConnectionFormAction::StatusMessage(
                    "User is required for test".to_string(),
                );
            }
        }

        let needs_port = self.kind == DbKind::Postgres || self.mongo_scheme() != "mongodb+srv";
        if needs_port {
            match self.port.parse::<u16>() {
                Ok(p) if p > 0 => p,
                _ => {
                    return ConnectionFormAction::StatusMessage("Invalid port number".to_string());
                }
            };
        }

        let test_password = if self.password.is_empty() {
            None
        } else {
            Some(self.password.clone())
        };
        let entry = self.build_entry("test".to_string(), test_password.as_deref());
        let mut entry = ConnectionEntry {
            password_in_keychain: false,
            color: ConnectionColor::None,
            ..entry
        };
        if entry.kind == DbKind::Mongo {
            entry.ssl_mode = None;
        }
        ConnectionFormAction::TestConnection {
            entry,
            password: test_password,
        }
    }

    /// Render the connection form modal.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Calculate modal size
        let modal_width = 60u16.min(area.width - 4);
        let modal_height = 21u16.min(area.height - 2);
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

        // Layout the form fields.
        // Order: Name → Kind → User → Password → [OnePasswordRef] → SavePassword → SSL Mode → Host → Port → Database → Color → UrlPaste
        let mut constraints = vec![
            Constraint::Length(1), // Name
            Constraint::Length(1), // Kind
            Constraint::Length(1), // Separator
            Constraint::Length(1), // User
            Constraint::Length(1), // Password
        ];
        if self.onepassword_enabled {
            constraints.push(Constraint::Length(1)); // 1Password ref
        }
        constraints.extend([
            Constraint::Length(1), // Save password checkbox
            Constraint::Length(1), // SSL mode
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Host
            Constraint::Length(1), // Port
            Constraint::Length(1), // Database
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Color
            Constraint::Length(1), // URL paste
            Constraint::Length(1), // Separator
            Constraint::Length(1), // Help line
        ]);
        let chunks = Layout::vertical(constraints).split(inner);

        let mut i = 0usize;
        self.render_text_field(
            frame,
            chunks[i],
            "Name:",
            &self.name,
            self.name_cursor,
            FormField::Name,
        );
        i += 1;
        self.render_kind_field(frame, chunks[i]);
        i += 1;
        self.render_separator(frame, chunks[i]);
        i += 1;
        self.render_text_field(
            frame,
            chunks[i],
            "User:",
            &self.user,
            self.user_cursor,
            FormField::User,
        );
        i += 1;
        self.render_password_field(frame, chunks[i]);
        i += 1;
        if self.onepassword_enabled {
            self.render_text_field(
                frame,
                chunks[i],
                "op ref:",
                &self.op_ref,
                self.op_ref_cursor,
                FormField::OnePasswordRef,
            );
            i += 1;
        }
        self.render_checkbox(
            frame,
            chunks[i],
            "Save to keychain",
            self.save_password,
            FormField::SavePassword,
        );
        i += 1;
        self.render_ssl_mode_field(frame, chunks[i]);
        i += 1;
        self.render_separator(frame, chunks[i]);
        i += 1;
        self.render_text_field(
            frame,
            chunks[i],
            "Host:",
            &self.host,
            self.host_cursor,
            FormField::Host,
        );
        i += 1;
        self.render_text_field(
            frame,
            chunks[i],
            "Port:",
            &self.port,
            self.port_cursor,
            FormField::Port,
        );
        i += 1;
        self.render_text_field(
            frame,
            chunks[i],
            "Database:",
            &self.database,
            self.database_cursor,
            FormField::Database,
        );
        i += 1;
        self.render_separator(frame, chunks[i]);
        i += 1;
        self.render_color_field(frame, chunks[i]);
        i += 1;
        self.render_url_paste_field(frame, chunks[i]);
        i += 1;
        self.render_separator(frame, chunks[i]);
        i += 1;
        self.render_help(frame, chunks[i]);
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
        let masked: String = "•".repeat(self.password.chars().count());
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

    fn render_checkbox(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        checked: bool,
        field: FormField,
    ) {
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

    fn render_kind_field(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focused == FormField::Kind;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new("Type:").style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        let kind_name = match self.kind {
            DbKind::Postgres => "Postgres",
            DbKind::Mongo => "MongoDB",
        };
        let mut spans = vec![];
        if is_focused {
            spans.push(Span::styled("◀ ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            format!("{:<11}", kind_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        if is_focused {
            spans.push(Span::styled(" ▶", Style::default().fg(Color::DarkGray)));
        }

        let widget = Paragraph::new(Line::from(spans));
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

    fn render_ssl_mode_field(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focused == FormField::SslMode;
        let label_width = 10;

        let chunks =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(area);

        // Label
        let label_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_widget = Paragraph::new("SSL:").style(label_style);
        frame.render_widget(label_widget, chunks[0]);

        let mode_name = if self.kind == DbKind::Mongo {
            "n/a"
        } else {
            self.ssl_mode.as_str()
        };
        let mut spans = vec![];
        if is_focused && self.kind == DbKind::Postgres {
            spans.push(Span::styled("◀ ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            format!("{:<11}", mode_name), // 11 chars to fit "verify-full"
            Style::default()
                .fg(if self.kind == DbKind::Mongo {
                    Color::DarkGray
                } else {
                    Color::White
                })
                .add_modifier(if self.kind == DbKind::Mongo {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ));
        if is_focused && self.kind == DbKind::Postgres {
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
                vec![Span::raw("postgres://... or mongodb://...")],
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
        let save_key = self.save_key_display();
        let test_key = self.test_key_display();

        let help_spans = vec![
            Span::styled("Tab", Style::default().fg(Color::Yellow)),
            Span::raw(" next  "),
            Span::styled(save_key, Style::default().fg(Color::Yellow)),
            Span::raw(" save  "),
            Span::styled(test_key, Style::default().fg(Color::Yellow)),
            Span::raw(" test  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ];

        let help =
            Paragraph::new(Line::from(help_spans)).alignment(ratatui::layout::Alignment::Center);
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
        // Order: Name → Kind → User → Password → OnePasswordRef → SavePassword → SSL Mode → Host → Port → Database → Color → UrlPaste
        assert_eq!(FormField::Name.next(), FormField::Kind);
        assert_eq!(FormField::Kind.next(), FormField::User);
        assert_eq!(FormField::User.next(), FormField::Password);
        assert_eq!(FormField::Password.next(), FormField::OnePasswordRef);
        assert_eq!(FormField::OnePasswordRef.next(), FormField::SavePassword);
        assert_eq!(FormField::SavePassword.next(), FormField::SslMode);
        assert_eq!(FormField::SslMode.next(), FormField::Host);
        assert_eq!(FormField::UrlPaste.next(), FormField::Name);
        assert_eq!(FormField::Name.prev(), FormField::UrlPaste);
        assert_eq!(FormField::Kind.prev(), FormField::Name);
        assert_eq!(FormField::User.prev(), FormField::Kind);
        assert_eq!(FormField::Password.prev(), FormField::User);
        assert_eq!(FormField::OnePasswordRef.prev(), FormField::Password);
        assert_eq!(FormField::SavePassword.prev(), FormField::OnePasswordRef);
        assert_eq!(FormField::Host.prev(), FormField::SslMode);
    }

    #[test]
    fn test_new_form_defaults() {
        let form = ConnectionFormModal::new();
        assert_eq!(form.kind, DbKind::Postgres);
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
    fn test_edit_form_initializes_unicode_cursor_by_character_count() {
        let entry = ConnectionEntry {
            name: "db—prod".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "admin".to_string(),
            ..Default::default()
        };

        let form = ConnectionFormModal::edit(&entry, None);
        assert_eq!(form.name_cursor, entry.name.chars().count());
    }

    #[test]
    fn test_duplicate_form() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "db.example.com".to_string(),
            port: 5433,
            database: "mydb".to_string(),
            user: "admin".to_string(),
            color: ConnectionColor::Green,
            favorite: Some(2),
            ..Default::default()
        };

        let form = ConnectionFormModal::duplicate_with_keymap_and_onepassword(
            &entry,
            Some("secret".to_string()),
            "test copy".to_string(),
            Keymap::default_connection_form_keymap(),
            true,
        );

        assert_eq!(form.name, "test copy");
        assert_eq!(form.host, "db.example.com");
        assert_eq!(form.port, "5433");
        assert_eq!(form.password, "secret");
        assert!(!form.editing);
        assert!(form.original_name.is_none());
        assert_eq!(form.title, "Duplicate: test");
    }

    #[test]
    fn test_tab_navigation() {
        let mut form = ConnectionFormModal::new();
        assert_eq!(form.focused, FormField::Name);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::Kind); // New order: Name → Kind

        form.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(form.focused, FormField::Name);
    }

    #[test]
    fn test_tab_navigation_skips_onepassword_when_disabled() {
        let mut form = ConnectionFormModal::with_keymap_and_onepassword(
            Keymap::default_connection_form_keymap(),
            false,
        );
        assert_eq!(form.focused, FormField::Name);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::Kind);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::User);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::Password);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            form.focused,
            FormField::SavePassword,
            "Tab should skip OnePasswordRef when disabled"
        );

        form.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(form.focused, FormField::Password);
    }

    #[test]
    fn test_tab_navigation_includes_onepassword_when_enabled() {
        let mut form = ConnectionFormModal::with_keymap_and_onepassword(
            Keymap::default_connection_form_keymap(),
            true,
        );
        assert_eq!(form.focused, FormField::Name);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::Kind);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::User);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.focused, FormField::Password);

        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            form.focused,
            FormField::OnePasswordRef,
            "Tab should land on OnePasswordRef when enabled"
        );
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
    fn test_unicode_input_and_editing_uses_char_indices() {
        let mut form = ConnectionFormModal::new();
        form.name.clear();
        form.name_cursor = 0;

        form.handle_key(KeyEvent::new(KeyCode::Char('—'), KeyModifiers::NONE));
        assert_eq!(form.name, "—");
        assert_eq!(form.name_cursor, 1);

        // This used to panic because cursor position was treated as a byte offset.
        form.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(form.name, "—a");
        assert_eq!(form.name_cursor, 2);

        form.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(form.name_cursor, 1);

        form.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(form.name, "—");
        assert_eq!(form.name_cursor, 1);

        form.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(form.name, "");
        assert_eq!(form.name_cursor, 0);
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
        assert_eq!(form.kind, DbKind::Postgres);
        assert!(form.url_paste.is_empty());
    }

    #[test]
    fn test_url_paste_mongodb_sets_kind_and_uri() {
        let mut form = ConnectionFormModal::new();
        form.focused = FormField::UrlPaste;
        form.url_paste =
            "mongodb://admin:secret@mongo.example.com:27018/sample?authSource=admin".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, ConnectionFormAction::StatusMessage(_)));
        assert_eq!(form.kind, DbKind::Mongo);
        assert_eq!(form.host, "mongo.example.com");
        assert_eq!(form.port, "27018");
        assert_eq!(form.database, "sample");
        assert_eq!(form.user, "admin");
        assert_eq!(form.password, "secret");
        let uri = form.mongo_uri.as_deref().unwrap_or("");
        assert!(uri.starts_with("mongodb://admin@mongo.example.com:27018/sample"));
        assert!(!uri.contains("secret"));
    }

    #[test]
    fn test_save_mongodb_preserves_uri() {
        let mut form = ConnectionFormModal::new();
        form.name = "mongo1".to_string();
        form.kind = DbKind::Mongo;
        form.host = "mongo.example.com".to_string();
        form.port = "27018".to_string();
        form.database = "sample".to_string();
        form.user = "admin".to_string();
        form.mongo_uri =
            Some("mongodb://admin@mongo.example.com:27018/sample?authSource=admin".to_string());

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        match action {
            ConnectionFormAction::Save { entry, .. } => {
                assert_eq!(entry.kind, DbKind::Mongo);
                assert!(entry.uri.is_some());
                let uri = entry.uri.as_deref().unwrap_or("");
                assert!(uri.contains("authSource=admin"));
                assert!(!uri.contains(":secret@"));
            }
            other => panic!("Expected Save action, got {:?}", other),
        }
    }

    #[test]
    fn test_test_connection_mongodb_includes_password_in_runtime_url() {
        let mut form = ConnectionFormModal::new();
        form.kind = DbKind::Mongo;
        form.host = "mongo.example.com".to_string();
        form.port = "27018".to_string();
        form.database = "sample".to_string();
        form.user = "admin".to_string();
        form.password = "secret".to_string();
        form.mongo_uri = Some("mongodb://admin@mongo.example.com:27018/sample".to_string());

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        match action {
            ConnectionFormAction::TestConnection { entry, password } => {
                assert_eq!(entry.kind, DbKind::Mongo);
                assert_eq!(password.as_deref(), Some("secret"));
                let url = entry.to_url(password.as_deref());
                assert!(url.contains("mongodb://admin:secret@mongo.example.com:27018/sample"));
            }
            other => panic!("Expected TestConnection action, got {:?}", other),
        }
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

    #[test]
    fn test_test_connection_preserves_trimmed_onepassword_ref() {
        let mut form = ConnectionFormModal::new();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();
        form.op_ref = "  op://vault/item/password  ".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        match action {
            ConnectionFormAction::TestConnection { entry, .. } => {
                assert_eq!(
                    entry.password_onepassword.as_deref(),
                    Some("op://vault/item/password")
                );
                assert!(!entry.no_password_required);
            }
            _ => panic!("Expected TestConnection action"),
        }
    }

    #[test]
    fn test_save_with_onepassword_ref_is_not_marked_no_password_required() {
        let mut form = ConnectionFormModal::new();
        form.name = "testconn".to_string();
        form.host = "localhost".to_string();
        form.database = "mydb".to_string();
        form.user = "postgres".to_string();
        form.op_ref = " op://vault/item/password ".to_string();
        form.password.clear();
        form.save_password = false;

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        match action {
            ConnectionFormAction::Save { entry, .. } => {
                assert_eq!(
                    entry.password_onepassword.as_deref(),
                    Some("op://vault/item/password")
                );
                assert!(!entry.no_password_required);
            }
            _ => panic!("Expected Save action"),
        }
    }

    // ========== Issue Reproduction Tests ==========

    /// Test that Ctrl+S saves the form (default save shortcut)
    #[test]
    fn test_ctrl_s_saves_form() {
        let mut form = ConnectionFormModal::new();
        form.name = "testconn".to_string();
        form.host = "localhost".to_string();
        form.database = "testdb".to_string();
        form.user = "postgres".to_string();

        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::Save { entry, .. } => {
                assert_eq!(entry.name, "testconn");
            }
            other => panic!("Expected Save action with Ctrl+S, got {:?}", other),
        }
    }

    /// Test that Ctrl+S works with exact CONTROL modifier
    #[test]
    fn test_ctrl_s_with_exact_control_modifier() {
        let mut form = ConnectionFormModal::new();
        form.name = "testconn".to_string();
        form.host = "localhost".to_string();
        form.database = "testdb".to_string();
        form.user = "postgres".to_string();

        // Exact CONTROL modifier (ideal case)
        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::Save { entry, .. } => {
                assert_eq!(entry.name, "testconn");
            }
            other => panic!("Expected Save action with exact CONTROL, got {:?}", other),
        }
    }

    /// Test custom keymap binding for save
    #[test]
    fn test_custom_save_keybinding() {
        use crate::config::{Action, KeyBinding, Keymap};

        // Create a custom keymap with Ctrl+W as the save key
        let mut keymap = Keymap::new();
        keymap.bind(
            KeyBinding::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Action::SaveConnection,
        );

        let mut form = ConnectionFormModal::with_keymap(keymap);
        form.name = "testconn".to_string();
        form.host = "localhost".to_string();
        form.database = "testdb".to_string();
        form.user = "postgres".to_string();

        // Ctrl+W should now save
        let action = form.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

        match action {
            ConnectionFormAction::Save { entry, .. } => {
                assert_eq!(entry.name, "testconn");
            }
            other => panic!("Expected Save action with Ctrl+W, got {:?}", other),
        }

        // Ctrl+S should NOT save (not in the custom keymap)
        let mut form2 = ConnectionFormModal::with_keymap(Keymap::new());
        form2.name = "testconn".to_string();
        form2.host = "localhost".to_string();
        form2.database = "testdb".to_string();
        form2.user = "postgres".to_string();

        let action2 = form2.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(
            action2,
            ConnectionFormAction::Continue,
            "Ctrl+S should not save with empty keymap"
        );
    }

    #[test]
    fn test_ctrl_s_first_press_should_work() {
        // Simulating the exact user scenario: fill form and press Ctrl+S once
        let mut form = ConnectionFormModal::new();

        // User types "Paypol" in name field
        form.name = "Paypol".to_string();
        form.name_cursor = 6;

        // User fills other fields
        form.host = "localhost".to_string();
        form.database = "paypol".to_string();
        form.user = "postgres".to_string();

        // User presses Ctrl+S ONCE - should work immediately
        let action = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        // This should be a Save action, not Continue
        assert!(
            matches!(action, ConnectionFormAction::Save { .. }),
            "First Ctrl+S press should trigger Save, got {:?}",
            action
        );
    }

    /// Issue 3: Esc requiring two presses
    /// Test Esc behavior with modified vs unmodified forms
    #[test]
    fn test_esc_on_unmodified_form_closes_immediately() {
        let form = ConnectionFormModal::new();
        // New form with default values should not be "modified"
        assert!(
            !form.is_modified(),
            "New form with defaults should not be modified"
        );
    }

    #[test]
    fn test_esc_on_modified_form_requests_confirmation() {
        let mut form = ConnectionFormModal::new();
        // User types a name - form is now modified
        form.name = "Paypol".to_string();

        assert!(form.is_modified(), "Form with name should be modified");

        let action = form.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            action,
            ConnectionFormAction::RequestClose,
            "Esc on modified form should request close (show confirmation)"
        );
    }

    #[test]
    fn test_form_modification_detection() {
        let mut form = ConnectionFormModal::new();

        // Initially unmodified (only defaults)
        assert!(!form.is_modified(), "New form should not be modified");

        // Setting name makes it modified
        form.name = "test".to_string();
        assert!(form.is_modified(), "Form with name should be modified");

        // Clear name, still modified because user changed something
        form.name.clear();
        // Actually, empty name with default host/port should NOT be modified
        assert!(
            !form.is_modified(),
            "Form back to defaults should not be modified"
        );

        // Change host from default
        form.host = "remotehost".to_string();
        assert!(
            form.is_modified(),
            "Form with non-default host should be modified"
        );
    }
}
