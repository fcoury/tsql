use semver::Version;

use crate::config::UpdateChannel;

use super::provider::ReleaseProvider;
use super::types::{UpdateCheckOutcome, UpdateInfo};

pub fn check_for_update(
    provider: &dyn ReleaseProvider,
    current: &Version,
    channel: UpdateChannel,
) -> UpdateCheckOutcome {
    match provider.latest(channel) {
        Ok(Some(release)) if release.version > *current => {
            UpdateCheckOutcome::UpdateAvailable(UpdateInfo {
                current: current.clone(),
                latest: release.version,
                notes_url: release.notes_url,
                asset_url: release.asset_url,
                checksum_url: release.checksum_url,
            })
        }
        Ok(Some(_)) | Ok(None) => UpdateCheckOutcome::UpToDate {
            current: current.clone(),
        },
        Err(error) => UpdateCheckOutcome::Error(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;
    use crate::update::provider::ReleaseCandidate;

    struct StubProvider {
        latest: Option<ReleaseCandidate>,
    }

    struct FailingProvider;

    impl ReleaseProvider for StubProvider {
        fn latest(&self, _channel: UpdateChannel) -> Result<Option<ReleaseCandidate>> {
            Ok(self.latest.clone())
        }
    }

    impl ReleaseProvider for FailingProvider {
        fn latest(&self, _channel: UpdateChannel) -> Result<Option<ReleaseCandidate>> {
            anyhow::bail!("Network error")
        }
    }

    #[test]
    fn test_check_for_update_reports_available_when_newer_version_exists() {
        let provider = StubProvider {
            latest: Some(ReleaseCandidate {
                version: Version::new(0, 5, 0),
                notes_url: Some("https://example.com/release".to_string()),
                asset_url: None,
                checksum_url: None,
            }),
        };

        let outcome = check_for_update(&provider, &Version::new(0, 4, 2), UpdateChannel::Stable);
        assert!(matches!(outcome, UpdateCheckOutcome::UpdateAvailable(_)));
    }

    #[test]
    fn test_check_for_update_reports_up_to_date_when_equal_version() {
        let provider = StubProvider {
            latest: Some(ReleaseCandidate {
                version: Version::new(0, 4, 2),
                notes_url: None,
                asset_url: None,
                checksum_url: None,
            }),
        };

        let outcome = check_for_update(&provider, &Version::new(0, 4, 2), UpdateChannel::Stable);
        assert!(matches!(outcome, UpdateCheckOutcome::UpToDate { .. }));
    }

    #[test]
    fn test_check_for_update_reports_error_on_provider_failure() {
        let provider = FailingProvider;
        let outcome = check_for_update(&provider, &Version::new(0, 4, 2), UpdateChannel::Stable);
        assert!(matches!(outcome, UpdateCheckOutcome::Error(_)));
    }
}
