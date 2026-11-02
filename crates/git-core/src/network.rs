//! Network operations: fetch, pull, push and clone.
//!
//! All of them run the `git` CLI with `--progress`, read stderr as it arrives
//! and hand each parsed line to the caller. The UI runs these on a background
//! thread and paints the progress; the functions here stay free of any UI
//! type. Cancelling a clone kills the child process and removes the folder it
//! created.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::{check_output, Error, Git, Head, Repo, Result};

/// Extra environment (askpass, SSH settings) and whether git may interactively
/// prompt. `NetworkContext::default()` is non-interactive.
#[derive(Debug, Clone, Default)]
pub struct NetworkContext {
    /// Variables laid over git's own ones, e.g. `GIT_ASKPASS` or
    /// `GIT_TERMINAL_PROMPT=1`.
    pub env: Vec<(String, String)>,
}

impl NetworkContext {
    pub fn with_env(env: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            env: env.into_iter().collect(),
        }
    }

    /// A context in which git may ask for credentials through `GIT_ASKPASS`.
    pub fn interactive(env: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut context = Self::with_env(env);
        context
            .env
            .push(("GIT_TERMINAL_PROMPT".to_owned(), "1".to_owned()));
        context
    }
}

/// One parsed line of `--progress` output.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The phase git is in, e.g. `Receiving objects`. Empty for summary lines.
    pub phase: String,
    /// 0–100 when the line carries a percentage.
    pub percent: Option<f32>,
    /// The line as git printed it, without the trailing `\r`.
    pub text: String,
}

impl Progress {
    /// `Some(0.0..=1.0)` for the progress bar, `None` for summary lines.
    pub fn fraction(&self) -> Option<f32> {
        self.percent.map(|p| (p / 100.0).clamp(0.0, 1.0))
    }
}

/// The lines a finished operation printed and whether it moved any refs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkOutcome {
    /// Non-progress lines, in order. Fetch and push summaries live here.
    pub lines: Vec<String>,
    /// Whether the output reported a new, updated or deleted ref.
    pub changed: bool,
}

impl NetworkOutcome {
    /// All non-empty lines joined, for an error message or a tooltip.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// A cooperative stop flag for a running git child.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What to fetch and how.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// `None` fetches every configured remote.
    pub remote: Option<String>,
    /// Fetch tags too (`--tags`).
    pub tags: bool,
    /// Remove remote-tracking refs whose branch disappeared (`--prune`).
    pub prune: bool,
}

/// What to push and where.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushOptions {
    /// `None` pushes the current branch to its upstream (or to `origin`).
    pub remote: Option<String>,
    /// `None` pushes the current branch.
    pub refspec: Option<String>,
    /// `--set-upstream`: remember `remote/refspec` as the branch's upstream.
    pub set_upstream: bool,
}

/// Everything `clone` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneOptions {
    pub url: String,
    pub dest: PathBuf,
    /// `--branch`; also accepts a tag.
    pub branch: Option<String>,
    /// `--depth` when shallow.
    pub depth: Option<u32>,
    pub recurse_submodules: bool,
}

/// The kind of address a clone URL uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneKind {
    Https,
    Ssh,
    /// A path on this machine (`file://`, `/tmp/repo`, `../repo`).
    Local,
}

/// A clone URL decomposed for validation and the SSH host-key preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneTarget {
    pub kind: CloneKind,
    /// Host name for HTTPS and SSH; `None` for a local path.
    pub host: Option<String>,
    /// Repository folder name, without a trailing `.git`.
    pub name: String,
}

/// Parses a clone address. `None` means the text is not a Git address these
/// dialogs understand.
pub fn clone_target(url: &str) -> Option<CloneTarget> {
    let url = url.trim();
    if url.is_empty() || url.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let (host, path) = rest.split_once('/')?;
        if host.is_empty() || path.trim_matches('/').is_empty() {
            return None;
        }
        return Some(CloneTarget {
            kind: CloneKind::Https,
            host: Some(host.rsplit('@').next().unwrap_or(host).to_owned()),
            name: repo_name_from_path(path)?,
        });
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, tail)| tail);
        let (host_port, path) = rest.split_once('/')?;
        let host = host_port
            .rsplit_once(':')
            .map_or(host_port, |(host, _)| host);
        if host.is_empty() {
            return None;
        }
        return Some(CloneTarget {
            kind: CloneKind::Ssh,
            host: Some(host.to_owned()),
            name: repo_name_from_path(path)?,
        });
    }
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(CloneTarget {
            kind: CloneKind::Local,
            host: None,
            name: repo_name_from_path(rest)?,
        });
    }
    if let Some((authority, path)) = url.split_once(':') {
        // `user@host:path` (scp syntax) or `host:path`, but not a Windows
        // drive (`C:\repo`) and not a plain `host` without a path.
        let looks_scp =
            !authority.contains('/') && !authority.is_empty() && !path.trim_matches('/').is_empty();
        if looks_scp {
            let host = authority.rsplit('@').next().unwrap_or(authority);
            // A single-letter authority is a Windows drive, not a host.
            if host.len() > 1 && !host.contains('\\') {
                return Some(CloneTarget {
                    kind: CloneKind::Ssh,
                    host: Some(host.to_owned()),
                    name: repo_name_from_path(path)?,
                });
            }
        }
    }
    if url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("../")
        || url.starts_with('~')
    {
        return Some(CloneTarget {
            kind: CloneKind::Local,
            host: None,
            name: repo_name_from_path(url)?,
        });
    }
    None
}

/// The last path component without `.git`, after trimming slashes.
fn repo_name_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let leaf = trimmed.rsplit('/').next()?.trim_end_matches(".git");
    // `rsplit` over an empty string still yields one empty item.
    (!leaf.is_empty()).then_some(leaf.to_owned())
}

/// Fetches one remote, or every remote when `options.remote` is `None`.
pub fn fetch(
    repo: &Repo,
    options: &FetchOptions,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args(["fetch", "--progress"]);
    if options.tags {
        cmd.arg("--tags");
    }
    if options.prune {
        cmd.arg("--prune");
    }
    let label = match &options.remote {
        Some(remote) => {
            cmd.arg("--").arg(remote);
            "fetch"
        }
        None => {
            cmd.arg("--all");
            "fetch --all"
        }
    };
    run_streaming(cmd, label, context, cancel, on_progress)
}

/// Pulls the current branch, fast-forward only.
///
/// Merging is not implemented yet (stage 12 owns the conflict editor), so a
/// diverged branch becomes an error instead of a messy merge state.
pub fn pull(
    repo: &Repo,
    remote: Option<&str>,
    branch: Option<&str>,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    match repo.head()? {
        Head::Branch { .. } => {}
        Head::Detached(_) => {
            return Err(Error::Git {
                command: "pull",
                code: None,
                stderr: "Cannot pull with a detached HEAD.".to_owned(),
            });
        }
    }
    let mut cmd = repo.command();
    cmd.args(["pull", "--ff-only", "--progress"]);
    if let Some(remote) = remote {
        cmd.arg(remote);
        if let Some(branch) = branch {
            cmd.arg(branch);
        }
    }
    run_streaming(cmd, "pull", context, cancel, on_progress)
}

/// Initialises and updates one submodule (and its own submodules) to the
/// commit the superproject records. This may clone, so it runs through the
/// same streaming runner as fetch.
pub fn update_submodule(
    repo: &Repo,
    path: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args([
        "submodule",
        "update",
        "--init",
        "--recursive",
        "--progress",
        "--",
        path,
    ]);
    run_streaming(cmd, "submodule update", context, cancel, on_progress)
}

/// The URL `.gitmodules` records for the submodule at `path`, if any.
pub fn submodule_url(repo: &Repo, path: &str) -> Result<Option<String>> {
    let out = repo
        .command()
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ])
        .output()
        .map_err(Error::Spawn)?;
    // Exit 1 means no match (or no .gitmodules): nothing to preflight.
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let name = text.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (value == path).then(|| {
            key.strip_prefix("submodule.")
                .and_then(|rest| rest.strip_suffix(".path"))
                .map(str::to_owned)
        })?
    });
    let Some(name) = name else {
        return Ok(None);
    };
    let out = repo
        .command()
        .args(["config", "--file", ".gitmodules", "--get"])
        .arg(format!("submodule.{name}.url"))
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok((!url.is_empty()).then_some(url))
}

/// Pushes the current branch (or `options.refspec`) to `options.remote`.
pub fn push(
    repo: &Repo,
    options: &PushOptions,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args(["push", "--progress"]);
    if options.set_upstream {
        cmd.arg("--set-upstream");
    }
    if let Some(remote) = &options.remote {
        cmd.arg(remote);
    }
    if let Some(refspec) = &options.refspec {
        cmd.arg(refspec);
    }
    run_streaming(cmd, "push", context, cancel, on_progress)
}

/// Pushes one tag to `remote`.
pub fn push_tag(
    repo: &Repo,
    remote: &str,
    tag: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    push(
        repo,
        &PushOptions {
            remote: Some(remote.to_owned()),
            refspec: Some(format!("refs/tags/{tag}")),
            set_upstream: false,
        },
        context,
        cancel,
        on_progress,
    )
}

/// Pushes every tag to `remote`.
pub fn push_tags(
    repo: &Repo,
    remote: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args(["push", "--progress", "--tags", remote]);
    run_streaming(cmd, "push --tags", context, cancel, on_progress)
}

/// Deletes `branch` on `remote` (`git push <remote> --delete <branch>`).
pub fn delete_remote_branch(
    repo: &Repo,
    remote: &str,
    branch: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args(["push", "--progress", remote, "--delete", branch]);
    run_streaming(cmd, "push --delete", context, cancel, on_progress)
}

/// Makes `remote/branch` the upstream of the local branch `local`.
pub fn set_upstream(repo: &Repo, local: &str, remote: &str, branch: &str) -> Result<()> {
    let upstream = format!("{remote}/{branch}");
    let out = repo
        .command()
        .args(["branch", "--set-upstream-to", &upstream, local])
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "branch --set-upstream-to").map(drop)
}

/// Clones `options.url` into `options.dest`, reporting progress as it goes.
///
/// Cancelling kills git and removes the destination folder when clone created
/// it; an existing folder (git refuses those anyway) is left alone.
pub fn clone(
    git: &Git,
    options: &CloneOptions,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let created = !options.dest.exists();
    let mut cmd = git.command();
    cmd.args(["clone", "--progress"]);
    if let Some(branch) = &options.branch {
        cmd.args(["--branch", branch]);
    }
    if let Some(depth) = options.depth {
        cmd.args(["--depth", &depth.to_string()]);
    }
    if options.recurse_submodules {
        cmd.arg("--recurse-submodules");
    }
    cmd.arg("--").arg(&options.url).arg(&options.dest);

    let result = run_streaming(cmd, "clone", context, cancel, on_progress);
    if result.is_err() && created && options.dest.exists() {
        let _ = std::fs::remove_dir_all(&options.dest);
    }
    result
}

/// The per-file name of a repository URL, for the clone dialog's default.
pub fn suggested_folder(url: &str) -> Option<String> {
    clone_target(url).map(|target| target.name)
}

/// The remote a toolbar action should use: `origin` when configured, the
/// first remote otherwise.
pub fn default_remote(repo: &Repo) -> Result<Option<String>> {
    let remotes = repo.remotes()?;
    if remotes.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        remotes
            .iter()
            .find(|remote| remote.as_str() == "origin")
            .cloned()
            .unwrap_or_else(|| remotes[0].clone()),
    ))
}

/// The URL of `remote`, or `None` when it is not configured.
pub fn remote_url(repo: &Repo, remote: &str) -> Result<Option<String>> {
    repo.remote_url(remote)
}

/// The feed half of the progress protocol: split git's stderr on `\r` and
/// `\n`, because progress lines overwrite themselves.
fn read_progress_lines<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(index) = pending
                        .iter()
                        .position(|byte| *byte == b'\r' || *byte == b'\n')
                    {
                        let line: Vec<u8> = pending.drain(..=index).collect();
                        let text = String::from_utf8_lossy(&line[..line.len() - 1])
                            .trim_end()
                            .to_owned();
                        if !text.is_empty() && tx.send(text).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending).trim_end().to_owned();
            if !text.is_empty() {
                let _ = tx.send(text);
            }
        }
    })
}

/// Runs `cmd` with `--progress` and streams its stderr.
fn run_streaming(
    mut cmd: Command,
    command: &'static str,
    context: &NetworkContext,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in &context.env {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().map_err(Error::Spawn)?;
    let stderr = child.stderr.take().ok_or(Error::Parse("git stderr"))?;
    let (line_tx, line_rx) = mpsc::channel();
    let reader = read_progress_lines(stderr, line_tx);

    let mut outcome = NetworkOutcome::default();
    let mut last_emitted: Option<(String, Option<f32>)> = None;
    let mut cancelled = false;
    loop {
        if cancel.is_cancelled() {
            cancelled = true;
            let _ = child.kill();
            break;
        }
        match line_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                let progress = parse_progress(&line);
                let key = (progress.text.clone(), progress.percent);
                if last_emitted.as_ref() != Some(&key) {
                    last_emitted = Some(key);
                    on_progress(progress.clone());
                }
                if !line.contains('%') {
                    let moved_ref = line.contains("->")
                        || line.contains("[new ")
                        || line.contains("[deleted]")
                        || line.contains("forced update");
                    if moved_ref {
                        outcome.changed = true;
                    }
                    if outcome.lines.len() < 200 {
                        outcome.lines.push(line);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait();
    let _ = reader.join();
    // Drain the tail that arrived while we were waiting for the child.
    while let Ok(line) = line_rx.try_recv() {
        if !line.contains('%') && outcome.lines.len() < 200 {
            outcome.lines.push(line);
        }
    }
    if cancelled {
        return Err(Error::Cancelled { command });
    }
    match status {
        Ok(status) if status.success() => Ok(outcome),
        Ok(status) => Err(Error::Git {
            command,
            code: status.code(),
            stderr: if outcome.lines.is_empty() {
                "git exited with an error".to_owned()
            } else {
                outcome.text()
            },
        }),
        Err(error) => Err(Error::Io(error)),
    }
}

/// `Receiving objects:  42% (…)` → phase, percent and the original line.
fn parse_progress(line: &str) -> Progress {
    let text = line.trim().to_owned();
    let phase = text
        .split_once(':')
        .map(|(phase, _)| phase.trim().to_owned())
        .unwrap_or_default();
    let percent = text.find('%').and_then(|index| {
        let digits = text[..index]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() {
            return None;
        }
        digits.chars().rev().collect::<String>().parse::<f32>().ok()
    });
    Progress {
        phase,
        percent,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clone_addresses() {
        let target = clone_target("https://github.com/nexu-io/open-design.git").unwrap();
        assert_eq!(target.kind, CloneKind::Https);
        assert_eq!(target.host.as_deref(), Some("github.com"));
        assert_eq!(target.name, "open-design");

        let target = clone_target("git@github.com:nexu-io/open-design.git").unwrap();
        assert_eq!(target.kind, CloneKind::Ssh);
        assert_eq!(target.host.as_deref(), Some("github.com"));
        assert_eq!(target.name, "open-design");

        let target = clone_target("ssh://git@git.example:2222/team/repo.git").unwrap();
        assert_eq!(target.kind, CloneKind::Ssh);
        assert_eq!(target.host.as_deref(), Some("git.example"));
        assert_eq!(target.name, "repo");

        assert_eq!(clone_target("github.com/nexu-io"), None);
        assert_eq!(clone_target("https://github.com"), None);
        assert_eq!(clone_target("  "), None);

        let local = clone_target("/tmp/repo.git").unwrap();
        assert_eq!(local.kind, CloneKind::Local);
        assert_eq!(local.name, "repo");
        assert_eq!(local.host, None);

        let file = clone_target("file:///tmp/repo.git").unwrap();
        assert_eq!(file.kind, CloneKind::Local);
        assert_eq!(file.name, "repo");
    }

    #[test]
    fn parses_progress_lines() {
        let progress = parse_progress("Receiving objects:  42% (420/1000), 1.2 MiB | 3.0 MiB/s");
        assert_eq!(progress.phase, "Receiving objects");
        assert_eq!(progress.percent, Some(42.0));
        assert_eq!(progress.fraction(), Some(0.42));

        let summary = parse_progress(" * [new branch]      main -> origin/main");
        assert_eq!(summary.percent, None);
        assert_eq!(summary.phase, "");
    }

    #[test]
    fn progress_is_capped() {
        assert_eq!(
            Progress {
                phase: String::new(),
                percent: Some(120.0),
                text: String::new()
            }
            .fraction(),
            Some(1.0)
        );
    }
}
