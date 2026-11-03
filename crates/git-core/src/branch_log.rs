//! History of one branch for the worktree page and the History branch
//! filter: the first-parent line down to the fork point, and merge info.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

use crate::log::{parse_record, trim_record, FORMAT, RECORD};
use crate::{check_output, CommitInfo, Error, Oid, Repo, Result};

/// The commits `git log --first-parent <branch>` shows, cut at the fork point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchLog {
    /// Newest first; ends with the fork point when there is one.
    pub commits: Vec<CommitInfo>,
    /// The first commit of the branch's first-parent line that is also on
    /// main's first-parent line. `None` for main itself, when no main is
    /// known, or when the fork lies beyond `limit`.
    pub fork_point: Option<Oid>,
    /// The merge commit on main's first-parent line that brought the whole
    /// branch in (its second parent reaches the branch tip); the oldest one.
    pub merged_into: Option<Oid>,
    /// For merge commits in `commits`: the label of the merged branch.
    pub merged_from: HashMap<Oid, String>,
}

pub(crate) fn head_summaries(repo: &Repo, ids: &[Oid]) -> Result<HashMap<Oid, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // `--ignore-missing` drops ids git does not know instead of failing the batch.
    let out = repo
        .command()
        .args([
            "log",
            "--no-walk=unsorted",
            "--ignore-missing",
            "--no-decorate",
            "--format=%H%x00%s",
            "--end-of-options",
        ])
        .args(ids.iter().map(Oid::to_string))
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "log --no-walk")?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .filter_map(|line| {
            let (id, summary) = line.split_once('\0')?;
            Some((Oid::from_hex(id)?, summary.to_owned()))
        })
        .collect())
}

pub(crate) fn branch_log(
    repo: &Repo,
    branch: &str,
    main: Option<&str>,
    limit: usize,
) -> Result<BranchLog> {
    let tip = resolve(repo, branch)?.ok_or(Error::Parse("branch not found"))?;
    let main_ref = match main {
        Some(main) if main != branch => main_ref_for(repo, branch, main)?,
        _ => None,
    };

    let mut commits = first_parent_log(repo, branch, limit)?;
    let mut fork_point = None;
    if let Some(main_ref) = &main_ref {
        let candidates: Vec<Oid> = commits.iter().map(|c| c.id).collect();
        if let Some(fork) = find_fork(repo, main_ref, &candidates)? {
            if let Some(at) = commits.iter().position(|c| c.id == fork) {
                commits.truncate(at + 1);
                fork_point = Some(fork);
            }
        }
    }
    let merged_into = match &main_ref {
        Some(main_ref) => find_merged_into(repo, tip, main_ref)?,
        None => None,
    };
    let merged_from = merged_from(repo, &commits)?;
    Ok(BranchLog {
        commits,
        fork_point,
        merged_into,
        merged_from,
    })
}

fn resolve(repo: &Repo, rev: &str) -> Result<Option<Oid>> {
    let out = repo
        .command()
        .args(["rev-parse", "-q", "--verify", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .map_err(Error::Spawn)?;
    Ok(Oid::from_hex(String::from_utf8_lossy(&out.stdout).trim()))
}

/// For a remote branch `origin/x`, main is `origin/<main>` when that exists;
/// otherwise the local `main`.
fn main_ref_for(repo: &Repo, branch: &str, main: &str) -> Result<Option<String>> {
    let is_local = resolve(repo, &format!("refs/heads/{branch}"))?.is_some();
    if !is_local {
        for remote in repo.remotes()? {
            if branch.starts_with(&format!("{remote}/")) {
                let remote_main = format!("refs/remotes/{remote}/{main}");
                if resolve(repo, &remote_main)?.is_some() {
                    return Ok(Some(remote_main));
                }
            }
        }
    }
    let local = format!("refs/heads/{main}");
    Ok(resolve(repo, &local)?.map(|_| local))
}

fn first_parent_log(repo: &Repo, branch: &str, limit: usize) -> Result<Vec<CommitInfo>> {
    let out = repo
        .command()
        .args(["log", "--first-parent", "--no-decorate", FORMAT])
        .arg(format!("--max-count={limit}"))
        .arg("--end-of-options")
        .arg(branch)
        .arg("--")
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "log --first-parent")?;
    out.stdout
        .split(|&b| b == RECORD)
        .map(trim_record)
        .filter(|record| !record.is_empty())
        .map(parse_record)
        .collect()
}

/// Walks main's first-parent line from its tip and returns the first commit
/// that is in `candidates`. Candidates are one first-parent chain, so that is
/// also the newest candidate on the branch. Stops reading at the first hit.
fn find_fork(repo: &Repo, main_ref: &str, candidates: &[Oid]) -> Result<Option<Oid>> {
    let mut child = repo
        .command()
        .args(["rev-list", "--first-parent", "--end-of-options"])
        .arg(main_ref)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Error::Spawn)?;
    let stdout = child.stdout.take().ok_or(Error::Parse("rev-list stdout"))?;
    let mut found = None;
    for line in BufReader::new(stdout).lines() {
        let Some(id) = Oid::from_hex(line?.trim()) else {
            continue;
        };
        if candidates.contains(&id) {
            found = Some(id);
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(found)
}

/// The oldest merge on main's first-parent line after `tip` whose second
/// parent reaches `tip`.
fn find_merged_into(repo: &Repo, tip: Oid, main_ref: &str) -> Result<Option<Oid>> {
    let out = repo
        .command()
        .args([
            "rev-list",
            "--first-parent",
            "--merges",
            "--ancestry-path",
            "--reverse",
            "--parents",
            "--end-of-options",
        ])
        .arg(format!("{tip}..{main_ref}"))
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-list --merges")?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut ids = line.split(' ').filter_map(Oid::from_hex);
        let (Some(merge), Some(_first), Some(second)) = (ids.next(), ids.next(), ids.next()) else {
            continue;
        };
        let reaches = repo
            .command()
            .args(["merge-base", "--is-ancestor"])
            .arg(tip.to_string())
            .arg(second.to_string())
            .output()
            .map_err(Error::Spawn)?;
        if reaches.status.success() {
            return Ok(Some(merge));
        }
    }
    Ok(None)
}

fn merged_from(repo: &Repo, commits: &[CommitInfo]) -> Result<HashMap<Oid, String>> {
    let mut out = HashMap::new();
    let mut local_tips: Option<Vec<(String, Oid)>> = None;
    for commit in commits.iter().filter(|c| c.parents.len() >= 2) {
        if let Some(name) = branch_in_subject(&commit.summary) {
            out.insert(commit.id, name.to_owned());
            continue;
        }
        let second = commit.parents[1];
        let tips = match &mut local_tips {
            Some(tips) => tips,
            slot => slot.insert(local_branch_tips(repo)?),
        };
        let label = tips
            .iter()
            .find(|(_, tip)| *tip == second)
            .map_or_else(|| second.short(), |(name, _)| name.clone());
        out.insert(commit.id, label);
    }
    Ok(out)
}

/// The name in git's default merge subject: `Merge branch 'x'` or
/// `Merge remote-tracking branch 'origin/x'`.
fn branch_in_subject(subject: &str) -> Option<&str> {
    let rest = subject.strip_prefix("Merge ")?;
    let rest = rest.strip_prefix("remote-tracking ").unwrap_or(rest);
    let rest = rest.strip_prefix("branch '")?;
    let name = &rest[..rest.find('\'')?];
    (!name.is_empty()).then_some(name)
}

/// Local branches with their tips, sorted by name so the label is stable.
fn local_branch_tips(repo: &Repo) -> Result<Vec<(String, Oid)>> {
    let mut tips: Vec<_> = repo
        .refs()?
        .into_iter()
        .filter(|r| r.kind == crate::RefKind::LocalBranch)
        .map(|r| (r.name, r.target))
        .collect();
    tips.sort();
    Ok(tips)
}

#[cfg(test)]
mod tests {
    use super::branch_in_subject;

    #[test]
    fn names_the_branch_from_a_default_merge_subject() {
        assert_eq!(branch_in_subject("Merge branch 'feat/x'"), Some("feat/x"));
        assert_eq!(
            branch_in_subject("Merge remote-tracking branch 'origin/x' into main"),
            Some("origin/x")
        );
        assert_eq!(branch_in_subject("Merge pull request #3 from a/b"), None);
        assert_eq!(branch_in_subject("Merge branch ''"), None);
    }
}
