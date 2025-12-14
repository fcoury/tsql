//! A minimal key hint popup displayed in the bottom-right corner.
//!
//! Shows available key completions when a multi-key sequence is pending.
//! Inspired by Helix editor's which-key style hints.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::key_sequence::PendingKey;

/// A single hint entry showing a key and its description.
#[derive(Debug, Clone)]
pub struct KeyHint {
    /// The key to press (e.g., "g", "e", "c")
    pub key: &'static str,
    /// Short description of what the key does
    pub description: &'static str,
}

impl KeyHint {
    pub const fn new(key: &'static str, description: &'static str) -> Self {
        Self { key, description }
    }
}

/// Hints for the 'g' (goto) prefix
const G_HINTS: &[KeyHint] = &[
    KeyHint::new("g", "first row"),
    KeyHint::new("e", "editor"),
    KeyHint::new("c", "connections"),
    KeyHint::new("t", "tables"),
    KeyHint::new("r", "results"),
];

/// The key hint popup widget.
pub struct KeyHintPopup {
    /// The currently pending key
    pending_key: PendingKey,
}

impl KeyHintPopup {
    /// Creates a new popup for the given pending key.
    pub fn new(pending_key: PendingKey) -> Self {
        Self { pending_key }
    }

    /// Returns the hints for the current pending key.
    fn hints(&self) -> &'static [KeyHint] {
        match self.pending_key {
            PendingKey::G => G_HINTS,
        }
    }

    /// Returns the title character for the popup.
    fn title_char(&self) -> char {
        self.pending_key.display_char()
    }

    /// Calculates the popup area positioned in the bottom-right corner.
    fn popup_area(&self, frame_area: Rect) -> Rect {
        let hints = self.hints();

        // Minimum sensible dimensions
        const MIN_WIDTH: u16 = 10;
        const MIN_HEIGHT: u16 = 3;
        const PADDING: u16 = 2;

        // Calculate dimensions based on content using Unicode display width
        // Width: " key" (space + key) + "  " (2 spaces) + description + " " (trailing space) + borders (2)
        let max_content_width = hints
            .iter()
            .map(|h| {
                let key_width = 1 + h.key.width(); // leading space + key
                let desc_width = h.description.width();
                key_width + 2 + desc_width + 1 // " key" + "  " + desc + " "
            })
            .max()
            .unwrap_or(10);

        // Add borders (2 chars for left + right)
        let desired_width = (max_content_width + 2) as u16;

        // Height: number of hints + borders (top and bottom)
        let desired_height = (hints.len() + 2) as u16;

        // Clamp dimensions to fit within frame_area, respecting padding
        let max_available_width = frame_area.width.saturating_sub(PADDING);
        let max_available_height = frame_area.height.saturating_sub(PADDING);

        let width = desired_width
            .max(MIN_WIDTH)
            .min(max_available_width)
            .min(frame_area.width); // Final safety clamp

        let height = desired_height
            .max(MIN_HEIGHT)
            .min(max_available_height)
            .min(frame_area.height); // Final safety clamp

        // Position in bottom-right with padding
        let x = frame_area.width.saturating_sub(width + PADDING);
        let y = frame_area.height.saturating_sub(height + PADDING);

        Rect::new(x, y, width, height)
    }

    /// Renders the popup to the frame.
    pub fn render(&self, frame: &mut Frame, frame_area: Rect) {
        let area = self.popup_area(frame_area);

        // Clear the background
        frame.render_widget(Clear, area);

        // Build the content lines
        let hints = self.hints();
        let lines: Vec<Line> = hints
            .iter()
            .map(|hint| {
                Line::from(vec![
                    Span::styled(
                        format!(" {}", hint.key),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(hint.description, Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                ])
            })
            .collect();

        // Create the paragraph with a titled border
        let title = format!(" {} ", self.title_char());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));

        let paragraph = Paragraph::new(lines).block(block);

        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g_hints() {
        let popup = KeyHintPopup::new(PendingKey::G);
        let hints = popup.hints();

        assert_eq!(hints.len(), 5);
        assert_eq!(hints[0].key, "g");
        assert_eq!(hints[0].description, "first row");
        assert_eq!(hints[1].key, "e");
        assert_eq!(hints[4].key, "r");
    }

    #[test]
    fn test_title_char() {
        let popup = KeyHintPopup::new(PendingKey::G);
        assert_eq!(popup.title_char(), 'g');
    }

    #[test]
    fn test_popup_area_calculation() {
        let popup = KeyHintPopup::new(PendingKey::G);
        let frame_area = Rect::new(0, 0, 100, 50);
        let area = popup.popup_area(frame_area);

        // Should be in bottom-right
        assert!(area.x > 50);
        assert!(area.y > 40);

        // Should have reasonable size
        assert!(area.width >= 15);
        assert!(area.height == 7); // 5 hints + 2 borders
    }

    #[test]
    fn test_popup_area_clamped_on_small_terminal() {
        let popup = KeyHintPopup::new(PendingKey::G);
        // Very small terminal
        let frame_area = Rect::new(0, 0, 15, 5);
        let area = popup.popup_area(frame_area);

        // Width and height should never exceed frame_area
        assert!(area.width <= frame_area.width);
        assert!(area.height <= frame_area.height);

        // Position should be valid (within frame bounds)
        assert!(area.x + area.width <= frame_area.width);
        assert!(area.y + area.height <= frame_area.height);
    }
}
