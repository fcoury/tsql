use std::time::{Duration, Instant};

use crate::config::{UpdateMode, UpdatesConfig};

use super::types::{UpdateCheckOutcome, UpdatePolicy};

#[derive(Debug, Clone)]
pub struct UpdateState {
    pub startup_check_started: bool,
    pub check_in_flight: bool,
    pub last_checked_at: Option<Instant>,
    pub last_outcome: Option<UpdateCheckOutcome>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            startup_check_started: false,
            check_in_flight: false,
            last_checked_at: None,
            last_outcome: None,
        }
    }
}

impl UpdateState {
    pub fn policy(config: &UpdatesConfig) -> UpdatePolicy {
        if !config.enabled || matches!(config.mode, UpdateMode::Off) {
            return UpdatePolicy::Off;
        }

        match config.mode {
            UpdateMode::Off => UpdatePolicy::Off,
            UpdateMode::Auto | UpdateMode::NotifyOnly => UpdatePolicy::NotifyOnly,
        }
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
            Some(last_checked_at) => now.duration_since(last_checked_at) >= interval,
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
            github_repo: "fcoury/tsql".to_string(),
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

        let mut state = UpdateState {
            startup_check_started: true,
            check_in_flight: false,
            last_checked_at: Some(now - Duration::from_secs(3600 * 25)),
            last_outcome: None,
        };
        assert!(state.should_check_by_interval(&config, now));

        state.last_checked_at = Some(now - Duration::from_secs(3600));
        assert!(!state.should_check_by_interval(&config, now));
    }
}
