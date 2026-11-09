//! The welcome screen: the three actions on the left, the recent repositories
//! on the right and the Initialize dialog.
//!
//! The state of each recent row (branch, or why it is unavailable) is read in
//! the background, one repository at a time, and arrives after the list is
//! already on screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, px, transparent_black, uniform_list, App, AppContext, ClickEvent, Context,
    ElementId, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, PathPromptOptions, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::{Error, Git, Repo};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_SM, FS_XL, MONO, R_MD, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, focus, menu, overlay, toast, Banner, BannerKind, Button, ButtonSize, Checkbox, Chip,
    Dialog, DialogFocus, DialogHost, EmptyState, Icon, IconName, Kbd, MenuEntry, MenuItem,
    Scrollbar, SearchField, Skeleton, TextInput, TextInputEvent, TextInputSubmit, ToastAction,
    ToastBody, ToastId, ToastKind, Tooltip,
};

use crate::clone_dialog::CloneDialog;
use crate::format::{display_path, expand_home, home_dir, relative_time, same_path};
use crate::history_view::unix_now;
use crate::init_flow::{banner_body, details, DialogKind, InitFlow, StatusKind};
use crate::recent_state::{classify, OpenResult, RecentSelection, RowState};
use crate::settings::RecentRepo;
use crate::welcome_activity::WelcomeActivity;
use crate::workspace::{
    git_label, git_tooltip, inline_window_controls, repo_name, title_strip, titlebar_end,
    titlebar_fill, uses_title_strip, window_titlebar, Workspace,
};

actions!(
    welcome,
    [
        NewRepository,
        RemoveSelected,
        SelectAllRecent,
        ExtendNext,
        ExtendPrev,
        OpenRowMenu,
        UndoRemoval
    ]
);

/// The left column: the design's fixed width.
const LEFT_W: f32 = 300.;
/// One action row.
const ACTION_H: f32 = 36.;
/// One recent repository row.
const ROW_H: f32 = 44.;
/// The branch column.
const BRANCH_W: f32 = 150.;
/// The relative-time column.
const TIME_W: f32 = 110.;
/// The slot reserved at the end of a row for the remove button, so showing it
/// never shifts the columns.
const REMOVE_SLOT_W: f32 = 20.;
/// How long the Undo toast of a removal stays (longer while hovered).
const UNDO_LIFETIME: Duration = Duration::from_secs(8);
/// The least time between two re-checks of the whole list on window activation.
const REPROBE_INTERVAL: Duration = Duration::from_secs(30);
/// How many background checks run at once. A dead network mount blocks its
/// check until the OS gives up, so without a cap a long list could occupy
/// the whole background pool; the rest wait in `probe_queue`.
const MAX_PROBES: usize = 4;

/// A recent repository row, resolved for one frame.
struct RecentRow {
    path: PathBuf,
    name: SharedString,
    path_label: SharedString,
    state: RowState,
    /// Why the row is unavailable, for its tooltip.
    tooltip: Option<SharedString>,
    time: SharedString,
}

pub struct WelcomeView {
    /// Resolved once per process by the caller; shown in the status bar and
    /// used for the row branches and the Initialize dialog.
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    /// The recent list, re-read from the settings global when it changes.
    recent: Vec<RecentRepo>,
    /// For `~` in the rows and in the dialog's folder field.
    home: Option<PathBuf>,
    filter: Entity<TextInput>,
    filter_text: String,
    /// The cursor row (an index into the filtered rows, never past the end)
    /// and the explicit multi-selection.
    selection: RecentSelection,
    /// The Undo toast of the last removal, replaced by the next one, and what
    /// it restores (Ctrl+Z does the same as its button while it is visible).
    undo_toast: Option<(ToastId, Vec<(usize, RecentRepo)>)>,
    /// The cleanup bar's "Show only these": the list shows unavailable rows only.
    only_unavailable: bool,
    /// What the background check found for each recent path; a path without an
    /// entry is still being checked (see `probing`).
    states: HashMap<PathBuf, RowState>,
    /// Paths with a background check in flight (at most [`MAX_PROBES`]).
    probing: HashSet<PathBuf>,
    /// Paths waiting for a free check slot, in order.
    probe_queue: VecDeque<PathBuf>,
    /// When the whole list was last re-checked on window activation.
    last_reprobe: Option<Instant>,
    /// Bumped whenever `recent` changes, so the activity card is only told
    /// again when the list or the selected row did.
    recent_revision: u64,
    /// What the activity card was last told: the list revision and the
    /// selected path.
    activity_sent: Option<(u64, Option<PathBuf>)>,
    /// Enter / double click on a row that was still being checked or was
    /// unavailable: it opens as soon as its (re-)check says it is available.
    open_when_ready: Option<PathBuf>,
    list_scroll: UniformListScrollHandle,
    /// The list's focus: the screen starts here so ↑/↓ work at once.
    focus: FocusHandle,
    open_focus: FocusHandle,
    clone_focus: FocusHandle,
    init_focus: FocusHandle,
    settings_focus: FocusHandle,
    /// The open Initialize dialog, if any.
    dialog: Option<Entity<InitDialog>>,
    /// The open Clone dialog, if any.
    clone_dialog: Option<Entity<CloneDialog>>,
    /// The "Your activity" card above the recent list.
    activity: Entity<WelcomeActivity>,
    _filter_events: Subscription,
    /// Re-checks every row when the window becomes active again.
    _activation: Subscription,
    /// Re-reads the recent list when the settings change.
    _settings: Subscription,
}

impl WelcomeView {
    pub fn new(
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("welcome.recent.filter"), cx);
            input
        });
        let filter_events = cx.subscribe(&filter, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.filter_text = text.clone();
                // Typing always starts again at the first match.
                this.selection.click(0);
                cx.notify();
            }
        });

        let activity = {
            let (git, workspace) = (git.clone(), workspace.clone());
            cx.new(|cx| WelcomeActivity::new(git, workspace, cx))
        };
        // A mount that came back (or a share that went away) while the window was
        // in the background shows up without a restart.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reprobe_all(window, cx);
            }
        });
        let settings = cx.observe_global::<crate::AppSettings>(|this, cx| {
            if this.sync_recent(cx) {
                cx.notify();
            }
        });
        Self {
            git,
            workspace,
            recent: read_recent(cx),
            home: home_dir(),
            filter,
            filter_text: String::new(),
            selection: RecentSelection::default(),
            undo_toast: None,
            only_unavailable: false,
            states: HashMap::new(),
            probing: HashSet::new(),
            probe_queue: VecDeque::new(),
            last_reprobe: None,
            recent_revision: 0,
            activity_sent: None,
            open_when_ready: None,
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            open_focus: cx.focus_handle().tab_stop(true),
            clone_focus: cx.focus_handle().tab_stop(false),
            init_focus: cx.focus_handle().tab_stop(true),
            settings_focus: cx.focus_handle().tab_stop(true),
            dialog: None,
            clone_dialog: None,
            activity,
            _filter_events: filter_events,
            _activation: activation,
            _settings: settings,
        }
    }

    /// The focus handle of the recent list.
    pub fn list_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Adopts a newly resolved git (after a Retry) and repaints the label.
    pub fn set_git(&mut self, git: Result<Arc<Git>, Arc<Error>>, cx: &mut Context<Self>) {
        self.git = git.clone();
        self.activity
            .update(cx, |activity, cx| activity.set_git(git, cx));
        cx.notify();
    }

    fn open_repository_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository_dialog(window, cx);
            });
        }
    }

    /// Re-reads the settings' recent list; a changed list clamps the
    /// selection instead of leaving it past the end. Returns whether it changed.
    fn sync_recent(&mut self, cx: &App) -> bool {
        let recent = read_recent(cx);
        if recent == self.recent {
            return false;
        }
        self.recent = recent;
        self.recent_revision += 1;
        let paths = self.visible_paths();
        self.selection.retain(&paths, paths.len());
        true
    }

    /// Tells the activity card the recent list and the selected row, when
    /// either changed since the last time.
    fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .visible()
            .nth(self.selection.cursor)
            .map(|repo| repo.path.clone());
        let sent = (self.recent_revision, selected);
        if self.activity_sent.as_ref() == Some(&sent) {
            return;
        }
        // Most recently opened first: worktrees of one repository are listed
        // under the newest entry.
        let mut by_recency: Vec<&RecentRepo> = self.recent.iter().collect();
        by_recency.sort_by_key(|repo| std::cmp::Reverse(repo.last_opened));
        let paths = by_recency.iter().map(|repo| repo.path.clone()).collect();
        let selected = sent.1.clone();
        self.activity_sent = Some(sent);
        self.activity
            .update(cx, |activity, cx| activity.sync(paths, selected, cx));
    }

    /// Starts one background check per row that has none yet. The file-system
    /// probe needs no git, so it runs even when git could not be found.
    fn ensure_states(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let missing: Vec<PathBuf> = self
            .recent
            .iter()
            .map(|repo| &repo.path)
            .filter(|path| !self.states.contains_key(*path) && !self.probing.contains(*path))
            .cloned()
            .collect();
        for path in missing {
            self.queue_probe(path, window, cx);
        }
    }

    /// [`Self::start_probe`] when a check slot is free, else the path waits its
    /// turn (at most [`MAX_PROBES`] checks run at once).
    fn queue_probe(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.probing.contains(&path) || self.probe_queue.contains(&path) {
            return;
        }
        if self.probing.len() < MAX_PROBES {
            self.start_probe(path, window, cx);
        } else {
            self.probe_queue.push_back(path);
        }
    }

    /// Starts queued checks while slots are free; entries removed from the list
    /// meanwhile are skipped.
    fn drain_probe_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while self.probing.len() < MAX_PROBES {
            let Some(path) = self.probe_queue.pop_front() else {
                break;
            };
            if self.recent.iter().any(|repo| repo.path == path) {
                self.start_probe(path, window, cx);
            }
        }
    }

    /// Checks one row in the background and stores the result. A check already
    /// running for the path is not doubled. The row keeps its current state
    /// meanwhile, so re-checking does not flicker.
    fn start_probe(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.probing.insert(path.clone()) {
            return;
        }
        let git = match &self.git {
            Ok(git) => Some(git.clone()),
            Err(error) => {
                log::debug!("recent branches are unavailable: {error}");
                None
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            let read_path = path.clone();
            let (state, linked) = cx
                .background_executor()
                .spawn(async move { check_recent(git, &read_path) })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_probe(path, state, linked, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_probe(
        &mut self,
        path: PathBuf,
        state: RowState,
        linked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.probing.remove(&path);
        self.drain_probe_queue(window, cx);
        if linked {
            // Linked worktrees are never listed; older versions recorded them,
            // so the entry is dropped from the settings (a one-off migration).
            crate::update_settings(cx, |settings| {
                settings.prune_recent_worktrees(|recent| recent == path);
            });
            if self.open_when_ready.as_ref() == Some(&path) {
                self.open_when_ready = None;
            }
            self.sync_recent(cx);
            cx.notify();
            return;
        }
        let available = matches!(state, RowState::Available { .. });
        let was_unavailable = self.is_unavailable(&path);
        // The chip label of the reason, for the "still unavailable" toast.
        let still_unavailable = state
            .unavailable()
            .filter(|_| was_unavailable)
            .map(|reason| reason.chip_label());
        self.states.insert(path.clone(), state);
        // An Enter / double click waited for this result. Unavailable: the row
        // stays selected, showing its chip and inline actions.
        if self.open_when_ready.as_ref() == Some(&path) {
            self.open_when_ready = None;
            if available {
                self.open_path(path, window, cx);
            } else if let Some(reason) = still_unavailable {
                // The re-check changed nothing: say so, or Enter looks dead.
                toast::show(
                    ToastKind::Info,
                    ui::t!("welcome.recent.toast.still_unavailable", reason = reason),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Re-checks every row (window activation): an unmounted share may be back,
    /// a repository may have been deleted meanwhile. At most every
    /// [`REPROBE_INTERVAL`]: each check reads the disk and opens the repository.
    fn reprobe_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The executor's clock, so tests can move it.
        let now = cx.background_executor().now();
        if self
            .last_reprobe
            .is_some_and(|last| now.saturating_duration_since(last) < REPROBE_INTERVAL)
        {
            return;
        }
        self.last_reprobe = Some(now);
        let paths: Vec<PathBuf> = self.recent.iter().map(|repo| repo.path.clone()).collect();
        for path in paths {
            self.queue_probe(path, window, cx);
        }
    }

    fn is_unavailable(&self, path: &Path) -> bool {
        self.states
            .get(path)
            .is_some_and(|state| state.unavailable().is_some())
    }

    /// Every unavailable entry of the whole list, filter or not: what the
    /// cleanup bar counts and removes.
    fn unavailable_paths(&self) -> Vec<PathBuf> {
        self.recent
            .iter()
            .filter(|repo| self.is_unavailable(&repo.path))
            .map(|repo| repo.path.clone())
            .collect()
    }

    /// The rows the list shows, in list order: those the filter text matches
    /// (and, under "Show only these", the unavailable ones).
    fn visible(&self) -> impl Iterator<Item = &RecentRepo> + '_ {
        let filter = self.filter_text.to_lowercase();
        self.recent.iter().filter(move |repo| {
            (!self.only_unavailable || self.is_unavailable(&repo.path))
                && matches_filter(
                    &recent_name(&repo.path),
                    &display_path(&repo.path, self.home.as_deref()),
                    &filter,
                )
        })
    }

    fn filtered_len(&self) -> usize {
        self.visible().count()
    }

    /// The paths of the visible rows, in list order: what [`RecentSelection`] indexes into.
    fn visible_paths(&self) -> Vec<PathBuf> {
        self.visible().map(|repo| repo.path.clone()).collect()
    }

    fn filtered_rows(&self) -> Vec<RecentRow> {
        let now = unix_now();
        self.visible()
            .map(|repo| {
                let state = self
                    .states
                    .get(&repo.path)
                    .cloned()
                    .unwrap_or(RowState::Checking);
                let tooltip = state
                    .unavailable()
                    .map(|reason| reason.tooltip(&repo.path, self.home.as_deref()));
                RecentRow {
                    path: repo.path.clone(),
                    name: recent_name(&repo.path).into(),
                    path_label: display_path(&repo.path, self.home.as_deref()).into(),
                    state,
                    tooltip,
                    time: relative_time(now, repo.last_opened).into(),
                }
            })
            .collect()
    }

    fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.filtered_len() {
            self.selection.click(index);
            cx.notify();
        }
    }

    /// A click on a row: Ctrl/Cmd toggles it, Shift extends a range from the
    /// anchor, a plain click selects it alone.
    fn click_row(&mut self, index: usize, toggle: bool, range: bool, cx: &mut Context<Self>) {
        let paths = self.visible_paths();
        if index >= paths.len() {
            return;
        }
        self.open_when_ready = None;
        if range {
            self.selection.extend_to(index, &paths);
        } else if toggle {
            self.selection.toggle(index, &paths);
        } else {
            self.selection.click(index);
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.filtered_len();
        if len == 0 {
            return;
        }
        let next = (self.selection.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        if next != self.selection.cursor || self.selection.count(&self.visible_paths()) != 1 {
            self.open_when_ready = None;
            self.selection.click(next);
            self.list_scroll
                .scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    /// Shift+↑/↓: grows the selection one row at a time. Only while the list has
    /// the focus: with the filter field focused the keys are left to the field
    /// (they must not change the list's selection behind it).
    fn extend_selection(&mut self, delta: isize, window: &Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            cx.propagate();
            return;
        }
        let paths = self.visible_paths();
        if paths.is_empty() {
            return;
        }
        self.selection.extend_by(delta, &paths);
        self.list_scroll
            .scroll_to_item(self.selection.cursor, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Ctrl/Cmd+A: every visible row (the focused filter keeps its own select-all).
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let paths = self.visible_paths();
        self.selection.select_all(&paths);
        cx.notify();
    }

    /// Esc: drops a selection of two or more rows. Returns whether it did,
    /// so a lone Esc can reach whatever sits behind the list.
    fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selection.count(&self.visible_paths()) < 2 {
            return false;
        }
        self.selection.clear();
        cx.notify();
        true
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_row(self.selection.cursor, window, cx);
    }

    fn open_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.filtered_rows();
        let Some(row) = rows.get(index) else {
            return;
        };
        let path = row.path.clone();
        match row.state {
            RowState::Available { .. } => self.open_path(path, window, cx),
            // A row that is still being checked, or is unavailable, is only
            // selected: opening an unverified path could fail (or, worse, open a
            // parent repository). It opens once its check says it is available;
            // an unavailable row is re-checked now, since it may have come back
            // (a remounted share, a transient git failure).
            RowState::Checking | RowState::Unavailable(_) => {
                self.select_row(index, cx);
                self.open_when_ready = Some(path.clone());
                self.start_probe(path, window, cx);
            }
        }
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx);
            });
        }
    }

    /// Del: removes the selected rows (or the cursor row).
    fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.selection.targets(&self.visible_paths());
        self.remove_paths(targets, window, cx);
    }

    /// The one removal path (× button, Del, the footer link): drops the
    /// entries from the recent list and offers Undo in a toast. Nothing asks
    /// for confirmation; Undo puts the entries back where they were.
    fn remove_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let mut removed = Vec::new();
        crate::update_settings(cx, |settings| removed = settings.remove_recent_many(&paths));
        if removed.is_empty() {
            return;
        }
        self.sync_recent(cx);
        cx.notify();

        let message = match removed.as_slice() {
            [(_, repo)] => ui::t!(
                "welcome.recent.toast.removed_one",
                name = recent_name(&repo.path)
            ),
            many => ui::i18n::t_count("welcome.recent.toast.removed_many", many.len() as u64, &[]),
        };
        let view = cx.entity().downgrade();
        let restorable = removed.clone();
        let undo = ToastAction::new(ui::t!("welcome.recent.toast.undo"), move |_, cx| {
            // The toast outlives the view (a repository opened within the
            // lifetime replaces the welcome screen), so the entries go back
            // through the settings global; the view is refreshed only if alive.
            restore_recent(cx, removed.clone());
            view.update(cx, |view, cx| {
                view.sync_recent(cx);
                cx.notify();
            })
            .ok();
        });
        // One Undo at a time: a newer removal replaces the older toast.
        if let Some((previous, _)) = self.undo_toast.take() {
            toast::dismiss(previous, window, cx);
        }
        let id = toast::show_with(
            ToastKind::Info,
            message,
            Some(ToastBody {
                lines: Vec::new(),
                action: Some(undo),
                lifetime: Some(UNDO_LIFETIME),
            }),
            window,
            cx,
        );
        self.undo_toast = Some((id, restorable));
    }

    /// Ctrl+Z: does what the Undo button of the removal toast does, while that
    /// toast is still visible; afterwards (or without one) the key is left alone.
    fn undo_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self
            .undo_toast
            .as_ref()
            .is_some_and(|(id, _)| toast::toasts(window, cx).iter().any(|t| t.id == *id));
        let Some((id, removed)) = self.undo_toast.take().filter(|_| visible) else {
            cx.propagate();
            return;
        };
        toast::dismiss(id, window, cx);
        restore_recent(cx, removed);
        self.sync_recent(cx);
        cx.notify();
    }

    /// Shift+F10 / the Menu key: the menu of the cursor row (of the selection
    /// when the cursor row is part of it), anchored below the row. Rows have a
    /// fixed height, so the anchor comes from the list's bounds and scroll
    /// offset; a row off screen is scrolled to the nearest edge first.
    fn open_cursor_row_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.selection.cursor;
        if index >= self.filtered_len() {
            return;
        }
        let (bounds, offset) = {
            let scroll = self.list_scroll.0.borrow();
            (scroll.base_handle.bounds(), scroll.base_handle.offset())
        };
        let row_height = px(ROW_H);
        let top = bounds.top() + row_height * index as f32 + offset.y;
        let lowest = (bounds.bottom() - row_height).max(bounds.top());
        let top_on_screen = top.clamp(bounds.top(), lowest);
        if top_on_screen != top {
            self.list_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
        // Past the folder icon, level with the name.
        let position = gpui::point(bounds.left() + px(48.), top_on_screen + row_height);
        self.open_row_menu(index, position, true, window, cx);
    }

    /// Right click on a row: selects it unless it is already part of the
    /// selection, then opens the row's menu at the pointer. `from_keyboard`
    /// (Shift+F10, the Menu key) highlights the menu's first item.
    fn open_row_menu(
        &mut self,
        index: usize,
        position: gpui::Point<gpui::Pixels>,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = self.visible_paths();
        let Some(path) = paths.get(index).cloned() else {
            return;
        };
        if !self.selection.is_selected(index, &paths) {
            self.selection.click(index);
        }
        let targets = self.selection.targets(&paths);
        let kind = if targets.len() > 1 {
            RowMenu::Many(targets.len())
        } else if self.is_unavailable(&path) {
            RowMenu::Unavailable
        } else {
            RowMenu::Available
        };
        let weak = cx.entity().downgrade();
        let entries = row_menu_entries(kind, |command| {
            let (weak, path, targets) = (weak.clone(), path.clone(), targets.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                weak.update(cx, |view, cx| {
                    view.run_menu_command(command, &path, &targets, window, cx)
                })
                .ok();
            })
        });
        cx.notify();
        if from_keyboard {
            menu::open_context_menu_from_keyboard(position, entries, window, cx);
        } else {
            menu::open_context_menu(position, entries, window, cx);
        }
    }

    fn run_menu_command(
        &mut self,
        command: MenuCommand,
        path: &Path,
        targets: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            MenuCommand::Open => {
                if let Some(index) = self.visible_paths().iter().position(|p| p == path) {
                    self.open_row(index, window, cx);
                }
            }
            MenuCommand::OpenWindow => {
                if let Some(workspace) = self.workspace.upgrade() {
                    let path = path.to_path_buf();
                    workspace.update(cx, |workspace, cx| {
                        workspace.open_repository_window(path, window, cx);
                    });
                }
            }
            MenuCommand::Reveal => cx.reveal_path(path),
            MenuCommand::CopyPath => {
                crate::copy_with_toast(
                    path.display().to_string(),
                    ui::t!("welcome.recent.toast.path_copied"),
                    window,
                    cx,
                );
            }
            MenuCommand::Locate => self.locate(path.to_path_buf(), window, cx),
            MenuCommand::Remove => self.remove_paths(targets.to_vec(), window, cx),
        }
    }

    /// Locate…: a folder picker, then a background `Repo::open` on the pick;
    /// a repository replaces the entry, anything else is an error toast.
    fn locate(&mut self, old: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(ui::t!(
                "welcome.recent.picker_prompt",
                name = recent_name(&old)
            )),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(error)) => {
                    this.update_in(cx, |_, window, cx| locate_picker_failed(&error, window, cx))
                        .ok();
                    return;
                }
                Err(error) => {
                    this.update_in(cx, |_, window, cx| locate_picker_failed(&error, window, cx))
                        .ok();
                    return;
                }
            };
            let Some(picked) = picked else {
                return;
            };
            let git = this.read_with(cx, |view, _| view.git.clone()).ok();
            let Some(git) = git else {
                return;
            };
            let opened = cx
                .background_executor()
                .spawn({
                    let picked = picked.clone();
                    async move {
                        let git = match git {
                            Ok(git) => git,
                            Err(error) => return LocateOutcome::Failed(error.to_string()),
                        };
                        let opened = Repo::open(git, &picked).map(|repo| repo.root().to_path_buf());
                        locate_outcome(&picked, opened, git_core::is_linked_worktree)
                    }
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_locate(old, picked, opened, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// The end of [`Self::locate`].
    fn finish_locate(
        &mut self,
        old: PathBuf,
        picked: PathBuf,
        outcome: LocateOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = match outcome {
            LocateOutcome::Repository(root) => root,
            LocateOutcome::NotRepository => {
                toast::show(
                    ToastKind::Error,
                    ui::t!(
                        "welcome.recent.toast.locate_not_repo",
                        path = display_path(&picked, self.home.as_deref())
                    ),
                    window,
                    cx,
                );
                return;
            }
            LocateOutcome::LinkedWorktree => {
                toast::show(
                    ToastKind::Error,
                    ui::t!(
                        "welcome.recent.toast.locate_worktree",
                        path = display_path(&picked, self.home.as_deref())
                    ),
                    window,
                    cx,
                );
                return;
            }
            LocateOutcome::Failed(error) => {
                toast::show(
                    ToastKind::Error,
                    ui::t!("welcome.recent.toast.locate_failed", error = error),
                    window,
                    cx,
                );
                return;
            }
        };
        let relocation = relocate_target(&old, &root, &self.recent);
        if let Relocation::Moved(new) | Relocation::Merged(new) = relocation {
            crate::update_settings(cx, |settings| settings.relocate_recent(&old, new));
        }
        // Check the entry again: it may be available now (and the new path
        // has not been probed yet).
        self.states.remove(&old);
        self.states.remove(&root);
        self.sync_recent(cx);
        cx.notify();
    }

    fn render_left(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let logo = crate::app_icon::logo(px(26.));

        let open = row_action(cx, |view, window, cx| {
            view.open_repository_dialog(window, cx)
        });
        let init = row_action(cx, |view, window, cx| view.open_init_dialog(window, cx));

        let open_ring = focus::focus_visible(&self.open_focus, window, cx);
        let init_ring = focus::focus_visible(&self.init_focus, window, cx);
        let settings_ring = focus::focus_visible(&self.settings_focus, window, cx);

        div()
            .w(px(LEFT_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(24.))
            .py(px(28.))
            .bg(colors.surface)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mb(px(4.))
                    .child(logo)
                    .child(
                        div()
                            .text_size(FS_XL)
                            .font_weight(FontWeight::BOLD)
                            .child("GitWarp"),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .mb(px(16.))
                    .text_color(colors.muted)
                    .child(ui::t!("welcome.tagline")),
            )
            .child(action_row(
                ActionSpec {
                    id: "welcome-open",
                    icon: IconName::Folder,
                    label: ui::t!("welcome.action.open"),
                    kbd: Some("Ctrl+O"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.open_tooltip"),
                },
                self.open_focus.clone(),
                open_ring,
                cx,
                Some(open),
            ))
            .child(action_row(
                ActionSpec {
                    id: "welcome-clone",
                    icon: IconName::Clone,
                    label: ui::t!("welcome.action.clone"),
                    kbd: Some("Ctrl+Shift+O"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.clone_tooltip"),
                },
                self.clone_focus.clone(),
                false,
                cx,
                {
                    let view = cx.entity().downgrade();
                    Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                        view.update(cx, |view, cx| view.open_clone_dialog(window, cx))
                            .ok();
                    }))
                },
            ))
            .child(action_row(
                ActionSpec {
                    id: "welcome-init",
                    icon: IconName::Init,
                    label: ui::t!("welcome.action.init"),
                    kbd: Some("Ctrl+N"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.init_tooltip"),
                },
                self.init_focus.clone(),
                init_ring,
                cx,
                Some(init),
            ))
            // Spacer: pushes Settings to the bottom of the panel.
            .child(div().flex_1())
            .child(action_row(
                ActionSpec {
                    id: "welcome-settings",
                    icon: IconName::Gear,
                    label: ui::t!("repo.toolbar.settings"),
                    kbd: Some("Ctrl+,"),
                    disabled: false,
                    tooltip: ui::t!("palette.command.open_settings"),
                },
                self.settings_focus.clone(),
                settings_ring,
                cx,
                {
                    let workspace = self.workspace.clone();
                    Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                        let _ = workspace
                            .update(cx, |workspace, cx| workspace.open_settings(window, cx));
                    }))
                },
            ))
    }

    fn render_right(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let rows = self.filtered_rows();
        // The visible paths, in the order of `rows`, and how many are selected.
        let paths: Vec<PathBuf> = rows.iter().map(|row| row.path.clone()).collect();
        let selected_count = self.selection.count(&paths);

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(FS_LG)
                    .font_weight(FontWeight::BOLD)
                    .child(ui::t!("welcome.recent.title")),
            )
            .child(
                div()
                    .w(px(240.))
                    .flex_none()
                    .child(SearchField::new("recent-filter", self.filter.clone())),
            );

        let panel = div()
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .track_focus(&self.focus)
            .bg(colors.bg)
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .overflow_hidden();

        let empty = if self.recent.is_empty() {
            Some(
                EmptyState::new(ui::t!("welcome.recent.empty_title"))
                    .icon(IconName::Folder)
                    .text(ui::t!("welcome.recent.empty_text"))
                    .into_any_element(),
            )
        } else if rows.is_empty() {
            Some(
                EmptyState::new(ui::t!("welcome.recent.no_matches_title"))
                    .icon(IconName::Folder)
                    .text(ui::t!(
                        "welcome.recent.no_matches_text",
                        filter = self.filter_text
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };

        let panel = if let Some(empty) = empty {
            panel.items_center().justify_center().child(empty)
        } else {
            let weak = cx.entity().downgrade();
            let focus = self.focus.clone();
            let selection = self.selection.clone();
            let alone = selected_count == 1;
            let paths = paths.clone();
            let count = rows.len();
            let list = uniform_list("recent-list", count, move |range, _, _| {
                range
                    .map(|index| {
                        render_recent_row(
                            index,
                            &rows[index],
                            RowPlace {
                                selected: selection.is_selected(index, &paths),
                                alone,
                                cursor: index == selection.cursor,
                                last: index + 1 == count,
                            },
                            weak.clone(),
                            focus.clone(),
                            colors,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.list_scroll)
            .flex_1()
            .min_h_0();
            // The scrollbar overlays the rows: as an in-flow sibling its 10px
            // track stays even without overflow and cuts the row highlight short.
            let scrollbar = div().absolute().top_0().bottom_0().right_0();
            panel
                .relative()
                .child(list)
                .child(scrollbar.child(Scrollbar::uniform(
                    "recent-scrollbar",
                    self.list_scroll.clone(),
                )))
        };

        let unavailable = self.unavailable_paths();
        let cleanup = (!unavailable.is_empty()).then(|| {
            let weak = cx.entity().downgrade();
            cleanup_bar(unavailable, self.only_unavailable, weak, colors)
        });

        let hint = div()
            .debug_selector(|| "recent-footer".to_string())
            .w_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted);
        // Two or more selected rows replace the key hints with bulk actions.
        let hint = if selected_count >= 2 {
            let remove = self.selection.targets(&paths);
            let weak = cx.entity().downgrade();
            let clear = weak.clone();
            hint.child(
                div()
                    .debug_selector(|| "recent-footer-selected".to_string())
                    .child(ui::i18n::t_count(
                        "welcome.recent.selected",
                        selected_count as u64,
                        &[],
                    )),
            )
            .child("·")
            .child(footer_link(
                "recent-footer-remove",
                ui::i18n::t_count("welcome.recent.menu.remove_n", selected_count as u64, &[]),
                colors,
                move |window, cx| {
                    let paths = remove.clone();
                    weak.update(cx, |view, cx| view.remove_paths(paths, window, cx))
                        .ok();
                },
            ))
            .child("·")
            .child(footer_link(
                "recent-footer-clear",
                ui::t!("welcome.recent.clear_selection"),
                colors,
                move |_, cx| {
                    clear
                        .update(cx, |view, cx| {
                            view.clear_selection(cx);
                        })
                        .ok();
                },
            ))
        } else {
            hint.child(Kbd::new("↑"))
                .child(Kbd::new("↓"))
                .child(ui::t!("welcome.recent.hint.select"))
                .child(Kbd::new("Enter"))
                .child(ui::t!("welcome.recent.hint.open"))
                .child(Kbd::new("Del"))
                .child(ui::t!("welcome.recent.hint.remove"))
        };

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(24.))
            .py(px(20.))
            .children(crate::activity_strip::enabled(cx).then(|| self.activity.clone()))
            .child(header)
            .children(cleanup)
            .child(panel)
            .child(hint)
    }

    /// Clears a dialog state whose layer was closed with Esc or the scrim.
    fn prune_dialog(&mut self, window: &Window, cx: &mut App) {
        if let Some(dialog) = &self.dialog {
            let open = dialog
                .read(cx)
                .layer
                .is_some_and(|layer| overlay::layer_view(layer, window, cx).is_some());
            if !open {
                self.dialog = None;
            }
        }
        if let Some(dialog) = &self.clone_dialog {
            let open = dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx));
            if open {
                self.clone_dialog = None;
            }
        }
    }

    /// Opens the Clone repository dialog; brings an existing one back if its
    /// layer was closed with Esc.
    pub(crate) fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.clone_dialog {
            if !dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx)) {
                return;
            }
        }
        let workspace = self.workspace.clone();
        let dialog = crate::clone_dialog::open(self.git.clone(), workspace, window, cx);
        self.clone_dialog = Some(dialog);
        cx.notify();
    }

    /// Opens the Initialize dialog; reopens it when the old layer is gone.
    ///
    /// The folder flow is the Open dialog's ([`crate::init_flow`]): the field
    /// is probed as it is typed, an existing repository is opened instead of
    /// initialized, and a missing folder is fine because `git init` creates it.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.dialog {
            if let Some(layer) = dialog.read(cx).layer {
                if overlay::layer_view(layer, window, cx).is_some() {
                    return;
                }
            }
        }

        let git = self.git.clone();
        let flow = cx.new(|_| InitFlow::new(git));
        let view = cx.entity().downgrade();
        flow.update(cx, |flow, _| {
            flow.set_on_open(Rc::new(move |root: PathBuf, created: bool, window, cx| {
                let Some(view) = view.upgrade() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    let label = display_path(&root, view.home.as_deref());
                    let name = repo_name(&root);
                    if let Some(dialog) = view.dialog.take() {
                        dialog.update(cx, |dialog, cx| dialog.close(window, cx));
                    }
                    if let Some(workspace) = view.workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_repository(root, window, cx);
                        });
                    }
                    if created {
                        toast::show(
                            ToastKind::Ok,
                            ui::t!("welcome.open.toast_initialized", path = label),
                            window,
                            cx,
                        );
                    } else {
                        toast::show(
                            ToastKind::Ok,
                            ui::t!("welcome.open.toast_opened", name = name),
                            window,
                            cx,
                        );
                    }
                });
            }));
        });

        let dialog = cx.new(|cx| InitDialog::new(flow, self.home.clone(), window, cx));

        let state = dialog.downgrade();
        let view = cx.entity().downgrade();
        let (layer, host) =
            dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
                Some(state) => build_init_dialog(&state, &view, cx),
                None => Dialog::new(ui::t!("welcome.init.title")),
            });

        dialog.update(cx, |dialog, _| {
            dialog.layer = Some(layer);
            dialog.host = Some(host);
        });
        self.dialog = Some(dialog);
        cx.notify();
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_dialog(window, cx);
        // Nothing unavailable is left (removed, relocated): leave the filtered view.
        if self.only_unavailable && self.unavailable_paths().is_empty() {
            self.only_unavailable = false;
        }
        let show_activity = crate::activity_strip::enabled(cx);
        self.activity
            .update(cx, |activity, cx| activity.set_enabled(show_activity, cx));
        self.sync_activity(cx);
        self.ensure_states(window, cx);

        let client = inline_window_controls(window);
        let colors = cx.theme().colors;
        let git = git_label(&self.git);
        let workspace = self.workspace.clone();

        let bar = window_titlebar(window, cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::app_icon::logo(px(16.)))
                    .child("GitWarp"),
            )
            .child(titlebar_fill());

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .key_context("welcome")
            .on_action(
                cx.listener(|view, _: &NewRepository, window, cx| {
                    view.open_init_dialog(window, cx)
                }),
            )
            .on_action(
                cx.listener(|view, _: &RemoveSelected, window, cx| {
                    view.remove_selected(window, cx)
                }),
            )
            .on_action(cx.listener(|view, _: &SelectAllRecent, _, cx| view.select_all(cx)))
            .on_action(cx.listener(|view, _: &OpenRowMenu, window, cx| {
                view.open_cursor_row_menu(window, cx)
            }))
            .on_action(
                cx.listener(|view, _: &UndoRemoval, window, cx| view.undo_removal(window, cx)),
            )
            .on_action(
                cx.listener(|view, _: &ExtendNext, window, cx| {
                    view.extend_selection(1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|view, _: &ExtendPrev, window, cx| {
                    view.extend_selection(-1, window, cx)
                }),
            )
            // Esc drops a multi-selection; otherwise it goes on to whatever
            // handles it behind the list.
            .on_action(cx.listener(|view, _: &ui::actions::Cancel, _, cx| {
                if !view.clear_selection(cx) {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|view, _: &ui::actions::SelectNext, _, cx| view.move_selection(1, cx)),
            )
            .on_action(
                cx.listener(|view, _: &ui::actions::SelectPrev, _, cx| view.move_selection(-1, cx)),
            )
            .on_action(cx.listener(|view, _: &ui::actions::Confirm, window, cx| {
                view.open_selected(window, cx)
            }))
            // Enter in the filter belongs to the list, not to the text field.
            .capture_action(
                cx.listener(|view, _: &TextInputSubmit, window, cx| view.open_selected(window, cx)),
            )
            .when(uses_title_strip(), |column| {
                column.child(title_strip("GitWarp".into(), cx))
            })
            .child(titlebar_end(bar, client))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(self.render_left(window, cx))
                    .child(self.render_right(cx)),
            )
            .child(
                div()
                    .h(STATUS)
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .bg(colors.sidebar)
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .id("welcome-status-version")
                            .tooltip(Tooltip::text(ui::t!("welcome.status.version_tooltip")))
                            .child(ui::t!(
                                "welcome.status.version",
                                version = env!("CARGO_PKG_VERSION")
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("welcome-status-git")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .hover(|this| this.bg(colors.hover))
                            .tooltip(Tooltip::text(git_tooltip(&self.git)))
                            .on_click(
                                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                                    let _ = workspace.update(cx, |workspace, cx| {
                                        workspace.open_settings_at(
                                            crate::settings_ui::Page::Git,
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            )
                            .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                            .child(git),
                    ),
            )
    }
}

/// The Initialize dialog's state. The dialog builder reads it on every frame,
/// so the flow's busy, error and probe state take effect at once.
struct InitDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    /// The shared probe and initialize flow (also used by the Open dialog).
    flow: Entity<InitFlow>,
    folder: Entity<TextInput>,
    branch: Entity<TextInput>,
    home: Option<PathBuf>,
    /// Set while the dialog fills the field itself: the change event that
    /// queues behind it must not restart the probe with the debounce.
    ignore_change: bool,
    folder_focus: FocusHandle,
    branch_focus: FocusHandle,
    browse_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    cancel_focus: FocusHandle,
    init_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _flow_observer: Subscription,
}

/// What the folder picker answered; the portal may be missing entirely.
enum PickerOutcome {
    Picked(Option<PathBuf>),
    Cancelled,
    Failed(String),
}

impl InitDialog {
    fn new(
        flow: Entity<InitFlow>,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("~/develop/new-project", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let branch = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("main", cx);
            input.set_text("main", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let folder_focus = folder.read(cx).focus_handle().clone();
        let branch_focus = branch.read(cx).focus_handle().clone();
        let subscriptions = vec![
            cx.subscribe(&folder, |this, _, event: &TextInputEvent, cx| {
                if matches!(event, TextInputEvent::Changed(_)) {
                    this.probe_folder(cx);
                }
            }),
            cx.subscribe(&branch, |this, _, _: &TextInputEvent, cx| {
                this.sync_error_border(cx);
                this.notify_host(cx);
                cx.notify();
            }),
        ];
        let flow_observer = cx.observe(&flow, |this, _, cx| {
            this.sync_error_border(cx);
            this.sync_busy(cx);
            this.notify_host(cx);
            cx.notify();
        });

        Self {
            layer: None,
            host: None,
            flow,
            folder,
            branch,
            home,
            ignore_change: false,
            folder_focus,
            branch_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            checkbox_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            init_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
            _flow_observer: flow_observer,
        }
    }

    fn spec(&self, cx: &App) -> crate::init_flow::StatusSpec {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let branch_blank = self.branch.read(cx).text().trim().is_empty();
        self.flow
            .read(cx)
            .spec(DialogKind::Initialize, empty, branch_blank)
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    /// Probes the typed folder, one debounce after the last keystroke.
    fn probe_folder(&mut self, cx: &mut Context<Self>) {
        if self.ignore_change {
            // The dialog filled the field and probed at once already.
            self.ignore_change = false;
            return;
        }
        let text = self.folder.read(cx).text().trim().to_owned();
        if text.is_empty() {
            self.flow.update(cx, |flow, cx| flow.reset(cx));
        } else {
            let path = expand_home(&text, self.home.as_deref());
            self.flow.update(cx, |flow, cx| flow.probe(path, true, cx));
        }
        self.sync_error_border(cx);
        cx.notify();
    }

    /// A blank default branch keeps the primary disabled and marks the field.
    fn sync_error_border(&mut self, cx: &mut Context<Self>) {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let blank = self.branch.read(cx).text().trim().is_empty();
        let error = self
            .flow
            .read(cx)
            .spec(DialogKind::Initialize, empty, blank)
            .kind
            == StatusKind::Plain
            && blank;
        self.branch
            .update(cx, |input, cx| input.set_error(error, cx));
    }

    /// The folder and branch fields are read-only while `git init` runs, so
    /// the path the init started with cannot change under it.
    fn sync_busy(&self, cx: &mut Context<Self>) {
        let busy = self.flow.read(cx).busy();
        self.folder
            .update(cx, |input, cx| input.set_disabled(busy, cx));
        self.branch
            .update(cx, |input, cx| input.set_disabled(busy, cx));
    }

    /// The primary button and Enter: open the repository the folder already
    /// belongs to, or initialize a plain folder.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.flow.read(cx).busy() {
            return;
        }
        let spec = self.spec(cx);
        if !spec.enabled {
            return;
        }
        match spec.kind {
            StatusKind::Repo | StatusKind::Nested => {
                let root = match self.flow.read(cx).result() {
                    Some(Ok(git_core::FolderProbe::Repo { root, .. }))
                    | Some(Ok(git_core::FolderProbe::Nested { root })) => root.clone(),
                    _ => return,
                };
                self.flow
                    .update(cx, |flow, cx| flow.open_root(root, window, cx));
            }
            StatusKind::Plain => {
                let text = self.folder.read(cx).text().trim().to_owned();
                if text.is_empty() {
                    return;
                }
                let path = expand_home(&text, self.home.as_deref());
                let branch_text = self.branch.read(cx).text().trim().to_owned();
                let branch = (!branch_text.is_empty()).then_some(branch_text);
                self.flow
                    .update(cx, |flow, cx| flow.start_init(path, branch, window, cx));
            }
            _ => {}
        }
    }

    /// Fills the field from the dialog itself. The input still queues a
    /// change event for it; `ignore_change` keeps that from restarting the
    /// probe with the debounce, because the caller probes at once.
    fn set_folder_text(&mut self, label: String, cx: &mut Context<Self>) {
        if self.folder.read(cx).text() == label {
            return;
        }
        self.ignore_change = true;
        self.folder
            .update(cx, |input, cx| input.set_text(label, cx));
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(ui::t!("welcome.init.picker_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = match receiver.await {
                Ok(Ok(Some(paths))) => PickerOutcome::Picked(paths.into_iter().next()),
                Ok(Ok(None)) => PickerOutcome::Cancelled,
                Ok(Err(error)) => PickerOutcome::Failed(error.to_string()),
                Err(error) => PickerOutcome::Failed(error.to_string()),
            };
            this.update_in(cx, |dialog, window, cx| {
                dialog.finish_browse(outcome, home, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_browse(
        &mut self,
        outcome: PickerOutcome,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A picker that was already open when `git init` started must not
        // change the folder the init is running for.
        if self.flow.read(cx).busy() {
            return;
        }
        match outcome {
            PickerOutcome::Picked(Some(path)) => {
                let label = display_path(&path, home.as_deref());
                self.set_folder_text(label, cx);
                // The picker is an explicit choice: probe at once.
                self.flow.update(cx, |flow, cx| flow.probe(path, false, cx));
                self.notify_host(cx);
                cx.notify();
            }
            PickerOutcome::Picked(None) | PickerOutcome::Cancelled => {}
            PickerOutcome::Failed(message) => {
                // No xdg portal, no picker: the same toast as the Open dialog.
                toast::show(
                    ToastKind::Error,
                    ui::t!("welcome.open.picker_failed", message = message),
                    window,
                    cx,
                );
            }
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }
}

/// Builds the Initialize dialog from its state. The builder runs on every
/// frame, so it only reads state that already exists.
fn build_init_dialog(
    state: &Entity<InitDialog>,
    view: &WeakEntity<WelcomeView>,
    cx: &App,
) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let flow = dialog.flow.read(cx);
    let spec = dialog.spec(cx);
    let busy = flow.busy();
    let initial_commit = flow.initial_commit();
    let error = flow.error().cloned();

    let browse: ClickAction = {
        let state = state.clone();
        Box::new(move |_, window, cx| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        })
    };
    let toggle: ToggleAction = {
        let state = state.clone();
        Box::new(move |next, _, cx| {
            state.update(cx, |dialog, cx| {
                dialog
                    .flow
                    .update(cx, |flow, cx| flow.set_initial_commit(next, cx));
                dialog.notify_host(cx);
            });
        })
    };
    let activate = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.activate(window, cx));
        }
    };
    let cancel = {
        let state = state.clone();
        let view = view.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
            view.update(cx, |view, cx| {
                view.dialog = None;
                cx.notify();
            })
            .ok();
        }
    };
    let on_initialize = {
        let activate = activate.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| activate(window, cx)
    };

    let label = |text: SharedString| div().text_size(FS_SM).text_color(colors.muted).child(text);

    let mut status = div().flex().flex_col().gap(px(8.));
    let mut has_status = false;
    if let Some((kind, title)) = spec.banner {
        let mut banner = Banner::new(kind, title);
        if let Some(body) = banner_body(flow, DialogKind::Initialize) {
            banner = banner.body(body);
        }
        status = status.child(banner);
        has_status = true;
    }
    if let Some(extra) = details(flow, dialog.home.as_deref(), colors) {
        status = status.child(extra);
        has_status = true;
    }
    if let Some(error) = error {
        status = status
            .child(Banner::new(BannerKind::Error, ui::t!("welcome.open.init_failed")).body(error));
        has_status = true;
    }

    let mut body = div().flex().flex_col().gap(px(16.)).child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label(ui::t!("welcome.open.folder")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().child(dialog.folder.clone()))
                    .child(
                        Button::new("init-browse")
                            .label(ui::t!("welcome.open.browse"))
                            .icon(IconName::Folder)
                            .disabled(busy)
                            .focus_handle(dialog.browse_focus.clone())
                            .on_click(browse),
                    ),
            ),
    );
    if has_status {
        body = body.child(status);
    }
    body = body
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label(ui::t!("welcome.init.default_branch")))
                .child(dialog.branch.clone()),
        )
        .child(
            Checkbox::new("init-initial-commit")
                .label(ui::t!("welcome.init.initial_commit"))
                .checked(initial_commit)
                .disabled(busy)
                .focus_handle(dialog.checkbox_focus.clone())
                .on_toggle(toggle),
        );

    let primary_enabled = spec.enabled && !busy;
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("init-cancel")
                .label(ui::t!("welcome.common.cancel"))
                .min_width(px(72.))
                .disabled(busy)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(move |_, window, cx| cancel(window, cx)),
        )
        .child(
            Button::primary("init-initialize")
                .label(spec.primary)
                .min_width(px(88.))
                .disabled(!primary_enabled)
                .focus_handle(dialog.init_focus.clone())
                .on_click(on_initialize),
        );

    let focus_order = vec![
        DialogFocus::input(dialog.folder_focus.clone()),
        DialogFocus::control(dialog.browse_focus.clone()),
        DialogFocus::input(dialog.branch_focus.clone()),
        DialogFocus::control(dialog.checkbox_focus.clone()),
        DialogFocus::control(dialog.cancel_focus.clone()),
        DialogFocus::control(dialog.init_focus.clone()),
    ];

    let mut panel = Dialog::new(ui::t!("welcome.init.title"))
        .body(body)
        .footer(footer)
        .width(px(520.))
        .busy(busy)
        .focus_order(focus_order)
        .autofocus(dialog.folder_focus.clone())
        .default_button(dialog.init_focus.clone())
        .on_default(activate);
    if let Some(hint) = spec.hint {
        panel = panel.hint(hint);
    }
    panel
}

/// What one left-column action row shows.
struct ActionSpec {
    id: &'static str,
    icon: IconName,
    label: SharedString,
    /// Shown only for shortcuts that are actually bound.
    kbd: Option<&'static str>,
    disabled: bool,
    tooltip: SharedString,
}

type RowAction = Rc<dyn Fn(&mut Window, &mut App)>;
type ClickAction = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ToggleAction = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A click handler for an action row, updating the welcome view.
fn row_action(
    cx: &Context<WelcomeView>,
    f: impl Fn(&mut WelcomeView, &mut Window, &mut Context<WelcomeView>) + 'static,
) -> RowAction {
    let view = cx.entity().downgrade();
    Rc::new(move |window, cx| {
        view.update(cx, |view, cx| f(view, window, cx)).ok();
    })
}

/// One action row: icon, label, key cap on the right. `Button` cannot render
/// this shape (it is 26px, centred, and has no trailing key cap), so the row
/// is a styled clickable div with the kit's hover and focus ring.
fn action_row(
    spec: ActionSpec,
    focus: FocusHandle,
    ring: bool,
    cx: &App,
    action: Option<RowAction>,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    let foreground = if spec.disabled {
        colors.disabled
    } else {
        colors.text
    };
    let mut row = div()
        .id(spec.id)
        .relative()
        .w_full()
        .h(px(ACTION_H))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .rounded(R_MD)
        .border_1()
        .border_color(if spec.disabled {
            colors.border
        } else {
            colors.border_strong
        })
        .bg(colors.bg)
        .text_color(foreground)
        .font_family(FONT_UI)
        .text_size(FS_BASE)
        .tooltip(Tooltip::text(spec.tooltip));

    if spec.disabled {
        row = row.cursor_not_allowed();
    } else {
        let hover = colors.hover;
        row = row
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .track_focus(&focus);
        if let Some(action) = action {
            let click = action.clone();
            row = row
                .on_click(move |_, window, cx| click(window, cx))
                .on_action(move |_: &ui::actions::Confirm, window, cx| action(window, cx));
        }
    }

    row = row
        .child(Icon::new(spec.icon).size(px(16.)).color(foreground))
        .child(div().flex_1().min_w_0().truncate().child(spec.label));
    if let Some(kbd) = spec.kbd {
        row = row.child(
            div()
                .flex_none()
                .when(spec.disabled, |this| this.opacity(0.6))
                .child(Kbd::new(ui::kbd::platform_label(kbd))),
        );
    }

    if ring {
        row = row.child(focus::focus_ring(focus::RingOffset::Inset2, R_MD, cx));
    }
    row
}

/// Where a row stands in the list.
#[derive(Clone, Copy)]
struct RowPlace {
    /// Part of the selection (highlighted).
    selected: bool,
    /// The only selected row: an unavailable one shows its inline actions.
    alone: bool,
    /// The keyboard cursor row: its remove button stays visible.
    cursor: bool,
    /// The last row has no bottom border.
    last: bool,
}

/// One 44px recent repository row with the design's four columns.
fn render_recent_row(
    index: usize,
    row: &RecentRow,
    place: RowPlace,
    weak: WeakEntity<WelcomeView>,
    focus: FocusHandle,
    colors: ThemeColors,
) -> impl IntoElement {
    let RowPlace {
        selected,
        alone,
        cursor,
        last,
    } = place;
    let group = SharedString::from(format!("recent-row-group-{index}"));
    let mut element = div()
        .id(ElementId::named_usize("recent-row", index))
        .debug_selector(move || format!("recent-row-{index}"))
        .group(group.clone())
        .relative()
        .w_full()
        .h(px(ROW_H))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .bg(if selected {
            colors.selection
        } else {
            transparent_black()
        })
        .cursor_pointer();

    let unavailable = row.state.unavailable().cloned();
    // A selected unavailable row offers Remove and Locate… instead of the time.
    let inline = unavailable.is_some() && selected && alone;
    if let Some(tip) = row.tooltip.clone() {
        element = element.tooltip(Tooltip::text(tip));
    }

    if !last {
        element = element.border_b_1().border_color(colors.border);
    }
    if !selected {
        let hover = colors.hover;
        element = element.hover(move |style| style.bg(hover));
    }

    let weak_remove = weak.clone();
    let weak_menu = weak.clone();
    let weak_inline = weak.clone();
    element = element.on_mouse_down(MouseButton::Right, {
        let focus = focus.clone();
        move |event: &MouseDownEvent, window, cx| {
            window.focus(&focus, cx);
            let position = event.position;
            weak_menu
                .update(cx, |view, cx| {
                    view.open_row_menu(index, position, false, window, cx)
                })
                .ok();
        }
    });
    element = element.on_click(move |event: &ClickEvent, window, cx| {
        window.focus(&focus, cx);
        let modifiers = event.modifiers();
        let (toggle, range) = (modifiers.secondary(cx), modifiers.shift);
        // A double click opens; with Ctrl/Shift held it is just two selections.
        let open = event.click_count() >= 2 && !toggle && !range;
        weak.update(cx, |view, cx| {
            view.click_row(index, toggle, range, cx);
            if open {
                view.open_row(index, window, cx);
            }
        })
        .ok();
    });

    element
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .child(folder_icon(unavailable.as_ref(), selected, colors)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .font_weight(if unavailable.is_some() {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::SEMIBOLD
                        })
                        .text_size(FS_BASE)
                        .text_color(if unavailable.is_some() {
                            colors.muted
                        } else {
                            colors.text
                        })
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(MONO)
                        .text_color(colors.muted)
                        .child(row.path_label.clone()),
                ),
        )
        .child(
            div()
                .w(px(BRANCH_W))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(branch_cell(index, row, unavailable.as_ref(), colors)),
        )
        .when(inline, |element| {
            element.child(inline_actions(index, row.path.clone(), weak_inline.clone()))
        })
        .when(!inline, |element| {
            element.child(
                div()
                    .w(px(TIME_W))
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .text_right()
                    .child(row.time.clone()),
            )
        })
        .when(!inline, |element| {
            element.child(
                // The slot is always reserved; the button shows on hover and on
                // the cursor row. Hidden by opacity, not removed, so it keeps its
                // size and never moves the columns.
                div()
                    .debug_selector(move || format!("recent-remove-{index}"))
                    .w(px(REMOVE_SLOT_W))
                    .flex_none()
                    .when(!cursor, |slot| {
                        slot.opacity(0.)
                            .group_hover(group, |style| style.opacity(1.))
                    })
                    .child(
                        Button::ghost(("recent-remove", index))
                            .size(ButtonSize::Xs)
                            .icon_only(IconName::X)
                            .tooltip(ui::t!("welcome.recent.remove_tip"))
                            .on_click(move |_, window, cx| {
                                // The row behind it would otherwise treat this as a click on itself.
                                cx.stop_propagation();
                                weak_remove
                                    .update(cx, |view, cx| {
                                        let path = view.visible_paths().get(index).cloned();
                                        view.remove_paths(path.into_iter().collect(), window, cx);
                                    })
                                    .ok();
                            }),
                    ),
            )
        })
}

/// The inline actions of a selected unavailable row, in place of the time
/// and the remove slot.
fn inline_actions(index: usize, path: PathBuf, weak: WeakEntity<WelcomeView>) -> impl IntoElement {
    let locate_path = path.clone();
    let weak_locate = weak.clone();
    div()
        .w(px(TIME_W + 12. + REMOVE_SLOT_W))
        .flex_none()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(4.))
        .child(
            div()
                .debug_selector(move || format!("recent-inline-remove-{index}"))
                .child(
                    Button::new(("recent-inline-remove", index))
                        .size(ButtonSize::Small)
                        .label(ui::t!("welcome.recent.remove"))
                        .on_click(move |_, window, cx| {
                            // The row behind it would otherwise treat this as a click on itself.
                            cx.stop_propagation();
                            let paths = vec![path.clone()];
                            weak.update(cx, |view, cx| view.remove_paths(paths, window, cx))
                                .ok();
                        }),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("recent-inline-locate-{index}"))
                .child(
                    Button::ghost(("recent-inline-locate", index))
                        .size(ButtonSize::Small)
                        .label(ui::t!("welcome.recent.locate"))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let path = locate_path.clone();
                            weak_locate
                                .update(cx, |view, cx| view.locate(path, window, cx))
                                .ok();
                        }),
                ),
        )
}

/// A text link of the footer line: accent coloured, underlined on hover.
fn footer_link(
    id: &'static str,
    label: SharedString,
    colors: ThemeColors,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .cursor_pointer()
        .text_color(colors.accent)
        .hover(|style| style.underline())
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(label)
}

/// The folder icon; an unavailable row gets a muted one with a small badge
/// (error for a missing folder, warning for the other reasons).
fn folder_icon(
    unavailable: Option<&crate::recent_state::Unavailable>,
    selected: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    let Some(reason) = unavailable else {
        return div()
            .child(Icon::new(IconName::Folder).size(px(16.)).color(colors.text))
            .into_any_element();
    };
    let missing = reason.chip_kind() == ui::ChipKind::Err;
    let (tone, glyph) = if missing {
        (colors.error, IconName::X)
    } else {
        (colors.warning, IconName::Warn)
    };
    // The ring matches the row background so the badge reads as cut out.
    let ring = if selected {
        colors.selection
    } else {
        colors.bg
    };
    div()
        .relative()
        .flex()
        .child(
            Icon::new(IconName::Folder)
                .size(px(16.))
                .color(colors.muted),
        )
        .child(
            div()
                .absolute()
                .right(px(-4.))
                .bottom(px(-3.))
                .size(px(11.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(tone)
                .border_color(ring)
                .border_2()
                .child(Icon::new(glyph).size(px(7.)).color(colors.bg)),
        )
        .into_any_element()
}

/// The branch column: the branch, a chip for an unavailable row or a
/// skeleton while the row is still being checked.
fn branch_cell(
    index: usize,
    row: &RecentRow,
    unavailable: Option<&crate::recent_state::Unavailable>,
    colors: ThemeColors,
) -> gpui::AnyElement {
    match (&row.state, unavailable) {
        (_, Some(reason)) => div()
            .debug_selector(move || format!("recent-chip-{index}"))
            .child(Chip::new(("recent-chip", index), reason.chip_label()).kind(reason.chip_kind()))
            .into_any_element(),
        (RowState::Checking, None) => {
            Skeleton::new(("recent-skeleton", index), px(60.)).into_any_element()
        }
        (
            RowState::Available {
                branch: Some(branch),
            },
            None,
        ) => div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                Icon::new(IconName::Branch)
                    .size(px(12.))
                    .color(colors.muted),
            )
            .child(div().min_w_0().truncate().child(branch.clone()))
            .into_any_element(),
        _ => div().child("—").into_any_element(),
    }
}

/// What a row's context menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowMenu {
    Available,
    Unavailable,
    /// Two or more selected rows: only the bulk removal.
    Many(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuCommand {
    Open,
    OpenWindow,
    Reveal,
    CopyPath,
    Locate,
    Remove,
}

/// What a menu command does when picked.
type MenuRun = Rc<dyn Fn(&mut Window, &mut App)>;

/// The menu's rows; `run` supplies the action behind each command. The
/// commands that do not apply to an unavailable row (Open, new window,
/// Reveal) are left out, not disabled.
fn row_menu_entries(kind: RowMenu, run: impl Fn(MenuCommand) -> MenuRun) -> Vec<MenuEntry> {
    let item = |label: SharedString, icon: IconName, command: MenuCommand| {
        MenuItem::new(label).icon(icon).action({
            let action = run(command);
            move |window, cx| action(window, cx)
        })
    };
    let copy = item(
        ui::t!("welcome.recent.menu.copy_path"),
        IconName::Copy,
        MenuCommand::CopyPath,
    );
    let remove = |label: SharedString| {
        MenuEntry::Item(
            item(label, IconName::Trash, MenuCommand::Remove)
                .shortcut("Del")
                .danger(),
        )
    };
    match kind {
        RowMenu::Many(n) => vec![remove(ui::i18n::t_count(
            "welcome.recent.menu.remove_n",
            n as u64,
            &[],
        ))],
        RowMenu::Unavailable => vec![
            MenuEntry::Item(item(
                ui::t!("welcome.recent.menu.locate"),
                IconName::Search,
                MenuCommand::Locate,
            )),
            MenuEntry::Item(copy),
            MenuEntry::Separator,
            remove(ui::t!("welcome.recent.menu.remove")),
        ],
        RowMenu::Available => vec![
            MenuEntry::Item(
                item(
                    ui::t!("welcome.recent.menu.open"),
                    IconName::Folder,
                    MenuCommand::Open,
                )
                .shortcut("Enter"),
            ),
            MenuEntry::Item(item(
                ui::t!("welcome.recent.menu.open_window"),
                IconName::PanelR,
                MenuCommand::OpenWindow,
            )),
            MenuEntry::Item(item(
                ui::t!("welcome.recent.menu.reveal"),
                IconName::Link,
                MenuCommand::Reveal,
            )),
            MenuEntry::Item(copy),
            MenuEntry::Separator,
            remove(ui::t!("welcome.recent.menu.remove")),
        ],
    }
}

/// Undo of [`WelcomeView::remove_paths`]: the entries return to their old positions.
fn restore_recent(cx: &mut App, removed: Vec<(usize, RecentRepo)>) {
    crate::update_settings(cx, |settings| settings.restore_recent(removed));
}

/// What the background check of a Locate… pick found.
#[derive(Debug, PartialEq, Eq)]
enum LocateOutcome {
    /// The picked folder is itself a repository root.
    Repository(PathBuf),
    /// Not a repository, or only inside one (a parent dotfiles repository).
    NotRepository,
    /// The pick is a linked worktree; Recent lists main repositories only.
    LinkedWorktree,
    /// Git could not be started or failed; carries the error text.
    Failed(String),
}

/// Classifies the result of `Repo::open(picked)`: the picked folder itself must be
/// the repository root (the same rule as [`classify`]), because `Repo::open` searches
/// upward and would accept an enclosing repository.
fn locate_outcome(
    picked: &Path,
    opened: Result<PathBuf, Error>,
    is_worktree: impl Fn(&Path) -> bool,
) -> LocateOutcome {
    match opened {
        Ok(root) if !same_path(&root, picked) => LocateOutcome::NotRepository,
        Ok(root) if is_worktree(&root) => LocateOutcome::LinkedWorktree,
        Ok(root) => LocateOutcome::Repository(root),
        Err(Error::NotARepository { .. }) => LocateOutcome::NotRepository,
        Err(other) => LocateOutcome::Failed(other.to_string()),
    }
}

/// What Locate… does with the repository the user picked.
#[derive(Debug, PartialEq, Eq)]
enum Relocation {
    /// The pick is the entry's own path: only check it again.
    Unchanged,
    /// The entry moves to the picked repository root.
    Moved(PathBuf),
    /// Like `Moved`, and the list already had an entry for the root: that
    /// one is dropped so the list has no duplicates.
    Merged(PathBuf),
}

/// Where the entry `old` points after Locate…, given the root of the picked
/// repository. A pick inside a repository relocates to that repository's root.
fn relocate_target(old: &Path, picked_root: &Path, recent: &[RecentRepo]) -> Relocation {
    if picked_root == old {
        Relocation::Unchanged
    } else if recent.iter().any(|repo| repo.path == picked_root) {
        Relocation::Merged(picked_root.to_path_buf())
    } else {
        Relocation::Moved(picked_root.to_path_buf())
    }
}

/// No xdg portal, no picker: the same toast as the Open dialog.
fn locate_picker_failed(error: &dyn std::fmt::Display, window: &mut Window, cx: &mut App) {
    toast::show(
        ToastKind::Error,
        ui::t!("welcome.open.picker_failed", message = error.to_string()),
        window,
        cx,
    );
}

/// The strip above the list while some recent entry is unavailable: how many,
/// a toggle that narrows the list to them, and one-click removal of all of
/// them (undoable, so no confirmation).
fn cleanup_bar(
    unavailable: Vec<PathBuf>,
    only: bool,
    weak: WeakEntity<WelcomeView>,
    colors: ThemeColors,
) -> impl IntoElement {
    let count = unavailable.len();
    let toggle = weak.clone();
    div()
        .debug_selector(|| "recent-cleanup".to_string())
        .w_full()
        .h(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(10.))
        .pr(px(6.))
        .border_1()
        .border_color(colors.warning.opacity(0.4))
        .rounded(R_MD)
        .bg(colors.warning.opacity(0.12))
        .text_size(FS_SM)
        .child(
            Icon::new(IconName::Warn)
                .size(px(14.))
                .color(colors.warning),
        )
        .child(ui::i18n::t_count(
            "welcome.recent.cleanup.text",
            count as u64,
            &[],
        ))
        .child(div().flex_1())
        .child(
            div()
                .debug_selector(|| "recent-cleanup-only".to_string())
                .child(
                    Button::new("recent-cleanup-only")
                        .size(ButtonSize::Small)
                        .label(if only {
                            ui::t!("welcome.recent.cleanup.show_all")
                        } else {
                            ui::t!("welcome.recent.cleanup.show_only")
                        })
                        .on_click(move |_, _, cx| {
                            toggle
                                .update(cx, |view, cx| {
                                    view.only_unavailable = !view.only_unavailable;
                                    view.selection.click(0);
                                    cx.notify();
                                })
                                .ok();
                        }),
                ),
        )
        .child(
            div()
                .debug_selector(|| "recent-cleanup-remove".to_string())
                .child(
                    // Neutral, not the danger fill: this only drops list entries and
                    // must not read as deleting files (the design's danger outline
                    // is deliberately not followed, see F2 report).
                    Button::new("recent-cleanup-remove")
                        .size(ButtonSize::Small)
                        .label(ui::i18n::t_count(
                            "welcome.recent.menu.remove_n",
                            count as u64,
                            &[],
                        ))
                        .on_click(move |_, window, cx| {
                            let paths = unavailable.clone();
                            weak.update(cx, |view, cx| view.remove_paths(paths, window, cx))
                                .ok();
                        }),
                ),
        )
}

/// The background check of one recent entry: the file-system probe first,
/// then (only for a plausible repository, and only when git is available)
/// opening it and reading HEAD.
///
/// The flag says the entry is a linked worktree (never listed in Recent).
fn check_recent(git: Option<Arc<Git>>, path: &Path) -> (RowState, bool) {
    let probe = git_core::probe_repo_path(path);
    let linked = matches!(
        probe,
        git_core::PathProbe::Candidate { linked: true }
            | git_core::PathProbe::BrokenWorktree { .. }
    );
    let open: Option<OpenResult> = match (&probe, git) {
        // A linked worktree is dropped from the list, so it is not opened.
        (git_core::PathProbe::Candidate { linked: false }, Some(git)) => {
            Some(Repo::open(git, path).map(|repo| (repo.root().to_path_buf(), repo.head().ok())))
        }
        _ => None,
    };
    (classify(path, probe, open), linked)
}

fn read_recent(cx: &App) -> Vec<RecentRepo> {
    cx.try_global::<crate::AppSettings>()
        .map(|state| state.settings.recent.clone())
        .unwrap_or_default()
}

/// The last path component, without touching the file system.
pub(crate) fn recent_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Case-insensitive substring match on the name or the full displayed path;
/// `filter_lower` is the filter text, already lower-cased.
fn matches_filter(name: &str, path: &str, filter_lower: &str) -> bool {
    if filter_lower.is_empty() {
        return true;
    }
    name.to_lowercase().contains(filter_lower) || path.to_lowercase().contains(filter_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_is_case_insensitive_on_the_name_or_the_path() {
        let matches = |name, path, filter: &str| matches_filter(name, path, &filter.to_lowercase());
        assert!(matches("GitWarp", "~/dev/gitwarp", ""));
        assert!(matches("GitWarp", "~/dev/gitwarp", "git"));
        assert!(matches("GitWarp", "~/dev/gitwarp", "GITWARP"));
        assert!(matches("notes", "/mnt/data/notes", "mnt/Data"));
        assert!(!matches("notes", "/mnt/data/notes", "gitwarp"));
    }

    #[test]
    fn the_row_name_is_the_last_component() {
        assert_eq!(recent_name(Path::new("/home/user/gitwarp")), "gitwarp");
        assert_eq!(recent_name(Path::new("/")), "/");
    }

    /// Opening an existing repository from the Initialize dialog runs from
    /// the dialog's own update; the flow re-enters it only after that
    /// returned, so no double entity lease.
    #[gpui::test]
    fn opening_an_existing_repository_closes_the_initialize_dialog(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("init-dialog");
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);

        let view = cx.update(|window, cx| {
            cx.new(|cx| WelcomeView::new(Ok(git), workspace.downgrade(), window, cx))
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.open_init_dialog(window, cx));
        });
        let dialog = view
            .read_with(&cx, |view, _| view.dialog.clone())
            .expect("the Initialize dialog is open");

        dialog.update(&mut cx, |dialog, cx| {
            dialog.ignore_change = true;
            dialog.folder.update(cx, |input, cx| {
                input.set_text(repo.path().display().to_string(), cx)
            });
            dialog.flow.update(cx, |flow, cx| {
                flow.probe(repo.path().to_path_buf(), false, cx)
            });
        });
        cx.run_until_parked();
        assert_eq!(
            dialog.update(&mut cx, |dialog, cx| dialog.spec(cx).kind),
            StatusKind::Repo,
            "the folder was not classified as a repository"
        );

        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| dialog.activate(window, cx));
        });
        cx.run_until_parked();

        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            0,
            "the dialog stayed open"
        );
        assert!(
            view.read_with(&cx, |view, _| view.dialog.is_none()),
            "the welcome view kept the closed dialog"
        );
        assert!(
            cx.debug_bounds("sidebar-filter").is_some(),
            "the workspace did not open the repository"
        );
    }

    /// Opens the real welcome window and drives the Initialize dialog through
    /// its action, so the builder, the focus trap and Esc are exercised.
    #[gpui::test]
    fn ctrl_n_opens_and_escape_closes_the_initialize_dialog(cx: &mut gpui::TestAppContext) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        cx.update(ui::init).expect("ui::init");
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(960.), px(620.)),
                    ))),
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        Workspace::new(
                            None,
                            Err(Arc::new(Error::PathNotFound {
                                path: PathBuf::from("/nonexistent"),
                            })),
                            git_core::GitChoice::Auto,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });

        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        cx.update(|window, _| window.activate_window());
        assert!(
            cx.debug_bounds("recent-filter").is_some(),
            "the welcome screen did not render"
        );

        cx.update(|window, cx| window.dispatch_action(Box::new(NewRepository), cx));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            1,
            "Ctrl N did not open the Initialize dialog"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            0,
            "Escape did not close the dialog"
        );

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(crate::workspace::OpenCloneDialog), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            1,
            "Ctrl Shift O did not open the Clone dialog"
        );
    }

    /// A missing folder shows its chip and a real repository does not; Enter
    /// on the unavailable row leaves the welcome screen open.
    #[gpui::test]
    fn an_unavailable_recent_row_shows_a_chip_and_does_not_open(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-states");
        let gone = repo.path().join("gone");
        let settings = crate::settings::Settings {
            recent: vec![
                RecentRepo {
                    path: gone.clone(),
                    last_opened: 2,
                },
                RecentRepo {
                    path: repo.path().to_path_buf(),
                    last_opened: 1,
                },
            ],
            ..Default::default()
        };
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings,
                save_in_flight: false,
                dirty: false,
            });
        });
        let (mut cx, workspace) =
            crate::test_support::open_workspace(crate::test_support::git(), cx);
        // The first render starts the checks; give them time to finish.
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-chip-0").is_some(),
            "the missing folder has no chip"
        );
        assert!(
            cx.debug_bounds("recent-chip-1").is_none(),
            "the real repository has a chip"
        );

        // Row 0 (the missing folder) is selected: Enter must not open it.
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Confirm), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-filter").is_some(),
            "an unavailable row left the welcome screen"
        );
        assert!(workspace.read_with(&cx, |workspace, _| workspace
            .repository_for_test()
            .is_none()));

        // The real repository still opens with Enter.
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::SelectNext), cx));
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Confirm), cx));
        cx.run_until_parked();
        assert!(workspace.read_with(&cx, |workspace, _| workspace
            .repository_for_test()
            .is_some()));
    }
    /// Four recent entries (none exists on disk; removal does not care) in the real window.
    fn open_with_recent(cx: &mut gpui::TestAppContext) -> (gpui::VisualTestContext, Vec<PathBuf>) {
        let paths: Vec<PathBuf> = ["a", "b", "c", "d"]
            .iter()
            .map(|name| PathBuf::from(format!("/nonexistent-recent/{name}")))
            .collect();
        let cx = open_with_paths(cx, &paths);
        (cx, paths)
    }

    /// The real welcome window with `paths` as the recent list, most recent first.
    fn open_with_paths(
        cx: &mut gpui::TestAppContext,
        paths: &[PathBuf],
    ) -> gpui::VisualTestContext {
        open_with_paths_in_workspace(cx, paths).0
    }

    fn open_with_paths_in_workspace(
        cx: &mut gpui::TestAppContext,
        paths: &[PathBuf],
    ) -> (gpui::VisualTestContext, gpui::Entity<Workspace>) {
        let settings = crate::settings::Settings {
            recent: paths
                .iter()
                .enumerate()
                .map(|(i, path)| RecentRepo {
                    path: path.clone(),
                    last_opened: 100 - i as u64,
                })
                .collect(),
            ..Default::default()
        };
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings,
                save_in_flight: false,
                dirty: false,
            });
        });
        crate::test_support::open_workspace(crate::test_support::git(), cx)
    }

    fn click_selector(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect(selector);
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();
    }

    fn recent_paths(cx: &mut gpui::VisualTestContext) -> Vec<PathBuf> {
        cx.update(|_, cx| read_recent(cx).into_iter().map(|r| r.path).collect())
    }

    fn click_row(cx: &mut gpui::VisualTestContext, index: usize, modifiers: gpui::Modifiers) {
        // `debug_bounds` wants a `'static` selector; a leaked string is fine in a test.
        let selector: &'static str = Box::leak(format!("recent-row-{index}").into_boxed_str());
        let bounds = cx.debug_bounds(selector).expect("the row is rendered");
        cx.simulate_click(bounds.center(), modifiers);
        cx.run_until_parked();
    }

    fn ctrl(cx: &mut gpui::VisualTestContext) -> gpui::Modifiers {
        cx.update(|_, cx| gpui::Modifiers::secondary_key(cx))
    }

    /// Ctrl-click two rows, Del removes both and shows a toast with Undo;
    /// the Undo puts them back where they were.
    #[gpui::test]
    fn ctrl_click_two_rows_then_del_removes_both_and_undo_restores_them(
        cx: &mut gpui::TestAppContext,
    ) {
        let (mut cx, paths) = open_with_recent(cx);
        cx.run_until_parked();
        // The cursor starts on row 0; Ctrl+click adds row 2.
        let modifiers = ctrl(&mut cx);
        click_row(&mut cx, 2, modifiers);
        assert!(
            cx.debug_bounds("recent-footer-selected").is_some(),
            "two selected rows did not bring up the selection footer"
        );

        cx.update(|window, cx| window.dispatch_action(Box::new(RemoveSelected), cx));
        cx.run_until_parked();
        assert_eq!(
            recent_paths(&mut cx),
            vec![paths[1].clone(), paths[3].clone()]
        );
        assert!(
            cx.debug_bounds("recent-footer-selected").is_none(),
            "the selection footer outlived the removed rows"
        );
        let action = cx.update(|window, cx| {
            let toasts = toast::toasts(window, cx);
            assert_eq!(toasts.len(), 1, "no Undo toast after a removal");
            assert_eq!(toasts[0].message, "Removed 2 repositories from Recent");
            toasts[0]
                .body
                .as_ref()
                .and_then(|body| body.action.clone())
                .expect("the toast has an Undo button")
        });

        cx.update(|window, cx| (action.on_click)(window, cx));
        cx.run_until_parked();
        assert_eq!(
            recent_paths(&mut cx),
            paths,
            "Undo did not restore the order"
        );
    }

    /// Ctrl+A selects every visible row, Shift+click selects a range and Esc
    /// drops a multi-selection.
    #[gpui::test]
    fn select_all_shift_click_and_escape(cx: &mut gpui::TestAppContext) {
        let (mut cx, paths) = open_with_recent(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("recent-footer-selected").is_none());

        cx.update(|window, cx| window.dispatch_action(Box::new(SelectAllRecent), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-footer-selected").is_some(),
            "Ctrl+A did not select the rows"
        );

        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Cancel), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-footer-selected").is_none(),
            "Esc did not clear the selection"
        );

        // Shift+click from row 0 to row 1, then remove the pair with the footer link.
        click_row(&mut cx, 1, gpui::Modifiers::shift());
        assert!(cx.debug_bounds("recent-footer-selected").is_some());
        let link = cx
            .debug_bounds("recent-footer-remove")
            .expect("the footer has a Remove link");
        cx.simulate_click(link.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert_eq!(recent_paths(&mut cx), paths[2..].to_vec());
    }

    /// One real repository and two missing folders, in that order.
    fn open_mixed(
        cx: &mut gpui::TestAppContext,
        repo: &crate::test_support::TempRepo,
    ) -> (gpui::VisualTestContext, Vec<PathBuf>) {
        let paths = vec![
            repo.path().to_path_buf(),
            repo.path().join("gone-1"),
            repo.path().join("gone-2"),
        ];
        let cx = open_with_paths(cx, &paths);
        (cx, paths)
    }

    /// The cleanup bar exists only while something is unavailable; "Show only
    /// these" narrows the list and "Remove 2" clears the dead entries in one click.
    #[gpui::test]
    fn the_cleanup_bar_filters_and_removes_every_unavailable_row(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-cleanup");
        let (mut cx, paths) = open_mixed(cx, &repo);
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-cleanup").is_some(),
            "no cleanup bar with two unavailable rows"
        );
        assert!(cx.debug_bounds("recent-row-2").is_some());

        click_selector(&mut cx, "recent-cleanup-only");
        assert!(cx.debug_bounds("recent-row-1").is_some());
        assert!(
            cx.debug_bounds("recent-row-2").is_none(),
            "Show only these kept the available row"
        );
        click_selector(&mut cx, "recent-cleanup-only");
        assert!(
            cx.debug_bounds("recent-row-2").is_some(),
            "Show all did not bring the rows back"
        );

        click_selector(&mut cx, "recent-cleanup-remove");
        assert_eq!(recent_paths(&mut cx), vec![paths[0].clone()]);
        assert!(
            cx.debug_bounds("recent-cleanup").is_none(),
            "the cleanup bar outlived the unavailable rows"
        );
        // One Undo restores them all.
        let action = cx.update(|window, cx| {
            toast::toasts(window, cx)[0]
                .body
                .as_ref()
                .and_then(|body| body.action.clone())
                .expect("the toast has an Undo button")
        });
        cx.update(|window, cx| (action.on_click)(window, cx));
        cx.run_until_parked();
        assert_eq!(recent_paths(&mut cx), paths);
    }

    #[gpui::test]
    fn there_is_no_cleanup_bar_when_every_row_is_available(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-all-good");
        let paths = vec![repo.path().to_path_buf()];
        let mut cx = open_with_paths(cx, &paths);
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(cx.debug_bounds("recent-row-0").is_some());
        assert!(cx.debug_bounds("recent-cleanup").is_none());
    }

    /// A selected unavailable row swaps its time for Remove and Locate…; an
    /// available one does not. Remove acts on the row alone.
    #[gpui::test]
    fn a_selected_unavailable_row_shows_inline_actions(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-inline");
        let (mut cx, paths) = open_mixed(cx, &repo);
        cx.run_until_parked();
        cx.run_until_parked();
        // Row 0 (the cursor) is the available repository.
        assert!(cx.debug_bounds("recent-inline-remove-0").is_none());
        click_row(&mut cx, 1, gpui::Modifiers::default());
        assert!(cx.debug_bounds("recent-inline-locate-1").is_some());
        assert!(
            cx.debug_bounds("recent-inline-remove-0").is_none(),
            "an unselected row shows inline actions"
        );
        // Two selected rows: no inline actions, the footer takes over.
        let modifiers = ctrl(&mut cx);
        click_row(&mut cx, 2, modifiers);
        assert!(cx.debug_bounds("recent-inline-locate-1").is_none());

        click_row(&mut cx, 2, gpui::Modifiers::default());
        click_selector(&mut cx, "recent-inline-remove-2");
        assert_eq!(recent_paths(&mut cx), paths[..2].to_vec());
    }

    /// Right click selects a row that is not part of the selection and opens
    /// a menu; on a row inside a multi-selection the selection stays.
    #[gpui::test]
    fn right_click_opens_the_row_menu_and_selects_the_row(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-menu");
        let (mut cx, _paths) = open_mixed(cx, &repo);
        cx.run_until_parked();
        cx.run_until_parked();
        let center = |cx: &mut gpui::VisualTestContext, index: usize| {
            let selector: &'static str = Box::leak(format!("recent-row-{index}").into_boxed_str());
            cx.debug_bounds(selector).expect(selector).center()
        };

        let at = center(&mut cx, 2);
        cx.simulate_mouse_down(at, gpui::MouseButton::Right, gpui::Modifiers::default());
        assert_eq!(cx.update(|window, cx| overlay::layer_count(window, cx)), 1);
        assert!(
            cx.debug_bounds("recent-inline-locate-2").is_some(),
            "the right-clicked row was not selected"
        );
        // The scrim swallows the next press and closes the menu.
        let at = center(&mut cx, 0);
        cx.simulate_mouse_down(at, gpui::MouseButton::Right, gpui::Modifiers::default());
        assert_eq!(cx.update(|window, cx| overlay::layer_count(window, cx)), 0);

        cx.update(|window, cx| window.dispatch_action(Box::new(SelectAllRecent), cx));
        cx.run_until_parked();
        let at = center(&mut cx, 1);
        cx.simulate_mouse_down(at, gpui::MouseButton::Right, gpui::Modifiers::default());
        assert_eq!(cx.update(|window, cx| overlay::layer_count(window, cx)), 1);
        assert!(
            cx.debug_bounds("recent-footer-selected").is_some(),
            "right click inside the selection dropped it"
        );
    }

    fn labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => item.label.to_string(),
                MenuEntry::Separator => "—".to_string(),
                MenuEntry::Header(header) => header.label.to_string(),
                MenuEntry::Filter { .. } => "<filter>".to_string(),
            })
            .collect()
    }

    fn noop_run(_: MenuCommand) -> MenuRun {
        Rc::new(|_, _| {})
    }

    #[gpui::test]
    fn the_row_menu_matches_the_row(cx: &mut gpui::TestAppContext) {
        cx.update(ui::init).expect("ui::init");
        assert_eq!(
            labels(&row_menu_entries(RowMenu::Available, noop_run)),
            [
                "Open",
                "Open in new window",
                "Reveal in file manager",
                "Copy path",
                "—",
                "Remove from list"
            ]
        );
        assert_eq!(
            labels(&row_menu_entries(RowMenu::Unavailable, noop_run)),
            ["Locate…", "Copy path", "—", "Remove from list"]
        );
        assert_eq!(
            labels(&row_menu_entries(RowMenu::Many(3), noop_run)),
            ["Remove 3 from list"]
        );
        let entries = row_menu_entries(RowMenu::Available, noop_run);
        let MenuEntry::Item(remove) = entries.last().unwrap() else {
            panic!("the last entry is the Remove item");
        };
        assert!(remove.danger);
        assert_eq!(remove.shortcut.as_deref(), Some("Del"));
    }

    #[test]
    fn locate_requires_the_picked_folder_to_be_the_root_and_not_a_worktree() {
        let picked = Path::new("/work/proj/sub");
        let never = |_: &Path| false;
        assert_eq!(
            locate_outcome(picked, Ok(picked.to_path_buf()), never),
            LocateOutcome::Repository(picked.to_path_buf())
        );
        // `Repo::open` found an enclosing repository (a parent dotfiles repo).
        assert_eq!(
            locate_outcome(picked, Ok(PathBuf::from("/work")), never),
            LocateOutcome::NotRepository
        );
        assert_eq!(
            locate_outcome(
                picked,
                Err(Error::NotARepository {
                    path: picked.to_path_buf()
                }),
                never
            ),
            LocateOutcome::NotRepository
        );
        assert_eq!(
            locate_outcome(picked, Ok(picked.to_path_buf()), |_| true),
            LocateOutcome::LinkedWorktree
        );
        assert!(matches!(
            locate_outcome(
                picked,
                Err(Error::PathNotFound {
                    path: picked.to_path_buf()
                }),
                never
            ),
            LocateOutcome::Failed(_)
        ));
    }

    #[test]
    fn relocate_target_follows_the_picked_repository_root() {
        let recent = |paths: &[&str]| -> Vec<RecentRepo> {
            paths
                .iter()
                .map(|path| RecentRepo {
                    path: PathBuf::from(path),
                    last_opened: 1,
                })
                .collect()
        };
        let list = recent(&["/old/app", "/other/app"]);
        let old = Path::new("/old/app");
        assert_eq!(
            relocate_target(old, Path::new("/old/app"), &list),
            Relocation::Unchanged
        );
        assert_eq!(
            relocate_target(old, Path::new("/moved/app"), &list),
            Relocation::Moved(PathBuf::from("/moved/app"))
        );
        // The list already has the target: the entry merges into it.
        assert_eq!(
            relocate_target(old, Path::new("/other/app"), &list),
            Relocation::Merged(PathBuf::from("/other/app"))
        );
    }

    /// The × button of a row removes just that row.
    #[gpui::test]
    fn the_remove_button_removes_its_row_only(cx: &mut gpui::TestAppContext) {
        let (mut cx, paths) = open_with_recent(cx);
        cx.run_until_parked();
        let button = cx
            .debug_bounds("recent-remove-1")
            .expect("the row has a remove button");
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        let mut expected = paths.clone();
        expected.remove(1);
        assert_eq!(recent_paths(&mut cx), expected);
    }

    /// A row cached as unavailable opens with Enter once the folder is back: the
    /// key re-checks it instead of staying dead for the session.
    #[gpui::test]
    fn enter_on_an_unavailable_row_rechecks_and_opens_it_when_it_came_back(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("recent-recheck");
        let path = repo.path().to_path_buf();
        let away = path.with_extension("away");
        std::fs::rename(&path, &away).unwrap();
        let (mut cx, workspace) = open_with_paths_in_workspace(cx, std::slice::from_ref(&path));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-chip-0").is_some(),
            "not unavailable"
        );

        // Still gone: Enter keeps the row selected and the screen open, and
        // says why nothing happened.
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Confirm), cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("recent-chip-0").is_some());
        let messages: Vec<String> = cx.update(|window, cx| {
            toast::toasts(window, cx)
                .iter()
                .map(|t| t.message.to_string())
                .collect()
        });
        assert_eq!(
            messages,
            vec![format!(
                "Still unavailable: {}",
                ui::t!("welcome.recent.state.missing")
            )]
        );
        assert!(workspace.read_with(&cx, |w, _| w.repository_for_test().is_none()));

        // The folder comes back (a remounted share).
        std::fs::rename(&away, &path).unwrap();
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Confirm), cx));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(workspace.read_with(&cx, |w, _| w.repository_for_test().is_some()));
    }

    /// Activating the window again re-checks every row without a click.
    #[gpui::test]
    fn window_activation_rechecks_the_rows(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-activate");
        let path = repo.path().to_path_buf();
        let away = path.with_extension("away");
        std::fs::rename(&path, &away).unwrap();
        let (mut cx, _workspace) = open_with_paths_in_workspace(cx, std::slice::from_ref(&path));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(cx.debug_bounds("recent-chip-0").is_some());

        // Another window takes the activation, so re-activating this one is a change.
        struct Blank;
        impl Render for Blank {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        let other = cx.update(|_, cx| {
            cx.open_window(gpui::WindowOptions::default(), |_, cx| cx.new(|_| Blank))
                .expect("open the second window")
        });
        other
            .update(&mut cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();

        std::fs::rename(&away, &path).unwrap();
        // Activations within the throttle interval do not re-check.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-chip-0").is_some(),
            "a re-check ran within the throttle interval"
        );
        other
            .update(&mut cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(REPROBE_INTERVAL);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-chip-0").is_none(),
            "the row stayed unavailable after the window was activated"
        );
    }

    /// A linked worktree recorded by an older version is removed from the
    /// stored list once the background check has seen it.
    #[gpui::test]
    fn linked_worktree_entries_are_not_listed(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-worktree");
        let wt = repo.path().join("wt");
        let gitdir = repo.path().join(".git/worktrees/wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let paths = vec![wt, repo.path().to_path_buf()];
        let mut cx = open_with_paths(cx, &paths);
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(cx.debug_bounds("recent-row-0").is_some());
        assert!(
            cx.debug_bounds("recent-row-1").is_none(),
            "the linked worktree is still listed"
        );
        assert_eq!(recent_paths(&mut cx), vec![paths[1].clone()]);
    }

    /// Undo still restores the entries after the welcome view is gone (a
    /// repository opened within the toast's lifetime).
    #[gpui::test]
    fn undo_works_after_the_welcome_view_is_gone(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("recent-undo-gone");
        let (mut cx, paths) = open_mixed(cx, &repo);
        cx.run_until_parked();
        cx.run_until_parked();
        click_selector(&mut cx, "recent-cleanup-remove");
        assert_eq!(recent_paths(&mut cx), vec![paths[0].clone()]);
        let action = cx.update(|window, cx| {
            toast::toasts(window, cx)[0]
                .body
                .as_ref()
                .and_then(|body| body.action.clone())
                .expect("the toast has an Undo button")
        });

        // Opening the repository replaces the welcome screen.
        click_row(&mut cx, 0, gpui::Modifiers::default());
        cx.update(|window, cx| window.dispatch_action(Box::new(ui::actions::Confirm), cx));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-filter").is_none(),
            "the welcome screen is still up"
        );

        cx.update(|window, cx| (action.on_click)(window, cx));
        cx.run_until_parked();
        let restored = recent_paths(&mut cx);
        assert!(
            paths[1..].iter().all(|path| restored.contains(path)),
            "Undo did nothing: {restored:?}"
        );
    }

    /// Shift+↓ extends the list's selection only while the list is focused; in
    /// the filter field it must leave the selection alone.
    #[gpui::test]
    fn shift_arrows_in_the_filter_field_do_not_change_the_selection(cx: &mut gpui::TestAppContext) {
        let (mut cx, _paths) = open_with_recent(cx);
        cx.run_until_parked();
        let filter = cx.debug_bounds("recent-filter").expect("filter");
        cx.simulate_click(filter.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.dispatch_action(Box::new(ExtendNext), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-footer-selected").is_none(),
            "Shift+Down in the filter extended the list selection"
        );

        click_row(&mut cx, 0, gpui::Modifiers::default());
        cx.update(|window, cx| window.dispatch_action(Box::new(ExtendNext), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("recent-footer-selected").is_some(),
            "Shift+Down in the list did not extend the selection"
        );
    }

    /// Shift+F10 / the Menu key open the cursor row's menu with its first item
    /// highlighted, so Enter runs it (Open) at once.
    #[gpui::test]
    fn the_menu_key_opens_the_cursor_rows_menu_and_enter_runs_its_first_item(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("recent-menu-key");
        let (mut cx, workspace) =
            open_with_paths_in_workspace(cx, std::slice::from_ref(&repo.path().to_path_buf()));
        cx.run_until_parked();
        cx.run_until_parked();
        cx.update(|window, cx| window.dispatch_action(Box::new(OpenRowMenu), cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|window, cx| overlay::layer_count(window, cx)), 1);
        assert!(workspace.read_with(&cx, |w, _| w.repository_for_test().is_none()));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            workspace.read_with(&cx, |w, _| w.repository_for_test().is_some()),
            "Enter did not run the menu's first item"
        );
    }

    /// Ctrl+Z restores the last removal while its toast is visible, and only once.
    #[gpui::test]
    fn undo_removal_restores_the_entries_once(cx: &mut gpui::TestAppContext) {
        let (mut cx, paths) = open_with_recent(cx);
        cx.run_until_parked();
        cx.update(|window, cx| window.dispatch_action(Box::new(RemoveSelected), cx));
        cx.run_until_parked();
        assert_eq!(recent_paths(&mut cx), paths[1..].to_vec());

        cx.update(|window, cx| window.dispatch_action(Box::new(UndoRemoval), cx));
        cx.run_until_parked();
        assert_eq!(recent_paths(&mut cx), paths);
        assert_eq!(
            cx.update(|window, cx| toast::count(window, cx)),
            0,
            "the Undo toast stayed after Ctrl+Z"
        );
        // Nothing left to undo: a second press changes nothing.
        cx.update(|window, cx| window.dispatch_action(Box::new(RemoveSelected), cx));
        cx.run_until_parked();
        cx.executor().advance_clock(UNDO_LIFETIME);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(cx.update(|window, cx| toast::count(window, cx)), 0);
        cx.update(|window, cx| window.dispatch_action(Box::new(UndoRemoval), cx));
        cx.run_until_parked();
        assert_eq!(
            recent_paths(&mut cx),
            paths[1..].to_vec(),
            "Ctrl+Z restored after the toast had expired"
        );
    }

    /// At most `MAX_PROBES` checks run at once; the rest wait and still finish.
    #[gpui::test]
    fn only_a_few_checks_run_at_once(cx: &mut gpui::TestAppContext) {
        let paths: Vec<PathBuf> = (0..10)
            .map(|n| PathBuf::from(format!("/nonexistent-recent/{n}")))
            .collect();
        let (mut cx, workspace) = open_with_paths_in_workspace(cx, &paths);
        let view = workspace.read_with(&cx, |w, _| w.welcome_for_test().expect("welcome"));
        // Window activation re-checks every row: the slots fill, the rest queue.
        view.update_in(&mut cx, |view, window, cx| {
            view.states.clear();
            view.probing.clear();
            view.probe_queue.clear();
            view.last_reprobe = None;
            view.reprobe_all(window, cx);
            assert_eq!(view.probing.len(), MAX_PROBES);
            assert_eq!(view.probe_queue.len(), paths.len() - MAX_PROBES);
        });
        cx.run_until_parked();
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert!(view.probing.is_empty() && view.probe_queue.is_empty());
            assert_eq!(view.states.len(), paths.len(), "a queued check never ran");
        });
    }
}
