# Repository Guidelines

## Project Structure & Module Organization

- `crates/tsql/`: main application crate (TUI + PostgreSQL client)
  - `crates/tsql/src/app/`: application state and orchestration
  - `crates/tsql/src/ui/`: UI widgets (grid, editor, popups)
  - `crates/tsql/src/config/`: config + keybinding schema
  - `crates/tsql/src/vim/`: modal editing primitives
- `crates/tui-syntax/`: tree-sitter powered highlighting (languages/themes)
- `crates/tsql/tests/`: integration tests (database-backed)
- `assets/`: screenshots/images, `docs/`: release docs, `vendor/`: git submodules

## Build, Test, and Development Commands

- Bootstrap submodules: `git submodule update --init --recursive`
- Build (debug/release): `cargo build` / `cargo build --release`
- Run locally: `cargo run -p tsql -- postgres://user:pass@localhost:5432/db`
  - Alternative: `DATABASE_URL=postgres://… cargo run -p tsql`
- Unit tests (no DB): `cargo test --lib --bins`
- Integration tests (requires Postgres):  
  `TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo test -p tsql --test integration_tests`
- Format: `cargo fmt --all`
- Lint (treat warnings as errors): `cargo clippy --all --all-targets -- -D warnings`

## Coding Style & Naming Conventions

- Rust toolchain: Rust **1.80+** (see workspace `rust-version`).
- Formatting: `rustfmt` (default settings) is the source of truth.
- Naming: `snake_case` (fns/modules), `CamelCase` (types/traits), `SCREAMING_SNAKE_CASE` (consts).
- Keep changes focused; prefer small, composable helpers over large UI/app methods.

## Testing Guidelines

- Prefer unit tests colocated with code (`#[cfg(test)]`) for pure logic.
- Put DB-dependent scenarios in `crates/tsql/tests/` and gate them via `TEST_DATABASE_URL`.

## Commit & Pull Request Guidelines

- Commit messages follow the prefixes used in history: `feat:`, `fix:`, `refactor:`, `chore:`, `style:`.
- PRs: include a clear description, verification steps, and screenshots/GIFs for UI changes.
- Before committing: run `cargo fmt --all`. Before pushing: run clippy + tests.

## Automation & Hooks (Optional)

- CI enforces formatting, clippy, unit tests, and Postgres-backed integration tests.
- Optional git-hook configuration lives in `.hookman/` and calls `scripts/` checks.
