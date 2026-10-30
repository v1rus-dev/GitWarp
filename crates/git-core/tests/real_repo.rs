//! End-to-end: build a small repository with the real `git`, read it back
//! and lay out its graph.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{
    probe_folder, ErrorClass, FolderProbe, Git, GitChoice, GitSource, Head, LogOptions, RefKind,
    Repo,
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
