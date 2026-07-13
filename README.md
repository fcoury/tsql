# tsql

A modern, keyboard-first PostgreSQL and MongoDB CLI with a TUI interface.

[![CI](https://github.com/fcoury/tsql/actions/workflows/ci.yml/badge.svg)](https://github.com/fcoury/tsql/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/tsql.svg)](https://crates.io/crates/tsql)
[![License](https://img.shields.io/crates/l/tsql.svg)](LICENSE)
[![Discord](https://img.shields.io/discord/1204152891049512960)](https://discord.gg/b928dKDcQq)

If you like this crate show some support by [following fcoury (me) on X](https://x.com/fcoury)

![tsql screenshot](assets/screenshot.png)

[Join us on Discord](https://discord.gg/b928dKDcQq)

## Features

- **Full-screen TUI** - Split-pane interface with query editor and results grid
- **Notebook workspace** - Build an ordered SQL narrative with durable inline outputs and PostgreSQL session-snapshot refinement
- **Vim-style keybindings** - Navigate and edit with familiar modal commands
- **Syntax highlighting** - SQL and JSON highlighting powered by tree-sitter
- **Smart completion** - Schema-aware autocomplete for tables, columns, and keywords
- **Results grid** - Scrollable, searchable data grid with column resizing, multi-row selection, flexible yank (TSV/CSV/JSON/Markdown), and server-backed Classic/PostgreSQL result transformations
- **Inline editing** - Edit cells directly in the grid with automatic SQL generation
- **JSON support** - Detect, format, and edit JSON/JSONB columns with syntax highlighting
- **Postgres + MongoDB** - Connect with `postgres://...` or `mongodb://...` URLs
- **Schema commands** - `psql`-style commands plus Mongo helpers (`:show dbs`, `:show collections`, `:describe`)
- **Query history** - Persistent history with fuzzy search, pinning, and deletion
- **AI query assistant** - Draft DB-aware queries with follow-ups and one-key accept (`:ai` / `Ctrl+G`)
- **External editor** - Open the current query in `$VISUAL` / `$EDITOR` with `vv`
- **1Password integration** - Store an `op://` secret reference per connection instead of a plain password
- **Update checks** - Notify of new versions with install-method specific upgrade hints, plus optional in-app apply for standalone installs when `updates.mode = "auto"`
- **Configurable** - Customize keybindings and appearance via config file

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap fcoury/tap
brew install tsql
```

### Cargo (from source)

```bash
cargo install tsql
```

### Binary Download

Download pre-built binaries from the [GitHub Releases](https://github.com/fcoury/tsql/releases) page.

## Quick Start

```bash
# Connect with a connection URL
tsql postgres://user:password@localhost:5432/mydb
tsql mongodb://user:password@localhost:27017/mydb

# Or set DATABASE_URL environment variable
export DATABASE_URL=postgres://user:password@localhost:5432/mydb
tsql

# Or configure a default connection in ~/.tsql/config.toml
tsql
```

Once connected:

1. Type a SQL query in the editor pane
2. Press `Enter` to execute
3. Use `Tab` / `Shift-Tab` to cycle panes, or `Ctrl-h/j/k/l` to move spatially
4. Press `?` for help with all keybindings (type `/` inside the help popup to filter)

Switch to the opt-in notebook workspace with `:notebook` or `:mode notebook`.
Use `:mode classic` to return without discarding either workspace.

## Keybindings

### Global

Pane movement follows the screen layout:

```text
Connections <-> Query
     ^           ^
     v           v
  Schema    <-> Results
```

| Key                                  | Action                                             |
| ------------------------------------ | -------------------------------------------------- |
| `Tab` / `Shift-Tab`                  | Cycle panes clockwise / counter-clockwise in Normal mode |
| `Ctrl-h/j/k/l`                       | Move between panes in Normal mode                  |
| `Alt-h/j/k/l`                        | Move between panes in any mode                     |
| `?`                                  | Toggle help popup (`/` to filter inside)           |
| `Ctrl+Shift+B` / `Ctrl+\` / `Ctrl+4` | Toggle sidebar                                     |
| `Ctrl+O`                             | Open connection picker                             |
| `Ctrl+Shift+C` / `gm`                | Open connection manager                            |
| `q`                                  | Quit application                                   |
| `Esc`                                | Return to normal mode / close popups               |

Moving left from Query or Results automatically reveals the aligned hidden
sidebar pane. Opening the sidebar with its toggle keeps the current pane
focused.

On taller terminals, the query editor grows from 7 to as many as 12 rows by
default. `Alt+M` toggles a maximized results view that hides the query editor
and sidebar, then restores the previous workspace layout when pressed again.

### Schema Sidebar

| Key            | Action         |
| -------------- | -------------- |
| `r` / `Ctrl-r` | Refresh schema |

The table template actions (`Enter`, then `s`/`i`/`u`/`d`) replace the query
editor. When it already contains a query, tsql asks for confirmation first.
`Enter`, then `n` still inserts only the table name at the cursor.

### Query Editor (Normal Mode)

| Key       | Action                                              |
| --------- | --------------------------------------------------- |
| `h/j/k/l` | Move cursor                                         |
| `i/a/I/A` | Enter insert mode                                   |
| `o/O`     | Open line below/above                               |
| `dd`      | Delete line                                         |
| `yy`      | Yank (copy) line                                    |
| `p/P`     | Paste after/before                                  |
| `u`       | Undo                                                |
| `v`       | Enter visual mode                                   |
| `vv`      | Open query in `$VISUAL` / `$EDITOR`, reload on exit |
| `/`       | Search                                              |
| `Ctrl-r`  | Fuzzy history search                                |
| `Ctrl-g`  | Open AI query assistant                             |
| `Enter`   | Execute query                                       |
| `:`       | Command mode                                        |

### Results Grid

| Key         | Action                                        |
| ----------- | --------------------------------------------- |
| `h/j/k/l`   | Navigate cells                                |
| `H/L`       | Scroll horizontally                           |
| `gg/G`      | First/last row                                |
| `Space`     | Toggle row selection and advance cursor       |
| `a`         | Select all rows (press again to deselect all) |
| `A`         | Invert selection                              |
| `Esc`       | Clear selection                               |
| `yy` / `yY` | Yank row(s) as TSV / TSV with headers         |
| `yj`        | Yank row(s) as JSON                           |
| `yc` / `yC` | Yank row(s) as CSV / CSV with headers         |
| `ym`        | Yank row(s) as Markdown table                 |
| `c`         | Copy cell                                     |
| `e`         | Edit cell                                     |
| `o`         | Open row detail view                          |
| `/`         | Search in results                             |
| `+/-`       | Widen/narrow column                           |
| `=`         | Fit/collapse column                           |
| `Ctrl-r`    | Rerun the last query                          |

Yank commands operate on all selected rows when a selection is active, or the cursor row otherwise.

### Classic result transformations

With a PostgreSQL result cell focused, open the Actions palette with `Ctrl+Shift+P`, `Cmd+K`, or `:actions` to sort or add a secondary sort, filter or exclude the current value, filter NULL/non-NULL or text contents, enter a custom comparison, choose columns, or group and count the current column. These transformations run on the server against the original read-only query, not just the rows loaded in the grid. Applying one reruns the source query, respects an existing `LIMIT`, and is unavailable during an active transaction; volatile functions and other session-level effects can still run again.

The same operations are available as commands: `:sort asc|desc|add-asc|add-desc|toggle [#column|name]`, `:filter [#column|name] eq|ne|<|<=|>|>=|contains|not-contains|null|not-null [value]`, `:columns`, and `:group-count`. Use `:clear-filters`, `:clear-sort`, or `:reset-result` to remove transformations, and `:result-sql copy|open` to copy the generated SQL or open it in the editor.

### Notebook Workspace

Notebook mode keeps one selected cell with Cell, Editor, and Result focus. PostgreSQL
read-only `SELECT`, `WITH`, `VALUES`, and `TABLE` results that fit the configured
retention limits are captured once as session-local TEMP snapshots. Data-changing
CTEs, `SELECT ... INTO`, and locking statements execute without retention. Pressing `r` creates a
dependent cell bound to that immutable result version; it does not rerun the source.
MongoDB cells execute normally, but refinement is unavailable.

Cell numbers are stable identities used by `@result_N`, so inserting or deleting
cells can leave them out of visual sequence. `@result` uses the most recently
completed refinable result, while `@result_N` uses the latest available result from
cell `N` when a restored or loaded cell has no live binding. The chosen snapshot is
pinned for that execution; existing live `@result_N` dependencies remain immutable.
After restarting, rerun the source cell before running its dependent cell.
Cells can join multiple sources naturally, including aliases, for example
`SELECT a.id FROM @result_1 AS a JOIN @result_2 b USING (id)`. Name a reusable
source with `:name recent_users` and reference it as `@result_recent_users`;
names are stable across saves, case-insensitive, and must be unique.
While editing a PostgreSQL cell, press `Tab` after `@res...` to complete the
latest, numbered, or named live result reference with its source/run/row-count
metadata.

| Key | Cell-focus action |
| --- | --- |
| `j` / `k` | Next / previous cell |
| `Enter` / `e` | Enter the selected editor |
| `Ctrl-e` | Execute in place |
| `o` | Inspect the result |
| `n` | Select or create the trailing draft |
| `r` | Refine a complete retained PostgreSQL result |
| `h` / `l` | Collapse / expand the selected result |
| `z` | Collapse or expand output |
| `x` | Clear the selected cell and dependent executions |
| `Esc` | Return from Editor/Result to Cell focus |

Mouse input is also supported: click a composer to focus its editor, click the right-edge
`▾` / `▸` chevron to expand or collapse its result, click an inline result to select its row
and column, and use the wheel to move within the focused region.
Clearing an execution preserves the SQL in every affected cell so the chain can be
rerun; a confirmation prompt lists how many dependent results will be cleared.

Result focus uses the same navigable table as the Classic results grid. Use `h/j/k/l` or
the arrow keys to move between cells, `gg/G` and `0/$` to jump between rows and columns,
`/` and `n/N` to search, `Space` to select rows, the yank commands to copy, `+/-` or `=`
to size a column, and `o` to open row detail. The active result expands to fill the
workspace while other cells compact into one-line summaries. Notebook results are
read-only; complete retained results fetch more rows as the cursor approaches the
end, and `G` loads through to the final retained row. Press `Esc` to restore the
notebook flow and return to Cell focus. Search clearly reports when a retained
result is only partially loaded. Full-result export streams all retained rows
directly to disk without expanding the display-memory budget; selecting loaded
rows with `Space` exports only those rows. CSV, JSON, TSV, and SQL INSERT
exports are supported (`:export sql:archive.users ./users.sql` overrides the
inferred/default destination table). Notebook previews enforce both row and
display-byte limits.

Use `:run-all`, `:run-above`, `:run-below`, or `:run-dependents` to replay cells in
dependency order. Execution is serial and stops at the first failed cell. Save a
source-and-lineage-only document with `:save-notebook <path>` and reopen it with
`:open-notebook <path>`; runtime outputs and TEMP handles are intentionally not
stored in portable notebook files. Recent run status, timing, row counts, SQL,
and compact output/error previews are kept in the local session only; use
`:cell-history` to browse and restore a previous source or `:cell-run <id>` to
restore a specific execution.
For larger documents, `:insert-above`, `:insert-below`, `:duplicate-cell`, and
`:move-cell-up` / `:move-cell-down` preserve stable identities while composing or
reordering a workflow. `:outline` lists runnable cells and names; use
`:cell <id-or-name>` to jump directly to one. `:collapse-source`,
`:expand-source`, and `:toggle-source` compact or reveal long composers.
Use `:error` to inspect a failed cell's full database diagnostic and `y` to copy
it from the error popup, or `:copy-error` to copy the selected cell error directly.
PostgreSQL positions are mapped through result-reference expansion and preview
wrappers; `:error-jump` expands the source and moves the cursor to the reported
line/column. Cells that finish or fail off-screen leave a status indicator;
`:activity` jumps to the latest update.
`:explain-cell` creates and runs a non-materialized PostgreSQL `EXPLAIN` cell below
the selection while preserving any result dependencies.
Set `TSQL_ASCII=1` (or use `TERM=dumb`) when a terminal cannot render the
notebook disclosure and rail glyphs; expanded results use `v`, collapsed results
use `>`, and cell rails use `|`.

Snapshots last only for the current database session. Reconnects keep displayed
output but disable refinement. Transaction/statement poolers are not compatible
with cross-cell TEMP snapshots; set `snapshot_mode = "off"` for those connections.
Metadata commands such as `:\\dt` open their results in the Classic grid; use
`:notebook` to return to the preserved notebook workspace.

### Row Detail (`o` to open)

| Key           | Action                             |
| ------------- | ---------------------------------- |
| `j/k`         | Next/previous field                |
| `g/G`         | First/last field                   |
| `yy` / `yY`   | Copy row as TSV / TSV with headers |
| `yj`          | Copy row as JSON                   |
| `yc` / `yC`   | Copy row as CSV / CSV with headers |
| `ym`          | Copy row as Markdown table         |
| `e` / `Enter` | Edit selected field                |
| `q` / `Esc`   | Close                              |

### History Picker (`Ctrl-r` or `gh`)

| Key      | Action                                                                          |
| -------- | ------------------------------------------------------------------------------- |
| `Enter`  | Load selected query into editor                                                 |
| `Ctrl-b` | Pin / unpin selected entry (pinned entries are never auto-pruned, shown with ★) |
| `Ctrl-d` | Delete selected entry                                                           |
| `Ctrl-t` | Toggle between full history and pinned-only view                                |
| `Esc`    | Close picker                                                                    |

### Actions and saved snippets

Press `Ctrl+Shift+P`, `Cmd+K` (where the terminal reports it), or run
`:actions` to open the contextual Actions palette. It exposes operations
relevant to the active Classic/Notebook editor, result, selection, or error,
including execution, exports, SQL generation, cell workflows, history, and
activity navigation.

Save a reusable query with `:snippet-save <name>`, browse/filter snippets with
`:snippets`, load one directly with `:snippet <name>`, and remove one with
`:snippet-delete <name>` (or `Ctrl-d` inside the picker). Snippets are stored in
the local query-history file and keep a sanitized connection hint.

### Troubleshooting keybindings

If a key combo isn't working in your terminal, you can inspect what `tsql` is actually receiving:

```bash
tsql --debug-keys
```

To also print mouse events:

```bash
tsql --debug-keys --mouse
```

### Commands

| Command                         | Description         |
| ------------------------------- | ------------------- |
| `:connect <url>`                | Connect to database |
| `:disconnect`                   | Disconnect          |
| `:ai [prompt]`                  | Open AI query assistant |
| `:export csv\|json\|tsv\|sql[:table] <path>` | Export selected rows or stream a full retained result |
| `:actions` / `:palette`        | Open contextual Actions palette |
| `:sort asc\|desc\|add-asc\|add-desc\|toggle` | Sort the focused Classic/PostgreSQL result |
| `:filter [#column\|name] eq\|ne\|<\|<=\|>\|>=\|contains\|not-contains\|null\|not-null [value]` | Filter the focused Classic/PostgreSQL result |
| `:columns` / `:group-count` | Choose result columns or group and count the current column |
| `:clear-filters` / `:clear-sort` / `:reset-result` | Clear or reset Classic result transformations |
| `:result-sql copy\|open` | Copy transformed SQL or open it in the editor |
| `:snippets` / `:snippet-save <name>` | Browse or save reusable query snippets |
| `:snippet <name>` / `:snippet-delete <name>` | Load or delete a saved snippet |
| `:update [check\|status\|apply]` | Check/apply updates |
| `:refresh`                      | Refresh focused schema or last query |
| `:notebook` / `:mode notebook` | Switch to Notebook workspace |
| `:mode classic`                | Switch to Classic workspace |
| `:rebase`                      | Rebind a dependent cell to its source's latest snapshot |
| `:rebind`                      | Allow the selected cell to run on the active connection |
| `:detach`                      | Remove the selected cell's previous result binding |
| `:run-without-snapshot`        | Explicitly retry a cell without retaining its result |
| `:run-all` / `:run-above` / `:run-below` | Run notebook cells in dependency order |
| `:run-dependents`              | Rerun the selected cell and its dependents |
| `:save-notebook <path>`        | Save a portable notebook document |
| `:open-notebook <path>`        | Open a portable notebook document |
| `:insert-above` / `:insert-below` / `:duplicate-cell` | Insert or duplicate a notebook cell |
| `:move-cell-up` / `:move-cell-down` | Reorder the selected notebook cell |
| `:name <identifier>` / `:name clear` | Set or clear a stable result name |
| `:outline` / `:cell <id-or-name>` | List notebook cells or jump to one |
| `:error` / `:copy-error`       | Inspect or copy the selected cell error |
| `:error-jump`                  | Move the editor cursor to a mapped PostgreSQL error |
| `:activity`                    | Jump to the latest off-screen cell update |
| `:cell-history` / `:cell-run <id>` | Browse cell runs or restore a previous source |
| `:explain-cell`                | Explain the selected PostgreSQL notebook cell |
| `:collapse-source` / `:expand-source` / `:toggle-source` | Compact or reveal the selected cell source |
| `:sbt` / `:sidebar-toggle`      | Toggle sidebar      |
| `:q` / `:quit`                  | Quit                |
| `:\dt`                          | List tables         |
| `:\d <table>`                   | Describe table      |
| `:\dn`                          | List schemas        |
| `:\di`                          | List indexes        |
| `:\l`                           | List databases      |
| `:\du`                          | List roles          |
| `:show dbs`                     | Mongo: list databases |
| `:show collections`             | Mongo: list collections |
| `:describe <collection>`        | Mongo: describe collection |
| `:use <database>`               | Mongo: switch database |

`:update apply` is only available in `updates.mode = "auto"` and only for
standalone binary installs.

## Configuration

tsql looks for configuration at `~/.tsql/config.toml` by default.
On Linux/macOS startup, legacy config folders are auto-migrated to `~/.tsql`.

```toml
[display]
# Built-ins: "one_dark" and "github_light". "default" maps to One Dark.
theme = "one_dark"

[connection]
# Default connection URL (can be overridden by CLI arg or DATABASE_URL)
default_url = "postgres://localhost/mydb"
# Enable 1Password CLI support for `password_onepassword` refs
enable_onepassword = false

[notebook]
startup = false
snapshot_mode = "auto" # use "off" with transaction/statement poolers
snapshot_max_rows = 2000
snapshot_max_bytes = 67108864
snapshot_total_bytes = 134217728
max_retained_snapshots = 8

[updates]
# Update checks + optional in-app apply for standalone installs
enabled = true
check_on_startup = true
channel = "stable"
mode = "auto"
interval_hours = 24
allow_apply_for_standalone = true
github_repo = "fcoury/tsql"

[ai]
# Enable AI query assistant (`:ai`, `Ctrl+G`)
enabled = false
# provider:
# - "open_ai"
# - "open_ai_compatible"
# - "ollama"
# - "anthropic"
# - "google"
# - "openrouter"
provider = "open_ai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
# base_url = "http://localhost:1234/v1"
# provider defaults when omitted:
# ANTHROPIC_API_KEY / GEMINI_API_KEY / OPENROUTER_API_KEY

[keymap]
# Custom keymap overrides (see config.example.toml for options)
```

See [config.example.toml](config.example.toml) for all available options.

### Custom themes

Custom themes use the same Helix-style TOML format as syntax highlighting and
can also define tsql's UI chrome. Place `<name>.toml` in
`~/.tsql/themes/`, then set `display.theme = "<name>"`. When
`TSQL_CONFIG_DIR` is set, tsql uses `$TSQL_CONFIG_DIR/themes/` instead.

Scope names containing dots must be quoted. A minimal theme can override only
the values it needs; missing UI fields inherit the One Dark fallback:

```toml
[palette]
foreground = "#d8dee9"
background = "#242933"
accent = "#88c0d0"

["ui.background"]
fg = "foreground"
bg = "background"

["ui.background.panel"]
bg = "#20242c"

["ui.background.elevated"]
bg = "#2e3440"

["ui.accent"]
fg = "accent"

[keyword]
fg = "accent"
```

Supported UI scope families are `ui.background`, `ui.text`, `ui.label`,
`ui.accent`, `ui.selection`, `ui.cursor`, `ui.search.match`, `ui.statusline`,
`ui.success`, `ui.warning`, `ui.error`, `ui.transaction`, `ui.overlay`
(modal surfaces, with `ui.overlay.border` and `ui.overlay.title`),
`ui.scrollbar`, and `ui.grid.header`. A missing, unreadable, or malformed
custom theme falls back to One Dark and reports a nonfatal startup warning.

### 1Password integration

1Password support is currently gated behind `connection.enable_onepassword = true`
in your config.

Connection entries support an optional **1Password ref** field
(`op://vault/item/field`). When enabled, `tsql` calls `op read` at connect time
to resolve the password, inheriting your shell's `PATH` and active `op` session
token. Configure it via the connection manager (`Ctrl+Shift+C` or `gm`).

Requires the 1Password CLI (`op`) to be installed and an active authenticated
session (for example via `op signin`).

## Requirements

- PostgreSQL 12 or later, or MongoDB 6.0+
- Terminal with 256-color support recommended

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
