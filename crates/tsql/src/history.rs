//! Query history management with JSON persistence and fuzzy search.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};
use serde::{Deserialize, Serialize};

use crate::config::history_path;

/// A single history entry with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The SQL query text.
    pub query: String,
    /// When the query was executed.
    pub timestamp: DateTime<Utc>,
    /// Optional connection string (sanitized - no passwords).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
}

impl HistoryEntry {
    pub fn new(query: String, connection: Option<String>) -> Self {
        Self {
            query,
            timestamp: Utc::now(),
            connection,
        }
    }
}

/// The history file format.
#[derive(Debug, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    entries: Vec<HistoryEntry>,
}

impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

/// A match result from fuzzy search.
#[derive(Debug, Clone)]
pub struct HistoryMatch {
    /// Index into the history entries.
    pub index: usize,
    /// The matching entry.
    pub entry: HistoryEntry,
    /// Match score (higher is better).
    pub score: u32,
    /// Character indices that matched (for highlighting).
    pub indices: Vec<u32>,
}

/// Manages query history with persistence and fuzzy search.
pub struct History {
    entries: Vec<HistoryEntry>,
    max_entries: usize,
    path: PathBuf,
    dirty: bool,
}

impl History {
    /// Load history from the default path.
    pub fn load(max_entries: usize) -> Result<Self> {
        let path = history_path().context("Could not determine history path")?;
        Self::load_from_path(&path, max_entries)
    }

    /// Load history from a specific path.
    pub fn load_from_path(path: &Path, max_entries: usize) -> Result<Self> {
        let entries = if path.exists() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read history file: {}", path.display()))?;

            let file: HistoryFile = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse history file: {}", path.display()))?;

            file.entries
        } else {
            Vec::new()
        };

        // Enforce max_entries limit on load.
        let entries = if entries.len() > max_entries {
            let skip_count = entries.len() - max_entries;
            entries.into_iter().skip(skip_count).collect()
        } else {
            entries
        };

        Ok(Self {
            entries,
            max_entries,
            path: path.to_path_buf(),
            dirty: false,
        })
    }

    /// Create a new empty history (for testing or when path isn't available).
    pub fn new_empty(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            path: PathBuf::new(),
            dirty: false,
        }
    }

    /// Save history to disk.
    pub fn save(&mut self) -> Result<()> {
        if !self.dirty || self.path.as_os_str().is_empty() {
            return Ok(());
        }

        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let file = HistoryFile {
            version: 1,
            entries: self.entries.clone(),
        };

        let content = serde_json::to_string_pretty(&file).context("Failed to serialize history")?;

        fs::write(&self.path, content)
            .with_context(|| format!("Failed to write history file: {}", self.path.display()))?;

        self.dirty = false;
        Ok(())
    }

    /// Add a query to history.
    pub fn push(&mut self, query: String, connection: Option<String>) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }

        let entry = HistoryEntry::new(trimmed.to_string(), connection);
        self.entries.push(entry);

        // Enforce max_entries limit.
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }

        self.dirty = true;
    }

    /// Get all history entries (oldest first).
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Search history with fuzzy matching.
    ///
    /// Returns matches sorted by score (best first), with character indices
    /// for highlighting.
    pub fn search(&self, pattern: &str) -> Vec<HistoryMatch> {
        if pattern.is_empty() {
            // Return all entries in reverse order (most recent first).
            return self
                .entries
                .iter()
                .enumerate()
                .rev()
                .map(|(index, entry)| HistoryMatch {
                    index,
                    entry: entry.clone(),
                    score: 0,
                    indices: Vec::new(),
                })
                .collect();
        }

        let mut matcher = Matcher::new(Config::DEFAULT);
        let pat = Pattern::parse(pattern, CaseMatching::Ignore, Normalization::Smart);

        let mut matches: Vec<HistoryMatch> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let mut indices = Vec::new();
                let mut buf = Vec::new();
                let haystack = Utf32Str::new(&entry.query, &mut buf);

                pat.indices(haystack, &mut matcher, &mut indices)
                    .map(|score| HistoryMatch {
                        index,
                        entry: entry.clone(),
                        score,
                        indices,
                    })
            })
            .collect();

        // Sort by score descending (best matches first).
        matches.sort_by(|a, b| b.score.cmp(&a.score));

        matches
    }
}

impl Drop for History {
    fn drop(&mut self) {
        // Try to save on drop, but don't panic on failure.
        let _ = self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tsql_history_test_{}_{}.json",
            std::process::id(),
            id
        ));
        path
    }

    #[test]
    fn test_load_missing_file() {
        let path = temp_path();
        let _ = fs::remove_file(&path); // Ensure it doesn't exist.

        let history = History::load_from_path(&path, 100).unwrap();
        assert!(history.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_save_and_load() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        {
            let mut history = History::load_from_path(&path, 100).unwrap();
            history.push("SELECT * FROM users".to_string(), None);
            history.push(
                "SELECT * FROM orders".to_string(),
                Some("localhost/mydb".to_string()),
            );
            history.save().unwrap();
        }

        let history = History::load_from_path(&path, 100).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history.entries()[0].query, "SELECT * FROM users");
        assert_eq!(history.entries()[1].query, "SELECT * FROM orders");
        assert!(history.entries()[0].connection.is_none());
        assert_eq!(
            history.entries()[1].connection.as_deref(),
            Some("localhost/mydb")
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_max_entries_limit() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        let mut history = History::load_from_path(&path, 3).unwrap();
        history.push("query1".to_string(), None);
        history.push("query2".to_string(), None);
        history.push("query3".to_string(), None);
        history.push("query4".to_string(), None);
        history.push("query5".to_string(), None);

        assert_eq!(history.len(), 3);
        assert_eq!(history.entries()[0].query, "query3");
        assert_eq!(history.entries()[1].query, "query4");
        assert_eq!(history.entries()[2].query, "query5");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_empty_query_not_added() {
        let mut history = History::new_empty(100);
        history.push("".to_string(), None);
        history.push("   ".to_string(), None);
        assert!(history.is_empty());
    }

    #[test]
    fn test_query_trimmed() {
        let mut history = History::new_empty(100);
        history.push("  SELECT 1  ".to_string(), None);
        assert_eq!(history.entries()[0].query, "SELECT 1");
    }

    #[test]
    fn test_search_empty_pattern_returns_all_reversed() {
        let mut history = History::new_empty(100);
        history.push("first".to_string(), None);
        history.push("second".to_string(), None);
        history.push("third".to_string(), None);

        let matches = history.search("");
        assert_eq!(matches.len(), 3);
        // Most recent first.
        assert_eq!(matches[0].entry.query, "third");
        assert_eq!(matches[1].entry.query, "second");
        assert_eq!(matches[2].entry.query, "first");
    }

    #[test]
    fn test_search_fuzzy_matching() {
        let mut history = History::new_empty(100);
        history.push("SELECT * FROM users".to_string(), None);
        history.push("SELECT * FROM orders".to_string(), None);
        history.push("INSERT INTO logs VALUES (1)".to_string(), None);
        history.push("DELETE FROM sessions".to_string(), None);

        let matches = history.search("users");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].entry.query, "SELECT * FROM users");
        assert!(!matches[0].indices.is_empty());
    }

    #[test]
    fn test_search_fuzzy_partial() {
        let mut history = History::new_empty(100);
        history.push("SELECT * FROM users".to_string(), None);
        history.push("SELECT * FROM orders".to_string(), None);

        // "sel ord" should match "SELECT * FROM orders".
        let matches = history.search("sel ord");
        assert!(!matches.is_empty());
        // The orders query should match.
        assert!(matches.iter().any(|m| m.entry.query.contains("orders")));
    }

    #[test]
    fn test_load_enforces_max_entries() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        // Create a file with more entries than max.
        let file = HistoryFile {
            version: 1,
            entries: (1..=10)
                .map(|i| HistoryEntry::new(format!("query{}", i), None))
                .collect(),
        };

        let mut f = fs::File::create(&path).unwrap();
        f.write_all(serde_json::to_string(&file).unwrap().as_bytes())
            .unwrap();

        // Load with max_entries = 3.
        let history = History::load_from_path(&path, 3).unwrap();
        assert_eq!(history.len(), 3);
        // Should keep the most recent (last) entries.
        assert_eq!(history.entries()[0].query, "query8");
        assert_eq!(history.entries()[1].query, "query9");
        assert_eq!(history.entries()[2].query, "query10");

        fs::remove_file(&path).ok();
    }
}
