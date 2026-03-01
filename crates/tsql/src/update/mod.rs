pub mod checker;
pub mod detect;
pub mod provider;
pub mod state;
pub mod types;

pub use checker::check_for_update;
pub use detect::{detect_current_install_method, detect_install_method, upgrade_hint};
pub use provider::{GitHubReleasesProvider, ReleaseProvider};
pub use state::UpdateState;
pub use types::{InstallMethod, UpdateCheckOutcome, UpdateInfo, UpdatePolicy};
