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

const MAX_SAVED_SNIPPETS: usize = 512;

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
    /// Whether this entry is pinned (immune to pruning).
    #[serde(default)]
    pub pinned: bool,
}

impl HistoryEntry {
    pub fn new(query: String, connection: Option<String>) -> Self {
        Self {
            query,
            timestamp: Utc::now(),
            connection,
            pinned: false,
        }
    }
}

/// A named, reusable query saved independently of execution history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedQuerySnippet {
    /// Human-readable snippet name, unique without regard to case.
    pub name: String,
    /// The SQL or Mongo query source.
    pub query: String,
    /// Optional connection hint (sanitized - no passwords).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// When this snippet was first saved.
    pub created_at: DateTime<Utc>,
    /// When this snippet was most recently updated.
    pub updated_at: DateTime<Utc>,
}

/// The history file format.
#[derive(Debug, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    entries: Vec<HistoryEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    snippets: Vec<SavedQuerySnippet>,
}

impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
            snippets: Vec::new(),
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
    snippets: Vec<SavedQuerySnippet>,
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
        let (entries, mut snippets) = if path.exists() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read history file: {}", path.display()))?;

            let file: HistoryFile = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse history file: {}", path.display()))?;

            (file.entries, file.snippets)
        } else {
            (Vec::new(), Vec::new())
        };

        if snippets.len() > MAX_SAVED_SNIPPETS {
            snippets.drain(..snippets.len() - MAX_SAVED_SNIPPETS);
        }

        // Enforce max_entries limit on load, skipping pinned entries when pruning.
        let mut entries = entries;
        while entries.len() > max_entries {
            if let Some(oldest_unpinned) = entries.iter().position(|e| !e.pinned) {
                entries.remove(oldest_unpinned);
            } else {
                break;
            }
        }

        Ok(Self {
            entries,
            snippets,
            max_entries,
            path: path.to_path_buf(),
            dirty: false,
        })
    }

    /// Create a new empty history (for testing or when path isn't available).
    pub fn new_empty(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            snippets: Vec::new(),
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
            snippets: self.snippets.clone(),
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

        // Enforce max_entries limit, but never remove pinned entries.
        // If the only unpinned entry is the one just added (last position), allow
        // overflow rather than immediately discarding what was just pushed.
        while self.entries.len() > self.max_entries {
            match self.entries.iter().position(|e| !e.pinned) {
                Some(idx) if idx < self.entries.len() - 1 => {
                    self.entries.remove(idx);
                }
                _ => break,
            }
        }

        self.dirty = true;
    }

    /// Toggle the pinned state of the entry at the given index.
    pub fn toggle_pin(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.pinned = !entry.pinned;
            self.dirty = true;
        }
    }

    /// Remove the entry at the given index.
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
            self.dirty = true;
        }
    }

    /// Get all history entries (oldest first).
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Returns saved snippets in their stable creation order.
    pub fn snippets(&self) -> &[SavedQuerySnippet] {
        &self.snippets
    }

    /// Creates or replaces a named query snippet. Names are case-insensitive.
    pub fn save_snippet(
        &mut self,
        name: &str,
        query: String,
        connection: Option<String>,
    ) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "Snippet name cannot be empty");
        anyhow::ensure!(name.chars().count() <= 80, "Snippet name is too long");
        let query = query.trim();
        anyhow::ensure!(!query.is_empty(), "Cannot save an empty query snippet");

        let now = Utc::now();
        if let Some(snippet) = self
            .snippets
            .iter_mut()
            .find(|snippet| snippet.name.eq_ignore_ascii_case(name))
        {
            snippet.name = name.to_string();
            snippet.query = query.to_string();
            snippet.connection = connection;
            snippet.updated_at = now;
        } else {
            anyhow::ensure!(
                self.snippets.len() < MAX_SAVED_SNIPPETS,
                "Cannot save more than {MAX_SAVED_SNIPPETS} query snippets"
            );
            self.snippets.push(SavedQuerySnippet {
                name: name.to_string(),
                query: query.to_string(),
                connection,
                created_at: now,
                updated_at: now,
            });
        }
        self.dirty = true;
        Ok(())
    }

    /// Removes a named query snippet. Returns whether a snippet was removed.
    pub fn remove_snippet(&mut self, name: &str) -> bool {
        let Some(index) = self
            .snippets
            .iter()
            .position(|snippet| snippet.name.eq_ignore_ascii_case(name.trim()))
        else {
            return false;
        };
        self.snippets.remove(index);
        self.dirty = true;
        true
    }

    /// Fuzzy-searches saved snippets by both name and query source.
    pub fn search_snippets(&self, pattern: &str) -> Vec<SavedQuerySnippet> {
        if pattern.trim().is_empty() {
            return self.snippets.iter().rev().cloned().collect();
        }

        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(pattern, CaseMatching::Ignore, Normalization::Smart);
        let mut matches = self
            .snippets
            .iter()
            .filter_map(|snippet| {
                let text = format!("{} {}", snippet.name, snippet.query);
                let mut indices = Vec::new();
                let mut buffer = Vec::new();
                pattern
                    .indices(
                        Utf32Str::new(&text, &mut buffer),
                        &mut matcher,
                        &mut indices,
                    )
                    .map(|score| (score, snippet.clone()))
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        matches.into_iter().map(|(_, snippet)| snippet).collect()
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
        matches.sort_by_key(|b| std::cmp::Reverse(b.score));

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
    fn test_toggle_pin() {
        let mut history = History::new_empty(100);
        history.push("query1".to_string(), None);
        history.push("query2".to_string(), None);

        assert!(!history.entries()[0].pinned);
        history.toggle_pin(0);
        assert!(history.entries()[0].pinned);
        history.toggle_pin(0);
        assert!(!history.entries()[0].pinned);

        // Out-of-bounds index is a no-op.
        history.toggle_pin(99);
    }

    #[test]
    fn test_push_does_not_prune_pinned() {
        let mut history = History::new_empty(3);
        history.push("query1".to_string(), None);
        history.push("query2".to_string(), None);
        history.push("query3".to_string(), None);

        // Pin query1 (index 0).
        history.toggle_pin(0);

        // Adding a fourth entry should remove query2 (oldest unpinned), not query1.
        history.push("query4".to_string(), None);

        assert_eq!(history.len(), 3);
        assert!(history
            .entries()
            .iter()
            .any(|e| e.query == "query1" && e.pinned));
        assert!(!history.entries().iter().any(|e| e.query == "query2"));
        assert!(history.entries().iter().any(|e| e.query == "query3"));
        assert!(history.entries().iter().any(|e| e.query == "query4"));
    }

    #[test]
    fn test_push_allows_overflow_when_all_pinned() {
        let mut history = History::new_empty(2);
        history.push("query1".to_string(), None);
        history.push("query2".to_string(), None);
        history.toggle_pin(0);
        history.toggle_pin(1);

        // Both pinned; adding more should not remove any.
        history.push("query3".to_string(), None);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_load_preserves_pinned_on_prune() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        let mut entries: Vec<HistoryEntry> = (1..=5)
            .map(|i| HistoryEntry::new(format!("query{}", i), None))
            .collect();
        // Pin query1 (oldest).
        entries[0].pinned = true;

        let file = HistoryFile {
            version: 1,
            entries,
            snippets: Vec::new(),
        };
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(serde_json::to_string(&file).unwrap().as_bytes())
            .unwrap();

        // Load with max_entries = 3; query1 is pinned so should survive.
        let history = History::load_from_path(&path, 3).unwrap();
        assert_eq!(history.len(), 3);
        assert!(history
            .entries()
            .iter()
            .any(|e| e.query == "query1" && e.pinned));

        fs::remove_file(&path).ok();
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
            snippets: Vec::new(),
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

    #[test]
    fn snippets_round_trip_update_case_insensitively_and_search_name_or_source() {
        let path = temp_path();
        let _ = fs::remove_file(&path);
        let created_at;

        {
            let mut history = History::load_from_path(&path, 100).unwrap();
            history
                .save_snippet(
                    "Recent users",
                    " SELECT * FROM users ORDER BY created_at DESC ".to_string(),
                    Some("localhost/app".to_string()),
                )
                .unwrap();
            created_at = history.snippets()[0].created_at;
            history
                .save_snippet("recent USERS", "SELECT id FROM users".to_string(), None)
                .unwrap();
            assert_eq!(history.snippets().len(), 1);
            assert_eq!(history.snippets()[0].created_at, created_at);
            assert_eq!(history.snippets()[0].name, "recent USERS");
            assert_eq!(history.snippets()[0].query, "SELECT id FROM users");
            assert!(history.snippets()[0].connection.is_none());
            assert_eq!(history.search_snippets("recent").len(), 1);
            assert_eq!(history.search_snippets("id users").len(), 1);
            history.save().unwrap();
        }

        let history = History::load_from_path(&path, 100).unwrap();
        assert_eq!(history.snippets().len(), 1);
        assert_eq!(history.snippets()[0].created_at, created_at);
        assert_eq!(history.search_snippets("")[0].query, "SELECT id FROM users");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn snippet_validation_and_removal_are_safe() {
        let mut history = History::new_empty(100);
        assert!(history
            .save_snippet(" ", "SELECT 1".to_string(), None)
            .is_err());
        assert!(history
            .save_snippet("valid", "  ".to_string(), None)
            .is_err());
        assert!(history
            .save_snippet(&"x".repeat(81), "SELECT 1".to_string(), None)
            .is_err());
        history
            .save_snippet("Useful", "SELECT 1".to_string(), None)
            .unwrap();
        assert!(!history.remove_snippet("missing"));
        assert!(history.remove_snippet(" useful "));
        assert!(history.snippets().is_empty());
    }

    #[test]
    fn legacy_history_without_snippets_still_loads() {
        let path = temp_path();
        let _ = fs::remove_file(&path);
        fs::write(
            &path,
            r#"{"version":1,"entries":[{"query":"SELECT 1","timestamp":"2026-01-01T00:00:00Z","pinned":false}]}"#,
        )
        .unwrap();

        let history = History::load_from_path(&path, 100).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history.snippets().is_empty());
        fs::remove_file(&path).ok();
    }
}
