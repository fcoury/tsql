//! Password input prompt for connections that require authentication.
//!
//! This component provides:
//! - A centered modal dialog with a password input field
//! - Masked password display (dots instead of characters)
//! - Enter to submit, Esc to cancel
//! - Stores the connection entry for context

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::config::ConnectionEntry;

/// Result of handling input in the password prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordPromptResult {
    /// Still waiting for user input.
    Pending,
    /// User submitted the password.
    Submitted(String),
    /// User cancelled (pressed Esc).
    Cancelled,
}

/// A password input prompt for database connections.
pub struct PasswordPrompt {
    /// The connection entry we're prompting for.
    entry: ConnectionEntry,
    /// The password being entered.
    password: String,
}

impl PasswordPrompt {
    /// Create a new password prompt for the given connection.
    pub fn new(entry: ConnectionEntry) -> Self {
        Self {
            entry,
            password: String::new(),
        }
    }

    /// Get the connection entry this prompt is for.
    pub fn entry(&self) -> &ConnectionEntry {
        &self.entry
    }

    /// Handle a key event and return the result.
    pub fn handle_key(&mut self, key: KeyEvent) -> PasswordPromptResult {
        match key.code {
            KeyCode::Enter => PasswordPromptResult::Submitted(std::mem::take(&mut self.password)),
            KeyCode::Esc => PasswordPromptResult::Cancelled,
            KeyCode::Backspace => {
                self.password.pop();
                PasswordPromptResult::Pending
            }
            KeyCode::Char(c) => {
                // Only add printable characters, ignore control characters
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.password.push(c);
                }
                PasswordPromptResult::Pending
            }
            _ => PasswordPromptResult::Pending,
        }
    }

    /// Render the password prompt dialog.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Calculate centered dialog size
        let dialog_width = 50u16.min(area.width.saturating_sub(4));
        let dialog_height = 7u16;

        let x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let y = area.y + (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

        // Clear the area behind the dialog
        frame.render_widget(Clear, dialog_area);

        // Create the dialog block
        let title = format!(" Password for {} ", self.entry.name);
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Layout for the content
        let chunks = Layout::vertical([
            Constraint::Length(1), // Connection info
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Password field
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Help text
        ])
        .split(inner);

        // Connection info
        let info = format!(
            "{}@{}:{}/{}",
            self.entry.user, self.entry.host, self.entry.port, self.entry.database
        );
        let info_paragraph = Paragraph::new(info)
            .style(Style::default().fg(Color::Gray))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(info_paragraph, chunks[0]);

        // Password field with masked display
        let masked: String = "\u{2022}".repeat(self.password.len()); // Unicode bullet
        let cursor = "\u{2588}"; // Block cursor
        let display = format!("{}{}", masked, cursor);

        let password_line = Line::from(vec![
            Span::styled("Password: ", Style::default().fg(Color::White)),
            Span::styled(display, Style::default().fg(Color::Yellow)),
        ]);
        let password_paragraph = Paragraph::new(password_line);
        frame.render_widget(password_paragraph, chunks[2]);

        // Help text
        let help = Line::from(vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" submit  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]);
        let help_paragraph = Paragraph::new(help).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(help_paragraph, chunks[4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn create_test_entry() -> ConnectionEntry {
        ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_enter_submits_password() {
        let mut prompt = PasswordPrompt::new(create_test_entry());
        prompt.password = "secret".to_string();

        let result = prompt.handle_key(key(KeyCode::Enter));
        assert_eq!(
            result,
            PasswordPromptResult::Submitted("secret".to_string())
        );
    }

    #[test]
    fn test_esc_cancels() {
        let mut prompt = PasswordPrompt::new(create_test_entry());
        prompt.password = "partial".to_string();

        let result = prompt.handle_key(key(KeyCode::Esc));
        assert_eq!(result, PasswordPromptResult::Cancelled);
    }

    #[test]
    fn test_typing_characters() {
        let mut prompt = PasswordPrompt::new(create_test_entry());

        prompt.handle_key(key(KeyCode::Char('a')));
        prompt.handle_key(key(KeyCode::Char('b')));
        prompt.handle_key(key(KeyCode::Char('c')));

        assert_eq!(prompt.password, "abc");
    }

    #[test]
    fn test_backspace_deletes() {
        let mut prompt = PasswordPrompt::new(create_test_entry());
        prompt.password = "test".to_string();

        prompt.handle_key(key(KeyCode::Backspace));
        assert_eq!(prompt.password, "tes");

        prompt.handle_key(key(KeyCode::Backspace));
        assert_eq!(prompt.password, "te");
    }

    #[test]
    fn test_backspace_on_empty_does_nothing() {
        let mut prompt = PasswordPrompt::new(create_test_entry());

        let result = prompt.handle_key(key(KeyCode::Backspace));
        assert_eq!(result, PasswordPromptResult::Pending);
        assert_eq!(prompt.password, "");
    }

    #[test]
    fn test_entry_accessible() {
        let entry = create_test_entry();
        let prompt = PasswordPrompt::new(entry.clone());

        assert_eq!(prompt.entry().name, "test");
        assert_eq!(prompt.entry().host, "localhost");
    }
}
