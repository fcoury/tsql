//! Configuration module for tsql.
//!
//! Handles loading and managing configuration from:
//! - Default values
//! - Config file (~/.config/tsql/config.toml)
//! - Environment variables
//! - Command-line arguments (future)

mod connections;
mod keymap;
mod schema;

pub use connections::{
    connections_path, load_connections, save_connections, ConnectionColor, ConnectionEntry,
    ConnectionsFile, SslMode,
};
pub use keymap::{Action, KeyBinding, Keymap};
pub use schema::{
    Config, ConnectionConfig, CustomKeyBinding, DisplayConfig, EditorConfig, IdentifierStyle,
    KeymapConfig, SqlConfig,
};

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Returns the config directory path.
///
/// Checks `TSQL_CONFIG_DIR` environment variable first, then falls back
/// to the system default (~/.config/tsql on Linux/macOS).
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TSQL_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::config_dir().map(|p| p.join("tsql"))
}

/// Returns the default config file path (~/.config/tsql/config.toml)
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("config.toml"))
}

/// Returns the history file path (~/.config/tsql/history)
pub fn history_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("history"))
}

/// Load configuration from the default path or return defaults
pub fn load_config() -> Result<Config> {
    if let Some(path) = config_path() {
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;
            let config: Config = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
            return Ok(config);
        }
    }
    Ok(Config::default())
}

/// Load configuration from a specific path
pub fn load_config_from(path: &PathBuf) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.display.show_row_numbers);
        assert_eq!(config.editor.tab_size, 4);
    }

    #[test]
    fn test_config_paths() {
        // These should return Some on most systems
        let config_dir = config_dir();
        let config_path = config_path();
        let history_path = history_path();

        // Just verify they're consistent
        if let (Some(dir), Some(cfg), Some(hist)) = (config_dir, config_path, history_path) {
            assert!(cfg.starts_with(&dir));
            assert!(hist.starts_with(&dir));
            assert!(cfg.ends_with("config.toml"));
            assert!(hist.ends_with("history"));
        }
    }

    #[test]
    fn test_parse_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn test_parse_partial_config() {
        let toml = r#"
[display]
show_row_numbers = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(!config.display.show_row_numbers);
        // Other fields should be default
        assert_eq!(config.editor.tab_size, 4);
    }
}
