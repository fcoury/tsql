use semver::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    CargoInstall,
    SystemPackage,
    StandaloneBinary,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePolicy {
    Off,
    NotifyOnly,
    ApplyAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current: Version,
    pub latest: Version,
    pub notes_url: Option<String>,
    pub asset_url: Option<String>,
    pub checksum_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckOutcome {
    UpToDate { current: Version },
    UpdateAvailable(UpdateInfo),
    Error(String),
    Disabled,
}
