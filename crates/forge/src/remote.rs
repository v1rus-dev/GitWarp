use crate::url::{parse_authority, split_host_path};

/// How a remote URL talks to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Https,
    Http,
    Ssh,
}

/// A parsed git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// Lowercase host, no user, no port.
    pub host: String,
    pub port: Option<u16>,
    /// Namespace segments in front of the repository; may be nested (GitLab
    /// groups) and may still contain an instance sub-path.
    pub path: Vec<String>,
    /// Repository name without `.git`.
    pub repo: String,
    pub transport: Transport,
}

/// A repository on one instance: namespace (for Azure: `[org, project]`) and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCoords {
    pub namespace: Vec<String>,
    pub repo: String,
}

impl RemoteRef {
    /// Parses https / http / ssh URLs and scp-like `[user@]host:path`.
    /// Returns `None` for local paths, `file://` and anything unparsable.
    ///
    /// Azure DevOps forms are normalized to `path = [org, project]`: the
    /// `_git` marker and the SSH `v3` prefix are dropped, and for
    /// `org.visualstudio.com` the organization is taken from the host.
    pub fn parse(url: &str) -> Option<RemoteRef> {
        let url = url.trim();
        match url.split_once("://") {
            Some((scheme, rest)) => parse_url(scheme, rest),
            None => parse_scp(url),
        }
    }

    /// Namespace segments joined with `/`.
    pub fn owner_path(&self) -> String {
        self.path.join("/")
    }

    /// `owner/…/repo`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner_path(), self.repo)
    }
}

fn parse_url(scheme: &str, rest: &str) -> Option<RemoteRef> {
    let transport = match scheme.to_ascii_lowercase().as_str() {
        "https" => Transport::Https,
        "http" => Transport::Http,
        "ssh" => Transport::Ssh,
        _ => return None,
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = split_host_path(rest);
    let (host, port) = parse_authority(authority)?;
    build(transport, host, port, path)
}

fn parse_scp(url: &str) -> Option<RemoteRef> {
    // Local paths, `~user/x` and Windows paths (`C:\x`) are not remotes.
    if url.is_empty() || url.starts_with(['/', '.', '~']) || url.contains('\\') {
        return None;
    }
    let (left, path) = url.split_once(':')?;
    let host_part = left.rsplit_once('@').map_or(left, |(_, h)| h);
    // A one-letter "host" is a drive letter (`C:/x`).
    if host_part.len() < 2 {
        return None;
    }
    let (host, port) = parse_authority(left)?;
    build(Transport::Ssh, host, port, path)
}

fn build(transport: Transport, host: String, port: Option<u16>, path: &str) -> Option<RemoteRef> {
    let mut segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    normalize_azure(&host, &mut segments);
    let mut repo = segments.pop()?;
    if let Some(stripped) = repo.strip_suffix(".git") {
        repo = stripped.to_string();
    }
    if repo.is_empty() || segments.is_empty() {
        return None;
    }
    Some(RemoteRef {
        host,
        port,
        path: segments,
        repo,
        transport,
    })
}

/// Rewrites Azure DevOps path layouts to `[org, project, repo]`.
fn normalize_azure(host: &str, segs: &mut Vec<String>) {
    if host == "ssh.dev.azure.com" || host == "vs-ssh.visualstudio.com" {
        if segs.first().is_some_and(|s| s == "v3") {
            segs.remove(0);
        }
        return;
    }
    let org = host.strip_suffix(".visualstudio.com");
    if org.is_none() && host != "dev.azure.com" {
        return;
    }
    let Some(git) = segs.iter().position(|s| s == "_git") else {
        return;
    };
    segs.remove(git);
    if let Some(org) = org {
        // Legacy `https://org.visualstudio.com/[DefaultCollection/]project/_git/repo`.
        if segs.first().is_some_and(|s| s == "DefaultCollection") {
            segs.remove(0);
        }
        segs.insert(0, org.to_string());
    }
    // `org/_git/repo`: the project has the repository's name.
    if segs.len() == 2 {
        segs.insert(1, segs[1].clone());
    }
}
