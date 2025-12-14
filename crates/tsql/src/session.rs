//! Session state persistence for restoring app state between launches.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// Current session file schema version.
const SESSION_VERSION: u32 = 1;

/// Serializable session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Whether sidebar is visible.
    #[serde(default = "default_sidebar_visible")]
    pub sidebar_visible: bool,
}

fn default_sidebar_visible() -> bool {
    true
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            connection_name: None,
            editor_content: String::new(),
            schema_expanded: Vec::new(),
            sidebar_visible: true,
        }
    }
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

/// Returns the session file path (~/.config/tsql/session.json).
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
pub fn save_session_to_path(state: &SessionState, path: &Path) -> Result<()> {
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let file = SessionFile {
        version: SESSION_VERSION,
        state: state.clone(),
    };

    let content = serde_json::to_string_pretty(&file).context("Failed to serialize session")?;

    fs::write(path, content)
        .with_context(|| format!("Failed to write session file: {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tsql_session_test_{}_{}.json",
            std::process::id(),
            id
        ));
        path
    }

    #[test]
    fn test_load_missing_file() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        let state = load_session_from_path(&path).unwrap();
        assert!(state.connection_name.is_none());
        assert!(state.editor_content.is_empty());
        assert!(state.sidebar_visible);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_save_and_load() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

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

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_corrupted_file_returns_error() {
        let path = temp_path();
        fs::write(&path, "not valid json {{{").unwrap();

        let result = load_session_from_path(&path);
        assert!(result.is_err());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_default_values() {
        let state = SessionState::default();
        assert!(state.connection_name.is_none());
        assert!(state.editor_content.is_empty());
        assert!(state.schema_expanded.is_empty());
        assert!(state.sidebar_visible);
    }

    #[test]
    fn test_partial_json_uses_defaults() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        // Write a session with only some fields.
        let content = r#"{"version": 1, "editor_content": "SELECT 1"}"#;
        fs::write(&path, content).unwrap();

        let loaded = load_session_from_path(&path).unwrap();
        assert!(loaded.connection_name.is_none());
        assert_eq!(loaded.editor_content, "SELECT 1");
        assert!(loaded.schema_expanded.is_empty());
        assert!(loaded.sidebar_visible); // default

        fs::remove_file(&path).ok();
    }
}
