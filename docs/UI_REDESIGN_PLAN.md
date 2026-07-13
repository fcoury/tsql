# Tonal Zones UI Redesign — Implementation Plan (Revision 2)

## Context

The current TUI wraps the four primary panes (connections, schema, query, and results) in `Block::default().borders(Borders::ALL)`. Focus is indicated only by changing the border color. This is visually noisy, consumes space, and provides a weak focus signal.

The replacement follows **Direction A (tonal zones)**: each primary pane has a background tone, uppercase section labels replace border titles, the focused pane has a left accent edge, and the status strip has a solid vim-mode pill.

**Maintainer decisions (locked):**

- **Full replacement:** remove the bordered primary-pane UI. Do not retain a legacy rendering path or `show_borders` flag.
- **Truecolor:** emit `Color::Rgb` unconditionally. This matches existing syntax highlighting and grid search styling.
- **Custom themes:** resolve `display.theme` against built-ins first, then `<config_dir>/themes/<name>.toml`, where `config_dir()` honors `TSQL_CONFIG_DIR`. Missing or invalid themes fall back to `one_dark` and produce a nonfatal startup warning.
- **One theme source:** Helix-style TOML drives both syntax highlighting and UI chrome through `ui.*` scopes.
- **Modals and popups:** retain their current bordered appearance. They are regression-tested but are not redesigned in this change.

## Verified load-bearing facts

- In ratatui 0.29, `Block::inner()` reserves the top row for a top title even without `Borders::TOP`, and `Block::render` applies the block style over its complete area. Zone labels therefore do not require separate rendering.
- Use `title_top(Line)` rather than the deprecated `Title` type because the repository runs clippy with `-D warnings`.
- `symbols::border::Set` fields are `&'static str`, so a const set with `vertical_left: "▍"` is valid.
- `HighlightedTextArea` exposes `.cursor_style()` and `.selection_style()`, and both rendering and native-cursor positioning derive their viewport from `block.inner()`.
- `HighlightedTextArea` replaces the syntax style on selected and block-cursor characters rather than patching it. Editor selection and cursor styles must therefore resolve to explicit foreground and background colors.
- The grid writes cells using styles whose unset fields inherit the buffer's existing style. A zone-level foreground and background provides a reliable base style for ordinary grid cells.
- `Config` and `DisplayConfig` use `#[serde(default)]` without `deny_unknown_fields`; removing `show_borders` keeps old configuration files parseable.
- The `tui-syntax` parser accepts arbitrary capture keys and supports foreground, background, and modifiers. Its current hierarchical fallback selects the nearest complete ancestor style; it does not merge missing fields from parent and child styles.
- `main.rs` currently writes joined startup warnings into `app.last_status`. Theme-loading diagnostics must join that warning collection without using `last_status` as temporary transport.

## Theme architecture

### `UiTheme`

Add `crates/tsql/src/ui/theme.rs` with a pre-resolved theme built once during `App::with_config`:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct UiTheme {
    // Pane tones and base text
    pub bg_base: Color,
    pub bg_panel: Color,
    pub bg_elevated: Color,
    pub bg_status: Color,
    pub text: Color,
    pub text_muted: Color,

    // Labels and focus
    pub label: Style,
    pub label_focused: Style,
    pub accent: Color,
    pub accent_insert: Color,
    pub accent_visual: Color,

    // Explicit foreground + background styles
    pub selection: Style,
    pub cursor_cell: Style,
    pub editor_cursor: Style,
    pub editor_selection: Style,
    pub search_match: Style,
    pub search_match_current: Style,

    // Semantic and miscellaneous styles
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub pill_fg: Color,
    pub scrollbar: Style,
    pub grid_header: Style,
}
```

`UiTheme::fallback()` is the canonical hardcoded One Dark UI theme. `UiTheme::from_theme(&Theme)` starts from that fallback and applies theme values component by component.

### Component-wise style resolution

Do not use `Theme::style_for()` alone to resolve UI styles. An exact child style containing only `bg` must still inherit its parent's `fg`, and both must retain hardcoded defaults for any remaining fields.

Add an exact lookup API to `tui-syntax`:

```rust
pub fn style_for_exact(&self, capture: &str) -> Option<RatatuiStyle>
```

Resolve a UI style in this order:

1. Start with the complete hardcoded fallback style.
2. Patch each defined ancestor from least to most specific, for example `ui`, `ui.selection`, then `ui.selection.editor`.
3. Preserve fallback fields when a theme style omits them.
4. Remove `REVERSED` from normalized selection and cursor styles; these styles use explicit colors.

For example, a custom theme with only:

```toml
["ui.selection.editor"]
bg = "#44475a"
```

retains the fallback editor-selection foreground while replacing its background.

Tests must cover exact lookup, parent/child component merging, modifier merging, and a child that supplies only a background.

### Theme scopes

Add matching `ui.*` scopes to both built-in themes during Phase 1:

- `ui.background`, `ui.background.panel`, `ui.background.elevated`
- `ui.text`, `ui.text.muted`
- `ui.label`, `ui.label.focused`
- `ui.accent`, `ui.accent.insert`, `ui.accent.visual`
- `ui.selection`, `ui.selection.editor`
- `ui.cursor`, `ui.cursor.cell`
- `ui.search.match`, `ui.search.match.current`
- `ui.statusline`, `ui.statusline.mode`
- `ui.success`, `ui.warning`, `ui.error`
- `ui.scrollbar`, `ui.grid.header`

One Dark tones:

- base `#282C34`
- panel `#21252B`
- elevated `#2C313A`
- status `#1D2025`

GitHub Light tones:

- base `#FFFFFF`
- panel `#F6F8FA`
- elevated `#EAEEF2`
- status `#D8DEE4`

Selection, cursor, and search scopes in both built-ins must specify explicit foreground and background colors. Syntax scopes remain foreground-only so the zone base style shows through.

### Theme loading and diagnostics

```rust
pub fn load_theme_from(
    name: &str,
    themes_dir: Option<&Path>,
) -> (Theme, Option<String>);

pub fn load_theme(name: &str) -> (Theme, Option<String>);
```

Resolution order:

1. `""`, `"default"`, or `"one_dark"` → built-in One Dark.
2. `"github_light"` → built-in GitHub Light.
3. A valid custom name → `<themes_dir>/<name>.toml`.
4. Missing, unreadable, or invalid TOML → One Dark plus a warning containing the requested name and failure reason.

Custom names must be a single file stem: reject absolute paths, path separators, `.` and `..`. This keeps theme resolution inside the themes directory.

Add a private `startup_warnings: Vec<String>` field to `App` and a `take_startup_warnings()` method. `App::with_config` pushes only theme-loading diagnostics into that collection. In `main.rs`, extend the existing local `startup_warnings` with `app.take_startup_warnings()` before joining them into `last_status`. Do not read or clear `last_status` during this merge.

## Zone geometry

```rust
pub const ZONE_EDGE_SET: border::Set = border::Set {
    vertical_left: "▍",
    ..border::PLAIN
};

pub fn zone_block(
    label: Line,
    tone: Color,
    text: Color,
    focused: bool,
    accent: Color,
) -> Block {
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

pub fn zone_inner(area: Rect) -> Rect;
pub fn zone_scrollbar_area(area: Rect) -> Rect;
pub fn zone_label(
    name: &str,
    detail_spans: Vec<Span<'static>>,
    focused: bool,
    accent: Color,
    theme: &UiTheme,
) -> Line<'static>;
```

For normal-sized rectangles, `zone_block(...).inner(area)` is `(x + 2, y + 1, width - 3, height - 1)`:

- one column for the left edge
- one column for left padding
- one column for right padding and the scrollbar gutter
- one row for the label

`zone_inner` must mirror ratatui's saturating behavior exactly for degenerate rectangles. A unit test pins `zone_block(...).inner(area) == zone_inner(area)` over normal and degenerate sizes.

The focused and unfocused forms always reserve the same geometry. Only styles change.

## Implementation phases

Each phase must compile, pass formatting and linting, pass tests, and leave every enabled built-in theme coherent. Transitional mixtures of bordered and zone panes are acceptable, but foreground/background combinations must remain legible.

### Phase 1 — Theme foundation and loading (M)

- Add all `ui.*` scopes to both `ONE_DARK_TOML` and `GITHUB_LIGHT_TOML`.
- Add `Theme::style_for_exact()` and tests in `crates/tui-syntax/src/theme.rs`.
- Add `crates/tsql/src/ui/theme.rs` with:
  - `UiTheme`
  - component-wise style resolution
  - `fallback()`
  - `mode_accent(mode)`
  - `load_theme()` and `load_theme_from()`
  - zone geometry helpers
  - test-only luminance and contrast helpers
- Register and re-export the required theme types/helpers from `crates/tsql/src/ui/mod.rs`.
- Remove `DisplayConfig::show_borders` and its default; update `config.example.toml`.
- Reword the `display.theme` configuration documentation.

Do not connect the new loader to `App` yet. Phase 1 is intentionally a no-visual-change foundation: the configured syntax and UI themes become active together in Phase 2, when the query pane has an explicit base foreground and background.

Tests:

- both built-ins parse all required `ui.*` scopes
- `UiTheme::from_theme(empty) == UiTheme::fallback()`
- partial child styles inherit parent and fallback components
- base text meets a 4.5:1 contrast ratio against each pane tone for both built-ins
- every explicit selection, cursor, and search foreground/background pair meets its documented contrast threshold
- the three pane tones are distinct
- valid, malformed, missing, and built-in theme loading
- custom theme names cannot escape the themes directory
- an old config containing `show_borders = true` still parses

### Phase 2 — Query zone and syntax-theme activation (L)

Application integration:

- Add `ui_theme` and structured `startup_warnings` fields to `App`.
- In `App::with_config`, load the configured theme once, build `UiTheme`, and pass that same loaded theme into the SQL highlighter.
- In `main.rs`, merge `app.take_startup_warnings()` into the existing startup-warning vector without reading or clearing `last_status`.

In the main draw closure:

- Replace the query border/title with `zone_block` using `bg_elevated`, `text`, and `mode_accent(mode)`.
- Build a `QUERY` label with focused mode detail and a modified `[+]` indicator.
- Change `create_sql_highlighter` to accept the loaded `Theme`. This activates the configured syntax and UI themes atomically now that the editor has an explicit matching base foreground/background.
- Apply `ui_theme.editor_cursor` and `ui_theme.editor_selection` to `HighlightedTextArea`.
- Derive editor scroll dimensions from `query_block.inner(query_area)` rather than subtracting hardcoded border widths.
- Render the scrollbar in `zone_scrollbar_area(query_area)` using `ui_theme.scrollbar`.
- Derive the completion-popup origin from `zone_inner(query_area)`.
- Rename `QUERY_BORDER_ROWS` to `QUERY_CHROME_ROWS` and change it from 2 to 1. Update query-height tests.

Tests:

- label appears on row 0
- focused edge uses `▍` and the current mode accent
- focused and unfocused buffers have identical symbols and geometry but different edge styles
- ordinary editor cells inherit `theme.text` and `bg_elevated`
- selection and cursor cells use their explicit allowed foreground/background pairs
- native cursor position for editor coordinate `(0, 0)` is `(inner.x, inner.y)`
- One Dark and GitHub Light query renders contain no reset foreground on nonblank content cells
- a bad theme and an unrelated `last_status`/`last_error` remain separate

Do not assert that every cell has the pane background: cursor and selection cells intentionally use different backgrounds.

### Phase 3 — Results grid zone (L)

In `grid.rs`:

- Add `label: Line<'a>` and `theme: &'a UiTheme` to `DataGrid`.
- Render with `zone_block(label, bg_base, text, focused, accent)`.
- Replace hardcoded styles with:
  - `grid_header`
  - `selection`
  - `cursor_cell`
  - `search_match`
  - `search_match_current`
  - `text` and `text_muted`
- Preserve the zone base foreground for ordinary body cells.
- Move the vertical scrollbar into `zone_scrollbar_area(area)` so it no longer overwrites the content column.

In `app.rs`:

- Build `RESULTS · {n} rows · {ms}ms`, with search match detail when active.
- Derive viewport dimensions from `zone_inner(grid_area)`.
- Preserve the existing marker-width parity behavior; changing it is outside this redesign.
- Change `grid_mouse_target` to use `zone_inner(grid_area)` and update its tests.

In `style.rs`, add a test helper that validates cells against an explicit set of allowed foreground/background pairs. Keep the existing modal selection helper.

Tests:

- label content and offsets
- ordinary data cells use `text` on `bg_base`
- selection, search, and cursor cells use allowed explicit pairs
- no nonblank grid cell has a reset foreground under either built-in theme
- focus changes styles without changing symbols or geometry
- scrollbar occupies only the right gutter
- mouse mapping matches rendered rows and columns

### Phase 4 — Sidebar zones (M)

- Change the sidebar split to `[Percentage(30), Length(1), Min(0)]`.
- Paint the spacer row with `bg_panel` and `text`.
- Render connections and schema with `zone_block(..., bg_panel, text, ...)`.
- Show the accent edge only on the focused sidebar subsection.
- Replace selection, current-connection, loading, error, empty-state, and muted styles with `UiTheme` values.
- Ensure non-current connection and ordinary schema text inherit `text` on `bg_panel`.
- Pass `&self.ui_theme` from `app.rs`.

Connection click-row math remains unchanged because the label occupies the former top-border row. Schema-tree clicks continue to use the absolute coordinates recorded by the tree widget. The spacer is outside both stored hit-test areas.

Tests:

- both labels render
- spacer has the panel base style
- only the focused subsection has a visible `▍`
- focus changes styles without changing symbols or geometry
- ordinary and selected text have explicit visible foregrounds
- clicks on labels, rows, blank content, and the spacer produce the expected result

### Phase 5 — Status strip and documentation (S/M)

- Add `separator_style()` to `StatusLineBuilder`.
- Render the mode segment as `" {mode} "` with `bg(mode_accent)`, `fg(pill_fg)`, and `BOLD`.
- Replace status segment colors with theme semantic colors.
- Return `Paragraph::new(line).style(Style::default().bg(bg_status).fg(text))` so padding and unstyled spans inherit the status base style.
- Add a status-line render test for narrow and wide widths under both built-ins.
- Document the custom theme directory, supported `ui.*` keys, exact quoted TOML syntax, fallback behavior, and a minimal example in the README.
- Update `config.example.toml` with `one_dark`, `github_light`, and custom-name examples.

Out of scope:

- `json_editor.rs` and `row_detail.rs` continue using One Dark syntax highlighting.
- Existing modal and popup color literals remain unchanged.
- Modal and popup borders remain visible.

## Risks and mitigations

1. **Reset foreground on painted backgrounds** — every zone applies both `text` and its tone as the base style; render tests reject reset foregrounds on nonblank content.
2. **Partial custom styles lose required fields** — UI styles merge fallback, parent, and exact child components before use.
3. **Geometry regressions** — `zone_block`, `zone_inner`, and `zone_scrollbar_area` are the shared geometry source; editor scroll, cursor placement, viewport sizing, and mouse hit-testing use those helpers.
4. **Cursor or selection disappears on tinted backgrounds** — normalized cursor, selection, and search styles always have explicit foreground and background colors and never rely on `REVERSED`.
5. **Built-in light/dark mismatch** — both built-ins receive complete UI palettes before the configured syntax theme is activated.
6. **Theme warnings overwrite runtime status** — theme diagnostics use a dedicated startup-warning collection and never use `last_status` as transport.
7. **Config compatibility** — unknown `show_borders` remains accepted, `theme = "default"` maps to One Dark, and configuration is not written back automatically.
8. **Theme path escape** — custom theme names are validated as a single file stem before joining them to the theme directory.
9. **Custom theme contrast** — hardcoded UI fallbacks keep omitted chrome scopes usable; custom syntax colors remain the theme author's responsibility and are documented as such.
10. **Deprecated ratatui APIs** — zone helpers use `title_top(Line)` exclusively.
11. **Insert-mode height change** — reducing query chrome from two rows to one intentionally adds one visible editor row; query-height tests pin the behavior.

## Critical files

- `crates/tsql/src/app/app.rs`
- `crates/tsql/src/main.rs`
- `crates/tsql/src/ui/theme.rs` (new)
- `crates/tsql/src/ui/highlighted_editor.rs`
- `crates/tsql/src/ui/grid.rs`
- `crates/tsql/src/ui/sidebar.rs`
- `crates/tsql/src/ui/status_line.rs`
- `crates/tsql/src/ui/style.rs`
- `crates/tsql/src/config/schema.rs`
- `crates/tui-syntax/src/theme.rs`
- `crates/tui-syntax/src/themes/mod.rs`
- `config.example.toml`
- `README.md`

## Verification

Per phase:

```sh
cargo fmt --all
cargo clippy --all --all-targets -- -D warnings
cargo test --all
```

After Phases 2–5, run the app against a live connection and verify:

- primary panes have no surrounding borders; modal and popup borders remain
- sidebar, query, results, and status tones are distinct
- ordinary text is legible in One Dark and GitHub Light regardless of the terminal's default foreground
- Tab, Shift-Tab, and Ctrl/Alt-hjkl move the `▍` edge without shifting content
- `i`, `v`, and Esc update the query edge and status pill colors
- long query and grid content scroll without the scrollbar overwriting content
- sidebar and grid mouse clicks target the rendered row and column
- selection, search, cursor, muted, error, and loading text remain legible
- `display.theme = "github_light"` renders a complete light UI
- a valid custom theme loads from `<config_dir>/themes`
- an invalid or missing theme falls back to One Dark and appears in the startup-warning status without replacing unrelated application errors
