//! Ref and history operations: checkout, branches, tags, remotes, merge (with a
//! conflict preview), cherry-pick, revert, reset and the stash.
//!
//! All of them run the `git` CLI, like the read paths, so hooks, signing
//! configuration and credential helpers keep working. The UI only sees plain
//! [`Result`]s and turns git's own message into the error banner.

use std::process::Output;

use crate::{check_output, Error, GitVersion, Oid, Repo, Result};

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

/// Checks out a remote branch as its local counterpart.
///
/// A missing local branch is created with `git switch --track`. An existing
/// one is switched to, fast-forwarded to the remote when it is strictly behind,
/// and gets an upstream only when it has none: an existing upstream and any
/// local commits are never overwritten.
///
/// The local name is the remote ref without its remote prefix. Remotes can be
/// named anything (`upstream`, …) and branch names may contain `/`, so the
/// prefix is matched against the configured remotes rather than the first `/`.
pub(crate) fn checkout_remote(repo: &Repo, remote_ref: &str) -> Result<()> {
    let local = remote_branch_leaf(repo, remote_ref)?;
    if !has_local_branch(repo, &local)? {
        return run(repo, &["switch", "--track", remote_ref], "switch --track");
    }
    // A branch checked out in another worktree makes git itself fail here; the
    // shell's "open that worktree" dialog handles the error.
    checkout_branch(repo, &local)?;
    let local_ref = format!("refs/heads/{local}");
    let remote = format!("refs/remotes/{remote_ref}");
    if is_ancestor(repo, &local_ref, &remote)? && !is_ancestor(repo, &remote, &local_ref)? {
        run(repo, &["merge", "--ff-only", &remote], "merge --ff-only")?;
    }
    if !has_upstream(repo, &local)? {
        run(
            repo,
            &["branch", "--set-upstream-to", &remote, &local],
            "branch --set-upstream-to",
        )?;
    }
    Ok(())
}

/// The local branch name of a remote ref: `origin/fix/x` becomes `fix/x`.
///
/// The remote prefix is the longest configured remote that matches, so a
/// remote called `upstream` and branches with `/` in their names both work.
fn remote_branch_leaf(repo: &Repo, remote_ref: &str) -> Result<String> {
    let remotes = repo.remotes()?;
    let remote = remotes
        .iter()
        .filter(|remote| {
            remote_ref
                .strip_prefix(remote.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('/'))
        })
        .max_by_key(|remote| remote.len());
    Ok(match remote {
        Some(remote) => remote_ref[remote.len() + 1..].to_owned(),
        // Not a configured remote (or still loading): keep the old first-`/`
        // split so a stale tracking ref still checks out somewhere sensible.
        None => remote_ref
            .split_once('/')
            .map_or(remote_ref, |(_, leaf)| leaf)
            .to_owned(),
    })
}

/// Whether `ancestor` is an ancestor of `descendant` (`git merge-base`).
fn is_ancestor(repo: &Repo, ancestor: &str, descendant: &str) -> Result<bool> {
    let out = repo
        .command()
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(Error::Spawn)?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failure("merge-base --is-ancestor", out)),
    }
}

/// Whether a local branch has any upstream configured.
fn has_upstream(repo: &Repo, branch: &str) -> Result<bool> {
    let out = repo
        .command()
        .args([
            "for-each-ref",
            "--format=%(upstream)",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "for-each-ref")?;
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
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

/// How `git merge` joins the branch into HEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeType {
    /// `--ff`: fast-forward when possible, a merge commit otherwise.
    #[default]
    FastForwardIfPossible,
    /// `--no-ff`: always create a merge commit.
    NoFastForward,
    /// `--ff-only`: fail instead of creating a merge commit.
    FastForwardOnly,
    /// `--squash`: stage the combined changes without committing.
    Squash,
}

impl MergeType {
    fn flag(self) -> &'static str {
        match self {
            MergeType::FastForwardIfPossible => "--ff",
            MergeType::NoFastForward => "--no-ff",
            MergeType::FastForwardOnly => "--ff-only",
            MergeType::Squash => "--squash",
        }
    }
}

/// What a [`merge`] did. Conflicts are an outcome, not an error: the merge is
/// left in progress for the conflict editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// A merge commit was created.
    Merged,
    FastForwarded,
    /// The changes are staged and nothing is committed (`--squash`).
    Squashed,
    /// There was nothing to merge.
    UpToDate,
    /// The merge stopped; these repo-relative paths are conflicted.
    Conflicts(Vec<String>),
}

/// What merging a branch into HEAD would do, computed without touching the
/// work tree or the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePreview {
    /// HEAD already contains the branch.
    UpToDate,
    /// HEAD is an ancestor of the branch.
    FastForward,
    /// Diverged and mergeable without conflicts; `commits` is the number of
    /// commits only the branch has.
    Clean {
        commits: usize,
    },
    Conflicts {
        commits: usize,
        paths: Vec<String>,
    },
    /// The outcome cannot be predicted (git too old, …); the reason is
    /// human-readable.
    Unavailable(String),
}

/// The multi-step operation a repository is stopped in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    None,
    Merge,
    /// `progress` is `(current step, total steps)` when git recorded it
    /// (`rebase-merge/msgnum` and `end`); the apply backend has none here.
    Rebase {
        progress: Option<(usize, usize)>,
    },
    CherryPick,
    Revert,
}

/// `git merge-tree --write-tree` needs 2.38; the app's floor is lower, so the
/// conflict preview degrades to [`MergePreview::Unavailable`] instead.
pub(crate) const MERGE_TREE_MIN: GitVersion = GitVersion {
    major: 2,
    minor: 38,
    patch: 0,
};

/// Merges `branch` into HEAD with `git merge --no-edit`.
///
/// A failed merge that left unmerged paths behind is [`MergeOutcome::Conflicts`]
/// (with `--squash` git writes no `MERGE_HEAD`, so the paths alone decide);
/// every other failure is git's own error. The outcome is told apart by HEAD
/// and its parent count, not by git's message (the runner forces `LC_ALL=C`,
/// but the text is still not a contract).
pub(crate) fn merge(repo: &Repo, branch: &str, ty: MergeType) -> Result<MergeOutcome> {
    let before = head_oid(repo)?;
    let out = repo
        .command()
        .args(["merge", "--no-edit", ty.flag(), "--end-of-options", branch])
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        let merging = ty == MergeType::Squash || repo.git_dir().join("MERGE_HEAD").exists();
        if merging {
            let paths = unmerged_paths(repo)?;
            if !paths.is_empty() {
                return Ok(MergeOutcome::Conflicts(paths));
            }
        }
        return Err(failure("merge", out));
    }
    if ty == MergeType::Squash {
        // A squash never moves HEAD, so "up to date" cannot be read from it.
        return Ok(if is_ancestor(repo, branch, "HEAD")? {
            MergeOutcome::UpToDate
        } else {
            MergeOutcome::Squashed
        });
    }
    let after = head_oid(repo)?;
    if after == before {
        return Ok(MergeOutcome::UpToDate);
    }
    // `rev-list --parents -n 1` prints the commit followed by its parents.
    let out = repo
        .command()
        .args(["rev-list", "--parents", "-n", "1", "HEAD", "--"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-list")?;
    let fields = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .count();
    Ok(if fields > 2 {
        MergeOutcome::Merged
    } else {
        MergeOutcome::FastForwarded
    })
}

fn head_oid(repo: &Repo) -> Result<String> {
    let out = repo
        .command()
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-parse")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Repo-relative paths with unresolved conflicts, in git's order.
fn unmerged_paths(repo: &Repo) -> Result<Vec<String>> {
    let out = repo
        .command()
        .args(["diff", "--name-only", "--diff-filter=U", "-z"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "diff --diff-filter=U")?;
    Ok(split_nul(&out.stdout))
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// Predicts `git merge branch` without touching the work tree or the index.
///
/// Ancestry decides up-to-date and fast-forward; otherwise `git merge-tree
/// --write-tree` merges in memory (it only writes objects). The branch is
/// resolved to a commit id first, so a bad ref is an error rather than a
/// guess, and a name that looks like an option cannot be misread.
pub(crate) fn merge_preview(repo: &Repo, branch: &str) -> Result<MergePreview> {
    let out = repo
        .command()
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(format!("{branch}^{{commit}}"))
        .output()
        .map_err(Error::Spawn)?;
    let target = check_output(out, "rev-parse")?;
    let target = String::from_utf8_lossy(&target.stdout).trim().to_owned();

    if is_ancestor(repo, &target, "HEAD")? {
        return Ok(MergePreview::UpToDate);
    }
    if is_ancestor(repo, "HEAD", &target)? {
        return Ok(MergePreview::FastForward);
    }
    let out = repo
        .command()
        .args(["rev-list", "--count"])
        .arg(format!("HEAD..{target}"))
        .arg("--")
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-list")?;
    let commits = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|_| Error::Parse("rev-list --count"))?;

    let version = repo.git_version();
    if version < MERGE_TREE_MIN {
        return Ok(MergePreview::Unavailable(format!(
            "git {version} is too old for a conflict preview (needs {MERGE_TREE_MIN})"
        )));
    }
    let out = repo
        .command()
        .args(["merge-tree", "--write-tree", "--name-only", "-z", "HEAD"])
        .arg(&target)
        .output()
        .map_err(Error::Spawn)?;
    match out.status.code() {
        Some(0) => Ok(MergePreview::Clean { commits }),
        // Exit 1 is also what a bad ref gives (empty stdout), so conflicts
        // only count when stdout starts with the result tree's hex id.
        Some(1) if starts_with_oid(&out.stdout) => {
            // `<tree>\0<path>\0…\0\0<messages>`: paths end at the empty field.
            let mut paths: Vec<String> = Vec::new();
            for field in out.stdout.split(|b| *b == 0).skip(1) {
                if field.is_empty() {
                    break;
                }
                let path = String::from_utf8_lossy(field).into_owned();
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            Ok(MergePreview::Conflicts { commits, paths })
        }
        _ => Err(failure("merge-tree", out)),
    }
}

fn starts_with_oid(stdout: &[u8]) -> bool {
    let first = stdout.split(|b| *b == 0).next().unwrap_or_default();
    first.len() >= 40 && first.iter().all(u8::is_ascii_hexdigit)
}

/// Abandons a merge in progress (`git merge --abort`).
pub(crate) fn merge_abort(repo: &Repo) -> Result<()> {
    run(repo, &["merge", "--abort"], "merge --abort")
}

/// The operation the repository is stopped in, read from the per-worktree git
/// dir. A rebase wins over the others: it replays commits through
/// cherry-pick, so `CHERRY_PICK_HEAD` can exist inside it.
pub(crate) fn operation(repo: &Repo) -> Result<Operation> {
    let dir = repo.git_dir();
    let merge_dir = dir.join("rebase-merge");
    if merge_dir.is_dir() {
        let number = |name: &str| {
            std::fs::read_to_string(merge_dir.join(name))
                .ok()
                .and_then(|text| text.trim().parse::<usize>().ok())
        };
        let progress = number("msgnum").zip(number("end"));
        return Ok(Operation::Rebase { progress });
    }
    if dir.join("rebase-apply").is_dir() {
        return Ok(Operation::Rebase { progress: None });
    }
    Ok(if dir.join("MERGE_HEAD").exists() {
        Operation::Merge
    } else if dir.join("CHERRY_PICK_HEAD").exists() {
        Operation::CherryPick
    } else if dir.join("REVERT_HEAD").exists() {
        Operation::Revert
    } else {
        Operation::None
    })
}

/// The raw `MERGE_MSG` of the per-worktree git dir; `None` when absent.
fn read_merge_msg(repo: &Repo) -> Result<Option<String>> {
    match std::fs::read_to_string(repo.git_dir().join("MERGE_MSG")) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Io(err)),
    }
}

/// See [`Repo::merge_heading`].
pub(crate) fn merge_heading(repo: &Repo) -> Result<Option<String>> {
    Ok(read_merge_msg(repo)?
        .as_deref()
        .and_then(|text| text.lines().next())
        .and_then(parse_merge_heading))
}

fn parse_merge_heading(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("Merge branch '")
        .or_else(|| line.strip_prefix("Merge remote-tracking branch '"))?;
    let end = rest.find('\'')?;
    let name = &rest[..end];
    (!name.is_empty()).then(|| name.to_string())
}

/// See [`Repo::merge_message`].
pub(crate) fn merge_message(repo: &Repo) -> Result<Option<String>> {
    if !repo.git_dir().join("MERGE_HEAD").exists() {
        return Ok(None);
    }
    // `git commit -F` keeps `#` lines (it does not strip them like the editor
    // flow), and git appends a `# Conflicts:` block to MERGE_MSG.
    Ok(read_merge_msg(repo)?.map(|text| {
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        kept.join("\n").trim_end().to_string()
    }))
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

/// Whether `git stash pop --index` failed only because it could not rebuild
/// the index (a conflicting index, or an index with its own changes). Git
/// says so outright and keeps the stash entry; a plain pop can still apply it.
fn index_restore_failed(error: &Error) -> bool {
    let Error::Git { stderr, .. } = error else {
        return false;
    };
    let stderr = stderr.to_lowercase();
    stderr.contains("conflicts in index")
        || stderr.contains("try without --index")
        || stderr.contains("your index contains uncommitted changes")
}

/// Pops a stash and restores which changes were staged. When the index cannot
/// be restored, falls back to a plain pop: the changes come back unstaged but
/// are not lost.
pub(crate) fn stash_pop_keep_index(repo: &Repo, index: usize) -> Result<()> {
    let reference = format!("stash@{{{index}}}");
    let out = repo
        .command()
        .args(["stash", "pop", "--index", &reference])
        .output()
        .map_err(Error::Spawn)?;
    match check_output(out, "stash pop") {
        Ok(_) => Ok(()),
        Err(error) if index_restore_failed(&error) => {
            run(repo, &["stash", "pop", &reference], "stash pop")
        }
        Err(error) => Err(error),
    }
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

/// The subjects of the commits in `base..head`, oldest first, at most the 500
/// newest of them. `base` and `head` are any revisions git accepts.
pub(crate) fn commit_subjects(repo: &Repo, base: &str, head: &str) -> Result<Vec<String>> {
    let out = repo
        .command()
        .args(["log", "--reverse", "--max-count=500", "--format=%s"])
        .arg(format!("{base}..{head}"))
        .arg("--")
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "log")?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

/// How many commits `head` has that `base` lacks (ahead) and the other way
/// round (behind).
pub(crate) fn ahead_behind(repo: &Repo, base: &str, head: &str) -> Result<(usize, usize)> {
    let out = repo
        .command()
        .args(["rev-list", "--left-right", "--count"])
        .arg(format!("{base}...{head}"))
        .arg("--")
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-list")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut numbers = text.split_whitespace().map(|n| n.parse::<usize>().ok());
    match (numbers.next().flatten(), numbers.next().flatten()) {
        // `--left-right` prints the commits only in `base` first.
        (Some(behind), Some(ahead)) => Ok((ahead, behind)),
        _ => Err(Error::Parse("rev-list --count")),
    }
}

/// The branch `refs/remotes/<remote>/HEAD` points at (`main` for
/// `origin/main`), or `None` when the remote has no such pointer (a remote
/// that was added, not cloned, never gets one until `git remote set-head`).
pub(crate) fn remote_default_branch(repo: &Repo, remote: &str) -> Result<Option<String>> {
    let out = repo
        .command()
        .args(["symbolic-ref", "--quiet", "--short"])
        .arg(format!("refs/remotes/{remote}/HEAD"))
        .output()
        .map_err(Error::Spawn)?;
    // Exit code 1 is "not a symbolic ref" / missing, which is a plain `None`.
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .trim()
        .strip_prefix(remote)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned))
}
