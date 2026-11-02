//! The git helpers behind "Create pull request" and the pull request panel's
//! Checkout: commit subjects of a range, ahead / behind, the remote's default
//! branch and fetching a pull request's head ref into a local branch. Every
//! test runs the real `git` against throw-away repositories.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::network::{self, CancelToken, NetworkContext};
use git_core::{Git, GitChoice, Head, RefKind, Repo};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git is installed");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn commit_file(dir: &Path, file: &str, message: &str) {
    fs::write(dir.join(file), message).unwrap();
    git(dir, &["add", "--", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A repository on `main` with one commit.
fn repo_on_main() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    commit_file(tmp.path(), "a.txt", "Initial commit");
    let repo = open(tmp.path());
    (tmp, repo)
}

#[test]
fn commit_subjects_lists_the_range_oldest_first() {
    let (tmp, repo) = repo_on_main();
    git(tmp.path(), &["switch", "-q", "-c", "feat/x"]);
    commit_file(tmp.path(), "b.txt", "feat: first");
    commit_file(tmp.path(), "c.txt", "fix: second");

    assert_eq!(
        repo.commit_subjects("main", "feat/x").unwrap(),
        ["feat: first", "fix: second"]
    );
    // The other direction has nothing.
    assert!(repo.commit_subjects("feat/x", "main").unwrap().is_empty());
}

#[test]
fn commit_subjects_of_an_unknown_revision_is_an_error() {
    let (_tmp, repo) = repo_on_main();
    assert!(repo.commit_subjects("main", "no-such-branch").is_err());
}

#[test]
fn ahead_behind_counts_both_sides() {
    let (tmp, repo) = repo_on_main();
    git(tmp.path(), &["switch", "-q", "-c", "topic"]);
    commit_file(tmp.path(), "b.txt", "topic 1");
    commit_file(tmp.path(), "c.txt", "topic 2");
    git(tmp.path(), &["switch", "-q", "main"]);
    commit_file(tmp.path(), "d.txt", "main 1");

    assert_eq!(repo.ahead_behind("main", "topic").unwrap(), (2, 1));
    assert_eq!(repo.ahead_behind("topic", "main").unwrap(), (1, 2));
    assert_eq!(repo.ahead_behind("main", "main").unwrap(), (0, 0));
}

#[test]
fn the_default_branch_comes_from_the_remote_head_pointer() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    let (tmp, repo) = repo_on_main();
    git(
        tmp.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(tmp.path(), &["push", "-q", "origin", "main"]);
    git(tmp.path(), &["fetch", "-q", "origin"]);

    // Recent git sets `origin/HEAD` on the first fetch; an older one, or a
    // deleted pointer, leaves the remote without a default branch.
    git(tmp.path(), &["remote", "set-head", "origin", "--delete"]);
    assert_eq!(repo.remote_default_branch("origin").unwrap(), None);
    git(tmp.path(), &["remote", "set-head", "origin", "main"]);
    assert_eq!(
        repo.remote_default_branch("origin").unwrap().as_deref(),
        Some("main")
    );
    assert_eq!(repo.remote_default_branch("nope").unwrap(), None);
}

#[test]
fn a_pull_request_head_is_fetched_into_a_new_local_branch() {
    // A bare remote that carries a pull request head ref, as GitHub keeps them.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    let (tmp, repo) = repo_on_main();
    git(
        tmp.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(tmp.path(), &["push", "-q", "origin", "main"]);
    git(tmp.path(), &["switch", "-q", "-c", "contrib"]);
    commit_file(tmp.path(), "b.txt", "from the pull request");
    git(
        tmp.path(),
        &["push", "-q", "origin", "contrib:refs/pull/7/head"],
    );
    git(tmp.path(), &["switch", "-q", "main"]);
    git(tmp.path(), &["branch", "-q", "-D", "contrib"]);

    network::fetch_refspec(
        &repo,
        "origin",
        "refs/pull/7/head:refs/heads/pr/7",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    let refs = repo.refs().unwrap();
    assert!(refs
        .iter()
        .any(|r| r.kind == RefKind::LocalBranch && r.name == "pr/7"));
    assert_eq!(
        repo.commit_subjects("main", "pr/7").unwrap(),
        ["from the pull request"]
    );
    // Still on main: fetching only creates the branch.
    assert!(matches!(repo.head().unwrap(), Head::Branch { name, .. } if name == "main"));
}

#[test]
fn fetching_into_a_diverged_branch_does_not_overwrite_it() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    let (tmp, repo) = repo_on_main();
    git(
        tmp.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(tmp.path(), &["switch", "-q", "-c", "contrib"]);
    commit_file(tmp.path(), "b.txt", "remote side");
    git(
        tmp.path(),
        &["push", "-q", "origin", "contrib:refs/pull/9/head"],
    );
    git(tmp.path(), &["switch", "-q", "main"]);
    // A local branch of the same name with a commit the remote lacks.
    git(tmp.path(), &["branch", "-q", "-f", "contrib", "main"]);
    git(tmp.path(), &["switch", "-q", "contrib"]);
    commit_file(tmp.path(), "c.txt", "local only");
    git(tmp.path(), &["switch", "-q", "main"]);

    let result = network::fetch_refspec(
        &repo,
        "origin",
        "refs/pull/9/head:refs/heads/contrib",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    );
    assert!(result.is_err());
    assert_eq!(
        repo.commit_subjects("main", "contrib").unwrap(),
        ["local only"]
    );
}
