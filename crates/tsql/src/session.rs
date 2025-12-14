//! Session state persistence for restoring app state between launches.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::config::config_dir;

/// Current session file schema version.
const SESSION_VERSION: u32 = 1;

/// Serializable session state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// Active connection name (if connected via saved connection).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub connection_name: Option<String>,

    /// Query editor content.
    #[serde(default)]
    pub editor_content: String,

    /// Expanded schema tree node identifiers.
    /// Each entry is the path to an expanded node (e.g., ["public", "users"]).
    #[serde(default)]
    pub schema_expanded: Vec<Vec<String>>,

    /// Whether sidebar is visible (default: false to match App default).
    #[serde(default)]
    pub sidebar_visible: bool,
}

/// The session file format with versioning.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    #[serde(flatten)]
    state: SessionState,
}

impl Default for SessionFile {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            state: SessionState::default(),
        }
    }
}

/// Returns the session file path (`<config_dir>/session.json`).
pub fn session_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("session.json"))
}

/// Load session state from the default path.
pub fn load_session() -> Result<SessionState> {
    let path = session_path().context("Could not determine session path")?;
    load_session_from_path(&path)
}

/// Load session state from a specific path.
pub fn load_session_from_path(path: &Path) -> Result<SessionState> {
    if !path.exists() {
        return Ok(SessionState::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;

    let file: SessionFile = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session file: {}", path.display()))?;

    // Handle future version migrations here if needed.
    if file.version > SESSION_VERSION {
        // Future version - return default to be safe.
        return Ok(SessionState::default());
    }

    Ok(file.state)
}

/// Save session state to the default path.
pub fn save_session(state: &SessionState) -> Result<()> {
    let path = session_path().context("Could not determine session path")?;
    save_session_to_path(state, &path)
}

/// Save session state to a specific path.
/// Uses atomic write (temp file + rename) to prevent corruption on crash.
pub fn save_session_to_path(state: &SessionState, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Session path has no parent directory")?;

    // Ensure parent directory exists.
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;

    let file = SessionFile {
        version: SESSION_VERSION,
        state: state.clone(),
    };

    let content = serde_json::to_string_pretty(&file).context("Failed to serialize session")?;

    // Atomic write: temp file in same directory + rename.
    let mut tmp = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "Failed to create temp session file in: {}",
            parent.display()
        )
    })?;

    tmp.write_all(content.as_bytes())
        .context("Failed to write temp session file")?;
    tmp.flush().context("Failed to flush temp session file")?;

    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("Failed to persist session file: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_missing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");

        let state = load_session_from_path(&path).unwrap();
        assert!(state.connection_name.is_none());
        assert!(state.editor_content.is_empty());
        assert!(!state.sidebar_visible); // default false
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");

        let state = SessionState {
            connection_name: Some("myconn".to_string()),
            editor_content: "SELECT * FROM users".to_string(),
            schema_expanded: vec![
                vec!["public".to_string()],
                vec!["public".to_string(), "users".to_string()],
            ],
            sidebar_visible: false,
        };

        save_session_to_path(&state, &path).unwrap();

        let loaded = load_session_from_path(&path).unwrap();
        assert_eq!(loaded.connection_name, Some("myconn".to_string()));
        assert_eq!(loaded.editor_content, "SELECT * FROM users");
        assert_eq!(loaded.schema_expanded.len(), 2);
        assert!(!loaded.sidebar_visible);
    }

    #[test]
    fn test_corrupted_file_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "not valid json {{{").unwrap();

        let result = load_session_from_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_values() {
        let state = SessionState::default();
        assert!(state.connection_name.is_none());
        assert!(state.editor_content.is_empty());
        assert!(state.schema_expanded.is_empty());
        assert!(!state.sidebar_visible); // default false, matches App default
    }

    #[test]
    fn test_partial_json_uses_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");

        // Write a session with only some fields.
        let content = r#"{"version": 1, "editor_content": "SELECT 1"}"#;
        fs::write(&path, content).unwrap();

        let loaded = load_session_from_path(&path).unwrap();
        assert!(loaded.connection_name.is_none());
        assert_eq!(loaded.editor_content, "SELECT 1");
        assert!(loaded.schema_expanded.is_empty());
        assert!(!loaded.sidebar_visible); // default false
    }

    #[test]
    fn test_future_version_returns_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");

        // Write a session with a future version number.
        let content = r#"{"version": 999, "editor_content": "SELECT 1", "sidebar_visible": true}"#;
        fs::write(&path, content).unwrap();

        // Future version should return defaults for safety.
        let loaded = load_session_from_path(&path).unwrap();
        assert!(loaded.connection_name.is_none());
        assert!(loaded.editor_content.is_empty()); // default, not "SELECT 1"
        assert!(!loaded.sidebar_visible); // default false, not true
    }
}
