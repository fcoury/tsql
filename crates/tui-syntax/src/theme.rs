//! Theme definitions and TOML parsing.
//!
//! Themes define how syntax elements are styled. The format is compatible with
//! [Helix editor themes](https://docs.helix-editor.com/themes.html).

use std::collections::HashMap;
use std::path::Path;

use ratatui::style::{Color, Modifier, Style as RatatuiStyle};
use serde::Deserialize;

/// Error loading or parsing a theme.
#[derive(Debug)]
pub enum ThemeError {
    /// IO error reading theme file
    Io(std::io::Error),
    /// TOML parsing error
    Parse(toml::de::Error),
    /// Invalid color format
    InvalidColor(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Io(e) => write!(f, "IO error: {}", e),
            ThemeError::Parse(e) => write!(f, "Parse error: {}", e),
            ThemeError::InvalidColor(c) => write!(f, "Invalid color: {}", c),
        }
    }
}

impl std::error::Error for ThemeError {}

impl From<std::io::Error> for ThemeError {
    fn from(e: std::io::Error) -> Self {
        ThemeError::Io(e)
    }
}

impl From<toml::de::Error> for ThemeError {
    fn from(e: toml::de::Error) -> Self {
        ThemeError::Parse(e)
    }
}

/// Style modifiers (bold, italic, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleModifier {
    Bold,
    Dim,
    Italic,
    Underlined,
    SlowBlink,
    RapidBlink,
    Reversed,
    Hidden,
    CrossedOut,
}

impl StyleModifier {
    fn to_ratatui_modifier(self) -> Modifier {
        match self {
            StyleModifier::Bold => Modifier::BOLD,
            StyleModifier::Dim => Modifier::DIM,
            StyleModifier::Italic => Modifier::ITALIC,
            StyleModifier::Underlined => Modifier::UNDERLINED,
            StyleModifier::SlowBlink => Modifier::SLOW_BLINK,
            StyleModifier::RapidBlink => Modifier::RAPID_BLINK,
            StyleModifier::Reversed => Modifier::REVERSED,
            StyleModifier::Hidden => Modifier::HIDDEN,
            StyleModifier::CrossedOut => Modifier::CROSSED_OUT,
        }
    }
}

/// A style definition for a syntax element.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Style {
    /// Foreground color (name from palette or hex)
    pub fg: Option<String>,
    /// Background color (name from palette or hex)
    pub bg: Option<String>,
    /// Style modifiers
    #[serde(default)]
    pub modifiers: Vec<StyleModifier>,
}

impl Style {
    /// Convert to ratatui Style using the given color palette.
    pub fn to_ratatui_style(&self, palette: &HashMap<String, String>) -> RatatuiStyle {
        let mut style = RatatuiStyle::default();

        if let Some(ref fg) = self.fg {
            if let Some(color) = resolve_color(fg, palette) {
                style = style.fg(color);
            }
        }

        if let Some(ref bg) = self.bg {
            if let Some(color) = resolve_color(bg, palette) {
                style = style.bg(color);
            }
        }

        for modifier in &self.modifiers {
            style = style.add_modifier(modifier.to_ratatui_modifier());
        }

        style
    }
}

/// Resolve a color string to a ratatui Color.
///
/// The color can be:
/// - A palette name (looked up in the palette)
/// - A hex color (#RRGGBB or #RGB)
/// - A named color (red, green, blue, etc.)
fn resolve_color(color: &str, palette: &HashMap<String, String>) -> Option<Color> {
    // First check if it's a palette reference
    if let Some(resolved) = palette.get(color) {
        return parse_color(resolved);
    }

    // Otherwise try to parse directly
    parse_color(color)
}

/// Parse a color string to a ratatui Color.
fn parse_color(color: &str) -> Option<Color> {
    let color = color.trim();

    // Hex color
    if let Some(hex) = color.strip_prefix('#') {
        return match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::Rgb(r, g, b))
            }
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Color::Rgb(r, g, b))
            }
            _ => None,
        };
    }

    // Named colors
    match color.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

/// Raw theme data as parsed from TOML.
#[derive(Debug, Deserialize)]
struct RawTheme {
    #[serde(default)]
    palette: HashMap<String, String>,
    #[serde(flatten)]
    styles: HashMap<String, StyleValue>,
}

/// A style value can be either a full Style object or a simple string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StyleValue {
    /// Full style definition
    Full(Style),
    /// Simple foreground color only
    Simple(String),
}

impl StyleValue {
    fn into_style(self) -> Style {
        match self {
            StyleValue::Full(s) => s,
            StyleValue::Simple(fg) => Style {
                fg: Some(fg),
                bg: None,
                modifiers: Vec::new(),
            },
        }
    }
}

/// A syntax highlighting theme.
///
/// Themes map capture names (like "keyword", "string", "comment") to styles.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Theme name
    pub name: String,
    /// Color palette (named colors)
    palette: HashMap<String, String>,
    /// Styles for each capture name
    styles: HashMap<String, Style>,
    /// Cached ratatui styles
    cached_styles: HashMap<String, RatatuiStyle>,
}

impl Theme {
    /// Parse a theme from TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, ThemeError> {
        Self::from_toml_with_name(toml_str, "custom")
    }

    /// Parse a theme from TOML string with a name.
    pub fn from_toml_with_name(toml_str: &str, name: &str) -> Result<Self, ThemeError> {
        let raw: RawTheme = toml::from_str(toml_str)?;

        let mut styles = HashMap::new();
        for (key, value) in raw.styles {
            // Skip the palette key
            if key == "palette" {
                continue;
            }
            styles.insert(key, value.into_style());
        }

        let mut theme = Self {
            name: name.to_string(),
            palette: raw.palette,
            styles,
            cached_styles: HashMap::new(),
        };

        // Pre-cache all styles
        theme.cache_styles();

        Ok(theme)
    }

    /// Load a theme from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self, ThemeError> {
        let content = std::fs::read_to_string(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom");
        Self::from_toml_with_name(&content, name)
    }

    /// Get the ratatui style for a capture name.
    ///
    /// Uses hierarchical fallback: "keyword.control" falls back to "keyword".
    pub fn style_for(&self, capture: &str) -> RatatuiStyle {
        // Check exact match first
        if let Some(style) = self.cached_styles.get(capture) {
            return *style;
        }

        // Try hierarchical fallback
        let mut parts: Vec<&str> = capture.split('.').collect();
        while parts.len() > 1 {
            parts.pop();
            let parent = parts.join(".");
            if let Some(style) = self.cached_styles.get(&parent) {
                return *style;
            }
        }

        // Default style
        RatatuiStyle::default()
    }

    /// Cache all styles as ratatui styles.
    fn cache_styles(&mut self) {
        self.cached_styles.clear();
        for (name, style) in &self.styles {
            let ratatui_style = style.to_ratatui_style(&self.palette);
            self.cached_styles.insert(name.clone(), ratatui_style);
        }
    }

    /// Get the list of all capture names this theme defines styles for.
    pub fn capture_names(&self) -> Vec<&str> {
        self.styles.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_color("#FF0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("#00FF00"), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(parse_color("#0000FF"), Some(Color::Rgb(0, 0, 255)));
        assert_eq!(parse_color("#F00"), Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn test_parse_named_color() {
        assert_eq!(parse_color("red"), Some(Color::Red));
        assert_eq!(parse_color("Blue"), Some(Color::Blue));
        assert_eq!(parse_color("GREEN"), Some(Color::Green));
    }

    #[test]
    fn test_theme_from_toml() {
        let toml = r##"
            [palette]
            red = "#E06C75"
            green = "#98C379"

            [keyword]
            fg = "red"
            modifiers = ["bold"]

            [string]
            fg = "green"
        "##;

        let theme = Theme::from_toml(toml).unwrap();

        let keyword_style = theme.style_for("keyword");
        assert!(keyword_style.fg.is_some());

        let string_style = theme.style_for("string");
        assert!(string_style.fg.is_some());
    }

    #[test]
    fn test_hierarchical_fallback() {
        let toml = r##"
            [keyword]
            fg = "#FF0000"
        "##;

        let theme = Theme::from_toml(toml).unwrap();

        // "keyword.control" should fall back to "keyword"
        let style = theme.style_for("keyword.control");
        assert_eq!(style.fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn test_simple_style_value() {
        let toml = r##"
            keyword = "#FF0000"
            string = "green"
        "##;

        let theme = Theme::from_toml(toml).unwrap();

        let keyword_style = theme.style_for("keyword");
        assert_eq!(keyword_style.fg, Some(Color::Rgb(255, 0, 0)));

        let string_style = theme.style_for("string");
        assert_eq!(string_style.fg, Some(Color::Green));
    }
}
