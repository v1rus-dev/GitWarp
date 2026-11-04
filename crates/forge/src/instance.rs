use crate::remote::{RemoteRef, RepoCoords, Transport};
use crate::url::{parse_authority, split_host_path, split_scheme};
use crate::ServiceKind;
use std::fmt;

/// The public instances GitWarp knows out of the box, as `(kind, host)`.
/// Base URL of the public GitHub REST API.
pub(crate) const GITHUB_API_BASE: &str = "https://api.github.com";

pub const PUBLIC_INSTANCES: [(ServiceKind, &str); 5] = [
    (ServiceKind::GitHub, "github.com"),
    (ServiceKind::GitLab, "gitlab.com"),
    (ServiceKind::Gitea, "codeberg.org"),
    (ServiceKind::Bitbucket, "bitbucket.org"),
    (ServiceKind::AzureDevOps, "dev.azure.com"),
];

/// Why [`Instance::new`] rejected its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceError {
    Empty,
    Unparsable,
    /// Only `http` and `https` are accepted; carries the offending scheme.
    UnsupportedScheme(String),
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstanceError::Empty => f.write_str("the URL is empty"),
            InstanceError::Unparsable => f.write_str("the URL cannot be parsed"),
            InstanceError::UnsupportedScheme(s) => write!(f, "unsupported scheme `{s}`"),
        }
    }
}

impl std::error::Error for InstanceError {}

/// A service type plus the base URL of one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub kind: ServiceKind,
    /// Normalized: `scheme://host[:port][/sub-path]`, no trailing `/`.
    pub base_url: String,
    /// Host used for git over SSH when it differs from the web host.
    pub ssh_host: Option<String>,
}

impl Instance {
    /// Normalizes `url`: default scheme `https://`, lowercase host, explicit
    /// port and sub-path (GitLab under `/gitlab`) kept, trailing `/`, query
    /// and fragment dropped.
    pub fn new(kind: ServiceKind, url: &str) -> Result<Instance, InstanceError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(InstanceError::Empty);
        }
        let (scheme, rest) = split_scheme(url);
        let scheme = match scheme.map(str::to_ascii_lowercase).as_deref() {
            None => "https".to_string(),
            Some(s @ ("http" | "https")) => s.to_string(),
            Some(other) => return Err(InstanceError::UnsupportedScheme(other.to_string())),
        };
        let rest = rest.split(['?', '#']).next().unwrap_or("");
        let (authority, path) = split_host_path(rest);
        let (host, port) = parse_authority(authority).ok_or(InstanceError::Unparsable)?;
        let port = port.map(|p| format!(":{p}")).unwrap_or_default();
        let path = path.trim_end_matches('/');
        Ok(Instance {
            kind,
            base_url: format!("{scheme}://{host}{port}{path}"),
            ssh_host: None,
        })
    }

    /// Splits the normalized base URL into (host, port, sub-path).
    fn parts(&self) -> (&str, Option<u16>, &str) {
        let (_, rest) = split_scheme(&self.base_url);
        let (authority, path) = split_host_path(rest);
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        match host_port.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok(), path),
            None => (host_port, None, path),
        }
    }

    /// Lowercase host of the base URL, without port.
    pub fn host(&self) -> &str {
        self.parts().0
    }

    /// Sub-path of the base URL (`/gitlab`), empty when installed at the root.
    pub fn sub_path(&self) -> &str {
        self.parts().2
    }

    /// True for the well-known public instance of this kind (github.com, ...).
    pub fn is_public(&self) -> bool {
        let (host, port, path) = self.parts();
        port.is_none()
            && path.is_empty()
            && PUBLIC_INSTANCES
                .iter()
                .any(|&(k, h)| k == self.kind && h == host)
    }

    /// Base URL of the REST API.
    pub fn api_base(&self) -> String {
        let base = &self.base_url;
        let public = self.is_public();
        match self.kind {
            ServiceKind::GitHub if public => GITHUB_API_BASE.to_string(),
            ServiceKind::GitHub => format!("{base}/api/v3"),
            ServiceKind::GitLab => format!("{base}/api/v4"),
            ServiceKind::Gitea => format!("{base}/api/v1"),
            ServiceKind::Bitbucket if public => "https://api.bitbucket.org/2.0".to_string(),
            ServiceKind::Bitbucket => format!("{base}/rest/api/1.0"),
            ServiceKind::AzureDevOps => base.clone(),
        }
    }

    /// Resolves `remote` against this instance: the host must be this
    /// instance's host, its `ssh_host` or a known alias; for http(s) remotes
    /// the port must agree and the instance sub-path is stripped.
    pub fn matches(&self, remote: &RemoteRef) -> Option<RepoCoords> {
        if !self.host_matches(&remote.host) {
            return None;
        }
        let rest = match remote.transport {
            // The SSH port belongs to sshd, not to the web server, and an
            // scp-like path never contains the web sub-path.
            Transport::Ssh => &remote.path[..],
            _ => {
                if default_port_to_none(remote.port) != default_port_to_none(self.parts().1) {
                    return None;
                }
                self.strip_sub_path(&remote.path)?
            }
        };
        (!rest.is_empty()).then(|| RepoCoords {
            namespace: rest.to_vec(),
            repo: remote.repo.clone(),
        })
    }

    fn host_matches(&self, remote_host: &str) -> bool {
        canonical_host(remote_host) == canonical_host(self.host())
            || self
                .ssh_host
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(remote_host))
    }

    /// Azure's base URL has no sub-path, so this is a no-op there.
    fn strip_sub_path<'a>(&self, path: &'a [String]) -> Option<&'a [String]> {
        let sub: Vec<&str> = self
            .sub_path()
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        let starts_with_sub = path.len() >= sub.len() && path.iter().zip(&sub).all(|(a, b)| a == b);
        starts_with_sub.then(|| &path[sub.len()..])
    }
}

/// 80 and 443 are the defaults of http / https; treat them as "no port".
fn default_port_to_none(port: Option<u16>) -> Option<u16> {
    port.filter(|p| !matches!(p, 80 | 443))
}

/// Maps SSH and legacy host aliases to the web host of the same service.
fn canonical_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "ssh.github.com" => "github.com".to_string(),
        "ssh.dev.azure.com" | "vs-ssh.visualstudio.com" => "dev.azure.com".to_string(),
        h if h.ends_with(".visualstudio.com") => "dev.azure.com".to_string(),
        _ => host,
    }
}

/// The service kind a host belongs to when it is a public service (or one of
/// its SSH / legacy aliases), so the UI can offer "Add account for <host>".
pub fn known_kind_for_host(host: &str) -> Option<ServiceKind> {
    let canonical = canonical_host(host);
    PUBLIC_INSTANCES
        .iter()
        .find(|&&(_, h)| h == canonical)
        .map(|&(k, _)| k)
}
