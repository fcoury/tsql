use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Action for column resize operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeAction {
    /// Widen the column.
    Widen,
    /// Narrow the column.
    Narrow,
    /// Auto-fit the column to its content.
    AutoFit,
}

/// Result of handling a key in the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridKeyResult {
    /// Key was handled, no special action needed.
    None,
    /// Open search prompt.
    OpenSearch,
    /// Open command prompt.
    OpenCommand,
    /// Copy text to clipboard.
    CopyToClipboard(String),
    /// Resize a column.
    ResizeColumn { col: usize, action: ResizeAction },
}

/// A match location in the grid (row, column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridMatch {
    pub row: usize,
    pub col: usize,
}

/// Search state for the grid.
#[derive(Default, Clone)]
pub struct GridSearch {
    /// The current search pattern (empty = no search).
    pub pattern: String,
    /// All matches found in the grid.
    pub matches: Vec<GridMatch>,
    /// Index of the current match in `matches` (None if no matches or search inactive).
    pub current_match: Option<usize>,
}

impl GridSearch {
    /// Clear the search state.
    pub fn clear(&mut self) {
        self.pattern.clear();
        self.matches.clear();
        self.current_match = None;
    }

    /// Set a new search pattern and find all matches in the grid.
    pub fn search(&mut self, pattern: &str, model: &GridModel) {
        self.pattern = pattern.to_lowercase();
        self.matches.clear();
        self.current_match = None;

        if self.pattern.is_empty() {
            return;
        }

        // Find all matches (case-insensitive)
        for (row_idx, row) in model.rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                if cell.to_lowercase().contains(&self.pattern) {
                    self.matches.push(GridMatch {
                        row: row_idx,
                        col: col_idx,
                    });
                }
            }
        }

        // Set current match to first one if any
        if !self.matches.is_empty() {
            self.current_match = Some(0);
        }
    }

    /// Move to the next match, wrapping around.
    pub fn next_match(&mut self) -> Option<GridMatch> {
        if self.matches.is_empty() {
            return None;
        }

        let next = match self.current_match {
            Some(idx) => (idx + 1) % self.matches.len(),
            None => 0,
        };
        self.current_match = Some(next);
        Some(self.matches[next])
    }

    /// Move to the previous match, wrapping around.
    pub fn prev_match(&mut self) -> Option<GridMatch> {
        if self.matches.is_empty() {
            return None;
        }

        let prev = match self.current_match {
            Some(idx) => {
                if idx == 0 {
                    self.matches.len() - 1
                } else {
                    idx - 1
                }
            }
            None => self.matches.len() - 1,
        };
        self.current_match = Some(prev);
        Some(self.matches[prev])
    }

    /// Get the current match.
    pub fn current(&self) -> Option<GridMatch> {
        self.current_match.map(|idx| self.matches[idx])
    }

    /// Check if a cell is a match.
    pub fn is_match(&self, row: usize, col: usize) -> bool {
        self.matches.iter().any(|m| m.row == row && m.col == col)
    }

    /// Check if a cell is the current match.
    pub fn is_current_match(&self, row: usize, col: usize) -> bool {
        self.current().map_or(false, |m| m.row == row && m.col == col)
    }

    /// Get match count info string.
    pub fn match_info(&self) -> Option<String> {
        if self.pattern.is_empty() {
            return None;
        }

        let total = self.matches.len();
        if total == 0 {
            return Some(format!("/{} (no matches)", self.pattern));
        }

        let current = self.current_match.map_or(0, |i| i + 1);
        Some(format!("/{} ({}/{})", self.pattern, current, total))
    }
}

#[derive(Default, Clone)]
pub struct GridState {
    pub row_offset: usize,
    pub col_offset: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub selected_rows: BTreeSet<usize>,
    pub search: GridSearch,
}

impl GridState {
    /// Returns true if this key should trigger a search prompt (handled by App).
    pub fn handle_key(&mut self, key: KeyEvent, model: &GridModel) -> GridKeyResult {
        let row_count = model.rows.len();
        let col_count = model.headers.len();

        match (key.code, key.modifiers) {
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                if row_count > 0 {
                    self.cursor_row = (self.cursor_row + 1).min(row_count - 1);
                }
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.cursor_row = self.cursor_row.saturating_sub(10);
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                if row_count > 0 {
                    self.cursor_row = (self.cursor_row + 10).min(row_count - 1);
                }
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
                self.cursor_row = 0;
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                if row_count > 0 {
                    self.cursor_row = row_count - 1;
                }
            }

            // Column cursor movement (h/l move cursor, H/L scroll viewport)
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                if col_count > 0 {
                    self.cursor_col = (self.cursor_col + 1).min(col_count - 1);
                }
            }
            // Viewport scrolling (Shift+H/L)
            (KeyCode::Char('H'), KeyModifiers::SHIFT) | (KeyCode::Char('H'), KeyModifiers::NONE) => {
                self.col_offset = self.col_offset.saturating_sub(1);
            }
            (KeyCode::Char('L'), KeyModifiers::SHIFT) | (KeyCode::Char('L'), KeyModifiers::NONE) => {
                if col_count > 0 {
                    self.col_offset = (self.col_offset + 1).min(col_count - 1);
                }
            }

            // Multi-select controls.
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                if row_count == 0 {
                    return GridKeyResult::None;
                }
                if self.selected_rows.contains(&self.cursor_row) {
                    self.selected_rows.remove(&self.cursor_row);
                } else {
                    self.selected_rows.insert(self.cursor_row);
                }
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.selected_rows.clear();
                for i in 0..row_count {
                    self.selected_rows.insert(i);
                }
            }

            // Search controls.
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                return GridKeyResult::OpenSearch;
            }
            // Command mode.
            (KeyCode::Char(':'), KeyModifiers::NONE) => {
                return GridKeyResult::OpenCommand;
            }
            (KeyCode::Char('n'), KeyModifiers::NONE) => {
                if let Some(m) = self.search.next_match() {
                    self.cursor_row = m.row;
                    self.cursor_col = m.col;
                    // Ensure the column is visible
                    self.col_offset = m.col;
                }
            }
            (KeyCode::Char('N'), KeyModifiers::SHIFT) | (KeyCode::Char('N'), KeyModifiers::NONE) => {
                if let Some(m) = self.search.prev_match() {
                    self.cursor_row = m.row;
                    self.cursor_col = m.col;
                    // Ensure the column is visible
                    self.col_offset = m.col;
                }
            }

            // Copy controls.
            // y - yank current row (or selected rows if any)
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                if row_count == 0 {
                    return GridKeyResult::None;
                }

                let text = if self.selected_rows.is_empty() {
                    // Copy current row
                    model.row_as_tsv(self.cursor_row).unwrap_or_default()
                } else {
                    // Copy selected rows
                    let indices: Vec<usize> = self.selected_rows.iter().copied().collect();
                    model.rows_as_tsv(&indices, false)
                };

                return GridKeyResult::CopyToClipboard(text);
            }
            // Y - yank with headers (Shift+Y sends SHIFT modifier)
            (KeyCode::Char('Y'), KeyModifiers::SHIFT) | (KeyCode::Char('Y'), KeyModifiers::NONE) => {
                if row_count == 0 {
                    return GridKeyResult::None;
                }

                let indices: Vec<usize> = if self.selected_rows.is_empty() {
                    vec![self.cursor_row]
                } else {
                    self.selected_rows.iter().copied().collect()
                };

                let text = model.rows_as_tsv(&indices, true);
                return GridKeyResult::CopyToClipboard(text);
            }
            // c - copy current cell
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                if row_count == 0 || col_count == 0 {
                    return GridKeyResult::None;
                }

                if let Some(cell) = model.cell(self.cursor_row, self.cursor_col) {
                    return GridKeyResult::CopyToClipboard(cell.to_string());
                }
            }

            // Clear selection (changed to Shift+C)
            (KeyCode::Char('C'), KeyModifiers::SHIFT) => {
                self.selected_rows.clear();
            }
            // Escape clears search or selection
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if !self.search.pattern.is_empty() {
                    self.search.clear();
                } else {
                    self.selected_rows.clear();
                }
            }

            // Column resize controls
            // + or > to widen column
            (KeyCode::Char('+'), _) | (KeyCode::Char('>'), _) => {
                if col_count > 0 {
                    return GridKeyResult::ResizeColumn {
                        col: self.cursor_col,
                        action: ResizeAction::Widen,
                    };
                }
            }
            // - or < to narrow column
            (KeyCode::Char('-'), _) | (KeyCode::Char('<'), _) => {
                if col_count > 0 {
                    return GridKeyResult::ResizeColumn {
                        col: self.cursor_col,
                        action: ResizeAction::Narrow,
                    };
                }
            }
            // = to auto-fit column
            (KeyCode::Char('='), _) => {
                if col_count > 0 {
                    return GridKeyResult::ResizeColumn {
                        col: self.cursor_col,
                        action: ResizeAction::AutoFit,
                    };
                }
            }

            _ => {}
        }

        GridKeyResult::None
    }

    /// Apply a search pattern to the grid.
    pub fn apply_search(&mut self, pattern: &str, model: &GridModel) {
        self.search.search(pattern, model);
        // Jump to first match if any
        if let Some(m) = self.search.current() {
            self.cursor_row = m.row;
            self.cursor_col = m.col;
            self.col_offset = m.col;
        }
    }

    /// Clear the current search.
    pub fn clear_search(&mut self) {
        self.search.clear();
    }

    pub fn ensure_cursor_visible(
        &mut self,
        viewport_rows: usize,
        row_count: usize,
        col_count: usize,
        col_widths: &[u16],
        viewport_width: u16,
    ) {
        // Handle rows
        if viewport_rows == 0 || row_count == 0 {
            self.row_offset = 0;
            self.cursor_row = 0;
        } else {
            self.cursor_row = self.cursor_row.min(row_count - 1);

            if self.cursor_row < self.row_offset {
                self.row_offset = self.cursor_row;
            }

            let last_visible = self.row_offset + viewport_rows - 1;
            if self.cursor_row > last_visible {
                self.row_offset = self.cursor_row.saturating_sub(viewport_rows - 1);
            }

            self.row_offset = self.row_offset.min(row_count.saturating_sub(1));
        }

        // Handle columns - ensure cursor_col is visible
        if col_count == 0 {
            self.col_offset = 0;
            self.cursor_col = 0;
        } else {
            self.cursor_col = self.cursor_col.min(col_count - 1);

            // If cursor is before visible area, scroll left
            if self.cursor_col < self.col_offset {
                self.col_offset = self.cursor_col;
            }

            // If cursor is after visible area, scroll right
            // Calculate the rightmost visible column from current col_offset
            if !col_widths.is_empty() && viewport_width > 0 {
                let mut width_used: u16 = 0;
                let mut last_fully_visible_col = self.col_offset;

                for col in self.col_offset..col_count {
                    let col_w = col_widths.get(col).copied().unwrap_or(0);
                    let col_total = col_w + 1; // +1 for padding

                    if width_used + col_w <= viewport_width {
                        last_fully_visible_col = col;
                        width_used += col_total;
                    } else {
                        break;
                    }
                }

                // If cursor is beyond the last fully visible column, scroll right
                if self.cursor_col > last_fully_visible_col {
                    // Scroll so cursor_col is visible
                    // We want cursor_col to be the rightmost visible column
                    let mut new_offset = self.cursor_col;
                    let mut width_needed: u16 = 0;

                    // Work backwards from cursor_col to find how many columns fit
                    while new_offset > 0 {
                        let col_w = col_widths.get(new_offset).copied().unwrap_or(0);
                        let col_total = col_w + 1;

                        if width_needed + col_total <= viewport_width {
                            width_needed += col_total;
                            new_offset -= 1;
                        } else {
                            break;
                        }
                    }

                    // Adjust: new_offset should be the first column to show
                    if new_offset < self.cursor_col {
                        new_offset += 1;
                    }

                    // Make sure cursor column itself fits
                    let cursor_width = col_widths.get(self.cursor_col).copied().unwrap_or(0);
                    if cursor_width > viewport_width {
                        // Column is wider than viewport, just show it from the start
                        new_offset = self.cursor_col;
                    }

                    self.col_offset = new_offset.max(self.col_offset);
                }
            }

            self.col_offset = self.col_offset.min(col_count.saturating_sub(1));
        }
    }
}

pub struct GridModel {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub col_widths: Vec<u16>,
    /// The source table name, if known (extracted from simple SELECT queries).
    pub source_table: Option<String>,
    /// Primary key column names for the source table, if known.
    pub primary_keys: Vec<String>,
}

impl GridModel {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let col_widths = compute_column_widths(&headers, &rows);
        Self {
            headers,
            rows,
            col_widths,
            source_table: None,
            primary_keys: Vec::new(),
        }
    }

    pub fn with_source_table(mut self, table: Option<String>) -> Self {
        self.source_table = table;
        self
    }

    pub fn with_primary_keys(mut self, keys: Vec<String>) -> Self {
        self.primary_keys = keys;
        self
    }

    pub fn empty() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            col_widths: Vec::new(),
            source_table: None,
            primary_keys: Vec::new(),
        }
    }

    /// Get the primary key column indices that are present in the current headers.
    pub fn pk_column_indices(&self) -> Vec<usize> {
        self.primary_keys
            .iter()
            .filter_map(|pk| self.headers.iter().position(|h| h == pk))
            .collect()
    }

    /// Check if we have valid primary key information for UPDATE/DELETE operations.
    pub fn has_valid_pk(&self) -> bool {
        if self.primary_keys.is_empty() {
            return false;
        }
        // All PK columns must be present in the headers
        self.primary_keys.iter().all(|pk| self.headers.contains(pk))
    }

    /// Get a specific cell value.
    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row).and_then(|r| r.get(col)).map(|s| s.as_str())
    }

    /// Format a single row as tab-separated values.
    pub fn row_as_tsv(&self, row_idx: usize) -> Option<String> {
        self.rows.get(row_idx).map(|row| row.join("\t"))
    }

    /// Format multiple rows as tab-separated values (with headers).
    pub fn rows_as_tsv(&self, row_indices: &[usize], include_headers: bool) -> String {
        let mut lines = Vec::new();

        if include_headers && !self.headers.is_empty() {
            lines.push(self.headers.join("\t"));
        }

        for &idx in row_indices {
            if let Some(row) = self.rows.get(idx) {
                lines.push(row.join("\t"));
            }
        }

        lines.join("\n")
    }

    /// Format a single row as CSV.
    pub fn row_as_csv(&self, row_idx: usize) -> Option<String> {
        self.rows.get(row_idx).map(|row| {
            row.iter()
                .map(|cell| escape_csv(cell))
                .collect::<Vec<_>>()
                .join(",")
        })
    }

    /// Format multiple rows as CSV (with headers).
    pub fn rows_as_csv(&self, row_indices: &[usize], include_headers: bool) -> String {
        let mut lines = Vec::new();

        if include_headers && !self.headers.is_empty() {
            lines.push(
                self.headers
                    .iter()
                    .map(|h| escape_csv(h))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }

        for &idx in row_indices {
            if let Some(row) = self.rows.get(idx) {
                lines.push(
                    row.iter()
                        .map(|cell| escape_csv(cell))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
        }

        lines.join("\n")
    }

    /// Format a single row as JSON object.
    pub fn row_as_json(&self, row_idx: usize) -> Option<String> {
        self.rows.get(row_idx).map(|row| {
            let pairs: Vec<String> = self
                .headers
                .iter()
                .zip(row.iter())
                .map(|(h, v)| format!("  \"{}\": \"{}\"", escape_json(h), escape_json(v)))
                .collect();
            format!("{{\n{}\n}}", pairs.join(",\n"))
        })
    }

    /// Format multiple rows as JSON array.
    pub fn rows_as_json(&self, row_indices: &[usize]) -> String {
        let objects: Vec<String> = row_indices
            .iter()
            .filter_map(|&idx| {
                self.rows.get(idx).map(|row| {
                    let pairs: Vec<String> = self
                        .headers
                        .iter()
                        .zip(row.iter())
                        .map(|(h, v)| format!("    \"{}\": \"{}\"", escape_json(h), escape_json(v)))
                        .collect();
                    format!("  {{\n{}\n  }}", pairs.join(",\n"))
                })
            })
            .collect();

        format!("[\n{}\n]", objects.join(",\n"))
    }

    /// Widen a column by a given amount.
    pub fn widen_column(&mut self, col: usize, amount: u16) {
        if let Some(width) = self.col_widths.get_mut(col) {
            *width = width.saturating_add(amount).min(200); // Max width of 200
        }
    }

    /// Narrow a column by a given amount.
    pub fn narrow_column(&mut self, col: usize, amount: u16) {
        if let Some(width) = self.col_widths.get_mut(col) {
            *width = width.saturating_sub(amount).max(3); // Min width of 3
        }
    }

    /// Auto-fit a column to its content.
    pub fn autofit_column(&mut self, col: usize) {
        if col >= self.headers.len() {
            return;
        }

        // Calculate optimal width based on header and all row values
        let header_width = display_width(&self.headers[col]) as u16;
        let max_data_width = self
            .rows
            .iter()
            .filter_map(|row| row.get(col))
            .map(|cell| display_width(cell) as u16)
            .max()
            .unwrap_or(0);

        let optimal_width = header_width.max(max_data_width).max(3).min(100); // Between 3 and 100

        if let Some(width) = self.col_widths.get_mut(col) {
            *width = optimal_width;
        }
    }

    /// Generate UPDATE SQL statements for specified rows.
    ///
    /// # Arguments
    /// * `table` - The table name to use in the UPDATE statement
    /// * `row_indices` - The row indices to generate UPDATE statements for
    /// * `key_columns` - Optional list of column names to use in WHERE clause.
    ///                   If None, all columns are used.
    ///
    /// # Returns
    /// A string containing one UPDATE statement per row, separated by newlines.
    pub fn generate_update_sql(
        &self,
        table: &str,
        row_indices: &[usize],
        key_columns: Option<&[&str]>,
    ) -> String {
        let mut statements = Vec::new();

        for &row_idx in row_indices {
            if let Some(row) = self.rows.get(row_idx) {
                let stmt = self.generate_single_update(table, row, key_columns);
                statements.push(stmt);
            }
        }

        statements.join("\n")
    }

    fn generate_single_update(
        &self,
        table: &str,
        row: &[String],
        key_columns: Option<&[&str]>,
    ) -> String {
        // Determine which columns are keys and which are values to set
        let key_indices: Vec<usize> = match key_columns {
            Some(keys) => self
                .headers
                .iter()
                .enumerate()
                .filter(|(_, h)| keys.contains(&h.as_str()))
                .map(|(i, _)| i)
                .collect(),
            None => {
                // Use first column as key by default
                if self.headers.is_empty() {
                    vec![]
                } else {
                    vec![0]
                }
            }
        };

        // SET clause: all non-key columns
        let set_parts: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .filter(|(i, _)| !key_indices.contains(i))
            .filter_map(|(i, header)| {
                row.get(i).map(|value| {
                    format!("{} = {}", quote_identifier(header), escape_sql_value(value))
                })
            })
            .collect();

        // WHERE clause: key columns
        let where_parts: Vec<String> = key_indices
            .iter()
            .filter_map(|&i| {
                let header = self.headers.get(i)?;
                let value = row.get(i)?;
                Some(format!(
                    "{} = {}",
                    quote_identifier(header),
                    escape_sql_value(value)
                ))
            })
            .collect();

        if set_parts.is_empty() {
            format!(
                "-- UPDATE {}: no columns to update (all columns are keys)",
                table
            )
        } else if where_parts.is_empty() {
            format!(
                "UPDATE {} SET {};  -- WARNING: no WHERE clause",
                table,
                set_parts.join(", ")
            )
        } else {
            format!(
                "UPDATE {} SET {} WHERE {};",
                table,
                set_parts.join(", "),
                where_parts.join(" AND ")
            )
        }
    }

    /// Generate DELETE SQL statements for specified rows.
    ///
    /// # Arguments
    /// * `table` - The table name to use in the DELETE statement
    /// * `row_indices` - The row indices to generate DELETE statements for
    /// * `key_columns` - Optional list of column names to use in WHERE clause.
    ///                   If None, all columns are used.
    ///
    /// # Returns
    /// A string containing one DELETE statement per row, separated by newlines.
    pub fn generate_delete_sql(
        &self,
        table: &str,
        row_indices: &[usize],
        key_columns: Option<&[&str]>,
    ) -> String {
        let mut statements = Vec::new();

        for &row_idx in row_indices {
            if let Some(row) = self.rows.get(row_idx) {
                let stmt = self.generate_single_delete(table, row, key_columns);
                statements.push(stmt);
            }
        }

        statements.join("\n")
    }

    fn generate_single_delete(
        &self,
        table: &str,
        row: &[String],
        key_columns: Option<&[&str]>,
    ) -> String {
        // Determine which columns to use in WHERE clause
        let key_indices: Vec<usize> = match key_columns {
            Some(keys) => self
                .headers
                .iter()
                .enumerate()
                .filter(|(_, h)| keys.contains(&h.as_str()))
                .map(|(i, _)| i)
                .collect(),
            None => {
                // Use all columns by default for safety
                (0..self.headers.len()).collect()
            }
        };

        // WHERE clause
        let where_parts: Vec<String> = key_indices
            .iter()
            .filter_map(|&i| {
                let header = self.headers.get(i)?;
                let value = row.get(i)?;
                Some(format!(
                    "{} = {}",
                    quote_identifier(header),
                    escape_sql_value(value)
                ))
            })
            .collect();

        if where_parts.is_empty() {
            format!("-- DELETE FROM {}: no columns for WHERE clause", table)
        } else {
            format!("DELETE FROM {} WHERE {};", table, where_parts.join(" AND "))
        }
    }

    /// Generate INSERT SQL statement for specified rows.
    ///
    /// # Arguments
    /// * `table` - The table name to use in the INSERT statement
    /// * `row_indices` - The row indices to generate INSERT statement for
    ///
    /// # Returns
    /// A string containing an INSERT statement with all rows as VALUES.
    pub fn generate_insert_sql(&self, table: &str, row_indices: &[usize]) -> String {
        if row_indices.is_empty() || self.headers.is_empty() {
            return format!("-- INSERT INTO {}: no data", table);
        }

        let columns: Vec<String> = self.headers.iter().map(|h| quote_identifier(h)).collect();

        let values: Vec<String> = row_indices
            .iter()
            .filter_map(|&idx| self.rows.get(idx))
            .map(|row| {
                let vals: Vec<String> = row.iter().map(|v| escape_sql_value(v)).collect();
                format!("({})", vals.join(", "))
            })
            .collect();

        if values.is_empty() {
            return format!("-- INSERT INTO {}: no valid rows", table);
        }

        format!(
            "INSERT INTO {} ({}) VALUES\n{};",
            table,
            columns.join(", "),
            values.join(",\n")
        )
    }
}

/// Quote a SQL identifier (column/table name).
fn quote_identifier(s: &str) -> String {
    // If it contains special chars or is a reserved word, quote it
    if s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.chars().next().map_or(true, |c| c.is_ascii_digit())
    {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

/// Escape a SQL value for use in a statement.
fn escape_sql_value(s: &str) -> String {
    // Handle NULL
    if s.is_empty() || s.eq_ignore_ascii_case("null") {
        return "NULL".to_string();
    }

    // Check if it looks like a number
    if s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok() {
        return s.to_string();
    }

    // Check for boolean
    if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false") {
        return s.to_uppercase();
    }

    // Otherwise, quote as string
    format!("'{}'", s.replace('\'', "''"))
}

/// Escape a string for CSV output.
fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Escape a string for JSON output.
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub struct DataGrid<'a> {
    pub model: &'a GridModel,
    pub state: &'a GridState,
    pub focused: bool,
}

impl<'a> Widget for DataGrid<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Build title with search info if active
        let base_title = "Results (j/k rows, h/l cols, +/- resize, = autofit, / search)";
        let title = if let Some(search_info) = self.state.search.match_info() {
            format!("{} {}", base_title, search_info)
        } else {
            base_title.to_string()
        };

        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.model.headers.is_empty() {
            Paragraph::new("No columns")
                .style(Style::default().fg(Color::Gray))
                .render(inner, buf);
            return;
        }

        // Reserve one line for header.
        if inner.height < 2 {
            Paragraph::new("Window too small")
                .style(Style::default().fg(Color::Gray))
                .render(inner, buf);
            return;
        }

        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };

        let body_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height - 1,
        };

        // Keep marker column fixed; horizontal scroll applies to data columns.
        let marker_w: u16 = 3; // cursor + selected + space
        let data_x = header_area.x.saturating_add(marker_w);
        let data_w = header_area.width.saturating_sub(marker_w);

        // Calculate the actual scroll position first (before rendering header)
        let mut state = self.state.clone();
        state.ensure_cursor_visible(
            body_area.height as usize,
            self.model.rows.len(),
            self.model.headers.len(),
            &self.model.col_widths,
            data_w,
        );

        // Header row (frozen vertically, but scrolls horizontally with body).
        render_marker_header(header_area, buf, marker_w);
        render_row_cells(
            data_x,
            header_area.y,
            data_w,
            &self.model.headers,
            &self.model.col_widths,
            state.col_offset, // Use the adjusted col_offset
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            None, // No search highlighting for headers
            buf,
        );

        // Body rows.
        if self.model.rows.is_empty() {
            Paragraph::new("(no rows)")
                .style(Style::default().fg(Color::Gray))
                .render(body_area, buf);
            return;
        }

        for i in 0..(body_area.height as usize) {
            let row_idx = state.row_offset + i;
            if row_idx >= self.model.rows.len() {
                break;
            }
            let y = body_area.y + i as u16;

            let is_cursor = row_idx == state.cursor_row;
            let is_selected = state.selected_rows.contains(&row_idx);

            let row_style = if is_cursor {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            render_marker_cell(
                body_area.x,
                y,
                marker_w,
                is_cursor,
                is_selected,
                row_style,
                buf,
            );

            // Determine cursor column for this row (only if this is the cursor row)
            let cursor_col = if is_cursor {
                Some(state.cursor_col)
            } else {
                None
            };

            render_row_cells_with_search(
                data_x,
                y,
                data_w,
                &self.model.rows[row_idx],
                &self.model.col_widths,
                state.col_offset,
                row_style,
                row_idx,
                cursor_col,
                &self.state.search,
                buf,
            );
        }
    }
}

fn render_marker_header(area: Rect, buf: &mut Buffer, marker_w: u16) {
    let mut x = area.x;
    for _ in 0..marker_w {
        buf.set_string(x, area.y, " ", Style::default());
        x += 1;
    }
}

fn render_marker_cell(
    x: u16,
    y: u16,
    marker_w: u16,
    is_cursor: bool,
    is_selected: bool,
    style: Style,
    buf: &mut Buffer,
) {
    let cursor_ch = if is_cursor { '>' } else { ' ' };
    let sel_ch = if is_selected { '*' } else { ' ' };

    let s = format!("{}{} ", cursor_ch, sel_ch);
    let s = fit_to_width(&s, marker_w);
    buf.set_string(x, y, s, style);
}

fn render_row_cells(
    mut x: u16,
    y: u16,
    available_w: u16,
    cells: &[String],
    col_widths: &[u16],
    col_offset: usize,
    style: Style,
    _search: Option<&GridSearch>, // Optional search state for highlighting
    buf: &mut Buffer,
) {
    if available_w == 0 {
        return;
    }

    let padding: u16 = 1;
    let max_x = x.saturating_add(available_w);

    let mut col = col_offset;
    while col < cells.len() && col < col_widths.len() && x < max_x {
        let w = col_widths[col];
        if w == 0 {
            col += 1;
            continue;
        }

        let remaining = max_x - x;
        if remaining == 0 {
            break;
        }

        // Allow a partially visible last column.
        let draw_w = w.min(remaining);
        let content = fit_to_width(&cells[col], draw_w);
        buf.set_string(x, y, content, style);
        x += draw_w;

        if x < max_x {
            buf.set_string(x, y, " ", style);
            x = x.saturating_add(padding).min(max_x);
        }

        col += 1;
    }

    while x < max_x {
        buf.set_string(x, y, " ", style);
        x += 1;
    }
}

/// Render row cells with search highlighting and cursor column.
fn render_row_cells_with_search(
    mut x: u16,
    y: u16,
    available_w: u16,
    cells: &[String],
    col_widths: &[u16],
    col_offset: usize,
    base_style: Style,
    row_idx: usize,
    cursor_col: Option<usize>,
    search: &GridSearch,
    buf: &mut Buffer,
) {
    if available_w == 0 {
        return;
    }

    let padding: u16 = 1;
    let max_x = x.saturating_add(available_w);

    // Styles for search matches and cursor
    let match_style = Style::default().bg(Color::Yellow).fg(Color::Black);
    let current_match_style = Style::default().bg(Color::Rgb(255, 165, 0)).fg(Color::Black); // Orange
    let cursor_cell_style = Style::default().bg(Color::Cyan).fg(Color::Black);

    let mut col = col_offset;
    while col < cells.len() && col < col_widths.len() && x < max_x {
        let w = col_widths[col];
        if w == 0 {
            col += 1;
            continue;
        }

        let remaining = max_x - x;
        if remaining == 0 {
            break;
        }

        // Determine cell style based on cursor position and search state
        let is_cursor_cell = cursor_col == Some(col);
        let cell_style = if is_cursor_cell {
            cursor_cell_style
        } else if search.is_current_match(row_idx, col) {
            current_match_style
        } else if search.is_match(row_idx, col) {
            match_style
        } else {
            base_style
        };

        // Allow a partially visible last column.
        let draw_w = w.min(remaining);
        let content = fit_to_width(&cells[col], draw_w);
        buf.set_string(x, y, content, cell_style);
        x += draw_w;

        if x < max_x {
            buf.set_string(x, y, " ", base_style);
            x = x.saturating_add(padding).min(max_x);
        }

        col += 1;
    }

    while x < max_x {
        buf.set_string(x, y, " ", base_style);
        x += 1;
    }
}

fn compute_column_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<u16> {
    // Keep columns readable but rely on horizontal scroll for the rest.
    let min_w: u16 = 3;
    let max_w: u16 = 40;

    let mut widths: Vec<u16> = headers
        .iter()
        .map(|h| clamp_u16(display_width(h) as u16, min_w, max_w))
        .collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i >= widths.len() {
                break;
            }
            let w = clamp_u16(display_width(cell) as u16, min_w, max_w);
            widths[i] = widths[i].max(w);
        }
    }

    widths
}

fn clamp_u16(v: u16, min_v: u16, max_v: u16) -> u16 {
    v.max(min_v).min(max_v)
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn fit_to_width(s: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }

    let current = display_width(s);
    if current == width {
        return s.to_string();
    }

    if current < width {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - current));
        return out;
    }

    // Truncate, keeping ASCII-only ellipsis.
    if width <= 3 {
        return truncate_by_display_width(s, width);
    }

    let prefix_w = width.saturating_sub(3);
    let mut out = truncate_by_display_width(s, prefix_w);
    out.push_str("...");

    truncate_by_display_width(&out, width)
}

fn truncate_by_display_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;

    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
        if used == width {
            break;
        }
    }

    let out_w = display_width(&out);
    if out_w < width {
        out.push_str(&" ".repeat(width - out_w));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_model() -> GridModel {
        GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![
                vec!["1".to_string(), "Alice".to_string()],
                vec!["2".to_string(), "Bob".to_string()],
            ],
        )
    }

    #[test]
    fn test_colon_key_opens_command_in_grid() {
        // Bug: pressing ':' in grid mode should open command prompt
        // but GridKeyResult doesn't have an OpenCommand variant
        let mut state = GridState::default();
        let model = create_test_model();

        let key = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
        let result = state.handle_key(key, &model);

        // This test documents the bug: ':' should return OpenCommand
        // Currently it returns None because ':' is not handled
        assert_eq!(
            result,
            GridKeyResult::OpenCommand,
            "Pressing ':' in grid should open command prompt"
        );
    }

    #[test]
    fn test_slash_key_opens_search_in_grid() {
        let mut state = GridState::default();
        let model = create_test_model();

        let key = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        let result = state.handle_key(key, &model);

        assert_eq!(result, GridKeyResult::OpenSearch);
    }

    #[test]
    fn test_rows_as_tsv_with_headers() {
        let model = create_test_model();

        // Test with headers
        let result = model.rows_as_tsv(&[0], true);
        assert_eq!(result, "id\tname\n1\tAlice", "Should include header row");

        // Test without headers
        let result = model.rows_as_tsv(&[0], false);
        assert_eq!(result, "1\tAlice", "Should not include header row");
    }

    #[test]
    fn test_yank_with_headers_returns_headers() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Press 'Y' (shift+y) to yank with headers
        // Note: In real terminal, Shift+Y sends KeyModifiers::SHIFT
        let key = KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT);
        let result = state.handle_key(key, &model);

        match result {
            GridKeyResult::CopyToClipboard(text) => {
                assert!(
                    text.starts_with("id\tname\n"),
                    "Yank with headers should start with header row, got: {}",
                    text
                );
                assert!(
                    text.contains("1\tAlice"),
                    "Should contain the row data"
                );
            }
            _ => panic!("Expected CopyToClipboard result, got {:?}", result),
        }
    }

    #[test]
    fn test_h_l_move_column_cursor() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Initial state: cursor_col should be 0
        assert_eq!(state.cursor_col, 0);

        // Press 'l' to move column cursor right
        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        state.handle_key(key, &model);
        assert_eq!(state.cursor_col, 1, "l should move cursor_col right");

        // Press 'l' again - should stay at max (1 for 2-column model)
        state.handle_key(key, &model);
        assert_eq!(state.cursor_col, 1, "cursor_col should not exceed column count");

        // Press 'h' to move column cursor left
        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        state.handle_key(key, &model);
        assert_eq!(state.cursor_col, 0, "h should move cursor_col left");

        // Press 'h' again - should stay at 0
        state.handle_key(key, &model);
        assert_eq!(state.cursor_col, 0, "cursor_col should not go below 0");
    }

    #[test]
    fn test_shift_h_l_scroll_viewport() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Initial state: col_offset should be 0
        assert_eq!(state.col_offset, 0);

        // Press 'L' (Shift+l) to scroll viewport right
        let key = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        state.handle_key(key, &model);
        assert_eq!(state.col_offset, 1, "L should scroll col_offset right");

        // Press 'H' (Shift+h) to scroll viewport left
        let key = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        state.handle_key(key, &model);
        assert_eq!(state.col_offset, 0, "H should scroll col_offset left");
    }

    fn create_wide_test_model() -> GridModel {
        // Create a model with many columns to test scrolling
        GridModel::new(
            vec![
                "col1".to_string(),
                "col2".to_string(),
                "col3".to_string(),
                "col4".to_string(),
                "col5".to_string(),
            ],
            vec![vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
            ]],
        )
    }

    #[test]
    fn test_cursor_col_scrolls_viewport_right() {
        let mut state = GridState::default();
        let model = create_wide_test_model();

        // Initial state
        assert_eq!(state.cursor_col, 0);
        assert_eq!(state.col_offset, 0);

        // Move cursor to the right multiple times
        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        for _ in 0..4 {
            state.handle_key(key, &model);
        }

        // Cursor should be at column 4
        assert_eq!(state.cursor_col, 4, "cursor_col should be at 4");

        // Now call ensure_cursor_visible with a narrow viewport
        // col_widths are 4 each (from "col1", "col2", etc.), + 1 padding = 5 per col
        // viewport of 12 would show ~2 columns (5 + 5 = 10, leaving room for 2 cols)
        let viewport_width = 12;
        state.ensure_cursor_visible(10, 1, 5, &model.col_widths, viewport_width);

        // col_offset should have scrolled right to make cursor visible
        // If cursor is at col 4 and we can see ~2 cols, offset should be >= 3
        assert!(
            state.col_offset > 0,
            "col_offset should scroll right to keep cursor visible, but col_offset={}",
            state.col_offset
        );
    }

    #[test]
    fn test_header_scrolls_with_body() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::Widget;

        // Create a model with several columns
        let model = create_wide_test_model();

        // Create state with cursor at rightmost column but col_offset at 0
        // This simulates the bug: cursor moved right but header hasn't scrolled
        let mut state = GridState::default();
        state.cursor_col = 4; // Last column
        state.col_offset = 0; // Header would use this if not updated

        let grid = DataGrid {
            model: &model,
            state: &state,
            focused: true,
        };

        // Render to a small buffer (narrow viewport)
        // Width of 20 should only fit ~2-3 columns with border + marker
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        grid.render(area, &mut buf);

        // The header row is at y=1 (after border)
        // After marker column (3 chars), data starts at x=4
        // Check that the header shows same columns as body
        let header_row: String = (4..area.width - 1)
            .map(|x| buf.cell((x, 1)).map(|c| c.symbol().chars().next().unwrap_or(' ')).unwrap_or(' '))
            .collect();

        // Body row is at y=2
        let body_row: String = (4..area.width - 1)
            .map(|x| buf.cell((x, 2)).map(|c| c.symbol().chars().next().unwrap_or(' ')).unwrap_or(' '))
            .collect();

        // The first column shown in header should match the first column shown in body
        // Extract first word from each
        let header_first_col: String = header_row.trim().split_whitespace().next().unwrap_or("").to_string();
        let body_first_col: String = body_row.trim().split_whitespace().next().unwrap_or("").to_string();

        // Get the column index from header (col1 -> 1, col2 -> 2, etc)
        let header_col_num: Option<u32> = header_first_col.strip_prefix("col").and_then(|n| n.parse().ok());
        let _body_col_num: Option<u32> = body_first_col.strip_prefix("col").and_then(|n| n.parse().ok());

        // For body, it shows data "a", "b", "c", etc which correspond to col1, col2, col3...
        // So body shows starting from col_offset that ensure_cursor_visible calculated
        // Header should show the same starting column

        // With cursor at col 4 in a narrow viewport, the viewport should scroll
        // Both header and body should start from a column > 1
        assert!(
            header_col_num.unwrap_or(1) > 1 || body_first_col == "a",
            "Header first col '{}' should scroll to match body which starts with '{}'. Full header: '{}', body: '{}'",
            header_first_col,
            body_first_col,
            header_row.trim(),
            body_row.trim()
        );
    }

    #[test]
    fn test_plus_key_widens_column() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Press '+' to widen the current column
        let key = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE);
        let result = state.handle_key(key, &model);

        assert_eq!(
            result,
            GridKeyResult::ResizeColumn {
                col: 0,
                action: ResizeAction::Widen
            },
            "'+' should return ResizeColumn with Widen action for current column"
        );
    }

    #[test]
    fn test_greater_than_key_widens_column() {
        let mut state = GridState::default();
        state.cursor_col = 1; // Move to second column
        let model = create_test_model();

        // Press '>' to widen the current column
        let key = KeyEvent::new(KeyCode::Char('>'), KeyModifiers::SHIFT);
        let result = state.handle_key(key, &model);

        assert_eq!(
            result,
            GridKeyResult::ResizeColumn {
                col: 1,
                action: ResizeAction::Widen
            },
            "'>' should return ResizeColumn with Widen action for current column"
        );
    }

    #[test]
    fn test_minus_key_narrows_column() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Press '-' to narrow the current column
        let key = KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE);
        let result = state.handle_key(key, &model);

        assert_eq!(
            result,
            GridKeyResult::ResizeColumn {
                col: 0,
                action: ResizeAction::Narrow
            },
            "'-' should return ResizeColumn with Narrow action for current column"
        );
    }

    #[test]
    fn test_less_than_key_narrows_column() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Press '<' to narrow the current column
        let key = KeyEvent::new(KeyCode::Char('<'), KeyModifiers::SHIFT);
        let result = state.handle_key(key, &model);

        assert_eq!(
            result,
            GridKeyResult::ResizeColumn {
                col: 0,
                action: ResizeAction::Narrow
            },
            "'<' should return ResizeColumn with Narrow action for current column"
        );
    }

    #[test]
    fn test_equals_key_autofits_column() {
        let mut state = GridState::default();
        let model = create_test_model();

        // Press '=' to auto-fit the current column
        let key = KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE);
        let result = state.handle_key(key, &model);

        assert_eq!(
            result,
            GridKeyResult::ResizeColumn {
                col: 0,
                action: ResizeAction::AutoFit
            },
            "'=' should return ResizeColumn with AutoFit action for current column"
        );
    }

    #[test]
    fn test_widen_column_increases_width() {
        let mut model = create_test_model();
        let original_width = model.col_widths[0];

        model.widen_column(0, 2);

        assert_eq!(
            model.col_widths[0],
            original_width + 2,
            "widen_column should increase width by the given amount"
        );
    }

    #[test]
    fn test_narrow_column_decreases_width() {
        let mut model = create_test_model();
        // Set a known width first
        model.col_widths[0] = 10;

        model.narrow_column(0, 2);

        assert_eq!(
            model.col_widths[0],
            8,
            "narrow_column should decrease width by the given amount"
        );
    }

    #[test]
    fn test_narrow_column_has_minimum_width() {
        let mut model = create_test_model();
        model.col_widths[0] = 5;

        // Try to narrow below minimum
        model.narrow_column(0, 10);

        assert_eq!(
            model.col_widths[0],
            3,
            "narrow_column should not go below minimum width of 3"
        );
    }

    #[test]
    fn test_widen_column_has_maximum_width() {
        let mut model = create_test_model();
        model.col_widths[0] = 199;

        // Try to widen above maximum
        model.widen_column(0, 10);

        assert_eq!(
            model.col_widths[0],
            200,
            "widen_column should not exceed maximum width of 200"
        );
    }

    #[test]
    fn test_autofit_column_fits_content() {
        let mut model = GridModel::new(
            vec!["short".to_string(), "verylongheadername".to_string()],
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string(), "d".to_string()],
            ],
        );

        // Second column should fit "verylongheadername" (18 chars)
        model.autofit_column(1);

        assert_eq!(
            model.col_widths[1],
            18,
            "autofit_column should size to longest content (header in this case)"
        );
    }

    #[test]
    fn test_generate_update_sql_with_key_column() {
        let model = GridModel::new(
            vec!["id".to_string(), "name".to_string(), "age".to_string()],
            vec![
                vec!["1".to_string(), "Alice".to_string(), "30".to_string()],
                vec!["2".to_string(), "Bob".to_string(), "25".to_string()],
            ],
        );

        let sql = model.generate_update_sql("users", &[0], Some(&["id"]));

        assert!(sql.contains("UPDATE users SET"), "Should have UPDATE clause");
        assert!(sql.contains("name = 'Alice'"), "Should set name column");
        assert!(sql.contains("age = 30"), "Should set age column (numeric)");
        assert!(sql.contains("WHERE id = 1"), "Should have WHERE with id");
    }

    #[test]
    fn test_generate_update_sql_multiple_rows() {
        let model = GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![
                vec!["1".to_string(), "Alice".to_string()],
                vec!["2".to_string(), "Bob".to_string()],
            ],
        );

        let sql = model.generate_update_sql("users", &[0, 1], Some(&["id"]));
        let lines: Vec<&str> = sql.lines().collect();

        assert_eq!(lines.len(), 2, "Should generate 2 UPDATE statements");
        assert!(lines[0].contains("WHERE id = 1"));
        assert!(lines[1].contains("WHERE id = 2"));
    }

    #[test]
    fn test_generate_delete_sql_with_all_columns() {
        let model = GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![vec!["1".to_string(), "Alice".to_string()]],
        );

        // No key columns specified = use all columns
        let sql = model.generate_delete_sql("users", &[0], None);

        assert!(sql.contains("DELETE FROM users WHERE"), "Should have DELETE clause");
        assert!(sql.contains("id = 1"), "Should have id in WHERE");
        assert!(sql.contains("name = 'Alice'"), "Should have name in WHERE");
    }

    #[test]
    fn test_generate_delete_sql_with_key_column() {
        let model = GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![vec!["1".to_string(), "Alice".to_string()]],
        );

        let sql = model.generate_delete_sql("users", &[0], Some(&["id"]));

        assert!(sql.contains("DELETE FROM users WHERE id = 1;"));
        assert!(!sql.contains("name"), "Should not include name in WHERE when id is the key");
    }

    #[test]
    fn test_generate_insert_sql() {
        let model = GridModel::new(
            vec!["id".to_string(), "name".to_string()],
            vec![
                vec!["1".to_string(), "Alice".to_string()],
                vec!["2".to_string(), "Bob".to_string()],
            ],
        );

        let sql = model.generate_insert_sql("users", &[0, 1]);

        assert!(sql.contains("INSERT INTO users (id, name) VALUES"));
        assert!(sql.contains("(1, 'Alice')"));
        assert!(sql.contains("(2, 'Bob')"));
    }

    #[test]
    fn test_generate_sql_handles_special_chars() {
        let model = GridModel::new(
            vec!["id".to_string(), "comment".to_string()],
            vec![vec!["1".to_string(), "It's a test".to_string()]],
        );

        let sql = model.generate_insert_sql("posts", &[0]);

        // Single quotes should be escaped
        assert!(sql.contains("'It''s a test'"), "Should escape single quotes");
    }

    #[test]
    fn test_generate_sql_handles_null() {
        let model = GridModel::new(
            vec!["id".to_string(), "optional".to_string()],
            vec![vec!["1".to_string(), "".to_string()]],
        );

        let sql = model.generate_insert_sql("items", &[0]);

        assert!(sql.contains("NULL"), "Empty string should become NULL");
    }

    #[test]
    fn test_generate_sql_quotes_special_identifiers() {
        let model = GridModel::new(
            vec!["user-id".to_string(), "First Name".to_string()],
            vec![vec!["1".to_string(), "Alice".to_string()]],
        );

        let sql = model.generate_insert_sql("users", &[0]);

        assert!(
            sql.contains("\"user-id\"") || sql.contains("\"First Name\""),
            "Should quote identifiers with special characters"
        );
    }
}
