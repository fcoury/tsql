//! Unified Vim keybinding module for all editors.
//!
//! This module provides a consistent vim-like editing experience across
//! all text editors in the application (query editor, JSON editor, etc.).
//!
//! # Architecture
//!
//! - `VimMode`: The current editing mode (Normal, Insert, Visual)
//! - `VimCommand`: High-level vim commands that can be executed
//! - `VimHandler`: Processes key events and returns commands to execute
//!
//! # Usage
//!
//! ```ignore
//! let mut handler = VimHandler::new(VimConfig::default());
//! let command = handler.handle_key(key_event, mode);
//! match command {
//!     VimCommand::Move(movement) => textarea.move_cursor(movement),
//!     VimCommand::EnterInsert => mode = VimMode::Insert,
//!     // ...
//! }
//! ```

mod command;
mod handler;
mod mode;

pub use command::{Motion, Operator, TextObject, VimCommand};
pub use handler::{VimConfig, VimHandler};
pub use mode::VimMode;
