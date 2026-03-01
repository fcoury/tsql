use std::path::Path;

use super::types::InstallMethod;

pub fn detect_install_method(path: &Path) -> InstallMethod {
    let normalized = path.to_string_lossy().to_ascii_lowercase();

    if normalized.contains("/cellar/tsql/") || normalized.contains("/homebrew/") {
        InstallMethod::Homebrew
    } else if normalized.contains("/.cargo/bin/") {
        InstallMethod::CargoInstall
    } else if normalized.starts_with("/usr/bin/")
        || normalized.starts_with("/usr/local/bin/")
        || normalized.contains("/snap/")
    {
        InstallMethod::SystemPackage
    } else if path.is_absolute() {
        InstallMethod::StandaloneBinary
    } else {
        InstallMethod::Unknown
    }
}

pub fn detect_current_install_method() -> InstallMethod {
    std::env::current_exe()
        .ok()
        .map(|path| detect_install_method(&path))
        .unwrap_or(InstallMethod::Unknown)
}

pub fn upgrade_hint(method: InstallMethod) -> Option<&'static str> {
    match method {
        InstallMethod::Homebrew => Some("brew upgrade tsql"),
        InstallMethod::CargoInstall => Some("cargo install --locked tsql"),
        InstallMethod::SystemPackage => None,
        InstallMethod::StandaloneBinary => None,
        InstallMethod::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_install_method_homebrew() {
        let method = detect_install_method(Path::new("/opt/homebrew/Cellar/tsql/0.4.2/bin/tsql"));
        assert_eq!(method, InstallMethod::Homebrew);
    }

    #[test]
    fn test_detect_install_method_cargo_install() {
        let method = detect_install_method(Path::new("/Users/alice/.cargo/bin/tsql"));
        assert_eq!(method, InstallMethod::CargoInstall);
    }

    #[test]
    fn test_detect_install_method_system_package() {
        let method = detect_install_method(Path::new("/usr/bin/tsql"));
        assert_eq!(method, InstallMethod::SystemPackage);
    }

    #[test]
    fn test_upgrade_hint_for_homebrew() {
        assert_eq!(
            upgrade_hint(InstallMethod::Homebrew),
            Some("brew upgrade tsql")
        );
    }
}
