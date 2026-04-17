//! Configuration module for tsql.
//!
//! Handles loading and managing configuration from:
//! - Default values
//! - Config file (~/.tsql/config.toml)
//! - Environment variables
//! - Command-line arguments (future)

mod connections;
mod keymap;
mod schema;

pub use connections::{
    connections_path, export_to_path, import_from_path, load_connections, save_connections,
    write_connections_atomic, ConnectionColor, ConnectionEntry, ConnectionsFile, DbKind,
    ImportConflict, ImportSummary, SortMode, SslMode,
};
pub use keymap::{Action, KeyBinding, Keymap};
pub use schema::{
    ClipboardBackend, ClipboardConfig, Config, ConnectionConfig, CustomKeyBinding, DisplayConfig,
    EditorConfig, IdentifierStyle, KeymapConfig, SqlConfig, UpdateChannel, UpdateMode,
    UpdatesConfig,
};

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[cfg(not(unix))]
fn platform_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("tsql"))
}

#[cfg(unix)]
fn unix_dot_tsql_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".tsql"))
}

#[cfg(unix)]
fn unix_legacy_xdg_config_dir() -> Option<PathBuf> {
    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg_config_home).join("tsql"));
    }

    dirs::home_dir().map(|p| p.join(".config").join("tsql"))
}

#[cfg(unix)]
fn unix_legacy_platform_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("tsql"))
}

#[cfg(unix)]
fn has_tsql_state(dir: &Path) -> bool {
    ["config.toml", "connections.toml", "history", "session.json"]
        .iter()
        .any(|name| dir.join(name).exists())
}

#[cfg(unix)]
fn legacy_config_dirs(target: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    for dir in [
        unix_legacy_xdg_config_dir(),
        unix_legacy_platform_config_dir(),
    ]
    .into_iter()
    .flatten()
    {
        if dir != target && !dirs.iter().any(|existing| existing == &dir) {
            dirs.push(dir);
        }
    }

    dirs
}

#[cfg(unix)]
fn move_path(src: &Path, dst: &Path) -> Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            if src.is_dir() {
                copy_dir_recursive(src, dst)?;
                std::fs::remove_dir_all(src).with_context(|| {
                    format!(
                        "Failed to remove source directory after copy: {}",
                        src.display()
                    )
                })?;
            } else {
                std::fs::copy(src, dst).with_context(|| {
                    format!(
                        "Failed to copy file from {} to {}",
                        src.display(),
                        dst.display()
                    )
                })?;
                std::fs::remove_file(src).with_context(|| {
                    format!("Failed to remove source file after copy: {}", src.display())
                })?;
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create directory: {}", dst.display()))?;

    for entry in std::fs::read_dir(src)
        .with_context(|| format!("Failed to read directory: {}", src.display()))?
    {
        let entry = entry.with_context(|| format!("Failed to read entry in {}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!(
                    "Failed to copy file from {} to {}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }

    Ok(())
}

#[cfg(unix)]
fn merge_legacy_dir_into_target(source: &Path, target: &Path) -> Result<()> {
    for entry in std::fs::read_dir(source)
        .with_context(|| format!("Failed to read directory: {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("Failed to read entry in {}", source.display()))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if target_path.exists() {
            continue;
        }

        move_path(&source_path, &target_path)?;
    }

    let source_is_empty = std::fs::read_dir(source)
        .with_context(|| format!("Failed to read directory: {}", source.display()))?
        .next()
        .is_none();
    if source_is_empty {
        std::fs::remove_dir(source).with_context(|| {
            format!(
                "Failed to remove legacy config directory after migration: {}",
                source.display()
            )
        })?;
    }

    Ok(())
}

#[cfg(unix)]
fn migrate_legacy_dirs(target: &Path, legacy_dirs: &[PathBuf]) -> Result<()> {
    let mut sources: Vec<PathBuf> = legacy_dirs
        .iter()
        .filter(|dir| has_tsql_state(dir))
        .cloned()
        .collect();

    if sources.is_empty() {
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    if !target.exists() {
        let source = sources.remove(0);
        move_path(&source, target).with_context(|| {
            format!(
                "Failed to migrate legacy config directory from {} to {}",
                source.display(),
                target.display()
            )
        })?;
    }

    std::fs::create_dir_all(target)
        .with_context(|| format!("Failed to create config directory: {}", target.display()))?;

    for source in sources {
        if !source.exists() {
            continue;
        }
        merge_legacy_dir_into_target(&source, target)?;
    }

    Ok(())
}

/// Migrate legacy Unix config directories to `~/.tsql` when present.
///
/// This runs at startup and is a no-op on non-Unix platforms or when
/// `TSQL_CONFIG_DIR` is explicitly set.
pub fn migrate_legacy_config_dir_on_startup() -> Result<()> {
    #[cfg(unix)]
    {
        if std::env::var("TSQL_CONFIG_DIR").is_ok() {
            return Ok(());
        }

        if let Some(target) = unix_dot_tsql_config_dir() {
            let legacy_dirs = legacy_config_dirs(&target);
            migrate_legacy_dirs(&target, &legacy_dirs)?;
        }
    }

    Ok(())
}

/// Returns the config directory path.
///
/// Checks `TSQL_CONFIG_DIR` environment variable first, then falls back
/// to `~/.tsql` on Unix-like systems by default.
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TSQL_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }

    #[cfg(unix)]
    {
        unix_dot_tsql_config_dir()
    }

    #[cfg(not(unix))]
    {
        platform_config_dir()
    }
}

/// Returns the default config file path (`~/.tsql/config.toml` on Unix).
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("config.toml"))
}

/// Returns the history file path (inside `config_dir()`).
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
    #[cfg(unix)]
    use tempfile::TempDir;

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

    #[cfg(unix)]
    #[test]
    fn test_migrate_legacy_dirs_moves_legacy_dir_when_target_missing() {
        let root = TempDir::new().unwrap();
        let target = root.path().join(".tsql");
        let legacy = root.path().join(".config").join("tsql");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("config.toml"), "[connection]\nmax_rows=100\n").unwrap();
        std::fs::write(legacy.join("connections.toml"), "connections = []\n").unwrap();

        migrate_legacy_dirs(&target, std::slice::from_ref(&legacy)).unwrap();

        assert!(target.join("config.toml").exists());
        assert!(target.join("connections.toml").exists());
        assert!(!legacy.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_migrate_legacy_dirs_merges_without_overwriting_target_files() {
        let root = TempDir::new().unwrap();
        let target = root.path().join(".tsql");
        let legacy = root.path().join("legacy").join("tsql");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();

        std::fs::write(target.join("config.toml"), "[connection]\nmax_rows=1\n").unwrap();
        std::fs::write(legacy.join("config.toml"), "[connection]\nmax_rows=2\n").unwrap();
        std::fs::write(legacy.join("history"), "select 1;\n").unwrap();

        migrate_legacy_dirs(&target, std::slice::from_ref(&legacy)).unwrap();

        let target_config = std::fs::read_to_string(target.join("config.toml")).unwrap();
        assert!(target_config.contains("max_rows=1"));
        assert!(target.join("history").exists());
        assert!(legacy.join("config.toml").exists());
    }
}
