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
use crate::ui::{
    ColumnInfo, CommandPrompt, CompletionKind, CompletionPopup, DataGrid, GridKeyResult,
    GridModel, GridState, HighlightedTextArea, QueryEditor, SchemaCache, SearchPrompt, TableInfo,
    create_sql_highlighter, determine_context, get_word_before_cursor,
};
use crate::util::format_pg_error;
use tui_syntax::Highlighter;

pub struct QueryResult {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub command_tag: Option<String>,
    pub truncated: bool,
    pub elapsed: Duration,
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

pub struct App {
    pub focus: Focus,
    pub mode: Mode,

    pub editor: QueryEditor,
    pub highlighter: Highlighter,
    pub search: SearchPrompt,
    pub search_target: SearchTarget,
    pub command: CommandPrompt,
    pub completion: CompletionPopup,
    pub schema_cache: SchemaCache,
    pub pending_key: Option<char>,

    pub rt: tokio::runtime::Handle,
    pub db_events_tx: mpsc::UnboundedSender<DbEvent>,
    pub db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
    pub db: DbSession,

    pub grid: GridModel,
    pub grid_state: GridState,

    pub show_help: bool,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

impl App {
    pub fn new(
        grid: GridModel,
        rt: tokio::runtime::Handle,
        db_events_tx: mpsc::UnboundedSender<DbEvent>,
        db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
        conn_str: Option<String>,
    ) -> Self {
        let editor = QueryEditor::new();

        let mut app = Self {
            focus: Focus::Query,
            mode: Mode::Normal,

            editor,
            highlighter: create_sql_highlighter(),
            search: SearchPrompt::new(),
            search_target: SearchTarget::Editor,
            command: CommandPrompt::new(),
            completion: CompletionPopup::new(),
            schema_cache: SchemaCache::new(),
            pending_key: None,

            rt,
            db_events_tx,
            db_events_rx,
            db: DbSession::new(),

            grid,
            grid_state: GridState::default(),

            show_help: false,
            last_status: None,
            last_error: None,
        };

        // Auto-connect if connection string provided
        if let Some(url) = conn_str {
            app.start_connect(url);
        }

        app
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
                        "Query [NORMAL] (i insert, Enter run, Ctrl-p/n history, Tab to grid)"
                    }
                    (Focus::Query, Mode::Insert) => "Query [INSERT] (Esc to normal)",
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
                .block(query_block);

                frame.render_widget(highlighted_editor, query_area);

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

                // Results grid.
                let grid_widget = DataGrid {
                    model: &self.grid,
                    state: &self.grid_state,
                    focused: self.focus == Focus::Grid,
                };
                frame.render_widget(grid_widget, grid_area);

                // Status.
                frame.render_widget(self.status_line(), status_area);

                if self.show_help {
                    let popup = centered_rect(80, 70, size);
                    frame.render_widget(Clear, popup);
                    frame.render_widget(help_popup(), popup);
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

        if self.search.active {
            self.handle_search_key(key);
            return false;
        }

        if self.command.active {
            return self.handle_command_key(key);
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
                    let result = self.grid_state.handle_key(key, &self.grid);
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
            _ => {
                self.last_status = Some(format!("Unknown command: {}", command));
            }
        }

        false
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

        self.editor.push_history(query.clone());

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

                    let result = QueryResult {
                        headers,
                        rows,
                        command_tag: last_cmd,
                        truncated,
                        elapsed,
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

                self.grid = GridModel::new(result.headers, result.rows);
                self.grid_state = GridState::default();

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
        }
    }

    fn status_line(&self) -> Paragraph<'static> {
        let row_count = self.grid.rows.len();
        let selected_count = self.grid_state.selected_rows.len();
        let cursor_row = if row_count == 0 {
            0
        } else {
            self.grid_state.cursor_row.saturating_add(1)
        };

        let focus = match self.focus {
            Focus::Query => "QUERY",
            Focus::Grid => "GRID",
        };
        let mode = match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        };

        let mut db_part = format!("DB: {}", self.db.status.label());
        if self.db.running {
            db_part.push_str(" (running)");
        }
        if let Some(tag) = self.db.last_command_tag.as_deref() {
            db_part.push_str("  Last: ");
            db_part.push_str(tag);
        }
        if let Some(elapsed) = self.db.last_elapsed {
            db_part.push_str(&format!(" ({} ms)", elapsed.as_millis()));
        }

        let status = self.last_status.as_deref().unwrap_or("Ready");

        let text = format!(
            "Focus: {}  Mode: {}  {}  Rows: {}  Selected: {}  Cursor: {}  ColOffset: {}   | {}",
            focus,
            mode,
            db_part,
            row_count,
            selected_count,
            cursor_row,
            self.grid_state.col_offset,
            status
        );

        Paragraph::new(text).style(Style::default().fg(Color::Gray))
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
            Span::raw("/ search, n/N next/prev, Enter run, Ctrl-p/n history"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::raw("   "),
            Span::raw(": commands (:connect, :disconnect, :export csv|json|tsv <path>)"),
        ]),
        Line::from(vec![
            Span::styled("Visual", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": "),
            Span::raw("h/j/k/l extend selection, y yank, d delete, c change, Esc cancel"),
        ]),
        Line::from(vec![
            Span::styled("Grid", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(":  "),
            Span::raw("j/k move, h/l scroll cols, Space select, a all, Esc clear"),
        ]),
        Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::raw("   "),
            Span::raw("/ search, n/N next/prev, c copy cell, y yank row, Y yank w/headers"),
        ]),
    ];

    Paragraph::new(lines)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .style(Style::default().fg(Color::White))
}
