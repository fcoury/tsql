# Contributing to tsql

Thank you for your interest in contributing to tsql! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- **Rust 1.80 or later** - Install via [rustup](https://rustup.rs/)
- **PostgreSQL** - For running integration tests (optional but recommended)

### Clone and Build

```bash
git clone https://github.com/fcoury/tsql.git
cd tsql
cargo build
```

### Running the Application

```bash
# With a connection URL
cargo run -- postgres://localhost/mydb

# Or set DATABASE_URL
export DATABASE_URL=postgres://localhost/mydb
cargo run
```

## Development Workflow

### Running Tests

```bash
# Run unit tests only (no database required)
cargo test --lib --bins

# Run all tests including integration tests (requires PostgreSQL)
export TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres
cargo test
```

### Code Quality

Before submitting a PR, ensure your code passes all checks:

```bash
# Format code
cargo fmt

# Run linter
cargo clippy -- -D warnings

# Run all tests
cargo test
```

### Project Structure

```
tsql/
├── crates/
│   ├── tsql/              # Main application
│   │   └── src/
│   │       ├── app/       # Application state and logic
│   │       ├── config/    # Configuration and keymaps
│   │       ├── ui/        # UI components (grid, editor, popups)
│   │       ├── history.rs # Query history
│   │       ├── util.rs    # Utility functions
│   │       ├── lib.rs     # Library exports
│   │       └── main.rs    # Entry point
│   └── tui-syntax/        # Syntax highlighting library
│       └── src/
│           ├── languages/ # Language configurations
│           ├── themes/    # Color themes
│           └── ...
├── tests/                 # Integration tests
├── assets/                # Screenshots and images
└── docs/                  # Documentation
```

## Coding Guidelines

### Code Style

- Follow Rust idioms and best practices
- Use `rustfmt` for formatting (default settings)
- Address all `clippy` warnings
- Write descriptive commit messages

### Documentation

- Add doc comments to public APIs
- Update README.md for user-facing changes
- Include inline comments for complex logic

### Testing

- Write unit tests for new functionality
- Add integration tests for database-related features
- Ensure existing tests pass before submitting

## Pull Request Process

1. **Fork the repository** and create a feature branch
   ```bash
   git checkout -b feature/my-feature
   ```

2. **Make your changes** with clear, focused commits

3. **Run all checks** before pushing
   ```bash
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```

4. **Push your branch** and create a Pull Request

5. **Describe your changes** in the PR description:
   - What problem does this solve?
   - How did you solve it?
   - Any breaking changes?

6. **Address review feedback** promptly

### PR Guidelines

- Keep PRs focused on a single feature or fix
- Include tests for new functionality
- Update documentation as needed
- Ensure CI passes before requesting review

## Issue Guidelines

### Reporting Bugs

When reporting bugs, please include:

- tsql version (`tsql --version`)
- Operating system and version
- PostgreSQL version
- Steps to reproduce
- Expected vs actual behavior
- Error messages or screenshots

### Feature Requests

For feature requests, please describe:

- The use case or problem you're trying to solve
- Your proposed solution (if any)
- Any alternatives you've considered

## Code of Conduct

- Be respectful and inclusive
- Provide constructive feedback
- Focus on the code, not the person
- Help others learn and grow

## Questions?

If you have questions, feel free to:

- Open a [GitHub Issue](https://github.com/fcoury/tsql/issues)
- Start a [GitHub Discussion](https://github.com/fcoury/tsql/discussions)

Thank you for contributing!
