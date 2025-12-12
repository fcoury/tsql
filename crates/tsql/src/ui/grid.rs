use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
            (KeyCode::PageUp, _) => {
                self.cursor_row = self.cursor_row.saturating_sub(10);
            }
            (KeyCode::PageDown, _) => {
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

            // Horizontal scroll is column-based (jump by columns).
            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                self.col_offset = self.col_offset.saturating_sub(1);
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
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

    pub fn ensure_cursor_visible(&mut self, viewport_rows: usize, row_count: usize) {
        if viewport_rows == 0 || row_count == 0 {
            self.row_offset = 0;
            self.cursor_row = 0;
            return;
        }

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
}

pub struct GridModel {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub col_widths: Vec<u16>,
}

impl GridModel {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let col_widths = compute_column_widths(&headers, &rows);
        Self {
            headers,
            rows,
            col_widths,
        }
    }

    pub fn empty() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            col_widths: Vec::new(),
        }
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
        let base_title = "Results (j/k move, h/l scroll cols, Space select, / search)";
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

        // Header row (frozen).
        render_marker_header(header_area, buf, marker_w);
        render_row_cells(
            data_x,
            header_area.y,
            data_w,
            &self.model.headers,
            &self.model.col_widths,
            self.state.col_offset,
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

        let mut state = self.state.clone();
        state.ensure_cursor_visible(body_area.height as usize, self.model.rows.len());

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

            render_row_cells_with_search(
                data_x,
                y,
                data_w,
                &self.model.rows[row_idx],
                &self.model.col_widths,
                state.col_offset,
                row_style,
                row_idx,
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

/// Render row cells with search highlighting.
fn render_row_cells_with_search(
    mut x: u16,
    y: u16,
    available_w: u16,
    cells: &[String],
    col_widths: &[u16],
    col_offset: usize,
    base_style: Style,
    row_idx: usize,
    search: &GridSearch,
    buf: &mut Buffer,
) {
    if available_w == 0 {
        return;
    }

    let padding: u16 = 1;
    let max_x = x.saturating_add(available_w);

    // Styles for search matches
    let match_style = Style::default().bg(Color::Yellow).fg(Color::Black);
    let current_match_style = Style::default().bg(Color::Rgb(255, 165, 0)).fg(Color::Black); // Orange

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

        // Determine cell style based on search state
        let cell_style = if search.is_current_match(row_idx, col) {
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
}
