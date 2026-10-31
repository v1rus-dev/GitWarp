//! The Worktrees page: the repository's working folders, their checked-out
//! branches and local changes, with add / remove / lock / prune.
//!
//! The page loads off the UI thread and is opened from the Worktrees section's
//! context menu, a worktree leaf's double-click, or the repository menu.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window,
};

use git_core::{Error, Git, Head, RefKind, Repo, Worktree};
use ui::metrics::{FONT_MONO, FS_BASE, FS_LG, FS_SM, FS_XS, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    toast, Banner, BannerKind, Button, ButtonSize, Chip, ChipKind, Icon, IconName, Spinner,
    ToastKind,
};

use crate::format::{display_path, home_dir};
use crate::workspace::Workspace;
use crate::worktree_dialog::{
    self, BranchChoice, Done, RemoveTarget, SelectedCommit, WorktreeDialog, WorktreeDialogContext,
};

/// One worktree plus the local changes found in its folder.
#[derive(Clone, Debug)]
pub struct WorktreeEntry {
    pub worktree: Worktree,
    pub changes: Vec<String>,
    pub missing: bool,
}

impl WorktreeEntry {
    #[cfg(test)]
    pub fn main(&self) -> bool {
        self.worktree.main
    }

    #[cfg(test)]
    pub fn locked(&self) -> bool {
        self.worktree.locked.is_some()
    }
}

/// Emitted after anything that can change the worktree list or a branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreesEvent {
    Changed,
}

pub struct WorktreesView {
    root: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    home: Option<PathBuf>,
    entries: Vec<WorktreeEntry>,
    branches: Vec<BranchChoice>,
    current_branch: Option<String>,
    selected: Option<SelectedCommit>,
    loaded: bool,
    loading: bool,
    generation: u64,
    error: Option<String>,
    focus: FocusHandle,
    dialog: Option<Entity<WorktreeDialog>>,
}

struct Loaded {
    entries: Vec<WorktreeEntry>,
    branches: Vec<BranchChoice>,
    current_branch: Option<String>,
}

impl WorktreesView {
    pub(crate) fn new(
        root: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            root,
            git,
            workspace,
            home: home_dir(),
            entries: Vec::new(),
            branches: Vec::new(),
            current_branch: None,
            selected: None,
            loaded: false,
            loading: false,
            generation: 0,
            error: None,
            focus: cx.focus_handle().tab_stop(false),
            dialog: None,
        };
        view.refresh(cx);
        view
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The selected history commit, offered as a "Start from" option.
    pub(crate) fn set_selected(
        &mut self,
        selected: Option<SelectedCommit>,
        cx: &mut Context<Self>,
    ) {
        if self.selected == selected {
            return;
        }
        self.selected = selected;
        cx.notify();
    }

    /// Adopts the repository root the history resolved (it differs when the
    /// repository was opened from a subdirectory) and reads the worktrees
    /// again, because the path validation depends on it.
    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root;
        self.refresh(cx);
    }

    /// Reads the worktree list, the branches and every worktree's changes off
    /// the UI thread.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.loaded = true;
                self.loading = false;
                self.error = Some(error.to_string());
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
            this.update(cx, |view, cx| view.set_loaded(generation, result, cx))
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
                self.entries = loaded.entries;
                self.branches = loaded.branches;
                self.current_branch = loaded.current_branch;
            }
            Err(error) => {
                self.error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn dialog_context(&self) -> WorktreeDialogContext {
        WorktreeDialogContext {
            root: self.root.clone(),
            git: self.git.clone(),
            home: self.home.clone(),
            branches: self.branches.clone(),
            worktrees: self
                .entries
                .iter()
                .map(|entry| entry.worktree.clone())
                .collect(),
            current_branch: self.current_branch.clone(),
            selected: self.selected.clone(),
            workspace: self.workspace.clone(),
        }
    }

    fn changed_callback(&self, cx: &Context<Self>) -> Done {
        let view = cx.entity();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let _ = window;
            view.update(cx, |view, cx| {
                view.refresh(cx);
                cx.emit(WorktreesEvent::Changed);
            });
        })
    }

    fn close_previous_dialog(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(previous) = self.dialog.take() {
            previous.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    }

    pub(crate) fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded || self.loading && self.entries.is_empty() {
            toast::show(
                ToastKind::Info,
                "The worktree list is still loading",
                window,
                cx,
            );
            self.refresh(cx);
            return;
        }
        self.close_previous_dialog(window, cx);
        let context = self.dialog_context();
        let on_done = self.changed_callback(cx);
        self.dialog = Some(worktree_dialog::open_add(context, on_done, window, cx));
    }

    pub(crate) fn open_remove(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.worktree.path == path)
            .cloned()
        else {
            toast::show(
                ToastKind::Warn,
                "The worktree is no longer in the list",
                window,
                cx,
            );
            self.refresh(cx);
            return;
        };
        self.close_previous_dialog(window, cx);
        let target = RemoveTarget {
            path: entry.worktree.path.clone(),
            branch: entry.worktree.branch.clone(),
            locked: entry.worktree.locked.clone(),
            missing: entry.missing,
            main: entry.worktree.main,
            changes: entry.changes.clone(),
        };
        let context = self.dialog_context();
        let on_done = self.changed_callback(cx);
        self.dialog = Some(worktree_dialog::open_remove(
            target, context, on_done, window, cx,
        ));
    }

    /// The row's lock button: locks with a manual reason or unlocks again.
    pub(crate) fn toggle_lock(
        &mut self,
        path: PathBuf,
        locked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let root = self.root.clone();
        let label = display_path(&path, self.home.as_deref());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    if locked {
                        repo.unlock_worktree(&path)?;
                    } else {
                        repo.lock_worktree(&path, Some("locked manually"))?;
                    }
                    Ok::<_, Error>(())
                })
                .await;
            this.update_in(cx, |view, window, cx| match result {
                Ok(()) => {
                    toast::show(
                        ToastKind::Ok,
                        if locked {
                            format!("Unlocked {label}")
                        } else {
                            format!("Locked {label}")
                        },
                        window,
                        cx,
                    );
                    view.refresh(cx);
                    cx.emit(WorktreesEvent::Changed);
                }
                Err(error) => {
                    toast::show(ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// "Prune N missing": drops the records whose folders are gone.
    pub(crate) fn prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    repo.prune_worktrees()
                })
                .await;
            this.update_in(cx, |view, window, cx| match result {
                Ok(()) => {
                    toast::show(ToastKind::Ok, "Pruned stale worktree records", window, cx);
                    view.refresh(cx);
                    cx.emit(WorktreesEvent::Changed);
                }
                Err(error) => {
                    toast::show(ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_window(&self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let label = display_path(&path, self.home.as_deref());
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_repository_window(path, window, cx);
            })
            .ok();
        toast::show(
            ToastKind::Ok,
            format!("Opened {label} in a new window"),
            window,
            cx,
        );
    }
}

impl EventEmitter<WorktreesEvent> for WorktreesView {}

impl Render for WorktreesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let count = self.entries.len();
        let missing = self.entries.iter().filter(|entry| entry.missing).count();

        let add = {
            let view = cx.entity();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_add(window, cx));
            }
        };
        let prune = {
            let view = cx.entity();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.prune(window, cx));
            }
        };

        let mut header = div().flex().items_center().gap(px(8.)).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(FS_LG)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Worktrees"),
                )
                .child(
                    div()
                        .text_size(FS_BASE)
                        .text_color(colors.muted)
                        .child(count.to_string()),
                ),
        );
        if missing > 0 {
            header = header.child(
                Button::new("worktrees-prune")
                    .label(format!("Prune {missing} missing"))
                    .icon(IconName::Trash)
                    .on_click(prune),
            );
        }
        header = header.child(
            Button::primary("worktrees-add")
                .label("Add worktree…")
                .icon(IconName::Plus)
                .on_click(add),
        );

        let mut list = div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .bg(colors.bg)
            .overflow_hidden()
            .child(table_header(colors));
        if self.loading && self.entries.is_empty() {
            list = list.child(
                div()
                    .h(px(80.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(Spinner::new("worktrees-loading", px(14.)))
                    .child("Loading worktrees…"),
            );
        } else {
            for (index, entry) in self.entries.iter().enumerate() {
                list = list.child(self.row(index, entry, cx));
            }
        }

        let mut content = div()
            .id("worktrees-view")
            .debug_selector(|| "worktrees-view".to_string())
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(20.))
            .py(px(16.))
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .child(header)
            .child(list);
        if let Some(error) = &self.error {
            content = content.child(
                Banner::new(BannerKind::Error, "Could not read the worktrees").body(error.clone()),
            );
        }
        content.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child("Each worktree is its own folder with its own checked-out branch. A branch can only be open in one worktree at a time."),
        )
    }
}

impl WorktreesView {
    fn row(&self, index: usize, entry: &WorktreeEntry, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let worktree = &entry.worktree;
        let label = display_path(&worktree.path, self.home.as_deref());
        let missing = entry.missing;
        let locked = worktree.locked.is_some();
        let main = worktree.main;

        let open = {
            let view = cx.entity();
            let path = worktree.path.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| {
                    view.open_window(path.clone(), window, cx);
                });
            }
        };
        let lock = {
            let view = cx.entity();
            let path = worktree.path.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| {
                    view.toggle_lock(path.clone(), locked, window, cx)
                });
            }
        };
        let remove = {
            let view = cx.entity();
            let path = worktree.path.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_remove(&path, window, cx));
            }
        };

        let folder = {
            let mut folder = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .when(missing, |this| this.line_through().text_color(colors.muted))
                        .child(label),
                );
            if let Some(reason) = &worktree.locked {
                folder = folder.child(
                    div()
                        .truncate()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(if reason.is_empty() {
                            "Locked".to_owned()
                        } else {
                            format!("Locked: {reason}")
                        }),
                );
            }
            folder
        };

        let head = worktree.head.map(|oid| {
            let mut short = oid.to_string();
            short.truncate(7);
            short
        });

        let status = status_chips(index, entry);

        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                Button::new(format!("worktree-open-{index}"))
                    .label("Open")
                    .size(ButtonSize::Small)
                    .disabled(missing)
                    .on_click(open),
            )
            .child(
                Button::ghost(format!("worktree-lock-{index}"))
                    .icon_only(IconName::Lock)
                    .size(ButtonSize::Small)
                    .tooltip(if locked { "Unlock" } else { "Lock" })
                    .disabled(main)
                    .on_click(lock),
            )
            .child(
                Button::ghost(format!("worktree-remove-{index}"))
                    .icon_only(IconName::Trash)
                    .size(ButtonSize::Small)
                    .tooltip("Remove worktree…")
                    .on_click(remove),
            );

        div()
            .id(SharedString::from(format!("worktree-row-{index}")))
            .debug_selector(move || format!("worktree-row-{index}"))
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(52.))
            .px(px(12.))
            .py(px(6.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div().w(px(20.)).flex_none().child(
                    Icon::new(IconName::Worktree)
                        .size(px(16.))
                        .color(if missing { colors.muted } else { colors.text }),
                ),
            )
            .child(folder)
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(1.)
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .child(
                        worktree
                            .branch
                            .clone()
                            .unwrap_or_else(|| "detached".to_owned()),
                    ),
            )
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(head.unwrap_or_default()),
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(1.)
                    .min_w_0()
                    .child(status),
            )
            .child(actions)
            .into_any_element()
    }
}

fn table_header(colors: ThemeColors) -> AnyElement {
    let cell = |text: &'static str| {
        div()
            .text_size(FS_XS)
            .text_color(colors.muted)
            .child(text.to_uppercase())
    };
    div()
        .w_full()
        .h(px(28.))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .bg(colors.surface)
        .border_b_1()
        .border_color(colors.border)
        .child(div().w(px(20.)).flex_none())
        .child(cell("Folder").flex_1().min_w_0())
        .child(cell("Branch").flex_basis(px(0.)).flex_grow(1.).min_w_0())
        .child(cell("HEAD").w(px(84.)).flex_none())
        .child(cell("Status").flex_basis(px(0.)).flex_grow(1.).min_w_0())
        .child(div().w(px(128.)).flex_none())
        .into_any_element()
}

fn status_chips(index: usize, entry: &WorktreeEntry) -> AnyElement {
    let worktree = &entry.worktree;
    let mut row = div().flex().items_center().gap(px(4.));
    if worktree.main {
        row = row.child(
            Chip::new(format!("wt-{index}-main"), "main worktree")
                .kind(ChipKind::Acc)
                .icon(IconName::Git),
        );
    }
    if worktree.locked.is_some() {
        row = row.child(
            Chip::new(format!("wt-{index}-locked"), "locked")
                .kind(ChipKind::Warn)
                .icon(IconName::Lock),
        );
    }
    if !entry.changes.is_empty() {
        row = row.child(
            Chip::new(
                format!("wt-{index}-dirty"),
                format!("{} changed", entry.changes.len()),
            )
            .kind(ChipKind::Warn)
            .icon(IconName::Edit),
        );
    }
    if entry.missing {
        row = row.child(
            Chip::new(format!("wt-{index}-missing"), "folder missing")
                .kind(ChipKind::Err)
                .icon(IconName::Warn),
        );
    }
    if !worktree.main && worktree.locked.is_none() && entry.changes.is_empty() && !entry.missing {
        row = row.child(
            Chip::new(format!("wt-{index}-clean"), "clean")
                .kind(ChipKind::Ok)
                .icon(IconName::Check),
        );
    }
    row.into_any_element()
}

/// Reads the list, branches and per-worktree changes; runs on the background
/// executor.
fn load(git: Arc<Git>, root: &Path) -> git_core::Result<Loaded> {
    let repo = Repo::open(git.clone(), root)?;
    let worktrees = repo.worktrees()?;
    let refs = repo.refs()?;
    let current_branch = match repo.head()? {
        Head::Branch { name, .. } => Some(name),
        Head::Detached(_) => None,
    };
    let branches: Vec<BranchChoice> = refs
        .iter()
        .filter(|reference| reference.kind == RefKind::LocalBranch)
        .map(|reference| BranchChoice {
            name: reference.name.clone(),
            target: reference.target,
        })
        .collect();
    let mut entries = Vec::with_capacity(worktrees.len());
    for worktree in worktrees {
        let missing = worktree.prunable || !worktree.path.is_dir();
        let changes = if missing {
            Vec::new()
        } else {
            match Repo::open(git.clone(), &worktree.path).and_then(|repo| repo.changed_paths()) {
                Ok(paths) => paths,
                Err(error) => {
                    log::warn!(
                        "could not read the changes in {}: {error}",
                        worktree.path.display()
                    );
                    Vec::new()
                }
            }
        };
        entries.push(WorktreeEntry {
            worktree,
            changes,
            missing,
        });
    }
    Ok(Loaded {
        entries,
        branches,
        current_branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git, TempRepo};
    use git_core::Oid;
    use std::process::Command;

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

    fn entry(main: bool, locked: Option<&str>, changes: &[&str], missing: bool) -> WorktreeEntry {
        WorktreeEntry {
            worktree: Worktree {
                path: PathBuf::from("/tmp/wt"),
                branch: Some("main".to_owned()),
                head: Oid::from_hex(&"ab".repeat(20)),
                bare: false,
                detached: false,
                locked: locked.map(str::to_owned),
                prunable: missing,
                current: main,
                main,
            },
            changes: changes.iter().map(|path| (*path).to_owned()).collect(),
            missing,
        }
    }

    #[test]
    fn clean_rows_show_one_chip_and_used_rows_show_their_state() {
        let _ = entry(false, None, &[], false);
        assert!(!entry(false, None, &[], false).locked());
        assert!(entry(true, None, &[], false).main());
        assert!(entry(false, Some("busy"), &[], false).locked());
        assert!(entry(true, None, &[], false).main());
    }

    #[test]
    fn loads_worktrees_branches_and_each_folder_changes() {
        let repo = TempRepo::new("worktrees-load");
        std::fs::write(repo.path().join("a.txt"), "one\n").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        run(repo.path(), &["branch", "feat"]);
        let linked = repo.path().parent().unwrap().join(format!(
            "{}-linked",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&linked);
        {
            let repo = git_core::Repo::open(git(), repo.path()).unwrap();
            repo.add_worktree(&linked, "feat").unwrap();
        }
        std::fs::write(linked.join("dirty.txt"), "dirty\n").unwrap();

        let loaded = load(git(), repo.path()).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.entries[0].worktree.main);
        assert!(loaded.entries[0].changes.is_empty());
        assert_eq!(loaded.entries[1].worktree.branch.as_deref(), Some("feat"));
        assert_eq!(loaded.entries[1].changes, vec!["dirty.txt".to_owned()]);
        assert_eq!(loaded.current_branch.as_deref(), Some("main"));
        assert!(loaded.branches.iter().any(|branch| branch.name == "main"));
        assert!(loaded.branches.iter().any(|branch| branch.name == "feat"));

        // A folder that disappears is flagged as missing; pruning drops the
        // record without touching the branch.
        std::fs::remove_dir_all(&linked).unwrap();
        let loaded = load(git(), repo.path()).unwrap();
        assert!(loaded.entries[1].missing);
        {
            let repo = git_core::Repo::open(git(), repo.path()).unwrap();
            repo.prune_worktrees().unwrap();
        }
        let loaded = load(git(), repo.path()).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&linked);
    }
}
