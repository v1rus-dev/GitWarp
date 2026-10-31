use std::path::{Path, PathBuf};

use crate::{check_output, Error, Oid, Repo, Result};

/// One work tree attached to the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Short branch name (`refs/heads/` stripped); `None` when detached or bare.
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub bare: bool,
    pub detached: bool,
    /// `Some(reason)` when locked; the reason may be empty.
    pub locked: Option<String>,
    /// git marks it prunable (its directory is gone).
    pub prunable: bool,
    /// This is the worktree the `Repo` was opened in (compare canonicalized paths with `Repo::root`).
    pub current: bool,
    /// The repository's main worktree: the one a linked worktree points back
    /// to. It cannot be removed; `git worktree list` prints it first.
    pub main: bool,
}

pub(crate) fn list(repo: &Repo) -> Result<Vec<Worktree>> {
    let out = repo
        .command()
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "worktree list")?;
    Ok(parse_worktrees(&out.stdout, repo.root()))
}

/// Checks out `branch` in a new worktree at `path`.
pub(crate) fn add(repo: &Repo, path: &Path, branch: &str) -> Result<()> {
    let out = repo
        .command()
        .args(["worktree", "add", "--"])
        .arg(path)
        .arg(branch)
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "worktree add").map(drop)
}

/// Creates `branch` at `start` and checks it out in a new worktree at `path`.
pub(crate) fn add_new_branch(repo: &Repo, path: &Path, branch: &str, start: Oid) -> Result<()> {
    let start = start.to_string();
    let out = repo
        .command()
        .args(["worktree", "add", "-b", branch, "--"])
        .arg(path)
        .arg(start)
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "worktree add -b").map(drop)
}

/// Removes the worktree at `path`. `force` discards staged, unstaged and
/// untracked files in it; without it git refuses to touch a dirty worktree.
/// A worktree whose folder is already gone is removed without `force`.
pub(crate) fn remove(repo: &Repo, path: &Path, force: bool) -> Result<()> {
    let mut command = repo.command();
    command.args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    let out = command.arg("--").arg(path).output().map_err(Error::Spawn)?;
    check_output(out, "worktree remove").map(drop)
}

/// Locks the worktree at `path`. `reason` is stored by git and shown to anyone
/// running `git worktree list`; an empty reason is allowed.
pub(crate) fn lock(repo: &Repo, path: &Path, reason: Option<&str>) -> Result<()> {
    let mut command = repo.command();
    command.args(["worktree", "lock"]);
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        command.args(["--reason", reason]);
    }
    let out = command.arg("--").arg(path).output().map_err(Error::Spawn)?;
    check_output(out, "worktree lock").map(drop)
}

/// Unlocks the worktree at `path`, so it can be removed again.
pub(crate) fn unlock(repo: &Repo, path: &Path) -> Result<()> {
    let out = repo
        .command()
        .args(["worktree", "unlock", "--"])
        .arg(path)
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "worktree unlock").map(drop)
}

/// Drops the records of worktrees whose folders no longer exist. Files are
/// never touched.
pub(crate) fn prune(repo: &Repo) -> Result<()> {
    let out = repo
        .command()
        .args(["worktree", "prune"])
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "worktree prune").map(drop)
}

/// Porcelain `-z` output: NUL-separated attributes, an empty field ending a
/// record. Unknown attributes are ignored.
fn parse_worktrees(bytes: &[u8], root: &Path) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut record = Record::default();
    for field in bytes.split(|&b| b == 0) {
        if field.is_empty() {
            if let Some(worktree) = record.finish(root, out.is_empty()) {
                out.push(worktree);
            }
            record = Record::default();
        } else {
            record.attribute(field);
        }
    }
    // Be lenient with output that does not end in an empty field.
    if let Some(worktree) = record.finish(root, out.is_empty()) {
        out.push(worktree);
    }
    out
}

/// An all-zero `HEAD` (unborn branch) means there is no commit yet.
fn parse_head(hex: &str) -> Option<Oid> {
    let oid = Oid::from_hex(hex)?;
    if oid.as_bytes().iter().all(|&b| b == 0) {
        None
    } else {
        Some(oid)
    }
}

#[derive(Default)]
struct Record {
    path: Option<PathBuf>,
    head: Option<Oid>,
    branch: Option<String>,
    bare: bool,
    detached: bool,
    locked: Option<String>,
    prunable: bool,
}

impl Record {
    fn attribute(&mut self, field: &[u8]) {
        let (key, value) = match field.iter().position(|&b| b == b' ') {
            Some(i) => (&field[..i], Some(&field[i + 1..])),
            None => (field, None),
        };
        let text = || String::from_utf8_lossy(value.unwrap_or_default()).into_owned();
        match key {
            b"worktree" => self.path = Some(PathBuf::from(text())),
            b"HEAD" => self.head = parse_head(&text()),
            b"branch" => {
                // Only local branch refs appear here; anything else is kept
                // as git printed it rather than silently dropped.
                let name = text();
                self.branch = Some(name.strip_prefix("refs/heads/").unwrap_or(&name).to_owned());
            }
            b"bare" => self.bare = true,
            b"detached" => self.detached = true,
            // A lock or prunable entry without a reason has no value at all.
            b"locked" => self.locked = Some(text()),
            b"prunable" => self.prunable = true,
            _ => {}
        }
    }

    fn finish(self, root: &Path, main: bool) -> Option<Worktree> {
        let path = self.path?;
        let current = crate::same_path(&path, root);
        Some(Worktree {
            path,
            branch: self.branch,
            head: self.head,
            bare: self.bare,
            detached: self.detached,
            locked: self.locked,
            prunable: self.prunable,
            current,
            main,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn parses_main_and_linked_worktrees() {
        let raw = format!(
            "worktree /repo\0HEAD {OID}\0branch refs/heads/main\0\0\
             worktree /repo/wt\0HEAD {OID}\0detached\0locked testing\0\0"
        );
        let list = parse_worktrees(raw.as_bytes(), Path::new("/repo"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, Path::new("/repo"));
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert!(list[0].current);
        assert!(list[0].main);
        assert!(!list[0].detached && !list[0].bare);
        assert_eq!(list[1].branch, None);
        assert!(!list[1].main);
        assert!(list[1].detached);
        assert_eq!(list[1].locked.as_deref(), Some("testing"));
        assert!(!list[1].current);
    }

    #[test]
    fn parses_a_lock_without_a_reason_and_unknown_attributes() {
        let raw = format!("worktree /repo\0HEAD {OID}\0locked\0future-attribute value\0\0");
        let list = parse_worktrees(raw.as_bytes(), Path::new("/elsewhere"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].locked.as_deref(), Some(""));
        assert!(!list[0].current);
        assert!(list[0].main);
        assert_eq!(list[0].head.unwrap().to_string(), OID);
    }

    #[test]
    fn parses_bare_and_prunable() {
        let raw = "worktree /bare.git\0bare\0\0\
                   worktree /gone\0HEAD 0123456789abcdef0123456789abcdef01234567\0\
                   branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0";
        let list = parse_worktrees(raw.as_bytes(), Path::new("/bare.git"));
        assert_eq!(list.len(), 2);
        assert!(list[0].bare && list[0].current);
        assert_eq!(list[0].branch, None);
        assert!(list[1].prunable);
        assert_eq!(list[1].branch.as_deref(), Some("gone"));
    }

    #[test]
    fn empty_output_has_no_worktrees() {
        assert!(parse_worktrees(b"", Path::new("/repo")).is_empty());
        assert!(parse_worktrees(b"\0\0", Path::new("/repo")).is_empty());
    }

    #[test]
    fn an_unborn_head_has_no_commit() {
        let raw = "worktree /repo\0HEAD 0000000000000000000000000000000000000000\0\
                   branch refs/heads/main\0\0";
        let list = parse_worktrees(raw.as_bytes(), Path::new("/repo"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].head, None);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_missing_trailing_separator_still_finishes_the_record() {
        let raw = format!("worktree /repo\0HEAD {OID}\0branch refs/heads/main");
        let list = parse_worktrees(raw.as_bytes(), Path::new("/repo"));
        assert_eq!(list.len(), 1);
        assert!(list[0].current);
    }
}
