//! The worktrees of one repository as a shared model.
//!
//! [`RepositoryView`](crate::repo_view::RepositoryView) owns one
//! [`WorktreesModel`]; the Worktrees page reads it today, the sidebar panel and
//! the titlebar switcher read it next. It loads the list, each folder's local
//! changes, each HEAD's summary and ahead/behind in one background batch, keeps
//! the selection and the recency order, and runs the operations (create,
//! switch, lock, prune, remove) that change the list. Every operation reloads
//! the list afterwards.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, ClipboardItem, Context, EventEmitter, WeakEntity, Window};

use git_core::{ChangedEntry, Error, Git, Head, Oid, RefKind, Repo, Worktree};
use ui::{toast, ToastKind};

use crate::format::{display_path, expand_home, home_dir};
use crate::ref_ops::ref_name_error;
use crate::settings::{AfterCreate, WorktreeSettings};
use crate::workspace::{repo_name, Workspace};
use crate::worktree_dialog::BranchChoice;
use crate::worktree_path::worktree_path_for;

/// One worktree with everything the pages show about it.
#[derive(Clone, Debug)]
pub struct WorktreeEntry {
    pub worktree: Worktree,
    /// The folder's local changes, one per path.
    pub changes: Vec<ChangedEntry>,
    /// The folder is gone (git lists it as prunable or it is not a directory).
    pub missing: bool,
    /// The first line of the HEAD commit message.
    pub head_summary: Option<String>,
    /// Commits ahead of / behind the branch's upstream.
    pub ahead: usize,
    pub behind: usize,
    /// The branch has an upstream.
    pub has_upstream: bool,
    /// When GitWarp last opened this worktree, in unix seconds.
    pub last_used: Option<u64>,
}

impl WorktreeEntry {
    pub fn path(&self) -> &Path {
        &self.worktree.path
    }

    /// The folder's name, which is what the design calls the worktree's name.
    pub fn name(&self) -> String {
        self.worktree
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.worktree.path.display().to_string())
    }

    /// The checked-out branch, `None` when detached.
    pub fn branch(&self) -> Option<&str> {
        self.worktree.branch.as_deref()
    }

    pub fn is_main(&self) -> bool {
        self.worktree.main
    }

    /// The worktree this window is open on.
    pub fn is_current(&self) -> bool {
        self.worktree.current
    }

    pub fn locked(&self) -> Option<&str> {
        self.worktree.locked.as_deref()
    }

    /// The paths of the local changes.
    pub fn changed_paths(&self) -> Vec<String> {
        self.changes
            .iter()
            .map(|change| change.path.clone())
            .collect()
    }
}

/// What typing a branch name into quick create (or the switcher) means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchCheck {
    Empty,
    /// Not a name git accepts.
    Invalid,
    /// Already checked out in another worktree.
    InUse {
        path: PathBuf,
        name: String,
    },
    /// A local branch nobody has checked out.
    Existing,
    /// Only `remote/branch` exists: a tracking branch is created.
    Tracking(String),
    /// A new branch from the main line.
    New,
}

/// Events of [`WorktreesModel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreesModelEvent {
    /// An operation changed the repository (worktrees, branches, HEADs): the
    /// history, sidebar and local changes should reload.
    Changed,
    /// quick create made a worktree; the name that was typed is used up.
    Created,
}

/// The result of one background read.
struct Loaded {
    entries: Vec<WorktreeEntry>,
    branches: Vec<BranchChoice>,
    remote_branches: Vec<String>,
    current_branch: Option<String>,
    main_line: Option<String>,
    /// Where new branches start: the main line's tip, else HEAD.
    start_point: Option<Oid>,
}

pub struct WorktreesModel {
    root: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    home: Option<PathBuf>,
    entries: Vec<WorktreeEntry>,
    branches: Vec<BranchChoice>,
    remote_branches: Vec<String>,
    current_branch: Option<String>,
    main_line: Option<String>,
    start_point: Option<Oid>,
    selected: Option<PathBuf>,
    /// Last-opened times by folder, from the settings plus this session.
    recency: BTreeMap<String, u64>,
    loaded: bool,
    loading: bool,
    generation: u64,
    error: Option<String>,
    creating: bool,
    /// The current worktree's recency is recorded once per model.
    recorded_current: bool,
    /// A read finished successfully before.
    loaded_once: bool,
}

impl EventEmitter<WorktreesModelEvent> for WorktreesModel {}

impl WorktreesModel {
    pub(crate) fn new(
        root: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut model = Self {
            root,
            git,
            workspace,
            home: home_dir(),
            entries: Vec::new(),
            branches: Vec::new(),
            remote_branches: Vec::new(),
            current_branch: None,
            main_line: None,
            start_point: None,
            selected: None,
            recency: BTreeMap::new(),
            loaded: false,
            loading: false,
            generation: 0,
            error: None,
            creating: false,
            recorded_current: false,
            loaded_once: false,
        };
        model.reload(cx);
        model
    }

    // ----- reading -----

    /// Every worktree: the current one first, then the most recently opened;
    /// worktrees never opened here keep git's order after those.
    pub fn entries(&self) -> &[WorktreeEntry] {
        &self.entries
    }

    /// The worktree this window is open on.
    pub fn current(&self) -> Option<&WorktreeEntry> {
        self.entries.iter().find(|entry| entry.is_current())
    }

    /// The worktree the detail pane shows: the selected one, else the current.
    pub fn selected(&self) -> Option<&WorktreeEntry> {
        self.selected
            .as_deref()
            .and_then(|path| self.entry(path))
            .or_else(|| self.current())
    }

    pub fn entry(&self, path: &Path) -> Option<&WorktreeEntry> {
        self.entries.iter().find(|entry| entry.path() == path)
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_creating(&self) -> bool {
        self.creating
    }

    /// The message of the last failed read, `None` after a successful one.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn git(&self) -> &Result<Arc<Git>, Arc<Error>> {
        &self.git
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn workspace(&self) -> &WeakEntity<Workspace> {
        &self.workspace
    }

    pub fn branches(&self) -> &[BranchChoice] {
        &self.branches
    }

    pub fn current_branch(&self) -> Option<&str> {
        self.current_branch.as_deref()
    }

    pub fn main_line(&self) -> Option<&str> {
        self.main_line.as_deref()
    }

    /// Local branches that no worktree has checked out.
    pub fn free_branches(&self) -> Vec<&str> {
        self.branches
            .iter()
            .map(|branch| branch.name.as_str())
            .filter(|name| {
                !self
                    .entries
                    .iter()
                    .any(|entry| entry.branch() == Some(name))
            })
            .collect()
    }

    pub fn check_branch(&self, name: &str) -> BranchCheck {
        check_branch(name, &self.entries, &self.branches, &self.remote_branches)
    }

    /// The folder quick create would use for `branch`.
    pub fn preview_path(&self, branch: &str, cx: &App) -> PathBuf {
        self.path_for(branch, &worktree_settings(cx))
    }

    fn path_for(&self, branch: &str, settings: &WorktreeSettings) -> PathBuf {
        let base = expand_home(&settings.base, self.home.as_deref());
        let repo = repo_name(&self.root).to_string();
        worktree_path_for(&base, &settings.template, &repo, branch, |path| {
            path.exists() || self.entries.iter().any(|entry| entry.path() == path)
        })
    }

    // ----- selection -----

    pub fn select(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(path) || self.entry(path).is_none() {
            return;
        }
        self.selected = Some(path.to_path_buf());
        cx.notify();
    }

    // ----- loading -----

    /// Adopts the repository root the history resolved (it differs when the
    /// repository was opened from a subdirectory) and reads again.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root;
        self.reload(cx);
    }

    /// Reads the list, the branches and every worktree's state off the UI
    /// thread.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.loaded = true;
                self.loading = false;
                self.error = Some(git_message(error));
                cx.notify();
                return;
            }
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let root = self.root.clone();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(git, &root) })
                .await;
            this.update(cx, |model, cx| model.set_loaded(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_loaded(
        &mut self,
        generation: u64,
        result: git_core::Result<Loaded>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        self.loaded = true;
        match result {
            Ok(loaded) => {
                self.error = None;
                // A worktree that appeared since the last read (the add
                // dialog made it) becomes the selection, like after quick
                // create.
                let appeared = self.loaded_once.then(|| {
                    loaded
                        .entries
                        .iter()
                        .find(|new| self.entries.iter().all(|old| old.path() != new.path()))
                        .map(|new| new.path().to_path_buf())
                });
                self.loaded_once = true;
                self.entries = loaded.entries;
                self.branches = loaded.branches;
                self.remote_branches = loaded.remote_branches;
                self.current_branch = loaded.current_branch;
                self.main_line = loaded.main_line;
                self.start_point = loaded.start_point;
                self.merge_stored_recency(cx);
                order_entries(&mut self.entries, &self.recency);
                if let Some(Some(path)) = appeared {
                    self.selected = Some(path);
                }
                if self
                    .selected
                    .as_deref()
                    .and_then(|p| self.entry(p))
                    .is_none()
                {
                    self.selected = self.current().map(|entry| entry.path().to_path_buf());
                }
                if !self.recorded_current {
                    if let Some(path) = self.current().map(|entry| entry.path().to_path_buf()) {
                        self.recorded_current = true;
                        self.touch(&path, cx);
                    }
                }
            }
            Err(error) => {
                self.error = Some(git_message(&error));
            }
        }
        cx.notify();
    }

    /// Takes the persisted times (another window may have written newer ones).
    fn merge_stored_recency(&mut self, cx: &App) {
        let Some(state) = cx.try_global::<crate::AppSettings>() else {
            return;
        };
        for (path, time) in &state.settings.worktree_recency {
            let slot = self.recency.entry(path.clone()).or_insert(*time);
            *slot = (*slot).max(*time);
        }
    }

    /// Records that `path` was opened now, in memory and in the settings.
    fn touch(&mut self, path: &Path, cx: &mut Context<Self>) {
        let now = now_secs();
        self.recency
            .insert(path.to_string_lossy().into_owned(), now);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.path() == path) {
            entry.last_used = Some(now);
        }
        let path = path.to_path_buf();
        crate::update_settings(cx, move |settings| settings.touch_worktree(&path, now));
    }

    // ----- operations -----

    fn git_or_toast(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Arc<Git>> {
        match &self.git {
            Ok(git) => Some(git.clone()),
            Err(error) => {
                toast::show(ToastKind::Error, error.to_string(), window, cx);
                None
            }
        }
    }

    /// Switches this window to the worktree at `path`.
    pub fn switch_to(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(path).cloned() else {
            toast::show(
                ToastKind::Warn,
                "The worktree is no longer in the list",
                window,
                cx,
            );
            self.reload(cx);
            return;
        };
        let name = entry.name();
        if entry.missing {
            toast::show(
                ToastKind::Error,
                format!(
                    "The folder of {name} is missing. Remove the record or restore the folder."
                ),
                window,
                cx,
            );
            return;
        }
        if entry.is_current() {
            toast::show(
                ToastKind::Info,
                format!("{name} is already the current worktree"),
                window,
                cx,
            );
            return;
        }
        self.touch(path, cx);
        let branch = entry.branch().unwrap_or("detached HEAD").to_owned();
        toast::show(
            ToastKind::Ok,
            format!("Switched to {name} · {branch}"),
            window,
            cx,
        );
        let path = path.to_path_buf();
        // This replaces the window's content, and with it this model.
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx);
            })
            .ok();
    }

    /// Opens the worktree at `path` in a second window.
    pub fn open_in_window(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(path).cloned() else {
            return;
        };
        let name = entry.name();
        if entry.missing {
            toast::show(
                ToastKind::Error,
                format!(
                    "The folder of {name} is missing. Remove the record or restore the folder."
                ),
                window,
                cx,
            );
            return;
        }
        self.touch(path, cx);
        let path = path.to_path_buf();
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_repository_window(path, window, cx);
            })
            .ok();
        toast::show(
            ToastKind::Ok,
            format!("Opened {name} in a new window"),
            window,
            cx,
        );
    }

    /// Creates a worktree for `branch` from the user's settings: the folder
    /// comes from the template, an existing local branch is checked out, a
    /// remote-only name gets a tracking branch, anything else a new branch
    /// from the main line. Then it switches to the worktree or stays, as the
    /// settings say.
    pub fn create(&mut self, branch: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let name = branch.trim().to_owned();
        let kind = match self.check_branch(&name) {
            BranchCheck::Existing => CreateKind::Existing,
            BranchCheck::Tracking(remote) => CreateKind::Tracking(remote),
            BranchCheck::New => CreateKind::New,
            BranchCheck::InUse { name: used, .. } => {
                toast::show(
                    ToastKind::Error,
                    format!("“{name}” is already checked out in {used}."),
                    window,
                    cx,
                );
                return;
            }
            BranchCheck::Empty | BranchCheck::Invalid => {
                toast::show(
                    ToastKind::Error,
                    "This is not a valid branch name.",
                    window,
                    cx,
                );
                return;
            }
        };
        let Some(git) = self.git_or_toast(window, cx) else {
            return;
        };
        let settings = worktree_settings(cx);
        let path = self.path_for(&name, &settings);
        let plan = CreatePlan {
            kind,
            branch: name.clone(),
            path: path.clone(),
            start: self.start_point,
            // The files come from the worktree this window shows.
            copy_env_from: settings
                .copy_env
                .then(|| self.current().map(|entry| entry.path().to_path_buf()))
                .flatten(),
        };
        let root = self.root.clone();
        let label = display_path(&path, self.home.as_deref());
        self.creating = true;
        toast::show(
            ToastKind::Info,
            format!("Creating worktree {label}…"),
            window,
            cx,
        );
        let after = settings.after_create;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { create_on_disk(git, &root, &plan) })
                .await;
            this.update_in(cx, |model, window, cx| {
                model.finish_create(name, path, after, result, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_create(
        &mut self,
        branch: String,
        path: PathBuf,
        after: AfterCreate,
        result: git_core::Result<CreatedWorktree>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.creating = false;
        let created = match result {
            Ok(created) => created,
            Err(error) => {
                toast::show(ToastKind::Error, error.to_string(), window, cx);
                self.reload(cx);
                cx.notify();
                return;
            }
        };
        let folder = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut message = if created.new_branch {
            format!("Created {folder} on new branch {branch}")
        } else {
            format!("Created {folder} on {branch}")
        };
        if !created.copied.is_empty() {
            message.push_str(&format!(" · copied {}", created.copied.join(", ")));
        }
        toast::show(ToastKind::Ok, message, window, cx);
        self.selected = Some(path.clone());
        cx.emit(WorktreesModelEvent::Created);
        match after {
            AfterCreate::Switch => {
                // The new folder is not in the list yet: open it directly.
                self.touch(&path, cx);
                self.workspace
                    .update(cx, |workspace, cx| {
                        workspace.open_repository(path, window, cx);
                    })
                    .ok();
            }
            AfterCreate::Stay => {
                self.reload(cx);
                cx.emit(WorktreesModelEvent::Changed);
            }
        }
        cx.notify();
    }

    /// Locks (with a manual reason) or unlocks the worktree at `path`.
    pub fn toggle_lock(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(path) else {
            return;
        };
        let locked = entry.locked().is_some();
        let name = entry.name();
        let Some(git) = self.git_or_toast(window, cx) else {
            return;
        };
        let root = self.root.clone();
        let path = path.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    if locked {
                        repo.unlock_worktree(&path)
                    } else {
                        repo.lock_worktree(&path, Some("locked manually"))
                    }
                })
                .await;
            this.update_in(cx, |model, window, cx| match result {
                Ok(()) => {
                    let verb = if locked { "Unlocked" } else { "Locked" };
                    toast::show(ToastKind::Ok, format!("{verb} {name}"), window, cx);
                    model.operation_done(cx);
                }
                Err(error) => {
                    toast::show(ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Prune N missing": drops the records whose folders are gone.
    pub fn prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stale = self.entries.iter().filter(|entry| entry.missing).count();
        let Some(git) = self.git_or_toast(window, cx) else {
            return;
        };
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::open(git, &root)?.prune_worktrees() })
                .await;
            this.update_in(cx, |model, window, cx| match result {
                Ok(()) => {
                    toast::show(
                        ToastKind::Ok,
                        format!(
                            "Pruned {stale} stale worktree record{}",
                            if stale == 1 { "" } else { "s" }
                        ),
                        window,
                        cx,
                    );
                    model.operation_done(cx);
                }
                Err(error) => {
                    toast::show(ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Removes a clean worktree without the dialog (Settings → Worktrees →
    /// "Removing a worktree" is off).
    pub fn remove_clean(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git_or_toast(window, cx) else {
            return;
        };
        let root = self.root.clone();
        let label = display_path(path, self.home.as_deref());
        let path = path.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::open(git, &root)?.remove_worktree(&path, false) })
                .await;
            this.update_in(cx, |model, window, cx| match result {
                Ok(()) => {
                    toast::show(
                        ToastKind::Ok,
                        format!("Removed worktree {label}"),
                        window,
                        cx,
                    );
                    model.operation_done(cx);
                }
                Err(error) => {
                    toast::show(ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reloads after an operation and tells the shell the repository changed.
    pub fn operation_done(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        cx.emit(WorktreesModelEvent::Changed);
    }

    /// Shows the folder in the file manager.
    pub fn reveal(&self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        cx.reveal_path(path);
        let label = display_path(path, self.home.as_deref());
        toast::show(ToastKind::Info, format!("Revealed {label}"), window, cx);
    }

    /// Copies the folder path to the clipboard.
    pub fn copy_path(&self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let text = path.display().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        toast::show(ToastKind::Ok, format!("Copied {text}"), window, cx);
    }
}

/// Switches the window to a worktree that was just created outside the model
/// (the add dialog): records its recency, then opens it in `workspace`'s window.
pub(crate) fn switch_window_to(
    workspace: &WeakEntity<Workspace>,
    path: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    let now = now_secs();
    let touched = path.clone();
    crate::update_settings(cx, move |settings| settings.touch_worktree(&touched, now));
    workspace
        .update(cx, |workspace, cx| {
            workspace.open_repository(path, window, cx)
        })
        .ok();
}

/// The user's worktree settings, the defaults when none are loaded (tests).
/// What a worktree banner shows for a failed git command: git's own message
/// (`fatal: …` / `error: …`), without the `git <cmd> failed (exit N):` wrapper
/// and without progress lines such as `Preparing worktree (…)`, as in the design.
pub(crate) fn git_message(error: &Error) -> String {
    let Error::Git { stderr, .. } = error else {
        return error.to_string();
    };
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let messages: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .collect();
    if messages.is_empty() {
        // Nothing recognisable: keep everything git said rather than nothing.
        error.to_string()
    } else {
        messages.join("\n")
    }
}

fn worktree_settings(cx: &App) -> WorktreeSettings {
    cx.try_global::<crate::AppSettings>()
        .map(|state| state.settings.worktrees.clone())
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// What typing `name` means given the worktrees and branches that exist.
pub fn check_branch(
    name: &str,
    entries: &[WorktreeEntry],
    branches: &[BranchChoice],
    remote_branches: &[String],
) -> BranchCheck {
    let name = name.trim();
    if name.is_empty() {
        return BranchCheck::Empty;
    }
    if ref_name_error(name, &[]).is_some() {
        return BranchCheck::Invalid;
    }
    if let Some(used) = entries.iter().find(|entry| entry.branch() == Some(name)) {
        return BranchCheck::InUse {
            path: used.path().to_path_buf(),
            name: used.name(),
        };
    }
    if branches.iter().any(|branch| branch.name == name) {
        return BranchCheck::Existing;
    }
    // `origin/feat/x` is the remote `origin` and the branch `feat/x`.
    if let Some(remote) = remote_branches.iter().find(|remote| {
        remote
            .split_once('/')
            .is_some_and(|(_, branch)| branch == name)
    }) {
        return BranchCheck::Tracking(remote.clone());
    }
    BranchCheck::New
}

/// Sorts for display and stamps each entry's recency: the current worktree
/// first, then the most recently opened; the rest keep git's order.
fn order_entries(entries: &mut [WorktreeEntry], recency: &BTreeMap<String, u64>) {
    for entry in entries.iter_mut() {
        entry.last_used = recency
            .get(entry.path().to_string_lossy().as_ref())
            .copied();
    }
    // `sort_by_key` is stable: equal keys keep git's order.
    entries.sort_by_key(|entry| {
        (
            !entry.is_current(),
            std::cmp::Reverse(entry.last_used.unwrap_or(0)),
        )
    });
}

/// Reads the list, branches, changes, summaries and ahead/behind; runs on the
/// background executor.
fn load(git: Arc<Git>, root: &Path) -> git_core::Result<Loaded> {
    let repo = Repo::open(git.clone(), root)?;
    let worktrees = repo.worktrees()?;
    let refs = repo.refs()?;
    let head = repo.head()?;
    let main_line = repo.main_line()?;
    let (current_branch, head_oid) = match head {
        Head::Branch { name, target } => (Some(name), target),
        Head::Detached(oid) => (None, Some(oid)),
    };
    let branches: Vec<BranchChoice> = refs
        .iter()
        .filter(|reference| reference.kind == RefKind::LocalBranch)
        .map(|reference| BranchChoice {
            name: reference.name.clone(),
            target: reference.target,
        })
        .collect();
    let remote_branches: Vec<String> = refs
        .iter()
        .filter(|reference| reference.kind == RefKind::RemoteBranch)
        // `origin/HEAD` is a pointer, not a branch to check out.
        .filter(|reference| !reference.name.ends_with("/HEAD"))
        .map(|reference| reference.name.clone())
        .collect();
    let start_point = main_line
        .as_ref()
        .and_then(|main| branches.iter().find(|branch| &branch.name == main))
        .map(|branch| branch.target)
        .or(head_oid);

    let heads: Vec<Oid> = worktrees.iter().filter_map(|w| w.head).collect();
    let summaries = match repo.head_summaries(&heads) {
        Ok(summaries) => summaries,
        Err(error) => {
            log::warn!("could not read the HEAD summaries: {error}");
            HashMap::new()
        }
    };

    let mut entries = Vec::with_capacity(worktrees.len());
    for worktree in worktrees {
        let missing = worktree.prunable || !worktree.path.is_dir();
        let changes = if missing {
            Vec::new()
        } else {
            match Repo::open(git.clone(), &worktree.path).and_then(|repo| repo.changed_entries()) {
                Ok(changes) => changes,
                Err(error) => {
                    log::warn!(
                        "could not read the changes in {}: {error}",
                        worktree.path.display()
                    );
                    Vec::new()
                }
            }
        };
        let upstream = worktree.branch.as_ref().and_then(|name| {
            refs.iter()
                .find(|r| r.kind == RefKind::LocalBranch && &r.name == name)
        });
        entries.push(WorktreeEntry {
            head_summary: worktree.head.and_then(|oid| summaries.get(&oid).cloned()),
            ahead: upstream.map_or(0, |r| r.ahead),
            behind: upstream.map_or(0, |r| r.behind),
            has_upstream: upstream.is_some_and(|r| r.upstream.is_some()),
            last_used: None,
            worktree,
            changes,
            missing,
        });
    }
    Ok(Loaded {
        entries,
        branches,
        remote_branches,
        current_branch,
        main_line,
        start_point,
    })
}

#[derive(Clone, Debug)]
enum CreateKind {
    Existing,
    Tracking(String),
    New,
}

struct CreatePlan {
    kind: CreateKind,
    branch: String,
    path: PathBuf,
    start: Option<Oid>,
    copy_env_from: Option<PathBuf>,
}

struct CreatedWorktree {
    new_branch: bool,
    /// The `.env` files copied into the new worktree.
    copied: Vec<String>,
}

/// Runs `git worktree add` for the plan, then copies the `.env` files.
fn create_on_disk(
    git: Arc<Git>,
    root: &Path,
    plan: &CreatePlan,
) -> git_core::Result<CreatedWorktree> {
    let repo = Repo::open(git, root)?;
    let new_branch = match &plan.kind {
        CreateKind::Existing => {
            repo.add_worktree(&plan.path, &plan.branch)?;
            false
        }
        CreateKind::Tracking(remote) => {
            repo.add_worktree_tracking(&plan.path, &plan.branch, remote)?;
            false
        }
        CreateKind::New => {
            let Some(start) = plan.start else {
                return Err(Error::Parse("start commit"));
            };
            repo.add_worktree_for_new_branch(&plan.path, &plan.branch, start)?;
            true
        }
    };
    let copied = match &plan.copy_env_from {
        Some(from) => copy_env_files(from, &plan.path),
        None => Vec::new(),
    };
    Ok(CreatedWorktree { new_branch, copied })
}

/// `.env`, `.env.local` and `.env.<name>.local`: the local configuration a
/// checkout does not bring along.
fn is_env_file(name: &str) -> bool {
    if name == ".env" || name == ".env.local" {
        return true;
    }
    name.strip_prefix(".env.")
        .and_then(|rest| rest.strip_suffix(".local"))
        .is_some_and(|middle| !middle.is_empty())
}

/// Copies the env files `from` has and `to` lacks (a tracked one is already
/// there after the checkout, so only untracked or ignored files are copied);
/// returns their names, sorted. A file that cannot be copied is logged and
/// skipped: the worktree itself was created.
fn copy_env_files(from: &Path, to: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(from) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_env_file(name))
        .collect();
    names.sort();
    let mut copied = Vec::new();
    for name in names {
        let target = to.join(&name);
        if target.exists() {
            continue;
        }
        match std::fs::copy(from.join(&name), &target) {
            Ok(_) => copied.push(name),
            Err(error) => log::warn!("could not copy {name} to {}: {error}", to.display()),
        }
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git, open_workspace, TempRepo};
    use std::process::Command;

    fn git_error(stderr: &str) -> Error {
        Error::Git {
            command: "worktree add",
            code: Some(128),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn a_banner_shows_only_gits_message() {
        // The wrapper and the progress line go; the message stays.
        assert_eq!(
            git_message(&git_error(
                "Preparing worktree (checking out 'x')\nfatal: could not create leading directories of '/a/b': Permission denied\n"
            )),
            "fatal: could not create leading directories of '/a/b': Permission denied"
        );
        assert_eq!(
            git_message(&git_error(
                "error: failed to delete '/a': Permission denied"
            )),
            "error: failed to delete '/a': Permission denied"
        );
        // Nothing recognisable: everything git said is kept, wrapper included.
        assert_eq!(
            git_message(&git_error("weird output")),
            "git worktree add failed (exit 128): weird output"
        );
        // Not a git failure: the error's own text.
        assert_eq!(
            git_message(&Error::Parse("start commit")),
            "could not parse start commit"
        );
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?} failed");
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-qm", &format!("add {file}")]);
    }

    fn sibling(repo: &TempRepo, suffix: &str) -> PathBuf {
        let path = repo.path().parent().unwrap().join(format!(
            "{}-{suffix}",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn entry(path: &str, current: bool, main: bool, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            worktree: Worktree {
                path: PathBuf::from(path),
                branch: branch.map(str::to_owned),
                head: None,
                bare: false,
                detached: branch.is_none(),
                locked: None,
                prunable: false,
                current,
                main,
            },
            changes: Vec::new(),
            missing: false,
            head_summary: None,
            ahead: 0,
            behind: 0,
            has_upstream: false,
            last_used: None,
        }
    }

    #[test]
    fn loads_entries_with_summaries_changes_and_ahead_behind() {
        let repo = TempRepo::new("model-load");
        commit(repo.path(), "a.txt", "one\n");
        run(repo.path(), &["branch", "feat"]);
        let linked = sibling(&repo, "linked");
        Repo::open(git(), repo.path())
            .unwrap()
            .add_worktree(&linked, "feat")
            .unwrap();
        std::fs::write(linked.join("new.txt"), "x\n").unwrap();
        std::fs::write(linked.join("a.txt"), "changed\n").unwrap();

        let loaded = load(git(), repo.path()).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        let main = &loaded.entries[0];
        assert!(main.is_main() && main.is_current());
        assert_eq!(main.head_summary.as_deref(), Some("add a.txt"));
        assert!(main.changes.is_empty());
        let feat = &loaded.entries[1];
        assert_eq!(feat.branch(), Some("feat"));
        assert_eq!(feat.head_summary.as_deref(), Some("add a.txt"));
        let letters: Vec<(String, char)> = feat
            .changes
            .iter()
            .map(|c| (c.path.clone(), c.status))
            .collect();
        assert_eq!(
            letters,
            vec![("a.txt".to_owned(), 'M'), ("new.txt".to_owned(), '?')]
        );
        assert!(!feat.has_upstream);
        // The main line is the repository's only branch.
        assert_eq!(loaded.current_branch, loaded.main_line);
        assert!(loaded.start_point.is_some());
        let _ = std::fs::remove_dir_all(&linked);
    }

    #[test]
    fn branch_names_are_classified() {
        let entries = vec![
            entry("/r/repo", true, true, Some("main")),
            entry("/r/repo-hot", false, false, Some("fix/hot")),
        ];
        let oid = Oid::from_hex(&"ab".repeat(20)).unwrap();
        let branches = ["main", "fix/hot", "idle"].map(|name| BranchChoice {
            name: name.to_owned(),
            target: oid,
        });
        let remotes = vec!["origin/main".to_owned(), "origin/feat/remote".to_owned()];
        let check = |name: &str| check_branch(name, &entries, &branches, &remotes);

        assert_eq!(check(""), BranchCheck::Empty);
        assert_eq!(check("   "), BranchCheck::Empty);
        assert_eq!(check("has space"), BranchCheck::Invalid);
        assert_eq!(check("bad..name"), BranchCheck::Invalid);
        assert_eq!(check("ends/"), BranchCheck::Invalid);
        assert_eq!(
            check("fix/hot"),
            BranchCheck::InUse {
                path: PathBuf::from("/r/repo-hot"),
                name: "repo-hot".to_owned()
            }
        );
        assert_eq!(check("idle"), BranchCheck::Existing);
        assert_eq!(
            check("feat/remote"),
            BranchCheck::Tracking("origin/feat/remote".to_owned())
        );
        assert_eq!(check(" brand-new "), BranchCheck::New);
    }

    #[test]
    fn current_first_then_most_recent_then_git_order() {
        let mut entries = vec![
            entry("/r/main", false, true, Some("main")),
            entry("/r/a", false, false, Some("a")),
            entry("/r/b", true, false, Some("b")),
            entry("/r/c", false, false, Some("c")),
            entry("/r/d", false, false, Some("d")),
        ];
        let mut recency = BTreeMap::new();
        recency.insert("/r/c".to_owned(), 300);
        recency.insert("/r/a".to_owned(), 100);
        recency.insert("/r/b".to_owned(), 999);
        order_entries(&mut entries, &recency);
        let order: Vec<&str> = entries.iter().map(|e| e.path().to_str().unwrap()).collect();
        // b is current; c and a by time; main and d were never opened and keep
        // git's relative order.
        assert_eq!(order, ["/r/b", "/r/c", "/r/a", "/r/main", "/r/d"]);
        assert_eq!(entries[1].last_used, Some(300));
        assert_eq!(entries[3].last_used, None);
    }

    #[test]
    fn env_files_are_recognised_and_copied_once() {
        assert!(is_env_file(".env"));
        assert!(is_env_file(".env.local"));
        assert!(is_env_file(".env.production.local"));
        assert!(!is_env_file(".env.production"));
        assert!(!is_env_file(".env..local"));
        assert!(!is_env_file("env.local"));
        assert!(!is_env_file(".envrc"));

        let base = std::env::temp_dir().join(format!("gitwarp-env-{}", std::process::id()));
        let (from, to) = (base.join("from"), base.join("to"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join(".env"), "A=1").unwrap();
        std::fs::write(from.join(".env.local"), "B=2").unwrap();
        std::fs::write(from.join(".env.production"), "C=3").unwrap();
        std::fs::write(to.join(".env"), "tracked").unwrap();
        let copied = copy_env_files(&from, &to);
        assert_eq!(copied, vec![".env.local".to_owned()]);
        assert_eq!(std::fs::read_to_string(to.join(".env")).unwrap(), "tracked");
        assert!(!to.join(".env.production").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn creating_covers_existing_tracking_and_new_branches() {
        let repo = TempRepo::new("model-create");
        commit(repo.path(), "a.txt", "one\n");
        run(repo.path(), &["branch", "old"]);
        std::fs::write(repo.path().join(".env.local"), "K=1").unwrap();
        let start = Repo::open(git(), repo.path()).unwrap().head().ok();
        let start = match start {
            Some(Head::Branch { target, .. }) => target,
            _ => None,
        };

        let existing = sibling(&repo, "existing");
        let created = create_on_disk(
            git(),
            repo.path(),
            &CreatePlan {
                kind: CreateKind::Existing,
                branch: "old".to_owned(),
                path: existing.clone(),
                start,
                copy_env_from: Some(repo.path().to_path_buf()),
            },
        )
        .unwrap();
        assert!(!created.new_branch);
        assert_eq!(created.copied, vec![".env.local".to_owned()]);
        assert!(existing.join(".env.local").is_file());

        let fresh = sibling(&repo, "fresh");
        let created = create_on_disk(
            git(),
            repo.path(),
            &CreatePlan {
                kind: CreateKind::New,
                branch: "feat/fresh".to_owned(),
                path: fresh.clone(),
                start,
                copy_env_from: None,
            },
        )
        .unwrap();
        assert!(created.new_branch && created.copied.is_empty());
        let loaded = load(git(), repo.path()).unwrap();
        assert!(loaded
            .entries
            .iter()
            .any(|e| e.branch() == Some("feat/fresh")));
        assert!(loaded.branches.iter().any(|b| b.name == "feat/fresh"));
        let _ = std::fs::remove_dir_all(&existing);
        let _ = std::fs::remove_dir_all(&fresh);
    }

    /// Drives `create` through a real window and checks where it ends up.
    fn create_flow(after: AfterCreate, cx: &mut gpui::TestAppContext) {
        let repo = TempRepo::new("model-create-ui");
        commit(repo.path(), "a.txt", "one\n");
        let base = repo.path().parent().unwrap().join(format!(
            "{}-wts",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&base);

        let (mut cx, workspace) = open_workspace(git(), cx);
        cx.update(|_, cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings {
                    worktrees: WorktreeSettings {
                        base: base.display().to_string(),
                        template: "{branch}".to_owned(),
                        after_create: after,
                        copy_env: false,
                        ..WorktreeSettings::default()
                    },
                    ..crate::settings::Settings::default()
                },
                save_in_flight: false,
                dirty: false,
            });
        });
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path.clone(), window, cx)
            });
        });
        cx.run_until_parked();
        let model_of = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                workspace
                    .read(cx)
                    .repository_for_test()
                    .expect("repository open")
                    .read(cx)
                    .worktrees_model()
            })
        };
        let model = model_of(&mut cx);
        model.read_with(&cx, |model, _| {
            assert!(model.is_loaded());
            assert_eq!(model.check_branch("feat/new"), BranchCheck::New);
        });
        cx.update(|window, cx| {
            model.update(cx, |model, cx| model.create("feat/new", window, cx));
        });
        cx.run_until_parked();

        let expected = base.join("feat-new");
        assert!(expected.is_dir(), "the worktree folder was not created");
        let opened = model_of(&mut cx);
        let root = opened.read_with(&cx, |model, _| model.root().to_path_buf());
        match after {
            AfterCreate::Switch => assert_eq!(
                root.canonicalize().unwrap(),
                expected.canonicalize().unwrap(),
                "Switch should open the new worktree in this window"
            ),
            AfterCreate::Stay => {
                assert_eq!(root, repo.path());
                opened.read_with(&cx, |model, _| {
                    assert!(model
                        .entries()
                        .iter()
                        .any(|e| e.branch() == Some("feat/new")));
                    assert_eq!(model.selected().and_then(|e| e.branch()), Some("feat/new"));
                    assert!(!model.is_creating());
                });
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The add dialog's path: a worktree made outside the model is selected
    /// after the reload, and `switch_window_to` opens it in this window.
    #[gpui::test]
    fn a_worktree_added_elsewhere_is_selected_and_can_be_switched_to(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = TempRepo::new("model-added");
        commit(repo.path(), "a.txt", "one\n");
        run(repo.path(), &["branch", "extra"]);
        let linked = sibling(&repo, "added");
        let (mut cx, workspace) = open_workspace(git(), cx);
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |w, cx| w.open_repository(path.clone(), window, cx));
        });
        cx.run_until_parked();
        let model_of = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                workspace
                    .read(cx)
                    .repository_for_test()
                    .unwrap()
                    .read(cx)
                    .worktrees_model()
            })
        };
        let model = model_of(&mut cx);
        Repo::open(git(), repo.path())
            .unwrap()
            .add_worktree(&linked, "extra")
            .unwrap();
        cx.update(|_, cx| model.update(cx, |model, cx| model.operation_done(cx)));
        cx.run_until_parked();
        model.read_with(&cx, |model, _| {
            assert_eq!(model.selected().and_then(|e| e.branch()), Some("extra"));
        });

        let weak = workspace.downgrade();
        let target = linked.clone();
        cx.update(|window, cx| switch_window_to(&weak, target, window, cx));
        cx.run_until_parked();
        let opened = model_of(&mut cx);
        let root = opened.read_with(&cx, |model, _| model.root().to_path_buf());
        assert_eq!(root.canonicalize().unwrap(), linked.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&linked);
    }

    #[gpui::test]
    fn create_switches_to_the_new_worktree(cx: &mut gpui::TestAppContext) {
        create_flow(AfterCreate::Switch, cx);
    }

    #[gpui::test]
    fn create_can_stay_in_the_current_worktree(cx: &mut gpui::TestAppContext) {
        create_flow(AfterCreate::Stay, cx);
    }
}
