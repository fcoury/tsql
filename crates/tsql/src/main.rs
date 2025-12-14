use std::env;
use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
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
