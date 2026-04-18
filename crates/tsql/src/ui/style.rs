use ratatui::style::{Color, Style};
use ratatui::text::Line;

#[cfg(test)]
use ratatui::buffer::Buffer;

/// Shared highlighted-row background.
pub(crate) const SELECTED_BG: Color = Color::DarkGray;

/// Secondary text that remains visible on `SELECTED_BG`.
pub(crate) const SELECTED_MUTED_FG: Color = Color::Gray;

/// Primary text for selected rows.
pub(crate) const SELECTED_PRIMARY_FG: Color = Color::White;

pub(crate) fn selected_row_style() -> Style {
    Style::default().bg(SELECTED_BG).fg(SELECTED_MUTED_FG)
}

pub(crate) fn selected_primary_style() -> Style {
    Style::default().bg(SELECTED_BG).fg(SELECTED_PRIMARY_FG)
}

pub(crate) fn selected_muted_style() -> Style {
    Style::default().bg(SELECTED_BG).fg(SELECTED_MUTED_FG)
}

pub(crate) fn on_selected_bg(style: Style) -> Style {
    let style = style.bg(SELECTED_BG);
    if matches!(style.fg, None | Some(Color::DarkGray)) {
        style.fg(SELECTED_MUTED_FG)
    } else {
        style
    }
}

pub(crate) fn selected_line<'a>(mut line: Line<'a>) -> Line<'a> {
    line.style = on_selected_bg(line.style);
    for span in &mut line.spans {
        span.style = on_selected_bg(span.style);
    }
    line
}

#[cfg(test)]
pub(crate) fn assert_selected_bg_has_visible_fg(buf: &Buffer) {
    for y in buf.area.y..buf.area.y.saturating_add(buf.area.height) {
        for x in buf.area.x..buf.area.x.saturating_add(buf.area.width) {
            let cell = buf.cell((x, y)).expect("cell in buffer");
            if cell.bg == SELECTED_BG {
                assert_ne!(
                    cell.fg,
                    Color::Reset,
                    "selected-bg cell at ({x}, {y}) has reset foreground"
                );
                assert_ne!(
                    cell.fg,
                    Color::DarkGray,
                    "selected-bg cell at ({x}, {y}) has dark gray foreground"
                );
            }
        }
    }
}
