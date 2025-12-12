use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Default, Clone)]
pub struct GridState {
    pub row_offset: usize,
    pub col_offset: usize,
    pub cursor_row: usize,
    pub selected_rows: BTreeSet<usize>,
}

impl GridState {
    pub fn handle_key(&mut self, key: KeyEvent, model: &GridModel) {
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
                    return;
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
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.selected_rows.clear();
            }

            _ => {}
        }
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
}

pub struct DataGrid<'a> {
    pub model: &'a GridModel,
    pub state: &'a GridState,
    pub focused: bool,
}

impl<'a> Widget for DataGrid<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = "Results (j/k move, h/l scroll cols, Space select)";

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

            render_row_cells(
                data_x,
                y,
                data_w,
                &self.model.rows[row_idx],
                &self.model.col_widths,
                state.col_offset,
                row_style,
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
