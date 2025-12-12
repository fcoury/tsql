use crossterm::event::KeyEvent;
use ratatui::style::{Modifier, Style};
use tui_textarea::{CursorMove, Input, TextArea};

pub struct SearchPrompt {
    pub active: bool,
    pub textarea: TextArea<'static>,
    /// When set, n/N moves between matches in the query editor.
    pub last_applied: Option<String>,
}

impl SearchPrompt {
    pub fn new() -> Self {
        let mut textarea = TextArea::new(vec![String::new()]);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            active: false,
            textarea,
            last_applied: None,
        }
    }

    pub fn open(&mut self) {
        self.active = true;
        self.textarea = TextArea::new(vec![String::new()]);
        self.textarea
            .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }
}

impl Default for SearchPrompt {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CommandPrompt {
    pub active: bool,
    pub textarea: TextArea<'static>,
}

impl CommandPrompt {
    pub fn new() -> Self {
        let mut textarea = TextArea::new(vec![String::new()]);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            active: false,
            textarea,
        }
    }

    pub fn open(&mut self) {
        self.active = true;
        self.textarea = TextArea::new(vec![String::new()]);
        self.textarea
            .set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }
}

impl Default for CommandPrompt {
    fn default() -> Self {
        Self::new()
    }
}

pub struct QueryEditor {
    pub textarea: TextArea<'static>,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
}

impl QueryEditor {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            history_draft: None,
        }
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn set_text(&mut self, s: String) {
        let lines: Vec<String> = if s.is_empty() {
            vec![String::new()]
        } else {
            s.lines().map(|l| l.to_string()).collect()
        };

        // Recreate the underlying textarea content.
        let mut textarea = TextArea::new(lines);
        textarea.set_cursor_line_style(Style::default().add_modifier(Modifier::UNDERLINED));
        self.textarea = textarea;

        self.history_index = None;
        self.history_draft = None;
    }

    pub fn push_history(&mut self, query: String) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.history.last().map(|s| s.trim()) == Some(trimmed) {
            return;
        }

        self.history.push(trimmed.to_string());
        self.history_index = None;
        self.history_draft = None;
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        if self.history_index.is_none() {
            self.history_draft = Some(self.text());
            self.history_index = Some(self.history.len().saturating_sub(1));
        } else {
            let i = self.history_index.unwrap();
            self.history_index = Some(i.saturating_sub(1));
        }

        if let Some(i) = self.history_index {
            self.set_text(self.history[i].clone());
        }
    }

    pub fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let Some(i) = self.history_index else {
            return;
        };

        let next = i + 1;
        if next >= self.history.len() {
            self.history_index = None;
            if let Some(draft) = self.history_draft.take() {
                self.set_text(draft);
            }
            return;
        }

        self.history_index = Some(next);
        self.set_text(self.history[next].clone());
    }

    pub fn input(&mut self, key: KeyEvent) {
        let input: Input = key.into();
        self.textarea.input(input);

        // If the user starts typing/editing, stop history navigation.
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft = None;
        }
    }

    /// Delete the entire current line (vim `dd`).
    pub fn delete_line(&mut self) {
        // Select the entire line and cut it.
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.delete_line_by_end();
        // If we're not on the last line, delete the newline too.
        self.textarea.delete_newline();
    }

    /// Clear the current line content but keep the line (vim `cc`).
    pub fn change_line(&mut self) {
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.delete_line_by_end();
    }

    /// Yank (copy) the current line (vim `yy`).
    pub fn yank_line(&mut self) {
        let (row, _) = self.textarea.cursor();
        let lines = self.textarea.lines();
        if row < lines.len() {
            let line = lines[row].clone() + "\n";
            self.textarea.set_yank_text(line);
        }
    }
}

impl Default for QueryEditor {
    fn default() -> Self {
        Self::new()
    }
}
