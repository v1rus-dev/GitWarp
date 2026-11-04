//! End-to-end tests for merge, the merge preview and the operation state: every
//! one runs the real `git` in a throw-away repository.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{
    ConflictSide, Git, GitChoice, GitVersion, MergeOutcome, MergePreview, MergeType, Operation,
    Repo,
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
    // Repo commands run the real git (global config included), so the identity
    // and signing must be local, as in tests/ref_ops.rs.
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
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
        MergeOutcome::Stopped {
            conflicts: vec!["a.txt".to_owned()]
        }
    );
}

#[test]
fn merge_conflicts_are_an_outcome_and_abort_restores_head() {
    let (tmp, repo) = conflicting();
    let head = git_out(tmp.path(), &["rev-parse", "HEAD"]);
    let outcome = repo
        .merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert_eq!(
        outcome,
        MergeOutcome::Stopped {
            conflicts: vec!["a.txt".to_owned()]
        }
    );
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

/// The shape from the bug report: two conflicted files plus one file the merge
/// added cleanly.
fn stopped_merge() -> (tempfile::TempDir, Repo) {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "graph.rs", "base\n", "graph");
    commit_file(dir, "sidebar.rs", "base\n", "sidebar");
    git(dir, &["branch", "-f", "side"]);
    commit_file(dir, "graph.rs", "main\n", "M graph");
    commit_file(dir, "sidebar.rs", "main\n", "M sidebar");
    git(dir, &["switch", "-q", "side"]);
    commit_file(dir, "graph.rs", "side\n", "S graph");
    commit_file(dir, "sidebar.rs", "side\n", "S sidebar");
    commit_file(dir, "new.rs", "new\n", "S new");
    git(dir, &["switch", "-q", "main"]);
    let outcome = repo
        .merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Stopped { .. }));
    (tmp, repo)
}

#[test]
fn status_lists_each_conflicted_path_once() {
    let (_tmp, repo) = stopped_merge();
    let status = repo.working_status().unwrap();
    let paths =
        |files: &[git_core::WorkingFile]| files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&status.conflicted), ["graph.rs", "sidebar.rs"]);
    assert_eq!(paths(&status.staged), ["new.rs"]);
    assert!(status.unstaged.is_empty(), "{:?}", status.unstaged);
}

#[test]
fn merge_heading_reads_the_branch_name() {
    let (_tmp, repo) = stopped_merge();
    assert_eq!(repo.merge_heading().unwrap().as_deref(), Some("side"));
    let message = repo.prepared_message().unwrap().unwrap();
    assert!(message.starts_with("Merge branch 'side'"));
    assert!(!message.contains('#'), "{message}");
}

#[test]
fn merge_heading_formats() {
    let (tmp, repo) = base();
    assert_eq!(repo.merge_heading().unwrap(), None);
    assert_eq!(repo.prepared_message().unwrap(), None);
    let msg = repo.git_dir().join("MERGE_MSG");
    for (text, want) in [
        ("Merge branch 'feature/x'\n", Some("feature/x")),
        (
            "Merge remote-tracking branch 'origin/dev'\n",
            Some("origin/dev"),
        ),
        (
            "Merge branch 'a' of https://host/repo\n\n# Conflicts:\n",
            Some("a"),
        ),
        ("Merge branch 'a' into main\n", Some("a")),
        ("Merged something\n", None),
        ("", None),
    ] {
        fs::write(&msg, text).unwrap();
        assert_eq!(repo.merge_heading().unwrap().as_deref(), want, "{text:?}");
    }
    drop(tmp);
}

#[test]
fn resolve_take_ours_and_theirs_then_commit_finishes_the_merge() {
    let (tmp, repo) = stopped_merge();
    repo.resolve_take("graph.rs", ConflictSide::Ours).unwrap();
    repo.resolve_take("sidebar.rs", ConflictSide::Theirs)
        .unwrap();
    assert_eq!(
        fs::read_to_string(tmp.path().join("graph.rs")).unwrap(),
        "main\n"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("sidebar.rs")).unwrap(),
        "side\n"
    );
    let status = repo.working_status().unwrap();
    assert!(status.conflicted.is_empty());
    // graph.rs took our side, so it no longer differs from HEAD.
    assert_eq!(status.staged.len(), 2);
    assert_eq!(repo.operation().unwrap(), Operation::Merge);
    let message = repo.prepared_message().unwrap().unwrap();
    repo.commit(&message, false).unwrap();
    assert_eq!(repo.operation().unwrap(), Operation::None);
    assert_eq!(
        git_out(tmp.path(), &["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        3
    );
}

#[test]
fn resolve_take_deletion_side_removes_the_file() {
    let (tmp, repo) = base();
    let dir = tmp.path();
    // main modifies a.txt, side deletes it.
    commit_file(dir, "a.txt", "main\n", "M");
    git(dir, &["switch", "-q", "side"]);
    git(dir, &["rm", "-q", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "S"]);
    git(dir, &["switch", "-q", "main"]);
    assert!(matches!(
        repo.merge("side", MergeType::FastForwardIfPossible)
            .unwrap(),
        MergeOutcome::Stopped { .. }
    ));
    assert_eq!(repo.working_status().unwrap().conflicted.len(), 1);
    repo.resolve_take("a.txt", ConflictSide::Theirs).unwrap();
    assert!(!dir.join("a.txt").exists());
    assert!(repo.working_status().unwrap().conflicted.is_empty());
    repo.commit("Merge side", false).unwrap();
    assert_eq!(repo.operation().unwrap(), Operation::None);
}

#[test]
fn resolve_take_keeps_a_file_the_chosen_side_still_has() {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "a.txt", "main\n", "M");
    git(dir, &["switch", "-q", "side"]);
    git(dir, &["rm", "-q", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "S"]);
    git(dir, &["switch", "-q", "main"]);
    repo.merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    repo.resolve_take("a.txt", ConflictSide::Ours).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "main\n");
}

#[test]
fn merge_during_a_stopped_merge_is_an_error_not_conflicts() {
    let (tmp, repo) = stopped_merge();
    git(tmp.path(), &["branch", "other", "side"]);
    let err = repo
        .merge("other", MergeType::FastForwardIfPossible)
        .unwrap_err();
    assert!(matches!(err, git_core::Error::Git { .. }), "{err:?}");
    assert_eq!(repo.operation().unwrap(), Operation::Merge);
}

#[test]
fn fast_forward_to_a_merge_commit_is_a_fast_forward() {
    let (tmp, repo) = diverged();
    let dir = tmp.path();
    // `merged` = main + side joined by a merge commit; main then sits behind it.
    git(dir, &["switch", "-q", "-c", "merged"]);
    git(dir, &["merge", "-q", "--no-edit", "side"]);
    git(dir, &["switch", "-q", "main"]);
    let tip = git_out(dir, &["rev-parse", "merged"]);
    assert_eq!(
        repo.merge("merged", MergeType::FastForwardIfPossible)
            .unwrap(),
        MergeOutcome::FastForwarded
    );
    assert_eq!(git_out(dir, &["rev-parse", "HEAD"]), tip);
}

#[test]
fn merge_no_commit_from_config_is_a_stopped_merge() {
    let (tmp, repo) = diverged();
    let dir = tmp.path();
    git(dir, &["config", "branch.main.mergeOptions", "--no-commit"]);
    let head = git_out(dir, &["rev-parse", "HEAD"]);
    assert_eq!(
        repo.merge("side", MergeType::NoFastForward).unwrap(),
        MergeOutcome::Stopped {
            conflicts: Vec::new()
        }
    );
    assert_eq!(git_out(dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(repo.operation().unwrap(), Operation::Merge);
}

#[cfg(unix)]
#[test]
fn rejecting_pre_merge_commit_hook_is_a_stopped_merge() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, repo) = diverged();
    let hook = repo.git_dir().join("hooks").join("pre-merge-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        repo.merge("side", MergeType::NoFastForward).unwrap(),
        MergeOutcome::Stopped {
            conflicts: Vec::new()
        }
    );
    assert_eq!(repo.operation().unwrap(), Operation::Merge);
    drop(tmp);
}

#[test]
fn conflicted_squash_is_a_squash_operation_and_abort_cleans_up() {
    let (tmp, repo) = conflicting();
    let dir = tmp.path();
    let head = git_out(dir, &["rev-parse", "HEAD"]);
    assert_eq!(
        repo.merge("side", MergeType::Squash).unwrap(),
        MergeOutcome::Stopped {
            conflicts: vec!["a.txt".to_owned()]
        }
    );
    assert_eq!(repo.operation().unwrap(), Operation::Squash);
    repo.merge_abort().unwrap();
    assert_eq!(repo.operation().unwrap(), Operation::None);
    assert!(!repo.git_dir().join("SQUASH_MSG").exists());
    assert_eq!(git_out(dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_out(dir, &["status", "--porcelain"]), "");
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "main\n");
}

#[test]
fn squash_prefills_the_squash_message() {
    let (_tmp, repo) = diverged();
    repo.merge("side", MergeType::Squash).unwrap();
    assert_eq!(repo.operation().unwrap(), Operation::Squash);
    let message = repo.prepared_message().unwrap().unwrap();
    assert!(
        message.contains("Squashed commit of the following"),
        "{message}"
    );
    assert!(message.contains("S2"), "{message}");
}

#[test]
fn prepared_message_respects_a_custom_comment_char() {
    let (tmp, repo) = stopped_merge();
    git(tmp.path(), &["config", "core.commentChar", ";"]);
    fs::write(
        repo.git_dir().join("MERGE_MSG"),
        "Merge branch 'side'\n\n# keep me\n; drop me\n",
    )
    .unwrap();
    let message = repo.prepared_message().unwrap().unwrap();
    assert_eq!(message, "Merge branch 'side'\n\n# keep me");
}

#[test]
fn resolve_take_treats_paths_literally() {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "a[1].txt", "base\n", "br");
    commit_file(dir, "a1.txt", "base\n", "plain");
    git(dir, &["branch", "-f", "side"]);
    commit_file(dir, "a[1].txt", "main\n", "M");
    git(dir, &["switch", "-q", "side"]);
    commit_file(dir, "a[1].txt", "side\n", "S");
    git(dir, &["switch", "-q", "main"]);
    assert!(matches!(
        repo.merge("side", MergeType::FastForwardIfPossible)
            .unwrap(),
        MergeOutcome::Stopped { .. }
    ));
    fs::write(dir.join("a1.txt"), "local edit\n").unwrap();
    repo.resolve_take("a[1].txt", ConflictSide::Theirs).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("a1.txt")).unwrap(),
        "local edit\n"
    );
    assert_eq!(fs::read_to_string(dir.join("a[1].txt")).unwrap(), "side\n");
}

#[test]
fn stage_unstage_discard_treat_paths_literally() {
    let (tmp, repo) = base();
    let dir = tmp.path();
    commit_file(dir, "a[1].txt", "base\n", "br");
    commit_file(dir, "a1.txt", "base\n", "plain");
    fs::write(dir.join("a[1].txt"), "x\n").unwrap();
    fs::write(dir.join("a1.txt"), "y\n").unwrap();
    repo.stage(&["a[1].txt".to_owned()]).unwrap();
    assert_eq!(
        git_out(dir, &["diff", "--cached", "--name-only"]),
        "a[1].txt"
    );
    repo.unstage(&["a[1].txt".to_owned()]).unwrap();
    assert_eq!(git_out(dir, &["diff", "--cached", "--name-only"]), "");
    repo.discard(&["a[1].txt".to_owned()], &[]).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a1.txt")).unwrap(), "y\n");
    assert_eq!(fs::read_to_string(dir.join("a[1].txt")).unwrap(), "base\n");
}

#[test]
fn stash_save_with_only_conflicts_surfaces_gits_error() {
    let (_tmp, repo) = conflicting();
    repo.merge("side", MergeType::FastForwardIfPossible)
        .unwrap();
    assert!(repo.working_status().unwrap().staged.is_empty());
    // `Ok(false)` ("nothing to stash") would hide the stopped merge.
    let result = repo.stash_save(None);
    assert!(result.is_err(), "{result:?}");
}
