//! The repository shell: toolbar, sidebar, history, details dock and status bar.
//!
//! One [`RepositoryView`] lives per open repository; the [`Workspace`] owns it
//! and swaps it for the welcome screen. Everything that belongs to a later
//! stage is a placeholder or a disabled control, never a dead click.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, size, uniform_list, AnyElement, App, AppContext, Bounds,
    ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, ListHorizontalSizingBehavior, MouseButton, ParentElement,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    StyledText, Subscription, Task, TextRun, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::network::{FetchOptions, Progress};
use git_core::{ChangedFile, CommitDetails, CommitInfo, Error, Git, Head, Oid, Ref, RefKind, Repo};
use ui::actions::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::metrics::{DOCK_HEADER, FONT_MONO, FONT_UI, FS_BASE, FS_MD, FS_SM, FS_XS, R_SM, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    menu, toast, Avatar, Banner, BannerKind, Button, ButtonSize, CiGlyph, CountPill, Counter,
    EmptyState, FileRow, Icon, IconName, MenuEntry, MenuItem, PrBadge, Scrollbar, SearchField,
    SegmentItem, Segmented, SelectableText, Skeleton, Spinner, Splitter, TextInput, TextInputEvent,
    Tooltip, Tree, TreeNodeData, TreePr,
};

use crate::activity_strip::ActivityStrip;
use crate::changes_view::{change_counts, ChangesEvent, ChangesView};
use crate::create_pr_dialog::{self, CreatePrDialog, PrDialogContext, PushRun};
use crate::details::{read_details, read_diff, DetailsState, DiffState};
use crate::diff_model::{DiffRow, RowKind, SplitCell, SplitCellKind, SplitRow};
use crate::error_dialog::{self, ErrorAction, KnownError};
use crate::format::{absolute_time, format_count, relative_time};
use crate::history_state::LoadState;
use crate::history_view::{HistoryEvent, HistoryView};
use crate::hosting_status::{
    self, CiEntry, ErrorEffect, HostingState, HostingView, PrPopover, PrTrigger, Request, Target,
};
use crate::network_ops::{self, Job};
use crate::network_service::NetworkService;
use crate::palette::{self, Command};
use crate::pr_logic::{self, PrTarget};
use crate::pr_panel::{PrEvent, PrModel, PrPanel};
use crate::ref_dialog::{self, RefDialog, RefDialogContext, RefDialogKind, RemoteEdit};
use crate::ref_ops::{CommitCard, RefAction};
use crate::remote_accounts::{self, AccountAction, RemoteResolution};
use crate::repo_watch::{
    GitLayout, Kinds, Mode as WatchMode, RepoWatcher, StartError, WatchSignal, OPERATION_MARKERS,
};
use crate::settings::{
    sidebar_width_limit, DiffLayout, Dock, SIDEBAR_WIDTH_DEFAULT, SIDEBAR_WIDTH_MAX,
    SIDEBAR_WIDTH_MIN,
};
use crate::settings_ui::account_dialog::{self, Open as AccountOpen};
use crate::settings_ui::accounts::open_picker;
use crate::sidebar::{
    commit_for, read_sidebar_extras, shortcut_action, sidebar_menu_with_links, sidebar_nodes_with,
    worktree_menu, worktree_panel_rows, Expansion, PanelEvent, PanelState, RemoteDisplays,
    SidebarAction, SidebarActionHandler, SidebarExtras, SidebarMenuEntry, WorktreeAction,
    WorktreePanel,
};
use crate::syntax::{self, HighlightSpan, Highlighted, Token};
use crate::web_links::{self, HostLink};
use crate::workspace::{
    git_label, git_tooltip, inline_window_controls, repo_name, title_strip, titlebar_end,
    titlebar_fill, uses_title_strip, window_titlebar, Workspace,
};
use crate::worktree_dialog::SelectedCommit;
use crate::worktree_switcher;
use crate::worktrees_model::{WorktreeEntry, WorktreesModel, WorktreesModelEvent};
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
        OpenWorktreeSwitcher,
        StashChanges,
        FetchAll,
        Refresh,
        PullBranch,
        PushBranch
    ]
);

/// The centre views the toolbar's segment switches between. `Worktrees` also
/// swaps the sidebar tree for the worktree panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    History,
    Changes,
    Worktrees,
}

/// What the sidebar shows besides its filter field: the repository tree or
/// the pull request panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarTab {
    Repository,
    PullRequests,
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
pub(crate) const TOOL_H: f32 = 28.;

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
    /// The worktree switcher button is shown.
    pub worktree_button: bool,
    /// Widest the worktree switcher's name may get; `None` hides it.
    pub worktree_name: Option<f32>,
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
            worktree_button: true,
            worktree_name: Some(96.),
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

    /// One more step at the narrowest window: the three view tabs took the
    /// room the search and the gaps kept.
    fn tighter(self) -> Self {
        Self { gap: 3., ..self }
    }
}

/// The toolbar steps, measured against the real text widths of every item.
///
/// The window's client-side frame keeps a 10px shadow band and a 1px border on
/// each side, so a window of `width` has `width - 22` of content. Every item
/// except the search field is `flex-none`, so the search is the only item that
/// can shrink; the steps below keep the worst case (names at their maximum,
/// `Fetch`/`Pull`/`Push`/`Stash`/`History`/`Changes` labelled, the worktree
/// button with its warning dot) inside the bar.
///
/// The labels are dropped from 1460 down: at 1200 a long repository name plus
/// the segmented labels do not fit even with the search at its minimum, which
/// clipped "Changes". The third view tab ("Worktrees") and the worktree
/// switcher button widen the bar, so the steps start higher than with the two
/// switchers alone. Below 740 the worktree button is dropped: Ctrl Alt W still
/// opens the switcher, under the branch switcher then.
///
/// The two labelled steps (1580 and 1460) were tuned to the English labels.
/// `label_extra` is how much wider the labels are in the active language
/// ([`label_extra_width`], 0 for English); both thresholds move up by it, so a
/// longer translation drops its labels earlier instead of clipping them.
pub fn toolbar_layout(width: f32, label_extra: f32) -> ToolbarLayout {
    let full = ToolbarLayout::full();
    if width >= 1580. + label_extra {
        // The design's own 140px cap on the repository and branch names.
        ToolbarLayout {
            repo_name: Some(140.),
            branch_name: Some(140.),
            ..full
        }
    } else if width >= 1460. + label_extra {
        ToolbarLayout {
            worktree_name: Some(64.),
            ..full
        }
    } else if width >= 1080. {
        // Icon-only tools and view switch, so the bar fits at 1200.
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            worktree_name: Some(64.),
            ..full
        }
    } else if width >= 990. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            worktree_name: Some(64.),
            search_basis: 180.,
            search_min: 120.,
            ..full
        }
    } else if width >= 970. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            worktree_name: Some(64.),
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
    } else if width >= 900. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            worktree_name: None,
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
    } else if width >= 810. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: Some(64.),
            worktree_name: None,
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
        .compact()
    } else if width >= 740. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: None,
            worktree_name: None,
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
            worktree_button: false,
            worktree_name: None,
            search_basis: 60.,
            search_min: 60.,
            padding: 6.,
            ..full
        }
        .compact()
        .tighter()
    }
}

/// Catalog keys of the labels the toolbar shows while `ToolbarLayout::labels` is on:
/// the four tool buttons and the three view tabs.
const TOOLBAR_LABELS: [&str; 7] = [
    "repo.toolbar.fetch",
    "repo.toolbar.pull",
    "repo.toolbar.push",
    "repo.toolbar.stash",
    "repo.toolbar.view_history",
    "repo.toolbar.view_changes",
    "repo.toolbar.view_worktrees",
];

/// How much wider the toolbar labels are in `lang` than in English, given a text
/// `measure`; never negative, so English (and any shorter language) keeps the
/// English steps.
pub fn label_extra_width(lang: &str, measure: impl Fn(&str) -> f32) -> f32 {
    let total = |lang: &str| -> f32 {
        TOOLBAR_LABELS
            .iter()
            .map(|key| measure(&ui::i18n::t_in(lang, key)))
            .sum()
    };
    (total(lang) - total("en")).max(0.)
}

/// [`label_extra_width`] for the active language, measured with the window's text
/// system at the toolbar font.
fn toolbar_label_extra(window: &Window) -> f32 {
    // Language and fonts are fixed for the process, so measure once; English needs no
    // measuring (the extra is 0 by definition).
    static EXTRA: OnceLock<f32> = OnceLock::new();
    if ui::i18n::language() == "en" {
        return 0.;
    }
    *EXTRA.get_or_init(|| measure_label_extra(window))
}

fn measure_label_extra(window: &Window) -> f32 {
    let font = ui::fonts::ui_font();
    label_extra_width(ui::i18n::language(), |text| {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window
                .text_system()
                .shape_line(SharedString::from(text.to_owned()), FS_BASE, &[run], None)
                .width,
        )
    })
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

/// The tallest a bottom-docked details panel may render: half of the window
/// height (the same share the right dock gets of the width), within the panel's
/// own 220–560 range, so History keeps its rows in a short window.
pub fn details_height_limit(window_height: f32) -> f32 {
    let (min, max) = dock_bounds(Dock::Bottom);
    (DETAILS_MAX_SHARE * window_height).clamp(min, max)
}

/// The height a bottom-docked details panel renders at: its stored size,
/// capped by [`details_height_limit`] and never below its 220 px minimum.
///
/// The stored size is not rewritten; only the rendered height shrinks.
pub fn effective_details_height(stored: f32, window_height: f32) -> f32 {
    clamp_details(stored, Dock::Bottom).min(details_height_limit(window_height))
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
    /// Visible rows reach the fetcher through an event, so reading the history
    /// in the handler is safe (the emitting update has finished).
    _history_events: Subscription,
    /// The commit heatmap above the history table.
    activity: Entity<ActivityStrip>,
    changes: Entity<ChangesView>,
    _changes_observer: Subscription,
    /// Redraws the sidebar's change counters when the status reloads.
    _changes_redraw: Subscription,
    worktrees: Entity<WorktreesView>,
    /// The worktree list shared by the Worktrees page, the sidebar panel and,
    /// next, the titlebar switcher.
    worktrees_model: Entity<WorktreesModel>,
    _worktrees_observer: Subscription,
    /// Redraws the sidebar's worktree panel when the model changes.
    _worktrees_redraw: Subscription,
    _worktrees_leave: Subscription,
    /// The worktree panel's keyboard focus and list scroll.
    worktree_panel_focus: FocusHandle,
    worktree_panel_scroll: ScrollHandle,
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
    /// The current-branch node the sidebar has already scrolled into view, so it
    /// reveals the checked-out branch when that changes but never re-applies the
    /// scroll on every frame (which would fight a manual scroll or a just-
    /// expanded folder).
    sidebar_revealed: Option<SharedString>,
    /// The sidebar's expansion overrides over the defaults.
    sidebar: Expansion,
    /// The sidebar filter field's text.
    filter: String,
    /// The sidebar's own lists; `None` while they load.
    sidebar_extras: Option<SidebarExtras>,
    /// The remote-account picker or Add account dialog opened from a remote's
    /// menu, kept alive while it is open.
    account_modal: Option<gpui::AnyEntity>,
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
    /// Window bounds of the worktree switcher button.
    worktree_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The worktree switcher popover while it is open; the button is pressed
    /// then.
    worktree_switcher: Option<worktree_switcher::Handle>,
    _worktree_switcher_release: Option<Subscription>,
    /// The stage 7 dialog currently open, if any.
    ref_dialog: Option<Entity<RefDialog>>,
    /// The Merge dialog while it is open (kept so its state outlives `open`).
    merge_dialog: Option<Entity<crate::merge_dialog::MergeDialog>>,
    /// The "Create pull request" dialog while it is open.
    pr_dialog: Option<Entity<CreatePrDialog>>,
    /// The loaded pull requests of the sidebar's panel.
    pr_model: Entity<PrModel>,
    _pr_events: Subscription,
    _pr_redraw: Subscription,
    /// Repository tree or pull request panel.
    sidebar_tab: SidebarTab,
    /// A ref operation without a dialog (checkout, stash apply, …) is running.
    ref_action_running: bool,
    /// The network action that is running, if any.
    network: Option<NetworkActivity>,
    /// Bumped per network job; an older poll loop stops.
    network_generation: u64,
    /// When the last fetch (background, or a Fetch/Pull the user started)
    /// began; activation fetches only once the interval has passed.
    last_fetch: Option<Instant>,
    /// The repeating background-fetch task.
    background_fetch_task: Option<Task<()>>,
    /// A background fetch is in flight; the next tick skips while it runs.
    background_fetch_running: bool,
    /// What the repository looked like when the views were last known to be
    /// current; `None` until the first read. See [`Fingerprint`].
    fingerprint: Option<Fingerprint>,
    /// Bumped by every re-baseline, so only the newest one is applied.
    fingerprint_generation: u64,
    /// A re-baseline (the app's own change was just reloaded) is being read. A check
    /// that starts meanwhile would take that change for an outside one, so it waits.
    baseline_in_flight: bool,
    /// Bumped by every check, so only the newest result is applied. Separate from
    /// `fingerprint_generation`: a re-baseline must not discard a check, nor the reverse.
    check_generation: u64,
    /// A check was asked for while a re-baseline was in flight; it runs when that ends.
    check_wanted: bool,
    /// When Changes was last re-read quietly on window activation.
    last_quiet_changes: Option<Instant>,
    /// The running file watcher; `None` until started (and in tests).
    watcher: Option<RepoWatcher>,
    /// The root the watcher was started for, so it is rebuilt when the root changes.
    watch_root: Option<PathBuf>,
    /// The watcher was rebuilt in [`WatchMode::Reduced`] after the OS watch limit was hit.
    watch_reduced: bool,
    /// Bumped per watcher start; a start that finishes later is dropped.
    watch_generation: u64,
    /// Drains the watcher's channel (replaced together with the watcher).
    watch_task: Option<Task<()>>,
    /// Change kinds reported but not yet applied (merged, or held while an operation runs).
    watch_pending: Kinds,
    /// The pending flush after [`WATCH_FLUSH`].
    watch_flush: Option<Task<()>>,
    /// When Changes was last re-read because work-tree files changed.
    last_watch_changes: Option<Instant>,
    /// The trailing quiet Changes refresh of a throttled burst of file changes.
    watch_changes_trailing: Option<Task<()>>,
    /// Pull request badges and CI status of the primary remote's repository.
    hosting: HostingState,
    /// What the rows were last given, to skip a redraw that changes nothing.
    hosting_view: Arc<HostingView>,
    /// The commits the history currently shows, as its last report said.
    ci_visible: Vec<Oid>,
    /// The commits a running checks batch still wants, shared with it: a row
    /// that scrolled away before its chunk starts is skipped.
    ci_wanted: Arc<Mutex<HashSet<String>>>,
    /// Whether the CI dots, the checks block and check requests are wanted;
    /// mirrored from the settings and kept current while the view renders.
    show_ci_status: bool,
    /// The `meta_version` of the history the tracking was last read at.
    hosting_meta_version: u64,
    /// What the next sidebar reload means for the pull request map: the
    /// first load is the repository opening, later ones only refresh when the
    /// map is stale, unless a fetch or the user's refresh says otherwise.
    hosting_trigger: PrTrigger,
    /// The open pull request popover, to update its review line.
    pr_popover: Option<Entity<PrPopover>>,
    /// Refreshes the pull request map when the window gets focus again.
    _window_activation: Subscription,
    /// The selected commit's checks request waits for the selection to settle;
    /// replacing the task cancels the earlier wait.
    ci_debounce: Option<Task<()>>,
    /// Asks again every 30 s while commits on screen have pending checks and
    /// the window is active.
    ci_timer: Option<Task<()>>,
}

/// What the status bar and the toolbar show while git is talking to a remote.
struct NetworkActivity {
    verb: SharedString,
    detail: SharedString,
    progress: Option<f32>,
}

/// One event from the worker to the UI.
/// What a job's result toasts need, known when it starts.
struct JobMeta {
    /// The accounts the job answered prompts with.
    used: Vec<remote_accounts::UsedAccount>,
    /// The remote's remembered account choice, for naming push targets.
    remembered: Option<String>,
    /// The branch or tag a push result names.
    subject: String,
}

/// What a background git network call needs besides the repository: the git
/// environment with the accounts' askpass, the guard that keeps them
/// registered and the SSH host-key asker.
struct JobEnv {
    context: git_core::network::NetworkContext,
    guard: Option<crate::network_service::JobGuard>,
    asker: crate::network_service::HostKeyAsker,
}

enum NetworkEvent {
    Progress(Progress),
    Done(Result<git_core::network::NetworkOutcome, Error>),
}

/// What the poll loop needs about the job it watches: the intent belongs to
/// this job instance and dies with it.
#[derive(Clone, Copy)]
struct NetworkRun<'a> {
    generation: u64,
    job: &'a Job,
    then_push: bool,
}

/// The first background fetch waits a little (for the history to load), so a
/// repository opened after a while away shows remote changes soon.
const BACKGROUND_FETCH_FIRST: Duration = Duration::from_secs(3);
/// Window activation fetches at most this often.
const ACTIVATION_FETCH_MIN: Duration = Duration::from_secs(60);
/// How often the UI drains the watcher's channel. Well under the 1 s Changes throttle, and the
/// channel is a std `mpsc` polled from a timer (as the network job does) because no async
/// channel dependency is approved. 250 ms keeps an idle window's wake-ups low.
const WATCH_POLL: Duration = Duration::from_millis(250);
/// Signals arriving within this window are merged into one reload.
const WATCH_FLUSH: Duration = Duration::from_millis(200);
/// Work-tree file changes re-read Changes at most this often.
const WATCH_CHANGES_MIN: Duration = Duration::from_secs(1);
/// How often the UI drains the job's progress channel.
const NETWORK_POLL: Duration = Duration::from_millis(80);

/// Whether the settings ask for CI status. Read fresh, so a toggle takes
/// effect without waiting for the view to render.
fn ci_status_enabled(cx: &App) -> bool {
    cx.try_global::<crate::AppSettings>()
        .is_none_or(|state| state.settings.show_ci_status)
}

/// Whether the settings ask for the background fetch. Read fresh, so a toggle
/// in Settings applies to windows that are already open.
fn background_fetch_enabled(cx: &App) -> bool {
    cx.try_global::<crate::AppSettings>()
        .is_none_or(|state| state.settings.background_fetch)
}

/// Pause between background fetches, read fresh from the settings.
fn background_fetch_interval(cx: &App) -> Duration {
    let minutes = cx.try_global::<crate::AppSettings>().map_or(
        crate::settings::Settings::default().fetch_interval_minutes,
        |state| state.settings.fetch_interval_minutes,
    );
    Duration::from_secs(u64::from(minutes) * 60)
}

/// What a requested external-change check does right now.
#[derive(Debug, PartialEq, Eq)]
enum CheckStart {
    /// The app is itself changing refs or talking to a remote: it re-baselines when done.
    Skip,
    /// A re-baseline is being read: wait for it and compare against its result.
    Defer,
    Run,
}

fn check_start(app_busy: bool, baseline_in_flight: bool) -> CheckStart {
    if app_busy {
        CheckStart::Skip
    } else if baseline_in_flight {
        CheckStart::Defer
    } else {
        CheckStart::Run
    }
}

/// What to do with a finished check, given the generations of checks and re-baselines when
/// it started and now.
#[derive(Debug, PartialEq, Eq)]
enum CheckOutcome {
    /// A newer check superseded it.
    Discard,
    /// A re-baseline began meanwhile: this read may hold the app's own change, so ask again
    /// once the new baseline is recorded.
    Rerun,
    Apply,
}

fn check_outcome(
    check_started: u64,
    check_now: u64,
    baseline_started: u64,
    baseline_now: u64,
) -> CheckOutcome {
    if check_started != check_now {
        CheckOutcome::Discard
    } else if baseline_started != baseline_now {
        CheckOutcome::Rerun
    } else {
        CheckOutcome::Apply
    }
}

/// Whether something throttled is due: it has not run yet, or its last run began `min` ago or
/// more. Used for fetches and for the quiet Changes refreshes.
fn fetch_due(last: Option<Instant>, now: Instant, min: Duration) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= min)
}

/// Whether a finished network job is followed by a push: only the pull that
/// "Pull, then push" started, never another job or a later pull.
fn follow_with_push(job: &Job, then_push: bool) -> bool {
    then_push && matches!(job, Job::Pull)
}

/// Which failures open the error dialog, and whether it offers "Pull, then
/// push". Only a rejected push of the *checked-out branch* can be fixed that
/// way (the pull updates HEAD's branch): a rejected tag or remote-branch
/// delete has no branch to pull, and nor has a push of another branch.
fn error_dialog_remedy(known: &KnownError, job: &Job, head: Option<&Head>) -> Option<bool> {
    match known {
        KnownError::PushRejected => Some(match job {
            Job::Push { branch } => branch.as_deref().is_none_or(
                |branch| matches!(head, Some(Head::Branch { name, .. }) if name == branch),
            ),
            _ => false,
        }),
        KnownError::PullNotFastForward if matches!(job, Job::Pull) => Some(false),
        _ => None,
    }
}

/// The stash message a switch saves the work tree under, so a failed restore
/// can name the stash that still holds the changes.
const SWITCH_STASH: &str = "GitWarp: stashed before switching";

/// Runs `checkout` after stashing the work tree. A failed checkout pops the
/// stash again, restoring the staged/unstaged split where git can so the
/// user's changes are not hidden; if that pop fails too, the error says the
/// changes are still in the stash, by its message.
fn stash_then_checkout<F>(
    repo: &git_core::Repo,
    checkout: F,
) -> git_core::Result<(ui::ToastKind, String)>
where
    F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)>,
{
    if !repo.stash_save(Some(SWITCH_STASH))? {
        return checkout(repo);
    }
    match checkout(repo) {
        Ok(done) => Ok(done),
        Err(error) => match repo.stash_pop_keep_index(0) {
            Ok(()) => Err(error),
            Err(restore) => Err(Error::Git {
                command: "stash pop",
                code: None,
                stderr: format!(
                    "{error}\nThe switch failed and the changes could not be restored; they are \
                     still in the stash \"{SWITCH_STASH}\": {restore}"
                ),
            }),
        },
    }
}

/// What a finished ref operation changed in the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefChange {
    /// HEAD or the refs moved, but the commits reachable from `--all` are the
    /// same: re-read HEAD and the refs, not the log.
    RefsOnly,
    /// Commits may have appeared or disappeared: walk the history again.
    History,
}

/// How much a ref dialog's operation can change. Checkout-like operations —
/// switching branches, detaching, creating a branch or tag at an existing
/// commit, renaming a branch — only move refs. Everything that can add or
/// remove commits (reset, delete, cherry-pick, revert, remote edits, stash
/// drops) keeps the full reload. Renaming a tag is left on the full reload
/// because tags can be the only ref that keeps a commit alive; the walk is
/// cheap enough there.
fn ref_change_for_dialog(kind: &RefDialogKind) -> RefChange {
    match kind {
        RefDialogKind::CreateBranch { .. }
        | RefDialogKind::CreateTag { .. }
        | RefDialogKind::RenameBranch { .. } => RefChange::RefsOnly,
        RefDialogKind::RenameTag { .. }
        | RefDialogKind::Remote { .. }
        | RefDialogKind::Reset { .. }
        | RefDialogKind::DeleteBranch { .. }
        | RefDialogKind::DeleteTag { .. }
        | RefDialogKind::RemoveRemote { .. }
        | RefDialogKind::DropStash { .. }
        | RefDialogKind::CherryPick { .. }
        | RefDialogKind::Revert { .. }
        | RefDialogKind::RenameWorktree { .. } => RefChange::History,
    }
}

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
        // The history is owned by this view, so its callbacks hold the view weakly: a
        // strong handle would be a cycle and keep the view (with its watcher and fetch
        // loop) alive after the window moved on to another repository or the welcome screen.
        let action_view = cx.weak_entity();
        let on_history_action = Rc::new(
            move |action: RefAction, window: &mut Window, cx: &mut App| {
                action_view
                    .update(cx, |view, cx| {
                        view.handle_history_action(action, window, cx);
                    })
                    .ok();
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
        let sidebar_view = cx.weak_entity();
        history.update(cx, |history, _| {
            history.set_sidebar_action_handler(Rc::new(move |action, window, cx| {
                sidebar_view
                    .update(cx, |view, cx| view.run_sidebar_action(action, window, cx))
                    .ok();
            }));
        });
        let pr_view = cx.weak_entity();
        history.update(cx, |history, _| {
            history.set_pr_click_handler(Rc::new(move |branch, position, window, cx| {
                pr_view
                    .update(cx, |view, cx| {
                        view.open_pr_popover(&branch, position, window, cx)
                    })
                    .ok();
            }));
        });
        let service_view = cx.weak_entity();
        history.update(cx, |history, _| {
            // The History asks while it is itself being updated, so it hands
            // over its own HEAD and refs instead of being read back.
            history.set_pr_service(Rc::new(move |branch, head, refs, cx| {
                service_view
                    .read_with(cx, |view, cx| {
                        let targets = view.pr_targets_in(head, refs, cx);
                        let upstream = view.branch_upstream_remote_in(branch, refs);
                        pr_logic::default_target(&targets, upstream.as_deref())
                            .map(|target| target.kind())
                    })
                    .ok()
                    .flatten()
            }));
        });
        let history_events =
            cx.subscribe(&history, |this, _, event: &HistoryEvent, cx| match event {
                HistoryEvent::VisibleRowsChanged(oids) => this.set_visible_ci(oids.clone(), cx),
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
                this.ensure_watcher(root, cx);
            }
            // HEAD and the refs were read again: the branches that track the
            // primary remote may have changed.
            let version = history.read(cx).meta_version();
            if version != this.hosting_meta_version {
                this.hosting_meta_version = version;
                this.hosting_sync(PrTrigger::Focus, cx);
            }
            this.sync_details(selected, cx);
            // The branch filter clears the history's search from inside the
            // list (chip, row menu); the field must not keep the dead query.
            if history.read(cx).search_query().is_empty() && !this.search.read(cx).text().is_empty()
            {
                this.search.update(cx, |input, cx| input.set_text("", cx));
            }
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
                    this.rebaseline_fingerprint(cx);
                }
                ChangesEvent::PushAfterCommit => this.push_branch(window, cx),
                // `merge --abort` does not move HEAD: the sidebar, the worktree
                // status and the badges are enough, not a history walk.
                // `abort_merge` already applied a fresh status, so Changes is not re-read.
                ChangesEvent::MergeAborted => {
                    this.after_ref_change_with(RefChange::RefsOnly, false, cx)
                }
            },
        );
        let changes_redraw = cx.observe(&changes, |this, changes, cx| {
            // The History's menus need the operation the Changes view read.
            let operation = changes.read(cx).operation();
            this.history
                .update(cx, |history, _| history.set_operation(operation));
            // Changes finishing its own work releases held watcher signals.
            this.retry_watch_flush(cx);
            cx.notify();
        });
        let worktrees_model =
            cx.new(|cx| WorktreesModel::new(path.clone(), git.clone(), workspace.clone(), cx));
        let worktrees = cx.new(|cx| WorktreesView::new(worktrees_model.clone(), window, cx));
        let worktrees_observer = cx.subscribe(
            &worktrees_model,
            |this, _, event: &WorktreesModelEvent, cx| {
                if *event == WorktreesModelEvent::Changed {
                    // A worktree was added, removed or moved: its HEAD is a
                    // ref already covered by `--all`, so the set of commits
                    // cannot change. Re-read HEAD and the refs so the sidebar,
                    // the history's badges and the local changes follow.
                    this.history
                        .update(cx, |history, cx| history.refresh_refs(cx));
                    this.changes.update(cx, |changes, cx| changes.refresh(cx));
                    this.refresh_with(hosting_status::INTERNAL_RELOAD, cx);
                    cx.notify();
                }
            },
        );
        let pr_model = cx.new(|_| PrModel::new());
        let pr_events =
            cx.subscribe_in(&pr_model, window, |this, _, event: &PrEvent, window, cx| {
                this.pr_event(event.clone(), window, cx)
            });
        // The panel redraws when a list arrives.
        let pr_redraw = cx.observe(&pr_model, |_, _, cx| cx.notify());
        let worktrees_redraw = cx.observe(&worktrees_model, |_, _, cx| cx.notify());
        let worktrees_leave = cx.subscribe_in(
            &worktrees,
            window,
            |this, _, _: &WorktreesEvent, window, cx| {
                // Esc on the Worktrees page goes back to History.
                this.set_view(ViewMode::History, window, cx);
            },
        );
        let search = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("repo.search.placeholder"), cx);
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
            input.set_placeholder(ui::t!("repo.sidebar.filter_refs"), cx);
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
        let window_activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.on_window_activated(window, cx);
            }
        });

        let mut view = Self {
            path,
            root,
            repo_name: name,
            git_label,
            git_tooltip,
            history,
            _history_observer: observer,
            _history_events: history_events,
            activity,
            changes,
            _changes_observer: changes_observer,
            _changes_redraw: changes_redraw,
            worktrees,
            worktrees_model,
            _worktrees_observer: worktrees_observer,
            _worktrees_redraw: worktrees_redraw,
            _worktrees_leave: worktrees_leave,
            worktree_panel_focus: cx.focus_handle().tab_stop(true),
            worktree_panel_scroll: ScrollHandle::new(),
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
            sidebar_revealed: None,
            sidebar: Expansion::default(),
            filter: String::new(),
            sidebar_extras: None,
            account_modal: None,
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
            worktree_bounds: Rc::new(Cell::new(None)),
            worktree_switcher: None,
            _worktree_switcher_release: None,
            ref_dialog: None,
            merge_dialog: None,
            pr_dialog: None,
            pr_model,
            _pr_events: pr_events,
            _pr_redraw: pr_redraw,
            sidebar_tab: SidebarTab::Repository,
            ref_action_running: false,
            network: None,
            network_generation: 0,
            last_fetch: None,
            background_fetch_task: None,
            background_fetch_running: false,
            fingerprint: None,
            fingerprint_generation: 0,
            baseline_in_flight: false,
            check_generation: 0,
            check_wanted: false,
            last_quiet_changes: None,
            watcher: None,
            watch_root: None,
            watch_reduced: false,
            watch_generation: 0,
            watch_task: None,
            watch_pending: Kinds::NONE,
            watch_flush: None,
            last_watch_changes: None,
            watch_changes_trailing: None,
            hosting: HostingState::default(),
            hosting_view: Arc::default(),
            ci_visible: Vec::new(),
            ci_wanted: Arc::default(),
            show_ci_status: settings.show_ci_status,
            hosting_meta_version: 0,
            hosting_trigger: PrTrigger::Open,
            pr_popover: None,
            _window_activation: window_activation,
            ci_debounce: None,
            ci_timer: None,
        };
        view.start_background_fetch(cx);
        view.rebaseline_fingerprint(cx);
        view
    }

    /// The window became active: besides the hosting and CI state, pick up
    /// whatever changed in the repository while the user was in a terminal or
    /// another tool. The file watcher normally reports those changes already;
    /// this focus check is the fallback for where it cannot (watch limit, a
    /// network file system, a failed start).
    fn on_window_activated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hosting_sync(PrTrigger::Focus, cx);
        self.ensure_ci_timer(window, cx);
        self.check_external_changes(cx);
        self.fetch_on_activation(cx);
        let now = Instant::now();
        // A busy Changes view re-reads the status itself; don't spend the interval on it.
        if !self.changes.read(cx).is_busy()
            && fetch_due(self.last_quiet_changes, now, QUIET_CHANGES_MIN)
        {
            self.last_quiet_changes = Some(now);
            self.changes
                .update(cx, |changes, cx| changes.refresh_quiet(cx));
        }
    }

    /// Fetches on activation when the background fetch is on and the last
    /// fetch is at least [`ACTIVATION_FETCH_MIN`] old. Returns whether a fetch
    /// tick was attempted (the tick itself still skips while a job runs).
    fn fetch_on_activation(&mut self, cx: &mut Context<Self>) -> bool {
        if !background_fetch_enabled(cx)
            || !fetch_due(self.last_fetch, Instant::now(), ACTIVATION_FETCH_MIN)
        {
            return false;
        }
        self.background_fetch_tick(cx);
        true
    }

    /// Reads the repository's fingerprint off the UI thread and reloads the
    /// parts [`decide_reload`] names when it differs from the last known one.
    /// Skipped while the app itself is changing refs or talking to a remote:
    /// those paths re-baseline when they finish. While such a re-baseline is
    /// still being read the check waits for it, so the app's own change is not
    /// mistaken for an outside one.
    fn check_external_changes(&mut self, cx: &mut Context<Self>) {
        let app_busy =
            self.ref_action_running || self.network.is_some() || self.background_fetch_running;
        match check_start(app_busy, self.baseline_in_flight) {
            CheckStart::Skip => return,
            CheckStart::Defer => {
                self.check_wanted = true;
                return;
            }
            CheckStart::Run => {}
        }
        let Ok(git) = self.git.clone() else {
            return;
        };
        self.check_generation = self.check_generation.wrapping_add(1);
        let generation = self.check_generation;
        let baseline_generation = self.fingerprint_generation;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let new = cx
                .background_executor()
                .spawn(async move { read_fingerprint(git, &root) })
                .await;
            let Some(new) = new else {
                return;
            };
            this.update(cx, |view, cx| {
                match check_outcome(
                    generation,
                    view.check_generation,
                    baseline_generation,
                    view.fingerprint_generation,
                ) {
                    CheckOutcome::Discard => return,
                    CheckOutcome::Rerun => {
                        // A re-baseline began while this was read: the baseline it will
                        // record is the one to compare with, so ask again after it.
                        view.check_wanted = true;
                        if !view.baseline_in_flight {
                            view.check_wanted = false;
                            view.check_external_changes(cx);
                        }
                        return;
                    }
                    CheckOutcome::Apply => {}
                }
                let decision = decide_reload(view.fingerprint.as_ref(), &new);
                view.fingerprint = Some(new);
                if let Some(change) = decision.refs {
                    // The Changes status is re-read quietly below, not through
                    // the `Loading` state the in-app paths use.
                    view.after_ref_change_with(change, false, cx);
                }
                if decision.changes {
                    view.changes
                        .update(cx, |changes, cx| changes.refresh_quiet(cx));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts the file watcher for `root` once per root (never in tests: the
    /// wiring is tested by calling [`Self::on_watch_signal`] directly).
    fn ensure_watcher(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if cfg!(test) || self.watch_root.as_ref() == Some(&root) {
            return;
        }
        self.watch_root = Some(root.clone());
        self.watch_reduced = false;
        self.start_watcher(root, WatchMode::Normal, cx);
    }

    /// Replaces the watcher and its channel. Starting walks directories, so it
    /// runs on the background executor; a start that is superseded meanwhile is
    /// dropped when it finishes.
    fn start_watcher(&mut self, root: PathBuf, mode: WatchMode, cx: &mut Context<Self>) {
        let Ok(git) = self.git.clone() else {
            return;
        };
        self.watcher = None;
        self.watch_generation = self.watch_generation.wrapping_add(1);
        let generation = self.watch_generation;
        let (tx, rx) = mpsc::channel::<WatchSignal>();
        let started = cx.background_executor().spawn(async move {
            let repo =
                Repo::open(git.clone(), &root).map_err(|e| StartError::Other(e.to_string()))?;
            // git-core has no logger of its own: route its recoverable-problem warnings here.
            git_core::set_warning_sink(|message| log::warn!("{message}"));
            // Scratch index copies a crashed run left behind (we are off the UI thread).
            let stale = repo.remove_stale_scratch_files();
            if stale > 0 {
                log::info!("removed {stale} stale scratch index file(s)");
            }
            let common =
                git_core::common_dir(&repo).map_err(|e| StartError::Other(e.to_string()))?;
            let layout = GitLayout::new(Some(repo.root()), repo.git_dir(), &common);
            let start = |mode| {
                let tx = tx.clone();
                RepoWatcher::start_in(&layout, mode, git.path(), move |signal| {
                    let _ = tx.send(signal);
                })
            };
            match start(mode) {
                // Out of OS watches: the work tree is left to the activation check.
                Err(StartError::Limit) if mode == WatchMode::Normal => {
                    log::warn!("file watcher hit the OS watch limit, watching git folders only");
                    start(WatchMode::Reduced)
                }
                other => other,
            }
        });
        cx.spawn(async move |this, cx| {
            let result = started.await;
            this.update(cx, |view, _| {
                if generation != view.watch_generation {
                    return;
                }
                match result {
                    Ok(watcher) => {
                        log::info!("file watcher started ({:?} mode)", watcher.mode());
                        view.watch_reduced = watcher.mode() == WatchMode::Reduced;
                        view.watcher = Some(watcher);
                    }
                    Err(error) => {
                        log::warn!("file watcher not started, relying on window focus: {error}");
                    }
                }
            })
            .ok();
        })
        .detach();
        self.watch_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(WATCH_POLL).await;
            let mut signals = Vec::new();
            // Every sender lives in the watcher's sink: once it is gone (the start failed or
            // the watcher was dropped) nothing can arrive any more, so the loop ends.
            let senders_gone = loop {
                match rx.try_recv() {
                    Ok(signal) => signals.push(signal),
                    Err(mpsc::TryRecvError::Empty) => break false,
                    Err(mpsc::TryRecvError::Disconnected) => break true,
                }
            };
            let alive = this.update(cx, |view, cx| {
                for signal in signals {
                    view.on_watch_signal(signal, cx);
                }
            });
            if alive.is_err() || senders_gone {
                break;
            }
        }));
    }

    /// Merges a watcher signal into the pending kinds and schedules the flush;
    /// a reported watch-limit overflow rebuilds the watcher in reduced mode.
    fn on_watch_signal(&mut self, signal: WatchSignal, cx: &mut Context<Self>) {
        self.watch_pending |= signal.kinds;
        if signal.limit_hit && !self.watch_reduced {
            log::warn!("file watcher hit the OS watch limit, rebuilding in git-folders-only mode");
            self.watch_reduced = true;
            if let Some(root) = self.watch_root.clone() {
                self.start_watcher(root, WatchMode::Reduced, cx);
            }
        }
        self.retry_watch_flush(cx);
    }

    /// Schedules a flush of the pending kinds after [`WATCH_FLUSH`], unless one
    /// is already scheduled or nothing is pending. Also called when an
    /// operation that held the signals finishes.
    fn retry_watch_flush(&mut self, cx: &mut Context<Self>) {
        if self.watch_pending.is_empty() || self.watch_flush.is_some() {
            return;
        }
        self.watch_flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WATCH_FLUSH).await;
            this.update(cx, |view, cx| {
                // Let the task finish; dropping the handle would cancel it midway.
                if let Some(task) = view.watch_flush.take() {
                    task.detach();
                }
                view.flush_watch(cx);
            })
            .ok();
        }));
    }

    /// Whether the app itself is changing the repository or the status.
    fn operation_running(&self, cx: &App) -> bool {
        self.ref_action_running
            || self.network.is_some()
            || self.background_fetch_running
            || self.changes.read(cx).is_busy()
    }

    /// Applies the pending kinds. Held (kept pending) while an operation runs:
    /// the code paths that finish one call [`Self::retry_watch_flush`].
    fn flush_watch(&mut self, cx: &mut Context<Self>) {
        if self.operation_running(cx) {
            return;
        }
        let kinds = std::mem::take(&mut self.watch_pending);
        if kinds.is_empty() {
            return;
        }
        if kinds.contains(Kinds::RESCAN) {
            // The backend lost events: everything may have changed.
            self.after_ref_change(RefChange::History, cx);
            return;
        }
        if kinds.intersects(Kinds::STASH | Kinds::CONFIG) {
            self.refresh_with(hosting_status::INTERNAL_RELOAD, cx);
        }
        if kinds.intersects(Kinds::WORKTREES) {
            self.worktrees
                .update(cx, |worktrees, cx| worktrees.refresh(cx));
        }
        if kinds.intersects(Kinds::HEAD | Kinds::REFS | Kinds::OPERATION) {
            // The fingerprint says whether anything the views show really changed.
            self.check_external_changes(cx);
        }
        if kinds.intersects(Kinds::INDEX) {
            // Straight to Changes: the fingerprint's coarse (mtime, length) index stamp must
            // not veto a staging change (it stays the focus fallback's tool). The refresh
            // drops a status equal to the shown one, so the app's own staging costs a read.
            self.changes
                .update(cx, |changes, cx| changes.refresh_quiet(cx));
        }
        if kinds.intersects(Kinds::WORKTREE_FILES) {
            self.refresh_changes_for_files(cx);
        }
    }

    /// Quiet Changes refresh for work-tree file changes: at most once per
    /// [`WATCH_CHANGES_MIN`], with one trailing refresh for a throttled burst.
    fn refresh_changes_for_files(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if fetch_due(self.last_watch_changes, now, WATCH_CHANGES_MIN) {
            self.last_watch_changes = Some(now);
            self.changes
                .update(cx, |changes, cx| changes.refresh_quiet(cx));
            return;
        }
        if self.watch_changes_trailing.is_some() {
            return;
        }
        let wait = self.last_watch_changes.map_or(Duration::ZERO, |last| {
            WATCH_CHANGES_MIN.saturating_sub(now.duration_since(last))
        });
        self.watch_changes_trailing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |view, cx| {
                if let Some(task) = view.watch_changes_trailing.take() {
                    task.detach();
                }
                view.watch_pending |= Kinds::WORKTREE_FILES;
                view.retry_watch_flush(cx);
            })
            .ok();
        }));
    }

    /// Records the repository's current state as the one the views show. Called
    /// after the app's own reloads, so what it just did is not mistaken for an
    /// outside change at the next check.
    fn rebaseline_fingerprint(&mut self, cx: &mut Context<Self>) {
        let Ok(git) = self.git.clone() else {
            return;
        };
        self.fingerprint_generation = self.fingerprint_generation.wrapping_add(1);
        self.baseline_in_flight = true;
        let generation = self.fingerprint_generation;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let new = cx
                .background_executor()
                .spawn(async move { read_fingerprint(git, &root) })
                .await;
            this.update(cx, |view, cx| {
                if generation != view.fingerprint_generation {
                    return;
                }
                if new.is_some() {
                    view.fingerprint = new;
                }
                view.baseline_in_flight = false;
                if std::mem::take(&mut view.check_wanted) {
                    view.check_external_changes(cx);
                }
            })
            .ok();
        })
        .detach();
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

    /// The worktree list shared with the sidebar panel and the switcher.
    #[allow(dead_code)] // read by the switcher (W5)
    pub(crate) fn worktrees_model(&self) -> Entity<WorktreesModel> {
        self.worktrees_model.clone()
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
        let was_worktrees = self.view == ViewMode::Worktrees;
        self.view = mode;
        if was_worktrees != (mode == ViewMode::Worktrees) {
            self.sync_sidebar_filter(cx);
        }
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

    /// The sidebar filter belongs to whatever the sidebar lists: branches and
    /// tags, or (in the Worktrees view) worktrees. Switching between them
    /// starts a fresh, empty filter with the matching placeholder.
    fn sync_sidebar_filter(&mut self, cx: &mut Context<Self>) {
        let placeholder = if self.view == ViewMode::Worktrees {
            ui::t!("repo.sidebar.filter_worktrees")
        } else {
            ui::t!("repo.sidebar.filter_refs")
        };
        self.filter.clear();
        self.sidebar_filter.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            input.set_text("", cx);
        });
    }

    /// The worktree panel's rows, in the order they are drawn.
    fn worktree_panel_paths(&self, cx: &App) -> Vec<PathBuf> {
        worktree_panel_rows(self.worktrees_model.read(cx).entries(), &self.filter)
            .into_iter()
            .map(|entry| entry.path().to_path_buf())
            .collect()
    }

    /// Everything the worktree panel reports: clicks, keys, the menu.
    fn worktree_panel_event(
        &mut self,
        event: PanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.worktrees_model.clone();
        let selected = model.read(cx).selected().map(|e| e.path().to_path_buf());
        match event {
            PanelEvent::Select(path) => model.update(cx, |model, cx| model.select(&path, cx)),
            PanelEvent::Switch(path) => {
                model.update(cx, |model, cx| model.switch_to(&path, window, cx));
            }
            PanelEvent::Menu(path, position) => {
                model.update(cx, |model, cx| model.select(&path, cx));
                self.open_worktree_menu(&path, position, window, cx);
            }
            PanelEvent::Move(delta) => {
                let paths = self.worktree_panel_paths(cx);
                if paths.is_empty() {
                    return;
                }
                let at = selected.and_then(|path| paths.iter().position(|p| *p == path));
                let next = match at {
                    Some(at) => at.saturating_add_signed(delta).min(paths.len() - 1),
                    None if delta < 0 => paths.len() - 1,
                    None => 0,
                };
                self.worktree_panel_scroll.scroll_to_item(next);
                model.update(cx, |model, cx| model.select(&paths[next], cx));
            }
            PanelEvent::Confirm => {
                if let Some(path) = selected {
                    model.update(cx, |model, cx| model.switch_to(&path, window, cx));
                }
            }
            PanelEvent::Delete => {
                // The main worktree cannot be removed.
                let removable = selected.filter(|path| {
                    model
                        .read(cx)
                        .entry(path)
                        .is_some_and(|entry| !entry.is_main())
                });
                if let Some(path) = removable {
                    self.worktrees
                        .update(cx, |worktrees, cx| worktrees.open_remove(&path, window, cx));
                }
            }
            PanelEvent::Leave => self.set_view(ViewMode::History, window, cx),
            PanelEvent::NewWorktree => {
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.focus_quick(window, cx));
            }
        }
    }

    /// The right-click menu of a worktree row.
    fn open_worktree_menu(
        &mut self,
        path: &Path,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.worktrees_model.read(cx).entry(path).cloned() else {
            return;
        };
        let view = cx.entity();
        let model = self.worktrees_model.clone();
        let entries: Vec<MenuEntry> = worktree_menu(&entry)
            .into_iter()
            .map(|item| {
                let Some(item) = item else {
                    return MenuEntry::Separator;
                };
                let mut menu_item = MenuItem::new(item.label)
                    .icon(item.icon)
                    .disabled(item.disabled);
                if item.danger {
                    menu_item = menu_item.danger();
                }
                let (view, model, path) = (view.clone(), model.clone(), path.to_path_buf());
                MenuEntry::Item(menu_item.action(move |window, cx| {
                    run_worktree_action(item.action, &path, &model, &view, window, cx);
                }))
            })
            .collect();
        menu::open_context_menu(position, entries, window, cx);
    }

    /// The panel that replaces the sidebar tree in the Worktrees view.
    fn worktree_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.worktrees_model.read(cx);
        let state = if model.error().is_some() {
            PanelState::Error
        } else if !model.is_loaded() {
            PanelState::Loading
        } else {
            PanelState::Ready
        };
        let rows: Vec<WorktreeEntry> = worktree_panel_rows(model.entries(), &self.filter)
            .into_iter()
            .cloned()
            .collect();
        let total = model.entries().len();
        let selected = model.selected().map(|entry| entry.path().to_path_buf());
        let view = cx.entity();
        WorktreePanel::new(
            state,
            rows,
            total,
            self.filter.clone(),
            selected,
            self.worktree_panel_focus.clone(),
            self.worktree_panel_scroll.clone(),
            move |event, window, cx| {
                view.update(cx, |view, cx| view.worktree_panel_event(event, window, cx));
            },
        )
        .into_any_element()
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
    fn toggle_node(
        &mut self,
        key: &str,
        current: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // While filtering every expandable row is forced open. Recording a
        // flip here would silently change the state restored when the filter
        // is cleared, so expansion toggles are ignored until then.
        if !self.filter.trim().is_empty() {
            return;
        }
        self.sidebar.toggle(key, current);
        cx.notify();
        // The sidebar scrollbar reads the tree's scroll height a frame late: it
        // is built during render, before the tree lays out the new rows. Without
        // a second frame its vertical thumb keeps the old, shorter range and
        // cannot reach the rows a just-expanded folder added. Re-render once more
        // so it picks up the grown height.
        cx.on_next_frame(window, |_, _, cx| cx.notify());
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
                self.sync_sidebar_filter(cx);
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
                self.changes.read(cx).operation(),
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
        let links = self.host_links(cx);
        let operation = self.changes.read(cx).operation();
        let mut model = {
            let history = self.history.read(cx);
            sidebar_menu_with_links(
                key,
                history.head(),
                self.sidebar_extras.as_ref(),
                history.refs(),
                &links,
                operation,
            )
        };
        // "Create pull request…" for a local branch when a remote can take one.
        if let Some(branch) = key.strip_prefix("local:") {
            // The noun of the target the dialog will preselect for this branch.
            let targets = self.pr_targets(cx);
            let upstream = self.branch_upstream_remote(branch, cx);
            if let Some(target) = pr_logic::default_target(&targets, upstream.as_deref()) {
                crate::sidebar::with_create_pull_request(&mut model, branch, target.kind());
            }
        }
        if model.is_empty() {
            return;
        }
        self.selected_node = Some(key.into());
        let view = cx.entity();
        let handler: SidebarActionHandler = Rc::new(move |action, window, cx| {
            view.update(cx, |view, cx| view.run_sidebar_action(action, window, cx));
        });
        let entries: Vec<MenuEntry> = model
            .into_iter()
            .map(|entry| match entry {
                SidebarMenuEntry::Separator => MenuEntry::Separator,
                SidebarMenuEntry::Item(item) => {
                    MenuEntry::Item(item.into_menu_item(handler.clone()))
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
            SidebarAction::OpenUrl { url, host } => self.open_on_host(&url, &host, window, cx),
            SidebarAction::Refresh => {
                self.refresh(cx);
                self.worktrees
                    .update(cx, |worktrees, cx| worktrees.refresh(cx));
            }
            SidebarAction::CheckoutBranch(name) => {
                self.confirm_checkout_branch(name, window, cx);
            }
            SidebarAction::CreatePullRequest(branch) => {
                self.open_create_pr(Some(branch), window, cx);
            }
            SidebarAction::CheckoutRemoteBranch(remote) => {
                let label = remote.clone();
                self.confirm_checkout(
                    ui::t!("repo.checkout.title", name = label),
                    ui::t!("history.checkout_confirm.body_remote", name = label),
                    ui::t!("history.checkout_confirm.confirm_checkout"),
                    move |view, window, cx| {
                        view.checkout_guarded(
                            ui::t!("repo.checkout.title", name = label).to_string(),
                            move |repo| {
                                repo.checkout_remote(&remote)?;
                                Ok((
                                    ui::ToastKind::Ok,
                                    ui::t!("repo.checkout.done", name = label).to_string(),
                                ))
                            },
                            window,
                            cx,
                        );
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
                        ui::t!("repo.toast.tag_no_commit"),
                        window,
                        cx,
                    );
                    return;
                };
                let label = name.clone();
                self.confirm_checkout(
                    ui::t!("repo.checkout.tag_title", name = label),
                    ui::t!("history.checkout_confirm.body_tag", name = label),
                    ui::t!("history.checkout_confirm.confirm_checkout"),
                    move |view, window, cx| {
                        view.checkout_guarded(
                            ui::t!("repo.checkout.tag_title", name = label).to_string(),
                            move |repo| {
                                repo.checkout_detached(target)?;
                                Ok((
                                    ui::ToastKind::Ok,
                                    ui::t!("repo.checkout.tag_done", name = label).to_string(),
                                ))
                            },
                            window,
                            cx,
                        );
                    },
                    window,
                    cx,
                );
            }
            SidebarAction::Merge(branch) => self.open_merge_dialog(branch, window, cx),
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
                    Ok((
                        ui::ToastKind::Ok,
                        ui::t!(
                            "repo.toast.stash_applied",
                            stash = format!("stash@{{{index}}}")
                        )
                        .to_string(),
                    ))
                });
            }
            SidebarAction::PopStash(index) => {
                self.run_repo_action(window, cx, move |repo| {
                    repo.stash_pop(index)?;
                    Ok((
                        ui::ToastKind::Ok,
                        ui::t!(
                            "repo.toast.stash_popped",
                            stash = format!("stash@{{{index}}}")
                        )
                        .to_string(),
                    ))
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
                            ui::t!(
                                "repo.toast.stash_gone",
                                stash = format!("stash@{{{index}}}")
                            ),
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
                    ui::t!("repo.toast.opened_in_window", name = path),
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
                        ui::t!("repo.toast.commit_before_tag"),
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
            SidebarAction::RemoteAccount(name) => self.remote_account(name, window, cx),
            SidebarAction::AddAccountFor(name) => {
                let kind = self
                    .resolution_of(&name, cx)
                    .and_then(|r| match r.fetch.account {
                        remote_accounts::UrlAccount::NoAccount(kind) => Some(kind),
                        _ => None,
                    });
                if let Some(kind) = kind {
                    self.open_add_account_for(name, kind, window, cx);
                }
            }
            SidebarAction::RemotePushAccounts(_) => {
                toast::show(
                    ui::ToastKind::Info,
                    ui::t!("repo.toast.account_push_urls"),
                    window,
                    cx,
                );
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
            SidebarAction::PushBranch(branch) => {
                let branch = Some(branch);
                self.start_network_job(Job::Push { branch }, window, cx);
            }
            SidebarAction::Pull => self.pull_branch(window, cx),
            SidebarAction::CreateTagAt(oid) => {
                let card = self.card_for(oid, cx);
                self.open_ref_dialog(RefDialogKind::CreateTag { card }, window, cx);
            }
            SidebarAction::CopyCommitMessage(oid) => self.copy_commit_message(oid, window, cx),
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
                        ui::t!("repo.toast.unknown_remote", name = name),
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
                    ui::t!("repo.delete_remote_branch.title", name = name),
                    ui::t!(
                        "repo.delete_remote_branch.body",
                        branch = branch,
                        remote = remote
                    ),
                    ui::t!("repo.delete_remote_branch.confirm"),
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
                self.run_ref_action(window, cx, move |repo| {
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
                            stderr: ui::t!(
                                "repo.upstream.missing",
                                upstream = upstream,
                                branch = branch
                            )
                            .to_string(),
                        });
                    }
                    git_core::network::set_upstream(repo, &branch, &remote, &branch)?;
                    Ok((
                        ui::ToastKind::Ok,
                        ui::t!("repo.upstream.tracks", branch = branch, upstream = upstream)
                            .to_string(),
                    ))
                });
            }
            SidebarAction::ShowBranchCommits(name) => {
                self.show_branch_commits(name, window, cx);
            }
            SidebarAction::ShowAllCommits => {
                self.history
                    .update(cx, |history, cx| history.set_branch_filter(None, cx));
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
                    ui::t!("repo.toast.opened_in_window", name = label),
                    window,
                    cx,
                );
            }
            SidebarAction::LockWorktree(path) => {
                let path = PathBuf::from(path);
                self.run_repo_action(window, cx, move |repo| {
                    repo.lock_worktree(&path, Some("locked manually"))?;
                    Ok((
                        ui::ToastKind::Ok,
                        ui::t!("repo.toast.worktree_locked", path = path.display()).to_string(),
                    ))
                });
            }
            SidebarAction::UnlockWorktree(path) => {
                let path = PathBuf::from(path);
                self.run_repo_action(window, cx, move |repo| {
                    repo.unlock_worktree(&path)?;
                    Ok((
                        ui::ToastKind::Ok,
                        ui::t!("repo.toast.worktree_unlocked", path = path.display()).to_string(),
                    ))
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
                        ui::t!("repo.toast.worktrees_pruned").to_string(),
                    ))
                });
            }
        }
    }

    /// The hosts the remotes point at (see [`web_links::host_links`]), the
    /// current branch's upstream first. Pure: no network, no keychain.
    fn host_links(&self, cx: &App) -> Vec<HostLink> {
        let Some(extras) = self.sidebar_extras.as_ref() else {
            return Vec::new();
        };
        let (accounts, _) = self.account_settings(cx);
        let history = self.history.read(cx);
        let preferred = web_links::upstream_remote(
            history.head(),
            history.refs().unwrap_or(&[]),
            &extras.remote_targets,
        );
        web_links::host_links(
            &remote_accounts::account_refs(&accounts),
            &extras.remote_targets,
            preferred.as_deref(),
        )
    }

    /// Opens `url` in the browser and says which host it was.
    fn open_on_host(&mut self, url: &str, host: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(url);
        toast::show(
            ui::ToastKind::Info,
            ui::t!("repo.toast.opened_host", host = host),
            window,
            cx,
        );
    }

    /// Copies `url` to the clipboard and confirms with `message`.
    fn copy_link(
        &mut self,
        url: String,
        message: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::copy_with_toast(url, message, window, cx);
    }

    /// The configured accounts and this repository's remembered choices.
    fn account_settings(
        &self,
        cx: &App,
    ) -> (
        Vec<crate::settings::AccountRecord>,
        std::collections::BTreeMap<String, String>,
    ) {
        let Some(state) = cx.try_global::<crate::AppSettings>() else {
            return Default::default();
        };
        let remembered = self
            .repo_key()
            .and_then(|key| state.settings.remote_accounts.get(&key).cloned())
            .unwrap_or_default();
        (state.settings.accounts.clone(), remembered)
    }

    /// The key of this repository in `Settings::remote_accounts`: the canonical
    /// common git dir, the same for all its worktrees.
    fn repo_key(&self) -> Option<String> {
        self.sidebar_extras
            .as_ref()?
            .common_dir
            .as_ref()
            .map(|dir| dir.display().to_string())
    }

    /// Every remote resolved against the current accounts and choices. Pure
    /// and cheap: no network, no keychain.
    fn resolutions(&self, cx: &App) -> Vec<RemoteResolution> {
        let Some(extras) = self.sidebar_extras.as_ref() else {
            return Vec::new();
        };
        let (accounts, remembered) = self.account_settings(cx);
        remote_accounts::resolve_all(&accounts, &extras.remote_targets, Some(&remembered))
    }

    fn resolution_of(&self, name: &str, cx: &App) -> Option<RemoteResolution> {
        self.resolutions(cx).into_iter().find(|r| r.name == name)
    }

    /// What the sidebar shows per remote, resolved again on every render so a
    /// change in Settings shows at once. Starts the anonymous public checks
    /// the answer is waiting for (once per repository and process).
    fn remote_displays(&mut self, cx: &mut Context<Self>) -> RemoteDisplays {
        let resolutions = self.resolutions(cx);
        if resolutions.is_empty() {
            return RemoteDisplays::new();
        }
        for probe in remote_accounts::public_probes(&resolutions) {
            self.start_public_check(probe, cx);
        }
        let (accounts, _) = self.account_settings(cx);
        resolutions
            .iter()
            .map(|resolution| {
                let display = remote_accounts::display(resolution, &accounts, &|probe| {
                    remote_accounts::cached_public(&probe.key())
                });
                (resolution.name.clone(), display)
            })
            .collect()
    }

    /// Asks the service in the background whether the repository is public.
    /// The sidebar shows nothing for it until the answer, or 5 seconds, came;
    /// a failed request counts as "not public".
    fn start_public_check(&mut self, probe: remote_accounts::PublicProbe, cx: &mut Context<Self>) {
        let key = probe.key();
        if !remote_accounts::claim_public_check(&key) {
            return;
        }
        let timeout_key = key.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            remote_accounts::expire_public_check(&timeout_key);
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    forge::api::repo_is_public(
                        &forge::api::Client::new(),
                        &probe.instance,
                        &probe.coords,
                    )
                })
                .await;
            match result {
                Ok(Some(public)) => remote_accounts::finish_public_check(&key, public),
                // No usable answer: nothing is remembered as "private"; the
                // question may be asked again after a minute.
                Ok(None) => remote_accounts::release_public_check(&key),
                Err(error) => {
                    log::debug!("public check of {key} failed: {error}");
                    remote_accounts::release_public_check(&key);
                }
            }
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Account… in a remote's menu.
    fn remote_account(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resolution) = self.resolution_of(&name, cx) else {
            return;
        };
        let (accounts, _) = self.account_settings(cx);
        match remote_accounts::account_action(&resolution, &accounts) {
            AccountAction::Pick { host, ids, current } => {
                let candidates: Vec<_> = ids
                    .iter()
                    .filter_map(|id| accounts.iter().find(|a| a.id == *id).cloned())
                    .collect();
                let view = cx.entity();
                let remote = name.clone();
                let picker = open_picker(
                    name,
                    host,
                    candidates,
                    current,
                    std::rc::Rc::new(move |account, window, cx| {
                        view.update(cx, |view, cx| {
                            view.remember_account(&remote, account, window, cx)
                        });
                    }),
                    window,
                    cx,
                );
                self.account_modal = Some(picker.into_any());
            }
            AccountAction::Add { kind, .. } => self.open_add_account_for(name, kind, window, cx),
            AccountAction::AlreadyOne { host, label } => {
                toast::show(
                    ui::ToastKind::Info,
                    ui::t!(
                        "repo.toast.account_one",
                        host = host,
                        label = label,
                        remote = name
                    ),
                    window,
                    cx,
                );
            }
            AccountAction::NoService { host } => {
                toast::show(
                    ui::ToastKind::Info,
                    ui::t!("repo.toast.account_unknown_host", host = host),
                    window,
                    cx,
                );
            }
        }
    }

    /// Stores the account picked for `remote` in this repository.
    fn remember_account(
        &mut self,
        remote: &str,
        account: &crate::settings::AccountRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.account_modal = None;
        let Some(key) = self.repo_key() else {
            // Without a repository key the choice cannot be stored; a job that
            // waited for it must not start on a guess.
            toast::show(
                ui::ToastKind::Error,
                ui::t!("repo.toast.account_not_saved", remote = remote),
                window,
                cx,
            );
            return false;
        };
        let (remote_name, id) = (remote.to_owned(), account.id.clone());
        crate::update_settings(cx, move |settings| {
            settings.set_remote_account(&key, &remote_name, &id)
        });
        toast::show(
            ui::ToastKind::Ok,
            ui::t!(
                "repo.toast.account_picked",
                remote = remote,
                label = account.display_label()
            ),
            window,
            cx,
        );
        cx.notify();
        true
    }

    /// Opens the Add account dialog for the service of `remote`, on its public
    /// instance, at the sign-in step.
    fn open_add_account_for(
        &mut self,
        remote: String,
        kind: forge::ServiceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let host = crate::accounts::public_host(kind).to_owned();
        let dialog = account_dialog::open(
            AccountOpen::AddFor {
                kind,
                base_url: format!("https://{host}"),
            },
            None,
            Some(std::rc::Rc::new(move |account, window, cx| {
                view.update(cx, |view, cx| {
                    view.account_added_for(&remote, &host, account, window, cx)
                });
            })),
            window,
            cx,
        );
        self.account_modal = Some(dialog.into_any());
    }

    /// An account was added from `remote`'s menu: it is the remote's choice
    /// when it joined others on the host, otherwise it is picked up on its own.
    fn account_added_for(
        &mut self,
        remote: &str,
        host: &str,
        account: &crate::settings::AccountRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.account_modal = None;
        let url = self.sidebar_extras.as_ref().and_then(|extras| {
            extras
                .remote_targets
                .iter()
                .find(|t| t.name == remote)
                .map(|t| t.fetch_url.clone())
        });
        let (accounts, _) = self.account_settings(cx);
        let choice = url
            .and_then(|url| remote_accounts::choice_after_add(&accounts, &url, &account.id))
            .zip(self.repo_key());
        if let Some((id, key)) = choice {
            let remote = remote.to_owned();
            crate::update_settings(cx, move |settings| {
                settings.set_remote_account(&key, &remote, &id)
            });
        }
        toast::show(
            ui::ToastKind::Ok,
            ui::t!(
                "repo.toast.account_added",
                label = account.display_label(),
                host = host,
                remote = remote
            ),
            window,
            cx,
        );
        cx.notify();
    }

    /// `origin` when configured, otherwise the first remote, otherwise
    /// `origin` (the operation then reports the missing remote).
    fn default_remote_name(&self) -> String {
        let names: Vec<String> = self
            .sidebar_extras
            .as_ref()
            .map(|extras| extras.remote_urls.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default();
        network_ops::default_remote_of(&names)
            .unwrap_or("origin")
            .to_owned()
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
                self.confirm_checkout_branch(name, window, cx);
            }
            RefAction::CheckoutCommit(card) => {
                let short = card.short.clone();
                self.confirm_checkout(
                    ui::t!("repo.checkout.title", name = short),
                    ui::t!("history.checkout_confirm.body_commit", name = short),
                    ui::t!("history.checkout_confirm.confirm_checkout"),
                    move |view, window, cx| {
                        view.checkout_guarded(
                            ui::t!("repo.checkout.title", name = short).to_string(),
                            move |repo| {
                                repo.checkout_detached(card.oid)?;
                                Ok((
                                    ui::ToastKind::Ok,
                                    ui::t!("repo.checkout.commit_done", name = short).to_string(),
                                ))
                            },
                            window,
                            cx,
                        );
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
        let dirty = changes.dirty_count();
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
        let change = ref_change_for_dialog(&kind);
        let view = cx.entity();
        // A removed remote must not leave its remembered account behind.
        let removed_remote = match &kind {
            RefDialogKind::RemoveRemote { name } => Some(name.clone()),
            _ => None,
        };
        let on_done = Rc::new(move |window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| {
                if let (Some(name), Some(key)) = (&removed_remote, view.repo_key()) {
                    let name = name.clone();
                    crate::update_settings(cx, move |settings| settings.forget_remote(&key, &name));
                }
                // The dialog entity stays owned until the next one replaces
                // it: dropping it here would free the state that is running
                // this callback.
                view.after_ref_change(change, cx);
                window.refresh();
            });
        });
        let dialog = ref_dialog::open(kind, context, on_done, window, cx);
        self.ref_dialog = Some(dialog);
    }

    /// Opens the Merge dialog for `branch` (a local branch or `origin/x`).
    fn open_merge_dialog(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        // Like the ref dialogs: a new dialog replaces one still open.
        if let Some(previous) = self.merge_dialog.take() {
            previous.update(cx, |dialog, cx| dialog.close(window, cx));
        }
        let head = match self.history.read(cx).head() {
            Some(Head::Branch { name, .. }) => name.clone(),
            // The menu disables Merge without a branch checked out.
            _ => return,
        };
        // Git refuses a merge while another operation is stopped; the menus
        // disable Merge then, so this is only a guard.
        if self.changes.read(cx).operation() != git_core::Operation::None {
            return;
        }
        let changes = self.changes.read(cx);
        let context = crate::merge_dialog::MergeDialogContext {
            root: self.root.clone(),
            git: self.git.clone(),
            branch,
            head,
            dirty: changes.dirty_count(),
        };
        let view = cx.entity();
        let dialog = crate::merge_dialog::open(
            context,
            move |done, window, cx| {
                view.update(cx, |view, cx| {
                    // A merge or fast-forward adds commits; a stop, a squash or
                    // a failure leaves HEAD where it was.
                    let change = if done.head_moved {
                        RefChange::History
                    } else {
                        RefChange::RefsOnly
                    };
                    // Switching to Changes reads its status itself; when the view
                    // is already there, `after_ref_change` has to.
                    let switching = done.show_changes && view.view != ViewMode::Changes;
                    view.after_ref_change_with(change, !switching, cx);
                    if done.show_changes {
                        view.show_changes(window, cx);
                    }
                    window.refresh();
                });
            },
            window,
            cx,
        );
        self.merge_dialog = Some(dialog);
    }

    /// Asks before a user-initiated checkout; `run` starts it once confirmed.
    /// Every user entry point (sidebar, ref menus, history rows, palette,
    /// branch switcher, pull request panel) goes through here, while internal
    /// follow-ups call `checkout_guarded` directly and stay silent.
    fn confirm_checkout(
        &mut self,
        title: SharedString,
        body: SharedString,
        primary: SharedString,
        run: impl FnOnce(&mut RepositoryView, &mut Window, &mut Context<RepositoryView>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ref_action_running {
            return;
        }
        let view = cx.entity();
        let run = RefCell::new(Some(run));
        crate::confirm_dialog::open_neutral(
            title,
            body,
            primary,
            move |window, cx| {
                if let Some(run) = run.borrow_mut().take() {
                    view.update(cx, |view, cx| run(view, window, cx));
                }
            },
            window,
            cx,
        );
    }

    /// `checkout_branch` behind the confirmation. A branch that is already
    /// checked out skips it: the checkout changes nothing.
    fn confirm_checkout_branch(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_head = matches!(
            self.history.read(cx).head(),
            Some(Head::Branch { name: head, .. }) if *head == name
        );
        if is_head {
            self.checkout_branch(name, window, cx);
            return;
        }
        self.confirm_checkout(
            ui::t!("repo.checkout.switch_title", name = name),
            ui::t!("history.checkout_confirm.body_branch", name = name),
            ui::t!("history.checkout_confirm.confirm_switch"),
            move |view, window, cx| view.checkout_branch(name, window, cx),
            window,
            cx,
        );
    }

    /// Checks out a local branch, asking first when the work tree is dirty.
    fn checkout_branch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let label = name.clone();
        self.checkout_guarded(
            ui::t!("repo.checkout.switch_title", name = label).to_string(),
            move |repo| {
                repo.checkout_branch(&name)?;
                Ok((
                    ui::ToastKind::Ok,
                    ui::t!("repo.checkout.switch_done", name = label).to_string(),
                ))
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
            changes.dirty_count()
        };
        if dirty == 0 {
            self.run_checkout_action(checkout, window, cx);
            return;
        }
        let body = ui::i18n::t_count("repo.checkout.dirty_body", dirty as u64, &[]);
        let view = cx.entity();
        // Each button may fire, but only one does. `Rc` lets every closure
        // hold the checkout and the chosen one take it.
        let checkout = Rc::new(RefCell::new(Some(checkout)));
        let take = |checkout: &Rc<RefCell<Option<F>>>| {
            checkout.borrow_mut().take().expect("checkout runs once")
        };
        crate::checkout_dialog::open(
            title,
            body,
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| {
                        view.run_ref_action(window, cx, move |repo| {
                            // An empty stash (nothing left to save) is fine:
                            // the switch still happens. The stash ref is
                            // excluded from the log (`--exclude=refs/stash`),
                            // so even this path only moves HEAD and refs.
                            stash_then_checkout(repo, checkout)
                        });
                    });
                }
            },
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| {
                        view.run_checkout_action(checkout, window, cx)
                    });
                }
            },
            {
                let view = view.clone();
                let checkout = checkout.clone();
                move |window, cx| {
                    let checkout = take(&checkout);
                    view.update(cx, |view, cx| {
                        view.run_ref_action(window, cx, move |repo| {
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

    /// Runs a checkout and, when git refuses because local changes would be
    /// overwritten, offers to stash them and retry the same checkout.
    fn run_checkout_action<F>(&mut self, checkout: F, window: &mut Window, cx: &mut Context<Self>)
    where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)>
            + Send
            + Clone
            + 'static,
    {
        let retry = checkout.clone();
        self.run_repo_action_with(
            RefChange::RefsOnly,
            window,
            cx,
            checkout,
            move |view, error, window, cx| {
                let Some(known) = error_dialog::classify_git_error(&error) else {
                    view.show_git_error(error, window, cx);
                    return;
                };
                if !matches!(&known, KnownError::LocalChangesOverwritten) {
                    view.show_git_error(error, window, cx);
                    return;
                }
                let title = known.title();
                let explanation = known.explanation();
                let details = error_dialog::raw_output(&error);
                let entity = cx.entity();
                let actions = vec![ErrorAction::new(
                    ui::t!("dialogs.error.stash_and_switch"),
                    move |window, cx| {
                        entity.update(cx, |view, cx| {
                            let checkout = retry.clone();
                            view.run_ref_action(window, cx, move |repo| {
                                stash_then_checkout(repo, checkout)
                            });
                        });
                    },
                )];
                error_dialog::open(title, explanation, details, actions, window, cx);
            },
        );
    }

    /// Reloads everything a ref operation can have changed. A refs-only
    /// operation (a checkout, branch/tag creation, rename, upstream) leaves
    /// the rows in place; only a walk-changing one reloads the history.
    fn after_ref_change(&mut self, change: RefChange, cx: &mut Context<Self>) {
        self.after_ref_change_with(change, true, cx);
    }

    /// [`Self::after_ref_change`] that can skip re-reading the Changes status,
    /// for callers that have just applied a fresh one.
    fn after_ref_change_with(
        &mut self,
        change: RefChange,
        refresh_changes: bool,
        cx: &mut Context<Self>,
    ) {
        match change {
            RefChange::RefsOnly => {
                self.history
                    .update(cx, |history, cx| history.refresh_refs(cx));
            }
            RefChange::History => self.reload_history(cx),
        }
        if refresh_changes {
            self.changes.update(cx, |changes, cx| changes.refresh(cx));
        }
        self.worktrees
            .update(cx, |worktrees, cx| worktrees.refresh(cx));
        self.refresh_with(hosting_status::INTERNAL_RELOAD, cx);
        self.rebaseline_fingerprint(cx);
        cx.notify();
    }

    /// Runs a repository mutation without a dialog, then reloads the views.
    fn run_repo_action<F>(&mut self, window: &mut Window, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)> + Send + 'static,
    {
        self.run_repo_action_with(
            RefChange::History,
            window,
            cx,
            action,
            |view, error, window, cx| {
                view.show_git_error(error, window, cx);
            },
        );
    }

    /// [`run_repo_action`] for an operation that only moves HEAD or the refs:
    /// the history keeps its rows and just re-reads its badges.
    fn run_ref_action<F>(&mut self, window: &mut Window, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)> + Send + 'static,
    {
        self.run_repo_action_with(
            RefChange::RefsOnly,
            window,
            cx,
            action,
            |view, error, window, cx| {
                view.show_git_error(error, window, cx);
            },
        );
    }

    /// [`run_repo_action`] with a caller-specific failure path, for operations
    /// that can offer a retry with a preparatory step.
    fn run_repo_action_with<F, E>(
        &mut self,
        change: RefChange,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: F,
        on_error: E,
    ) where
        F: FnOnce(&git_core::Repo) -> git_core::Result<(ui::ToastKind, String)> + Send + 'static,
        E: FnOnce(&mut Self, Error, &mut Window, &mut Context<Self>) + 'static,
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
                view.retry_watch_flush(cx);
                match result {
                    Ok((kind, notice)) => {
                        toast::show(kind, notice, window, cx);
                        view.after_ref_change(change, cx);
                    }
                    Err(error) => on_error(view, error, window, cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The failure path shared by repository actions: a known git failure
    /// opens a dialog with a remedy, everything else stays a toast.
    fn show_git_error(&mut self, error: Error, window: &mut Window, cx: &mut Context<Self>) {
        let Some(known) = error_dialog::classify_git_error(&error) else {
            toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
            return;
        };
        let title = known.title();
        let explanation = known.explanation();
        let details = error_dialog::raw_output(&error);
        match known {
            KnownError::BranchInWorktree { path, .. } => {
                let model = self.worktrees_model.clone();
                let path = PathBuf::from(path);
                let actions = vec![ErrorAction::new(
                    ui::t!("dialogs.error.open_worktree"),
                    move |window, cx| {
                        model.update(cx, |model, cx| model.switch_to(&path, window, cx));
                    },
                )];
                error_dialog::open(title, explanation, details, actions, window, cx);
            }
            KnownError::PushRejected => {
                let view = cx.entity();
                let actions = vec![ErrorAction::new(
                    ui::t!("dialogs.error.pull_then_push"),
                    move |window, cx| {
                        view.update(cx, |view, cx| view.pull_then_push(window, cx));
                    },
                )];
                error_dialog::open(title, explanation, details, actions, window, cx);
            }
            KnownError::LocalChangesOverwritten | KnownError::PullNotFastForward => {
                error_dialog::open(title, explanation, details, Vec::new(), window, cx);
            }
        }
    }

    /// Pulls the current branch, then pushes once the pull completed. The
    /// intent rides with this pull only; a refused start pushes nothing.
    fn pull_then_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job_with(Job::Pull, true, window, cx);
    }

    /// The toolbar's Stash button: stashes every local change.
    fn stash_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_repo_action(window, cx, |repo| {
            if repo.stash_save(None)? {
                Ok((ui::ToastKind::Ok, ui::t!("repo.toast.stashed").to_string()))
            } else {
                Ok((
                    ui::ToastKind::Info,
                    ui::t!("repo.toast.nothing_to_stash").to_string(),
                ))
            }
        });
    }

    /// Starts the repeating, quiet fetch of every remote.
    fn start_background_fetch(&mut self, cx: &mut Context<Self>) {
        self.background_fetch_task = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            loop {
                // The interval is read each round, so a change in Settings
                // applies without reopening the repository.
                let delay = if first {
                    BACKGROUND_FETCH_FIRST
                } else {
                    match this.update(cx, |_, cx| background_fetch_interval(cx)) {
                        Ok(interval) => interval,
                        Err(_) => break,
                    }
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
        if !background_fetch_enabled(cx) || self.network.is_some() || self.background_fetch_running
        {
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
        // Account hosts are answered from the keychain; a prompt for anything
        // else is cancelled (the fetch never opens a dialog), and a remote
        // that needs a choice is skipped.
        let (_, credentials) = self.job_credentials(&Job::Fetch(FetchOptions::default()), cx);
        let store = match crate::accounts::cached_store(cx) {
            Some(Ok(store)) => Some(store),
            _ => None,
        };
        let (context, guard) = match cx.try_global::<NetworkService>() {
            Some(service) => service.job_context(crate::network_service::JobAuth {
                interactive: false,
                accounts: credentials.accounts,
                store,
            }),
            None => Default::default(),
        };
        self.background_fetch_running = true;
        self.last_fetch = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            let changed = cx
                .background_executor()
                .spawn(async move {
                    let _guard = guard;
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
                view.retry_watch_flush(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Turns the background fetch on or off and remembers it in the settings.
    fn set_background_fetch(&mut self, enabled: bool, cx: &mut Context<Self>) {
        crate::update_settings(cx, |settings| settings.background_fetch = enabled);
        cx.notify();
    }

    /// Runs a network job in the background and paints its progress in the
    /// status bar. Only one runs at a time.
    fn start_network_job(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job_with(job, false, window, cx);
    }

    /// [`Self::start_network_job`] carrying the intent that belongs to this one
    /// job: `then_push` makes a successful pull push afterwards. It is passed
    /// with the job, so it cannot outlive it or fire on a later pull.
    fn start_network_job_with(
        &mut self,
        job: Job,
        then_push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.network.is_some() {
            if then_push {
                toast::show(ui::ToastKind::Warn, ui::t!("sidebar.prs.busy"), window, cx);
            }
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let (uses, credentials) = self.job_credentials(&job, cx);
        // Several accounts fit a remote and none was chosen: ask first, then
        // start again (it asks about the next remote, if any).
        if let Some(remote) = credentials.choose.first() {
            self.pick_account_for_job(remote.clone(), job, then_push, window, cx);
            return;
        }
        let (accounts, remembered) = self.account_settings(cx);
        let meta = JobMeta {
            used: remote_accounts::used_accounts(&credentials, &accounts),
            remembered: uses
                .first()
                .and_then(|u| remembered.get(&u.remote).cloned()),
            subject: self.job_subject(&job, cx),
        };
        let JobEnv {
            context,
            guard,
            asker,
        } = Self::job_environment(credentials.accounts, cx);
        let root = self.root.clone();
        self.network_generation = self.network_generation.wrapping_add(1);
        let generation = self.network_generation;
        if matches!(job, Job::Fetch(_) | Job::Pull) {
            self.last_fetch = Some(Instant::now());
        }
        self.network = Some(NetworkActivity {
            verb: job.verb(),
            detail: ui::t!("repo.network.connecting"),
            progress: None,
        });
        cx.notify();

        let (tx, rx) = mpsc::channel();
        let worker_job = job.clone();
        cx.background_executor()
            .spawn(async move {
                // The job's accounts stay registered until the job ended.
                let _guard = guard;
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
            let run = NetworkRun {
                generation,
                job: &job,
                then_push,
            };
            let done = this
                .update_in(cx, |view, window, cx| {
                    view.poll_network(&rx, run, &meta, window, cx)
                })
                .unwrap_or(true);
            if done {
                break;
            }
        })
        .detach();
    }

    /// The environment of an interactive network call that may answer git's
    /// prompts with `accounts`.
    fn job_environment(
        accounts: HashMap<
            crate::network_service::HostKey,
            crate::network_service::AccountCredential,
        >,
        cx: &App,
    ) -> JobEnv {
        match cx.try_global::<NetworkService>() {
            Some(service) => {
                let store = match crate::accounts::cached_store(cx) {
                    Some(Ok(store)) => Some(store),
                    _ => None,
                };
                let (context, guard) = service.job_context(crate::network_service::JobAuth {
                    interactive: true,
                    accounts,
                    store,
                });
                JobEnv {
                    context,
                    guard,
                    asker: service.host_keys(),
                }
            }
            None => JobEnv {
                context: git_core::network::NetworkContext::default(),
                guard: None,
                asker: crate::network_service::HostKeyAsker::unavailable(),
            },
        }
    }

    /// Returns whether the job finished. `run.then_push` belongs to this job
    /// alone: only the pull started by "Pull, then push" pushes afterwards.
    fn poll_network(
        &mut self,
        rx: &Receiver<NetworkEvent>,
        run: NetworkRun<'_>,
        meta: &JobMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if run.generation != self.network_generation {
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
            self.retry_watch_flush(cx);
            match result {
                Ok(outcome) if !outcome.targets.is_empty() => {
                    self.show_push_result(&outcome, meta, window, cx);
                    if outcome.targets.iter().any(|t| t.result.is_ok()) {
                        self.reload_after_network(cx);
                    }
                }
                Ok(_) if matches!(run.job, Job::FetchPullRequest { .. }) => {
                    self.reload_after_network(cx);
                    if let Job::FetchPullRequest { branch, number, .. } = run.job {
                        self.checkout_pr_branch(branch.clone(), *number, window, cx);
                    }
                }
                Ok(outcome) => {
                    toast::show(
                        ui::ToastKind::Ok,
                        run.job.done_message(&outcome),
                        window,
                        cx,
                    );
                    self.reload_after_network(cx);
                    if follow_with_push(run.job, run.then_push) {
                        self.push_branch(window, cx);
                    }
                }
                Err(error) => {
                    self.show_job_error(&error, meta, run.job, window, cx);
                }
            }
            return true;
        }
        cx.notify();
        false
    }

    /// The remotes `job` contacts and the accounts git may answer with.
    fn job_credentials(
        &self,
        job: &Job,
        cx: &App,
    ) -> (
        Vec<remote_accounts::RemoteUse>,
        remote_accounts::JobCredentials,
    ) {
        let Some(extras) = self.sidebar_extras.as_ref() else {
            return Default::default();
        };
        let (accounts, remembered) = self.account_settings(cx);
        let history = self.history.read(cx);
        let uses =
            network_ops::job_remote_uses(job, history.head(), history.refs(), &extras.remotes);
        let credentials =
            remote_accounts::job_credentials(&accounts, &extras.remote_targets, &uses, &remembered);
        (uses, credentials)
    }

    /// What a push result names: the branch, the tag, or all tags.
    fn job_subject(&self, job: &Job, cx: &App) -> String {
        match job {
            Job::PushTag { tag, .. } => tag.clone(),
            Job::PushTags { .. } => ui::t!("network.push.subject_tags").to_string(),
            Job::DeleteRemoteBranch { branch, .. } => branch.clone(),
            Job::Push {
                branch: Some(branch),
            } => branch.clone(),
            _ => match self.history.read(cx).head() {
                Some(git_core::Head::Branch { name, .. }) => name.clone(),
                _ => String::new(),
            },
        }
    }

    /// Asks which account answers for `remote`, then starts `job` again with
    /// the same intent. Cancel starts nothing.
    fn pick_account_for_job(
        &mut self,
        remote: String,
        job: Job,
        then_push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (accounts, _) = self.account_settings(cx);
        let Some(resolution) = self.resolution_of(&remote, cx) else {
            return;
        };
        let target = std::iter::once(&resolution.fetch)
            .chain(resolution.pushes.iter())
            .find(|t| matches!(t.account, remote_accounts::UrlAccount::Choose(_)))
            .cloned();
        let Some(target) = target else {
            return;
        };
        let candidates: Vec<_> = target
            .matching
            .iter()
            .filter_map(|id| accounts.iter().find(|a| a.id == *id).cloned())
            .collect();
        let view = cx.entity();
        let name = remote.clone();
        let picker = open_picker(
            remote,
            target.host.clone(),
            candidates,
            None,
            std::rc::Rc::new(move |account, window, cx| {
                let job = job.clone();
                let name = name.clone();
                view.update(cx, |view, cx| {
                    if view.remember_account(&name, account, window, cx) {
                        view.start_network_job_with(job, then_push, window, cx);
                    }
                });
            }),
            window,
            cx,
        );
        self.account_modal = Some(picker.into_any());
    }

    /// The account dialog on the token step of `id`.
    fn reconnect_account(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (accounts, _) = self.account_settings(cx);
        let Some(account) = accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let dialog = account_dialog::open(AccountOpen::Reconnect(account), None, None, window, cx);
        self.account_modal = Some(dialog.into_any());
    }

    fn reconnect_action(&self, id: String, cx: &mut Context<Self>) -> ui::ToastAction {
        let view = cx.entity();
        ui::ToastAction::new(ui::t!("network.push.reconnect"), move |window, cx| {
            let id = id.clone();
            view.update(cx, |view, cx| view.reconnect_account(&id, window, cx));
        })
    }

    /// The error toast of a failed job; a refused account token gets a
    /// Reconnect button.
    fn show_job_error(
        &mut self,
        error: &Error,
        meta: &JobMeta,
        job: &Job,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(known) = error_dialog::classify_git_error(error) {
            if let Some(offer_pull_then_push) =
                error_dialog_remedy(&known, job, self.history.read(cx).head())
            {
                let title = known.title();
                let explanation = known.explanation();
                let details = error_dialog::raw_output(error);
                let actions = if offer_pull_then_push {
                    let view = cx.entity();
                    vec![ErrorAction::new(
                        ui::t!("dialogs.error.pull_then_push"),
                        move |window, cx| {
                            view.update(cx, |view, cx| view.pull_then_push(window, cx));
                        },
                    )]
                } else {
                    Vec::new()
                };
                error_dialog::open(title, explanation, details, actions, window, cx);
                return;
            }
        }
        if network_ops::is_auth_error(error) {
            if let Some(account) = remote_accounts::failed_account(&meta.used, &error.to_string()) {
                let body = ui::ToastBody {
                    lines: Vec::new(),
                    action: Some(self.reconnect_action(account.account_id.clone(), cx)),
                    ..Default::default()
                };
                toast::show_with(
                    ui::ToastKind::Error,
                    ui::t!(
                        "network.toast.token_rejected",
                        host = account.host,
                        label = account.label
                    ),
                    Some(body),
                    window,
                    cx,
                );
                return;
            }
        }
        toast::show(
            ui::ToastKind::Error,
            network_ops::friendly_error(error),
            window,
            cx,
        );
    }

    /// The result of a push that went to each of several URLs.
    fn show_push_result(
        &mut self,
        outcome: &git_core::network::NetworkOutcome,
        meta: &JobMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use git_core::network::PushStatus;
        let (accounts, _) = self.account_settings(cx);
        let total = outcome.targets.len();
        let ok = outcome.targets.iter().filter(|t| t.result.is_ok()).count();
        let mut lines = Vec::with_capacity(total);
        for target in &outcome.targets {
            let (host, account) = remote_accounts::push_target_info(
                &accounts,
                &target.url,
                meta.remembered.as_deref(),
            );
            let label = account
                .as_ref()
                .map_or_else(|| "\u{2014}".to_owned(), |(_, label)| label.clone());
            let text = format!("{host} \u{b7} {label}");
            lines.push(match &target.result {
                Ok(status) => ui::ToastLine {
                    ok: true,
                    text: text.into(),
                    detail: Some(match status {
                        PushStatus::Pushed { commits: Some(n) } => {
                            ui::i18n::t_count("network.push.commits", *n as u64, &[])
                        }
                        PushStatus::Pushed { commits: None } => ui::t!("network.push.pushed"),
                        PushStatus::NewBranch => ui::t!("network.push.new_branch"),
                        PushStatus::NewTag => ui::t!("network.push.new_tag"),
                        PushStatus::Deleted => ui::t!("network.push.deleted"),
                        PushStatus::UpToDate => ui::t!("network.push.up_to_date"),
                    }),
                    action: None,
                },
                Err(text_err) => {
                    let (reason, auth) = network_ops::target_failure(text_err);
                    // Some refs of the push went through, some were refused.
                    let reason = if target.partial {
                        ui::t!("network.push.partial", reason = reason).to_string()
                    } else {
                        reason
                    };
                    ui::ToastLine {
                        ok: false,
                        text: text.into(),
                        detail: Some(reason.into()),
                        action: account
                            .filter(|_| auth)
                            .map(|(id, _)| self.reconnect_action(id, cx)),
                    }
                }
            });
        }
        let (kind, title) = if ok == total {
            (
                ui::ToastKind::Ok,
                ui::i18n::t_count(
                    "network.push.title_all",
                    total as u64,
                    &[("subject", &meta.subject)],
                ),
            )
        } else {
            (
                ui::ToastKind::Error,
                ui::i18n::t_count(
                    "network.push.title_some",
                    total as u64,
                    &[("subject", &meta.subject), ("ok", &ok)],
                ),
            )
        };
        toast::show_with(
            kind,
            title,
            Some(ui::ToastBody {
                lines,
                action: None,
                ..Default::default()
            }),
            window,
            cx,
        );
    }

    /* ------------------------ pull / merge requests ------------------------ */

    /// The remotes that can take a pull request, the current branch's
    /// upstream first. Pure: no network, no keychain.
    fn pr_targets(&self, cx: &App) -> Vec<PrTarget> {
        let history = self.history.read(cx);
        self.pr_targets_in(history.head(), history.refs().unwrap_or(&[]), cx)
    }

    /// [`Self::pr_targets`] over a given HEAD and refs, for callers that run
    /// inside an update of the history (which cannot be read then).
    fn pr_targets_in(&self, head: Option<&Head>, refs: &[Ref], cx: &App) -> Vec<PrTarget> {
        let Some(extras) = self.sidebar_extras.as_ref() else {
            return Vec::new();
        };
        let (accounts, remembered) = self.account_settings(cx);
        let preferred = web_links::upstream_remote(head, refs, &extras.remote_targets);
        pr_logic::pr_targets(
            &accounts,
            &extras.remote_targets,
            &remembered,
            preferred.as_deref(),
        )
    }

    /// Hands the panel the remotes it can list. Cheap when nothing changed.
    fn sync_pr_targets(&mut self, cx: &mut Context<Self>) {
        let targets = self.pr_targets(cx);
        self.pr_model
            .update(cx, |model, cx| model.set_targets(targets, cx));
    }

    fn set_sidebar_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        if self.sidebar_tab != tab {
            self.sidebar_tab = tab;
            cx.notify();
        }
    }

    fn pr_event(&mut self, event: PrEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PrEvent::Reconnect(id) => self.reconnect_pr_account(&id, window, cx),
            PrEvent::Open { url, host } => self.open_on_host(&url, &host, window, cx),
            PrEvent::Checkout(request) => {
                let (pr, target) = *request;
                self.start_pr_checkout(&pr, target.remote.clone(), target.kind(), window, cx);
            }
        }
    }

    /// The account dialog on the token step; the panel reloads once saved.
    fn reconnect_pr_account(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (accounts, _) = self.account_settings(cx);
        let Some(account) = accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let view = cx.entity();
        let dialog = account_dialog::open(
            AccountOpen::Reconnect(account),
            None,
            Some(Rc::new(move |_, _, cx| {
                view.update(cx, |view, cx| {
                    view.pr_model.update(cx, |model, cx| model.invalidate(cx));
                });
            })),
            window,
            cx,
        );
        self.account_modal = Some(dialog.into_any());
    }

    /// A push of `branch` to `remote` (setting the upstream) with that
    /// remote's account, ready to run on the background executor.
    fn prepare_push(&self, remote: &str, branch: &str, cx: &App) -> Option<PushRun> {
        let git = self.git.as_ref().ok()?.clone();
        let extras = self.sidebar_extras.as_ref()?;
        let (accounts, remembered) = self.account_settings(cx);
        let uses = [remote_accounts::RemoteUse {
            remote: remote.to_owned(),
            direction: remote_accounts::Direction::Push,
        }];
        let credentials =
            remote_accounts::job_credentials(&accounts, &extras.remote_targets, &uses, &remembered);
        let env = Self::job_environment(credentials.accounts, cx);
        let root = self.root.clone();
        let options = git_core::network::PushOptions {
            remote: Some(remote.to_owned()),
            refspec: Some(branch.to_owned()),
            set_upstream: true,
        };
        Some(Box::new(move || {
            // The account stays registered until the push ended.
            let _guard = env.guard;
            let repo = Repo::open(git, &root)?;
            network_ops::push(
                &repo,
                &options,
                &env.context,
                &env.asker,
                &git_core::network::CancelToken::new(),
                |_| {},
            )
        }))
    }

    /// The remote the local branch `branch` tracks, if it is a configured one.
    fn branch_upstream_remote(&self, branch: &str, cx: &App) -> Option<String> {
        self.branch_upstream_remote_in(branch, self.history.read(cx).refs().unwrap_or(&[]))
    }

    /// [`Self::branch_upstream_remote`] over a given list of refs.
    fn branch_upstream_remote_in(&self, branch: &str, refs: &[Ref]) -> Option<String> {
        let remote_targets: &[remote_accounts::RemoteTargets] = self
            .sidebar_extras
            .as_ref()
            .map_or(&[], |extras| &extras.remote_targets);
        refs.iter()
            .find(|r| r.kind == RefKind::LocalBranch && r.name == branch)
            .and_then(|r| r.upstream.as_deref())
            .and_then(|upstream| web_links::split_remote_ref(upstream, remote_targets))
            .map(|(remote, _)| remote.to_owned())
    }

    /// Opens "Create pull request" for `branch`, or for the current branch.
    /// Does nothing without a remote that has an account.
    pub(crate) fn open_create_pr(
        &mut self,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.pr_targets(cx);
        if targets.is_empty() {
            return;
        }
        let head = match branch {
            Some(branch) => branch,
            None => match self.history.read(cx).head() {
                Some(Head::Branch { name, .. }) => name.clone(),
                _ => {
                    toast::show(
                        ui::ToastKind::Info,
                        ui::t!("repo.toast.detached_no_branch"),
                        window,
                        cx,
                    );
                    return;
                }
            },
        };
        let upstream_remote = self.branch_upstream_remote(&head, cx);
        let remote_branches = {
            let history = self.history.read(cx);
            let refs = history.refs().unwrap_or(&[]);
            let branches: HashMap<String, Vec<String>> = targets
                .iter()
                .map(|target| {
                    let prefix = format!("{}/", target.remote);
                    let names = refs
                        .iter()
                        .filter(|r| r.kind == RefKind::RemoteBranch)
                        .filter_map(|r| r.name.strip_prefix(&prefix))
                        .filter(|name| *name != "HEAD")
                        .map(str::to_owned)
                        .collect();
                    (target.remote.clone(), names)
                })
                .collect();
            branches
        };
        let target = pr_logic::default_target(&targets, upstream_remote.as_deref())
            .and_then(|default| targets.iter().position(|t| t == default))
            .unwrap_or(0);
        let view = cx.entity();
        let context = PrDialogContext {
            root: self.root.clone(),
            git: self.git.clone(),
            targets,
            target,
            head,
            remote_branches,
            prepare_push: Rc::new({
                let view = view.clone();
                move |remote, branch, cx| view.read(cx).prepare_push(remote, branch, cx)
            }),
            reconnect: Rc::new({
                let view = view.clone();
                move |id, window, cx| {
                    view.update(cx, |view, cx| view.reconnect_pr_account(&id, window, cx));
                }
            }),
            open_url: Rc::new({
                let view = view.clone();
                move |url, host, window, cx| {
                    view.update(cx, |view, cx| view.open_on_host(&url, &host, window, cx));
                }
            }),
            changed: Rc::new({
                let view = view.clone();
                move |pushed, created, _, cx| {
                    view.update(cx, |view, cx| {
                        // Forget the lists first: the reload below then finds
                        // them stale and loads them once.
                        if created {
                            view.pr_model.update(cx, |model, cx| model.invalidate(cx));
                        }
                        if pushed {
                            view.reload_after_network(cx);
                        }
                    });
                }
            }),
            closed: Rc::new(move |cx| {
                view.update(cx, |view, _| view.pr_dialog = None);
            }),
        };
        self.pr_dialog = Some(create_pr_dialog::open(context, window, cx));
    }

    /// Reloads everything a remote change can affect.
    fn reload_after_network(&mut self, cx: &mut Context<Self>) {
        self.reload_history(cx);
        self.changes.update(cx, |changes, cx| changes.refresh(cx));
        self.refresh_with(PrTrigger::AfterNetwork, cx);
        self.rebaseline_fingerprint(cx);
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

    /// Copies `oid`'s full message; `CommitInfo` carries only the subject, so
    /// the body is read in the background.
    fn copy_commit_message(&mut self, oid: Oid, window: &mut Window, cx: &mut Context<Self>) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                toast::show(ui::ToastKind::Error, error.to_string(), window, cx);
                return;
            }
        };
        let root = self.root.clone();
        let window_handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    repo.commit_details(oid)
                })
                .await;
            cx.update(|cx| match result {
                Ok(details) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(full_message_text(&details)));
                }
                Err(error) => {
                    window_handle
                        .update(cx, |_, window, cx| {
                            toast::show(
                                ui::ToastKind::Error,
                                ui::t!("history.refmenu.message_failed", error = error.to_string()),
                                window,
                                cx,
                            );
                        })
                        .ok();
                }
            });
        })
        .detach();
    }

    pub(crate) fn pull_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job(Job::Pull, window, cx);
    }

    pub(crate) fn push_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_network_job(Job::Push { branch: None }, window, cx);
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
        self.refresh_with(PrTrigger::Manual, cx);
    }

    /// [`Self::refresh`] that also says why, for the pull request map.
    fn refresh_with(&mut self, trigger: PrTrigger, cx: &mut Context<Self>) {
        self.hosting_trigger = trigger;
        let root = self.sidebar_root.clone().or_else(|| {
            self.history
                .read(cx)
                .root()
                .map(std::path::Path::to_path_buf)
        });
        if let Some(root) = root {
            self.start_sidebar(root, cx);
        }
        // Only the user's own refresh bypasses the 60 s staleness window.
        let force = trigger.resets_health();
        self.pr_model
            .update(cx, |model, cx| model.refresh(force, cx));
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
        // A remote renamed or removed since the last load takes its remembered
        // account along (or drops it).
        if let (Some(previous), Some(key)) = (&self.sidebar_extras, extras.common_dir.as_ref()) {
            let before: Vec<String> = previous.remotes.clone();
            let after = extras.remotes.clone();
            // An empty list may be a failed read, not a repository without
            // remotes: the explicit Remove remote path covers the last one.
            if before != after && !after.is_empty() {
                let key = key.display().to_string();
                crate::update_settings(cx, move |settings| {
                    settings.reconcile_remote_accounts(&key, &before, &after)
                });
            }
        }
        let targets = extras.remote_targets.clone();
        self.history
            .update(cx, |history, _| history.set_remote_targets(targets));
        self.sidebar_extras = Some(extras);
        let trigger = std::mem::replace(&mut self.hosting_trigger, PrTrigger::Focus);
        self.hosting_sync(trigger, cx);
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
        self.schedule_selected_ci(cx);
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

    /// The worktree switcher popover is open (its button is pressed).
    pub(crate) fn worktree_switcher_open(&self) -> bool {
        self.worktree_switcher
            .as_ref()
            .is_some_and(|weak| weak.upgrade().is_some())
    }

    /// Opens the worktree switcher (Ctrl Alt W) under its titlebar button; a
    /// second call closes it.
    pub(crate) fn open_worktree_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self
            .worktree_switcher
            .as_ref()
            .and_then(|weak| weak.upgrade())
        {
            open.update(cx, |switcher, cx| switcher.close(window, cx));
            return;
        }
        // Without the button (narrow bar) the popover opens under the branch
        // switcher instead.
        let anchor = self
            .worktree_bounds
            .get()
            .or(self.branch_bounds.get())
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(200.), px(28.))));
        let repo = cx.entity().downgrade();
        let add: worktree_switcher::OpenAdd = Rc::new(move |window, cx| {
            repo.update(cx, |view, cx| {
                view.worktrees
                    .update(cx, |worktrees, cx| worktrees.open_add(window, cx));
            })
            .ok();
        });
        let switcher =
            worktree_switcher::open(self.worktrees_model.clone(), anchor, add, window, cx);
        // Any way of closing it (Esc, a click outside, a pick) releases the
        // popover; the button un-presses then.
        self._worktree_switcher_release = Some(cx.observe_release(&switcher, |view, _, cx| {
            view.worktree_switcher = None;
            cx.notify();
        }));
        self.worktree_switcher = Some(switcher.downgrade());
        cx.notify();
    }

    /// Opens the command palette (Ctrl P) over this repository.
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pr_targets = self.pr_targets(cx);
        let history = self.history.read(cx);
        let state = palette::ViewState {
            idle_lanes_collapsed: history.idle_lanes_collapsed(),
            activity_collapsed: self.activity.read(cx).is_collapsed(),
            pull_requests: !pr_targets.is_empty(),
            merge_requests: pr_targets
                .first()
                .is_some_and(|target| target.kind() == forge::ServiceKind::GitLab),
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
                            ui::t!("repo.toast.commit_before_branch"),
                            window,
                            cx,
                        );
                    }
                }
            }
            Command::CreateTagHere => self.run_sidebar_action(SidebarAction::CreateTag, window, cx),
            Command::ShowBranchCommits => match self.history.read(cx).head() {
                Some(Head::Branch { name, .. }) => {
                    let name = name.clone();
                    self.show_branch_commits(name, window, cx);
                }
                _ => {
                    ui::toast::show(
                        ui::ToastKind::Info,
                        ui::t!("repo.toast.detached_no_branch"),
                        window,
                        cx,
                    );
                }
            },
            Command::AddRemote => self.run_sidebar_action(SidebarAction::AddRemote, window, cx),
            Command::AddWorktree => self.run_sidebar_action(SidebarAction::AddWorktree, window, cx),
            Command::CreatePullRequest => self.open_create_pr(None, window, cx),
            Command::Refresh => self.run_sidebar_action(SidebarAction::Refresh, window, cx),
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::ToggleDetails => self.toggle_details(cx),
            Command::ToggleActivity if !crate::activity_strip::enabled(cx) => {}
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

    /// Shows the History narrowed to `branch`'s own commits and empties the
    /// search field the filter replaces.
    fn show_branch_commits(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::History, window, cx);
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.history.update(cx, |history, cx| {
            history.set_branch_filter(Some(branch), cx)
        });
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
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
                        ui::t!("repo.toast.no_commits_yet", path = path),
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
            MenuItem::new(ui::t!("repo.menu.open_repository"))
                .icon(IconName::Folder)
                .shortcut("Ctrl+O")
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
            MenuItem::new(ui::t!("repo.menu.worktrees"))
                .icon(IconName::Worktree)
                .action(move |window, cx| {
                    view.update(cx, |view, cx| {
                        view.set_view(ViewMode::Worktrees, window, cx);
                    });
                }),
        ));
        let workspace = self.workspace.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new(ui::t!("repo.menu.close_repository"))
                .icon(IconName::X)
                .action(move |window, cx| {
                    workspace
                        .update(cx, |workspace, cx| workspace.show_welcome(window, cx))
                        .ok();
                }),
        ));
        entries.push(MenuEntry::Separator);
        let enabled = background_fetch_enabled(cx);
        let view = cx.entity();
        entries.push(MenuEntry::Item(
            MenuItem::new(if enabled {
                ui::t!("repo.menu.fetch_background_on")
            } else {
                ui::t!("repo.menu.fetch_background_off")
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
                MenuItem::new(ui::t!("repo.menu.loading_branches"))
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
                MenuItem::new(ui::t!("repo.menu.no_local_branches"))
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
        let mut create = MenuItem::new(ui::t!("repo.menu.create_branch")).icon(IconName::Plus);
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
            Some(Head::Detached(_)) => ui::t!("repo.toolbar.detached_head"),
            None => "…".into(),
        }
    }

    fn render_toolbar(
        &mut self,
        layout: ToolbarLayout,
        client: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let branch = self.branch_label(cx);

        let sidebar_toggle = Button::ghost("sidebar-toggle")
            .icon_only(IconName::Sidebar)
            .tooltip_kbd(
                ui::t!("repo.toolbar.toggle_sidebar"),
                ui::kbd::platform_label("Ctrl+B"),
            )
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
            (ui::t!("repo.toolbar.switch_repository"), None),
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
            (ui::t!("repo.toolbar.switch_branch"), Some("Ctrl+Shift+B")),
            self.branch_bounds.clone(),
            branch_click,
            colors,
        );

        if !layout.worktree_button {
            self.worktree_bounds.set(None);
        }
        let worktree_click = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_worktree_switcher(window, cx));
            }
        };
        let (worktree_name, worktree_changed) = {
            let model = self.worktrees_model.read(cx);
            match model.current() {
                Some(entry) => (entry.name().into(), entry.changes.len()),
                None => (self.repo_name.clone(), 0),
            }
        };
        let worktree_switcher = worktree_switcher::titlebar_button(
            worktree_name,
            layout.worktree_name,
            layout.item_pad,
            worktree_changed,
            self.worktree_switcher_open(),
            crate::settings_ui::keybindings::key_cap(cx, "worktree_switcher"),
            self.worktree_bounds.clone(),
            worktree_click,
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
            .tooltip_kbd(
                ui::t!("repo.toolbar.stash_tip"),
                ui::kbd::platform_label("Ctrl+Shift+S"),
            )
            .on_click(move |_, window, cx| {
                stash_view.update(cx, |view, cx| view.stash_changes(window, cx));
            });
        if layout.labels {
            stash_button = stash_button.label(ui::t!("repo.toolbar.stash"));
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
            ui::t!("repo.toolbar.fetch"),
            ui::t!("repo.toolbar.fetch_tip").to_string(),
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
            ui::t!("repo.toolbar.pull"),
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
            ui::t!("repo.toolbar.push"),
            hints.push_tip,
            crate::settings_ui::keybindings::key_cap(cx, "push"),
            (hints.ahead > 0).then(|| Counter::new(format!("↑{}", hints.ahead)).accent()),
            layout.labels,
            busy,
            push,
        );

        // The sidebar carries the view nav while it is open; collapsing it
        // keeps the switcher in the titlebar, right beside the toggle, so
        // navigation is never lost.
        let view_switch = (!self.sidebar_open).then(|| {
            let history_item = if layout.labels {
                SegmentItem::new(ui::t!("repo.toolbar.view_history")).with_icon(IconName::Lanes)
            } else {
                SegmentItem::icon(IconName::Lanes)
            };
            let changes_item = if layout.labels {
                SegmentItem::new(ui::t!("repo.toolbar.view_changes")).with_icon(IconName::Edit)
            } else {
                SegmentItem::icon(IconName::Edit)
            };
            let worktrees_item = if layout.labels {
                SegmentItem::new(ui::t!("repo.toolbar.view_worktrees"))
                    .with_icon(IconName::Worktree)
            } else {
                SegmentItem::icon(IconName::Worktree)
            };
            let switch_view = cx.entity();
            Segmented::new("view-switch")
                .item(history_item)
                .item(changes_item.tooltip(ui::t!("repo.toolbar.local_changes")))
                .item(
                    match crate::settings_ui::keybindings::key_cap(cx, "worktree_switcher") {
                        Some(cap) => {
                            worktrees_item.tooltip_kbd(ui::t!("repo.toolbar.view_worktrees"), cap)
                        }
                        None => worktrees_item.tooltip(ui::t!("repo.toolbar.view_worktrees")),
                    },
                )
                .selected(segment_for_view(self.view))
                .on_select(move |index, window, cx| {
                    let mode = view_for_segment(index);
                    switch_view.update(cx, |view, cx| view.set_view(mode, window, cx));
                })
        });

        let mut bar = window_titlebar(window, cx)
            .gap(px(layout.gap))
            .px(px(layout.padding))
            .child(no_shrink(sidebar_toggle))
            .when_some(view_switch, |bar, view_switch| {
                bar.child(no_shrink(view_switch))
            })
            .child(repo_switcher)
            .child(branch_switcher)
            .when(layout.worktree_button, |bar| bar.child(worktree_switcher))
            .child(no_shrink(
                div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(colors.border),
            ))
            .child(no_shrink(fetch_button))
            .child(no_shrink(pull_button))
            .child(no_shrink(push_button))
            .child(no_shrink(stash_button))
            .child(titlebar_fill());

        let mut field = SearchField::new("toolbar-search", self.search.clone());
        match self.history.read(cx).search_status() {
            // A query replaces the key hint with its count (or "No matches").
            Some(status) => field = field.trailing_text(status),
            None if layout.search_kbd => field = field.hint("Ctrl+K"),
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
                    ui::t!("repo.search.placeholder"),
                    ui::kbd::platform_label("Ctrl+K"),
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
                .tooltip_kbd(
                    ui::t!("repo.toolbar.settings"),
                    ui::kbd::platform_label("Ctrl+,"),
                )
                .on_click(settings_click),
        ));

        titlebar_end(bar, client)
    }

    /// The History / Changes / Worktrees switcher at the very top of the
    /// sidebar, as a short vertical list (Fork's sidebar header).
    fn render_view_nav(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let go = |mode: ViewMode| {
            let view = view.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.set_view(mode, window, cx));
            }
        };
        let worktrees_tip = {
            let tooltip = Tooltip::new(ui::t!("repo.toolbar.view_worktrees"));
            match crate::settings_ui::keybindings::key_cap(cx, "worktree_switcher") {
                Some(cap) => tooltip.kbd(cap),
                None => tooltip,
            }
        };
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(6.))
            .pt(px(10.))
            .pb(px(4.))
            .child(view_nav_row(
                "view-nav-history",
                IconName::Lanes,
                ui::t!("repo.toolbar.view_history"),
                self.view == ViewMode::History,
                None,
                None,
                colors,
                go(ViewMode::History),
            ))
            .child(view_nav_row(
                "view-nav-changes",
                IconName::Edit,
                ui::t!("repo.toolbar.view_changes"),
                self.view == ViewMode::Changes,
                Some(Tooltip::new(ui::t!("repo.toolbar.local_changes"))),
                self.change_counters(cx),
                colors,
                go(ViewMode::Changes),
            ))
            .child(view_nav_row(
                "view-nav-worktrees",
                IconName::Worktree,
                ui::t!("repo.toolbar.view_worktrees"),
                self.view == ViewMode::Worktrees,
                Some(worktrees_tip),
                None,
                colors,
                go(ViewMode::Worktrees),
            ))
    }

    /// The coloured `+N ~N −N` counters of the Changes row.
    fn change_counters(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let counts = change_counts(self.changes.read(cx).status());
        if counts.is_empty() {
            return None;
        }
        let colors = cx.theme().colors;
        let counter = |id: &'static str, value: String, color: Hsla, tooltip: SharedString| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .text_size(FS_XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .tooltip(Tooltip::text(tooltip))
                .child(value)
        };
        let mut group = div().flex().items_center().gap(px(6.));
        if counts.added > 0 {
            group = group.child(counter(
                "view-nav-count-added",
                format!("+{}", counts.added),
                colors.st_added,
                ui::t!("repo.sidebar.nav.added", n = counts.added),
            ));
        }
        if counts.modified > 0 {
            group = group.child(counter(
                "view-nav-count-modified",
                format!("~{}", counts.modified),
                colors.st_modified,
                ui::t!("repo.sidebar.nav.modified", n = counts.modified),
            ));
        }
        if counts.deleted > 0 {
            group = group.child(counter(
                "view-nav-count-deleted",
                format!("−{}", counts.deleted),
                colors.st_deleted,
                ui::t!("repo.sidebar.nav.deleted", n = counts.deleted),
            ));
        }
        Some(group.into_any_element())
    }

    fn render_sidebar(&mut self, width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let worktrees_mode = self.view == ViewMode::Worktrees;
        let panel = worktrees_mode.then(|| self.worktree_panel(cx));
        // The Repository / PRs tabs exist while a remote can take requests;
        // the PRs tab replaces the filter and the tree.
        self.sync_pr_targets(cx);
        let has_prs = self.pr_model.read(cx).has_target();
        let show_prs = has_prs && !worktrees_mode && self.sidebar_tab == SidebarTab::PullRequests;
        self.pr_model
            .update(cx, |model, cx| model.set_active(show_prs, cx));
        let tabs = (has_prs && !worktrees_mode).then(|| self.sidebar_tabs(cx));
        // The tree is not built while the worktree panel or the PR panel
        // stands in for it.
        let nodes = if worktrees_mode || show_prs {
            Vec::new()
        } else {
            let remotes = self.remote_displays(cx);
            self.tree_nodes(&remotes, cx)
        };
        if let Some(index) = nodes.iter().position(|node| node.current) {
            // Reveal the checked-out branch only when it actually changes.
            // ScrollHandle re-applies a `scroll_to_item` on every prepaint, so
            // doing this each render would snap the list back to the current
            // branch forever, fighting a manual scroll and the rows a freshly
            // expanded folder adds. The key guards against that.
            let key = nodes[index].key.clone();
            if self.sidebar_revealed.as_ref() != Some(&key) {
                // ScrollHandle applies this during prepaint, so asking during
                // render lands on the rows that are about to be laid out.
                self.sidebar_scroll.scroll_to_item(index);
                self.sidebar_revealed = Some(key);
            }
        }
        let filtering = !self.filter.trim().is_empty();
        let no_matches = !worktrees_mode && !show_prs && filtering && nodes.is_empty();
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
            move |index: usize, window: &mut Window, cx: &mut App| {
                let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    if let Some(remote) = key.strip_prefix("addacc:") {
                        // "Add account for <host>" is an action row, not a node to reveal.
                        let remote = remote.to_owned();
                        view.selected_node = Some(key.clone());
                        view.run_sidebar_action(SidebarAction::AddAccountFor(remote), window, cx);
                        cx.notify();
                    } else if has_children {
                        view.toggle_node(&key, expanded, window, cx);
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

        // A click on a branch row's pull request badge opens its popover.
        let pr_click = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, position: Point<Pixels>, window: &mut Window, cx: &mut App| {
                let Some((key, _, _)) = rows.get(index).cloned() else {
                    return;
                };
                let Some(branch) = key.strip_prefix("local:") else {
                    return;
                };
                let branch = branch.to_owned();
                view.update(cx, |view, cx| {
                    view.open_pr_popover(&branch, position, window, cx)
                });
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
            .child(self.render_view_nav(cx))
            .when_some(tabs, |this, tabs| this.child(tabs))
            .when(!show_prs, |this| {
                this.child(div().w_full().px(px(10.)).pt(px(12.)).pb(px(8.)).child(
                    SearchField::new("sidebar-filter", self.sidebar_filter.clone()),
                ))
            })
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when_some(panel, |this, panel| this.child(panel))
                    .when(show_prs, |this| {
                        this.child(PrPanel::new(self.pr_model.clone()))
                    })
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
                                .child(ui::t!("repo.sidebar.no_matches")),
                        )
                    })
                    .when(!no_matches && !worktrees_mode && !show_prs, |this| {
                        // The tree scrolls both ways on one handle. The vertical
                        // bar overlays its right edge (an in-flow track would cut
                        // the row highlight short); the horizontal one sits below.
                        let vscroll = div().absolute().top_0().bottom_0().right_0();
                        this.child(
                            div()
                                .relative()
                                .w_full()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .child(
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
                                        .on_context_menu(context_menu)
                                        .on_pr_click(pr_click),
                                )
                                .child(
                                    vscroll.child(
                                        Scrollbar::vertical(
                                            "sidebar-vscrollbar",
                                            self.sidebar_scroll.clone(),
                                        )
                                        .background(colors.sidebar),
                                    ),
                                ),
                        )
                        .child(Scrollbar::horizontal(
                            "sidebar-scrollbar",
                            self.sidebar_scroll.clone(),
                        ))
                    }),
            )
    }

    /// The Repository / PRs switch above the sidebar's filter field.
    fn sidebar_tabs(&self, cx: &Context<Self>) -> Div {
        let model = self.pr_model.read(cx);
        let short = model.kind().map_or_else(
            || pr_logic::noun_short(forge::ServiceKind::GitHub),
            pr_logic::noun_short,
        );
        let label = match model.open_count() {
            Some(count) => format!("{short} {count}"),
            None => short.to_string(),
        };
        let view = cx.entity();
        div().w_full().flex_none().px(px(10.)).pt(px(12.)).child(
            Segmented::new("sidebar-tabs")
                .items([
                    SegmentItem::new(ui::t!("sidebar.prs.tab_repository")),
                    SegmentItem::new(label),
                ])
                .selected(match self.sidebar_tab {
                    SidebarTab::Repository => 0,
                    SidebarTab::PullRequests => 1,
                })
                .on_select(move |index, _, cx| {
                    let tab = if index == 0 {
                        SidebarTab::Repository
                    } else {
                        SidebarTab::PullRequests
                    };
                    view.update(cx, |view, cx| view.set_sidebar_tab(tab, cx));
                }),
        )
    }

    fn tree_nodes(&self, remotes: &RemoteDisplays, cx: &Context<Self>) -> Vec<TreeNodeData> {
        let history = self.history.read(cx);
        let mut nodes = sidebar_nodes_with(
            history.refs(),
            history.head(),
            self.sidebar_extras.as_ref(),
            remotes,
            &self.sidebar,
            &self.filter,
            self.selected_node.as_deref(),
        );
        // A local branch with a pull request carries its badge.
        for node in &mut nodes {
            let badge = node
                .key
                .strip_prefix("local:")
                .and_then(|name| self.hosting_view.badge(name));
            if let Some(badge) = badge {
                node.pr = Some(TreePr {
                    state: badge.state,
                    text: badge.text.clone(),
                    tooltip: badge.tooltip.clone(),
                });
            }
        }
        nodes
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
                    .tooltip(ui::t!("repo.details.dock_right")),
            )
            .item(
                SegmentItem::icon(IconName::PanelB)
                    .id("dock-bottom")
                    .tooltip(ui::t!("repo.details.dock_bottom")),
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
            .child(ui::t!("repo.details.title"))
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
            DetailsState::Empty => {
                muted_note(ui::t!("repo.details.select_commit"), colors).into_any_element()
            }
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
                        .child(ui::t!("repo.details.load_failed")),
                )
                .child(div().text_color(colors.error).child(SelectableText::new(
                    gpui::ElementId::Name("details-info-error".into()),
                    message.clone(),
                )))
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
            div()
                .text_color(colors.muted)
                .child(ui::t!("repo.details.root_commit"))
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

        let tools = self.render_host_tools(details.id.to_string(), cx);
        let checks = self.render_checks(details.id, cx);
        column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(FS_SM)
                    .child(meta_row(ui::t!("repo.details.author"), author, colors))
                    .child(meta_row(ui::t!("repo.details.authored"), authored, colors))
                    .child(meta_row(
                        ui::t!("repo.details.committer"),
                        committer,
                        colors,
                    ))
                    .child(meta_row(ui::t!("repo.details.hash"), hash, colors))
                    .child(meta_row(ui::t!("repo.details.parents"), parents, colors)),
            )
            .children(tools)
            .children(checks)
            .into_any_element()
    }

    /// "Open on <Service>" and "Copy link" for the commit, under its metadata;
    /// none when no remote points at a known host.
    fn render_host_tools(&self, sha: String, cx: &mut Context<Self>) -> Option<Div> {
        let links = self.host_links(cx);
        let link = links.first()?;
        let url = link.url(&forge::WebTarget::Commit(sha))?;
        let host = link.host().to_owned();
        let view = cx.entity();
        let copy_view = view.clone();
        let pr_view = view.clone();
        let copy_url = url.clone();
        // The pull request of the local branch that stands on this commit.
        let pr = self
            .details_oid
            .and_then(|oid| self.hosting.pr_branch_at(oid))
            .and_then(|branch| {
                self.hosting_view
                    .badge(branch)
                    .map(|badge| (branch.to_owned(), badge.clone()))
            });
        Some(
            div()
                .mt(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("details-open-on-host")
                        .size(ButtonSize::Small)
                        .icon(IconName::External)
                        .label(ui::t!(
                            "repo.details.open_on_host",
                            service = link.service_name()
                        ))
                        .on_click(move |_, window, cx| {
                            view.update(cx, |view, cx| view.open_on_host(&url, &host, window, cx));
                        }),
                )
                .child(
                    Button::new("details-copy-link")
                        .size(ButtonSize::Small)
                        .icon(IconName::Copy)
                        .label(ui::t!("repo.details.copy_link"))
                        .on_click(move |_, window, cx| {
                            copy_view.update(cx, |view, cx| {
                                view.copy_link(
                                    copy_url.clone(),
                                    ui::t!("repo.toast.copied_link"),
                                    window,
                                    cx,
                                )
                            });
                        }),
                )
                .children(pr.map(|(branch, badge)| {
                    PrBadge::new("details-pr", badge.state, badge.text.clone())
                        .review(badge.review)
                        .tooltip(badge.tooltip.clone())
                        .on_click(move |event, window, cx| {
                            pr_view.update(cx, |view, cx| {
                                view.open_pr_popover(&branch, event.position(), window, cx)
                            });
                        })
                })),
        )
    }

    /// Right-click on a changed file: its page, blame and permalink on the
    /// host (at the selected commit), and Copy path.
    fn open_file_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some((path, deleted)), Some(sha)) = (
            self.details
                .files()
                .and_then(|files| files.get(index))
                .map(|file| {
                    (
                        file.path.clone(),
                        file.kind == git_core::ChangeKind::Deleted,
                    )
                }),
            self.details.details().map(|details| details.id.to_string()),
        ) else {
            return;
        };
        let links = self.host_links(cx);
        let view = cx.entity();
        let mut entries: Vec<MenuEntry> = Vec::new();
        // A file deleted by the commit has no page at that commit: only Copy path.
        if let Some(link) = links.first().filter(|_| !deleted) {
            let file = forge::WebTarget::File {
                rev: sha.clone(),
                path: path.clone(),
            };
            let blame = forge::WebTarget::Blame {
                rev: sha,
                path: path.clone(),
            };
            let service = link.service_name();
            let open = |label: SharedString, url: String| {
                let (view, host) = (view.clone(), link.host().to_owned());
                MenuEntry::Item(MenuItem::new(label).icon(IconName::External).action(
                    move |window, cx| {
                        view.update(cx, |view, cx| view.open_on_host(&url, &host, window, cx));
                    },
                ))
            };
            if let Some(url) = link.url(&file) {
                entries.push(open(
                    ui::t!("repo.details.open_file_on_host", service = service),
                    url.clone(),
                ));
                // The URL is pinned to the commit's hash, so it is a permalink.
                let permalink_view = view.clone();
                let permalink = MenuItem::new(ui::t!("repo.details.copy_permalink"))
                    .icon(IconName::Link)
                    .action(move |window, cx| {
                        permalink_view.update(cx, |view, cx| {
                            view.copy_link(
                                url.clone(),
                                ui::t!("repo.toast.copied_permalink"),
                                window,
                                cx,
                            )
                        });
                    });
                if let Some(blame_url) = link.url(&blame) {
                    entries.push(open(
                        ui::t!("repo.details.open_blame_on_host", service = service),
                        blame_url,
                    ));
                }
                entries.push(MenuEntry::Item(permalink));
                entries.push(MenuEntry::Separator);
            }
        }
        entries.push(MenuEntry::Item(
            MenuItem::new(ui::t!("repo.details.copy_path"))
                .icon(IconName::Copy)
                .action(move |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                }),
        ));
        menu::open_context_menu(position, entries, window, cx);
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
                    .child(ui::t!("repo.details.changed_files")),
            );
        if let Some(count) = count {
            header = header.child(CountPill::new(count.to_string()));
        }
        header = header.child(div().flex_1());
        if is_merge {
            header = header.child(
                div()
                    .id("details-vs-parent")
                    .tooltip(Tooltip::text(ui::t!("repo.details.vs_parent_tip")))
                    .child(ui::t!("repo.details.vs_parent")),
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
                DetailsState::Failed(message) => {
                    div()
                        .p(px(12.))
                        .text_color(colors.error)
                        .child(SelectableText::new(
                            gpui::ElementId::Name("details-files-error".into()),
                            message.clone(),
                        ))
                }
                _ => muted_note(ui::t!("repo.details.no_files"), colors),
            };
        };
        if files.is_empty() {
            return muted_note(ui::t!("repo.details.no_files"), colors);
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
            let menu_view = view.clone();
            list = list.child(
                div()
                    .debug_selector(move || format!("details-file-{index}"))
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        menu_view.update(cx, |view, cx| {
                            view.select_file(index, cx);
                            view.open_file_menu(index, event.position, window, cx);
                        });
                        // Keep the menu that just took focus.
                        window.prevent_default();
                    })
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
                SegmentItem::new(ui::t!("repo.diff.unified"))
                    .with_icon(IconName::Unified)
                    .id("diff-unified"),
            )
            .item(
                SegmentItem::new(ui::t!("repo.diff.side_by_side"))
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
            DiffState::Empty => diff_note(ui::t!("repo.diff.select_file"), colors),
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
                diff_note(ui::t!("repo.diff.too_large"), colors)
            }
            DiffState::Failed(message) => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .text_color(colors.error)
                .child(SelectableText::new(
                    gpui::ElementId::Name("details-diff-error".into()),
                    message.clone(),
                )),
            DiffState::Loaded(data) => {
                if data.diff.binary {
                    diff_note(ui::t!("repo.diff.binary"), colors)
                } else if data.rows.is_empty() {
                    diff_note(ui::t!("repo.diff.no_changes"), colors)
                } else {
                    let data = data.clone();
                    let list = if layout == DiffLayout::Split {
                        let row_width = split_width(width);
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
                    .child(network.verb.clone())
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
                        .child(ui::t!("repo.status.loading_changes")),
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
            bar = bar.child(ui::t!(
                "repo.status.unstaged_staged",
                // Conflicted paths are still unstaged work for the status bar.
                unstaged = changes.unstaged_count() + changes.conflicted_count(),
                staged = changes.staged_count()
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
                            .child(ui::t!("repo.status.loading_history")),
                    );
                }
                LoadState::Cancelled => {
                    bar = bar.child(
                        div()
                            .text_color(colors.muted)
                            .child(ui::t!("repo.status.loading_cancelled")),
                    );
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
            bar = bar.child(ui::i18n::t_count(
                "repo.status.commits_loaded",
                history.commit_count() as u64,
                &[("n", &format_count(history.commit_count()))],
            ));
        }

        let workspace = self.workspace.clone();
        bar = bar.child(
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
        );

        // The details dock toggle (like Fork's) sits to the right of the git
        // version. The dock belongs to History only, so it shows there; it
        // highlights while the panel is hidden.
        if self.view == ViewMode::History {
            let view = cx.entity();
            let hidden = !self.details_open;
            let color = if hidden { colors.accent } else { colors.muted };
            bar = bar.child(
                div()
                    .id("status-dock")
                    .h(px(20.))
                    .w(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(R_SM)
                    .cursor_pointer()
                    .when(hidden, |this| this.bg(colors.hover))
                    .hover(|this| this.bg(colors.hover))
                    .tooltip(Tooltip::with_kbd(
                        ui::t!("repo.toolbar.toggle_dock"),
                        ui::kbd::platform_label("Ctrl+J"),
                    ))
                    .on_click(
                        move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
                            view.update(cx, |view, cx| view.toggle_details(cx));
                        },
                    )
                    .child(Icon::new(IconName::PanelB).size(px(14.)).color(color)),
            );
        }
        bar
    }
}

/* ----------------------- pull requests and CI status ---------------------- */

impl RepositoryView {
    /// Works out which remote speaks for the repository and which branches
    /// track it, then starts whatever requests the policy says are due.
    /// Cheap and non-blocking: the requests run on the background executor.
    fn hosting_sync(&mut self, trigger: PrTrigger, cx: &mut Context<Self>) {
        let Some(extras) = self.sidebar_extras.as_ref() else {
            return;
        };
        let (accounts, remembered) = self.account_settings(cx);
        let account_refs = remote_accounts::account_refs(&accounts);
        let (target, tracked, remote_tips) = {
            let history = self.history.read(cx);
            let refs = history.refs().unwrap_or(&[]);
            let target = hosting_status::primary_target(
                &account_refs,
                &extras.remote_targets,
                history.head(),
                refs,
                &remembered,
            );
            let tracked = target
                .as_ref()
                .map(|t| hosting_status::tracked_branches(refs, &extras.remote_targets, t))
                .unwrap_or_default();
            let remote_tips = target
                .as_ref()
                .map(|t| hosting_status::primary_remote_tips(refs, &t.remote))
                .unwrap_or_default();
            (target, tracked, remote_tips)
        };
        if trigger.resets_health() {
            // The user asked again: a token rejected before may be new now.
            self.hosting.health.manual_retry();
        }
        self.hosting.retarget(target, tracked, remote_tips);
        let now = std::time::Instant::now();
        if self
            .hosting
            .wants_prs(trigger, now, hosting_status::unix_now())
        {
            self.hosting_fetch_prs(cx);
        }
        self.hosting_fetch_ci(cx);
        self.publish_hosting(cx);
    }

    fn hosting_request(&self, target: &Target, cx: &App) -> Request {
        let store = match crate::accounts::cached_store(cx) {
            Some(Ok(store)) => Some(store),
            _ => None,
        };
        Request::new(self.hosting.client.clone(), target, store)
    }

    fn hosting_fetch_prs(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.hosting.target.clone() else {
            return;
        };
        let branches = self.hosting.pr_branches();
        self.hosting.begin_prs(std::time::Instant::now());
        let request = self.hosting_request(&target, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request.pull_requests(&branches) })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_prs(target, result, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_prs(
        &mut self,
        target: Target,
        result: Result<
            std::collections::HashMap<String, forge::api::PullRequest>,
            forge::api::ApiError,
        >,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The repository's primary remote changed while this was running.
        if self.hosting.target.as_ref() != Some(&target) {
            return;
        }
        let effect = self.hosting.finish_prs(result, hosting_status::unix_now());
        if let Some(effect) = effect {
            log::debug!("pull requests of {}: {effect:?}", target.link.host());
            self.hosting_failed(&target, effect, window, cx);
        }
        self.hosting_fetch_ci(cx);
        self.publish_hosting(cx);
        self.ensure_ci_timer(window, cx);
    }

    /// Recomputes which commits the host is known to have, from the commits
    /// the history has loaded and the primary remote's remote-tracking tips.
    /// Pure memory work, redone only when the loaded commits changed.
    fn refresh_reach(&mut self, cx: &mut Context<Self>) {
        let loaded = self.history.read(cx).loaded_len();
        if self.hosting.target.is_none() || !self.hosting.reach_outdated(loaded) {
            return;
        }
        let reach = self
            .history
            .read(cx)
            .reachable_from(self.hosting.remote_tips());
        self.hosting.set_reach(reach, loaded);
    }

    /// Asks for the selected commit's checks once the selection has rested for
    /// 300 ms; a newer selection cancels the wait.
    fn schedule_selected_ci(&mut self, cx: &mut Context<Self>) {
        if !ci_status_enabled(cx) {
            return;
        }
        self.ci_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update(cx, |view, cx| {
                view.ci_debounce = None;
                view.hosting_fetch_ci(cx);
            })
            .ok();
        }));
    }

    /// The history's visible rows changed: ask for their checks without
    /// waiting for the re-check timer.
    fn set_visible_ci(&mut self, oids: Vec<Oid>, cx: &mut Context<Self>) {
        if self.ci_visible == oids {
            return;
        }
        self.ci_visible = oids;
        self.hosting_fetch_ci(cx);
    }

    /// Starts the 30 s re-check while a commit on screen has pending checks and
    /// the window is active. The loop ends itself when neither holds.
    fn ensure_ci_timer(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !ci_status_enabled(cx)
            || self.ci_timer.is_some()
            || !window.is_window_active()
            || !self.hosting.has_pending_ci(
                self.details_oid,
                self.head_oid(cx),
                &self.ci_visible,
                true,
            )
        {
            return;
        }
        self.ci_timer = Some(cx.spawn(async move |this, cx| loop {
            // A second of slack: an answer is re-asked once it is 30 s old.
            cx.background_executor()
                .timer(hosting_status::CI_PENDING_REFETCH + Duration::from_secs(1))
                .await;
            let keep = this
                .update_in(cx, |view, window, cx| view.ci_timer_tick(window, cx))
                .unwrap_or(false);
            if !keep {
                break;
            }
        }));
    }

    /// One tick of the re-check; says whether the timer goes on.
    fn ci_timer_tick(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !ci_status_enabled(cx) {
            self.ci_timer = None;
            return false;
        }
        let pending = self.hosting.has_pending_ci(
            self.details_oid,
            self.head_oid(cx),
            &self.ci_visible,
            true,
        );
        if !window.is_window_active() || !pending {
            self.ci_timer = None;
            return false;
        }
        self.hosting_fetch_ci(cx);
        true
    }

    /// Asks for the checks of the commits that are due: the selected one,
    /// HEAD, the pull request tips and the rows visible on screen, when the
    /// host has them.
    fn hosting_fetch_ci(&mut self, cx: &mut Context<Self>) {
        if !ci_status_enabled(cx) {
            return;
        }
        let Some(target) = self.hosting.target.clone() else {
            return;
        };
        self.refresh_reach(cx);
        // Keep the shared wanted set current even when nothing is due, so a
        // batch already running skips rows that scrolled away.
        let candidates =
            self.hosting
                .ci_candidates(self.details_oid, self.head_oid(cx), &self.ci_visible, true);
        {
            let mut wanted = self.ci_wanted.lock().unwrap_or_else(|e| e.into_inner());
            wanted.clear();
            wanted.extend(candidates.iter().map(Oid::to_string));
        }
        let due = self.hosting.ci_due(
            self.details_oid,
            self.head_oid(cx),
            &self.ci_visible,
            true,
            std::time::Instant::now(),
            hosting_status::unix_now(),
        );
        if due.is_empty() {
            return;
        }
        let shas: Vec<String> = due.iter().map(Oid::to_string).collect();
        for sha in &shas {
            self.hosting.ci.mark_inflight(sha);
        }
        let request = self
            .hosting_request(&target, cx)
            .only_wanted(self.ci_wanted.clone());
        cx.spawn(async move |this, cx| {
            let asked = shas.clone();
            let answers = cx
                .background_executor()
                .spawn(async move { request.checks(&asked) })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_ci(target, shas, answers, window, cx)
            })
            .ok();
        })
        .detach();
        // The details pane shows its loading rows for the selected commit.
        cx.notify();
    }

    fn finish_ci(
        &mut self,
        target: Target,
        asked: Vec<String>,
        answers: Vec<(
            String,
            Result<forge::api::CommitChecks, forge::api::ApiError>,
        )>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.hosting.target.as_ref() != Some(&target) {
            return;
        }
        let now = std::time::Instant::now();
        let now_unix = hosting_status::unix_now();
        // Commits the batch stopped before reaching were never asked.
        for sha in &asked {
            if !answers.iter().any(|(answered, _)| answered == sha) {
                self.hosting.ci.release(sha);
            }
        }
        let mut effect = None;
        for (sha, result) in answers {
            match &result {
                Ok(_) => self.hosting.health.succeeded(),
                Err(error) => effect = Some(self.hosting.record(error, now_unix)),
            }
            self.hosting.ci.store(&sha, result, now);
        }
        if let Some(effect) = effect {
            log::debug!("checks of {}: {effect:?}", target.link.host());
            self.hosting_failed(&target, effect, window, cx);
        }
        self.publish_hosting(cx);
        self.ensure_ci_timer(window, cx);
    }

    /// What a failure asks of the UI: a rejected token is announced once per
    /// session; every other kind is silent (the badges just stay or go).
    fn hosting_failed(
        &mut self,
        target: &Target,
        effect: ErrorEffect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if effect != ErrorEffect::TokenRejected {
            return;
        }
        let Some(id) = target.account_id.clone() else {
            return;
        };
        if !hosting_status::session_rejections().first(&id) {
            return;
        }
        let (accounts, _) = self.account_settings(cx);
        let Some(account) = accounts.iter().find(|a| a.id == id) else {
            return;
        };
        let body = ui::ToastBody {
            lines: Vec::new(),
            action: Some(self.reconnect_action(id.clone(), cx)),
            ..Default::default()
        };
        toast::show_with(
            ui::ToastKind::Error,
            ui::t!(
                "network.toast.token_rejected",
                host = target.link.host(),
                label = account.label.as_str()
            ),
            Some(body),
            window,
            cx,
        );
    }

    /// Gives the history rows and the sidebar the current badges and dots.
    fn publish_hosting(&mut self, cx: &mut Context<Self>) {
        let mut view = HostingView::from(&self.hosting);
        if !self.show_ci_status {
            view.ci.clear();
        }
        if view == *self.hosting_view {
            return;
        }
        let view = Arc::new(view);
        self.hosting_view = view.clone();
        self.history
            .update(cx, |history, cx| history.set_hosting(view, cx));
        cx.notify();
    }

    /// Opens the popover of the pull request on local branch `branch` and
    /// asks for its review state.
    fn open_pr_popover(
        &mut self,
        branch: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pr) = self.hosting.pr_for_local(branch).cloned() else {
            return;
        };
        let Some(model) = self.popover_model_for(pr.number, None) else {
            return;
        };
        let weak = cx.weak_entity();
        let popover = cx.new(|_| PrPopover::new(model, weak));
        let layer = ui::overlay::open_popover(position, popover.clone().into(), window, cx);
        popover.update(cx, |popover, _| popover.set_layer(layer));
        self.pr_popover = Some(popover);

        let Some(target) = self.hosting.target.clone() else {
            return;
        };
        if self.hosting.review(pr.number).is_some()
            || !self.hosting.health.can_request(hosting_status::unix_now())
        {
            return;
        }
        let number = pr.number;
        let request = self.hosting_request(&target, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request.review(number) })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_review(target, number, result, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// The popover of pull request `number`; `review` overrides the cached
    /// review state (the answer that just arrived, or "none" after a failure).
    fn popover_model_for(
        &self,
        number: u64,
        review: Option<forge::api::ReviewState>,
    ) -> Option<hosting_status::PopoverModel> {
        let pr = self.hosting.pr_by_number(number)?;
        let checks = self
            .hosting
            .tip_of_pr(number)
            .and_then(|tip| self.hosting.ci.checks(&tip.to_string()));
        Some(hosting_status::popover_model(
            pr,
            review.or_else(|| self.hosting.review(number)),
            checks,
            &self.hosting.service_name(),
            hosting_status::unix_now(),
        ))
    }

    fn finish_review(
        &mut self,
        target: Target,
        number: u64,
        result: Result<forge::api::ReviewState, forge::api::ApiError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.hosting.target.as_ref() != Some(&target) {
            return;
        }
        let review = match result {
            Ok(review) => {
                self.hosting.set_review(number, review);
                self.hosting.health.succeeded();
                review
            }
            Err(error) => {
                let effect = self.hosting.record(&error, hosting_status::unix_now());
                self.hosting_failed(&target, effect, window, cx);
                // Not cached: the next popover asks again. The popover stops
                // saying "loading".
                forge::api::ReviewState::None
            }
        };
        if let Some(popover) = self.pr_popover.clone() {
            if popover.read(cx).number() == number {
                if let Some(model) = self.popover_model_for(number, Some(review)) {
                    popover.update(cx, |popover, cx| popover.set_model(model, cx));
                }
            }
        }
        self.publish_hosting(cx);
    }

    /// "Open on <Service>" from the popover.
    pub(crate) fn open_hosting_url(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = self
            .hosting
            .target
            .as_ref()
            .map(|t| t.link.host().to_owned())
            .unwrap_or_default();
        self.open_on_host(url, &host, window, cx);
    }

    /// Checkout from the popover: fetch the pull request's head ref into a
    /// local branch, then check that branch out.
    pub(crate) fn checkout_pull_request(
        &mut self,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(pr), Some(target)) = (
            self.hosting.pr_by_number(number).cloned(),
            self.hosting.target.clone(),
        ) else {
            return;
        };
        self.start_pr_checkout(&pr, target.remote, target.link.instance.kind, window, cx);
    }

    /// Fetches `pr`'s head ref from `remote` into a local branch (not forced)
    /// and checks it out; shared by the popover and the pull request panel.
    fn start_pr_checkout(
        &mut self,
        pr: &forge::api::PullRequest,
        remote: String,
        kind: forge::ServiceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `start_network_job` ignores a second job: say so instead.
        if self.network.is_some() {
            toast::show(ui::ToastKind::Warn, ui::t!("sidebar.prs.busy"), window, cx);
            return;
        }
        let locals: Vec<String> = self
            .history
            .read(cx)
            .refs()
            .unwrap_or(&[])
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch)
            .map(|r| r.name.clone())
            .collect();
        let branch = hosting_status::checkout_branch_name(pr, &locals);
        let Ok(refspec) = hosting_status::checkout_refspec(kind, pr.number, &branch) else {
            return;
        };
        let number = pr.number;
        // Only this user-initiated start asks; the checkout that follows the
        // fetch (`checkout_pr_branch`) does not.
        self.confirm_checkout(
            ui::t!("repo.checkout.switch_title", name = branch),
            ui::t!(
                "history.checkout_confirm.body_pr",
                number = number,
                name = branch
            ),
            ui::t!("history.checkout_confirm.confirm_checkout"),
            move |view, window, cx| {
                view.start_network_job(
                    Job::FetchPullRequest {
                        remote,
                        refspec,
                        branch,
                        number,
                    },
                    window,
                    cx,
                );
            },
            window,
            cx,
        );
    }

    /// The branch a pull request was fetched into becomes the current one.
    fn checkout_pr_branch(
        &mut self,
        branch: String,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = branch.clone();
        self.checkout_guarded(
            ui::t!("repo.checkout.switch_title", name = label).to_string(),
            move |repo| {
                repo.checkout_branch(&branch)?;
                Ok((
                    ui::ToastKind::Ok,
                    ui::t!(
                        "repo.hosting.toast.checked_out",
                        branch = branch,
                        number = number
                    )
                    .to_string(),
                ))
            },
            window,
            cx,
        );
    }

    /// "Try again" under the checks of the selected commit.
    fn retry_checks(&mut self, cx: &mut Context<Self>) {
        if !ci_status_enabled(cx) {
            return;
        }
        if let Some(oid) = self.details_oid {
            self.hosting.ci.forget(&oid.to_string());
        }
        self.hosting.health.manual_retry();
        self.hosting_fetch_ci(cx);
        self.publish_hosting(cx);
    }

    /// The checks of the commit in the details pane: the list, or its loading,
    /// empty and failed state. `None` when the repository has no service with
    /// an API, or nothing was asked yet.
    fn render_checks(&self, oid: Oid, cx: &mut Context<Self>) -> Option<Div> {
        if !self.show_ci_status {
            return None;
        }
        let target = self.hosting.target.as_ref()?;
        if !self.hosting.health.badges_visible() {
            return None;
        }
        let colors = cx.theme().colors;
        let sha = oid.to_string();
        let service = target.link.service_name();
        let body = match self.hosting.ci.get(&sha) {
            None if self.hosting.ci.is_inflight(&sha) => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .children(
                    [60., 72., 48., 66.]
                        .into_iter()
                        .enumerate()
                        .map(|(i, width)| {
                            Skeleton::new(format!("checks-skeleton-{i}"), px(width * 2.5))
                        }),
                )
                .into_any_element(),
            // Not pushed, or never reached: nothing to show.
            None | Some(CiEntry::Absent { .. } | CiEntry::Offline { .. }) => return None,
            Some(CiEntry::Failed { message, .. }) => {
                let view = cx.entity();
                Banner::new(BannerKind::Error, ui::t!("repo.hosting.checks.error_title"))
                    .body(message.clone())
                    .action(
                        Button::new("checks-retry")
                            .size(ButtonSize::Small)
                            .icon(IconName::Sync)
                            .label(ui::t!("repo.hosting.checks.retry"))
                            .on_click(move |_, _, cx| {
                                view.update(cx, |view, cx| view.retry_checks(cx));
                            }),
                    )
                    .into_any_element()
            }
            Some(CiEntry::Loaded { checks, .. }) if checks.overall == forge::api::CiState::None => {
                EmptyState::new(ui::t!("repo.hosting.checks.none_title"))
                    .text(ui::t!("repo.hosting.checks.none_body", service = service))
                    .into_any_element()
            }
            Some(CiEntry::Loaded { checks, .. }) => {
                let header_glyph = hosting_status::ci_glyph(checks.overall);
                let title = ui::t!(
                    "repo.hosting.checks.title",
                    summary = hosting_status::checks_summary(checks)
                )
                .to_uppercase();
                let rows = checks.checks.iter().enumerate().map(|(index, check)| {
                    let glyph = hosting_status::ci_glyph(check.state);
                    let open = check.web_url.clone().map(|url| {
                        Button::ghost(format!("check-open-{index}"))
                            .size(ButtonSize::Small)
                            .icon_only(IconName::External)
                            .tooltip(ui::t!("repo.hosting.checks.open_on", service = service))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    });
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(26.))
                        .px(px(8.))
                        .text_size(FS_SM)
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(colors.border)
                        })
                        .child(match glyph {
                            Some(glyph) => CiGlyph::new(format!("check-glyph-{index}"), glyph)
                                .tooltip(hosting_status::ci_label(glyph))
                                .into_any_element(),
                            None => div().w(px(12.)).into_any_element(),
                        })
                        .child(
                            div()
                                .id(format!("check-name-{index}"))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .tooltip(Tooltip::text(check.name.clone()))
                                .child(check.name.clone()),
                        )
                        .child(
                            div()
                                .text_color(colors.muted)
                                .child(hosting_status::format_duration(check.duration_secs)),
                        )
                        .child(div().w(px(24.)).children(open))
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(ui::metrics::FS_XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.muted)
                            .children(
                                header_glyph.map(|glyph| CiGlyph::new("checks-header", glyph)),
                            )
                            .child(title),
                    )
                    .child(div().flex().flex_col().children(rows))
                    .into_any_element()
            }
        };
        Some(div().mt(px(14.)).child(body))
    }
}

impl From<&HostingState> for HostingView {
    fn from(state: &HostingState) -> Self {
        state.view()
    }
}

impl Render for RepositoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client = inline_window_controls(window);
        let colors = cx.theme().colors;
        // The settings page may have toggled CI status while this view stayed
        // open: stop the loop, drop the dots and the checks block, or ask for
        // the rows again, without a restart.
        let show_ci = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.show_ci_status)
            .unwrap_or(true);
        if show_ci != self.show_ci_status {
            self.show_ci_status = show_ci;
            if !show_ci {
                self.ci_timer = None;
            }
            self.publish_hosting(cx);
            self.hosting_fetch_ci(cx);
            self.ensure_ci_timer(window, cx);
        }
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let layout = toolbar_layout(width, toolbar_label_extra(window));

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

        let toolbar = self.render_toolbar(layout, client, window, cx);

        // The sidebar may take at most half of the visible area. Only the
        // rendered width is capped; the stored width stays as saved.
        let sidebar_width = if self.sidebar_open {
            effective_sidebar_width(self.sidebar_width, width)
        } else {
            0.
        };
        let details_limit = match self.dock {
            Dock::Right => details_width_limit(width, sidebar_width),
            Dock::Bottom => details_height_limit(height),
        };
        let details_width = if self.details_open {
            match self.dock {
                Dock::Right => effective_details_width(self.details_size, width, sidebar_width),
                Dock::Bottom => effective_details_height(self.details_size, height),
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

        // The strip is off with the Settings switch; its day filter would
        // otherwise leave History narrowed with no visible reason.
        let show_activity = crate::activity_strip::enabled(cx);
        self.activity
            .update(cx, |activity, cx| activity.set_enabled(show_activity, cx));
        if !show_activity && self.history.read(cx).day_filter().is_some() {
            self.history
                .update(cx, |history, cx| history.set_day_filter(None, cx));
        }

        let content = match self.view {
            ViewMode::History => div()
                .size_full()
                .flex()
                .flex_col()
                .children(show_activity.then(|| self.activity.clone()))
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
            .on_action(cx.listener(|view, _: &OpenWorktreeSwitcher, window, cx| {
                view.open_worktree_switcher(window, cx);
            }))
            .on_action(cx.listener(|view, _: &StashChanges, window, cx| {
                view.stash_changes(window, cx);
            }))
            .on_action(cx.listener(|view, _: &FetchAll, window, cx| {
                view.fetch_remotes(window, cx);
            }))
            .on_action(cx.listener(|view, _: &Refresh, _, cx| {
                view.refresh(cx);
            }))
            .on_action(cx.listener(|view, _: &PullBranch, window, cx| {
                view.pull_branch(window, cx);
            }))
            .on_action(cx.listener(|view, _: &PushBranch, window, cx| {
                view.push_branch(window, cx);
            }))
            .when(uses_title_strip(), |column| {
                column.child(title_strip(self.repo_name.clone(), cx))
            })
            .child(toolbar)
            .child(body)
            .child(status)
    }
}

/// The text "Copy › Full message" puts on the clipboard: the raw commit message
/// (`%B`), not `summary + body`, since `%s` joins a wrapped first paragraph.
fn full_message_text(details: &CommitDetails) -> String {
    details.message.clone()
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
    tooltip: (SharedString, Option<&'static str>),
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
        Some(kbd) => row.tooltip(Tooltip::with_kbd(
            tooltip.0.clone(),
            ui::kbd::platform_label(kbd),
        )),
        None => row.tooltip(Tooltip::text(tooltip.0.clone())),
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
    label: SharedString,
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
        _ if ahead > 0 && behind > 0 => ui::t!("repo.network.pull_diverged").to_string(),
        Some(upstream) if behind > 0 => ui::i18n::t_count(
            "repo.network.pull_from",
            behind as u64,
            &[("upstream", &upstream)],
        )
        .to_string(),
        Some(_) => ui::t!("repo.network.pull_nothing").to_string(),
        None => ui::t!("repo.network.pull_no_upstream").to_string(),
    };
    let push_tip = match (upstream, branch) {
        (Some(upstream), _) if ahead > 0 => ui::i18n::t_count(
            "repo.network.push_to",
            ahead as u64,
            &[("upstream", &upstream)],
        )
        .to_string(),
        (None, Some(branch)) => {
            // Same remote the push itself picks: `origin`, else the first.
            let remote = remotes
                .iter()
                .find(|remote| remote.as_str() == "origin")
                .or_else(|| remotes.first());
            match remote {
                Some(remote) => ui::t!(
                    "repo.network.push_set_upstream",
                    remote = remote,
                    branch = branch
                )
                .to_string(),
                None => ui::t!("repo.network.push_current").to_string(),
            }
        }
        _ => ui::t!("repo.network.push_current").to_string(),
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

/// Re-read Changes on window activation at most this often.
const QUIET_CHANGES_MIN: Duration = Duration::from_secs(2);

/// Everything cheap to read that tells the views are stale: HEAD, every ref,
/// the index's (mtime, length) and which operation files exist.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    head: Option<Head>,
    refs: Vec<(String, Oid)>,
    /// `None` when there is no index file (a fresh repository).
    index: Option<(SystemTime, u64)>,
    markers: u16,
}

/// Reads the fingerprint of the repository at `root`; `None` when it cannot be
/// opened. Runs `git for-each-ref` and `rev-parse`, so call it off the UI thread.
fn read_fingerprint(git: Arc<Git>, root: &Path) -> Option<Fingerprint> {
    let repo = Repo::open(git, root).ok()?;
    let git_dir = repo.git_dir();
    let index = std::fs::metadata(git_dir.join("index"))
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    let markers = OPERATION_MARKERS
        .iter()
        .enumerate()
        .filter(|(_, name)| git_dir.join(name).exists())
        .fold(0u16, |bits, (bit, _)| bits | (1 << bit));
    Some(Fingerprint {
        head: repo.head().ok(),
        refs: ref_snapshot(&repo),
        index,
        markers,
    })
}

/// What to reload after comparing two fingerprints.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReloadDecision {
    /// Reload HEAD/refs (and the history, for [`RefChange::History`]).
    refs: Option<RefChange>,
    /// Re-read the Changes status.
    changes: bool,
}

/// Decides what an outside change requires. Without a previous fingerprint
/// there is nothing to compare: the new one becomes the baseline.
fn decide_reload(old: Option<&Fingerprint>, new: &Fingerprint) -> ReloadDecision {
    let Some(old) = old.filter(|old| *old != new) else {
        return ReloadDecision::default();
    };
    let refs_moved = old.refs != new.refs;
    let head_or_operation = old.head != new.head || old.markers != new.markers;
    ReloadDecision {
        refs: if refs_moved {
            // A ref was added, removed or moved: commits may have changed.
            Some(RefChange::History)
        } else if head_or_operation {
            // e.g. a checkout of an existing branch: only HEAD moved.
            Some(RefChange::RefsOnly)
        } else {
            None
        },
        changes: refs_moved || head_or_operation || old.index != new.index,
    }
}

/// One diff row and its cells, in pixels.
const DIFF_ROW: f32 = 22.;
const DIFF_GUTTER: f32 = 44.;
const DIFF_MARKER: f32 = 16.;

/// A muted, centred message for a diff that has nothing to paint.
fn diff_note(text: SharedString, colors: ThemeColors) -> Div {
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
fn split_width(pane: f32) -> Pixels {
    // Two equal halves that fill the pane whatever the content width; each
    // half clips its own lines (design: `.sbs { overflow: hidden }`).
    px(pane)
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
fn muted_note(label: SharedString, colors: ThemeColors) -> Div {
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
fn meta_row(label: SharedString, value: impl IntoElement, colors: ThemeColors) -> Div {
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

/// One row of the sidebar's view nav: icon + label, bold with an accent bar
/// when active, and an optional trailing element (the change counters).
#[allow(clippy::too_many_arguments)]
fn view_nav_row(
    id: &'static str,
    icon: IconName,
    label: SharedString,
    active: bool,
    tooltip: Option<Tooltip>,
    trailing: Option<AnyElement>,
    colors: ThemeColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .relative()
        .w_full()
        .h(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .rounded(R_SM)
        .cursor_pointer()
        .text_size(FS_SM)
        .text_color(if active { colors.text } else { colors.muted })
        .when(active, |row| {
            row.font_weight(FontWeight::BOLD).child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(5.))
                    .bottom(px(5.))
                    .w(px(2.))
                    .rounded_full()
                    .bg(colors.accent),
            )
        })
        .hover(move |style| style.bg(colors.hover))
        .on_click(on_click)
        .child(
            Icon::new(icon)
                .size(px(14.))
                .color(if active { colors.text } else { colors.muted }),
        )
        .child(div().flex_1().min_w_0().truncate().child(label));
    if let Some(trailing) = trailing {
        row = row.child(trailing);
    }
    if let Some(tooltip) = tooltip {
        row = row.tooltip(move |_, cx| cx.new(|_| tooltip.clone()).into());
    }
    row
}

/// The toolbar segment that shows `view`: History, Changes, Worktrees.
fn segment_for_view(view: ViewMode) -> usize {
    match view {
        ViewMode::History => 0,
        ViewMode::Changes => 1,
        ViewMode::Worktrees => 2,
    }
}

/// The view the toolbar segment at `index` opens.
fn view_for_segment(index: usize) -> ViewMode {
    match index {
        1 => ViewMode::Changes,
        2 => ViewMode::Worktrees,
        _ => ViewMode::History,
    }
}

/// Runs one item of a worktree's context menu.
fn run_worktree_action(
    action: WorktreeAction,
    path: &Path,
    model: &Entity<WorktreesModel>,
    view: &Entity<RepositoryView>,
    window: &mut Window,
    cx: &mut App,
) {
    match action {
        WorktreeAction::Switch => model.update(cx, |model, cx| model.switch_to(path, window, cx)),
        WorktreeAction::OpenInWindow => {
            model.update(cx, |model, cx| model.open_in_window(path, window, cx))
        }
        WorktreeAction::Reveal => model.update(cx, |model, cx| model.reveal(path, window, cx)),
        WorktreeAction::CopyPath => model.update(cx, |model, cx| model.copy_path(path, window, cx)),
        WorktreeAction::ToggleLock => {
            model.update(cx, |model, cx| model.toggle_lock(path, window, cx))
        }
        WorktreeAction::Remove => view.update(cx, |view, cx| {
            view.worktrees
                .update(cx, |worktrees, cx| worktrees.open_remove(path, window, cx));
        }),
    }
}

/// A click, Enter/Space or ←/→ on an expandable row flips its expansion.
fn node_toggler(
    rows: Vec<(SharedString, bool, bool)>,
    view: Entity<RepositoryView>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    move |index, window, cx| {
        let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
            return;
        };
        if !has_children {
            return;
        }
        view.update(cx, |view, cx| view.toggle_node(&key, expanded, window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn full_message_text_is_the_raw_message() {
        let details = CommitDetails {
            id: Oid::from_hex(&"a".repeat(40)).unwrap(),
            parents: Vec::new(),
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            committer_name: String::new(),
            committer_email: String::new(),
            committer_time: 0,
            // `%s` joins the wrapped first paragraph into one line.
            summary: "A long subject wrapped over two lines".to_owned(),
            body: "Body".to_owned(),
            message: "A long subject wrapped\nover two lines\n\nBody".to_owned(),
        };
        assert_eq!(
            full_message_text(&details),
            "A long subject wrapped\nover two lines\n\nBody"
        );
    }

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
        // measurement is 51px for "gitwarp" (7.3px/char). Wide (CJK) characters
        // take 13px.
        fn text(label: &str) -> f32 {
            label
                .chars()
                .map(|c| match c as u32 {
                    0..=0x7f => 8.,
                    // Accented Latin and Cyrillic run a little wider than ASCII.
                    0x80..=0x2e7f => 9.,
                    _ => 13.,
                })
                .sum()
        }
        // Every item at its worst case, in the same terms as the toolbar;
        // `labels_width` is the summed width of the seven toolbar labels.
        fn needed(layout: ToolbarLayout, labels_width: f32) -> f32 {
            let name_item = |name: Option<f32>| {
                let name = name.unwrap_or(0.);
                2. * layout.item_pad + 14. + 6. + name + 6. + 12.
            };
            // Button: 8px padding each side, 14px icon, 6px gap; the segment adds
            // 10px padding each side, 14px icon, 6px gap and a 1px border.
            let labelled = 4. * (16. + 14. + 6.) + 3. * (20. + 14. + 6.) + 2.;
            let controls = if layout.labels {
                labelled + labels_width
            } else {
                // Icon-only tool buttons are 28 wide (`Button` squares them), and
                // three 34px icon segments plus the border.
                112. + 104.
            };
            // The worktree button: icon, name, the warning dot (6px + gap),
            // chevron.
            let worktree_item = if layout.worktree_button {
                6. + name_item(layout.worktree_name) + layout.gap
            } else {
                0.
            };
            let fixed = 26. // sidebar toggle
                + name_item(layout.repo_name)
                + name_item(layout.branch_name)
                + worktree_item
                + 9. // separator
                + controls
                + 26. // settings
                + 104.; // window controls
            fixed + layout.search_min + 2. * layout.padding + 12. * layout.gap
        }
        let labels_width = |lang: &str| -> f32 {
            TOOLBAR_LABELS
                .iter()
                .map(|key| text(&ui::i18n::t_in(lang, key)))
                .sum()
        };

        // Every language, not just English: `t_in` reads the catalogs without
        // touching the process-wide active language.
        for lang in ui::i18n::LANGUAGES {
            let extra = label_extra_width(lang, text);
            let labels = labels_width(lang);
            for width in [
                620., 699., 700., 760., 839., 840., 869., 870., 1000., 1200., 1340., 1419., 1420.,
                1459., 1460., 1579., 1580., 1600., 1700., 1800., 2000.,
            ] {
                let layout = toolbar_layout(width, extra);
                // The client-side frame keeps a 10px shadow band and a 1px border
                // on each side.
                let content = width - 22.;
                assert!(
                    needed(layout, labels) <= content,
                    "{lang}: toolbar overflows at {width}: needs {} of {content}",
                    needed(layout, labels)
                );
            }
        }
    }

    #[test]
    fn longer_labels_move_the_labelled_steps_up_and_english_keeps_its_own() {
        assert_eq!(label_extra_width("en", |t| t.chars().count() as f32), 0.);
        let extra = label_extra_width("fr", |t| t.chars().count() as f32);
        assert!(extra > 0.);
        assert!(toolbar_layout(1460., 0.).labels);
        assert!(!toolbar_layout(1460., extra).labels);
        assert!(toolbar_layout(1460. + extra, extra).labels);
        assert_eq!(toolbar_layout(1580., 0.).repo_name, Some(140.));
        assert_eq!(toolbar_layout(1580., extra).repo_name, Some(96.));
    }

    #[test]
    fn toolbar_steps_hide_labels_and_names_in_order() {
        assert!(toolbar_layout(1600., 0.).labels);
        assert!(toolbar_layout(1460., 0.).labels);
        assert!(!toolbar_layout(1459., 0.).labels);
        assert_eq!(toolbar_layout(1579., 0.).repo_name, Some(96.));
        assert!(!toolbar_layout(1200., 0.).labels);
        assert_eq!(toolbar_layout(1600., 0.).repo_name, Some(140.));
        assert_eq!(toolbar_layout(1200., 0.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(1080., 0.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(1000., 0.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(900., 0.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(899., 0.).repo_name, None);
        assert_eq!(toolbar_layout(810., 0.).repo_name, None);
        assert_eq!(toolbar_layout(810., 0.).branch_name, Some(64.));
        assert_eq!(toolbar_layout(809., 0.).branch_name, None);
        assert_eq!(toolbar_layout(650., 0.).search_min, 60.);
        assert_eq!(toolbar_layout(650., 0.).padding, 6.);
        assert_eq!(toolbar_layout(760., 0.).search_min, 96.);
        assert_eq!(toolbar_layout(1200., 0.).search_min, 150.);
        assert!(!toolbar_layout(900., 0.).search_kbd);
        assert!(!toolbar_layout(1200., 0.).search_kbd);
        assert!(toolbar_layout(1600., 0.).search_kbd);
        assert_eq!(toolbar_layout(1200., 0.).gap, 6.);
        assert_eq!(toolbar_layout(800., 0.).gap, 4.);
        assert_eq!(toolbar_layout(1200., 0.).item_pad, 8.);
        assert_eq!(toolbar_layout(800., 0.).item_pad, 7.);
    }

    #[test]
    fn the_worktree_button_loses_its_name_then_the_button_as_the_bar_narrows() {
        assert_eq!(toolbar_layout(1600., 0.).worktree_name, Some(96.));
        assert_eq!(toolbar_layout(1200., 0.).worktree_name, Some(64.));
        assert_eq!(toolbar_layout(899., 0.).worktree_name, None);
        assert!(toolbar_layout(740., 0.).worktree_button);
        assert!(!toolbar_layout(739., 0.).worktree_button);
        assert!(!toolbar_layout(620., 0.).worktree_button);
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
    fn the_bottom_dock_is_capped_to_half_the_window_height() {
        // A stored 560 in an 800 px window renders at 400.
        assert_eq!(effective_details_height(560., 800.), 400.);
        // A tall window keeps the stored size, within the 220–560 range.
        assert_eq!(effective_details_height(300., 1386.), 300.);
        assert_eq!(effective_details_height(900., 1386.), 560.);
        // Never below the minimum.
        assert_eq!(effective_details_height(560., 300.), 220.);
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

    /// The sidebar carries the three view rows; collapsing it moves the
    /// switcher into the titlebar, beside the sidebar toggle.
    #[gpui::test]
    fn the_view_nav_lives_at_the_top_of_the_sidebar(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("view-nav");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        for id in ["view-nav-history", "view-nav-changes", "view-nav-worktrees"] {
            assert!(cx.debug_bounds(id).is_some(), "{id} was not drawn");
        }
        assert!(cx.debug_bounds("view-switch-segment-0").is_none());

        cx.update(|_, cx| view.update(cx, |view, cx| view.toggle_sidebar(cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("view-nav-history").is_none());
        assert!(cx.debug_bounds("view-switch-segment-0").is_some());
    }

    /// The Changes row shows a coloured counter per changed path, and the
    /// counters update when the status reloads.
    #[gpui::test]
    fn the_changes_row_counts_the_changed_paths(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("view-nav-counts");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        std::fs::write(repo.path().join("new.txt"), "new\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("view-nav-count-added").is_some());
        assert!(cx.debug_bounds("view-nav-count-modified").is_some());
        assert!(cx.debug_bounds("view-nav-count-deleted").is_none());

        std::fs::remove_file(repo.path().join("new.txt")).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.changes.update(cx, |changes, cx| changes.refresh(cx));
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("view-nav-count-added").is_none());
        assert!(cx.debug_bounds("view-nav-count-modified").is_some());
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
            cx.debug_bounds("worktree-detail").is_some(),
            "the current worktree's details were not drawn"
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

    /// A repository whose `origin` is on a service with an account gets the
    /// Repository / PRs tabs; the PRs tab draws the (cached) requests, and
    /// "Create pull request…" opens the dialog on the current branch.
    #[gpui::test]
    fn a_remote_with_an_account_gets_the_pull_request_panel(cx: &mut gpui::TestAppContext) {
        use forge::api::{PrState, PullRequest, ReviewState};

        let repo = crate::test_support::TempRepo::new("pr-panel");
        commit(repo.path(), "a.txt", "one\n");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .expect("git runs");
            assert!(out.status.success());
        };
        git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
        let mut settings = crate::settings::Settings::default();
        settings.accounts.push(crate::settings::AccountRecord {
            id: "a1".to_owned(),
            kind: "github".to_owned(),
            base_url: "https://github.com".to_owned(),
            login: "mira-k".to_owned(),
            label: "mira-k".to_owned(),
            method: crate::settings::AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        });
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings,
                save_in_flight: false,
                dirty: false,
            });
        });
        // A fresh list in the shared cache: the panel needs no network.
        let key = "https://github.com|o/r|a1";
        let listing = |numbers: &[u64]| crate::pr_logic::Listing {
            items: numbers
                .iter()
                .map(|&number| PullRequest {
                    number,
                    title: format!("PR {number}"),
                    state: PrState::Open,
                    review: ReviewState::None,
                    author: "mira-k".to_owned(),
                    head_branch: format!("feat/{number}"),
                    head_repo_full_name: None,
                    base_branch: "main".to_owned(),
                    updated_at: 0,
                    web_url: String::new(),
                })
                .collect(),
            has_next: false,
            fetched: std::time::Instant::now(),
        };
        {
            let mut cache = crate::pr_logic::cache().lock().unwrap();
            cache.put(key, crate::pr_logic::PrTab::Mine, listing(&[1, 2]));
            cache.put(key, crate::pr_logic::PrTab::All, listing(&[1, 2, 3]));
        }
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        // The tabs exist, the tree is still shown.
        assert!(cx.debug_bounds("sidebar-tabs-segment-0").is_some());
        assert!(cx.debug_bounds("sidebar-tree-node-0").is_some());
        view.update(&mut cx, |view, cx| {
            view.set_sidebar_tab(SidebarTab::PullRequests, cx)
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            let model = view.pr_model.read(cx);
            assert!(model.has_target());
            assert_eq!(model.open_count(), Some(3));
        });
        assert!(cx.debug_bounds("pr-filter-segment-0").is_some());
        assert!(cx.debug_bounds("sidebar-tree-node-0").is_none());

        // "Create pull request…" on the current branch.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.open_create_pr(None, window, cx));
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert!(view.pr_dialog.is_some()));

        // Closing the dialog (here by its own close) clears the view's handle.
        let dialog = view.read_with(&cx, |view, _| view.pr_dialog.clone().unwrap());
        cx.update(|window, cx| dialog.update(cx, |dialog, cx| dialog.close(window, cx)));
        drop(dialog);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert!(view.pr_dialog.is_none()));
    }

    /// Without an account there is nothing to list or create: no tabs.
    #[gpui::test]
    fn a_remote_without_an_account_has_no_pull_request_tabs(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("pr-no-account");
        commit(repo.path(), "a.txt", "one\n");
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["remote", "add", "origin", "https://github.com/o/r.git"])
            .output()
            .expect("git runs");
        assert!(out.status.success());
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
        assert!(cx.debug_bounds("sidebar-tabs-segment-0").is_none());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.open_create_pr(None, window, cx));
        });
        view.read_with(&cx, |view, _| assert!(view.pr_dialog.is_none()));
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

    /// A checkout-like ref operation re-reads HEAD and the refs without
    /// walking the log: the rows, the selection and the scroll stay, and only
    /// the HEAD badge moves.
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
        let history = view.read_with(&cx, |view, _| view.history());
        let (rows, selected, generation, meta_version) = history.read_with(&cx, |history, _| {
            assert!(matches!(history.state(), LoadState::Loaded));
            assert_eq!(
                history.selected_oid(),
                Some(oid),
                "the HEAD row is not selected"
            );
            (
                history.commit_count(),
                history.selected_row(),
                history.load_generation(),
                history.meta_version(),
            )
        });
        assert_eq!(
            history.read_with(&cx, |history, _| history.head_badge(oid).map(str::to_owned)),
            Some("main".to_owned())
        );

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
        cx.update(crate::confirm_dialog::confirm_last_for_test);
        cx.run_until_parked();
        cx.run_until_parked();

        let repo = git_core::Repo::open(crate::test_support::git(), repo.path()).unwrap();
        match repo.head().unwrap() {
            Head::Branch { name, .. } => assert_eq!(name, "feature"),
            Head::Detached(_) => panic!("checkout left a detached HEAD"),
        }
        history.read_with(&cx, |history, _| {
            assert!(
                matches!(history.state(), LoadState::Loaded),
                "the checkout restarted the load: {:?}",
                history.state()
            );
            assert_eq!(
                history.commit_count(),
                rows,
                "the checkout changed the rows"
            );
            assert_eq!(
                history.selected_row(),
                selected,
                "the checkout lost the selection"
            );
            assert_eq!(history.selected_oid(), Some(oid));
            assert_eq!(
                history.load_generation(),
                generation,
                "the checkout walked the log again"
            );
            assert!(
                history.meta_version() > meta_version,
                "the checkout did not re-read HEAD and the refs"
            );
            assert_eq!(
                history.head_badge(oid),
                Some("feature"),
                "the HEAD badge did not move to the checked-out branch"
            );
        });
    }

    /// Every ref dialog is classified deliberately: checkout-like ones refresh
    /// the refs, the rest keep the full history walk.
    #[test]
    fn only_ref_moving_dialogs_skip_the_history_walk() {
        let card = CommitCard::bare(Oid::from_hex(&"a".repeat(40)).unwrap());
        let cases = [
            (
                RefDialogKind::CreateBranch { card: card.clone() },
                RefChange::RefsOnly,
            ),
            (
                RefDialogKind::CreateTag { card: card.clone() },
                RefChange::RefsOnly,
            ),
            (
                RefDialogKind::RenameBranch {
                    old: "old".to_owned(),
                },
                RefChange::RefsOnly,
            ),
            (
                RefDialogKind::RenameTag {
                    old: "v1".to_owned(),
                },
                RefChange::History,
            ),
            (RefDialogKind::Remote { editing: None }, RefChange::History),
            (
                RefDialogKind::Reset { card: card.clone() },
                RefChange::History,
            ),
            (
                RefDialogKind::DeleteBranch {
                    name: "gone".to_owned(),
                    force: false,
                },
                RefChange::History,
            ),
            (
                RefDialogKind::DeleteTag {
                    name: "v1".to_owned(),
                },
                RefChange::History,
            ),
            (
                RefDialogKind::RemoveRemote {
                    name: "origin".to_owned(),
                },
                RefChange::History,
            ),
            (
                RefDialogKind::DropStash {
                    index: 0,
                    label: "stash@{0}".to_owned(),
                },
                RefChange::History,
            ),
            (
                RefDialogKind::CherryPick { card: card.clone() },
                RefChange::History,
            ),
            (RefDialogKind::Revert { card }, RefChange::History),
            (
                RefDialogKind::RenameWorktree {
                    from: PathBuf::from("/tmp/wt"),
                },
                RefChange::History,
            ),
        ];
        for (kind, expected) in cases {
            assert_eq!(ref_change_for_dialog(&kind), expected, "{kind:?}");
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
                view.tree_nodes(&RemoteDisplays::new(), cx)
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
        cx.update(crate::confirm_dialog::confirm_last_for_test);
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

    /// When the window is too narrow for the whole grid it scrolls, and it
    /// opens (and stays after a resize) at the newest week, not the oldest.
    #[gpui::test]
    fn the_activity_grid_opens_at_the_newest_week(cx: &mut gpui::TestAppContext) {
        let (repo, _day) = day_filter_repo("strip-scroll-end");
        let (cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        let strip = view.read_with(&cx, |view, _| view.activity.clone());
        let scroll = strip.read_with(&cx, |strip, _| strip.scroll_handle());

        cx.simulate_resize(gpui::size(px(560.), px(900.)));
        cx.run_until_parked();
        let max = scroll.max_offset().x;
        assert!(max > px(0.), "the grid should overflow at 560 px");
        assert_eq!(scroll.offset().x, -max, "scroll starts at the newest week");

        // A resize keeps the end while the user did not scroll away.
        cx.simulate_resize(gpui::size(px(480.), px(900.)));
        cx.run_until_parked();
        assert_eq!(scroll.offset().x, -scroll.max_offset().x);
    }

    /// Enter on a keyboard-selected day filters History; Esc then clears the
    /// filter and the heatmap keeps the focus (a second Enter picks again).
    #[gpui::test]
    fn escape_after_a_keyboard_pick_clears_the_filter_and_keeps_the_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (repo, day) = day_filter_repo("strip-esc");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        let (strip, history) =
            view.read_with(&cx, |view, _| (view.activity.clone(), view.history.clone()));
        let today = strip.read_with(&cx, |strip, _| strip.today());

        // Tab to the grid: its keyboard ring (and tooltip) shows once it has
        // the focus.
        for _ in 0..60 {
            if cx.debug_bounds("activity-heatmap-tip").is_some() {
                break;
            }
            cx.simulate_keystrokes("tab");
            cx.run_until_parked();
        }
        assert!(
            cx.debug_bounds("activity-heatmap-tip").is_some(),
            "Tab never reached the heatmap"
        );

        // End jumps to today; ← steps a week back, ↑ a day back.
        let back = today - day;
        cx.simulate_keystrokes("end");
        for _ in 0..back / 7 {
            cx.simulate_keystrokes("left");
        }
        for _ in 0..back % 7 {
            cx.simulate_keystrokes("up");
        }
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert_eq!(history.day_filter().expect("Enter picked the day").day, day);
        });

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert!(history.day_filter().is_none(), "Esc left the filter on");
        });

        // The focus stayed on the grid: its ring is still drawn, and Enter
        // picks the same day again.
        assert!(
            cx.debug_bounds("activity-heatmap-tip").is_some(),
            "the heatmap lost the focus after Esc"
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        history.read_with(&cx, |history, _| {
            assert!(
                history.day_filter().is_some(),
                "the heatmap lost the focus after Esc"
            );
        });
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

    #[test]
    fn the_view_segment_has_a_third_tab_for_worktrees() {
        for (index, view) in [
            (0, ViewMode::History),
            (1, ViewMode::Changes),
            (2, ViewMode::Worktrees),
        ] {
            assert_eq!(view_for_segment(index), view);
            assert_eq!(segment_for_view(view), index);
        }
    }

    /// A repository with a linked worktree on branch `extra`, opened in a real
    /// window. Returns the folder of the linked worktree.
    fn open_with_linked_worktree(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (
        gpui::VisualTestContext,
        Entity<Workspace>,
        Entity<RepositoryView>,
        PathBuf,
    ) {
        commit(repo.path(), "a.txt", "one\n");
        let git = crate::test_support::git();
        let linked = repo.path().parent().unwrap().join(format!(
            "{}-linked",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&linked);
        let opened = Repo::open(git.clone(), repo.path()).expect("open the repository");
        let tip = match opened.head().expect("read HEAD") {
            Head::Branch {
                target: Some(oid), ..
            }
            | Head::Detached(oid) => oid,
            Head::Branch { target: None, .. } => panic!("no commit"),
        };
        opened
            .create_branch("extra", tip, false)
            .expect("create the branch");
        opened
            .add_worktree(&linked, "extra")
            .expect("add the worktree");

        let (mut cx, workspace) = crate::test_support::open_workspace(git, cx);
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx)
            });
        });
        cx.run_until_parked();
        let view = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .expect("the repository is open")
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_view(ViewMode::Worktrees, window, cx)
            });
        });
        cx.run_until_parked();
        (cx, workspace, view, linked)
    }

    /// The Worktrees view replaces the sidebar tree with the worktree panel;
    /// History brings the tree back.
    #[gpui::test]
    fn the_worktrees_view_swaps_the_sidebar_tree_for_the_panel(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-panel-swap");
        let (mut cx, _workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        assert!(cx.debug_bounds("worktree-panel").is_some());
        assert!(cx.debug_bounds("sidebar-tree-node-0").is_none());
        // The main worktree is pinned first; the linked one follows.
        assert!(cx.debug_bounds("worktree-row-0").is_some());
        assert!(cx.debug_bounds("worktree-row-1").is_some());
        assert!(cx.debug_bounds("worktree-row-2").is_none());

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::History, window, cx));
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("worktree-panel").is_none());
        assert!(cx.debug_bounds("sidebar-tree-node-0").is_some());
        let _ = std::fs::remove_dir_all(linked);
    }

    /// ↑/↓ move and select, Enter switches the window to the selected
    /// worktree.
    #[gpui::test]
    fn arrows_select_and_enter_switches_the_worktree(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-panel-enter");
        let (mut cx, workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        let model = view.read_with(&cx, |view, _| view.worktrees_model());
        let selected_branch = |cx: &gpui::VisualTestContext| {
            model.read_with(cx, |model, _| {
                model.selected().and_then(|e| e.branch()).map(str::to_owned)
            })
        };
        let focus = view.read_with(&cx, |view, _| view.worktree_panel_focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        assert_eq!(selected_branch(&cx).as_deref(), Some("main"));

        cx.simulate_keystrokes("down");
        assert_eq!(selected_branch(&cx).as_deref(), Some("extra"));
        cx.simulate_keystrokes("down");
        assert_eq!(
            selected_branch(&cx).as_deref(),
            Some("extra"),
            "stays on the last row"
        );
        cx.simulate_keystrokes("up");
        assert_eq!(selected_branch(&cx).as_deref(), Some("main"));

        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        let opened = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .expect("a repository is open")
        });
        let root = opened.read_with(&cx, |view, cx| {
            view.worktrees_model().read(cx).root().to_path_buf()
        });
        assert_eq!(root.canonicalize().unwrap(), linked.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(linked);
    }

    /// The worktree panel is a Tab stop, reachable like the branch tree.
    #[gpui::test]
    fn the_worktree_panel_is_reachable_with_tab(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-panel-tab");
        let (mut cx, _workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        let focus = view.read_with(&cx, |view, _| view.worktree_panel_focus.clone());
        let mut reached = false;
        for _ in 0..40 {
            cx.simulate_keystrokes("tab");
            if cx.update(|window, _| focus.is_focused(window)) {
                reached = true;
                break;
            }
        }
        assert!(reached, "Tab never reached the worktree panel");
        let _ = std::fs::remove_dir_all(linked);
    }

    /// The Remove dialog of a dirty worktree has a disabled primary button;
    /// it must still own the keyboard: Esc closes it.
    #[gpui::test]
    fn escape_closes_the_remove_dialog_of_a_dirty_worktree(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-remove-esc");
        let (mut cx, _workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        std::fs::write(linked.join("dirty.txt"), "uncommitted").expect("dirty the worktree");
        let model = view.read_with(&cx, |view, _| view.worktrees_model());
        cx.update(|_, cx| model.update(cx, |model, cx| model.reload(cx)));
        cx.run_until_parked();
        let layers = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| ui::overlay::layer_count(window, cx))
        };
        let focus = view.read_with(&cx, |view, _| view.worktree_panel_focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));

        cx.simulate_keystrokes("down delete");
        cx.run_until_parked();
        assert_eq!(
            layers(&mut cx),
            1,
            "Remove should open for a linked worktree"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(layers(&mut cx), 0, "Esc did not close the Remove dialog");
        let _ = std::fs::remove_dir_all(linked);
    }

    /// `git worktree remove` can fail after the fact (here: the folder turned
    /// dirty while the dialog was open): the list is reloaded anyway, so it no
    /// longer shows the stale state. Enter on the focused primary button runs it.
    #[gpui::test]
    fn a_failed_remove_still_reloads_the_worktree_list(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-remove-fails");
        let (mut cx, _workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        let model = view.read_with(&cx, |view, _| view.worktrees_model());
        let focus = view.read_with(&cx, |view, _| view.worktree_panel_focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("down delete");
        cx.run_until_parked();
        assert!(view.read_with(&cx, |view, cx| view.worktrees.read(cx).has_dialog()));
        let dirty = |cx: &gpui::VisualTestContext| {
            // Compared canonically: on macOS git reports the temp folder as /private/var/...
            let linked = linked
                .canonicalize()
                .expect("canonicalize the worktree path");
            model.read_with(cx, |model, _| {
                model
                    .entries()
                    .iter()
                    .find(|entry| entry.path().canonicalize().ok().as_deref() == Some(&*linked))
                    .map(|entry| entry.changes.len())
            })
        };
        assert_eq!(dirty(&cx), Some(0), "the worktree starts clean");

        // An untracked file makes plain `git worktree remove` refuse.
        std::fs::write(linked.join("late.txt"), "late").expect("dirty the worktree");
        // Tab: delete-branch box, Cancel, Remove. Enter runs the focused button.
        cx.simulate_keystrokes("tab tab tab enter");
        cx.run_until_parked();
        assert_eq!(
            dirty(&cx),
            Some(1),
            "the list was not reloaded after the failed remove"
        );
        assert!(view.read_with(&cx, |view, cx| view.worktrees.read(cx).has_dialog()));
        let _ = std::fs::remove_dir_all(linked);
    }

    /// Del opens the Remove dialog for a linked worktree and does nothing on
    /// the main one.
    #[gpui::test]
    fn delete_opens_remove_only_for_a_non_main_worktree(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("worktree-panel-delete");
        let (mut cx, _workspace, view, linked) = open_with_linked_worktree(&repo, cx);
        let model = view.read_with(&cx, |view, _| view.worktrees_model());
        let focus = view.read_with(&cx, |view, _| view.worktree_panel_focus.clone());
        let has_dialog = |cx: &gpui::VisualTestContext| {
            view.read_with(cx, |view, cx| view.worktrees.read(cx).has_dialog())
        };
        cx.update(|window, cx| window.focus(&focus, cx));

        // The main worktree is selected first.
        cx.simulate_keystrokes("delete");
        cx.run_until_parked();
        assert!(!has_dialog(&cx), "the main worktree cannot be removed");

        cx.simulate_keystrokes("down delete");
        cx.run_until_parked();
        model.read_with(&cx, |model, _| {
            assert_eq!(model.selected().and_then(|e| e.branch()), Some("extra"));
        });
        assert!(has_dialog(&cx), "Remove should open for a linked worktree");
        let _ = std::fs::remove_dir_all(linked);
    }

    #[test]
    fn push_after_pull_belongs_only_to_that_pull() {
        assert!(follow_with_push(&Job::Pull, true));
        assert!(!follow_with_push(&Job::Pull, false));
        // Any later job, including a plain pull, must not inherit the intent.
        assert!(!follow_with_push(&Job::Push { branch: None }, true));
        assert!(!follow_with_push(
            &Job::Fetch(FetchOptions::default()),
            true
        ));
    }

    #[test]
    fn only_a_push_of_the_checked_out_branch_offers_pull_then_push() {
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let push = |branch: Option<&str>| Job::Push {
            branch: branch.map(str::to_owned),
        };
        // The current branch by name, and via the toolbar (`None`): offered.
        for job in [push(Some("main")), push(None)] {
            assert_eq!(
                error_dialog_remedy(&KnownError::PushRejected, &job, Some(&head)),
                Some(true)
            );
        }
        // Another branch, or no branch checked out: the pull would not help.
        assert_eq!(
            error_dialog_remedy(&KnownError::PushRejected, &push(Some("feat")), Some(&head)),
            Some(false)
        );
        assert_eq!(
            error_dialog_remedy(&KnownError::PushRejected, &push(Some("main")), None),
            Some(false)
        );
        assert_eq!(
            error_dialog_remedy(
                &KnownError::PushRejected,
                &Job::Push { branch: None },
                Some(&head)
            ),
            Some(true)
        );
        for job in [
            Job::PushTag {
                remote: "origin".into(),
                tag: "v1".into(),
            },
            Job::PushTags {
                remote: "origin".into(),
            },
            Job::DeleteRemoteBranch {
                remote: "origin".into(),
                branch: "topic".into(),
            },
        ] {
            assert_eq!(
                error_dialog_remedy(&KnownError::PushRejected, &job, Some(&head)),
                Some(false),
                "{job:?} must show the dialog without the pull-then-push remedy"
            );
        }
        assert_eq!(
            error_dialog_remedy(&KnownError::PullNotFastForward, &Job::Pull, Some(&head)),
            Some(false)
        );
        assert_eq!(
            error_dialog_remedy(
                &KnownError::PullNotFastForward,
                &Job::Push { branch: None },
                Some(&head)
            ),
            None
        );
        assert_eq!(
            error_dialog_remedy(
                &KnownError::LocalChangesOverwritten,
                &Job::Pull,
                Some(&head)
            ),
            None
        );
    }

    fn checkout_failed() -> Error {
        Error::Git {
            command: "checkout",
            code: None,
            stderr: "fatal: checkout refused".into(),
        }
    }

    #[test]
    fn a_failed_switch_restores_the_stashed_changes() {
        let tmp = crate::test_support::TempRepo::new("stash-switch");
        commit(tmp.path(), "seed.txt", "init");
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, "unsaved").unwrap();
        let repo = git_core::Repo::open(crate::test_support::git(), tmp.path()).unwrap();

        let result = stash_then_checkout(&repo, |_| Err(checkout_failed()));
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "unsaved");
        assert!(
            repo.stashes().unwrap().is_empty(),
            "the stash was not popped after the failed switch"
        );
    }

    #[test]
    fn a_failed_restore_says_the_changes_are_in_the_stash() {
        let tmp = crate::test_support::TempRepo::new("stash-switch-pop-fails");
        commit(tmp.path(), "seed.txt", "init");
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, "unsaved").unwrap();
        let repo = git_core::Repo::open(crate::test_support::git(), tmp.path()).unwrap();

        let result = stash_then_checkout(&repo, |_| {
            // Recreate the stashed file with other content: popping would
            // overwrite it, so git refuses.
            std::fs::write(&file, "changed again").unwrap();
            Err(checkout_failed())
        });
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains(SWITCH_STASH),
            "the error does not name the stash: {message}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed again");
        assert_eq!(repo.stashes().unwrap().len(), 1);
    }

    fn fp(head: &str, refs: &[(&str, u8)], index: u64, markers: u16) -> Fingerprint {
        let oid = |n: u8| Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap();
        Fingerprint {
            head: Some(Head::Branch {
                name: head.to_owned(),
                target: None,
            }),
            refs: refs
                .iter()
                .map(|(n, o)| ((*n).to_owned(), oid(*o)))
                .collect(),
            index: Some((SystemTime::UNIX_EPOCH, index)),
            markers,
        }
    }

    #[test]
    fn decide_reload_compares_fingerprints() {
        let base = fp("main", &[("refs/heads/main", 1)], 10, 0);
        let none = ReloadDecision::default();
        // No baseline yet, or nothing changed: nothing to reload.
        assert_eq!(decide_reload(None, &base), none);
        assert_eq!(decide_reload(Some(&base), &base.clone()), none);
        // Only the index changed (an external `git add`): just Changes.
        assert_eq!(
            decide_reload(Some(&base), &fp("main", &[("refs/heads/main", 1)], 11, 0)),
            ReloadDecision {
                refs: None,
                changes: true
            }
        );
        // Only HEAD moved (checkout of an existing branch): refs, not the log.
        assert_eq!(
            decide_reload(Some(&base), &fp("dev", &[("refs/heads/main", 1)], 10, 0)),
            ReloadDecision {
                refs: Some(RefChange::RefsOnly),
                changes: true
            }
        );
        // An operation started or finished.
        assert_eq!(
            decide_reload(Some(&base), &fp("main", &[("refs/heads/main", 1)], 10, 1)),
            ReloadDecision {
                refs: Some(RefChange::RefsOnly),
                changes: true
            }
        );
        // A moved ref (external commit) and a new ref: the history.
        for refs in [
            &[("refs/heads/main", 2)][..],
            &[("refs/heads/main", 1), ("refs/tags/v1", 1)],
        ] {
            assert_eq!(
                decide_reload(Some(&base), &fp("main", refs, 10, 0)),
                ReloadDecision {
                    refs: Some(RefChange::History),
                    changes: true
                }
            );
        }
    }

    #[test]
    fn fetch_is_due_after_a_minute() {
        let now = Instant::now();
        let min = Duration::from_secs(60);
        assert!(fetch_due(None, now, min));
        assert!(!fetch_due(Some(now), now + Duration::from_secs(59), min));
        assert!(fetch_due(Some(now), now + min, min));
    }

    /// Activation ticks a fetch only when the background fetch is on and the
    /// last fetch is old enough. The test repository has no remote, so the
    /// fetch itself does nothing and touches no network.
    #[gpui::test]
    fn activation_fetches_only_when_due_and_enabled(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("activation-fetch");
        commit(repo.path(), "a.txt", "one\n");
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings {
                    background_fetch: false,
                    ..crate::settings::Settings::default()
                },
                save_in_flight: false,
                dirty: false,
            });
        });
        let (mut cx, view) = open_repo_view(&repo, cx);
        let activate = |cx: &mut gpui::VisualTestContext| {
            let ticked =
                cx.update(|_, cx| view.update(cx, |view, cx| view.fetch_on_activation(cx)));
            cx.run_until_parked();
            ticked
        };
        assert!(!activate(&mut cx), "disabled in the settings");
        cx.update(|_, cx| crate::update_settings(cx, |settings| settings.background_fetch = true));
        assert!(activate(&mut cx), "enabled and never fetched");
        assert!(view.read_with(&cx, |view, _| view.last_fetch.is_some()));
        assert!(!activate(&mut cx), "fetched a moment ago");
        view.update(&mut cx, |view, _| {
            view.last_fetch = Some(Instant::now() - Duration::from_secs(61));
        });
        assert!(activate(&mut cx), "the last fetch is over a minute old");
    }

    #[test]
    fn a_check_waits_for_an_in_flight_baseline_and_never_discards_it() {
        assert_eq!(check_start(true, true), CheckStart::Skip);
        assert_eq!(check_start(true, false), CheckStart::Skip);
        assert_eq!(check_start(false, true), CheckStart::Defer);
        assert_eq!(check_start(false, false), CheckStart::Run);
        // Newest check wins; one that overlapped a re-baseline is redone, not applied and
        // not dropped (dropping it would lose an outside change made meanwhile).
        assert_eq!(check_outcome(2, 3, 5, 5), CheckOutcome::Discard);
        assert_eq!(check_outcome(3, 3, 5, 6), CheckOutcome::Rerun);
        assert_eq!(check_outcome(3, 3, 5, 5), CheckOutcome::Apply);
    }

    #[test]
    fn throttle_helper_limits_to_once_per_interval() {
        let now = Instant::now();
        let min = Duration::from_secs(2);
        assert!(fetch_due(None, now, min));
        assert!(!fetch_due(
            Some(now),
            now + Duration::from_millis(1999),
            min
        ));
        assert!(fetch_due(Some(now), now + min, min));
    }

    /// Opens `repo` in the real window and waits for the history and the
    /// Changes status to load.
    fn open_repo_view(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, Entity<RepositoryView>) {
        let (mut cx, workspace) =
            crate::test_support::open_workspace(crate::test_support::git(), cx);
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx)
            });
        });
        cx.run_until_parked();
        let view = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .expect("the repository is open")
        });
        (cx, view)
    }

    /// Runs `git` in `repo`, panicking on failure.
    fn run_git(repo: &Path, args: &[&str]) {
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
    }

    /// A commit made in a terminal while the window is open shows up when the
    /// window becomes active again.
    #[gpui::test]
    fn an_external_commit_appears_on_window_activation(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("external-commit");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repo_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        assert_eq!(history.read_with(&cx, |h, _| h.commit_count()), 1);

        run_git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "two"]);
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_window_activated(window, cx)));
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |h, _| h.commit_count()),
            2,
            "the commit made outside the app did not appear after activation"
        );
    }

    /// A file staged in a terminal is counted by Changes after activation,
    /// without visiting the Changes tab.
    #[gpui::test]
    fn an_external_stage_appears_on_window_activation(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("external-stage");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repo_view(&repo, cx);
        let changes = view.read_with(&cx, |view, _| view.changes.clone());
        assert_eq!(changes.read_with(&cx, |c, _| c.staged_count()), 0);

        std::fs::write(repo.path().join("b.txt"), "new\n").unwrap();
        run_git(repo.path(), &["add", "b.txt"]);
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_window_activated(window, cx)));
        cx.run_until_parked();
        assert_eq!(
            changes.read_with(&cx, |c, _| c.staged_count()),
            1,
            "the file staged outside the app did not show up after activation"
        );
    }

    /// A watcher signal reloads the views after the merge delay.
    #[gpui::test]
    fn a_watch_signal_reloads_after_the_flush_delay(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("watch-signal");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repo_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        assert_eq!(history.read_with(&cx, |h, _| h.commit_count()), 1);

        run_git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "two"]);
        let signal = WatchSignal {
            kinds: Kinds::HEAD | Kinds::REFS,
            limit_hit: false,
        };
        view.update(&mut cx, |view, cx| view.on_watch_signal(signal, cx));
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |h, _| h.commit_count()),
            1,
            "the reload must wait for the merge delay"
        );
        cx.executor().advance_clock(WATCH_FLUSH * 2);
        cx.run_until_parked();
        assert_eq!(history.read_with(&cx, |h, _| h.commit_count()), 2);
    }

    /// An INDEX signal re-reads Changes directly: the fingerprint's index stamp
    /// is not asked, so a stamp that did not change cannot veto the refresh.
    #[gpui::test]
    fn an_index_signal_refreshes_changes_without_the_fingerprint(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("watch-index");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repo_view(&repo, cx);
        let changes = view.read_with(&cx, |view, _| view.changes.clone());

        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        run_git(repo.path(), &["add", "a.txt"]);
        view.update(&mut cx, |view, cx| {
            // Pretend the baseline already holds the new index stamp: the fingerprint
            // comparison alone would see no change.
            view.fingerprint = read_fingerprint(view.git.clone().unwrap(), &view.root);
            let signal = WatchSignal {
                kinds: Kinds::INDEX,
                limit_hit: false,
            };
            view.on_watch_signal(signal, cx);
        });
        cx.executor().advance_clock(WATCH_FLUSH * 2);
        cx.run_until_parked();
        assert_eq!(changes.read_with(&cx, |c, _| c.staged_count()), 1);
    }

    /// Signals are held while the app runs a ref action and applied after it.
    #[gpui::test]
    fn watch_signals_are_held_while_a_ref_action_runs(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("watch-held");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, view) = open_repo_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());

        run_git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "two"]);
        view.update(&mut cx, |view, cx| {
            view.ref_action_running = true;
            let signal = WatchSignal {
                kinds: Kinds::REFS,
                limit_hit: false,
            };
            view.on_watch_signal(signal, cx);
        });
        cx.executor().advance_clock(WATCH_FLUSH * 2);
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |h, _| h.commit_count()),
            1,
            "applied while a ref action was running"
        );
        assert!(!view.read_with(&cx, |view, _| view.watch_pending.is_empty()));

        view.update(&mut cx, |view, cx| {
            view.ref_action_running = false;
            view.retry_watch_flush(cx);
        });
        cx.executor().advance_clock(WATCH_FLUSH * 2);
        cx.run_until_parked();
        assert_eq!(history.read_with(&cx, |h, _| h.commit_count()), 2);
        assert!(view.read_with(&cx, |view, _| view.watch_pending.is_empty()));
    }

    /// Only an open repository is watched and fetched: once its window shows the welcome
    /// screen, the view (with its watcher, fetch loop and tasks) is dropped.
    #[gpui::test]
    fn a_repository_view_is_released_when_the_workspace_shows_welcome(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("release-welcome");
        commit(repo.path(), "a.txt", "one\n");
        let (mut cx, workspace) =
            crate::test_support::open_workspace(crate::test_support::git(), cx);
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx)
            });
        });
        cx.run_until_parked();
        let weak = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .expect("the repository is open")
                .downgrade()
        });
        assert!(weak.upgrade().is_some());

        cx.update(|window, cx| workspace.update(cx, |ws, cx| ws.show_welcome(window, cx)));
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "the repository view outlived the switch to Welcome"
        );
    }

    /// Opening another repository in the same window drops the previous view too.
    #[gpui::test]
    fn a_repository_view_is_released_when_another_repository_opens(cx: &mut gpui::TestAppContext) {
        let first = crate::test_support::TempRepo::new("release-first");
        commit(first.path(), "a.txt", "one\n");
        let second = crate::test_support::TempRepo::new("release-second");
        commit(second.path(), "b.txt", "two\n");
        let (mut cx, workspace) =
            crate::test_support::open_workspace(crate::test_support::git(), cx);
        let mut weak = None;
        for path in [first.path(), second.path()] {
            let path = path.to_path_buf();
            cx.update(|window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.open_repository(path, window, cx)
                });
            });
            cx.run_until_parked();
            if weak.is_none() {
                weak = cx.update(|_, cx| {
                    workspace
                        .read(cx)
                        .repository_for_test()
                        .map(|view| view.downgrade())
                });
            }
        }
        let weak = weak.expect("the first view");
        assert!(
            weak.upgrade().is_none(),
            "the first repository view outlived opening another repository"
        );
    }
}
