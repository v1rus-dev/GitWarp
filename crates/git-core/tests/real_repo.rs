//! End-to-end: build a small repository with the real `git`, read it back
//! and lay out its graph.

use std::path::Path;
use std::process::Command;

use git_core::{Head, LogOptions, RefKind, Repo};
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
    let repo = Repo::open(tmp.path()).unwrap();
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
    let repo = Repo::open(tmp.path()).unwrap();
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
    let repo = Repo::open(tmp.path()).unwrap();
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
    let repo = Repo::open(tmp.path()).unwrap();
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
    let repo = Repo::open(tmp.path()).unwrap();
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
fn rejects_non_repository() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(Repo::open(tmp.path()).is_err());
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
    let repo = Repo::open(&bare).unwrap();
    assert_eq!(repo.root(), repo.git_dir());
    assert_eq!(repo.log(&LogOptions::default()).unwrap().count(), 6);
}

#[test]
fn writes_commit_graph() {
    let tmp = sample_repo();
    let repo = Repo::open(tmp.path()).unwrap();
    // Fresh repos may or may not have one depending on git version/config.
    repo.write_commit_graph().unwrap();
    assert!(repo.has_commit_graph());
}
