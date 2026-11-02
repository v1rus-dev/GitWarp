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
    /// `key=value` pairs passed to git as `-c key=value` before the
    /// subcommand, e.g. `credential.https://host.helper=`. They go on the
    /// command line, so they must never hold a secret.
    pub git_config: Vec<String>,
}

impl NetworkContext {
    pub fn with_env(env: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            env: env.into_iter().collect(),
            git_config: Vec::new(),
        }
    }

    /// Adds `-c key=value` settings for the git command.
    pub fn with_git_config(mut self, config: impl IntoIterator<Item = String>) -> Self {
        self.git_config.extend(config);
        self
    }

    /// The `-c key=value` arguments, to be placed before the subcommand.
    pub fn config_args(&self) -> Vec<String> {
        self.git_config
            .iter()
            .flat_map(|setting| ["-c".to_owned(), setting.clone()])
            .collect()
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
    /// One entry per push URL when a push went to each of several URLs
    /// separately ([`push_each`]); empty otherwise.
    pub targets: Vec<PushTargetOutcome>,
}

/// What one push URL of a multi-push remote answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTargetOutcome {
    pub url: String,
    /// `Err` holds the reason: the porcelain summary of a refused ref
    /// (`rejected (non-fast-forward)`), otherwise git's own text, for the
    /// caller to classify and word.
    pub result: std::result::Result<PushStatus, String>,
    /// The target refused some refs but took others of the same push.
    pub partial: bool,
}

/// A target that took the push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    UpToDate,
    /// `commits` is how many commits the target did not have, when git said.
    Pushed {
        commits: Option<usize>,
    },
    /// A branch the target did not have, when its commits could not be counted.
    NewBranch,
    /// A tag the target did not have.
    NewTag,
    /// The ref was deleted on the target.
    Deleted,
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
    cmd.args(context.config_args());
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

/// Fetches one refspec of one remote, e.g. a pull request's head ref into a
/// local branch (`refs/pull/5/head:refs/heads/feat`). The remote and the
/// refspec come after `--`, so neither can be read as an option.
pub fn fetch_refspec(
    repo: &Repo,
    remote: &str,
    refspec: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let mut cmd = repo.command();
    cmd.args(context.config_args());
    cmd.args(["fetch", "--progress", "--"])
        .arg(remote)
        .arg(refspec);
    run_streaming(cmd, "fetch", context, cancel, on_progress)
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
    cmd.args(context.config_args());
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
    cmd.args(context.config_args());
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
    cmd.args(context.config_args());
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

/// Pushes to each of `urls` in its own git process and reports a result per
/// URL, in order.
///
/// One `git push <remote>` to several push URLs stops at the first transport
/// or authentication failure, so the later targets would never be tried.
/// Afterwards, when at least one target took the push, the remote-tracking
/// ref (and the upstream, with `options.set_upstream`) is updated the way git
/// does for a named remote. A failed target never fails the call; only
/// cancelling does.
pub fn push_each(
    repo: &Repo,
    urls: &[String],
    options: &PushOptions,
    context: &NetworkContext,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let (refspec, local, branch) = push_refspec(repo, options)?;
    let mut outcome = NetworkOutcome::default();
    for url in urls {
        // Pushing to a URL instead of the remote's name bypasses
        // `remote.<name>.push`, `push.default` and `push.followTags` on
        // purpose: the app always passes an explicit refspec, and the
        // tracking ref is recorded below the way a named remote would.
        let mut cmd = repo.command();
        cmd.args(context.config_args());
        cmd.args(["push", "--progress", "--porcelain", "--", url, &refspec]);
        let (result, stdout) =
            run_streaming_inner(cmd, "push", context, cancel, &mut on_progress, true);
        let refs = parse_porcelain(&stdout);
        let mut partial = false;
        let result = match result {
            Ok(done) => {
                outcome.lines.extend(done.lines);
                push_status(repo, &refs)
            }
            Err(error @ Error::Cancelled { .. }) => return Err(error),
            // git exits 1 when a ref is refused, but still lists every ref on
            // stdout: the refusal reason is there, not in the exit text.
            Err(Error::Git { .. }) if refs.iter().any(|r| r.flag == '!') => {
                partial = refs.iter().any(|r| !matches!(r.flag, '!' | '='));
                Err(refused_reasons(&refs))
            }
            Err(Error::Git { stderr, .. }) => Err(stderr),
            Err(error) => Err(error.to_string()),
        };
        outcome.targets.push(PushTargetOutcome {
            url: url.clone(),
            result,
            partial,
        });
    }
    outcome.changed = outcome
        .targets
        .iter()
        .any(|t| matches!(t.result, Ok(PushStatus::Pushed { .. })));
    let any_ok = outcome.targets.iter().any(|t| t.result.is_ok());
    if let (true, Some(remote), Some(branch), Some(local)) =
        (any_ok, &options.remote, branch, local)
    {
        record_push(repo, remote, &local, &branch, options.set_upstream)?;
    }
    Ok(outcome)
}

/// The refspec to push and, when it pushes a local branch to a branch, that
/// branch's local and remote names.
fn push_refspec(
    repo: &Repo,
    options: &PushOptions,
) -> Result<(String, Option<String>, Option<String>)> {
    // `HEAD` is never passed on as a refspec: the tracking ref needs the
    // branch's name, so it is replaced by the checked-out branch.
    let refspec = match options.refspec.as_ref().filter(|r| r.as_str() != "HEAD") {
        Some(refspec) => refspec.clone(),
        None => match repo.head()? {
            Head::Branch { name, .. } => name,
            Head::Detached(_) => {
                return Err(Error::Git {
                    command: "push",
                    code: None,
                    stderr: "Cannot push with a detached HEAD.".to_owned(),
                });
            }
        },
    };
    let spec = refspec.trim_start_matches('+');
    let (src, dst) = spec.split_once(':').unwrap_or((spec, spec));
    let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
    // A tag, a deletion (`:branch`) or any other ref has no tracking branch.
    let branch_push = !src.is_empty() && !dst.is_empty() && !dst.starts_with("refs/");
    let local = src.strip_prefix("refs/heads/").unwrap_or(src);
    Ok((
        refspec.clone(),
        branch_push.then(|| local.to_owned()),
        branch_push.then(|| dst.to_owned()),
    ))
}

/// Points `refs/remotes/<remote>/<branch>` at what was pushed and, when asked,
/// makes it the upstream of `local`.
fn record_push(
    repo: &Repo,
    remote: &str,
    local: &str,
    branch: &str,
    set_upstream_to: bool,
) -> Result<()> {
    let out = repo
        .command()
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{local}^{{commit}}"))
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-parse")?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let out = repo
        .command()
        .args(["update-ref"])
        .arg(format!("refs/remotes/{remote}/{branch}"))
        .arg(&sha)
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "update-ref")?;
    if set_upstream_to {
        set_upstream(repo, local, remote, branch)?;
    }
    Ok(())
}

/// One ref line of `git push --porcelain`: `<flag>\t<from>:<to>\t<summary>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainRef {
    /// ` ` fast-forward, `+` forced, `-` deleted, `*` new, `!` rejected,
    /// `=` up to date.
    pub flag: char,
    pub refs: String,
    /// `abc..def`, `[new branch]`, `[rejected] (non-fast-forward)`, ...
    pub summary: String,
}

/// The ref lines of `git push --porcelain` output; `To <url>` and `Done` are
/// skipped.
pub fn parse_porcelain(stdout: &str) -> Vec<PorcelainRef> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let flag = parts.next()?;
            let refs = parts.next()?;
            let summary = parts.next()?;
            let mut chars = flag.chars();
            let flag = chars.next()?;
            if chars.next().is_some() || !matches!(flag, ' ' | '+' | '-' | '*' | '!' | '=') {
                return None;
            }
            Some(PorcelainRef {
                flag,
                refs: refs.to_owned(),
                summary: summary.trim().to_owned(),
            })
        })
        .collect()
}

/// The status of one target from its porcelain lines. A rejected ref is an
/// error even though the other refs of the same push may have gone through.
fn push_status(repo: &Repo, refs: &[PorcelainRef]) -> std::result::Result<PushStatus, String> {
    if refs.iter().any(|r| r.flag == '!') {
        return Err(refused_reasons(refs));
    }
    let moved: Vec<&PorcelainRef> = refs.iter().filter(|r| r.flag != '=').collect();
    if moved.is_empty() {
        // Only `=` lines say "up to date"; no ref lines at all say nothing.
        return Ok(if refs.is_empty() {
            PushStatus::Pushed { commits: None }
        } else {
            PushStatus::UpToDate
        });
    }
    let commits = moved
        .iter()
        .map(|r| commits_pushed(repo, r))
        .try_fold(0usize, |sum, count| count.map(|count| sum + count));
    if commits.is_none() && moved.len() == 1 {
        let only = moved[0];
        let to_tag = only
            .refs
            .rsplit_once(':')
            .is_some_and(|(_, to)| to.starts_with("refs/tags/"));
        match only.flag {
            '-' => return Ok(PushStatus::Deleted),
            '*' if to_tag => return Ok(PushStatus::NewTag),
            '*' => return Ok(PushStatus::NewBranch),
            _ => {}
        }
    }
    Ok(PushStatus::Pushed { commits })
}

/// The reasons of the refused (`!`) refs, from git's summary text:
/// `[remote rejected] (pre-receive hook declined)` becomes
/// `remote rejected (pre-receive hook declined)`.
fn refused_reasons(refs: &[PorcelainRef]) -> String {
    let mut reasons: Vec<String> = Vec::new();
    for r in refs.iter().filter(|r| r.flag == '!') {
        let reason = r.summary.replace(['[', ']'], "");
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    reasons.join("; ")
}

/// How many commits a pushed ref added, from git's `old..new` summary or, for
/// a new branch, from what no remote-tracking ref holds yet.
fn commits_pushed(repo: &Repo, pushed: &PorcelainRef) -> Option<usize> {
    let range = match pushed.flag {
        ' ' | '+' => {
            let (old, new) = pushed
                .summary
                .split_once("...")
                .or_else(|| pushed.summary.split_once(".."))?;
            let new = new.split_whitespace().next()?;
            format!("{old}..{new}")
        }
        '*' => {
            let (local, to) = pushed.refs.split_once(':')?;
            // A new tag is not counted in commits.
            if to.starts_with("refs/tags/") {
                return None;
            }
            let out = repo
                .command()
                .args(["rev-list", "--count", local, "--not", "--remotes"])
                .output()
                .ok()?;
            return String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .ok()
                .filter(|count| *count > 0);
        }
        _ => return None,
    };
    let out = repo
        .command()
        .args(["rev-list", "--count", &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
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
    cmd.args(context.config_args());
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
    cmd.args(context.config_args());
    cmd.args(["push", "--progress", remote, "--delete", branch]);
    run_streaming(cmd, "push --delete", context, cancel, on_progress)
}

/// Deletes `branch` on each of `urls` in its own git process (see
/// [`push_each`]) and, when at least one target took it, drops the
/// remote-tracking ref like `git push <remote> --delete` does.
pub fn delete_each(
    repo: &Repo,
    urls: &[String],
    remote: &str,
    branch: &str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let options = PushOptions {
        remote: None,
        refspec: Some(format!(":refs/heads/{branch}")),
        set_upstream: false,
    };
    let outcome = push_each(repo, urls, &options, context, cancel, on_progress)?;
    if outcome.targets.iter().any(|t| t.result.is_ok()) {
        let tracking = format!("refs/remotes/{remote}/{branch}");
        let exists = repo
            .command()
            .args(["rev-parse", "--verify", "--quiet", &tracking])
            .output()
            .map_err(Error::Spawn)?
            .status
            .success();
        if exists {
            let out = repo
                .command()
                .args(["update-ref", "-d", &tracking])
                .output()
                .map_err(Error::Spawn)?;
            check_output(out, "update-ref")?;
        }
    }
    Ok(outcome)
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
    cmd.args(context.config_args());
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
    cmd: Command,
    command: &'static str,
    context: &NetworkContext,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    run_streaming_inner(cmd, command, context, cancel, &mut on_progress, false).0
}

/// [`run_streaming`], optionally keeping what git printed on stdout (for
/// `push --porcelain`). The text is returned for failures too.
fn run_streaming_inner(
    mut cmd: Command,
    command: &'static str,
    context: &NetworkContext,
    cancel: &CancelToken,
    on_progress: &mut impl FnMut(Progress),
    capture_stdout: bool,
) -> (Result<NetworkOutcome>, String) {
    cmd.stdin(Stdio::null())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped());
    for (key, value) in &context.env {
        cmd.env(key, value);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => return (Err(Error::Spawn(error)), String::new()),
    };
    let Some(stderr) = child.stderr.take() else {
        return (Err(Error::Parse("git stderr")), String::new());
    };
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        })
    });
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
    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    // Drain the tail that arrived while we were waiting for the child.
    while let Ok(line) = line_rx.try_recv() {
        if !line.contains('%') && outcome.lines.len() < 200 {
            outcome.lines.push(line);
        }
    }
    if cancelled {
        return (Err(Error::Cancelled { command }), stdout);
    }
    let result = match status {
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
    };
    (result, stdout)
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

    #[test]
    fn config_settings_become_dash_c_arguments() {
        let context = NetworkContext::default().with_git_config([
            "credential.https://a.test.helper=".to_owned(),
            "credential.https://b.test.helper=".to_owned(),
        ]);
        assert_eq!(
            context.config_args(),
            [
                "-c",
                "credential.https://a.test.helper=",
                "-c",
                "credential.https://b.test.helper="
            ]
        );
        assert!(NetworkContext::default().config_args().is_empty());
    }

    #[test]
    fn parses_porcelain_ref_lines() {
        let out = "To /srv/a.git\n \trefs/heads/main:refs/heads/main\t1a2b3c4..5d6e7f8\n\
                   *\trefs/heads/x:refs/heads/x\t[new branch]\n\
                   =\trefs/heads/y:refs/heads/y\t[up to date]\n\
                   !\trefs/heads/z:refs/heads/z\t[rejected] (non-fast-forward)\n\
                   -\t:refs/heads/gone\t[deleted]\nDone\n";
        let refs = parse_porcelain(out);
        let flags: Vec<char> = refs.iter().map(|r| r.flag).collect();
        assert_eq!(flags, [' ', '*', '=', '!', '-']);
        assert_eq!(refs[0].refs, "refs/heads/main:refs/heads/main");
        assert_eq!(refs[0].summary, "1a2b3c4..5d6e7f8");
        assert_eq!(refs[3].summary, "[rejected] (non-fast-forward)");
        assert!(parse_porcelain("To x\nDone\n").is_empty());
        assert!(parse_porcelain("").is_empty());
    }
}
