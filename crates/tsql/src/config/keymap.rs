//! Keymap and action definitions for tsql.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

/// All possible actions that can be triggered by keybindings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // Navigation
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveToTop,
    MoveToBottom,
    MoveToStart,
    MoveToEnd,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,

    // Mode switching
    EnterInsertMode,
    EnterNormalMode,
    EnterVisualMode,
    EnterCommandMode,

    // Focus
    FocusQuery,
    FocusGrid,
    ToggleFocus,

    // Editor actions
    DeleteChar,
    DeleteWord,
    DeleteLine,
    Undo,
    Redo,
    Copy,
    Paste,
    Cut,
    SelectAll,

    // Query execution
    ExecuteQuery,
    CancelQuery,

    // Grid actions
    SelectRow,
    GridSelectAll,
    ClearSelection,
    CopySelection,
    CopyCsv,
    CopyJson,
    CopyTsv,
    ExportCsv,
    ExportJson,
    EditCell,
    GenerateUpdate,
    GenerateDelete,
    GenerateInsert,

    // Search
    StartSearch,
    NextMatch,
    PrevMatch,
    ClearSearch,

    // Column operations
    ResizeColumnLeft,
    ResizeColumnRight,
    AutoFitColumn,

    // Application
    Quit,
    ForceQuit,
    Help,
    ShowHistory,
    Refresh,

    // Connection
    Connect,
    Disconnect,
    Reconnect,
}

impl Action {
    /// Get the default description for this action
    pub fn description(&self) -> &'static str {
        match self {
            Action::MoveUp => "Move cursor up",
            Action::MoveDown => "Move cursor down",
            Action::MoveLeft => "Move cursor left",
            Action::MoveRight => "Move cursor right",
            Action::MoveToTop => "Move to top",
            Action::MoveToBottom => "Move to bottom",
            Action::MoveToStart => "Move to start of line",
            Action::MoveToEnd => "Move to end of line",
            Action::PageUp => "Page up",
            Action::PageDown => "Page down",
            Action::HalfPageUp => "Half page up",
            Action::HalfPageDown => "Half page down",
            Action::EnterInsertMode => "Enter insert mode",
            Action::EnterNormalMode => "Enter normal mode",
            Action::EnterVisualMode => "Enter visual/select mode",
            Action::EnterCommandMode => "Enter command mode",
            Action::FocusQuery => "Focus query editor",
            Action::FocusGrid => "Focus results grid",
            Action::ToggleFocus => "Toggle focus between panes",
            Action::DeleteChar => "Delete character",
            Action::DeleteWord => "Delete word",
            Action::DeleteLine => "Delete line",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Copy => "Copy",
            Action::Paste => "Paste",
            Action::Cut => "Cut",
            Action::SelectAll => "Select all",
            Action::ExecuteQuery => "Execute query",
            Action::CancelQuery => "Cancel running query",
            Action::SelectRow => "Select/toggle row",
            Action::GridSelectAll => "Select all rows",
            Action::ClearSelection => "Clear selection",
            Action::CopySelection => "Copy selection",
            Action::CopyCsv => "Copy as CSV",
            Action::CopyJson => "Copy as JSON",
            Action::CopyTsv => "Copy as TSV",
            Action::ExportCsv => "Export to CSV file",
            Action::ExportJson => "Export to JSON file",
            Action::EditCell => "Edit cell",
            Action::GenerateUpdate => "Generate UPDATE statement",
            Action::GenerateDelete => "Generate DELETE statement",
            Action::GenerateInsert => "Generate INSERT statement",
            Action::StartSearch => "Start search",
            Action::NextMatch => "Next match",
            Action::PrevMatch => "Previous match",
            Action::ClearSearch => "Clear search",
            Action::ResizeColumnLeft => "Make column narrower",
            Action::ResizeColumnRight => "Make column wider",
            Action::AutoFitColumn => "Auto-fit column width",
            Action::Quit => "Quit",
            Action::ForceQuit => "Force quit without saving",
            Action::Help => "Show help",
            Action::ShowHistory => "Show query history",
            Action::Refresh => "Refresh/re-run query",
            Action::Connect => "Connect to database",
            Action::Disconnect => "Disconnect from database",
            Action::Reconnect => "Reconnect to database",
        }
    }
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Convert to snake_case for matching
        let normalized = s.trim().to_lowercase().replace('-', "_");

        match normalized.as_str() {
            // Navigation
            "move_up" => Ok(Action::MoveUp),
            "move_down" => Ok(Action::MoveDown),
            "move_left" => Ok(Action::MoveLeft),
            "move_right" => Ok(Action::MoveRight),
            "move_to_top" => Ok(Action::MoveToTop),
            "move_to_bottom" => Ok(Action::MoveToBottom),
            "move_to_start" => Ok(Action::MoveToStart),
            "move_to_end" => Ok(Action::MoveToEnd),
            "page_up" => Ok(Action::PageUp),
            "page_down" => Ok(Action::PageDown),
            "half_page_up" => Ok(Action::HalfPageUp),
            "half_page_down" => Ok(Action::HalfPageDown),

            // Mode switching
            "enter_insert_mode" => Ok(Action::EnterInsertMode),
            "enter_normal_mode" => Ok(Action::EnterNormalMode),
            "enter_visual_mode" => Ok(Action::EnterVisualMode),
            "enter_command_mode" => Ok(Action::EnterCommandMode),

            // Focus
            "focus_query" => Ok(Action::FocusQuery),
            "focus_grid" => Ok(Action::FocusGrid),
            "toggle_focus" => Ok(Action::ToggleFocus),

            // Editor actions
            "delete_char" => Ok(Action::DeleteChar),
            "delete_word" => Ok(Action::DeleteWord),
            "delete_line" => Ok(Action::DeleteLine),
            "undo" => Ok(Action::Undo),
            "redo" => Ok(Action::Redo),
            "copy" => Ok(Action::Copy),
            "paste" => Ok(Action::Paste),
            "cut" => Ok(Action::Cut),
            "select_all" => Ok(Action::SelectAll),

            // Query execution
            "execute_query" => Ok(Action::ExecuteQuery),
            "cancel_query" => Ok(Action::CancelQuery),

            // Grid actions
            "select_row" => Ok(Action::SelectRow),
            "grid_select_all" => Ok(Action::GridSelectAll),
            "clear_selection" => Ok(Action::ClearSelection),
            "copy_selection" => Ok(Action::CopySelection),
            "copy_csv" => Ok(Action::CopyCsv),
            "copy_json" => Ok(Action::CopyJson),
            "copy_tsv" => Ok(Action::CopyTsv),
            "export_csv" => Ok(Action::ExportCsv),
            "export_json" => Ok(Action::ExportJson),
            "edit_cell" => Ok(Action::EditCell),
            "generate_update" => Ok(Action::GenerateUpdate),
            "generate_delete" => Ok(Action::GenerateDelete),
            "generate_insert" => Ok(Action::GenerateInsert),

            // Search
            "start_search" => Ok(Action::StartSearch),
            "next_match" => Ok(Action::NextMatch),
            "prev_match" => Ok(Action::PrevMatch),
            "clear_search" => Ok(Action::ClearSearch),

            // Column operations
            "resize_column_left" => Ok(Action::ResizeColumnLeft),
            "resize_column_right" => Ok(Action::ResizeColumnRight),
            "auto_fit_column" => Ok(Action::AutoFitColumn),

            // Application
            "quit" => Ok(Action::Quit),
            "force_quit" => Ok(Action::ForceQuit),
            "help" => Ok(Action::Help),
            "show_history" => Ok(Action::ShowHistory),
            "refresh" => Ok(Action::Refresh),

            // Connection
            "connect" => Ok(Action::Connect),
            "disconnect" => Ok(Action::Disconnect),
            "reconnect" => Ok(Action::Reconnect),

            _ => Err(format!("Unknown action: {}", s)),
        }
    }
}

/// Represents a key binding (key + modifiers)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Parse a key binding from a string like "ctrl+s", "g", "shift+tab"
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        let parts: Vec<&str> = s.split('+').collect();

        let mut modifiers = KeyModifiers::NONE;
        let key_part = if parts.len() == 1 {
            parts[0]
        } else {
            // Parse modifiers
            for part in &parts[..parts.len() - 1] {
                match *part {
                    "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                    "alt" | "meta" => modifiers |= KeyModifiers::ALT,
                    "shift" => modifiers |= KeyModifiers::SHIFT,
                    _ => return None, // Unknown modifier
                }
            }
            parts[parts.len() - 1]
        };

        // Parse the key code
        let code = match key_part {
            // Special keys
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "insert" | "ins" => KeyCode::Insert,
            // Function keys
            "f1" => KeyCode::F(1),
            "f2" => KeyCode::F(2),
            "f3" => KeyCode::F(3),
            "f4" => KeyCode::F(4),
            "f5" => KeyCode::F(5),
            "f6" => KeyCode::F(6),
            "f7" => KeyCode::F(7),
            "f8" => KeyCode::F(8),
            "f9" => KeyCode::F(9),
            "f10" => KeyCode::F(10),
            "f11" => KeyCode::F(11),
            "f12" => KeyCode::F(12),
            // Single character
            s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
            _ => return None,
        };

        Some(Self { code, modifiers })
    }
}

impl std::fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<&str> = Vec::new();

        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift");
        }

        let key = match self.code {
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::F(n) => format!("F{}", n),
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_uppercase().to_string(),
            _ => "?".to_string(),
        };

        // Build the final string
        let mut result_parts: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
        result_parts.push(key);
        write!(f, "{}", result_parts.join("+"))
    }
}

impl From<KeyEvent> for KeyBinding {
    fn from(event: KeyEvent) -> Self {
        Self {
            code: event.code,
            modifiers: event.modifiers,
        }
    }
}

/// A keymap is a collection of key bindings mapped to actions
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<KeyBinding, Action>,
}

impl Keymap {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Add a binding
    pub fn bind(&mut self, key: KeyBinding, action: Action) {
        self.bindings.insert(key, action);
    }

    /// Remove a binding
    pub fn unbind(&mut self, key: &KeyBinding) {
        self.bindings.remove(key);
    }

    /// Look up an action for a key
    pub fn get(&self, key: &KeyBinding) -> Option<&Action> {
        self.bindings.get(key)
    }

    /// Look up an action for a KeyEvent
    pub fn get_action(&self, event: &KeyEvent) -> Option<Action> {
        let binding = KeyBinding::from(*event);
        self.bindings.get(&binding).copied()
    }

    /// Get all bindings
    pub fn bindings(&self) -> &HashMap<KeyBinding, Action> {
        &self.bindings
    }

    /// Create the default keymap for grid navigation
    pub fn default_grid_keymap() -> Self {
        let mut km = Self::new();

        // Vim-style navigation
        km.bind(
            KeyBinding::new(KeyCode::Char('h'), KeyModifiers::NONE),
            Action::MoveLeft,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE),
            Action::MoveDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('k'), KeyModifiers::NONE),
            Action::MoveUp,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('l'), KeyModifiers::NONE),
            Action::MoveRight,
        );

        // Arrow keys
        km.bind(
            KeyBinding::new(KeyCode::Left, KeyModifiers::NONE),
            Action::MoveLeft,
        );
        km.bind(
            KeyBinding::new(KeyCode::Down, KeyModifiers::NONE),
            Action::MoveDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::Up, KeyModifiers::NONE),
            Action::MoveUp,
        );
        km.bind(
            KeyBinding::new(KeyCode::Right, KeyModifiers::NONE),
            Action::MoveRight,
        );

        // Page navigation
        km.bind(
            KeyBinding::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Action::HalfPageDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Action::HalfPageUp,
        );
        km.bind(
            KeyBinding::new(KeyCode::PageDown, KeyModifiers::NONE),
            Action::PageDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::PageUp, KeyModifiers::NONE),
            Action::PageUp,
        );

        // Jump to start/end
        km.bind(
            KeyBinding::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            Action::MoveToBottom,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('0'), KeyModifiers::NONE),
            Action::MoveToStart,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('$'), KeyModifiers::SHIFT),
            Action::MoveToEnd,
        );
        km.bind(
            KeyBinding::new(KeyCode::Home, KeyModifiers::NONE),
            Action::MoveToStart,
        );
        km.bind(
            KeyBinding::new(KeyCode::End, KeyModifiers::NONE),
            Action::MoveToEnd,
        );

        // Selection
        km.bind(
            KeyBinding::new(KeyCode::Char(' '), KeyModifiers::NONE),
            Action::SelectRow,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('v'), KeyModifiers::NONE),
            Action::EnterVisualMode,
        );
        km.bind(
            KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE),
            Action::ClearSelection,
        );

        // Copy
        km.bind(
            KeyBinding::new(KeyCode::Char('y'), KeyModifiers::NONE),
            Action::CopySelection,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::CopySelection,
        );

        // Edit
        km.bind(
            KeyBinding::new(KeyCode::Char('e'), KeyModifiers::NONE),
            Action::EditCell,
        );
        km.bind(
            KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE),
            Action::EditCell,
        );

        // Search
        km.bind(
            KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE),
            Action::StartSearch,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('n'), KeyModifiers::NONE),
            Action::NextMatch,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
            Action::PrevMatch,
        );

        // Column resize
        km.bind(
            KeyBinding::new(KeyCode::Char('<'), KeyModifiers::SHIFT),
            Action::ResizeColumnLeft,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('>'), KeyModifiers::SHIFT),
            Action::ResizeColumnRight,
        );

        // Focus
        km.bind(
            KeyBinding::new(KeyCode::Tab, KeyModifiers::NONE),
            Action::ToggleFocus,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('i'), KeyModifiers::NONE),
            Action::FocusQuery,
        );

        // Commands
        km.bind(
            KeyBinding::new(KeyCode::Char(':'), KeyModifiers::NONE),
            Action::EnterCommandMode,
        );

        km
    }

    /// Create the default keymap for the query editor in normal mode
    pub fn default_editor_normal_keymap() -> Self {
        let mut km = Self::new();

        // Mode switching
        km.bind(
            KeyBinding::new(KeyCode::Char('i'), KeyModifiers::NONE),
            Action::EnterInsertMode,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('a'), KeyModifiers::NONE),
            Action::EnterInsertMode,
        ); // append
        km.bind(
            KeyBinding::new(KeyCode::Char('v'), KeyModifiers::NONE),
            Action::EnterVisualMode,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char(':'), KeyModifiers::NONE),
            Action::EnterCommandMode,
        );

        // Navigation
        km.bind(
            KeyBinding::new(KeyCode::Char('h'), KeyModifiers::NONE),
            Action::MoveLeft,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE),
            Action::MoveDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('k'), KeyModifiers::NONE),
            Action::MoveUp,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('l'), KeyModifiers::NONE),
            Action::MoveRight,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('0'), KeyModifiers::NONE),
            Action::MoveToStart,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('$'), KeyModifiers::SHIFT),
            Action::MoveToEnd,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            Action::MoveToBottom,
        );

        // Arrow keys (always work)
        km.bind(
            KeyBinding::new(KeyCode::Left, KeyModifiers::NONE),
            Action::MoveLeft,
        );
        km.bind(
            KeyBinding::new(KeyCode::Down, KeyModifiers::NONE),
            Action::MoveDown,
        );
        km.bind(
            KeyBinding::new(KeyCode::Up, KeyModifiers::NONE),
            Action::MoveUp,
        );
        km.bind(
            KeyBinding::new(KeyCode::Right, KeyModifiers::NONE),
            Action::MoveRight,
        );

        // Copy/paste
        km.bind(
            KeyBinding::new(KeyCode::Char('y'), KeyModifiers::NONE),
            Action::Copy,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('p'), KeyModifiers::NONE),
            Action::Paste,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('d'), KeyModifiers::NONE),
            Action::DeleteLine,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('x'), KeyModifiers::NONE),
            Action::DeleteChar,
        );

        // Undo/redo
        km.bind(
            KeyBinding::new(KeyCode::Char('u'), KeyModifiers::NONE),
            Action::Undo,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Action::Redo,
        );

        // Search
        km.bind(
            KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE),
            Action::StartSearch,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('n'), KeyModifiers::NONE),
            Action::NextMatch,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
            Action::PrevMatch,
        );

        // Execute query
        km.bind(
            KeyBinding::new(KeyCode::Enter, KeyModifiers::CONTROL),
            Action::ExecuteQuery,
        );

        // Focus grid
        km.bind(
            KeyBinding::new(KeyCode::Tab, KeyModifiers::NONE),
            Action::ToggleFocus,
        );

        // Show history picker
        km.bind(
            KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Action::ShowHistory,
        );

        km
    }

    /// Create the default keymap for the query editor in insert mode
    pub fn default_editor_insert_keymap() -> Self {
        let mut km = Self::new();

        // Exit insert mode
        km.bind(
            KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE),
            Action::EnterNormalMode,
        );

        // Execute query
        km.bind(
            KeyBinding::new(KeyCode::Enter, KeyModifiers::CONTROL),
            Action::ExecuteQuery,
        );

        // Standard editing shortcuts
        km.bind(
            KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Copy,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            Action::Paste,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            Action::Cut,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
            Action::Undo,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            Action::Redo,
        );
        km.bind(
            KeyBinding::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            Action::SelectAll,
        );

        // Show history picker
        km.bind(
            KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Action::ShowHistory,
        );

        km
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_key() {
        let kb = KeyBinding::parse("a").unwrap();
        assert_eq!(kb.code, KeyCode::Char('a'));
        assert_eq!(kb.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn test_parse_ctrl_key() {
        let kb = KeyBinding::parse("ctrl+s").unwrap();
        assert_eq!(kb.code, KeyCode::Char('s'));
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_parse_multiple_modifiers() {
        let kb = KeyBinding::parse("ctrl+shift+a").unwrap();
        assert_eq!(kb.code, KeyCode::Char('a'));
        assert!(kb.modifiers.contains(KeyModifiers::CONTROL));
        assert!(kb.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn test_parse_special_keys() {
        assert_eq!(KeyBinding::parse("enter").unwrap().code, KeyCode::Enter);
        assert_eq!(KeyBinding::parse("tab").unwrap().code, KeyCode::Tab);
        assert_eq!(KeyBinding::parse("esc").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyBinding::parse("space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(KeyBinding::parse("f1").unwrap().code, KeyCode::F(1));
        assert_eq!(KeyBinding::parse("pgup").unwrap().code, KeyCode::PageUp);
    }

    #[test]
    fn test_parse_case_insensitive() {
        let kb1 = KeyBinding::parse("CTRL+A").unwrap();
        let kb2 = KeyBinding::parse("ctrl+a").unwrap();
        assert_eq!(kb1.code, kb2.code);
        assert_eq!(kb1.modifiers, kb2.modifiers);
    }

    #[test]
    fn test_key_binding_display() {
        let kb = KeyBinding::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(kb.to_string(), "Ctrl+S");

        let kb = KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(kb.to_string(), "Enter");
    }

    #[test]
    fn test_keymap_bind_and_get() {
        let mut km = Keymap::new();
        let key = KeyBinding::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        km.bind(key, Action::ExecuteQuery);

        assert_eq!(km.get(&key), Some(&Action::ExecuteQuery));
    }

    #[test]
    fn test_default_grid_keymap() {
        let km = Keymap::default_grid_keymap();

        // Check vim navigation
        let h = KeyBinding::new(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(km.get(&h), Some(&Action::MoveLeft));

        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(km.get(&j), Some(&Action::MoveDown));

        // Check search
        let slash = KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(km.get(&slash), Some(&Action::StartSearch));
    }

    #[test]
    fn test_action_from_str() {
        assert_eq!("move_up".parse::<Action>().unwrap(), Action::MoveUp);
        assert_eq!("move_down".parse::<Action>().unwrap(), Action::MoveDown);
        assert_eq!(
            "execute_query".parse::<Action>().unwrap(),
            Action::ExecuteQuery
        );
        assert_eq!(
            "enter_insert_mode".parse::<Action>().unwrap(),
            Action::EnterInsertMode
        );
        assert_eq!(
            "copy_selection".parse::<Action>().unwrap(),
            Action::CopySelection
        );

        // Test with dashes (should normalize to underscores)
        assert_eq!("move-up".parse::<Action>().unwrap(), Action::MoveUp);

        // Test case insensitivity
        assert_eq!("MOVE_UP".parse::<Action>().unwrap(), Action::MoveUp);
        assert_eq!("Move_Up".parse::<Action>().unwrap(), Action::MoveUp);
    }

    #[test]
    fn test_action_from_str_invalid() {
        assert!("invalid_action".parse::<Action>().is_err());
        assert!("".parse::<Action>().is_err());
        assert!("not_an_action".parse::<Action>().is_err());
    }

    #[test]
    fn test_keymap_override() {
        let mut km = Keymap::default_grid_keymap();

        // Verify default binding
        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(km.get(&j), Some(&Action::MoveDown));

        // Override j to do something else
        km.bind(j, Action::PageDown);
        assert_eq!(km.get(&j), Some(&Action::PageDown));
    }

    #[test]
    fn test_keymap_custom_binding() {
        let mut km = Keymap::new();

        // Add a custom binding
        let ctrl_enter = KeyBinding::new(KeyCode::Enter, KeyModifiers::CONTROL);
        km.bind(ctrl_enter, Action::ExecuteQuery);

        assert_eq!(km.get(&ctrl_enter), Some(&Action::ExecuteQuery));
    }

    #[test]
    fn test_keymap_get_action_from_key_event() {
        let mut km = Keymap::new();
        let binding = KeyBinding::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        km.bind(binding, Action::ExecuteQuery);

        // Create a KeyEvent that matches
        let key_event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(km.get_action(&key_event), Some(Action::ExecuteQuery));

        // Create a KeyEvent that doesn't match
        let key_event2 = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(km.get_action(&key_event2), None);
    }

    #[test]
    fn test_keymap_unbind() {
        let mut km = Keymap::default_grid_keymap();

        let j = KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(km.get(&j).is_some());

        km.unbind(&j);
        assert!(km.get(&j).is_none());
    }
}
