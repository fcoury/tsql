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
- **Vim-style keybindings** - Navigate and edit with familiar modal commands
- **Syntax highlighting** - SQL and JSON highlighting powered by tree-sitter
- **Smart completion** - Schema-aware autocomplete for tables, columns, and keywords
- **Results grid** - Scrollable, searchable data grid with column resizing, multi-row selection, and flexible yank (TSV/CSV/JSON/Markdown)
- **Inline editing** - Edit cells directly in the grid with automatic SQL generation
- **JSON support** - Detect, format, and edit JSON/JSONB columns with syntax highlighting
- **Postgres + MongoDB + SQLite** - Connect with `postgres://...`, `mongodb://...`, or `sqlite://...` URLs
- **Schema commands** - `psql`-style commands plus Mongo helpers (`:show dbs`, `:show collections`, `:describe`) and SQLite metadata
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
tsql sqlite:///path/to/app.db
tsql 'sqlite://app.db?mode=rwc' # create if missing

# Or set DATABASE_URL environment variable
export DATABASE_URL=postgres://user:password@localhost:5432/mydb
tsql

# Or configure a default connection in ~/.tsql/config.toml
tsql
```

Once connected:

1. Type a SQL query in the editor pane
2. Press `Enter` to execute
3. Use `Tab` to switch between editor and results grid
4. Press `?` for help with all keybindings (type `/` inside the help popup to filter)

## Keybindings

### Global

| Key                                  | Action                                             |
| ------------------------------------ | -------------------------------------------------- |
| `Tab`                                | Switch focus between query editor and results grid |
| `?`                                  | Toggle help popup (`/` to filter inside)           |
| `Ctrl+Shift+B` / `Ctrl+\` / `Ctrl+4` | Toggle sidebar                                     |
| `Ctrl+O`                             | Open connection picker                             |
| `Ctrl+Shift+C` / `gm`                | Open connection manager                            |
| `q`                                  | Quit application                                   |
| `Esc`                                | Return to normal mode / close popups               |

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

Yank commands operate on all selected rows when a selection is active, or the cursor row otherwise.

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
| `:export csv\|json\|tsv <path>` | Export results      |
| `:update [check\|status\|apply]` | Check/apply updates |
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
[connection]
# Default connection URL (can be overridden by CLI arg or DATABASE_URL)
default_url = "postgres://localhost/mydb"
# SQLite opens existing files by default. Add ?mode=rwc to create a file.
# default_url = "sqlite:///path/to/app.db"
# Enable 1Password CLI support for `password_onepassword` refs
enable_onepassword = false

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
