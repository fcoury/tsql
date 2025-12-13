//! Configuration schema definitions.

use serde::{Deserialize, Serialize};

/// Root configuration structure
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    /// Display settings
    pub display: DisplayConfig,
    /// Editor settings
    pub editor: EditorConfig,
    /// Connection settings
    pub connection: ConnectionConfig,
    /// Keymap customizations
    pub keymap: KeymapConfig,
}

/// Display-related settings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Show row numbers in the grid
    pub show_row_numbers: bool,
    /// Default column width (characters)
    pub default_column_width: u16,
    /// Minimum column width
    pub min_column_width: u16,
    /// Maximum column width
    pub max_column_width: u16,
    /// Truncate long cell values with ellipsis
    pub truncate_cells: bool,
    /// Show NULL values as a distinct indicator
    pub show_null_indicator: bool,
    /// NULL indicator text
    pub null_indicator: String,
    /// Show borders around cells
    pub show_borders: bool,
    /// Theme name (for future theme support)
    pub theme: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            show_row_numbers: true,
            default_column_width: 20,
            min_column_width: 4,
            max_column_width: 100,
            truncate_cells: true,
            show_null_indicator: true,
            null_indicator: "NULL".to_string(),
            show_borders: true,
            theme: "default".to_string(),
        }
    }
}

/// Editor-related settings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Tab size in spaces
    pub tab_size: u8,
    /// Use spaces instead of tabs
    pub expand_tabs: bool,
    /// Enable auto-indent
    pub auto_indent: bool,
    /// Enable line numbers in the query editor
    pub line_numbers: bool,
    /// Enable syntax highlighting
    pub syntax_highlighting: bool,
    /// Enable auto-completion
    pub auto_completion: bool,
    /// Completion trigger delay in milliseconds
    pub completion_delay_ms: u32,
    /// Maximum history entries to keep
    pub max_history: usize,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_size: 4,
            expand_tabs: true,
            auto_indent: true,
            line_numbers: true,
            syntax_highlighting: true,
            auto_completion: true,
            completion_delay_ms: 100,
            max_history: 1000,
        }
    }
}

/// Connection-related settings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    /// Default database URL (can be overridden by DATABASE_URL env var)
    pub default_url: Option<String>,
    /// Connection timeout in seconds
    pub connect_timeout_secs: u32,
    /// Query timeout in seconds (0 = no timeout)
    pub query_timeout_secs: u32,
    /// Maximum rows to fetch (0 = no limit)
    pub max_rows: usize,
    /// Auto-reconnect on connection loss
    pub auto_reconnect: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            default_url: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 0,
            max_rows: 0,
            auto_reconnect: true,
        }
    }
}

/// Keymap customization settings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
    /// Use vim-style keybindings
    pub vim_mode: bool,
    /// Custom keybindings for normal mode
    #[serde(default)]
    pub normal: Vec<CustomKeyBinding>,
    /// Custom keybindings for insert mode
    #[serde(default)]
    pub insert: Vec<CustomKeyBinding>,
    /// Custom keybindings for visual/select mode
    #[serde(default)]
    pub visual: Vec<CustomKeyBinding>,
    /// Custom keybindings for grid navigation
    #[serde(default)]
    pub grid: Vec<CustomKeyBinding>,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            vim_mode: true,
            normal: Vec::new(),
            insert: Vec::new(),
            visual: Vec::new(),
            grid: Vec::new(),
        }
    }
}

/// A custom keybinding definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomKeyBinding {
    /// Key combination (e.g., "ctrl+s", "g g", "leader f")
    pub key: String,
    /// Action to perform
    pub action: String,
    /// Optional description for help display
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_config_parse() {
        let toml = r#"
[display]
show_row_numbers = false
default_column_width = 30
null_indicator = "<null>"
theme = "dracula"

[editor]
tab_size = 2
expand_tabs = false
max_history = 500

[connection]
default_url = "postgres://localhost/mydb"
connect_timeout_secs = 5
max_rows = 10000

[keymap]
vim_mode = true

[[keymap.normal]]
key = "ctrl+s"
action = "save_query"
description = "Save the current query"

[[keymap.grid]]
key = "ctrl+e"
action = "export_csv"
description = "Export results as CSV"
"#;

        let config: Config = toml::from_str(toml).unwrap();

        // Display
        assert!(!config.display.show_row_numbers);
        assert_eq!(config.display.default_column_width, 30);
        assert_eq!(config.display.null_indicator, "<null>");
        assert_eq!(config.display.theme, "dracula");

        // Editor
        assert_eq!(config.editor.tab_size, 2);
        assert!(!config.editor.expand_tabs);
        assert_eq!(config.editor.max_history, 500);

        // Connection
        assert_eq!(
            config.connection.default_url,
            Some("postgres://localhost/mydb".to_string())
        );
        assert_eq!(config.connection.connect_timeout_secs, 5);
        assert_eq!(config.connection.max_rows, 10000);

        // Keymap
        assert!(config.keymap.vim_mode);
        assert_eq!(config.keymap.normal.len(), 1);
        assert_eq!(config.keymap.normal[0].key, "ctrl+s");
        assert_eq!(config.keymap.normal[0].action, "save_query");

        assert_eq!(config.keymap.grid.len(), 1);
        assert_eq!(config.keymap.grid[0].key, "ctrl+e");
    }

    #[test]
    fn test_serialize_config() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("[display]"));
        assert!(toml_str.contains("[editor]"));
        assert!(toml_str.contains("[connection]"));
    }
}
