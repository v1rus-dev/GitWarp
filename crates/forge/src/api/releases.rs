//! Published releases of a GitHub repository, for GitWarp's own update check.
//!
//! `GET /repos/{owner}/{repo}/releases?per_page=100` —
//! <https://docs.github.com/en/rest/releases/releases#list-releases>.
//! Anonymous: no token is ever sent.

use super::{ApiError, Auth, Client};
use crate::ServiceKind;
use serde::Deserialize;

/// Base URL of the GitHub REST API. `Instance::api_base` yields the same value
/// but only through a fallible `Instance::new`, which is not worth it for a
/// fixed host.
const GITHUB_API: &str = "https://api.github.com";

/// One release as far as the update check needs it; unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub prerelease: bool,
    pub draft: bool,
}

impl Client {
    /// The newest 100 releases (GitHub's maximum page) of `owner/repo` on
    /// github.com, drafts included (callers filter them). 100 so that a long
    /// run of pre-releases cannot hide the latest stable one.
    pub fn list_releases(&self, owner: &str, repo: &str) -> Result<Vec<Release>, ApiError> {
        self.list_releases_at(GITHUB_API, owner, repo)
    }

    pub(super) fn list_releases_at(
        &self,
        api_base: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<Release>, ApiError> {
        let url = format!("{api_base}/repos/{owner}/{repo}/releases?per_page=100");
        // Anonymous, with the same GitHub headers as every other GitHub call.
        let headers = Auth::anonymous().headers(ServiceKind::GitHub);
        let (releases, _) = self.get_json::<Vec<Release>>(&url, &headers)?;
        Ok(releases)
    }
}
