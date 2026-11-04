//! End-to-end tests for merge, the merge preview and the operation state: every
//! one runs the real `git` in a throw-away repository.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{
    Git, GitChoice, GitVersion, MergeOutcome, MergePreview, MergeType, Operation, Repo,
};

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git is installed");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) {
    git_out(dir, args);
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn commit_file(dir: &Path, file: &str, contents: &str, message: &str) {
    fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", "--", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn merge_tree_supported() -> bool {
    Git::resolve(GitChoice::System).unwrap().git.version()
        >= GitVersion {
            major: 2,
            minor: 38,
            patch: 0,
        }
}

/// `main` has A; `side` branches from it. Callers add commits on either.
fn base() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    commit_file(dir, "a.txt", "one\n", "A");
    git(dir, &["branch", "side"]);
    let repo = open(dir);
    (tmp, repo)
}

/// Diverged: `main` has main.txt, `side` has side.txt (two commits).
fn diverged() -> (tempfile::TempDir, Repo) {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "main.txt", "m\n", "M");
    git(dir, &["switch", "-q", "side"]);
    commit_file(dir, "side.txt", "s1\n", "S1");
    commit_file(dir, "side.txt", "s2\n", "S2");
    git(dir, &["switch", "-q", "main"]);
    (tmp, repo)
}

/// Both branches rewrite a.txt differently.
fn conflicting() -> (tempfile::TempDir, Repo) {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "a.txt", "main\n", "M");
    git(dir, &["switch", "-q", "side"]);
    commit_file(dir, "a.txt", "side\n", "S");
    git(dir, &["switch", "-q", "main"]);
    (tmp, repo)
}

#[test]
fn preview_up_to_date() {
    let (_tmp, repo) = base();
    assert_eq!(repo.merge_preview("side").unwrap(), MergePreview::UpToDate);
}

#[test]
fn preview_fast_forward() {
    let (tmp, repo) = base();
    git(tmp.path(), &["switch", "-q", "side"]);
    commit_file(tmp.path(), "s.txt", "s\n", "S");
    git(tmp.path(), &["switch", "-q", "main"]);
    assert_eq!(
        repo.merge_preview("side").unwrap(),
        MergePreview::FastForward
    );
}

#[test]
fn preview_clean_counts_branch_only_commits() {
    let (_tmp, repo) = diverged();
    let preview = repo.merge_preview("side").unwrap();
    if merge_tree_supported() {
        assert_eq!(preview, MergePreview::Clean { commits: 2 });
    } else {
        assert!(matches!(preview, MergePreview::Unavailable(_)));
    }
}

#[test]
fn preview_content_conflict_lists_path() {
    let (_tmp, repo) = conflicting();
    let preview = repo.merge_preview("side").unwrap();
    if merge_tree_supported() {
        assert_eq!(
            preview,
            MergePreview::Conflicts {
                commits: 1,
                paths: vec!["a.txt".to_owned()]
            }
        );
    } else {
        assert!(matches!(preview, MergePreview::Unavailable(_)));
    }
}

#[test]
fn preview_modify_delete_conflict_lists_path() {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "a.txt", "changed\n", "M");
    git(dir, &["switch", "-q", "side"]);
    git(dir, &["rm", "-q", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "S"]);
    git(dir, &["switch", "-q", "main"]);
    if !merge_tree_supported() {
        return;
    }
    assert_eq!(
        repo.merge_preview("side").unwrap(),
        MergePreview::Conflicts {
            commits: 1,
            paths: vec!["a.txt".to_owned()]
        }
    );
}

#[test]
fn preview_bad_ref_is_an_error() {
    let (_tmp, repo) = diverged();
    assert!(repo.merge_preview("nope").is_err());
}

#[test]
fn preview_leaves_a_dirty_work_tree_untouched() {
    let (tmp, repo) = conflicting();
    fs::write(tmp.path().join("a.txt"), "dirty\n").unwrap();
    let status_before = git_out(tmp.path(), &["status", "--porcelain"]);
    repo.merge_preview("side").unwrap();
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        git_out(tmp.path(), &["status", "--porcelain"]),
        status_before
    );
    assert_eq!(repo.operation().unwrap(), Operation::None);
}

#[test]
fn merge_fast_forwards() {
    let (tmp, repo) = base();
    git(tmp.path(), &["switch", "-q", "side"]);
    commit_file(tmp.path(), "s.txt", "s\n", "S");
    let side = git_out(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["switch", "-q", "main"]);
    let outcome = repo
        .merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert_eq!(outcome, MergeOutcome::FastForwarded);
    assert_eq!(git_out(tmp.path(), &["rev-parse", "HEAD"]), side);
}

#[test]
fn merge_creates_a_merge_commit_when_diverged() {
    let (tmp, repo) = diverged();
    let outcome = repo
        .merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert_eq!(outcome, MergeOutcome::Merged);
    let parents = git_out(tmp.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3);
}

#[test]
fn merge_no_ff_creates_a_commit_even_when_fast_forwardable() {
    let (tmp, repo) = base();
    git(tmp.path(), &["switch", "-q", "side"]);
    commit_file(tmp.path(), "s.txt", "s\n", "S");
    git(tmp.path(), &["switch", "-q", "main"]);
    assert_eq!(
        repo.merge("side", MergeType::NoFastForward).unwrap(),
        MergeOutcome::Merged
    );
    let parents = git_out(tmp.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3);
}

#[test]
fn merge_ff_only_fails_when_diverged() {
    let (tmp, repo) = diverged();
    let head = git_out(tmp.path(), &["rev-parse", "HEAD"]);
    assert!(repo.merge("side", MergeType::FastForwardOnly).is_err());
    assert_eq!(git_out(tmp.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(repo.operation().unwrap(), Operation::None);
}

#[test]
fn merge_squash_stages_without_committing() {
    let (tmp, repo) = diverged();
    let head = git_out(tmp.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        repo.merge("side", MergeType::Squash).unwrap(),
        MergeOutcome::Squashed
    );
    assert_eq!(git_out(tmp.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_out(tmp.path(), &["diff", "--cached", "--name-only"]),
        "side.txt"
    );
}

#[test]
fn merge_squash_with_conflicts_reports_paths() {
    let (_tmp, repo) = conflicting();
    assert_eq!(
        repo.merge("side", MergeType::Squash).unwrap(),
        MergeOutcome::Conflicts(vec!["a.txt".to_owned()])
    );
}

#[test]
fn merge_conflicts_are_an_outcome_and_abort_restores_head() {
    let (tmp, repo) = conflicting();
    let head = git_out(tmp.path(), &["rev-parse", "HEAD"]);
    let outcome = repo
        .merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert_eq!(outcome, MergeOutcome::Conflicts(vec!["a.txt".to_owned()]));
    assert_eq!(repo.operation().unwrap(), Operation::Merge);

    repo.merge_abort().unwrap();
    assert_eq!(repo.operation().unwrap(), Operation::None);
    assert_eq!(git_out(tmp.path(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn merge_up_to_date() {
    let (_tmp, repo) = base();
    assert_eq!(
        repo.merge("side", MergeType::FastForwardIfPossible)
            .unwrap(),
        MergeOutcome::UpToDate
    );
    assert_eq!(
        repo.merge("side", MergeType::Squash).unwrap(),
        MergeOutcome::UpToDate
    );
}

#[test]
fn merge_dirty_overwrite_is_an_error() {
    let (tmp, repo) = base();
    git(tmp.path(), &["switch", "-q", "side"]);
    commit_file(tmp.path(), "a.txt", "side\n", "S");
    git(tmp.path(), &["switch", "-q", "main"]);
    fs::write(tmp.path().join("a.txt"), "dirty\n").unwrap();
    assert!(repo
        .merge("side", MergeType::FastForwardIfPossible)
        .is_err());
}

#[test]
fn operation_is_none_on_a_clean_repo() {
    let (_tmp, repo) = base();
    assert_eq!(repo.operation().unwrap(), Operation::None);
}

#[test]
fn operation_reports_a_stopped_rebase() {
    let (tmp, repo) = conflicting();
    let dir = tmp.path();
    git(dir, &["switch", "-q", "side"]);
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(["rebase", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
        .status;
    assert!(!status.success());
    assert!(matches!(
        repo.operation().unwrap(),
        Operation::Rebase { .. }
    ));
    git(dir, &["rebase", "--abort"]);
    assert_eq!(repo.operation().unwrap(), Operation::None);
}
