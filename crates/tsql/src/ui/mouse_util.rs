//! Mouse utility functions for UI components.

use ratatui::layout::Rect;

/// Check if coordinates are inside a rectangle.
#[inline]
pub fn is_inside(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}
