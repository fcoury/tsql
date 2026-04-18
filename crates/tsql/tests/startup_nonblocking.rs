//! Static regression coverage for startup paths that must not block first draw.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn main_does_not_resolve_saved_connection_passwords() {
    let main_rs = fs::read_to_string(crate_root().join("src/main.rs")).unwrap();

    assert!(
        !main_rs.contains("get_password_with_timeout"),
        "startup password lookup belongs in App after the first draw"
    );
    assert!(main_rs.contains("--safe-mode"));
    assert!(main_rs.contains("set_safe_mode"));
    assert!(main_rs.contains("set_pending_startup_reconnect"));
}

#[test]
fn app_resolves_startup_passwords_via_background_event() {
    let app_rs = fs::read_to_string(crate_root().join("src/app/app.rs")).unwrap();

    assert!(app_rs.contains("PasswordResolveReason::Startup"));
    assert!(app_rs.contains("DbEvent::PasswordResolved"));
    assert!(app_rs.contains("dispatch_pending_startup_reconnect"));
    assert!(app_rs.contains("spawn_blocking"));
}
