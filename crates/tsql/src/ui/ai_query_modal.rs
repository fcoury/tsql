//! Full-screen modal for the AI query assistant.
//!
//! Owns the prompt input, conversation history, latest proposal, and pending
//! request state. All keyboard input is captured while the modal is open;
//! `App` delegates to [`AiQueryModal::handle_key`] and reacts to the returned
//! [`AiQueryModalAction`].
//!
//! Key bindings inside the modal:
//! - **Ctrl+E** -- send the current prompt to the AI backend.
//! - **Ctrl+Y** -- accept the latest proposal into the query editor.
//! - **Ctrl+U** -- clear the prompt input.
//! - **Esc** -- close the modal without accepting.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use tui_textarea::{Input, TextArea};

use crate::ai::{AiProposal, AiTurn};

/// Result of handling a single key event inside the AI modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiQueryModalAction {
    /// Key was consumed; no state change needed in `App`.
    Continue,
    /// User wants to close the modal.
    Close,
    /// User wants to send the given prompt to the AI backend.
    Send { prompt: String },
    /// User wants to accept the latest proposal into the query editor.
    Accept,
}

/// Modal state for the AI query assistant.
///
/// The modal tracks its own conversation and the latest AI proposal. `App`
/// calls [`begin_request`](Self::begin_request) before spawning the async
/// task and [`apply_reply`](Self::apply_reply) when the `DbEvent::AiReply`
/// arrives.
pub struct AiQueryModal {
    input: TextArea<'static>,
    conversation: Vec<AiTurn>,
    latest_proposal: Option<AiProposal>,
    pending: bool,
    /// The prompt text that was sent with the in-flight request, used to
    /// record the conversation turn when the reply arrives.
    pending_prompt: Option<String>,
    last_error: Option<String>,
}

impl AiQueryModal {
    pub fn new(prefill: Option<String>) -> Self {
        let mut input = TextArea::new(vec![prefill.unwrap_or_default()]);
        input.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
        Self {
            input,
            conversation: Vec::new(),
            latest_proposal: None,
            pending: false,
            pending_prompt: None,
            last_error: None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn conversation(&self) -> Vec<AiTurn> {
        self.conversation.clone()
    }

    pub fn latest_query(&self) -> Option<&str> {
        self.latest_proposal.as_ref().map(|p| p.query.as_str())
    }

    pub fn set_input_text(&mut self, text: String) {
        self.input = TextArea::new(vec![text]);
        self.input
            .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
        self.last_error = None;
    }

    /// Mark the modal as "waiting for AI response" and stash the prompt so it
    /// can be recorded into the conversation when the reply arrives.
    pub fn begin_request(&mut self, prompt: String) {
        self.pending = true;
        self.pending_prompt = Some(prompt);
        self.last_error = None;
    }

    /// Consume an AI backend response. On success the proposal is stored and a
    /// conversation turn is recorded. On failure the error is shown in the
    /// status area. If no `pending_prompt` is set (shouldn't happen in
    /// practice), the call is a no-op.
    pub fn apply_reply(&mut self, result: std::result::Result<AiProposal, String>) {
        let Some(prompt) = self.pending_prompt.take() else {
            self.pending = false;
            return;
        };

        self.pending = false;
        match result {
            Ok(proposal) => {
                self.last_error = None;
                self.conversation.push(AiTurn {
                    user_prompt: prompt,
                    assistant_query: proposal.query.clone(),
                });
                self.latest_proposal = Some(proposal);
            }
            Err(error) => {
                self.last_error = Some(error);
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AiQueryModalAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, KeyModifiers::NONE) => AiQueryModalAction::Close,
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                if self.pending {
                    self.last_error = Some("AI request already running".to_string());
                    return AiQueryModalAction::Continue;
                }
                let prompt = self.text().trim().to_string();
                if prompt.is_empty() {
                    self.last_error = Some("Prompt cannot be empty".to_string());
                    return AiQueryModalAction::Continue;
                }
                self.input = TextArea::new(vec![String::new()]);
                self.input
                    .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
                AiQueryModalAction::Send { prompt }
            }
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                if self.pending {
                    self.last_error = Some("Wait for AI response before accepting".to_string());
                    return AiQueryModalAction::Continue;
                }
                if self.latest_proposal.is_none() {
                    self.last_error = Some("No AI proposal to accept yet".to_string());
                    return AiQueryModalAction::Continue;
                }
                AiQueryModalAction::Accept
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.input = TextArea::new(vec![String::new()]);
                self.input
                    .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
                self.last_error = None;
                AiQueryModalAction::Continue
            }
            _ => {
                let input: Input = key.into();
                self.input.input(input);
                self.last_error = None;
                AiQueryModalAction::Continue
            }
        }
    }

    fn text(&self) -> String {
        self.input.lines().join("\n")
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let width = modal_dimension(area.width, 90, 70);
        let height = modal_dimension(area.height, 80, 18);

        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" AI Query Assistant ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(block, popup);

        let inner = popup.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);

        let header = Line::from(vec![
            Span::raw("Ctrl+E send  "),
            Span::raw("Ctrl+Y accept  "),
            Span::raw("Esc close"),
        ]);
        frame.render_widget(
            Paragraph::new(header).style(Style::default().fg(Color::Gray)),
            chunks[0],
        );

        self.input.set_block(
            Block::default()
                .title(" Prompt / Follow-up ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
        frame.render_widget(&self.input, chunks[1]);

        let mut details = Vec::new();
        if self.pending {
            details.push(Line::from(Span::styled(
                "Generating query...",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            if let Some(prompt) = self.pending_prompt.as_deref() {
                details.push(Line::from(format!("Last request: {}", prompt.trim())));
            }
            details.push(Line::from(""));
        }

        if let Some(proposal) = self.latest_proposal.as_ref() {
            details.push(Line::from(Span::styled(
                "Proposed query:",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in proposal.query.lines() {
                details.push(Line::from(line.to_string()));
            }
            if let Some(explanation) = proposal.explanation.as_deref() {
                details.push(Line::from(""));
                details.push(Line::from(Span::styled(
                    "Explanation:",
                    Style::default().fg(Color::Gray),
                )));
                details.push(Line::from(explanation.to_string()));
            }
            details.push(Line::from(""));
        } else {
            details.push(Line::from(Span::styled(
                "No proposal yet. Type a prompt and press Ctrl+E.",
                Style::default().fg(Color::Gray),
            )));
            details.push(Line::from(""));
        }

        if !self.conversation.is_empty() {
            details.push(Line::from(Span::styled(
                "Recent turns:",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )));
            for turn in self.conversation.iter().rev().take(3).rev() {
                details.push(Line::from(format!("U: {}", turn.user_prompt.trim())));
                details.push(Line::from(format!("A: {}", turn.assistant_query.trim())));
                details.push(Line::from(""));
            }
        }

        frame.render_widget(
            Paragraph::new(details)
                .block(
                    Block::default()
                        .title(" Proposal ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Blue)),
                )
                .wrap(Wrap { trim: false }),
            chunks[2],
        );

        let status = if let Some(err) = self.last_error.as_deref() {
            Line::from(Span::styled(
                err.to_string(),
                Style::default().fg(Color::Red),
            ))
        } else if self.pending {
            Line::from(Span::styled(
                "Waiting for AI response...",
                Style::default().fg(Color::Cyan),
            ))
        } else {
            Line::from(Span::styled(
                "Accept replaces the entire query editor content.",
                Style::default().fg(Color::Gray),
            ))
        };
        frame.render_widget(Paragraph::new(status), chunks[3]);
    }
}

fn modal_dimension(total: u16, percent: u16, preferred_min: u16) -> u16 {
    let max = total.saturating_sub(2).max(1);
    let desired = total.saturating_mul(percent) / 100;
    desired.max(preferred_min).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn test_render_does_not_panic_on_small_terminal() {
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut modal = AiQueryModal::new(None);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    modal.render(frame, area);
                })
                .unwrap();
        }));

        assert!(result.is_ok(), "render should not panic on a small terminal");
    }

    #[test]
    fn test_modal_dimension_caps_to_available_space() {
        assert_eq!(modal_dimension(20, 90, 70), 18);
        assert_eq!(modal_dimension(8, 80, 18), 6);
        assert_eq!(modal_dimension(120, 90, 70), 108);
    }
}
