use std::time::{Duration, Instant};

use crate::config::{UpdateMode, UpdatesConfig};

use super::types::{InstallMethod, UpdateCheckOutcome, UpdatePolicy};

#[derive(Debug, Clone, Default)]
pub struct UpdateState {
    pub startup_check_started: bool,
    pub check_in_flight: bool,
    pub last_checked_at: Option<Instant>,
    pub last_outcome: Option<UpdateCheckOutcome>,
}

impl UpdateState {
    pub fn policy(config: &UpdatesConfig) -> UpdatePolicy {
        if !config.enabled || matches!(config.mode, UpdateMode::Off) {
            return UpdatePolicy::Off;
        }

        UpdatePolicy::NotifyOnly
    }

    pub fn should_check_on_startup(&self, config: &UpdatesConfig) -> bool {
        if self.check_in_flight || self.startup_check_started {
            return false;
        }

        config.check_on_startup && !matches!(Self::policy(config), UpdatePolicy::Off)
    }

    pub fn should_check_by_interval(&self, config: &UpdatesConfig, now: Instant) -> bool {
        if self.check_in_flight || matches!(Self::policy(config), UpdatePolicy::Off) {
            return false;
        }

        let interval = Duration::from_secs(config.interval_hours.saturating_mul(3600));

        match self.last_checked_at {
            None => true,
            Some(last_checked_at) => now.saturating_duration_since(last_checked_at) >= interval,
        }
    }

    pub fn apply_allowed(config: &UpdatesConfig, install_method: InstallMethod) -> bool {
        !cfg!(windows)
            && config.enabled
            && matches!(config.mode, UpdateMode::Auto)
            && config.allow_apply_for_standalone
            && matches!(install_method, InstallMethod::StandaloneBinary)
    }

    pub fn mark_startup_skipped(&mut self, now: Instant) {
        if self.startup_check_started {
            return;
        }
        self.startup_check_started = true;
        if self.last_checked_at.is_none() {
            self.last_checked_at = Some(now);
        }
    }

    pub fn mark_check_started(&mut self, from_startup: bool) {
        self.check_in_flight = true;
        if from_startup {
            self.startup_check_started = true;
        }
    }

    pub fn mark_check_finished(&mut self, outcome: UpdateCheckOutcome) {
        self.check_in_flight = false;
        self.last_checked_at = Some(Instant::now());
        self.last_outcome = Some(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{UpdateChannel, UpdatesConfig};

    #[test]
    fn test_policy_is_off_when_disabled() {
        let config = UpdatesConfig {
            enabled: false,
            check_on_startup: true,
            channel: UpdateChannel::Stable,
            mode: UpdateMode::Auto,
            interval_hours: 24,
            allow_apply_for_standalone: true,
            github_repo: "rekurt/tsql".to_string(),
        };

        assert_eq!(UpdateState::policy(&config), UpdatePolicy::Off);
    }

    #[test]
    fn test_should_check_on_startup_only_once() {
        let config = UpdatesConfig::default();
        let mut state = UpdateState::default();

        assert!(state.should_check_on_startup(&config));
        state.mark_check_started(true);
        assert!(!state.should_check_on_startup(&config));
    }

    #[test]
    fn test_should_check_by_interval_respects_elapsed_time() {
        let config = UpdatesConfig::default();
        let now = Instant::now();
        let well_after_interval = now
            .checked_add(Duration::from_secs(3600 * 25))
            .expect("instant add should be representable");
        let within_interval = now
            .checked_add(Duration::from_secs(3600))
            .expect("instant add should be representable");

        let mut state = UpdateState {
            startup_check_started: true,
            check_in_flight: false,
            last_checked_at: Some(now),
            last_outcome: None,
        };
        assert!(state.should_check_by_interval(&config, well_after_interval));

        state.last_checked_at = Some(now);
        assert!(!state.should_check_by_interval(&config, within_interval));
    }

    #[test]
    fn test_should_check_by_interval_handles_future_last_checked() {
        let config = UpdatesConfig::default();
        let now = Instant::now();
        let future = now
            .checked_add(Duration::from_secs(3600))
            .expect("instant add should be representable");
        let state = UpdateState {
            startup_check_started: true,
            check_in_flight: false,
            last_checked_at: Some(future),
            last_outcome: None,
        };

        assert!(!state.should_check_by_interval(&config, now));
    }

    #[test]
    fn test_apply_allowed_only_for_auto_mode_standalone() {
        let config = UpdatesConfig::default();
        let expected = !cfg!(windows);
        assert_eq!(
            UpdateState::apply_allowed(&config, InstallMethod::StandaloneBinary),
            expected
        );
        assert!(!UpdateState::apply_allowed(
            &config,
            InstallMethod::Homebrew
        ));
    }
}
