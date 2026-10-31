//! End-to-end tests for the stage 8 network operations. Everything runs
//! against bare repositories in temporary folders, so no network is needed.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::network::{
    self, CancelToken, CloneOptions, FetchOptions, NetworkContext, PushOptions,
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
