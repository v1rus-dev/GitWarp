//! End-to-end: build a small repository with the real `git`, read it back
//! and lay out its graph.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{
    probe_folder, ErrorClass, FolderProbe, Git, GitChoice, GitSource, Head, LogOptions, RefKind,
    Repo, SubmoduleState, Worktree,
};
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
fn reads_ahead_and_behind_against_the_upstream() {
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q"]);
    commit(origin.path(), "A");

    let work = tempfile::tempdir().unwrap();
    let checkout = work.path().join("work");
    git(
        work.path(),
        &[
            "clone",
            "-q",
            origin.path().to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    commit(&checkout, "ahead");
    commit(origin.path(), "behind");
    git(&checkout, &["fetch", "-q", "origin"]);

    let repo = open(&checkout).unwrap();
    let main = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == RefKind::LocalBranch && r.name == "main")
        .unwrap();
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind, main.upstream_gone), (1, 1, false));
}

#[test]
fn a_deleted_upstream_is_marked_gone() {
    let tmp = sample_repo();
    git(
        tmp.path(),
        &["remote", "add", "origin", "https://example.invalid/x.git"],
    );
    git(tmp.path(), &["checkout", "-q", "-b", "orphaned"]);
    git(tmp.path(), &["config", "branch.orphaned.remote", "origin"]);
    git(
        tmp.path(),
        &[
            "config",
            "branch.orphaned.merge",
            "refs/heads/does-not-exist",
        ],
    );
    let repo = open(tmp.path()).unwrap();
    let orphaned = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == "orphaned")
        .unwrap();
    assert!(orphaned.upstream_gone);
    assert_eq!((orphaned.ahead, orphaned.behind), (0, 0));
}

#[test]
fn lists_remotes_in_gits_order() {
    let tmp = sample_repo();
    // `git remote` sorts by name; the list must keep that order.
    git(
        tmp.path(),
        &["remote", "add", "zeta", "https://example.invalid/z.git"],
    );
    git(
        tmp.path(),
        &["remote", "add", "alpha", "https://example.invalid/a.git"],
    );
    assert_eq!(
        open(tmp.path()).unwrap().remotes().unwrap(),
        ["alpha", "zeta"]
    );
}

#[test]
fn lists_stashes_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    std::fs::write(tmp.path().join("work.txt"), b"one").unwrap();
    git(tmp.path(), &["add", "work.txt"]);
    commit(tmp.path(), "A");

    let repo = open(tmp.path()).unwrap();
    assert!(repo.stashes().unwrap().is_empty());

    std::fs::write(tmp.path().join("work.txt"), b"two").unwrap();
    git(tmp.path(), &["stash", "push", "-q", "-m", "first"]);
    std::fs::write(tmp.path().join("work.txt"), b"three").unwrap();
    git(tmp.path(), &["stash", "push", "-q", "-m", "second"]);

    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes.len(), 2);
    assert_eq!(stashes[0].index, 0);
    assert_eq!(stashes[1].index, 1);
    assert_eq!(stashes[0].message, "On main: second");
    assert_eq!(stashes[1].message, "On main: first");
    assert_ne!(stashes[0].oid, stashes[1].oid);
}

fn worktree<'a>(list: &'a [Worktree], path: &Path) -> &'a Worktree {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    list.iter()
        .find(|w| w.path == path)
        .unwrap_or_else(|| panic!("no worktree at {}", path.display()))
}

#[test]
fn lists_worktrees_and_marks_the_current_one() {
    let tmp = tempfile::tempdir().unwrap();
    let main_dir = tmp.path().join("main");
    std::fs::create_dir(&main_dir).unwrap();
    git(&main_dir, &["init", "-q"]);
    commit(&main_dir, "A");

    let linked = tmp.path().join("linked");
    git(
        &main_dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-branch",
            linked.to_str().unwrap(),
        ],
    );
    let detached = tmp.path().join("detached");
    git(
        &main_dir,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
            "main",
        ],
    );
    git(
        &main_dir,
        &[
            "worktree",
            "lock",
            "--reason",
            "testing",
            detached.to_str().unwrap(),
        ],
    );
    let gone = tmp.path().join("gone");
    git(
        &main_dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-gone",
            gone.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&gone).unwrap();

    let repo = open(&main_dir).unwrap();
    let list = repo.worktrees().unwrap();
    assert_eq!(list.len(), 4);

    let main = worktree(&list, &main_dir);
    assert!(main.current && !main.detached && !main.bare);
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert!(main.head.is_some());
    assert!(main.locked.is_none() && !main.prunable);

    let branch = worktree(&list, &linked);
    assert!(!branch.current);
    assert_eq!(branch.branch.as_deref(), Some("wt-branch"));
    assert!(!branch.detached);

    let det = worktree(&list, &detached);
    assert!(det.detached && det.branch.is_none());
    assert_eq!(det.locked.as_deref(), Some("testing"));
    assert!(!det.current);

    let pruned = worktree(&list, &gone);
    assert!(pruned.prunable && !pruned.current);

    // Opened from the added worktree, that one is current.
    let linked_repo = open(&linked).unwrap();
    let list = linked_repo.worktrees().unwrap();
    assert_eq!(list.iter().filter(|w| w.current).count(), 1);
    assert!(worktree(&list, &linked).current);
    assert!(!worktree(&list, &main_dir).current);
}

#[test]
fn adds_removes_locks_and_prunes_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let main_dir = tmp.path().join("main");
    std::fs::create_dir(&main_dir).unwrap();
    git(&main_dir, &["init", "-q"]);
    commit(&main_dir, "A");
    git(&main_dir, &["branch", "other"]);
    let repo = open(&main_dir).unwrap();
    let oid = repo.head_commit().unwrap().unwrap().id;

    // An existing branch in a new folder.
    let linked = tmp.path().join("linked");
    repo.add_worktree(&linked, "other").unwrap();
    let list = repo.worktrees().unwrap();
    let added = worktree(&list, &linked);
    assert_eq!(added.branch.as_deref(), Some("other"));
    assert!(!added.main && !added.current);
    std::fs::write(linked.join("local.txt"), b"dirty").unwrap();

    // A new branch from a commit.
    let fresh = tmp.path().join("fresh");
    repo.add_worktree_for_new_branch(&fresh, "wt-new", oid)
        .unwrap();
    let list = repo.worktrees().unwrap();
    let new = worktree(&list, &fresh);
    assert_eq!(new.branch.as_deref(), Some("wt-new"));
    assert_eq!(new.head, Some(oid));
    assert!(repo
        .refs()
        .unwrap()
        .iter()
        .any(|r| r.kind == RefKind::LocalBranch && r.name == "wt-new"));

    // Locking keeps a reason; unlocking clears it.
    repo.lock_worktree(&linked, Some("review in progress"))
        .unwrap();
    let list = repo.worktrees().unwrap();
    assert_eq!(
        worktree(&list, &linked).locked.as_deref(),
        Some("review in progress")
    );
    repo.unlock_worktree(&linked).unwrap();
    let list = repo.worktrees().unwrap();
    assert!(worktree(&list, &linked).locked.is_none());

    // A dirty worktree needs force; the branch stays in the repository.
    assert!(repo.remove_worktree(&linked, false).is_err());
    repo.remove_worktree(&linked, true).unwrap();
    assert!(!linked.exists());
    let list = repo.worktrees().unwrap();
    assert!(list.iter().all(|w| w.path != linked));
    assert!(repo
        .refs()
        .unwrap()
        .iter()
        .any(|r| r.kind == RefKind::LocalBranch && r.name == "other"));

    // A worktree whose folder is gone is pruned without touching files.
    std::fs::remove_dir_all(&fresh).unwrap();
    let list = repo.worktrees().unwrap();
    assert!(worktree(&list, &fresh).prunable);
    repo.prune_worktrees().unwrap();
    assert!(repo.worktrees().unwrap().iter().all(|w| w.path != fresh));

    // The main worktree cannot be removed.
    assert!(repo.remove_worktree(&main_dir, true).is_err());
    assert!(main_dir.exists());
}

#[test]
fn lists_submodules_and_their_state() {
    let lib = tempfile::tempdir().unwrap();
    git(lib.path(), &["init", "-q"]);
    commit(lib.path(), "L1");

    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("parent");
    std::fs::create_dir(&d).unwrap();
    git(&d, &["init", "-q"]);
    commit(&d, "P1");
    // Local-path submodules need the file protocol allowed explicitly.
    git(
        &d,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            lib.path().to_str().unwrap(),
            "vendor/lib",
        ],
    );
    commit(&d, "add submodule");

    let repo = open(&d).unwrap();
    let subs = repo.submodules().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].path, "vendor/lib");
    assert_eq!(subs[0].state, SubmoduleState::Clean);
    let clean_oid = subs[0].oid.expect("the checked-out commit");

    // A new commit inside the submodule is "modified".
    commit(&d.join("vendor/lib"), "L2");
    let subs = repo.submodules().unwrap();
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert_ne!(subs[0].oid, Some(clean_oid));

    // A plain clone leaves the submodule uninitialized.
    let clone = tmp.path().join("clone");
    git(
        tmp.path(),
        &["clone", "-q", d.to_str().unwrap(), clone.to_str().unwrap()],
    );
    let cloned = open(&clone).unwrap();
    assert_eq!(
        cloned.submodules().unwrap()[0].state,
        SubmoduleState::Uninitialized
    );
    git(
        &clone,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "-q",
            "--init",
        ],
    );
    assert_eq!(cloned.submodules().unwrap()[0].state, SubmoduleState::Clean);

    // A bare repository has no work tree to report submodules from.
    let bare = tmp.path().join("bare.git");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            d.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    assert!(open(&bare).unwrap().submodules().unwrap().is_empty());
}

#[test]
fn a_repository_without_a_gitmodules_has_no_submodules() {
    let tmp = sample_repo();
    assert!(open(tmp.path()).unwrap().submodules().unwrap().is_empty());
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

/// A throw-away global git configuration for `Repo::init_with_env`, so the
/// test never mutates the process environment (which every test thread
/// shares). `init_with` deliberately uses the user's own configuration, so the
/// test has to provide one instead of passing `-c`.
struct TestIdentity {
    config: std::path::PathBuf,
}

impl TestIdentity {
    fn install(dir: &Path) -> Self {
        let config = dir.join("gitconfig");
        std::fs::write(
            &config,
            "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        Self { config }
    }

    /// The environment that points git at the throw-away configuration.
    fn env(&self) -> [(&str, &str); 2] {
        [
            ("GIT_CONFIG_GLOBAL", self.config.to_str().unwrap()),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ]
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
    let identity = TestIdentity::install(tmp.path());
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init_with_env(git, &dir, Some("main"), true, &identity.env()).unwrap();

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

/// The user's `.gitignore` is theirs: it keeps its content and is what the
/// initial commit adds.
#[test]
fn init_with_keeps_an_existing_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("own-gitignore");
    std::fs::create_dir(&dir).unwrap();
    let content = b"target/\n*.log\n# keep this\n";
    std::fs::write(dir.join(".gitignore"), content).unwrap();
    let identity = TestIdentity::install(tmp.path());
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    let repo = Repo::init_with_env(git, &dir, Some("main"), true, &identity.env()).unwrap();

    assert_eq!(std::fs::read(dir.join(".gitignore")).unwrap(), content);
    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["show", "--stat", "--format=", "HEAD"])
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains(".gitignore"), "{shown}");
    assert_eq!(repo.commit_count().unwrap(), 1);
}

/// Only `.gitignore` may enter the initial commit; files in the folder stay
/// untracked.
#[test]
fn init_with_commits_only_the_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("extra-files");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), b"draft\n").unwrap();
    let identity = TestIdentity::install(tmp.path());
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    Repo::init_with_env(git, &dir, Some("main"), true, &identity.env()).unwrap();

    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["ls-tree", "--name-only", "HEAD"])
        .output()
        .unwrap();
    let names = String::from_utf8_lossy(&out.stdout);
    assert_eq!(names.trim(), ".gitignore");
    let status = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).contains("notes.txt"));
}

#[test]
fn init_with_refuses_an_existing_repository() {
    let tmp = sample_repo();
    let before = open(tmp.path()).unwrap().head().unwrap();
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    match Repo::init(git, tmp.path()) {
        Err(git_core::Error::AlreadyARepository { root }) => {
            assert_eq!(root, tmp.path().canonicalize().unwrap());
        }
        other => panic!("unexpected result {other:?}"),
    }
    assert_eq!(open(tmp.path()).unwrap().head().unwrap(), before);
}

#[test]
fn init_with_refuses_a_subfolder_of_a_repository() {
    let tmp = sample_repo();
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    match Repo::init(git, &sub) {
        Err(git_core::Error::AlreadyARepository { root }) => {
            assert_eq!(root, tmp.path().canonicalize().unwrap());
        }
        other => panic!("unexpected result {other:?}"),
    }
    assert!(!sub.join(".git").exists());
}

#[test]
fn init_with_refuses_a_missing_subfolder_of_a_repository() {
    let tmp = sample_repo();
    // The folder does not exist, so a plain probe sees `Missing`; git init
    // would nevertheless create it inside the sample repository.
    let missing = tmp.path().join("new").join("project");
    let git = Arc::new(Git::resolve(GitChoice::System).unwrap().git);
    match Repo::init(git, &missing) {
        Err(git_core::Error::AlreadyARepository { root }) => {
            assert_eq!(root, tmp.path().canonicalize().unwrap());
        }
        other => panic!("unexpected result {other:?}"),
    }
    assert!(
        !tmp.path().join("new").exists(),
        "git init created a nested repository"
    );
}

#[test]
fn probe_reports_a_missing_folder_inside_a_repository_as_nested() {
    let tmp = sample_repo();
    assert_eq!(
        probe(&tmp.path().join("new").join("project")).unwrap(),
        FolderProbe::Nested {
            root: tmp.path().canonicalize().unwrap(),
        }
    );
}

#[test]
fn probe_reports_a_missing_folder_outside_any_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert_eq!(
        probe(&plain.join("new").join("project")).unwrap(),
        FolderProbe::Missing
    );
}

#[cfg(target_os = "linux")]
#[test]
fn killing_a_log_stops_git() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let mut stream = repo.log(&LogOptions::default()).unwrap();
    let killer = stream.killer();
    let pid = killer.pid().expect("the log has a child process");
    killer.kill();
    assert!(
        !process_alive(pid),
        "git log (pid {pid}) is still running after the kill"
    );
    // The stream ends after the kill: either with no commits or an error.
    let rest: Vec<_> = stream.by_ref().collect();
    assert!(rest.iter().all(|item| item.is_err()) || rest.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn killing_a_commit_count_stops_git() {
    let tmp = sample_repo();
    let repo = open(tmp.path()).unwrap();
    let count = repo.commit_count_cancellable().unwrap();
    let pid = count.pid().expect("the count has a child process");
    count.killer().kill();
    assert!(
        !process_alive(pid),
        "git rev-list (pid {pid}) is still running after the kill"
    );
    // The killed child was reaped by the killer: no exit status is left.
    assert!(count.wait().is_err(), "a killed count must not report one");
}

/// Whether `pid` still has a process on Linux; the killed child is reaped, so
/// its `/proc` entry goes away with it.
#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
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

fn probe(path: &Path) -> git_core::Result<FolderProbe> {
    probe_folder(Arc::new(Git::resolve(GitChoice::System)?.git), path)
}

#[test]
fn probe_reports_a_missing_folder_or_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        probe(&tmp.path().join("does-not-exist")).unwrap(),
        FolderProbe::Missing
    );
    // The dialog's field may point at a file; that is not an openable folder.
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, b"hello").unwrap();
    assert_eq!(probe(&file).unwrap(), FolderProbe::Missing);
}

#[test]
fn probe_reports_a_plain_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plain");
    std::fs::create_dir(&dir).unwrap();
    assert_eq!(probe(&dir).unwrap(), FolderProbe::Plain);
}

#[test]
fn probe_reports_a_repository_root_with_branch_and_count() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    commit(tmp.path(), "A");
    commit(tmp.path(), "B");
    match probe(tmp.path()).unwrap() {
        FolderProbe::Repo {
            root,
            branch,
            commits,
        } => {
            assert_eq!(root, tmp.path().canonicalize().unwrap());
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(commits, 2);
        }
        other => panic!("unexpected probe {other:?}"),
    }
}

#[test]
fn probe_reports_an_empty_repository_root_with_its_unborn_branch() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q"]);
    match probe(tmp.path()).unwrap() {
        FolderProbe::Repo {
            branch, commits, ..
        } => {
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(commits, 0);
        }
        other => panic!("unexpected probe {other:?}"),
    }
}

#[test]
fn probe_reports_a_detached_head_without_a_branch() {
    let tmp = sample_repo();
    git(tmp.path(), &["checkout", "-q", "--detach"]);
    match probe(tmp.path()).unwrap() {
        FolderProbe::Repo { branch, .. } => assert_eq!(branch, None),
        other => panic!("unexpected probe {other:?}"),
    }
}

#[test]
fn probe_reports_a_subdirectory_as_nested_under_the_root() {
    let tmp = sample_repo();
    let sub = tmp.path().join("crates").join("app");
    std::fs::create_dir_all(&sub).unwrap();
    match probe(&sub).unwrap() {
        FolderProbe::Nested { root } => {
            assert_eq!(root, tmp.path().canonicalize().unwrap());
        }
        other => panic!("unexpected probe {other:?}"),
    }
}

#[test]
fn probe_reports_a_bare_repository_as_a_root() {
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
    match probe(&bare).unwrap() {
        FolderProbe::Repo {
            root,
            branch,
            commits,
        } => {
            assert_eq!(root, bare.canonicalize().unwrap());
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(commits, 6);
        }
        other => panic!("unexpected probe {other:?}"),
    }
}

/// Writes `content` to `file` under `dir`, creating parent directories.
fn write(dir: &Path, file: &str, content: impl AsRef<[u8]>) {
    let path = dir.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// `git add -A` then a commit with `message`.
fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn head_oid(repo: &Repo) -> git_core::Oid {
    match repo.head().unwrap() {
        Head::Branch { target, .. } => target.unwrap(),
        Head::Detached(target) => target,
    }
}

#[test]
fn reads_commit_details_with_body_and_committer() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\n");
    git(d, &["add", "a.txt"]);
    let status = Command::new("git")
        .arg("-C")
        .arg(d)
        .args([
            "-c",
            "user.name=Committer",
            "-c",
            "user.email=committer@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "Add a file\n\nWhy it exists.\n\nCloses #1",
        ])
        .env("GIT_AUTHOR_NAME", "Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2026-10-29T14:08:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-29T15:00:00Z")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git runs");
    assert!(status.success());

    let repo = open(d).unwrap();
    let details = repo.commit_details(head_oid(&repo)).unwrap();
    assert_eq!(details.author_name, "Author");
    assert_eq!(details.author_email, "author@example.com");
    assert_eq!(details.committer_name, "Committer");
    assert_eq!(details.committer_email, "committer@example.com");
    assert_eq!(details.summary, "Add a file");
    assert_eq!(details.body, "Why it exists.\n\nCloses #1");
    assert_eq!(details.committer_time - details.author_time, 52 * 60);
    assert!(details.parents.is_empty(), "the root commit has no parents");
}

#[test]
fn lists_add_modify_delete_rename_and_root_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "src/a.txt", "one\ntwo\nthree\n");
    commit_all(d, "root");
    let repo = open(d).unwrap();

    let root = head_oid(&repo);
    let files = repo.commit_changes(root).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, git_core::ChangeKind::Added);
    assert_eq!(files[0].path, "src/a.txt");
    assert_eq!((files[0].added, files[0].deleted), (3, 0));

    write(d, "src/a.txt", "one\nTWO\nthree\n");
    write(d, "src/b.txt", "new\n");
    commit_all(d, "modify and add");
    let commit = head_oid(&repo);
    let files = repo.commit_changes(commit).unwrap();
    assert_eq!(files.len(), 2);
    let a = files.iter().find(|f| f.path == "src/a.txt").unwrap();
    assert_eq!(a.kind, git_core::ChangeKind::Modified);
    assert_eq!((a.added, a.deleted), (1, 1));
    let b = files.iter().find(|f| f.path == "src/b.txt").unwrap();
    assert_eq!(b.kind, git_core::ChangeKind::Added);

    git(d, &["mv", "src/a.txt", "src/c.txt"]);
    write(d, "src/c.txt", "one\nTWO\nthree\nfour\n");
    commit_all(d, "rename");
    let commit = head_oid(&repo);
    let files = repo.commit_changes(commit).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, git_core::ChangeKind::Renamed);
    assert_eq!(files[0].path, "src/c.txt");
    assert_eq!(files[0].old_path.as_deref(), Some("src/a.txt"));
    let diff = repo
        .file_diff(commit, "src/c.txt", Some("src/a.txt"))
        .unwrap();
    assert_eq!(diff.old_path.as_deref(), Some("src/a.txt"));
    assert_eq!(diff.hunks.len(), 1);

    std::fs::remove_file(d.join("src/b.txt")).unwrap();
    commit_all(d, "delete");
    let commit = head_oid(&repo);
    let files = repo.commit_changes(commit).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, git_core::ChangeKind::Deleted);
    let diff = repo.file_diff(commit, "src/b.txt", None).unwrap();
    assert!(diff.hunks[0]
        .lines
        .iter()
        .all(|l| l.kind == git_core::LineKind::Removed));
    assert!(diff.hunks[0].lines.iter().all(|l| l.new_line.is_none()));
}

#[test]
fn reads_a_merge_against_its_first_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "base.txt", "base\n");
    commit_all(d, "base");
    git(d, &["checkout", "-q", "-b", "feature"]);
    write(d, "feature.txt", "from the feature branch\n");
    commit_all(d, "feature work");
    git(d, &["checkout", "-q", "main"]);
    write(d, "main.txt", "on main\n");
    commit_all(d, "main work");
    git(
        d,
        &[
            "merge",
            "-q",
            "--no-ff",
            "--no-edit",
            "-m",
            "merge",
            "feature",
        ],
    );

    let repo = open(d).unwrap();
    let merge = head_oid(&repo);
    let details = repo.commit_details(merge).unwrap();
    assert_eq!(details.parents.len(), 2);
    let files = repo.commit_changes(merge).unwrap();
    let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["feature.txt"]);
    let diff = repo.file_diff(merge, "feature.txt", None).unwrap();
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.hunks[0].lines[0].kind, git_core::LineKind::Added);
}

#[test]
fn reports_binary_files_without_hunks() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "img.bin", [0u8, 1, 2, 0, 255]);
    commit_all(d, "binary");
    let repo = open(d).unwrap();
    let oid = head_oid(&repo);

    let files = repo.commit_changes(oid).unwrap();
    assert!(files[0].binary);
    let diff = repo.file_diff(oid, "img.bin", None).unwrap();
    assert!(diff.binary);
    assert!(diff.hunks.is_empty());
    let texts = repo.file_texts(oid, "img.bin", None).unwrap();
    assert!(texts.binary);
    assert!(texts.old.is_none() && texts.new.is_none());
}

#[test]
fn reads_old_and_new_text_for_highlighting() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.rs", "fn main() {}\n");
    commit_all(d, "one");
    write(d, "a.rs", "fn main() {\n    run();\n}\n");
    commit_all(d, "two");
    let repo = open(d).unwrap();

    let texts = repo.file_texts(head_oid(&repo), "a.rs", None).unwrap();
    assert_eq!(texts.old.as_deref(), Some("fn main() {}\n"));
    assert_eq!(texts.new.as_deref(), Some("fn main() {\n    run();\n}\n"));
    assert!(!texts.binary);

    // An added file has no old side; a deleted file has no new side.
    write(d, "b.rs", "x\n");
    commit_all(d, "add b");
    let texts = repo.file_texts(head_oid(&repo), "b.rs", None).unwrap();
    assert_eq!(texts.old, None);
    assert_eq!(texts.new.as_deref(), Some("x\n"));
}

#[test]
fn keeps_the_no_newline_marker_and_marks_a_huge_diff_too_large() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one");
    commit_all(d, "no newline");
    write(d, "a.txt", "one\ntwo");
    commit_all(d, "still no newline");
    let repo = open(d).unwrap();
    let diff = repo.file_diff(head_oid(&repo), "a.txt", None).unwrap();
    assert!(diff.hunks[0].lines.last().unwrap().no_newline);

    let mut huge = String::new();
    for i in 0..21_000 {
        huge.push_str(&format!("line {i}\n"));
    }
    write(d, "huge.txt", huge);
    commit_all(d, "huge");
    let err = repo
        .file_diff(head_oid(&repo), "huge.txt", None)
        .unwrap_err();
    assert!(matches!(err, git_core::Error::TooLarge { .. }), "{err}");
}

#[test]
fn reads_the_working_status_and_moves_files_between_the_sides() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\n");
    write(d, "b.txt", "gone\n");
    commit_all(d, "one");
    write(d, "a.txt", "one\nchanged\n");
    write(d, "c.txt", "new\nfile\n");
    std::fs::remove_file(d.join("b.txt")).unwrap();

    let repo = open(d).unwrap();
    let status = repo.working_status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.head.as_ref().unwrap().summary, "one");
    assert_eq!(status.unstaged_count(), 3);
    assert_eq!(status.staged_count(), 0);
    let by_path = |path: &str| {
        status
            .unstaged
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} in {status:?}"))
            .clone()
    };
    let a = by_path("a.txt");
    assert_eq!(
        (a.kind, a.added, a.deleted),
        (git_core::ChangeKind::Modified, 1, 0)
    );
    let b = by_path("b.txt");
    assert_eq!((b.kind, b.deleted), (git_core::ChangeKind::Deleted, 1));
    let c = by_path("c.txt");
    assert_eq!(
        (c.kind, c.added, c.untracked),
        (git_core::ChangeKind::Added, 2, true)
    );

    repo.stage(&["a.txt".to_string()]).unwrap();
    let status = repo.working_status().unwrap();
    assert_eq!(status.staged.len(), 1);
    assert_eq!(status.staged[0].path, "a.txt");
    assert_eq!(status.unstaged_count(), 2);

    repo.unstage(&["a.txt".to_string()]).unwrap();
    let status = repo.working_status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged_count(), 3);
}

#[test]
fn discards_tracked_and_untracked_files() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\n");
    commit_all(d, "one");
    write(d, "a.txt", "changed\n");
    write(d, "new.txt", "new\n");

    let repo = open(d).unwrap();
    repo.discard(&["a.txt".to_string()], &["new.txt".to_string()])
        .unwrap();
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "one\n");
    assert!(!d.join("new.txt").exists());
    assert_eq!(repo.working_status().unwrap().unstaged_count(), 0);
}

#[test]
fn commits_and_amends_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    git(d, &["config", "user.name", "Test"]);
    git(d, &["config", "user.email", "test@example.com"]);
    write(d, "a.txt", "one\n");

    let repo = open(d).unwrap();
    repo.stage(&["a.txt".to_string()]).unwrap();
    let first = repo.commit("first\n\nWhy.\n", false).unwrap();
    assert_eq!(first, head_oid(&repo));
    let details = repo.commit_details(first).unwrap();
    assert_eq!(details.summary, "first");
    assert_eq!(details.body, "Why.");

    write(d, "a.txt", "two\n");
    repo.stage(&["a.txt".to_string()]).unwrap();
    let amended = repo.commit("second", true).unwrap();
    assert_eq!(amended, head_oid(&repo));
    assert_ne!(amended, first);
    assert!(repo.commit_details(amended).unwrap().parents.is_empty());
    let subjects: Vec<_> = repo
        .log(&LogOptions::default())
        .unwrap()
        .collect::<git_core::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .map(|commit| commit.summary)
        .collect();
    assert_eq!(subjects, ["second"]);
}

#[test]
fn stages_and_unstages_one_hunk_of_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    let mut original = String::new();
    for line in 1..=20 {
        original.push_str(&format!("line {line}\n"));
    }
    write(d, "a.txt", &original);
    commit_all(d, "one");
    let changed = original
        .replace("line 2\n", "line two\n")
        .replace("line 18\n", "line eighteen\n");
    write(d, "a.txt", &changed);

    let repo = open(d).unwrap();
    let status = repo.working_status().unwrap();
    let file = status.unstaged[0].clone();
    let diff = repo.unstaged_diff("a.txt", None).unwrap();
    assert_eq!(diff.hunks.len(), 2);

    repo.stage_selection(&file, &diff, &[git_core::HunkPick::whole(1)])
        .unwrap();
    let cached = run_git(d, &["diff", "--cached", "-U0"]);
    assert!(cached.contains("+line eighteen"), "{cached}");
    assert!(!cached.contains("+line two"), "{cached}");
    let status = repo.working_status().unwrap();
    assert_eq!(status.staged[0].path, "a.txt");
    assert_eq!(status.unstaged[0].path, "a.txt");

    let staged = status.staged[0].clone();
    let index_diff = repo.staged_diff("a.txt", None).unwrap();
    assert_eq!(index_diff.hunks.len(), 1);
    repo.unstage_selection(&staged, &index_diff, &[git_core::HunkPick::whole(0)])
        .unwrap();
    let status = repo.working_status().unwrap();
    assert!(status.staged.is_empty());
    let cached = run_git(d, &["diff", "--cached", "-U0"]);
    assert!(cached.is_empty(), "{cached}");
}

#[test]
fn stages_selected_lines_of_a_hunk() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\ntwo\nthree\n");
    commit_all(d, "one");
    write(d, "a.txt", "one\nTWO\nthree\nfour\n");

    let repo = open(d).unwrap();
    let status = repo.working_status().unwrap();
    let file = status.unstaged[0].clone();
    let diff = repo.unstaged_diff("a.txt", None).unwrap();
    let hunk = &diff.hunks[0];
    // Pick the removed "two" and the added "TWO", but not "four".
    let picks: Vec<usize> = hunk
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            matches!(line.kind, git_core::LineKind::Removed)
                || (matches!(line.kind, git_core::LineKind::Added) && line.text == "TWO")
        })
        .map(|(index, _)| index)
        .collect();
    repo.stage_selection(&file, &diff, &[git_core::HunkPick::lines(0, picks)])
        .unwrap();
    let cached = run_git(d, &["diff", "--cached", "-U0"]);
    assert!(cached.contains("-two"), "{cached}");
    assert!(cached.contains("+TWO"), "{cached}");
    assert!(!cached.contains("+four"), "{cached}");
    let remaining = run_git(d, &["diff", "-U0"]);
    assert!(remaining.contains("+four"), "{remaining}");
}

/// Runs git in `dir` and returns its stdout, failing on a non-zero exit.
fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git is installed");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn works_in_an_unborn_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    git(d, &["config", "user.name", "Test"]);
    git(d, &["config", "user.email", "test@example.com"]);
    write(d, "a.txt", "one\n");

    let repo = open(d).unwrap();
    let status = repo.working_status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.head.is_none());
    assert_eq!(status.unstaged[0].path, "a.txt");
    assert!(status.unstaged[0].untracked);

    repo.stage(&["a.txt".to_string()]).unwrap();
    assert_eq!(repo.working_status().unwrap().staged.len(), 1);
    repo.unstage(&["a.txt".to_string()]).unwrap();
    let status = repo.working_status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.len(), 1);

    repo.stage(&["a.txt".to_string()]).unwrap();
    let oid = repo.commit("first", false).unwrap();
    assert_eq!(oid, head_oid(&repo));
    let status = repo.working_status().unwrap();
    assert_eq!(status.head.as_ref().unwrap().summary, "first");
    assert_eq!(status.unstaged_count(), 0);
}

#[test]
fn an_untracked_file_reads_as_additions_and_can_be_partly_staged() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "new.txt", "one\ntwo\nthree\n");

    let repo = open(d).unwrap();
    let file = repo.working_status().unwrap().unstaged[0].clone();
    assert!(file.untracked);
    let diff = repo.untracked_diff("new.txt").unwrap();
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.hunks[0].lines.len(), 3);
    assert!(diff.hunks[0]
        .lines
        .iter()
        .all(|line| line.kind == git_core::LineKind::Added));

    // Stage the first and last line only: the index gets a two-line file and
    // the still-unstaged middle line shows up as a modification.
    repo.stage_selection(&file, &diff, &[git_core::HunkPick::lines(0, vec![0, 2])])
        .unwrap();
    let cached = run_git(d, &["show", ":new.txt"]);
    assert_eq!(cached, "one\nthree\n");
    let status = repo.working_status().unwrap();
    assert_eq!(status.staged.len(), 1);
    assert_eq!(status.unstaged.len(), 1);
}

#[test]
fn stages_a_hunk_of_a_file_whose_name_has_a_space() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    let mut original = String::new();
    for line in 1..=20 {
        original.push_str(&format!("line {line}\n"));
    }
    write(d, "my file.txt", &original);
    commit_all(d, "one");
    let changed = original
        .replace("line 2\n", "line two\n")
        .replace("line 18\n", "line eighteen\n");
    write(d, "my file.txt", &changed);

    let repo = open(d).unwrap();
    let file = repo.working_status().unwrap().unstaged[0].clone();
    let diff = repo.unstaged_diff("my file.txt", None).unwrap();
    assert_eq!(diff.hunks.len(), 2);
    repo.stage_selection(&file, &diff, &[git_core::HunkPick::whole(1)])
        .unwrap();
    let cached = run_git(d, &["diff", "--cached", "-U0"]);
    assert!(cached.contains("+line eighteen"), "{cached}");
    assert!(!cached.contains("+line two"), "{cached}");
}

#[test]
fn unstages_selected_lines_of_a_staged_hunk() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\ntwo\nthree\n");
    commit_all(d, "one");
    write(d, "a.txt", "one\nTWO\nthree\nfour\n");
    git(d, &["add", "a.txt"]);

    let repo = open(d).unwrap();
    let staged = repo.working_status().unwrap().staged[0].clone();
    let diff = repo.staged_diff("a.txt", None).unwrap();
    let hunk = &diff.hunks[0];
    let picks: Vec<usize> = hunk
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            matches!(line.kind, git_core::LineKind::Removed)
                || (matches!(line.kind, git_core::LineKind::Added) && line.text == "TWO")
        })
        .map(|(index, _)| index)
        .collect();
    repo.unstage_selection(&staged, &diff, &[git_core::HunkPick::lines(0, picks)])
        .unwrap();

    let cached = run_git(d, &["diff", "--cached", "-U0"]);
    assert!(cached.contains("+four"), "{cached}");
    assert!(!cached.contains("+TWO"), "{cached}");
    assert!(!cached.contains("-two"), "{cached}");
    // The work tree keeps both changes.
    let worktree = std::fs::read_to_string(d.join("a.txt")).unwrap();
    assert_eq!(worktree, "one\nTWO\nthree\nfour\n");
}

#[test]
fn reads_a_staged_rename_against_its_old_path() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    git(d, &["init", "-q"]);
    write(d, "a.txt", "one\ntwo\n");
    commit_all(d, "one");
    git(d, &["mv", "a.txt", "b.txt"]);
    write(d, "b.txt", "one\nTWO\n");
    git(d, &["add", "b.txt"]);

    let repo = open(d).unwrap();
    let status = repo.working_status().unwrap();
    let file = &status.staged[0];
    assert_eq!(file.kind, git_core::ChangeKind::Renamed);
    assert_eq!(file.old_path.as_deref(), Some("a.txt"));
    let diff = repo
        .staged_diff(&file.path, file.old_path.as_deref())
        .unwrap();
    assert_eq!(diff.old_path.as_deref(), Some("a.txt"));
    let kinds: Vec<_> = diff.hunks[0].lines.iter().map(|line| line.kind).collect();
    assert!(kinds.contains(&git_core::LineKind::Removed), "{kinds:?}");
    assert!(kinds.contains(&git_core::LineKind::Added), "{kinds:?}");
}
