//! Vim commands and motions.
//!
//! This module defines the commands that can be produced by the vim handler.
//! These are high-level operations that the editor then executes using
//! its specific implementation (e.g., tui_textarea).

use tui_textarea::CursorMove;

use super::VimMode;

/// A motion defines cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Move cursor in a specific direction (wraps CursorMove).
    Cursor(CursorMove),
    /// Move up N lines.
    Up(usize),
    /// Move down N lines.
    Down(usize),
}

impl Motion {
    /// Create a motion for a single cursor move.
    pub fn cursor(cm: CursorMove) -> Self {
        Motion::Cursor(cm)
    }

    /// Move left (back) one character.
    pub fn left() -> Self {
        Motion::Cursor(CursorMove::Back)
    }

    /// Move right (forward) one character.
    pub fn right() -> Self {
        Motion::Cursor(CursorMove::Forward)
    }

    /// Move up one line.
    pub fn up() -> Self {
        Motion::Cursor(CursorMove::Up)
    }

    /// Move down one line.
    pub fn down() -> Self {
        Motion::Cursor(CursorMove::Down)
    }

    /// Move to start of line.
    pub fn line_start() -> Self {
        Motion::Cursor(CursorMove::Head)
    }

    /// Move to end of line.
    pub fn line_end() -> Self {
        Motion::Cursor(CursorMove::End)
    }

    /// Move to start of document.
    pub fn document_start() -> Self {
        Motion::Cursor(CursorMove::Top)
    }

    /// Move to end of document.
    pub fn document_end() -> Self {
        Motion::Cursor(CursorMove::Bottom)
    }

    /// Move forward one word.
    pub fn word_forward() -> Self {
        Motion::Cursor(CursorMove::WordForward)
    }

    /// Move backward one word.
    pub fn word_back() -> Self {
        Motion::Cursor(CursorMove::WordBack)
    }

    /// Move to end of word.
    pub fn word_end() -> Self {
        Motion::Cursor(CursorMove::WordEnd)
    }
}

/// A text object defines a region of text (for operations like `ciw`, `daw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    /// Inner word (iw) - just the word.
    InnerWord,
    /// A word (aw) - word plus surrounding whitespace.
    AWord,
    /// Inner WORD (iW) - just the WORD (whitespace-delimited).
    InnerWORD,
    /// A WORD (aW) - WORD plus surrounding whitespace.
    AWORD,
    /// Inner paragraph (ip).
    InnerParagraph,
    /// A paragraph (ap).
    AParagraph,
    /// Inner quotes (i", i').
    InnerQuotes(char),
    /// A quotes (a", a').
    AQuotes(char),
    /// Inner brackets (i(, i[, i{, i<).
    InnerBrackets(char),
    /// A brackets (a(, a[, a{, a<).
    ABrackets(char),
}

/// An operator that can be combined with a motion or text object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// Delete operator (d).
    Delete,
    /// Change operator (c) - delete and enter insert mode.
    Change,
    /// Yank operator (y) - copy to register.
    Yank,
}

/// A complete vim command to be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimCommand {
    /// No operation - continue waiting for input.
    None,

    /// Move the cursor.
    Move(Motion),

    /// Enter a different mode.
    ChangeMode(VimMode),

    /// Enter insert mode at a specific position.
    EnterInsertAt {
        /// Where to position cursor before entering insert.
        motion: Option<Motion>,
        /// The new mode (always Insert, but explicit for clarity).
        mode: VimMode,
    },

    /// Open a new line and enter insert mode.
    OpenLine {
        /// True for 'O' (above), false for 'o' (below).
        above: bool,
    },

    /// Delete character under cursor (x).
    DeleteChar,

    /// Delete character before cursor (X).
    DeleteCharBefore,

    /// Delete to end of line (D).
    DeleteToEnd,

    /// Delete entire line (dd).
    DeleteLine,

    /// Delete by motion (dw, de, db, d$, d0, etc.).
    DeleteMotion(Motion),

    /// Change to end of line (C).
    ChangeToEnd,

    /// Change entire line (cc or S).
    ChangeLine,

    /// Change by motion (cw, ce, cb, c$, c0, etc.).
    ChangeMotion(Motion),

    /// Yank (copy) entire line (yy or Y).
    YankLine,

    /// Yank by motion (yw, ye, yb, y$, y0, etc.).
    YankMotion(Motion),

    /// Paste after cursor (p).
    PasteAfter,

    /// Paste before cursor (P).
    PasteBefore,

    /// Undo.
    Undo,

    /// Redo.
    Redo,

    /// Start visual selection.
    StartVisual,

    /// Cancel visual selection.
    CancelVisual,

    /// In visual mode: yank selection.
    VisualYank,

    /// In visual mode: delete selection.
    VisualDelete,

    /// In visual mode: change selection (delete and enter insert).
    VisualChange,

    /// Pass key through to the textarea (for insert mode).
    PassThrough,

    /// Editor-specific action (e.g., save, execute, cancel).
    Custom(String),
}

impl VimCommand {
    /// Create a custom command.
    pub fn custom(name: impl Into<String>) -> Self {
        VimCommand::Custom(name.into())
    }

    /// Returns true if this command requires mode change to Insert.
    pub fn enters_insert_mode(&self) -> bool {
        matches!(
            self,
            VimCommand::ChangeMode(VimMode::Insert)
                | VimCommand::EnterInsertAt { .. }
                | VimCommand::OpenLine { .. }
                | VimCommand::ChangeToEnd
                | VimCommand::ChangeLine
                | VimCommand::ChangeMotion(_)
                | VimCommand::VisualChange
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_motion_constructors() {
        assert_eq!(Motion::left(), Motion::Cursor(CursorMove::Back));
        assert_eq!(Motion::right(), Motion::Cursor(CursorMove::Forward));
        assert_eq!(Motion::up(), Motion::Cursor(CursorMove::Up));
        assert_eq!(Motion::down(), Motion::Cursor(CursorMove::Down));
    }

    #[test]
    fn test_vim_command_enters_insert() {
        assert!(VimCommand::ChangeMode(VimMode::Insert).enters_insert_mode());
        assert!(VimCommand::ChangeToEnd.enters_insert_mode());
        assert!(VimCommand::ChangeLine.enters_insert_mode());
        assert!(VimCommand::OpenLine { above: false }.enters_insert_mode());

        assert!(!VimCommand::DeleteLine.enters_insert_mode());
        assert!(!VimCommand::Move(Motion::left()).enters_insert_mode());
        assert!(!VimCommand::Undo.enters_insert_mode());
    }
}
