//! Calls behind the worktree workspace and the History branch filter:
//! changed entries, head summaries, main line, branch log, tracking worktree.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{Git, GitChoice, Oid, Repo};

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
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
        .output()
        .expect("git is installed")
}

fn git(dir: &Path, args: &[&str]) {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn rev(dir: &Path, name: &str) -> Oid {
    let out = run(dir, &["rev-parse", name]);
    Oid::from_hex(String::from_utf8_lossy(&out.stdout).trim()).unwrap()
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
}

fn write_commit(dir: &Path, file: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn new_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    tmp
}

#[test]
fn changed_entries_carry_a_status_letter() {
    let tmp = new_repo();
    let d = tmp.path();
    write_commit(d, "mod.txt", "1\n", "a");
    write_commit(d, "del.txt", "x\n", "b");
    write_commit(d, "old.txt", "some longer content\nfor rename\n", "c");
    std::fs::write(d.join("mod.txt"), "2\n").unwrap();
    std::fs::write(d.join("add.txt"), "new\n").unwrap();
    git(d, &["add", "add.txt"]);
    git(d, &["rm", "-q", "del.txt"]);
    git(d, &["mv", "old.txt", "new.txt"]);
    std::fs::write(d.join("untracked.txt"), "u\n").unwrap();

    let entries = open(d).changed_entries().unwrap();
    let got: Vec<(&str, char)> = entries
        .iter()
        .map(|e| (e.path.as_str(), e.status))
        .collect();
    assert_eq!(
        got,
        [
            ("add.txt", 'A'),
            ("del.txt", 'D'),
            ("mod.txt", 'M'),
            ("new.txt", 'R'),
            ("untracked.txt", '?'),
        ]
    );
    // The thin wrapper keeps listing the same paths.
    let paths = open(d).changed_paths().unwrap();
    assert_eq!(paths.len(), 5);
}

#[test]
fn changed_entries_mark_conflicts_with_u() {
    let tmp = new_repo();
    let d = tmp.path();
    write_commit(d, "f.txt", "base\n", "base");
    git(d, &["checkout", "-q", "-b", "other"]);
    write_commit(d, "f.txt", "other\n", "other");
    git(d, &["checkout", "-q", "main"]);
    write_commit(d, "f.txt", "main\n", "main");
    assert!(!run(d, &["merge", "other"]).status.success());

    let entries = open(d).changed_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].path.as_str(), entries[0].status),
        ("f.txt", 'U')
    );
}

#[test]
fn head_summaries_batch_skips_unknown_ids() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "first one");
    let a = rev(d, "HEAD");
    commit(d, "second one");
    let b = rev(d, "HEAD");
    commit(d, "third one");
    let c = rev(d, "HEAD");
    let unknown = Oid::from_hex(&"1".repeat(40)).unwrap();

    let repo = open(d);
    let map = repo.head_summaries(&[a, unknown, b, c]).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(map[&a], "first one");
    assert_eq!(map[&b], "second one");
    assert_eq!(map[&c], "third one");
    assert!(repo.head_summaries(&[]).unwrap().is_empty());
}

#[test]
fn main_line_follows_origin_head_when_the_local_branch_exists() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "a");
    git(d, &["branch", "develop"]);
    git(
        d,
        &["remote", "add", "origin", "https://example.invalid/r.git"],
    );
    git(d, &["update-ref", "refs/remotes/origin/develop", "HEAD"]);
    git(
        d,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    );
    assert_eq!(open(d).main_line().unwrap().as_deref(), Some("develop"));

    // origin/HEAD pointing at a branch with no local counterpart falls back to main.
    git(d, &["update-ref", "refs/remotes/origin/gone", "HEAD"]);
    git(
        d,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/gone",
        ],
    );
    assert_eq!(open(d).main_line().unwrap().as_deref(), Some("main"));
}

#[test]
fn main_line_falls_back_to_master_then_current_then_none() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "a");
    git(d, &["branch", "-m", "master"]);
    assert_eq!(open(d).main_line().unwrap().as_deref(), Some("master"));

    git(d, &["branch", "-m", "trunk"]);
    assert_eq!(open(d).main_line().unwrap().as_deref(), Some("trunk"));

    git(d, &["checkout", "-q", "--detach"]);
    assert_eq!(open(d).main_line().unwrap(), None);
}

#[test]
fn branch_log_of_a_linear_feature_branch_ends_at_the_fork_point() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    commit(d, "B");
    let fork = rev(d, "HEAD");
    git(d, &["checkout", "-q", "-b", "feature"]);
    commit(d, "C1");
    commit(d, "C2");
    git(d, &["checkout", "-q", "main"]);
    commit(d, "D");

    let log = open(d).branch_log("feature", Some("main"), 100).unwrap();
    let names: Vec<_> = log.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(names, ["C2", "C1", "B"]);
    assert_eq!(log.fork_point, Some(fork));
    assert_eq!(log.commits.last().unwrap().id, fork);
    assert_eq!(log.merged_into, None);
    assert!(log.merged_from.is_empty());
}

#[test]
fn branch_log_of_main_has_no_fork_point_and_names_merged_branches() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    git(d, &["checkout", "-q", "-b", "feat-one"]);
    commit(d, "C1");
    git(d, &["checkout", "-q", "main"]);
    commit(d, "B");
    git(d, &["merge", "-q", "--no-ff", "--no-edit", "feat-one"]);
    git(d, &["checkout", "-q", "-b", "feat-two"]);
    commit(d, "C2");
    let two_tip = rev(d, "HEAD");
    git(d, &["checkout", "-q", "main"]);
    // A custom subject without a branch name falls back to the branch tip, then to the hash.
    git(
        d,
        &["merge", "-q", "--no-ff", "-m", "custom subject", "feat-two"],
    );
    let merge_two = rev(d, "HEAD");
    git(d, &["branch", "-D", "feat-two"]);

    let log = open(d).branch_log("main", Some("main"), 100).unwrap();
    assert_eq!(log.fork_point, None);
    assert_eq!(log.merged_into, None);
    let merges: Vec<_> = log
        .commits
        .iter()
        .filter(|c| c.parents.len() == 2)
        .collect();
    assert_eq!(merges.len(), 2);
    assert_eq!(log.merged_from[&merges[0].id], two_tip.short());
    assert_eq!(merges[0].id, merge_two);
    assert_eq!(log.merged_from[&merges[1].id], "feat-one");
    // First-parent only: the side branch commits are not listed.
    assert!(log.commits.iter().all(|c| c.summary != "C1"));
}

#[test]
fn branch_log_names_a_merged_branch_by_its_local_tip_when_the_subject_has_no_name() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    git(d, &["checkout", "-q", "-b", "topic"]);
    commit(d, "C1");
    git(d, &["checkout", "-q", "main"]);
    commit(d, "B");
    git(
        d,
        &["merge", "-q", "--no-ff", "-m", "custom subject", "topic"],
    );
    let log = open(d).branch_log("main", None, 100).unwrap();
    let merge = log.commits.iter().find(|c| c.parents.len() == 2).unwrap();
    assert_eq!(log.merged_from[&merge.id], "topic");
}

#[test]
fn branch_log_of_a_merged_branch_points_at_the_merge() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    git(d, &["checkout", "-q", "-b", "feature"]);
    commit(d, "C1");
    git(d, &["checkout", "-q", "main"]);
    commit(d, "B");
    git(d, &["merge", "-q", "--no-ff", "--no-edit", "feature"]);
    let merge = rev(d, "HEAD");
    commit(d, "after");
    let log = open(d).branch_log("feature", Some("main"), 100).unwrap();
    assert_eq!(log.merged_into, Some(merge));
    assert!(log.fork_point.is_some());
}

#[test]
fn branch_log_of_a_missing_branch_is_an_error() {
    let tmp = new_repo();
    commit(tmp.path(), "A");
    assert!(open(tmp.path())
        .branch_log("nope", Some("main"), 10)
        .is_err());
}

#[test]
fn branch_log_works_for_remote_branches_against_the_remote_main() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    git(
        d,
        &["remote", "add", "origin", "https://example.invalid/r.git"],
    );
    git(d, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let fork = rev(d, "HEAD");
    // The local main moves on; the remote branch must compare against origin/main.
    commit(d, "local only");
    git(d, &["checkout", "-q", "-b", "scratch", "origin/main"]);
    commit(d, "R1");
    git(d, &["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    git(d, &["checkout", "-q", "main"]);
    git(d, &["branch", "-D", "scratch"]);

    let log = open(d)
        .branch_log("origin/feat", Some("main"), 100)
        .unwrap();
    let names: Vec<_> = log.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(names, ["R1", "A"]);
    assert_eq!(log.fork_point, Some(fork));
}

#[test]
fn add_worktree_tracking_creates_a_tracking_branch() {
    let tmp = new_repo();
    let d = tmp.path();
    commit(d, "A");
    git(
        d,
        &["remote", "add", "origin", "https://example.invalid/r.git"],
    );
    git(d, &["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    let target = tmp.path().join("wt-feat");

    open(d)
        .add_worktree_tracking(&target, "feat", "origin/feat")
        .unwrap();

    assert!(target.join(".git").exists());
    let upstream = run(d, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]);
    assert_eq!(
        String::from_utf8_lossy(&upstream.stdout).trim(),
        "origin/feat"
    );
}
