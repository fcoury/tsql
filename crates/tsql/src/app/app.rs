use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio_postgres::{CancelToken, Client, NoTls, SimpleQueryMessage};
use tui_textarea::{CursorMove, Input};

use super::state::{DbStatus, Focus, Mode, PanelDirection, SearchTarget, SidebarSection};
use crate::config::{
    load_connections, save_connections, Action, Config, ConnectionEntry, ConnectionsFile,
    KeyBinding, Keymap,
};
use crate::history::{History, HistoryEntry};
use crate::session::SessionState;
use crate::ui::{
    create_sql_highlighter, determine_context, escape_sql_value, get_word_before_cursor, is_inside,
    quote_identifier, ColumnInfo, CommandPrompt, CompletionKind, CompletionPopup, ConfirmContext,
    ConfirmPrompt, ConfirmResult, ConnectionFormAction, ConnectionFormModal, ConnectionInfo,
    ConnectionManagerAction, ConnectionManagerModal, CursorShape, DataGrid, FuzzyPicker,
    GridKeyResult, GridModel, GridState, HelpAction, HelpPopup, HighlightedTextArea,
    JsonEditorAction, JsonEditorModal, KeyHintPopup, KeySequenceAction, KeySequenceCompletion,
    KeySequenceHandlerWithContext, KeySequenceResult, PendingKey, PickerAction, Priority,
    QueryEditor, ResizeAction, RowDetailAction, RowDetailModal, SchemaCache, SearchPrompt, Sidebar,
    SidebarAction, StatusLineBuilder, StatusSegment, TableInfo,
};
use crate::util::format_pg_error;
use crate::util::{is_json_column_type, should_use_multiline_editor};
use throbber_widgets_tui::{Throbber, ThrobberState, BRAILLE_SIX};
use tui_syntax::Highlighter;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SchemaTableContext {
    schema: String,
    table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchemaTreeSelection {
    Schema {
        schema: String,
    },
    Table {
        schema: String,
        table: String,
    },
    Column {
        schema: String,
        table: String,
        column: String,
    },
    Unknown {
        raw: String,
    },
}

/// Characters to percent-encode in schema tree identifiers (`:` is the delimiter).
const SCHEMA_ID_ENCODE_SET: &AsciiSet = &CONTROLS.add(b':').add(b'%');

/// Percent-encode a component for use in schema tree identifiers.
pub fn encode_schema_id_component(s: &str) -> String {
    utf8_percent_encode(s, SCHEMA_ID_ENCODE_SET).to_string()
}

/// Percent-decode a component from a schema tree identifier.
fn decode_schema_id_component(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn parse_schema_tree_identifier(identifier: &str) -> SchemaTreeSelection {
    if let Some(schema) = identifier.strip_prefix("schema:") {
        let schema = decode_schema_id_component(schema);
        if !schema.is_empty() {
            return SchemaTreeSelection::Schema { schema };
        }
    }

    if let Some(rest) = identifier.strip_prefix("table:") {
        let mut parts = rest.splitn(2, ':');
        let schema = parts
            .next()
            .map(decode_schema_id_component)
            .unwrap_or_default();
        let table = parts
            .next()
            .map(decode_schema_id_component)
            .unwrap_or_default();
        if !schema.is_empty() && !table.is_empty() {
            return SchemaTreeSelection::Table { schema, table };
        }
    }

    if let Some(rest) = identifier.strip_prefix("column:") {
        let mut parts = rest.splitn(3, ':');
        let schema = parts
            .next()
            .map(decode_schema_id_component)
            .unwrap_or_default();
        let table = parts
            .next()
            .map(decode_schema_id_component)
            .unwrap_or_default();
        let column = parts
            .next()
            .map(decode_schema_id_component)
            .unwrap_or_default();
        if !schema.is_empty() && !table.is_empty() && !column.is_empty() {
            return SchemaTreeSelection::Column {
                schema,
                table,
                column,
            };
        }
    }

    SchemaTreeSelection::Unknown {
        raw: identifier.to_string(),
    }
}

// Meta-command SQL queries (psql-style \dt, \d, etc.)

/// List all tables in the current database
const META_QUERY_TABLES: &str = r#"
SELECT 
    schemaname AS schema,
    tablename AS name,
    tableowner AS owner
FROM pg_catalog.pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY schemaname, tablename
"#;

/// List all schemas
const META_QUERY_SCHEMAS: &str = r#"
SELECT 
    schema_name AS name,
    schema_owner AS owner
FROM information_schema.schemata
WHERE schema_name NOT LIKE 'pg_%'
  AND schema_name != 'information_schema'
ORDER BY schema_name
"#;

/// Describe a table (columns, types, constraints)
const META_QUERY_DESCRIBE: &str = r#"
SELECT 
    c.column_name AS column,
    c.data_type AS type,
    CASE WHEN c.is_nullable = 'YES' THEN 'NULL' ELSE 'NOT NULL' END AS nullable,
    c.column_default AS default,
    CASE WHEN pk.column_name IS NOT NULL THEN 'PK' ELSE '' END AS key
FROM information_schema.columns c
LEFT JOIN (
    SELECT ku.column_name
    FROM information_schema.table_constraints tc
    JOIN information_schema.key_column_usage ku
        ON tc.constraint_name = ku.constraint_name
        AND tc.table_schema = ku.table_schema
    WHERE tc.constraint_type = 'PRIMARY KEY'
      AND tc.table_name = '$1'
) pk ON c.column_name = pk.column_name
WHERE c.table_name = '$1'
ORDER BY c.ordinal_position
"#;

/// List all indexes
const META_QUERY_INDEXES: &str = r#"
SELECT 
    schemaname AS schema,
    tablename AS table,
    indexname AS index,
    indexdef AS definition
FROM pg_catalog.pg_indexes
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY schemaname, tablename, indexname
"#;

/// List all databases (\l)
const META_QUERY_DATABASES: &str = r#"
SELECT 
    datname AS name,
    pg_catalog.pg_get_userbyid(datdba) AS owner,
    pg_catalog.pg_encoding_to_char(encoding) AS encoding
FROM pg_catalog.pg_database
WHERE datallowconn = true
ORDER BY datname
"#;

/// List all roles/users (\du)
const META_QUERY_ROLES: &str = r#"
SELECT 
    rolname AS role,
    CASE WHEN rolsuper THEN 'Superuser' ELSE '' END AS super,
    CASE WHEN rolcreaterole THEN 'Create role' ELSE '' END AS create_role,
    CASE WHEN rolcreatedb THEN 'Create DB' ELSE '' END AS create_db,
    CASE WHEN rolcanlogin THEN 'Login' ELSE '' END AS login
FROM pg_catalog.pg_roles
WHERE rolname NOT LIKE 'pg_%'
ORDER BY rolname
"#;

/// List all views (\dv)
const META_QUERY_VIEWS: &str = r#"
SELECT 
    schemaname AS schema,
    viewname AS name,
    viewowner AS owner
FROM pg_catalog.pg_views
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY schemaname, viewname
"#;

/// List all functions (\df)
const META_QUERY_FUNCTIONS: &str = r#"
SELECT 
    n.nspname AS schema,
    p.proname AS name,
    pg_catalog.pg_get_function_result(p.oid) AS result_type,
    pg_catalog.pg_get_function_arguments(p.oid) AS arguments
FROM pg_catalog.pg_proc p
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
ORDER BY n.nspname, p.proname
"#;

/// Get primary key columns for a table
const META_QUERY_PRIMARY_KEYS: &str = r#"
SELECT ku.column_name
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage ku
    ON tc.constraint_name = ku.constraint_name
    AND tc.table_schema = ku.table_schema
WHERE tc.constraint_type = 'PRIMARY KEY'
  AND tc.table_name = '$1'
ORDER BY ku.ordinal_position
"#;

/// Escape a SQL identifier for use in queries (prevents SQL injection)
fn escape_sql_identifier(s: &str) -> String {
    // Remove any existing quotes and escape internal quotes
    let cleaned = s.trim_matches('"').replace('"', "\"\"");
    // For simple identifiers, return as-is; otherwise quote
    if cleaned
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        cleaned
    } else {
        format!("\"{}\"", cleaned)
    }
}

/// Fetch primary key column names for a table.
async fn fetch_primary_keys(client: &SharedClient, table: &str) -> Vec<String> {
    let query = META_QUERY_PRIMARY_KEYS.replace("$1", &escape_sql_identifier(table));
    let guard = client.lock().await;

    match guard.simple_query(&query).await {
        Ok(messages) => {
            let mut pks = Vec::new();
            for msg in messages {
                if let SimpleQueryMessage::Row(row) = msg {
                    if let Some(col_name) = row.get(0) {
                        pks.push(col_name.to_string());
                    }
                }
            }
            pks
        }
        Err(_) => Vec::new(), // Silently fail - PK detection is optional
    }
}

/// Query to fetch column types for a table.
const META_QUERY_COLUMN_TYPES: &str = r#"
SELECT column_name, data_type
FROM information_schema.columns
WHERE table_name = '$1'
ORDER BY ordinal_position
"#;

/// Fetch column types for a table, returning a map of column_name -> data_type.
async fn fetch_column_types(
    client: &SharedClient,
    table: &str,
) -> std::collections::HashMap<String, String> {
    let query = META_QUERY_COLUMN_TYPES.replace("$1", &escape_sql_identifier(table));
    let guard = client.lock().await;

    match guard.simple_query(&query).await {
        Ok(messages) => {
            let mut types = std::collections::HashMap::new();
            for msg in messages {
                if let SimpleQueryMessage::Row(row) = msg {
                    if let (Some(col_name), Some(data_type)) = (row.get(0), row.get(1)) {
                        types.insert(col_name.to_string(), data_type.to_string());
                    }
                }
            }
            types
        }
        Err(_) => std::collections::HashMap::new(), // Silently fail - type detection is optional
    }
}

pub struct QueryResult {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub command_tag: Option<String>,
    pub truncated: bool,
    pub elapsed: Duration,
    /// The source table name, if extracted from a simple SELECT query.
    pub source_table: Option<String>,
    /// Primary key column names for the source table.
    pub primary_keys: Vec<String>,
    /// Column data types from PostgreSQL (e.g., "jsonb", "text", "int4").
    pub col_types: Vec<String>,
}

/// Extract the table name from a simple SELECT query.
/// Returns Some(table_name) for queries like:
/// - SELECT * FROM users
/// - SELECT id, name FROM public.users
/// - select * from "My Table"
///
/// Returns None for complex queries (JOINs, subqueries, etc.)
fn extract_table_from_query(query: &str) -> Option<String> {
    let query = query.trim().to_lowercase();

    // Must start with SELECT
    if !query.starts_with("select") {
        return None;
    }

    // Find FROM keyword
    let from_pos = query.find(" from ")?;
    let after_from = &query[from_pos + 6..].trim_start();

    // Extract the table name (first word after FROM)
    // Stop at whitespace, semicolon, or end of string
    let table_end = after_from
        .find(|c: char| c.is_whitespace() || c == ';' || c == ')')
        .unwrap_or(after_from.len());

    let table = after_from[..table_end].trim();

    if table.is_empty() {
        return None;
    }

    // Check for complex queries (JOINs, subqueries)
    let rest = &after_from[table_end..];
    if rest.contains(" join ") || table.starts_with('(') {
        return None;
    }

    // Remove schema prefix if present (public.users -> users)
    let table_name = table.rsplit('.').next().unwrap_or(table);

    // Remove quotes if present
    let table_name = table_name.trim_matches('"').trim_matches('\'');

    Some(table_name.to_string())
}

pub type SharedClient = Arc<Mutex<Client>>;

pub enum DbEvent {
    Connected {
        client: SharedClient,
        cancel_token: CancelToken,
    },
    ConnectError {
        error: String,
    },
    ConnectionLost {
        error: String,
    },
    QueryFinished {
        result: QueryResult,
    },
    QueryError {
        error: String,
    },
    QueryCancelled,
    SchemaLoaded {
        tables: Vec<TableInfo>,
    },
    /// A cell was successfully updated.
    CellUpdated {
        row: usize,
        col: usize,
        value: String,
    },
    /// Result of a connection test (from connection form).
    TestConnectionResult {
        success: bool,
        message: String,
    },
}

pub struct DbSession {
    pub status: DbStatus,
    pub conn_str: Option<String>,
    pub client: Option<SharedClient>,
    pub cancel_token: Option<CancelToken>,
    pub last_command_tag: Option<String>,
    pub last_elapsed: Option<Duration>,
    pub running: bool,
    /// Whether we're currently in a transaction (after BEGIN, before COMMIT/ROLLBACK)
    pub in_transaction: bool,
}

impl DbSession {
    pub fn new() -> Self {
        Self {
            status: DbStatus::Disconnected,
            conn_str: None,
            client: None,
            cancel_token: None,
            last_command_tag: None,
            last_elapsed: None,
            running: false,
            in_transaction: false,
        }
    }
}

impl Default for DbSession {
    fn default() -> Self {
        Self::new()
    }
}

/// State for inline cell editing with cursor support.
#[derive(Default)]
pub struct CellEditor {
    /// Whether cell editing is active.
    pub active: bool,
    /// The row being edited.
    pub row: usize,
    /// The column being edited.
    pub col: usize,
    /// The current edit value.
    pub value: String,
    /// The original value (for cancel).
    pub original_value: String,
    /// Cursor position within the value (byte offset).
    pub cursor: usize,
    /// Horizontal scroll offset for display.
    pub scroll_offset: usize,
}

impl CellEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, row: usize, col: usize, value: String) {
        self.active = true;
        self.row = row;
        self.col = col;
        self.original_value = value.clone();
        self.cursor = value.len(); // Start cursor at end
        self.scroll_offset = 0;
        self.value = value;
    }

    pub fn close(&mut self) {
        self.active = false;
        self.value.clear();
        self.original_value.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Check if the value has been modified from the original.
    pub fn is_modified(&self) -> bool {
        self.active && self.value != self.original_value
    }

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character before the cursor (backspace).
    pub fn delete_char_before(&mut self) {
        if self.cursor > 0 {
            // Find the previous character boundary
            let prev_boundary = self.value[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.value.remove(prev_boundary);
            self.cursor = prev_boundary;
        }
    }

    /// Delete the character at the cursor (delete key).
    pub fn delete_char_at(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    /// Move cursor left by one character.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            // Find the previous character boundary
            self.cursor = self.value[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Move cursor right by one character.
    pub fn move_right(&mut self) {
        if self.cursor < self.value.len() {
            // Find the next character boundary
            self.cursor = self.value[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.value.len());
        }
    }

    /// Move cursor to the start of the value.
    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end of the value.
    pub fn move_to_end(&mut self) {
        self.cursor = self.value.len();
    }

    /// Clear the entire value.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Delete from cursor to end of line (Ctrl+K).
    pub fn delete_to_end(&mut self) {
        self.value.truncate(self.cursor);
    }

    /// Delete from start to cursor (Ctrl+U).
    pub fn delete_to_start(&mut self) {
        self.value = self.value[self.cursor..].to_string();
        self.cursor = 0;
    }

    /// Get the visible portion of the value for display, given a width.
    /// Returns (visible_text, cursor_position_in_visible).
    pub fn visible_text(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }

        let chars: Vec<char> = self.value.chars().collect();
        let cursor_char_pos = self.value[..self.cursor].chars().count();

        // Adjust scroll offset to keep cursor visible
        let mut scroll = self.scroll_offset;

        // If cursor is before the visible window, scroll left
        if cursor_char_pos < scroll {
            scroll = cursor_char_pos;
        }

        // If cursor is after the visible window, scroll right
        // Leave room for the cursor indicator
        let visible_width = width.saturating_sub(1);
        if cursor_char_pos >= scroll + visible_width {
            scroll = cursor_char_pos.saturating_sub(visible_width) + 1;
        }

        // Extract visible characters
        let visible_chars: String = chars.iter().skip(scroll).take(visible_width).collect();

        let cursor_in_visible = cursor_char_pos.saturating_sub(scroll);

        (visible_chars, cursor_in_visible)
    }

    /// Update scroll offset based on cursor position and display width.
    pub fn update_scroll(&mut self, width: usize) {
        if width == 0 {
            return;
        }

        let cursor_char_pos = self.value[..self.cursor].chars().count();
        let visible_width = width.saturating_sub(1);

        // If cursor is before the visible window, scroll left
        if cursor_char_pos < self.scroll_offset {
            self.scroll_offset = cursor_char_pos;
        }

        // If cursor is after the visible window, scroll right
        if cursor_char_pos >= self.scroll_offset + visible_width {
            self.scroll_offset = cursor_char_pos.saturating_sub(visible_width) + 1;
        }
    }
}

pub struct App {
    pub focus: Focus,
    pub mode: Mode,

    /// Application configuration
    pub config: Config,

    /// Keymap for grid navigation
    pub grid_keymap: Keymap,
    /// Keymap for editor in normal mode
    pub editor_normal_keymap: Keymap,
    /// Keymap for editor in insert mode
    pub editor_insert_keymap: Keymap,
    /// Keymap for connection form
    pub connection_form_keymap: Keymap,

    pub editor: QueryEditor,
    pub highlighter: Highlighter,
    pub search: SearchPrompt,
    pub search_target: SearchTarget,
    pub command: CommandPrompt,
    pub completion: CompletionPopup,
    pub schema_cache: SchemaCache,
    pub pending_key: Option<char>,
    /// Key sequence handler for multi-key commands like `gg`, `gc`, etc.
    key_sequence: KeySequenceHandlerWithContext<SchemaTableContext>,
    /// Editor scroll offset (row, col) for horizontal scrolling support.
    pub editor_scroll: (u16, u16),

    pub rt: tokio::runtime::Handle,
    pub db_events_tx: mpsc::UnboundedSender<DbEvent>,
    pub db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
    pub db: DbSession,

    pub grid: GridModel,
    pub grid_state: GridState,

    /// Cell editor for inline editing.
    pub cell_editor: CellEditor,

    /// JSON editor modal for multiline JSON editing.
    pub json_editor: Option<JsonEditorModal<'static>>,

    /// Last known grid viewport dimensions for scroll calculations.
    /// (viewport_rows, viewport_width)
    pub last_grid_viewport: Option<(usize, u16)>,

    /// Help popup (Some when open, None when closed).
    pub help_popup: Option<HelpPopup>,
    /// Row detail modal (Some when open, None when closed).
    pub row_detail: Option<RowDetailModal>,
    /// Confirmation prompt (Some when showing confirmation dialog).
    pub confirm_prompt: Option<ConfirmPrompt>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,

    /// Query history with persistence.
    pub history: History,
    /// Fuzzy picker for history search (when open).
    pub history_picker: Option<FuzzyPicker<HistoryEntry>>,

    /// Last rendered area for query editor (for mouse click handling).
    render_query_area: Option<Rect>,
    /// Last rendered area for results grid (for mouse click handling).
    render_grid_area: Option<Rect>,
    /// Last rendered area for sidebar (for mouse click handling).
    render_sidebar_area: Option<Rect>,

    /// Saved database connections.
    pub connections: ConnectionsFile,
    /// Name of the currently connected connection (if from saved connections).
    pub current_connection_name: Option<String>,
    /// Connection picker (fuzzy picker for quick connection selection).
    pub connection_picker: Option<FuzzyPicker<ConnectionEntry>>,
    /// Connection manager modal (when open).
    pub connection_manager: Option<ConnectionManagerModal>,
    /// Connection form modal (when open, for add/edit).
    pub connection_form: Option<ConnectionFormModal>,

    /// Sidebar component state.
    pub sidebar: Sidebar,
    /// Whether sidebar is visible.
    pub sidebar_visible: bool,
    /// Which section of the sidebar is focused.
    pub sidebar_focus: SidebarSection,
    /// Sidebar width in characters.
    pub sidebar_width: u16,
    /// Pending schema expanded paths to apply after schema loads.
    pending_schema_expanded: Option<Vec<Vec<String>>>,
    /// Cached cursor style to avoid redundant terminal updates.
    /// Uses a simple enum since SetCursorStyle doesn't implement PartialEq.
    last_cursor_style: Option<CachedCursorStyle>,

    /// Throbber animation state for loading indicator.
    throbber_state: ThrobberState,
    /// When the current query started (for elapsed time display).
    query_start_time: Option<Instant>,
}

/// Local enum to track cursor style changes (SetCursorStyle doesn't implement PartialEq).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CachedCursorStyle {
    BlinkingBar,
    SteadyBlock,
}

impl App {
    pub fn new(
        grid: GridModel,
        rt: tokio::runtime::Handle,
        db_events_tx: mpsc::UnboundedSender<DbEvent>,
        db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
        conn_str: Option<String>,
    ) -> Self {
        Self::with_config(
            grid,
            rt,
            db_events_tx,
            db_events_rx,
            conn_str,
            Config::default(),
        )
    }

    pub fn with_config(
        grid: GridModel,
        rt: tokio::runtime::Handle,
        db_events_tx: mpsc::UnboundedSender<DbEvent>,
        db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
        conn_str: Option<String>,
        config: Config,
    ) -> Self {
        let editor = QueryEditor::new();

        // Load history
        let history = History::load(config.editor.max_history).unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load history: {}", e);
            History::new_empty(config.editor.max_history)
        });

        // Determine connection string: CLI arg > config > env var
        let effective_conn_str = conn_str.or_else(|| config.connection.default_url.clone());

        // Build keymaps from defaults + config overrides
        let grid_keymap = Self::build_grid_keymap(&config);
        let editor_normal_keymap = Self::build_editor_normal_keymap(&config);
        let editor_insert_keymap = Self::build_editor_insert_keymap(&config);
        let connection_form_keymap = Self::build_connection_form_keymap(&config);
        let key_sequence_timeout_ms = config.keymap.key_sequence_timeout_ms;

        let mut app = Self {
            focus: Focus::Query,
            mode: Mode::Normal,

            config,

            grid_keymap,
            editor_normal_keymap,
            editor_insert_keymap,
            connection_form_keymap,

            editor,
            highlighter: create_sql_highlighter(),
            search: SearchPrompt::new(),
            search_target: SearchTarget::Editor,
            command: CommandPrompt::new(),
            completion: CompletionPopup::new(),
            schema_cache: SchemaCache::new(),
            pending_key: None,
            key_sequence: KeySequenceHandlerWithContext::new(key_sequence_timeout_ms),
            editor_scroll: (0, 0),

            rt,
            db_events_tx,
            db_events_rx,
            db: DbSession::new(),

            grid,
            grid_state: GridState::default(),

            cell_editor: CellEditor::new(),
            json_editor: None,

            last_grid_viewport: None,

            help_popup: None,
            row_detail: None,
            confirm_prompt: None,
            last_status: None,
            last_error: None,

            history,
            history_picker: None,

            render_query_area: None,
            render_grid_area: None,
            render_sidebar_area: None,

            connections: ConnectionsFile::new(),
            current_connection_name: None,
            connection_picker: None,
            connection_manager: None,
            connection_form: None,

            sidebar: Sidebar::new(),
            sidebar_visible: false,
            sidebar_focus: SidebarSection::Connections,
            sidebar_width: 30,
            pending_schema_expanded: None,
            last_cursor_style: None,

            throbber_state: ThrobberState::default(),
            query_start_time: None,
        };

        // Load saved connections
        app.connections = load_connections().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load connections: {}", e);
            ConnectionsFile::new()
        });

        // Handle connection on startup (only if explicit connection specified)
        if let Some(url) = effective_conn_str {
            // Check if this looks like a connection name (no :// scheme)
            if !url.contains("://") {
                // Try to find a connection by name
                if let Some(entry) = app.connections.find_by_name(&url) {
                    app.connect_to_entry(entry.clone());
                } else {
                    app.last_error = Some(format!("Unknown connection: {}", url));
                    // Open connection picker so user can select (falls back to manager if empty)
                    app.open_connection_picker();
                }
            } else {
                // It's a URL, connect directly
                app.start_connect(url);
            }
        }
        // Note: connection picker is NOT opened here when no URL specified.
        // This allows main.rs to first check session state for auto-reconnect.

        app
    }

    /// Capture current session state for persistence.
    pub fn capture_session_state(&self) -> SessionState {
        SessionState {
            connection_name: self.current_connection_name.clone(),
            editor_content: self.editor.text(),
            schema_expanded: self.sidebar.get_expanded_nodes(),
            sidebar_visible: self.sidebar_visible,
        }
    }

    /// Save session state to disk.
    pub fn save_session(&self) -> Result<()> {
        let state = self.capture_session_state();
        crate::session::save_session(&state)
    }

    /// Apply restored session state.
    /// Returns the connection name to auto-connect to, if any.
    pub fn apply_session_state(&mut self, state: SessionState) -> Option<String> {
        // Restore editor content (apply exactly, even if empty)
        self.editor.set_text(state.editor_content);
        self.editor.mark_saved();

        // Restore sidebar visibility
        self.sidebar_visible = state.sidebar_visible;

        // Store pending schema expanded for later application when schema loads
        // Always set (even to None) to clear any prior pending paths
        self.pending_schema_expanded = if state.schema_expanded.is_empty() {
            None
        } else {
            Some(state.schema_expanded)
        };

        // Return connection name for auto-connect handling
        state.connection_name
    }

    /// Apply pending schema expanded state after schema loads.
    fn apply_pending_schema_expanded(&mut self) {
        if let Some(paths) = self.pending_schema_expanded.take() {
            self.sidebar.restore_expanded_nodes(&paths);
        }
    }

    /// Build the grid keymap from defaults + config overrides
    fn build_grid_keymap(config: &Config) -> Keymap {
        let mut keymap = Keymap::default_grid_keymap();

        // Apply custom bindings from config
        for binding in &config.keymap.grid {
            if let Some(key) = KeyBinding::parse(&binding.key) {
                if let Ok(action) = binding.action.parse::<Action>() {
                    keymap.bind(key, action);
                }
            }
        }

        keymap
    }

    /// Build the editor normal mode keymap from defaults + config overrides
    fn build_editor_normal_keymap(config: &Config) -> Keymap {
        let mut keymap = Keymap::default_editor_normal_keymap();

        // Apply custom bindings from config
        for binding in &config.keymap.normal {
            if let Some(key) = KeyBinding::parse(&binding.key) {
                if let Ok(action) = binding.action.parse::<Action>() {
                    keymap.bind(key, action);
                }
            }
        }

        keymap
    }

    /// Build the editor insert mode keymap from defaults + config overrides
    fn build_editor_insert_keymap(config: &Config) -> Keymap {
        let mut keymap = Keymap::default_editor_insert_keymap();

        // Apply custom bindings from config
        for binding in &config.keymap.insert {
            if let Some(key) = KeyBinding::parse(&binding.key) {
                if let Ok(action) = binding.action.parse::<Action>() {
                    keymap.bind(key, action);
                }
            }
        }

        keymap
    }

    /// Build the connection form keymap from defaults + config overrides
    fn build_connection_form_keymap(config: &Config) -> Keymap {
        let mut keymap = Keymap::default_connection_form_keymap();

        // Apply custom bindings from config
        for binding in &config.keymap.connection_form {
            if let Some(key) = KeyBinding::parse(&binding.key) {
                if let Ok(action) = binding.action.parse::<Action>() {
                    keymap.bind(key, action);
                }
            }
        }

        keymap
    }

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            self.drain_db_events();

            // Advance throbber animation when query is running
            if self.db.running {
                self.throbber_state.calc_next();
            }

            // Pre-compute highlighted lines before the draw closure
            let query_text = self.editor.text();
            let highlighted_lines = self
                .highlighter
                .highlight("sql", &query_text)
                .unwrap_or_else(|_| {
                    // Fallback to plain text if highlighting fails
                    query_text
                        .lines()
                        .map(|s| Line::from(s.to_string()))
                        .collect()
                });

            // Compute hint visibility once per tick to avoid time-based state
            // flipping between calls during the same render cycle.
            let show_key_hint = self.key_sequence.should_show_hint();
            let pending_key_for_hint = self.key_sequence.pending();

            // Set terminal cursor style based on vim mode (only when changed)
            let cached_style = match (self.focus, self.mode) {
                (Focus::Query, Mode::Insert) => CachedCursorStyle::BlinkingBar,
                _ => CachedCursorStyle::SteadyBlock,
            };
            if self.last_cursor_style != Some(cached_style) {
                let cursor_style = match cached_style {
                    CachedCursorStyle::BlinkingBar => SetCursorStyle::BlinkingBar,
                    CachedCursorStyle::SteadyBlock => SetCursorStyle::SteadyBlock,
                };
                let _ = execute!(io::stdout(), cursor_style);
                self.last_cursor_style = Some(cached_style);
            }

            terminal.draw(|frame| {
                let size = frame.area();

                // Split horizontally for sidebar + main content
                let horizontal = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(if self.sidebar_visible {
                            self.sidebar_width
                        } else {
                            0
                        }),
                        Constraint::Min(60), // Main content minimum width
                    ])
                    .split(size);

                let sidebar_area = horizontal[0];
                let main_area = horizontal[1];

                // Render sidebar if visible
                if self.sidebar_visible && sidebar_area.width > 0 {
                    // Store sidebar area for mouse click handling
                    self.render_sidebar_area = Some(sidebar_area);

                    let schema_items = self.schema_cache.build_tree_items();
                    let has_focus = matches!(self.focus, Focus::Sidebar(_));

                    self.sidebar.render(
                        frame,
                        sidebar_area,
                        &self.connections,
                        self.current_connection_name.as_deref(),
                        &schema_items,
                        !self.schema_cache.loaded && self.db.status == DbStatus::Connected,
                        None, // No error handling yet
                        self.sidebar_focus,
                        has_focus,
                    );
                } else {
                    self.render_sidebar_area = None;
                }

                // Determine if we have an error to show.
                let error_height = if self.last_error.is_some() {
                    4u16
                } else {
                    0u16
                };

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(7),
                        Constraint::Length(error_height),
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(main_area);

                let query_area = chunks[0];
                let error_area = chunks[1];
                let grid_area = chunks[2];
                let status_area = chunks[3];

                // Store rendered areas for mouse click handling
                self.render_query_area = Some(query_area);
                self.render_grid_area = Some(grid_area);

                // Query editor with syntax highlighting
                let query_border = match (self.focus, self.mode) {
                    (Focus::Query, Mode::Normal) => Style::default().fg(Color::Cyan),
                    (Focus::Query, Mode::Insert) => Style::default().fg(Color::Green),
                    (Focus::Query, Mode::Visual) => Style::default().fg(Color::Yellow),
                    (Focus::Grid, _) | (Focus::Sidebar(_), _) => {
                        Style::default().fg(Color::DarkGray)
                    }
                };

                // Build query title with [+] indicator if modified
                let modified_indicator = if self.editor.is_modified() {
                    " [+]"
                } else {
                    ""
                };
                let query_title = match (self.focus, self.mode) {
                    (Focus::Query, Mode::Normal) => {
                        format!(
                            "Query [NORMAL]{} (i insert, Enter run, Ctrl-r history, Tab to grid)",
                            modified_indicator
                        )
                    }
                    (Focus::Query, Mode::Insert) => {
                        format!(
                            "Query [INSERT]{} (Esc normal, Ctrl-r history)",
                            modified_indicator
                        )
                    }
                    (Focus::Query, Mode::Visual) => {
                        format!(
                            "Query [VISUAL]{} (y yank, d delete, Esc cancel)",
                            modified_indicator
                        )
                    }
                    (Focus::Grid, _) | (Focus::Sidebar(_), _) => "Query (Tab to focus)".to_string(),
                };

                let query_block = Block::default()
                    .borders(Borders::ALL)
                    .title(query_title.as_str())
                    .border_style(query_border);

                // Choose cursor shape based on vim mode
                let cursor_shape = match self.mode {
                    Mode::Normal | Mode::Visual => CursorShape::Block,
                    Mode::Insert => CursorShape::Bar,
                };

                let is_editor_focused = matches!(self.focus, Focus::Query);
                let highlighted_editor =
                    HighlightedTextArea::new(&self.editor.textarea, highlighted_lines.clone())
                        .block(query_block.clone())
                        .scroll(self.editor_scroll)
                        .show_cursor(is_editor_focused)
                        .cursor_shape(cursor_shape);

                // Get cursor screen position before rendering (for Bar/Underline cursors)
                let cursor_pos = highlighted_editor.cursor_screen_position(query_area);

                frame.render_widget(highlighted_editor, query_area);

                // For Bar/Underline cursor shapes, use the terminal's native cursor
                if is_editor_focused && cursor_shape != CursorShape::Block {
                    if let Some(pos) = cursor_pos {
                        frame.set_cursor_position(pos);
                    }
                }

                // Update editor scroll based on cursor position
                // The inner area height is query_area.height - 2 (for borders)
                let inner_height = query_area.height.saturating_sub(2) as usize;
                let inner_width = query_area.width.saturating_sub(2) as usize;
                let (cursor_row, cursor_col) = self.editor.textarea.cursor();
                self.editor_scroll = calculate_editor_scroll(
                    cursor_row,
                    cursor_col,
                    self.editor_scroll,
                    inner_height,
                    inner_width,
                );

                // Render scrollbar for query editor if content exceeds visible area
                let total_lines = self.editor.textarea.lines().len();
                if total_lines > inner_height && inner_height > 0 {
                    let inner_area = query_block.inner(query_area);
                    let scrollbar_area = Rect {
                        x: inner_area.x + inner_area.width.saturating_sub(1),
                        y: inner_area.y,
                        width: 1,
                        height: inner_area.height,
                    };

                    let scrollbar = if scrollbar_area.height >= 7 {
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

                    let mut scrollbar_state =
                        ScrollbarState::new(total_lines).position(self.editor_scroll.0 as usize);

                    frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
                }

                // Error display (if any).
                if let Some(ref err) = self.last_error {
                    let error_block = Block::default()
                        .borders(Borders::ALL)
                        .title("Error (Enter to dismiss)")
                        .border_style(Style::default().fg(Color::Red));

                    let error_text = Paragraph::new(err.as_str())
                        .block(error_block)
                        .style(Style::default().fg(Color::Red))
                        .wrap(ratatui::widgets::Wrap { trim: false });

                    frame.render_widget(error_text, error_area);
                }

                // Calculate grid viewport dimensions for scroll handling
                // Inner area: grid_area minus borders (2 for borders)
                // Body area: inner minus header row (1)
                // Data width: inner width minus marker column (3)
                let inner_height = grid_area.height.saturating_sub(2);
                let body_height = inner_height.saturating_sub(1); // minus header
                let inner_width = grid_area.width.saturating_sub(2);
                let data_width = inner_width.saturating_sub(3); // minus marker column

                // Update grid state scroll position based on viewport
                self.grid_state.ensure_cursor_visible(
                    body_height as usize,
                    self.grid.rows.len(),
                    self.grid.headers.len(),
                    &self.grid.col_widths,
                    data_width,
                );

                // Store viewport dimensions for potential future use
                self.last_grid_viewport = Some((body_height as usize, data_width));

                // Results grid.
                let grid_widget = DataGrid {
                    model: &self.grid,
                    state: &self.grid_state,
                    focused: self.focus == Focus::Grid,
                    show_row_numbers: self.config.display.show_row_numbers,
                    show_scrollbar: true,
                };
                frame.render_widget(grid_widget, grid_area);

                // Loading overlay when query is running (only if grid area is large enough)
                if self.db.running && grid_area.width >= 20 && grid_area.height >= 5 {
                    // Calculate centered overlay area (40% width, minimum 20 chars, 5 lines height)
                    let overlay_width = (grid_area.width * 40 / 100).max(20).min(grid_area.width);
                    let overlay_height = 5u16.min(grid_area.height);
                    let overlay_x =
                        grid_area.x + (grid_area.width.saturating_sub(overlay_width)) / 2;
                    let overlay_y =
                        grid_area.y + (grid_area.height.saturating_sub(overlay_height)) / 2;
                    let overlay_area = Rect {
                        x: overlay_x,
                        y: overlay_y,
                        width: overlay_width,
                        height: overlay_height,
                    };

                    // Clear the overlay area
                    frame.render_widget(Clear, overlay_area);

                    // Create bordered block
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan))
                        .style(Style::default().bg(Color::Black));

                    let inner = block.inner(overlay_area);
                    frame.render_widget(block, overlay_area);

                    // Layout for spinner and elapsed time
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1), // Spinner with label
                            Constraint::Length(1), // Elapsed time
                        ])
                        .split(inner);

                    // Render spinner with label
                    let throbber = Throbber::default()
                        .label(" Executing...")
                        .style(Style::default().fg(Color::White))
                        .throbber_style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )
                        .throbber_set(BRAILLE_SIX);

                    frame.render_stateful_widget(throbber, chunks[0], &mut self.throbber_state);

                    // Render elapsed time
                    if let Some(start_time) = self.query_start_time {
                        let elapsed = start_time.elapsed();
                        let elapsed_text = format!("{:.1}s elapsed", elapsed.as_secs_f64());
                        let elapsed_widget = Paragraph::new(elapsed_text)
                            .style(Style::default().fg(Color::DarkGray))
                            .alignment(Alignment::Center);
                        frame.render_widget(elapsed_widget, chunks[1]);
                    }
                }

                // Status.
                frame.render_widget(self.status_line(status_area.width), status_area);

                if let Some(ref mut help) = self.help_popup {
                    help.render(frame, size);
                }

                // Render history picker if open
                if let Some(ref mut picker) = self.history_picker {
                    picker.render(frame, size);
                }

                // Render connection picker if open
                if let Some(ref mut picker) = self.connection_picker {
                    picker.render(frame, size);
                }

                if self.search.active {
                    // Render the search prompt as a bottom overlay.
                    let h = 3u16.min(size.height);
                    let y = size.height.saturating_sub(h);
                    let area = Rect {
                        x: 0,
                        y,
                        width: size.width,
                        height: h,
                    };

                    let search_title = match self.search_target {
                        SearchTarget::Editor => "/ Search Query (Enter apply, Esc cancel)",
                        SearchTarget::Grid => "/ Search Grid (Enter apply, Esc cancel)",
                    };

                    self.search.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(search_title)
                            .border_style(Style::default().fg(Color::Yellow)),
                    );

                    frame.render_widget(Clear, area);
                    frame.render_widget(&self.search.textarea, area);
                }

                if self.command.active {
                    // Render the command prompt as a bottom overlay.
                    let h = 3u16.min(size.height);
                    let y = size.height.saturating_sub(h);
                    let area = Rect {
                        x: 0,
                        y,
                        width: size.width,
                        height: h,
                    };

                    self.command.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(": Command (Enter run, Esc cancel)")
                            .border_style(Style::default().fg(Color::Magenta)),
                    );

                    frame.render_widget(Clear, area);
                    frame.render_widget(&self.command.textarea, area);
                }

                // Render completion popup if active
                if self.completion.active {
                    let max_visible = 8usize;
                    let visible = self.completion.visible_items(max_visible);
                    let total_items = self.completion.filtered_count();
                    let needs_scrollbar = total_items > max_visible;

                    if !visible.is_empty() {
                        // Position popup near the cursor
                        let (cursor_row, cursor_col) = self.editor.textarea.cursor();
                        // Estimate position (query_area starts at y=0, each line is 1 row)
                        let popup_y = query_area.y + 1 + cursor_row as u16;
                        let popup_x = query_area.x
                            + 1
                            + cursor_col.saturating_sub(self.completion.prefix.len()) as u16;

                        let popup_height = (visible.len() + 2) as u16; // +2 for borders
                        let base_width = 40u16;
                        let popup_width = (base_width + if needs_scrollbar { 1 } else { 0 })
                            .min(size.width.saturating_sub(popup_x));

                        // Make sure popup fits on screen
                        let popup_y = if popup_y + popup_height > size.height {
                            size.height.saturating_sub(popup_height)
                        } else {
                            popup_y
                        };

                        let popup_area = Rect {
                            x: popup_x.min(size.width.saturating_sub(popup_width)),
                            y: popup_y,
                            width: popup_width,
                            height: popup_height,
                        };

                        // Build completion list
                        let lines: Vec<Line> = visible
                            .iter()
                            .map(|(idx, item)| {
                                let is_selected = *idx == self.completion.selected;
                                let prefix = match item.kind {
                                    CompletionKind::Keyword => "K",
                                    CompletionKind::Table => "T",
                                    CompletionKind::Column => "C",
                                    CompletionKind::Schema => "S",
                                    CompletionKind::Function => "F",
                                };
                                let style = if is_selected {
                                    Style::default().bg(Color::Blue).fg(Color::White)
                                } else {
                                    Style::default()
                                };
                                Line::from(vec![
                                    Span::styled(
                                        format!("{} ", prefix),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                    Span::styled(&item.label, style),
                                ])
                            })
                            .collect();

                        let completion_block = Block::default()
                            .borders(Borders::ALL)
                            .title("Completions (Tab select, Esc cancel)")
                            .border_style(Style::default().fg(Color::Cyan));

                        let completion_list = Paragraph::new(lines).block(completion_block.clone());

                        frame.render_widget(Clear, popup_area);
                        frame.render_widget(completion_list, popup_area);

                        // Render scrollbar if needed
                        if needs_scrollbar {
                            let inner = completion_block.inner(popup_area);
                            let scrollbar_area = Rect {
                                x: inner.x + inner.width.saturating_sub(1),
                                y: inner.y,
                                width: 1,
                                height: inner.height,
                            };

                            let scrollbar = if scrollbar_area.height >= 7 {
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

                            let scroll_offset = self.completion.scroll_offset(max_visible);
                            let mut scrollbar_state =
                                ScrollbarState::new(total_items).position(scroll_offset);

                            frame.render_stateful_widget(
                                scrollbar,
                                scrollbar_area,
                                &mut scrollbar_state,
                            );
                        }
                    }
                }

                // Render cell editor popup if active
                if self.cell_editor.active {
                    let col_name = self
                        .grid
                        .headers
                        .get(self.cell_editor.col)
                        .cloned()
                        .unwrap_or_else(|| "?".to_string());

                    // Calculate popup size - make it wider for large content
                    let value_len = self.cell_editor.value.chars().count();
                    let min_width = 50u16;
                    let max_width = size.width.saturating_sub(4);
                    // Use 80% of screen width for large values, but at least min_width
                    let desired_width = if value_len > 45 {
                        (size.width as f32 * 0.8) as u16
                    } else {
                        min_width
                    };
                    let popup_width = desired_width.clamp(min_width, max_width);
                    let popup_height = 5u16;
                    let popup_x = (size.width.saturating_sub(popup_width)) / 2;
                    let popup_y = grid_area.y + 2; // Near top of grid

                    let popup_area = Rect {
                        x: popup_x,
                        y: popup_y,
                        width: popup_width,
                        height: popup_height,
                    };

                    // Calculate inner width for text display (minus borders)
                    let inner_width = popup_width.saturating_sub(2) as usize;

                    // Update scroll offset based on cursor position
                    self.cell_editor.update_scroll(inner_width);

                    let modified_indicator = if self.cell_editor.is_modified() {
                        " [+]"
                    } else {
                        ""
                    };
                    let title = format!(
                        "Edit: {}{} (Enter confirm, Esc cancel)",
                        col_name, modified_indicator
                    );
                    let edit_block = Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .border_style(Style::default().fg(Color::Yellow));

                    // Get visible text with cursor position
                    let (visible_text, cursor_pos) = self.cell_editor.visible_text(inner_width);

                    // Build display with cursor
                    let mut display_spans = Vec::new();
                    let chars: Vec<char> = visible_text.chars().collect();

                    if cursor_pos < chars.len() {
                        // Cursor is within text
                        let before: String = chars[..cursor_pos].iter().collect();
                        let cursor_char = chars[cursor_pos];
                        let after: String = chars[cursor_pos + 1..].iter().collect();

                        display_spans.push(Span::raw(before));
                        display_spans.push(Span::styled(
                            cursor_char.to_string(),
                            Style::default().bg(Color::White).fg(Color::Black),
                        ));
                        display_spans.push(Span::raw(after));
                    } else {
                        // Cursor is at end
                        display_spans.push(Span::raw(visible_text));
                        display_spans.push(Span::styled(
                            " ",
                            Style::default().bg(Color::White).fg(Color::Black),
                        ));
                    }

                    // Show scroll indicators if needed
                    let total_chars = self.cell_editor.value.chars().count();
                    let scroll_indicator = if self.cell_editor.scroll_offset > 0
                        || total_chars > inner_width
                    {
                        let at_start = self.cell_editor.scroll_offset == 0;
                        let at_end = self.cell_editor.scroll_offset + inner_width >= total_chars;
                        match (at_start, at_end) {
                            (true, false) => " →",
                            (false, true) => "← ",
                            (false, false) => "←→",
                            (true, true) => "",
                        }
                    } else {
                        ""
                    };

                    let edit_content = Paragraph::new(Line::from(display_spans))
                        .block(edit_block)
                        .style(Style::default().fg(Color::White));

                    frame.render_widget(Clear, popup_area);
                    frame.render_widget(edit_content, popup_area);

                    // Show scroll indicator and length info in a second line if there's room
                    if popup_height > 4 && (!scroll_indicator.is_empty() || value_len > 20) {
                        let info = format!(
                            "{} len: {} pos: {}",
                            scroll_indicator,
                            value_len,
                            self.cell_editor.value[..self.cell_editor.cursor]
                                .chars()
                                .count()
                        );
                        let info_area = Rect {
                            x: popup_area.x + 1,
                            y: popup_area.y + 3,
                            width: popup_area.width.saturating_sub(2),
                            height: 1,
                        };
                        let info_widget =
                            Paragraph::new(info).style(Style::default().fg(Color::DarkGray));
                        frame.render_widget(info_widget, info_area);
                    }
                }

                // Render JSON editor modal if active
                if let Some(ref mut json_editor) = self.json_editor {
                    json_editor.render(frame, size);
                }

                // Render row detail modal if active
                if let Some(ref mut row_detail) = self.row_detail {
                    row_detail.render(frame, size);
                }

                // Render connection manager modal if active
                if let Some(ref mut manager) = self.connection_manager {
                    manager.render(frame, size);
                }

                // Render connection form modal if active (on top of manager)
                if let Some(ref form) = self.connection_form {
                    form.render(frame, size);
                }

                // Render key hint popup if active (shows after timeout when 'g' is pending)
                if show_key_hint {
                    if let Some(pending_key) = pending_key_for_hint {
                        let hint_popup = KeyHintPopup::new(pending_key);
                        hint_popup.render(frame, size);
                    }
                }

                // Render confirmation prompt if active (topmost layer)
                if let Some(ref mut prompt) = self.confirm_prompt {
                    prompt.render(frame, size);
                }
            })?;

            // Mark hint as shown after rendering (must be outside draw closure)
            if show_key_hint && !self.key_sequence.is_hint_shown() {
                self.key_sequence.mark_hint_shown();
            }

            // Use faster polling when query is running to keep UI responsive
            let poll_duration = if self.db.running {
                Duration::from_millis(16) // ~60 FPS when query running
            } else {
                Duration::from_millis(50) // Normal 20 FPS when idle
            };

            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if self.on_key(key) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        if self.on_mouse(mouse) {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Save session state before exiting (if enabled)
        if self.config.editor.persist_session {
            if let Err(e) = self.save_session() {
                eprintln!("Warning: Failed to save session: {}", e);
            }
        }

        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        // Handle confirmation prompt when active (highest priority)
        if let Some(mut prompt) = self.confirm_prompt.take() {
            match prompt.handle_key(key) {
                ConfirmResult::Confirmed => {
                    return self.handle_confirm_confirmed(prompt.context().clone());
                }
                ConfirmResult::Cancelled => {
                    self.handle_confirm_cancelled(prompt.context().clone());
                    return false;
                }
                ConfirmResult::Pending => {
                    // Put it back, wait for valid input
                    self.confirm_prompt = Some(prompt);
                    return false;
                }
            }
        }

        // Handle row detail modal when active - it captures all input
        if self.row_detail.is_some() {
            return self.handle_row_detail_key(key);
        }

        // Handle JSON editor when active - it captures all input
        if self.json_editor.is_some() {
            return self.handle_json_editor_key(key);
        }

        // Handle connection form when active - it captures all input
        if self.connection_form.is_some() {
            let action = self.connection_form.as_mut().unwrap().handle_key(key);
            self.handle_connection_form_action(action);
            return false;
        }

        // Handle connection manager when active - it captures all input
        if self.connection_manager.is_some() {
            let action = self.connection_manager.as_mut().unwrap().handle_key(key);
            self.handle_connection_manager_action(action);
            return false;
        }

        // Ctrl-c: cancel running query.
        if key.code == KeyCode::Char('c')
            && key.modifiers == KeyModifiers::CONTROL
            && self.db.running
        {
            self.cancel_query();
            return false;
        }

        // Esc: cancel running query, close popups, or quit if nothing is open.
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            if self.db.running {
                self.cancel_query();
                return false;
            }

            // Check if anything is open that needs to be closed
            let has_open_ui = self.help_popup.is_some()
                || self.search.active
                || self.command.active
                || self.completion.active
                || self.cell_editor.active
                || self.history_picker.is_some()
                || self.connection_picker.is_some()
                || self.pending_key.is_some()
                || self.last_error.is_some()
                || self.key_sequence.is_waiting()
                || self.mode != Mode::Normal;

            if has_open_ui {
                // Cancel any pending multi-key sequence (e.g., started with 'g')
                self.key_sequence.cancel();

                self.help_popup = None;
                self.search.close();
                self.command.close();
                self.completion.close();
                self.cell_editor.close();
                self.history_picker = None;
                self.connection_picker = None;
                self.pending_key = None;
                self.last_error = None;
                self.mode = Mode::Normal;
            } else {
                // Nothing open - behave like 'q' and show quit confirmation
                if self.editor.is_modified() {
                    self.confirm_prompt = Some(ConfirmPrompt::new(
                        "You have unsaved changes. Quit anyway?",
                        ConfirmContext::QuitApp,
                    ));
                } else {
                    self.confirm_prompt = Some(ConfirmPrompt::new(
                        "Are you sure you want to quit?",
                        ConfirmContext::QuitAppClean,
                    ));
                }
            }
            return false;
        }

        // Handle history picker when open (takes priority over error dismissal)
        if self.history_picker.is_some() {
            return self.handle_history_picker_key(key);
        }

        // Handle connection picker when open (takes priority over error dismissal)
        if self.connection_picker.is_some() {
            // Clear any error when interacting with the picker
            self.last_error = None;
            return self.handle_connection_picker_key(key);
        }

        // If error is showing, Enter dismisses it.
        if self.last_error.is_some() {
            if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::NONE {
                self.last_error = None;
                return false;
            }
            // Absorb other keys while error is showing, except Esc which we handled above.
            return false;
        }

        // Global Ctrl+E to execute query (works regardless of mode/focus)
        if key.code == KeyCode::Char('e') && key.modifiers == KeyModifiers::CONTROL {
            self.execute_query();
            return false;
        }

        if self.search.active {
            self.handle_search_key(key);
            return false;
        }

        if self.command.active {
            return self.handle_command_key(key);
        }

        // Handle cell editor when active
        if self.cell_editor.active {
            return self.handle_cell_edit_key(key);
        }

        // Handle completion popup when active
        if self.completion.active {
            match (key.code, key.modifiers) {
                // Enter accepts the completion
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    self.apply_completion();
                    return false;
                }
                // Tab cycles to next item (wraps around)
                (KeyCode::Tab, KeyModifiers::NONE)
                | (KeyCode::Down, KeyModifiers::NONE)
                | (KeyCode::Char('n'), KeyModifiers::CONTROL)
                | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                    self.completion.select_next();
                    return false;
                }
                // Shift+Tab cycles to previous item (wraps around)
                (KeyCode::Tab, KeyModifiers::SHIFT)
                | (KeyCode::Up, KeyModifiers::NONE)
                | (KeyCode::Char('p'), KeyModifiers::CONTROL)
                | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                    self.completion.select_prev();
                    return false;
                }
                // Escape closes completion without accepting
                (KeyCode::Esc, KeyModifiers::NONE) => {
                    self.completion.close();
                    return false;
                }
                (KeyCode::Char(c), KeyModifiers::NONE) if c.is_alphanumeric() || c == '_' => {
                    // Continue typing - update the completion filter
                    self.editor.textarea.insert_char(c);
                    let (row, col) = self.editor.textarea.cursor();
                    let lines = self.editor.textarea.lines();
                    if row < lines.len() {
                        let line = &lines[row];
                        let (prefix, _) = get_word_before_cursor(line, col);
                        self.completion.update_prefix(prefix);
                    }
                    return false;
                }
                (KeyCode::Backspace, KeyModifiers::NONE) => {
                    self.editor.textarea.delete_char();
                    let (row, col) = self.editor.textarea.cursor();
                    let lines = self.editor.textarea.lines();
                    if row < lines.len() {
                        let line = &lines[row];
                        let (prefix, _) = get_word_before_cursor(line, col);
                        if prefix.is_empty() {
                            self.completion.close();
                        } else {
                            self.completion.update_prefix(prefix);
                        }
                    }
                    return false;
                }
                _ => {
                    // Any other key closes completion
                    self.completion.close();
                }
            }
        }

        // Handle second key of any pending key sequence (e.g., g* or schema-table Enter+key).
        if self.key_sequence.is_waiting() {
            // Prevent legacy operator-pending state from leaking across key sequences.
            self.pending_key = None;
            if let KeyCode::Char(c) = key.code {
                if key.modifiers == KeyModifiers::NONE {
                    let result = self.key_sequence.process_second_key(c);
                    match result {
                        KeySequenceResult::Completed(completed) => {
                            self.execute_key_sequence_completion(completed);
                            return false;
                        }
                        KeySequenceResult::Cancelled => {
                            // Invalid second key - show feedback and let it fall through.
                            self.last_status = Some("Invalid key sequence".to_string());
                        }
                        _ => {}
                    }
                } else {
                    // Modifier key pressed - cancel sequence
                    self.key_sequence.cancel();
                }
            } else {
                // Non-char key pressed (arrows, etc.) - cancel sequence
                // Note: Esc is handled earlier in on_key() at the global Esc handler
                self.key_sequence.cancel();
            }
        }

        // Handle key sequences (e.g., gg, gc, gt, ge, gr) in Normal mode
        if self.mode == Mode::Normal {
            // Start a new key sequence for 'g' key (only when no sequence is pending)
            if !self.key_sequence.is_waiting() {
                if let KeyCode::Char('g') = key.code {
                    if key.modifiers == KeyModifiers::NONE {
                        // Starting a global `g*` sequence should cancel any editor operator-pending state.
                        self.pending_key = None;
                        let result = self.key_sequence.process_first_key('g');
                        if matches!(result, KeySequenceResult::Started(_)) {
                            return false;
                        }
                    }
                }
            }
        }

        // Global keys are only active in Normal mode.
        if self.mode == Mode::Normal {
            match (key.code, key.modifiers) {
                // Ctrl+Shift+C: Open connection manager
                (KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                | (KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                    self.open_connection_manager();
                    return false;
                }
                // Ctrl+O: Open connection picker
                (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                    self.open_connection_picker();
                    return false;
                }
                (KeyCode::Char('q'), KeyModifiers::NONE) => {
                    // Always show confirmation prompt, with different message based on unsaved changes
                    if self.editor.is_modified() {
                        self.confirm_prompt = Some(ConfirmPrompt::new(
                            "You have unsaved changes. Quit anyway?",
                            ConfirmContext::QuitApp,
                        ));
                    } else {
                        self.confirm_prompt = Some(ConfirmPrompt::new(
                            "Are you sure you want to quit?",
                            ConfirmContext::QuitAppClean,
                        ));
                    }
                    return false;
                }
                (KeyCode::Char('?'), KeyModifiers::NONE) => {
                    // Toggle help popup
                    if self.help_popup.is_some() {
                        self.help_popup = None;
                    } else {
                        self.help_popup = Some(HelpPopup::new());
                    }
                    return false;
                }
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.focus = match self.focus {
                        Focus::Query => Focus::Grid,
                        Focus::Grid => {
                            if self.sidebar_visible {
                                self.sidebar_focus = SidebarSection::Connections;
                                Focus::Sidebar(SidebarSection::Connections)
                            } else {
                                Focus::Query
                            }
                        }
                        Focus::Sidebar(SidebarSection::Connections) => {
                            self.sidebar_focus = SidebarSection::Schema;
                            self.sidebar.select_first_schema_if_empty();
                            Focus::Sidebar(SidebarSection::Schema)
                        }
                        Focus::Sidebar(SidebarSection::Schema) => Focus::Query,
                    };
                    return false;
                }
                (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                    self.focus = match self.focus {
                        Focus::Query => {
                            if self.sidebar_visible {
                                self.sidebar_focus = SidebarSection::Schema;
                                self.sidebar.select_first_schema_if_empty();
                                Focus::Sidebar(SidebarSection::Schema)
                            } else {
                                Focus::Grid
                            }
                        }
                        Focus::Grid => Focus::Query,
                        Focus::Sidebar(SidebarSection::Schema) => {
                            self.sidebar_focus = SidebarSection::Connections;
                            Focus::Sidebar(SidebarSection::Connections)
                        }
                        Focus::Sidebar(SidebarSection::Connections) => Focus::Grid,
                    };
                    return false;
                }
                // Ctrl+B: Toggle sidebar
                (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                    self.sidebar_visible = !self.sidebar_visible;
                    // If hiding sidebar and focus was on it, move focus to query
                    if !self.sidebar_visible && matches!(self.focus, Focus::Sidebar(_)) {
                        self.focus = Focus::Query;
                    }
                    return false;
                }
                // Ctrl+HJKL: Directional panel navigation
                (KeyCode::Char('h' | 'j' | 'k' | 'l'), KeyModifiers::CONTROL) => {
                    if self.handle_panel_navigation(&key) {
                        return false;
                    }
                }
                _ => {}
            }
        }

        // Handle help popup key events
        if let Some(ref mut help) = self.help_popup {
            match help.handle_key(key) {
                HelpAction::Close => {
                    self.help_popup = None;
                }
                HelpAction::Continue => {}
            }
            return false;
        }

        match self.focus {
            Focus::Grid => {
                if self.mode == Mode::Normal {
                    // First, try to look up action in keymap
                    let result = if let Some(action) = self.grid_keymap.get_action(&key) {
                        // Handle special actions at the App level
                        match action {
                            Action::ToggleFocus => {
                                self.focus = Focus::Query;
                                GridKeyResult::None
                            }
                            Action::FocusQuery => {
                                self.focus = Focus::Query;
                                self.mode = Mode::Insert;
                                GridKeyResult::None
                            }
                            Action::Quit => {
                                return true;
                            }
                            Action::Help => {
                                self.help_popup = Some(HelpPopup::new());
                                GridKeyResult::None
                            }
                            Action::ToggleSidebar => {
                                self.sidebar_visible = !self.sidebar_visible;
                                // Note: No need to change focus here - we're already in Focus::Grid
                                GridKeyResult::None
                            }
                            // Goto navigation (custom keybindings for navigation)
                            Action::GotoFirst => {
                                // In grid context, go to first row
                                self.grid_state.cursor_row = 0;
                                GridKeyResult::None
                            }
                            Action::GotoEditor => {
                                self.focus = Focus::Query;
                                GridKeyResult::None
                            }
                            Action::GotoConnections => {
                                self.sidebar_visible = true;
                                self.sidebar_focus = SidebarSection::Connections;
                                self.focus = Focus::Sidebar(SidebarSection::Connections);
                                GridKeyResult::None
                            }
                            Action::GotoTables => {
                                self.focus_schema();
                                GridKeyResult::None
                            }
                            Action::GotoResults => {
                                // Already in grid, this is a no-op but keep focus
                                self.focus = Focus::Grid;
                                GridKeyResult::None
                            }
                            _ => {
                                // Delegate to grid state
                                self.grid_state.handle_action(action, &self.grid)
                            }
                        }
                    } else {
                        // Fall back to legacy key handling for unmapped keys
                        self.grid_state.handle_key(key, &self.grid)
                    };

                    match result {
                        GridKeyResult::OpenSearch => {
                            self.search_target = SearchTarget::Grid;
                            self.search.open();
                        }
                        GridKeyResult::OpenCommand => {
                            self.command.open();
                        }
                        GridKeyResult::CopyToClipboard(text) => {
                            self.copy_to_clipboard(&text);
                        }
                        GridKeyResult::ResizeColumn { col, action } => match action {
                            ResizeAction::Widen => self.grid.widen_column(col, 2),
                            ResizeAction::Narrow => self.grid.narrow_column(col, 2),
                            ResizeAction::AutoFit => self.grid.autofit_column(col),
                        },
                        GridKeyResult::EditCell { row, col } => {
                            self.start_cell_edit(row, col);
                        }
                        GridKeyResult::OpenRowDetail { row } => {
                            self.open_row_detail(row);
                        }
                        GridKeyResult::StatusMessage(msg) => {
                            self.last_status = Some(msg);
                        }
                        GridKeyResult::GotoFirstRow => {
                            // This shouldn't happen anymore since we handle 'g' at the app level,
                            // but handle it for completeness
                            self.grid_state.cursor_row = 0;
                        }
                        GridKeyResult::None => {}
                    }
                }
            }
            Focus::Query => {
                self.handle_editor_key(key);
            }
            Focus::Sidebar(section) => {
                self.handle_sidebar_key(key, section);
            }
        }

        false
    }

    /// Handle directional panel navigation (Ctrl+HJKL).
    /// Returns true if a navigation key was handled (even if no-op).
    fn handle_panel_navigation(&mut self, key: &KeyEvent) -> bool {
        // Only handle Ctrl+HJKL
        if key.modifiers != KeyModifiers::CONTROL {
            return false;
        }

        let direction = match key.code {
            KeyCode::Char('h') => PanelDirection::Left,
            KeyCode::Char('j') => PanelDirection::Down,
            KeyCode::Char('k') => PanelDirection::Up,
            KeyCode::Char('l') => PanelDirection::Right,
            _ => return false,
        };

        // Calculate new focus based on direction and current state
        let new_focus = self.calculate_focus_for_direction(direction);

        if let Some(focus) = new_focus {
            self.focus = focus;
            // Update sidebar focus if moving to sidebar
            if let Focus::Sidebar(section) = focus {
                self.sidebar_focus = section;
            }
        }

        true // Key was handled (even if no-op)
    }

    /// Calculate the target focus for a given direction.
    /// Returns None if there is no panel in that direction (boundary/no-op).
    fn calculate_focus_for_direction(&self, direction: PanelDirection) -> Option<Focus> {
        // If sidebar hidden, Ctrl+H/L do nothing
        if !self.sidebar_visible
            && matches!(direction, PanelDirection::Left | PanelDirection::Right)
        {
            return None;
        }

        // Navigation is spatially precise based on vertical alignment:
        // ┌─────────────────┬──────────────────┐
        // │  Connections    │  Query Editor    │  ← Top row
        // ├─────────────────┼──────────────────┤
        // │  Schema         │  Results Grid    │  ← Bottom row
        // └─────────────────┴──────────────────┘

        match (&self.focus, direction) {
            // From Query (top-right) - aligned with Connections
            (Focus::Query, PanelDirection::Left) => {
                Some(Focus::Sidebar(SidebarSection::Connections))
            }
            (Focus::Query, PanelDirection::Down) => Some(Focus::Grid),

            // From Grid (bottom-right) - aligned with Schema
            (Focus::Grid, PanelDirection::Left) => Some(Focus::Sidebar(SidebarSection::Schema)),
            (Focus::Grid, PanelDirection::Up) => Some(Focus::Query),

            // From Sidebar(Connections) (top-left) - aligned with Query
            (Focus::Sidebar(SidebarSection::Connections), PanelDirection::Down) => {
                Some(Focus::Sidebar(SidebarSection::Schema))
            }
            (Focus::Sidebar(SidebarSection::Connections), PanelDirection::Right) => {
                Some(Focus::Query)
            }

            // From Sidebar(Schema) (bottom-left) - aligned with Grid
            (Focus::Sidebar(SidebarSection::Schema), PanelDirection::Up) => {
                Some(Focus::Sidebar(SidebarSection::Connections))
            }
            (Focus::Sidebar(SidebarSection::Schema), PanelDirection::Right) => Some(Focus::Grid),

            // All other combinations are no-ops (at boundary)
            _ => None,
        }
    }

    /// Handle key events when sidebar is focused
    fn handle_sidebar_key(&mut self, key: KeyEvent, section: SidebarSection) {
        match (key.code, key.modifiers, section) {
            // Navigation within connections list
            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE, SidebarSection::Connections) => {
                self.sidebar.connections_up(self.connections.sorted().len());
            }
            (
                KeyCode::Down | KeyCode::Char('j'),
                KeyModifiers::NONE,
                SidebarSection::Connections,
            ) => {
                // Check if at bottom of connections list - if so, move to schema section
                let count = self.connections.sorted().len();
                let at_bottom =
                    self.sidebar.connections_state.selected() == Some(count.saturating_sub(1));
                if at_bottom && count > 0 {
                    self.focus_schema();
                } else {
                    self.sidebar.connections_down(count);
                }
            }
            // Enter on connection: switch to that connection
            (KeyCode::Enter, KeyModifiers::NONE, SidebarSection::Connections) => {
                if let Some(entry) = self.sidebar.get_selected_connection(&self.connections) {
                    self.connect_to_entry(entry.clone());
                }
            }
            // 'a' or 'e' to open connection manager
            (
                KeyCode::Char('a') | KeyCode::Char('e'),
                KeyModifiers::NONE,
                SidebarSection::Connections,
            ) => {
                self.open_connection_manager();
            }

            // Navigation within schema tree
            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE, SidebarSection::Schema) => {
                // Check if at top of schema tree - if so, move to connections section
                let at_top = self.sidebar.schema_state.selected().is_empty();
                if at_top {
                    self.sidebar_focus = SidebarSection::Connections;
                    self.focus = Focus::Sidebar(SidebarSection::Connections);
                } else {
                    self.sidebar.schema_up();
                }
            }
            (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE, SidebarSection::Schema) => {
                self.sidebar.schema_down();
            }
            (KeyCode::Right | KeyCode::Char('l'), KeyModifiers::NONE, SidebarSection::Schema) => {
                self.sidebar.schema_right();
            }
            (KeyCode::Left | KeyCode::Char('h'), KeyModifiers::NONE, SidebarSection::Schema) => {
                self.sidebar.schema_left();
            }
            // Enter on schema item: insert name at cursor or toggle expand
            (KeyCode::Enter, KeyModifiers::NONE, SidebarSection::Schema) => {
                let Some(id) = self.sidebar.schema_state.selected().last().cloned() else {
                    self.sidebar.schema_toggle();
                    return;
                };

                match parse_schema_tree_identifier(&id) {
                    SchemaTreeSelection::Schema { .. } => {
                        // Schema node: toggle expand/collapse
                        self.sidebar.schema_toggle();
                    }
                    SchemaTreeSelection::Table { schema, table } => {
                        // Table node: start a follow-up key sequence (Enter + key)
                        self.key_sequence.start_with_context(
                            PendingKey::SchemaTable,
                            SchemaTableContext { schema, table },
                        );
                    }
                    SchemaTreeSelection::Column { column, .. } => {
                        // Column node: preserve existing behavior (insert column name)
                        self.editor.textarea.insert_str(&column);
                        self.focus = Focus::Query;
                        self.mode = Mode::Insert;
                    }
                    SchemaTreeSelection::Unknown { raw } => {
                        // Fallback to previous behavior: insert last segment after ':'
                        let insert_name = raw
                            .rsplit_once(':')
                            .map(|(_, name)| name.to_string())
                            .unwrap_or(raw);
                        self.editor.textarea.insert_str(&insert_name);
                        self.focus = Focus::Query;
                        self.mode = Mode::Insert;
                    }
                }
            }
            // Space toggles tree node
            (KeyCode::Char(' '), KeyModifiers::NONE, SidebarSection::Schema) => {
                self.sidebar.schema_toggle();
            }
            // 'r' to refresh schema
            (KeyCode::Char('r'), KeyModifiers::NONE, SidebarSection::Schema) => {
                self.load_schema();
            }

            // Tab or Escape to leave sidebar
            (KeyCode::Tab, KeyModifiers::NONE, _) | (KeyCode::Esc, KeyModifiers::NONE, _) => {
                self.focus = Focus::Query;
            }

            _ => {}
        }
    }

    /// Handle mouse events. Returns true if the app should quit.
    fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        // Route mouse events to modals in priority order

        // Confirmation prompt has highest priority (topmost modal)
        if let Some(mut prompt) = self.confirm_prompt.take() {
            match prompt.handle_mouse(mouse) {
                ConfirmResult::Confirmed => {
                    return self.handle_confirm_confirmed(prompt.context().clone());
                }
                ConfirmResult::Cancelled => {
                    self.handle_confirm_cancelled(prompt.context().clone());
                    return false;
                }
                ConfirmResult::Pending => {
                    // Put it back, wait for valid input
                    self.confirm_prompt = Some(prompt);
                    return false;
                }
            }
        }

        // Help popup has mouse support
        if let Some(ref mut help_popup) = self.help_popup {
            let action = help_popup.handle_mouse(mouse);
            match action {
                HelpAction::Close => {
                    self.help_popup = None;
                }
                HelpAction::Continue => {}
            }
            return false;
        }

        // History picker has mouse support
        if let Some(ref mut picker) = self.history_picker {
            let action = picker.handle_mouse(mouse);
            match action {
                PickerAction::Selected(entry) => {
                    // Load selected query into editor (mirror keyboard path)
                    self.editor.set_text(entry.query);
                    self.editor.mark_saved(); // Mark as unmodified since it's loaded content
                    self.history_picker = None;
                    self.last_status = Some("Loaded from history".to_string());
                }
                PickerAction::Cancelled => {
                    self.history_picker = None;
                }
                PickerAction::Continue => {}
            }
            return false;
        }

        // Connection picker has mouse support
        if let Some(ref mut picker) = self.connection_picker {
            let action = picker.handle_mouse(mouse);
            match action {
                PickerAction::Selected(entry) => {
                    self.connection_picker = None;
                    self.last_error = None;
                    if self.editor.is_modified() {
                        self.confirm_prompt = Some(ConfirmPrompt::new(
                            "You have unsaved changes. Switch connection anyway?",
                            ConfirmContext::SwitchConnection { entry },
                        ));
                    } else {
                        self.connect_to_entry(entry);
                    }
                }
                PickerAction::Cancelled => {
                    self.connection_picker = None;
                    self.last_error = None;
                }
                PickerAction::Continue => {}
            }
            return false;
        }

        // Connection manager has mouse support (but not if connection_form is open on top)
        if self.connection_form.is_none() {
            if let Some(ref mut manager) = self.connection_manager {
                let action = manager.handle_mouse(mouse);
                // Handle action (the method already exists)
                self.handle_connection_manager_action(action);
                return false;
            }
        }

        // Don't process mouse events for other modals without mouse support
        if self.json_editor.is_some() || self.row_detail.is_some() || self.connection_form.is_some()
        {
            return false;
        }

        // Check if mouse is over sidebar first
        if self.sidebar_visible {
            if let Some(sidebar_area) = self.render_sidebar_area {
                if is_inside(mouse.column, mouse.row, sidebar_area) {
                    // Delegate to sidebar mouse handler
                    let (action, section) = self.sidebar.handle_mouse(mouse, &self.connections);

                    // Update focus to sidebar if a section was clicked
                    if let Some(section) = section {
                        self.focus = Focus::Sidebar(section);
                        self.sidebar_focus = section;
                        if section == SidebarSection::Schema {
                            self.sidebar.select_first_schema_if_empty();
                        }
                    }

                    // Handle any action from the sidebar
                    if let Some(action) = action {
                        self.handle_sidebar_action(action);
                    }
                    return false;
                }
            }
        }

        // Handle mouse for main UI (query editor / grid)
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_mouse_click(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollUp => {
                self.handle_mouse_scroll(-3);
            }
            MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(3);
            }
            _ => {}
        }
        false
    }

    /// Handle a mouse click at the given position
    fn handle_mouse_click(&mut self, x: u16, y: u16) {
        // Check if click is in query area
        if let Some(query_area) = self.render_query_area {
            if x >= query_area.x
                && x < query_area.x + query_area.width
                && y >= query_area.y
                && y < query_area.y + query_area.height
            {
                // Click in query editor - focus it
                if self.focus != Focus::Query {
                    self.focus = Focus::Query;
                    self.mode = Mode::Normal;
                }
                return;
            }
        }

        // Check if click is in grid area
        if let Some(grid_area) = self.render_grid_area {
            if x >= grid_area.x
                && x < grid_area.x + grid_area.width
                && y >= grid_area.y
                && y < grid_area.y + grid_area.height
            {
                // Ignore clicks on the top border/header row(s)
                if y <= grid_area.y + 1 {
                    // Still focus the grid, just don't select a row
                    if self.focus != Focus::Grid {
                        self.focus = Focus::Grid;
                        self.mode = Mode::Normal;
                    }
                    return;
                }

                // Click in grid - focus it and try to select the row
                if self.focus != Focus::Grid {
                    self.focus = Focus::Grid;
                    self.mode = Mode::Normal;
                }

                // Calculate which row was clicked (accounting for border and header)
                let inner_y = y.saturating_sub(grid_area.y + 2); // +2 for border and header
                let clicked_row = self.grid_state.row_offset + inner_y as usize;

                if clicked_row < self.grid.rows.len() {
                    self.grid_state.cursor_row = clicked_row;
                }
            }
        }
    }

    /// Handle mouse scroll in the focused area
    fn handle_mouse_scroll(&mut self, delta: i32) {
        match self.focus {
            Focus::Query => {
                // Scroll the query editor
                if delta < 0 {
                    // Scroll up
                    self.editor_scroll.0 = self.editor_scroll.0.saturating_sub((-delta) as u16);
                } else {
                    // Scroll down
                    let max_scroll = self.editor.textarea.lines().len().saturating_sub(1) as u16;
                    self.editor_scroll.0 = (self.editor_scroll.0 + delta as u16).min(max_scroll);
                }
            }
            Focus::Grid => {
                // Scroll the results grid
                let row_count = self.grid.rows.len();
                if row_count == 0 {
                    return;
                }

                if delta < 0 {
                    // Scroll up
                    let amount = (-delta) as usize;
                    self.grid_state.cursor_row = self.grid_state.cursor_row.saturating_sub(amount);
                } else {
                    // Scroll down
                    let amount = delta as usize;
                    self.grid_state.cursor_row =
                        (self.grid_state.cursor_row + amount).min(row_count - 1);
                }
            }
            Focus::Sidebar(section) => {
                // Scroll sidebar sections
                match section {
                    SidebarSection::Connections => {
                        if delta < 0 {
                            self.sidebar.connections_up(self.connections.sorted().len());
                        } else {
                            self.sidebar
                                .connections_down(self.connections.sorted().len());
                        }
                    }
                    SidebarSection::Schema => {
                        if delta < 0 {
                            self.sidebar.schema_up();
                        } else {
                            self.sidebar.schema_down();
                        }
                    }
                }
            }
        }
    }

    /// Focus on the Schema section of the sidebar, ensuring first item is selected
    fn focus_schema(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_focus = SidebarSection::Schema;
        self.sidebar.select_first_schema_if_empty();
        self.focus = Focus::Sidebar(SidebarSection::Schema);
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(text) {
                Ok(()) => {
                    let lines = text.lines().count();
                    let chars = text.len();
                    self.last_status = Some(format!(
                        "Copied {} line{}, {} char{}",
                        lines,
                        if lines == 1 { "" } else { "s" },
                        chars,
                        if chars == 1 { "" } else { "s" }
                    ));
                }
                Err(e) => {
                    self.last_error = Some(format!("Failed to copy: {}", e));
                }
            },
            Err(e) => {
                self.last_error = Some(format!("Clipboard unavailable: {}", e));
            }
        }
    }

    fn start_cell_edit(&mut self, row: usize, col: usize) {
        // Check if we have a valid PK for safe editing
        if !self.grid.has_valid_pk() {
            self.last_error = Some(
                "Cannot edit: no primary key detected. Run a simple SELECT from a table with a PK."
                    .to_string(),
            );
            return;
        }

        // Check if we have a source table
        if self.grid.source_table.is_none() {
            self.last_error =
                Some("Cannot edit: unknown source table. Run a simple SELECT query.".to_string());
            return;
        }

        // Get the current cell value
        let value = self
            .grid
            .cell(row, col)
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Get the column type and name
        let col_type = self.grid.col_type(col).unwrap_or("").to_string();
        let col_name = self.grid.headers.get(col).cloned().unwrap_or_default();

        // Determine if we should use the multiline JSON editor
        if should_use_multiline_editor(&value) || is_json_column_type(&col_type) {
            // Open JSON editor modal
            self.json_editor = Some(JsonEditorModal::new(value, col_name, col_type, row, col));
        } else {
            // Use inline editor for simple values
            self.cell_editor.open(row, col, value);
        }
    }

    /// Open the row detail modal to show all columns for a row.
    fn open_row_detail(&mut self, row: usize) {
        if row >= self.grid.rows.len() {
            return;
        }

        let headers = self.grid.headers.clone();
        let values = self.grid.rows[row].clone();
        let col_types = self.grid.col_types.clone();

        self.row_detail = Some(RowDetailModal::new(headers, values, col_types, row));
    }

    /// Handle key events for the row detail modal.
    fn handle_row_detail_key(&mut self, key: KeyEvent) -> bool {
        // Take the modal temporarily to avoid borrow issues
        let mut modal = match self.row_detail.take() {
            Some(m) => m,
            None => return false,
        };

        match modal.handle_key(key) {
            RowDetailAction::Continue => {
                // Put the modal back
                self.row_detail = Some(modal);
            }
            RowDetailAction::Close => {
                // Modal is already taken, just don't put it back
            }
            RowDetailAction::Edit { col } => {
                // Get the row from the modal before closing
                let row = self.grid_state.cursor_row;
                // Close the detail modal first
                // Then start editing the selected field
                self.start_cell_edit(row, col);
            }
        }
        false
    }

    /// Handle confirmed action based on context.
    fn handle_confirm_confirmed(&mut self, context: ConfirmContext) -> bool {
        match context {
            ConfirmContext::CloseJsonEditor { .. } => {
                self.json_editor = None;
                self.last_status = Some("Changes discarded".to_string());
                false
            }
            ConfirmContext::CloseCellEditor { .. } => {
                self.cell_editor.close();
                self.last_status = Some("Changes discarded".to_string());
                false
            }
            ConfirmContext::QuitApp | ConfirmContext::QuitAppClean => {
                true // Quit the application
            }
            ConfirmContext::DeleteConnection { name } => {
                // Delete the connection
                if let Err(e) = self.connections.remove(&name) {
                    self.last_error = Some(format!("Failed to delete: {}", e));
                } else {
                    // Also try to delete password from keychain
                    if let Some(entry) = self.connections.find_by_name(&name) {
                        let _ = entry.delete_password_from_keychain();
                    }
                    if let Err(e) = save_connections(&self.connections) {
                        self.last_error = Some(format!("Failed to save: {}", e));
                    } else {
                        self.last_status = Some(format!("Connection '{}' deleted", name));
                    }
                    // Update manager if open
                    if let Some(ref mut manager) = self.connection_manager {
                        manager.update_connections(&self.connections);
                    }
                }
                false
            }
            ConfirmContext::CloseConnectionForm => {
                // Close the connection form without saving
                self.connection_form = None;
                self.last_status = Some("Changes discarded".to_string());
                false
            }
            ConfirmContext::SwitchConnection { entry } => {
                // Proceed with connection switch despite unsaved changes
                self.connect_to_entry(entry);
                false
            }
        }
    }

    /// Handle cancelled confirmation based on context.
    fn handle_confirm_cancelled(&mut self, context: ConfirmContext) {
        match context {
            ConfirmContext::CloseJsonEditor { .. } => {
                // Editor is still open, nothing to do
                self.last_status = Some("Continuing edit".to_string());
            }
            ConfirmContext::CloseCellEditor { .. } => {
                // Cell editor is still open, nothing to do
                self.last_status = Some("Continuing edit".to_string());
            }
            ConfirmContext::QuitApp | ConfirmContext::QuitAppClean => {
                // Stay in the app
                self.last_status = Some("Quit cancelled".to_string());
            }
            ConfirmContext::DeleteConnection { .. } => {
                // Cancelled delete, nothing to do
                self.last_status = Some("Delete cancelled".to_string());
            }
            ConfirmContext::CloseConnectionForm => {
                // Keep the form open
                self.last_status = Some("Continuing edit".to_string());
            }
            ConfirmContext::SwitchConnection { .. } => {
                // Cancelled connection switch
                self.last_status = Some("Connection switch cancelled".to_string());
            }
        }
    }

    fn handle_cell_edit_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            // Enter: confirm edit
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.commit_cell_edit();
                return false;
            }
            // Escape: cancel edit (with confirmation if modified)
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if self.cell_editor.is_modified() {
                    // Show confirmation prompt
                    self.confirm_prompt = Some(ConfirmPrompt::new(
                        "You have unsaved changes. Discard them?",
                        ConfirmContext::CloseCellEditor {
                            row: self.cell_editor.row,
                            col: self.cell_editor.col,
                        },
                    ));
                } else {
                    self.cell_editor.close();
                    self.last_status = Some("Edit cancelled".to_string());
                }
                return false;
            }
            // Backspace: delete character before cursor
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                self.cell_editor.delete_char_before();
            }
            // Delete: delete character at cursor
            (KeyCode::Delete, KeyModifiers::NONE) => {
                self.cell_editor.delete_char_at();
            }
            // Arrow keys for cursor movement
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.cell_editor.move_left();
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.cell_editor.move_right();
            }
            // Home/End for start/end of line
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.cell_editor.move_to_start();
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.cell_editor.move_to_end();
            }
            // Ctrl+A: move to start
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.cell_editor.move_to_start();
            }
            // Ctrl+E: move to end
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                self.cell_editor.move_to_end();
            }
            // Ctrl+U: delete from start to cursor
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.cell_editor.delete_to_start();
            }
            // Ctrl+K: delete from cursor to end
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.cell_editor.delete_to_end();
            }
            // Ctrl+W: delete word before cursor (simplified: clear all)
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                self.cell_editor.clear();
            }
            // Regular character input
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.cell_editor.insert_char(c);
            }
            _ => {}
        }
        false
    }

    /// Handle key events for the JSON editor modal.
    fn handle_json_editor_key(&mut self, key: KeyEvent) -> bool {
        // Take the editor temporarily to avoid borrow issues
        let mut editor = match self.json_editor.take() {
            Some(e) => e,
            None => return false,
        };

        match editor.handle_key(key) {
            JsonEditorAction::Continue => {
                // Put the editor back
                self.json_editor = Some(editor);
            }
            JsonEditorAction::Save { value, row, col } => {
                // Commit the edit
                self.commit_json_edit(value, row, col);
            }
            JsonEditorAction::Cancel => {
                // Editor is already taken, just don't put it back
                self.last_status = Some("Edit cancelled".to_string());
            }
            JsonEditorAction::RequestClose { row, col } => {
                // Show confirmation prompt, keep editor open
                self.json_editor = Some(editor);
                self.confirm_prompt = Some(ConfirmPrompt::new(
                    "You have unsaved changes. Discard them?",
                    ConfirmContext::CloseJsonEditor { row, col },
                ));
            }
            JsonEditorAction::Error(msg) => {
                // Show error but keep editor open
                self.last_error = Some(msg);
                self.json_editor = Some(editor);
            }
        }
        false
    }

    /// Commit a JSON edit to the database.
    fn commit_json_edit(&mut self, new_value: String, row: usize, col: usize) {
        // Generate UPDATE SQL (similar to commit_cell_edit)
        let table = match &self.grid.source_table {
            Some(t) => t.clone(),
            None => {
                self.last_error = Some("Cannot update: unknown source table".to_string());
                return;
            }
        };

        let column_name = match self.grid.headers.get(col) {
            Some(name) => name.clone(),
            None => {
                self.last_error = Some("Cannot update: invalid column".to_string());
                return;
            }
        };

        // Build WHERE clause from primary key values
        let pk_conditions: Vec<String> = self
            .grid
            .primary_keys
            .iter()
            .filter_map(|pk_name| {
                let pk_col_idx = self.grid.headers.iter().position(|h| h == pk_name)?;
                let pk_value = self.grid.rows.get(row)?.get(pk_col_idx)?;
                Some(format!(
                    "{} = {}",
                    quote_identifier(pk_name),
                    escape_sql_value(pk_value)
                ))
            })
            .collect();

        if pk_conditions.is_empty() {
            self.last_error = Some("Cannot update: missing primary key values".to_string());
            return;
        }

        let update_sql = format!(
            "UPDATE {} SET {} = {} WHERE {}",
            table,
            quote_identifier(&column_name),
            escape_sql_value(&new_value),
            pk_conditions.join(" AND ")
        );

        self.execute_cell_update(update_sql, row, col, new_value);
    }

    fn commit_cell_edit(&mut self) {
        let row = self.cell_editor.row;
        let col = self.cell_editor.col;
        let new_value = self.cell_editor.value.clone();
        let original_value = self.cell_editor.original_value.clone();

        // If value hasn't changed, just close
        if new_value == original_value {
            self.cell_editor.close();
            self.last_status = Some("No changes".to_string());
            return;
        }

        // Generate UPDATE SQL
        let table = match &self.grid.source_table {
            Some(t) => t.clone(),
            None => {
                self.cell_editor.close();
                self.last_error = Some("Cannot update: unknown source table".to_string());
                return;
            }
        };

        let column_name = match self.grid.headers.get(col) {
            Some(name) => name.clone(),
            None => {
                self.cell_editor.close();
                self.last_error = Some("Cannot update: invalid column".to_string());
                return;
            }
        };

        // Build WHERE clause from primary key values
        let pk_conditions: Vec<String> = self
            .grid
            .primary_keys
            .iter()
            .filter_map(|pk_name| {
                let pk_col_idx = self.grid.headers.iter().position(|h| h == pk_name)?;
                let pk_value = self.grid.rows.get(row)?.get(pk_col_idx)?;
                Some(format!(
                    "{} = {}",
                    quote_identifier(pk_name),
                    escape_sql_value(pk_value)
                ))
            })
            .collect();

        if pk_conditions.is_empty() {
            self.cell_editor.close();
            self.last_error = Some("Cannot update: missing primary key values".to_string());
            return;
        }

        let update_sql = format!(
            "UPDATE {} SET {} = {} WHERE {}",
            table,
            quote_identifier(&column_name),
            escape_sql_value(&new_value),
            pk_conditions.join(" AND ")
        );

        // Close editor and execute update
        self.cell_editor.close();
        self.execute_cell_update(update_sql, row, col, new_value);
    }

    fn execute_cell_update(&mut self, sql: String, row: usize, col: usize, new_value: String) {
        let Some(client) = self.db.client.clone() else {
            self.last_error = Some("Not connected".to_string());
            return;
        };

        if self.db.running {
            self.last_error = Some("Another query is running".to_string());
            return;
        }

        self.db.running = true;
        self.last_status = Some("Updating...".to_string());
        self.query_start_time = Some(Instant::now());

        let tx = self.db_events_tx.clone();

        // Store row/col/value for updating grid on success
        let update_row = row;
        let update_col = col;
        let update_value = new_value;

        self.rt.spawn(async move {
            let guard = client.lock().await;
            match guard.simple_query(&sql).await {
                Ok(_) => {
                    drop(guard);
                    // Send a custom event to update the cell
                    let _ = tx.send(DbEvent::CellUpdated {
                        row: update_row,
                        col: update_col,
                        value: update_value,
                    });
                }
                Err(e) => {
                    let _ = tx.send(DbEvent::QueryError {
                        error: format_pg_error(&e),
                    });
                }
            }
        });
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let pattern = self.search.text();
                let pattern = pattern.trim().to_string();

                match self.search_target {
                    SearchTarget::Editor => {
                        self.handle_editor_search(pattern);
                    }
                    SearchTarget::Grid => {
                        self.handle_grid_search(pattern);
                    }
                }
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                // Clear input.
                self.search.open();
            }
            _ => {
                let input: Input = key.into();
                self.search.textarea.input(input);
            }
        }
    }

    fn handle_editor_search(&mut self, pattern: String) {
        if pattern.is_empty() {
            let _ = self.editor.textarea.set_search_pattern("");
            self.search.last_applied = None;
            self.search.close();
            self.last_status = Some("Search cleared".to_string());
            return;
        }

        match self.editor.textarea.set_search_pattern(&pattern) {
            Ok(()) => {
                self.search.last_applied = Some(pattern.clone());
                self.search.close();

                let found = self.editor.textarea.search_forward(true);
                if found {
                    self.last_status = Some(format!("Search: /{}", pattern));
                } else {
                    self.last_status = Some(format!("Search: /{} (no match)", pattern));
                }
            }
            Err(e) => {
                // Keep the prompt open so the user can fix the regex.
                self.last_status = Some(format!("Invalid search pattern: {}", e));
            }
        }
    }

    fn handle_grid_search(&mut self, pattern: String) {
        if pattern.is_empty() {
            self.grid_state.clear_search();
            self.search.close();
            self.last_status = Some("Grid search cleared".to_string());
            return;
        }

        self.grid_state.apply_search(&pattern, &self.grid);
        self.search.close();

        let match_count = self.grid_state.search.match_count();
        if match_count > 0 {
            self.last_status = Some(format!("Grid: /{} ({} matches)", pattern, match_count));
        } else {
            self.last_status = Some(format!("Grid: /{} (no matches)", pattern));
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let cmd = self.command.text();
                let cmd = cmd.trim();
                self.command.close();
                return self.execute_command(cmd);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                // Clear input.
                self.command.open();
            }
            _ => {
                let input: Input = key.into();
                self.command.textarea.input(input);
            }
        }
        false
    }

    fn execute_command(&mut self, cmd: &str) -> bool {
        if cmd.is_empty() {
            return false;
        }

        // Handle numeric commands (:N to go to row N)
        if let Ok(row_num) = cmd.parse::<usize>() {
            return self.goto_result_row(row_num);
        }

        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        let command = parts[0];
        let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

        match command {
            "q" | "quit" | "exit" => {
                return true;
            }
            "connect" | "c" => {
                if args.is_empty() {
                    self.last_status = Some("Usage: :connect <connection_url>".to_string());
                } else {
                    self.start_connect(args.to_string());
                }
            }
            "disconnect" | "dc" => {
                self.db.client = None;
                self.db.cancel_token = None;
                self.db.status = DbStatus::Disconnected;
                self.db.running = false;
                self.last_status = Some("Disconnected".to_string());
            }
            "help" | "h" => {
                self.help_popup = Some(HelpPopup::new());
            }
            "export" | "e" => {
                self.handle_export_command(args);
            }
            "gen" | "generate" => {
                self.handle_gen_command(args);
            }
            // psql-style backslash commands
            "\\dt" | "dt" => {
                self.execute_meta_query(META_QUERY_TABLES, None);
            }
            "\\dn" | "dn" => {
                self.execute_meta_query(META_QUERY_SCHEMAS, None);
            }
            "\\d" | "d" => {
                if args.is_empty() {
                    // \d without args is same as \dt
                    self.execute_meta_query(META_QUERY_TABLES, None);
                } else {
                    // \d <table> - describe table
                    self.execute_meta_query(META_QUERY_DESCRIBE, Some(args));
                }
            }
            "\\di" | "di" => {
                self.execute_meta_query(META_QUERY_INDEXES, None);
            }
            "\\l" | "l" => {
                self.execute_meta_query(META_QUERY_DATABASES, None);
            }
            "\\du" | "du" => {
                self.execute_meta_query(META_QUERY_ROLES, None);
            }
            "\\dv" | "dv" => {
                self.execute_meta_query(META_QUERY_VIEWS, None);
            }
            "\\df" | "df" => {
                self.execute_meta_query(META_QUERY_FUNCTIONS, None);
            }
            "\\conninfo" | "conninfo" => {
                self.show_connection_info();
            }
            "\\?" | "?" => {
                // psql-style help alias
                self.help_popup = Some(HelpPopup::new());
            }
            "history" => {
                self.open_history_picker();
            }
            "connections" | "conn" => {
                self.open_connection_manager();
            }
            _ => {
                self.last_status = Some(format!("Unknown command: {}", command));
            }
        }

        false
    }

    fn goto_result_row(&mut self, row_num: usize) -> bool {
        if self.grid.rows.is_empty() {
            self.last_status = Some("No results to navigate".to_string());
            return false;
        }

        let target_row = if row_num == 0 {
            0
        } else {
            (row_num - 1).min(self.grid.rows.len() - 1)
        };

        self.grid_state.cursor_row = target_row;
        self.focus = Focus::Grid;
        self.last_status = Some(format!(
            "Row {} of {}",
            target_row + 1,
            self.grid.rows.len()
        ));
        false
    }

    /// Execute a meta-command query (like \dt, \d, etc.)
    fn execute_meta_query(&mut self, query_template: &str, table_arg: Option<&str>) {
        let Some(client) = self.db.client.clone() else {
            self.last_error = Some("Not connected. Use :connect <url> first.".to_string());
            return;
        };

        if self.db.running {
            self.last_status = Some("Query already running".to_string());
            return;
        }

        // Build the query, substituting table name if provided
        let query = if let Some(table) = table_arg {
            query_template.replace("$1", &escape_sql_identifier(table))
        } else {
            query_template.to_string()
        };

        self.db.running = true;
        self.last_status = Some("Running...".to_string());
        self.query_start_time = Some(Instant::now());

        let tx = self.db_events_tx.clone();
        let started = Instant::now();

        self.rt.spawn(async move {
            let guard = client.lock().await;
            match guard.simple_query(&query).await {
                Ok(messages) => {
                    drop(guard);
                    let elapsed = started.elapsed();

                    let mut headers: Vec<String> = Vec::new();
                    let mut rows: Vec<Vec<String>> = Vec::new();

                    for msg in messages {
                        match msg {
                            SimpleQueryMessage::Row(row) => {
                                if headers.is_empty() {
                                    headers = row
                                        .columns()
                                        .iter()
                                        .map(|c| c.name().to_string())
                                        .collect();
                                }
                                let mut out_row = Vec::with_capacity(row.len());
                                for i in 0..row.len() {
                                    out_row.push(row.get(i).unwrap_or("NULL").to_string());
                                }
                                rows.push(out_row);
                            }
                            SimpleQueryMessage::CommandComplete(_) => {}
                            _ => {}
                        }
                    }

                    let result = QueryResult {
                        headers,
                        rows,
                        command_tag: None,
                        truncated: false,
                        elapsed,
                        source_table: None,
                        primary_keys: Vec::new(),
                        col_types: Vec::new(), // Meta queries don't need column types
                    };

                    let _ = tx.send(DbEvent::QueryFinished { result });
                }
                Err(e) => {
                    let _ = tx.send(DbEvent::QueryError {
                        error: format_pg_error(&e),
                    });
                }
            }
        });
    }

    fn handle_export_command(&mut self, args: &str) {
        if self.grid.rows.is_empty() {
            self.last_error = Some("No data to export".to_string());
            return;
        }

        let parts: Vec<&str> = args.splitn(2, ' ').collect();
        if parts.is_empty() || parts[0].is_empty() {
            self.last_status = Some("Usage: :export csv|json|tsv <path>".to_string());
            return;
        }

        let format = parts[0].to_lowercase();
        let path = parts.get(1).map(|s| s.trim()).unwrap_or("");

        if path.is_empty() {
            self.last_status = Some(format!("Usage: :export {} <path>", format));
            return;
        }

        // Get all row indices
        let indices: Vec<usize> = (0..self.grid.rows.len()).collect();

        let content = match format.as_str() {
            "csv" => self.grid.rows_as_csv(&indices, true),
            "json" => self.grid.rows_as_json(&indices),
            "tsv" => self.grid.rows_as_tsv(&indices, true),
            _ => {
                self.last_error = Some(format!(
                    "Unknown format: {}. Use csv, json, or tsv.",
                    format
                ));
                return;
            }
        };

        // Expand ~ to home directory
        let expanded_path = if let Some(stripped) = path.strip_prefix("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                std::path::PathBuf::from(home).join(stripped)
            } else {
                std::path::PathBuf::from(path)
            }
        } else {
            std::path::PathBuf::from(path)
        };

        match std::fs::write(&expanded_path, &content) {
            Ok(()) => {
                let rows = self.grid.rows.len();
                self.last_status = Some(format!(
                    "Exported {} rows to {} as {}",
                    rows,
                    expanded_path.display(),
                    format.to_uppercase()
                ));
            }
            Err(e) => {
                self.last_error = Some(format!("Failed to write file: {}", e));
            }
        }
    }

    /// Show connection information (psql \conninfo equivalent).
    fn show_connection_info(&mut self) {
        match self.db.status {
            DbStatus::Disconnected => {
                self.last_status = Some("Not connected.".to_string());
            }
            DbStatus::Connecting => {
                self.last_status = Some("Connection in progress...".to_string());
            }
            DbStatus::Connected => {
                if let Some(ref conn_str) = self.db.conn_str {
                    let info = ConnectionInfo::parse(conn_str);
                    let user = info.user.as_deref().unwrap_or("unknown");
                    let host = info.host.as_deref().unwrap_or("localhost");
                    let port = info.port.unwrap_or(5432);
                    let database = info.database.as_deref().unwrap_or("unknown");
                    self.last_status = Some(format!(
                        "Connected to database \"{}\" as user \"{}\" on host \"{}\" port {}.",
                        database, user, host, port
                    ));
                } else {
                    self.last_status =
                        Some("Connected (no connection string available).".to_string());
                }
            }
            DbStatus::Error => {
                self.last_status =
                    Some("Connection error. Use :connect <url> to reconnect.".to_string());
            }
        }
    }

    fn handle_gen_command(&mut self, args: &str) {
        if self.grid.rows.is_empty() {
            self.last_error = Some("No data to generate SQL from".to_string());
            return;
        }

        // Parse: gen <type> [table] [key_col1,key_col2,...]
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.is_empty() {
            self.last_status =
                Some("Usage: :gen <update|delete|insert> [table] [key_columns]".to_string());
            return;
        }

        let gen_type = parts[0].to_lowercase();

        // Use provided table or fall back to source_table from query
        let table: String = match parts.get(1) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => match &self.grid.source_table {
                Some(t) => t.clone(),
                None => {
                    self.last_error = Some(format!(
                        "No table specified and couldn't infer from query. Usage: :gen {} <table>",
                        gen_type
                    ));
                    return;
                }
            },
        };

        // Parse optional key columns (comma-separated) - shifts by 1 if table was provided
        // If not provided, try to use primary keys from the grid
        let explicit_keys: Option<Vec<String>> = if parts.len() > 2 {
            Some(parts[2].split(',').map(|s| s.to_string()).collect())
        } else {
            None
        };

        // Get row indices: selected rows or current row
        let row_indices: Vec<usize> = if self.grid_state.selected_rows.is_empty() {
            vec![self.grid_state.cursor_row]
        } else {
            self.grid_state.selected_rows.iter().copied().collect()
        };

        // Determine which key columns to use:
        // 1. Explicitly provided keys
        // 2. Primary keys from grid (if available and valid)
        // 3. None (will use defaults in generate functions)
        let key_columns: Option<Vec<String>> = explicit_keys.or_else(|| {
            if self.grid.has_valid_pk() {
                Some(self.grid.primary_keys.clone())
            } else {
                None
            }
        });

        let sql = match gen_type.as_str() {
            "update" | "u" => {
                let keys: Option<Vec<&str>> = key_columns
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect());
                self.grid
                    .generate_update_sql(&table, &row_indices, keys.as_deref())
            }
            "delete" | "d" => {
                let keys: Option<Vec<&str>> = key_columns
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect());
                self.grid
                    .generate_delete_sql(&table, &row_indices, keys.as_deref())
            }
            "insert" | "i" => self.grid.generate_insert_sql(&table, &row_indices),
            _ => {
                self.last_error = Some(format!(
                    "Unknown generate type: {}. Use update, delete, or insert.",
                    gen_type
                ));
                return;
            }
        };

        // Put the generated SQL into the editor
        self.editor.textarea.select_all();
        self.editor.textarea.cut();
        self.editor.textarea.insert_str(&sql);

        // Move focus to the editor so user can review/edit
        self.focus = Focus::Query;
        self.mode = Mode::Normal;

        let row_count = row_indices.len();
        self.last_status = Some(format!(
            "Generated {} {} statement{} for {} row{}",
            gen_type.to_uppercase(),
            row_count,
            if row_count == 1 { "" } else { "s" },
            row_count,
            if row_count == 1 { "" } else { "s" }
        ));
    }

    /// Handle an editor action from the keymap. Returns true if the action was handled.
    fn handle_editor_action(&mut self, action: Action) -> bool {
        match action {
            // Navigation
            Action::MoveUp => {
                self.editor.textarea.move_cursor(CursorMove::Up);
            }
            Action::MoveDown => {
                self.editor.textarea.move_cursor(CursorMove::Down);
            }
            Action::MoveLeft => {
                self.editor.textarea.move_cursor(CursorMove::Back);
            }
            Action::MoveRight => {
                self.editor.textarea.move_cursor(CursorMove::Forward);
            }
            Action::MoveToTop => {
                self.editor.textarea.move_cursor(CursorMove::Top);
            }
            Action::MoveToBottom => {
                self.editor.textarea.move_cursor(CursorMove::Bottom);
            }
            Action::MoveToStart => {
                self.editor.textarea.move_cursor(CursorMove::Head);
            }
            Action::MoveToEnd => {
                self.editor.textarea.move_cursor(CursorMove::End);
            }
            Action::PageUp => {
                for _ in 0..10 {
                    self.editor.textarea.move_cursor(CursorMove::Up);
                }
            }
            Action::PageDown => {
                for _ in 0..10 {
                    self.editor.textarea.move_cursor(CursorMove::Down);
                }
            }
            Action::HalfPageUp => {
                for _ in 0..5 {
                    self.editor.textarea.move_cursor(CursorMove::Up);
                }
            }
            Action::HalfPageDown => {
                for _ in 0..5 {
                    self.editor.textarea.move_cursor(CursorMove::Down);
                }
            }

            // Mode switching
            Action::EnterInsertMode => {
                self.mode = Mode::Insert;
            }
            Action::EnterNormalMode => {
                self.mode = Mode::Normal;
            }
            Action::EnterVisualMode => {
                self.editor.textarea.start_selection();
                self.mode = Mode::Visual;
            }
            Action::EnterCommandMode => {
                self.command.open();
            }

            // Focus
            Action::ToggleFocus => {
                self.focus = Focus::Grid;
            }
            Action::FocusGrid => {
                self.focus = Focus::Grid;
            }

            // Editor actions
            Action::DeleteChar => {
                self.editor.textarea.delete_next_char();
            }
            Action::DeleteLine => {
                self.editor.delete_line();
            }
            Action::Undo => {
                self.editor.textarea.undo();
            }
            Action::Redo => {
                self.editor.textarea.redo();
            }
            Action::Copy => {
                self.editor.textarea.copy();
                self.last_status = Some("Copied".to_string());
            }
            Action::Paste => {
                self.editor.textarea.paste();
            }
            Action::Cut => {
                self.editor.textarea.cut();
            }
            Action::SelectAll => {
                self.editor.textarea.select_all();
            }

            // Query execution
            Action::ExecuteQuery => {
                self.execute_query();
            }

            // Search
            Action::StartSearch => {
                self.search_target = SearchTarget::Editor;
                self.search.open();
            }
            Action::NextMatch => {
                if let Some(p) = self.search.last_applied.clone() {
                    let found = self.editor.textarea.search_forward(false);
                    if found {
                        self.last_status = Some(format!("Search next: /{}", p));
                    } else {
                        self.last_status = Some(format!("Search next: /{} (no match)", p));
                    }
                }
            }
            Action::PrevMatch => {
                if let Some(p) = self.search.last_applied.clone() {
                    let found = self.editor.textarea.search_back(false);
                    if found {
                        self.last_status = Some(format!("Search prev: /{}", p));
                    } else {
                        self.last_status = Some(format!("Search prev: /{} (no match)", p));
                    }
                }
            }

            // Application
            Action::Help => {
                self.help_popup = Some(HelpPopup::new());
            }
            Action::ShowHistory => {
                self.open_history_picker();
            }
            Action::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                if !self.sidebar_visible && matches!(self.focus, Focus::Sidebar(_)) {
                    self.focus = Focus::Query;
                }
            }

            // Goto navigation (custom keybindings for navigation)
            Action::GotoFirst => {
                // In editor context, go to document start
                self.editor.textarea.move_cursor(CursorMove::Top);
                self.editor.textarea.move_cursor(CursorMove::Head);
            }
            Action::GotoEditor => {
                // Already in editor, this is a no-op but keep focus
                self.focus = Focus::Query;
            }
            Action::GotoConnections => {
                self.sidebar_visible = true;
                self.sidebar_focus = SidebarSection::Connections;
                self.focus = Focus::Sidebar(SidebarSection::Connections);
            }
            Action::GotoTables => {
                self.focus_schema();
            }
            Action::GotoResults => {
                self.focus = Focus::Grid;
            }

            // Actions not applicable to editor
            _ => return false,
        }
        true
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => {
                // Handle pending operator commands (d, c, g).
                if let Some(pending) = self.pending_key {
                    self.pending_key = None;
                    match (pending, key.code, key.modifiers) {
                        // gg - go to top
                        ('g', KeyCode::Char('g'), KeyModifiers::NONE) => {
                            self.editor.textarea.move_cursor(CursorMove::Top);
                            return;
                        }
                        // dd - delete line
                        ('d', KeyCode::Char('d'), KeyModifiers::NONE) => {
                            self.editor.delete_line();
                            return;
                        }
                        // dw - delete word forward
                        ('d', KeyCode::Char('w'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_word();
                            return;
                        }
                        // de - delete to end of word
                        ('d', KeyCode::Char('e'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_word();
                            return;
                        }
                        // db - delete word backward
                        ('d', KeyCode::Char('b'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_word();
                            return;
                        }
                        // d$ - delete to end of line
                        ('d', KeyCode::Char('$'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_line_by_end();
                            return;
                        }
                        // d0 - delete to start of line
                        ('d', KeyCode::Char('0'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_line_by_head();
                            return;
                        }
                        // dh - delete character left (like X)
                        ('d', KeyCode::Char('h'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_char();
                            return;
                        }
                        // dl - delete character right (like x)
                        ('d', KeyCode::Char('l'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_char();
                            return;
                        }
                        // dj - delete current line and line below
                        ('d', KeyCode::Char('j'), KeyModifiers::NONE) => {
                            self.editor.delete_line();
                            self.editor.delete_line();
                            return;
                        }
                        // dk - delete current line and line above
                        ('d', KeyCode::Char('k'), KeyModifiers::NONE) => {
                            self.editor.delete_line();
                            self.editor.textarea.move_cursor(CursorMove::Up);
                            self.editor.delete_line();
                            return;
                        }
                        // dG - delete to end of file
                        ('d', KeyCode::Char('G'), KeyModifiers::SHIFT)
                        | ('d', KeyCode::Char('G'), KeyModifiers::NONE) => {
                            // Delete from current line to end of file
                            loop {
                                let (row, _) = self.editor.textarea.cursor();
                                let line_count = self.editor.textarea.lines().len();
                                if line_count <= 1 {
                                    // Clear the last line
                                    self.editor.textarea.move_cursor(CursorMove::Head);
                                    self.editor.textarea.delete_line_by_end();
                                    break;
                                }
                                self.editor.delete_line();
                                // Check if we're at the last line
                                let new_row = self.editor.textarea.cursor().0;
                                if new_row >= self.editor.textarea.lines().len().saturating_sub(1) {
                                    self.editor.textarea.move_cursor(CursorMove::Head);
                                    self.editor.textarea.delete_line_by_end();
                                    break;
                                }
                                if row == new_row && row == 0 {
                                    break;
                                }
                            }
                            return;
                        }
                        // cc - change line
                        ('c', KeyCode::Char('c'), KeyModifiers::NONE) => {
                            self.editor.change_line();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // cw - change word forward
                        ('c', KeyCode::Char('w'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_word();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // ce - change to end of word
                        ('c', KeyCode::Char('e'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_word();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // cb - change word backward
                        ('c', KeyCode::Char('b'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_word();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // c$ - change to end of line
                        ('c', KeyCode::Char('$'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_line_by_end();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // c0 - change to start of line
                        ('c', KeyCode::Char('0'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_line_by_head();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // ch - change character left
                        ('c', KeyCode::Char('h'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_char();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // cl - change character right (like s)
                        ('c', KeyCode::Char('l'), KeyModifiers::NONE) => {
                            self.editor.textarea.delete_next_char();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // cj - change current line and line below
                        ('c', KeyCode::Char('j'), KeyModifiers::NONE) => {
                            self.editor.delete_line();
                            self.editor.change_line();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // ck - change current line and line above
                        ('c', KeyCode::Char('k'), KeyModifiers::NONE) => {
                            self.editor.delete_line();
                            self.editor.textarea.move_cursor(CursorMove::Up);
                            self.editor.change_line();
                            self.mode = Mode::Insert;
                            return;
                        }
                        // yy - yank (copy) line to system clipboard
                        ('y', KeyCode::Char('y'), KeyModifiers::NONE) => {
                            if let Some(text) = self.editor.yank_line() {
                                self.copy_to_clipboard(&text);
                            }
                            return;
                        }
                        _ => {
                            // Unknown combo, ignore pending
                            return;
                        }
                    }
                }

                // Try keymap first for normal mode actions
                if let Some(action) = self.editor_normal_keymap.get_action(&key) {
                    self.pending_key = None;
                    if self.handle_editor_action(action) {
                        return;
                    }
                }

                // Fall back to vim-specific keys that need special handling
                match (key.code, key.modifiers) {
                    (KeyCode::Char('g'), KeyModifiers::NONE) => {
                        self.pending_key = Some('g');
                    }
                    // Start operator-pending mode for d and c
                    (KeyCode::Char('d'), KeyModifiers::NONE) => {
                        self.pending_key = Some('d');
                    }
                    (KeyCode::Char('c'), KeyModifiers::NONE) => {
                        self.pending_key = Some('c');
                    }
                    (KeyCode::Char('G'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('G'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Bottom);
                    }

                    (KeyCode::Char('0'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Head);
                    }
                    (KeyCode::Char('$'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::End);
                    }

                    (KeyCode::Char('w'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::WordForward);
                    }
                    (KeyCode::Char('b'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::WordBack);
                    }
                    (KeyCode::Char('e'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::WordEnd);
                    }

                    (KeyCode::Char('/'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.search_target = SearchTarget::Editor;
                        self.search.open();
                    }
                    (KeyCode::Char(':'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.command.open();
                    }
                    (KeyCode::Char('n'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        if let Some(p) = self.search.last_applied.clone() {
                            let found = self.editor.textarea.search_forward(false);
                            if found {
                                self.last_status = Some(format!("Search next: /{}", p));
                            } else {
                                self.last_status = Some(format!("Search next: /{} (no match)", p));
                            }
                        }
                    }
                    (KeyCode::Char('N'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('N'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        if let Some(p) = self.search.last_applied.clone() {
                            let found = self.editor.textarea.search_back(false);
                            if found {
                                self.last_status = Some(format!("Search prev: /{}", p));
                            } else {
                                self.last_status = Some(format!("Search prev: /{} (no match)", p));
                            }
                        }
                    }

                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        self.pending_key = None;
                        for _ in 0..10 {
                            self.editor.textarea.move_cursor(CursorMove::Down);
                        }
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        self.pending_key = None;
                        for _ in 0..10 {
                            self.editor.textarea.move_cursor(CursorMove::Up);
                        }
                    }

                    (KeyCode::Char('i'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('a'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Forward);
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('A'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('A'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::End);
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('I'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('I'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Head);
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('o'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::End);
                        self.editor.textarea.insert_newline();
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('O'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('O'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Head);
                        self.editor.textarea.insert_newline();
                        self.editor.textarea.move_cursor(CursorMove::Up);
                        self.mode = Mode::Insert;
                    }

                    // Delete commands.
                    (KeyCode::Char('x'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_next_char();
                    }
                    (KeyCode::Char('X'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('X'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_char();
                    }
                    (KeyCode::Char('D'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('D'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_line_by_end();
                    }
                    (KeyCode::Char('C'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('C'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_line_by_end();
                        self.mode = Mode::Insert;
                    }

                    // Undo/Redo.
                    (KeyCode::Char('u'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.undo();
                    }
                    (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                        self.pending_key = None;
                        self.editor.textarea.redo();
                    }

                    // Visual mode.
                    (KeyCode::Char('v'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.start_selection();
                        self.mode = Mode::Visual;
                    }

                    // Paste.
                    (KeyCode::Char('p'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.paste();
                    }
                    (KeyCode::Char('P'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('P'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        // Paste before cursor: move back, paste, then adjust.
                        self.editor.textarea.move_cursor(CursorMove::Back);
                        self.editor.textarea.paste();
                    }

                    // Yank current line (yy).
                    (KeyCode::Char('y'), KeyModifiers::NONE) => {
                        self.pending_key = Some('y');
                    }

                    // Execute query: in Normal mode, Enter runs.
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.execute_query();
                    }

                    // History navigation.
                    (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                        self.pending_key = None;
                        self.editor.history_prev();
                    }
                    (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                        self.pending_key = None;
                        self.editor.history_next();
                    }

                    // Vim-like movement.
                    (KeyCode::Char('h'), KeyModifiers::NONE)
                    | (KeyCode::Left, KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Back);
                    }
                    (KeyCode::Char('j'), KeyModifiers::NONE)
                    | (KeyCode::Down, KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Down);
                    }
                    (KeyCode::Char('k'), KeyModifiers::NONE)
                    | (KeyCode::Up, KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Up);
                    }
                    (KeyCode::Char('l'), KeyModifiers::NONE)
                    | (KeyCode::Right, KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::Forward);
                    }

                    _ => {
                        self.pending_key = None;
                    }
                }
            }

            Mode::Insert => {
                // Handle Tab for completion in Insert mode
                if key.code == KeyCode::Tab && key.modifiers == KeyModifiers::NONE {
                    self.trigger_completion();
                    return;
                }

                // Check keymap for insert mode actions (e.g., Ctrl+S to execute)
                if let Some(action) = self.editor_insert_keymap.get_action(&key) {
                    match action {
                        Action::EnterNormalMode => {
                            self.mode = Mode::Normal;
                            return;
                        }
                        Action::ExecuteQuery => {
                            self.execute_query();
                            return;
                        }
                        Action::ToggleFocus => {
                            self.focus = Focus::Grid;
                            return;
                        }
                        Action::Undo => {
                            self.editor.textarea.undo();
                            return;
                        }
                        Action::Redo => {
                            self.editor.textarea.redo();
                            return;
                        }
                        Action::Copy => {
                            self.editor.textarea.copy();
                            self.last_status = Some("Copied".to_string());
                            return;
                        }
                        Action::Paste => {
                            self.editor.textarea.paste();
                            return;
                        }
                        Action::Cut => {
                            self.editor.textarea.cut();
                            return;
                        }
                        Action::SelectAll => {
                            self.editor.textarea.select_all();
                            return;
                        }
                        // Goto navigation (custom keybindings for navigation)
                        Action::GotoFirst => {
                            self.editor.textarea.move_cursor(CursorMove::Top);
                            self.editor.textarea.move_cursor(CursorMove::Head);
                            return;
                        }
                        Action::GotoEditor => {
                            // Already in editor
                            return;
                        }
                        Action::GotoConnections => {
                            self.sidebar_visible = true;
                            self.sidebar_focus = SidebarSection::Connections;
                            self.focus = Focus::Sidebar(SidebarSection::Connections);
                            return;
                        }
                        Action::GotoTables => {
                            self.focus_schema();
                            return;
                        }
                        Action::GotoResults => {
                            self.focus = Focus::Grid;
                            return;
                        }
                        _ => {}
                    }
                }

                // Forward nearly everything to the textarea.
                self.editor.input(key);
            }

            Mode::Visual => {
                // In visual mode, movement extends selection, y/d/c act on selection.
                match (key.code, key.modifiers) {
                    // Exit visual mode.
                    (KeyCode::Esc, KeyModifiers::NONE) => {
                        self.editor.textarea.cancel_selection();
                        self.mode = Mode::Normal;
                    }
                    // Yank (copy) selection to system clipboard.
                    (KeyCode::Char('y'), KeyModifiers::NONE) => {
                        // Copy to internal buffer first (this also captures the selection)
                        self.editor.textarea.copy();
                        // Get the yanked text and copy to system clipboard
                        if let Some(text) = self.editor.get_selection() {
                            self.copy_to_clipboard(&text);
                        }
                        self.editor.textarea.cancel_selection();
                        self.mode = Mode::Normal;
                    }
                    // Delete selection.
                    (KeyCode::Char('d'), KeyModifiers::NONE)
                    | (KeyCode::Char('x'), KeyModifiers::NONE) => {
                        self.editor.textarea.cut();
                        self.mode = Mode::Normal;
                    }
                    // Change selection (delete and enter insert mode).
                    (KeyCode::Char('c'), KeyModifiers::NONE) => {
                        self.editor.textarea.cut();
                        self.mode = Mode::Insert;
                    }
                    // Movement keys extend selection.
                    (KeyCode::Char('h'), KeyModifiers::NONE)
                    | (KeyCode::Left, KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Back);
                    }
                    (KeyCode::Char('j'), KeyModifiers::NONE)
                    | (KeyCode::Down, KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Down);
                    }
                    (KeyCode::Char('k'), KeyModifiers::NONE)
                    | (KeyCode::Up, KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Up);
                    }
                    (KeyCode::Char('l'), KeyModifiers::NONE)
                    | (KeyCode::Right, KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Forward);
                    }
                    (KeyCode::Char('w'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::WordForward);
                    }
                    (KeyCode::Char('b'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::WordBack);
                    }
                    (KeyCode::Char('e'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::WordEnd);
                    }
                    (KeyCode::Char('0'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Head);
                    }
                    (KeyCode::Char('$'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::End);
                    }
                    (KeyCode::Char('g'), KeyModifiers::NONE) => {
                        self.pending_key = Some('g');
                    }
                    (KeyCode::Char('G'), KeyModifiers::SHIFT)
                    | (KeyCode::Char('G'), KeyModifiers::NONE) => {
                        self.editor.textarea.move_cursor(CursorMove::Bottom);
                    }
                    _ => {}
                }

                // Handle gg in visual mode.
                if self.pending_key == Some('g') {
                    if key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::NONE {
                        self.editor.textarea.move_cursor(CursorMove::Top);
                    }
                    self.pending_key = None;
                }
            }
        }
    }

    pub fn start_connect(&mut self, conn_str: String) {
        self.db.status = DbStatus::Connecting;
        self.db.conn_str = Some(conn_str.clone());
        self.db.client = None;
        self.db.running = false;

        self.last_status = Some("Connecting...".to_string());

        let tx = self.db_events_tx.clone();
        let rt = self.rt.clone();

        self.rt.spawn(async move {
            match tokio_postgres::connect(&conn_str, NoTls).await {
                Ok((client, connection)) => {
                    // Drive the connection on the runtime and surface errors.
                    let tx2 = tx.clone();
                    rt.spawn(async move {
                        if let Err(e) = connection.await {
                            let _ = tx2.send(DbEvent::ConnectionLost {
                                error: format_pg_error(&e),
                            });
                        }
                    });

                    let token = client.cancel_token();
                    let shared = Arc::new(Mutex::new(client));
                    let _ = tx.send(DbEvent::Connected {
                        client: shared,
                        cancel_token: token,
                    });
                }
                Err(e) => {
                    let _ = tx.send(DbEvent::ConnectError {
                        error: format_pg_error(&e),
                    });
                }
            }
        });
    }

    /// Connect to a saved connection entry.
    pub fn connect_to_entry(&mut self, entry: ConnectionEntry) {
        // Try to get the password
        let password = match entry.get_password() {
            Ok(Some(pwd)) => Some(pwd),
            Ok(None) => None,
            Err(e) => {
                self.last_error = Some(format!("Failed to get password: {}", e));
                None
            }
        };

        // Build the connection URL
        let url = entry.to_url(password.as_deref());
        self.current_connection_name = Some(entry.name.clone());
        self.start_connect(url);
    }

    /// Open the connection picker (fuzzy finder for quick connection selection).
    pub fn open_connection_picker(&mut self) {
        // Reload connections from disk to pick up changes from other instances
        if let Ok(connections) = load_connections() {
            self.connections = connections;
        }

        // If no connections, open the full manager instead
        if self.connections.connections.is_empty() {
            self.open_connection_manager();
            return;
        }

        // Get sorted connections (favorites first, then alphabetical)
        // Clone them since FuzzyPicker needs owned values
        let entries: Vec<ConnectionEntry> =
            self.connections.sorted().into_iter().cloned().collect();

        let picker = FuzzyPicker::with_display(entries, "Connect (Ctrl+O: manage)", |entry| {
            // Display: "[fav] name - user@host/db"
            let fav = entry
                .favorite
                .map(|f| format!("[{}] ", f))
                .unwrap_or_default();
            format!("{}{} - {}", fav, entry.name, entry.short_display())
        });

        self.connection_picker = Some(picker);
    }

    /// Handle key events when connection picker is open.
    fn handle_connection_picker_key(&mut self, key: KeyEvent) -> bool {
        // Check for Ctrl+O to open connection manager
        if key.code == KeyCode::Char('o') && key.modifiers == KeyModifiers::CONTROL {
            self.connection_picker = None;
            self.open_connection_manager();
            return false;
        }

        let picker = match self.connection_picker.as_mut() {
            Some(p) => p,
            None => return false,
        };

        match picker.handle_key(key) {
            PickerAction::Continue => false,
            PickerAction::Selected(entry) => {
                self.connection_picker = None;
                if self.editor.is_modified() {
                    self.confirm_prompt = Some(ConfirmPrompt::new(
                        "You have unsaved changes. Switch connection anyway?",
                        ConfirmContext::SwitchConnection { entry },
                    ));
                } else {
                    self.connect_to_entry(entry);
                }
                false
            }
            PickerAction::Cancelled => {
                self.connection_picker = None;
                false
            }
        }
    }

    fn quote_identifier_always(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }

    fn format_table_ref(&self, schema: &str, table: &str) -> String {
        use crate::config::IdentifierStyle;

        match self.config.sql.identifier_style {
            IdentifierStyle::QualifiedQuoted => format!(
                "{}.{}",
                Self::quote_identifier_always(schema),
                Self::quote_identifier_always(table)
            ),
            IdentifierStyle::Minimal => {
                format!("{}.{}", quote_identifier(schema), quote_identifier(table))
            }
        }
    }

    fn format_column(&self, column: &str) -> String {
        use crate::config::IdentifierStyle;

        match self.config.sql.identifier_style {
            IdentifierStyle::QualifiedQuoted => Self::quote_identifier_always(column),
            IdentifierStyle::Minimal => quote_identifier(column),
        }
    }

    /// Format just the table name (without schema qualification).
    fn format_table_name_only(&self, table: &str) -> String {
        use crate::config::IdentifierStyle;

        match self.config.sql.identifier_style {
            IdentifierStyle::QualifiedQuoted => Self::quote_identifier_always(table),
            IdentifierStyle::Minimal => quote_identifier(table),
        }
    }

    fn schema_table_columns(&self, schema: &str, table: &str) -> Option<Vec<String>> {
        self.schema_cache
            .tables
            .iter()
            .find(|t| t.schema == schema && t.name == table)
            .map(|t| t.columns.iter().map(|c| c.name.clone()).collect())
    }

    fn build_select_template(&self, ctx: &SchemaTableContext) -> String {
        let table_ref = self.format_table_ref(&ctx.schema, &ctx.table);
        let limit = self.config.sql.default_select_limit;
        format!("SELECT *\nFROM {}\nLIMIT {};", table_ref, limit)
    }

    fn build_insert_template(&self, ctx: &SchemaTableContext) -> String {
        let table_ref = self.format_table_ref(&ctx.schema, &ctx.table);
        let columns = self.schema_table_columns(&ctx.schema, &ctx.table);

        let Some(columns) = columns else {
            return format!(
                "INSERT INTO {} (\n  -- TODO: columns\n) VALUES (\n  -- TODO: values\n);",
                table_ref
            );
        };

        if columns.is_empty() {
            return format!(
                "INSERT INTO {} (\n  -- TODO: columns\n) VALUES (\n  -- TODO: values\n);",
                table_ref
            );
        }

        let column_lines = columns
            .iter()
            .map(|c| format!("  {}", self.format_column(c)))
            .collect::<Vec<_>>()
            .join(",\n");

        let value_lines = columns
            .iter()
            .map(|c| format!("  NULL -- {}", c))
            .collect::<Vec<_>>()
            .join(",\n");

        format!(
            "INSERT INTO {} (\n{}\n) VALUES (\n{}\n);",
            table_ref, column_lines, value_lines
        )
    }

    fn build_update_template(&self, ctx: &SchemaTableContext) -> String {
        let table_ref = self.format_table_ref(&ctx.schema, &ctx.table);
        let first_col = self
            .schema_table_columns(&ctx.schema, &ctx.table)
            .and_then(|cols| cols.into_iter().next());

        let set_line = match first_col {
            Some(col) => format!("  {} = NULL", self.format_column(&col)),
            None => "  -- TODO: set clause".to_string(),
        };

        format!(
            "UPDATE {}\nSET\n{}\nWHERE\n  -- TODO: condition\n;",
            table_ref, set_line
        )
    }

    fn build_delete_template(&self, ctx: &SchemaTableContext) -> String {
        let table_ref = self.format_table_ref(&ctx.schema, &ctx.table);
        format!("DELETE FROM {}\nWHERE\n  -- TODO: condition\n;", table_ref)
    }

    fn insert_into_editor_and_focus(&mut self, text: &str) {
        self.editor.textarea.insert_str(text);
        self.focus = Focus::Query;
        self.mode = Mode::Insert;
    }

    /// Execute a completed key sequence (action + optional context).
    fn execute_key_sequence_completion(
        &mut self,
        completed: KeySequenceCompletion<SchemaTableContext>,
    ) {
        match completed.action {
            KeySequenceAction::GotoFirst => {
                // Go to first row in grid, or document start in editor
                match self.focus {
                    Focus::Grid => {
                        self.grid_state.cursor_row = 0;
                    }
                    Focus::Query => {
                        // Move to document start
                        self.editor.textarea.move_cursor(CursorMove::Top);
                        self.editor.textarea.move_cursor(CursorMove::Head);
                    }
                    Focus::Sidebar(_) => {
                        // In sidebar, just go to first item (connections section)
                        self.sidebar_focus = SidebarSection::Connections;
                        self.focus = Focus::Sidebar(SidebarSection::Connections);
                        self.sidebar.select_first_connection();
                    }
                }
            }
            KeySequenceAction::GotoEditor => {
                self.focus = Focus::Query;
            }
            KeySequenceAction::GotoConnections => {
                self.sidebar_visible = true;
                self.sidebar_focus = SidebarSection::Connections;
                self.focus = Focus::Sidebar(SidebarSection::Connections);
            }
            KeySequenceAction::GotoTables => {
                self.focus_schema();
            }
            KeySequenceAction::GotoResults => {
                self.focus = Focus::Grid;
            }

            KeySequenceAction::SchemaTableSelect
            | KeySequenceAction::SchemaTableInsert
            | KeySequenceAction::SchemaTableUpdate
            | KeySequenceAction::SchemaTableDelete
            | KeySequenceAction::SchemaTableName => {
                let Some(ctx) = completed.context else {
                    return;
                };

                let sql = match completed.action {
                    KeySequenceAction::SchemaTableSelect => self.build_select_template(&ctx),
                    KeySequenceAction::SchemaTableInsert => self.build_insert_template(&ctx),
                    KeySequenceAction::SchemaTableUpdate => self.build_update_template(&ctx),
                    KeySequenceAction::SchemaTableDelete => self.build_delete_template(&ctx),
                    KeySequenceAction::SchemaTableName => self.format_table_name_only(&ctx.table),
                    _ => return,
                };

                self.insert_into_editor_and_focus(&sql);
            }
        }
    }

    /// Open the connection manager modal.
    fn open_connection_manager(&mut self) {
        // Reload connections from disk to pick up changes from other instances
        if let Ok(connections) = load_connections() {
            self.connections = connections;
        }
        self.connection_manager = Some(ConnectionManagerModal::new(
            &self.connections,
            self.current_connection_name.clone(),
        ));
    }

    /// Handle connection manager actions.
    fn handle_connection_manager_action(&mut self, action: ConnectionManagerAction) {
        match action {
            ConnectionManagerAction::Continue => {}
            ConnectionManagerAction::Close => {
                self.connection_manager = None;
            }
            ConnectionManagerAction::Connect { entry } => {
                self.connection_manager = None;
                if self.editor.is_modified() {
                    self.confirm_prompt = Some(ConfirmPrompt::new(
                        "You have unsaved changes. Switch connection anyway?",
                        ConfirmContext::SwitchConnection { entry },
                    ));
                } else {
                    self.connect_to_entry(entry);
                }
            }
            ConnectionManagerAction::Add => {
                self.connection_form = Some(ConnectionFormModal::with_keymap(
                    self.connection_form_keymap.clone(),
                ));
            }
            ConnectionManagerAction::Edit { entry } => {
                // Try to get existing password for editing
                let password = entry.get_password().ok().flatten();
                self.connection_form = Some(ConnectionFormModal::edit_with_keymap(
                    &entry,
                    password,
                    self.connection_form_keymap.clone(),
                ));
            }
            ConnectionManagerAction::Delete { name } => {
                // Show confirmation for delete
                self.confirm_prompt = Some(ConfirmPrompt::new(
                    format!("Delete connection '{}'?", name),
                    ConfirmContext::DeleteConnection { name },
                ));
            }
            ConnectionManagerAction::SetFavorite { name, current } => {
                // For now, just toggle or cycle favorites
                // TODO: Could show a picker for 1-9
                let new_favorite = match current {
                    Some(f) if f < 9 => Some(f + 1),
                    Some(_) => None, // Was 9, clear it
                    None => Some(1),
                };
                if let Err(e) = self.connections.set_favorite(&name, new_favorite) {
                    self.last_error = Some(format!("Failed to set favorite: {}", e));
                } else {
                    // Save and update the manager
                    if let Err(e) = save_connections(&self.connections) {
                        self.last_error = Some(format!("Failed to save connections: {}", e));
                    }
                    if let Some(ref mut manager) = self.connection_manager {
                        manager.update_connections(&self.connections);
                    }
                }
            }
            ConnectionManagerAction::StatusMessage(msg) => {
                self.last_status = Some(msg);
            }
        }
    }

    /// Handle sidebar actions (from mouse clicks or keyboard).
    fn handle_sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::Connect(name) => {
                // Find the connection entry and connect
                if let Some(entry) = self
                    .connections
                    .sorted()
                    .into_iter()
                    .find(|e| e.name == name)
                {
                    if self.editor.is_modified() {
                        self.confirm_prompt = Some(ConfirmPrompt::new(
                            "You have unsaved changes. Switch connection anyway?",
                            ConfirmContext::SwitchConnection {
                                entry: entry.clone(),
                            },
                        ));
                    } else {
                        self.connect_to_entry(entry.clone());
                    }
                }
            }
            SidebarAction::InsertText(text) => {
                // Insert text into query editor
                self.editor.textarea.insert_str(&text);
                self.focus = Focus::Query;
            }
            SidebarAction::OpenAddConnection => {
                self.connection_form = Some(ConnectionFormModal::with_keymap(
                    self.connection_form_keymap.clone(),
                ));
            }
            SidebarAction::OpenEditConnection(name) => {
                if let Some(entry) = self
                    .connections
                    .sorted()
                    .into_iter()
                    .find(|e| e.name == name)
                {
                    let password = entry.get_password().ok().flatten();
                    self.connection_form = Some(ConnectionFormModal::edit_with_keymap(
                        entry,
                        password,
                        self.connection_form_keymap.clone(),
                    ));
                }
            }
            SidebarAction::RefreshSchema => {
                if self.db.status == DbStatus::Connected {
                    self.schema_cache.loaded = false;
                    self.load_schema();
                }
            }
            SidebarAction::FocusEditor => {
                self.focus = Focus::Query;
            }
        }
    }

    /// Handle connection form actions.
    fn handle_connection_form_action(&mut self, action: ConnectionFormAction) {
        match action {
            ConnectionFormAction::Continue => {}
            ConnectionFormAction::Cancel => {
                self.connection_form = None;
            }
            ConnectionFormAction::Save {
                entry,
                password,
                save_password,
                original_name,
            } => {
                // Handle add vs edit
                let result = if let Some(ref orig) = original_name {
                    self.connections.update(orig, entry.clone())
                } else {
                    self.connections.add(entry.clone())
                };

                match result {
                    Ok(()) => {
                        // Save password to keychain if requested
                        if save_password {
                            if let Some(ref pwd) = password {
                                if let Err(e) = entry.set_password_in_keychain(pwd) {
                                    self.last_error =
                                        Some(format!("Failed to save password: {}", e));
                                }
                            }
                        }

                        // Save connections file
                        if let Err(e) = save_connections(&self.connections) {
                            self.last_error = Some(format!("Failed to save connections: {}", e));
                        } else {
                            self.last_status = Some(format!(
                                "Connection '{}' {}",
                                entry.name,
                                if original_name.is_some() {
                                    "updated"
                                } else {
                                    "added"
                                }
                            ));
                        }

                        // Close form and update manager
                        self.connection_form = None;
                        if let Some(ref mut manager) = self.connection_manager {
                            manager.update_connections(&self.connections);
                        }
                    }
                    Err(e) => {
                        self.last_error = Some(format!("Failed to save connection: {}", e));
                    }
                }
            }
            ConnectionFormAction::TestConnection { entry, password } => {
                // Build URL and test
                let url = entry.to_url(password.as_deref());
                self.last_status = Some(format!("Testing connection to {}...", entry.host));

                let tx = self.db_events_tx.clone();
                self.rt.spawn(async move {
                    match tokio_postgres::connect(&url, NoTls).await {
                        Ok((client, _)) => {
                            drop(client);
                            let _ = tx.send(DbEvent::TestConnectionResult {
                                success: true,
                                message: "Connection successful!".to_string(),
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(DbEvent::TestConnectionResult {
                                success: false,
                                message: format!("Connection failed: {}", format_pg_error(&e)),
                            });
                        }
                    }
                });
            }
            ConnectionFormAction::StatusMessage(msg) => {
                self.last_status = Some(msg);
            }
            ConnectionFormAction::RequestClose => {
                // Show confirmation prompt for unsaved changes
                self.confirm_prompt = Some(ConfirmPrompt::new(
                    "You have unsaved changes. Discard them?",
                    ConfirmContext::CloseConnectionForm,
                ));
            }
        }
    }

    fn load_schema(&mut self) {
        let Some(client) = self.db.client.clone() else {
            return;
        };

        let tx = self.db_events_tx.clone();

        self.rt.spawn(async move {
            let query = r#"
                SELECT
                    n.nspname AS schema_name,
                    c.relname AS table_name,
                    a.attname AS column_name,
                    pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type
                FROM pg_catalog.pg_class c
                JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
                JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
                WHERE c.relkind IN ('r', 'v', 'm')
                    AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
                    AND a.attnum > 0
                    AND NOT a.attisdropped
                ORDER BY n.nspname, c.relname, a.attnum
            "#;

            let guard = client.lock().await;
            match guard.simple_query(query).await {
                Ok(messages) => {
                    drop(guard);
                    let mut tables: Vec<TableInfo> = Vec::new();
                    let mut current_table: Option<(String, String)> = None;
                    let mut current_columns: Vec<ColumnInfo> = Vec::new();

                    for msg in messages {
                        if let SimpleQueryMessage::Row(row) = msg {
                            let schema = row.get(0).unwrap_or("").to_string();
                            let table = row.get(1).unwrap_or("").to_string();
                            let column = row.get(2).unwrap_or("").to_string();
                            let dtype = row.get(3).unwrap_or("").to_string();

                            let key = (schema.clone(), table.clone());

                            if current_table.as_ref() != Some(&key) {
                                if let Some((prev_schema, prev_table)) = current_table.take() {
                                    tables.push(TableInfo {
                                        schema: prev_schema,
                                        name: prev_table,
                                        columns: std::mem::take(&mut current_columns),
                                    });
                                }
                                current_table = Some(key);
                            }

                            current_columns.push(ColumnInfo {
                                name: column,
                                data_type: dtype,
                            });
                        }
                    }

                    // Don't forget the last table
                    if let Some((schema, table)) = current_table {
                        tables.push(TableInfo {
                            schema,
                            name: table,
                            columns: current_columns,
                        });
                    }

                    let _ = tx.send(DbEvent::SchemaLoaded { tables });
                }
                Err(_) => {
                    // Schema loading failed silently - not critical
                }
            }
        });
    }

    fn trigger_completion(&mut self) {
        // Get the current word being typed
        let (row, col) = self.editor.textarea.cursor();
        let lines = self.editor.textarea.lines();

        if row >= lines.len() {
            return;
        }

        let line = &lines[row];
        let (prefix, start_col) = get_word_before_cursor(line, col);

        // Determine completion context
        let full_text = self.editor.text();
        // Calculate approximate position in full text
        let pos_in_text: usize = lines.iter().take(row).map(|l| l.len() + 1).sum::<usize>() + col;
        let context = determine_context(&full_text, pos_in_text);

        // Get completion items based on context
        let items = self.schema_cache.get_completion_items(context);

        if items.is_empty() {
            self.last_status = Some("No completions available".to_string());
            return;
        }

        self.completion.open(items, prefix, start_col);
    }

    fn apply_completion(&mut self) {
        if let Some(item) = self.completion.selected_item() {
            let label = item.label.clone();
            let start_col = self.completion.start_col;
            let (_, col) = self.editor.textarea.cursor();

            // Delete the prefix
            let chars_to_delete = col - start_col;
            for _ in 0..chars_to_delete {
                self.editor.textarea.delete_char();
            }

            // Insert the completion
            self.editor.textarea.insert_str(&label);

            self.completion.close();
        }
    }

    fn execute_query(&mut self) {
        let query = self.editor.text();
        if query.trim().is_empty() {
            self.last_status = Some("No query to run".to_string());
            return;
        }

        // Push to both editor history (for Ctrl-p/n navigation) and persistent history.
        self.editor.push_history(query.clone());
        let conn_info = self
            .db
            .conn_str
            .as_ref()
            .map(|s| ConnectionInfo::parse(s).format(50));
        self.history.push(query.clone(), conn_info);

        let Some(client) = self.db.client.clone() else {
            self.last_error =
                Some("Not connected. Use :connect <url> or set DATABASE_URL.".to_string());
            return;
        };

        if self.db.running {
            self.last_status = Some("Query already running".to_string());
            return;
        }

        self.db.running = true;
        self.last_status = Some("Running...".to_string());
        self.query_start_time = Some(Instant::now());

        let tx = self.db_events_tx.clone();
        let started = Instant::now();
        let source_table = extract_table_from_query(&query);

        self.rt.spawn(async move {
            const MAX_ROWS: usize = 2000;

            let guard = client.lock().await;
            match guard.simple_query(&query).await {
                Ok(messages) => {
                    drop(guard);
                    let elapsed = started.elapsed();

                    let mut current_headers: Option<Vec<String>> = None;
                    let mut current_rows: Vec<Vec<String>> = Vec::new();
                    let mut last_headers: Vec<String> = Vec::new();
                    let mut last_rows: Vec<Vec<String>> = Vec::new();
                    let mut last_cmd: Option<String> = None;
                    let mut truncated = false;

                    for msg in messages {
                        match msg {
                            SimpleQueryMessage::Row(row) => {
                                if current_headers.is_none() {
                                    current_headers = Some(
                                        row.columns()
                                            .iter()
                                            .map(|c| c.name().to_string())
                                            .collect(),
                                    );
                                }

                                if current_rows.len() < MAX_ROWS {
                                    let mut out_row = Vec::with_capacity(row.len());
                                    for i in 0..row.len() {
                                        out_row.push(row.get(i).unwrap_or("NULL").to_string());
                                    }
                                    current_rows.push(out_row);
                                } else {
                                    truncated = true;
                                }
                            }
                            SimpleQueryMessage::CommandComplete(rows_affected) => {
                                last_cmd = Some(format!("{} rows", rows_affected));

                                if let Some(h) = current_headers.take() {
                                    last_headers = h;
                                    last_rows = std::mem::take(&mut current_rows);
                                } else {
                                    current_rows.clear();
                                }
                            }
                            SimpleQueryMessage::RowDescription(_) => {
                                // We get headers from the Row itself.
                            }
                            _ => {
                                // Catch any future variants; do nothing.
                            }
                        }
                    }

                    if let Some(h) = current_headers.take() {
                        last_headers = h;
                        last_rows = current_rows;
                    }

                    let (headers, rows) = if last_headers.is_empty() {
                        let status = last_cmd.clone().unwrap_or_else(|| "OK".to_string());
                        (vec!["status".to_string()], vec![vec![status]])
                    } else {
                        (last_headers, last_rows)
                    };

                    // Fetch column types if we have a source table
                    let col_types = if let Some(ref table) = source_table {
                        let type_map = fetch_column_types(&client, table).await;
                        headers
                            .iter()
                            .map(|h| type_map.get(h).cloned().unwrap_or_default())
                            .collect()
                    } else {
                        vec![String::new(); headers.len()]
                    };

                    // Fetch primary keys if we have a source table
                    let primary_keys = if let Some(ref table) = source_table {
                        fetch_primary_keys(&client, table).await
                    } else {
                        Vec::new()
                    };

                    let result = QueryResult {
                        headers,
                        rows,
                        command_tag: last_cmd,
                        truncated,
                        elapsed,
                        source_table,
                        primary_keys,
                        col_types,
                    };

                    let _ = tx.send(DbEvent::QueryFinished { result });
                }
                Err(e) => {
                    let _ = tx.send(DbEvent::QueryError {
                        error: format_pg_error(&e),
                    });
                }
            }
        });
    }

    fn cancel_query(&mut self) {
        if !self.db.running {
            return;
        }

        let Some(token) = self.db.cancel_token.clone() else {
            self.last_status = Some("No cancel token available".to_string());
            return;
        };

        self.last_status = Some("Cancelling...".to_string());

        let tx = self.db_events_tx.clone();

        self.rt.spawn(async move {
            // Attempt to cancel. We ignore errors since cancellation is best-effort.
            let _ = token.cancel_query(NoTls).await;
            // The query task will return an error which we handle normally.
            // We also send a cancelled event in case the query finished before the cancel arrived.
            let _ = tx.send(DbEvent::QueryCancelled);
        });
    }

    fn drain_db_events(&mut self) {
        while let Ok(ev) = self.db_events_rx.try_recv() {
            self.apply_db_event(ev);
        }
    }

    fn apply_db_event(&mut self, ev: DbEvent) {
        match ev {
            DbEvent::Connected {
                client,
                cancel_token,
            } => {
                self.db.status = DbStatus::Connected;
                self.db.client = Some(client);
                self.db.cancel_token = Some(cancel_token);
                self.db.running = false;
                self.last_status = Some("Connected, loading schema...".to_string());
                // Load schema for completion
                self.load_schema();
            }
            DbEvent::ConnectError { error } => {
                self.db.status = DbStatus::Error;
                self.db.client = None;
                self.db.running = false;
                self.current_connection_name = None;
                self.last_status = Some("Connect failed (see error)".to_string());
                self.last_error = Some(format!("Connection error: {}", error));
            }
            DbEvent::ConnectionLost { error } => {
                self.db.status = DbStatus::Error;
                self.db.client = None;
                self.db.running = false;
                self.current_connection_name = None;
                self.last_status = Some("Connection lost (see error)".to_string());
                self.last_error = Some(format!("Connection lost: {}", error));
            }
            DbEvent::QueryFinished { result } => {
                self.db.running = false;
                self.query_start_time = None;
                self.db.last_command_tag = result.command_tag.clone();
                self.db.last_elapsed = Some(result.elapsed);
                self.last_error = None; // Clear any previous error.

                // Track transaction state based on command tag
                if let Some(ref tag) = result.command_tag {
                    let tag_upper = tag.to_uppercase();
                    if tag_upper.starts_with("BEGIN") {
                        self.db.in_transaction = true;
                    } else if tag_upper.starts_with("COMMIT")
                        || tag_upper.starts_with("ROLLBACK")
                        || tag_upper.starts_with("END")
                    {
                        self.db.in_transaction = false;
                    }
                }

                self.grid = GridModel::new(result.headers, result.rows)
                    .with_source_table(result.source_table)
                    .with_primary_keys(result.primary_keys)
                    .with_col_types(result.col_types);
                self.grid_state = GridState::default();

                // Move focus to grid to show results
                self.focus = Focus::Grid;

                // Mark the query as "saved" since it was successfully executed
                self.editor.mark_saved();

                let mut msg = String::new();
                if let Some(tag) = result.command_tag {
                    msg.push_str(&tag);
                } else {
                    msg.push_str("Query complete");
                }
                msg.push_str(&format!(" ({} ms)", result.elapsed.as_millis()));
                if result.truncated {
                    msg.push_str(" [truncated]");
                }
                self.last_status = Some(msg);
            }
            DbEvent::QueryError { error } => {
                self.db.running = false;
                self.query_start_time = None;
                self.last_status = Some("Query error (see above)".to_string());
                self.last_error = Some(error);
            }
            DbEvent::QueryCancelled => {
                self.db.running = false;
                self.query_start_time = None;
                self.last_status = Some("Query cancelled".to_string());
            }
            DbEvent::SchemaLoaded { tables } => {
                self.schema_cache.tables = tables;
                self.schema_cache.loaded = true;
                // Apply any pending schema expanded state from session restore
                self.apply_pending_schema_expanded();
                self.last_status = Some(format!(
                    "Schema loaded: {} tables",
                    self.schema_cache.tables.len()
                ));
            }
            DbEvent::CellUpdated { row, col, value } => {
                self.db.running = false;
                self.query_start_time = None;
                // Update the grid cell
                if let Some(grid_row) = self.grid.rows.get_mut(row) {
                    if let Some(cell) = grid_row.get_mut(col) {
                        *cell = value;
                    }
                }
                self.last_status = Some("Cell updated successfully".to_string());
            }
            DbEvent::TestConnectionResult { success, message } => {
                if success {
                    self.last_status = Some(message);
                    self.last_error = None;
                } else {
                    self.last_error = Some(message);
                }
            }
        }
    }

    fn status_line(&self, width: u16) -> Paragraph<'static> {
        let row_count = self.grid.rows.len();
        let selected_count = self.grid_state.selected_rows.len();
        let cursor_row = if row_count == 0 {
            0
        } else {
            self.grid_state.cursor_row.saturating_add(1)
        };

        // Mode indicator with color
        let (mode_text, mode_style) = match self.mode {
            Mode::Normal => ("NORMAL", Style::default().fg(Color::Cyan)),
            Mode::Insert => ("INSERT", Style::default().fg(Color::Green)),
            Mode::Visual => ("VISUAL", Style::default().fg(Color::Yellow)),
        };

        // Connection info
        let conn_segment = if self.db.status == DbStatus::Connected {
            if let Some(ref conn_str) = self.db.conn_str {
                let info = ConnectionInfo::parse(conn_str);
                // Allow up to 30 chars for connection, will be auto-truncated if needed
                info.format(30)
            } else {
                "connected".to_string()
            }
        } else if self.db.status == DbStatus::Connecting {
            "connecting...".to_string()
        } else if self.db.status == DbStatus::Error {
            "error".to_string()
        } else {
            "disconnected".to_string()
        };

        let conn_style = match self.db.status {
            DbStatus::Connected => Style::default().fg(Color::Green),
            DbStatus::Connecting => Style::default().fg(Color::Yellow),
            DbStatus::Error => Style::default().fg(Color::Red),
            DbStatus::Disconnected => Style::default().fg(Color::DarkGray),
        };

        // Row info
        let row_info = format!("Row {}/{}", cursor_row, row_count);

        // Selection info (only if selected)
        let selection_info = if selected_count > 0 {
            Some(format!("{} sel", selected_count))
        } else {
            None
        };

        // Query timing info
        let timing_info = if let Some(ref tag) = self.db.last_command_tag {
            let time_part = self
                .db
                .last_elapsed
                .map(|e| format!(" ({}ms)", e.as_millis()))
                .unwrap_or_default();
            Some(format!("{}{}", tag, time_part))
        } else {
            None
        };

        // Running indicator
        let running_indicator = if self.db.running {
            Some("⏳ running")
        } else {
            None
        };

        // Status message (right-aligned)
        let status = self.last_status.as_deref().unwrap_or("Ready").to_string();
        let status_style = if self.last_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Build status line with priority-based segments
        let line = StatusLineBuilder::new()
            // Critical: Mode (always shown)
            .segment(StatusSegment::new(mode_text, Priority::Critical).style(mode_style))
            // Critical: Connection info
            .segment(
                StatusSegment::new(conn_segment, Priority::Critical)
                    .style(conn_style)
                    .min_width(40),
            )
            // Critical: Running indicator (if running) - always visible
            .segment_if(
                running_indicator.is_some(),
                StatusSegment::new(running_indicator.unwrap_or_default(), Priority::Critical)
                    .style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
            )
            // High: Transaction indicator (if in transaction)
            .segment_if(
                self.db.in_transaction,
                StatusSegment::new("TXN", Priority::High)
                    .style(Style::default().fg(Color::Magenta)),
            )
            // Medium: Row info
            .segment(StatusSegment::new(row_info, Priority::Medium).min_width(50))
            // Medium: Selection (if any selected)
            .segment_if(
                selection_info.is_some(),
                StatusSegment::new(selection_info.unwrap_or_default(), Priority::Medium)
                    .style(Style::default().fg(Color::Cyan))
                    .min_width(60),
            )
            // Low: Query timing
            .segment_if(
                timing_info.is_some(),
                StatusSegment::new(timing_info.unwrap_or_default(), Priority::Low)
                    .style(Style::default().fg(Color::DarkGray))
                    .min_width(80),
            )
            // Right-aligned: Status message
            .segment(
                StatusSegment::new(status, Priority::Critical)
                    .style(status_style)
                    .right_align(),
            )
            .build(width);

        Paragraph::new(line)
    }

    /// Open the history fuzzy picker.
    fn open_history_picker(&mut self) {
        if self.history.is_empty() {
            self.last_status = Some("No history yet".to_string());
            return;
        }

        // Create picker with history entries.
        let entries: Vec<HistoryEntry> = self.history.entries().to_vec();
        let picker = FuzzyPicker::with_display(
            entries,
            format!("History (Ctrl-R) - {} queries", self.history.len()),
            |entry| entry.query.clone(),
        );

        self.history_picker = Some(picker);
    }

    /// Handle key events when history picker is open.
    fn handle_history_picker_key(&mut self, key: KeyEvent) -> bool {
        let picker = match self.history_picker.as_mut() {
            Some(p) => p,
            None => return false,
        };

        match picker.handle_key(key) {
            PickerAction::Continue => false,
            PickerAction::Selected(entry) => {
                // Load selected query into editor.
                self.editor.set_text(entry.query);
                self.editor.mark_saved(); // Mark as unmodified since it's loaded content
                self.history_picker = None;
                self.last_status = Some("Loaded from history".to_string());
                false
            }
            PickerAction::Cancelled => {
                self.history_picker = None;
                false
            }
        }
    }
}

/// Calculate the scroll offset needed to keep cursor visible in the editor viewport.
fn calculate_editor_scroll(
    cursor_row: usize,
    cursor_col: usize,
    current_scroll: (u16, u16),
    viewport_height: usize,
    viewport_width: usize,
) -> (u16, u16) {
    let (mut scroll_row, mut scroll_col) = (current_scroll.0 as usize, current_scroll.1 as usize);

    // Vertical scrolling
    if viewport_height > 0 {
        // If cursor is above the viewport, scroll up
        if cursor_row < scroll_row {
            scroll_row = cursor_row;
        }
        // If cursor is below the viewport, scroll down
        let viewport_bottom = scroll_row + viewport_height;
        if cursor_row >= viewport_bottom {
            scroll_row = cursor_row.saturating_sub(viewport_height - 1);
        }
    }

    // Horizontal scrolling
    if viewport_width > 0 {
        // Leave some margin (3 chars) for context
        let margin = 3.min(viewport_width / 4);

        // If cursor is left of the viewport, scroll left
        if cursor_col < scroll_col + margin {
            scroll_col = cursor_col.saturating_sub(margin);
        }
        // If cursor is right of the viewport, scroll right
        let viewport_right = scroll_col + viewport_width;
        if cursor_col + margin >= viewport_right {
            scroll_col = (cursor_col + margin).saturating_sub(viewport_width - 1);
        }
    }

    (scroll_row as u16, scroll_col as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Guard that sets TSQL_CONFIG_DIR to a temp directory for test isolation.
    /// Automatically cleans up when dropped (even on panic).
    struct ConfigDirGuard {
        _temp_dir: tempfile::TempDir,
    }

    impl ConfigDirGuard {
        fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            std::env::set_var("TSQL_CONFIG_DIR", temp_dir.path());
            Self {
                _temp_dir: temp_dir,
            }
        }
    }

    impl Drop for ConfigDirGuard {
        fn drop(&mut self) {
            std::env::remove_var("TSQL_CONFIG_DIR");
        }
    }

    // ========== CellEditor Tests ==========

    #[test]
    fn test_cell_editor_open_sets_cursor_at_end() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        assert!(editor.active);
        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 5); // Cursor at end
        assert_eq!(editor.original_value, "hello");
    }

    #[test]
    fn test_cell_editor_not_modified_initially() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        assert!(
            !editor.is_modified(),
            "Editor should not be modified when just opened"
        );
    }

    #[test]
    fn test_cell_editor_modified_after_change() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        editor.insert_char('!');

        assert!(
            editor.is_modified(),
            "Editor should be modified after inserting a character"
        );
    }

    #[test]
    fn test_cell_editor_not_modified_when_restored() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        editor.insert_char('!');
        editor.delete_char_before(); // Remove the '!' we just added

        assert!(
            !editor.is_modified(),
            "Editor should not be modified when content matches original"
        );
    }

    #[test]
    fn test_cell_editor_not_modified_when_inactive() {
        let editor = CellEditor::new();

        assert!(
            !editor.is_modified(),
            "Inactive editor should not be considered modified"
        );
    }

    #[test]
    fn test_cell_editor_insert_char_at_cursor() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hllo".to_string());
        editor.cursor = 1; // Position after 'h'

        editor.insert_char('e');

        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 2); // Cursor moved after inserted char
    }

    #[test]
    fn test_cell_editor_insert_char_at_end() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hell".to_string());

        editor.insert_char('o');

        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn test_cell_editor_delete_char_before() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        editor.delete_char_before(); // Delete 'o'

        assert_eq!(editor.value, "hell");
        assert_eq!(editor.cursor, 4);
    }

    #[test]
    fn test_cell_editor_delete_char_before_in_middle() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 2; // After 'he'

        editor.delete_char_before(); // Delete 'e'

        assert_eq!(editor.value, "hllo");
        assert_eq!(editor.cursor, 1);
    }

    #[test]
    fn test_cell_editor_delete_char_before_at_start_does_nothing() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 0;

        editor.delete_char_before();

        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn test_cell_editor_delete_char_at() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 0;

        editor.delete_char_at(); // Delete 'h'

        assert_eq!(editor.value, "ello");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn test_cell_editor_delete_char_at_end_does_nothing() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        // cursor is at end (5)

        editor.delete_char_at();

        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn test_cell_editor_move_left() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        editor.move_left();
        assert_eq!(editor.cursor, 4);

        editor.move_left();
        assert_eq!(editor.cursor, 3);
    }

    #[test]
    fn test_cell_editor_move_left_at_start_stays() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 0;

        editor.move_left();
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn test_cell_editor_move_right() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 0;

        editor.move_right();
        assert_eq!(editor.cursor, 1);

        editor.move_right();
        assert_eq!(editor.cursor, 2);
    }

    #[test]
    fn test_cell_editor_move_right_at_end_stays() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        // cursor at end (5)

        editor.move_right();
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn test_cell_editor_move_to_start_end() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.cursor = 3;

        editor.move_to_start();
        assert_eq!(editor.cursor, 0);

        editor.move_to_end();
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn test_cell_editor_clear() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        editor.clear();

        assert_eq!(editor.value, "");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn test_cell_editor_delete_to_end() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello world".to_string());
        editor.cursor = 5; // After "hello"

        editor.delete_to_end();

        assert_eq!(editor.value, "hello");
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn test_cell_editor_delete_to_start() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello world".to_string());
        editor.cursor = 6; // After "hello "

        editor.delete_to_start();

        assert_eq!(editor.value, "world");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn test_cell_editor_unicode_handling() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "héllo".to_string()); // 'é' is 2 bytes

        assert_eq!(editor.cursor, 6); // 6 bytes total

        editor.move_left();
        assert_eq!(editor.cursor, 5); // Before 'o'

        editor.move_left();
        assert_eq!(editor.cursor, 4); // Before 'l'

        editor.delete_char_before();
        assert_eq!(editor.value, "hélo");
    }

    #[test]
    fn test_cell_editor_visible_text_short_string() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());

        let (visible, cursor_pos) = editor.visible_text(20);

        assert_eq!(visible, "hello");
        assert_eq!(cursor_pos, 5); // Cursor at end
    }

    #[test]
    fn test_cell_editor_visible_text_long_string_cursor_at_end() {
        let mut editor = CellEditor::new();
        let long_text = "This is a very long string that exceeds the width";
        editor.open(0, 0, long_text.to_string());

        // Width of 20, cursor at end (49 chars)
        editor.update_scroll(20);
        let (visible, cursor_pos) = editor.visible_text(20);

        // Should show the end of the string
        assert!(visible.len() <= 19); // Leave room for cursor
        assert!(cursor_pos <= 19);
    }

    #[test]
    fn test_cell_editor_visible_text_long_string_cursor_at_start() {
        let mut editor = CellEditor::new();
        let long_text = "This is a very long string that exceeds the width";
        editor.open(0, 0, long_text.to_string());
        editor.cursor = 0;

        editor.update_scroll(20);
        let (visible, cursor_pos) = editor.visible_text(20);

        // Should show the start of the string
        assert!(visible.starts_with("This"));
        assert_eq!(cursor_pos, 0);
    }

    #[test]
    fn test_cell_editor_visible_text_cursor_in_middle() {
        let mut editor = CellEditor::new();
        let long_text = "This is a very long string that exceeds the width";
        editor.open(0, 0, long_text.to_string());
        editor.cursor = 20; // Middle of string

        editor.update_scroll(20);
        let (_visible, cursor_pos) = editor.visible_text(20);

        // Cursor should be visible within the window
        assert!(cursor_pos < 20);
    }

    #[test]
    fn test_cell_editor_scroll_follows_cursor() {
        let mut editor = CellEditor::new();
        let long_text = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        editor.open(0, 0, long_text.to_string());
        editor.cursor = 0;
        editor.scroll_offset = 0;

        // Move cursor to end character by character
        for _ in 0..36 {
            editor.move_right();
            editor.update_scroll(10);
            let (_, cursor_pos) = editor.visible_text(10);
            // Cursor should always be visible (within window)
            assert!(
                cursor_pos < 10,
                "Cursor should be visible, got pos {}",
                cursor_pos
            );
        }
    }

    #[test]
    fn test_cell_editor_close_resets_all_state() {
        let mut editor = CellEditor::new();
        editor.open(0, 0, "hello".to_string());
        editor.scroll_offset = 10;

        editor.close();

        assert!(!editor.active);
        assert_eq!(editor.value, "");
        assert_eq!(editor.original_value, "");
        assert_eq!(editor.cursor, 0);
        assert_eq!(editor.scroll_offset, 0);
    }

    // ========== App Tests ==========

    #[test]
    fn test_query_finished_moves_focus_to_grid() {
        // Create a minimal App for testing
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Initially focus should be on Query
        assert_eq!(app.focus, Focus::Query, "Initial focus should be Query");

        // Simulate a query finishing with results
        let result = QueryResult {
            headers: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec!["1".to_string(), "Alice".to_string()]],
            command_tag: Some("SELECT 1".to_string()),
            truncated: false,
            elapsed: Duration::from_millis(10),
            source_table: Some("users".to_string()),
            primary_keys: vec!["id".to_string()],
            col_types: vec!["int4".to_string(), "text".to_string()],
        };

        app.apply_db_event(DbEvent::QueryFinished { result });

        // Focus should now be on Grid
        assert_eq!(
            app.focus,
            Focus::Grid,
            "Focus should move to Grid after query finishes"
        );
    }

    #[test]
    fn test_extract_table_from_simple_select() {
        assert_eq!(
            extract_table_from_query("SELECT * FROM users"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_query("select id, name from users"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_query("SELECT * FROM public.users"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_query("SELECT * FROM users WHERE id = 1"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_query("SELECT * FROM users;"),
            Some("users".to_string())
        );
    }

    #[test]
    fn test_extract_table_returns_none_for_complex_queries() {
        // JOINs
        assert_eq!(
            extract_table_from_query(
                "SELECT * FROM users JOIN orders ON users.id = orders.user_id"
            ),
            None
        );
        // Subqueries
        assert_eq!(
            extract_table_from_query("SELECT * FROM (SELECT * FROM users) AS u"),
            None
        );
        // Non-SELECT
        assert_eq!(
            extract_table_from_query("INSERT INTO users VALUES (1, 'Alice')"),
            None
        );
        assert_eq!(
            extract_table_from_query("UPDATE users SET name = 'Bob'"),
            None
        );
    }

    // ========== Config Integration Tests ==========

    #[test]
    fn test_app_uses_default_keymaps() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Verify default grid keymap has vim keys
        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.grid_keymap.get(&j), Some(&Action::MoveDown));

        let k = KeyBinding::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.grid_keymap.get(&k), Some(&Action::MoveUp));
    }

    #[test]
    fn test_app_with_custom_config_keybindings() {
        use crate::config::CustomKeyBinding;

        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Create config with custom grid keybinding
        let mut config = Config::default();
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+j".to_string(),
            action: "page_down".to_string(),
            description: Some("Page down with Ctrl+J".to_string()),
        });

        let app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            config,
        );

        // Verify custom keybinding was added
        let ctrl_j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_j), Some(&Action::PageDown));

        // Verify default bindings still work
        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.grid_keymap.get(&j), Some(&Action::MoveDown));
    }

    #[test]
    fn test_app_custom_config_overrides_default() {
        use crate::config::CustomKeyBinding;

        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Create config that overrides j to do page_down instead of move_down
        let mut config = Config::default();
        config.keymap.grid.push(CustomKeyBinding {
            key: "j".to_string(),
            action: "page_down".to_string(),
            description: None,
        });

        let app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            config,
        );

        // Verify j was overridden
        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.grid_keymap.get(&j), Some(&Action::PageDown));
    }

    #[test]
    fn test_build_keymap_ignores_invalid_bindings() {
        use crate::config::CustomKeyBinding;

        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Create config with invalid keybindings
        let mut config = Config::default();
        config.keymap.grid.push(CustomKeyBinding {
            key: "invalid_key_combo!!!".to_string(),
            action: "move_down".to_string(),
            description: None,
        });
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+x".to_string(),
            action: "invalid_action_name".to_string(),
            description: None,
        });

        // This should not panic - invalid bindings are silently ignored
        let app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            config,
        );

        // Default bindings should still work
        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.grid_keymap.get(&j), Some(&Action::MoveDown));
    }

    // ========== Global Ctrl+E (Execute Query) Tests ==========

    #[test]
    fn test_ctrl_e_binding_exists_in_editor_normal_keymap() {
        // Verify Ctrl+E is bound to ExecuteQuery in editor normal keymap
        let keymap = crate::config::Keymap::default_editor_normal_keymap();
        let ctrl_e = KeyBinding::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(
            keymap.get(&ctrl_e),
            Some(&Action::ExecuteQuery),
            "Ctrl+E should be bound to ExecuteQuery in normal mode"
        );
    }

    #[test]
    fn test_ctrl_e_binding_exists_in_editor_insert_keymap() {
        // Verify Ctrl+E is bound to ExecuteQuery in editor insert keymap
        let keymap = crate::config::Keymap::default_editor_insert_keymap();
        let ctrl_e = KeyBinding::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(
            keymap.get(&ctrl_e),
            Some(&Action::ExecuteQuery),
            "Ctrl+E should be bound to ExecuteQuery in insert mode"
        );
    }

    #[test]
    fn test_global_ctrl_e_key_detection() {
        // Test that Ctrl+E is correctly detected as the key combination
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(key.code, KeyCode::Char('e'));
        assert_eq!(key.modifiers, KeyModifiers::CONTROL);

        // Verify it's different from plain 'e'
        let plain_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_ne!(key.modifiers, plain_e.modifiers);
    }

    #[test]
    fn test_ctrl_e_executes_query_when_grid_focused() {
        // Create a minimal App for testing
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Close connection picker/manager that auto-opens when no connection provided
        app.connection_picker = None;
        app.connection_manager = None;

        // Set focus to Grid and mode to Normal
        app.focus = Focus::Grid;
        app.mode = Mode::Normal;

        // Put some text in the editor so we have a query to execute
        app.editor.set_text("SELECT 1".to_string());

        // Press Ctrl+E
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        let quit = app.on_key(key);

        // Should not quit
        assert!(!quit, "Ctrl+E should not quit");

        // Since we have no DB connection, execute_query will set last_error
        // But if EditCell was triggered instead, last_status would be different
        // Check that execute_query was attempted (shows error about no connection)
        assert!(
            app.last_error.is_some()
                || app.last_status == Some("No query to run".to_string())
                || app.last_status == Some("Running...".to_string()),
            "Ctrl+E should attempt to execute query, not edit cell. last_error={:?}, last_status={:?}",
            app.last_error,
            app.last_status
        );
    }

    #[test]
    fn test_ctrl_e_does_not_open_cell_editor_on_grid() {
        // Create a minimal App for testing with some grid data
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let grid = GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![vec!["1".to_string(), "Alice".to_string()]],
        )
        .with_source_table(Some("users".to_string()));

        let mut app = App::new(grid, rt.handle().clone(), tx, rx, None);

        // Close connection manager that auto-opens when no connection provided
        app.connection_manager = None;

        // Set focus to Grid and mode to Normal
        app.focus = Focus::Grid;
        app.mode = Mode::Normal;

        // Put some text in the editor
        app.editor.set_text("SELECT 1".to_string());

        // Press Ctrl+E
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        let _quit = app.on_key(key);

        // Cell editor should NOT be opened
        assert!(
            !app.cell_editor.active,
            "Ctrl+E should not open cell editor, but it did"
        );
    }

    // ========== Connection Manager Issue Tests ==========

    /// Helper to type a string into the app by simulating key presses
    fn type_string(app: &mut App, s: &str) {
        for c in s.chars() {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            app.on_key(key);
        }
    }

    /// Issue 2: After adding a connection, pressing 'a' should open a new form
    #[test]
    #[serial]
    fn test_pressing_a_after_saving_connection_opens_new_form() {
        let _guard = ConfigDirGuard::new(); // Isolate config to temp directory
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing connections and pickers to set up clean state
        app.connections = ConnectionsFile::new();
        app.connection_picker = None;
        app.connection_manager = None;

        // Open connection manager (since we have no connections)
        app.open_connection_manager();

        // App should now have connection manager open
        assert!(
            app.connection_manager.is_some(),
            "Connection manager should be open"
        );
        assert!(
            app.connection_form.is_none(),
            "Connection form should not be open initially"
        );

        // Step 1: Press 'a' to open add form
        let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        app.on_key(key_a);

        assert!(
            app.connection_form.is_some(),
            "Connection form should open after pressing 'a'"
        );

        // Step 2: Fill in the form by typing (form starts focused on Name field)
        // Use a unique name based on test timestamp
        type_string(&mut app, "testconn_unique_12345");

        // Tab to User field
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        type_string(&mut app, "postgres");

        // Tab to Password, then SavePassword, then Host
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // Password
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // SavePassword
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // Host (already localhost)

        // Tab to Port (already 5432), then Database
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // Port
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // Database
        type_string(&mut app, "testdb");

        // Step 3: Press Ctrl+S to save
        let key_ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        app.on_key(key_ctrl_s);

        // After save, form should be closed
        assert!(
            app.connection_form.is_none(),
            "Connection form should close after successful save. last_error={:?}, last_status={:?}",
            app.last_error,
            app.last_status
        );

        // Connection manager should still be open
        assert!(
            app.connection_manager.is_some(),
            "Connection manager should still be open after save"
        );

        // Step 4: Press 'a' again - this is the issue: should open a new form
        app.on_key(key_a);

        assert!(
            app.connection_form.is_some(),
            "Connection form should open when pressing 'a' after saving a connection"
        );
    }

    /// Issue: Ctrl+S requires two presses when EDITING a connection
    #[test]
    #[serial]
    fn test_ctrl_s_works_first_press_when_editing_connection() {
        let _guard = ConfigDirGuard::new(); // Isolate config to temp directory
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any pickers
        app.connection_picker = None;
        app.connection_manager = None;

        // Add a connection to edit
        let entry = ConnectionEntry {
            name: "editme".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };
        app.connections = ConnectionsFile::new();
        app.connections.add(entry.clone()).unwrap();

        // Create manager directly without reload from disk
        app.connection_manager = Some(ConnectionManagerModal::new(
            &app.connections,
            app.current_connection_name.clone(),
        ));

        // Press 'e' to edit the selected connection
        let key_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        app.on_key(key_e);

        assert!(
            app.connection_form.is_some(),
            "Connection form should open after pressing 'e'"
        );

        // Make a change - add something to the name
        // First, go to end of name field and add a character
        app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        type_string(&mut app, "2");

        // Now press Ctrl+S ONCE - should save
        let key_ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        app.on_key(key_ctrl_s);

        // Form should be closed after first Ctrl+S
        assert!(
            app.connection_form.is_none(),
            "Connection form should close after first Ctrl+S. last_error={:?}, last_status={:?}",
            app.last_error,
            app.last_status
        );

        // Verify the connection was updated
        assert!(
            app.connections.find_by_name("editme2").is_some(),
            "Connection should be renamed to 'editme2'"
        );
    }

    /// Issue 3: Esc in connection manager should close it in one press
    #[test]
    #[serial]
    fn test_esc_closes_connection_manager_single_press() {
        let _guard = ConfigDirGuard::new(); // Isolate config to temp directory
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing state and explicitly open the manager
        app.connection_picker = None;
        app.connection_manager = None;
        app.open_connection_manager();

        // Connection manager is open
        assert!(app.connection_manager.is_some());

        // Press Esc once
        let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.on_key(key_esc);

        // Connection manager should be closed
        assert!(
            app.connection_manager.is_none(),
            "Connection manager should close with single Esc press"
        );
    }

    /// Issue 3: Esc on connection form with unsaved changes shows confirmation
    #[test]
    #[serial]
    fn test_esc_on_modified_form_shows_confirmation() {
        let _guard = ConfigDirGuard::new(); // Isolate config to temp directory
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing state and open the manager
        app.connection_picker = None;
        app.connection_manager = None;
        app.open_connection_manager();

        // Open the add form
        let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        app.on_key(key_a);

        // Modify the form by typing
        type_string(&mut app, "testconn");

        // Press Esc
        let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.on_key(key_esc);

        // Confirmation prompt should appear
        assert!(
            app.confirm_prompt.is_some(),
            "Confirmation prompt should appear when Esc on modified form"
        );

        // Form should still be open (waiting for confirmation)
        assert!(
            app.connection_form.is_some(),
            "Form should still be open while confirmation is pending"
        );

        // Press 'y' to confirm discard
        let key_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        app.on_key(key_y);

        // Now form should be closed
        assert!(
            app.connection_form.is_none(),
            "Form should close after confirming discard"
        );
    }

    /// Test: Enter in connection picker should select connection, not execute query
    #[test]
    fn test_enter_in_connection_picker_selects_connection() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing state
        app.connection_picker = None;
        app.connection_manager = None;
        app.last_error = None;

        // Add a test connection
        let entry = ConnectionEntry {
            name: "testconn".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };
        app.connections = ConnectionsFile::new();
        app.connections.add(entry.clone()).unwrap();

        // Manually create the picker without reloading from disk
        let entries: Vec<ConnectionEntry> = app.connections.sorted().into_iter().cloned().collect();
        app.connection_picker = Some(FuzzyPicker::with_display(entries, "Connect", |entry| {
            entry.name.clone()
        }));

        assert!(
            app.connection_picker.is_some(),
            "Connection picker should be open"
        );

        // Verify there's a connection to select
        let picker = app.connection_picker.as_ref().unwrap();
        assert!(
            picker.filtered_count() > 0,
            "Connection picker should have at least one connection"
        );

        // Press Enter to select the connection
        let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.on_key(key_enter);

        // Connection picker should be closed after Enter
        assert!(
            app.connection_picker.is_none(),
            "Connection picker should close after Enter"
        );

        // Should have started connecting (connection name should be set)
        assert_eq!(
            app.current_connection_name,
            Some("testconn".to_string()),
            "Should have set current_connection_name after selecting connection"
        );
    }

    /// Test: When error is shown AND connection picker is open, Enter should select connection
    /// not just dismiss the error (bug fix)
    #[test]
    fn test_enter_with_error_and_picker_should_select_connection() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing state
        app.connection_picker = None;
        app.connection_manager = None;

        // Add a test connection
        let entry = ConnectionEntry {
            name: "testconn".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };
        app.connections = ConnectionsFile::new();
        app.connections.add(entry.clone()).unwrap();

        // Manually create the picker
        let entries: Vec<ConnectionEntry> = app.connections.sorted().into_iter().cloned().collect();
        app.connection_picker = Some(FuzzyPicker::with_display(entries, "Connect", |entry| {
            entry.name.clone()
        }));

        // ALSO set an error (simulating "Unknown connection" scenario)
        app.last_error = Some("Unknown connection: badname".to_string());

        assert!(app.connection_picker.is_some(), "Picker should be open");
        assert!(app.last_error.is_some(), "Error should be shown");

        // Press Enter ONCE - should select connection AND dismiss error
        let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        app.on_key(key_enter);

        // Both should be resolved in one Enter press
        assert!(
            app.connection_picker.is_none(),
            "Connection picker should close after Enter (was: {:?})",
            app.connection_picker.is_some()
        );
        assert!(
            app.last_error.is_none(),
            "Error should be dismissed after Enter (was: {:?})",
            app.last_error
        );

        // Should have started connecting
        assert_eq!(
            app.current_connection_name,
            Some("testconn".to_string()),
            "Should have set current_connection_name after selecting connection"
        );
    }

    /// Test the full workflow: Esc on unmodified form closes immediately
    #[test]
    #[serial]
    fn test_esc_on_unmodified_form_closes_immediately() {
        let _guard = ConfigDirGuard::new(); // Isolate config to temp directory
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Clear any existing state and open the manager
        app.connection_picker = None;
        app.connection_manager = None;
        app.open_connection_manager();

        // Open the add form
        let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        app.on_key(key_a);

        assert!(app.connection_form.is_some(), "Form should be open");

        // Don't modify anything - just press Esc
        let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.on_key(key_esc);

        // No confirmation needed - form should close immediately
        assert!(
            app.confirm_prompt.is_none(),
            "No confirmation should appear for unmodified form"
        );
        assert!(
            app.connection_form.is_none(),
            "Unmodified form should close immediately with Esc"
        );
    }

    // ========== Goto Action Tests ==========

    #[test]
    fn test_goto_action_bindings_in_grid_keymap() {
        use crate::config::CustomKeyBinding;

        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Create config with goto action keybindings
        let mut config = Config::default();
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+g".to_string(),
            action: "goto_first".to_string(),
            description: Some("Go to first row".to_string()),
        });
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+e".to_string(),
            action: "goto_editor".to_string(),
            description: Some("Go to editor".to_string()),
        });
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+c".to_string(),
            action: "goto_connections".to_string(),
            description: Some("Go to connections".to_string()),
        });
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+t".to_string(),
            action: "goto_tables".to_string(),
            description: Some("Go to tables".to_string()),
        });
        config.keymap.grid.push(CustomKeyBinding {
            key: "ctrl+r".to_string(),
            action: "goto_results".to_string(),
            description: Some("Go to results".to_string()),
        });

        let app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            config,
        );

        // Verify the goto actions were registered correctly
        let ctrl_g = KeyBinding::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_g), Some(&Action::GotoFirst));

        let ctrl_e = KeyBinding::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_e), Some(&Action::GotoEditor));

        let ctrl_c = KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_c), Some(&Action::GotoConnections));

        let ctrl_t = KeyBinding::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_t), Some(&Action::GotoTables));

        let ctrl_r = KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(app.grid_keymap.get(&ctrl_r), Some(&Action::GotoResults));
    }

    // ========== Tab Cycling Focus Tests ==========

    #[test]
    fn test_tab_from_connections_to_schema_updates_sidebar_focus() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Close connection manager that auto-opens
        app.connection_manager = None;
        app.connection_picker = None;

        // Make sidebar visible and set focus to Connections
        app.sidebar_visible = true;
        app.focus = Focus::Sidebar(SidebarSection::Connections);
        app.sidebar_focus = SidebarSection::Connections;

        // Press Tab to move from Connections to Schema
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        app.on_key(key);

        // Both focus and sidebar_focus should be updated to Schema
        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Schema),
            "focus should move to Schema"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Schema,
            "sidebar_focus should also be updated to Schema"
        );
    }

    #[test]
    fn test_shift_tab_from_schema_to_connections_updates_sidebar_focus() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Close connection manager that auto-opens
        app.connection_manager = None;
        app.connection_picker = None;

        // Make sidebar visible and set focus to Schema
        app.sidebar_visible = true;
        app.focus = Focus::Sidebar(SidebarSection::Schema);
        app.sidebar_focus = SidebarSection::Schema;

        // Press Shift+Tab to move from Schema to Connections
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        app.on_key(key);

        // Both focus and sidebar_focus should be updated to Connections
        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Connections),
            "focus should move to Connections"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Connections,
            "sidebar_focus should also be updated to Connections"
        );
    }

    #[test]
    fn test_tab_from_grid_to_connections_updates_sidebar_focus() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Close connection manager that auto-opens
        app.connection_manager = None;
        app.connection_picker = None;

        // Make sidebar visible, set focus to Grid, sidebar_focus to Schema (mismatched)
        app.sidebar_visible = true;
        app.focus = Focus::Grid;
        app.sidebar_focus = SidebarSection::Schema; // This simulates the bug condition

        // Press Tab to move from Grid to Connections
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        app.on_key(key);

        // Both focus and sidebar_focus should be Connections
        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Connections),
            "focus should move to Connections"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Connections,
            "sidebar_focus should be updated to Connections"
        );
    }

    #[test]
    fn test_shift_tab_from_query_to_schema_updates_sidebar_focus() {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::new(GridModel::empty(), rt.handle().clone(), tx, rx, None);

        // Close connection manager that auto-opens
        app.connection_manager = None;
        app.connection_picker = None;

        // Make sidebar visible, set focus to Query, sidebar_focus to Connections (mismatched)
        app.sidebar_visible = true;
        app.focus = Focus::Query;
        app.sidebar_focus = SidebarSection::Connections; // This simulates the bug condition

        // Press Shift+Tab to move from Query to Schema
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        app.on_key(key);

        // Both focus and sidebar_focus should be Schema
        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Schema),
            "focus should move to Schema"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Schema,
            "sidebar_focus should be updated to Schema"
        );
    }

    // ========== Panel Navigation Tests (Ctrl+HJKL) ==========

    #[test]
    #[serial]
    fn test_ctrl_h_from_query_moves_to_sidebar_connections() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        // Close auto-opened pickers
        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Query;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Connections),
            "Ctrl+H from Query should move to Sidebar(Connections)"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Connections,
            "sidebar_focus should be updated"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_h_from_grid_moves_to_sidebar_schema() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Grid;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Schema),
            "Ctrl+H from Grid should move to Sidebar(Schema)"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_h_noop_when_sidebar_hidden() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Query;
        app.sidebar_visible = false;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Query,
            "Ctrl+H should be no-op when sidebar is hidden"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_j_from_query_moves_to_grid() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Query;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Grid,
            "Ctrl+J from Query should move to Grid"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_k_from_grid_moves_to_query() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Grid;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Query,
            "Ctrl+K from Grid should move to Query"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_l_from_sidebar_connections_moves_to_query() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Sidebar(SidebarSection::Connections);
        app.sidebar_focus = SidebarSection::Connections;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Query,
            "Ctrl+L from Sidebar(Connections) should move to Query"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_l_from_sidebar_schema_moves_to_grid() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Sidebar(SidebarSection::Schema);
        app.sidebar_focus = SidebarSection::Schema;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Grid,
            "Ctrl+L from Sidebar(Schema) should move to Grid"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_j_within_sidebar_moves_to_schema() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Sidebar(SidebarSection::Connections);
        app.sidebar_focus = SidebarSection::Connections;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Schema),
            "Ctrl+J from Sidebar(Connections) should move to Sidebar(Schema)"
        );
        assert_eq!(
            app.sidebar_focus,
            SidebarSection::Schema,
            "sidebar_focus should be updated"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_k_within_sidebar_moves_to_connections() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Sidebar(SidebarSection::Schema);
        app.sidebar_focus = SidebarSection::Schema;
        app.sidebar_visible = true;
        app.mode = Mode::Normal;

        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Sidebar(SidebarSection::Connections),
            "Ctrl+K from Sidebar(Schema) should move to Sidebar(Connections)"
        );
    }

    #[test]
    #[serial]
    fn test_ctrl_hjkl_noop_in_insert_mode() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Query;
        app.sidebar_visible = true;
        app.mode = Mode::Insert; // Insert mode should not handle Ctrl+HJKL

        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Query,
            "Ctrl+H should be no-op in Insert mode"
        );
    }

    #[test]
    #[serial]
    fn test_boundary_noop_ctrl_k_from_query() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Query;
        app.mode = Mode::Normal;

        // Ctrl+K from Query (nothing above) should be no-op
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Query,
            "Ctrl+K from Query should be no-op (at boundary)"
        );
    }

    #[test]
    #[serial]
    fn test_boundary_noop_ctrl_j_from_grid() {
        let _guard = ConfigDirGuard::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut app = App::with_config(
            GridModel::empty(),
            rt.handle().clone(),
            tx,
            rx,
            None,
            Config::default(),
        );

        app.connection_manager = None;
        app.connection_picker = None;

        app.focus = Focus::Grid;
        app.mode = Mode::Normal;

        // Ctrl+J from Grid (nothing below) should be no-op
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        app.on_key(key);

        assert_eq!(
            app.focus,
            Focus::Grid,
            "Ctrl+J from Grid should be no-op (at boundary)"
        );
    }
}
