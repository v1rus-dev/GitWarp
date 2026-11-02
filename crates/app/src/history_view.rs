//! The history screen: column header + commit graph and commit list, loaded in
//! the background and painted through a virtualized list (only visible rows are
//! built).
//!
//! The view also owns the repository metadata the shell needs — HEAD and the
//! refs — because it is the view that opens the [`Repo`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::{
    actions, canvas, container_query, div, px, uniform_list, AnyElement, App, AsyncApp, ClickEvent,
    ClipboardItem, ContentMask, Context, Div, DragMoveEvent, EmptyView, FocusHandle, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, Render, ScrollStrategy,
    SharedString, StyledText, Task, TextRun, TextStyle, UniformListScrollHandle, WeakEntity,
    Window,
};

use git_core::{CommitInfo, Error, Git, Head, LogKiller, LogOptions, LogStream, Oid, Ref, Repo};
use git_graph::{ColumnGeometry, GraphBuilder, IdleLanes, Row};
use ui::actions::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::focus::{self, RingOffset};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_SM, FS_XS, R_MD};
use ui::theme::ThemeColors;
use ui::{
    menu, ActiveTheme, Avatar, Button, ButtonSize, EmptyState, Icon, IconName, ListRow, MenuEntry,
    MenuItem, Progress, RefBadge, RefKind, Skeleton, Spinner, ToastKind, Tooltip,
};

use crate::format::{format_count, relative_time};
use crate::graph_paint::paint_row;
use crate::history_search::{self, Query};
use crate::history_state::{
    percent_text, progress_fraction, skeleton_percent, source_label, Failure, LoadState,
    SkeletonColumn,
};
use crate::ref_ops::{CardRefKind, CommitCard, RefAction};

/// What the history row menu asks the shell to do.
pub(crate) type RefActionHandler = Rc<dyn Fn(RefAction, &mut Window, &mut App)>;
use crate::settings::{GRAPH_WIDTH_DEFAULT, GRAPH_WIDTH_MAX, GRAPH_WIDTH_MIN};
use crate::theme;
use crate::workspace::{OpenGitSettings, Workspace};

/// Rows are delivered to the UI in pages of this size.
const PAGE: usize = 2_000;

/// How long the field waits after the last keystroke before the search runs.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Column widths of the history table, shared by the header and the rows.
const COL_GAP: f32 = 18.;
/// Author and date columns on a wide pane.
const COL_AUTHOR: f32 = 130.;
const COL_DATE: f32 = 104.;
/// The narrower author and date columns below [`PANE_MEDIUM`].
const COL_AUTHOR_NARROW: f32 = 104.;
const COL_DATE_NARROW: f32 = 76.;
const COL_HASH: f32 = 64.;
/// The pane widths where the table drops a column, as the design's container
/// queries do: 560 px loses Hash, 460 px loses Author.
const PANE_MEDIUM: f32 = 560.;
const PANE_NARROW: f32 = 460.;
/// The graph column's design cap on a wide pane and on top of that its share
/// of the pane, so the summary always keeps room. The user width `gw` (40–480)
/// replaces the old lane auto-fit; the 480 cap stays a belt-and-braces bound.
const GRAPH_MAX_WIDE: f32 = GRAPH_WIDTH_MAX;
const GRAPH_SHARE_WIDE: f32 = 0.40;
const GRAPH_SHARE_MEDIUM: f32 = 0.40;
const GRAPH_SHARE_NARROW: f32 = 0.36;
/// The header and the rows share this side margin and padding.
const SIDE_MARGIN: f32 = 6.;
const PAD_LEFT: f32 = 14.;
const PAD_RIGHT: f32 = 18.;
/// The graph column never gets narrower than this, even with one lane.
const GRAPH_MIN: f32 = GRAPH_WIDTH_MIN;
/// Fitting the graph to its content never widens it past this, and leaves this
/// much room beside the lanes.
const GRAPH_FIT_MAX: f32 = 420.;
const GRAPH_FIT_PAD: f32 = 8.;
/// The summary column is flexible; its skeleton bars are percentages of this.
const SKELETON_SUMMARY_REF: f32 = 360.;
/// Skeleton rows shown until the first page of history arrives.
const SKELETON_ROWS: usize = 10;
/// The gap between a row's ref badges and the summary text (the summary cell's
/// flex gap).
const BADGE_GAP: f32 = 6.;
/// The widest a badge label may get; a longer ref name is ellipsized.
const BADGE_LABEL_MAX: f32 = 160.;
/// A badge's label is wrapped in 6 px padding and a 1 px border on each side;
/// a kind with an icon adds the 11 px icon and its 3 px gap.
const BADGE_PADDING: f32 = 14.;
const BADGE_ICON: f32 = 14.;
/// The narrowest room the summary text keeps beside its badges, and at most
/// 40 % of the cell when the cell is narrower than 200 px.
const BADGE_TEXT_MIN: f32 = 80.;
/// Relative dates are recomputed at this interval while the view is open.
const CLOCK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// The starter file the empty history's "Add .gitignore" writes.
const STARTER_GITIGNORE: &str = "# OS and editor files
.DS_Store
Thumbs.db
*.swp
.idea/
.vscode/
";

actions!(
    gitwarp,
    [
        NextCommit,
        PrevCommit,
        PageDownCommits,
        PageUpCommits,
        CopyCommitHash
    ]
);

/// How many rows Page Down / Page Up move: one viewport of rows minus the one
/// the previous page already showed, at least a single row.
pub fn page_rows(viewport_height: f32, row_height: f32) -> usize {
    let rows = (viewport_height / row_height.max(1.)).floor();
    (rows as usize).saturating_sub(1).max(1)
}

/// Binds the list's extra keys in its own `history-list` context, so they
/// fire only while the list has the keyboard: typing `j` in the search field
/// still types `j`. Called once at startup.
pub(crate) fn bind_history_keys(cx: &mut App, settings: &crate::settings::Settings) {
    let key = |id: &str, default: &str| {
        settings
            .keybindings
            .get(id)
            .filter(|keys| !keys.is_empty())
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    cx.bind_keys([
        ui::actions::key_binding(cx, &key("next", "j"), NextCommit, Some("history-list")),
        ui::actions::key_binding(cx, &key("prev", "k"), PrevCommit, Some("history-list")),
        ui::actions::key_binding(cx, "pagedown", PageDownCommits, Some("history-list")),
        ui::actions::key_binding(cx, "pageup", PageUpCommits, Some("history-list")),
        ui::actions::key_binding(cx, "secondary-c", CopyCommitHash, Some("history-list")),
    ]);
}

type Page = Result<Vec<(CommitInfo, Row<Oid>)>, Failure>;

/// One ref badge on a history row, derived from HEAD and the refs once they
/// load — never per frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeSpec {
    pub kind: RefKind,
    /// The label inside the badge: the short branch/tag name, or `HEAD`.
    pub label: SharedString,
    /// The full ref name, shown on hover.
    pub tooltip: SharedString,
}

/// How many of a row's badges fit beside its summary text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgeFit {
    /// Badges shown in order; the first (HEAD) is always among them.
    pub shown: usize,
    /// Whether the rest collapse into one "+N" badge.
    pub collapsed: bool,
}

/// Pick the badges that fit in `available` px, leaving the summary text
/// `min(80, 0.4·available)` px.
///
/// Badges are taken in order and the first one (HEAD) is always shown. Hidden
/// badges become one "+N" badge of `plus_width`, which must also fit. A
/// degenerate cell narrower than the first badge still shows that badge alone.
pub fn badge_overflow(widths: &[f32], available: f32, gap: f32, plus_width: f32) -> BadgeFit {
    let count = widths.len();
    if count == 0 {
        return BadgeFit {
            shown: 0,
            collapsed: false,
        };
    }
    let available = available.max(0.);
    let text_min = (available * 0.4).min(BADGE_TEXT_MIN);
    let fits = |shown: usize, plus: bool| {
        let used: f32 = widths[..shown].iter().sum();
        let trailing = if plus { plus_width + gap } else { 0. };
        used + trailing + shown as f32 * gap + text_min <= available
    };
    if fits(count, false) {
        return BadgeFit {
            shown: count,
            collapsed: false,
        };
    }
    for shown in (1..count).rev() {
        if fits(shown, true) {
            return BadgeFit {
                shown,
                collapsed: true,
            };
        }
    }
    // HEAD never hides, even when nothing else fits.
    BadgeFit {
        shown: 1,
        collapsed: count > 1,
    }
}

/// The badge specs for every commit, keyed by commit id.
///
/// Built once when HEAD and the refs load: the current branch is the HEAD
/// badge (never also a local one), symbolic `origin/HEAD` remotes, stashes and
/// other refs are hidden, and the order is HEAD, local, remote, tag with
/// alphabetical names inside a kind.
pub fn build_badges(head: Option<&Head>, refs: &[Ref]) -> HashMap<Oid, Vec<BadgeSpec>> {
    let mut badges: HashMap<Oid, Vec<BadgeSpec>> = HashMap::new();
    let mut current_branch = None;
    match head {
        Some(Head::Branch { name, target }) => {
            current_branch = Some(name.as_str());
            if let Some(target) = target {
                badges.entry(*target).or_default().push(BadgeSpec {
                    kind: RefKind::Head,
                    label: name.as_str().into(),
                    tooltip: format!("refs/heads/{name}").into(),
                });
            }
        }
        Some(Head::Detached(oid)) => badges.entry(*oid).or_default().push(BadgeSpec {
            kind: RefKind::Head,
            label: "HEAD".into(),
            tooltip: "HEAD".into(),
        }),
        None => {}
    }

    for (source, kind) in [
        (git_core::RefKind::LocalBranch, RefKind::Local),
        (git_core::RefKind::RemoteBranch, RefKind::Remote),
        (git_core::RefKind::Tag, RefKind::Tag),
    ] {
        let mut group: Vec<&Ref> = refs
            .iter()
            .filter(|reference| reference.kind == source)
            // The current branch is drawn once, as the HEAD badge.
            .filter(|reference| {
                !(kind == RefKind::Local && Some(reference.name.as_str()) == current_branch)
            })
            // `origin/HEAD` is a symbolic duplicate of a real remote branch.
            .filter(|reference| !(kind == RefKind::Remote && reference.name.ends_with("/HEAD")))
            .collect();
        group.sort_by(|a, b| a.name.cmp(&b.name));
        for reference in group {
            badges.entry(reference.target).or_default().push(BadgeSpec {
                kind,
                label: reference.name.clone().into(),
                tooltip: reference.full_name.clone().into(),
            });
        }
    }
    badges
}

/// The width of the flexible summary column: the pane minus the side margins,
/// the row padding, the fixed columns and the gaps between the present
/// columns.
pub fn summary_width(pane_width: f32, columns: Columns) -> f32 {
    let mut fixed = columns.graph + columns.date;
    let mut count = 3; // graph, summary, date
    if let Some(author) = columns.author {
        fixed += author;
        count += 1;
    }
    if let Some(hash) = columns.hash {
        fixed += hash;
        count += 1;
    }
    (pane_width - 2. * SIDE_MARGIN - PAD_LEFT - PAD_RIGHT - fixed - (count - 1) as f32 * COL_GAP)
        .max(0.)
}

/// A badge face, as far as text measurement is concerned: the weight and
/// whether an icon and its gap precede the label.
fn badge_face(kind: RefKind) -> (FontWeight, f32) {
    let icon = if kind == RefKind::Overflow {
        0.
    } else {
        BADGE_ICON
    };
    let weight = if kind == RefKind::Remote {
        FontWeight::MEDIUM
    } else {
        FontWeight::SEMIBOLD
    };
    (weight, icon)
}

/// Measures one badge's full width with the window's text system, caching the
/// result per label and kind. Only badges of the rows being painted are
/// measured.
fn measure_badge(
    window: &Window,
    label: &SharedString,
    kind: RefKind,
    cache: &mut HashMap<(SharedString, RefKind), Pixels>,
) -> Pixels {
    let key = (label.clone(), kind);
    if let Some(width) = cache.get(&key) {
        return *width;
    }
    let (weight, icon) = badge_face(kind);
    let mut font = gpui::font(FONT_UI);
    font.weight = weight;
    let run = TextRun {
        len: label.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let text = window
        .text_system()
        .layout_line(label, FS_XS, &[run], None)
        .width
        .min(px(BADGE_LABEL_MAX));
    let width = text + px(BADGE_PADDING + icon);
    cache.insert(key, width);
    width
}

/// The history table's columns for one pane width, shared by the header, the
/// rows and the skeleton rows so the three stay aligned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    /// Width of the graph column.
    pub graph: f32,
    /// Width of the author column; `None` once the pane is too narrow.
    pub author: Option<f32>,
    /// Width of the date column.
    pub date: f32,
    /// Width of the hash column; `None` once the pane is too narrow.
    pub hash: Option<f32>,
}

/// The table layout for a history pane `pane_width` wide whose graph column
/// has the user width `graph_width` (`gw`, 40–480).
///
/// This is the design's container-query table: the user width shrinks to a
/// share of the pane (40 % down to 560 px, 36 % below 460 px), the hash column
/// goes at 560 px and the author column at 460 px.
pub fn columns(pane_width: f32, graph_width: f32) -> Columns {
    let gw = clamp_graph_width(graph_width);
    let share = if pane_width <= PANE_NARROW {
        GRAPH_SHARE_NARROW
    } else if pane_width <= PANE_MEDIUM {
        GRAPH_SHARE_MEDIUM
    } else {
        GRAPH_SHARE_WIDE
    };
    let graph = gw
        .min(GRAPH_MAX_WIDE)
        .min(share * pane_width)
        .max(GRAPH_MIN);

    if pane_width <= PANE_NARROW {
        Columns {
            graph,
            author: None,
            date: COL_DATE_NARROW,
            hash: None,
        }
    } else if pane_width <= PANE_MEDIUM {
        Columns {
            graph,
            author: Some(COL_AUTHOR_NARROW),
            date: COL_DATE_NARROW,
            hash: None,
        }
    } else {
        Columns {
            graph,
            author: Some(COL_AUTHOR),
            date: COL_DATE,
            hash: Some(COL_HASH),
        }
    }
}

/// The graph width clamped to its persisted range; a value from a broken file
/// falls back to the design default.
fn clamp_graph_width(graph_width: f32) -> f32 {
    if graph_width.is_finite() {
        graph_width.clamp(GRAPH_MIN, GRAPH_MAX_WIDE)
    } else {
        GRAPH_WIDTH_DEFAULT
    }
}

/// The "fit the graph to its content" width: the graph's current full width
/// plus a little room, never past 420 px and never below the 40 px floor.
pub fn fit_width(graph_width: f32) -> f32 {
    (graph_width + GRAPH_FIT_PAD).clamp(GRAPH_MIN, GRAPH_FIT_MAX)
}

/// How many lanes have their centre past the graph column's right edge, i.e.
/// the `N` of the header's "+N" pill. `column_width` is the effective column
/// width, not the raw user setting (the pane's share may have narrowed it).
pub fn clipped_lanes(geometry: &ColumnGeometry, column_width: f32) -> usize {
    geometry.xs.iter().filter(|&&x| x > column_width).count()
}

struct Item {
    commit: Arc<CommitInfo>,
    row: Rc<Row<Oid>>,
}

/// A commit the user asked the history to reveal that has not arrived yet.
///
/// Pages are scanned as they land, so the pending commit never costs a full
/// scan of the rows already loaded.
#[derive(Debug, Default)]
struct PendingReveal {
    oid: Option<Oid>,
}

impl PendingReveal {
    fn set(&mut self, oid: Oid) {
        self.oid = Some(oid);
    }

    fn clear(&mut self) {
        self.oid = None;
    }

    /// The commit still waiting to be revealed, for the tests.
    #[cfg(test)]
    fn oid(&self) -> Option<Oid> {
        self.oid
    }

    fn take(&mut self) -> Option<Oid> {
        self.oid.take()
    }

    /// Looks for the pending commit among the rows that just arrived
    /// (`items[start..]`). When it is there, clears the reveal and returns its
    /// row; the already loaded rows are never rescanned.
    fn check(&mut self, items: &[Item], start: usize) -> Option<usize> {
        let oid = self.oid?;
        let offset = items[start..]
            .iter()
            .position(|item| item.commit.id == oid)?;
        self.oid = None;
        Some(start + offset)
    }
}

/// The grip drag in flight: where the pointer started and the width it is
/// resizing from. A click that never moved does not persist anything.
struct GripDrag {
    start_x: f32,
    start_width: f32,
    dragging: bool,
}

/// The drag payload of the graph grip; `on_drag_move` only reacts to this type.
#[derive(Clone)]
struct GripDragPayload;

/// A kill handle for a git child that is started on a background thread:
/// Cancel can arrive before the child exists, and then the child must die as
/// soon as it is published instead of running to completion.
#[derive(Clone, Default)]
struct LateKiller(Arc<Mutex<LateKill>>);

#[derive(Default)]
struct LateKill {
    killer: Option<LogKiller>,
    cancelled: bool,
}

impl LateKiller {
    /// Publishes the handle of a just-started child.
    fn set(&self, killer: LogKiller) {
        let mut state = lock(&self.0);
        if state.cancelled {
            killer.kill();
        } else {
            state.killer = Some(killer);
        }
    }

    /// Kills the child now, or the next one published.
    fn kill(&self) {
        let mut state = lock(&self.0);
        state.cancelled = true;
        if let Some(killer) = state.killer.take() {
            killer.kill();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the sidebar knows about the repository's refs.
#[derive(Debug, Default)]
enum RefsState {
    #[default]
    Loading,
    Loaded(Vec<Ref>),
    Failed,
}

pub struct HistoryView {
    /// The path the view was opened for; retried as-is.
    path: PathBuf,
    /// The resolved git, or why there is none. Retrying the resolution is the
    /// workspace's job; this is the handle for this repository.
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    /// What the shell does with a row action (branch/tag/reset dialogs).
    on_action: RefActionHandler,
    /// The repository root, once `Repo::open` succeeded.
    root: Option<PathBuf>,
    items: Vec<Item>,
    /// A reload is in flight and the rows still belong to the previous walk.
    /// The first page of the new walk replaces them.
    pending_rows: bool,
    max_lanes: usize,
    selected: Option<usize>,
    state: LoadState,
    /// Bumped for every load; results from an older load are dropped.
    generation: u64,
    /// The running load. Dropping it cancels the page task and the git log.
    task: Option<Task<()>>,
    /// The `commit_count` probe that feeds the load bar; cancelled with the
    /// load.
    count_task: Option<Task<()>>,
    /// The running `git init`, kept so a cancel or a new load drops it (and
    /// the load generation guards its answer).
    init_task: Option<Task<()>>,
    /// Stops the running `git log` at once, without waiting for its next page.
    log_killer: LateKiller,
    /// The same for `rev-list --count`.
    count_killer: LateKiller,
    /// `None` until the background read finishes; `Err` leaves the toolbar
    /// showing the placeholder.
    head: Option<Result<Head, String>>,
    refs: RefsState,
    /// The ref badges of every commit, built when HEAD and the refs load.
    badges: HashMap<Oid, Vec<BadgeSpec>>,
    /// Measured badge widths, cached per label and kind so the per-frame work
    /// stays proportional to the visible rows.
    badge_widths: HashMap<(SharedString, RefKind), Pixels>,
    /// The table layout for the pane's last measured width, so the virtualized
    /// rows paint with the same columns the header was built for.
    columns: Columns,
    /// The summary column's width for the current pane, used to fit badges.
    summary_width: f32,
    /// Notifies once a minute so relative dates stay fresh; dropped with the
    /// view.
    clock_task: Option<Task<()>>,
    /// The user's graph column width (`gw`), kept live while the grip drags
    /// and persisted on drag end.
    graph_width: f32,
    /// Per-column statistics for the idle-lane collapse, fed every emitted row
    /// in order. Reset whenever the graph is rebuilt.
    idle_lanes: IdleLanes,
    /// The x positions the painter uses, recomputed when rows arrive or
    /// `collapse` changes — never per row per frame.
    geometry: Rc<ColumnGeometry>,
    /// Whether idle lanes fold into gap lanes, persisted in the settings.
    collapse: bool,
    /// The grip drag in flight, if any: the pointer x and width it started at.
    grip_drag: Option<GripDrag>,
    /// The row under the mouse. The canvas cannot see the CSS-like hover, so
    /// the painter takes the halo and merge-ring fill from this.
    hovered: Option<usize>,
    /// The list's focus handle: clicking a row focuses it, and only while it
    /// is focused do the list's keys and the keyboard ring apply.
    focus: FocusHandle,
    /// The virtualized list's scroll state, so a selection moved by the
    /// keyboard can be scrolled minimally into view.
    scroll: UniformListScrollHandle,
    /// Whether the user picked a row themselves. Until then the selection
    /// follows the HEAD row as it arrives instead of staying on row 0.
    user_selected: bool,
    /// A commit the sidebar asked to reveal that has not been loaded yet;
    /// checked against each arriving page.
    pending_reveal: PendingReveal,
    /// The one commit-graph writer of this view. Started only while nothing
    /// writes one and the repository has none, and never dropped by Retry or
    /// Cancel: it is useful work that finishes with the view.
    commit_graph_task: Option<Task<()>>,
    /// Whether that writer is still running, so a later load does not start a
    /// second one. The finished task stays in its slot (inert) until it is
    /// replaced.
    commit_graph_running: bool,
    /// The search field's current text. Applied after [`SEARCH_DEBOUNCE`].
    search_text: String,
    /// The query the matches belong to; `None` while the field is empty.
    search: Option<Query>,
    /// The matching rows of the applied query, sorted by row.
    search_matches: Vec<history_search::HistoryMatch>,
    /// How many rows the matches cover. Pages that arrive later are scanned
    /// from here, never rescanned from the top.
    search_scanned: usize,
    /// Whether a background scan is in flight; new pages wait for it.
    search_scanning: bool,
    /// Bumped on every query or load change; results of an older scan are
    /// dropped instead of appended to the wrong query.
    search_generation: u64,
    /// The pending debounce timer; dropping it cancels the wait.
    search_debounce: Option<Task<()>>,
    /// The running background scan.
    search_task: Option<Task<()>>,
    /// Whether the next finished scan should select the first match. Cleared
    /// once the selection jumped, or with the query.
    search_jump: bool,
}

/// Adds one row-menu item that asks the shell to open a ref dialog.
#[allow(clippy::too_many_arguments)]
fn push_row_action(
    entries: &mut Vec<MenuEntry>,
    card: &CommitCard,
    on_action: &RefActionHandler,
    label: String,
    icon: IconName,
    shortcut: Option<&str>,
    build: fn(CommitCard) -> RefAction,
    danger: bool,
) {
    let on_action = on_action.clone();
    let card = card.clone();
    let mut item = MenuItem::new(label).icon(icon);
    if let Some(shortcut) = shortcut {
        item = item.shortcut(shortcut.to_owned());
    }
    if danger {
        item = item.danger();
    }
    entries.push(MenuEntry::Item(item.action(move |window, cx| {
        on_action(build(card.clone()), window, cx);
    })));
}

impl HistoryView {
    /// Starts in the loading state; opening the repository, reading HEAD, the
    /// refs and the history all run in the background and fill in the rows or
    /// the failure as they arrive. `git` is resolved once per process by the
    /// caller.
    pub fn open(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        on_action: RefActionHandler,
        cx: &mut Context<Self>,
    ) -> Self {
        let (graph_width, collapse) = cx
            .try_global::<crate::AppSettings>()
            .map(|state| {
                (
                    state.settings.graph_width,
                    state.settings.collapse_idle_lanes,
                )
            })
            .unwrap_or((GRAPH_WIDTH_DEFAULT, false));
        let graph_width = clamp_graph_width(graph_width);
        // Replaced with the pane's real width in the first prepaint.
        let columns = columns(1200., graph_width);
        let mut view = Self {
            path,
            git,
            workspace,
            on_action,
            root: None,
            items: Vec::new(),
            pending_rows: false,
            max_lanes: 1,
            selected: None,
            state: LoadState::Loading { total: None },
            generation: 0,
            task: None,
            count_task: None,
            init_task: None,
            log_killer: LateKiller::default(),
            count_killer: LateKiller::default(),
            head: None,
            refs: RefsState::Loading,
            badges: HashMap::new(),
            badge_widths: HashMap::new(),
            columns,
            summary_width: summary_width(1200., columns),
            clock_task: None,
            graph_width,
            idle_lanes: IdleLanes::new(),
            geometry: Rc::new(IdleLanes::new().geometry(collapse)),
            collapse,
            grip_drag: None,
            hovered: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            user_selected: false,
            pending_reveal: PendingReveal::default(),
            commit_graph_task: None,
            commit_graph_running: false,
            search_text: String::new(),
            search: None,
            search_matches: Vec::new(),
            search_scanned: 0,
            search_scanning: false,
            search_generation: 0,
            search_debounce: None,
            search_task: None,
            search_jump: false,
        };
        view.start(cx);
        // Relative dates are derived at render time from the clock, so the
        // rows only need a repaint; the task ends with the view.
        view.clock_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(CLOCK_INTERVAL).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }));
        view
    }

    /// Start a fresh load: a new task, a new generation and the loading state.
    fn start(&mut self, cx: &mut Context<Self>) {
        // Stopping the old tasks kills their git processes before new ones
        // start.
        self.stop_tasks();
        self.generation += 1;
        let generation = self.generation;
        self.state = LoadState::Loading { total: None };
        // The graph starts over with the load, so its geometry does too.
        self.idle_lanes = IdleLanes::new();
        self.refresh_geometry();

        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.state = LoadState::Failed(Failure::from_error(error));
                return;
            }
        };
        let path = self.path.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = Self::run(git, path, generation, &this, cx).await;
            this.update(cx, |view, cx| view.finish(generation, result, cx))
                .ok();
        }));
    }

    /// Drops the running tasks and kills the git children they started.
    fn stop_tasks(&mut self) {
        self.task = None;
        self.count_task = None;
        self.init_task = None;
        self.log_killer.kill();
        self.count_killer.kill();
        self.log_killer = LateKiller::default();
        self.count_killer = LateKiller::default();
    }

    /// Open the repository and stream its history into the view. A closed
    /// window just ends the task with `Ok`.
    async fn run(
        git: Arc<Git>,
        path: PathBuf,
        generation: u64,
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) -> Result<(), Failure> {
        let executor = cx.background_executor().clone();
        let repo = executor
            .spawn(async move { Repo::open(git, &path) })
            .await
            .map_err(|e| Failure::from_error(&e))?;

        // The repository opened: remember it for the switcher, whatever the
        // history walk does next. The root is canonicalized (and the settings
        // saved) off the UI thread; the entry is stored as canonical, so later
        // touches compare as stored.
        let root = repo.root().to_path_buf();
        let recent_root = root.clone();
        cx.spawn(async move |cx| {
            let canonical = cx
                .background_executor()
                .spawn(async move { recent_root.canonicalize().unwrap_or(recent_root) })
                .await;
            cx.update(|cx| {
                crate::update_settings(cx, |settings| {
                    settings.touch_recent(canonical, unix_now());
                });
            });
        })
        .detach();

        // HEAD and the refs feed the toolbar and the sidebar; read them in the
        // background so the log is not held back.
        let meta_repo = repo.clone();
        let meta_root = root.clone();
        let meta_this = this.clone();
        cx.spawn(async move |cx| {
            let (head, refs) = cx
                .background_executor()
                .spawn(async move { (meta_repo.head(), meta_repo.refs()) })
                .await;
            meta_this
                .update(cx, |view, cx| view.set_meta(meta_root, head, refs, cx))
                .ok();
        })
        .detach();

        // The count feeds the load bar's percentage; it runs beside the log so
        // the first page is not delayed. A failure only loses the percentage.
        // The view keeps the handle so cancelling the load kills the count.
        let counter = repo.clone();
        let count_this = this.clone();
        let count_task = cx.spawn(async move |cx| {
            let count = cx
                .background_executor()
                .spawn(async move { counter.commit_count_cancellable() })
                .await;
            let count = match count {
                Ok(count) => count,
                Err(e) => {
                    log::warn!("could not start counting the commits: {e}");
                    return;
                }
            };
            let killer = count.killer();
            count_this
                .update(cx, |view, _cx| {
                    // A newer load owns the slot now; this count is stale.
                    if view.generation == generation {
                        view.count_killer.set(killer);
                    } else {
                        killer.kill();
                    }
                })
                .ok();
            let result = cx
                .background_executor()
                .spawn(async move { count.wait() })
                .await;
            match result {
                Ok(total) => {
                    count_this
                        .update(cx, |view, cx| view.set_total(generation, total, cx))
                        .ok();
                }
                Err(e) => log::warn!("could not count the commits: {e}"),
            }
        });
        this.update(cx, |view, _cx| view.count_task = Some(count_task))
            .ok();

        // Without a commit-graph file the first screen waits for the whole
        // history walk. Write one for next time, in parallel with this load.
        // The view owns the one writer, so Retry or Cancel cannot start a
        // second copy of the same work.
        let writer = repo.clone();
        this.update(cx, |view, cx| view.start_commit_graph_writer(writer, cx))
            .ok();

        // Both the git process and the layout run off the UI thread; the
        // UI thread only appends finished pages and repaints.
        let stream = executor
            .spawn(async move { repo.log(&LogOptions::default()) })
            .await
            .map_err(|e| Failure::from_error(&e))?;
        let killer = stream.killer();
        this.update(cx, |view, _cx| {
            if view.generation == generation {
                view.log_killer.set(killer);
            } else {
                killer.kill();
            }
        })
        .ok();
        let mut state = Some((stream, GraphBuilder::new()));
        while let Some((stream, graph)) = state.take() {
            let (stream, graph, page) = executor
                .spawn(async move { next_page(stream, graph) })
                .await;
            let page = page?;
            if page.is_empty() {
                break;
            }
            if this.update(cx, |view, cx| view.append(page, cx)).is_err() {
                return Ok(()); // the window was closed: stop reading
            }
            state = Some((stream, graph));
        }
        Ok(())
    }

    /// Starts the view's one commit-graph writer when the repository has none
    /// and no writer of this view is running. The task is kept in the view and
    /// dropped with it; Retry and Cancel leave it alone.
    fn start_commit_graph_writer(&mut self, repo: Repo, cx: &mut Context<Self>) {
        if self.commit_graph_running || repo.has_commit_graph() {
            return;
        }
        self.commit_graph_running = true;
        self.commit_graph_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { repo.write_commit_graph() })
                .await;
            if let Err(e) = result {
                log::warn!("could not write the commit-graph: {e}");
            }
            this.update(cx, |view, _cx| view.commit_graph_running = false)
                .ok();
        }));
    }

    fn append(&mut self, page: Vec<(CommitInfo, Row<Oid>)>, cx: &mut Context<Self>) {
        // A reload keeps the previous rows until the first page of the new
        // walk arrives, so the list does not flash empty.
        if self.pending_rows {
            self.items.clear();
            self.max_lanes = 1;
            self.idle_lanes = IdleLanes::new();
            self.pending_rows = false;
        }
        let start = self.items.len();
        for (commit, row) in page {
            self.max_lanes = self.max_lanes.max(row.width);
            self.idle_lanes.push(&row);
            self.items.push(Item {
                commit: Arc::new(commit),
                row: Rc::new(row),
            });
        }
        self.refresh_geometry();
        // The HEAD row may only arrive now; the initial selection follows it
        // until the user picks a row.
        self.settle_initial_selection(cx);
        // A page that arrives while a query is active is searched as it lands,
        // never by rescanning the rows already covered.
        self.scan_more(cx);
        // A sidebar reveal that was waiting for this page lands now, and only
        // the rows that just arrived are scanned for it.
        if let Some(index) = self.pending_reveal.check(&self.items, start) {
            self.reveal_row(index, cx);
        }
        cx.notify();
    }

    /// Rebuild the painter's x snapshot. Called when rows were added or the
    /// collapse toggle changed, not while rendering.
    fn refresh_geometry(&mut self) {
        self.geometry = Rc::new(self.idle_lanes.geometry(self.collapse));
    }

    /// The row of the HEAD commit, once it is among the loaded rows.
    fn head_index(&self) -> Option<usize> {
        let head = self.head_oid()?;
        self.items.iter().position(|item| item.commit.id == head)
    }

    /// The row a key press lands on when nothing is selected: the HEAD row
    /// when it is loaded, else row 0.
    fn initial_row(&self) -> Option<usize> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.head_index().unwrap_or(0))
        }
    }

    /// While the user has not picked a row, the selection follows the HEAD row
    /// as it arrives, else row 0. Selecting is not focusing: this never moves
    /// the keyboard focus or scrolls the list.
    fn settle_initial_selection(&mut self, cx: &mut Context<Self>) {
        if self.user_selected || self.items.is_empty() {
            return;
        }
        let target = self.initial_row().unwrap_or(0);
        if self.selected != Some(target) {
            self.selected = Some(target);
            cx.notify();
        }
    }

    /// Selects `index` because the user picked it (a click, the row menu or a
    /// navigation key): a pending search jump and a pending sidebar reveal are
    /// cancelled, so a later page cannot yank the selection away.
    fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.search_jump = false;
        self.pending_reveal.clear();
        self.select(index, cx);
    }

    /// Selects `index` because the search found it; the deferred jump stays
    /// armed until it lands. The search jump replaces any pending sidebar
    /// reveal.
    fn select_row_from_search(&mut self, index: usize, cx: &mut Context<Self>) {
        self.pending_reveal.clear();
        self.select(index, cx);
    }

    /// Selects `index`, scrolls it minimally into view and repaints. Any pick
    /// stops the initial selection from following the load.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.items.len() || self.selected == Some(index) {
            self.user_selected = true;
            return;
        }
        self.user_selected = true;
        self.selected = Some(index);
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// The sidebar asked for `oid`: selects its row as a user pick and centres
    /// it in the list, without moving the keyboard focus (the sidebar keeps
    /// it). While the history is still streaming the commit may not have
    /// arrived yet; then it is remembered and applied when its page lands.
    pub fn reveal_commit(&mut self, oid: Oid, cx: &mut Context<Self>) {
        self.pending_reveal.clear();
        if let Some(index) = self.items.iter().position(|item| item.commit.id == oid) {
            self.reveal_row(index, cx);
        } else if self.is_loading() {
            self.pending_reveal.set(oid);
        } else {
            // The load is over, so no later page will bring it.
            log::info!("the commit {oid} is not in the loaded history");
        }
    }

    /// Selects the revealed row and centres it in the list.
    fn reveal_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_row(index, cx);
        self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        cx.notify();
    }

    /// ↓ / j and ↑ / k: one row, clamped at the ends. With nothing selected
    /// the first press selects the initial row instead of moving off it.
    fn step_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(current) = self.selected.or_else(|| self.initial_row()) else {
            return;
        };
        let next = if self.selected.is_none() {
            current
        } else {
            let last = self.items.len().saturating_sub(1) as isize;
            (current as isize + delta).clamp(0, last) as usize
        };
        self.select_row(next, cx);
    }

    /// Home / End: the first or the last loaded row (`usize::MAX` for End).
    fn jump_selection(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(last) = self.items.len().checked_sub(1) else {
            return;
        };
        self.select_row(index.min(last), cx);
    }

    /// PgDn / PgUp: one viewport of rows minus one, clamped.
    fn page_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(current) = self.selected.or_else(|| self.initial_row()) else {
            return;
        };
        if self.selected.is_none() {
            self.select_row(current, cx);
            return;
        }
        let delta = page_rows(self.viewport_height(), f32::from(cx.theme().row_height)) as isize;
        let delta = if down { delta } else { -delta };
        let last = self.items.len().saturating_sub(1) as isize;
        let next = (current as isize + delta).clamp(0, last) as usize;
        self.select_row(next, cx);
    }

    /// The list's viewport height, from the last layout of the virtualized
    /// list; 0 before its first layout.
    fn viewport_height(&self) -> f32 {
        self.scroll
            .0
            .borrow()
            .last_item_size
            .map(|size| f32::from(size.item.height))
            .unwrap_or(0.)
    }

    /// `Ctrl+C` on the focused list: the selected commit's full 40-char hash
    /// on the clipboard.
    fn copy_selected_hash(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected.or_else(|| self.initial_row()) else {
            return;
        };
        self.select_row(index, cx);
        if let Some(item) = self.items.get(index) {
            cx.write_to_clipboard(ClipboardItem::new_string(item.commit.id.to_string()));
        }
    }

    /// Double-click: switch to the row's first local branch that is not HEAD,
    /// or check the commit out detached when the row has no such branch.
    fn checkout_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let card = CommitCard::from_commit(&item.commit, self.refs().unwrap_or(&[]), self.head());
        let head_branch = match self.head() {
            Some(Head::Branch { name, .. }) => Some(name.as_str()),
            _ => None,
        };
        let branch = card.refs.iter().find(|reference| {
            reference.kind == CardRefKind::Local && Some(reference.name.as_str()) != head_branch
        });
        let action = match branch {
            Some(reference) => RefAction::CheckoutBranch(reference.name.clone()),
            None if head_branch.is_some()
                && card
                    .refs
                    .iter()
                    .any(|reference| reference.kind == CardRefKind::Local) =>
            {
                // The row's only local branch is the one already checked out.
                return;
            }
            None => RefAction::CheckoutCommit(card),
        };
        (self.on_action)(action, window, cx);
    }

    /// Right-clicking a row selects it and opens the row menu at the pointer.
    fn open_row_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hash = self.items.get(index).map(|item| item.commit.id.to_string());
        let card = self.items.get(index).map(|item| {
            CommitCard::from_commit(&item.commit, self.refs().unwrap_or(&[]), self.head())
        });
        let branch = match self.head() {
            Some(Head::Branch { name, .. }) => name.clone(),
            _ => "HEAD".to_owned(),
        };
        let mut entries = vec![MenuEntry::Item(
            MenuItem::new("Copy commit hash")
                .icon(IconName::Copy)
                .shortcut("Ctrl+C")
                .disabled(hash.is_none())
                .action(move |_, cx| {
                    if let Some(hash) = hash.clone() {
                        cx.write_to_clipboard(ClipboardItem::new_string(hash));
                    }
                }),
        )];
        if let Some(card) = card {
            let on_action = self.on_action.clone();
            let head_branch = match self.head() {
                Some(Head::Branch { name, .. }) => Some(name.clone()),
                _ => None,
            };
            let locals: Vec<String> = card
                .refs
                .iter()
                .filter(|reference| reference.kind == CardRefKind::Local)
                .map(|reference| reference.name.clone())
                .filter(|name| Some(name) != head_branch.as_ref())
                .collect();
            if locals.is_empty() {
                let checkout = on_action.clone();
                let checkout_card = card.clone();
                entries.push(MenuEntry::Item(
                    MenuItem::new("Checkout this commit")
                        .icon(IconName::Check)
                        .action(move |window, cx| {
                            checkout(RefAction::CheckoutCommit(checkout_card.clone()), window, cx);
                        }),
                ));
            }
            for name in locals {
                let checkout = on_action.clone();
                let branch = name.clone();
                entries.push(MenuEntry::Item(
                    MenuItem::new(format!("Switch to {name}"))
                        .icon(IconName::Check)
                        .action(move |window, cx| {
                            checkout(RefAction::CheckoutBranch(branch.clone()), window, cx);
                        }),
                ));
            }
            push_row_action(
                &mut entries,
                &card,
                &on_action,
                "Create branch here…".to_owned(),
                IconName::Branch,
                Some("B"),
                RefAction::CreateBranch,
                false,
            );
            push_row_action(
                &mut entries,
                &card,
                &on_action,
                "Create tag here…".to_owned(),
                IconName::Tag,
                None,
                RefAction::CreateTag,
                false,
            );
            entries.push(MenuEntry::Separator);
            push_row_action(
                &mut entries,
                &card,
                &on_action,
                "Cherry-pick".to_owned(),
                IconName::Undo,
                None,
                RefAction::CherryPick,
                false,
            );
            push_row_action(
                &mut entries,
                &card,
                &on_action,
                "Revert commit".to_owned(),
                IconName::Undo,
                None,
                RefAction::Revert,
                false,
            );
            push_row_action(
                &mut entries,
                &card,
                &on_action,
                format!("Reset {branch} to here…"),
                IconName::Undo,
                None,
                RefAction::Reset,
                true,
            );
        }
        menu::open_context_menu(position, entries, window, cx);
    }

    /// The commit card a dialog shows for `oid`; a commit outside the loaded
    /// page still gets its hash.
    pub fn card_for(&self, oid: Oid) -> CommitCard {
        self.items
            .iter()
            .find(|item| item.commit.id == oid)
            .map(|item| {
                CommitCard::from_commit(&item.commit, self.refs().unwrap_or(&[]), self.head())
            })
            .unwrap_or_else(|| CommitCard::bare(oid))
    }

    /// Sets the idle-lane collapse, recomputes the geometry and persists it.
    fn set_collapse(&mut self, collapse: bool, cx: &mut Context<Self>) {
        if self.collapse == collapse {
            return;
        }
        self.collapse = collapse;
        self.refresh_geometry();
        crate::update_settings(cx, move |settings| {
            settings.collapse_idle_lanes = collapse;
        });
        cx.notify();
    }

    /// Fits the graph column to the current geometry and persists the width.
    /// Used by the grip's double-click, the "+N" pill and the context menu.
    fn fit_graph(&mut self, cx: &mut Context<Self>) {
        let width = fit_width(self.geometry.width);
        self.graph_width = width;
        crate::update_settings(cx, move |settings| settings.graph_width = width);
        cx.notify();
    }

    /// Ends a grip drag: only a drag that actually moved is persisted, so a
    /// plain click on the grip does not write the settings.
    fn finish_grip_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.grip_drag.take() else {
            return;
        };
        if !drag.dragging {
            return;
        }
        let width = self.graph_width;
        crate::update_settings(cx, move |settings| settings.graph_width = width);
        cx.notify();
    }

    /// Opens the graph header's context menu at the pointer.
    fn open_graph_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let toggle = view.clone();
        let fit = view;
        let checked = self.collapse;
        let entries = vec![
            MenuEntry::Item(
                MenuItem::new("Collapse idle lanes")
                    // The kit menu has no checked state: the icon says it.
                    .icon(if checked {
                        IconName::Check
                    } else {
                        IconName::Lanes
                    })
                    .action(move |_, cx| {
                        toggle
                            .update(cx, |view, cx| view.set_collapse(!view.collapse, cx))
                            .ok();
                    }),
            ),
            MenuEntry::Item(
                MenuItem::new("Fit graph to content")
                    .icon(IconName::Split)
                    .action(move |_, cx| {
                        fit.update(cx, |view, cx| view.fit_graph(cx)).ok();
                    }),
            ),
        ];
        menu::open_context_menu(position, entries, window, cx);
    }

    /// The commit count arrived; only the current load may set it.
    fn set_total(&mut self, generation: u64, total: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        if let LoadState::Loading { total: slot } = &mut self.state {
            *slot = Some(total);
            cx.notify();
        }
    }

    fn set_meta(
        &mut self,
        root: PathBuf,
        head: Result<Head, Error>,
        refs: Result<Vec<Ref>, Error>,
        cx: &mut Context<Self>,
    ) {
        self.root = Some(root);
        match head {
            Ok(head) => self.head = Some(Ok(head)),
            Err(e) => {
                log::warn!("could not read HEAD: {e}");
                self.head = Some(Err(e.to_string()));
            }
        }
        self.refs = match refs {
            Ok(refs) => RefsState::Loaded(refs),
            Err(e) => {
                log::warn!("could not read the refs: {e}");
                RefsState::Failed
            }
        };
        // The per-commit badges are derived once here, never while painting.
        let head = self.head.as_ref().and_then(|head| head.as_ref().ok());
        let refs = match &self.refs {
            RefsState::Loaded(refs) => refs.as_slice(),
            RefsState::Loading | RefsState::Failed => &[],
        };
        self.badges = build_badges(head, refs);
        // HEAD may have arrived after the first rows: the initial selection
        // can now move to its row.
        self.settle_initial_selection(cx);
        cx.notify();
    }

    /// The search field's text changed. The actual search waits for the
    /// debounce; an empty query clears the matches at once.
    pub fn set_search_query(&mut self, text: String, cx: &mut Context<Self>) {
        if self.search_text == text {
            return;
        }
        self.search_text = text;
        // Dropping the old timer and the old scan cancels both.
        self.search_debounce = None;
        self.search_task = None;
        self.search_scanning = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        if Query::parse(&self.search_text).is_none() {
            self.search = None;
            self.search_matches.clear();
            self.search_scanned = 0;
            self.search_jump = false;
            cx.notify();
            return;
        }
        self.search_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |view, cx| {
                if view.search_generation == generation {
                    view.apply_search(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Esc: no query, no matches, and the next query starts a fresh scan.
    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_text.clear();
        self.search = None;
        self.search_matches.clear();
        self.search_scanned = 0;
        self.search_scanning = false;
        self.search_jump = false;
        self.search_debounce = None;
        self.search_task = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        cx.notify();
    }

    /// What the search field's trailing text says: the position of the
    /// selected match and the total, "No matches", or `None` for an empty
    /// field (the hint comes back then).
    pub fn search_status(&self) -> Option<SharedString> {
        self.search.as_ref()?;
        // While rows are still arriving or unscanned, the count is a lower
        // bound: "No matches" would be a lie about a scan that is not done.
        let pending =
            self.is_loading() || self.search_scanning || self.search_scanned < self.items.len();
        if self.search_matches.is_empty() {
            return Some(if pending {
                "– / 0+".into()
            } else {
                "No matches".into()
            });
        }
        let total = self.search_matches.len();
        let suffix = if pending { "+" } else { "" };
        let position = self
            .selected
            .and_then(|row| history_search::position_of(&self.search_matches, row))
            .map(|position| (position + 1).to_string())
            .unwrap_or_else(|| "–".to_owned());
        Some(format!("{position} / {total}{suffix}").into())
    }

    /// The history list's focus handle, so Esc and ↓ in the search field can
    /// hand the keyboard back without moving the selection.
    pub fn list_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The selected row, for the search's gpui test in the repository shell.
    #[cfg(test)]
    pub(crate) fn selected_row(&self) -> Option<usize> {
        self.selected
    }

    /// Enter / F3 (next) and Shift+Enter / Shift+F3 (previous), both wrapping.
    pub fn search_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        // The timer handle stays in its slot after it fires, so compare the
        // field with the applied query instead: different text means the
        // debounce has not searched it yet, and Enter must apply it now rather
        // than step over the previous query's matches.
        if Query::parse(&self.search_text) != self.search {
            self.search_debounce = None;
            self.apply_search(cx);
        }
        let Some(position) = history_search::step(&self.search_matches, self.selected, forward)
        else {
            return;
        };
        let row = self.search_matches[position].row;
        self.select_row_from_search(row, cx);
    }

    /// The debounce elapsed: parse the field's text and start the scan.
    fn apply_search(&mut self, cx: &mut Context<Self>) {
        let Some(query) = Query::parse(&self.search_text) else {
            return;
        };
        self.search = Some(query);
        self.search_matches.clear();
        self.search_scanned = 0;
        self.search_scanning = false;
        self.search_task = None;
        // The first finished scan jumps to the first match at or after the
        // selection, even when the match only arrives with a later page.
        self.search_jump = true;
        self.scan_more(cx);
        cx.notify();
    }

    /// Scan the rows that were not searched yet. The snapshot shares the
    /// commits with the view (`Arc`), and the match runs on the background
    /// executor: even a 100k-row history never blocks a frame.
    fn scan_more(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.clone() else {
            return;
        };
        if self.search_scanning || self.search_scanned >= self.items.len() {
            return;
        }
        let start = self.search_scanned;
        let end = self.items.len();
        let rows: Vec<Arc<CommitInfo>> = self.items[start..end]
            .iter()
            .map(|item| item.commit.clone())
            .collect();
        let generation = self.search_generation;
        let executor = cx.background_executor().clone();
        self.search_scanning = true;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let found = executor
                .spawn(async move {
                    history_search::search(
                        &query,
                        rows.iter()
                            .enumerate()
                            .map(|(offset, commit)| (start + offset, &**commit)),
                    )
                })
                .await;
            this.update(cx, |view, cx| view.finish_scan(generation, end, found, cx))
                .ok();
        }));
    }

    /// A background scan finished: keep its matches when it still belongs to
    /// the current query, then either jump or search the rows that arrived
    /// while it ran.
    fn finish_scan(
        &mut self,
        generation: u64,
        end: usize,
        found: Vec<history_search::HistoryMatch>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.search_generation {
            return;
        }
        self.search_scanning = false;
        self.search_matches.extend(found);
        self.search_scanned = end;
        if self.search_jump {
            if let Some(position) = history_search::first_from(&self.search_matches, self.selected)
            {
                self.search_jump = false;
                let row = self.search_matches[position].row;
                self.select_row_from_search(row, cx);
            }
        }
        self.scan_more(cx);
        cx.notify();
    }

    /// A new load replaces the rows: keep the query and search the new rows
    /// from the top.
    fn restart_search(&mut self, cx: &mut Context<Self>) {
        // The field may show text typed since the last debounce fired; apply it
        // here, or cancelling the timer would silently drop the new query.
        self.search_debounce = None;
        self.search = Query::parse(&self.search_text);
        self.search_matches.clear();
        self.search_scanned = 0;
        self.search_scanning = false;
        self.search_task = None;
        self.search_jump = self.search.is_some();
        self.search_generation = self.search_generation.wrapping_add(1);
        self.scan_more(cx);
    }

    fn finish(&mut self, generation: u64, result: Result<(), Failure>, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.state = match result {
            Ok(()) => LoadState::Loaded,
            Err(failure) => LoadState::Failed(failure),
        };
        // No later page can bring a commit the load did not deliver.
        if let Some(oid) = self.pending_reveal.take() {
            log::info!("the commit {oid} is not in the loaded history");
        }
        cx.notify();
    }

    /// Stop the load; the rows that arrived stay on screen.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_loading() {
            return;
        }
        self.stop_tasks();
        // A late answer from the cancelled task (or a running `git init`)
        // must not touch the view any more.
        self.generation += 1;
        self.state = LoadState::Cancelled;
        cx.notify();
    }

    /// Reloads the history from scratch, e.g. after a commit moved HEAD.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.retry(cx);
    }

    /// Start over: load again, keeping the rows until the first new page lands.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.pending_rows = !self.items.is_empty();
        self.selected = None;
        self.user_selected = false;
        self.hovered = None;
        // The new load starts at the top, so the HEAD selection that settles
        // later is visible instead of being left below the old scroll offset.
        self.scroll.0.borrow().base_handle.set_offset(Point {
            x: px(0.),
            y: px(0.),
        });
        // The query stays in the field; its matches belong to the old rows.
        self.restart_search(cx);
        self.start(cx);
        cx.notify();
    }

    /// `git init` the folder, then load the new repository.
    fn init_repository(&mut self, cx: &mut Context<Self>) {
        let Ok(git) = self.git.clone() else {
            return;
        };
        let path = self.path.clone();
        self.stop_tasks();
        self.generation += 1;
        let generation = self.generation;
        self.state = LoadState::Loading { total: None };
        self.init_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::init(git, &path) })
                .await;
            this.update(cx, |view, cx| {
                // A cancel or a newer load wins over this answer.
                if view.generation != generation {
                    return;
                }
                match result {
                    Ok(_) => view.retry(cx),
                    Err(error) => {
                        view.state = LoadState::Failed(Failure::from_error(&error));
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// The repository root, once `Repo::open` succeeded.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// HEAD, once the background read finished and succeeded.
    pub fn head(&self) -> Option<&Head> {
        self.head.as_ref().and_then(|head| head.as_ref().ok())
    }

    /// The commit HEAD points at, for the graph's HEAD ring. Detached HEAD has
    /// a ring too; a branch whose target could not be read does not.
    fn head_oid(&self) -> Option<Oid> {
        match self.head.as_ref()?.as_ref().ok()? {
            Head::Branch { target, .. } => *target,
            Head::Detached(oid) => Some(*oid),
        }
    }

    /// The refs, once loaded; `None` while loading or after a failure.
    pub fn refs(&self) -> Option<&[Ref]> {
        match &self.refs {
            RefsState::Loaded(refs) => Some(refs),
            RefsState::Loading | RefsState::Failed => None,
        }
    }

    /// What the centre pane is showing, for the status bar.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Whether the history is still streaming in.
    pub fn is_loading(&self) -> bool {
        matches!(self.state, LoadState::Loading { .. })
    }

    /// How many commits have arrived so far.
    pub fn commit_count(&self) -> usize {
        self.items.len()
    }

    /// The short hash of the selected commit, for the details header.
    pub fn selected_short(&self) -> Option<SharedString> {
        self.selected.and_then(|ix| self.items.get(ix)).map(|item| {
            let mut hash = item.commit.id.to_string();
            hash.truncate(7);
            hash.into()
        })
    }

    /// The selected commit, for the details dock.
    pub fn selected_commit(&self) -> Option<Arc<CommitInfo>> {
        self.selected
            .and_then(|ix| self.items.get(ix))
            .map(|item| item.commit.clone())
    }

    /// The graph header cell: the label, the "+N" clipped-lanes pill, the
    /// resize grip and the right-click menu.
    fn render_graph_header(&self, columns: Columns, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let accent = colors.accent;
        let clipped = clipped_lanes(&self.geometry, columns.graph);
        let dragging = self.grip_drag.as_ref().is_some_and(|drag| drag.dragging);

        let mut cell = div()
            .id("history-graph-header")
            .w(px(columns.graph))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .debug_selector(|| "history-graph-header".to_string())
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.open_graph_menu(event.position, window, cx);
                }),
            )
            .child("Graph");
        if clipped > 0 {
            cell = cell.child(
                div()
                    .id("history-graph-clipped")
                    .flex_none()
                    .px(px(6.))
                    .rounded(px(8.))
                    .bg(colors.accent_soft)
                    .text_size(FS_XS)
                    .font_weight(FontWeight::BOLD)
                    .text_color(accent)
                    .cursor_pointer()
                    .tooltip(Tooltip::text(format!(
                        "{clipped} lanes are clipped. Click to widen the column."
                    )))
                    .on_click(cx.listener(|view, _, _, cx| view.fit_graph(cx)))
                    .child(format!("+{clipped}")),
            );
        }

        let grip = div()
            .id("history-graph-grip")
            .absolute()
            .right(px(-8.))
            .top(px(3.))
            .bottom(px(3.))
            .w(px(7.))
            .rounded(px(2.))
            .cursor_col_resize()
            .hover(move |style| style.bg(accent.opacity(0.7)))
            .when(dragging, |this| this.bg(accent.opacity(0.7)))
            .tooltip(Tooltip::text(
                "Drag to resize the graph column · double-click to fit",
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        view.fit_graph(cx);
                    } else {
                        view.grip_drag = Some(GripDrag {
                            start_x: f32::from(event.position.x),
                            start_width: view.graph_width,
                            dragging: false,
                        });
                    }
                }),
            )
            .on_drag(GripDragPayload, |_, _, _, cx| cx.new(|_| EmptyView))
            .on_drag_move(
                cx.listener(|view, event: &DragMoveEvent<GripDragPayload>, _, cx| {
                    let Some(drag) = &mut view.grip_drag else {
                        return;
                    };
                    let dx = f32::from(event.event.position.x) - drag.start_x;
                    drag.dragging = true;
                    let width = (drag.start_width + dx).clamp(GRAPH_MIN, GRAPH_MAX_WIDE);
                    if view.graph_width != width {
                        view.graph_width = width;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, cx| view.finish_grip_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, cx| view.finish_grip_drag(cx)),
            );

        cell.child(grip)
    }

    fn render_header(&self, columns: Columns, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        // A 6px gutter around the header, so it lines up with the rows.
        let mut head = div()
            .w_full()
            .h(cx.theme().row_height)
            .pl(px(PAD_LEFT))
            .pr(px(PAD_RIGHT))
            .flex()
            .items_center()
            .gap(px(COL_GAP))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.muted)
            .child(self.render_graph_header(columns, cx))
            .child(div().flex_1().min_w_0().truncate().child("Summary"));
        if let Some(author) = columns.author {
            head = head.child(div().w(px(author)).flex_none().child("Author"));
        }
        head = head.child(div().w(px(columns.date)).flex_none().child("Date"));
        if let Some(hash) = columns.hash {
            head = head.child(div().w(px(hash)).flex_none().child("Hash"));
        }
        div().w_full().px(px(SIDE_MARGIN)).child(head)
    }

    /// The one-line load bar under the header.
    fn render_load_bar(&self, loaded: u64, total: Option<u64>, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let progress = match progress_fraction(loaded, total) {
            Some(value) => Progress::new("history-progress", value).fill(),
            None => Progress::new("history-progress", 0.).indeterminate().fill(),
        };
        // The folded lanes are part of the history's own progress line: say
        // how many the graph is hiding while the rows stream in.
        let folded = if self.collapse && self.geometry.folded_count > 0 {
            format!(" · {} idle lanes folded", self.geometry.folded_count)
        } else {
            String::new()
        };
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(8.))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .child(Spinner::new("history-spinner", px(14.)))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text)
                    .child(format!(
                        "Loading… {} commits{folded}",
                        format_count(loaded as usize)
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(progress),
            )
            .when_some(percent_text(loaded, total), |this, percent| {
                this.child(div().flex_none().text_color(colors.muted).child(percent))
            })
            .child(
                Button::new("history-cancel")
                    .label("Cancel")
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
            )
    }

    /// Ten placeholder rows while the first page of history is on its way.
    fn render_skeletons(&self, columns: Columns, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        div()
            .w_full()
            .flex()
            .flex_col()
            .children((0..SKELETON_ROWS).map(|index| {
                let mut row = div()
                    .w_full()
                    .h(cx.theme().row_height)
                    .pl(px(PAD_LEFT))
                    .pr(px(PAD_RIGHT))
                    .flex()
                    .items_center()
                    .gap(px(COL_GAP))
                    .child(graph_skeleton(px(columns.graph), colors))
                    // The summary bar is a percentage of a fixed reference
                    // width; clip it so a narrow pane's bar cannot run into
                    // the author column.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(Skeleton::new(
                                format!("history-skeleton-summary-{index}"),
                                skeleton_width(SkeletonColumn::Summary, index, columns),
                            )),
                    );
                if let Some(author) = columns.author {
                    row = row.child(div().w(px(author)).flex_none().child(Skeleton::new(
                        format!("history-skeleton-author-{index}"),
                        skeleton_width(SkeletonColumn::Author, index, columns),
                    )));
                }
                row = row.child(div().w(px(columns.date)).flex_none().child(Skeleton::new(
                    format!("history-skeleton-date-{index}"),
                    skeleton_width(SkeletonColumn::Date, index, columns),
                )));
                if let Some(hash) = columns.hash {
                    row = row.child(div().w(px(hash)).flex_none().child(Skeleton::new(
                        format!("history-skeleton-hash-{index}"),
                        skeleton_width(SkeletonColumn::Hash, index, columns),
                    )));
                }
                div().w_full().px(px(SIDE_MARGIN)).child(row)
            }))
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        let colors = cx.theme().colors;
        let ui_theme = cx.theme().clone();
        let row_height = cx.theme().row_height;
        let row_font = cx.theme().ui_font_size - px(1.);
        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let columns = self.columns;
        let geometry = self.geometry.clone();
        let hovered = self.hovered;
        let head = self.head_oid();
        let selected_index = self.selected;
        // The row states follow the table in the design: a selected row is
        // `selection` only while the list itself has focus, and the keyboard
        // ring appears only for a keyboard-modality focus.
        let list_focused = self.focus.is_focused(window);
        let ring_visible = focus::focus_visible(&self.focus, window, cx);
        let now = unix_now();
        let summary_width = self.summary_width;
        let items = &self.items;
        let badges = &self.badges;
        let badge_widths = &mut self.badge_widths;
        let search_matches = &self.search_matches;
        range
            .map(|ix| {
                let item = &items[ix];
                let row = item.row.clone();
                let selected = selected_index == Some(ix);
                let is_hovered = hovered == Some(ix) && !selected;
                let c = &item.commit;
                let fields = search_matches
                    .binary_search_by_key(&ix, |found| found.row)
                    .ok()
                    .map(|position| &search_matches[position].fields);
                let background = if selected {
                    if list_focused {
                        colors.selection
                    } else {
                        colors.selection_inactive
                    }
                } else if is_hovered {
                    colors.hover
                } else {
                    colors.bg
                };
                let summary_color = if selected && list_focused {
                    colors.selection_fg
                } else {
                    colors.text
                };
                let secondary = ListRow::secondary_color(colors, selected, list_focused);
                let is_head = head == Some(c.id);
                let row_theme = ui_theme.clone();
                let row_geometry = geometry.clone();
                let summary_ranges = fields.map(|f| f.summary.as_slice()).unwrap_or(&[]);
                let author_ranges = fields.map(|f| f.author_name.as_slice()).unwrap_or(&[]);

                // The 6px gutter lives on the wrapper: a uniform_list lays
                // each item out with a definite width and does not stretch it,
                // so margins on the row itself would overflow.
                let mut row_div = div()
                    .id(ix)
                    .relative()
                    .h(row_height)
                    .w_full()
                    .pl(px(PAD_LEFT))
                    .pr(px(PAD_RIGHT))
                    .flex()
                    .items_center()
                    .gap(px(COL_GAP))
                    .text_size(row_font)
                    .rounded(R_MD)
                    .debug_selector(move || format!("history-row-{ix}"))
                    .bg(background)
                    .on_hover(cx.listener(move |view, entered: &bool, _, cx| {
                        // A leave only clears the row it belongs to: the next
                        // row's enter may already have run.
                        let next = if *entered {
                            Some(ix)
                        } else if view.hovered == Some(ix) {
                            None
                        } else {
                            view.hovered
                        };
                        if view.hovered != next {
                            view.hovered = next;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |view, event: &ClickEvent, window, cx| {
                        // Clicking focuses the list and selects the row; the
                        // ring stays hidden because the modality is the mouse.
                        let focus = view.focus.clone();
                        window.focus(&focus, cx);
                        view.select_row(ix, cx);
                        // A double click checks the row's branch out, or the
                        // commit itself when the row has no local branch.
                        if event.click_count() >= 2 {
                            view.checkout_row(ix, window, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            let focus = view.focus.clone();
                            window.focus(&focus, cx);
                            view.select_row(ix, cx);
                            view.open_row_menu(ix, event.position, window, cx);
                            // Keep the menu that just took focus.
                            window.prevent_default();
                        }),
                    )
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                // Clip to the graph column so lanes past its
                                // edge are cut instead of drawn over Summary.
                                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                    paint_row(
                                        &row,
                                        ix,
                                        bounds,
                                        background,
                                        &row_geometry,
                                        is_head,
                                        &row_theme,
                                        settings.graph_colored,
                                        px(f32::from(settings.graph_line_width)),
                                        window,
                                    )
                                })
                            },
                        )
                        .w(px(columns.graph))
                        .h_full()
                        .flex_none(),
                    );

                // The summary cell: ref badges (fixed size) first, then the
                // summary text, which takes the room that is left.
                let specs = badges.get(&c.id).map(Vec::as_slice).unwrap_or(&[]);
                let fit = if specs.is_empty() {
                    BadgeFit {
                        shown: 0,
                        collapsed: false,
                    }
                } else {
                    let widths: Vec<f32> = specs
                        .iter()
                        .map(|spec| {
                            f32::from(measure_badge(window, &spec.label, spec.kind, badge_widths))
                        })
                        .collect();
                    // The most hidden badges the "+N" label can stand for.
                    let plus = SharedString::from(format!("+{}", specs.len()));
                    let plus_width = f32::from(measure_badge(
                        window,
                        &plus,
                        RefKind::Overflow,
                        badge_widths,
                    ));
                    badge_overflow(&widths, summary_width, BADGE_GAP, plus_width)
                };

                let mut summary = div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(BADGE_GAP));
                for (index, spec) in specs.iter().take(fit.shown).enumerate() {
                    summary = summary.child(
                        RefBadge::new(format!("history-badge-{ix}-{index}"), spec.label.clone())
                            .kind(spec.kind)
                            .max_label_width(px(BADGE_LABEL_MAX))
                            .tooltip(spec.tooltip.clone()),
                    );
                }
                if fit.collapsed {
                    let hidden = specs[fit.shown..]
                        .iter()
                        .map(|spec| spec.tooltip.as_ref())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let count = specs.len() - fit.shown;
                    summary = summary.child(
                        RefBadge::new(format!("history-badge-{ix}-more"), format!("+{count}"))
                            .kind(RefKind::Overflow)
                            .tooltip(hidden),
                    );
                }
                row_div = row_div.child(summary.child({
                    let mut cell = div()
                        .id(format!("history-summary-{ix}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(FS_BASE)
                        .text_color(summary_color)
                        .when(is_head, |text| text.font_weight(FontWeight::BOLD))
                        .tooltip(Tooltip::text(c.summary.clone()));
                    // Only a matching cell pays for styled runs; the plain
                    // path keeps the cheap shared string.
                    cell = if summary_ranges.is_empty() {
                        cell.child(c.summary.clone())
                    } else {
                        let style = TextStyle {
                            color: summary_color,
                            font_family: FONT_UI.into(),
                            font_size: FS_BASE.into(),
                            font_weight: if is_head {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            },
                            ..TextStyle::default()
                        };
                        cell.child(highlighted_text(
                            &c.summary,
                            summary_ranges,
                            &style,
                            colors.accent_soft,
                        ))
                    };
                    cell
                }));
                if let Some(author) = columns.author {
                    row_div = row_div.child(
                        div()
                            .id(format!("history-author-{ix}"))
                            .w(px(author))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(BADGE_GAP))
                            .text_color(secondary)
                            .tooltip(Tooltip::text(c.author_name.clone()))
                            .child(
                                Avatar::new(c.author_email.clone(), px(16.))
                                    .name(c.author_name.clone()),
                            )
                            .child({
                                let mut cell = div().flex_1().min_w_0().truncate();
                                cell = if author_ranges.is_empty() {
                                    cell.child(c.author_name.clone())
                                } else {
                                    let style = TextStyle {
                                        color: secondary,
                                        font_family: FONT_UI.into(),
                                        font_size: FS_SM.into(),
                                        ..TextStyle::default()
                                    };
                                    cell.child(highlighted_text(
                                        &c.author_name,
                                        author_ranges,
                                        &style,
                                        colors.accent_soft,
                                    ))
                                };
                                cell
                            }),
                    );
                }
                row_div = row_div.child(
                    div()
                        .id(format!("history-date-{ix}"))
                        .w(px(columns.date))
                        .flex_none()
                        .truncate()
                        .text_color(secondary)
                        .tooltip(Tooltip::text(format_time_tooltip(c.author_time)))
                        .child(match settings.date_format {
                            crate::settings::DateFormat::Relative => {
                                relative_time(now, c.author_time.max(0) as u64)
                            }
                            crate::settings::DateFormat::Absolute => {
                                crate::format::absolute_time(c.author_time)
                            }
                        }),
                );
                if let Some(hash) = columns.hash {
                    let short = short_hash(&c.id);
                    // The hash field only matches at the start of the full
                    // hash, so the highlight covers the visible prefix.
                    let hash_ranges = fields
                        .and_then(|fields| fields.hash.as_ref())
                        .map(|range| 0..range.end.min(short.len()))
                        .filter(|range| !range.is_empty())
                        .into_iter()
                        .collect::<Vec<_>>();
                    row_div = row_div.child({
                        let mut cell = div()
                            .w(px(hash))
                            .flex_none()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_color(secondary);
                        cell = if hash_ranges.is_empty() {
                            cell.child(short.clone())
                        } else {
                            let style = TextStyle {
                                color: secondary,
                                font_family: FONT_MONO.into(),
                                font_size: FS_SM.into(),
                                ..TextStyle::default()
                            };
                            cell.child(highlighted_text(
                                &short,
                                &hash_ranges,
                                &style,
                                colors.accent_soft,
                            ))
                        };
                        cell
                    });
                }
                if ring_visible && selected {
                    row_div = row_div.child(focus::focus_ring(RingOffset::Inset2, R_MD, cx));
                }
                div().w_full().px(px(SIDE_MARGIN)).child(row_div)
            })
            .collect()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        uniform_list(
            "history",
            self.items.len(),
            cx.processor(|view, range, window, cx| view.render_rows(range, window, cx)),
        )
        .track_scroll(&self.scroll)
        .track_focus(&self.focus)
        .key_context("history-list")
        .flex_1()
        .min_h_0()
        .w_full()
        // The kit's arrow / Home / End actions are bound globally; the list
        // handles them only while its own focus handle is the focused one.
        .on_action(cx.listener(|view, _: &SelectNext, _, cx| view.step_selection(1, cx)))
        .on_action(cx.listener(|view, _: &SelectPrev, _, cx| view.step_selection(-1, cx)))
        .on_action(cx.listener(|view, _: &SelectFirst, _, cx| view.jump_selection(0, cx)))
        .on_action(cx.listener(|view, _: &SelectLast, _, cx| view.jump_selection(usize::MAX, cx)))
        .on_action(cx.listener(|view, _: &NextCommit, _, cx| view.step_selection(1, cx)))
        .on_action(cx.listener(|view, _: &PrevCommit, _, cx| view.step_selection(-1, cx)))
        .on_action(cx.listener(|view, _: &PageDownCommits, _, cx| view.page_selection(true, cx)))
        .on_action(cx.listener(|view, _: &PageUpCommits, _, cx| view.page_selection(false, cx)))
        .on_action(cx.listener(|view, _: &CopyCommitHash, _, cx| view.copy_selected_hash(cx)))
        .into_any_element()
    }

    fn render_body(&mut self, columns: Columns, cx: &mut Context<Self>) -> AnyElement {
        // The state is small and cloned so the render helpers can take `&mut
        // self` for their click listeners.
        let state = self.state.clone();
        match state {
            LoadState::Loading { total } => {
                let loaded = self.items.len() as u64;
                let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
                column = column.child(self.render_load_bar(loaded, total, cx));
                if self.items.is_empty() {
                    column
                        .child(self.render_skeletons(columns, cx))
                        .into_any_element()
                } else {
                    column.child(self.render_list(cx)).into_any_element()
                }
            }
            LoadState::Loaded => {
                if self.items.is_empty() {
                    self.render_no_commits(cx)
                } else {
                    self.render_list(cx)
                }
            }
            LoadState::Cancelled => {
                if self.items.is_empty() {
                    self.render_cancelled(cx)
                } else {
                    // Rows already arrived: keep them and say a retry is
                    // possible instead of showing the full empty state.
                    let colors = cx.theme().colors;
                    let strip = self.status_strip(
                        "History is incomplete — loading was cancelled".into(),
                        IconName::Warn,
                        colors.warning,
                        false,
                        cx,
                    );
                    let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
                    column = column.child(strip);
                    column.child(self.render_list(cx)).into_any_element()
                }
            }
            LoadState::Failed(failure) => {
                if self.items.is_empty() {
                    self.render_failure(&failure, cx)
                } else {
                    self.render_incomplete(&failure, cx)
                }
            }
        }
    }

    /// Switches the repository view to Local changes, as the toolbar segment does.
    fn show_local_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| workspace.show_changes(window, cx));
        }
    }

    /// Writes the starter `.gitignore` when it is missing, then shows it as an
    /// untracked file in Local changes.
    fn add_gitignore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone().unwrap_or_else(|| self.path.clone());
        let path = root.join(".gitignore");
        if path.exists() {
            ui::toast::show(ToastKind::Info, ".gitignore already exists", window, cx);
        } else if let Err(error) = std::fs::write(&path, STARTER_GITIGNORE) {
            ui::toast::show(
                ToastKind::Error,
                format!("Could not add a .gitignore: {error}"),
                window,
                cx,
            );
            return;
        } else {
            ui::toast::show(ToastKind::Ok, "Added a .gitignore", window, cx);
        }
        self.show_local_changes(window, cx);
    }

    /// Empty repository: an invitation to make the first commit.
    fn render_no_commits(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        let body = BodyText::new()
            .plain("This repository is empty. Stage some files and make your first commit to see history here.");
        let actions = div()
            .flex()
            .gap(px(8.))
            .child(
                Button::primary("empty-no-commits-primary")
                    .label("Go to Local changes")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_local_changes(window, cx);
                    })),
            )
            .child(
                Button::new("empty-no-commits-secondary")
                    .label("Add .gitignore")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_gitignore(window, cx);
                    })),
            );
        centred(
            EmptyState::new("No commits yet")
                .icon(IconName::Git)
                .icon_color(colors.muted)
                .body(body.element(&style))
                .actions(actions),
        )
    }

    /// The user stopped the load before it finished.
    fn render_cancelled(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let actions = div().flex().gap(px(8.)).child(
            Button::primary("empty-cancelled-retry")
                .label("Retry")
                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
        );
        centred(
            EmptyState::new("Loading cancelled")
                .icon(IconName::Fetch)
                .icon_color(cx.theme().colors.muted)
                .text("The history was not loaded completely.")
                .actions(actions),
        )
    }

    fn render_failure(&mut self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        match failure {
            Failure::NotARepository(path) => {
                let path = path.display().to_string();
                let body = BodyText::new()
                    .code(&path)
                    .plain(" is not inside a git working tree.");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-not-a-repo-init")
                            .icon(IconName::Init)
                            .label("Initialize repository here")
                            .on_click(cx.listener(|this, _, _, cx| this.init_repository(cx))),
                    )
                    .child(
                        Button::new("empty-not-a-repo-open")
                            .label("Open another…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(workspace) = this.workspace.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.open_repository_dialog(window, cx);
                                    });
                                }
                            })),
                    );
                centred(
                    EmptyState::new("Not a git repository")
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::PathNotFound(path) => {
                let shown = path.display().to_string();
                let remove_path = path.clone();
                let body = BodyText::new().code(&shown).plain(" does not exist.");
                // Removing the dead entry is the way out: the welcome screen
                // shows the rest of the recent list again. Retry only repeats
                // the same answer.
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-path-missing-remove")
                            .label("Remove from recent")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                crate::update_settings(cx, |settings| {
                                    settings.remove_recent(&remove_path)
                                });
                                if let Some(workspace) = this.workspace.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.show_welcome(window, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new("empty-path-missing-open")
                            .label("Open another…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(workspace) = this.workspace.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.open_repository_dialog(window, cx);
                                    });
                                }
                            })),
                    );
                centred(
                    EmptyState::new("Folder not found")
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::GitNotFound => {
                let body = BodyText::new()
                    .plain("GitWarp could not find a usable git. The bundled copy is missing and there is no ")
                    .code("git")
                    .plain(" in PATH.");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-not-found-settings")
                            .icon(IconName::Gear)
                            .label("Open Git settings")
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(OpenGitSettings), cx);
                            })),
                    )
                    .child(self.retry_git_button("empty-git-not-found-retry", cx));
                centred(
                    EmptyState::new("git was not found")
                        .body(body.element(&style))
                        .icon(IconName::Err)
                        .icon_color(colors.error)
                        .actions(actions),
                )
            }
            Failure::GitTooOld {
                found,
                required,
                source,
                path,
            } => {
                let body = BodyText::new()
                    .plain("GitWarp needs git ")
                    .plain(&required.to_string())
                    .plain(" or newer. ")
                    .plain(source_label(*source))
                    .plain(" git at ")
                    .code(&path.display().to_string())
                    .plain(" reports ")
                    .plain(&found.to_string())
                    .plain(".");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-too-old-settings")
                            .icon(IconName::Gear)
                            .label("Open Git settings")
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(OpenGitSettings), cx);
                            })),
                    )
                    .child(self.retry_git_button("empty-git-too-old-retry", cx));
                centred(
                    EmptyState::new(failure.title())
                        .body(body.element(&style))
                        .icon(IconName::Warn)
                        .icon_color(colors.warning)
                        .actions(actions),
                )
            }
            Failure::Git {
                command,
                code,
                stderr,
            } => {
                let body = BodyText::new()
                    .plain("The command ")
                    .code(&format!("git {command}"));
                let body = match code {
                    Some(code) => body.plain(&format!(" failed with exit code {code}.")),
                    None => body.plain(" was stopped."),
                };
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-error-retry")
                            .icon(IconName::Fetch)
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("empty-git-error-copy")
                            .icon(IconName::Copy)
                            .label("Copy details")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let LoadState::Failed(failure) = &this.state {
                                    if let Some(details) = failure.copy_details() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(details));
                                    }
                                }
                            })),
                    );
                let state = EmptyState::new(failure.title())
                    .body(body.element(&style))
                    .icon(IconName::Err)
                    .icon_color(colors.error)
                    .actions(actions);
                // Git always says something on stderr; an empty box would only
                // be noise for a process that was killed.
                let state = if stderr.is_empty() {
                    state
                } else {
                    state.error(stderr.clone())
                };
                centred(state)
            }
            Failure::Other { message } => {
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-other-retry")
                            .icon(IconName::Fetch)
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("empty-other-copy")
                            .icon(IconName::Copy)
                            .label("Copy details")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let LoadState::Failed(failure) = &this.state {
                                    if let Some(details) = failure.copy_details() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(details));
                                    }
                                }
                            })),
                    );
                centred(
                    EmptyState::new(failure.title())
                        .icon(IconName::Err)
                        .icon_color(colors.error)
                        .error(message.clone())
                        .actions(actions),
                )
            }
        }
    }

    /// Retry through the workspace: it resolves git again and reopens the
    /// repository with the new handle.
    fn retry_git_button(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        let workspace = self.workspace.clone();
        Button::new(id)
            .label("Retry")
            .on_click(cx.listener(move |_, _, window, cx| {
                workspace
                    .update(cx, |workspace, cx| workspace.retry_git(window, cx))
                    .ok();
            }))
    }

    /// The log broke after rows had arrived: keep the rows and say what
    /// happened in a strip under the header.
    fn render_incomplete(&mut self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let text: SharedString = match failure {
            Failure::Git { command, .. } => {
                format!("History is incomplete: git {command} failed").into()
            }
            Failure::Other { message } => format!("History is incomplete: {message}").into(),
            _ => "History is incomplete".into(),
        };
        let colors = cx.theme().colors;
        let strip = self.status_strip(text, IconName::Err, colors.error, true, cx);
        let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
        column = column.child(strip);
        column.child(self.render_list(cx)).into_any_element()
    }

    /// One-line strip above the rows: what went wrong, Retry and (for a
    /// failure that has details) Copy details.
    fn status_strip(
        &mut self,
        text: SharedString,
        icon: IconName,
        color: gpui::Hsla,
        copy_details: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let mut strip = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Icon::new(icon).size(px(14.)).color(color))
            .child(text)
            .child(div().flex_1())
            .child(
                Button::new("history-strip-retry")
                    .label("Retry")
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
            );
        if copy_details {
            strip = strip.child(
                Button::new("history-strip-copy")
                    .label("Copy details")
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let LoadState::Failed(failure) = &this.state {
                            if let Some(details) = failure.copy_details() {
                                cx.write_to_clipboard(ClipboardItem::new_string(details));
                            }
                        }
                    })),
            );
        }
        strip
    }
}

impl HistoryView {
    /// The whole table for a pane this wide: the header, then the body for the
    /// current state. Called with the width the pane was actually assigned.
    fn render_at(&mut self, pane_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let columns = columns(pane_width, self.graph_width);
        // The virtualized rows are painted after this and must agree with the
        // header.
        self.columns = columns;
        self.summary_width = summary_width(pane_width, columns);
        let body = self.render_body(columns, cx);
        // The column header stays above every state.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .child(self.render_header(columns, cx))
            .child(body)
            .into_any_element()
    }
}

/// Closing the view (or switching repositories) must not leave `git log`
/// running: the task is dropped with the view, but its blocking read may be
/// pages away, so the children are killed here too.
impl Drop for HistoryView {
    fn drop(&mut self) {
        self.log_killer.kill();
        self.count_killer.kill();
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The pane's own width is only known after layout. A container query
        // hands it back in prepaint, so the table is rebuilt for the width the
        // pane really got — no measure-and-notify round trip, no stale frame.
        let view = cx.entity().downgrade();
        container_query(move |size, _window, cx| {
            view.update(cx, |view, cx| view.render_at(f32::from(size.width), cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
    }
}

/// A body sentence with inline code runs, laid out as one wrapped paragraph.
struct BodyText {
    text: String,
    code: Vec<std::ops::Range<usize>>,
}

impl BodyText {
    fn new() -> Self {
        Self {
            text: String::new(),
            code: Vec::new(),
        }
    }

    /// A plain run in the body font.
    fn plain(mut self, text: &str) -> Self {
        self.text.push_str(text);
        self
    }

    /// A run of inline code, drawn in the mono font at the same colour.
    fn code(mut self, text: &str) -> Self {
        let start = self.text.len();
        self.text.push_str(text);
        self.code.push(start..self.text.len());
        self
    }

    /// The styled runs for [`Self::element`]: one per plain or code stretch.
    ///
    /// `StyledText::with_font_family_overrides` has no visible effect in this
    /// GPUI snapshot, so the runs are built explicitly; only the family
    /// differs, the size comes from the parent's text style.
    fn runs(&self, style: &TextStyle) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut start = 0;
        for range in &self.code {
            if start < range.start {
                runs.push(style.to_run(range.start - start));
            }
            let mut code_style = style.clone();
            code_style.font_family = FONT_MONO.into();
            runs.push(code_style.to_run(range.len()));
            start = range.end;
        }
        if start < self.text.len() {
            runs.push(style.to_run(self.text.len() - start));
        }
        runs
    }

    fn element(self, style: &TextStyle) -> StyledText {
        let runs = self.runs(style);
        StyledText::new(self.text).with_runs(runs)
    }
}

/// The empty-state body's text style: muted, 12px, the UI font. The code runs
/// are the same, only with the mono family.
fn body_style(colors: ThemeColors) -> TextStyle {
    TextStyle {
        color: colors.muted,
        font_family: FONT_UI.into(),
        font_size: FS_SM.into(),
        ..TextStyle::default()
    }
}

/// The empty states share the centre of the area under the header.
fn centred(state: EmptyState) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(FS_SM)
        .child(state)
        .into_any_element()
}

/// The graph column's placeholder: a lane line and one dot per row.
fn graph_skeleton(width: Pixels, colors: ThemeColors) -> Div {
    // The lane centre, plus the half-width of the 2 px line the placeholder
    // draws there. (The skeleton's dot is centred on the same x.)
    let centre = theme::LANE_WIDTH * 0.5 + px(1.);
    div()
        .w(width)
        .h_full()
        .flex_none()
        .relative()
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(centre - px(1.))
                .w(px(2.))
                .bg(colors.border_strong),
        )
        .child(
            div()
                .absolute()
                .left(centre - px(4.5))
                .top(px(8.5))
                .w(px(9.))
                .h(px(9.))
                .rounded(px(4.5))
                .bg(colors.border_strong),
        )
}

/// A skeleton bar's width in pixels for its column.
fn skeleton_width(column: SkeletonColumn, index: usize, columns: Columns) -> Pixels {
    let fraction = skeleton_percent(column, index) / 100.;
    let width = match column {
        SkeletonColumn::Summary => SKELETON_SUMMARY_REF,
        SkeletonColumn::Author => columns.author.unwrap_or(0.),
        SkeletonColumn::Date => columns.date,
        SkeletonColumn::Hash => columns.hash.unwrap_or(0.),
    };
    px(width * fraction)
}

fn next_page(
    mut stream: LogStream,
    mut graph: GraphBuilder<Oid>,
) -> (LogStream, GraphBuilder<Oid>, Page) {
    let page = stream
        .next_chunk(PAGE)
        .map_err(|e| Failure::from_error(&e))
        .map(|commits| {
            commits
                .into_iter()
                .map(|c| {
                    let row = graph.push(c.id, &c.parents);
                    (c, row)
                })
                .collect()
        });
    (stream, graph, page)
}

/// Seconds since the unix epoch; 0 if the clock is before it.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The 7-character abbreviated hash the history table shows.
fn short_hash(id: &Oid) -> String {
    let mut hash = id.to_string();
    hash.truncate(7);
    hash
}

/// One cell's text with every matched run painted `highlight`.
///
/// GPUI text runs carry a background colour but no corner radius, so the
/// design's 3px radius is dropped: the highlight is a plain rectangle.
fn highlighted_text(
    text: &str,
    ranges: &[std::ops::Range<usize>],
    style: &TextStyle,
    highlight: gpui::Hsla,
) -> StyledText {
    StyledText::new(text.to_owned()).with_runs(text_runs(text, ranges, style, highlight))
}

/// The runs behind [`highlighted_text`]: plain stretches in `style`, matched
/// stretches in `style` plus the background. Ranges that are empty, outside
/// the text or not on a character boundary are ignored.
fn text_runs(
    text: &str,
    ranges: &[std::ops::Range<usize>],
    style: &TextStyle,
    highlight: gpui::Hsla,
) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut cursor = 0;
    for range in ranges {
        let start = range.start.max(cursor);
        let end = range.end.min(text.len());
        if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        if cursor < start {
            runs.push(style.to_run(start - cursor));
        }
        let mut matched = style.clone();
        matched.background_color = Some(highlight);
        runs.push(matched.to_run(end - start));
        cursor = end;
    }
    if cursor < text.len() {
        runs.push(style.to_run(text.len() - cursor));
    }
    runs
}

/// The date cell's tooltip: `YYYY-MM-DD HH:MM UTC`.
fn format_time_tooltip(unix: i64) -> String {
    format!("{} UTC", format_time(unix))
}

/// `YYYY-MM-DD HH:MM` in UTC, for the date cell's tooltip.
fn format_time(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn formats_unix_time() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_time_tooltip(0), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn the_hash_is_abbreviated_to_seven_characters() {
        let oid = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(short_hash(&oid), "0123456");
    }

    fn oid(byte: u8) -> Oid {
        Oid::from_hex(&format!("{byte:02x}").repeat(20)).unwrap()
    }

    fn reference(kind: git_core::RefKind, full_name: &str, name: &str, target: Oid) -> Ref {
        Ref {
            full_name: full_name.to_owned(),
            name: name.to_owned(),
            kind,
            target,
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    #[test]
    fn badges_order_head_locals_remotes_and_tags_without_duplicating_head() {
        let head_oid = oid(1);
        let refs = vec![
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/main",
                "main",
                head_oid,
            ),
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/zeta",
                "zeta",
                head_oid,
            ),
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/alpha",
                "alpha",
                head_oid,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/main",
                "origin/main",
                head_oid,
            ),
            // Symbolic remote refs are duplicates of a real branch.
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/HEAD",
                "origin/HEAD",
                head_oid,
            ),
            reference(git_core::RefKind::Tag, "refs/tags/v1", "v1", head_oid),
            reference(git_core::RefKind::Stash, "refs/stash", "stash", head_oid),
            reference(
                git_core::RefKind::Other,
                "refs/notes/commits",
                "refs/notes/commits",
                head_oid,
            ),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(head_oid),
        };
        let badges = build_badges(Some(&head), &refs);
        let row = &badges[&head_oid];
        let labels: Vec<(&RefKind, &str)> = row
            .iter()
            .map(|spec| (&spec.kind, spec.label.as_ref()))
            .collect();
        assert_eq!(
            labels,
            vec![
                (&RefKind::Head, "main"),
                (&RefKind::Local, "alpha"),
                (&RefKind::Local, "zeta"),
                (&RefKind::Remote, "origin/main"),
                (&RefKind::Tag, "v1"),
            ]
        );
        // The HEAD badge names its branch's full ref.
        assert_eq!(row[0].tooltip.as_ref(), "refs/heads/main");
        // Every other badge points at its own full ref name.
        assert_eq!(row[2].tooltip.as_ref(), "refs/heads/zeta");
        assert_eq!(row[3].tooltip.as_ref(), "refs/remotes/origin/main");
    }

    #[test]
    fn detached_head_gets_a_head_badge_and_keeps_the_branches() {
        let detached = oid(2);
        let branch = oid(3);
        let refs = vec![reference(
            git_core::RefKind::LocalBranch,
            "refs/heads/main",
            "main",
            branch,
        )];
        let badges = build_badges(Some(&Head::Detached(detached)), &refs);
        assert_eq!(
            badges[&detached],
            vec![BadgeSpec {
                kind: RefKind::Head,
                label: "HEAD".into(),
                tooltip: "HEAD".into(),
            }]
        );
        assert_eq!(badges[&branch][0].kind, RefKind::Local);
    }

    #[test]
    fn an_unborn_branch_has_no_head_badge() {
        let refs = vec![reference(
            git_core::RefKind::LocalBranch,
            "refs/heads/main",
            "main",
            oid(4),
        )];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let badges = build_badges(Some(&head), &refs);
        assert!(badges.is_empty(), "the current branch is not drawn twice");
    }

    #[test]
    fn badge_overflow_keeps_room_for_text_and_head() {
        let widths = [100., 100., 100.];
        // All three fit and leave the 80 px text minimum.
        assert_eq!(
            badge_overflow(&widths, 500., 6., 30.),
            BadgeFit {
                shown: 3,
                collapsed: false
            }
        );
        // Two badges leave the text its room, but a third would not; the
        // hidden one needs the +N badge, which does not fit beside two.
        assert_eq!(
            badge_overflow(&widths, 300., 6., 30.),
            BadgeFit {
                shown: 1,
                collapsed: true
            }
        );
        // A wide cell fits the two badges and the +N.
        assert_eq!(
            badge_overflow(&widths, 340., 6., 30.),
            BadgeFit {
                shown: 2,
                collapsed: true
            }
        );
    }

    #[test]
    fn badge_overflow_always_shows_head() {
        assert_eq!(
            badge_overflow(&[400., 100.], 100., 6., 30.),
            BadgeFit {
                shown: 1,
                collapsed: true
            }
        );
        assert_eq!(
            badge_overflow(&[], 100., 6., 30.),
            BadgeFit {
                shown: 0,
                collapsed: false
            }
        );
        // One badge is never collapsed into a "+N".
        assert_eq!(
            badge_overflow(&[400.], 100., 6., 30.),
            BadgeFit {
                shown: 1,
                collapsed: false
            }
        );
    }

    #[test]
    fn summary_width_subtracts_the_fixed_columns_and_gaps() {
        let wide = columns(1200., GRAPH_WIDTH_DEFAULT);
        assert_eq!(summary_width(1200., wide), 674.);
        let medium = columns(560., GRAPH_WIDTH_DEFAULT);
        assert_eq!(
            summary_width(560., medium),
            560. - 2. * SIDE_MARGIN
                - PAD_LEFT
                - PAD_RIGHT
                - medium.graph
                - medium.author.unwrap()
                - medium.date
                - 3. * COL_GAP
        );
        // A pane narrower than its fixed columns leaves nothing, not a
        // negative width.
        assert_eq!(summary_width(10., wide), 0.);
    }

    #[test]
    fn columns_drop_the_wide_columns_at_the_container_queries() {
        // 560 px is the design's first breakpoint: Hash goes, Author and Date
        // take their narrow widths; the default graph width still fits.
        let medium = columns(560., GRAPH_WIDTH_DEFAULT);
        assert_eq!(medium.hash, None);
        assert_eq!(medium.author, Some(COL_AUTHOR_NARROW));
        assert_eq!(medium.date, COL_DATE_NARROW);
        assert_eq!(medium.graph, GRAPH_WIDTH_DEFAULT);

        // 460 px is the second: Author goes too.
        let narrow = columns(460., GRAPH_WIDTH_DEFAULT);
        assert_eq!(narrow.hash, None);
        assert_eq!(narrow.author, None);
        assert_eq!(narrow.date, COL_DATE_NARROW);
        assert_eq!(narrow.graph, GRAPH_WIDTH_DEFAULT);

        // A wide pane keeps all five columns.
        let wide = columns(1200., GRAPH_WIDTH_DEFAULT);
        assert_eq!(wide.author, Some(COL_AUTHOR));
        assert_eq!(wide.date, COL_DATE);
        assert_eq!(wide.hash, Some(COL_HASH));
    }

    #[test]
    fn columns_use_the_user_graph_width_at_the_three_pane_widths() {
        // The user width is the column width while it fits the pane's share.
        assert_eq!(columns(1200., 400.).graph, 400.);
        // A 300 px user width still fits the 40 % share at 560 px.
        assert_eq!(columns(560., 200.).graph, 200.);
        assert_eq!(columns(460., 150.).graph, 150.);

        // Above 560 px the 40 % share and the 480 cap bound it.
        assert_eq!(columns(800., 400.).graph, 320.);
        assert_eq!(columns(1200., 9999.).graph, GRAPH_MAX_WIDE);
        // At and below 560 / 460 the pane's share narrows it further.
        assert_eq!(columns(560., 480.).graph, GRAPH_SHARE_MEDIUM * 560.);
        assert_eq!(columns(460., 480.).graph, GRAPH_SHARE_NARROW * 460.);
        // The 40 px floor holds whatever the file or the pane says.
        assert_eq!(columns(1200., 1.).graph, GRAPH_MIN);
        assert_eq!(columns(300., 480.).graph, GRAPH_SHARE_NARROW * 300.);
        assert_eq!(columns(100., 480.).graph, GRAPH_MIN);
    }

    #[test]
    fn fitting_the_graph_leaves_room_and_stays_in_range() {
        assert_eq!(fit_width(0.), GRAPH_MIN);
        assert_eq!(fit_width(GRAPH_WIDTH_DEFAULT), 120.);
        assert_eq!(fit_width(412.), GRAPH_FIT_MAX);
        // A wide graph (241 lanes, 3374 px) still fits at the 420 px cap.
        assert_eq!(fit_width(3374.), GRAPH_FIT_MAX);
    }

    #[test]
    fn clipped_lanes_count_the_centres_past_the_column_edge() {
        let geometry = ColumnGeometry {
            xs: vec![7., 21., 35., 49.],
            folded: vec![false; 4],
            width: 56.,
            folded_count: 0,
        };
        // A centre exactly on the edge is not clipped.
        assert_eq!(clipped_lanes(&geometry, 56.), 0);
        assert_eq!(clipped_lanes(&geometry, 28.), 2);
        assert_eq!(clipped_lanes(&geometry, 21.), 2);
        assert_eq!(clipped_lanes(&geometry, 7.), 3);
    }

    #[test]
    fn skeleton_widths_are_cell_percentages() {
        let wide = columns(1200., GRAPH_WIDTH_DEFAULT);
        assert_eq!(skeleton_width(SkeletonColumn::Summary, 0, wide), px(144.));
        assert_eq!(
            skeleton_width(SkeletonColumn::Summary, 1, wide),
            px(SKELETON_SUMMARY_REF * 0.77)
        );
        assert_eq!(skeleton_width(SkeletonColumn::Author, 0, wide), px(65.));
        assert_eq!(skeleton_width(SkeletonColumn::Date, 4, wide), px(62.4));
        assert_eq!(skeleton_width(SkeletonColumn::Hash, 4, wide), px(51.2));
        // The narrow table uses the narrow date column.
        let narrow = columns(460., GRAPH_WIDTH_DEFAULT);
        assert_eq!(
            skeleton_width(SkeletonColumn::Date, 4, narrow),
            px(COL_DATE_NARROW * (skeleton_percent(SkeletonColumn::Date, 4) / 100.))
        );
    }

    #[test]
    fn body_text_tracks_its_code_runs() {
        let body = BodyText::new()
            .plain("GitWarp needs git ")
            .plain("2.31.0")
            .plain(" or newer. System git at ")
            .code("/usr/bin/git")
            .plain(" reports 2.28.1.");
        assert_eq!(body.code.len(), 1);
        let range = body.code[0].clone();
        assert_eq!(&body.text[range], "/usr/bin/git");
    }

    #[test]
    fn highlight_runs_cover_the_text_and_paint_only_the_match() {
        let style = TextStyle {
            color: gpui::black(),
            font_family: FONT_UI.into(),
            font_size: FS_SM.into(),
            ..TextStyle::default()
        };
        let highlight = gpui::blue();
        let matched = [std::ops::Range { start: 4, end: 7 }];
        let runs = text_runs("Fix the parser", &matched, &style, highlight);
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![4, 3, 7]
        );
        assert_eq!(runs[0].background_color, None);
        assert_eq!(runs[1].background_color, Some(highlight));
        assert_eq!(runs[2].background_color, None);
        // The unhighlighted runs keep the cell's colour and font.
        assert!(runs.iter().all(|run| run.color == style.color));
        assert!(runs
            .iter()
            .all(|run| run.font.family == style.font().family));

        // Every occurrence is painted, with the plain gap in between.
        let runs = text_runs("ab ab", &[0..2, 3..5], &style, highlight);
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![2, 1, 2]
        );
        assert_eq!(runs[0].background_color, Some(highlight));
        assert_eq!(runs[1].background_color, None);
        assert_eq!(runs[2].background_color, Some(highlight));

        // A range that does not sit on a character boundary is ignored, and
        // the cell is still laid out whole.
        let matched = [std::ops::Range { start: 1, end: 3 }];
        let runs = text_runs("Ünï", &matched, &style, highlight);
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), 5);
        assert!(runs.iter().all(|run| run.background_color.is_none()));
    }

    #[test]
    fn body_runs_use_the_mono_font_for_code_only() {
        let body = BodyText::new()
            .plain("The command ")
            .code("git log --all --topo-order")
            .plain(" failed with exit code 128.");
        let style = body_style(ui::Theme::light().colors);
        let runs = body.runs(&style);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            (runs[0].len, runs[1].len, runs[2].len),
            (
                "The command ".len(),
                "git log --all --topo-order".len(),
                " failed with exit code 128.".len(),
            )
        );
        assert_eq!(runs[0].font.family.as_ref(), FONT_UI);
        assert_eq!(runs[1].font.family.as_ref(), FONT_MONO);
        assert_eq!(runs[2].font.family.as_ref(), FONT_UI);
        // Same colour across the sentence.
        assert!(runs.iter().all(|run| run.color == style.color));
        assert!(runs.iter().all(|run| run.font.weight == style.font_weight));
    }

    #[test]
    fn page_steps_are_one_viewport_minus_a_row() {
        // 26 px rows: a full page leaves one row of overlap, and a viewport
        // shorter than a row still moves by one.
        assert_eq!(page_rows(0., 26.), 1);
        assert_eq!(page_rows(26., 26.), 1);
        assert_eq!(page_rows(51., 26.), 1);
        assert_eq!(page_rows(52., 26.), 1);
        assert_eq!(page_rows(260., 26.), 9);
        assert_eq!(page_rows(259., 26.), 8);
        assert_eq!(page_rows(620., 26.), 22);
    }

    /// The test root: the history view plus the overlay stack, like the
    /// workspace renders them in the app.
    struct GraphMenuTestRoot {
        history: gpui::Entity<HistoryView>,
    }

    impl Render for GraphMenuTestRoot {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .child(self.history.clone())
                .child(ui::overlay::render_overlays(window, cx))
        }
    }

    /// Right-clicking the graph header opens the two-item graph menu, and its
    /// first item toggles the idle-lane collapse.
    #[gpui::test]
    fn right_clicking_the_graph_header_opens_the_graph_menu(cx: &mut gpui::TestAppContext) {
        use git_core::GitChoice;
        use gpui::{
            point, size, AppContext, Bounds, Modifiers, VisualTestContext,
            WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
        };

        let repo = crate::test_support::TempRepo::new("graph-header-menu");
        let git = crate::test_support::git();
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
                    // The history view is the window root so the test can read
                    // its state; the workspace only supplies the weak handle.
                    let workspace = cx.new(|cx| {
                        Workspace::new(None, Ok(git.clone()), GitChoice::Auto, window, cx)
                    });
                    let history = cx.new(|cx| {
                        HistoryView::open(
                            repo.path().to_path_buf(),
                            Ok(git.clone()),
                            workspace.downgrade(),
                            Rc::new(|_, _, _| {}),
                            cx,
                        )
                    });
                    cx.new(|_| GraphMenuTestRoot { history })
                },
            )
            .expect("open window")
        });

        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        let root = cx
            .update(|window, _| window.root::<GraphMenuTestRoot>().flatten())
            .expect("the history test root is the window root");
        let history = root.read_with(&cx, |root, _| root.history.clone());

        let header = cx
            .debug_bounds("history-graph-header")
            .expect("the graph header was not drawn");
        let position = header.center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                1,
                "right-clicking the graph header did not open a menu"
            );
        });

        // Item 0 sits 20 px below the menu's top-left corner: the 6 px panel
        // padding plus half of the 28 px first row.
        cx.simulate_click(
            point(position.x + px(40.), position.y + px(20.)),
            Modifiers::default(),
        );
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert!(
                view.collapse,
                "the first menu item did not toggle the idle-lane collapse"
            );
        });
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                0,
                "the menu stayed open after an item was clicked"
            );
        });
    }

    /// Makes one commit on the repository and returns its full hash.
    fn commit(repo: &std::path::Path, file: &str, message: &str) -> String {
        std::fs::write(repo.join(file), message).expect("write the committed file");
        let run = |args: &[&str]| -> String {
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
            String::from_utf8(out.stdout).expect("git prints UTF-8")
        };
        run(&["add", file]);
        run(&["commit", "-q", "-m", message]);
        run(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// Opens the history view as the window root on `repo` and returns it with
    /// its visual test context. The view loads on its own background tasks.
    fn open_history(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, gpui::Entity<HistoryView>) {
        use git_core::GitChoice;
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        let git = crate::test_support::git();
        cx.update(ui::init).expect("ui::init");
        cx.update(|cx| bind_history_keys(cx, &crate::settings::Settings::default()));
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
                    let workspace = cx.new(|cx| {
                        Workspace::new(None, Ok(git.clone()), GitChoice::Auto, window, cx)
                    });
                    let history = cx.new(|cx| {
                        HistoryView::open(
                            repo.path().to_path_buf(),
                            Ok(git.clone()),
                            workspace.downgrade(),
                            Rc::new(|_, _, _| {}),
                            cx,
                        )
                    });
                    cx.new(|_| GraphMenuTestRoot { history })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let root = cx
            .update(|window, _| window.root::<GraphMenuTestRoot>().flatten())
            .expect("the history test root is the window root");
        let history = root.read_with(&cx, |root, _| root.history.clone());
        (cx, history)
    }

    /// Clicking focuses the list and selects the row; the keyboard then moves
    /// the selection, Home/End jump to the ends and Ctrl+C copies the full
    /// hash of the selected commit.
    #[gpui::test]
    fn clicking_then_the_keyboard_selects_rows_and_copies_the_hash(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("history-keys");
        commit(repo.path(), "a.txt", "A");
        let b = commit(repo.path(), "b.txt", "B");
        commit(repo.path(), "c.txt", "C");

        let (mut cx, history) = open_history(&repo, cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.commit_count()),
            3,
            "the history did not finish loading"
        );

        // Clicking row 2 selects it and gives the list the keyboard.
        let row = cx
            .debug_bounds("history-row-2")
            .expect("the third history row was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert_eq!(view.selected, Some(2), "the clicked row is not selected");
        });
        cx.update(|window, cx| {
            assert!(
                history.read(cx).focus.is_focused(window),
                "clicking a row did not focus the history list"
            );
        });

        // ↓ / j move down and clamp at the last row; ↑ / k move back up.
        cx.simulate_keystrokes("up");
        assert_eq!(history.read_with(&cx, |view, _| view.selected), Some(1));
        cx.simulate_keystrokes("j");
        assert_eq!(history.read_with(&cx, |view, _| view.selected), Some(2));
        cx.simulate_keystrokes("j");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected),
            Some(2),
            "the selection wrapped past the last row"
        );
        cx.simulate_keystrokes("k");
        assert_eq!(history.read_with(&cx, |view, _| view.selected), Some(1));

        // Home / End jump to the loaded ends.
        cx.simulate_keystrokes("home");
        assert_eq!(history.read_with(&cx, |view, _| view.selected), Some(0));
        cx.simulate_keystrokes("end");
        assert_eq!(history.read_with(&cx, |view, _| view.selected), Some(2));

        // Ctrl+C copies the full 40-char hash of the selected commit.
        cx.simulate_keystrokes("home");
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("ctrl-c");
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("Ctrl+C did not write the hash to the clipboard");
        assert_eq!(copied, b, "the clipboard holds the wrong commit hash");
    }

    /// Right-clicking a row selects it and opens the row menu; its first item
    /// copies the row's full hash.
    #[gpui::test]
    fn right_clicking_a_row_opens_the_row_menu_and_copies_the_hash(cx: &mut gpui::TestAppContext) {
        use gpui::{point, Modifiers, MouseButton};

        let repo = crate::test_support::TempRepo::new("history-row-menu");
        commit(repo.path(), "a.txt", "A");
        let b = commit(repo.path(), "b.txt", "B");
        commit(repo.path(), "c.txt", "C");

        let (mut cx, history) = open_history(&repo, cx);
        let row = cx
            .debug_bounds("history-row-1")
            .expect("the second history row was not drawn");
        let position = row.center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert_eq!(
                view.selected,
                Some(1),
                "the right-clicked row is not selected"
            );
        });
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                1,
                "right-clicking a row did not open the row menu"
            );
        });

        // Item 0 sits 20 px below the menu's top-left corner: the 6 px panel
        // padding plus half of the 28 px first row.
        cx.simulate_click(
            point(position.x + px(40.), position.y + px(20.)),
            Modifiers::default(),
        );
        cx.run_until_parked();
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("the row menu's Copy item did not write the hash");
        assert_eq!(copied, b, "the row menu copied the wrong commit hash");
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                0,
                "the row menu stayed open after its item was clicked"
            );
        });
    }

    /// The status only says "No matches" once the whole history was scanned; a
    /// running or unfinished scan shows the partial count with a "+".
    #[gpui::test]
    fn the_search_status_is_partial_until_the_scan_finishes(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-search-status");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta");

        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, _| {
            view.search = Query::parse("zzz");
            view.selected = Some(0);
            // A scan in flight over rows that are not covered yet.
            view.search_matches.clear();
            view.search_scanned = 0;
            view.search_scanning = true;
            assert_eq!(view.search_status().unwrap().as_ref(), "– / 0+");
            // The scan ended, but rows are still unscanned: not "No matches".
            view.search_scanning = false;
            assert_eq!(
                view.search_status().unwrap().as_ref(),
                "– / 0+",
                "unscanned rows are still pending"
            );
            // A match found by a scan that has not covered every row.
            view.search_matches.push(history_search::HistoryMatch {
                row: 0,
                fields: Default::default(),
            });
            assert_eq!(view.search_status().unwrap().as_ref(), "1 / 1+");
            // Every row scanned and no match: only now is "No matches" true.
            view.search_scanned = view.items.len();
            assert_eq!(view.search_status().unwrap().as_ref(), "1 / 1");
            view.search_matches.clear();
            assert_eq!(view.search_status().unwrap().as_ref(), "No matches");
        });
    }

    /// A row the user picks cancels a pending search jump, so a match from a
    /// later page cannot yank the selection; the jump stays armed when the
    /// search itself picks the row.
    #[gpui::test]
    fn a_user_pick_cancels_the_deferred_search_jump(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-search-jump");
        commit(repo.path(), "a.txt", "A");
        commit(repo.path(), "b.txt", "B");

        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            view.search_jump = true;
            view.select_row(1, cx);
            assert!(
                !view.search_jump,
                "a user pick must cancel the deferred search jump"
            );
            view.search_jump = true;
            view.select_row_from_search(0, cx);
            assert!(
                view.search_jump,
                "a search pick must keep the deferred jump armed"
            );
        });
    }

    /// Retry during the debounce applies the field's text instead of dropping
    /// it with the cancelled timer.
    #[gpui::test]
    fn retry_during_the_debounce_keeps_the_typed_query(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-search-retry");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta");

        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            view.set_search_query("Alpha".to_owned(), cx);
            view.retry(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 1".to_owned()),
            "retry dropped the query typed before the debounce fired"
        );
    }

    /// The first selection is the HEAD row, even when HEAD is not row 0, and
    /// the list does not steal the focus for it.
    #[gpui::test]
    fn the_initial_selection_is_the_head_row(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-head");
        commit(repo.path(), "a.txt", "A");
        let b = commit(repo.path(), "b.txt", "B");
        // A newer commit on another branch puts HEAD at row 1 of `--all`.
        let git = |args: &[&str]| -> String {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
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
            String::from_utf8(out.stdout).expect("git prints UTF-8")
        };
        // The initial branch name comes from the user's git configuration.
        let main_branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .trim()
            .to_owned();
        git(&["checkout", "-q", "-b", "other"]);
        let c = commit(repo.path(), "c.txt", "C");
        git(&["checkout", "-q", &main_branch]);

        let (mut cx, history) = open_history(&repo, cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected),
            Some(1),
            "the initial selection is not the HEAD row"
        );
        history.read_with(&cx, |view, _| {
            assert_eq!(
                view.items[1].commit.id.to_string(),
                b,
                "HEAD is not the row the view selected"
            );
            assert_ne!(view.items[0].commit.id.to_string(), b);
            assert_eq!(view.items[0].commit.id.to_string(), c);
        });
        cx.update(|window, cx| {
            assert!(
                !history.read(cx).focus.is_focused(window),
                "the initial selection stole the focus"
            );
        });
    }

    fn commit_info(byte: u8) -> CommitInfo {
        CommitInfo {
            id: oid(byte),
            parents: Vec::new(),
            author_name: "Test User".to_owned(),
            author_email: "test@example.com".to_owned(),
            author_time: 0,
            summary: format!("Commit {byte}"),
        }
    }

    fn history_item(byte: u8) -> Item {
        let commit = commit_info(byte);
        let row = Row {
            id: commit.id,
            column: 0,
            color: 0,
            above: Vec::new(),
            below: Vec::new(),
            width: 1,
            is_tip: true,
            is_merge: false,
        };
        Item {
            commit: Arc::new(commit),
            row: Rc::new(row),
        }
    }

    /// A pending reveal rides along until a later page brings the commit; the
    /// page scan then finds it, clears the reveal and reports its row.
    #[test]
    fn a_pending_reveal_is_applied_when_a_later_page_brings_the_commit() {
        let mut pending = PendingReveal::default();
        let mut items = vec![history_item(1), history_item(2)];
        pending.set(oid(3));
        // The first page does not hold it: the reveal stays pending.
        assert_eq!(pending.check(&items, 0), None);
        assert_eq!(pending.oid(), Some(oid(3)));

        items.push(history_item(3));
        items.push(history_item(4));
        assert_eq!(pending.check(&items, 2), Some(2));
        assert_eq!(pending.oid(), None);
        // Applied once: the next page does not re-select it.
        assert_eq!(pending.check(&items, 4), None);
    }

    /// The per-page lookup only scans the rows that just arrived, never the
    /// rows already on screen.
    #[test]
    fn a_pending_reveal_only_scans_the_new_rows() {
        let mut pending = PendingReveal::default();
        let items = vec![history_item(1), history_item(2), history_item(3)];
        pending.set(oid(1));
        assert_eq!(pending.check(&items, 2), None);
        assert_eq!(pending.oid(), Some(oid(1)));
    }

    /// The sidebar's reveal selects a loaded commit and keeps the keyboard
    /// focus where it is (the sidebar's), instead of focusing the list.
    #[gpui::test]
    fn reveal_commit_selects_a_loaded_commit_without_focusing_the_list(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("history-reveal");
        let a = commit(repo.path(), "a.txt", "A");
        commit(repo.path(), "b.txt", "B");
        commit(repo.path(), "c.txt", "C");

        let (mut cx, history) = open_history(&repo, cx);
        // Newest first: row 2 is the oldest commit.
        let target = history.read_with(&cx, |view, _| view.items[2].commit.id);
        assert_eq!(target.to_string(), a);
        history.update(&mut cx, |view, cx| view.reveal_commit(target, cx));
        history.read_with(&cx, |view, _| {
            assert_eq!(
                view.selected,
                Some(2),
                "the revealed commit is not selected"
            );
            assert!(view.user_selected, "a reveal must count as a user pick");
            assert!(
                view.pending_reveal.oid().is_none(),
                "a loaded commit must not stay pending"
            );
        });
        cx.update(|window, cx| {
            assert!(
                !history.read(cx).focus.is_focused(window),
                "the reveal stole the keyboard focus from the sidebar"
            );
        });
    }

    /// A user pick or a search jump replaces the pending reveal, so a later
    /// page cannot yank the selection back.
    #[gpui::test]
    fn a_user_pick_clears_the_pending_reveal(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-reveal-pick");
        commit(repo.path(), "a.txt", "A");
        commit(repo.path(), "b.txt", "B");

        let (mut cx, history) = open_history(&repo, cx);
        let missing = oid(0x99);
        history.update(&mut cx, |view, cx| {
            view.pending_reveal.set(missing);
            view.select_row(0, cx);
            assert!(
                view.pending_reveal.oid().is_none(),
                "a user pick must clear the pending reveal"
            );
            view.pending_reveal.set(missing);
            view.select_row_from_search(1, cx);
            assert!(
                view.pending_reveal.oid().is_none(),
                "a search jump must clear the pending reveal"
            );
        });
    }
}
