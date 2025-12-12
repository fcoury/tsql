//! Built-in themes for syntax highlighting.
//!
//! These themes are embedded at compile time for fast startup.
//! Additional themes can be loaded from TOML files at runtime.

use crate::theme::Theme;

/// One Dark theme (dark background, inspired by Atom's One Dark).
pub fn one_dark() -> Theme {
    Theme::from_toml_with_name(ONE_DARK_TOML, "one_dark").expect("Built-in theme should be valid")
}

/// GitHub Light theme (light background).
pub fn github_light() -> Theme {
    Theme::from_toml_with_name(GITHUB_LIGHT_TOML, "github_light")
        .expect("Built-in theme should be valid")
}

/// One Dark theme TOML (Helix-compatible format).
const ONE_DARK_TOML: &str = r##"
[palette]
# One Dark color palette
red = "#E06C75"
green = "#98C379"
yellow = "#E5C07B"
blue = "#61AFEF"
purple = "#C678DD"
cyan = "#56B6C2"
orange = "#D19A66"
gray = "#5C6370"
light_gray = "#ABB2BF"
dark_gray = "#4B5263"
white = "#DCDFE4"

# Comments
[comment]
fg = "gray"
modifiers = ["italic"]

["comment.documentation"]
fg = "gray"
modifiers = ["italic"]

# Strings
[string]
fg = "green"

["string.escape"]
fg = "cyan"

["string.regexp"]
fg = "orange"

["string.special"]
fg = "cyan"

# Keywords
[keyword]
fg = "purple"

["keyword.control"]
fg = "purple"

["keyword.control.conditional"]
fg = "purple"

["keyword.control.repeat"]
fg = "purple"

["keyword.control.import"]
fg = "purple"

["keyword.control.return"]
fg = "purple"

["keyword.function"]
fg = "purple"

["keyword.operator"]
fg = "purple"

["keyword.storage"]
fg = "purple"

["keyword.storage.type"]
fg = "purple"

["keyword.storage.modifier"]
fg = "purple"

["keyword.special"]
fg = "cyan"

# Functions
[function]
fg = "blue"

["function.builtin"]
fg = "cyan"

["function.call"]
fg = "blue"

["function.method"]
fg = "blue"

["function.macro"]
fg = "cyan"

# Types
[type]
fg = "yellow"

["type.builtin"]
fg = "yellow"

# Variables
[variable]
fg = "light_gray"

["variable.builtin"]
fg = "red"

["variable.parameter"]
fg = "orange"

# Constants
[constant]
fg = "orange"

["constant.builtin"]
fg = "orange"

# Numbers and booleans
[number]
fg = "orange"

[boolean]
fg = "orange"

# Operators
[operator]
fg = "cyan"

# Punctuation
[punctuation]
fg = "light_gray"

["punctuation.bracket"]
fg = "light_gray"

["punctuation.delimiter"]
fg = "light_gray"

["punctuation.special"]
fg = "cyan"

# Attributes and properties
[attribute]
fg = "yellow"

[property]
fg = "red"

# Namespaces and labels
[namespace]
fg = "yellow"

[label]
fg = "red"

# Tags (for markup/HTML)
[tag]
fg = "red"

# Constructor
[constructor]
fg = "yellow"

# Special
[special]
fg = "cyan"

# Embedded code
[embedded]
fg = "cyan"

# Escape sequences
[escape]
fg = "cyan"
"##;

/// GitHub Light theme TOML.
const GITHUB_LIGHT_TOML: &str = r##"
[palette]
# GitHub Light color palette
red = "#CF222E"
green = "#116329"
yellow = "#4D2D00"
blue = "#0550AE"
purple = "#8250DF"
cyan = "#1B7C83"
orange = "#953800"
gray = "#6E7781"
light_gray = "#57606A"
dark_gray = "#24292F"
white = "#FFFFFF"

# Comments
[comment]
fg = "gray"
modifiers = ["italic"]

["comment.documentation"]
fg = "gray"
modifiers = ["italic"]

# Strings
[string]
fg = "blue"

["string.escape"]
fg = "cyan"

["string.regexp"]
fg = "cyan"

["string.special"]
fg = "cyan"

# Keywords
[keyword]
fg = "red"

["keyword.control"]
fg = "red"

["keyword.control.conditional"]
fg = "red"

["keyword.control.repeat"]
fg = "red"

["keyword.control.import"]
fg = "red"

["keyword.control.return"]
fg = "red"

["keyword.function"]
fg = "red"

["keyword.operator"]
fg = "red"

["keyword.storage"]
fg = "red"

["keyword.storage.type"]
fg = "red"

["keyword.storage.modifier"]
fg = "red"

["keyword.special"]
fg = "red"

# Functions
[function]
fg = "purple"

["function.builtin"]
fg = "purple"

["function.call"]
fg = "purple"

["function.method"]
fg = "purple"

["function.macro"]
fg = "purple"

# Types
[type]
fg = "orange"

["type.builtin"]
fg = "orange"

# Variables
[variable]
fg = "dark_gray"

["variable.builtin"]
fg = "orange"

["variable.parameter"]
fg = "dark_gray"

# Constants
[constant]
fg = "blue"

["constant.builtin"]
fg = "blue"

# Numbers and booleans
[number]
fg = "blue"

[boolean]
fg = "blue"

# Operators
[operator]
fg = "red"

# Punctuation
[punctuation]
fg = "dark_gray"

["punctuation.bracket"]
fg = "dark_gray"

["punctuation.delimiter"]
fg = "dark_gray"

["punctuation.special"]
fg = "red"

# Attributes and properties
[attribute]
fg = "purple"

[property]
fg = "blue"

# Namespaces and labels
[namespace]
fg = "orange"

[label]
fg = "dark_gray"

# Tags (for markup/HTML)
[tag]
fg = "green"

# Constructor
[constructor]
fg = "orange"

# Special
[special]
fg = "red"

# Embedded code
[embedded]
fg = "dark_gray"

# Escape sequences
[escape]
fg = "cyan"
"##;
