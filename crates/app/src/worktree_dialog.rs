//! The stage 9 dialogs: add a worktree (existing or new branch) and remove
//! one, with the locked, dirty and missing states of the design.
//!
//! One [`WorktreeDialog`] entity drives both kinds. Operations run the real
//! git in the background; on success the dialog closes, a toast reports it and
//! `on_done` refreshes the shell.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, FocusHandle, IntoElement,
    ParentElement, PathPromptOptions, Styled, Subscription, WeakEntity, Window,
};

use git_core::{Error, Git, Oid, Repo, Worktree};
use ui::metrics::{FONT_MONO, FS_SM};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    Icon, IconName, SegmentItem, Segmented, Select, TextInput, TextInputEvent, ToastKind,
};

use crate::format::{display_path, expand_home};
use crate::ref_ops::ref_name_error;
use crate::workspace::Workspace;

/// One local branch offered by the add dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchChoice {
    pub name: String,
    pub target: Oid,
}

/// The commit the history has selected, offered as a "Start from" option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedCommit {
    pub oid: Oid,
    pub short: String,
}

/// Everything the add dialog needs. Worktrees supply the branches already in
/// use and the paths that must not be reused.
pub(crate) struct WorktreeDialogContext {
    pub root: PathBuf,
    pub git: Result<Arc<Git>, Arc<Error>>,
    pub home: Option<PathBuf>,
    pub branches: Vec<BranchChoice>,
    pub worktrees: Vec<Worktree>,
    pub current_branch: Option<String>,
    pub selected: Option<SelectedCommit>,
    pub workspace: WeakEntity<Workspace>,
    /// The main line, for telling whether a branch is merged.
    pub main_line: Option<String>,
    /// Settings → Worktrees: ask again before deleting an unmerged branch.
    pub confirm_delete_branch: bool,
    /// The "After creating" setting is Switch: without the "new window" box
    /// the add dialog switches this window to the new worktree, like quick
    /// create does; otherwise it stays.
    pub open_after: bool,
}

/// A worktree as the remove dialog sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoveTarget {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub locked: Option<String>,
    pub missing: bool,
    pub main: bool,
    pub changes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Add,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BranchMode {
    Existing,
    New,
}

type DialogAction = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
pub(crate) type Done = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct WorktreeDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    mode: Mode,
    root: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    home: Option<PathBuf>,
    workspace: WeakEntity<Workspace>,
    branches: Vec<BranchChoice>,
    worktrees: Vec<Worktree>,
    selected: Option<SelectedCommit>,
    target: Option<RemoveTarget>,
    path: Option<Entity<TextInput>>,
    new_name: Option<Entity<TextInput>>,
    branch_mode: BranchMode,
    branch: usize,
    base: usize,
    /// The "Open the worktree in a new window" box (always starts unticked).
    open_after: bool,
    /// Switch this window to the new worktree when the box is not ticked.
    switch_after: bool,
    force: bool,
    delete_branch: bool,
    main_line: Option<String>,
    confirm_delete_branch: bool,
    /// Whether the branch to delete is unmerged; `None` until checked.
    unmerged: Option<bool>,
    /// The user confirmed deleting the unmerged branch.
    force_branch: bool,
    running: bool,
    unlocking: bool,
    /// Set once an input changed, so errors do not colour an untouched field.
    path_touched: bool,
    name_touched: bool,
    error: Option<String>,
    path_focus: FocusHandle,
    name_focus: FocusHandle,
    browse_focus: FocusHandle,
    open_focus: FocusHandle,
    unlock_focus: FocusHandle,
    force_focus: FocusHandle,
    delete_focus: FocusHandle,
    force_branch_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    on_done: Done,
}

impl WorktreeDialog {
    fn new_add(
        context: WorktreeDialogContext,
        on_done: Done,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let path = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_placeholder("~/develop/gitwarp-hotfix", cx);
            input
        });
        subscriptions.push(cx.subscribe(&path, |this, _, _: &TextInputEvent, cx| {
            this.path_touched = true;
            this.notify_host(cx);
            cx.notify();
        }));
        let new_name = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_placeholder("fix/my-change", cx);
            input
        });
        subscriptions.push(cx.subscribe(&new_name, |this, _, _: &TextInputEvent, cx| {
            this.name_touched = true;
            this.notify_host(cx);
            cx.notify();
        }));
        let branch = context
            .branches
            .iter()
            .position(|branch| !is_checked_out(&branch.name, &context.worktrees))
            .unwrap_or(0);
        let base = context
            .current_branch
            .as_ref()
            .and_then(|name| {
                context
                    .branches
                    .iter()
                    .position(|branch| &branch.name == name)
            })
            .unwrap_or(0);
        let path_focus = path.read(cx).focus_handle().clone();
        let name_focus = new_name.read(cx).focus_handle().clone();
        Self {
            layer: None,
            host: None,
            mode: Mode::Add,
            root: context.root,
            git: context.git,
            home: context.home,
            workspace: context.workspace,
            branches: context.branches,
            worktrees: context.worktrees,
            selected: context.selected,
            target: None,
            path: Some(path),
            new_name: Some(new_name),
            branch_mode: BranchMode::Existing,
            branch,
            base,
            open_after: false,
            switch_after: context.open_after,
            force: false,
            delete_branch: false,
            main_line: None,
            confirm_delete_branch: true,
            unmerged: None,
            force_branch: false,
            running: false,
            unlocking: false,
            path_touched: false,
            name_touched: false,
            error: None,
            path_focus,
            name_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            open_focus: cx.focus_handle().tab_stop(true),
            unlock_focus: cx.focus_handle().tab_stop(true),
            force_focus: cx.focus_handle().tab_stop(true),
            delete_focus: cx.focus_handle().tab_stop(true),
            force_branch_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
            on_done,
        }
    }

    fn new_remove(
        target: RemoveTarget,
        context: WorktreeDialogContext,
        on_done: Done,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            layer: None,
            host: None,
            mode: Mode::Remove,
            root: context.root,
            git: context.git,
            home: context.home,
            workspace: context.workspace,
            branches: Vec::new(),
            worktrees: Vec::new(),
            selected: None,
            target: Some(target),
            path: None,
            new_name: None,
            branch_mode: BranchMode::Existing,
            branch: 0,
            base: 0,
            open_after: false,
            switch_after: false,
            force: false,
            delete_branch: false,
            main_line: context.main_line,
            confirm_delete_branch: context.confirm_delete_branch,
            unmerged: None,
            force_branch: false,
            running: false,
            unlocking: false,
            path_touched: false,
            name_touched: false,
            error: None,
            path_focus: cx.focus_handle(),
            name_focus: cx.focus_handle(),
            browse_focus: cx.focus_handle(),
            open_focus: cx.focus_handle(),
            unlock_focus: cx.focus_handle().tab_stop(true),
            force_focus: cx.focus_handle().tab_stop(true),
            delete_focus: cx.focus_handle().tab_stop(true),
            force_branch_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: Vec::new(),
            on_done,
        }
    }

    fn path_text(&self, cx: &App) -> String {
        self.path
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    fn name_text(&self, cx: &App) -> String {
        self.new_name
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    /// The path typed in the folder field, expanded and made absolute the way
    /// git will see it from the repository root.
    fn resolved_path(&self, cx: &App) -> PathBuf {
        let raw = self.path_text(cx);
        let path = expand_home(&raw, self.home.as_deref());
        if path.is_relative() {
            self.root.join(path)
        } else {
            path
        }
    }

    fn path_error(&self, cx: &App) -> Option<&'static str> {
        let raw = self.path_text(cx);
        worktree_path_error(
            &raw,
            &self.root,
            &self
                .worktrees
                .iter()
                .map(|worktree| worktree.path.clone())
                .collect::<Vec<_>>(),
            self.home.as_deref(),
        )
    }

    fn branch_used(&self, name: &str) -> Option<&Worktree> {
        self.worktrees
            .iter()
            .find(|worktree| worktree.branch.as_deref() == Some(name))
    }

    fn name_error(&self, cx: &App) -> Option<String> {
        let name = self.name_text(cx);
        let taken: Vec<String> = self
            .branches
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        ref_name_error(&name, &taken)
    }

    fn base_options(&self) -> Vec<(String, Oid)> {
        let mut options: Vec<(String, Oid)> = self
            .branches
            .iter()
            .map(|branch| (branch.name.clone(), branch.target))
            .collect();
        if let Some(selected) = &self.selected {
            options.push((
                format!("{} — selected commit", selected.short),
                selected.oid,
            ));
        }
        options
    }

    fn base_oid(&self) -> Option<Oid> {
        self.base_options().get(self.base).map(|(_, oid)| *oid)
    }

    fn validity(&self, cx: &App) -> Validity {
        if self.mode == Mode::Remove {
            return Validity {
                enabled: self.remove_enabled(),
                hint: self.remove_hint(),
            };
        }
        let path = self.path_text(cx);
        let path_error = self.path_error(cx);
        let used = self
            .branches
            .get(self.branch)
            .filter(|_| self.branch_mode == BranchMode::Existing)
            .and_then(|branch| self.branch_used(&branch.name));
        let name = self.name_text(cx);
        let name_error = (self.branch_mode == BranchMode::New)
            .then(|| self.name_error(cx))
            .flatten();
        let enabled = !path.is_empty()
            && path_error.is_none()
            && used.is_none()
            && (self.branch_mode == BranchMode::Existing
                || (!name.is_empty() && name_error.is_none() && self.base_oid().is_some()))
            && (self.branch_mode == BranchMode::New || self.branches.get(self.branch).is_some())
            && !self.running;
        let hint = if path.is_empty() {
            Some("Choose a folder for the worktree")
        } else if path_error.is_some() {
            Some("Fix the folder to continue")
        } else if self.branch_mode == BranchMode::Existing
            && self.branches.get(self.branch).is_none()
        {
            Some("Choose a branch to check out")
        } else if used.is_some() {
            Some("Choose a branch that is not in use")
        } else if self.branch_mode == BranchMode::New && name.is_empty() {
            Some("Enter a branch name")
        } else if name_error.is_some() {
            Some("Fix the name to continue")
        } else if self.branch_mode == BranchMode::New && self.base_oid().is_none() {
            Some("Choose a start commit")
        } else {
            None
        };
        Validity { enabled, hint }
    }

    fn remove_target(&self) -> Option<&RemoveTarget> {
        self.target.as_ref()
    }

    fn remove_enabled(&self) -> bool {
        self.remove_target().is_some_and(|target| {
            !target.main
                && target.locked.is_none()
                && (!dirty(target) || self.force)
                && !self.branch_confirm_pending()
                && !self.running
                && !self.unlocking
        })
    }

    /// Deleting an unmerged branch asks once more (when the setting says so).
    fn needs_branch_confirm(&self) -> bool {
        self.delete_branch && self.confirm_delete_branch && self.unmerged == Some(true)
    }

    fn branch_confirm_pending(&self) -> bool {
        self.needs_branch_confirm() && !self.force_branch
    }

    /// Whether the branch is deleted even though it is unmerged: confirmed,
    /// or the user turned the confirmation off.
    fn force_delete_branch(&self) -> bool {
        self.unmerged == Some(true) && (self.force_branch || !self.confirm_delete_branch)
    }

    /// Finds out in the background whether `branch` is merged into the main
    /// line, so ticking "Also delete branch" can ask again for an unmerged one.
    fn check_branch_merged(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self.target.as_ref().and_then(|t| t.branch.clone()) else {
            return;
        };
        let Ok(git) = self.git.clone() else {
            return;
        };
        let root = self.root.clone();
        let main = self.main_line.clone();
        cx.spawn(async move |this, cx| {
            let unmerged = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    let log = repo.branch_log(&branch, main.as_deref(), 1)?;
                    let on_main = log
                        .fork_point
                        .is_some_and(|fork| log.commits.first().is_some_and(|c| c.id == fork));
                    Ok::<_, Error>(log.merged_into.is_none() && !on_main)
                })
                .await
                // When the state cannot be read, ask: deleting is the risk.
                .unwrap_or(true);
            this.update(cx, |dialog, cx| {
                dialog.unmerged = Some(unmerged);
                dialog.notify_host(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_hint(&self) -> Option<&'static str> {
        let target = self.remove_target()?;
        if target.main {
            Some("Not available for the main worktree")
        } else if target.locked.is_some() {
            Some("Unlock the worktree first")
        } else if dirty(target) && !self.force {
            Some("Confirm to discard changes")
        } else if self.branch_confirm_pending() {
            Some("Confirm deleting the unmerged branch")
        } else {
            None
        }
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    /// Runs the dialog's operation off the UI thread.
    fn start(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running || !self.validity(cx).enabled {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let root = self.root.clone();
        let operation: Operation = match self.mode {
            Mode::Add => {
                let path = self.resolved_path(cx);
                let branch_mode = self.branch_mode;
                let branch = self
                    .branches
                    .get(self.branch)
                    .map(|branch| branch.name.clone())
                    .unwrap_or_default();
                let new_name = self.name_text(cx);
                let base = self.base_oid();
                Box::new(move |repo: &Repo| match branch_mode {
                    BranchMode::Existing => {
                        repo.add_worktree(&path, &branch)?;
                        Ok(format!("Added worktree {} on {branch}", path.display()))
                    }
                    BranchMode::New => {
                        let Some(base) = base else {
                            return Err(Error::Parse("start commit"));
                        };
                        repo.add_worktree_for_new_branch(&path, &new_name, base)?;
                        Ok(format!("Added worktree {} on {new_name}", path.display()))
                    }
                })
            }
            Mode::Remove => {
                let Some(target) = self.target.clone() else {
                    return;
                };
                let force = dirty(&target) && self.force;
                let delete_branch = self.delete_branch;
                let force_branch = self.force_delete_branch();
                Box::new(move |repo: &Repo| {
                    repo.remove_worktree(&target.path, force)?;
                    let mut notice = format!("Removed worktree {}", target.path.display());
                    if delete_branch {
                        if let Some(branch) = &target.branch {
                            // The worktree is gone already: a branch that is
                            // not fully merged stays unless the user confirmed
                            // (or switched the confirmation off), and the toast
                            // says so instead of leaving the dialog on a stale path.
                            if let Err(error) = repo.delete_branch(branch, force_branch) {
                                notice = format!(
                                    "Removed worktree {}; kept {branch}: {}",
                                    target.path.display(),
                                    error
                                );
                            }
                        }
                    }
                    Ok(notice)
                })
            }
        };
        self.running = true;
        self.error = None;
        self.notify_host(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    operation(&repo)
                })
                .await;
            this.update_in(cx, |dialog, window, cx| dialog.finish(result, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        result: std::result::Result<String, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        match result {
            Ok(notice) => {
                let added = (self.mode == Mode::Add).then(|| self.resolved_path(cx));
                let open_path = added.clone().filter(|_| self.open_after);
                let switch_path = added.filter(|_| !self.open_after && self.switch_after);
                self.close(window, cx);
                toast::show(ToastKind::Ok, notice, window, cx);
                let on_done = self.on_done.clone();
                on_done(window, cx);
                if let Some(path) = open_path {
                    self.workspace
                        .update(cx, |workspace, cx| {
                            workspace.open_repository_window(path, window, cx);
                        })
                        .ok();
                } else if let Some(path) = switch_path {
                    crate::worktrees_model::switch_window_to(&self.workspace, path, window, cx);
                }
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.notify_host(cx);
                cx.notify();
            }
        }
    }

    /// The remove dialog's Unlock button: unlocks in place and re-renders the
    /// dialog, so "Remove worktree" becomes available without reopening it.
    fn unlock(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.unlocking || self.running {
            return;
        }
        let Some(path) = self.target.as_ref().map(|target| target.path.clone()) else {
            return;
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let root = self.root.clone();
        self.unlocking = true;
        self.error = None;
        self.notify_host(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    repo.unlock_worktree(&path)
                })
                .await;
            this.update_in(cx, |dialog, window, cx| {
                dialog.unlocking = false;
                match result {
                    Ok(()) => {
                        if let Some(target) = &mut dialog.target {
                            target.locked = None;
                        }
                        toast::show(ToastKind::Ok, "Worktree unlocked", window, cx);
                        let on_done = dialog.on_done.clone();
                        on_done(window, cx);
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                dialog.notify_host(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

type Operation = Box<dyn FnOnce(&Repo) -> git_core::Result<String> + Send>;

struct Validity {
    enabled: bool,
    hint: Option<&'static str>,
}

fn dirty(target: &RemoveTarget) -> bool {
    !target.changes.is_empty()
}

fn is_checked_out(branch: &str, worktrees: &[Worktree]) -> bool {
    worktrees
        .iter()
        .any(|worktree| worktree.branch.as_deref() == Some(branch))
}

/// Why `input` cannot be a new worktree folder, or `None` when nothing is
/// wrong. Empty input is "not filled in yet", not an error.
pub(crate) fn worktree_path_error(
    input: &str,
    root: &Path,
    worktrees: &[PathBuf],
    home: Option<&Path>,
) -> Option<&'static str> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    let path = expand_home(raw, home);
    let path = if path.is_relative() {
        root.join(path)
    } else {
        path
    };
    let path = trim_trailing_separators(&path);
    if worktrees.iter().any(|worktree| same_path(&path, worktree)) {
        return Some("A worktree already exists at this path.");
    }
    if path.starts_with(root) {
        return Some("This path is inside the repository. Choose a sibling folder instead.");
    }
    if path.exists() && !path.is_dir() {
        return Some("This path is not a folder.");
    }
    None
}

fn trim_trailing_separators(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(trimmed)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Opens the add-worktree dialog and keeps its state in the shell.
pub(crate) fn open_add<T: 'static>(
    context: WorktreeDialogContext,
    on_done: Done,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<WorktreeDialog> {
    let dialog = cx.new(|cx| WorktreeDialog::new_add(context, on_done, window, cx));
    finish_open(dialog, window, cx)
}

/// Opens the remove-worktree dialog for `target`.
pub(crate) fn open_remove<T: 'static>(
    target: RemoveTarget,
    context: WorktreeDialogContext,
    on_done: Done,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<WorktreeDialog> {
    let dialog = cx.new(|cx| WorktreeDialog::new_remove(target, context, on_done, cx));
    finish_open(dialog, window, cx)
}

fn finish_open<T: 'static>(
    dialog: Entity<WorktreeDialog>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<WorktreeDialog> {
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Worktree").icon(IconName::Worktree),
    });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
        cx.notify();
    });
    dialog
}

fn build(state: &Entity<WorktreeDialog>, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let valid = dialog.validity(cx);
    let (title, icon, width) = match dialog.mode {
        Mode::Add => ("Add worktree", IconName::Worktree, 520.),
        Mode::Remove => ("Remove worktree", IconName::Trash, 500.),
    };

    let mut body: AnyElement = match dialog.mode {
        Mode::Add => add_body(state, dialog, colors, cx),
        Mode::Remove => remove_body(state, dialog, colors),
    };
    if let Some(error) = &dialog.error {
        body = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(body)
            .child(Banner::new(BannerKind::Error, "The operation failed").body(error.clone()))
            .into_any_element();
    }

    let cancel = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }) as DialogAction
    };
    let primary = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(window, cx));
        }) as DialogAction
    };

    let primary_label = match dialog.mode {
        Mode::Add => "Add worktree",
        Mode::Remove => {
            let dirty = dialog.remove_target().is_some_and(dirty);
            if dirty && dialog.force {
                "Discard and remove"
            } else {
                "Remove worktree"
            }
        }
    };
    let dangerous = dialog.mode == Mode::Remove;
    let mut primary_button = if dangerous {
        Button::danger("worktree-primary")
    } else {
        Button::primary("worktree-primary")
    };
    primary_button = primary_button
        .label(primary_label)
        .min_width(px(96.))
        .disabled(!valid.enabled)
        .focus_handle(dialog.primary_focus.clone())
        .on_click(primary);
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("worktree-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .disabled(dialog.running || dialog.unlocking)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(primary_button);

    let mut focus_order = Vec::new();
    match dialog.mode {
        Mode::Add => {
            focus_order.push(DialogFocus::input(dialog.path_focus.clone()));
            if dialog.branch_mode == BranchMode::New {
                focus_order.push(DialogFocus::input(dialog.name_focus.clone()));
            }
            focus_order.push(DialogFocus::control(dialog.browse_focus.clone()));
            focus_order.push(DialogFocus::control(dialog.open_focus.clone()));
        }
        Mode::Remove => {
            if dialog
                .remove_target()
                .is_some_and(|target| target.locked.is_some())
            {
                focus_order.push(DialogFocus::control(dialog.unlock_focus.clone()));
            }
            if dialog.remove_target().is_some_and(dirty) {
                focus_order.push(DialogFocus::control(dialog.force_focus.clone()));
            }
            if dialog
                .remove_target()
                .is_some_and(|target| target.branch.is_some())
            {
                focus_order.push(DialogFocus::control(dialog.delete_focus.clone()));
            }
            if dialog.needs_branch_confirm() {
                focus_order.push(DialogFocus::control(dialog.force_branch_focus.clone()));
            }
        }
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let mut panel = Dialog::new(title)
        .icon(icon)
        .width(px(width))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(dialog.running || dialog.unlocking)
        .focus_order(focus_order)
        .default_button(dialog.primary_focus.clone())
        .default_danger(dangerous);
    if dialog.mode == Mode::Add {
        let state = state.clone();
        panel = panel
            .autofocus(dialog.path_focus.clone())
            .on_default(move |window, cx| {
                state.update(cx, |dialog, cx| dialog.start(window, cx));
            });
    }
    if let Some(hint) = valid.hint {
        panel = panel.hint(hint);
    } else if dialog.unlocking {
        panel = panel.hint("Unlocking…");
    } else if dialog.running {
        panel = panel.hint("Working…");
    }
    panel
}

fn add_body(
    state: &Entity<WorktreeDialog>,
    dialog: &WorktreeDialog,
    colors: ThemeColors,
    cx: &App,
) -> AnyElement {
    let path_error = dialog.path_touched.then(|| dialog.path_error(cx)).flatten();
    let browse = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        }) as DialogAction
    };
    let branch_options: Vec<String> = dialog
        .branches
        .iter()
        .map(|branch| match dialog.branch_used(&branch.name) {
            Some(worktree) => format!(
                "{}  — checked out in {}",
                branch.name,
                display_path(&worktree.path, dialog.home.as_deref())
            ),
            None => branch.name.clone(),
        })
        .collect();
    let branch_listener = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.branch = index;
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };
    let mode_listener = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.branch_mode = if index == 1 {
                    BranchMode::New
                } else {
                    BranchMode::Existing
                };
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };
    let base_options: Vec<String> = dialog
        .base_options()
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    let base_listener = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.base = index;
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };
    let open_toggle = {
        let state = state.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.open_after = next;
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };

    let used = dialog
        .branches
        .get(dialog.branch)
        .filter(|_| dialog.branch_mode == BranchMode::Existing)
        .and_then(|branch| dialog.branch_used(&branch.name));
    let branch_error = used.map(|worktree| {
        format!(
            "“{}” is already checked out in {}. A branch can only be open in one worktree.",
            dialog.branches[dialog.branch].name,
            display_path(&worktree.path, dialog.home.as_deref())
        )
    });
    let name_error = dialog.name_touched.then(|| dialog.name_error(cx)).flatten();

    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(
                    "A worktree is a second working folder for this repository, so you can work on another branch without stashing.",
                ),
        )
        .child(
            field(
                "Folder",
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().flex_1().min_w_0().child(
                            dialog
                                .path
                                .clone()
                                .map(IntoElement::into_any_element)
                                .unwrap_or_else(|| div().into_any_element()),
                        ),
                    )
                    .child(
                        Button::new("worktree-browse")
                            .label("Browse…")
                            .icon(IconName::Folder)
                            .focus_handle(dialog.browse_focus.clone())
                            .on_click(browse),
                    )
                    .into_any_element(),
                path_error,
                Some("Pick a folder outside the repository. It is created if it does not exist."),
                true,
                colors,
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child("Branch"),
                )
                .child(
                    Segmented::new("worktree-mode")
                        .item(SegmentItem::new("Existing branch"))
                        .item(SegmentItem::new("New branch"))
                        .selected(if dialog.branch_mode == BranchMode::New {
                            1
                        } else {
                            0
                        })
                        .on_select(mode_listener),
                ),
        );
    if dialog.branch_mode == BranchMode::Existing {
        body = body.child(field(
            "Check out",
            div()
                .child(
                    Select::new("worktree-branch")
                        .options(branch_options)
                        .selected(dialog.branch)
                        .disabled(dialog.branches.is_empty())
                        .on_change(branch_listener),
                )
                .into_any_element(),
            branch_error.as_deref(),
            None,
            false,
            colors,
        ));
    } else {
        body = body.child(
            div()
                .flex()
                .gap(px(12.))
                .child(
                    div().flex_1().min_w_0().child(field(
                        "New branch name",
                        dialog
                            .new_name
                            .clone()
                            .map(IntoElement::into_any_element)
                            .unwrap_or_else(|| div().into_any_element()),
                        name_error.as_deref(),
                        None,
                        true,
                        colors,
                    )),
                )
                .child(
                    div().flex_1().min_w_0().child(field(
                        "Start from",
                        div()
                            .child(
                                Select::new("worktree-base")
                                    .options(base_options)
                                    .selected(dialog.base)
                                    .disabled(dialog.base_options().is_empty())
                                    .on_change(base_listener),
                            )
                            .into_any_element(),
                        None,
                        None,
                        false,
                        colors,
                    )),
                ),
        );
    }
    body = body.child(
        Checkbox::new("worktree-open")
            .label("Open the worktree in a new window")
            .checked(dialog.open_after)
            .focus_handle(dialog.open_focus.clone())
            .on_toggle(open_toggle),
    );
    body.into_any_element()
}

fn remove_body(
    state: &Entity<WorktreeDialog>,
    dialog: &WorktreeDialog,
    colors: ThemeColors,
) -> AnyElement {
    let Some(target) = dialog.remove_target() else {
        return div().into_any_element();
    };
    let mut body = div().flex().flex_col().gap(px(14.));
    if target.main {
        return body
            .child(
                Banner::new(BannerKind::Error, "This is the main worktree").body(
                    "It holds the repository itself and cannot be removed. Close the repository instead if you are done with it.",
                ),
            )
            .into_any_element();
    }
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(kv("Folder", target.path.display().to_string(), colors))
            .child(kv(
                "Branch",
                target
                    .branch
                    .clone()
                    .unwrap_or_else(|| "detached".to_owned()),
                colors,
            )),
    );
    if target.missing {
        body = body.child(
            Banner::new(BannerKind::Info, "The folder no longer exists on disk").body(
                "Git still lists this worktree. Removing it only cleans up the record — no files are touched.",
            ),
        );
    }
    if let Some(reason) = &target.locked {
        let reason = if reason.is_empty() {
            "review in progress"
        } else {
            reason
        };
        let unlock = {
            let state = state.clone();
            Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| dialog.unlock(window, cx));
            }) as DialogAction
        };
        body = body
            .child(
                Banner::new(BannerKind::Warn, "This worktree is locked").body(format!(
                    "Locked with reason “{reason}”. Unlock it before removing."
                )),
            )
            .child(
                Button::new("worktree-unlock")
                    .label("Unlock")
                    .icon(IconName::Lock)
                    .focus_handle(dialog.unlock_focus.clone())
                    .disabled(dialog.unlocking)
                    .on_click(unlock),
            );
    }
    if !target.changes.is_empty() {
        let force_toggle = {
            let state = state.clone();
            move |next: bool, _: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| {
                    dialog.force = next;
                    dialog.notify_host(cx);
                    cx.notify();
                });
            }
        };
        body = body
            .child(
                Banner::new(
                    BannerKind::Warn,
                    format!(
                        "{} file{} have uncommitted changes",
                        target.changes.len(),
                        if target.changes.len() == 1 { "" } else { "s" }
                    ),
                )
                .body("Removing the worktree discards them for good.")
                .list(target.changes.clone()),
            )
            .child(
                Checkbox::new("worktree-force")
                    .label("Discard these changes and remove the worktree")
                    .checked(dialog.force)
                    .focus_handle(dialog.force_focus.clone())
                    .on_toggle(force_toggle),
            );
    } else if !target.missing && target.locked.is_none() {
        body = body.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(format!(
                    "The folder is deleted from disk. The branch {} and its commits stay in the repository.",
                    target.branch.clone().unwrap_or_else(|| "HEAD".to_owned())
                )),
        );
    }
    if let Some(branch) = &target.branch {
        let delete_toggle = {
            let state = state.clone();
            move |next: bool, _: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| {
                    dialog.delete_branch = next;
                    if !next {
                        dialog.force_branch = false;
                    } else if dialog.unmerged.is_none() {
                        dialog.check_branch_merged(cx);
                    }
                    dialog.notify_host(cx);
                    cx.notify();
                });
            }
        };
        body = body.child(
            Checkbox::new("worktree-delete-branch")
                .label(format!("Also delete branch {branch}"))
                .checked(dialog.delete_branch)
                .focus_handle(dialog.delete_focus.clone())
                .on_toggle(delete_toggle),
        );
        if dialog.needs_branch_confirm() {
            let force_toggle = {
                let state = state.clone();
                move |next: bool, _: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| {
                        dialog.force_branch = next;
                        dialog.notify_host(cx);
                        cx.notify();
                    });
                }
            };
            body = body
                .child(
                    Banner::new(BannerKind::Warn, format!("{branch} is not merged"))
                        .body("Unmerged commits stay reachable from the reflog for 30 days."),
                )
                .child(
                    Checkbox::new("worktree-force-branch")
                        .label(format!("Delete {branch} anyway"))
                        .checked(dialog.force_branch)
                        .focus_handle(dialog.force_branch_focus.clone())
                        .on_toggle(force_toggle),
                );
        }
    }
    body.into_any_element()
}

fn kv(label: &'static str, value: String, colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(FONT_MONO)
                .text_size(FS_SM)
                .child(value),
        )
}

fn field(
    label: &'static str,
    control: AnyElement,
    error: Option<&str>,
    help: Option<&str>,
    required: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    let mut field = div().flex().flex_col().gap(px(4.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(label)
            .when(required, |this| {
                this.child(div().text_size(FS_SM).text_color(colors.error).child("*"))
            }),
    );
    field = field.child(control);
    if let Some(error) = error {
        field = field.child(
            div()
                .flex()
                .items_start()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(error.to_owned()),
                ),
        );
    } else if let Some(help) = help {
        field = field.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .whitespace_normal()
                .child(help.to_owned()),
        );
    }
    field
}

impl WorktreeDialog {
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a folder for the worktree".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => Ok(paths.into_iter().next()),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            this.update_in(cx, |dialog, window, cx| match picked {
                Ok(Some(path)) => {
                    let label = display_path(&path, home.as_deref());
                    if let Some(input) = &dialog.path {
                        input.update(cx, |input, cx| input.set_text(label, cx));
                    }
                    dialog.path_touched = true;
                    dialog.notify_host(cx);
                    cx.notify();
                }
                Ok(None) => {}
                Err(message) => {
                    toast::show(
                        ToastKind::Error,
                        format!("Could not open the folder picker: {message}"),
                        window,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/mira")
    }

    #[test]
    fn rejects_paths_that_are_worktrees_or_inside_the_repository() {
        let root = Path::new("/home/mira/develop/gitwarp");
        let worktrees = vec![
            root.to_path_buf(),
            PathBuf::from("/home/mira/develop/gitwarp-hotfix"),
        ];
        assert_eq!(
            worktree_path_error(
                "/home/mira/develop/gitwarp",
                root,
                &worktrees,
                Some(&home())
            ),
            Some("A worktree already exists at this path.")
        );
        assert_eq!(
            worktree_path_error(
                "/home/mira/develop/gitwarp/tmp",
                root,
                &worktrees,
                Some(&home())
            ),
            Some("This path is inside the repository. Choose a sibling folder instead.")
        );
        assert_eq!(
            worktree_path_error("~/develop/gitwarp-review", root, &worktrees, Some(&home())),
            None
        );
        assert_eq!(
            worktree_path_error("~/develop/gitwarp-", root, &worktrees, Some(&home())),
            None
        );
    }

    #[test]
    fn empty_path_is_not_an_error() {
        assert_eq!(
            worktree_path_error("", Path::new("/repo"), &[], Some(&home())),
            None
        );
        assert_eq!(
            worktree_path_error("   ", Path::new("/repo"), &[], Some(&home())),
            None
        );
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_repository() {
        assert_eq!(
            worktree_path_error("tmp", Path::new("/repo"), &[], None),
            Some("This path is inside the repository. Choose a sibling folder instead.")
        );
    }

    #[test]
    fn the_main_worktree_cannot_be_removed() {
        let target = RemoveTarget {
            path: PathBuf::from("/repo"),
            branch: Some("main".to_owned()),
            locked: None,
            missing: false,
            main: true,
            changes: Vec::new(),
        };
        assert!(target.main);
    }
}
