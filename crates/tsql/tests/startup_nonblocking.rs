//! Regression tests for the macOS "app hangs on startup" issue.
//!
//! The original bug was in `main.rs`: before the first terminal draw,
//! startup would synchronously call `ConnectionEntry::get_password_with_timeout_and_options`,
//! which in turn did a `thread::spawn + recv_timeout`. On macOS the first
//! keychain access can block for 500ms-5s, and with a 1Password entry it
//! blocked up to 5s, all while the terminal was already in alt-screen /
//! raw mode — so the user only saw a frozen black screen.
//!
//! The fix moves that work off of the startup path: `main.rs` merely
//! queues a `PendingStartupReconnect` on the `App` struct, and the
//! actual password resolve happens on a tokio `spawn_blocking` task
//! AFTER the first draw. These tests ensure that:
//!
//! 1. `main.rs` does not reintroduce the blocking call on the pre-draw
//!    code path. (Source-level scan.)
//! 2. `--safe-mode` / `--no-auto-connect` flags exist and are wired up.

use std::fs;

#[test]
fn main_rs_does_not_call_get_password_before_first_draw() {
    let main_src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read main.rs");
    assert!(
        !main_src.contains("get_password_with_timeout_and_options"),
        "main.rs must never call get_password_with_timeout_and_options synchronously \
         on the startup path; it would block before the first draw and look like a \
         frozen app on macOS. Use App::set_pending_startup_reconnect instead.",
    );
    assert!(
        !main_src.contains("get_password_from_keychain"),
        "main.rs must never call get_password_from_keychain synchronously on \
         startup either — same reason.",
    );
}

#[test]
fn main_rs_exposes_safe_mode_flag() {
    let main_src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read main.rs");
    assert!(
        main_src.contains("--safe-mode") || main_src.contains("--no-auto-connect"),
        "--safe-mode must exist as a CLI flag so users can always recover a wedged \
         startup: `tsql --safe-mode`.",
    );
    assert!(
        main_src.contains("set_safe_mode"),
        "main.rs must propagate the safe-mode decision into the App via set_safe_mode",
    );
}
