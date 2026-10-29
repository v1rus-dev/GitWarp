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
use std::process::Command;
use std::sync::Arc;

pub use error::{Error, Result};
pub use git::{Git, GitChoice, GitSource, GitVersion};
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
    /// Shared by all linked worktrees; objects and the commit-graph live here.
    common_dir: PathBuf,
}

impl Repo {
    /// Open the repository containing `path`.
    pub fn open(git: Arc<Git>, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let rev_parse = |args: &[&str]| -> Result<Option<String>> {
            let out = git_command(git.exe(), path)
                .arg("rev-parse")
                .args(args)
                .output()
                .map_err(Error::Spawn)?;
            Ok(out
                .status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
        };

        let info = rev_parse(&[
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--is-bare-repository",
        ])?
        .ok_or_else(|| Error::NotARepository(path.to_path_buf()))?;
        let mut lines = info.lines();
        let git_dir = PathBuf::from(lines.next().ok_or(Error::Parse("rev-parse output"))?);
        let common_dir = PathBuf::from(lines.next().ok_or(Error::Parse("rev-parse output"))?);
        let bare = lines.next() == Some("true");

        // Bare repositories have no work tree; `--show-toplevel` would fail there.
        let root = if bare {
            git_dir.clone()
        } else {
            rev_parse(&["--show-toplevel"])?
                .map(PathBuf::from)
                .ok_or_else(|| Error::NotARepository(path.to_path_buf()))?
        };
        Ok(Self {
            git,
            root,
            git_dir,
            common_dir,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// A `git` command already pointed at this repository.
    pub fn command(&self) -> Command {
        git_command(self.git.exe(), &self.root)
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
        let info = self.common_dir.join("objects").join("info");
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
        if out.status.success() {
            Ok(())
        } else {
            Err(Error::Git {
                command: "commit-graph write",
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
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

/// Base `git` invocation with settings that make output stable to parse,
/// whatever the user's config says.
fn git_command(exe: &Path, dir: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.quotepath=off",
            "-c",
            "color.ui=false",
            "-c",
            "log.showSignature=false",
            "--no-pager",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    // Never pop up a console window on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
