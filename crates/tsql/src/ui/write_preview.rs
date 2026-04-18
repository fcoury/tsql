//! Modal for reviewing generated write operations before execution.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePreviewAction {
    Continue,
    Apply,
    EditInQuery,
    Copy,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritePreviewModal {
    title: String,
    target: String,
    field: String,
    old_value: Option<String>,
    new_value: Option<String>,
    body_label: String,
    body: String,
    scroll: usize,
    read_only: bool,
}

impl WritePreviewModal {
    pub fn review(
        target: impl Into<String>,
        field: impl Into<String>,
        old_value: Option<String>,
        new_value: Option<String>,
        body_label: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            title: " Review Write ".to_string(),
            target: target.into(),
            field: field.into(),
            old_value,
            new_value,
            body_label: body_label.into(),
            body: body.into(),
            scroll: 0,
            read_only: false,
        }
    }

    pub fn read_only(
        title: impl Into<String>,
        body_label: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            target: String::new(),
            field: String::new(),
            old_value: None,
            new_value: None,
            body_label: body_label.into(),
            body: body.into(),
            scroll: 0,
            read_only: true,
        }
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> WritePreviewAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) if !self.read_only => WritePreviewAction::Apply,
            (KeyCode::Char('e'), KeyModifiers::NONE) if !self.read_only => {
                WritePreviewAction::EditInQuery
            }
            (KeyCode::Char('y'), KeyModifiers::NONE) => WritePreviewAction::Copy,
            (KeyCode::Esc, KeyModifiers::NONE) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                WritePreviewAction::Cancel
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.scroll = self.scroll.saturating_add(1);
                WritePreviewAction::Continue
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                self.scroll = self.scroll.saturating_sub(1);
                WritePreviewAction::Continue
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL)
            | (KeyCode::PageDown, KeyModifiers::NONE) => {
                self.scroll = self.scroll.saturating_add(10);
                WritePreviewAction::Continue
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::PageUp, KeyModifiers::NONE) => {
                self.scroll = self.scroll.saturating_sub(10);
                WritePreviewAction::Continue
            }
            _ => WritePreviewAction::Continue,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 4 {
            return;
        }

        let max_width = area.width.saturating_sub(4).max(1);
        let max_height = area.height.saturating_sub(2).max(1);
        let min_width = 50.min(max_width);
        let min_height = 12.min(max_height);
        let modal_width = ((area.width as f32 * 0.82) as u16).clamp(min_width, max_width);
        let modal_height = ((area.height as f32 * 0.72) as u16).clamp(min_height, max_height);
        let modal_area = Rect {
            x: area.width.saturating_sub(modal_width) / 2,
            y: area.height.saturating_sub(modal_height) / 2,
            width: modal_width,
            height: modal_height,
        };

        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title.as_str())
            .border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if self.read_only { 0 } else { 4 }),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

        if !self.read_only {
            let meta = vec![
                Line::from(vec![
                    Span::styled("Target: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.target),
                ]),
                Line::from(vec![
                    Span::styled("Field:  ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.field),
                ]),
                Line::from(vec![
                    Span::styled("Old:    ", Style::default().fg(Color::DarkGray)),
                    Span::raw(shorten(self.old_value.as_deref().unwrap_or(""), 96)),
                ]),
                Line::from(vec![
                    Span::styled("New:    ", Style::default().fg(Color::DarkGray)),
                    Span::raw(shorten(self.new_value.as_deref().unwrap_or(""), 96)),
                ]),
            ];
            frame.render_widget(Paragraph::new(meta), chunks[0]);
        }

        let body_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.body_label))
            .border_style(Style::default().fg(Color::DarkGray));
        let body_inner = body_block.inner(chunks[1]);
        let total_lines = self.body.lines().count().max(1);
        let visible_lines = body_inner.height as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);
        self.scroll = self.scroll.min(max_scroll);

        let paragraph = Paragraph::new(self.body.clone())
            .block(body_block)
            .scroll((self.scroll as u16, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, chunks[1]);

        if total_lines > visible_lines && visible_lines > 0 {
            let scrollbar_area = Rect {
                x: body_inner.x + body_inner.width.saturating_sub(1),
                y: body_inner.y,
                width: 1,
                height: body_inner.height,
            };
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("█")
                .track_symbol(Some("│"));
            let mut state = ScrollbarState::new(max_scroll + 1).position(self.scroll);
            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut state);
        }

        let footer = if self.read_only {
            " y copy  q/Esc close  j/k scroll "
        } else {
            " Enter apply  e edit in query  y copy  Esc cancel  j/k scroll "
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                footer,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[2],
        );
    }
}

fn shorten(value: &str, max_chars: usize) -> String {
    let value = value.replace('\n', "\\n");
    let mut chars = value.chars();
    let shortened: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}
