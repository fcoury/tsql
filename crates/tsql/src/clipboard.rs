use crate::config::{ClipboardBackend, ClipboardConfig};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("Failed to wait for wl-copy: {}", e))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        return Err(anyhow!("wl-copy failed with exit status {}", output.status));
    }
    Err(anyhow!("wl-copy failed: {}", stderr))
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
    if cmd.contains('/') || cmd.contains('\\') {
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
        write_executable(&fake, "#!/bin/sh\necho boom 1>&2\nexit 1\n");

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
        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("wl-copy");
        write_executable(&fake, "#!/bin/sh\ncat >/dev/null\nexit 0\n");

        let old_path = std::env::var_os("PATH");
        let old_wayland = std::env::var_os("WAYLAND_DISPLAY");

        std::env::set_var("PATH", dir.path().as_os_str());
        std::env::set_var("WAYLAND_DISPLAY", "wayland-1");

        let cfg = base_cfg();
        let choice = choose_backend(&cfg).unwrap();
        assert!(matches!(choice, ClipboardBackendChoice::WlCopy { .. }));

        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
    }
}
