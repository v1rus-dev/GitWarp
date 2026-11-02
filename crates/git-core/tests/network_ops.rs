//! End-to-end tests for the stage 8 network operations. Everything runs
//! against bare repositories in temporary folders, so no network is needed.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::network::{
    self, CancelToken, CloneOptions, FetchOptions, NetworkContext, PushOptions, PushStatus,
};
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
            "tag.gpgsign=false",
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

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git is installed");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn commit_file(dir: &Path, file: &str, contents: &str, message: &str) {
    fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", "--", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A source repository plus a bare remote it can push to.
fn source_with_remote() -> (tempfile::TempDir, tempfile::TempDir, Repo) {
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q"]);
    git(source.path(), &["config", "user.name", "Test"]);
    git(source.path(), &["config", "user.email", "test@example.com"]);
    commit_file(source.path(), "a.txt", "one\n", "A");

    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        source.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let repo = open(source.path());
    (source, remote, repo)
}

fn clone_into(remote: &Path) -> (tempfile::TempDir, Repo) {
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("clone");
    network::clone(
        &Git::resolve(GitChoice::System).unwrap().git,
        &CloneOptions {
            url: remote.display().to_string(),
            dest: dest.clone(),
            branch: None,
            depth: None,
            recurse_submodules: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    // Git for Windows enables `core.autocrlf` system-wide; files pulled later must keep LF.
    git(&dest, &["config", "core.autocrlf", "false"]);
    let repo = open(&dest);
    (parent, repo)
}

fn branch(repo: &Repo) -> String {
    match repo.head().unwrap() {
        Head::Branch { name, .. } => name,
        Head::Detached(_) => panic!("expected a branch"),
    }
}

fn tag_names(repo: &Repo) -> Vec<String> {
    repo.refs()
        .unwrap()
        .into_iter()
        .filter(|reference| reference.kind == RefKind::Tag)
        .map(|reference| reference.name)
        .collect()
}

#[test]
fn pushes_a_branch_and_remembers_the_upstream() {
    let (_source, _remote, repo) = source_with_remote();
    let outcome = network::push(
        &repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    assert!(outcome.changed, "{:?}", outcome.lines);

    let branch = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|reference| reference.kind == RefKind::LocalBranch && reference.name == "main")
        .unwrap();
    assert_eq!(branch.upstream.as_deref(), Some("origin/main"));
}

#[test]
fn fetches_new_commits_tags_and_prunes_deleted_branches() {
    let (source, remote, source_repo) = source_with_remote();
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    git(source.path(), &["tag", "v1"]);
    network::push_tag(
        &source_repo,
        "origin",
        "v1",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    let (_parent, clone) = clone_into(remote.path());
    assert!(tag_names(&clone).contains(&"v1".to_owned()));

    // A new commit and a branch on the source, then a second push.
    commit_file(source.path(), "b.txt", "two\n", "B");
    git(source.path(), &["branch", "gone"]);
    let source_repo = open(source.path());
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("gone".to_owned()),
            set_upstream: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    let mut changes = Vec::new();
    let outcome = network::fetch(
        &clone,
        &FetchOptions {
            remote: Some("origin".to_owned()),
            tags: true,
            prune: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |progress| changes.push(progress),
    )
    .unwrap();
    assert!(outcome.changed, "{:?}", outcome.lines);
    assert!(!changes.is_empty(), "fetch should report progress");
    let names: Vec<String> = clone.refs().unwrap().into_iter().map(|r| r.name).collect();
    assert!(names.contains(&"origin/main".to_owned()), "{names:?}");
    assert!(names.contains(&"origin/gone".to_owned()), "{names:?}");

    // Delete the branch and prune the stale tracking ref.
    network::delete_remote_branch(
        &source_repo,
        "origin",
        "gone",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    network::fetch(
        &clone,
        &FetchOptions {
            remote: Some("origin".to_owned()),
            tags: false,
            prune: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    let names: Vec<String> = clone.refs().unwrap().into_iter().map(|r| r.name).collect();
    assert!(!names.contains(&"origin/gone".to_owned()), "{names:?}");
}

#[test]
fn pulls_a_fast_forward_and_refuses_to_merge() {
    let (source, remote, source_repo) = source_with_remote();
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    let (_parent, clone) = clone_into(remote.path());
    assert_eq!(branch(&clone), "main");

    commit_file(source.path(), "b.txt", "two\n", "B");
    let source_repo = open(source.path());
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    let outcome = network::pull(
        &clone,
        None,
        None,
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    assert!(!outcome.lines.is_empty());
    assert_eq!(
        fs::read_to_string(clone.root().join("b.txt")).unwrap(),
        "two\n"
    );

    // A diverged branch must fail rather than start a merge.
    commit_file(source.path(), "c.txt", "three\n", "C");
    let source_repo = open(source.path());
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    commit_file(clone.root(), "local.txt", "local\n", "local");
    let error = network::pull(
        &clone,
        None,
        None,
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("fast-forward"), "{error}");
}

#[test]
fn pushes_all_tags() {
    let (_source, _remote, repo) = source_with_remote();
    git(repo.root(), &["tag", "v1"]);
    git(repo.root(), &["tag", "v2"]);
    network::push(
        &repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    network::push_tags(
        &repo,
        "origin",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    let remote_tags = git_out(repo.root(), &["ls-remote", "--tags", "origin"]);
    assert!(remote_tags.contains("refs/tags/v1"), "{remote_tags}");
    assert!(remote_tags.contains("refs/tags/v2"), "{remote_tags}");
}

#[test]
fn sets_an_upstream_explicitly() {
    let (source, _remote, source_repo) = source_with_remote();
    git(source.path(), &["branch", "feat"]);
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("feat".to_owned()),
            set_upstream: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    network::set_upstream(&source_repo, "feat", "origin", "feat").unwrap();
    let feat = source_repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|reference| reference.name == "feat")
        .unwrap();
    assert_eq!(feat.upstream.as_deref(), Some("origin/feat"));
}

#[test]
fn clone_reports_progress_and_creates_a_repository() {
    let (_source, remote, source_repo) = source_with_remote();
    network::push(
        &source_repo,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some("main".to_owned()),
            set_upstream: true,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("work");
    let mut progresses = Vec::new();
    network::clone(
        &Git::resolve(GitChoice::System).unwrap().git,
        &CloneOptions {
            url: remote.path().display().to_string(),
            dest: dest.clone(),
            branch: Some("main".to_owned()),
            depth: Some(1),
            recurse_submodules: false,
        },
        &NetworkContext::default(),
        &CancelToken::new(),
        |progress| progresses.push(progress),
    )
    .unwrap();
    let repo = open(&dest);
    assert_eq!(branch(&repo), "main");
    assert_eq!(
        repo.remote_url("origin").unwrap(),
        Some(remote.path().display().to_string())
    );
    assert!(dest.join("a.txt").is_file());
    assert!(
        progresses
            .iter()
            .any(|progress| progress.text.contains("Cloning")),
        "{progresses:?}"
    );
}

#[test]
fn clone_target_extracts_names() {
    assert_eq!(
        network::suggested_folder("https://github.com/nexu-io/open-design.git").as_deref(),
        Some("open-design")
    );
    assert_eq!(
        network::suggested_folder("git@github.com:nexu-io/open-design.git").as_deref(),
        Some("open-design")
    );
}

/// A context that ignores the user's own git configuration.
fn isolated() -> NetworkContext {
    NetworkContext::with_env([
        ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
    ])
}

fn bare() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--bare"]);
    dir
}

fn push_main(set_upstream: bool) -> PushOptions {
    PushOptions {
        remote: Some("origin".to_owned()),
        refspec: Some("main".to_owned()),
        set_upstream,
    }
}

#[test]
fn multi_push_continues_after_a_failed_target_and_updates_the_tracking_ref() {
    let (source, _remote, repo) = source_with_remote();
    let (first, third) = (bare(), bare());
    let missing = source.path().join("does-not-exist.git");
    let urls: Vec<String> = [first.path(), missing.as_path(), third.path()]
        .iter()
        .map(|p| p.to_str().unwrap().to_owned())
        .collect();

    let outcome = network::push_each(
        &repo,
        &urls,
        &push_main(true),
        &isolated(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.targets.len(), 3);
    assert_eq!(
        outcome.targets[0].result,
        Ok(PushStatus::Pushed { commits: Some(1) })
    );
    assert!(
        outcome.targets[1].result.is_err(),
        "{:?}",
        outcome.targets[1]
    );
    assert_eq!(
        outcome.targets[2].result,
        Ok(PushStatus::Pushed { commits: Some(1) })
    );
    assert!(outcome.changed);
    // The ones after the failure were pushed too.
    let head = git_out(source.path(), &["rev-parse", "HEAD"]);
    assert_eq!(git_out(first.path(), &["rev-parse", "main"]), head);
    assert_eq!(git_out(third.path(), &["rev-parse", "main"]), head);
    // Tracking ref and upstream follow, as for a named remote.
    assert_eq!(
        git_out(source.path(), &["rev-parse", "refs/remotes/origin/main"]),
        head
    );
    let local = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == RefKind::LocalBranch && r.name == "main")
        .unwrap();
    assert_eq!(local.upstream.as_deref(), Some("origin/main"));
}

#[test]
fn multi_push_to_all_targets_reports_each_and_updates_the_tracking_ref_once() {
    let (source, _remote, repo) = source_with_remote();
    let (first, second) = (bare(), bare());
    let urls: Vec<String> = [first.path(), second.path()]
        .iter()
        .map(|p| p.to_str().unwrap().to_owned())
        .collect();
    let push = |repo: &Repo| {
        network::push_each(
            repo,
            &urls,
            &push_main(false),
            &isolated(),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap()
    };

    let outcome = push(&repo);
    assert!(outcome.targets.iter().all(|t| t.result.is_ok()));
    let head = git_out(source.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        git_out(source.path(), &["rev-parse", "refs/remotes/origin/main"]),
        head
    );

    // Nothing new: every target is up to date, and nothing "changed".
    let again = push(&repo);
    assert!(!again.changed);
    assert!(again
        .targets
        .iter()
        .all(|t| t.result == Ok(PushStatus::UpToDate)));

    // A new commit is counted per target.
    commit_file(source.path(), "b.txt", "two\n", "B");
    let third_push = push(&repo);
    assert!(third_push
        .targets
        .iter()
        .all(|t| t.result == Ok(PushStatus::Pushed { commits: Some(1) })));
    assert_eq!(
        git_out(source.path(), &["rev-parse", "refs/remotes/origin/main"]),
        git_out(source.path(), &["rev-parse", "HEAD"])
    );
}

#[test]
fn multi_push_reports_a_rejected_target_without_touching_the_tracking_ref() {
    let (source, _remote, repo) = source_with_remote();
    let target = bare();
    // The target already has a diverging `main`.
    let other = tempfile::tempdir().unwrap();
    git(other.path(), &["init", "-q"]);
    commit_file(other.path(), "x.txt", "x\n", "X");
    git(
        other.path(),
        &[
            "push",
            "-q",
            target.path().to_str().unwrap(),
            "HEAD:refs/heads/main",
        ],
    );
    let urls = vec![target.path().to_str().unwrap().to_owned()];

    let outcome = network::push_each(
        &repo,
        &urls,
        &push_main(false),
        &isolated(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    // The reason is git's porcelain summary, not "failed to push some refs".
    assert_eq!(
        outcome.targets[0].result,
        Err("rejected (fetch first)".to_owned())
    );
    assert!(!outcome.targets[0].partial);
    assert!(!outcome.changed);
    let tracking = Command::new("git")
        .arg("-C")
        .arg(source.path())
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/remotes/origin/main",
        ])
        .output()
        .unwrap();
    assert!(!tracking.status.success(), "no target took the push");
}

fn urls_of(dirs: &[&Path]) -> Vec<String> {
    dirs.iter()
        .map(|p| p.to_str().unwrap().to_owned())
        .collect()
}

fn run_each(repo: &Repo, urls: &[String], refspec: &str) -> network::NetworkOutcome {
    network::push_each(
        repo,
        urls,
        &PushOptions {
            remote: Some("origin".to_owned()),
            refspec: Some(refspec.to_owned()),
            set_upstream: false,
        },
        &isolated(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap()
}

#[test]
fn multi_push_gives_the_hooks_reason_for_a_declined_target() {
    let (source, _remote, repo) = source_with_remote();
    let (open_target, declining) = (bare(), bare());
    let hook = declining.path().join("hooks/pre-receive");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let urls = urls_of(&[declining.path(), open_target.path()]);
    let outcome = run_each(&repo, &urls, "main");
    assert_eq!(
        outcome.targets[0].result,
        Err("remote rejected (pre-receive hook declined)".to_owned())
    );
    // The next target is still pushed, and the tracking ref follows it.
    assert!(outcome.targets[1].result.is_ok());
    let head = git_out(source.path(), &["rev-parse", "HEAD"]);
    assert_eq!(git_out(open_target.path(), &["rev-parse", "main"]), head);
    assert_eq!(
        git_out(source.path(), &["rev-parse", "refs/remotes/origin/main"]),
        head
    );
}

#[test]
fn multi_push_reports_a_partly_refused_push_as_partial() {
    let (source, _remote, repo) = source_with_remote();
    let target = bare();
    // The target already has `v1` at another commit; `v2` is new there.
    git(source.path(), &["tag", "v2"]);
    commit_file(source.path(), "b.txt", "two\n", "B");
    git(source.path(), &["tag", "v1"]);
    let other = tempfile::tempdir().unwrap();
    git(other.path(), &["init", "-q"]);
    commit_file(other.path(), "x.txt", "x\n", "X");
    git(other.path(), &["tag", "v1"]);
    git(
        other.path(),
        &[
            "push",
            "-q",
            target.path().to_str().unwrap(),
            "refs/tags/v1",
        ],
    );
    let urls = urls_of(&[target.path()]);
    let outcome = run_each(&repo, &urls, "refs/tags/*:refs/tags/*");
    let only = &outcome.targets[0];
    assert!(only.partial, "{only:?}");
    assert_eq!(only.result, Err("rejected (already exists)".to_owned()));
    // The new tag did go through.
    assert!(git_out(target.path(), &["tag", "--list"]).contains("v2"));
}

#[test]
fn a_new_branch_is_not_reported_as_up_to_date() {
    let (source, _remote, repo) = source_with_remote();
    git(source.path(), &["checkout", "-q", "-b", "feature"]);
    for n in 0..3 {
        commit_file(source.path(), &format!("f{n}.txt"), "x\n", "F");
    }
    let (a, b) = (bare(), bare());
    let outcome = run_each(&repo, &urls_of(&[a.path(), b.path()]), "feature");
    for target in &outcome.targets {
        assert_eq!(target.result, Ok(PushStatus::Pushed { commits: Some(4) }));
    }
    // A branch whose commits are all on a remote ref already: no count, but
    // still "new branch", never "up to date".
    git(source.path(), &["branch", "same", "feature"]);
    let c = bare();
    git(
        source.path(),
        &["push", "-q", c.path().to_str().unwrap(), "feature"],
    );
    git(
        source.path(),
        &["update-ref", "refs/remotes/origin/feature", "feature"],
    );
    let outcome = run_each(&repo, &urls_of(&[c.path()]), "same");
    assert_eq!(outcome.targets[0].result, Ok(PushStatus::NewBranch));
}

#[test]
fn multi_push_tags_go_to_every_target() {
    let (source, _remote, repo) = source_with_remote();
    git(source.path(), &["tag", "v1"]);
    git(source.path(), &["tag", "v2"]);
    let missing = source.path().join("missing.git");
    let (a, b) = (bare(), bare());
    let urls = urls_of(&[a.path(), missing.as_path(), b.path()]);

    let one = run_each(&repo, &urls, "refs/tags/v1");
    assert_eq!(one.targets[0].result, Ok(PushStatus::NewTag));
    assert!(one.targets[1].result.is_err());
    assert_eq!(one.targets[2].result, Ok(PushStatus::NewTag));
    assert_eq!(git_out(a.path(), &["tag", "--list"]).trim(), "v1");

    let all = run_each(&repo, &urls, "refs/tags/*:refs/tags/*");
    assert!(all.targets[0].result.is_ok() && all.targets[2].result.is_ok());
    assert!(all.targets[1].result.is_err());
    for target in [&a, &b] {
        assert_eq!(git_out(target.path(), &["tag", "--list"]).trim(), "v1\nv2");
    }
    // A tag push has no tracking branch to move.
    let tracking = Command::new("git")
        .arg("-C")
        .arg(source.path())
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/remotes/origin/main",
        ])
        .output()
        .unwrap();
    assert!(!tracking.status.success());
}

#[test]
fn delete_each_removes_the_branch_everywhere_and_the_tracking_ref() {
    let (source, _remote, repo) = source_with_remote();
    let (a, b) = (bare(), bare());
    let missing = source.path().join("missing.git");
    git(source.path(), &["branch", "topic"]);
    let all = urls_of(&[a.path(), b.path()]);
    run_each(&repo, &all, "topic");
    git(
        source.path(),
        &["update-ref", "refs/remotes/origin/topic", "topic"],
    );

    // One target is unreachable: the others still lose the branch.
    let urls = urls_of(&[a.path(), missing.as_path(), b.path()]);
    let outcome = network::delete_each(
        &repo,
        &urls,
        "origin",
        "topic",
        &isolated(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.targets[0].result, Ok(PushStatus::Deleted));
    assert!(outcome.targets[1].result.is_err());
    assert_eq!(outcome.targets[2].result, Ok(PushStatus::Deleted));
    for target in [&a, &b] {
        assert!(!git_out(target.path(), &["branch", "--list"]).contains("topic"));
    }
    let tracking = Command::new("git")
        .arg("-C")
        .arg(source.path())
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/remotes/origin/topic",
        ])
        .output()
        .unwrap();
    assert!(!tracking.status.success(), "the tracking ref is gone");

    // Nothing took it: the tracking ref stays.
    git(
        source.path(),
        &["update-ref", "refs/remotes/origin/topic", "main"],
    );
    let none = urls_of(&[missing.as_path()]);
    network::delete_each(
        &repo,
        &none,
        "origin",
        "topic",
        &isolated(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    git_out(
        source.path(),
        &["rev-parse", "--verify", "refs/remotes/origin/topic"],
    );
}
