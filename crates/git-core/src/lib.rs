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
    /// `objects/info`, where the commit-graph lives.
    objects_info: PathBuf,
}

impl Repo {
    /// Open the repository containing `path`.
    pub fn open(git: Arc<Git>, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let rev_parse = |args: &[&str]| -> Result<String> {
            let out = git_command(git.exe(), path)
                .arg("rev-parse")
                .args(args)
                .output()
                .map_err(Error::Spawn)?;
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned());
            }
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            // Other failures (unknown extension, dubious ownership, ...) must
            // not be reported as "not a repository": the message is the fix.
            Err(if stderr.contains("not a git repository") {
                Error::NotARepository(path.to_path_buf())
            } else {
                Error::Git {
                    command: "rev-parse",
                    stderr,
                }
            })
        };

        let info = rev_parse(&[
            "--path-format=absolute",
            "--git-dir",
            "--git-path",
            "objects/info",
            "--is-bare-repository",
        ])?;
        let mut lines = info.lines();
        let git_dir = PathBuf::from(lines.next().ok_or(Error::Parse("rev-parse output"))?);
        // Asked from git so GIT_OBJECT_DIRECTORY and linked worktrees (whose
        // objects live in the common dir) are respected.
        let objects_info = PathBuf::from(lines.next().ok_or(Error::Parse("rev-parse output"))?);
        let bare = lines.next() == Some("true");

        // Bare repositories have no work tree; `--show-toplevel` would fail there.
        let root = if bare {
            git_dir.clone()
        } else {
            PathBuf::from(rev_parse(&["--show-toplevel"])?)
        };
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

/// A bare `git` process with settings that make output stable to parse,
/// whatever the user's config says.
pub(crate) fn base_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args([
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

fn git_command(exe: &Path, dir: &Path) -> Command {
    let mut cmd = base_command(exe);
    cmd.arg("-C").arg(dir);
    cmd
}
