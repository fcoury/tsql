use std::collections::BTreeSet;
use std::env;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_textarea::{CursorMove, Input, TextArea};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_postgres::{CancelToken, Client, NoTls, SimpleQueryMessage};

fn main() -> Result<()> {
    let rt = Runtime::new().context("failed to initialize tokio runtime")?;
    let (db_events_tx, db_events_rx) = mpsc::unbounded_channel();

    let mut terminal =
        init_terminal().context("failed to initialize terminal; are you running in a real TTY?")?;

    let mut app = App::new(
        sample_table(),
        rt.handle().clone(),
        db_events_tx,
        db_events_rx,
    );
    let res = app.run(&mut terminal);

    restore_terminal(terminal)?;

    res
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Query,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    Insert,
    Visual,
}

struct SearchPrompt {
    active: bool,
    textarea: TextArea<'static>,
    // When set, n/N moves between matches in the query editor.
    last_applied: Option<String>,
}

impl SearchPrompt {
    fn new() -> Self {
        let mut textarea = TextArea::new(vec![String::new()]);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            active: false,
            textarea,
            last_applied: None,
        }
    }

    fn open(&mut self) {
        self.active = true;
        self.textarea = TextArea::new(vec![String::new()]);
        self.textarea
            .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
    }

    fn close(&mut self) {
        self.active = false;
    }

    fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }
}

struct CommandPrompt {
    active: bool,
    textarea: TextArea<'static>,
}

impl CommandPrompt {
    fn new() -> Self {
        let mut textarea = TextArea::new(vec![String::new()]);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            active: false,
            textarea,
        }
    }

    fn open(&mut self) {
        self.active = true;
        self.textarea = TextArea::new(vec![String::new()]);
        self.textarea
            .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
    }

    fn close(&mut self) {
        self.active = false;
    }

    fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }
}

struct QueryResult {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    command_tag: Option<String>,
    truncated: bool,
    elapsed: Duration,
}

type SharedClient = Arc<Mutex<Client>>;

enum DbEvent {
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
}

enum DbStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

struct DbSession {
    status: DbStatus,
    conn_str: Option<String>,
    client: Option<SharedClient>,
    cancel_token: Option<CancelToken>,
    last_command_tag: Option<String>,
    last_elapsed: Option<Duration>,
    running: bool,
}

impl DbSession {
    fn new() -> Self {
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

    fn status_label(&self) -> &'static str {
        match self.status {
            DbStatus::Disconnected => "DISCONNECTED",
            DbStatus::Connecting => "CONNECTING",
            DbStatus::Connected => "CONNECTED",
            DbStatus::Error => "ERROR",
        }
    }
}

struct App {
    focus: Focus,
    mode: Mode,

    editor: QueryEditor,
    search: SearchPrompt,
    command: CommandPrompt,
    pending_key: Option<char>,

    rt: tokio::runtime::Handle,
    db_events_tx: mpsc::UnboundedSender<DbEvent>,
    db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
    db: DbSession,

    grid: GridModel,
    grid_state: GridState,

    show_help: bool,
    last_status: Option<String>,
    last_error: Option<String>,
}

impl App {
    fn new(
        grid: GridModel,
        rt: tokio::runtime::Handle,
        db_events_tx: mpsc::UnboundedSender<DbEvent>,
        db_events_rx: mpsc::UnboundedReceiver<DbEvent>,
    ) -> Self {
        let mut editor = QueryEditor::new();
        editor.set_text("-- Type a query here\nSELECT 1;".to_string());

        let mut app = Self {
            focus: Focus::Query,
            mode: Mode::Normal,

            editor,
            search: SearchPrompt::new(),
            command: CommandPrompt::new(),
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

        app.auto_connect_from_env();
        app
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            self.drain_db_events();

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

                // Query editor.
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

                self.editor.textarea.set_block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(query_title)
                        .border_style(query_border),
                );

                frame.render_widget(&self.editor.textarea, query_area);

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

                    self.search.textarea.set_block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("/ Search (Enter apply, Esc cancel)")
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
                    self.grid_state.handle_key(key, &self.grid);
                }
            }
            Focus::Query => {
                self.handle_editor_key(key);
            }
        }

        false
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let pattern = self.search.text();
                let pattern = pattern.trim().to_string();

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
            _ => {
                self.last_status = Some(format!("Unknown command: {}", command));
            }
        }

        false
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
                    (KeyCode::Char('G'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('N'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('A'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.move_cursor(CursorMove::End);
                        self.mode = Mode::Insert;
                    }
                    (KeyCode::Char('I'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('O'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('X'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_char();
                    }
                    (KeyCode::Char('D'), KeyModifiers::NONE) => {
                        self.pending_key = None;
                        self.editor.textarea.delete_line_by_end();
                    }
                    (KeyCode::Char('C'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('P'), KeyModifiers::NONE) => {
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
                    (KeyCode::Char('G'), KeyModifiers::NONE) => {
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

    fn auto_connect_from_env(&mut self) {
        if let Ok(conn_str) = env::var("DATABASE_URL") {
            self.start_connect(conn_str);
        }
    }

    fn start_connect(&mut self, conn_str: String) {
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
                                error: e.to_string(),
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
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    fn execute_query(&mut self) {
        let query = self.editor.text();
        if query.trim().is_empty() {
            self.last_status = Some("No query to run".to_string());
            return;
        }

        self.editor.push_history(query.clone());

        let Some(client) = self.db.client.clone() else {
            self.last_status = Some("Not connected. Set DATABASE_URL to connect.".to_string());
            self.grid = sample_table_for_query(&query);
            self.grid_state = GridState::default();
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
                        error: e.to_string(),
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
                self.last_status = Some("Connected".to_string());
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

        let mut db_part = format!("DB: {}", self.db.status_label());
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

struct QueryEditor {
    textarea: TextArea<'static>,

    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
}

impl QueryEditor {
    fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            history_draft: None,
        }
    }

    fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn set_text(&mut self, s: String) {
        let lines: Vec<String> = if s.is_empty() {
            vec![String::new()]
        } else {
            s.lines().map(|l| l.to_string()).collect()
        };

        // Recreate the underlying textarea content.
        let mut textarea = TextArea::new(lines);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
        self.textarea = textarea;

        self.history_index = None;
        self.history_draft = None;
    }

    fn push_history(&mut self, query: String) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.history.last().map(|s| s.trim()) == Some(trimmed) {
            return;
        }

        self.history.push(trimmed.to_string());
        self.history_index = None;
        self.history_draft = None;
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        if self.history_index.is_none() {
            self.history_draft = Some(self.text());
            self.history_index = Some(self.history.len().saturating_sub(1));
        } else {
            let i = self.history_index.unwrap();
            self.history_index = Some(i.saturating_sub(1));
        }

        if let Some(i) = self.history_index {
            self.set_text(self.history[i].clone());
        }
    }

    fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let Some(i) = self.history_index else {
            return;
        };

        let next = i + 1;
        if next >= self.history.len() {
            self.history_index = None;
            if let Some(draft) = self.history_draft.take() {
                self.set_text(draft);
            }
            return;
        }

        self.history_index = Some(next);
        self.set_text(self.history[next].clone());
    }

    fn input(&mut self, key: KeyEvent) {
        let input: Input = key.into();
        self.textarea.input(input);

        // If the user starts typing/editing, stop history navigation.
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft = None;
        }
    }

    /// Delete the entire current line (vim `dd`).
    fn delete_line(&mut self) {
        // Select the entire line and cut it.
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.delete_line_by_end();
        // If we're not on the last line, delete the newline too.
        self.textarea.delete_newline();
    }

    /// Clear the current line content but keep the line (vim `cc`).
    fn change_line(&mut self) {
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.delete_line_by_end();
    }

    /// Yank (copy) the current line (vim `yy`).
    fn yank_line(&mut self) {
        let (row, _) = self.textarea.cursor();
        let lines = self.textarea.lines();
        if row < lines.len() {
            let line = lines[row].clone() + "\n";
            self.textarea.set_yank_text(line);
        }
    }
}

#[derive(Default, Clone)]
struct GridState {
    row_offset: usize,
    col_offset: usize,
    cursor_row: usize,
    selected_rows: BTreeSet<usize>,
}

impl GridState {
    fn handle_key(&mut self, key: KeyEvent, model: &GridModel) {
        let row_count = model.rows.len();
        let col_count = model.headers.len();

        match (key.code, key.modifiers) {
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                if row_count > 0 {
                    self.cursor_row = (self.cursor_row + 1).min(row_count - 1);
                }
            }
            (KeyCode::PageUp, _) => {
                self.cursor_row = self.cursor_row.saturating_sub(10);
            }
            (KeyCode::PageDown, _) => {
                if row_count > 0 {
                    self.cursor_row = (self.cursor_row + 10).min(row_count - 1);
                }
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
                self.cursor_row = 0;
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                if row_count > 0 {
                    self.cursor_row = row_count - 1;
                }
            }

            // Horizontal scroll is column-based (jump by columns).
            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                self.col_offset = self.col_offset.saturating_sub(1);
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                if col_count > 0 {
                    self.col_offset = (self.col_offset + 1).min(col_count - 1);
                }
            }

            // Multi-select controls.
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                if row_count == 0 {
                    return;
                }
                if self.selected_rows.contains(&self.cursor_row) {
                    self.selected_rows.remove(&self.cursor_row);
                } else {
                    self.selected_rows.insert(self.cursor_row);
                }
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.selected_rows.clear();
                for i in 0..row_count {
                    self.selected_rows.insert(i);
                }
            }
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.selected_rows.clear();
            }

            _ => {}
        }
    }

    fn ensure_cursor_visible(&mut self, viewport_rows: usize, row_count: usize) {
        if viewport_rows == 0 || row_count == 0 {
            self.row_offset = 0;
            self.cursor_row = 0;
            return;
        }

        self.cursor_row = self.cursor_row.min(row_count - 1);

        if self.cursor_row < self.row_offset {
            self.row_offset = self.cursor_row;
        }

        let last_visible = self.row_offset + viewport_rows - 1;
        if self.cursor_row > last_visible {
            self.row_offset = self.cursor_row.saturating_sub(viewport_rows - 1);
        }

        self.row_offset = self.row_offset.min(row_count.saturating_sub(1));
    }
}

struct GridModel {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    col_widths: Vec<u16>,
}

impl GridModel {
    fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let col_widths = compute_column_widths(&headers, &rows);
        Self {
            headers,
            rows,
            col_widths,
        }
    }
}

struct DataGrid<'a> {
    model: &'a GridModel,
    state: &'a GridState,
    focused: bool,
}

impl<'a> Widget for DataGrid<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = "Results (j/k move, h/l scroll cols, Space select)";

        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.model.headers.is_empty() {
            Paragraph::new("No columns")
                .style(Style::default().fg(Color::Gray))
                .render(inner, buf);
            return;
        }

        // Reserve one line for header.
        if inner.height < 2 {
            Paragraph::new("Window too small")
                .style(Style::default().fg(Color::Gray))
                .render(inner, buf);
            return;
        }

        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };

        let body_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height - 1,
        };

        // Keep marker column fixed; horizontal scroll applies to data columns.
        let marker_w: u16 = 3; // cursor + selected + space
        let data_x = header_area.x.saturating_add(marker_w);
        let data_w = header_area.width.saturating_sub(marker_w);

        // Header row (frozen).
        render_marker_header(header_area, buf, marker_w);
        render_row_cells(
            data_x,
            header_area.y,
            data_w,
            &self.model.headers,
            &self.model.col_widths,
            self.state.col_offset,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            buf,
        );

        // Body rows.
        if self.model.rows.is_empty() {
            Paragraph::new("(no rows)")
                .style(Style::default().fg(Color::Gray))
                .render(body_area, buf);
            return;
        }

        let mut state = self.state.clone();
        state.ensure_cursor_visible(body_area.height as usize, self.model.rows.len());

        for i in 0..(body_area.height as usize) {
            let row_idx = state.row_offset + i;
            if row_idx >= self.model.rows.len() {
                break;
            }
            let y = body_area.y + i as u16;

            let is_cursor = row_idx == state.cursor_row;
            let is_selected = state.selected_rows.contains(&row_idx);

            let row_style = if is_cursor {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            render_marker_cell(
                body_area.x,
                y,
                marker_w,
                is_cursor,
                is_selected,
                row_style,
                buf,
            );

            render_row_cells(
                data_x,
                y,
                data_w,
                &self.model.rows[row_idx],
                &self.model.col_widths,
                state.col_offset,
                row_style,
                buf,
            );
        }
    }
}

fn render_marker_header(area: Rect, buf: &mut Buffer, marker_w: u16) {
    let mut x = area.x;
    for _ in 0..marker_w {
        buf.set_string(x, area.y, " ", Style::default());
        x += 1;
    }
}

fn render_marker_cell(
    x: u16,
    y: u16,
    marker_w: u16,
    is_cursor: bool,
    is_selected: bool,
    style: Style,
    buf: &mut Buffer,
) {
    let cursor_ch = if is_cursor { '>' } else { ' ' };
    let sel_ch = if is_selected { '*' } else { ' ' };

    let s = format!("{}{} ", cursor_ch, sel_ch);
    let s = fit_to_width(&s, marker_w);
    buf.set_string(x, y, s, style);
}

fn render_row_cells(
    mut x: u16,
    y: u16,
    available_w: u16,
    cells: &[String],
    col_widths: &[u16],
    col_offset: usize,
    style: Style,
    buf: &mut Buffer,
) {
    if available_w == 0 {
        return;
    }

    let padding: u16 = 1;
    let max_x = x.saturating_add(available_w);

    let mut col = col_offset;
    while col < cells.len() && col < col_widths.len() && x < max_x {
        let w = col_widths[col];
        if w == 0 {
            col += 1;
            continue;
        }

        let remaining = max_x - x;
        if remaining == 0 {
            break;
        }

        // Allow a partially visible last column.
        let draw_w = w.min(remaining);
        let content = fit_to_width(&cells[col], draw_w);
        buf.set_string(x, y, content, style);
        x += draw_w;

        if x < max_x {
            buf.set_string(x, y, " ", style);
            x = x.saturating_add(padding).min(max_x);
        }

        col += 1;
    }

    while x < max_x {
        buf.set_string(x, y, " ", style);
        x += 1;
    }
}

fn compute_column_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<u16> {
    // Keep columns readable but rely on horizontal scroll for the rest.
    let min_w: u16 = 3;
    let max_w: u16 = 40;

    let mut widths: Vec<u16> = headers
        .iter()
        .map(|h| clamp_u16(display_width(h) as u16, min_w, max_w))
        .collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i >= widths.len() {
                break;
            }
            let w = clamp_u16(display_width(cell) as u16, min_w, max_w);
            widths[i] = widths[i].max(w);
        }
    }

    widths
}

fn clamp_u16(v: u16, min_v: u16, max_v: u16) -> u16 {
    v.max(min_v).min(max_v)
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn fit_to_width(s: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }

    let current = display_width(s);
    if current == width {
        return s.to_string();
    }

    if current < width {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - current));
        return out;
    }

    // Truncate, keeping ASCII-only ellipsis.
    if width <= 3 {
        return truncate_by_display_width(s, width);
    }

    let prefix_w = width.saturating_sub(3);
    let mut out = truncate_by_display_width(s, prefix_w);
    out.push_str("...");

    truncate_by_display_width(&out, width)
}

fn truncate_by_display_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;

    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
        if used == width {
            break;
        }
    }

    let out_w = display_width(&out);
    if out_w < width {
        out.push_str(&" ".repeat(width - out_w));
    }

    out
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
            Span::raw(": command mode (:connect <url>, :disconnect, :q, :help)"),
        ]),
        Line::from(vec![
            Span::styled("Visual", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": "),
            Span::raw("h/j/k/l extend selection, y yank, d delete, c change, Esc cancel"),
        ]),
        Line::from(vec![
            Span::styled("Grid", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(":  "),
            Span::raw("j/k move, h/l scroll columns, Space select, a all, c clear"),
        ]),
    ];

    Paragraph::new(lines)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .style(Style::default().fg(Color::White))
}

fn sample_table() -> GridModel {
    let headers = vec![
        "id".to_string(),
        "email".to_string(),
        "status".to_string(),
        "created_at".to_string(),
        "notes".to_string(),
    ];

    let mut rows = Vec::new();
    for i in 1..=200 {
        rows.push(vec![
            i.to_string(),
            format!("user{}@example.com", i),
            if i % 7 == 0 {
                "disabled"
            } else if i % 5 == 0 {
                "pending"
            } else {
                "active"
            }
            .to_string(),
            format!("2025-12-{:02} 12:{:02}:00", (i % 28) + 1, i % 60),
            format!(
                "Some long-ish text to prove horizontal scrolling works (row {}).",
                i
            ),
        ]);
    }

    GridModel::new(headers, rows)
}

fn sample_table_for_query(query: &str) -> GridModel {
    let mut model = sample_table();
    if let Some(notes_idx) = model.headers.iter().position(|h| h == "notes") {
        for (i, row) in model.rows.iter_mut().enumerate() {
            if notes_idx < row.len() {
                row[notes_idx] = format!("Query hash: {:08x} | {}", fxhash(query), row[notes_idx]);
            }
            if i < 3 && !row.is_empty() {
                row[0] = format!("{}*", row[0]);
            }
        }
    }
    model
}

fn fxhash(s: &str) -> u32 {
    // Small non-cryptographic hash for the stub.
    let mut h: u32 = 2166136261;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}
