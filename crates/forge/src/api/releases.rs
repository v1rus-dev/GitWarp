//! Published releases of a GitHub repository, for GitWarp's own update check.
//!
//! `GET /repos/{owner}/{repo}/releases?per_page=30` —
//! <https://docs.github.com/en/rest/releases/releases#list-releases>.
//! Anonymous: no token is ever sent.

use super::{ApiError, Client};
use serde::Deserialize;

/// Base URL of the GitHub REST API.
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
    /// The newest 30 releases of `owner/repo` on github.com, drafts included
    /// (callers filter them).
    pub fn list_releases(&self, owner: &str, repo: &str) -> Result<Vec<Release>, ApiError> {
        self.list_releases_at(GITHUB_API, owner, repo)
    }

    fn list_releases_at(
        &self,
        api_base: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<Release>, ApiError> {
        let url = format!("{api_base}/repos/{owner}/{repo}/releases?per_page=30");
        let headers = [("Accept", "application/vnd.github+json".to_string())];
        let (releases, _) = self.get_json::<Vec<Release>>(&url, &headers)?;
        Ok(releases)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn list_releases_decodes_two_releases() {
        let body = r#"[
            {"tag_name":"v0.2.0","html_url":"https://example.test/r/v0.2.0","prerelease":false,"draft":false,"id":1},
            {"tag_name":"v0.3.0-rc.1","html_url":"https://example.test/r/v0.3.0-rc.1","prerelease":true,"draft":false,"name":"x"}
        ]"#;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Some(stream) = listener.incoming().flatten().next() {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = reader.get_mut().write_all(out.as_bytes());
            }
        });
        let releases = Client::for_tests()
            .list_releases_at(&base, "o", "r")
            .unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag_name, "v0.2.0");
        assert!(!releases[0].prerelease);
        assert!(releases[1].prerelease);
        assert_eq!(releases[1].html_url, "https://example.test/r/v0.3.0-rc.1");
    }
}
