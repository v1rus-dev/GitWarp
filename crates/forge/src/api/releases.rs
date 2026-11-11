//! Published releases of a GitHub repository, for GitWarp's own update check.
//!
//! `GET /repos/{owner}/{repo}/releases?per_page=100` —
//! <https://docs.github.com/en/rest/releases/releases#list-releases>.
//! Anonymous: no token is ever sent.

use super::{ApiError, Auth, Client};
use crate::instance::GITHUB_API_BASE;
use crate::ServiceKind;
use serde::Deserialize;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// One release as far as the update check needs it; unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub prerelease: bool,
    pub draft: bool,
    /// Files attached to the release; empty when the response has none.
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
}

/// Redirects followed by a download (GitHub answers with one, to its CDN).
const MAX_DOWNLOAD_REDIRECTS: usize = 5;
/// Read buffer of a download; progress is reported once per read.
const DOWNLOAD_CHUNK: usize = 64 * 1024;

impl Client {
    /// The newest 100 releases (GitHub's maximum page) of `owner/repo` on
    /// github.com, drafts included (callers filter them). 100 so that a long
    /// run of pre-releases cannot hide the latest stable one.
    pub fn list_releases(&self, owner: &str, repo: &str) -> Result<Vec<Release>, ApiError> {
        self.list_releases_at(GITHUB_API_BASE, owner, repo)
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

    /// Downloads `url` (https only) to `dest`, creating parent directories.
    /// The data goes to `dest` + `.part` and is renamed on success; the part
    /// file is removed on any failure. `on_progress(done, total)` is called
    /// after every chunk, `total` from Content-Length when sent. Redirects are
    /// followed by hand, https only, and no credentials are sent. When
    /// `cancel` becomes true the download stops with
    /// `ApiError::Unreachable("download cancelled")`.
    pub fn download(
        &self,
        url: &str,
        dest: &Path,
        on_progress: impl FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<(), ApiError> {
        self.download_inner(url, dest, on_progress, cancel, false)
    }

    pub(super) fn download_inner(
        &self,
        url: &str,
        dest: &Path,
        mut on_progress: impl FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
        allow_http: bool,
    ) -> Result<(), ApiError> {
        let scheme_ok =
            |u: &str| u.starts_with("https://") || (allow_http && u.starts_with("http://"));
        let mut current = url.to_string();
        let mut redirects = 0;
        let response = loop {
            if !scheme_ok(&current) {
                return Err(ApiError::Rejected("downloads must use https".to_string()));
            }
            // The agent-wide 15 s limit would cut a large file short.
            let response = self
                .agent
                .get(&current)
                .config()
                .timeout_global(None)
                .build()
                .call()
                .map_err(super::map_transport)?;
            if !response.status().is_redirection() {
                break super::check_status(response)?;
            }
            redirects += 1;
            let status = response.status().as_u16();
            let location = super::header_str(&response, "location")
                .filter(|_| redirects <= MAX_DOWNLOAD_REDIRECTS)
                .ok_or(ApiError::Status(status))?;
            current = location.to_string();
        };
        let total = super::header_str(&response, "content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut part_name: OsString = dest.as_os_str().to_owned();
        part_name.push(".part");
        let part = Path::new(&part_name);
        let result = (|| {
            let mut file = File::create(part).map_err(io_error)?;
            let mut reader = response.into_body().into_reader();
            let mut buffer = vec![0u8; DOWNLOAD_CHUNK];
            let mut done = 0u64;
            on_progress(0, total);
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ApiError::Unreachable("download cancelled".to_string()));
                }
                let n = reader.read(&mut buffer).map_err(io_error)?;
                if n == 0 {
                    break;
                }
                file.write_all(&buffer[..n]).map_err(io_error)?;
                done += n as u64;
                on_progress(done, total);
            }
            if total.is_some_and(|t| t != done) {
                return Err(ApiError::Decode("the download was cut short".to_string()));
            }
            file.flush().map_err(io_error)?;
            drop(file);
            std::fs::rename(part, dest).map_err(io_error)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(part);
        }
        result
    }
}

fn io_error(error: std::io::Error) -> ApiError {
    ApiError::Unreachable(error.to_string())
}
