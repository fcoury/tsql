//! Shared style test helpers.
//!
//! Runtime styling lives in [`super::theme`]; the helpers here back the
//! render tests that assert legibility invariants on painted surfaces.

#[cfg(test)]
use ratatui::buffer::Buffer;
#[cfg(test)]
use ratatui::style::Color;

/// Assert every non-blank cell carries an explicit foreground color.
///
/// On tonal-zone and overlay surfaces a `Color::Reset` foreground would fall
/// back to the terminal default, which is not guaranteed to be legible on the
/// painted background.
#[cfg(test)]
pub(crate) fn assert_nonblank_cells_have_explicit_fg(buf: &Buffer) {
    for y in buf.area.y..buf.area.y.saturating_add(buf.area.height) {
        for x in buf.area.x..buf.area.x.saturating_add(buf.area.width) {
            let cell = buf.cell((x, y)).expect("cell in buffer");
            if !cell.symbol().trim().is_empty() {
                assert_ne!(
                    cell.fg,
                    Color::Reset,
                    "nonblank cell at ({x}, {y}) has reset foreground"
                );
            }
        }
    }
}
