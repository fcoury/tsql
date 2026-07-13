//! Session state persistence for restoring app state between launches.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::config::config_dir;

/// Current session file schema version.
const SESSION_VERSION: u32 = 2;
const NOTEBOOK_FILE_VERSION: u32 = 1;
pub const MAX_NOTEBOOK_RUN_HISTORY: usize = 20;
const MAX_NOTEBOOK_RUN_SOURCE_CHARS: usize = 64 * 1024;
const MAX_NOTEBOOK_RUN_PREVIEW_ROWS: usize = 5;
const MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS: usize = 24;
const MAX_NOTEBOOK_RUN_PREVIEW_CELL_CHARS: usize = 240;
const MAX_NOTEBOOK_RUN_ERROR_CHARS: usize = 8 * 1024;

/// Durable status for a completed notebook-cell execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotebookRunStatus {
    Succeeded,
    Failed,
    Cancelled,
}

/// A compact, portable record of a previous cell execution and its output preview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookRunRecord {
    pub execution_id: u64,
    pub source_revision: u64,
    pub source: String,
    pub status: NotebookRunStatus,
    pub finished_at: DateTime<Utc>,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

impl NotebookRunRecord {
    /// Creates a durable run record. Call `with_output` or `with_error` for details.
    pub fn new(
        execution_id: u64,
        source_revision: u64,
        source: String,
        status: NotebookRunStatus,
    ) -> Self {
        Self {
            execution_id,
            source_revision,
            source,
            status,
            finished_at: Utc::now(),
            elapsed_ms: 0,
            row_count: 0,
            headers: Vec::new(),
            rows: Vec::new(),
            error: None,
            truncated: false,
        }
    }

    /// Captures a compact output preview from a completed execution.
    pub fn with_output(
        mut self,
        elapsed: Duration,
        row_count: usize,
        headers: &[String],
        rows: &[Vec<String>],
        truncated: bool,
    ) -> Self {
        self.elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.row_count = row_count;
        self.headers = headers
            .iter()
            .take(MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS)
            .cloned()
            .collect();
        self.rows = rows
            .iter()
            .take(MAX_NOTEBOOK_RUN_PREVIEW_ROWS)
            .cloned()
            .collect();
        self.truncated = truncated || row_count > self.rows.len();
        self.bounded()
    }

    /// Captures a failed or cancelled execution's error and elapsed time.
    pub fn with_error(mut self, elapsed: Duration, error: Option<String>) -> Self {
        self.elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.error = error;
        self.bounded()
    }

    /// Bounds all variable-sized fields so one cell's history cannot grow without limit.
    pub fn bounded(mut self) -> Self {
        self.source = truncate_chars(self.source, MAX_NOTEBOOK_RUN_SOURCE_CHARS);
        self.headers.truncate(MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS);
        self.headers = self
            .headers
            .into_iter()
            .map(|header| truncate_chars(header, MAX_NOTEBOOK_RUN_PREVIEW_CELL_CHARS))
            .collect();
        self.rows.truncate(MAX_NOTEBOOK_RUN_PREVIEW_ROWS);
        for row in &mut self.rows {
            row.truncate(MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS);
            for value in row {
                *value = truncate_chars(std::mem::take(value), MAX_NOTEBOOK_RUN_PREVIEW_CELL_CHARS);
            }
        }
        self.error = self
            .error
            .map(|error| truncate_chars(error, MAX_NOTEBOOK_RUN_ERROR_CHARS));
        self
    }
}

/// Keeps the newest bounded run records in chronological order.
pub fn bounded_notebook_run_history(mut history: Vec<NotebookRunRecord>) -> Vec<NotebookRunRecord> {
    if history.len() > MAX_NOTEBOOK_RUN_HISTORY {
        history.drain(..history.len() - MAX_NOTEBOOK_RUN_HISTORY);
    }
    history
        .into_iter()
        .map(NotebookRunRecord::bounded)
        .collect()
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// Persisted notebook cell state. Runtime outputs and snapshot handles are excluded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookCellSession {
    /// Stable runtime cell identity used by persisted structural dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_id: Option<u64>,
    /// Optional stable name exposed as `@result_<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_name: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_collapsed: bool,
    #[serde(default)]
    pub output_collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<NotebookDependencySession>,
    /// Additional immutable sources for a cell that joins multiple results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_dependencies: Vec<NotebookDependencySession>,
    /// Bounded metadata and output previews from previous cell executions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_history: Vec<NotebookRunRecord>,
}

/// Persisted structural lineage to an immutable result version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookDependencySession {
    pub source_cell_id: u64,
    pub source_execution_id: u64,
    pub source_revision: u64,
}

/// Persisted notebook document state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookSession {
    #[serde(default)]
    pub cells: Vec<NotebookCellSession>,
    #[serde(default)]
    pub selected_index: usize,
}

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

    /// Whether sidebar is visible (default: false to match App default).
    #[serde(default)]
    pub sidebar_visible: bool,

    /// Last active workspace (`classic` or `notebook`).
    #[serde(default = "default_workspace")]
    pub workspace: String,

    /// Notebook sources and document UI state.
    #[serde(default)]
    pub notebook: NotebookSession,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            connection_name: None,
            editor_content: String::new(),
            schema_expanded: Vec::new(),
            sidebar_visible: false,
            workspace: default_workspace(),
            notebook: NotebookSession::default(),
        }
    }
}

fn default_workspace() -> String {
    "classic".to_string()
}

fn bounded_notebook_session(mut notebook: NotebookSession) -> NotebookSession {
    for cell in &mut notebook.cells {
        cell.execution_history =
            bounded_notebook_run_history(std::mem::take(&mut cell.execution_history));
    }
    notebook
}

fn source_only_notebook_session(mut notebook: NotebookSession) -> NotebookSession {
    for cell in &mut notebook.cells {
        cell.execution_history.clear();
    }
    notebook
}

/// The session file format with versioning.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    #[serde(flatten)]
    state: SessionState,
}

/// Portable, source-only notebook document. Runtime outputs and backend handles
/// are deliberately excluded so documents can be safely reopened or shared.
#[derive(Debug, Serialize, Deserialize)]
struct NotebookFile {
    version: u32,
    #[serde(flatten)]
    notebook: NotebookSession,
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
/// Returns default state if file is missing or corrupted (graceful degradation).
pub fn load_session_from_path(path: &Path) -> Result<SessionState> {
    if !path.exists() {
        return Ok(SessionState::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;

    // Parse errors are treated as corrupted file - return default state
    let file: SessionFile = match serde_json::from_str(&content) {
        Ok(f) => f,
        Err(_) => {
            // Corrupted session file - treat as missing and return default
            return Ok(SessionState::default());
        }
    };

    // Handle future version migrations here if needed.
    if file.version > SESSION_VERSION {
        // Future version - return default to be safe.
        return Ok(SessionState::default());
    }

    let mut state = file.state;
    state.notebook = bounded_notebook_session(state.notebook);
    Ok(state)
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

    let mut saved_state = state.clone();
    saved_state.notebook = bounded_notebook_session(saved_state.notebook);
    let file = SessionFile {
        version: SESSION_VERSION,
        state: saved_state,
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

    // Set restrictive permissions on Unix (best-effort, queries can be sensitive)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

/// Load a portable notebook document from a specific path.
pub fn load_notebook_from_path(path: &Path) -> Result<NotebookSession> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read notebook file: {}", path.display()))?;
    let file: NotebookFile = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse notebook file: {}", path.display()))?;
    anyhow::ensure!(
        file.version <= NOTEBOOK_FILE_VERSION,
        "Notebook file version {} is newer than the supported version {}",
        file.version,
        NOTEBOOK_FILE_VERSION
    );
    Ok(source_only_notebook_session(file.notebook))
}

/// Save a portable notebook document atomically. Query sources can contain
/// sensitive information, so the resulting file is owner-readable on Unix.
pub fn save_notebook_to_path(notebook: &NotebookSession, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create notebook directory: {}", parent.display()))?;

    let content = serde_json::to_string_pretty(&NotebookFile {
        version: NOTEBOOK_FILE_VERSION,
        notebook: source_only_notebook_session(notebook.clone()),
    })
    .context("Failed to serialize notebook")?;
    let mut tmp = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "Failed to create temporary notebook file in: {}",
            parent.display()
        )
    })?;
    tmp.write_all(content.as_bytes())
        .context("Failed to write temporary notebook file")?;
    tmp.flush()
        .context("Failed to flush temporary notebook file")?;
    tmp.persist(path)
        .map_err(|error| anyhow::anyhow!("Failed to persist notebook file: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn portable_notebook_round_trips_sources_lineage_and_ui_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("investigation.tsql-notebook.json");
        let notebook = NotebookSession {
            cells: vec![
                NotebookCellSession {
                    cell_id: Some(4),
                    result_name: Some("recent_users".to_string()),
                    source: "SELECT 1 AS value".to_string(),
                    source_collapsed: true,
                    output_collapsed: true,
                    dependency: None,
                    additional_dependencies: Vec::new(),
                    execution_history: vec![NotebookRunRecord {
                        execution_id: 8,
                        source_revision: 1,
                        source: "SELECT 1 AS value".to_string(),
                        status: NotebookRunStatus::Succeeded,
                        finished_at: "2026-07-11T12:00:00Z".parse().unwrap(),
                        elapsed_ms: 12,
                        row_count: 1,
                        headers: vec!["value".to_string()],
                        rows: vec![vec!["1".to_string()]],
                        error: None,
                        truncated: false,
                    }],
                },
                NotebookCellSession {
                    cell_id: Some(7),
                    result_name: None,
                    source: "SELECT * FROM @result_4".to_string(),
                    source_collapsed: false,
                    output_collapsed: false,
                    dependency: Some(NotebookDependencySession {
                        source_cell_id: 4,
                        source_execution_id: 9,
                        source_revision: 2,
                    }),
                    additional_dependencies: vec![NotebookDependencySession {
                        source_cell_id: 5,
                        source_execution_id: 10,
                        source_revision: 1,
                    }],
                    execution_history: Vec::new(),
                },
            ],
            selected_index: 1,
        };

        save_notebook_to_path(&notebook, &path).unwrap();
        let restored = load_notebook_from_path(&path).unwrap();

        let mut expected = notebook.clone();
        expected.cells[0].execution_history.clear();
        assert_eq!(restored, expected);
        let serialized = fs::read_to_string(path).unwrap();
        assert!(serialized.contains("\"version\": 1"));
        assert!(!serialized.contains("connection_name"));
        assert!(!serialized.contains("editor_content"));
        assert!(!serialized.contains("execution_history"));
    }

    #[test]
    fn portable_notebook_rejects_invalid_and_future_documents() {
        let dir = tempdir().unwrap();
        let invalid = dir.path().join("invalid.json");
        fs::write(&invalid, "not json").unwrap();
        assert!(load_notebook_from_path(&invalid).is_err());

        let future = dir.path().join("future.json");
        fs::write(&future, r#"{"version":2,"cells":[],"selected_index":0}"#).unwrap();
        let error = load_notebook_from_path(&future).unwrap_err().to_string();
        assert!(error.contains("newer than the supported version"));
    }

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
            ..SessionState::default()
        };

        save_session_to_path(&state, &path).unwrap();

        let loaded = load_session_from_path(&path).unwrap();
        assert_eq!(loaded.connection_name, Some("myconn".to_string()));
        assert_eq!(loaded.editor_content, "SELECT * FROM users");
        assert_eq!(loaded.schema_expanded.len(), 2);
        assert!(!loaded.sidebar_visible);
    }

    #[test]
    fn test_corrupted_file_returns_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "not valid json {{{").unwrap();

        // Corrupted files should gracefully return default state
        let state = load_session_from_path(&path).unwrap();
        assert!(state.connection_name.is_none());
        assert!(state.editor_content.is_empty());
        assert!(!state.sidebar_visible);
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
        assert_eq!(loaded.workspace, "classic");
        assert!(loaded.notebook.cells.is_empty());
    }

    #[test]
    fn test_notebook_state_round_trip_excludes_runtime_outputs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let state = SessionState {
            workspace: "notebook".to_string(),
            notebook: NotebookSession {
                cells: vec![NotebookCellSession {
                    cell_id: Some(3),
                    result_name: None,
                    source: "SELECT * FROM @result_1".to_string(),
                    source_collapsed: false,
                    output_collapsed: true,
                    dependency: Some(NotebookDependencySession {
                        source_cell_id: 1,
                        source_execution_id: 4,
                        source_revision: 2,
                    }),
                    additional_dependencies: Vec::new(),
                    execution_history: Vec::new(),
                }],
                selected_index: 0,
            },
            ..SessionState::default()
        };

        save_session_to_path(&state, &path).unwrap();
        let loaded = load_session_from_path(&path).unwrap();

        assert_eq!(loaded.workspace, "notebook");
        assert_eq!(loaded.notebook, state.notebook);
    }

    #[test]
    fn test_v2_notebook_cells_without_ids_remain_compatible() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let content = r#"{
            "version": 2,
            "workspace": "notebook",
            "notebook": {
                "cells": [
                    {"source": "SELECT 1"},
                    {
                        "source": "SELECT * FROM @result_1",
                        "dependency": {
                            "source_cell_id": 1,
                            "source_execution_id": 4,
                            "source_revision": 2
                        }
                    }
                ],
                "selected_index": 1
            }
        }"#;
        fs::write(&path, content).unwrap();

        let loaded = load_session_from_path(&path).unwrap();

        assert_eq!(loaded.workspace, "notebook");
        assert_eq!(loaded.notebook.selected_index, 1);
        assert!(loaded
            .notebook
            .cells
            .iter()
            .all(|cell| cell.cell_id.is_none()));
        assert_eq!(
            loaded.notebook.cells[1].dependency,
            Some(NotebookDependencySession {
                source_cell_id: 1,
                source_execution_id: 4,
                source_revision: 2,
            })
        );
    }

    #[test]
    fn notebook_run_history_is_bounded_and_preserves_newest_records() {
        let oversized = || NotebookRunRecord {
            execution_id: 0,
            source_revision: 1,
            source: "界".repeat(MAX_NOTEBOOK_RUN_SOURCE_CHARS + 50),
            status: NotebookRunStatus::Failed,
            finished_at: "2026-07-11T12:00:00Z".parse().unwrap(),
            elapsed_ms: 25,
            row_count: 999,
            headers: vec!["h".repeat(300); MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS + 3],
            rows: vec![
                vec!["界".repeat(300); MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS + 3];
                MAX_NOTEBOOK_RUN_PREVIEW_ROWS + 3
            ],
            error: Some("e".repeat(MAX_NOTEBOOK_RUN_ERROR_CHARS + 10)),
            truncated: true,
        };
        let history = (0..MAX_NOTEBOOK_RUN_HISTORY + 4)
            .map(|index| NotebookRunRecord {
                execution_id: index as u64,
                ..oversized()
            })
            .collect();

        let history = bounded_notebook_run_history(history);
        assert_eq!(history.len(), MAX_NOTEBOOK_RUN_HISTORY);
        assert_eq!(history.first().unwrap().execution_id, 4);
        let newest = history.last().unwrap();
        assert_eq!(newest.source.chars().count(), MAX_NOTEBOOK_RUN_SOURCE_CHARS);
        assert!(newest.source.ends_with('…'));
        assert_eq!(newest.headers.len(), MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS);
        assert_eq!(
            newest.headers[0].chars().count(),
            MAX_NOTEBOOK_RUN_PREVIEW_CELL_CHARS
        );
        assert_eq!(newest.rows.len(), MAX_NOTEBOOK_RUN_PREVIEW_ROWS);
        assert_eq!(newest.rows[0].len(), MAX_NOTEBOOK_RUN_PREVIEW_COLUMNS);
        assert_eq!(
            newest.rows[0][0].chars().count(),
            MAX_NOTEBOOK_RUN_PREVIEW_CELL_CHARS
        );
        assert_eq!(
            newest.error.as_ref().unwrap().chars().count(),
            MAX_NOTEBOOK_RUN_ERROR_CHARS
        );
        assert_eq!(newest.row_count, 999);
    }

    #[test]
    fn notebook_run_builders_capture_compact_output_and_error_details() {
        let headers = vec!["value".to_string()];
        let rows = (0..8).map(|row| vec![row.to_string()]).collect::<Vec<_>>();
        let succeeded = NotebookRunRecord::new(
            7,
            2,
            "SELECT value FROM source".to_string(),
            NotebookRunStatus::Succeeded,
        )
        .with_output(
            Duration::from_millis(17),
            rows.len(),
            &headers,
            &rows,
            false,
        );
        assert_eq!(succeeded.execution_id, 7);
        assert_eq!(succeeded.elapsed_ms, 17);
        assert_eq!(succeeded.row_count, 8);
        assert_eq!(succeeded.rows.len(), MAX_NOTEBOOK_RUN_PREVIEW_ROWS);
        assert!(succeeded.truncated);
        assert_eq!(succeeded.headers, headers);

        let failed = NotebookRunRecord::new(
            8,
            3,
            "SELECT missing".to_string(),
            NotebookRunStatus::Failed,
        )
        .with_error(
            Duration::from_millis(23),
            Some("column missing does not exist".to_string()),
        );
        assert_eq!(failed.elapsed_ms, 23);
        assert_eq!(
            failed.error.as_deref(),
            Some("column missing does not exist")
        );
        assert!(failed.rows.is_empty());
    }

    #[test]
    fn legacy_notebook_without_execution_history_still_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy-notebook.json");
        fs::write(
            &path,
            r#"{"version":1,"cells":[{"cell_id":1,"source":"SELECT 1","output_collapsed":false}],"selected_index":0}"#,
        )
        .unwrap();

        let restored = load_notebook_from_path(&path).unwrap();
        assert_eq!(restored.cells.len(), 1);
        assert!(restored.cells[0].execution_history.is_empty());
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
