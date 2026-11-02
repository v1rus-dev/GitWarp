//! Ref and history operations: checkout, branches, tags, remotes, cherry-pick,
//! revert, reset and the stash.
//!
//! All of them run the `git` CLI, like the read paths, so hooks, signing
//! configuration and credential helpers keep working. The UI only sees plain
//! [`Result`]s and turns git's own message into the error banner.

use std::process::Output;

use crate::{check_output, Error, Oid, Repo, Result};

/// How much of the work tree and index `git reset` rewrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Moves HEAD only; index and work tree stay as they are.
    Soft,
    /// Moves HEAD and resets the index; the work tree keeps its files.
    Mixed,
    /// Moves HEAD, the index and the work tree: local changes are lost.
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// Checks out a local branch.
pub(crate) fn checkout_branch(repo: &Repo, name: &str) -> Result<()> {
    run(repo, &["switch", name], "switch")
}

/// Checks out a commit with a detached HEAD.
pub(crate) fn checkout_detached(repo: &Repo, oid: Oid) -> Result<()> {
    let oid = oid.to_string();
    run(repo, &["switch", "--detach", &oid], "switch --detach")
}

/// Checks out a remote branch as a local tracking branch.
///
/// When a local branch with the remote's leaf name already exists, that branch
/// is checked out instead: creating it again would fail. The leaf name is the
/// part after the remote's first `/`, so `origin/feat/x` becomes `feat/x`.
pub(crate) fn checkout_remote(repo: &Repo, remote_ref: &str) -> Result<()> {
    let local = remote_ref
        .split_once('/')
        .map_or(remote_ref, |(_, leaf)| leaf);
    if has_local_branch(repo, local)? {
        return checkout_branch(repo, local);
    }
    run(repo, &["switch", "--track", remote_ref], "switch --track")
}

fn has_local_branch(repo: &Repo, name: &str) -> Result<bool> {
    let out = repo
        .command()
        .args(["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")])
        .output()
        .map_err(Error::Spawn)?;
    Ok(out.status.success())
}

/// Creates `name` at `start`, and optionally switches to it.
pub(crate) fn create_branch(repo: &Repo, name: &str, start: Oid, checkout: bool) -> Result<()> {
    let start = start.to_string();
    if checkout {
        run(repo, &["switch", "-c", name, &start], "switch -c")
    } else {
        run(repo, &["branch", name, &start], "branch")
    }
}

/// Renames a local branch, moving it when it is the checked-out one.
pub(crate) fn rename_branch(repo: &Repo, old: &str, new: &str) -> Result<()> {
    run(repo, &["branch", "-m", old, new], "branch -m")
}

/// Deletes a local branch. `force` drops unmerged commits with it.
pub(crate) fn delete_branch(repo: &Repo, name: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };
    let command = if force { "branch -D" } else { "branch -d" };
    run(repo, &["branch", flag, name], command)
}

/// Creates a lightweight tag, or an annotated/signed one when `message` is
/// given. A signed tag also needs a message; the signing key comes from git's
/// own configuration.
pub(crate) fn create_tag(
    repo: &Repo,
    name: &str,
    target: Oid,
    message: Option<&str>,
    sign: bool,
) -> Result<()> {
    let target = target.to_string();
    match (message, sign) {
        (Some(message), true) => run(repo, &["tag", "-s", "-m", message, name, &target], "tag -s"),
        (Some(message), false) => run(repo, &["tag", "-a", "-m", message, name, &target], "tag -a"),
        (None, _) => run(repo, &["tag", name, &target], "tag"),
    }
}

/// Renames a tag. A lightweight tag stays lightweight; an annotated one keeps
/// its message but gets a new tagger (and loses a signature, if it had one).
pub(crate) fn rename_tag(repo: &Repo, old: &str, new: &str) -> Result<()> {
    let out = repo
        .command()
        .args([
            "for-each-ref",
            "--format=%(objecttype)%1f%(objectname)%1f%(*objectname)%1f%(contents)",
            &format!("refs/tags/{old}"),
        ])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "for-each-ref")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.splitn(4, '\x1f');
    let object_type = fields.next().unwrap_or("");
    let object = fields.next().unwrap_or("");
    let peeled = fields.next().unwrap_or("");
    let contents = fields.next().unwrap_or("");
    if object.is_empty() {
        return Err(Error::Git {
            command: "for-each-ref",
            code: Some(1),
            stderr: format!("tag {old} not found"),
        });
    }
    let target = Oid::from_hex(if peeled.is_empty() { object } else { peeled })
        .ok_or(Error::Parse("tag target"))?;
    if object_type == "tag" {
        let message = contents.trim_end_matches('\n');
        create_tag(repo, new, target, Some(message), false)?;
    } else {
        create_tag(repo, new, target, None, false)?;
    }
    delete_tag(repo, old)
}

pub(crate) fn delete_tag(repo: &Repo, name: &str) -> Result<()> {
    run(repo, &["tag", "-d", name], "tag -d")
}

pub(crate) fn add_remote(repo: &Repo, name: &str, url: &str) -> Result<()> {
    run(repo, &["remote", "add", name, url], "remote add")
}

pub(crate) fn set_remote_url(repo: &Repo, name: &str, url: &str) -> Result<()> {
    run(repo, &["remote", "set-url", name, url], "remote set-url")
}

pub(crate) fn rename_remote(repo: &Repo, old: &str, new: &str) -> Result<()> {
    run(repo, &["remote", "rename", old, new], "remote rename")
}

pub(crate) fn remove_remote(repo: &Repo, name: &str) -> Result<()> {
    run(repo, &["remote", "remove", name], "remote remove")
}

/// The fetch URL of a configured remote, or `None` when the name is unknown.
pub(crate) fn remote_url(repo: &Repo, name: &str) -> Result<Option<String>> {
    let out = repo
        .command()
        .args(["remote", "get-url", name])
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok((!url.is_empty()).then_some(url))
}

/// Every push URL of a configured remote, one per `remote.<name>.pushurl`
/// entry. Git answers with the fetch URL when no `pushurl` is set, and so does
/// this: the list is never empty for a configured remote. An unknown name
/// gives an empty list.
pub(crate) fn remote_push_urls(repo: &Repo, name: &str) -> Result<Vec<String>> {
    let out = repo
        .command()
        .args(["remote", "get-url", "--push", "--all", name])
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Applies one commit on top of HEAD as a new commit.
///
/// A conflict is aborted before it reaches the UI: there is no conflict editor
/// yet, and leaving `CHERRY_PICK_HEAD` behind would make every later operation
/// fail until the user cleaned it up in a terminal.
pub(crate) fn cherry_pick(repo: &Repo, oid: Oid) -> Result<()> {
    let oid = oid.to_string();
    let out = repo
        .command()
        .args(["cherry-pick", &oid])
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        return Ok(());
    }
    let error = failure("cherry-pick", out);
    abort(repo, "cherry-pick");
    Err(error)
}

/// Reverts one commit with a new commit, aborting a conflicted revert.
pub(crate) fn revert(repo: &Repo, oid: Oid) -> Result<()> {
    let oid = oid.to_string();
    let out = repo
        .command()
        .args(["revert", "--no-edit", &oid])
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        return Ok(());
    }
    let error = failure("revert", out);
    abort(repo, "revert");
    Err(error)
}

fn abort(repo: &Repo, operation: &str) {
    let _ = repo
        .command()
        .args([operation, "--abort"])
        .output()
        .map(drop);
}

pub(crate) fn reset(repo: &Repo, oid: Oid, mode: ResetMode) -> Result<()> {
    let oid = oid.to_string();
    run(repo, &["reset", mode.flag(), &oid], "reset")
}

/// Stashes every change, including untracked files. Returns `false` when there
/// was nothing to stash, so the caller can say so instead of toasting success.
pub(crate) fn stash_save(repo: &Repo, message: Option<&str>) -> Result<bool> {
    let status = crate::status::status(repo)?;
    if status.staged.is_empty() && status.unstaged.is_empty() {
        return Ok(false);
    }
    let mut args = vec!["stash", "push", "--include-untracked"];
    if let Some(message) = message {
        args.extend(["-m", message]);
    }
    run(repo, &args, "stash push")?;
    Ok(true)
}

pub(crate) fn stash_apply(repo: &Repo, index: usize) -> Result<()> {
    let reference = format!("stash@{{{index}}}");
    run(repo, &["stash", "apply", &reference], "stash apply")
}

pub(crate) fn stash_pop(repo: &Repo, index: usize) -> Result<()> {
    let reference = format!("stash@{{{index}}}");
    run(repo, &["stash", "pop", &reference], "stash pop")
}

pub(crate) fn stash_drop(repo: &Repo, index: usize) -> Result<()> {
    let reference = format!("stash@{{{index}}}");
    run(repo, &["stash", "drop", &reference], "stash drop")
}

/// The user's configured signing key, for the tag dialog's "Sign with" label.
pub(crate) fn signing_key(repo: &Repo) -> Result<Option<String>> {
    let out = repo
        .command()
        .args(["config", "--get", "user.signingkey"])
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    let key = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok((!key.is_empty()).then_some(key))
}

fn run(repo: &Repo, args: &[&str], command: &'static str) -> Result<()> {
    let out = repo.command().args(args).output().map_err(Error::Spawn)?;
    check_output(out, command).map(drop)
}

fn failure(command: &'static str, out: Output) -> Error {
    Error::Git {
        command,
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}
