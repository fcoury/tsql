//! Vim editing modes.

/// The current vim editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Normal mode - navigation and commands.
    #[default]
    Normal,
    /// Insert mode - text input.
    Insert,
    /// Visual mode - text selection.
    Visual,
}

impl VimMode {
    /// Returns true if in insert mode.
    pub fn is_insert(&self) -> bool {
        matches!(self, VimMode::Insert)
    }

    /// Returns true if in normal mode.
    pub fn is_normal(&self) -> bool {
        matches!(self, VimMode::Normal)
    }

    /// Returns true if in visual mode.
    pub fn is_visual(&self) -> bool {
        matches!(self, VimMode::Visual)
    }

    /// Returns the mode name for display.
    pub fn label(&self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vim_mode_default() {
        assert_eq!(VimMode::default(), VimMode::Normal);
    }

    #[test]
    fn test_vim_mode_predicates() {
        assert!(VimMode::Normal.is_normal());
        assert!(!VimMode::Normal.is_insert());
        assert!(!VimMode::Normal.is_visual());

        assert!(VimMode::Insert.is_insert());
        assert!(!VimMode::Insert.is_normal());

        assert!(VimMode::Visual.is_visual());
        assert!(!VimMode::Visual.is_normal());
    }

    #[test]
    fn test_vim_mode_labels() {
        assert_eq!(VimMode::Normal.label(), "NORMAL");
        assert_eq!(VimMode::Insert.label(), "INSERT");
        assert_eq!(VimMode::Visual.label(), "VISUAL");
    }
}
