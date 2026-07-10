//! UI theme resolution and shared tonal-zone geometry.

use std::path::{Component, Path};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};
use tui_syntax::{themes, Theme};

use crate::app::Mode;
use crate::config::config_dir;

/// Pre-resolved styles used by the application's primary UI chrome.
#[derive(Clone, Debug, PartialEq)]
pub struct UiTheme {
    pub bg_base: Color,
    pub bg_panel: Color,
    pub bg_elevated: Color,
    pub bg_status: Color,
    pub text: Color,
    pub text_muted: Color,
    pub label: Style,
    pub label_focused: Style,
    pub accent: Color,
    pub accent_insert: Color,
    pub accent_visual: Color,
    pub selection: Style,
    pub cursor_cell: Style,
    pub editor_cursor: Style,
    pub editor_selection: Style,
    pub search_match: Style,
    pub search_match_current: Style,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub transaction: Color,
    pub pill_fg: Color,
    pub scrollbar: Style,
    pub grid_header: Style,
    pub overlay: Style,
    pub overlay_border: Style,
    pub overlay_title: Style,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self::fallback()
    }
}

impl UiTheme {
    /// Build the canonical One Dark UI fallback.
    pub fn fallback() -> Self {
        let text = rgb(0xDC, 0xDF, 0xE4);
        let text_muted = rgb(0x9D, 0xA5, 0xB4);
        let accent = rgb(0x56, 0xB6, 0xC2);
        let ink = rgb(0x1D, 0x20, 0x25);
        let selection_bg = rgb(0x3E, 0x44, 0x51);

        Self {
            bg_base: rgb(0x28, 0x2C, 0x34),
            bg_panel: rgb(0x21, 0x25, 0x2B),
            bg_elevated: rgb(0x2C, 0x31, 0x3A),
            bg_status: ink,
            text,
            text_muted,
            label: Style::default().fg(text_muted),
            label_focused: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            accent,
            accent_insert: rgb(0x98, 0xC3, 0x79),
            accent_visual: rgb(0xE5, 0xC0, 0x7B),
            selection: Style::default().fg(text).bg(selection_bg),
            cursor_cell: Style::default().fg(ink).bg(accent),
            editor_cursor: Style::default().fg(ink).bg(accent),
            editor_selection: Style::default().fg(text).bg(selection_bg),
            search_match: Style::default().fg(ink).bg(rgb(0xE5, 0xC0, 0x7B)),
            search_match_current: Style::default().fg(ink).bg(rgb(0xD1, 0x9A, 0x66)),
            success: rgb(0x98, 0xC3, 0x79),
            warning: rgb(0xE5, 0xC0, 0x7B),
            error: rgb(0xE0, 0x6C, 0x75),
            transaction: rgb(0xC6, 0x78, 0xDD),
            pill_fg: ink,
            scrollbar: Style::default().fg(text_muted),
            grid_header: Style::default()
                .fg(text)
                .bg(rgb(0x2C, 0x31, 0x3A))
                .add_modifier(Modifier::BOLD),
            overlay: Style::default().fg(text).bg(rgb(0x33, 0x3B, 0x49)),
            overlay_border: Style::default().fg(rgb(0x5C, 0x66, 0x73)),
            overlay_title: Style::default().fg(accent).add_modifier(Modifier::BOLD),
        }
    }

    /// Resolve UI chrome from a syntax theme, preserving complete fallbacks for
    /// fields omitted by parent or child scopes.
    pub fn from_theme(theme: &Theme) -> Self {
        let fallback = Self::fallback();

        let background = resolve_style(
            theme,
            "ui.background",
            Style::default().fg(fallback.text).bg(fallback.bg_base),
        );
        let panel = resolve_style(
            theme,
            "ui.background.panel",
            Style::default().fg(fallback.text).bg(fallback.bg_panel),
        );
        let elevated = resolve_style(
            theme,
            "ui.background.elevated",
            Style::default().fg(fallback.text).bg(fallback.bg_elevated),
        );
        let statusline = resolve_style(
            theme,
            "ui.statusline",
            Style::default().fg(fallback.text).bg(fallback.bg_status),
        );
        let text_style = resolve_style(theme, "ui.text", Style::default().fg(fallback.text));
        let muted_style = resolve_style(
            theme,
            "ui.text.muted",
            Style::default().fg(fallback.text_muted),
        );

        Self {
            bg_base: background.bg.unwrap_or(fallback.bg_base),
            bg_panel: panel.bg.unwrap_or(fallback.bg_panel),
            bg_elevated: elevated.bg.unwrap_or(fallback.bg_elevated),
            bg_status: statusline.bg.unwrap_or(fallback.bg_status),
            text: text_style.fg.unwrap_or(fallback.text),
            text_muted: muted_style.fg.unwrap_or(fallback.text_muted),
            label: resolve_style(theme, "ui.label", fallback.label),
            label_focused: resolve_style(theme, "ui.label.focused", fallback.label_focused),
            accent: resolve_color(theme, "ui.accent", fallback.accent),
            accent_insert: resolve_color(theme, "ui.accent.insert", fallback.accent_insert),
            accent_visual: resolve_color(theme, "ui.accent.visual", fallback.accent_visual),
            selection: normalize_explicit(resolve_style(theme, "ui.selection", fallback.selection)),
            cursor_cell: normalize_explicit(resolve_style(
                theme,
                "ui.cursor.cell",
                fallback.cursor_cell,
            )),
            editor_cursor: normalize_explicit(resolve_style(
                theme,
                "ui.cursor",
                fallback.editor_cursor,
            )),
            editor_selection: normalize_explicit(resolve_style(
                theme,
                "ui.selection.editor",
                fallback.editor_selection,
            )),
            search_match: normalize_explicit(resolve_style(
                theme,
                "ui.search.match",
                fallback.search_match,
            )),
            search_match_current: normalize_explicit(resolve_style(
                theme,
                "ui.search.match.current",
                fallback.search_match_current,
            )),
            success: resolve_color(theme, "ui.success", fallback.success),
            warning: resolve_color(theme, "ui.warning", fallback.warning),
            error: resolve_color(theme, "ui.error", fallback.error),
            transaction: resolve_color(theme, "ui.transaction", fallback.transaction),
            pill_fg: resolve_color(theme, "ui.statusline.mode", fallback.pill_fg),
            scrollbar: resolve_style(theme, "ui.scrollbar", fallback.scrollbar),
            grid_header: resolve_style(theme, "ui.grid.header", fallback.grid_header),
            overlay: normalize_explicit(resolve_style(theme, "ui.overlay", fallback.overlay)),
            overlay_border: resolve_style(theme, "ui.overlay.border", fallback.overlay_border),
            overlay_title: resolve_style(theme, "ui.overlay.title", fallback.overlay_title),
        }
    }

    /// Return the focus accent associated with a vim mode.
    pub fn mode_accent(&self, mode: Mode) -> Color {
        match mode {
            Mode::Normal => self.accent,
            Mode::Insert => self.accent_insert,
            Mode::Visual => self.accent_visual,
        }
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}

fn resolve_color(theme: &Theme, scope: &str, fallback: Color) -> Color {
    resolve_style(theme, scope, Style::default().fg(fallback))
        .fg
        .unwrap_or(fallback)
}

fn resolve_style(theme: &Theme, scope: &str, fallback: Style) -> Style {
    let mut resolved = fallback;

    for (index, character) in scope.char_indices() {
        if character == '.' {
            if let Some(style) = theme.style_for_exact(&scope[..index]) {
                resolved = resolved.patch(style);
            }
        }
    }

    if let Some(style) = theme.style_for_exact(scope) {
        resolved = resolved.patch(style);
    }

    resolved
}

fn normalize_explicit(mut style: Style) -> Style {
    style.add_modifier.remove(Modifier::REVERSED);
    style.sub_modifier.remove(Modifier::REVERSED);
    style
}

/// Left-edge symbols used by every tonal zone.
pub const ZONE_EDGE_SET: border::Set = border::Set {
    vertical_left: "▍",
    ..border::PLAIN
};

/// Build a primary-pane block with stable geometry in both focus states.
pub fn zone_block<'a>(
    label: Line<'a>,
    tone: Color,
    text: Color,
    focused: bool,
    accent: Color,
) -> Block<'a> {
    let edge = if focused {
        Style::default().fg(accent).bg(tone)
    } else {
        Style::default().fg(tone).bg(tone)
    };

    Block::default()
        .style(Style::default().fg(text).bg(tone))
        .borders(Borders::LEFT)
        .border_set(ZONE_EDGE_SET)
        .border_style(edge)
        .title_top(label)
        .padding(Padding::new(1, 1, 0, 0))
}

/// Return the shared content rectangle for a tonal zone.
pub fn zone_inner(area: Rect) -> Rect {
    let mut inner = area;
    inner.x = inner.x.saturating_add(1).min(inner.right());
    inner.width = inner.width.saturating_sub(1);
    inner.y = inner.y.saturating_add(1).min(inner.bottom());
    inner.height = inner.height.saturating_sub(1);
    inner.x = inner.x.saturating_add(1);
    inner.width = inner.width.saturating_sub(2);
    inner
}

/// Return the right-padding column below a zone's label.
pub fn zone_scrollbar_area(area: Rect) -> Rect {
    Rect {
        x: if area.width == 0 {
            area.x
        } else {
            area.right().saturating_sub(1)
        },
        y: area.y.saturating_add(1).min(area.bottom()),
        width: u16::from(area.width > 0),
        height: area.height.saturating_sub(1),
    }
}

/// Build the standard floating-overlay block: a rounded border and themed
/// title on the overlay surface tone.
pub fn overlay_block(title: &str, theme: &UiTheme) -> Block<'static> {
    let block = Block::default()
        .style(theme.overlay)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.overlay_border);
    if title.is_empty() {
        block
    } else {
        block.title_top(Line::from(Span::styled(
            format!(" {title} "),
            theme.overlay_title,
        )))
    }
}

/// Build a consistently styled uppercase zone label.
pub fn zone_label(
    name: &str,
    detail_spans: Vec<Span<'static>>,
    focused: bool,
    accent: Color,
    theme: &UiTheme,
) -> Line<'static> {
    let label_style = if focused {
        theme.label_focused.fg(accent)
    } else {
        theme.label
    };
    let mut spans = Vec::with_capacity(detail_spans.len() + 1);
    spans.push(Span::styled(format!(" {name}"), label_style));
    spans.extend(detail_spans);
    Line::from(spans)
}

/// Load a built-in or custom theme from the configured themes directory.
pub fn load_theme(name: &str) -> (Theme, Option<String>) {
    let themes_dir = config_dir().map(|path| path.join("themes"));
    load_theme_from(name, themes_dir.as_deref())
}

/// Load a built-in or custom theme from an explicit themes directory.
pub fn load_theme_from(name: &str, themes_dir: Option<&Path>) -> (Theme, Option<String>) {
    match name {
        "" | "default" | "one_dark" => return (themes::one_dark(), None),
        "github_light" => return (themes::github_light(), None),
        _ => {}
    }

    if !is_valid_theme_name(name) {
        return fallback_with_warning(name, "theme name must be a single file stem");
    }

    let Some(themes_dir) = themes_dir else {
        return fallback_with_warning(name, "configuration directory is unavailable");
    };
    let path = themes_dir.join(format!("{name}.toml"));

    match Theme::from_file(&path) {
        Ok(theme) => (theme, None),
        Err(error) => fallback_with_warning(name, &error.to_string()),
    }
}

fn is_valid_theme_name(name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return false;
    }

    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

fn fallback_with_warning(name: &str, reason: &str) -> (Theme, Option<String>) {
    (
        themes::one_dark(),
        Some(format!(
            "Failed to load theme '{name}': {reason}; using one_dark"
        )),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::buffer::Buffer;
    use ratatui::widgets::Widget;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn empty_theme_uses_complete_fallback() {
        let theme = Theme::from_toml("").unwrap();

        assert_eq!(UiTheme::from_theme(&theme), UiTheme::fallback());
    }

    #[test]
    fn child_style_merges_parent_and_fallback_components() {
        let theme = Theme::from_toml(
            r##"
            ["ui.selection"]
            fg = "#F0F0F0"
            modifiers = ["bold"]

            ["ui.selection.editor"]
            bg = "#303030"
            "##,
        )
        .unwrap();

        let ui = UiTheme::from_theme(&theme);

        assert_eq!(ui.editor_selection.fg, Some(rgb(0xF0, 0xF0, 0xF0)));
        assert_eq!(ui.editor_selection.bg, Some(rgb(0x30, 0x30, 0x30)));
        assert!(ui.editor_selection.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn normalized_explicit_styles_drop_reversed_modifier() {
        let theme = Theme::from_toml(
            r##"
            ["ui.cursor"]
            modifiers = ["reversed", "bold"]
            "##,
        )
        .unwrap();

        let cursor = UiTheme::from_theme(&theme).editor_cursor;

        assert!(!cursor.add_modifier.contains(Modifier::REVERSED));
        assert!(cursor.add_modifier.contains(Modifier::BOLD));
        assert!(cursor.fg.is_some());
        assert!(cursor.bg.is_some());
    }

    #[test]
    fn built_in_themes_define_required_ui_scopes() {
        for theme in [themes::one_dark(), themes::github_light()] {
            for scope in [
                "ui.background",
                "ui.background.panel",
                "ui.background.elevated",
                "ui.text",
                "ui.text.muted",
                "ui.label",
                "ui.label.focused",
                "ui.accent",
                "ui.accent.insert",
                "ui.accent.visual",
                "ui.selection",
                "ui.selection.editor",
                "ui.cursor",
                "ui.cursor.cell",
                "ui.search.match",
                "ui.search.match.current",
                "ui.statusline",
                "ui.statusline.mode",
                "ui.success",
                "ui.warning",
                "ui.error",
                "ui.transaction",
                "ui.overlay",
                "ui.overlay.border",
                "ui.overlay.title",
                "ui.scrollbar",
                "ui.grid.header",
            ] {
                assert!(
                    theme.style_for_exact(scope).is_some(),
                    "{} is missing {scope}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn built_in_base_text_has_readable_contrast() {
        for theme in [themes::one_dark(), themes::github_light()] {
            let ui = UiTheme::from_theme(&theme);
            for background in [ui.bg_base, ui.bg_panel, ui.bg_elevated, ui.bg_status] {
                assert!(
                    contrast_ratio(ui.text, background) >= 4.5,
                    "{} text contrast is too low on {background:?}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn built_in_explicit_styles_have_readable_contrast_and_distinct_tones() {
        for theme in [themes::one_dark(), themes::github_light()] {
            let ui = UiTheme::from_theme(&theme);
            assert_ne!(ui.bg_base, ui.bg_panel);
            assert_ne!(ui.bg_base, ui.bg_elevated);
            assert_ne!(ui.bg_panel, ui.bg_elevated);

            for style in [
                ui.selection,
                ui.cursor_cell,
                ui.editor_cursor,
                ui.editor_selection,
                ui.search_match,
                ui.search_match_current,
                ui.overlay,
            ] {
                let foreground = style.fg.expect("explicit style foreground");
                let background = style.bg.expect("explicit style background");
                assert!(
                    contrast_ratio(foreground, background) >= 4.5,
                    "{} explicit style contrast is too low: {style:?}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn zone_geometry_matches_block_inner_for_small_and_normal_areas() {
        let theme = UiTheme::fallback();
        for width in 0..12 {
            for height in 0..8 {
                let area = Rect::new(4, 7, width, height);
                let block = zone_block(
                    Line::from(" QUERY"),
                    theme.bg_elevated,
                    theme.text,
                    true,
                    theme.accent,
                );
                assert_eq!(zone_inner(area), block.inner(area));
            }
        }
    }

    #[test]
    fn zone_block_sets_base_foreground_and_background() {
        let theme = UiTheme::fallback();
        let area = Rect::new(0, 0, 12, 4);
        let mut buffer = Buffer::empty(area);
        zone_block(
            Line::from(" QUERY"),
            theme.bg_elevated,
            theme.text,
            false,
            theme.accent,
        )
        .render(area, &mut buffer);

        let cell = buffer.cell((5, 2)).unwrap();
        assert_eq!(cell.fg, theme.text);
        assert_eq!(cell.bg, theme.bg_elevated);
    }

    #[test]
    fn focus_changes_zone_edge_style_without_changing_symbols() {
        let theme = UiTheme::fallback();
        let area = Rect::new(0, 0, 12, 4);
        let mut focused = Buffer::empty(area);
        let mut unfocused = Buffer::empty(area);

        zone_block(
            Line::from(" QUERY"),
            theme.bg_elevated,
            theme.text,
            true,
            theme.accent,
        )
        .render(area, &mut focused);
        zone_block(
            Line::from(" QUERY"),
            theme.bg_elevated,
            theme.text,
            false,
            theme.accent,
        )
        .render(area, &mut unfocused);

        let focused_symbols: Vec<_> = focused.content.iter().map(|cell| cell.symbol()).collect();
        let unfocused_symbols: Vec<_> =
            unfocused.content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(focused_symbols, unfocused_symbols);
        assert_eq!(focused.cell((0, 1)).unwrap().fg, theme.accent);
        assert_eq!(unfocused.cell((0, 1)).unwrap().fg, theme.bg_elevated);
    }

    #[test]
    fn load_theme_from_handles_built_ins_and_custom_files() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("custom.toml"),
            r##"["ui.background"]
            fg = "#010203"
            bg = "#F0F0F0"
            "##,
        )
        .unwrap();

        let (built_in, warning) = load_theme_from("github_light", Some(directory.path()));
        assert_eq!(built_in.name, "github_light");
        assert_eq!(warning, None);

        let (custom, warning) = load_theme_from("custom", Some(directory.path()));
        assert_eq!(custom.name, "custom");
        assert_eq!(warning, None);
        assert_eq!(
            custom.style_for_exact("ui.background").unwrap().bg,
            Some(rgb(0xF0, 0xF0, 0xF0))
        );
    }

    #[test]
    fn load_theme_from_falls_back_for_missing_malformed_and_escaping_names() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("malformed.toml"), "not = [valid").unwrap();

        for name in [
            "missing",
            "malformed",
            "../outside",
            "nested/theme",
            "nested\\theme",
        ] {
            let (theme, warning) = load_theme_from(name, Some(directory.path()));
            assert_eq!(theme.name, "one_dark");
            assert!(warning.unwrap().contains(name));
        }
    }

    fn contrast_ratio(foreground: Color, background: Color) -> f64 {
        let foreground = luminance(foreground);
        let background = luminance(background);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    }

    fn luminance(color: Color) -> f64 {
        let Color::Rgb(red, green, blue) = color else {
            panic!("contrast tests require RGB colors, got {color:?}");
        };
        let component = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };

        0.2126 * component(red) + 0.7152 * component(green) + 0.0722 * component(blue)
    }
}
