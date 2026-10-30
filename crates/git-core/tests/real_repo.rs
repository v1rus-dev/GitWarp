//! End-to-end: build a small repository with the real `git`, read it back
//! and lay out its graph.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{ErrorClass, Git, GitChoice, GitSource, Head, LogOptions, RefKind, Repo};
use git_graph::GraphBuilder;

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
    assert!(status.success(), "git {args:?} failed");
}

fn open(path: &Path) -> git_core::Result<Repo> {
    Repo::open(Arc::new(Git::resolve(GitChoice::System)?.git), path)
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
}

/// main:    A - B ------- M - D
///               \       /
/// feature:       C1 - C2
fn sample_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    commit(d, "A");
    commit(d, "B");
    git(d, &["checkout", "-q", "-b", "feature"]);
    commit(d, "C1");
    commit(d, "C2");
    git(d, &["checkout", "-q", "main"]);
    git(
        d,
        &["merge", "-q", "--no-ff", "--no-edit", "-m", "M", "feature"],
    );
    commit(d, "D");
    git(d, &["tag", "-a", "v1", "-m", "release"]);
    tmp
}

#[test]
fn reads_log_in_topological_order() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let commits: Vec<_> = repo
        .log(&LogOptions::default())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let subjects: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(subjects.len(), 6);
    assert_eq!(subjects[0], "D");
    assert_eq!(subjects[1], "M");
    assert_eq!(*subjects.last().unwrap(), "A");
    assert_eq!(commits[1].parents.len(), 2);
    assert!(commits.last().unwrap().parents.is_empty());
    assert_eq!(commits[0].author_name, "Test");
}

#[test]
fn log_streams_in_chunks_and_stops_early() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let mut stream = repo.log(&LogOptions::default()).unwrap();
    assert_eq!(stream.next_chunk(4).unwrap().len(), 4);
    assert_eq!(stream.next_chunk(4).unwrap().len(), 2);
    assert!(stream.next_chunk(4).unwrap().is_empty());
    // Dropping a half-read stream must not hang.
    let mut s = repo.log(&LogOptions::default()).unwrap();
    s.next();
    drop(s);
}

#[test]
fn lays_out_the_real_history() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let mut graph = GraphBuilder::new();
    let rows: Vec<_> = repo
        .log(&LogOptions::default())
        .unwrap()
        .map(|c| {
            let c = c.unwrap();
            (c.summary.clone(), graph.push(c.id, &c.parents))
        })
        .collect();

    let col = |s: &str| rows.iter().find(|(n, _)| n == s).unwrap().1.column;
    assert_eq!([col("D"), col("M"), col("B"), col("A")], [0, 0, 0, 0]);
    assert_eq!([col("C1"), col("C2")], [1, 1]);
    assert_eq!(graph.open_lanes(), 0);

    let text = git_graph::ascii::render(
        &rows.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
        |r| rows.iter().find(|(_, x)| x.id == r.id).unwrap().0.clone(),
    );
    println!("{text}");
}

#[test]
fn reads_refs_and_head() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let refs = repo.refs().unwrap();
    let names: Vec<_> = refs.iter().map(|r| (r.kind, r.name.as_str())).collect();
    assert!(names.contains(&(RefKind::LocalBranch, "main")));
    assert!(names.contains(&(RefKind::LocalBranch, "feature")));
    assert!(names.contains(&(RefKind::Tag, "v1")));

    // The annotated tag is peeled to the commit main points at.
    let main = refs.iter().find(|r| r.name == "main").unwrap();
    let tag = refs.iter().find(|r| r.name == "v1").unwrap();
    assert_eq!(main.target, tag.target);

    match repo.head().unwrap() {
        Head::Branch { name, target } => {
            assert_eq!(name, "main");
            assert_eq!(target, Some(main.target));
        }
        other => panic!("unexpected head {other:?}"),
    }
}

#[test]
fn empty_repository_has_empty_log() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    let repo = open(tmp.path()).unwrap();
    let n = repo
        .log(&LogOptions {
            all: false,
            ..Default::default()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .len();
    assert_eq!(n, 0);
}

#[test]
fn empty_repository_has_empty_log_with_all() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    let repo = open(tmp.path()).unwrap();
    assert_eq!(repo.log(&LogOptions::default()).unwrap().count(), 0);
}

#[test]
fn rejects_non_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let err = open(tmp.path()).unwrap_err();
    assert!(
        matches!(err.class(), ErrorClass::NotARepository(path) if path == tmp.path()),
        "{err:?}"
    );
}

#[test]
fn classifies_a_missing_path() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");
    let err = open(&missing).unwrap_err();
    assert!(
        matches!(err.class(), ErrorClass::PathNotFound(path) if path == missing),
        "{err:?}"
    );
}

#[test]
fn counts_commits_on_every_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    commit(d, "A");
    commit(d, "B");
    git(d, &["checkout", "-q", "-b", "feature"]);
    commit(d, "C");
    git(d, &["checkout", "-q", "main"]);
    let repo = open(d).unwrap();
    assert_eq!(repo.commit_count().unwrap(), 3);
}

#[test]
fn counts_no_commits_in_an_empty_repository() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    let repo = open(tmp.path()).unwrap();
    assert_eq!(repo.commit_count().unwrap(), 0);
    let commits: Vec<_> = repo
        .log(&LogOptions::default())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(commits.is_empty());
}

#[test]
fn init_makes_a_repository_that_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("fresh");
    std::fs::create_dir(&dir).unwrap();
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init(git, &dir).unwrap();
    assert_eq!(repo.root(), dir.canonicalize().unwrap());
    assert_eq!(repo.commit_count().unwrap(), 0);
}

#[test]
fn init_creates_a_missing_directory_too() {
    // `git init <path>` creates the directory, so the action works on a
    // folder the user has not made yet either.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("not-there");
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init(git, &dir).unwrap();
    assert_eq!(repo.commit_count().unwrap(), 0);
}

/// Gives the in-process git commands a user identity for the duration of a
/// test. `Repo::init_with` deliberately uses the user's own configuration, so
/// the test has to provide one instead of passing `-c`.
struct TestIdentity {
    global: Option<std::ffi::OsString>,
    nosystem: Option<std::ffi::OsString>,
}

impl TestIdentity {
    fn install(dir: &Path) -> Self {
        let config = dir.join("gitconfig");
        std::fs::write(
            &config,
            "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let global = std::env::var_os("GIT_CONFIG_GLOBAL");
        let nosystem = std::env::var_os("GIT_CONFIG_NOSYSTEM");
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        Self { global, nosystem }
    }
}

impl Drop for TestIdentity {
    fn drop(&mut self) {
        match &self.global {
            Some(value) => std::env::set_var("GIT_CONFIG_GLOBAL", value),
            None => std::env::remove_var("GIT_CONFIG_GLOBAL"),
        }
        match &self.nosystem {
            Some(value) => std::env::set_var("GIT_CONFIG_NOSYSTEM", value),
            None => std::env::remove_var("GIT_CONFIG_NOSYSTEM"),
        }
    }
}

#[test]
fn init_with_sets_the_initial_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("fresh-branch");
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init_with(git, &dir, Some("trunk"), false).unwrap();

    assert_eq!(repo.root(), dir.canonicalize().unwrap());
    assert_eq!(repo.commit_count().unwrap(), 0);
    assert!(!dir.join(".gitignore").exists());
    match repo.head().unwrap() {
        Head::Branch { name, target } => {
            assert_eq!(name, "trunk");
            assert_eq!(target, None);
        }
        other => panic!("unexpected head {other:?}"),
    }
}

#[test]
fn init_with_makes_the_optional_initial_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("with-initial-commit");
    let _identity = TestIdentity::install(tmp.path());
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init_with(git, &dir, Some("main"), true).unwrap();

    assert!(dir.join(".gitignore").is_file());
    assert_eq!(repo.commit_count().unwrap(), 1);
    let commits: Vec<_> = repo
        .log(&LogOptions::default())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(commits[0].summary, "Initial commit");
    assert!(commits[0].parents.is_empty());
    match repo.head().unwrap() {
        Head::Branch { name, target } => {
            assert_eq!(name, "main");
            assert_eq!(target, Some(commits[0].id));
        }
        other => panic!("unexpected head {other:?}"),
    }
}

#[test]
fn a_broken_ref_makes_git_fail_with_its_exit_code() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    std::fs::write(
        tmp.path().join(".git/refs/heads/broken"),
        "0000000000000000000000000000000000000001\n",
    )
    .unwrap();
    match repo.commit_count() {
        Err(git_core::Error::Git {
            code, ref stderr, ..
        }) => {
            assert!(code.is_some(), "no exit code: {stderr}");
            assert_eq!(code, Some(128), "{stderr}");
            assert!(!stderr.trim().is_empty());
        }
        other => panic!("unexpected result {other:?}"),
    }
}

#[test]
fn opens_bare_repository() {
    let src = sample_repo();
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("bare.git");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            src.path().to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let repo = open(&bare).unwrap();
    assert_eq!(repo.root(), repo.git_dir());
    assert_eq!(repo.log(&LogOptions::default()).unwrap().count(), 6);
}

#[test]
fn writes_commit_graph() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    // Fresh repos may or may not have one depending on git version/config.
    repo.write_commit_graph().unwrap();
    assert!(repo.has_commit_graph());
}

#[test]
fn resolves_system_git() {
    let git = Git::resolve(GitChoice::System).unwrap().git;
    assert_eq!(git.source(), GitSource::System);
    assert!(git.version() >= git_core::GitVersion::MIN);
}

#[test]
fn commit_graph_is_found_from_a_linked_worktree() {
    let tmp = sample_repo();
    let main = open(tmp.path()).unwrap();
    main.write_commit_graph().unwrap();

    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    git(
        tmp.path(),
        &[
            "worktree",
            "add",
            "-q",
            wt_path.to_str().unwrap(),
            "feature",
        ],
    );
    let linked = open(&wt_path).unwrap();
    // The linked worktree's git dir is `.git/worktrees/wt`; the commit-graph
    // lives in the common dir.
    assert_ne!(linked.git_dir(), main.git_dir());
    assert!(linked.has_commit_graph());
}

#[test]
fn revision_starting_with_a_dash_is_not_an_option() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let out = tmp.path().join("injected.txt");
    let result = repo
        .log(&LogOptions {
            all: false,
            revs: vec![format!("--output={}", out.display())],
            ..Default::default()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>();
    assert!(result.is_err());
    assert!(!out.exists());
}

#[test]
fn open_failure_other_than_not_a_repository_keeps_gits_message() {
    let tmp = sample_repo();
    git(tmp.path(), &["config", "core.repositoryformatversion", "1"]);
    git(tmp.path(), &["config", "extensions.frobnicate", "yes"]);
    match open(tmp.path()) {
        Err(git_core::Error::Git { stderr, .. }) => assert!(stderr.contains("frobnicate")),
        other => panic!("unexpected result {other:?}"),
    }
}
