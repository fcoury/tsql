use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use semver::Version;
use serde::Deserialize;

use crate::config::UpdateChannel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseCandidate {
    pub version: Version,
    pub notes_url: Option<String>,
    pub asset_url: Option<String>,
    pub checksum_url: Option<String>,
}

pub trait ReleaseProvider: Send + Sync {
    fn latest(&self, channel: UpdateChannel) -> Result<Option<ReleaseCandidate>>;
}

#[derive(Debug, Clone)]
pub struct GitHubReleasesProvider {
    repo: String,
}

impl GitHubReleasesProvider {
    pub fn new(repo: String) -> Self {
        Self { repo }
    }
}

impl ReleaseProvider for GitHubReleasesProvider {
    fn latest(&self, channel: UpdateChannel) -> Result<Option<ReleaseCandidate>> {
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=20",
            self.repo
        );

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout_read(Duration::from_secs(8))
            .timeout_write(Duration::from_secs(8))
            .build();

        let response = agent
            .get(&url)
            .set("User-Agent", "tsql-update-checker")
            .set("Accept", "application/vnd.github+json")
            .call()
            .map_err(|error| map_fetch_error(&self.repo, error))?;

        let body = response
            .into_string()
            .context("failed to read GitHub releases response body")?;
        let releases: Vec<GitHubRelease> =
            serde_json::from_str(&body).context("failed to parse GitHub releases payload")?;

        for release in releases {
            if release.draft {
                continue;
            }
            if matches!(channel, UpdateChannel::Stable) && release.prerelease {
                continue;
            }

            let Some(version) = parse_tag_version(&release.tag_name) else {
                continue;
            };

            let asset_url = release
                .assets
                .iter()
                .find(|asset| {
                    asset.name.ends_with(".tar.gz")
                        || asset.name.ends_with(".zip")
                        || asset.name.ends_with(".tar.xz")
                })
                .map(|asset| asset.browser_download_url.clone());

            let checksum_url = release
                .assets
                .iter()
                .find(|asset| {
                    let name = asset.name.to_ascii_lowercase();
                    name.starts_with("sha256") || name.contains("checksums")
                })
                .map(|asset| asset.browser_download_url.clone());

            return Ok(Some(ReleaseCandidate {
                version,
                notes_url: Some(release.html_url),
                asset_url,
                checksum_url,
            }));
        }

        Ok(None)
    }
}

fn map_fetch_error(repo: &str, error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(code, response) => {
            let status_text = response.status_text();
            anyhow!(
                "Update check failed: GitHub API returned {} {} for {}",
                code,
                status_text,
                repo
            )
        }
        ureq::Error::Transport(error) => {
            let message = error.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("timed out") || lower.contains("timeout") {
                anyhow!("Update check timed out for {}", repo)
            } else {
                anyhow!("Update check network error for {}: {}", repo, message)
            }
        }
    }
}

fn parse_tag_version(tag: &str) -> Option<Version> {
    let trimmed = tag.trim().trim_start_matches('v');
    Version::parse(trimmed).ok()
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tag_version_with_v_prefix() {
        let version = parse_tag_version("v0.4.2").expect("valid semver");
        assert_eq!(version, Version::new(0, 4, 2));
    }

    #[test]
    fn test_parse_tag_version_without_v_prefix() {
        let version = parse_tag_version("1.2.3").expect("valid semver");
        assert_eq!(version, Version::new(1, 2, 3));
    }

    #[test]
    fn test_parse_tag_version_invalid_returns_none() {
        assert!(parse_tag_version("release-2026").is_none());
    }

    #[test]
    fn test_map_fetch_error_status_is_descriptive() {
        let response = ureq::Response::new(403, "Forbidden", "").expect("valid response");
        let error = map_fetch_error("fcoury/tsql", ureq::Error::Status(403, response));
        assert!(
            error
                .to_string()
                .contains("GitHub API returned 403 Forbidden"),
            "unexpected error: {error}"
        );
    }
}
