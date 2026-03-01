pub mod apply;
pub mod checker;
pub mod detect;
pub mod provider;
pub mod state;
pub mod types;

pub use apply::{apply_update, ApplyResult};
pub use checker::check_for_update;
pub use detect::{
    current_target_triple, detect_current_install_method, detect_install_method, upgrade_hint,
};
pub use provider::{GitHubReleasesProvider, ReleaseProvider};
pub use state::UpdateState;
pub use types::{InstallMethod, UpdateCheckOutcome, UpdateInfo, UpdatePolicy};
