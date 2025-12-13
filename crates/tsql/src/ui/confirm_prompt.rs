//! Reusable confirmation prompt component for unsaved changes dialogs.
//!
//! This component provides:
//! - A centered modal dialog with a message
//! - Yes/No key bindings (y/n or Esc)
//! - Context tracking for what action triggered the confirmation
//! - Consistent styling (yellow border for warning)

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Result of handling a key in the confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    /// Still waiting for user input.
    Pending,
    /// User confirmed (pressed y/Y).
    Confirmed,
    /// User cancelled (pressed n/N/Esc).
    Cancelled,
}

/// Context describing what action triggered the confirmation.
#[derive(Debug, Clone)]
pub enum ConfirmContext {
    /// Closing JSON editor with unsaved changes.
    CloseJsonEditor { row: usize, col: usize },
    /// Closing inline cell editor with unsaved changes.
    CloseCellEditor { row: usize, col: usize },
    /// Quitting application with unsaved query.
    QuitApp,
    /// Quitting application without unsaved changes (clean quit).
    QuitAppClean,
    /// Deleting a saved connection.
    DeleteConnection { name: String },
    /// Closing connection form with unsaved changes.
    CloseConnectionForm,
}

/// A reusable confirmation dialog for unsaved changes.
pub struct ConfirmPrompt {
    /// The message to display.
    message: String,
    /// What triggered this confirmation.
    context: ConfirmContext,
}

impl ConfirmPrompt {
    /// Create a new confirmation prompt.
    pub fn new(message: impl Into<String>, context: ConfirmContext) -> Self {
        Self {
            message: message.into(),
            context,
        }
    }

    /// Get the context that triggered this confirmation.
    pub fn context(&self) -> &ConfirmContext {
        &self.context
    }

    /// Handle a key event and return the result.
    pub fn handle_key(&self, key: KeyEvent) -> ConfirmResult {
        match (key.code, key.modifiers) {
            // y or Y confirms
            (KeyCode::Char('y'), KeyModifiers::NONE)
            | (KeyCode::Char('Y'), KeyModifiers::SHIFT)
            | (KeyCode::Char('Y'), KeyModifiers::NONE) => ConfirmResult::Confirmed,

            // n, N, or Esc cancels
            (KeyCode::Char('n'), KeyModifiers::NONE)
            | (KeyCode::Char('N'), KeyModifiers::SHIFT)
            | (KeyCode::Char('N'), KeyModifiers::NONE)
            | (KeyCode::Esc, KeyModifiers::NONE) => ConfirmResult::Cancelled,

            // All other keys are ignored
            _ => ConfirmResult::Pending,
        }
    }

    /// Get the dialog title based on context.
    fn title(&self) -> &'static str {
        match self.context {
            ConfirmContext::CloseJsonEditor { .. }
            | ConfirmContext::CloseCellEditor { .. }
            | ConfirmContext::QuitApp
            | ConfirmContext::CloseConnectionForm => " Unsaved Changes ",
            ConfirmContext::QuitAppClean => " Confirm Quit ",
            ConfirmContext::DeleteConnection { .. } => " Delete Connection ",
        }
    }

    /// Render the confirmation dialog.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Calculate dialog size
        let dialog_width = 50u16.min(area.width.saturating_sub(4));
        let dialog_height = 7u16;
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area = Rect {
            x: dialog_x,
            y: dialog_y,
            width: dialog_width,
            height: dialog_height,
        };

        // Clear the background
        frame.render_widget(Clear, dialog_area);

        // Create the dialog block with yellow border (warning color)
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title())
            .title_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Build the content
        let lines = vec![
            Line::from(""),
            Line::from(self.message.as_str()),
            Line::from(""),
            Line::from(vec![
                Span::raw("               "),
                Span::styled("(y)es", Style::default().fg(Color::Red)),
                Span::raw("   "),
                Span::styled("(n)o", Style::default().fg(Color::Green)),
            ]),
            Line::from(""),
        ];

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn test_confirm_y_lowercase_returns_confirmed() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(prompt.handle_key(key(KeyCode::Char('y'))), ConfirmResult::Confirmed);
    }

    #[test]
    fn test_confirm_y_uppercase_returns_confirmed() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key_shift(KeyCode::Char('Y'))),
            ConfirmResult::Confirmed
        );
    }

    #[test]
    fn test_confirm_n_lowercase_returns_cancelled() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(prompt.handle_key(key(KeyCode::Char('n'))), ConfirmResult::Cancelled);
    }

    #[test]
    fn test_confirm_n_uppercase_returns_cancelled() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key_shift(KeyCode::Char('N'))),
            ConfirmResult::Cancelled
        );
    }

    #[test]
    fn test_confirm_esc_returns_cancelled() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(prompt.handle_key(key(KeyCode::Esc)), ConfirmResult::Cancelled);
    }

    #[test]
    fn test_confirm_other_keys_return_pending() {
        let prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);

        // Random keys should return Pending
        assert_eq!(prompt.handle_key(key(KeyCode::Char('a'))), ConfirmResult::Pending);
        assert_eq!(prompt.handle_key(key(KeyCode::Char('x'))), ConfirmResult::Pending);
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), ConfirmResult::Pending);
        assert_eq!(prompt.handle_key(key(KeyCode::Tab)), ConfirmResult::Pending);
    }

    #[test]
    fn test_confirm_context_accessible() {
        let prompt = ConfirmPrompt::new(
            "Discard?",
            ConfirmContext::CloseJsonEditor { row: 5, col: 3 },
        );

        match prompt.context() {
            ConfirmContext::CloseJsonEditor { row, col } => {
                assert_eq!(*row, 5);
                assert_eq!(*col, 3);
            }
            _ => panic!("Expected CloseJsonEditor context"),
        }
    }

    #[test]
    fn test_confirm_quit_app_context() {
        let prompt = ConfirmPrompt::new("Quit?", ConfirmContext::QuitApp);

        assert!(matches!(prompt.context(), ConfirmContext::QuitApp));
    }

    #[test]
    fn test_confirm_cell_editor_context() {
        let prompt = ConfirmPrompt::new(
            "Discard?",
            ConfirmContext::CloseCellEditor { row: 1, col: 2 },
        );

        match prompt.context() {
            ConfirmContext::CloseCellEditor { row, col } => {
                assert_eq!(*row, 1);
                assert_eq!(*col, 2);
            }
            _ => panic!("Expected CloseCellEditor context"),
        }
    }
}
