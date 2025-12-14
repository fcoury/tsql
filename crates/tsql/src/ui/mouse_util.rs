//! Mouse utility functions for UI components.

use ratatui::layout::Rect;

/// Check if coordinates are inside a rectangle.
///
/// Uses u32 arithmetic internally to prevent overflow when adding
/// rect position and dimensions.
#[inline]
pub fn is_inside(x: u16, y: u16, rect: Rect) -> bool {
    let x = x as u32;
    let y = y as u32;
    let rx = rect.x as u32;
    let ry = rect.y as u32;
    let rw = rect.width as u32;
    let rh = rect.height as u32;

    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}
