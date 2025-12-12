use std::env;
use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use tsql::app::App;
use tsql::config;
use tsql::ui::GridModel;

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

    // Load configuration from ~/.config/tsql/config.toml
    let cfg = config::load_config().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load config: {}", e);
        config::Config::default()
    });

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
        conn_str,
        cfg,
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
