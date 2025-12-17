use crate::config::{ClipboardBackend, ClipboardConfig};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBackendChoice {
    Disabled,
    Arboard,
    WlCopy { cmd: PathBuf },
}

pub fn choose_backend(cfg: &ClipboardConfig) -> Result<ClipboardBackendChoice> {
    match cfg.backend {
        ClipboardBackend::Disabled => Ok(ClipboardBackendChoice::Disabled),
        ClipboardBackend::Arboard => Ok(ClipboardBackendChoice::Arboard),
        ClipboardBackend::WlCopy => {
            let cmd = find_in_path(&cfg.wl_copy_cmd).ok_or_else(|| {
                anyhow!(
                    "Clipboard backend wl-copy selected, but '{}' was not found on PATH",
                    cfg.wl_copy_cmd
                )
            })?;
            Ok(ClipboardBackendChoice::WlCopy { cmd })
        }
        ClipboardBackend::Auto => {
            if cfg!(target_os = "linux") && is_wayland_session() {
                if let Some(cmd) = find_in_path(&cfg.wl_copy_cmd) {
                    return Ok(ClipboardBackendChoice::WlCopy { cmd });
                }
            }
            Ok(ClipboardBackendChoice::Arboard)
        }
    }
}

pub fn copy_with_wl_copy(text: &str, cfg: &ClipboardConfig, cmd: &Path) -> Result<()> {
    let mut command = Command::new(cmd);

    if cfg.wl_copy_primary {
        command.arg("-p");
    }
    if cfg.wl_copy_trim_newline {
        command.arg("-n");
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to start wl-copy: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow!("Failed to write to wl-copy stdin: {}", e))?;
    }

    // `wl-copy` may keep running to serve clipboard requests (selection ownership).
    // It typically forks into the background, but we should never block the TUI waiting for it.
    // We only wait briefly to catch immediate failures; otherwise we detach and reap in a thread
    // to avoid zombie processes once it exits.
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child
            .try_wait()
            .map_err(|e| anyhow!("Failed to check wl-copy status: {}", e))?
        {
            Some(status) => {
                if status.success() {
                    return Ok(());
                }

                // Only attempt to read stderr when failing. If `wl-copy` forks,
                // the background process may inherit the stderr pipe and keep it
                // open, so reading on success can block indefinitely.
                let mut stderr_bytes = Vec::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use std::io::Read;
                    let _ = stderr.read_to_end(&mut stderr_bytes);
                }

                let stderr = String::from_utf8_lossy(&stderr_bytes);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    return Err(anyhow!("wl-copy failed with exit status {}", status));
                }
                return Err(anyhow!("wl-copy failed: {}", stderr));
            }
            None => {
                if Instant::now() >= deadline {
                    // Already returning success; spawn thread just to reap and prevent zombie.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn is_wayland_session() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }

    match std::env::var("XDG_SESSION_TYPE") {
        Ok(v) => v.eq_ignore_ascii_case("wayland"),
        Err(_) => false,
    }
}

fn find_in_path(cmd: &str) -> Option<PathBuf> {
    let cmd_path = Path::new(cmd);
    // Check if cmd looks like a path (contains separator). On Unix, backslash is a
    // valid filename character, so only check forward slash there.
    #[cfg(windows)]
    let has_separator = cmd.contains('/') || cmd.contains('\\');
    #[cfg(not(windows))]
    let has_separator = cmd.contains('/');

    if has_separator {
        return is_executable_file(cmd_path).then(|| cmd_path.to_path_buf());
    }

    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(target_os = "linux")]
    use serial_test::serial;

    fn base_cfg() -> ClipboardConfig {
        ClipboardConfig {
            backend: ClipboardBackend::Auto,
            wl_copy_cmd: "wl-copy".to_string(),
            wl_copy_primary: false,
            wl_copy_trim_newline: false,
        }
    }

    #[test]
    fn forced_wl_copy_errors_when_missing() {
        let mut cfg = base_cfg();
        cfg.backend = ClipboardBackend::WlCopy;
        cfg.wl_copy_cmd = "definitely-not-a-real-wl-copy-binary".to_string();

        let err = choose_backend(&cfg).unwrap_err().to_string();
        assert!(err.contains("wl-copy selected"));
        assert!(err.contains("not found"));
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &str) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        let mut perms = file.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn copy_with_wl_copy_surfaces_stderr_on_failure() {
        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("wl-copy");
        // Must read stdin before exiting to avoid EPIPE race on fast Linux systems
        write_executable(&fake, "#!/bin/sh\ncat >/dev/null\necho boom 1>&2\nexit 1\n");

        let cfg = base_cfg();
        let err = copy_with_wl_copy("hello", &cfg, &fake)
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom"));
    }

    #[test]
    #[cfg(unix)]
    fn copy_with_wl_copy_ok_on_success() {
        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("wl-copy");
        write_executable(&fake, "#!/bin/sh\ncat >/dev/null\nexit 0\n");

        let cfg = base_cfg();
        copy_with_wl_copy("hello", &cfg, &fake).unwrap();
    }

    #[test]
    #[serial]
    #[cfg(target_os = "linux")]
    fn auto_selects_wl_copy_when_wayland_and_present() {
        struct EnvGuard {
            path: Option<std::ffi::OsString>,
            wayland: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match self.path.take() {
                    Some(v) => std::env::set_var("PATH", v),
                    None => std::env::remove_var("PATH"),
                }
                match self.wayland.take() {
                    Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                    None => std::env::remove_var("WAYLAND_DISPLAY"),
                }
            }
        }

        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("wl-copy");
        write_executable(&fake, "#!/bin/sh\ncat >/dev/null\nexit 0\n");

        let _guard = EnvGuard {
            path: std::env::var_os("PATH"),
            wayland: std::env::var_os("WAYLAND_DISPLAY"),
        };

        std::env::set_var("PATH", dir.path().as_os_str());
        std::env::set_var("WAYLAND_DISPLAY", "wayland-1");

        let cfg = base_cfg();
        let choice = choose_backend(&cfg).unwrap();
        assert!(matches!(choice, ClipboardBackendChoice::WlCopy { .. }));
    }
}
