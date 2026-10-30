//! Repository access for the client.
//!
//! Everything here runs the `git` executable. That is deliberate for the
//! first version: hooks, credential helpers, LFS, `includeIf` configs and
//! signing all keep working exactly as in the terminal. Hot read paths
//! (log, status, diff) can later move to `gix` behind the same API without
//! touching the UI.

mod error;
mod git;
mod log;
mod oid;
mod refs;

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

pub use error::{Error, ErrorClass, Result};
pub use git::{Git, GitChoice, GitSource, GitVersion, Resolution};
pub use log::{CommitCount, CommitInfo, LogKiller, LogOptions, LogStream};
pub use oid::Oid;
pub use refs::{Head, Ref, RefKind};

/// An opened repository (work tree or bare).
#[derive(Debug, Clone)]
pub struct Repo {
    git: Arc<Git>,
    /// Top of the work tree, or the git dir for bare repositories.
    root: PathBuf,
    git_dir: PathBuf,
    /// `objects/info`, where the commit-graph lives.
    objects_info: PathBuf,
}

impl Repo {
    /// Open the repository containing `path`.
    pub fn open(git: Arc<Git>, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // Checked here so a missing folder is not reported as "not a git
        // repository" by git itself.
        if !path.is_dir() {
            return Err(Error::PathNotFound {
                path: path.to_path_buf(),
            });
        }
        let out = git
            .command()
            .arg("-C")
            .arg(path)
            .args([
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-path",
                "objects/info",
                "--is-bare-repository",
                "--show-toplevel",
            ])
            .output()
            .map_err(Error::Spawn)?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let lines: Vec<&str> = stdout.lines().collect();
        let bare = lines.get(2) == Some(&"true");
        // In a bare repository `--show-toplevel` fails (exit 128) after the
        // other answers were already printed; anywhere else a failure is real
        // and git's own message ("not a git repository", unknown extension,
        // dubious ownership, ...) is what the user needs to see.
        if !bare {
            match check_output(out, "rev-parse") {
                Ok(_) => {}
                // The one failure the UI treats separately: a folder that is
                // simply not inside a working tree.
                Err(Error::Git {
                    command,
                    code,
                    stderr,
                }) => {
                    if stderr.contains("not a git repository") {
                        return Err(Error::NotARepository {
                            path: path.to_path_buf(),
                        });
                    }
                    return Err(Error::Git {
                        command,
                        code,
                        stderr,
                    });
                }
                Err(e) => return Err(e),
            }
        }
        let line = |i: usize| {
            lines
                .get(i)
                .map(PathBuf::from)
                .ok_or(Error::Parse("rev-parse output"))
        };
        let git_dir = line(0)?;
        // Asked from git so GIT_OBJECT_DIRECTORY and linked worktrees (whose
        // objects live in the common dir) are respected.
        let objects_info = line(1)?;
        let root = if bare { git_dir.clone() } else { line(3)? };
        Ok(Self {
            git,
            root,
            git_dir,
            objects_info,
        })
    }

    /// Create a repository at `path` (`git init -q`) and open it.
    pub fn init(git: Arc<Git>, path: &Path) -> Result<Self> {
        Self::init_with(git, path, None, false)
    }

    /// Create a repository at `path` and open it.
    ///
    /// `branch` names the initial branch (`git init -b`), and
    /// `initial_commit_gitignore` creates an empty `.gitignore` (only when the
    /// folder has none) and commits just that file as "Initial commit". That
    /// commit needs a `user.name`/`user.email` from the user's git
    /// configuration; when it fails the repository itself still exists and the
    /// error is returned as git reported it.
    ///
    /// A folder that is already inside a working tree is not initialized
    /// again: git would reuse that repository and the "initial commit" would
    /// land in someone else's history. [`Error::AlreadyARepository`] names the
    /// root instead. A folder that does not exist yet counts as inside the
    /// work tree that `git init` would put it in.
    pub fn init_with(
        git: Arc<Git>,
        path: &Path,
        branch: Option<&str>,
        initial_commit_gitignore: bool,
    ) -> Result<Self> {
        Self::init_with_env(git, path, branch, initial_commit_gitignore, &[])
    }

    /// [`Repo::init_with`] with extra environment for the git commands.
    ///
    /// The tests use it to point git at a throw-away global configuration
    /// without touching the process environment (which is shared by every
    /// test thread).
    #[doc(hidden)]
    pub fn init_with_env(
        git: Arc<Git>,
        path: &Path,
        branch: Option<&str>,
        initial_commit_gitignore: bool,
        env: &[(&str, &str)],
    ) -> Result<Self> {
        if let FolderProbe::Repo { root, .. } | FolderProbe::Nested { root } =
            probe_folder(git.clone(), path)?
        {
            return Err(Error::AlreadyARepository { root });
        }

        let mut cmd = git.command();
        cmd.args(["init", "-q"]);
        if let Some(branch) = branch {
            cmd.args(["-b", branch]);
        }
        // The path may start with `-`; after `--` it is always the directory.
        let out = cmd
            .args(["--"])
            .arg(path)
            .envs(env.iter().copied())
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "init")?;
        let repo = Self::open(git, path)?;

        if initial_commit_gitignore {
            create_gitignore(&repo.root)?;
            let out = repo
                .command()
                .args(["add", "--", ".gitignore"])
                .envs(env.iter().copied())
                .output()
                .map_err(Error::Spawn)?;
            check_output(out, "add")?;
            // Only the `.gitignore`: whatever else the user staged in the
            // folder (if it was no repository, nothing could have been staged,
            // but the commit stays explicit about its content) must not end up
            // in this first commit.
            let out = repo
                .command()
                .args(["commit", "-q", "-m", "Initial commit", "--", ".gitignore"])
                .envs(env.iter().copied())
                .output()
                .map_err(Error::Spawn)?;
            check_output(out, "commit")?;
        }
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// A `git` command already pointed at this repository.
    pub(crate) fn command(&self) -> Command {
        let mut cmd = self.git.command();
        cmd.arg("-C").arg(&self.root);
        cmd
    }

    /// Stream commits in topological order. Commits arrive as git produces
    /// them, so the first screen of history renders before the log finishes.
    pub fn log(&self, options: &LogOptions) -> Result<LogStream> {
        LogStream::spawn(self, options)
    }

    /// How many commits the history log will show: every branch, remote and
    /// tag, minus the stash. An empty repository counts 0.
    pub fn commit_count(&self) -> Result<u64> {
        self.commit_count_cancellable()?.wait()
    }

    /// Start the count without waiting, so the caller can kill it (the log
    /// beside it may be cancelled while this still walks a cold repository).
    pub fn commit_count_cancellable(&self) -> Result<CommitCount> {
        CommitCount::spawn(self)
    }

    /// Whether git keeps a commit-graph file for this repository.
    ///
    /// Without it `git log --topo-order` must walk the whole history before
    /// printing the first commit (on git.git: ~2 s instead of ~0.1 s), so the
    /// app should call [`Repo::write_commit_graph`] in the background when
    /// this returns false.
    pub fn has_commit_graph(&self) -> bool {
        let info = &self.objects_info;
        info.join("commit-graph").is_file() || info.join("commit-graphs").is_dir()
    }

    /// Write (or refresh) the commit-graph file. Takes a few seconds on big
    /// repositories; run it off the UI thread.
    pub fn write_commit_graph(&self) -> Result<()> {
        let out = self
            .command()
            .args([
                "commit-graph",
                "write",
                "--reachable",
                "--split",
                "--no-progress",
            ])
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "commit-graph write").map(drop)
    }

    /// All branches, remote branches and tags.
    pub fn refs(&self) -> Result<Vec<Ref>> {
        refs::list(self)
    }

    /// What HEAD points at.
    pub fn head(&self) -> Result<Head> {
        refs::head(self)
    }
}

/// What the "Open from folder" dialog found at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderProbe {
    /// The path does not exist, or is not a directory.
    Missing,
    /// The path exists but is inside no git working tree.
    Plain,
    /// The path is the root of a work tree (or a bare repository).
    Repo {
        root: PathBuf,
        /// The checked-out branch; `None` for a detached HEAD.
        branch: Option<String>,
        commits: u64,
    },
    /// The path is inside a work tree whose root is `root`.
    Nested { root: PathBuf },
}

/// Inspect `path` for the Open dialog without changing anything.
///
/// Paths that simply are not repositories (a folder outside a work tree, a
/// missing folder) are answers, not errors; only git failures that leave the
/// question open are returned as `Err`.
pub fn probe_folder(git: Arc<Git>, path: &Path) -> Result<FolderProbe> {
    if !path.is_dir() {
        // A folder that does not exist yet could still be created by `git
        // init`; when the closest folder that does exist lies in a working
        // tree, the new repository would be nested inside it. Reporting it
        // like any other folder inside a repository keeps the dialogs and
        // `init_with` on one classification.
        if path.exists() {
            return Ok(FolderProbe::Missing);
        }
        return match nearest_existing_dir(path) {
            Some(dir) => match Repo::open(git, dir) {
                Ok(repo) => Ok(FolderProbe::Nested {
                    root: repo.root().to_path_buf(),
                }),
                Err(Error::NotARepository { .. }) | Err(Error::PathNotFound { .. }) => {
                    Ok(FolderProbe::Missing)
                }
                Err(error) => Err(error),
            },
            None => Ok(FolderProbe::Missing),
        };
    }
    let repo = match Repo::open(git, path) {
        Ok(repo) => repo,
        Err(Error::PathNotFound { .. }) => return Ok(FolderProbe::Missing),
        Err(Error::NotARepository { .. }) => return Ok(FolderProbe::Plain),
        Err(error) => return Err(error),
    };
    let root = repo.root().to_path_buf();
    if !same_path(path, &root) {
        return Ok(FolderProbe::Nested { root });
    }
    // HEAD of an empty repository is an unborn branch and still has a name.
    let branch = match repo.head()? {
        Head::Branch { name, .. } => Some(name),
        Head::Detached(_) => None,
    };
    let commits = repo.commit_count()?;
    Ok(FolderProbe::Repo {
        root,
        branch,
        commits,
    })
}

/// Writes an empty `.gitignore` unless the folder already has one, in which
/// case the user's file is left exactly as it is (and committed as such).
fn create_gitignore(root: &Path) -> Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(".gitignore"))
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// `path` or its closest ancestor that still is a directory.
fn nearest_existing_dir(path: &Path) -> Option<&Path> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.is_dir() {
            return Some(dir);
        }
        current = dir.parent();
    }
    None
}

/// Whether two paths name the same directory, following symlinks where they
/// exist. `git rev-parse --show-toplevel` may print a symlinked path, so the
/// comparison cannot be textual.
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `out` if git succeeded, otherwise git's own message as an error.
pub(crate) fn check_output(out: Output, command: &'static str) -> Result<Output> {
    if out.status.success() {
        Ok(out)
    } else {
        Err(Error::Git {
            command,
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
