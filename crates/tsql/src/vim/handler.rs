//! Vim key event handler.
//!
//! This module processes key events and converts them into vim commands.
//! It handles operator-pending mode, multi-key sequences (like `gg`, `dd`),
//! and different behaviors for Normal, Insert, and Visual modes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::command::{Motion, VimCommand};
use super::mode::VimMode;

/// Configuration for vim behavior.
#[derive(Debug, Clone)]
pub struct VimConfig {
    /// Number of lines to scroll with Ctrl+d/u (half page).
    pub half_page_lines: usize,
    /// Number of lines to scroll with Ctrl+f/b (full page).
    pub full_page_lines: usize,
    /// Whether visual mode is enabled.
    pub enable_visual: bool,
    /// Whether search is enabled.
    pub enable_search: bool,
    /// Whether Esc immediately exits insert mode or requires double-tap.
    pub double_esc_to_exit: bool,
}

impl Default for VimConfig {
    fn default() -> Self {
        Self {
            half_page_lines: 10,
            full_page_lines: 20,
            enable_visual: true,
            enable_search: true,
            double_esc_to_exit: false,
        }
    }
}

impl VimConfig {
    /// Create config for JSON editor (single Esc to close, no search).
    pub fn json_editor() -> Self {
        Self {
            half_page_lines: 10,
            full_page_lines: 20,
            enable_visual: true,
            enable_search: false,
            double_esc_to_exit: false, // Changed: single Esc triggers close
        }
    }
}

/// Pending operator state for commands like `dw`, `cw`, `yy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOp {
    /// No pending operator.
    None,
    /// Delete operator (d).
    Delete,
    /// Change operator (c).
    Change,
    /// Yank operator (y).
    Yank,
    /// Waiting for second 'g' (for gg).
    G,
}

/// Vim key event handler.
///
/// Processes key events based on the current mode and returns
/// high-level vim commands to be executed by the editor.
#[derive(Debug, Clone)]
pub struct VimHandler {
    /// Configuration options.
    config: VimConfig,
    /// Pending operator (d, c, y, g).
    pending: PendingOp,
    /// Whether Esc was pressed (for double-Esc detection).
    esc_pressed: bool,
}

impl VimHandler {
    /// Create a new handler with the given configuration.
    pub fn new(config: VimConfig) -> Self {
        Self {
            config,
            pending: PendingOp::None,
            esc_pressed: false,
        }
    }

    /// Create a handler with default configuration.
    pub fn default_config() -> Self {
        Self::new(VimConfig::default())
    }

    /// Handle a key event in the given mode.
    ///
    /// Returns a `VimCommand` describing what action to take.
    pub fn handle_key(&mut self, key: KeyEvent, mode: VimMode) -> VimCommand {
        match mode {
            VimMode::Normal => self.handle_normal_mode(key),
            VimMode::Insert => self.handle_insert_mode(key),
            VimMode::Visual => self.handle_visual_mode(key),
        }
    }

    /// Check if there's a pending operator.
    pub fn has_pending(&self) -> bool {
        self.pending != PendingOp::None
    }

    /// Clear pending state.
    pub fn clear_pending(&mut self) {
        self.pending = PendingOp::None;
    }

    /// Handle key events in Normal mode.
    fn handle_normal_mode(&mut self, key: KeyEvent) -> VimCommand {
        // Reset esc_pressed unless we're handling Esc
        if key.code != KeyCode::Esc {
            self.esc_pressed = false;
        }

        // If we have a pending operator, handle motion/text-object
        if self.pending != PendingOp::None {
            return self.handle_pending_operator(key);
        }

        match (key.code, key.modifiers) {
            // === Mode changes ===
            (KeyCode::Esc, KeyModifiers::NONE) => {
                // In normal mode, Esc can trigger cancel in some contexts
                if self.config.double_esc_to_exit {
                    if self.esc_pressed {
                        self.esc_pressed = false;
                        return VimCommand::custom("cancel");
                    }
                    self.esc_pressed = true;
                }
                VimCommand::None
            }

            // === Insert mode entry ===
            (KeyCode::Char('i'), KeyModifiers::NONE) => VimCommand::ChangeMode(VimMode::Insert),
            (KeyCode::Char('a'), KeyModifiers::NONE) => VimCommand::EnterInsertAt {
                motion: Some(Motion::right()),
                mode: VimMode::Insert,
            },
            (KeyCode::Char('I'), KeyModifiers::SHIFT) => VimCommand::EnterInsertAt {
                motion: Some(Motion::line_start()),
                mode: VimMode::Insert,
            },
            (KeyCode::Char('A'), KeyModifiers::SHIFT) => VimCommand::EnterInsertAt {
                motion: Some(Motion::line_end()),
                mode: VimMode::Insert,
            },
            (KeyCode::Char('o'), KeyModifiers::NONE) => VimCommand::OpenLine { above: false },
            (KeyCode::Char('O'), KeyModifiers::SHIFT) => VimCommand::OpenLine { above: true },

            // === Basic movement ===
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                VimCommand::Move(Motion::left())
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                VimCommand::Move(Motion::down())
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                VimCommand::Move(Motion::up())
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                VimCommand::Move(Motion::right())
            }

            // === Word movement ===
            (KeyCode::Char('w'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_forward()),
            (KeyCode::Char('b'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_back()),
            (KeyCode::Char('e'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_end()),

            // === Line movement ===
            (KeyCode::Char('0'), KeyModifiers::NONE) => VimCommand::Move(Motion::line_start()),
            (KeyCode::Char('$'), KeyModifiers::NONE) | (KeyCode::End, _) => {
                VimCommand::Move(Motion::line_end())
            }
            (KeyCode::Char('^'), KeyModifiers::NONE) | (KeyCode::Home, _) => {
                // ^ goes to first non-whitespace, but we'll use line start for simplicity
                VimCommand::Move(Motion::line_start())
            }

            // === Document movement ===
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.pending = PendingOp::G;
                VimCommand::None
            }
            (KeyCode::Char('G'), KeyModifiers::SHIFT) => VimCommand::Move(Motion::document_end()),

            // === Page movement ===
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                VimCommand::Move(Motion::Up(self.config.half_page_lines))
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                VimCommand::Move(Motion::Down(self.config.half_page_lines))
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) | (KeyCode::PageUp, _) => {
                VimCommand::Move(Motion::Up(self.config.full_page_lines))
            }
            (KeyCode::Char('f'), KeyModifiers::CONTROL) | (KeyCode::PageDown, _) => {
                VimCommand::Move(Motion::Down(self.config.full_page_lines))
            }

            // === Delete operators ===
            (KeyCode::Char('x'), KeyModifiers::NONE) => VimCommand::DeleteChar,
            (KeyCode::Char('X'), KeyModifiers::SHIFT) => VimCommand::DeleteCharBefore,
            (KeyCode::Char('D'), KeyModifiers::SHIFT) => VimCommand::DeleteToEnd,
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                self.pending = PendingOp::Delete;
                VimCommand::None
            }

            // === Change operators ===
            (KeyCode::Char('C'), KeyModifiers::SHIFT) => VimCommand::ChangeToEnd,
            (KeyCode::Char('S'), KeyModifiers::SHIFT) => VimCommand::ChangeLine,
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.pending = PendingOp::Change;
                VimCommand::None
            }

            // === Yank/paste ===
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                self.pending = PendingOp::Yank;
                VimCommand::None
            }
            (KeyCode::Char('Y'), KeyModifiers::SHIFT) => VimCommand::YankLine,
            (KeyCode::Char('p'), KeyModifiers::NONE) => VimCommand::PasteAfter,
            (KeyCode::Char('P'), KeyModifiers::SHIFT) => VimCommand::PasteBefore,

            // === Undo/redo ===
            (KeyCode::Char('u'), KeyModifiers::NONE) => VimCommand::Undo,
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => VimCommand::Redo,

            // === Visual mode ===
            (KeyCode::Char('v'), KeyModifiers::NONE) if self.config.enable_visual => {
                VimCommand::StartVisual
            }

            // === Search ===
            (KeyCode::Char('/'), KeyModifiers::NONE) if self.config.enable_search => {
                VimCommand::custom("search")
            }
            (KeyCode::Char('n'), KeyModifiers::NONE) if self.config.enable_search => {
                VimCommand::custom("search_next")
            }
            (KeyCode::Char('N'), KeyModifiers::SHIFT) if self.config.enable_search => {
                VimCommand::custom("search_prev")
            }

            // === Command mode ===
            (KeyCode::Char(':'), KeyModifiers::NONE) => VimCommand::custom("command"),

            // === Editor-specific ===
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => VimCommand::custom("save"),
            (KeyCode::Enter, KeyModifiers::NONE) => VimCommand::custom("enter"),

            _ => VimCommand::None,
        }
    }

    /// Handle key when there's a pending operator (d, c, y, g).
    fn handle_pending_operator(&mut self, key: KeyEvent) -> VimCommand {
        let op = self.pending;
        self.pending = PendingOp::None;

        match (op, key.code, key.modifiers) {
            // === gg - go to document start ===
            (PendingOp::G, KeyCode::Char('g'), KeyModifiers::NONE) => {
                VimCommand::Move(Motion::document_start())
            }

            // === dd - delete line ===
            (PendingOp::Delete, KeyCode::Char('d'), KeyModifiers::NONE) => VimCommand::DeleteLine,
            // === dw, de, db, d$, d0 ===
            (PendingOp::Delete, KeyCode::Char('w'), KeyModifiers::NONE) => {
                VimCommand::DeleteMotion(Motion::word_forward())
            }
            (PendingOp::Delete, KeyCode::Char('e'), KeyModifiers::NONE) => {
                VimCommand::DeleteMotion(Motion::word_end())
            }
            (PendingOp::Delete, KeyCode::Char('b'), KeyModifiers::NONE) => {
                VimCommand::DeleteMotion(Motion::word_back())
            }
            (PendingOp::Delete, KeyCode::Char('$'), KeyModifiers::NONE) => VimCommand::DeleteToEnd,
            (PendingOp::Delete, KeyCode::Char('0'), KeyModifiers::NONE) => {
                VimCommand::DeleteMotion(Motion::line_start())
            }

            // === cc - change line ===
            (PendingOp::Change, KeyCode::Char('c'), KeyModifiers::NONE) => VimCommand::ChangeLine,
            // === cw, ce, cb, c$, c0 ===
            (PendingOp::Change, KeyCode::Char('w'), KeyModifiers::NONE) => {
                VimCommand::ChangeMotion(Motion::word_forward())
            }
            (PendingOp::Change, KeyCode::Char('e'), KeyModifiers::NONE) => {
                VimCommand::ChangeMotion(Motion::word_end())
            }
            (PendingOp::Change, KeyCode::Char('b'), KeyModifiers::NONE) => {
                VimCommand::ChangeMotion(Motion::word_back())
            }
            (PendingOp::Change, KeyCode::Char('$'), KeyModifiers::NONE) => VimCommand::ChangeToEnd,
            (PendingOp::Change, KeyCode::Char('0'), KeyModifiers::NONE) => {
                VimCommand::ChangeMotion(Motion::line_start())
            }

            // === yy - yank line ===
            (PendingOp::Yank, KeyCode::Char('y'), KeyModifiers::NONE) => VimCommand::YankLine,
            // === yw, ye, yb, y$, y0 ===
            (PendingOp::Yank, KeyCode::Char('w'), KeyModifiers::NONE) => {
                VimCommand::YankMotion(Motion::word_forward())
            }
            (PendingOp::Yank, KeyCode::Char('e'), KeyModifiers::NONE) => {
                VimCommand::YankMotion(Motion::word_end())
            }
            (PendingOp::Yank, KeyCode::Char('b'), KeyModifiers::NONE) => {
                VimCommand::YankMotion(Motion::word_back())
            }
            (PendingOp::Yank, KeyCode::Char('$'), KeyModifiers::NONE) => {
                VimCommand::YankMotion(Motion::line_end())
            }
            (PendingOp::Yank, KeyCode::Char('0'), KeyModifiers::NONE) => {
                VimCommand::YankMotion(Motion::line_start())
            }

            // Cancel pending on Esc or unrecognized key
            _ => VimCommand::None,
        }
    }

    /// Handle key events in Insert mode.
    fn handle_insert_mode(&mut self, key: KeyEvent) -> VimCommand {
        match (key.code, key.modifiers) {
            // Exit insert mode
            (KeyCode::Esc, KeyModifiers::NONE) => VimCommand::ChangeMode(VimMode::Normal),

            // Page movement works in insert mode too
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                VimCommand::Move(Motion::Down(self.config.full_page_lines))
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                VimCommand::Move(Motion::Up(self.config.full_page_lines))
            }

            // Editor-specific shortcuts
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => VimCommand::custom("save"),
            (KeyCode::Enter, KeyModifiers::CONTROL) => VimCommand::custom("save"),

            // Everything else passes through to the textarea
            _ => VimCommand::PassThrough,
        }
    }

    /// Handle key events in Visual mode.
    fn handle_visual_mode(&mut self, key: KeyEvent) -> VimCommand {
        match (key.code, key.modifiers) {
            // Exit visual mode
            (KeyCode::Esc, KeyModifiers::NONE) => VimCommand::CancelVisual,
            (KeyCode::Char('v'), KeyModifiers::NONE) => VimCommand::CancelVisual,

            // Movement extends selection
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                VimCommand::Move(Motion::left())
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                VimCommand::Move(Motion::down())
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                VimCommand::Move(Motion::up())
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                VimCommand::Move(Motion::right())
            }

            // Word movement
            (KeyCode::Char('w'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_forward()),
            (KeyCode::Char('b'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_back()),
            (KeyCode::Char('e'), KeyModifiers::NONE) => VimCommand::Move(Motion::word_end()),

            // Line movement
            (KeyCode::Char('0'), KeyModifiers::NONE) => VimCommand::Move(Motion::line_start()),
            (KeyCode::Char('$'), KeyModifiers::NONE) => VimCommand::Move(Motion::line_end()),

            // Document movement
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                // In visual mode, 'g' alone goes to top (simplified)
                VimCommand::Move(Motion::document_start())
            }
            (KeyCode::Char('G'), KeyModifiers::SHIFT) => VimCommand::Move(Motion::document_end()),

            // Operations on selection
            (KeyCode::Char('y'), KeyModifiers::NONE) => VimCommand::VisualYank,
            (KeyCode::Char('d'), KeyModifiers::NONE) | (KeyCode::Char('x'), KeyModifiers::NONE) => {
                VimCommand::VisualDelete
            }
            (KeyCode::Char('c'), KeyModifiers::NONE) => VimCommand::VisualChange,

            _ => VimCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn test_basic_movement() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('h')), VimMode::Normal),
            VimCommand::Move(Motion::left())
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('j')), VimMode::Normal),
            VimCommand::Move(Motion::down())
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('k')), VimMode::Normal),
            VimCommand::Move(Motion::up())
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('l')), VimMode::Normal),
            VimCommand::Move(Motion::right())
        );
    }

    #[test]
    fn test_word_movement() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('w')), VimMode::Normal),
            VimCommand::Move(Motion::word_forward())
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('b')), VimMode::Normal),
            VimCommand::Move(Motion::word_back())
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('e')), VimMode::Normal),
            VimCommand::Move(Motion::word_end())
        );
    }

    #[test]
    fn test_insert_mode_entry() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('i')), VimMode::Normal),
            VimCommand::ChangeMode(VimMode::Insert)
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('a')), VimMode::Normal),
            VimCommand::EnterInsertAt {
                motion: Some(Motion::right()),
                mode: VimMode::Insert
            }
        );
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('o')), VimMode::Normal),
            VimCommand::OpenLine { above: false }
        );
        assert_eq!(
            handler.handle_key(key_shift('O'), VimMode::Normal),
            VimCommand::OpenLine { above: true }
        );
    }

    #[test]
    fn test_delete_line() {
        let mut handler = VimHandler::default_config();

        // First 'd' sets pending
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('d')), VimMode::Normal),
            VimCommand::None
        );
        assert!(handler.has_pending());

        // Second 'd' completes dd
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('d')), VimMode::Normal),
            VimCommand::DeleteLine
        );
        assert!(!handler.has_pending());
    }

    #[test]
    fn test_delete_word() {
        let mut handler = VimHandler::default_config();

        handler.handle_key(key(KeyCode::Char('d')), VimMode::Normal);
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('w')), VimMode::Normal),
            VimCommand::DeleteMotion(Motion::word_forward())
        );
    }

    #[test]
    fn test_change_line() {
        let mut handler = VimHandler::default_config();

        handler.handle_key(key(KeyCode::Char('c')), VimMode::Normal);
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('c')), VimMode::Normal),
            VimCommand::ChangeLine
        );
    }

    #[test]
    fn test_yank_line() {
        let mut handler = VimHandler::default_config();

        handler.handle_key(key(KeyCode::Char('y')), VimMode::Normal);
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('y')), VimMode::Normal),
            VimCommand::YankLine
        );
    }

    #[test]
    fn test_gg_goto_top() {
        let mut handler = VimHandler::default_config();

        handler.handle_key(key(KeyCode::Char('g')), VimMode::Normal);
        assert_eq!(
            handler.handle_key(key(KeyCode::Char('g')), VimMode::Normal),
            VimCommand::Move(Motion::document_start())
        );
    }

    #[test]
    fn test_page_movement() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key_ctrl('d'), VimMode::Normal),
            VimCommand::Move(Motion::Down(10))
        );
        assert_eq!(
            handler.handle_key(key_ctrl('u'), VimMode::Normal),
            VimCommand::Move(Motion::Up(10))
        );
        assert_eq!(
            handler.handle_key(key_ctrl('f'), VimMode::Normal),
            VimCommand::Move(Motion::Down(20))
        );
        assert_eq!(
            handler.handle_key(key_ctrl('b'), VimMode::Normal),
            VimCommand::Move(Motion::Up(20))
        );
    }

    #[test]
    fn test_insert_mode_esc() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Esc), VimMode::Insert),
            VimCommand::ChangeMode(VimMode::Normal)
        );
    }

    #[test]
    fn test_insert_mode_passthrough() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('a')), VimMode::Insert),
            VimCommand::PassThrough
        );
    }

    #[test]
    fn test_visual_mode_yank() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('y')), VimMode::Visual),
            VimCommand::VisualYank
        );
    }

    #[test]
    fn test_visual_mode_delete() {
        let mut handler = VimHandler::default_config();

        assert_eq!(
            handler.handle_key(key(KeyCode::Char('d')), VimMode::Visual),
            VimCommand::VisualDelete
        );
    }

    #[test]
    fn test_double_esc_config() {
        // Create a config with double_esc_to_exit enabled
        let config = VimConfig {
            double_esc_to_exit: true,
            ..VimConfig::default()
        };
        let mut handler = VimHandler::new(config);

        // First Esc does nothing (just sets flag)
        assert_eq!(
            handler.handle_key(key(KeyCode::Esc), VimMode::Normal),
            VimCommand::None
        );

        // Second Esc triggers cancel
        assert_eq!(
            handler.handle_key(key(KeyCode::Esc), VimMode::Normal),
            VimCommand::custom("cancel")
        );
    }

    #[test]
    fn test_single_esc_json_editor_config() {
        // json_editor config now has double_esc_to_exit: false
        let config = VimConfig::json_editor();
        let mut handler = VimHandler::new(config);

        // Single Esc in Normal mode returns None (handled by json_editor itself)
        assert_eq!(
            handler.handle_key(key(KeyCode::Esc), VimMode::Normal),
            VimCommand::None
        );
    }
}
