//! The repository shell: toolbar, sidebar, history, details dock and status bar.
//!
//! One [`RepositoryView`] lives per open repository; the [`Workspace`] owns it
//! and swaps it for the welcome screen. Everything that belongs to a later
//! stage is a placeholder or a disabled control, never a dead click.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, size, uniform_list, AnyElement, App, AppContext, Bounds,
    ClickEvent, ClipboardItem, Context, Decorations, Div, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, ListHorizontalSizingBehavior, MouseButton, ParentElement,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    StyledText, Subscription, Task, TextRun, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::network::{FetchOptions, Progress};
use git_core::{ChangedFile, CommitDetails, CommitInfo, Error, Git, Head, Oid, RefKind, Repo};
use ui::actions::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::metrics::{DOCK_HEADER, FONT_MONO, FONT_UI, FS_BASE, FS_MD, FS_SM, R_SM, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    menu, toast, Avatar, Button, CountPill, Counter, FileRow, Icon, IconName, MenuEntry, MenuItem,
    Scrollbar, SearchField, SegmentItem, Segmented, Skeleton, Spinner, Splitter, TextInput,
    TextInputEvent, Tooltip, Tree, TreeNodeData,
};

use crate::activity_strip::ActivityStrip;
use crate::changes_view::{ChangesEvent, ChangesView};
use crate::details::{read_details, read_diff, DetailsState, DiffState};
use crate::diff_model::{DiffRow, RowKind, SplitCell, SplitCellKind, SplitRow};
use crate::format::{absolute_time, format_count, relative_time};
use crate::history_state::LoadState;
use crate::history_view::HistoryView;
use crate::network_ops::{self, Job};
use crate::network_service::NetworkService;
use crate::palette::{self, Command};
use crate::ref_dialog::{self, RefDialog, RefDialogContext, RefDialogKind, RemoteEdit};
use crate::ref_ops::{CommitCard, RefAction};
use crate::settings::{
    sidebar_width_limit, DiffLayout, Dock, SIDEBAR_WIDTH_DEFAULT, SIDEBAR_WIDTH_MAX,
    SIDEBAR_WIDTH_MIN,
};
use crate::sidebar::{
    commit_for, read_sidebar_extras, shortcut_action, sidebar_menu, sidebar_nodes, Expansion,
    SidebarAction, SidebarExtras, SidebarMenuEntry,
};
use crate::syntax::{self, HighlightSpan, Highlighted, Token};
use crate::workspace::{
    git_label, git_tooltip, repo_name, titlebar, titlebar_end, titlebar_fill, Workspace,
};
use crate::worktree_dialog::SelectedCommit;
use crate::worktrees_view::{WorktreesEvent, WorktreesView};

actions!(
    gitwarp,
    [
        ToggleSidebar,
        ToggleDetails,
        FocusSearch,
        SearchNext,
        SearchPrev,
        SearchClear,
        SearchFocusList,
        OpenRepoSwitcher,
        OpenBranchSwitcher,
        StashChanges,
        FetchAll,
        PullBranch,
        PushBranch
    ]
);

/// The centre views the sidebar and toolbar switch between. `Worktrees` is
/// opened from the sidebar's Worktrees section, so the toolbar's History /
/// Changes switch stays on History while it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    History,
    Changes,
    Worktrees,
}

/// Binds the search field's own keys in its `history-search` context, so
/// Enter/F3/Esc/↓ never fire while another input has the keyboard. Enter is
/// handled through the input's own `Submit` event instead. Called once at
/// startup.
pub(crate) fn bind_search_keys(cx: &mut App) {
    cx.bind_keys([
        ui::actions::key_binding(cx, "shift-enter", SearchPrev, Some("history-search")),
        ui::actions::key_binding(cx, "f3", SearchNext, Some("history-search")),
        ui::actions::key_binding(cx, "shift-f3", SearchPrev, Some("history-search")),
        ui::actions::key_binding(cx, "escape", SearchClear, Some("history-search")),
        ui::actions::key_binding(cx, "down", SearchFocusList, Some("history-search")),
    ]);
}

/// Height of a toolbar tool button.
const TOOL_H: f32 = 28.;

/// What the toolbar shows at a given window width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarLayout {
    /// Labels on the tool buttons and the segmented view switch.
    pub labels: bool,
    /// The Ctrl K hint inside the search field.
    pub search_kbd: bool,
    /// Widest the repository name may get; `None` hides it.
    pub repo_name: Option<f32>,
    /// Widest the branch name may get; `None` hides it.
    pub branch_name: Option<f32>,
    /// The search field's flex basis.
    pub search_basis: f32,
    /// The search field's minimum width.
    pub search_min: f32,
    /// Gap between the toolbar items.
    pub gap: f32,
    /// Horizontal padding of the bar.
    pub padding: f32,
    /// Horizontal padding of the two switchers and the tool buttons.
    pub item_pad: f32,
}

impl ToolbarLayout {
    fn full() -> Self {
        Self {
            labels: true,
            search_kbd: true,
            repo_name: Some(96.),
            branch_name: Some(96.),
            search_basis: 270.,
            search_min: 150.,
            gap: 6.,
            padding: 12.,
            item_pad: 8.,
        }
    }

    fn compact(self) -> Self {
        Self {
            gap: 4.,
            item_pad: 7.,
            ..self
        }
    }
}

/// The toolbar steps, measured against the real text widths of every item.
///
/// The window's client-side frame keeps a 10px shadow band and a 1px border on
/// each side, so a window of `width` has `width - 22` of content. Every item
/// except the search field is `flex-none`, so the search is the only item that
/// can shrink; the steps below keep the worst case (names at their maximum,
/// `Fetch`/`Pull`/`Push`/`Stash`/`History`/`Changes` labelled) inside the bar.
///
/// The labels are dropped from 1360 down: at 1200 a long repository name plus
/// the segmented labels do not fit even with the search at its minimum, which
/// clipped "Changes".
pub fn toolbar_layout(width: f32) -> ToolbarLayout {
    let full = ToolbarLayout::full();
    if width >= 1360. {
        // The design's own 140px cap on both names.
        ToolbarLayout {
            repo_name: Some(140.),
            branch_name: Some(140.),
            ..full
        }
    } else if width >= 1000. {
        // Icon-only tools and view switch, so the bar fits at 1200.
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            ..full
        }
    } else if width >= 870. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            search_basis: 180.,
            search_min: 120.,
            ..full
        }
    } else if width >= 810. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
    } else if width >= 760. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: Some(64.),
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
        .compact()
    } else if width >= 700. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: None,
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
        .compact()
    } else {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: None,
            search_basis: 72.,
            search_min: 72.,
            padding: 8.,
            ..full
        }
        .compact()
    }
}

/// Where the details dock sits: the stored choice, else by window width.
pub fn resolve_dock(setting: Option<Dock>, width: f32) -> Dock {
    setting.unwrap_or(if width >= 1400. {
        Dock::Right
    } else {
        Dock::Bottom
    })
}

/// The default size of the details panel for a dock.
pub fn dock_default(dock: Dock) -> f32 {
    match dock {
        Dock::Bottom => 340.,
        Dock::Right => 640.,
    }
}

/// The smallest and largest size of the details panel for a dock.
pub fn dock_bounds(dock: Dock) -> (f32, f32) {
    match dock {
        Dock::Bottom => (220., 560.),
        Dock::Right => (220., 900.),
    }
}

/// Clamps a stored size to the current dock's range.
pub fn clamp_details(size: f32, dock: Dock) -> f32 {
    let (min, max) = dock_bounds(dock);
    if size.is_finite() {
        size.clamp(min, max)
    } else {
        dock_default(dock)
    }
}

/// A right-docked details panel never takes more than this share of the space
/// right of the sidebar.
const DETAILS_MAX_SHARE: f32 = 0.5;

/// The widest a right-docked details panel may render: half of the space right
/// of the sidebar, within the panel's own 220–900 range.
///
/// `sidebar_width` is the width the sidebar actually renders at (0 while it is
/// collapsed).
pub fn details_width_limit(window_width: f32, sidebar_width: f32) -> f32 {
    let (min, max) = dock_bounds(Dock::Right);
    (DETAILS_MAX_SHARE * (window_width - sidebar_width)).clamp(min, max)
}

/// The width a right-docked details panel renders at: its stored size, capped
/// by [`details_width_limit`] and never below its 220 px minimum.
///
/// The stored size is not rewritten; only the rendered width shrinks.
pub fn effective_details_width(stored: f32, window_width: f32, sidebar_width: f32) -> f32 {
    clamp_details(stored, Dock::Right).min(details_width_limit(window_width, sidebar_width))
}

/// The width the sidebar renders at: its stored width, capped to half the
/// visible area and never below its own 160 px minimum.
///
/// The stored width is kept as saved; only the rendered width shrinks.
pub fn effective_sidebar_width(stored: f32, window_width: f32) -> f32 {
    stored.clamp(SIDEBAR_WIDTH_MIN, sidebar_width_limit(window_width))
}

pub struct RepositoryView {
    /// The path the view was opened for, so the workspace can reopen it.
    path: PathBuf,
    /// Canonical root, for the "current" marker in the repository menu.
    root: PathBuf,
    repo_name: SharedString,
    /// `git 2.56.0 · bundled`, or `git not found`.
    git_label: SharedString,
    /// The informational tooltip of the status-bar git label.
    git_tooltip: SharedString,
    history: Entity<HistoryView>,
    _history_observer: Subscription,
    /// The commit heatmap above the history table.
    activity: Entity<ActivityStrip>,
    changes: Entity<ChangesView>,
    _changes_observer: Subscription,
    worktrees: Entity<WorktreesView>,
    _worktrees_observer: Subscription,
    view: ViewMode,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    search: Entity<TextInput>,
    search_focus: FocusHandle,
    /// The search field's text and submit events, forwarded to the history.
    _search_events: Subscription,
    sidebar_filter: Entity<TextInput>,
    sidebar_open: bool,
    sidebar_width: f32,
    /// The sidebar tree's scroll state, shared with its horizontal scrollbar.
    sidebar_scroll: ScrollHandle,
    /// The sidebar's expansion overrides over the defaults.
    sidebar: Expansion,
    /// The sidebar filter field's text.
    filter: String,
    /// The sidebar's own lists; `None` while they load.
    sidebar_extras: Option<SidebarExtras>,
    /// The root the lists belong to, so they load once per root.
    sidebar_root: Option<PathBuf>,
    /// Bumped per load; an older result that finishes later is dropped.
    sidebar_generation: u64,
    /// The sidebar filter's text events.
    _sidebar_filter_events: Subscription,
    /// The resolved git, for the sidebar's own background reads.
    git: Result<Arc<Git>, Arc<Error>>,
    selected_node: Option<SharedString>,
    dock: Dock,
    details_open: bool,
    details_size: f32,
    /// What the selected commit's details dock shows.
    details: DetailsState,
    /// The commit `details` belongs to; `None` while nothing is selected.
    details_oid: Option<Oid>,
    /// Bumped per details load; an older result that finishes later is dropped.
    details_generation: u64,
    /// The row of `details.files()` the diff pane shows.
    selected_file: Option<usize>,
    /// The selected file's diff.
    diff: DiffState,
    /// Bumped per diff load; an older result that finishes later is dropped.
    diff_generation: u64,
    diff_scroll: UniformListScrollHandle,
    /// Advance of one mono character at [`MONO`], for the diff's scroll width.
    mono_cell: Pixels,
    /// Unified or side-by-side, persisted in the settings.
    diff_layout: DiffLayout,
    /// Syntax spans per (commit, path), so revisiting a file is free.
    highlight_cache: HashMap<(Oid, String), Arc<Highlighted>>,
    info_scroll: ScrollHandle,
    files_scroll: ScrollHandle,
    /// The file list's keyboard focus; arrow keys move the file selection only
    /// while it is focused.
    files_focus: FocusHandle,
    /// Window bounds of the two dropdown anchors, captured while painting.
    repo_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    branch_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The stage 7 dialog currently open, if any.
    ref_dialog: Option<Entity<RefDialog>>,
    /// A ref operation without a dialog (checkout, stash apply, …) is running.
    ref_action_running: bool,
    /// The network action that is running, if any.
    network: Option<NetworkActivity>,
    /// Bumped per network job; an older poll loop stops.
    network_generation: u64,
    /// Whether the background fetch is on, mirrored in the settings.
    background_fetch: bool,
    /// Pause between background fetches, from the settings.
    background_fetch_interval: Duration,
    /// The repeating background-fetch task.
    background_fetch_task: Option<Task<()>>,
    /// A background fetch is in flight; the next tick skips while it runs.
    background_fetch_running: bool,
}

/// What the status bar and the toolbar show while git is talking to a remote.
struct NetworkActivity {
    verb: &'static str,
    detail: SharedString,
    progress: Option<f32>,
}

/// One event from the worker to the UI.
enum NetworkEvent {
    Progress(Progress),
    Done(Result<git_core::network::NetworkOutcome, Error>),
}

/// The first background fetch waits a little, so opening a repository is not
/// immediately followed by network traffic.
const BACKGROUND_FETCH_FIRST: Duration = Duration::from_secs(30);
/// How often the UI drains the job's progress channel.
const NETWORK_POLL: Duration = Duration::from_millis(80);

impl RepositoryView {
    pub fn new(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Canonicalising is filesystem work; the history view reports the
        // real root as soon as `Repo::open` finished and the observer below
        // adopts it. Until then the given path is the best answer.
        let root = path.clone();
        let name = repo_name(&path);
        let git_label = git_label(&git);
        let git_tooltip = git_tooltip(&git);
        let action_view = cx.entity();
        let on_history_action = Rc::new(
            move |action: RefAction, window: &mut Window, cx: &mut App| {
                action_view.update(cx, |view, cx| {
                    view.handle_history_action(action, window, cx);
                });
            },
        );
        let history = cx.new(|cx| {
            HistoryView::open(
                path.clone(),
                git.clone(),
                workspace.clone(),
                on_history_action,
                cx,
            )
        });
        let observer = cx.observe(&history, |this, history, cx| {
            // A repository opened from a subdirectory is remembered by its
            // root, so the switcher's "current" marker must follow it.
            let (root, selected) = {
                let history = history.read(cx);
                (
                    history.root().map(std::path::Path::to_path_buf),
                    history.selected_commit(),
                )
            };
            if let Some(root) = root {
                if this.root != root {
                    this.root = root.clone();
                    this.repo_name = repo_name(&root);
                    this.worktrees
                        .update(cx, |worktrees, cx| worktrees.set_root(root.clone(), cx));
                }
                // The sidebar's own lists need the root: load them once, when
                // the history first reports it.
                this.ensure_sidebar(cx);
            }
            this.sync_details(selected, cx);
            cx.notify();
        });
        let activity = cx.new(|cx| {
            ActivityStrip::new(
                path.clone(),
                git.clone(),
                workspace.clone(),
                history.clone(),
                cx,
            )
        });
        let changes = cx.new(|cx| ChangesView::new(path.clone(), git.clone(), window, cx));
        let changes_observer = cx.subscribe_in(
            &changes,
            window,
            |this, _, event: &ChangesEvent, window, cx| match event {
                ChangesEvent::Committed => {
                    this.reload_history(cx);
                }
                ChangesEvent::PushAfterCommit => this.push_branch(window, cx),
            },
        );
        let worktrees = cx
            .new(|cx| WorktreesView::new(path.clone(), git.clone(), workspace.clone(), window, cx));
        let worktrees_observer = cx.subscribe(&worktrees, |this, _, _: &WorktreesEvent, cx| {
            // A branch was created or deleted, or a worktree changed: the
            // sidebar, the history's ref labels and the local changes all
            // read the same repository.
            this.reload_history(cx);
            this.changes.update(cx, |changes, cx| changes.refresh(cx));
            this.refresh(cx);
            cx.notify();
        });
        let search = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Search commits, authors, hashes", cx);
            input
        });
        let search_focus = search.read(cx).focus_handle().clone();
        let search_events = cx.subscribe(&search, |this, _, event: &TextInputEvent, cx| {
            match event {
                TextInputEvent::Changed(text) => {
                    this.history
                        .update(cx, |history, cx| history.set_search_query(text.clone(), cx));
                }
                // Enter in the field: the next match, wrapping.
                TextInputEvent::Submit => {
                    this.history
                        .update(cx, |history, cx| history.search_step(true, cx));
                }
            }
        });
        let sidebar_filter = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Filter branches, tags…", cx);
            input
        });
        let sidebar_filter_events =
            cx.subscribe(&sidebar_filter, |this, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(text) = event {
                    this.filter = text.clone();
                    cx.notify();
                }
            });

        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let dock = resolve_dock(settings.dock, f32::from(window.viewport_size().width));
        let details_size = settings
            .details_size
            .map(|size| clamp_details(size, dock))
            .unwrap_or_else(|| dock_default(dock));
        let focus = cx.focus_handle().tab_stop(false);

        let mut view = Self {
            path,
            root,
            repo_name: name,
            git_label,
            git_tooltip,
            history,
            _history_observer: observer,
            activity,
            changes,
            _changes_observer: changes_observer,
            worktrees,
            _worktrees_observer: worktrees_observer,
            view: ViewMode::History,
            workspace,
            focus: focus.clone(),
            search,
            search_focus,
            _search_events: search_events,
            sidebar_filter,
            sidebar_open: true,
            sidebar_width: settings
                .sidebar_width
                .clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX),
            sidebar_scroll: ScrollHandle::new(),
            sidebar: Expansion::default(),
            filter: String::new(),
            sidebar_extras: None,
            sidebar_root: None,
            sidebar_generation: 0,
            _sidebar_filter_events: sidebar_filter_events,
            git,
            selected_node: None,
            dock,
            details_open: true,
            details_size,
            details: DetailsState::Empty,
            details_oid: None,
            details_generation: 0,
            selected_file: None,
            diff: DiffState::Empty,
            diff_generation: 0,
            diff_scroll: UniformListScrollHandle::new(),
            mono_cell: window
                .text_system()
                .ch_advance(
                    window.text_system().resolve_font(&gpui::font(FONT_MONO)),
                    px(f32::from(settings.mono_font_size)),
                )
                .unwrap_or(px(7.2)),
            diff_layout: settings.diff_layout,
            highlight_cache: HashMap::new(),
            info_scroll: ScrollHandle::new(),
            files_scroll: ScrollHandle::new(),
            files_focus: cx.focus_handle().tab_stop(true),
            repo_bounds: Rc::new(Cell::new(None)),
            branch_bounds: Rc::new(Cell::new(None)),
            ref_dialog: None,
            ref_action_running: false,
            network: None,
            network_generation: 0,
            background_fetch: settings.background_fetch,
            background_fetch_interval: Duration::from_secs(
                u64::from(settings.fetch_interval_minutes) * 60,
            ),
            background_fetch_task: None,
            background_fetch_running: false,
        };
        view.start_background_fetch(cx);
        view
    }

    /// The focus handle of the repository root; the workspace focuses it after
    /// opening, so the keyboard lands in the new content.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The focus handle of the toolbar search field.
    pub fn search_focus(&self) -> FocusHandle {
        self.search_focus.clone()
    }

    /// The path this view was opened for; reopened after a git retry.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    pub(crate) fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    /// Switches the centre pane and moves the keyboard to the new content.
    fn set_view(&mut self, mode: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == mode {
            // Re-opening the Worktrees page is also a request for fresh
            // numbers; the other views keep their loaded state.
            if mode == ViewMode::Worktrees {
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.refresh(cx));
                let focus = self.worktrees.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            return;
        }
        self.view = mode;
        match mode {
            ViewMode::Changes => {
                self.changes.update(cx, |changes, cx| changes.refresh(cx));
                let focus = self.changes.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            ViewMode::Worktrees => {
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.refresh(cx));
                let focus = self.worktrees.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            ViewMode::History => {
                let focus = self.history.read(cx).list_focus();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    /// Reloads the history and the activity counts that depend on it.
    fn reload_history(&mut self, cx: &mut Context<Self>) {
        self.history.update(cx, |history, cx| history.reload(cx));
        self.activity.update(cx, |activity, cx| activity.reload(cx));
    }

    /// Switches to Local changes, as the toolbar's segment does.
    pub(crate) fn show_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Changes, window, cx);
    }

    /// The history view, for the search's focus hand-off, the welcome screen's
    /// day filter and the tests.
    pub(crate) fn history(&self) -> Entity<HistoryView> {
        self.history.clone()
    }

    /// Enter / F3 in the search field: the next match.
    fn search_next(&mut self, cx: &mut Context<Self>) {
        self.history
            .update(cx, |history, cx| history.search_step(true, cx));
    }

    /// Shift+Enter / Shift+F3: the previous match.
    fn search_prev(&mut self, cx: &mut Context<Self>) {
        self.history
            .update(cx, |history, cx| history.search_step(false, cx));
    }

    /// Esc: clear the field's text (which clears the search) and give the list
    /// the keyboard, keeping its selection.
    fn search_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.history
            .update(cx, |history, cx| history.clear_search(cx));
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
    }

    /// ↓ in the field: hand the keyboard to the list without moving the
    /// selection.
    fn search_focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
    }

    fn set_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        if self.dock != dock {
            self.dock = dock;
            // The size the user dragged stays: it is clamped to the new
            // dock's range when rendered, never overwritten.
        }
        self.details_open = true;
        crate::update_settings(cx, |settings| settings.dock = Some(dock));
        cx.notify();
    }

    fn set_sidebar_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        self.sidebar_width = f32::from(width).clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
        cx.notify();
    }

    fn set_details_size(&mut self, size: Pixels, cx: &mut Context<Self>) {
        self.details_size = clamp_details(f32::from(size), self.dock);
        cx.notify();
    }

    /// Saves both panel sizes once a splitter drag ends.
    fn persist_layout(&mut self, cx: &mut Context<Self>) {
        let sidebar_width = self.sidebar_width;
        let details_size = self.details_size;
        crate::update_settings(cx, |settings| {
            settings.sidebar_width = sidebar_width;
            settings.details_size = Some(details_size);
        });
    }

    fn reset_details_size(&mut self, cx: &mut Context<Self>) {
        self.details_size = dock_default(self.dock);
        let size = self.details_size;
        crate::update_settings(cx, |settings| settings.details_size = Some(size));
        cx.notify();
    }

    /// Flips the expansion of any expandable sidebar row, by key. `current`
    /// is the row's effective state as it was built.
    fn toggle_node(&mut self, key: &str, current: bool, cx: &mut Context<Self>) {
        // While filtering every expandable row is forced open. Recording a
        // flip here would silently change the state restored when the filter
        // is cleared, so expansion toggles are ignored until then.
        if !self.filter.trim().is_empty() {
            return;
        }
        self.sidebar.toggle(key, current);
        cx.notify();
    }

    /// Click / Enter / Space on a leaf: selects it and reveals its commit in
    /// the history. Folders, remotes and sections only toggle, so they never
    /// reach this.
    fn activate_node(&mut self, key: &str, cx: &mut Context<Self>) {
        self.selected_node = Some(key.into());
        self.reveal_node(key, cx);
        cx.notify();
    }

    /// Jumps the history to the commit a leaf points at, when it has one.
    fn reveal_node(&mut self, key: &str, cx: &mut Context<Self>) {
        let oid = {
            let history = self.history.read(cx);
            commit_for(
                key,
                history.refs().unwrap_or(&[]),
                self.sidebar_extras.as_ref(),
            )
        };
        if let Some(oid) = oid {
            // A sidebar leaf always shows its commit in the history; the
            // Worktrees page is left when one of its rows is selected.
            if self.view == ViewMode::Worktrees {
                self.view = ViewMode::History;
            }
            self.history
                .update(cx, |history, cx| history.reveal_commit(oid, cx));
        }
    }

    /// Double-click: a worktree opens the Worktrees page; a local branch,
    /// remote branch or tag checks out through the same path as its menu.
    fn open_sidebar_node(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = key.strip_prefix("wt:") {
            let _ = path;
            self.set_view(ViewMode::Worktrees, window, cx);
            return;
        }
        let action = if let Some(name) = key.strip_prefix("local:") {
            Some(SidebarAction::CheckoutBranch(name.to_owned()))
        } else if let Some(name) = key.strip_prefix("rbranch:") {
            Some(SidebarAction::CheckoutRemoteBranch(name.to_owned()))
        } else {
            key.strip_prefix("tag:")
                .map(|name| SidebarAction::CheckoutTag(name.to_owned()))
        };
        if let Some(action) = action {
            self.run_sidebar_action(action, window, cx);
        }
    }

    /// Enter / F2 / Delete on a sidebar row: runs the menu item carrying that
    /// shortcut hint. Returns whether the row has one (and so took the key).
    fn run_sidebar_shortcut(
        &mut self,
        key: &str,
        hint: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let action = {
            let history = self.history.read(cx);
            shortcut_action(
                key,
                hint,
                history.head(),
                self.sidebar_extras.as_ref(),
                history.refs(),
            )
        };
        match action {
            Some(action) => {
                self.run_sidebar_action(action, window, cx);
                true
            }
            None => false,
        }
    }

    /// Right-click on a sidebar row: selects it (without the jump) and opens
    /// its kind's menu at the pointer. A kind without a menu does nothing.
    fn open_sidebar_menu(
        &mut self,
        key: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = {
            let history = self.history.read(cx);
            sidebar_menu(
                key,
                history.head(),
                self.sidebar_extras.as_ref(),
                history.refs(),
            )
        };
        if model.is_empty() {
            return;
        }
        self.selected_node = Some(key.into());
        let view = cx.entity();
        let entries: Vec<MenuEntry> = model
            .into_iter()
            .map(|entry| match entry {
                SidebarMenuEntry::Separator => MenuEntry::Separator,
                SidebarMenuEntry::Item(item) => {
                    let mut menu_item = MenuItem::new(item.label)
                        .icon(item.icon)
                        .disabled(!item.enabled);
                    if let Some(shortcut) = item.shortcut {
                        menu_item = menu_item.shortcut(shortcut);
                    }
                    if item.danger {
                        menu_item = menu_item.danger();
                    }
                    if !matches!(item.action, SidebarAction::None) {
                        let action = item.action.clone();
                        let view = view.clone();
                        menu_item = menu_item.action(move |window, cx| {
                            view.update(cx, |view, cx| {
                                view.run_sidebar_action(action.clone(), window, cx);
                            });
                        });
                    }
                    MenuEntry::Item(menu_item)
                }
            })
            .collect();
        menu::open_context_menu(position, entries, window, cx);
        cx.notify();
    }

    /// Runs one stage 7 sidebar action: the checkout/stash operations start at
    /// once, the rest open a dialog.
    fn run_sidebar_action(
        &mut self,
        action: SidebarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SidebarAction::None => {}
            SidebarAction::CopyText(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            SidebarAction::Refresh => {
                self.refresh(cx);
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.refresh(cx));
            }
            SidebarAction::CheckoutBranch(name) => {
                self.checkout_branch(name, window, cx);
            }
            SidebarAction::CheckoutRemoteBranch(remote) => {
                let label = remote.clone();
                self.checkout_guarded(
                    format!("Check out {label}?"),
                    move |repo| {
                        repo.checkout_remote(&remote)?;
                        Ok((ui::ToastKind::Ok, format!("Checked out {label}")))
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::CheckoutTag(name) => {
                let target = {
                    let history = self.history.read(cx);
                    commit_for(
                        &format!("tag:{name}"),
                        history.refs().unwrap_or(&[]),
                        self.sidebar_extras.as_ref(),
                    )
                };
                let Some(target) = target else {
                    toast::show(
                        ui::ToastKind::Warn,
                        "The tag has no commit to check out",
                        window,
                        cx,
                    );
                    return;
                };
                let label = name.clone();
                self.checkout_guarded(
                    format!("Check out tag {label}?"),
                    move |repo| {
                        repo.checkout_detached(target)?;
                        Ok((
                            ui::ToastKind::Ok,
                            format!("Checked out tag {label} (detached HEAD)"),
                        ))
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::CreateBranchAt(oid) => {
                let card = self.card_for(oid, cx);
                self.open_ref_dialog(RefDialogKind::CreateBranch { card }, window, cx);
            }
            SidebarAction::RenameBranch(old) => {
                self.open_ref_dialog(RefDialogKind::RenameBranch { old }, window, cx);
            }
            SidebarAction::DeleteBranch(name) => {
                self.open_ref_dialog(
                    RefDialogKind::DeleteBranch { name, force: false },
                    window,
                    cx,
                );
            }
            SidebarAction::RenameTag(old) => {
                self.open_ref_dialog(RefDialogKind::RenameTag { old }, window, cx);
            }
            SidebarAction::DeleteTag(name) => {
                self.open_ref_dialog(RefDialogKind::DeleteTag { name }, window, cx);
            }
            SidebarAction::ApplyStash(index) => {
                self.run_repo_action(window, cx, move |repo| {
                    repo.stash_apply(index)?;
                    Ok((ui::ToastKind::Ok, format!("Applied stash@{{{index}}}")))
                });
            }
            SidebarAction::PopStash(index) => {
                self.run_repo_action(window, cx, move |repo| {
                    repo.stash_pop(index)?;
                    Ok((ui::ToastKind::Ok, format!("Popped stash@{{{index}}}")))
                });
            }
            SidebarAction::ViewStash(index) => {
                let oid = self
                    .sidebar_extras
                    .as_ref()
                    .and_then(|extras| extras.stashes.iter().find(|stash| stash.index == index))
                    .map(|stash| stash.oid);
                match oid {
                    Some(oid) => self.start_details(oid, cx),
                    None => {
                        toast::show(
                            ui::ToastKind::Warn,
                            format!("stash@{{{index}}} no longer exists"),
                            window,
                            cx,
                        );
                    }
                }
            }
            SidebarAction::UpdateSubmodule(path) => {
                self.start_network_job(Job::UpdateSubmodule { path }, window, cx);
            }
            SidebarAction::OpenSubmodule(path) => {
                let folder = self.root.join(&path);
                self.workspace
                    .update(cx, |workspace, cx| {
                        workspace.open_repository_window(folder, window, cx);
                    })
                    .ok();
                toast::show(
                    ui::ToastKind::Ok,
                    format!("Opened {path} in a new window"),
                    window,
                    cx,
                );
            }
            SidebarAction::DropStash(index) => {
                let label = self
                    .sidebar_extras
                    .as_ref()
                    .and_then(|extras| {
                        extras
                            .stashes
                            .iter()
                            .find(|stash| stash.index == index)
                            .map(|stash| stash.message.clone())
                    })
                    .unwrap_or_else(|| format!("stash@{{{index}}}"));
                self.open_ref_dialog(RefDialogKind::DropStash { index, label }, window, cx);
            }
            SidebarAction::CreateTag => {
                // "Here" is the selected commit, as in the design, with HEAD
                // as the fallback when nothing is selected yet.
                let oid = self
                    .history
                    .read(cx)
                    .selected_commit()
                    .map(|commit| commit.id)
                    .or_else(|| self.head_oid(cx));
                let Some(oid) = oid else {
                    toast::show(
                        ui::ToastKind::Warn,
                        "Create a commit before tagging",
                        window,
                        cx,
                    );
                    return;
                };
                let card = self.card_for(oid, cx);
                self.open_ref_dialog(RefDialogKind::CreateTag { card }, window, cx);
            }
            SidebarAction::AddRemote => {
                self.open_ref_dialog(RefDialogKind::Remote { editing: None }, window, cx);
            }
            SidebarAction::EditRemote(name) => {
                let url = self
                    .sidebar_extras
                    .as_ref()
                    .and_then(|extras| {
                        extras
                            .remote_urls
                            .iter()
                            .find(|(remote, _)| *remote == name)
                            .map(|(_, url)| url.clone())
                    })
                    .unwrap_or_default();
                self.open_ref_dialog(
                    RefDialogKind::Remote {
                        editing: Some(RemoteEdit { name, url }),
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::RemoveRemote(name) => {
                self.open_ref_dialog(RefDialogKind::RemoveRemote { name }, window, cx);
            }
            SidebarAction::FetchRemote(name) => {
                self.start_network_job(
                    Job::Fetch(FetchOptions {
                        remote: Some(name),
                        tags: true,
                        prune: false,
                    }),
                    window,
                    cx,
                );
            }
            SidebarAction::FetchAll => self.fetch_remotes(window, cx),
            SidebarAction::FetchPrune => {
                self.start_network_job(
                    Job::Fetch(FetchOptions {
                        remote: None,
                        tags: true,
                        prune: true,
                    }),
                    window,
                    cx,
                );
            }
            SidebarAction::PushTag(tag) => {
                let remote = self.default_remote_name();
                self.start_network_job(Job::PushTag { remote, tag }, window, cx);
            }
            SidebarAction::PushTags => {
                let remote = self.default_remote_name();
                self.start_network_job(Job::PushTags { remote }, window, cx);
            }
            SidebarAction::DeleteRemoteBranch(name) => {
                let Some((remote, branch)) = name.split_once('/') else {
                    toast::show(
                        ui::ToastKind::Warn,
                        format!("Cannot tell which remote {name} belongs to"),
                        window,
                        cx,
                    );
                    return;
                };
                let remote = remote.to_owned();
                let branch = branch.to_owned();
                let view = cx.entity();
                let confirm = cx
                    .try_global::<crate::AppSettings>()
                    .map(|state| state.settings.confirm_delete_branch)
                    .unwrap_or(true);
                if !confirm {
                    self.start_network_job(
                        Job::DeleteRemoteBranch {
                            remote: remote.clone(),
                            branch: branch.clone(),
                        },
                        window,
                        cx,
                    );
                    return;
                }
                crate::confirm_dialog::open(
                    format!("Delete {name}?"),
                    format!("This deletes the branch “{branch}” on {remote}. It cannot be undone."),
                    "Delete branch",
                    move |window, cx| {
                        view.update(cx, |view, cx| {
                            view.start_network_job(
                                Job::DeleteRemoteBranch {
                                    remote: remote.clone(),
                                    branch: branch.clone(),
                                },
                                window,
                                cx,
                            );
                        });
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::SetUpstream(branch) => {
                let fallback = self.default_remote_name();
                self.run_repo_action(window, cx, move |repo| {
                    let remote = git_core::network::default_remote(repo)?
                        .unwrap_or_else(|| fallback.clone());
                    let upstream = format!("{remote}/{branch}");
                    let exists = repo.refs()?.iter().any(|reference| {
                        reference.kind == RefKind::RemoteBranch && reference.name == upstream
                    });
                    if !exists {
                        return Err(Error::Git {
                            command: "branch --set-upstream-to",
                            code: None,
                            stderr: format!(
                                "There is no {upstream} yet. Push {branch} first to create it."
                            ),
                        });
                    }
                    git_core::network::set_upstream(repo, &branch, &remote, &branch)?;
                    Ok((ui::ToastKind::Ok, format!("{branch} now tracks {upstream}")))
                });
            }
            SidebarAction::ShowWorktrees => {
                self.set_view(ViewMode::Worktrees, window, cx);
            }
            SidebarAction::AddWorktree => {
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.open_add(window, cx));
            }
            SidebarAction::OpenWorktree(path) => {
                let label = path.clone();
                self.workspace
                    .update(cx, |workspace, cx| {
                        workspace.open_repository_window(PathBuf::from(path), window, cx);
                    })
                    .ok();
                toast::show(
                    ui::ToastKind::Ok,
                    format!("Opened {label} in a new window"),
                    window,
                    cx,
                );
            }
            SidebarAction::LockWorktree(path) => {
                let path = PathBuf::from(path);
                self.run_repo_action(window, cx, move |repo| {
                    repo.lock_worktree(&path, Some("locked manually"))?;
                    Ok((ui::ToastKind::Ok, format!("Locked {}", path.display())))
                });
            }
            SidebarAction::UnlockWorktree(path) => {
                let path = PathBuf::from(path);
                self.run_repo_action(window, cx, move |repo| {
                    repo.unlock_worktree(&path)?;
                    Ok((ui::ToastKind::Ok, format!("Unlocked {}", path.display())))
                });
            }
            SidebarAction::RenameWorktree(path) => {
                self.open_ref_dialog(
                    RefDialogKind::RenameWorktree {
                        from: PathBuf::from(path),
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::RemoveWorktree(path) => {
                self.worktrees.update(cx, |worktrees, cx| {
                    worktrees.open_remove(Path::new(&path), window, cx);
                });
            }
            SidebarAction::PruneWorktrees => {
                self.run_repo_action(window, cx, |repo| {
                    repo.prune_worktrees()?;
                    Ok((
                        ui::ToastKind::Ok,
                        "Pruned stale worktree records".to_owned(),
                    ))
                });
            }
        }
    }

    /// `origin` when configured, otherwise the first remote, otherwise
    /// `origin` (the operation then reports the missing remote).
    fn default_remote_name(&self) -> String {
        let remotes = self
            .sidebar_extras
            .as_ref()
            .map(|extras| &extras.remote_urls)
            .filter(|urls| !urls.is_empty());
        let Some(remotes) = remotes else {
            return "origin".to_owned();
        };
        remotes
            .iter()
            .find(|(name, _)| name == "origin")
            .or_else(|| remotes.first())
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "origin".to_owned())
    }

    /// Runs one history-row action.
    fn handle_history_action(
        &mut self,
        action: RefAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            RefAction::CheckoutBranch(name) => {
                self.checkout_branch(name, window, cx);
            }
            RefAction::CheckoutCommit(card) => {
                let short = card.short.clone();
                self.checkout_guarded(
                    format!("Check out {short}?"),
                    move |repo| {
                        repo.checkout_detached(card.oid)?;
                        Ok((
                            ui::ToastKind::Ok,
                            format!("Checked out {short} (detached HEAD)"),
                        ))
                    },
                    window,
                    cx,
                );
            }
            other => {
                let kind = match other {
                    RefAction::CheckoutBranch(_) | RefAction::CheckoutCommit(_) => return,
                    RefAction::CreateBranch(card) => RefDialogKind::CreateBranch { card },
                    RefAction::CreateTag(card) => RefDialogKind::CreateTag { card },
                    RefAction::CherryPick(card) => RefDialogKind::CherryPick { card },
                    RefAction::Revert(card) => RefDialogKind::Revert { card },
                    RefAction::Reset(card) => RefDialogKind::Reset { card },
                };
                self.open_ref_dialog(kind, window, cx);
            }
        }
    }

    /// HEAD's commit, or `None` while the branch is unborn.
    fn head_oid(&self, cx: &App) -> Option<Oid> {
        match self.history.read(cx).head()? {
            Head::Branch { target, .. } => *target,
            Head::Detached(oid) => Some(*oid),
        }
    }

    /// The commit card for a history row or a sidebar ref target.
    fn card_for(&self, oid: Oid, cx: &Context<Self>) -> CommitCard {
        self.history.read(cx).card_for(oid)
    }

    /// The refs, remotes and HEAD a ref dialog needs.
    fn ref_dialog_context(&self, kind: &RefDialogKind, cx: &Context<Self>) -> RefDialogContext {
        let (refs, head_oid, branch) = {
            let history = self.history.read(cx);
            let refs = history.refs().unwrap_or(&[]).to_vec();
            let head_oid = match history.head() {
                Some(Head::Branch { target, .. }) => *target,
                Some(Head::Detached(oid)) => Some(*oid),
                None => None,
            };
            let branch = match history.head() {
                Some(Head::Branch { name, .. }) => name.clone(),
                _ => "HEAD".to_owned(),
            };
            (refs, head_oid, branch)
        };
        let remotes = self
            .sidebar_extras
            .as_ref()
            .map(|extras| extras.remotes.clone())
            .unwrap_or_default();
        let signing_key = self
            .sidebar_extras
            .as_ref()
            .and_then(|extras| extras.signing_key.clone());
        // Only the Create tag dialog pushes to a remote; other dialogs skip the
        // git spawn on the UI thread.
        let default_remote = if matches!(kind, RefDialogKind::CreateTag { .. }) {
            self.git
                .as_ref()
                .ok()
                .and_then(|git| Repo::open(git.clone(), &self.root).ok())
                .and_then(|repo| git_core::network::default_remote(&repo).ok().flatten())
        } else {
            None
        };
        let view = cx.entity();
        let network: crate::ref_dialog::NetworkAction = Rc::new(move |job, window, cx| {
            view.update(cx, |view, cx| view.start_network_job(job, window, cx));
        });
        let changes = self.changes.read(cx);
        let dirty = changes.staged_count() + changes.unstaged_count();
        RefDialogContext {
            root: self.root.clone(),
            git: self.git.clone(),
            refs,
            remotes,
            default_remote,
            network: Some(network),
            signing_key,
            head_oid,
            branch,
            dirty,
        }
    }

    fn open_ref_dialog(
        &mut self,
        kind: RefDialogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A second menu action while one dialog is still open replaces it
        // rather than stacking two modals.
        if let Some(previous) = self.ref_dialog.clone() {
            previous.update(cx, |dialog, cx| dialog.close(window, cx));
        }
        let context = self.ref_dialog_context(&kind, cx);
        let view = cx.entity();
        let on_done = Rc::new(move |window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| {
                // The dialog entity stays owned until the next one replaces
                // it: dropping it here would free the state that is running
                // this callback.
                view.after_ref_change(cx);
                window.refresh();
            });
        });
        let dialog = ref_dialog::open(kind, context, on_done, window, cx);
        self.ref_dialog = Some(dialog);
    }

    /// Checks out a local branch, asking first when the work tree is dirty.
    fn checkout_branch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let label = name.clone();
        self.checkout_guarded(
            format!("Switch to {label}?"),
            move |repo| {
                repo.checkout_branch(&name)?;
                Ok((ui::ToastKind::Ok, format!("Switched to {label}")))
            },
            window,
            cx,
        );
    }

    /// Runs a checkout at once when the work tree is clean. Otherwise asks
    /// whether to stash the changes, carry them along, or discard them.
    fn checkout_guarded<F>(
        &mut self,
        title: String,
        checkout: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)>
            + Send
            + Clone
            + 'static,
    {
        if self.ref_action_running {
            return;
        }
        let dirty = {
            let changes = self.changes.read(cx);
            changes.staged_count() + changes.unstaged_count()
        };
        if dirty == 0 {
            self.run_repo_action(window, cx, checkout);
            return;
        }
        let files = if dirty == 1 {
            "1 uncommitted change".to_owned()
        } else {
            format!("{dirty} uncommitted changes")
        };
        let view = cx.entity();
        // Each button may fire, but only one does. `Rc` lets every closure
        // hold the checkout and the chosen one take it.
        let checkout = Rc::new(RefCell::new(Some(checkout)));
        let take = |checkout: &Rc<RefCell<Option<F>>>| {
            checkout.borrow_mut().take().expect("checkout runs once")
        };
        crate::checkout_dialog::open(
            title,
            format!("{files} in the working tree. What should happen to them?"),
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| {
                        view.run_repo_action(window, cx, move |repo| {
                            // An empty stash (nothing left to save) is fine:
                            // the switch still happens.
                            repo.stash_save(Some("GitWarp: stashed before switching"))?;
                            checkout(repo)
                        });
                    });
                }
            },
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| view.run_repo_action(window, cx, checkout));
                }
            },
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| {
                        view.run_repo_action(window, cx, move |repo| {
                            repo.discard_all()?;
                            checkout(repo)
                        });
                    });
                }
            },
            window,
            cx,
        );
    }

    /// Reloads everything a ref operation can have changed: the history and
    /// its badges, the sidebar's lists, the local-changes view and HEAD.
    fn after_ref_change(&mut self, cx: &mut Context<Self>) {
        self.reload_history(cx);
        self.changes.update(cx, |changes, cx| changes.refresh(cx));
        self.worktrees
            .update(cx, |worktrees, cx| worktrees.refresh(cx));
        self.refresh(cx);
        cx.notify();
    }

    /// Runs a repository mutation without a dialog, then reloads the views.
    fn run_repo_action<F>(&mut self, window: &mut Window, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)> + Send + 'static,
    {
        if self.ref_action_running {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        self.ref_action_running = true;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = git_core::Repo::open(git, &root)?;
                    action(&repo)
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.ref_action_running = false;
                match result {
                    Ok((kind, notice)) => {
                        toast::show(kind, notice, window, cx);
                        view.after_ref_change(cx);
                    }
                    Err(error) => {
                        toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The toolbar's Stash button: stashes every local change.
    fn stash_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_repo_action(window, cx, |repo| {
            if repo.stash_save(None)? {
                Ok((ui::ToastKind::Ok, "Stashed local changes".to_owned()))
            } else {
                Ok((ui::ToastKind::Info, "No local changes to stash".to_owned()))
            }
        });
    }

    /// Starts the repeating, quiet fetch of every remote.
    fn start_background_fetch(&mut self, cx: &mut Context<Self>) {
        let interval = self.background_fetch_interval;
        self.background_fetch_task = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            loop {
                let delay = if first {
                    BACKGROUND_FETCH_FIRST
                } else {
                    interval
                };
                first = false;
                cx.background_executor().timer(delay).await;
                if this
                    .update(cx, |view, cx| view.background_fetch_tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// One background fetch, skipped while another network action runs.
    fn background_fetch_tick(&mut self, cx: &mut Context<Self>) {
        if !self.background_fetch || self.network.is_some() || self.background_fetch_running {
            return;
        }
        let Some(root) = self
            .history
            .read(cx)
            .root()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(_) => return,
        };
        let context = cx
            .try_global::<NetworkService>()
            .map(|service| service.context(false))
            .unwrap_or_default();
        self.background_fetch_running = true;
        cx.spawn(async move |this, cx| {
            let changed = cx
                .background_executor()
                .spawn(async move {
                    let repo = match Repo::open(git, &root) {
                        Ok(repo) => repo,
                        Err(error) => {
                            log::debug!("background fetch could not open the repository: {error}");
                            return false;
                        }
                    };
                    let before = ref_snapshot(&repo);
                    let outcome = git_core::network::fetch(
                        &repo,
                        &FetchOptions {
                            remote: None,
                            tags: true,
                            prune: true,
                        },
                        &context,
                        &git_core::network::CancelToken::new(),
                        |_| {},
                    );
                    match outcome {
                        Ok(_) => before != ref_snapshot(&repo),
                        Err(error) => {
                            log::debug!("background fetch failed: {error}");
                            false
                        }
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                view.background_fetch_running = false;
                if changed {
                    view.reload_after_network(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Turns the background fetch on or off and remembers it in the settings.
    fn set_background_fetch(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.background_fetch = enabled;
        crate::update_settings(cx, |settings| settings.background_fetch = enabled);
        cx.notify();
    }

    /// Runs a network job in the background and paints its progress in the
    /// status bar. Only one runs at a time.
    fn start_network_job(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        if self.network.is_some() {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let (context, asker) = match cx.try_global::<NetworkService>() {
            Some(service) => (service.context(true), service.host_keys()),
            None => (
                git_core::network::NetworkContext::default(),
                crate::network_service::HostKeyAsker::unavailable(),
            ),
        };
        let root = self.root.clone();
        self.network_generation = self.network_generation.wrapping_add(1);
        let generation = self.network_generation;
        self.network = Some(NetworkActivity {
            verb: job.verb(),
            detail: "Connecting…".into(),
            progress: None,
        });
        cx.notify();

        let (tx, rx) = mpsc::channel();
        let worker_job = job.clone();
        cx.background_executor()
            .spawn(async move {
                let result = Repo::open(git, &root).and_then(|repo| {
                    worker_job.run(
                        &repo,
                        &context,
                        &asker,
                        &git_core::network::CancelToken::new(),
                        |progress| {
                            let _ = tx.send(NetworkEvent::Progress(progress));
                        },
                    )
                });
                let _ = tx.send(NetworkEvent::Done(result));
            })
            .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(NETWORK_POLL).await;
            let done = this
                .update_in(cx, |view, window, cx| {
                    view.poll_network(&rx, generation, &job, window, cx)
                })
                .unwrap_or(true);
            if done {
                break;
            }
        })
        .detach();
    }

    /// Returns whether the job finished.
    fn poll_network(
        &mut self,
        rx: &Receiver<NetworkEvent>,
        generation: u64,
        job: &Job,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.network_generation {
            return true;
        }
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                NetworkEvent::Progress(progress) => {
                    if let Some(activity) = self.network.as_mut() {
                        if let Some(fraction) = progress.fraction() {
                            activity.progress = Some(fraction);
                        }
                        let text = progress.text.trim();
                        if !text.is_empty() {
                            activity.detail = text.to_owned().into();
                        }
                    }
                }
                NetworkEvent::Done(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            self.network = None;
            match result {
                Ok(outcome) => {
                    toast::show(ui::ToastKind::Ok, job.done_message(&outcome), window, cx);
                    self.reload_after_network(cx);
                }
                Err(error) => {
                    toast::show(
                        ui::ToastKind::Error,
                        network_ops::friendly_error(&error),
                        window,
                        cx,
                    );
                }
            }
            return true;
        }
        cx.notify();
        false
    }

    /// Reloads everything a remote change can affect.
    fn reload_after_network(&mut self, cx: &mut Context<Self>) {
        self.reload_history(cx);
        self.changes.update(cx, |changes, cx| changes.refresh(cx));
        self.refresh(cx);
        cx.notify();
    }

    fn fetch_remotes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job(
            Job::Fetch(FetchOptions {
                remote: None,
                tags: true,
                prune: true,
            }),
            window,
            cx,
        );
    }

    pub(crate) fn pull_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job(Job::Pull, window, cx);
    }

    pub(crate) fn push_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job(Job::Push, window, cx);
    }

    /// Starts the sidebar's background load when the history first reports a
    /// root. Loading is once per root; `refresh` forces it again.
    fn ensure_sidebar(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self
            .history
            .read(cx)
            .root()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        if self.sidebar_root.as_ref() == Some(&root) {
            return;
        }
        self.start_sidebar(root, cx);
    }

    /// Re-runs the sidebar's background load (the "Refresh list" action).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.sidebar_root.clone().or_else(|| {
            self.history
                .read(cx)
                .root()
                .map(std::path::Path::to_path_buf)
        });
        if let Some(root) = root {
            self.start_sidebar(root, cx);
        }
    }

    /// Reads the four sidebar lists in the background. Each failure is logged
    /// and becomes an empty list for that section, so one failure never hides
    /// the others.
    fn start_sidebar(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.sidebar_root = Some(root.clone());
        self.sidebar_generation = self.sidebar_generation.wrapping_add(1);
        let generation = self.sidebar_generation;
        // Keep the lists that are already on screen. Clearing them here makes
        // the tree blink empty until the background read returns.
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("could not read the sidebar: {error}");
                self.sidebar_extras = Some(SidebarExtras::default());
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let extras = cx
                .background_executor()
                .spawn(async move { read_sidebar_extras(git, &root) })
                .await;
            this.update(cx, |view, cx| {
                view.set_sidebar_extras(generation, extras, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_sidebar_extras(
        &mut self,
        generation: u64,
        extras: SidebarExtras,
        cx: &mut Context<Self>,
    ) {
        // A newer load owns the lists now; this older answer is stale.
        if generation != self.sidebar_generation {
            return;
        }
        self.sidebar_extras = Some(extras);
        cx.notify();
    }

    /// Loads the details when the history selection moved to another commit,
    /// and clears them when nothing is selected.
    fn sync_details(&mut self, selected: Option<Arc<CommitInfo>>, cx: &mut Context<Self>) {
        let worktree_selected = selected.as_ref().map(|commit| {
            let mut short = commit.id.to_string();
            short.truncate(7);
            SelectedCommit {
                oid: commit.id,
                short,
            }
        });
        self.worktrees.update(cx, |worktrees, cx| {
            worktrees.set_selected(worktree_selected, cx)
        });
        let Some(commit) = selected else {
            if self.details_oid.take().is_some() {
                self.details_generation = self.details_generation.wrapping_add(1);
                self.details = DetailsState::Empty;
                self.selected_file = None;
                self.diff_generation = self.diff_generation.wrapping_add(1);
                self.diff = DiffState::Empty;
                cx.notify();
            }
            return;
        };
        if self.details_oid == Some(commit.id) {
            return;
        }
        self.start_details(commit.id, cx);
    }

    /// Reads one commit's details and changed files in the background.
    fn start_details(&mut self, oid: Oid, cx: &mut Context<Self>) {
        self.details_oid = Some(oid);
        self.selected_file = None;
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.diff = DiffState::Empty;
        self.details_generation = self.details_generation.wrapping_add(1);
        let generation = self.details_generation;
        self.details = DetailsState::Loading;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.details = DetailsState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_details(git, &root, oid) })
                .await;
            this.update(cx, |view, cx| view.set_details(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_details(
        &mut self,
        generation: u64,
        result: Result<crate::details::CommitDetailsData, String>,
        cx: &mut Context<Self>,
    ) {
        // A newer selection owns the pane now; this older answer is stale.
        if generation != self.details_generation {
            return;
        }
        match result {
            Ok(data) => {
                self.selected_file = (!data.files.is_empty()).then_some(0);
                self.details = DetailsState::Loaded(Box::new(data));
                self.start_diff(cx);
            }
            Err(error) => {
                log::warn!("could not read the commit details: {error}");
                self.details = DetailsState::Failed(error);
            }
        }
        cx.notify();
    }

    /// Selects one changed file and scrolls it into view.
    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let count = self.details.files().map(<[ChangedFile]>::len).unwrap_or(0);
        if index >= count || self.selected_file == Some(index) {
            return;
        }
        self.selected_file = Some(index);
        self.files_scroll.scroll_to_item(index);
        self.start_diff(cx);
        cx.notify();
    }

    /// Moves the file selection by `step`, clamped at the ends.
    fn step_file(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.details.files().map(<[ChangedFile]>::len).unwrap_or(0);
        if count == 0 {
            return;
        }
        let current = self.selected_file.unwrap_or(0) as isize;
        let next = current.saturating_add(step).clamp(0, count as isize - 1) as usize;
        self.select_file(next, cx);
    }

    /// Focuses the file list, so its arrow keys work.
    fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.files_focus, cx);
    }

    /// Reads the selected file's diff in the background.
    fn start_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        self.diff_scroll = UniformListScrollHandle::new();
        self.diff = DiffState::Loading;
        let (Some(oid), Some(file)) = (
            self.details_oid,
            self.selected_file
                .and_then(|index| self.details.files().and_then(|files| files.get(index)))
                .cloned(),
        ) else {
            self.diff = DiffState::Empty;
            return;
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.diff = DiffState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let key = (oid, file.path.clone());
        let cached = self.highlight_cache.get(&key).cloned();
        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let diff_options = settings.diff_options();
        let word_highlight = settings.diff_word_highlight;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    read_diff(git, &root, oid, &file, cached, diff_options, word_highlight)
                })
                .await;
            this.update(cx, |view, cx| view.set_diff(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps the syntax spans of a file, dropping the oldest entry past the
    /// cache's small cap.
    fn cache_highlight(&mut self, key: (Oid, String), spans: Arc<Highlighted>) {
        const LIMIT: usize = 64;
        if self.highlight_cache.len() >= LIMIT && !self.highlight_cache.contains_key(&key) {
            if let Some(oldest) = self.highlight_cache.keys().next().cloned() {
                self.highlight_cache.remove(&oldest);
            }
        }
        self.highlight_cache.insert(key, spans);
    }

    /// Switches the diff pane between unified and side-by-side and remembers
    /// the choice.
    fn set_diff_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.diff_layout == layout {
            return;
        }
        self.diff_layout = layout;
        self.diff_scroll = UniformListScrollHandle::new();
        crate::update_settings(cx, |settings| settings.diff_layout = layout);
        cx.notify();
    }

    fn set_diff(
        &mut self,
        generation: u64,
        result: Result<crate::details::DiffData, String>,
        cx: &mut Context<Self>,
    ) {
        // A newer file owns the pane now; this older answer is stale.
        if generation != self.diff_generation {
            return;
        }
        match result {
            Ok(data) => {
                if let (Some(oid), Some(file)) = (
                    self.details_oid,
                    self.selected_file
                        .and_then(|index| self.details.files().and_then(|files| files.get(index)))
                        .cloned(),
                ) {
                    self.cache_highlight((oid, file.path.clone()), data.highlight.clone());
                }
                self.diff = DiffState::Loaded(Arc::new(data));
            }
            Err(error) => {
                log::warn!("could not read the file diff: {error}");
                self.diff = DiffState::Failed(error);
            }
        }
        cx.notify();
    }

    /// Opens the recent-repositories dropdown under the repository switcher.
    pub fn open_repo_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.repo_menu_entries(cx);
        let anchor = self
            .repo_bounds
            .get()
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(200.), px(28.))));
        menu::open_dropdown(anchor, entries, window, cx);
    }

    /// Opens the local-branches dropdown under the branch switcher.
    pub(crate) fn open_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.branch_menu_entries(cx);
        let anchor = self
            .branch_bounds
            .get()
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(200.), px(28.))));
        menu::open_dropdown(anchor, entries, window, cx);
    }

    /// Opens the command palette (Ctrl P) over this repository.
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let history = self.history.read(cx);
        let state = palette::ViewState {
            idle_lanes_collapsed: history.idle_lanes_collapsed(),
            activity_collapsed: self.activity.read(cx).is_collapsed(),
        };
        let sources = palette::Sources::new(
            state,
            history.head(),
            history.refs().unwrap_or(&[]),
            &history.loaded_commits(),
        );
        let init = palette::Init {
            repo: cx.weak_entity(),
            workspace: self.workspace.clone(),
            sources,
            git: self.git.clone(),
            root: self.root.clone(),
        };
        palette::open(init, window, cx);
    }

    /// Runs a palette command that belongs to the repository view. The ones
    /// the workspace owns (settings, open, clone, close) never get here.
    pub(crate) fn run_palette_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            Command::FetchAll => self.fetch_remotes(window, cx),
            Command::Pull => self.pull_branch(window, cx),
            Command::Push => self.push_branch(window, cx),
            Command::StashChanges => self.stash_changes(window, cx),
            Command::SwitchBranch => self.open_branch_menu(window, cx),
            Command::CreateBranchHere => {
                // "Here" is the selected commit, with HEAD as the fallback,
                // as for "Create tag here".
                let oid = self
                    .history
                    .read(cx)
                    .selected_commit()
                    .map(|commit| commit.id)
                    .or_else(|| self.head_oid(cx));
                match oid {
                    Some(oid) => {
                        self.run_sidebar_action(SidebarAction::CreateBranchAt(oid), window, cx);
                    }
                    None => {
                        toast::show(
                            ui::ToastKind::Warn,
                            "Create a commit before branching",
                            window,
                            cx,
                        );
                    }
                }
            }
            Command::CreateTagHere => self.run_sidebar_action(SidebarAction::CreateTag, window, cx),
            Command::AddRemote => self.run_sidebar_action(SidebarAction::AddRemote, window, cx),
            Command::AddWorktree => self.run_sidebar_action(SidebarAction::AddWorktree, window, cx),
            Command::Refresh => self.run_sidebar_action(SidebarAction::Refresh, window, cx),
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::ToggleDetails => self.toggle_details(cx),
            Command::ToggleActivity => {
                let collapsed = self.activity.read(cx).is_collapsed();
                self.activity
                    .update(cx, |activity, cx| activity.set_collapsed(!collapsed, cx));
            }
            Command::FocusSearch => {
                let focus = self.search_focus();
                window.focus(&focus, cx);
            }
            Command::ShowHistory => self.set_view(ViewMode::History, window, cx),
            Command::ShowChanges => self.set_view(ViewMode::Changes, window, cx),
            Command::ShowWorktrees => self.set_view(ViewMode::Worktrees, window, cx),
            Command::CollapseIdleLanes => self.history.update(cx, |history, cx| {
                let collapsed = history.idle_lanes_collapsed();
                history.set_collapse(!collapsed, cx);
            }),
            Command::FitGraph => self.history.update(cx, |history, cx| history.fit_graph(cx)),
            Command::OpenSettings(_)
            | Command::OpenRepository
            | Command::Clone
            | Command::CloseRepository => {
                debug_assert!(false, "{command:?} is run by the workspace");
            }
        }
    }

    /// A palette branch pick: the sidebar's checkout, with its dialog for
    /// uncommitted changes.
    pub(crate) fn palette_checkout(
        &mut self,
        name: String,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = if remote {
            SidebarAction::CheckoutRemoteBranch(name)
        } else {
            SidebarAction::CheckoutBranch(name)
        };
        self.run_sidebar_action(action, window, cx);
    }

    /// A palette commit pick: shows the History and selects the commit.
    pub(crate) fn palette_reveal_commit(
        &mut self,
        oid: Oid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_view(ViewMode::History, window, cx);
        self.history
            .update(cx, |history, cx| history.reveal_commit(oid, cx));
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
    }

    /// A palette file pick. A file with local changes opens Local changes;
    /// any other file selects the latest commit that touched it in History.
    pub(crate) fn palette_reveal_file(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let root = self.root.clone();
        let file = path.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    if repo.changed_paths()?.contains(&file) {
                        return Ok(None);
                    }
                    let options = git_core::LogOptions {
                        paths: vec![file],
                        max_count: Some(1),
                        ..git_core::LogOptions::default()
                    };
                    let latest = repo.log(&options)?.next_chunk(1)?.into_iter().next();
                    Ok::<_, Error>(Some(latest.map(|commit| commit.id)))
                })
                .await;
            this.update_in(cx, |view, window, cx| match found {
                Ok(None) => view.set_view(ViewMode::Changes, window, cx),
                Ok(Some(Some(oid))) => view.palette_reveal_commit(oid, window, cx),
                Ok(Some(None)) => {
                    toast::show(
                        ui::ToastKind::Info,
                        format!("{path} has no commits yet"),
                        window,
                        cx,
                    );
                }
                Err(error) => {
                    toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn repo_menu_entries(&self, cx: &Context<Self>) -> Vec<MenuEntry> {
        let recent = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.recent.clone())
            .unwrap_or_default();
        let mut entries = Vec::new();
        for repo in recent {
            let name = repo
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| repo.path.display().to_string());
            let current = repo.path == self.root;
            let workspace = self.workspace.clone();
            let path = repo.path.clone();
            entries.push(MenuEntry::Item(
                MenuItem::new(name)
                    .icon(if current {
                        IconName::Check
                    } else {
                        IconName::Folder
                    })
                    .action(move |window, cx| {
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace.open_repository(path.clone(), window, cx);
                            })
                            .ok();
                    }),
            ));
        }
        if !entries.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        let workspace = self.workspace.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new("Open repository…")
                .icon(IconName::Folder)
                .shortcut("Ctrl O")
                .action(move |window, cx| {
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.open_repository_dialog(window, cx);
                        })
                        .ok();
                }),
        ));
        let view = cx.entity();
        entries.push(MenuEntry::Item(
            MenuItem::new("Worktrees…")
                .icon(IconName::Worktree)
                .action(move |window, cx| {
                    view.update(cx, |view, cx| {
                        view.set_view(ViewMode::Worktrees, window, cx);
                    });
                }),
        ));
        let workspace = self.workspace.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new("Close repository")
                .icon(IconName::X)
                .action(move |window, cx| {
                    workspace
                        .update(cx, |workspace, cx| workspace.show_welcome(window, cx))
                        .ok();
                }),
        ));
        entries.push(MenuEntry::Separator);
        let enabled = self.background_fetch;
        let view = cx.entity();
        entries.push(MenuEntry::Item(
            MenuItem::new(if enabled {
                "Fetch in the background"
            } else {
                "Background fetch is off"
            })
            .icon(if enabled {
                IconName::Check
            } else {
                IconName::Fetch
            })
            .action(move |_, cx| {
                view.update(cx, |view, cx| view.set_background_fetch(!enabled, cx));
            }),
        ));
        entries
    }

    fn branch_menu_entries(&self, cx: &Context<Self>) -> Vec<MenuEntry> {
        let history = self.history.read(cx);
        let head_branch = match history.head() {
            Some(Head::Branch { name, .. }) => Some(name.as_str()),
            _ => None,
        };
        let Some(refs) = history.refs() else {
            return vec![MenuEntry::Item(
                MenuItem::new("Loading branches…")
                    .icon(IconName::Branch)
                    .disabled(true),
            )];
        };
        let mut entries: Vec<MenuEntry> = Vec::new();
        let mut count = 0;
        for branch in refs.iter().filter(|r| r.kind == RefKind::LocalBranch) {
            let current = head_branch == Some(branch.name.as_str());
            let mut item = MenuItem::new(branch.name.clone())
                .icon(if current {
                    IconName::Check
                } else {
                    IconName::Branch
                })
                .disabled(current);
            if !current {
                let name = branch.name.clone();
                let view = cx.entity();
                item = item.action(move |window, cx| {
                    view.update(cx, |view, cx| {
                        view.run_sidebar_action(
                            SidebarAction::CheckoutBranch(name.clone()),
                            window,
                            cx,
                        );
                    });
                });
            }
            entries.push(MenuEntry::Item(item));
            count += 1;
        }
        if count == 0 {
            entries.push(MenuEntry::Item(
                MenuItem::new("No local branches")
                    .icon(IconName::Branch)
                    .disabled(true),
            ));
        }
        entries.push(MenuEntry::Separator);
        let head_oid = match history.head() {
            Some(Head::Branch { target, .. }) => *target,
            Some(Head::Detached(oid)) => Some(*oid),
            None => None,
        };
        let card = head_oid.map(CommitCard::bare);
        let mut create = MenuItem::new("Create branch…").icon(IconName::Plus);
        if let Some(card) = card {
            let view = cx.entity();
            let card = history.card_for(card.oid);
            create = create.action(move |window, cx| {
                view.update(cx, |view, cx| {
                    view.open_ref_dialog(
                        RefDialogKind::CreateBranch { card: card.clone() },
                        window,
                        cx,
                    );
                });
            });
        } else {
            create = create.disabled(true);
        }
        entries.push(MenuEntry::Item(create));
        entries
    }

    fn branch_label(&self, cx: &Context<Self>) -> SharedString {
        match self.history.read(cx).head() {
            Some(Head::Branch { name, .. }) => name.clone().into(),
            Some(Head::Detached(oid)) => oid.short().into(),
            None => "…".into(),
        }
    }

    fn render_toolbar(
        &mut self,
        layout: ToolbarLayout,
        client: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let branch = self.branch_label(cx);

        let sidebar_toggle = Button::ghost("sidebar-toggle")
            .icon_only(IconName::Sidebar)
            .tooltip_kbd("Toggle sidebar", "Ctrl B")
            .on_click(cx.listener(|view, _, _, cx| view.toggle_sidebar(cx)));

        let repo_click = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_repo_menu(window, cx));
            }
        };
        let repo_switcher = switcher(
            "repo-switcher",
            IconName::Folder,
            Some(self.repo_name.clone()),
            layout.repo_name,
            layout.item_pad,
            ("Switch repository", None),
            self.repo_bounds.clone(),
            repo_click,
            colors,
        );

        let branch_click = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_branch_menu(window, cx));
            }
        };
        let branch_switcher = switcher(
            "branch-switcher",
            IconName::Branch,
            Some(branch),
            layout.branch_name,
            layout.item_pad,
            ("Switch branch", Some("Ctrl Shift B")),
            self.branch_bounds.clone(),
            branch_click,
            colors,
        );

        let stash_count = self
            .sidebar_extras
            .as_ref()
            .map(|extras| extras.stashes.len())
            .unwrap_or(0);
        let stash_view = cx.entity();
        let mut stash_button = Button::tool("toolbar-stash")
            .icon(IconName::Stash)
            .tooltip_kbd("Stash local changes", "Ctrl Shift S")
            .on_click(move |_, window, cx| {
                stash_view.update(cx, |view, cx| view.stash_changes(window, cx));
            });
        if layout.labels {
            stash_button = stash_button.label("Stash");
        }
        if stash_count > 0 {
            stash_button = stash_button.counter(Counter::new(stash_count.to_string()));
        }

        let busy = self.network.is_some();
        let fetch = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.fetch_remotes(window, cx));
            }
        };
        let pull = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.pull_branch(window, cx));
            }
        };
        let push = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.push_branch(window, cx));
            }
        };
        let fetch_button = network_tool(
            "toolbar-fetch",
            IconName::Fetch,
            "Fetch",
            "Fetch all remotes, tags and pruned branches".to_owned(),
            crate::settings_ui::keybindings::key_cap(cx, "fetch"),
            None,
            layout.labels,
            busy,
            fetch,
        );
        let hints = {
            let history = self.history.read(cx);
            let remotes = self
                .sidebar_extras
                .as_ref()
                .map_or(&[][..], |extras| extras.remotes.as_slice());
            network_hints(history.head(), history.refs(), remotes)
        };
        let pull_button = network_tool(
            "toolbar-pull",
            IconName::Down,
            "Pull",
            hints.pull_tip,
            crate::settings_ui::keybindings::key_cap(cx, "pull"),
            (hints.behind > 0).then(|| Counter::new(format!("↓{}", hints.behind))),
            layout.labels,
            busy,
            pull,
        );
        let push_button = network_tool(
            "toolbar-push",
            IconName::Up,
            "Push",
            hints.push_tip,
            crate::settings_ui::keybindings::key_cap(cx, "push"),
            (hints.ahead > 0).then(|| Counter::new(format!("↑{}", hints.ahead)).accent()),
            layout.labels,
            busy,
            push,
        );

        let mut bar = titlebar(cx)
            .gap(px(layout.gap))
            .px(px(layout.padding))
            .child(no_shrink(sidebar_toggle))
            .child(repo_switcher)
            .child(branch_switcher)
            .child(no_shrink(
                div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(colors.border),
            ))
            .child(no_shrink(fetch_button))
            .child(no_shrink(pull_button))
            .child(no_shrink(push_button))
            .child(no_shrink(stash_button))
            .child(titlebar_fill());

        let history_item = if layout.labels {
            SegmentItem::new("History").with_icon(IconName::Lanes)
        } else {
            SegmentItem::icon(IconName::Lanes)
        };
        let changes_item = if layout.labels {
            SegmentItem::new("Changes").with_icon(IconName::Edit)
        } else {
            SegmentItem::icon(IconName::Edit)
        };
        let switch_view = cx.entity();
        bar = bar.child(no_shrink(
            Segmented::new("view-switch")
                .item(history_item)
                .item(changes_item.tooltip("Local changes"))
                .selected(match self.view {
                    ViewMode::History | ViewMode::Worktrees => 0,
                    ViewMode::Changes => 1,
                })
                .on_select(move |index, window, cx| {
                    let mode = if index == 1 {
                        ViewMode::Changes
                    } else {
                        ViewMode::History
                    };
                    switch_view.update(cx, |view, cx| view.set_view(mode, window, cx));
                }),
        ));

        let mut field = SearchField::new("toolbar-search", self.search.clone());
        match self.history.read(cx).search_status() {
            // A query replaces the key hint with its count (or "No matches").
            Some(status) => field = field.trailing_text(status),
            None if layout.search_kbd => field = field.hint("Ctrl K"),
            None => {}
        }
        bar = bar.child(
            div()
                .id("toolbar-search-wrap")
                .flex_basis(px(layout.search_basis))
                .flex_shrink(1.)
                .min_w(px(layout.search_min))
                // The field's own keys live here, so they never fire while
                // the sidebar filter or another input has the keyboard.
                .key_context("history-search")
                .on_action(cx.listener(|view, _: &SearchNext, _, cx| view.search_next(cx)))
                .on_action(cx.listener(|view, _: &SearchPrev, _, cx| view.search_prev(cx)))
                .on_action(cx.listener(|view, _: &SearchClear, window, cx| {
                    view.search_clear(window, cx);
                }))
                .on_action(cx.listener(|view, _: &SearchFocusList, window, cx| {
                    view.search_focus_list(window, cx);
                }))
                .tooltip(Tooltip::with_kbd(
                    "Search commits, authors, hashes",
                    "Ctrl K",
                ))
                .child(field),
        );

        let settings_click = {
            let workspace = self.workspace.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = workspace.update(cx, |workspace, cx| workspace.open_settings(window, cx));
            }
        };
        bar = bar.child(no_shrink(
            Button::ghost("settings-button")
                .icon_only(IconName::Gear)
                .tooltip_kbd("Settings", "Ctrl ,")
                .on_click(settings_click),
        ));

        titlebar_end(bar, client)
    }

    fn render_sidebar(&mut self, width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let nodes = self.tree_nodes(cx);
        if let Some(index) = nodes.iter().position(|node| node.current) {
            // ScrollHandle applies this during prepaint, so asking during
            // render lands on the rows that are about to be laid out.
            self.sidebar_scroll.scroll_to_item(index);
        }
        let filtering = !self.filter.trim().is_empty();
        let no_matches = filtering && nodes.is_empty();
        // Every row's identity and expansion, so a click, Enter/Space or
        // ←/→ can toggle any expandable node, not only the sections.
        let rows: Vec<(SharedString, bool, bool)> = nodes
            .iter()
            .map(|node| (node.key.clone(), node.has_children, node.expanded))
            .collect();

        let view = cx.entity();
        let activate = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, _: &mut Window, cx: &mut App| {
                let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    if has_children {
                        view.toggle_node(&key, expanded, cx);
                    } else {
                        view.activate_node(&key, cx);
                    }
                });
            }
        };
        // Enter checks out a branch or tag row; every other row keeps the
        // activation Space gives (select, reveal, or toggle a folder).
        let confirm = {
            let activate = activate.clone();
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, window: &mut Window, cx: &mut App| {
                let Some((key, has_children, _)) = rows.get(index).cloned() else {
                    return;
                };
                let handled = !has_children
                    && view.update(cx, |view, cx| {
                        view.run_sidebar_shortcut(&key, "Enter", window, cx)
                    });
                if !handled {
                    activate(index, window, cx);
                }
            }
        };
        // F2 and Delete run the row menu's Rename / Delete item.
        let shortcut = |hint: &'static str| {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, window: &mut Window, cx: &mut App| {
                let Some((key, _, _)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    view.run_sidebar_shortcut(&key, hint, window, cx);
                });
            }
        };
        let rename = shortcut("F2");
        let delete = shortcut("Del");
        // A right-click selects the row and opens its kind's menu at the
        // pointer; the sidebar keeps the keyboard focus.
        let context_menu = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, position: Point<Pixels>, window: &mut Window, cx: &mut App| {
                let Some((key, _, _)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    view.open_sidebar_menu(&key, position, window, cx);
                });
            }
        };
        // Double-clicking a worktree leaf opens the Worktrees page. A branch,
        // remote branch or tag checks out, asking first when the work tree is
        // dirty. A single click keeps selecting the row.
        let open = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, window: &mut Window, cx: &mut App| {
                let Some((key, _, _)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| view.open_sidebar_node(&key, window, cx));
            }
        };

        div()
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            // GPUI flex columns lay their children out from the start instead
            // of stretching them: every row spans the sidebar explicitly, or
            // an auto-width filter field collapses to its icon.
            .items_start()
            .bg(colors.sidebar)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .px(px(10.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .child(SearchField::new(
                        "sidebar-filter",
                        self.sidebar_filter.clone(),
                    )),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(no_matches, |this| {
                        this.child(
                            div()
                                .mx(px(6.))
                                .pl(px(6.))
                                .pr(px(8.))
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("No matches"),
                        )
                    })
                    .when(!no_matches, |this| {
                        this.child(
                            div().w_full().flex_1().min_w_0().min_h_0().child(
                                Tree::new("sidebar-tree", nodes)
                                    .horizontal_scroll()
                                    .scroll_handle(self.sidebar_scroll.clone())
                                    .on_toggle(node_toggler(rows.clone(), view.clone()))
                                    .on_expand(node_toggler(rows.clone(), view.clone()))
                                    .on_collapse(node_toggler(rows.clone(), view.clone()))
                                    .on_activate(activate)
                                    .on_confirm(confirm)
                                    .on_rename(rename)
                                    .on_delete(delete)
                                    .on_open(open)
                                    .on_context_menu(context_menu),
                            ),
                        )
                        .child(Scrollbar::horizontal(
                            "sidebar-scrollbar",
                            self.sidebar_scroll.clone(),
                        ))
                    }),
            )
    }

    fn tree_nodes(&self, cx: &Context<Self>) -> Vec<TreeNodeData> {
        let history = self.history.read(cx);
        sidebar_nodes(
            history.refs(),
            history.head(),
            self.sidebar_extras.as_ref(),
            &self.sidebar,
            &self.filter,
            self.selected_node.as_deref(),
        )
    }

    fn render_sidebar_splitter(
        &mut self,
        width: f32,
        max: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let reset_view = view.clone();
        let persist_view = view.clone();
        Splitter::vertical("sidebar-splitter")
            .size(px(width))
            .bounds(px(SIDEBAR_WIDTH_MIN), px(max))
            .on_resize(move |size, _, cx| {
                view.update(cx, |view, cx| view.set_sidebar_width(size, cx));
            })
            .on_resize_end(move |_, cx| {
                persist_view.update(cx, |view, cx| view.persist_layout(cx));
            })
            .on_reset(move |_, cx| {
                reset_view.update(cx, |view, cx| {
                    view.set_sidebar_width(px(SIDEBAR_WIDTH_DEFAULT), cx);
                    view.persist_layout(cx);
                });
            })
    }

    fn render_details_splitter(
        &mut self,
        width: f32,
        limit: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (min, _) = dock_bounds(self.dock);
        let size = px(width);
        let splitter = match self.dock {
            Dock::Bottom => Splitter::horizontal("details-splitter"),
            Dock::Right => Splitter::vertical("details-splitter"),
        };
        let view = cx.entity();
        let reset_view = view.clone();
        let persist_view = view.clone();
        splitter
            .size(size)
            .bounds(px(min), px(limit))
            // A panel docked bottom or right grows as the pointer moves up or
            // left.
            .inverted(true)
            .on_resize(move |size, _, cx| {
                view.update(cx, |view, cx| view.set_details_size(size, cx));
            })
            .on_resize_end(move |_, cx| {
                persist_view.update(cx, |view, cx| view.persist_layout(cx));
            })
            .on_reset(move |_, cx| {
                reset_view.update(cx, |view, cx| view.reset_details_size(cx));
            })
    }

    fn render_details(&mut self, size: f32, main_width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut panel = div().flex_none().flex().flex_col().bg(colors.bg);
        panel = match self.dock {
            Dock::Bottom => panel.w_full().h(px(size)).border_t_1(),
            Dock::Right => panel.h_full().w(px(size)).border_l_1(),
        };
        panel
            .border_color(colors.border)
            .child(self.render_details_header(cx))
            .child(self.render_details_body(size, main_width, colors, cx))
    }

    fn render_details_header(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let short = self.history.read(cx).selected_short();
        let dock = self.dock;
        let view = cx.entity();
        let switch = Segmented::new("dock-switch")
            .small()
            .item(
                SegmentItem::icon(IconName::PanelR)
                    .id("dock-right")
                    .tooltip("Dock details to the right"),
            )
            .item(
                SegmentItem::icon(IconName::PanelB)
                    .id("dock-bottom")
                    .tooltip("Dock details to the bottom"),
            )
            .selected(if dock == Dock::Right { 0 } else { 1 })
            .on_select(move |index, _, cx| {
                let dock = if index == 0 {
                    Dock::Right
                } else {
                    Dock::Bottom
                };
                view.update(cx, |view, cx| view.set_dock(dock, cx));
            });

        div()
            .h(DOCK_HEADER)
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .font_weight(FontWeight::SEMIBOLD)
            .child("Commit")
            .when_some(short, |this, hash| {
                this.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.muted)
                        .child(hash),
                )
            })
            .child(div().flex_1())
            .child(no_shrink(switch))
    }

    fn render_details_body(
        &mut self,
        size: f32,
        main_width: f32,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let diff_width = match self.dock {
            Dock::Bottom => (main_width - 560.).max(0.),
            Dock::Right => size,
        };
        match self.dock {
            Dock::Bottom => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .child(
                    self.render_info_pane(colors, cx)
                        .w(px(310.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(
                    self.render_files_pane(colors, cx)
                        .w(px(250.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(
                    self.render_diff_pane(diff_width, colors, cx)
                        .flex_1()
                        .min_w_0()
                        .h_full(),
                ),
            Dock::Right => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    self.render_info_pane(colors, cx)
                        .h(px(230.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(
                    self.render_files_pane(colors, cx)
                        .h(px(160.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(
                    self.render_diff_pane(diff_width, colors, cx)
                        .flex_1()
                        .w_full(),
                ),
        }
    }

    /// The commit's message and metadata, scrolling when it is long.
    fn render_info_pane(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let content = self.render_info_content(colors, cx);
        let viewport = div()
            .id("details-info-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.info_scroll)
            .child(content);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .border_color(colors.border)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(viewport)
                    .child(Scrollbar::vertical(
                        "details-info-bar",
                        self.info_scroll.clone(),
                    )),
            )
    }

    fn render_info_content(&self, colors: ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        match &self.details {
            DetailsState::Empty => muted_note("Select a commit", colors).into_any_element(),
            DetailsState::Loading => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .px(px(18.))
                .py(px(16.))
                .child(Skeleton::new("details-skeleton-title", px(190.)))
                .child(Skeleton::new("details-skeleton-body", px(240.)))
                .child(Skeleton::new("details-skeleton-meta", px(150.)))
                .into_any_element(),
            DetailsState::Failed(message) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(18.))
                .py(px(16.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Could not load the commit"),
                )
                .child(div().text_color(colors.error).child(message.clone()))
                .into_any_element(),
            DetailsState::Loaded(data) => self.render_loaded_info(&data.details, cx),
        }
    }

    fn render_loaded_info(&self, details: &CommitDetails, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let now = crate::history_view::unix_now();
        let mut column = div().flex().flex_col().px(px(18.)).py(px(16.)).child(
            div()
                .mb(px(8.))
                .text_size(FS_MD)
                .line_height(px(19.))
                .font_weight(FontWeight::BOLD)
                .child(details.summary.clone()),
        );
        if !details.body.is_empty() {
            column = column.child(
                div()
                    .mb(px(8.))
                    .text_size(FS_BASE)
                    .child(details.body.clone()),
            );
        }

        let author = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                Avatar::new(details.author_email.clone(), px(20.))
                    .name(details.author_name.clone()),
            )
            .child(div().flex_1().min_w_0().truncate().child(format!(
                "{} <{}>",
                details.author_name, details.author_email
            )));
        let committer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                Avatar::new(details.committer_email.clone(), px(20.))
                    .name(details.committer_name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(details.committer_name.clone()),
            );
        let authored = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(absolute_time(details.author_time))
            .child(div().text_color(colors.muted).child(format!(
                "({})",
                relative_time(now, details.author_time.max(0) as u64)
            )));
        let full_hash = details.id.to_string();
        let mut hash = full_hash.clone();
        hash.truncate(20);
        hash.push('…');
        let hash = div()
            .id("details-hash")
            .font_family(FONT_MONO)
            .text_size(FS_SM)
            .truncate()
            .tooltip(Tooltip::text(full_hash))
            .child(hash);

        let parents = if details.parents.is_empty() {
            div().text_color(colors.muted).child("none (root commit)")
        } else {
            let view = cx.entity();
            let mut row = div().flex().items_center().gap(px(6.));
            for (index, parent) in details.parents.iter().copied().enumerate() {
                let view = view.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("details-parent-{index}")))
                        .debug_selector(move || format!("details-parent-{index}"))
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.accent)
                        .cursor_pointer()
                        .hover(move |style| style.text_color(colors.accent_hover))
                        .child({
                            let mut hash = parent.to_string();
                            hash.truncate(7);
                            hash
                        })
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.history.update(cx, |history, cx| {
                                    history.reveal_commit(parent, cx);
                                });
                            });
                        }),
                );
            }
            row
        };

        column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(FS_SM)
                    .child(meta_row("Author", author, colors))
                    .child(meta_row("Authored", authored, colors))
                    .child(meta_row("Committer", committer, colors))
                    .child(meta_row("Hash", hash, colors))
                    .child(meta_row("Parents", parents, colors)),
            )
            .into_any_element()
    }

    /// The changed-file header and rows.
    fn render_files_pane(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let count = self.details.files().map(<[ChangedFile]>::len);
        let is_merge = self
            .details
            .details()
            .is_some_and(|details| details.parents.len() > 1);
        let mut header = div()
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text)
                    .child("Changed files"),
            );
        if let Some(count) = count {
            header = header.child(CountPill::new(count.to_string()));
        }
        header = header.child(div().flex_1());
        if is_merge {
            header = header.child(
                div()
                    .id("details-vs-parent")
                    .tooltip(Tooltip::text("Combined diff against first parent"))
                    .child("vs parent 1"),
            );
        }

        let body = self.render_files_body(colors, cx);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .border_color(colors.border)
            .child(header)
            .child(body)
    }

    fn render_files_body(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let Some(files) = self.details.files() else {
            return match &self.details {
                DetailsState::Loading => {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(8.))
                        .children((0..4).map(|index| {
                            Skeleton::new(
                                SharedString::from(format!("details-files-skeleton-{index}")),
                                px(180.),
                            )
                        }))
                }
                DetailsState::Failed(message) => div()
                    .p(px(12.))
                    .text_color(colors.error)
                    .child(message.clone()),
                _ => muted_note("No files changed", colors),
            };
        };
        if files.is_empty() {
            return muted_note("No files changed", colors);
        }

        let view = cx.entity();
        let selected = self.selected_file;
        let mut list = div()
            .id("details-files-list")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.files_scroll)
            .track_focus(&self.files_focus)
            .key_context("details-files")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| view.focus_files(window, cx)),
            )
            .on_action(cx.listener(|view, _: &SelectNext, _, cx| view.step_file(1, cx)))
            .on_action(cx.listener(|view, _: &SelectPrev, _, cx| view.step_file(-1, cx)))
            .on_action(cx.listener(|view, _: &SelectFirst, _, cx| view.step_file(isize::MIN, cx)))
            .on_action(cx.listener(|view, _: &SelectLast, _, cx| view.step_file(isize::MAX, cx)));
        for (index, file) in files.iter().enumerate() {
            let view = view.clone();
            list = list.child(
                div()
                    .debug_selector(move || format!("details-file-{index}"))
                    .child(
                        FileRow::new(
                            SharedString::from(format!("details-file-{index}")),
                            file.kind.letter(),
                            file.path.clone(),
                        )
                        .added(file.added)
                        .deleted(file.deleted)
                        .selected(selected == Some(index))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| view.select_file(index, cx));
                        }),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(list)
            .child(Scrollbar::vertical(
                "details-files-bar",
                self.files_scroll.clone(),
            ))
    }

    /// The diff toolbar and the selected file's unified diff.
    fn render_diff_pane(&mut self, width: f32, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let path = self
            .selected_file
            .and_then(|index| self.details.files().and_then(|files| files.get(index)))
            .map(|file| match &file.old_path {
                Some(old) => format!("{} ← {old}", file.path),
                None => file.path.clone(),
            });
        let mut toolbar = div()
            .id("details-diff-toolbar")
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border);
        if let Some(path) = path {
            toolbar = toolbar.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.text)
                    .child(path),
            );
        } else {
            toolbar = toolbar.child(div().flex_1());
        }

        let layout = self.diff_layout;
        let view = cx.entity();
        let switch = Segmented::new("diff-layout")
            .small()
            .item(
                SegmentItem::new("Unified")
                    .with_icon(IconName::Unified)
                    .id("diff-unified"),
            )
            .item(
                SegmentItem::new("Side-by-side")
                    .with_icon(IconName::Split)
                    .id("diff-split"),
            )
            .selected(if layout == DiffLayout::Split { 1 } else { 0 })
            .on_select(move |index, _, cx| {
                let layout = if index == 1 {
                    DiffLayout::Split
                } else {
                    DiffLayout::Unified
                };
                view.update(cx, |view, cx| view.set_diff_layout(layout, cx));
            });
        toolbar = toolbar.child(no_shrink(switch));

        let body = match &self.diff {
            DiffState::Empty => diff_note("Select a file", colors),
            DiffState::Loading => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .child(Skeleton::new("diff-skeleton-1", px(320.)))
                .child(Skeleton::new("diff-skeleton-2", px(260.)))
                .child(Skeleton::new("diff-skeleton-3", px(290.))),
            DiffState::Failed(message) if message.contains("too large") => {
                diff_note("Diff too large to display", colors)
            }
            DiffState::Failed(message) => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .text_color(colors.error)
                .child(message.clone()),
            DiffState::Loaded(data) => {
                if data.diff.binary {
                    diff_note("Binary file", colors)
                } else if data.rows.is_empty() {
                    diff_note("No changes", colors)
                } else {
                    let data = data.clone();
                    let list = if layout == DiffLayout::Split {
                        let row_width = split_width(self.mono_cell, data.columns, width);
                        uniform_list(
                            "details-diff",
                            data.split_rows.len(),
                            cx.processor(move |view, range, window, cx| {
                                view.render_split_rows(
                                    &data.split_rows,
                                    &data.highlight,
                                    range,
                                    row_width,
                                    window,
                                    cx,
                                )
                            }),
                        )
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .track_scroll(&self.diff_scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .into_any_element()
                    } else {
                        let row_width = diff_width(self.mono_cell, data.columns, width);
                        uniform_list(
                            "details-diff",
                            data.rows.len(),
                            cx.processor(move |view, range, window, cx| {
                                view.render_diff_rows(
                                    &data.rows,
                                    &data.highlight,
                                    range,
                                    row_width,
                                    window,
                                    cx,
                                )
                            }),
                        )
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .track_scroll(&self.diff_scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .into_any_element()
                    };
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "details-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .text_size(cx.theme().mono_font_size)
            .child(toolbar)
            .child(body)
    }

    fn render_diff_rows(
        &mut self,
        rows: &[DiffRow],
        highlight: &Highlighted,
        range: std::ops::Range<usize>,
        width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        range
            .filter_map(|index| {
                rows.get(index).map(|row| {
                    let (old_spans, new_spans) = match row.kind {
                        RowKind::Removed => (highlight.spans(true, row.old_line), &[][..]),
                        RowKind::Added => (&[][..], highlight.spans(false, row.new_line)),
                        _ => {
                            let spans = if row.old_line.is_some() {
                                highlight.spans(true, row.old_line)
                            } else {
                                highlight.spans(false, row.new_line)
                            };
                            (spans, &[][..])
                        }
                    };
                    diff_row(
                        row,
                        index,
                        width,
                        colors,
                        old_spans,
                        new_spans,
                        show_numbers,
                    )
                })
            })
            .collect()
    }

    fn render_split_rows(
        &mut self,
        rows: &[SplitRow],
        highlight: &Highlighted,
        range: std::ops::Range<usize>,
        width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        let gutters = if show_numbers { 2. * DIFF_GUTTER } else { 0. };
        let half = px(((f32::from(width) - gutters) / 2.).max(0.));
        range
            .filter_map(|index| {
                rows.get(index)
                    .map(|row| split_row(row, index, width, half, colors, highlight, show_numbers))
            })
            .collect()
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut bar = div()
            .h(STATUS)
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(12.))
            .bg(colors.bg)
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted);

        if let Some(network) = &self.network {
            let detail = network.detail.clone();
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Spinner::new("network-status-spinner", px(12.)))
                    .child(network.verb)
                    .child(div().max_w(px(260.)).truncate().child(detail)),
            );
            bar = bar.child(match network.progress {
                Some(value) => ui::Progress::new("network-status-progress", value),
                None => ui::Progress::new("network-status-progress", 0.).indeterminate(),
            });
        }

        if self.view == ViewMode::Changes {
            let changes = self.changes.read(cx);
            if changes.is_loading() {
                bar = bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(Spinner::new("status-spinner", px(12.)))
                        .child("Loading changes…"),
                );
            } else if let Some(error) = changes.failure() {
                bar = bar.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.error)
                        .child(error.to_string()),
                );
            }
            bar = bar.child(div().flex_1());
            bar = bar.child(format!(
                "{} unstaged · {} staged",
                changes.unstaged_count(),
                changes.staged_count()
            ));
        } else {
            let history = self.history.read(cx);
            let mut fill = true;
            match history.state() {
                LoadState::Loading { .. } => {
                    bar = bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(Spinner::new("status-spinner", px(12.)))
                            .child("Loading history…"),
                    );
                }
                LoadState::Cancelled => {
                    bar = bar.child(div().text_color(colors.muted).child("Loading cancelled"));
                }
                LoadState::Failed(failure) => {
                    bar = bar.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.error)
                            .child(failure.title()),
                    );
                    fill = false;
                }
                LoadState::Loaded => {}
            }
            if fill {
                bar = bar.child(div().flex_1());
            }
            bar = bar.child(format!(
                "{} commits loaded",
                format_count(history.commit_count())
            ));
        }

        let workspace = self.workspace.clone();
        bar.child(
            div()
                .id("status-git")
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(R_SM)
                .text_color(colors.muted)
                .cursor_pointer()
                .hover(|this| this.bg(colors.hover))
                .tooltip(Tooltip::text(self.git_tooltip.clone()))
                .on_click(
                    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = workspace.update(cx, |workspace, cx| {
                            workspace.open_settings_at(crate::settings_ui::Page::Git, window, cx)
                        });
                    },
                )
                .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                .child(self.git_label.clone()),
        )
    }
}

impl Render for RepositoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let colors = cx.theme().colors;
        let width = f32::from(window.viewport_size().width);
        let layout = toolbar_layout(width);

        // Until the user picks a side, the dock follows the window width, so
        // a window resized past the breakpoint docks the details right. The
        // dragged size is kept across the flip and only clamped for the dock
        // that renders it.
        let auto_dock = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.dock.is_none())
            .unwrap_or(false);
        if auto_dock {
            let dock = resolve_dock(None, width);
            if dock != self.dock {
                self.dock = dock;
            }
        }

        let toolbar = self.render_toolbar(layout, client, cx);

        // The sidebar may take at most half of the visible area. Only the
        // rendered width is capped; the stored width stays as saved.
        let sidebar_width = if self.sidebar_open {
            effective_sidebar_width(self.sidebar_width, width)
        } else {
            0.
        };
        let details_limit = match self.dock {
            Dock::Right => details_width_limit(width, sidebar_width),
            Dock::Bottom => dock_bounds(Dock::Bottom).1,
        };
        let details_width = if self.details_open {
            match self.dock {
                Dock::Right => effective_details_width(self.details_size, width, sidebar_width),
                Dock::Bottom => clamp_details(self.details_size, Dock::Bottom),
            }
        } else {
            0.
        };
        // The splitter may not drag the sidebar past half the window either.
        let sidebar_limit = sidebar_width_limit(width);

        let sidebar = self.sidebar_open.then(|| {
            (
                self.render_sidebar(sidebar_width, cx),
                self.render_sidebar_splitter(sidebar_width, sidebar_limit, cx),
            )
        });
        // The local-changes view has its own diff pane; the commit details
        // dock belongs to history only. The worktrees page fills the centre.
        let details_open = self.details_open && self.view == ViewMode::History;
        let dock = self.dock;
        let splitter =
            details_open.then(|| self.render_details_splitter(details_width, details_limit, cx));
        let panel = details_open
            .then(|| self.render_details(details_width, width - sidebar_width - 5., cx));
        let status = self.render_status_bar(cx);

        let content = match self.view {
            ViewMode::History => div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.activity.clone())
                .child(div().flex_1().min_h_0().child(self.history.clone()))
                .into_any_element(),
            ViewMode::Changes => self.changes.clone().into_any_element(),
            ViewMode::Worktrees => self.worktrees.clone().into_any_element(),
        };
        let history_pane = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(content);

        let main = match (details_open, dock) {
            (false, _) => div().flex_1().min_w_0().flex().child(history_pane),
            (true, Dock::Bottom) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(history_pane)
                .when_some(splitter, |this, splitter| this.child(splitter))
                .when_some(panel, |this, panel| this.child(panel)),
            (true, Dock::Right) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .child(history_pane)
                .when_some(splitter, |this, splitter| this.child(splitter))
                .when_some(panel, |this, panel| this.child(panel)),
        };

        let mut body = div().flex_1().min_h_0().w_full().flex();
        if let Some((sidebar, splitter)) = sidebar {
            body = body.child(sidebar).child(splitter);
        }
        let body = body.child(main);

        div()
            .id("repository-view")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .key_context("repository")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &ToggleSidebar, _, cx| view.toggle_sidebar(cx)))
            .on_action(cx.listener(|view, _: &ToggleDetails, _, cx| view.toggle_details(cx)))
            .on_action(cx.listener(|view, _: &FocusSearch, window, cx| {
                let handle = view.search_focus();
                window.focus(&handle, cx);
                // Ctrl+K selects the field's text, so typing replaces it.
                view.search.update(cx, |input, cx| input.select_all(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &OpenRepoSwitcher, window, cx| {
                view.open_repo_menu(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenBranchSwitcher, window, cx| {
                view.open_branch_menu(window, cx);
            }))
            .on_action(cx.listener(|view, _: &StashChanges, window, cx| {
                view.stash_changes(window, cx);
            }))
            .on_action(cx.listener(|view, _: &FetchAll, window, cx| {
                view.fetch_remotes(window, cx);
            }))
            .on_action(cx.listener(|view, _: &PullBranch, window, cx| {
                view.pull_branch(window, cx);
            }))
            .on_action(cx.listener(|view, _: &PushBranch, window, cx| {
                view.push_branch(window, cx);
            }))
            .child(toolbar)
            .child(body)
            .child(status)
    }
}

/// A toolbar item with rich content: icon · bold name · chevron.
///
/// `Button` has no rich-content variant, and the design's two switchers are
/// custom `btn-tool` rows, so this builds just the row (hover, tooltip and
/// click) rather than re-implementing the button component.
#[allow(clippy::too_many_arguments)]
fn switcher(
    id: &'static str,
    icon: IconName,
    name: Option<SharedString>,
    name_max: Option<f32>,
    padding: f32,
    tooltip: (&'static str, Option<&'static str>),
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    colors: ThemeColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .relative()
        .h(px(TOOL_H))
        .px(px(padding))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(R_SM)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(Icon::new(icon).size(px(14.)).color(colors.muted));
    row = match tooltip.1 {
        Some(kbd) => row.tooltip(Tooltip::with_kbd(tooltip.0, kbd)),
        None => row.tooltip(Tooltip::text(tooltip.0)),
    };
    if let (Some(name), Some(max)) = (name, name_max) {
        row = row.child(
            div()
                .max_w(px(max))
                .truncate()
                .font_weight(FontWeight::BOLD)
                .child(name),
        );
    }
    row.child(Icon::new(IconName::ChevD).size(px(12.)).color(colors.muted))
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |_, painted, _, _| bounds.set(Some(painted)),
            )
            .absolute()
            .inset_0(),
        )
}

/// A toolbar item that must keep its width: only the search field may shrink.
fn no_shrink(child: impl IntoElement) -> Div {
    div().flex_none().child(child)
}

/// A working toolbar tool button: the label is dropped on narrow windows.
#[allow(clippy::too_many_arguments)]
fn network_tool(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    tooltip: String,
    kbd: Option<String>,
    counter: Option<Counter>,
    labels: bool,
    busy: bool,
    action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let mut button = Button::tool(id).icon(icon).disabled(busy).on_click(action);
    button = match kbd {
        Some(kbd) => button.tooltip_kbd(tooltip, kbd),
        None => button.tooltip(tooltip),
    };
    if let Some(counter) = counter {
        button = button.counter(counter);
    }
    if labels {
        button.label(label)
    } else {
        button
    }
}

/// What the Pull and Push toolbar buttons say about the current branch.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NetworkHints {
    pull_tip: String,
    push_tip: String,
    ahead: usize,
    behind: usize,
}

/// `1 commit` / `3 commits`.
fn commits(count: usize) -> String {
    format!("{count} commit{}", if count == 1 { "" } else { "s" })
}

/// The Pull / Push tooltips and counters, from the ahead/behind the refs
/// already carry (as of the last fetch) — no git command runs per frame.
fn network_hints(
    head: Option<&Head>,
    refs: Option<&[git_core::Ref]>,
    remotes: &[String],
) -> NetworkHints {
    let branch = match head {
        Some(Head::Branch { name, .. }) => Some(name.as_str()),
        _ => None,
    };
    let local = branch.and_then(|name| {
        refs.unwrap_or(&[])
            .iter()
            .find(|r| r.kind == RefKind::LocalBranch && r.name == name)
    });
    let upstream = local
        .filter(|local| !local.upstream_gone)
        .and_then(|local| local.upstream.as_deref());
    let (ahead, behind) = match (local, upstream) {
        (Some(local), Some(_)) => (local.ahead, local.behind),
        _ => (0, 0),
    };
    let pull_tip = match upstream {
        _ if ahead > 0 && behind > 0 => "Pull — branches diverged; fast-forward only".to_owned(),
        Some(upstream) if behind > 0 => format!("Pull {} from {upstream}", commits(behind)),
        Some(_) => "Pull — nothing to pull".to_owned(),
        None => "Pull — no upstream branch".to_owned(),
    };
    let push_tip = match (upstream, branch) {
        (Some(upstream), _) if ahead > 0 => format!("Push {} to {upstream}", commits(ahead)),
        (None, Some(branch)) => {
            // Same remote the push itself picks: `origin`, else the first.
            let remote = remotes
                .iter()
                .find(|remote| remote.as_str() == "origin")
                .or_else(|| remotes.first());
            match remote {
                Some(remote) => format!("Push and set upstream to {remote}/{branch}"),
                None => "Push the current branch".to_owned(),
            }
        }
        _ => "Push the current branch".to_owned(),
    };
    NetworkHints {
        pull_tip,
        push_tip,
        ahead,
        behind,
    }
}

/// The name and target of every ref, for "did the fetch change anything?".
fn ref_snapshot(repo: &Repo) -> Vec<(String, Oid)> {
    repo.refs()
        .map(|refs| {
            refs.into_iter()
                .map(|reference| (reference.name, reference.target))
                .collect()
        })
        .unwrap_or_default()
}

/// One diff row and its cells, in pixels.
const DIFF_ROW: f32 = 22.;
const DIFF_GUTTER: f32 = 44.;
const DIFF_MARKER: f32 = 16.;

/// A muted, centred message for a diff that has nothing to paint.
fn diff_note(text: &'static str, colors: ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(text)
}

/// The width of one diff row: its gutters plus the widest line, or the pane
/// width when the lines are shorter, so a line's background fills the pane.
fn diff_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let text = columns as f32 * f32::from(cell) + 24.;
    px((DIFF_GUTTER * 2. + DIFF_MARKER + text).max(pane))
}

/// One unified-view row: a hunk header, a numbered line or a no-newline note.
fn diff_row(
    row: &DiffRow,
    index: usize,
    width: Pixels,
    colors: ThemeColors,
    old_spans: &[HighlightSpan],
    new_spans: &[HighlightSpan],
    show_numbers: bool,
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("diff-row-{index}")))
        .debug_selector(move || format!("diff-row-{index}"))
        .h(px(DIFF_ROW))
        .w(width)
        .flex_none()
        .flex()
        .items_center()
        .whitespace_nowrap();
    match row.kind {
        RowKind::Hunk => base
            .px(px(8.))
            .bg(colors.diff_hunk_bg)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(row.text.clone())
            .into_any_element(),
        RowKind::Note => base
            .px(px(8.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(row.text.clone())
            .into_any_element(),
        RowKind::Context | RowKind::Added | RowKind::Removed => {
            let (old_background, new_background, line_background, strong) = match row.kind {
                RowKind::Added => (
                    None,
                    Some(colors.diff_add_bg),
                    Some(colors.diff_add_bg),
                    colors.diff_add_strong,
                ),
                RowKind::Removed => (
                    Some(colors.diff_del_bg),
                    None,
                    Some(colors.diff_del_bg),
                    colors.diff_del_strong,
                ),
                _ => (None, None, None, colors.diff_add_strong),
            };
            let spans = match row.kind {
                RowKind::Removed => old_spans,
                RowKind::Added => new_spans,
                _ => old_spans,
            };
            let mut base = base;
            if show_numbers {
                base = base
                    .child(diff_gutter(row.old_line, old_background, colors))
                    .child(diff_gutter(row.new_line, new_background, colors));
            }
            base.child(diff_marker(row.kind, line_background, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl(px(6.))
                        .pr(px(6.))
                        .when_some(line_background, |this, background| this.bg(background))
                        .text_color(colors.text)
                        .child(styled_diff_text(
                            &row.text,
                            &row.emphasis,
                            spans,
                            colors,
                            strong,
                        )),
                )
                .into_any_element()
        }
    }
}

/// One 44 px line-number gutter; an empty cell when the line is not on that
/// side of the diff.
fn diff_gutter(line: Option<u32>, background: Option<Hsla>, colors: ThemeColors) -> Div {
    div()
        .w(px(DIFF_GUTTER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(px(6.))
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.muted)
        .child(line.map(|line| line.to_string()).unwrap_or_default())
}

/// The 16 px `+` / `−` marker column.
fn diff_marker(kind: RowKind, background: Option<Hsla>, colors: ThemeColors) -> Div {
    let (marker, color) = match kind {
        RowKind::Added => ('+', colors.success),
        RowKind::Removed => ('−', colors.error),
        _ => (' ', colors.muted),
    };
    div()
        .w(px(DIFF_MARKER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .when_some(background, |this, background| this.bg(background))
        .text_color(color)
        .child(marker.to_string())
}

/// The width of one side-by-side row: two gutters and two equal text halves.
fn split_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let half = columns as f32 * f32::from(cell) + 12.;
    px((2. * DIFF_GUTTER + 2. * half).max(pane))
}

/// One side-by-side row: a hunk header, a note, or an old/new cell pair.
fn split_row(
    row: &SplitRow,
    index: usize,
    width: Pixels,
    half: Pixels,
    colors: ThemeColors,
    highlight: &Highlighted,
    show_numbers: bool,
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("split-row-{index}")))
        .debug_selector(move || format!("split-row-{index}"))
        .h(px(DIFF_ROW))
        .w(width)
        .flex_none()
        .flex()
        .items_center()
        .whitespace_nowrap();
    match row {
        SplitRow::Header(text) => base
            .px(px(8.))
            .bg(colors.diff_hunk_bg)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(text.clone())
            .into_any_element(),
        SplitRow::Note(text) => base
            .px(px(8.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(text.clone())
            .into_any_element(),
        SplitRow::Pair { old, new } => {
            let mut base = base;
            if show_numbers {
                base = base.child(split_gutter(old.as_ref(), colors));
            }
            base = base.child(split_content(
                old.as_ref(),
                half,
                colors,
                highlight.spans(true, old.as_ref().map(|cell| cell.line)),
            ));
            if show_numbers {
                base = base.child(
                    split_gutter(new.as_ref(), colors)
                        .border_l_1()
                        .border_color(colors.border),
                );
            }
            base.child(split_content(
                new.as_ref(),
                half,
                colors,
                highlight.spans(false, new.as_ref().map(|cell| cell.line)),
            ))
            .into_any_element()
        }
    }
}

/// A split row's line-number gutter; a missing side stays transparent.
fn split_gutter(cell: Option<&SplitCell>, colors: ThemeColors) -> Div {
    let background = cell.and_then(|cell| match cell.kind {
        SplitCellKind::Added => Some(colors.diff_add_bg),
        SplitCellKind::Removed => Some(colors.diff_del_bg),
        SplitCellKind::Context => None,
    });
    div()
        .w(px(DIFF_GUTTER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(px(6.))
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.muted)
        .child(cell.map(|cell| cell.line.to_string()).unwrap_or_default())
}

/// One half of a split row. A missing side is a hatched filler.
fn split_content(
    cell: Option<&SplitCell>,
    width: Pixels,
    colors: ThemeColors,
    spans: &[HighlightSpan],
) -> Div {
    let content = div()
        .w(width)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .pl(px(6.))
        .pr(px(6.))
        .overflow_hidden();
    let Some(cell) = cell else {
        return content.bg(gpui::pattern_slash(colors.active, 1., 5.));
    };
    let (background, strong) = match cell.kind {
        SplitCellKind::Added => (Some(colors.diff_add_bg), colors.diff_add_strong),
        SplitCellKind::Removed => (Some(colors.diff_del_bg), colors.diff_del_strong),
        SplitCellKind::Context => (None, colors.diff_add_strong),
    };
    content
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.text)
        .child(styled_diff_text(
            &cell.text,
            &cell.emphasis,
            spans,
            colors,
            strong,
        ))
}

/// A line's text with the word-diff ranges painted over the line background.
///
/// GPUI text runs carry the background but no corner radius, so the design's
/// 2 px radius is dropped, like the history search's highlight.
fn styled_diff_text(
    text: &str,
    emphasis: &[std::ops::Range<usize>],
    spans: &[HighlightSpan],
    colors: ThemeColors,
    background: Hsla,
) -> StyledText {
    if emphasis.is_empty() && spans.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    let font = gpui::font(FONT_MONO);
    let runs = syntax::line_segments(text, spans, emphasis)
        .into_iter()
        .map(|segment| {
            let color = segment
                .token
                .map(|token| token_color(token, colors))
                .unwrap_or(colors.text);
            diff_text_run(
                segment.range.len(),
                &font,
                color,
                segment.changed.then_some(background),
            )
        })
        .collect::<Vec<_>>();
    if runs.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
}

/// The theme colour of a syntax token.
fn token_color(token: Token, colors: ThemeColors) -> Hsla {
    match token {
        Token::Keyword => colors.code_kw,
        Token::String => colors.code_str,
        Token::Number => colors.code_num,
        Token::Comment => colors.code_com,
        Token::Type => colors.code_ty,
        Token::Function => colors.code_fn,
    }
}

fn diff_text_run(len: usize, font: &gpui::Font, color: Hsla, background: Option<Hsla>) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: background,
        underline: None,
        strikethrough: None,
    }
}

/// A muted one-line note, for an empty pane or a cleared selection.
fn muted_note(label: &'static str, colors: ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .p(px(16.))
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(label)
}

/// One label/value row of the details metadata grid.
fn meta_row(label: &'static str, value: impl IntoElement, colors: ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(74.))
                .flex_none()
                .text_color(colors.muted)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// A click, Enter/Space or ←/→ on an expandable row flips its expansion.
fn node_toggler(
    rows: Vec<(SharedString, bool, bool)>,
    view: Entity<RepositoryView>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    move |index, _, cx| {
        let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
            return;
        };
        if !has_children {
            return;
        }
        view.update(cx, |view, cx| view.toggle_node(&key, expanded, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    /// Builds the real window (the workspace frame around the repository
    /// view) in a test window and returns the bounds of the sidebar filter.
    fn sidebar_filter_bounds(cx: &mut gpui::TestAppContext) -> gpui::Bounds<Pixels> {
        use gpui::{
            point, size, Bounds, VisualTestContext, WindowBackgroundAppearance, WindowBounds,
            WindowDecorations, WindowOptions,
        };

        cx.update(ui::init).expect("ui::init");
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1600.), px(1000.)),
                    ))),
                    // The app always asks for client-side decorations.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        crate::workspace::Workspace::new(
                            Some(PathBuf::from("/nonexistent")),
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
        cx.debug_bounds("sidebar-filter")
            .expect("the sidebar filter was not drawn")
    }

    #[gpui::test]
    fn the_sidebar_filter_spans_the_sidebar(cx: &mut gpui::TestAppContext) {
        let bounds = sidebar_filter_bounds(cx);
        // The sidebar is 240 wide with a 1px right border and 10px padding
        // on each side of the filter area.
        assert_eq!(
            f32::from(bounds.size.width),
            SIDEBAR_WIDTH_DEFAULT - 1. - 20.,
            "the filter field collapsed instead of spanning the sidebar"
        );
        assert_eq!(f32::from(bounds.size.height), 26.);
    }

    #[test]
    fn toolbar_keeps_the_window_controls_visible_at_every_width() {
        // Inter 13px, generously rounded up per character: the prototype's own
        // measurement is 51px for "gitwarp" (7.3px/char).
        fn text(chars: usize) -> f32 {
            chars as f32 * 8.
        }
        // Every item at its worst case, in the same terms as the toolbar.
        fn needed(layout: ToolbarLayout) -> f32 {
            let name_item = |name: Option<f32>| {
                let name = name.unwrap_or(0.);
                2. * layout.item_pad + 14. + 6. + name + 6. + 12.
            };
            // Button: 8px padding each side, 14px icon, 6px gap.
            let tools = if layout.labels {
                4. * (16. + 14. + 6.)
                    + text("Fetch".len())
                    + text("Pull".len())
                    + text("Push".len())
                    + text("Stash".len())
            } else {
                // Icon-only tool buttons are 28 wide (`Button` squares them).
                112.
            };
            // Segment: 10px padding each side, 14px icon, 6px gap, 1px border.
            let segmented = if layout.labels {
                2. * (20. + 14. + 6.) + text("History".len()) + text("Changes".len()) + 2.
            } else {
                70.
            };
            let fixed = 26. // sidebar toggle
                + name_item(layout.repo_name)
                + name_item(layout.branch_name)
                + 9. // separator
                + tools
                + segmented
                + 26. // settings
                + 104.; // window controls
            fixed + layout.search_min + 2. * layout.padding + 12. * layout.gap
        }

        for width in [
            620., 700., 760., 810., 870., 1000., 1200., 1360., 1400., 1600.,
        ] {
            let layout = toolbar_layout(width);
            // The client-side frame keeps a 10px shadow band and a 1px border
            // on each side.
            let content = width - 22.;
            assert!(
                needed(layout) <= content,
                "toolbar overflows at {width}: needs {} of {content}",
                needed(layout)
            );
        }
    }

    #[test]
    fn toolbar_steps_hide_labels_and_names_in_order() {
        assert!(toolbar_layout(1600.).labels);
        assert!(toolbar_layout(1360.).labels);
        assert!(!toolbar_layout(1359.).labels);
        assert!(!toolbar_layout(1200.).labels);
        assert_eq!(toolbar_layout(1600.).repo_name, Some(140.));
        assert_eq!(toolbar_layout(1200.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(1000.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(900.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(820.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(800.).repo_name, None);
        assert_eq!(toolbar_layout(800.).branch_name, Some(64.));
        assert_eq!(toolbar_layout(720.).branch_name, None);
        assert_eq!(toolbar_layout(650.).search_min, 72.);
        assert_eq!(toolbar_layout(650.).padding, 8.);
        assert_eq!(toolbar_layout(720.).search_min, 96.);
        assert_eq!(toolbar_layout(1200.).search_min, 150.);
        assert!(!toolbar_layout(900.).search_kbd);
        assert!(!toolbar_layout(1200.).search_kbd);
        assert!(toolbar_layout(1600.).search_kbd);
        assert_eq!(toolbar_layout(1200.).gap, 6.);
        assert_eq!(toolbar_layout(800.).gap, 4.);
        assert_eq!(toolbar_layout(1200.).item_pad, 8.);
        assert_eq!(toolbar_layout(800.).item_pad, 7.);
    }

    #[test]
    fn dock_defaults_to_right_only_on_wide_windows() {
        assert_eq!(resolve_dock(None, 1400.), Dock::Right);
        assert_eq!(resolve_dock(None, 1399.), Dock::Bottom);
        assert_eq!(resolve_dock(Some(Dock::Bottom), 1600.), Dock::Bottom);
        assert_eq!(resolve_dock(Some(Dock::Right), 800.), Dock::Right);
    }

    #[test]
    fn details_size_is_clamped_to_the_dock() {
        assert_eq!(dock_default(Dock::Bottom), 340.);
        assert_eq!(dock_default(Dock::Right), 640.);
        assert_eq!(clamp_details(100., Dock::Bottom), 220.);
        assert_eq!(clamp_details(999., Dock::Bottom), 560.);
        assert_eq!(clamp_details(999., Dock::Right), 900.);
        assert_eq!(clamp_details(f32::NAN, Dock::Right), 640.);
    }

    #[test]
    fn right_docked_details_never_take_over_half_the_pane() {
        // The live bug: 1270 px with a 420 px sidebar and a 900 px panel left
        // the history pane ~200 px. Half the space right of the sidebar is
        // 425 px.
        assert_eq!(effective_details_width(900., 1270., 420.), 425.);
        assert_eq!(details_width_limit(1270., 420.), 425.);
        // A stored 640 px panel still fits on a wide window.
        assert_eq!(effective_details_width(640., 1600., 240.), 640.);
        assert_eq!(details_width_limit(1600., 240.), 680.);
        // On a tiny window the 220 px minimum beats the half rule.
        assert_eq!(effective_details_width(900., 500., 300.), 220.);
        assert_eq!(details_width_limit(500., 300.), 220.);
    }

    #[test]
    fn the_sidebar_never_takes_more_than_half_the_window() {
        assert_eq!(sidebar_width_limit(1600.), 800.);
        assert_eq!(sidebar_width_limit(1000.), 500.);
        // The 160 px minimum beats the half rule on a tiny window.
        assert_eq!(sidebar_width_limit(300.), 160.);

        // The stored width stands while it fits in the allowed half.
        assert_eq!(effective_sidebar_width(240., 1600.), 240.);
        assert_eq!(effective_sidebar_width(420., 1000.), 420.);
        // Wider stored values render at half the visible area.
        assert_eq!(effective_sidebar_width(900., 1600.), 800.);
        assert_eq!(effective_sidebar_width(420., 500.), 250.);
        assert_eq!(effective_sidebar_width(420., 300.), 160.);
    }

    #[test]
    fn details_header_keeps_its_height() {
        assert_eq!(DOCK_HEADER, px(34.));
        assert_eq!(ui::metrics::BAR, px(40.));
        assert_eq!(STATUS, px(24.));
    }

    /// Makes one commit on the repository.
    fn commit(repo: &Path, file: &str, message: &str) {
        std::fs::write(repo.join(file), message).expect("write the committed file");
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test User")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["add", file]);
        run(&["commit", "-q", "-m", message]);
    }

    /// Opens the repository view as the window root on a real repository and
    /// waits for its history to load.
    fn open_repository_view(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, Entity<RepositoryView>) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        let git = crate::test_support::git();
        cx.update(ui::init).expect("ui::init");
        cx.update(bind_search_keys);
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1400.), px(900.)),
                    ))),
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    let workspace = cx.new(|cx| {
                        Workspace::new(None, Ok(git.clone()), git_core::GitChoice::Auto, window, cx)
                    });
                    let weak = workspace.downgrade();
                    cx.new(|cx| {
                        RepositoryView::new(
                            repo.path().to_path_buf(),
                            Ok(git.clone()),
                            weak,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let view = cx
            .update(|window, _| window.root::<RepositoryView>().flatten())
            .expect("the repository view is the window root");
        (cx, view)
    }

    /// The toolbar switch replaces the history pane with the local-changes
    /// view and back; the changes view loads the work-tree status.
    #[gpui::test]
    fn the_view_switch_opens_local_changes(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("view-switch");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::Changes, window, cx));
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.view, ViewMode::Changes);
            let changes = view.changes.read(cx);
            assert_eq!(changes.unstaged_count(), 1, "{:?}", changes.status());
        });
        assert!(cx.debug_bounds("changes-view").is_some());
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_some(),
            "the changed file was not drawn"
        );

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::History, window, cx));
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.view, ViewMode::History));
        assert!(cx.debug_bounds("changes-view").is_none());
    }

    /// The Worktrees page replaces the history pane and lists the repository's
    /// worktrees; switching back to History removes it.
    #[gpui::test]
    fn the_worktrees_page_lists_the_worktrees(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktrees-page");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_view(ViewMode::Worktrees, window, cx)
            });
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.view, ViewMode::Worktrees));
        assert!(cx.debug_bounds("worktrees-view").is_some());
        assert!(
            cx.debug_bounds("worktree-row-0").is_some(),
            "the main worktree row was not drawn"
        );

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::History, window, cx));
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("worktrees-view").is_none());
    }

    /// A commit made in the changes view reloads the history behind it.
    #[gpui::test]
    fn committing_from_changes_reloads_the_history(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-commit-reload");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::Changes, window, cx));
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.changes.update(cx, |changes, cx| {
                    changes.refresh(cx);
                    changes.set_message_for_test("feat: two", cx);
                });
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.changes.update(cx, |changes, cx| {
                    changes.stage_all(crate::changes_view::Side::Unstaged, cx)
                });
            });
            let _ = window;
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.changes
                    .update(cx, |changes, cx| changes.commit(window, cx));
            });
        });
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let history = view.history.read(cx);
            assert_eq!(history.commit_count(), 2, "the new commit is missing");
            assert_eq!(
                history.selected_commit().expect("HEAD is selected").summary,
                "feat: two"
            );
        });
    }

    /// Selecting a commit loads its details and changed files into the dock,
    /// and clicking another row replaces them.
    #[gpui::test]
    fn selecting_a_commit_fills_the_details_dock(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-panel");
        commit(repo.path(), "a.txt", "one\ntwo\n");
        commit(repo.path(), "b.txt", "three\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        view.read_with(&cx, |view, _| {
            let details = view
                .details
                .details()
                .expect("the selected commit's details did not load");
            assert_eq!(details.summary, "three");
            let files = view
                .details
                .files()
                .expect("the changed files did not load");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "b.txt");
            assert_eq!(
                view.selected_file,
                Some(0),
                "the first file is not selected"
            );
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the selected file's diff did not load");
            };
            assert!(diff.rows.len() > 1, "the diff has no body rows");
            assert!(diff.rows.iter().any(|row| row.text.contains("three")));
        });
        assert!(
            cx.debug_bounds("details-file-0").is_some(),
            "the changed file row was not drawn"
        );
        assert!(
            cx.debug_bounds("diff-row-0").is_some(),
            "the diff rows were not drawn"
        );

        // Clicking the older row loads that commit instead.
        let row = cx
            .debug_bounds("history-row-1")
            .expect("the second history row was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            let details = view
                .details
                .details()
                .expect("the clicked commit's details did not load");
            assert_eq!(details.summary, "one two");
            let files = view
                .details
                .files()
                .expect("the changed files did not load");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "a.txt");
            assert_eq!(view.selected_file, Some(0));
        });
    }

    /// Clicking a changed file selects it, loads its diff and takes the
    /// keyboard: ↑/↓ then move the file selection.
    #[gpui::test]
    fn the_file_list_selects_files_and_answers_arrow_keys(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-files");
        std::fs::write(repo.path().join("a.txt"), "alpha\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "beta\n").unwrap();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["add", "-A"])
            .output()
            .expect("git runs");
        assert!(out.status.success());
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["commit", "-q", "-m", "two files"])
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git runs");
        assert!(out.status.success());

        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.details.files().map(<[git_core::ChangedFile]>::len),
                Some(2)
            );
            assert_eq!(view.selected_file, Some(0));
        });

        let row = cx
            .debug_bounds("details-file-1")
            .expect("the second changed file was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.selected_file, Some(1));
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the second file's diff did not load");
            };
            assert!(diff.rows.iter().any(|row| row.text.contains("beta")));
        });

        cx.simulate_keystrokes("up");
        view.read_with(&cx, |view, _| assert_eq!(view.selected_file, Some(0)));
        cx.simulate_keystrokes("down");
        view.read_with(&cx, |view, _| assert_eq!(view.selected_file, Some(1)));
    }

    /// Clicking a parent's short hash selects that commit in the history.
    #[gpui::test]
    fn clicking_a_parent_hash_selects_that_commit(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-parent");
        commit(repo.path(), "a.txt", "one\n");
        commit(repo.path(), "b.txt", "two\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.details
                    .details()
                    .map(|details| details.summary.as_str()),
                Some("two")
            );
        });

        let parent = cx
            .debug_bounds("details-parent-0")
            .expect("the parent link was not drawn");
        cx.simulate_click(parent.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert_eq!(
                view.details
                    .details()
                    .map(|details| details.summary.as_str()),
                Some("one"),
                "the parent commit's details did not load"
            );
            assert_eq!(
                view.history
                    .read(cx)
                    .selected_commit()
                    .map(|c| c.summary.clone()),
                Some("one".to_string()),
                "the parent was not selected in the history"
            );
        });
    }

    /// The layout toggle switches the pane to side-by-side rows and stores the
    /// choice in the settings.
    #[gpui::test]
    fn the_diff_toggle_switches_to_side_by_side_and_persists(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("details-split");
        commit(repo.path(), "main.rs", "fn main() {}\n");
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings::default(),
                save_in_flight: false,
                dirty: false,
            });
        });
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.diff_layout, DiffLayout::Unified);
        });

        view.update(&mut cx, |view, cx| {
            view.set_diff_layout(DiffLayout::Split, cx)
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.diff_layout, DiffLayout::Split);
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the diff did not load");
            };
            assert!(!diff.split_rows.is_empty());
            assert_eq!(diff.highlight.language, Some("rust"));
            assert!(
                !diff.highlight.new.is_empty(),
                "the new side was not highlighted"
            );
        });
        assert!(
            cx.debug_bounds("split-row-0").is_some(),
            "the side-by-side rows were not drawn"
        );
        let saved = cx.update(|_, cx| cx.global::<crate::AppSettings>().settings.diff_layout);
        assert_eq!(saved, DiffLayout::Split, "the layout was not persisted");
    }

    /// Lets the 150 ms search debounce and the background scan finish. The
    /// test scheduler does not advance its clock on its own while parked.
    fn settle_search(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
    }

    /// Typing a query into the toolbar field jumps the list to the first
    /// match; Enter / F3 step forward and Shift+Enter back, both wrapping; Esc
    /// clears the field and hands the list the keyboard.
    #[gpui::test]
    fn the_search_field_jumps_to_matches_and_steps_through_them(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("history-search");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta one");
        commit(repo.path(), "c.txt", "Gamma");
        commit(repo.path(), "d.txt", "Beta two");

        let (mut cx, view) = open_repository_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        // Newest first: 0 "Beta two", 1 "Gamma", 2 "Beta one", 3 "Alpha".
        assert_eq!(history.read_with(&cx, |view, _| view.commit_count()), 4);

        let field = cx
            .debug_bounds("toolbar-search")
            .expect("the toolbar search was not drawn");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();

        // A match below the current selection is selected after the debounce.
        cx.simulate_input("gamma");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(1),
            "typing did not jump to the matching row"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 1".to_owned())
        );

        // Esc clears the query, the count disappears and the list gets the
        // keyboard without moving the selection.
        cx.simulate_keystrokes("escape");
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status()),
            None,
            "Esc did not clear the search"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(1)
        );
        let text = view.read_with(&cx, |view, app| view.search.read(app).text().to_owned());
        assert!(text.is_empty(), "Esc did not clear the field's text");
        cx.update(|window, app| {
            assert!(
                history.read(app).list_focus().is_focused(window),
                "Esc did not focus the history list"
            );
        });

        // Two matches, and the selection is on row 1 ("Gamma"), so the first
        // match at or after it is row 2.
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input("beta");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("2 / 2".to_owned())
        );

        cx.simulate_keystrokes("enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0),
            "Enter did not wrap to the first match"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 2".to_owned())
        );
        cx.simulate_keystrokes("enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0),
            "Shift+Enter did not wrap back to the first match"
        );
        cx.simulate_keystrokes("f3");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2),
            "F3 did not step to the next match"
        );

        // ↓ keeps the selection and gives the list the keyboard.
        cx.simulate_keystrokes("down");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        cx.update(|window, app| {
            assert!(
                history.read(app).list_focus().is_focused(window),
                "↓ did not focus the history list"
            );
        });
    }

    /// Enter inside the debounce window applies the field's text first, so the
    /// step (and the scan's jump) uses the new query, not the previous one's
    /// matches.
    #[gpui::test]
    fn enter_during_the_debounce_applies_the_new_query(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("history-search-debounce");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta one");
        commit(repo.path(), "c.txt", "Gamma");
        commit(repo.path(), "d.txt", "Beta two");

        let (mut cx, view) = open_repository_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        let field = cx
            .debug_bounds("toolbar-search")
            .expect("the toolbar search was not drawn");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();

        // Apply "beta": it matches rows 0 and 2, and the jump selects row 0,
        // the first match at or after the initial selection.
        cx.simulate_input("beta");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0)
        );

        // A new query followed by Enter before the debounce fires must search
        // "alpha" (row 3), not step over the old "beta" matches.
        cx.simulate_keystrokes("escape");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input("alpha");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(3),
            "Enter inside the debounce did not apply the new query"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 1".to_owned())
        );
    }
    /// The history row's "Create branch here…" opens the dialog, and its
    /// primary button creates the branch and switches to it.
    #[gpui::test]
    fn the_create_branch_dialog_creates_and_switches(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("branch-dialog");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        let oid = view
            .read_with(&cx, |view, cx| view.head_oid(cx))
            .expect("the history loaded HEAD");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let card = view.card_for(oid, cx);
                view.handle_history_action(
                    crate::ref_ops::RefAction::CreateBranch(card),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        let dialog = view
            .read_with(&cx, |view, _| view.ref_dialog.clone())
            .expect("the branch dialog opened");
        dialog.update(&mut cx, |dialog, cx| {
            dialog.set_name_for_test("feat/two", cx)
        });
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| dialog.submit_for_test(window, cx));
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let repo = git_core::Repo::open(crate::test_support::git(), repo.path()).unwrap();
        assert!(repo
            .refs()
            .unwrap()
            .iter()
            .any(|r| r.name == "feat/two" && r.kind == git_core::RefKind::LocalBranch));
        match repo.head().unwrap() {
            Head::Branch { name, .. } => assert_eq!(name, "feat/two"),
            Head::Detached(_) => panic!("creating a branch left a detached HEAD"),
        }
        assert_eq!(
            std::fs::read_to_string(repo.root().join("a.txt")).unwrap(),
            "one\n"
        );
    }

    /// The toolbar's Stash button stashes every local change.
    #[gpui::test]
    fn the_stash_button_saves_local_changes(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("stash-button");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.stash_changes(window, cx));
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let repo = git_core::Repo::open(crate::test_support::git(), repo.path()).unwrap();
        assert_eq!(repo.stashes().unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(repo.root().join("a.txt")).unwrap(),
            "one\n"
        );
    }

    /// Checking out a branch from the sidebar switcher moves HEAD and reloads
    /// the history.
    #[gpui::test]
    fn a_sidebar_checkout_moves_head(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("sidebar-checkout");
        commit(repo.path(), "a.txt", "one\n");
        let oid = git_core::Repo::open(crate::test_support::git(), repo.path())
            .unwrap()
            .head_commit()
            .unwrap()
            .unwrap()
            .id;
        git_core::Repo::open(crate::test_support::git(), repo.path())
            .unwrap()
            .create_branch("feature", oid, false)
            .unwrap();

        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.run_sidebar_action(
                    crate::sidebar::SidebarAction::CheckoutBranch("feature".into()),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let repo = git_core::Repo::open(crate::test_support::git(), repo.path()).unwrap();
        match repo.head().unwrap() {
            Head::Branch { name, .. } => assert_eq!(name, "feature"),
            Head::Detached(_) => panic!("checkout left a detached HEAD"),
        }
    }

    /// Clicks the sidebar row with `key` so the tree holds the keyboard.
    fn click_sidebar_row(
        cx: &mut gpui::VisualTestContext,
        view: &Entity<RepositoryView>,
        key: &str,
    ) {
        let index = view
            .update(cx, |view, cx| {
                view.tree_nodes(cx)
                    .iter()
                    .position(|node| node.key.as_ref() == key)
            })
            .unwrap_or_else(|| panic!("no sidebar row {key}"));
        // `debug_bounds` wants a `'static` name; a leak in a test is harmless.
        let name: &'static str = Box::leak(format!("sidebar-tree-node-{index}").into_boxed_str());
        let row = cx
            .debug_bounds(name)
            .expect("the sidebar row was not drawn");
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        cx.run_until_parked();
    }

    fn branch_names(repo: &crate::test_support::TempRepo) -> Vec<String> {
        git_core::Repo::open(crate::test_support::git(), repo.path())
            .unwrap()
            .refs()
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == git_core::RefKind::LocalBranch)
            .map(|r| r.name)
            .collect()
    }

    fn repo_with_feature_branch(name: &str) -> crate::test_support::TempRepo {
        let repo = crate::test_support::TempRepo::new(name);
        commit(repo.path(), "a.txt", "one\n");
        let git = crate::test_support::git();
        let repo_handle = git_core::Repo::open(git, repo.path()).unwrap();
        let oid = repo_handle.head_commit().unwrap().unwrap().id;
        repo_handle.create_branch("feature", oid, false).unwrap();
        repo
    }

    /// F2 on a focused sidebar branch row opens the Rename dialog.
    #[gpui::test]
    fn f2_on_a_sidebar_branch_row_opens_rename(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("sidebar-f2");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        click_sidebar_row(&mut cx, &view, "local:feature");

        cx.simulate_keystrokes("f2");
        cx.run_until_parked();
        let dialog = view
            .read_with(&cx, |view, _| view.ref_dialog.clone())
            .expect("F2 did not open the rename dialog");
        dialog.update(&mut cx, |dialog, cx| {
            dialog.set_name_for_test("renamed", cx)
        });
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| dialog.submit_for_test(window, cx));
        });
        cx.run_until_parked();
        cx.run_until_parked();
        let names = branch_names(&repo);
        assert!(names.iter().any(|n| n == "renamed"), "{names:?}");
        assert!(!names.iter().any(|n| n == "feature"), "{names:?}");
    }

    /// Enter on a focused sidebar branch row checks the branch out.
    #[gpui::test]
    fn enter_on_a_sidebar_branch_row_checks_it_out(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("sidebar-enter");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        click_sidebar_row(&mut cx, &view, "local:feature");

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.run_until_parked();
        let repo = git_core::Repo::open(crate::test_support::git(), repo.path()).unwrap();
        match repo.head().unwrap() {
            Head::Branch { name, .. } => assert_eq!(name, "feature"),
            Head::Detached(_) => panic!("Enter left a detached HEAD"),
        }
    }

    /// B on the focused history list opens "Create branch here…".
    #[gpui::test]
    fn b_on_the_history_list_opens_create_branch(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-b");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.update(|_, cx| {
            crate::history_view::bind_history_keys(cx, &crate::settings::Settings::default());
        });
        cx.run_until_parked();
        let history = view.read_with(&cx, |view, _| view.history());
        cx.update(|window, cx| {
            let focus = history.read(cx).list_focus();
            window.focus(&focus, cx);
        });

        cx.simulate_keystrokes("b");
        cx.run_until_parked();
        let dialog = view
            .read_with(&cx, |view, _| view.ref_dialog.clone())
            .expect("B did not open the create-branch dialog");
        dialog.update(&mut cx, |dialog, cx| dialog.set_name_for_test("from-b", cx));
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| dialog.submit_for_test(window, cx));
        });
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(branch_names(&repo).iter().any(|n| n == "from-b"));
    }

    fn local_ref(name: &str, upstream: Option<&str>, ahead: usize, behind: usize) -> git_core::Ref {
        git_core::Ref {
            full_name: format!("refs/heads/{name}"),
            name: name.to_owned(),
            kind: RefKind::LocalBranch,
            target: Oid::from_hex(&"1".repeat(40)).unwrap(),
            upstream: upstream.map(str::to_owned),
            ahead,
            behind,
            upstream_gone: false,
        }
    }

    #[test]
    fn pull_and_push_tooltips_follow_ahead_and_behind() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let remotes = vec!["origin".to_owned()];
        let hints = |refs: &[git_core::Ref]| network_hints(Some(&head), Some(refs), &remotes);

        let idle = hints(&[local_ref("main", Some("origin/main"), 0, 0)]);
        assert_eq!(idle.pull_tip, "Pull — nothing to pull");
        assert_eq!(idle.push_tip, "Push the current branch");
        assert_eq!((idle.ahead, idle.behind), (0, 0));

        let ahead = hints(&[local_ref("main", Some("origin/main"), 3, 0)]);
        assert_eq!(ahead.push_tip, "Push 3 commits to origin/main");
        assert_eq!(ahead.ahead, 3);

        let behind = hints(&[local_ref("main", Some("origin/main"), 0, 1)]);
        assert_eq!(behind.pull_tip, "Pull 1 commit from origin/main");

        let diverged = hints(&[local_ref("main", Some("origin/main"), 2, 2)]);
        assert_eq!(
            diverged.pull_tip,
            "Pull — branches diverged; fast-forward only"
        );

        let fresh = hints(&[local_ref("main", None, 0, 0)]);
        assert_eq!(fresh.push_tip, "Push and set upstream to origin/main");
        assert_eq!(fresh.ahead, 0);
    }

    /// Makes one commit authored by `email` at `date` (ISO, UTC).
    fn commit_at(repo: &Path, file: &str, message: &str, date: &str, email: &str) {
        std::fs::write(repo.join(file), message).expect("write the committed file");
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test User")
                .env("GIT_AUTHOR_EMAIL", email)
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_NAME", "Test User")
                .env("GIT_COMMITTER_EMAIL", email)
                .env("GIT_COMMITTER_DATE", date)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["add", file]);
        run(&["commit", "-q", "-m", message]);
    }

    /// Three commits: two by different authors on 10 March 2026 (noon UTC, so
    /// the same local day in nearly every timezone) and one on the 12th. The
    /// repository's own `user.email` is the first author.
    fn day_filter_repo(name: &str) -> (crate::test_support::TempRepo, i32) {
        let repo = crate::test_support::TempRepo::new(name);
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["config", "user.email", "a@x.io"])
            .status()
            .expect("git runs");
        assert!(status.success());
        commit_at(repo.path(), "a", "c1", "2026-03-10T12:00:00Z", "a@x.io");
        commit_at(repo.path(), "b", "c2", "2026-03-10T13:00:00Z", "b@x.io");
        commit_at(repo.path(), "c", "c3", "2026-03-12T12:00:00Z", "a@x.io");
        (repo, git_core::days_from_civil(2026, 3, 10))
    }

    /// A day filter narrows the History list to that day's commits (honouring
    /// Me / Everyone), and Esc on the focused list clears it again.
    #[gpui::test]
    fn the_day_filter_lists_one_days_commits_and_escape_clears_it(cx: &mut gpui::TestAppContext) {
        use crate::history_view::DayFilter;
        let (repo, day) = day_filter_repo("day-filter");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        let history = view.read_with(&cx, |view, _| view.history.clone());

        history.update(&mut cx, |history, cx| {
            history.set_day_filter(Some(DayFilter { day, me: None }), cx)
        });
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert_eq!(history.day_filter_count(), Some(2));
            // Newest first: the filtered list selects its first row.
            assert_eq!(history.selected_commit().expect("a row").summary, "c2");
        });

        // Me keeps only the repository's own email.
        history.update(&mut cx, |history, cx| {
            history.set_day_filter(
                Some(DayFilter {
                    day,
                    me: Some("A@X.io".to_owned()),
                }),
                cx,
            )
        });
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert_eq!(history.day_filter_count(), Some(1));
            assert_eq!(history.selected_commit().expect("a row").summary, "c1");
        });

        // A day without commits is an empty list, not the whole history.
        history.update(&mut cx, |history, cx| {
            history.set_day_filter(
                Some(DayFilter {
                    day: day + 1,
                    me: None,
                }),
                cx,
            )
        });
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert_eq!(history.day_filter_count(), Some(0));
            assert!(history.selected_commit().is_none());
        });

        // Esc with the list focused clears the filter and re-selects HEAD.
        history.update(&mut cx, |history, cx| {
            history.set_day_filter(Some(DayFilter { day, me: None }), cx)
        });
        cx.run_until_parked();
        let focus = history.read_with(&cx, |history, _| history.list_focus());
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert!(history.day_filter().is_none(), "Esc left the filter on");
            assert_eq!(history.commit_count(), 3);
            assert_eq!(history.selected_commit().expect("HEAD").summary, "c3");
        });
    }

    /// The strip counts the repository's days; clicking a day filters to it
    /// and clicking it again clears the filter.
    #[gpui::test]
    fn the_activity_strip_picks_and_unpicks_a_day(cx: &mut gpui::TestAppContext) {
        let (repo, day) = day_filter_repo("strip-pick");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        let (strip, history) =
            view.read_with(&cx, |view, _| (view.activity.clone(), view.history.clone()));
        strip.read_with(&cx, |strip, _| assert!(strip.is_ready()));

        // Me is the default author here (user.email is set): c1 and c3 are
        // a@x.io's. They all fall in the last 12 months only while the clock
        // is before March 2027, so only the day pick is checked, not a total.
        strip.update(&mut cx, |strip, cx| strip.pick_day(day, cx));
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            let filter = history.day_filter().expect("the day is filtered");
            assert_eq!(filter.day, day);
            assert_eq!(filter.me.as_deref(), Some("a@x.io"));
            assert_eq!(history.day_filter_count(), Some(1));
        });
        strip.update(&mut cx, |strip, cx| strip.pick_day(day, cx));
        cx.run_until_parked();
        history.read_with(&cx, |history, _| assert!(history.day_filter().is_none()));

        // A day nobody committed on is not pickable.
        strip.update(&mut cx, |strip, cx| strip.pick_day(day + 1, cx));
        cx.run_until_parked();
        history.read_with(&cx, |history, _| assert!(history.day_filter().is_none()));
    }

    /// The strip starts collapsed when the settings say so, and collapsing it
    /// writes the preference back.
    #[gpui::test]
    fn the_activity_strip_remembers_collapsing(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("strip-collapse");
        // `commit` authors as test@example.com; "Me" counts that email.
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["config", "user.email", "test@example.com"])
            .status()
            .expect("git runs");
        assert!(status.success());
        commit(repo.path(), "a.txt", "one\n");
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings {
                    activity_collapsed: true,
                    ..crate::settings::Settings::default()
                },
                save_in_flight: false,
                dirty: false,
            });
        });
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        let strip = view.read_with(&cx, |view, _| view.activity.clone());
        strip.read_with(&cx, |strip, _| {
            assert!(strip.is_collapsed(), "the saved state was ignored");
            assert_eq!(strip.total(), Some(1));
        });

        strip.update(&mut cx, |strip, cx| strip.set_collapsed(false, cx));
        cx.update(|_, cx| {
            assert!(
                !cx.global::<crate::AppSettings>()
                    .settings
                    .activity_collapsed
            );
        });
    }
}
