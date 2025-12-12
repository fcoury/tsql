use std::io::Stdout;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_postgres::{CancelToken, Client, NoTls, SimpleQueryMessage};
use tui_textarea::{CursorMove, Input};

use super::state::{DbStatus, Focus, Mode, SearchTarget};
use crate::config::{Action, Config, KeyBinding, Keymap};
use crate::history::{History, HistoryEntry};
use crate::ui::{
    ColumnInfo, CommandPrompt, CompletionKind, CompletionPopup, ConnectionInfo, DataGrid,
    FuzzyPicker, GridKeyResult, GridModel, GridState, HighlightedTextArea, JsonEditorAction,
    JsonEditorModal, PickerAction, Priority, QueryEditor, ResizeAction, SchemaCache, SearchPrompt,
    StatusLineBuilder, StatusSegment, TableInfo, create_sql_highlighter, determine_context,
    escape_sql_value, get_word_before_cursor, quote_identifier,
};
use crate::util::{is_json_column_type, should_use_multiline_editor};
use crate::util::format_pg_error;
use tui_syntax::Highlighter;

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
    if cleaned.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
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
async fn fetch_column_types(client: &SharedClient, table: &str) -> std::collections::HashMap<String, String> {
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
}

pub struct DbSession {
    pub status: DbStatus,
    pub conn_str: Option<String>,
    pub client: Option<SharedClient>,
    pub cancel_token: Option<CancelToken>,
    pub last_command_tag: Option<String>,
    pub last_elapsed: Option<Duration>,
    pub running: bool,
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
        let visible_chars: String = chars
            .iter()
            .skip(scroll)
            .take(visible_width)
            .collect();

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

    pub editor: QueryEditor,
    pub highlighter: Highlighter,
    pub search: SearchPrompt,
    pub search_target: SearchTarget,
    pub command: CommandPrompt,
    pub completion: CompletionPopup,
    pub schema_cache: SchemaCache,
    pub pending_key: Option<char>,
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

    pub show_help: bool,
    pub last_status: Option<String>,
    pub last_error: Option<String>,

    /// Query history with persistence.
    pub history: History,
    /// Fuzzy picker for history search (when open).
    pub history_picker: Option<FuzzyPicker<HistoryEntry>>,
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

        let mut app = Self {
            focus: Focus::Query,
            mode: Mode::Normal,

            config,

            grid_keymap,
            editor_normal_keymap,
            editor_insert_keymap,

            editor,
            highlighter: create_sql_highlighter(),
            search: SearchPrompt::new(),
            search_target: SearchTarget::Editor,
            command: CommandPrompt::new(),
            completion: CompletionPopup::new(),
            schema_cache: SchemaCache::new(),
            pending_key: None,
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

            show_help: false,
            last_status: None,
            last_error: None,

            history,
            history_picker: None,
        };

        // Auto-connect if connection string provided
        if let Some(url) = effective_conn_str {
            app.start_connect(url);
        }

        app
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

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            self.drain_db_events();

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

            terminal.draw(|frame| {
                let size = frame.area();

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
                    .split(size);

                let query_area = chunks[0];
                let error_area = chunks[1];
                let grid_area = chunks[2];
                let status_area = chunks[3];

                // Query editor with syntax highlighting
                let query_border = match (self.focus, self.mode) {
                    (Focus::Query, Mode::Normal) => Style::default().fg(Color::Cyan),
                    (Focus::Query, Mode::Insert) => Style::default().fg(Color::Green),
                    (Focus::Query, Mode::Visual) => Style::default().fg(Color::Yellow),
                    (Focus::Grid, _) => Style::default().fg(Color::DarkGray),
                };

                let query_title = match (self.focus, self.mode) {
                    (Focus::Query, Mode::Normal) => {
                        "Query [NORMAL] (i insert, Enter run, Ctrl-r history, Tab to grid)"
                    }
                    (Focus::Query, Mode::Insert) => "Query [INSERT] (Esc normal, Ctrl-r history)",
                    (Focus::Query, Mode::Visual) => "Query [VISUAL] (y yank, d delete, Esc cancel)",
                    (Focus::Grid, _) => "Query (Tab to focus)",
                };

                let query_block = Block::default()
                    .borders(Borders::ALL)
                    .title(query_title)
                    .border_style(query_border);

                let highlighted_editor = HighlightedTextArea::new(
                    &self.editor.textarea,
                    highlighted_lines.clone(),
                )
                .block(query_block)
                .scroll(self.editor_scroll);

                frame.render_widget(highlighted_editor, query_area);
                
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
                };
                frame.render_widget(grid_widget, grid_area);

                // Status.
                frame.render_widget(self.status_line(status_area.width), status_area);

                if self.show_help {
                    let popup = centered_rect(80, 70, size);
                    frame.render_widget(Clear, popup);
                    frame.render_widget(help_popup(), popup);
                }

                // Render history picker if open
                if let Some(ref mut picker) = self.history_picker {
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

                    if !visible.is_empty() {
                        // Position popup near the cursor
                        let (cursor_row, cursor_col) = self.editor.textarea.cursor();
                        // Estimate position (query_area starts at y=0, each line is 1 row)
                        let popup_y = query_area.y + 1 + cursor_row as u16;
                        let popup_x = query_area.x + 1 + cursor_col.saturating_sub(self.completion.prefix.len()) as u16;

                        let popup_height = (visible.len() + 2) as u16; // +2 for borders
                        let popup_width = 40u16.min(size.width.saturating_sub(popup_x));

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
                                    Span::styled(format!("{} ", prefix), Style::default().fg(Color::DarkGray)),
                                    Span::styled(&item.label, style),
                                ])
                            })
                            .collect();

                        let completion_block = Block::default()
                            .borders(Borders::ALL)
                            .title("Completions (Tab select, Esc cancel)")
                            .border_style(Style::default().fg(Color::Cyan));

                        let completion_list = Paragraph::new(lines).block(completion_block);

                        frame.render_widget(Clear, popup_area);
                        frame.render_widget(completion_list, popup_area);
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

                    let title = format!("Edit: {} (Enter confirm, Esc cancel)", col_name);
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
                    let scroll_indicator = if self.cell_editor.scroll_offset > 0 || total_chars > inner_width {
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
                            self.cell_editor.value[..self.cell_editor.cursor].chars().count()
                        );
                        let info_area = Rect {
                            x: popup_area.x + 1,
                            y: popup_area.y + 3,
                            width: popup_area.width.saturating_sub(2),
                            height: 1,
                        };
                        let info_widget = Paragraph::new(info)
                            .style(Style::default().fg(Color::DarkGray));
                        frame.render_widget(info_widget, info_area);
                    }
                }

                // Render JSON editor modal if active
                if let Some(ref mut json_editor) = self.json_editor {
                    json_editor.render(frame, size);
                }
            })?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    if self.on_key(key) {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        // Handle JSON editor when active - it captures all input
        if self.json_editor.is_some() {
            return self.handle_json_editor_key(key);
        }

        // Ctrl-c: cancel running query.
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            if self.db.running {
                self.cancel_query();
                return false;
            }
        }

        // Esc: cancel running query, or return to Normal, close help, dismiss errors.
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            if self.db.running {
                self.cancel_query();
                return false;
            }

            self.show_help = false;
            self.search.close();
            self.command.close();
            self.completion.close();
            self.cell_editor.close();
            self.history_picker = None;
            self.pending_key = None;
            self.last_error = None;
            self.mode = Mode::Normal;
            return false;
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

        // Handle history picker when open
        if self.history_picker.is_some() {
            return self.handle_history_picker_key(key);
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
                (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Enter, KeyModifiers::NONE) => {
                    self.apply_completion();
                    return false;
                }
                (KeyCode::Down, KeyModifiers::NONE)
                | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                    self.completion.select_next();
                    return false;
                }
                (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                    self.completion.select_prev();
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

        // Global keys are only active in Normal mode.
        if self.mode == Mode::Normal {
            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), KeyModifiers::NONE) => return true,
                (KeyCode::Char('?'), KeyModifiers::NONE) => {
                    self.show_help = !self.show_help;
                    return false;
                }
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.focus = match self.focus {
                        Focus::Query => Focus::Grid,
                        Focus::Grid => Focus::Query,
                    };
                    return false;
                }
                _ => {}
            }
        }

        if self.show_help {
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
                                self.show_help = true;
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
                        GridKeyResult::ResizeColumn { col, action } => {
                            match action {
                                ResizeAction::Widen => self.grid.widen_column(col, 2),
                                ResizeAction::Narrow => self.grid.narrow_column(col, 2),
                                ResizeAction::AutoFit => self.grid.autofit_column(col),
                            }
                        }
                        GridKeyResult::EditCell { row, col } => {
                            self.start_cell_edit(row, col);
                        }
                        GridKeyResult::None => {}
                    }
                }
            }
            Focus::Query => {
                self.handle_editor_key(key);
            }
        }

        false
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                match clipboard.set_text(text) {
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
                }
            }
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
            self.json_editor = Some(JsonEditorModal::new(
                value, col_name, col_type, row, col,
            ));
        } else {
            // Use inline editor for simple values
            self.cell_editor.open(row, col, value);
        }
    }

    fn handle_cell_edit_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            // Enter: confirm edit
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.commit_cell_edit();
                return false;
            }
            // Escape: cancel edit
            (KeyCode::Esc, KeyModifiers::NONE) => {
                self.cell_editor.close();
                self.last_status = Some("Edit cancelled".to_string());
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

        let match_count = self.grid_state.search.matches.len();
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
                self.show_help = true;
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
            "history" => {
                self.open_history_picker();
            }
            _ => {
                self.last_status = Some(format!("Unknown command: {}", command));
            }
        }

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
                                    headers = row.columns()
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
                self.last_error = Some(format!("Unknown format: {}. Use csv, json, or tsv.", format));
                return;
            }
        };

        // Expand ~ to home directory
        let expanded_path = if path.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                std::path::PathBuf::from(home).join(&path[2..])
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
            _ => {
                match &self.grid.source_table {
                    Some(t) => t.clone(),
                    None => {
                        self.last_error = Some(format!(
                            "No table specified and couldn't infer from query. Usage: :gen {} <table>",
                            gen_type
                        ));
                        return;
                    }
                }
            }
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
                let keys: Option<Vec<&str>> = key_columns.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
                self.grid
                    .generate_update_sql(&table, &row_indices, keys.as_deref())
            }
            "delete" | "d" => {
                let keys: Option<Vec<&str>> = key_columns.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());
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
                self.show_help = true;
            }
            Action::ShowHistory => {
                self.open_history_picker();
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
                        // yy - yank (copy) line
                        ('y', KeyCode::Char('y'), KeyModifiers::NONE) => {
                            self.editor.yank_line();
                            self.last_status = Some("Yanked line".to_string());
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
                    (KeyCode::Char('G'), KeyModifiers::SHIFT) | (KeyCode::Char('G'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('N'), KeyModifiers::SHIFT) | (KeyCode::Char('N'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('A'), KeyModifiers::SHIFT) | (KeyCode::Char('A'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::End);
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('I'), KeyModifiers::SHIFT) | (KeyCode::Char('I'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('O'), KeyModifiers::SHIFT) | (KeyCode::Char('O'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('X'), KeyModifiers::SHIFT) | (KeyCode::Char('X'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_char();
                    }
                    (KeyCode::Char('D'), KeyModifiers::SHIFT) | (KeyCode::Char('D'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_line_by_end();
                    }
                    (KeyCode::Char('C'), KeyModifiers::SHIFT) | (KeyCode::Char('C'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('P'), KeyModifiers::SHIFT) | (KeyCode::Char('P'), KeyModifiers::NONE) => {
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

                // Check keymap for insert mode actions (e.g., Ctrl+Enter to execute)
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
                    // Yank (copy) selection.
                    (KeyCode::Char('y'), KeyModifiers::NONE) => {
                        self.editor.textarea.copy();
                        self.editor.textarea.cancel_selection();
                        self.mode = Mode::Normal;
                        self.last_status = Some("Yanked".to_string());
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
                    (KeyCode::Char('G'), KeyModifiers::SHIFT) | (KeyCode::Char('G'), KeyModifiers::NONE) => {
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
        let conn_info = self.db.conn_str.as_ref().map(|s| {
            ConnectionInfo::parse(s).format(50)
        });
        self.history.push(query.clone(), conn_info);

        let Some(client) = self.db.client.clone() else {
            self.last_error = Some("Not connected. Use :connect <url> or set DATABASE_URL.".to_string());
            return;
        };

        if self.db.running {
            self.last_status = Some("Query already running".to_string());
            return;
        }

        self.db.running = true;
        self.last_status = Some("Running...".to_string());

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
                        headers.iter().map(|h| type_map.get(h).cloned().unwrap_or_default()).collect()
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
                self.last_status = Some("Connect failed (see error)".to_string());
                self.last_error = Some(format!("Connection error: {}", error));
            }
            DbEvent::ConnectionLost { error } => {
                self.db.status = DbStatus::Error;
                self.db.client = None;
                self.db.running = false;
                self.last_status = Some("Connection lost (see error)".to_string());
                self.last_error = Some(format!("Connection lost: {}", error));
            }
            DbEvent::QueryFinished { result } => {
                self.db.running = false;
                self.db.last_command_tag = result.command_tag.clone();
                self.db.last_elapsed = Some(result.elapsed);
                self.last_error = None; // Clear any previous error.

                self.grid = GridModel::new(result.headers, result.rows)
                    .with_source_table(result.source_table)
                    .with_primary_keys(result.primary_keys)
                    .with_col_types(result.col_types);
                self.grid_state = GridState::default();

                // Move focus to grid to show results
                self.focus = Focus::Grid;

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
                self.last_status = Some("Query error (see above)".to_string());
                self.last_error = Some(error);
            }
            DbEvent::QueryCancelled => {
                self.db.running = false;
                self.last_status = Some("Query cancelled".to_string());
            }
            DbEvent::SchemaLoaded { tables } => {
                self.schema_cache.tables = tables;
                self.schema_cache.loaded = true;
                self.last_status = Some(format!(
                    "Schema loaded: {} tables",
                    self.schema_cache.tables.len()
                ));
            }
            DbEvent::CellUpdated { row, col, value } => {
                self.db.running = false;
                // Update the grid cell
                if let Some(grid_row) = self.grid.rows.get_mut(row) {
                    if let Some(cell) = grid_row.get_mut(col) {
                        *cell = value;
                    }
                }
                self.last_status = Some("Cell updated successfully".to_string());
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
            let time_part = self.db.last_elapsed
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
            .add(StatusSegment::new(mode_text, Priority::Critical).style(mode_style))
            // Critical: Connection info
            .add(StatusSegment::new(conn_segment, Priority::Critical).style(conn_style).min_width(40))
            // High: Running indicator (if running)
            .add_if(
                running_indicator.is_some(),
                StatusSegment::new(running_indicator.unwrap_or_default(), Priority::High)
                    .style(Style::default().fg(Color::Yellow))
            )
            // Medium: Row info
            .add(StatusSegment::new(row_info, Priority::Medium).min_width(50))
            // Medium: Selection (if any selected)
            .add_if(
                selection_info.is_some(),
                StatusSegment::new(selection_info.unwrap_or_default(), Priority::Medium)
                    .style(Style::default().fg(Color::Cyan))
                    .min_width(60)
            )
            // Low: Query timing
            .add_if(
                timing_info.is_some(),
                StatusSegment::new(timing_info.unwrap_or_default(), Priority::Low)
                    .style(Style::default().fg(Color::DarkGray))
                    .min_width(80)
            )
            // Right-aligned: Status message
            .add(StatusSegment::new(status, Priority::Critical).style(status_style).right_align())
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

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn help_popup() -> Paragraph<'static> {
    let lines = vec![
        Line::from(vec![Span::styled(
            "tsql - PostgreSQL CLI",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Global", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(":  "),
            Span::raw("Tab focus switch, Esc normal/close, q quit, ? help"),
        ]),
        Line::from(vec![
            Span::styled("Query", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(":  "),
            Span::raw("NORMAL: h/j/k/l move, w/b/e word, 0/$ line, gg/G doc, Ctrl-d/u scroll"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw("i/a/A/I insert, o/O open line, x/X delete char, D/C del/chg to EOL"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw("dd/cc del/chg line, dw/cw/db/cb del/chg word, u undo, Ctrl-r redo"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw("v visual mode, yy yank line, p/P paste after/before"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw("/ search, n/N next/prev, Enter run, Ctrl-p/n history nav"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw("Ctrl-r fuzzy history search, :history command"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw(": commands (:connect, :export, :gen, :\\dt, :\\d <tbl>, :\\dn, :\\di)"),
        ]),
        Line::from(vec![
            Span::styled("Visual", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": "),
            Span::raw("h/j/k/l extend selection, y yank, d delete, c change, Esc cancel"),
        ]),
        Line::from(vec![
            Span::styled("Grid", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(":  "),
            Span::raw("j/k rows, h/l cols, H/L scroll, Space select, a all, Esc clear"),
        ]),
        Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::raw("   "),
            Span::raw("/ search, n/N next/prev, c copy cell, y yank row, Y w/headers"),
        ]),
        Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::raw("   "),
            Span::raw("+/> widen col, -/< narrow col, = auto-fit col, e/Enter edit cell"),
        ]),
    ];

    Paragraph::new(lines)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .style(Style::default().fg(Color::White))
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
            assert!(cursor_pos < 10, "Cursor should be visible, got pos {}", cursor_pos);
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
            extract_table_from_query("SELECT * FROM users JOIN orders ON users.id = orders.user_id"),
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
}
