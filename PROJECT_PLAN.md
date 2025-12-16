# tsql: A Better psql (Rust)

This document is the initial project plan for building a modern, keyboard-first Postgres CLI inspired by the terminal UX of the Codex CLI project.

Working title: `tsql` (rename later if desired).

## Vision

Make interacting with Postgres feel like using a great terminal-native tool:

- Fast feedback, clear state, and predictable output.
- Keyboard-first, modal workflows (Vim-like) without sacrificing discoverability.
- Results feel like a real data UI: scrollable, selectable, editable.

## UX Principles (Codex-inspired)

Codex CLI’s “terminal interaction” inspiration translated to a DB client:

- **State is always visible**: connection info, mode, transaction status, row counts.
- **Progressive disclosure**: minimal noise by default; expand details on demand.
- **Safe by default**: destructive actions are explicit and confirmable.
- **Great ergonomics**: consistent keybindings, command palette, inline help.
- **Config-first**: a single config file with sane defaults, easy overrides.

## Core Features (Target)

### 1) Query input experience

- Autocompletion:
  - SQL keywords / functions
  - Schema-aware completion (tables, columns, schemas)
  - Context-aware where possible (e.g., after `FROM` propose relations)
- History:
  - Incremental search
  - Edit previous multi-line queries
  - Named snippets / favorites
- Syntax highlighting:
  - SQL highlighting (Postgres flavor)
  - Highlight errors/warnings when parser can detect them

### 2) Result viewer (data grid)

- Beautiful tables with:
  - Horizontal + vertical scrolling
  - Optional **frozen header**
  - Column width controls (auto-fit, min/max, wrap off)
  - Search within results
  - Copy cell/row/selection to clipboard
- Avoid wrapping by default; support truncation and horizontal scroll.
- “Pager-like” feel: scrolling only affects the results pane, not the prompt.

### 3) Selection, actions, editing

- Select one or many records/rows.
- Actions on selection (initial set):
  - Copy as SQL/CSV/JSON
  - Open row in a detail view
  - Generate `UPDATE`/`DELETE` templates
- Inline editing:
  - Edit a cell (or entire row) inside the grid
  - Commit changes back to DB (with clear constraints; see “Row identity”)

### 4) Vim-like navigation and modes

- Modes:
  - **Normal**: navigate panes, run commands, move cursor
  - **Select**: mark rows/cells/ranges
  - **Edit**: edit query text or a cell value
- Familiar keys:
  - `hjkl`, `gg/G`, `Ctrl-d/Ctrl-u`, `/` search, `n/N` next match
  - `:` for command palette / command line

## Non-Goals (At First)

- Full compatibility with every `psql` meta-command.
- Supporting multiple DB engines (start with Postgres).
- A full SQL IDE (this is a terminal tool).

## Interaction Model (Proposed)

The app runs as a full-screen TUI with two primary panes:

- **Query pane** (top): multi-line editor with completion and highlighting.
- **Results pane** (bottom): grid viewer with scrolling, selection, and actions.

A status line (or two) is always visible:

- Current connection (`host/db/user`), transaction state, search state.
- Mode indicator (`NORMAL | SELECT | EDIT`), plus hints for next actions.

## Commands and Meta-Commands

Start with a small set (expand later):

- `:connect <connstr>` or `:connect` (interactive)
- `:q` / `:quit`
- `:help` (keybindings + commands)
- `:history`
- `:timing on|off`
- `:export csv|json <path>` (export last results)

Optional `psql`-style backslash aliases can be added later:

- `\dt`, `\d <table>`, `\dn`, `\x`, `\pset`

## Architecture Plan (Rust)

### High-level components

- `app`: high-level state machine (modes, focus, routing)
- `ui`: rendering (ratatui widgets + custom grid)
- `input`: editor model (cursor, selection, history, completion)
- `db`: connection pool/session, query execution, cancellation
- `sql`: parsing, tokenization, highlighting, completion context
- `actions`: operations on selected rows/cells
- `config`: config file + keymap definitions

### Concurrency model

- Use `tokio` as runtime.
- UI runs an event loop consuming:
  - terminal events (keys, resize)
  - async DB events (query results arriving, query finished, errors)
- Queries should support cancellation (e.g., `Esc` or `Ctrl-c`).

### Result handling

- For small results: buffer fully.
- For large results:
  - stream rows and keep a sliding window / virtualized model
  - provide a “max rows” safety and a “fetch more” mechanism

### Row identity (required for safe editing/actions)

Inline editing and “actions on selected rows” require a stable way to identify rows.

Options:

- Prefer: require presence of a primary key (or unique key) in the result set.
- Alternative: let the user configure key columns for a query.
- Postgres-specific fallback: `ctid` (powerful but can be surprising; document clearly).

Initial plan: implement selection/copy first; add editing only after identity rules are solid.

## Dependency Candidates (Decision Needed)

Terminal and UI:

- `crossterm` for terminal IO/events.
- `ratatui` for layout and widgets.
- Custom grid widget for frozen headers + horizontal scroll.

Editor/history/completion:

Two viable approaches:

1. **Full TUI editor** (recommended for unified modes)
   - `tui-textarea` (or equivalent) for multi-line editing
   - implement vi-like modes at the app layer

2. **Integrate a line editor** (great completion/history; harder to embed)
   - `reedline` for vi mode, history, completion, hints
   - may be tricky to embed inside a full-screen ratatui app; worth a spike

SQL parsing/highlighting:

- `tree-sitter` + a SQL grammar (ideally Postgres flavored)
- Fallback/simple: keyword-based tokenizer for early MVP

Postgres connectivity:

- `tokio-postgres` (direct, lightweight) or `sqlx` (heavier but broad)

Config:

- `toml` + `serde` for `~/.config/tsql/config.toml` (or platform-appropriate)

## Milestones

### M0: Spikes (de-risk the hard parts)

- Prototype a scrollable grid with:
  - frozen header
  - horizontal scroll
  - row cursor + multi-select
- Prototype editor integration:
  - multi-line input
  - history recall
  - vi-like mode transitions

Exit criteria:

- Clear choice of editor approach (TUI-native vs reedline).
- Grid architecture proven with acceptable performance.

### M1: MVP CLI (no full-screen UI yet)

- Connect to Postgres via conn string / env vars.
- Run a query and print a nice table to stdout.
- Basic history persisted to disk.

Exit criteria:

- Useful as a simple `psql` replacement for running queries.

### M2: Full-screen TUI shell

- Two-pane UI (query + results).
- Execute query, show streaming progress, render result grid.
- Cancel running queries.

Exit criteria:

- Stable interactive experience.

### M3: Completion + highlighting

- SQL syntax highlighting in editor.
- Completion for keywords + schema introspection.
- History search and editing polish.

### M4: Grid polish

- Column sizing controls.
- Search within results.
- Copy/export flows.

### M5: Selection actions

- Multi-select rows/cells.
- Actions palette:
  - copy as CSV/JSON
  - generate SQL templates

### M6: Inline editing

- Cell editor overlay.
- Validate and commit updates.
- Guardrails: row identity requirements and confirmations.

### M7: Vim modes + keymap config

- Refine Normal/Select/Edit mode semantics.
- Configurable keymaps.

### M8: Packaging and quality

- Integration tests against Postgres.
- Release workflow (brew, cargo install, etc.).

## Testing Strategy

- Unit tests:
  - keybinding/mode transitions
  - grid scrolling math (viewport/offset)
  - SQL tokenization/highlighting correctness (basic)
- Integration tests:
  - connect + run query
  - cancellation
  - schema introspection for completion

## Risks / Open Questions

- Embedding a rich line editor inside a full-screen TUI (if using reedline).
- Tree-sitter SQL grammar quality for Postgres-specific syntax.
- Large result sets: memory, performance, and UX.
- Inline editing semantics and safety (identity, transactions, concurrency).

## Proposed Repository Layout (When We Start Implementing)

- `src/main.rs` (bootstrap)
- `src/app/mod.rs` (state machine)
- `src/ui/mod.rs` (ratatui rendering)
- `src/db/mod.rs` (tokio-postgres session)
- `src/sql/mod.rs` (highlighting, completion context)
- `src/config/mod.rs` (config + keymaps)

## Next Step

Pick the first spike:

- A) Grid spike (frozen header + scroll + selection)
- B) Editor spike (multi-line + vi modes + history)

If you tell me which to start with, I can begin implementing it in this crate.
