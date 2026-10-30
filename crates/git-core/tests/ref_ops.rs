//! End-to-end tests for the stage 7 ref operations: every one runs the real
//! `git` in a throw-away repository.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{Git, GitChoice, Head, RefKind, Repo, ResetMode};

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
            "tag.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git is installed");
    assert!(status.success(), "git {args:?} failed");
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn commit_file(dir: &Path, file: &str, contents: &str, message: &str) {
    fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", "--", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A repository with one commit and the local identity the operations need.
fn repo_with_history() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "tag.gpgsign", "false"]);
    commit_file(dir, "a.txt", "one\n", "A");
    let repo = open(dir);
    (tmp, repo)
}

fn branch_name(repo: &Repo) -> Option<String> {
    match repo.head().unwrap() {
        Head::Branch { name, .. } => Some(name),
        Head::Detached(_) => None,
    }
}

fn head_target(repo: &Repo) -> git_core::Oid {
    match repo.head().unwrap() {
        Head::Branch { target, .. } => target.expect("the test branch has a commit"),
        Head::Detached(oid) => oid,
    }
}

fn summary(repo: &Repo) -> String {
    repo.head_commit().unwrap().unwrap().summary
}

#[test]
fn branches_are_created_renamed_checked_out_and_deleted() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    let start = repo.head().unwrap();
    let Head::Branch { target, .. } = start else {
        panic!("expected a branch");
    };
    let first = target.unwrap();
    commit_file(dir, "b.txt", "two\n", "B");

    repo.create_branch("feat/x", first, false).unwrap();
    let names: Vec<String> = repo
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .map(|r| r.name)
        .collect();
    assert!(names.contains(&"feat/x".to_owned()), "{names:?}");

    repo.checkout_branch("feat/x").unwrap();
    assert_eq!(branch_name(&repo).as_deref(), Some("feat/x"));
    assert_eq!(summary(&repo), "A");

    repo.rename_branch("feat/x", "fix/y").unwrap();
    assert_eq!(branch_name(&repo).as_deref(), Some("fix/y"));

    repo.checkout_branch("main").unwrap();
    repo.delete_branch("fix/y", false).unwrap();
    assert!(!repo.refs().unwrap().iter().any(|r| r.name == "fix/y"));
}

#[test]
fn create_and_switch_lands_on_the_new_branch() {
    let (_tmp, repo) = repo_with_history();
    let Head::Branch { target, .. } = repo.head().unwrap() else {
        panic!("expected a branch");
    };
    repo.create_branch("feature", target.unwrap(), true)
        .unwrap();
    assert_eq!(branch_name(&repo).as_deref(), Some("feature"));
}

#[test]
fn an_unmerged_branch_needs_force() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    repo.create_branch("side", head_target(&repo), false)
        .unwrap();
    repo.checkout_branch("side").unwrap();
    commit_file(dir, "side.txt", "side\n", "Side");
    repo.checkout_branch("main").unwrap();

    let error = repo.delete_branch("side", false).unwrap_err().to_string();
    assert!(error.contains("not fully merged"), "{error}");
    repo.delete_branch("side", true).unwrap();
    assert!(!repo.refs().unwrap().iter().any(|r| r.name == "side"));
}

#[test]
fn checkout_remote_creates_a_tracking_branch() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    let target = head_target(&repo);
    // A configured remote and its tracking ref, as a fetch would have written
    // them.
    git(
        dir,
        &["remote", "add", "origin", "https://example.com/repo.git"],
    );
    git(
        dir,
        &[
            "update-ref",
            "refs/remotes/origin/feat",
            &target.to_string(),
        ],
    );
    repo.checkout_remote("origin/feat").unwrap();
    assert_eq!(branch_name(&repo).as_deref(), Some("feat"));
    let branch = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == RefKind::LocalBranch && r.name == "feat")
        .expect("the tracking branch exists");
    assert_eq!(branch.upstream.as_deref(), Some("origin/feat"));
}

#[test]
fn checkout_detached_leaves_head_on_the_commit() {
    let (_tmp, repo) = repo_with_history();
    let target = head_target(&repo);
    repo.checkout_detached(target).unwrap();
    match repo.head().unwrap() {
        Head::Detached(oid) => assert_eq!(oid, target),
        Head::Branch { name, .. } => panic!("expected a detached HEAD, got {name}"),
    }
}

#[test]
fn tags_are_created_listed_renamed_and_deleted() {
    let (_tmp, repo) = repo_with_history();
    let target = head_target(&repo);

    repo.create_tag("light", target, None, false).unwrap();
    repo.create_tag("v1.0", target, Some("Release 1.0"), false)
        .unwrap();
    let tags: Vec<String> = repo
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == RefKind::Tag)
        .map(|r| r.name)
        .collect();
    assert!(tags.contains(&"light".to_owned()), "{tags:?}");
    assert!(tags.contains(&"v1.0".to_owned()), "{tags:?}");

    repo.rename_tag("v1.0", "v1.1").unwrap();
    let tags: Vec<String> = repo
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == RefKind::Tag)
        .map(|r| r.name)
        .collect();
    assert!(tags.contains(&"v1.1".to_owned()), "{tags:?}");
    assert!(!tags.contains(&"v1.0".to_owned()), "{tags:?}");

    repo.delete_tag("light").unwrap();
    assert!(!repo.refs().unwrap().iter().any(|r| r.name == "light"));
}

#[test]
fn remotes_are_added_edited_renamed_and_removed() {
    let (_tmp, repo) = repo_with_history();
    repo.add_remote("origin", "https://example.com/acme/repo.git")
        .unwrap();
    assert_eq!(repo.remotes().unwrap(), ["origin"]);

    repo.set_remote_url("origin", "git@example.com:acme/repo.git")
        .unwrap();
    repo.rename_remote("origin", "upstream").unwrap();
    assert_eq!(repo.remotes().unwrap(), ["upstream"]);

    repo.remove_remote("upstream").unwrap();
    assert!(repo.remotes().unwrap().is_empty());
}

#[test]
fn cherry_pick_applies_a_commit_on_top_of_head() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    repo.create_branch("side", head_target(&repo), false)
        .unwrap();
    repo.checkout_branch("side").unwrap();
    commit_file(dir, "side.txt", "side\n", "Side");
    let side = repo.head_commit().unwrap().unwrap().id;
    repo.checkout_branch("main").unwrap();

    repo.cherry_pick(side).unwrap();
    assert_eq!(summary(&repo), "Side");
    assert_eq!(fs::read_to_string(dir.join("side.txt")).unwrap(), "side\n");
}

#[test]
fn revert_adds_an_undo_commit() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    commit_file(dir, "b.txt", "two\n", "B");
    let head = repo.head_commit().unwrap().unwrap().id;

    repo.revert(head).unwrap();
    let summary = summary(&repo);
    assert!(summary.starts_with("Revert"), "{summary}");
    assert!(!dir.join("b.txt").exists());
}

#[test]
fn reset_modes_touch_index_and_work_tree_differently() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    commit_file(dir, "a.txt", "two\n", "B");
    let first = repo
        .log(&git_core::LogOptions::default())
        .unwrap()
        .map(|c| c.unwrap())
        .last()
        .unwrap()
        .id;

    repo.reset(first, ResetMode::Soft).unwrap();
    assert_eq!(repo.head_commit().unwrap().unwrap().id, first);
    let status = repo.working_status().unwrap();
    assert_eq!(status.staged_count(), 1);
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");

    repo.reset(first, ResetMode::Hard).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    assert_eq!(repo.working_status().unwrap().staged_count(), 0);
}

#[test]
fn stash_save_apply_pop_and_drop() {
    let (tmp, repo) = repo_with_history();
    let dir = tmp.path();
    fs::write(dir.join("a.txt"), "changed\n").unwrap();
    fs::write(dir.join("new.txt"), "new\n").unwrap();

    assert!(repo.stash_save(Some("WIP")).unwrap());
    assert_eq!(repo.stashes().unwrap().len(), 1);
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    assert!(!dir.join("new.txt").exists());

    repo.stash_apply(0).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed\n");
    assert_eq!(repo.stashes().unwrap().len(), 1);

    // Put the work tree back so the same stash can be popped cleanly.
    git(dir, &["reset", "--hard", "-q"]);
    git(dir, &["clean", "-fdq"]);

    repo.stash_pop(0).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed\n");
    assert!(repo.stashes().unwrap().is_empty());

    // Nothing left to stash: the caller shows "No local changes".
    // The pop restored the changes, so stash them once more and drop them.
    assert!(repo.stash_save(None).unwrap());
    repo.stash_drop(0).unwrap();
    assert!(repo.stashes().unwrap().is_empty());
}

#[test]
fn a_clean_work_tree_has_nothing_to_stash() {
    let (_tmp, repo) = repo_with_history();
    assert!(!repo.stash_save(None).unwrap());
}

#[test]
fn signing_key_follows_the_configuration() {
    let (tmp, repo) = repo_with_history();
    assert_eq!(repo.signing_key().unwrap(), None);
    git(tmp.path(), &["config", "user.signingkey", "9F3A21C4"]);
    assert_eq!(repo.signing_key().unwrap().as_deref(), Some("9F3A21C4"));
}
