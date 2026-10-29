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

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

pub use error::{Error, Result};
pub use git::{Git, GitChoice, GitSource, GitVersion, Resolution};
pub use log::{CommitInfo, LogOptions, LogStream};
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
        let out = git
            .command()
            .arg("-C")
            .arg(path.as_ref())
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
            check_output(out, "rev-parse")?;
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

/// `out` if git succeeded, otherwise git's own message as an error.
pub(crate) fn check_output(out: Output, command: &'static str) -> Result<Output> {
    if out.status.success() {
        Ok(out)
    } else {
        Err(Error::Git {
            command,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
