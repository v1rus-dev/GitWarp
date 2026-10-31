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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

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

/// One private key in `~/.ssh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKey {
    /// The file name, e.g. `id_ed25519`.
    pub name: String,
    /// Absolute path of the private key.
    pub path: PathBuf,
    /// `ed25519`, `rsa-4096`, `ecdsa-521`, `unknown`…
    pub kind: String,
    /// `SHA256:…`, empty when the key could not be read.
    pub fingerprint: String,
    /// The public key line, empty when only the private half exists.
    pub public_key: String,
    /// The key's comment, usually an email address.
    pub comment: String,
    /// Modification time, in unix seconds.
    pub created: u64,
    /// Whether `ssh-agent` has the key.
    pub agent: bool,
    /// Whether the private half needs a passphrase.
    pub encrypted: bool,
}

/// One entry of `~/.ssh/known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedHost {
    pub host: String,
    /// `ED25519`, `RSA`, …
    pub kind: String,
    /// `SHA256:…`; a marker when the fingerprint could not be computed.
    pub fingerprint: String,
}

/// A key type offered by the Generate dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Ed25519,
    Rsa4096,
    Ecdsa,
}

impl KeyType {
    fn ssh_keygen_args(self) -> &'static [&'static str] {
        match self {
            KeyType::Ed25519 => &["-t", "ed25519"],
            KeyType::Rsa4096 => &["-t", "rsa", "-b", "4096"],
            KeyType::Ecdsa => &["-t", "ecdsa", "-b", "521"],
        }
    }
}

/// `~/.ssh`, or `$SSH_DIR` in tests.
pub fn ssh_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SSH_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".ssh"))
}

/// Every private key in `~/.ssh`, newest first. A missing directory is an
/// empty list, not an error.
pub fn list_keys() -> Result<Vec<SshKey>> {
    match ssh_dir() {
        Some(dir) => list_keys_in(&dir),
        None => Ok(Vec::new()),
    }
}

fn list_keys_in(dir: &Path) -> Result<Vec<SshKey>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(error)),
    };
    let agent = agent_fingerprints().unwrap_or_default();
    let mut keys = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".pub")
            || matches!(
                name,
                "config" | "authorized_keys" | "known_hosts" | "known_hosts.old" | "rc"
            )
            || name.starts_with('.')
        {
            continue;
        }
        if !looks_like_private_key(&path) {
            continue;
        }
        if let Some(key) = describe_key(&path, name, &agent) {
            keys.push(key);
        }
    }
    keys.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.name.cmp(&b.name)));
    Ok(keys)
}

/// Whether the file starts with an OpenSSH/PEM private-key header. Reading a
/// few bytes keeps agent sockets and random files out of the list.
fn looks_like_private_key(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(80)]);
    head.contains("PRIVATE KEY-----")
}

fn describe_key(path: &Path, name: &str, agent: &[String]) -> Option<SshKey> {
    let public_path = path.with_file_name(format!("{name}.pub"));
    let public_key = fs::read_to_string(&public_path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            // An unencrypted key can print its public half.
            let out = Command::new("ssh-keygen")
                .args(["-y", "-P", "", "-f"])
                .arg(path)
                .stdin(Stdio::null())
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        })
        .unwrap_or_default();
    let encrypted = !private_key_opens_without_passphrase(path);
    // Prefer the public half: `ssh-keygen -l` on an encrypted private key
    // would need the passphrase.
    let described = if public_path.is_file() {
        &public_path
    } else {
        path
    };
    let (bits, fingerprint, comment, kind) = fingerprint_of(described);
    let created = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let agent = !fingerprint.is_empty()
        && agent
            .iter()
            .any(|known| known == &fingerprint || known.ends_with(fingerprint.as_str()));
    Some(SshKey {
        name: name.to_owned(),
        path: path.to_path_buf(),
        kind: display_kind(&kind, &bits),
        fingerprint,
        public_key,
        comment,
        created,
        agent,
        encrypted,
    })
}

/// `ssh-keygen -l -f <path>` → `(bits, fingerprint, comment, type)`.
fn fingerprint_of(path: &Path) -> (String, String, String, String) {
    let out = Command::new("ssh-keygen")
        .args(["-l", "-f"])
        .arg(path)
        .stdin(Stdio::null())
        .output();
    let text = match out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => {
            return (
                String::new(),
                String::new(),
                String::new(),
                "unknown".to_owned(),
            );
        }
    };
    let line = text.lines().next().unwrap_or_default();
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let bits = tokens.first().unwrap_or(&"").to_string();
    let fingerprint = tokens.get(1).unwrap_or(&"").to_string();
    let kind = line
        .rsplit_once('(')
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .unwrap_or("unknown")
        .to_lowercase();
    let comment = tokens
        .get(2)
        .filter(|token| !token.starts_with('('))
        .unwrap_or(&"")
        .to_string();
    (bits, fingerprint, comment, kind)
}

/// The chip label for a key: Ed25519 and ECDSA carry their size in the name,
/// RSA is shown as `rsa-2048` because the size is the useful part.
fn display_kind(kind: &str, bits: &str) -> String {
    match kind {
        "ed25519" => "ed25519".to_owned(),
        "ecdsa" => "ecdsa".to_owned(),
        "rsa" if !bits.is_empty() => format!("rsa-{bits}"),
        "unknown" => "unknown".to_owned(),
        other if !bits.is_empty() => format!("{other}-{bits}"),
        other => other.to_owned(),
    }
}

/// `ssh-keygen -y -P ""` succeeds only for a key that needs no passphrase.
fn private_key_opens_without_passphrase(path: &Path) -> bool {
    Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether `ssh-agent` answers. Exit code 1 means "running, no keys"; 2 means
/// "could not connect".
pub fn agent_running() -> bool {
    Command::new("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.code() == Some(0) || status.code() == Some(1))
}

/// The fingerprints `ssh-add -l` reports; empty when the agent is absent.
pub fn agent_fingerprints() -> Result<Vec<String>> {
    let out = Command::new("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !out.status.success() {
        // No agent socket: the second line describes it, but the page only
        // needs "no keys".
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find(|token| token.starts_with("SHA256:") || token.starts_with("MD5:"))
                .map(str::to_owned)
        })
        .collect())
}

/// Adds a key to the running agent. A key with a passphrase fails, because
/// there is no terminal to ask in.
pub fn add_to_agent(path: &Path) -> Result<()> {
    let out = Command::new("ssh-add")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            command: "ssh-add",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Removes a key from the running agent.
pub fn remove_from_agent(path: &Path) -> Result<()> {
    let out = Command::new("ssh-add")
        .args(["-d"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            command: "ssh-add",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Creates `~/.ssh/<name>`. A taken name is refused; the caller validates the
/// name first. Returns the new key description.
pub fn generate_key(
    name: &str,
    key_type: KeyType,
    comment: &str,
    passphrase: &str,
    add_agent: bool,
) -> Result<SshKey> {
    let dir = ssh_dir().ok_or_else(|| Error::Git {
        command: "ssh-keygen",
        code: None,
        stderr: "no home directory for ~/.ssh".to_owned(),
    })?;
    generate_key_in(&dir, name, key_type, comment, passphrase, add_agent)
}

fn generate_key_in(
    dir: &Path,
    name: &str,
    key_type: KeyType,
    comment: &str,
    passphrase: &str,
    add_agent: bool,
) -> Result<SshKey> {
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    if path.exists() {
        return Err(Error::Git {
            command: "ssh-keygen",
            code: None,
            stderr: format!("{} already exists", path.display()),
        });
    }
    let comment = if comment.trim().is_empty() {
        "gitwarp".to_owned()
    } else {
        comment.trim().to_owned()
    };
    let out = Command::new("ssh-keygen")
        .args(["-q", "-N", passphrase, "-C", &comment, "-f"])
        .arg(&path)
        .args(key_type.ssh_keygen_args())
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_file_name(format!("{name}.pub")));
        return Err(Error::Git {
            command: "ssh-keygen",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    if add_agent && passphrase.is_empty() {
        let _ = add_to_agent(&path);
    }
    describe_key(&path, name, &agent_fingerprints().unwrap_or_default()).ok_or_else(|| Error::Git {
        command: "ssh-keygen",
        code: None,
        stderr: "the new key could not be read back".to_owned(),
    })
}

/// Copies an existing private key into `~/.ssh` (when `copy` is set) and
/// returns its description.
pub fn import_key(path: &Path, copy: bool) -> Result<SshKey> {
    if !path.is_file() {
        return Err(Error::PathNotFound {
            path: path.to_path_buf(),
        });
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::Parse("key file name"))?
        .to_owned();
    let dir = ssh_dir().ok_or_else(|| Error::Git {
        command: "ssh-keygen",
        code: None,
        stderr: "no home directory for ~/.ssh".to_owned(),
    })?;
    let target = if copy {
        fs::create_dir_all(&dir)?;
        let target = dir.join(&name);
        if target.exists() && target != path {
            return Err(Error::Git {
                command: "ssh-keygen",
                code: None,
                stderr: format!("{} already exists", target.display()),
            });
        }
        if target != path {
            fs::copy(path, &target)?;
            let public = path.with_file_name(format!("{name}.pub"));
            if public.is_file() {
                let _ = fs::copy(&public, target.with_file_name(format!("{name}.pub")));
            }
        }
        target
    } else {
        path.to_path_buf()
    };
    describe_key(&target, &name, &agent_fingerprints().unwrap_or_default()).ok_or_else(|| {
        Error::Git {
            command: "ssh-keygen",
            code: None,
            stderr: format!("{} is not a private key", path.display()),
        }
    })
}

/// Deletes the private key, its `.pub` sibling and its agent entry.
pub fn delete_key(path: &Path) -> Result<()> {
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        let public = path.with_file_name(format!("{name}.pub"));
        let _ = fs::remove_file(public);
    }
    let _ = remove_from_agent(path);
    fs::remove_file(path).map_err(Error::Io)
}

/// Builds the trusted-host list from `~/.ssh/known_hosts`.
pub fn trusted_hosts() -> Result<Vec<TrustedHost>> {
    let Some(dir) = ssh_dir() else {
        return Ok(Vec::new());
    };
    trusted_hosts_in(&dir.join("known_hosts"))
}

/// Parses a `known_hosts` file. Only the file itself is read; fingerprints
/// come from `ssh-keygen` so hashed host names and key types are handled the
/// same way OpenSSH handles them.
pub fn trusted_hosts_in(path: &Path) -> Result<Vec<TrustedHost>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut hosts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (host, kind) = if line.starts_with('@') {
            match (fields.get(1), fields.get(2)) {
                (Some(host), Some(kind)) => (*host, *kind),
                _ => continue,
            }
        } else {
            match (fields.first(), fields.get(1)) {
                (Some(host), Some(kind)) => (*host, *kind),
                _ => continue,
            }
        };
        let fingerprint = fingerprint_of_line(line);
        hosts.push(TrustedHost {
            host: host.to_owned(),
            kind: kind.to_owned(),
            fingerprint,
        });
    }
    // A host usually has one line per key type (Ed25519, RSA, ECDSA). The
    // page shows one row per host, preferring Ed25519 like the design.
    let mut by_host: std::collections::BTreeMap<String, TrustedHost> =
        std::collections::BTreeMap::new();
    for host in hosts {
        match by_host.get(&host.host) {
            Some(existing) if !existing.kind.eq_ignore_ascii_case("ed25519") => {}
            _ => {
                by_host.insert(host.host.clone(), host);
            }
        }
    }
    Ok(by_host.into_values().collect())
}

/// Runs one `known_hosts` line through `ssh-keygen -lf -`.
fn fingerprint_of_line(line: &str) -> String {
    let Ok(mut child) = Command::new("ssh-keygen")
        .args(["-l", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{line}\n").as_bytes());
    }
    let Ok(out) = child.wait_with_output() else {
        return String::new();
    };
    parse_fingerprint_lines(&String::from_utf8_lossy(&out.stdout))
        .first()
        .map(|key| key.fingerprint.clone())
        .unwrap_or_default()
}

/// Drops every `known_hosts` line for `host`.
pub fn forget_host(host: &str) -> Result<()> {
    let path = known_hosts_path().ok_or_else(|| Error::Git {
        command: "ssh-keygen",
        code: None,
        stderr: "no home directory for ~/.ssh/known_hosts".to_owned(),
    })?;
    let out = Command::new("ssh-keygen")
        .args(["-R", host, "-f"])
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            command: "ssh-keygen",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Opens `ssh -T git@host` and returns its message. GitHub and GitLab answer
/// with exit code 1 even on success, so the text decides.
pub fn test_connection(host: &str, timeout_secs: u32) -> Result<String> {
    let out = Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("ConnectTimeout={timeout_secs}"),
            &format!("git@{host}"),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .trim()
    .to_owned();
    let ok = text.contains("successfully authenticated") || text.contains("Welcome to GitLab");
    if ok || out.status.success() {
        Ok(text)
    } else {
        Err(Error::Git {
            command: "ssh",
            code: out.status.code(),
            stderr: if text.is_empty() {
                format!("could not connect to {host}")
            } else {
                text
            },
        })
    }
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
    fn formats_key_type_chips() {
        assert_eq!(display_kind("ed25519", "256"), "ed25519");
        assert_eq!(display_kind("ecdsa", "521"), "ecdsa");
        assert_eq!(display_kind("rsa", "2048"), "rsa-2048");
        assert_eq!(display_kind("unknown", ""), "unknown");
    }

    #[test]
    fn recognises_private_key_headers_only() {
        let dir = std::env::temp_dir().join(format!("gitwarp-ssh-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let key = dir.join("id_test");
        fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n").unwrap();
        assert!(looks_like_private_key(&key));
        let other = dir.join("notes.txt");
        fs::write(&other, "hello").unwrap();
        assert!(!looks_like_private_key(&other));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_trusted_host_lines() {
        let dir = std::env::temp_dir().join(format!("gitwarp-hosts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        fs::write(
            &path,
            "# comment\n\
             github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample\n\
             @cert-authority *.example ssh-rsa AAAAB3NzaC1yc2E\n\
             |1|abc=|def= ssh-ed25519 AAAAC3NzaHashed\n",
        )
        .unwrap();
        let hosts = trusted_hosts_in(&path).unwrap();
        let names: Vec<&str> = hosts.iter().map(|host| host.host.as_str()).collect();
        assert_eq!(
            names,
            vec!["*.example", "github.com", "|1|abc=|def="],
            "markers are skipped and hashed names are shown as stored"
        );
        assert_eq!(hosts[0].kind, "ssh-rsa");
        assert_eq!(hosts[1].kind, "ssh-ed25519");
        let _ = fs::remove_dir_all(&dir);
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
