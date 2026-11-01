//! Repository access for the client.
//!
//! Opening a repository goes through [`GitRunner`]. The default engine is
//! in-process `gix` ([`GitEngine::Native`]); [`GitEngine::Cli`] still shells
//! out to `git rev-parse`. Mutations stay on the `git` executable so hooks,
//! credential helpers, LFS, `includeIf` configs and signing keep working
//! exactly as in the terminal. Further hot read paths (log, status, diff)
//! move onto the same runner without touching the UI.

mod config;
pub mod credential;
mod diff;
mod error;
mod git;
mod log;
pub mod network;
pub mod oid;
mod ops;
mod refs;
mod runner;
pub mod ssh;
mod stash;
mod status;
mod submodule;
mod worktree;

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

pub use config::{
    global_get, global_identity, set_global_identity, set_global_signing, ConfigScope, Identity,
};
pub use credential::Credential;
pub use diff::{
    ChangeKind, ChangedFile, CommitDetails, DiffLine, DiffOptions, DiffWhitespace, FileDiff,
    FileTexts, Hunk, LineKind,
};
pub use error::{Error, ErrorClass, Result};
pub use git::{Git, GitChoice, GitEngine, GitSource, GitVersion, Resolution};
pub use log::{CommitCount, CommitInfo, LogKiller, LogOptions, LogStream};
pub use oid::Oid;
pub use ops::ResetMode;
pub use refs::{Head, Ref, RefKind};
pub use runner::{DiscoveredRepo, GitRunner};
pub use stash::Stash;
pub use status::{HeadCommit, HunkPick, WorkingFile, WorkingStatus};
pub use submodule::{Submodule, SubmoduleState};
pub use worktree::Worktree;

/// An opened repository (work tree or bare).
#[derive(Debug, Clone)]
pub struct Repo {
    git: Arc<Git>,
    /// Top of the work tree, or the git dir for bare repositories.
    root: PathBuf,
    git_dir: PathBuf,
    /// `objects/info`, where the commit-graph lives.
    objects_info: PathBuf,
    bare: bool,
}

impl Repo {
    /// Open the repository containing `path`.
    pub fn open(git: Arc<Git>, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // Checked here so a missing folder is not reported as "not a git
        // repository" by the discovery engine.
        if !path.is_dir() {
            return Err(Error::PathNotFound {
                path: path.to_path_buf(),
            });
        }
        let found = git.discover(path)?;
        Ok(Self {
            git,
            root: found.root,
            git_dir: found.git_dir,
            objects_info: found.objects_info,
            bare: found.bare,
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

    /// Whether this is a bare repository (no work tree).
    pub(crate) fn is_bare(&self) -> bool {
        self.bare
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

    /// The configured remotes, in git's order.
    pub fn remotes(&self) -> Result<Vec<String>> {
        refs::remotes(self)
    }

    /// The stash entries, newest first (`stash@{0}` first).
    pub fn stashes(&self) -> Result<Vec<Stash>> {
        stash::list(self)
    }

    /// The worktrees attached to this repository. The one this repository was
    /// opened in is marked `current`.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        worktree::list(self)
    }

    /// Checks out `branch` in a new worktree at `path`. The folder is created
    /// when it does not exist.
    pub fn add_worktree(&self, path: &Path, branch: &str) -> Result<()> {
        worktree::add(self, path, branch)
    }

    /// Creates `branch` at `start` and checks it out in a new worktree at
    /// `path`.
    pub fn add_worktree_for_new_branch(&self, path: &Path, branch: &str, start: Oid) -> Result<()> {
        worktree::add_new_branch(self, path, branch, start)
    }

    /// Removes the worktree at `path`. `force` discards its local changes.
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        worktree::remove(self, path, force)
    }

    /// Locks the worktree at `path`; `reason` is kept by git.
    pub fn lock_worktree(&self, path: &Path, reason: Option<&str>) -> Result<()> {
        worktree::lock(self, path, reason)
    }

    /// Unlocks the worktree at `path`.
    pub fn unlock_worktree(&self, path: &Path) -> Result<()> {
        worktree::unlock(self, path)
    }

    /// Drops the records of worktrees whose folders are gone.
    pub fn prune_worktrees(&self) -> Result<()> {
        worktree::prune(self)
    }

    /// The submodules recorded in `.gitmodules`.
    pub fn submodules(&self) -> Result<Vec<Submodule>> {
        submodule::list(self)
    }

    /// One commit's full message, author and committer.
    pub fn commit_details(&self, oid: Oid) -> Result<CommitDetails> {
        diff::details(self, oid)
    }

    /// The files `oid` changed, against its first parent (the empty tree for
    /// a root commit).
    pub fn commit_changes(&self, oid: Oid) -> Result<Vec<ChangedFile>> {
        diff::changes(self, oid)
    }

    /// The unified diff of one file in `oid`, against its first parent.
    ///
    /// `old_path` is the parent's path for a rename or a copy.
    pub fn file_diff(&self, oid: Oid, path: &str, old_path: Option<&str>) -> Result<FileDiff> {
        diff::file_diff(self, oid, path, old_path)
    }

    /// [`Repo::file_diff`] with the context and whitespace options chosen in
    /// Diff & merge settings.
    pub fn file_diff_with(
        &self,
        oid: Oid,
        path: &str,
        old_path: Option<&str>,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        diff::file_diff_with(self, oid, path, old_path, options)
    }

    /// The old and new text of one file, for syntax highlighting.
    pub fn file_texts(&self, oid: Oid, path: &str, old_path: Option<&str>) -> Result<FileTexts> {
        diff::file_texts(self, oid, path, old_path)
    }

    /// Everything the local-changes view shows: HEAD, the branch, staged and
    /// unstaged files with their line counts.
    pub fn working_status(&self) -> Result<WorkingStatus> {
        status::status(self)
    }

    /// The paths with local changes, staged or not, without line counts. The
    /// worktree page uses it for each worktree's changed count and file list.
    pub fn changed_paths(&self) -> Result<Vec<String>> {
        status::changed_paths(self)
    }

    /// Adds `paths` to the index (new files, modifications and deletions).
    pub fn stage(&self, paths: &[String]) -> Result<()> {
        status::stage(self, paths)
    }

    /// Removes `paths` from the index, leaving the work tree alone.
    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        status::unstage(self, paths)
    }

    /// Restores `tracked` files from the index and deletes `untracked` ones.
    pub fn discard(&self, tracked: &[String], untracked: &[String]) -> Result<()> {
        status::discard(self, tracked, untracked)
    }

    /// One file's work-tree diff (index to work tree).
    pub fn unstaged_diff(&self, path: &str, old_path: Option<&str>) -> Result<FileDiff> {
        status::unstaged_diff(self, path, old_path)
    }

    /// [`Repo::unstaged_diff`] with the Diff & merge options.
    pub fn unstaged_diff_with(
        &self,
        path: &str,
        old_path: Option<&str>,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        status::unstaged_diff_with(self, path, old_path, options)
    }

    /// One file's index diff (HEAD to index).
    pub fn staged_diff(&self, path: &str, old_path: Option<&str>) -> Result<FileDiff> {
        status::staged_diff(self, path, old_path)
    }

    /// [`Repo::staged_diff`] with the Diff & merge options.
    pub fn staged_diff_with(
        &self,
        path: &str,
        old_path: Option<&str>,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        status::staged_diff_with(self, path, old_path, options)
    }

    /// An untracked file as an all-additions diff.
    pub fn untracked_diff(&self, path: &str) -> Result<FileDiff> {
        status::untracked_diff(self, path)
    }

    /// Stages the picked hunks/lines of one unstaged file.
    pub fn stage_selection(
        &self,
        file: &WorkingFile,
        diff: &FileDiff,
        picks: &[HunkPick],
    ) -> Result<()> {
        status::stage_selection(self, file, diff, picks)
    }

    /// Unstages the picked hunks/lines of one staged file.
    pub fn unstage_selection(
        &self,
        file: &WorkingFile,
        diff: &FileDiff,
        picks: &[HunkPick],
    ) -> Result<()> {
        status::unstage_selection(self, file, diff, picks)
    }

    /// Commits the index, or rewrites HEAD with `amend`. Returns the new
    /// commit.
    pub fn commit(&self, message: &str, amend: bool) -> Result<Oid> {
        status::commit(self, message, amend)
    }

    /// HEAD's commit, or `None` while the branch is unborn.
    pub fn head_commit(&self) -> Result<Option<HeadCommit>> {
        status::head_commit(self)
    }

    // ---- ref and history operations (stage 7) -------------------------

    /// Checks out a local branch.
    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        ops::checkout_branch(self, name)
    }

    /// Checks out a commit with a detached HEAD (a tag or an older commit).
    pub fn checkout_detached(&self, oid: Oid) -> Result<()> {
        ops::checkout_detached(self, oid)
    }

    /// Checks out a remote branch, creating a local tracking branch when one
    /// with the same leaf name does not exist yet.
    pub fn checkout_remote(&self, remote_ref: &str) -> Result<()> {
        ops::checkout_remote(self, remote_ref)
    }

    /// Creates a branch at `start`; `checkout` switches to it straight away.
    pub fn create_branch(&self, name: &str, start: Oid, checkout: bool) -> Result<()> {
        ops::create_branch(self, name, start, checkout)
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        ops::rename_branch(self, old, new)
    }

    /// Deletes a local branch; `force` also drops unmerged commits.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        ops::delete_branch(self, name, force)
    }

    /// Creates a lightweight tag, or an annotated (and optionally signed) one
    /// when `message` is given.
    pub fn create_tag(
        &self,
        name: &str,
        target: Oid,
        message: Option<&str>,
        sign: bool,
    ) -> Result<()> {
        ops::create_tag(self, name, target, message, sign)
    }

    pub fn rename_tag(&self, old: &str, new: &str) -> Result<()> {
        ops::rename_tag(self, old, new)
    }

    pub fn delete_tag(&self, name: &str) -> Result<()> {
        ops::delete_tag(self, name)
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        ops::add_remote(self, name, url)
    }

    pub fn set_remote_url(&self, name: &str, url: &str) -> Result<()> {
        ops::set_remote_url(self, name, url)
    }

    pub fn rename_remote(&self, old: &str, new: &str) -> Result<()> {
        ops::rename_remote(self, old, new)
    }

    pub fn remove_remote(&self, name: &str) -> Result<()> {
        ops::remove_remote(self, name)
    }

    /// The fetch URL of `name`, or `None` when no such remote is configured.
    pub fn remote_url(&self, name: &str) -> Result<Option<String>> {
        ops::remote_url(self, name)
    }

    /// Applies `oid` on top of HEAD; a conflict is aborted and returned.
    pub fn cherry_pick(&self, oid: Oid) -> Result<()> {
        ops::cherry_pick(self, oid)
    }

    /// Reverts `oid` with a new commit; a conflict is aborted and returned.
    pub fn revert(&self, oid: Oid) -> Result<()> {
        ops::revert(self, oid)
    }

    /// Moves HEAD to `oid`, rewriting the index and work tree per `mode`.
    pub fn reset(&self, oid: Oid, mode: ResetMode) -> Result<()> {
        ops::reset(self, oid, mode)
    }

    /// Stashes all local changes including untracked files; `false` when the
    /// work tree was already clean.
    pub fn stash_save(&self, message: Option<&str>) -> Result<bool> {
        ops::stash_save(self, message)
    }

    pub fn stash_apply(&self, index: usize) -> Result<()> {
        ops::stash_apply(self, index)
    }

    pub fn stash_pop(&self, index: usize) -> Result<()> {
        ops::stash_pop(self, index)
    }

    pub fn stash_drop(&self, index: usize) -> Result<()> {
        ops::stash_drop(self, index)
    }

    /// `user.signingkey`, for the tag dialog's signing option.
    pub fn signing_key(&self) -> Result<Option<String>> {
        ops::signing_key(self)
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
