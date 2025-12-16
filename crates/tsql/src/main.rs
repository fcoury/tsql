use std::env;
use std::io::{self, Stdout, Write};

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use tsql::app::App;
use tsql::config::{self, load_connections};
use tsql::session::load_session;
use tsql::ui::GridModel;

fn print_version() {
    println!("tsql {}", env!("CARGO_PKG_VERSION"));
}

fn print_usage() {
    eprintln!("tsql - A modern PostgreSQL CLI");
    eprintln!();
    eprintln!("Usage: tsql [OPTIONS] [CONNECTION_URL]");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  [CONNECTION_URL]  PostgreSQL connection URL");
    eprintln!("                    (e.g., postgres://user:pass@host:5432/dbname)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -h, --help        Print this help message");
    eprintln!("  -V, --version     Print version information");
    eprintln!("      --debug-keys  Print detected key/mouse events (for troubleshooting)");
    eprintln!("      --mouse       (with --debug-keys) Also print mouse events");
    eprintln!();
    eprintln!("Environment Variables:");
    eprintln!("  DATABASE_URL      Default connection URL if not provided as argument");
    eprintln!();
    eprintln!("Configuration:");
    if let Some(path) = config::config_path() {
        eprintln!("  Config file: {}", path.display());
    }
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  tsql postgres://localhost/mydb");
    eprintln!("  DATABASE_URL=postgres://localhost/mydb tsql");
    eprintln!("  tsql --debug-keys");
    eprintln!("  tsql --debug-keys --mouse");
}

fn main() -> Result<()> {
    // Parse command-line arguments
    let args: Vec<String> = env::args().collect();

    // Check for help flag
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }

    // Check for version flag
    if args.iter().any(|a| a == "-V" || a == "--version") {
        print_version();
        return Ok(());
    }

    // Key debug mode (helps identify what the terminal actually sends)
    let debug_keys_mode = args
        .iter()
        .any(|a| a == "--debug-keys" || a == "--debug-keys-mouse")
        || args.get(1).is_some_and(|a| a == "debug-keys");
    if debug_keys_mode {
        let debug_mouse = args
            .iter()
            .any(|a| a == "--debug-keys-mouse" || a == "--mouse");
        return run_debug_keys(debug_mouse);
    }

    // Load configuration from ~/.config/tsql/config.toml
    let cfg = config::load_config().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load config: {}", e);
        config::Config::default()
    });

    // Load session state if persistence is enabled
    let session = if cfg.editor.persist_session {
        load_session().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load session: {}", e);
            Default::default()
        })
    } else {
        Default::default()
    };

    // Connection string priority: CLI arg > DATABASE_URL env var > config file
    let conn_str = if args.len() > 1 && !args[1].starts_with('-') {
        // First argument is the connection string
        Some(args[1].clone())
    } else {
        // Fall back to DATABASE_URL environment variable
        env::var("DATABASE_URL").ok()
    };

    let rt = Runtime::new().context("failed to initialize tokio runtime")?;
    let (db_events_tx, db_events_rx) = mpsc::unbounded_channel();

    let mut terminal =
        init_terminal().context("failed to initialize terminal; are you running in a real TTY?")?;

    let mut app = App::with_config(
        GridModel::empty(),
        rt.handle().clone(),
        db_events_tx,
        db_events_rx,
        conn_str.clone(),
        cfg,
    );

    // Apply session state (editor content, sidebar visibility, pending schema expanded)
    let session_connection = app.apply_session_state(session);

    // Auto-connect from session if no CLI/env connection was specified
    let mut session_reconnected = false;
    if conn_str.is_none() {
        if let Some(conn_name) = session_connection {
            // Verify connection still exists
            let connections = load_connections().unwrap_or_default();
            if let Some(entry) = connections.find_by_name(&conn_name) {
                // Check if password is available (not requiring prompt)
                match entry.get_password() {
                    Ok(Some(_)) | Ok(None) => {
                        // Password available or not needed - auto-connect
                        app.connect_to_entry(entry.clone());
                        session_reconnected = true;
                    }
                    Err(_) => {
                        // Password retrieval failed - skip auto-connect
                        // User can manually connect
                    }
                }
            }
            // If connection doesn't exist, silently skip auto-connect
        }

        // Only open connection picker if no connection was established
        // (no CLI/env URL and no session reconnection)
        if !session_reconnected {
            app.open_connection_picker();
        }
    }

    let res = app.run(&mut terminal);

    restore_terminal(terminal)?;

    res
}

fn run_debug_keys(with_mouse: bool) -> Result<()> {
    struct DebugTerminalGuard {
        stdout: Stdout,
        with_mouse: bool,
    }

    impl DebugTerminalGuard {
        fn new(with_mouse: bool) -> Result<Self> {
            enable_raw_mode()?;
            let mut stdout = io::stdout();
            if with_mouse {
                execute!(stdout, EnableMouseCapture)?;
            }
            Ok(Self { stdout, with_mouse })
        }

        fn println(&mut self, line: &str) -> Result<()> {
            write!(self.stdout, "\r\n{line}")?;
            self.stdout.flush()?;
            Ok(())
        }
    }

    impl Drop for DebugTerminalGuard {
        fn drop(&mut self) {
            if self.with_mouse {
                let _ = execute!(self.stdout, DisableMouseCapture);
            }
            let _ = disable_raw_mode();
            let _ = self.stdout.flush();
        }
    }

    fn describe_key(key: KeyEvent) -> String {
        let mut parts = Vec::new();
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            parts.push("Ctrl");
        }
        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) {
            parts.push("Alt");
        }
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT)
        {
            parts.push("Shift");
        }

        let key_name = match key.code {
            KeyCode::Char(c) => format!("{c:?}"),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "BackTab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };

        if parts.is_empty() {
            key_name
        } else {
            format!("{}+{key_name}", parts.join("+"))
        }
    }

    let mut guard = DebugTerminalGuard::new(with_mouse)?;
    let mouse_msg = if with_mouse { "on" } else { "off" };
    guard.println(&format!(
        "tsql --debug-keys (mouse: {mouse_msg}) (press Esc or Ctrl+C to exit)"
    ))?;

    loop {
        let ev = event::read()?;
        match ev {
            Event::Key(key) => {
                let desc = describe_key(key);
                guard.println(&format!("Key: {desc}    raw={key:?}"))?;

                let is_ctrl_c = key.code == KeyCode::Char('c')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL);
                if key.code == KeyCode::Esc || is_ctrl_c {
                    break;
                }
            }
            Event::Mouse(mouse) => {
                if with_mouse {
                    guard.println(&format!("Mouse: {mouse:?}"))?;
                }
            }
            Event::Resize(w, h) => {
                guard.println(&format!("Resize: {w}x{h}"))?;
            }
            Event::Paste(text) => {
                guard.println(&format!("Paste: {:?} ({} bytes)", text, text.len()))?;
            }
            other => {
                guard.println(&format!("Event: {other:?}"))?;
            }
        }
    }

    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
