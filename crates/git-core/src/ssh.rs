//! SSH host-key checks for the first connection to a Git host.
//!
//! A GUI process has no terminal, so OpenSSH's own "trust this host?" prompt
//! would either block or fail. Before an SSH fetch, push or clone the app asks
//! here: `known_hosts` is consulted with `ssh-keygen -F`, an unknown host is
//! scanned with `ssh-keyscan`, and the UI shows the fingerprint before the key
//! is appended to `known_hosts`. A changed key is reported instead of being
//! silently accepted.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::{Error, Result};

/// The host and port an SSH remote connects to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
}

/// One key `ssh-keygen -l` described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// e.g. `SHA256:+DiY3wvvV6TuJ…`.
    pub fingerprint: String,
    /// e.g. `ED25519`; empty when `ssh-keygen` did not say.
    pub kind: String,
}

/// What `known_hosts` says about a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// The host is already trusted.
    Known,
    /// No entry yet: the caller should show `keys` and ask.
    Unknown { keys: Vec<HostKey> },
    /// An entry exists but the host now presents different keys.
    Changed {
        old: Vec<HostKey>,
        new: Vec<HostKey>,
    },
    /// `ssh-keyscan` could not reach the host. The actual git operation will
    /// produce the authoritative network error.
    Unreachable { message: String },
    /// `ssh-keygen` is not installed; let git handle SSH itself.
    Unavailable,
}

/// Parses an SSH remote URL into host and port.
///
/// Handles `user@host:path`, `ssh://user@host/path` and
/// `ssh://user@host:2222/path`; HTTPS and local paths are not SSH.
pub fn ssh_target(url: &str) -> Option<SshTarget> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, tail)| tail);
        let (authority, _path) = rest.split_once('/')?;
        if authority.is_empty() {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok()?),
            None => (authority, 22),
        };
        return (!host.is_empty()).then(|| SshTarget {
            host: host.to_owned(),
            port,
        });
    }
    if url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("../")
        || url.starts_with('~')
    {
        return None;
    }
    let (authority, path) = url.split_once(':')?;
    if authority.contains('/') || authority.is_empty() || path.trim_matches('/').is_empty() {
        return None;
    }
    let host = authority.rsplit('@').next().unwrap_or(authority);
    (host.len() > 1).then(|| SshTarget {
        host: host.to_owned(),
        port: 22,
    })
}

/// `host` or `[host]:port`, the form `ssh-keygen` and `known_hosts` use.
pub fn host_argument(target: &SshTarget) -> String {
    if target.port == 22 {
        target.host.clone()
    } else {
        format!("[{}]:{}", target.host, target.port)
    }
}

/// Checks whether `target` is trusted, scanning it when it is not.
pub fn check_host_key(target: &SshTarget) -> Result<HostKeyStatus> {
    let known_hosts = match known_hosts_path() {
        Some(path) => path,
        None => return Ok(HostKeyStatus::Unavailable),
    };
    if !ssh_keygen_available() {
        return Ok(HostKeyStatus::Unavailable);
    }
    let host = host_argument(target);
    let known = known_keys(&host, &known_hosts)?;
    let old = known_fingerprints(&host, &known_hosts);
    let scan = match scan_host(target) {
        Ok(scan) => scan,
        Err(error) => {
            // The key is already trusted and we cannot re-scan right now (a
            // captive portal, a flaky connection): let git decide.
            return if known.is_empty() {
                Ok(HostKeyStatus::Unreachable {
                    message: error.to_string(),
                })
            } else {
                Ok(HostKeyStatus::Known)
            };
        }
    };
    let keys = fingerprints(&scan);
    if known.is_empty() {
        return Ok(HostKeyStatus::Unknown { keys });
    }
    if scan
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .any(|line| known.iter().any(|key| line.contains(key)))
    {
        Ok(HostKeyStatus::Known)
    } else {
        Ok(HostKeyStatus::Changed { old, new: keys })
    }
}

/// Appends the host's current keys to `known_hosts`.
pub fn trust_host_key(target: &SshTarget) -> Result<()> {
    let known_hosts = known_hosts_path().ok_or_else(|| Error::Git {
        command: "ssh-keyscan",
        code: None,
        stderr: "no home directory for ~/.ssh/known_hosts".to_owned(),
    })?;
    let scan = scan_host(target)?;
    if scan.lines().all(|line| line.trim().is_empty()) {
        return Err(Error::Git {
            command: "ssh-keyscan",
            code: None,
            stderr: format!("{} did not present a host key", host_argument(target)),
        });
    }
    if let Some(parent) = known_hosts.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&known_hosts)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    for line in scan.lines().filter(|line| !line.trim().is_empty()) {
        writeln!(file, "{line}")?;
    }
    file.flush()?;
    Ok(())
}

/// Replaces the trusted keys for `target` with the ones it presents now.
pub fn replace_host_key(target: &SshTarget) -> Result<()> {
    if let Some(path) = known_hosts_path() {
        let out = Command::new("ssh-keygen")
            .args(["-R", &host_argument(target), "-f"])
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .map_err(Error::Spawn)?;
        // `ssh-keygen -R` exits 0 when it removed something and 1 when the
        // host was not in the file; both leave a usable file behind.
        let _ = out;
    }
    trust_host_key(target)
}

/// Keys in `known_hosts` for `host`, as key blobs (`AAAA…`).
fn known_keys(host: &str, known_hosts: &PathBuf) -> Result<Vec<String>> {
    let out = Command::new("ssh-keygen")
        .args(["-F", host, "-f"])
        .arg(known_hosts)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_known_keys(&String::from_utf8_lossy(&out.stdout)))
}

/// The key blobs of `known_hosts` lines, skipping `#` comments and the
/// `@cert-authority`/`@revoked` markers.
fn parse_known_keys(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            if line.starts_with('@') {
                let _marker = fields.next()?;
            }
            let _host = fields.next()?;
            let _kind = fields.next()?;
            fields.next().map(str::to_owned)
        })
        .collect()
}

/// Fingerprints of the keys already trusted for `host`.
fn known_fingerprints(host: &str, known_hosts: &PathBuf) -> Vec<HostKey> {
    let Ok(out) = Command::new("ssh-keygen")
        .args(["-l", "-F", host, "-f"])
        .arg(known_hosts)
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    parse_fingerprint_lines(&String::from_utf8_lossy(&out.stdout))
}

/// `ssh-keyscan` output for `target`.
fn scan_host(target: &SshTarget) -> Result<String> {
    let out = Command::new("ssh-keyscan")
        .args(["-T", "10", "-p", &target.port.to_string()])
        .arg(&target.host)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(Error::Git {
            command: "ssh-keyscan",
            code: out.status.code(),
            stderr: if stderr.is_empty() {
                format!("could not reach {}", target.host)
            } else {
                stderr
            },
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Fingerprints for every key line in `scan`.
fn fingerprints(scan: &str) -> Vec<HostKey> {
    let mut child = match Command::new("ssh-keygen")
        .args(["-l", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return fallback_fingerprints(scan),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(scan.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else {
        return fallback_fingerprints(scan);
    };
    let keys = parse_fingerprint_lines(&String::from_utf8_lossy(&out.stdout));
    if keys.is_empty() {
        fallback_fingerprints(scan)
    } else {
        keys
    }
}

/// Parses both shapes `ssh-keygen` prints fingerprints in:
///
/// * `-lf -`: `256 SHA256:… comment (ED25519)`
/// * `-lF host -f known_hosts`: `github.com ED25519 SHA256:…`
///
/// Comment lines (`# Host … found: line 4`) are skipped.
fn parse_fingerprint_lines(output: &str) -> Vec<HostKey> {
    output
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let index = tokens
                .iter()
                .position(|token| token.starts_with("SHA256:") || token.starts_with("MD5:"))?;
            let kind = line
                .rsplit_once('(')
                .and_then(|(_, tail)| tail.strip_suffix(')'))
                .map(str::to_owned)
                .or_else(|| index.checked_sub(1).map(|index| tokens[index].to_owned()))
                .unwrap_or_default();
            Some(HostKey {
                fingerprint: tokens[index].to_owned(),
                kind,
            })
        })
        .collect()
}

/// When `ssh-keygen -l` refuses the scan output, at least show the key type
/// and the start of the blob instead of an empty dialog.
fn fallback_fingerprints(scan: &str) -> Vec<HostKey> {
    scan.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _host = fields.next();
            let kind = fields.next().unwrap_or("key").to_owned();
            let blob = fields.next().unwrap_or("");
            let short: String = blob.chars().take(16).collect();
            HostKey {
                fingerprint: format!("{short}…"),
                kind,
            }
        })
        .collect()
}

fn ssh_keygen_available() -> bool {
    Command::new("ssh-keygen")
        .arg("-?")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// `~/.ssh/known_hosts`, or `$SSH_KNOWN_HOSTS_FILE` in tests.
fn known_hosts_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SSH_KNOWN_HOSTS_FILE") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_addresses() {
        assert_eq!(
            ssh_target("git@github.com:nexu-io/open-design.git"),
            Some(SshTarget {
                host: "github.com".into(),
                port: 22
            })
        );
        assert_eq!(
            ssh_target("ssh://git@git.example:2222/team/repo.git"),
            Some(SshTarget {
                host: "git.example".into(),
                port: 2222
            })
        );
        assert_eq!(
            ssh_target("ssh://git@github.com/org/repo.git"),
            Some(SshTarget {
                host: "github.com".into(),
                port: 22
            })
        );
        assert_eq!(ssh_target("https://github.com/a/b.git"), None);
        assert_eq!(ssh_target("/tmp/repo"), None);
        assert_eq!(ssh_target("C:\\repos\\x"), None);
    }

    #[test]
    fn reads_key_blobs_from_known_hosts_lines() {
        let lines = "# Host github.com found: line 4 \ngithub.com ssh-ed25519 AAAAC3Nza\n|1|abc=|def= ssh-rsa AAAAB3Nza\n";
        assert_eq!(parse_known_keys(lines), vec!["AAAAC3Nza", "AAAAB3Nza"]);
    }

    #[test]
    fn formats_host_arguments() {
        assert_eq!(
            host_argument(&SshTarget {
                host: "github.com".into(),
                port: 22
            }),
            "github.com"
        );
        assert_eq!(
            host_argument(&SshTarget {
                host: "git.example".into(),
                port: 2222
            }),
            "[git.example]:2222"
        );
    }

    #[test]
    fn parses_fingerprint_output() {
        // Real shape of `ssh-keygen -lf -` output.
        let line = "256 SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU no comment (ED25519)";
        let mut fields = line.split_whitespace();
        let _bits = fields.next().unwrap();
        let fingerprint = fields.next().unwrap();
        let kind = line
            .rsplit_once('(')
            .and_then(|(_, tail)| tail.strip_suffix(')'))
            .unwrap();
        assert_eq!(
            fingerprint,
            "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"
        );
        assert_eq!(kind, "ED25519");
    }
}
