//! Reusable confirmation prompt component for unsaved changes dialogs.
//!
//! This component provides:
//! - A centered modal dialog with a message
//! - Yes/No key bindings (y/n, arrow keys, Enter, or Esc)
//! - Mouse support (click buttons or outside to cancel)
//! - Context tracking for what action triggered the confirmation
//! - Consistent styling (yellow border for warning)

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{BorderType, Borders};
use ratatui::Frame;
use tui_confirm_dialog::{ConfirmDialog, ConfirmDialogState};

use crate::config::ConnectionEntry;

/// Widget ID for the confirmation dialog (only one dialog is used at a time).
const CONFIRM_DIALOG_ID: u16 = 0;

/// Result of handling input in the confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    /// Still waiting for user input.
    Pending,
    /// User confirmed (pressed y/Y or clicked Yes).
    Confirmed,
    /// User cancelled (pressed n/N/Esc or clicked No/outside).
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
    /// Switching to a new connection with unsaved query changes.
    SwitchConnection { entry: ConnectionEntry },
}

/// A reusable confirmation dialog for unsaved changes.
pub struct ConfirmPrompt {
    /// The underlying dialog state.
    state: ConfirmDialogState,
    /// What triggered this confirmation.
    context: ConfirmContext,
}

impl ConfirmPrompt {
    /// Create a new confirmation prompt.
    pub fn new(message: impl Into<String>, context: ConfirmContext) -> Self {
        let title = Self::title_for_context(&context);
        let mut state = ConfirmDialogState::new(CONFIRM_DIALOG_ID, title, message.into());
        state.open();
        Self { state, context }
    }

    /// Get the context that triggered this confirmation.
    pub fn context(&self) -> &ConfirmContext {
        &self.context
    }

    /// Handle a key event and return the result.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConfirmResult {
        let was_opened = self.state.is_opened();
        let _handled = self.state.handle(&key);

        if was_opened && !self.state.is_opened() {
            // Dialog was closed - check last_result for the actual outcome
            Self::result_from_last(self.state.last_result)
        } else {
            ConfirmResult::Pending
        }
    }

    /// Handle a mouse event and return the result.
    pub fn handle_mouse(&mut self, event: MouseEvent) -> ConfirmResult {
        let was_opened = self.state.is_opened();
        let _handled = self.state.handle_mouse(&event);

        if was_opened && !self.state.is_opened() {
            // Dialog was closed - check last_result for the actual outcome
            Self::result_from_last(self.state.last_result)
        } else {
            ConfirmResult::Pending
        }
    }

    /// Convert the library's result format to our ConfirmResult.
    fn result_from_last(last_result: Option<Option<bool>>) -> ConfirmResult {
        match last_result {
            Some(Some(true)) => ConfirmResult::Confirmed,
            Some(Some(false)) => ConfirmResult::Cancelled,
            Some(None) => ConfirmResult::Cancelled, // Esc or click outside
            None => ConfirmResult::Pending,
        }
    }

    /// Get the dialog title based on context.
    fn title_for_context(context: &ConfirmContext) -> &'static str {
        match context {
            ConfirmContext::CloseJsonEditor { .. }
            | ConfirmContext::CloseCellEditor { .. }
            | ConfirmContext::QuitApp
            | ConfirmContext::CloseConnectionForm
            | ConfirmContext::SwitchConnection { .. } => " Unsaved Changes ",
            ConfirmContext::QuitAppClean => " Confirm Quit ",
            ConfirmContext::DeleteConnection { .. } => " Delete Connection ",
        }
    }

    /// Render the confirmation dialog.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let dialog = ConfirmDialog::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .button_style(Style::default().fg(Color::White))
            .selected_button_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
            .text_style(Style::default().fg(Color::White));

        frame.render_stateful_widget(dialog, area, &mut self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn test_confirm_y_lowercase_returns_confirmed() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key(KeyCode::Char('y'))),
            ConfirmResult::Confirmed
        );
    }

    #[test]
    fn test_confirm_y_uppercase_returns_confirmed() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key_shift(KeyCode::Char('Y'))),
            ConfirmResult::Confirmed
        );
    }

    #[test]
    fn test_confirm_n_lowercase_returns_cancelled() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key(KeyCode::Char('n'))),
            ConfirmResult::Cancelled
        );
    }

    #[test]
    fn test_confirm_n_uppercase_returns_cancelled() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key_shift(KeyCode::Char('N'))),
            ConfirmResult::Cancelled
        );
    }

    #[test]
    fn test_confirm_esc_returns_cancelled() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);
        assert_eq!(
            prompt.handle_key(key(KeyCode::Esc)),
            ConfirmResult::Cancelled
        );
    }

    #[test]
    fn test_confirm_other_keys_return_pending() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);

        // Random keys should return Pending
        assert_eq!(
            prompt.handle_key(key(KeyCode::Char('a'))),
            ConfirmResult::Pending
        );
        assert_eq!(
            prompt.handle_key(key(KeyCode::Char('x'))),
            ConfirmResult::Pending
        );
        // Enter now confirms (selects the highlighted button)
        // Tab still returns Pending
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

    #[test]
    fn test_arrow_key_navigation() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);

        // Initially Yes is selected, so Enter confirms
        // Navigate to No with Right arrow
        assert_eq!(
            prompt.handle_key(key(KeyCode::Right)),
            ConfirmResult::Pending
        );
        // Now No is selected, Enter should cancel
        assert_eq!(
            prompt.handle_key(key(KeyCode::Enter)),
            ConfirmResult::Cancelled
        );
    }

    #[test]
    fn test_enter_confirms_when_yes_selected() {
        let mut prompt = ConfirmPrompt::new("Test?", ConfirmContext::QuitApp);

        // Initially Yes is selected
        assert_eq!(
            prompt.handle_key(key(KeyCode::Enter)),
            ConfirmResult::Confirmed
        );
    }
}
