# tsql

A modern, keyboard-first PostgreSQL CLI with a TUI interface.

[![CI](https://github.com/fcoury/tsql/actions/workflows/ci.yml/badge.svg)](https://github.com/fcoury/tsql/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/tsql.svg)](https://crates.io/crates/tsql)
[![License](https://img.shields.io/crates/l/tsql.svg)](LICENSE)
[![Discord](https://img.shields.io/discord/1204152891049512960)(https://discord.gg/b928dKDcQq)

![tsql screenshot](assets/screenshot.png)

[Join us on Discord](https://discord.gg/b928dKDcQq)

## Features

- **Full-screen TUI** - Split-pane interface with query editor and results grid
- **Vim-style keybindings** - Navigate and edit with familiar modal commands
- **Syntax highlighting** - SQL and JSON highlighting powered by tree-sitter
- **Smart completion** - Schema-aware autocomplete for tables, columns, and keywords
- **Results grid** - Scrollable, searchable data grid with column resizing
- **Inline editing** - Edit cells directly in the grid with automatic SQL generation
- **JSON support** - Detect, format, and edit JSON/JSONB columns with syntax highlighting
- **psql compatibility** - Familiar commands like `\dt`, `\d`, `\dn`, `\l`, and more
- **Query history** - Persistent history with fuzzy search
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

# Or set DATABASE_URL environment variable
export DATABASE_URL=postgres://user:password@localhost:5432/mydb
tsql

# Or configure a default connection in ~/.config/tsql/config.toml
tsql
```

Once connected:

1. Type a SQL query in the editor pane
2. Press `Enter` to execute
3. Use `Tab` to switch between editor and results grid
4. Press `?` for help with all keybindings

## Keybindings

### Global

| Key   | Action                                             |
| ----- | -------------------------------------------------- |
| `Tab` | Switch focus between query editor and results grid |
| `?`   | Toggle help popup                                  |
| `Ctrl+Shift+B` / `Ctrl+\` / `Ctrl+4` | Toggle sidebar                  |
| `q`   | Quit application                                   |
| `Esc` | Return to normal mode / close popups               |

### Query Editor (Normal Mode)

| Key       | Action                |
| --------- | --------------------- |
| `h/j/k/l` | Move cursor           |
| `i/a/I/A` | Enter insert mode     |
| `o/O`     | Open line below/above |
| `dd`      | Delete line           |
| `yy`      | Yank (copy) line      |
| `p/P`     | Paste after/before    |
| `u`       | Undo                  |
| `Ctrl-r`  | Redo                  |
| `/`       | Search                |
| `Enter`   | Execute query         |
| `:`       | Command mode          |

### Results Grid

| Key       | Action               |
| --------- | -------------------- |
| `h/j/k/l` | Navigate cells       |
| `H/L`     | Scroll horizontally  |
| `gg/G`    | First/last row       |
| `Space`   | Toggle row selection |
| `y`       | Copy row             |
| `c`       | Copy cell            |
| `e`       | Edit cell            |
| `/`       | Search in results    |
| `+/-`     | Widen/narrow column  |
| `=`       | Fit/collapse column  |

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
| `:export csv\|json\|tsv <path>` | Export results      |
| `:q` / `:quit`                  | Quit                |
| `:\dt`                          | List tables         |
| `:\d <table>`                   | Describe table      |
| `:\dn`                          | List schemas        |
| `:\di`                          | List indexes        |
| `:\l`                           | List databases      |
| `:\du`                          | List roles          |

## Configuration

tsql looks for configuration at `~/.config/tsql/config.toml`.

```toml
[connection]
# Default connection URL (can be overridden by CLI arg or DATABASE_URL)
default_url = "postgres://localhost/mydb"

[keybindings]
# Custom keybindings (see config.example.toml for options)
```

See [config.example.toml](config.example.toml) for all available options.

## Requirements

- PostgreSQL 12 or later
- Terminal with 256-color support recommended

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
