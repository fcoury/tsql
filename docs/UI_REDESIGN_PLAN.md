# Tonal Zones UI Redesign — Implementation Plan

## Context

The current TUI wraps all four panes (connections, schema, query, results) in `Block::default().borders(Borders::ALL)` with focus shown by a border-color change — visually noisy, space-wasting, and a weak focus signal. Following the case study, the maintainer chose **Direction A (tonal zones)**, opencode-style: background tones per pane, uppercase section labels instead of border titles, a left accent edge on the focused pane, and a status strip with a solid vim-mode pill.

**Maintainer decisions (locked):**
- **Full replacement** — the bordered UI is deleted; no legacy path, no `show_borders` flag (field removed), no truecolor fallback/detection. `Color::Rgb` is emitted unconditionally (matches existing stance: syntax highlighting and `grid.rs:1746` already do).
- **Custom theme files in scope** — `display.theme` resolves built-ins first, then `~/.tsql/themes/<name>.toml` (honoring `TSQL_CONFIG_DIR`), falling back to `one_dark` with a nonfatal startup warning.
- One Helix-style TOML drives both syntax and UI chrome via new `ui.*` keys; modals/popups keep their current bordered look (only must-not-regress).

**Verified load-bearing facts:**
- ratatui 0.29 `Block::inner()` reserves the top row when a top title exists even without `Borders::TOP` (`block.rs:644`), and `Block::render` paints `self.style` over the whole area — so `zone_block` needs no manual label rendering. Use `title_top(Line)` (the `Title` struct is deprecated; repo pushes with `-D warnings`).
- `symbols::border::Set` fields are `&'static str` — a custom set with `vertical_left: "▍"` is const-legal.
- `HighlightedTextArea` already has `.cursor_style()` / `.selection_style()` setters (`highlighted_editor.rs:66-74`) and derives cursor position from `block.inner()`.
- Syntax themes set **fg only** (no bg anywhere in one_dark/github_light) — pane tones show through tokens.
- The grid renders manually via `buf.set_string` with `Style::default()` (bg None) for all non-highlight cells — a block-level bg composes cleanly.
- `Config`/`DisplayConfig` use `#[serde(default)]`, no `deny_unknown_fields` — removing `show_borders` keeps old configs parseable (pin with a test).
- `tui-syntax` `Theme` parser accepts arbitrary keys via `#[serde(flatten)]`, supports fg/bg/modifiers, and `style_for()` has hierarchical dotted-scope fallback.
- `main.rs:367-369` overwrites `app.last_status` with startup warnings — the theme warning must be merged into `startup_warnings`, not just set in `with_config`.

## Core new module: `crates/tsql/src/ui/theme.rs`

### `UiTheme` — pre-resolved chrome styles, built once in `App::with_config`

```rust
pub struct UiTheme {
    // tones
    pub bg_base: Color,      // ui.background           — results grid
    pub bg_panel: Color,     // ui.background.panel     — sidebar
    pub bg_elevated: Color,  // ui.background.elevated  — query editor
    pub bg_status: Color,    // ui.statusline bg
    // text
    pub text: Color, pub text_muted: Color,
    // labels
    pub label: Style, pub label_focused: Style,
    // focus accents (vim-mode)
    pub accent: Color, pub accent_insert: Color, pub accent_visual: Color,
    // selection & cursors (explicit fg+bg, never REVERSED)
    pub selection: Style, pub cursor_cell: Style,
    pub editor_cursor: Style, pub editor_selection: Style,
    // search
    pub search_match: Style, pub search_match_current: Style,
    // semantic
    pub success: Color, pub warning: Color, pub error: Color,
    // misc
    pub pill_fg: Color, pub scrollbar: Style, pub grid_header: Style,
}
```

- `UiTheme::from_theme(&tui_syntax::Theme)` — `style_for("ui.*")` lookups; every field has a hardcoded One Dark constant as final fallback (any theme, even empty, yields a coherent UI). `UiTheme::fallback()` for tests/Default.
- `mode_accent(mode) -> Color`: Normal→accent, Insert→accent_insert, Visual→accent_visual.

### Zone helpers (geometry single source of truth)

```rust
pub const ZONE_EDGE_SET: border::Set = border::Set { vertical_left: "▍", ..border::PLAIN };

pub fn zone_block(label: Line, tone: Color, focused: bool, accent: Color) -> Block {
    let edge = if focused { Style::default().fg(accent).bg(tone) }
               else { Style::default().fg(tone).bg(tone) };  // invisible, column still reserved
    Block::default().style(Style::default().bg(tone))
        .borders(Borders::LEFT).border_set(ZONE_EDGE_SET).border_style(edge)
        .title_top(label).padding(Padding::new(1, 1, 0, 0))
}
pub fn zone_inner(area: Rect) -> Rect      // (x+2, y+1, w-3, h-1) — mirror of zone_block().inner()
pub fn zone_scrollbar_area(area: Rect) -> Rect  // right padding column below label row
pub fn zone_label(name, detail_spans, focused, accent, theme) -> Line<'static>  // " QUERY" style
```

**Inner geometry** (vs. old `Borders::ALL` inset of +1,+1,−2,−2): `x+2, y+1, width−3, height−1`. Identical focused/unfocused — no layout shift on focus change, by construction. Unit test pins `zone_block(...).inner(area) == zone_inner(area)` incl. degenerate rects.

### Theme loading

```rust
pub fn load_theme_from(name: &str, themes_dir: Option<&Path>) -> (Theme, Option<String>)
// "" | "default" | "one_dark" → built-in one_dark; "github_light" → built-in;
// else themes_dir/<name>.toml; missing/parse-error → one_dark + warning message
pub fn load_theme(name: &str) -> (Theme, Option<String>)  // dir = config_dir().join("themes")
```

Warning surfaces via `startup_warnings` in `main.rs` (merge `app.last_status.take()` before the join at `main.rs:367-369`).

### `ui.*` keys appended to `ONE_DARK_TOML` (`crates/tui-syntax/src/themes/mod.rs`)

Scopes: `ui.background` (#282C34) / `.panel` (#21252B) / `.elevated` (#2C313A); `ui.text` / `.muted`; `ui.label` / `.focused` (cyan bold); `ui.accent` (cyan) / `.insert` (green) / `.visual` (yellow); `ui.selection` (#3E4451 bg) / `.editor`; `ui.cursor` (explicit fg/bg) / `.cell`; `ui.search.match` / `.current`; `ui.statusline` (bg #1D2025) / `.mode` (pill ink); `ui.success/warning/error`; `ui.scrollbar`; `ui.grid.header`. Syntax scopes stay fg-only.

## Phases

Each phase: compiles, `cargo fmt --all`, `cargo clippy --all --all-targets -- -D warnings`, `cargo test --all` pass; app renders coherently (transitional mixed look between phases is acceptable).

### Phase 1 — Theme foundation, no visual change (M)
- `crates/tui-syntax/src/themes/mod.rs`: append `ui.*` block to `ONE_DARK_TOML`.
- `crates/tui-syntax/src/theme.rs`: tests only (`ui.*` parses with bg; hierarchical fallback panel→background).
- **New** `crates/tsql/src/ui/theme.rs`: `UiTheme`, `from_theme`, `fallback`, `mode_accent`, `load_theme(_from)`, zone helpers; `#[cfg(test)]` `luminance`/`contrast_ratio` helpers.
- `crates/tsql/src/ui/mod.rs`: register module.
- `crates/tsql/src/ui/highlighted_editor.rs:429`: `create_sql_highlighter(theme: Theme)` (was zero-arg hardcoded one_dark).
- `crates/tsql/src/app/app.rs`: `pub ui_theme: UiTheme` field (~:2434); build in `with_config` (:2654) from `config.display.theme`; pass syntax theme to highlighter (:2697).
- `crates/tsql/src/main.rs:367-369`: merge `last_status` into `startup_warnings`.
- `crates/tsql/src/config/schema.rs`: **delete** `show_borders` (:47, :62); reword `theme` doc (:48). Update `config.example.toml` (:27-31).
- Tests: tone resolution from one_dark; `from_theme(empty) == fallback()`; contrast ratios ≥ 4.5 for text-on-tone pairs across all built-ins + fallback, tones mutually distinguishable; `load_theme_from` tempdir matrix (valid/malformed/missing/built-in); old config with `show_borders = true` still parses.

### Phase 2 — Zone helpers live: query pane + geometry plumbing (L)
All in `app.rs` draw closure (:2995-3280) unless noted:
- :3071-3100 — replace border/title with `zone_block(zone_label("QUERY", …), ui_theme.bg_elevated, focused, ui_theme.mode_accent(mode))`; label detail: `[INSERT]` (focused, accent) + modified `[+]`.
- :3109-3114 — add `.cursor_style(ui_theme.editor_cursor)` + `.selection_style(ui_theme.editor_selection)` (kills REVERSED-on-tint and hardcoded Blue).
- :3128-3139 — **load-bearing**: replace `query_area.height/width.saturating_sub(2)` with `query_block.inner(query_area)` dims for `calculate_editor_scroll`.
- :3141-3170 — scrollbar → `zone_scrollbar_area(query_area)` + `ui_theme.scrollbar`.
- :3345-3348 — completion popup anchor derives from `zone_inner(query_area)`.
- :801 — `QUERY_BORDER_ROWS = 2` → `QUERY_CHROME_ROWS = 1`; usage :872; update `compute_query_panel_height` tests (:11693-11726).
- Tests (TestBackend/Buffer): label on row 0; `▍` accent column when focused; **focused-vs-unfocused buffer symbols identical** (geometry-stability regression test); all cell bg == tone; cursor position (x+2, y+1) for cursor (0,0).

### Phase 3 — Results grid zone (L)
- `grid.rs` `DataGrid` (:1391): add `label: Line<'a>`, `theme: &'a UiTheme` fields; render (:1400-1432) uses `zone_block(label, theme.bg_base, focused, theme.accent)`. Style swaps: header (:1503) → `theme.grid_header`; cursor row (:1529) → `theme.selection`; search/cursor-cell (:1744-1748) → theme; gutter DarkGray/Gray (:1607-1654) → `theme.text_muted/text`; empty-state messages → `theme.text_muted`; scrollbar (:1572-1592) → `zone_scrollbar_area` (content regains its last column).
- `app.rs`: build grid label in draw closure (`RESULTS · {n} rows · {ms}ms` + search match info — data lives on App); viewport math (:3172-3191) via `zone_inner(grid_area)` (preserve existing marker_w parity quirk — do not fix in this pass); `grid_mouse_target` (:11040-11096) hardcoded inset → `zone_inner`, update its tests (~:12322).
- `style.rs`: add `#[cfg(test)] assert_bg_cells_have_visible_fg(buf, bg)` (generalizes :46-64; keep old helper for modals).
- Tests: update grid render tests (:2046-2066, :2145-2175) for new offsets (data starts x=5); label row content; cell bg ∈ allowed set; contrast assertion on selection; focus-toggle symbol equality; mouse mapping.

### Phase 4 — Sidebar zones (M)
- `sidebar.rs` render (:64-100): add `theme: &UiTheme` param; split `[Percentage(30), Length(1), Min(0)]` with the 1-row spacer painted `bg_panel`; areas = chunks[0]/chunks[2].
- `render_connections` (:102-191) / `render_schema` (:193-270): `zone_block(…, bg_panel, focused, theme.accent)` each (accent edge only on the focused sub-section); selection highlight → `theme.selection` (focused) / `fg(theme.accent)` (unfocused, replaces Yellow); current-conn marker → `theme.success`; loading/error/empty → `theme.warning/error/text_muted`.
- Click math unchanged (label row occupies the old border row — `sidebar.rs:441-468`); schema tree uses stored absolute coords (:477-482).
- `app.rs:3031-3041`: pass `&self.ui_theme`.
- Tests: labels render; spacer bg; `▍` on focused section only; focus-toggle symbol equality; selection contrast.

### Phase 5 — Status strip + pill, light theme, docs (S/M)
- `status_line.rs`: add `separator_style()` to builder (separator hardcodes DarkGray at :270).
- `app.rs status_line()` (:10726-10881): mode segment = pill (`" {mode} "` with `bg(mode_accent) fg(pill_fg) BOLD`); segment styles → theme semantic colors; return `Paragraph::new(line).style(bg(bg_status).fg(text))` (paints full strip).
- `themes/mod.rs`: matching `ui.*` block for `GITHUB_LIGHT_TOML` (base #ffffff, panel #f6f8fa, elevated #eaeef2…) — Phase 1 contrast tests cover it automatically.
- Docs: `config.example.toml`, README note on custom themes dir.
- Noted follow-up (out of scope): `json_editor.rs:108` / `row_detail.rs:89` hardcode one_dark for modal syntax; ~170 modal `Color::` literals stay.

## Risks & mitigations
1. **Geometry regressions** (cursor drift, scroll jumps, mouse mis-hits) → single geometry source (`zone_block`/`zone_inner` + equality test); editor scroll uses the same `block.inner()` as the widget; per-pane focus-toggle symbol-equality tests.
2. **REVERSED cursor on tinted bg** → explicit `ui.cursor` style from Phase 2; fallback constants guarantee it's always set.
3. **Contrast on tones** → all highlight styles carry explicit fg+bg; contrast-ratio unit tests over every built-in + fallback; buffer assertions in render tests.
4. **Config back-compat** → `show_borders` removal safe (`#[serde(default)]`, no deny_unknown_fields; pinned by test); `theme = "default"` → one_dark; Config never written back to disk (verified).
5. **Sparse custom themes** → hierarchical `style_for` fallback + hardcoded final fallbacks; test with a minimal TOML.
6. **Clippy -D warnings vs deprecated `Title`** → use `title_top(Line)` exclusively.
7. **Insert-mode auto-height** grows one row (chrome 2→1) — intended; tests updated in Phase 2.

## Critical files
- `crates/tsql/src/app/app.rs` (draw closure :2995-3280, scroll math :3128, viewport :3172, `grid_mouse_target` :11040, `status_line()` :10726, `with_config` :2654)
- `crates/tsql/src/ui/theme.rs` (new)
- `crates/tsql/src/ui/grid.rs` (:1391-1593, :1744-1748)
- `crates/tsql/src/ui/sidebar.rs` (:64-270, :441-468)
- `crates/tui-syntax/src/themes/mod.rs`, `crates/tsql/src/config/schema.rs`, `crates/tsql/src/ui/status_line.rs`, `crates/tsql/src/main.rs`

## Verification
- Per phase: `cargo fmt --all` && `cargo clippy --all --all-targets -- -D warnings` && `cargo test --all`.
- End-to-end after Phases 2-5: run the app against a live connection (`cargo run`), and verify visually: no borders anywhere; tones distinct (sidebar/query/results/status); focus cycling (Tab / Ctrl-hjkl) moves the `▍` edge with **zero content shift**; vim mode flips edge+pill colors (i / v / Esc); scroll long query + grid with scrollbars in the gutter; mouse clicks on tree items and grid cells hit the right rows; search highlights legible; `display.theme = "github_light"` renders the light variant; a custom `~/.tsql/themes/test.toml` loads (and a bogus name shows the startup warning in the status strip).
