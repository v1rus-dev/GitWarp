//! The history screen: column header + commit graph and commit list, loaded in
//! the background and painted through a virtualized list (only visible rows are
//! built).
//!
//! The view also owns the repository metadata the shell needs — HEAD and the
//! refs — because it is the view that opens the [`Repo`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::{
    actions, canvas, container_query, div, px, uniform_list, AnyElement, App, AsyncApp, ClickEvent,
    ClipboardItem, ContentMask, Context, Div, DragMoveEvent, EmptyView, EventEmitter, FocusHandle,
    FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, Render,
    ScrollStrategy, SharedString, StyledText, Task, TextRun, TextStyle, UniformListScrollHandle,
    WeakEntity, Window,
};

use forge::ServiceKind;
use git_core::{
    BranchLog, CommitInfo, Error, Git, Head, LogKiller, LogOptions, LogStream, Oid, Operation, Ref,
    Repo,
};
use git_graph::{ColumnGeometry, GraphBuilder, IdleLanes, Row};
use ui::actions::{Cancel, SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::focus::{self, RingOffset};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_SM, FS_XS, R_MD, R_SM};
use ui::theme::ThemeColors;
use ui::{
    menu, ActiveTheme, Avatar, Button, ButtonSize, CiGlyph, EmptyState, Icon, IconName, ListRow,
    MenuEntry, MenuItem, PrBadge, Progress, RefBadge, RefKind, Scrollbar, SelectableText, Skeleton,
    Spinner, ToastKind, Tooltip,
};

use crate::format::{format_count, relative_time};
use crate::graph_paint::{content_width, lane_centre, overdraw_mask_bounds, paint_row};
use crate::history_search::{self, Query};
use crate::history_state::{
    percent_text, progress_fraction, skeleton_percent, source_label, Failure, LoadState,
    SkeletonColumn,
};
use crate::ref_ops::{CardRefKind, CommitCard, RefAction};

/// What the history row menu asks the shell to do.
pub(crate) type RefActionHandler = Rc<dyn Fn(RefAction, &mut Window, &mut App)>;

/// What the shell does with a sidebar action chosen from a ref's menu.
pub(crate) type SidebarActionHandler = Rc<dyn Fn(SidebarAction, &mut Window, &mut App)>;

/// Opens the pull request popover of a local branch at a window position.
pub(crate) type PrClickHandler = Rc<dyn Fn(String, Point<Pixels>, &mut Window, &mut App)>;

/// What the history tells the shell outside its render and action callbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    /// The commits of the rows on screen (with a small margin) changed after
    /// the range stopped changing; the shell asks for their checks.
    VisibleRowsChanged(Vec<Oid>),
}

/// The service "Create pull request…" names for a local branch, from the
/// shell: `(branch, HEAD, refs)`. The History hands over its own HEAD and refs
/// because it is being updated when it asks and cannot be read back.
pub(crate) type PrServiceHook =
    Rc<dyn Fn(&str, Option<&Head>, &[Ref], &App) -> Option<ServiceKind>>;
use crate::hosting_status::{
    ci_label, HostingView, PrBadgeView, CI_VISIBLE_DEBOUNCE, CI_VISIBLE_MARGIN,
};
use crate::ref_menus::{self, RefMenuCtx, RefMenuEntry, RefRow, RefRowKind};
use crate::remote_accounts::RemoteTargets;
use crate::settings::{
    AUTHOR_WIDTH_DEFAULT, AUTHOR_WIDTH_MIN, DATE_WIDTH_DEFAULT, DATE_WIDTH_MIN,
    GRAPH_WIDTH_DEFAULT, GRAPH_WIDTH_MIN, HASH_WIDTH_DEFAULT, HASH_WIDTH_MIN,
};
use crate::sidebar::SidebarAction;
use crate::theme;
use crate::web_links::{self, HostLink};
use crate::workspace::{OpenGitSettings, Workspace};

/// Rows are delivered to the UI in pages of this size.
const PAGE: usize = 2_000;

/// How long the field waits after the last keystroke before the search runs.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// The gap between the history table's columns.
const COL_GAP: f32 = 18.;
/// The pane widths where the table drops a column, as the design's container
/// queries do: 560 px loses Hash, 460 px loses Author.
const PANE_MEDIUM: f32 = 560.;
const PANE_NARROW: f32 = 460.;
/// The summary column is flexible; a fixed column may never be dragged so far
/// that it leaves the summary less room than this.
const SUMMARY_MIN: f32 = 80.;
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
/// How faint the branch filter's fork-point row is drawn.
const FORK_ROW_OPACITY: f32 = 0.62;
/// The widest a badge label may get; a longer ref name is ellipsized.
const BADGE_LABEL_MAX: f32 = 160.;
/// A badge's label is wrapped in 6 px padding and a 1 px border on each side;
/// a kind with an icon adds the 11 px icon and its 3 px gap.
const BADGE_PADDING: f32 = 14.;
const BADGE_ICON: f32 = 14.;
/// The space between a local badge's label and its `+origin` remote suffix.
const BADGE_SUFFIX_GAP: f32 = 4.;
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
        CopyCommitHash,
        CreateBranchHere
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
        ui::actions::key_binding(cx, "b", CreateBranchHere, Some("history-list")),
    ]);
}

type Page = Result<Vec<(CommitInfo, Row<Oid>)>, Failure>;

/// Width reserved for a row's pull request badge: icon, `#1234` and padding.
const PR_BADGE_WIDTH: f32 = 46.;

/// The pull request badge of a local branch badge (the current branch's HEAD
/// badge included), with the branch it belongs to.
fn pr_badge_of<'a>(
    hosting: &'a HostingView,
    spec: &'a BadgeSpec,
) -> Option<(&'a str, &'a PrBadgeView)> {
    matches!(spec.kind, RefKind::Local | RefKind::Head)
        .then(|| {
            hosting
                .badge(&spec.label)
                .map(|badge| (spec.label.as_ref(), badge))
        })
        .flatten()
}

/// The muted tail of a merged local/remote badge: `+origin` for one remote,
/// `+2` for several (their menus still list every ref).
fn remote_suffix(remotes: &[SharedString]) -> Option<SharedString> {
    match remotes {
        [] => None,
        [remote] => Some(format!("+{remote}").into()),
        several => Some(format!("+{}", several.len()).into()),
    }
}

/// One ref badge on a history row, derived from HEAD and the refs once they
/// load — never per frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeSpec {
    pub kind: RefKind,
    /// The label inside the badge: the short branch/tag name, or `HEAD`.
    pub label: SharedString,
    /// The full ref name, shown on hover.
    pub tooltip: SharedString,
    /// The remotes whose branch of the same name is on this commit. A local
    /// branch and its remote counterparts then render as one badge with a
    /// `+origin` suffix instead of two badges.
    pub remotes: Vec<SharedString>,
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
///
/// A local branch (or HEAD's branch) and its same-named remote branches on the
/// same commit become one badge carrying [`BadgeSpec::remotes`]; a remote
/// branch on another commit stays a badge of its own.
pub fn build_badges(head: Option<&Head>, refs: &[Ref]) -> HashMap<Oid, Vec<BadgeSpec>> {
    let mut badges: HashMap<Oid, Vec<BadgeSpec>> = HashMap::new();
    let current_branch = match head {
        Some(Head::Branch { name, .. }) => Some(name.as_str()),
        Some(Head::Detached(_)) | None => None,
    };

    // Remote branches by the commit and local branch name they would check out
    // as, plus the ones that merge into a local or HEAD badge.
    let mut remote_names: HashMap<(Oid, &str), Vec<&str>> = HashMap::new();
    let mut merged: HashSet<&str> = HashSet::new();
    for reference in refs
        .iter()
        .filter(|reference| reference.kind == git_core::RefKind::RemoteBranch)
    {
        let Some((remote, leaf)) = reference.name.split_once('/') else {
            continue;
        };
        if remote.is_empty() || leaf.is_empty() || leaf == "HEAD" {
            continue;
        }
        remote_names
            .entry((reference.target, leaf))
            .or_default()
            .push(remote);
        let local_here = refs.iter().any(|local| {
            local.kind == git_core::RefKind::LocalBranch
                && local.target == reference.target
                && local.name == leaf
                && Some(local.name.as_str()) != current_branch
        });
        let head_here = matches!(
            head,
            Some(Head::Branch { name, target })
                if name == leaf && *target == Some(reference.target)
        );
        if local_here || head_here {
            merged.insert(reference.full_name.as_str());
        }
    }
    for remotes in remote_names.values_mut() {
        remotes.sort_unstable();
        remotes.dedup();
    }
    let remotes_for = |target: Oid, branch: &str| -> Vec<SharedString> {
        remote_names
            .get(&(target, branch))
            .map(|remotes| remotes.iter().map(|remote| (*remote).into()).collect())
            .unwrap_or_default()
    };

    match head {
        Some(Head::Branch {
            name,
            target: Some(target),
        }) => badges.entry(*target).or_default().push(BadgeSpec {
            kind: RefKind::Head,
            label: name.as_str().into(),
            tooltip: format!("refs/heads/{name}").into(),
            remotes: remotes_for(*target, name),
        }),
        Some(Head::Branch { target: None, .. }) => {}
        Some(Head::Detached(oid)) => badges.entry(*oid).or_default().push(BadgeSpec {
            kind: RefKind::Head,
            label: "HEAD".into(),
            tooltip: "HEAD".into(),
            remotes: Vec::new(),
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
            // A remote merged into a local/HEAD badge is drawn there.
            .filter(|reference| {
                !(kind == RefKind::Remote && merged.contains(reference.full_name.as_str()))
            })
            .collect();
        group.sort_by(|a, b| a.name.cmp(&b.name));
        for reference in group {
            let remotes = if kind == RefKind::Local {
                remotes_for(reference.target, &reference.name)
            } else {
                Vec::new()
            };
            badges.entry(reference.target).or_default().push(BadgeSpec {
                kind,
                label: reference.name.clone().into(),
                tooltip: reference.full_name.clone().into(),
                remotes,
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
    remote_suffix: Option<&SharedString>,
    cache: &mut HashMap<(SharedString, RefKind, Option<SharedString>), Pixels>,
) -> Pixels {
    let key = (label.clone(), kind, remote_suffix.cloned());
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
    let mut width = text + px(BADGE_PADDING + icon);
    if let Some(suffix) = remote_suffix {
        let mut font = gpui::font(FONT_UI);
        font.weight = FontWeight::MEDIUM;
        let run = TextRun {
            len: suffix.len(),
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let suffix = window
            .text_system()
            .layout_line(suffix, FS_XS, &[run], None)
            .width;
        width += px(BADGE_SUFFIX_GAP) + suffix;
    }
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

/// A fixed-width column the grips can resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeColumn {
    Graph,
    Author,
    Date,
    Hash,
}

impl ResizeColumn {
    /// The narrowest the column may get.
    fn min(self) -> f32 {
        match self {
            Self::Graph => GRAPH_MIN,
            Self::Author => AUTHOR_WIDTH_MIN,
            Self::Date => DATE_WIDTH_MIN,
            Self::Hash => HASH_WIDTH_MIN,
        }
    }

    /// The width a double-click restores.
    fn default_width(self) -> f32 {
        match self {
            Self::Graph => GRAPH_WIDTH_DEFAULT,
            Self::Author => AUTHOR_WIDTH_DEFAULT,
            Self::Date => DATE_WIDTH_DEFAULT,
            Self::Hash => HASH_WIDTH_DEFAULT,
        }
    }

    /// The drag direction: the graph's grip sits on its right edge, so moving
    /// right widens it; the others sit on their left edge, so moving right
    /// narrows them.
    fn drag_sign(self) -> f32 {
        match self {
            Self::Graph => 1.,
            Self::Author | Self::Date | Self::Hash => -1.,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Graph => "history-graph-grip",
            Self::Author => "history-author-grip",
            Self::Date => "history-date-grip",
            Self::Hash => "history-hash-grip",
        }
    }
}

/// The table layout for a history pane `pane_width` wide with the user widths
/// of the four resizable columns.
///
/// The hash column goes at 560 px and the author column at 460 px. Otherwise
/// the saved widths are used as they are (each at least its own minimum); no
/// fixed cap applies, only the pane's room and the summary's minimum.
pub fn columns(
    pane_width: f32,
    graph_width: f32,
    author_width: f32,
    date_width: f32,
    hash_width: f32,
) -> Columns {
    let graph = clamp_graph_width(graph_width);
    let author = clamp_width(author_width, AUTHOR_WIDTH_MIN, AUTHOR_WIDTH_DEFAULT);
    let date = clamp_width(date_width, DATE_WIDTH_MIN, DATE_WIDTH_DEFAULT);
    let hash = clamp_width(hash_width, HASH_WIDTH_MIN, HASH_WIDTH_DEFAULT);

    let mut table = if pane_width <= PANE_NARROW {
        Columns {
            graph,
            author: None,
            date,
            hash: None,
        }
    } else if pane_width <= PANE_MEDIUM {
        Columns {
            graph,
            author: Some(author),
            date,
            hash: None,
        }
    } else {
        Columns {
            graph,
            author: Some(author),
            date,
            hash: Some(hash),
        }
    };
    // Keep the summary's minimum even when a saved width no longer fits the
    // pane (a window may have shrunk since the drag). Later clamps only shrink
    // the total, so one pass leaves the table within the pane's room.
    table.graph = clamp_column_width(ResizeColumn::Graph, table.graph, pane_width, table);
    if let Some(author) = table.author {
        table.author = Some(clamp_column_width(
            ResizeColumn::Author,
            author,
            pane_width,
            table,
        ));
    }
    table.date = clamp_column_width(ResizeColumn::Date, table.date, pane_width, table);
    if let Some(hash) = table.hash {
        table.hash = Some(clamp_column_width(
            ResizeColumn::Hash,
            hash,
            pane_width,
            table,
        ));
    }
    table
}

/// Clamps a resized (or saved) column width to its own minimum and to the room
/// the pane leaves after the other fixed columns, the gaps and the summary's
/// minimum. There is deliberately no fixed maximum.
fn clamp_column_width(
    column: ResizeColumn,
    requested: f32,
    pane_width: f32,
    columns: Columns,
) -> f32 {
    let panel = pane_width - 2. * SIDE_MARGIN - PAD_LEFT - PAD_RIGHT;
    let count = 3 + usize::from(columns.author.is_some()) + usize::from(columns.hash.is_some());
    let gaps = (count - 1) as f32 * COL_GAP;
    let others =
        columns.graph + columns.date + columns.author.unwrap_or(0.) + columns.hash.unwrap_or(0.)
            - match column {
                ResizeColumn::Graph => columns.graph,
                ResizeColumn::Author => columns.author.unwrap_or(0.),
                ResizeColumn::Date => columns.date,
                ResizeColumn::Hash => columns.hash.unwrap_or(0.),
            };
    let max = (panel - gaps - SUMMARY_MIN - others).max(column.min());
    requested.clamp(column.min(), max)
}

/// A saved column width: finite and at least `min`, otherwise `default`.
fn clamp_width(width: f32, min: f32, default: f32) -> f32 {
    if width.is_finite() {
        width.max(min)
    } else {
        default
    }
}

/// The graph width clamped to its floor; a value from a broken file falls back
/// to the design default. There is no upper cap.
fn clamp_graph_width(graph_width: f32) -> f32 {
    clamp_width(graph_width, GRAPH_MIN, GRAPH_WIDTH_DEFAULT)
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
    geometry
        .xs
        .iter()
        .filter(|&&x| lane_centre(x) > column_width)
        .count()
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

/// The grip drag in flight: which column it resizes, where the pointer started
/// and the width it is resizing from. A click that never moved does not persist
/// anything.
struct GripDrag {
    column: ResizeColumn,
    start_x: f32,
    start_width: f32,
    pane_width: f32,
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

/// The History list narrowed to the commits authored on one local calendar
/// day (the activity strip's day filter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayFilter {
    /// Days since the Unix epoch of the local calendar date.
    pub day: i32,
    /// `Some(email)` keeps only that author (the strip's "Me").
    pub me: Option<String>,
}

#[derive(Debug)]
enum DayState {
    Loading,
    Ready,
    Failed(String),
}

/// The rows a [`DayFilter`] shows. They replace the live `items` for display,
/// selection and search while the filter is on; the streaming load keeps
/// filling `items` underneath, so clearing the filter is instant.
struct DayRows {
    filter: DayFilter,
    state: DayState,
    items: Vec<Item>,
    idle_lanes: IdleLanes,
    geometry: Rc<ColumnGeometry>,
    /// The one-off `git log` for the day; dropping it drops the answer.
    _task: Option<Task<()>>,
}

/// The most commits the branch filter lists for one branch.
const BRANCH_FILTER_LIMIT: usize = 50_000;

/// What [`HistoryView::set_branch_filter`] found for a branch.
struct BranchData {
    log: BranchLog,
    /// The main line's name, for the "Merged into {main}" button.
    main: Option<String>,
    rows: Vec<(CommitInfo, Row<Oid>)>,
}

/// The rows the History branch filter shows: the branch's own commits
/// (`git log --first-parent`), cut at the fork point. Like [`DayRows`] they
/// replace the live `items` for display, selection and search while the filter
/// is on.
struct BranchRows {
    branch: String,
    state: DayState,
    items: Vec<Item>,
    idle_lanes: IdleLanes,
    geometry: Rc<ColumnGeometry>,
    /// The fork-point commit: its row is dimmed and carries a "fork point" note.
    fork: Option<Oid>,
    /// Merge rows: the branch each one merged in (the "from {x}" chip).
    merged_from: HashMap<Oid, String>,
    /// The merge on main that brought the whole branch in, with main's name.
    merged_into: Option<(Oid, String)>,
    /// The one-off query; dropping it drops the answer.
    _task: Option<Task<()>>,
}

impl BranchRows {
    fn new(branch: String, state: DayState, collapse: bool, task: Option<Task<()>>) -> Self {
        Self {
            branch,
            state,
            items: Vec::new(),
            idle_lanes: IdleLanes::new(),
            geometry: Rc::new(IdleLanes::new().geometry(collapse)),
            fork: None,
            merged_from: HashMap::new(),
            merged_into: None,
            _task: task,
        }
    }
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
    /// What the shell does with a sidebar action chosen from a ref's menu
    /// (a branch submenu of the row menu, or a badge's menu); `None` until the
    /// shell installs it, and then those items do nothing.
    sidebar_action: Option<SidebarActionHandler>,
    /// The repository's remotes, for the "Open on <host>" menu items; set by
    /// the shell whenever it reloads them.
    remote_targets: Vec<RemoteTargets>,
    /// The operation the repository is stopped in, for the badge and row
    /// menus (Merge is disabled while there is one). The shell keeps it current.
    operation: Operation,
    /// The pull request badges and CI dots, pushed by the shell.
    hosting: Arc<HostingView>,
    /// What a click on a pull request badge does; installed by the shell.
    pr_click: Option<PrClickHandler>,
    /// Names the service "Create pull request…" opens for a branch.
    pr_service: Option<PrServiceHook>,
    /// The commits of the rows the virtualized list last rendered, plus a
    /// margin; the shell asks for their checks.
    visible_oids: Vec<Oid>,
    /// The set last reported to the shell, so a repeated range costs nothing.
    visible_reported: Vec<Oid>,
    /// Waits for the visible range to rest before reporting it.
    visible_debounce: Option<Task<()>>,
    /// Bumped whenever HEAD and the refs were read again, so the shell knows
    /// to look at its branch tracking once more.
    meta_version: u64,
    /// The repository root, once `Repo::open` succeeded.
    root: Option<PathBuf>,
    items: Vec<Item>,
    /// A reload is in flight and the rows still belong to the previous walk.
    /// The first page of the new walk replaces them.
    pending_rows: bool,
    /// The commit the reload should select again, once its new row arrives.
    /// Only set when the rows were already shown and the user had a selection.
    pending_selection: Option<Oid>,
    max_lanes: usize,
    selected: Option<usize>,
    state: LoadState,
    /// Bumped for every load; results from an older load are dropped.
    generation: u64,
    /// Bumped for every HEAD/refs read; a stale answer never overwrites a
    /// newer one. Separate from `generation` because a refs-only refresh does
    /// not restart the history walk.
    meta_generation: u64,
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
    badge_widths: HashMap<(SharedString, RefKind, Option<SharedString>), Pixels>,
    /// The table layout for the pane's last measured width, so the virtualized
    /// rows paint with the same columns the header was built for.
    columns: Columns,
    /// The summary column's width for the current pane, used to fit badges.
    summary_width: f32,
    /// Notifies once a minute so relative dates stay fresh; dropped with the
    /// view.
    clock_task: Option<Task<()>>,
    /// The user's graph column width, kept live while its grip drags and
    /// persisted on drag end.
    graph_width: f32,
    /// The user's author, date and hash widths, handled like `graph_width`.
    author_width: f32,
    date_width: f32,
    hash_width: f32,
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
    /// The active day filter, if any.
    day_rows: Option<DayRows>,
    /// Bumped on every filter change; an older answer is dropped.
    day_generation: u64,
    /// The active branch filter, if any. Exclusive with the day filter.
    branch_rows: Option<BranchRows>,
    /// Bumped on every branch filter change; an older answer is dropped.
    branch_generation: u64,
}

impl EventEmitter<HistoryEvent> for HistoryView {}

/// Adds one row-menu item that asks the shell to open a ref dialog.
#[allow(clippy::too_many_arguments)]
fn push_row_action(
    entries: &mut Vec<MenuEntry>,
    card: &CommitCard,
    on_action: &RefActionHandler,
    label: SharedString,
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

/// A ref-menu model as UI-kit menu entries. Items dispatch their
/// [`SidebarAction`] through `handler`; a disabled item (no action) stays
/// shown but off.
fn ref_menu_entries(
    model: Vec<RefMenuEntry>,
    handler: Option<SidebarActionHandler>,
) -> Vec<MenuEntry> {
    model
        .into_iter()
        .map(|entry| match entry {
            RefMenuEntry::Separator => MenuEntry::Separator,
            RefMenuEntry::Header { label, tools } => {
                let mut header = menu::MenuHeader::new(label);
                for tool in tools {
                    header = header.tool(
                        menu::MenuTool::new(tool.icon, tool.tooltip)
                            .selected(tool.selected)
                            .action(dispatch(handler.clone(), tool.action)),
                    );
                }
                MenuEntry::Header(header)
            }
            RefMenuEntry::Item(item) => {
                let mut menu_item = MenuItem::new(item.label)
                    .icon(item.icon)
                    .disabled(!item.enabled);
                if let Some(shortcut) = item.shortcut {
                    menu_item = menu_item.shortcut(shortcut);
                }
                if let Some(hint) = item.hint {
                    menu_item = menu_item.hint(hint);
                }
                if item.danger {
                    menu_item = menu_item.danger();
                }
                MenuEntry::Item(menu_item.action(dispatch(handler.clone(), item.action)))
            }
        })
        .collect()
}

/// What a menu entry does: the shell's sidebar-action handler, or, in a
/// History without one, just opening a link.
fn dispatch(
    handler: Option<SidebarActionHandler>,
    action: SidebarAction,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |window, cx| match &handler {
        Some(handler) => handler(action.clone(), window, cx),
        None => {
            if let SidebarAction::OpenUrl { url, .. } = &action {
                cx.open_url(url);
            }
        }
    }
}

/// The ref row a badge stands for, or `None` for a badge that has no menu (a
/// detached HEAD, the "+N" overflow). A merged local/remote badge keeps its
/// remotes, so its menu shows one section per ref.
fn badge_row(spec: &BadgeSpec) -> Option<RefRow> {
    let kind = match spec.kind {
        RefKind::Head if spec.label.as_ref() == "HEAD" => return None,
        RefKind::Head | RefKind::Local => RefRowKind::Local {
            head: spec.kind == RefKind::Head,
            remotes: spec.remotes.iter().map(|r| r.to_string()).collect(),
            // Only the row's hint reads these; a badge menu has no hint.
            ahead: 0,
            behind: 0,
        },
        RefKind::Remote => RefRowKind::Remote,
        RefKind::Tag => RefRowKind::Tag,
        _ => return None,
    };
    Some(RefRow {
        kind,
        name: spec.label.to_string(),
    })
}

/// Everything the history row menu is built from. The menu is pure data plus
/// callbacks, so its shape is testable without a window: one cascading row per
/// ref of the commit (see [`ref_menus`]), then the commit's own actions.
#[derive(Clone)]
struct RowMenu {
    hash: String,
    card: CommitCard,
    /// The commit's subject and `Name <email>`, for "Copy ›".
    summary: String,
    author: String,
    /// The branch "Reset … to here…" names; `HEAD` when detached.
    reset_branch: String,
    /// The ref rows of the commit, in menu order.
    rows: Vec<RefRow>,
    ctx: Rc<RefMenuCtx>,
    /// The service "Create pull request…" names, per local branch with one.
    pr_kinds: Rc<HashMap<String, ServiceKind>>,
    on_action: RefActionHandler,
    on_sidebar: Option<SidebarActionHandler>,
    /// The hosts the commit can be opened on.
    open_links: Vec<HostLink>,
}

impl RowMenu {
    fn entries(&self) -> Vec<MenuEntry> {
        let (shown, more) = ref_menus::split_overflow(self.rows.clone());
        let mut entries: Vec<MenuEntry> = shown
            .iter()
            .map(|row| MenuEntry::Item(self.ref_item(row)))
            .collect();
        if !more.is_empty() {
            let this = self.clone();
            entries.push(MenuEntry::Item(
                MenuItem::new(ui::t!("history.refmenu.more_refs", n = more.len()))
                    .icon(IconName::Branch)
                    .submenu(move || {
                        let mut rest = vec![MenuEntry::Filter {
                            placeholder: ui::t!("history.refmenu.filter_refs"),
                        }];
                        rest.extend(more.iter().map(|row| MenuEntry::Item(this.ref_item(row))));
                        rest
                    }),
            ));
        }
        if !entries.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        let (card, on_action) = (&self.card, &self.on_action);
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.checkout_commit"),
            IconName::Check,
            None,
            RefAction::CheckoutCommit,
            false,
        );
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.create_branch_here"),
            IconName::Branch,
            Some("B"),
            RefAction::CreateBranch,
            false,
        );
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.create_tag_here"),
            IconName::Tag,
            None,
            RefAction::CreateTag,
            false,
        );
        entries.push(MenuEntry::Separator);
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.cherry_pick"),
            IconName::Undo,
            None,
            RefAction::CherryPick,
            false,
        );
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.revert_commit"),
            IconName::Undo,
            None,
            RefAction::Revert,
            false,
        );
        push_row_action(
            &mut entries,
            card,
            on_action,
            ui::t!("history.menu.reset_to_here", branch = self.reset_branch),
            IconName::Undo,
            None,
            RefAction::Reset,
            true,
        );
        entries.push(MenuEntry::Separator);
        let this = self.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new(ui::t!("history.refmenu.copy"))
                .icon(IconName::Copy)
                .submenu(move || this.copy_entries()),
        ));
        self.push_open_items(&mut entries);
        entries
    }

    /// The row of one ref: its name, hint and a submenu with the ref's
    /// sections (the same entries a badge's right-click shows).
    fn ref_item(&self, row: &RefRow) -> MenuItem {
        let mut item = MenuItem::new(row.name.clone()).icon(row.icon());
        if row.is_head() {
            item = item.accent_icon();
        }
        if let Some(hint) = ref_menus::row_hint(row) {
            item = item.hint(hint);
        }
        let (row, this) = (row.clone(), self.clone());
        item.submenu(move || {
            let mut ctx = (*this.ctx).clone();
            ctx.pr_kind = this.pr_kinds.get(&row.name).copied();
            ref_menu_entries(
                ref_menus::badge_sections(&ctx, &row),
                this.on_sidebar.clone(),
            )
        })
    }

    /// "Copy ›": the hash (also Ctrl+C on the selected row), the short hash,
    /// the subject, the full message, the author and a link per host.
    fn copy_entries(&self) -> Vec<MenuEntry> {
        let copy = |label: SharedString, text: String| {
            MenuItem::new(label).action(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            })
        };
        let (handler, oid) = (self.on_sidebar.clone(), self.card.oid);
        let mut entries = vec![
            MenuEntry::Item(
                copy(ui::t!("history.refmenu.copy_hash"), self.hash.clone()).shortcut("Ctrl+C"),
            ),
            MenuEntry::Item(copy(
                ui::t!("history.refmenu.copy_short"),
                self.card.short.clone(),
            )),
            MenuEntry::Item(copy(
                ui::t!("history.refmenu.copy_subject"),
                self.summary.clone(),
            )),
            // The body is not loaded with the row: the shell reads it in the
            // background and copies subject + body.
            MenuEntry::Item(
                MenuItem::new(ui::t!("history.refmenu.copy_message")).action(move |window, cx| {
                    if let Some(handler) = &handler {
                        handler(SidebarAction::CopyCommitMessage(oid), window, cx);
                    }
                }),
            ),
            MenuEntry::Item(copy(
                ui::t!("history.refmenu.copy_author"),
                self.author.clone(),
            )),
        ];
        let several = self.open_links.len() > 1;
        for link in &self.open_links {
            let Some(url) = link.url(&forge::WebTarget::Commit(self.hash.clone())) else {
                continue;
            };
            let label = ui::t!("history.refmenu.copy_link", service = link.service_name());
            let label: SharedString = if several {
                format!("{label} ({})", link.remote()).into()
            } else {
                label
            };
            entries.push(MenuEntry::Item(copy(label, url)));
        }
        entries
    }

    /// "Open on <Service>" per host the commit can be opened on; the remote's
    /// name tells them apart when there are several.
    fn push_open_items(&self, entries: &mut Vec<MenuEntry>) {
        let several = self.open_links.len() > 1;
        for link in &self.open_links {
            let Some(url) = link.url(&forge::WebTarget::Commit(self.hash.clone())) else {
                continue;
            };
            let label = if several {
                ui::t!(
                    "history.menu.open_on_host_remote",
                    service = link.service_name(),
                    remote = link.remote()
                )
            } else {
                ui::t!("history.menu.open_on_host", service = link.service_name())
            };
            let action = SidebarAction::OpenUrl {
                url,
                host: link.host().to_owned(),
            };
            entries.push(MenuEntry::Item(
                MenuItem::new(label)
                    .icon(IconName::External)
                    .action(dispatch(self.on_sidebar.clone(), action)),
            ));
        }
    }
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
        let (graph_width, author_width, date_width, hash_width, collapse) = cx
            .try_global::<crate::AppSettings>()
            .map(|state| {
                (
                    state.settings.graph_width,
                    state.settings.author_width,
                    state.settings.date_width,
                    state.settings.hash_width,
                    state.settings.collapse_idle_lanes,
                )
            })
            .unwrap_or((
                GRAPH_WIDTH_DEFAULT,
                AUTHOR_WIDTH_DEFAULT,
                DATE_WIDTH_DEFAULT,
                HASH_WIDTH_DEFAULT,
                false,
            ));
        let graph_width = clamp_graph_width(graph_width);
        let author_width = clamp_width(author_width, AUTHOR_WIDTH_MIN, AUTHOR_WIDTH_DEFAULT);
        let date_width = clamp_width(date_width, DATE_WIDTH_MIN, DATE_WIDTH_DEFAULT);
        let hash_width = clamp_width(hash_width, HASH_WIDTH_MIN, HASH_WIDTH_DEFAULT);
        // Replaced with the pane's real width in the first prepaint.
        let columns = columns(1200., graph_width, author_width, date_width, hash_width);
        let mut view = Self {
            path,
            git,
            workspace,
            on_action,
            sidebar_action: None,
            remote_targets: Vec::new(),
            operation: Operation::None,
            hosting: Arc::default(),
            pr_click: None,
            pr_service: None,
            visible_oids: Vec::new(),
            visible_reported: Vec::new(),
            visible_debounce: None,
            meta_version: 0,
            root: None,
            items: Vec::new(),
            pending_rows: false,
            pending_selection: None,
            max_lanes: 1,
            selected: None,
            state: LoadState::Loading { total: None },
            generation: 0,
            meta_generation: 0,
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
            author_width,
            date_width,
            hash_width,
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
            day_rows: None,
            branch_rows: None,
            branch_generation: 0,
            day_generation: 0,
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
        // A fresh walk reads HEAD and the refs again: an older refs-only
        // refresh must not overwrite that answer.
        self.meta_generation = self.meta_generation.wrapping_add(1);
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
        let meta_generation = this
            .update(cx, |view, _cx| view.next_meta_generation())
            .unwrap_or(0);
        let meta_repo = repo.clone();
        let meta_root = root.clone();
        let meta_this = this.clone();
        cx.spawn(async move |cx| {
            let (head, refs) = Self::read_meta(meta_repo, cx).await;
            meta_this
                .update(cx, |view, cx| {
                    view.set_meta(meta_generation, meta_root, head, refs, cx)
                })
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
            // The old rows are gone; the selected commit is selected again by
            // oid once its new row arrives.
            if self.pending_selection.is_some() {
                self.selected = None;
            }
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
        // A reload keeps the selection on the commit it was on, by oid: the
        // row number may have moved. Pages land in order, so a commit below
        // the first page is restored by a later append.
        if let Some(oid) = self.pending_selection {
            if let Some(index) = self.items.iter().position(|item| item.commit.id == oid) {
                self.selected = Some(index);
                self.pending_selection = None;
            }
        }
        // The HEAD row may only arrive now; the initial selection follows it
        // until the user picks a row.
        self.settle_initial_selection(cx);
        // A page that arrives while a query is active is searched as it lands,
        // never by rescanning the rows already covered.
        self.scan_more(cx);
        // A sidebar reveal that was waiting for this page lands now, and only
        // the rows that just arrived are scanned for it.
        if let Some(index) = self
            .pending_reveal
            .check(&self.items, start)
            .filter(|_| self.day_rows.is_none() && self.branch_rows.is_none())
        {
            self.reveal_row(index, cx);
        }
        cx.notify();
    }

    /// Rebuild the painter's x snapshot. Called when rows were added or the
    /// collapse toggle changed, not while rendering.
    fn refresh_geometry(&mut self) {
        self.geometry = Rc::new(self.idle_lanes.geometry(self.collapse));
        if let Some(day) = &mut self.day_rows {
            day.geometry = Rc::new(day.idle_lanes.geometry(self.collapse));
        }
        if let Some(branch) = &mut self.branch_rows {
            branch.geometry = Rc::new(branch.idle_lanes.geometry(self.collapse));
        }
    }

    /// The rows on screen: the day or branch filter's while one is on, else
    /// the history.
    fn rows(&self) -> &[Item] {
        match (&self.day_rows, &self.branch_rows) {
            (Some(day), _) => &day.items,
            (None, Some(branch)) => &branch.items,
            (None, None) => &self.items,
        }
    }

    /// The graph geometry that belongs to [`Self::rows`].
    fn geometry(&self) -> &Rc<ColumnGeometry> {
        match (&self.day_rows, &self.branch_rows) {
            (Some(day), _) => &day.geometry,
            (None, Some(branch)) => &branch.geometry,
            (None, None) => &self.geometry,
        }
    }

    /// The active day filter.
    pub fn day_filter(&self) -> Option<&DayFilter> {
        self.day_rows.as_ref().map(|day| &day.filter)
    }

    /// How many commits the active filter found, once its query finished.
    pub fn day_filter_count(&self) -> Option<usize> {
        let day = self.day_rows.as_ref()?;
        matches!(day.state, DayState::Ready).then_some(day.items.len())
    }

    /// Turns the day filter on, changes it or (with `None`) clears it.
    ///
    /// The commits come from a one-off `git log` for that day instead of the
    /// loaded rows: the day may be older than what has streamed in, and the
    /// strip counts the same way (author date, local day, mailmapped email),
    /// so the list always matches the number the strip showed. The list code
    /// is untouched: it reads [`Self::rows`], which is either this day's rows
    /// or the history.
    pub fn set_day_filter(&mut self, filter: Option<DayFilter>, cx: &mut Context<Self>) {
        self.day_generation = self.day_generation.wrapping_add(1);
        let generation = self.day_generation;
        let Some(filter) = filter else {
            if self.day_rows.take().is_some() {
                self.reset_rows_view(cx);
            }
            return;
        };
        if self.day_rows.as_ref().map(|day| &day.filter) == Some(&filter) {
            return;
        }
        // The day and branch filters are exclusive: the new one replaces the
        // other.
        self.branch_generation = self.branch_generation.wrapping_add(1);
        self.branch_rows = None;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.day_rows = Some(DayRows {
                    filter,
                    state: DayState::Failed(error.to_string()),
                    items: Vec::new(),
                    idle_lanes: IdleLanes::new(),
                    geometry: Rc::new(IdleLanes::new().geometry(self.collapse)),
                    _task: None,
                });
                self.reset_rows_view(cx);
                return;
            }
        };
        let path = self.path.clone();
        let (day, me) = (filter.day, filter.me.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|e| e.to_string())?;
                    let commits = git_core::commits_on_day(&repo, day, me.as_deref())
                        .map_err(|e| e.to_string())?;
                    Ok(layout_day(commits))
                })
                .await;
            this.update(cx, |view, cx| view.finish_day_rows(generation, result, cx))
                .ok();
        });
        self.day_rows = Some(DayRows {
            filter,
            state: DayState::Loading,
            items: Vec::new(),
            idle_lanes: IdleLanes::new(),
            geometry: Rc::new(IdleLanes::new().geometry(self.collapse)),
            _task: Some(task),
        });
        self.reset_rows_view(cx);
    }

    /// The day query answered.
    fn finish_day_rows(
        &mut self,
        generation: u64,
        result: Result<Vec<(CommitInfo, Row<Oid>)>, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.day_generation {
            return;
        }
        let collapse = self.collapse;
        let Some(day) = &mut self.day_rows else {
            return;
        };
        match result {
            Ok(page) => {
                for (commit, row) in page {
                    day.idle_lanes.push(&row);
                    day.items.push(Item {
                        commit: Arc::new(commit),
                        row: Rc::new(row),
                    });
                }
                day.geometry = Rc::new(day.idle_lanes.geometry(collapse));
                day.state = DayState::Ready;
            }
            Err(message) => {
                log::warn!("could not list the commits of the day: {message}");
                day.state = DayState::Failed(message);
            }
        }
        self.reset_rows_view(cx);
    }

    /// The branch the History is narrowed to, if any.
    pub fn branch_filter(&self) -> Option<&str> {
        self.branch_rows.as_ref().map(|rows| rows.branch.as_str())
    }

    /// Turns the branch filter on ("Show all commits of {branch}"), changes it
    /// or (with `None`) clears it.
    ///
    /// The rows are the branch's own commits (`git log --first-parent`) down
    /// to its fork point, from a one-off query like the day filter's, laid out
    /// with the parents that are in the list only. Turning it on replaces the
    /// day filter, clears the search, and puts the selection on the first row
    /// at the top of the list.
    pub fn set_branch_filter(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        self.branch_generation = self.branch_generation.wrapping_add(1);
        let generation = self.branch_generation;
        let Some(branch) = branch else {
            if self.branch_rows.take().is_some() {
                self.reset_rows_view(cx);
            }
            return;
        };
        // The filters are exclusive, and the search belongs to the rows it ran
        // over (the shell empties the field when it sees the query is gone).
        self.day_generation = self.day_generation.wrapping_add(1);
        self.day_rows = None;
        self.clear_search(cx);
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.branch_rows = Some(BranchRows::new(
                    branch,
                    DayState::Failed(error.to_string()),
                    self.collapse,
                    None,
                ));
                self.reset_rows_view(cx);
                self.select_first_row(cx);
                return;
            }
        };
        let path = self.path.clone();
        let name = branch.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|e| e.to_string())?;
                    let main = repo.main_line().map_err(|e| e.to_string())?;
                    let log = match repo.branch_log(&name, main.as_deref(), BRANCH_FILTER_LIMIT) {
                        Ok(log) => log,
                        // A branch that is gone (or not fetched yet) is the
                        // empty state, not an error.
                        Err(Error::Parse("branch not found")) => BranchLog {
                            commits: Vec::new(),
                            fork_point: None,
                            merged_into: None,
                            merged_from: HashMap::new(),
                        },
                        Err(error) => return Err(error.to_string()),
                    };
                    let rows = layout_branch(&log);
                    Ok(BranchData { log, main, rows })
                })
                .await;
            this.update(cx, |view, cx| {
                view.finish_branch_rows(generation, result, cx)
            })
            .ok();
        });
        self.branch_rows = Some(BranchRows::new(
            branch,
            DayState::Loading,
            self.collapse,
            Some(task),
        ));
        self.reset_rows_view(cx);
        self.select_first_row(cx);
    }

    /// The branch query answered.
    fn finish_branch_rows(
        &mut self,
        generation: u64,
        result: Result<BranchData, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.branch_generation {
            return;
        }
        let collapse = self.collapse;
        let Some(rows) = &mut self.branch_rows else {
            return;
        };
        match result {
            Ok(data) => {
                for (commit, row) in data.rows {
                    rows.idle_lanes.push(&row);
                    rows.items.push(Item {
                        commit: Arc::new(commit),
                        row: Rc::new(row),
                    });
                }
                rows.geometry = Rc::new(rows.idle_lanes.geometry(collapse));
                rows.fork = data.log.fork_point;
                rows.merged_from = data.log.merged_from;
                rows.merged_into = data.log.merged_into.zip(data.main);
                rows.state = DayState::Ready;
            }
            Err(message) => {
                log::warn!("could not list the commits of {}: {message}", rows.branch);
                rows.state = DayState::Failed(message);
            }
        }
        self.reset_rows_view(cx);
        self.select_first_row(cx);
    }

    /// Selects the first row (once there is one) as a user pick, so the
    /// selection does not follow HEAD inside a filtered list.
    fn select_first_row(&mut self, cx: &mut Context<Self>) {
        if !self.rows().is_empty() {
            self.select_row(0, cx);
        }
    }

    /// The rows on screen changed wholesale: back to the top, selection and
    /// search over the new rows.
    fn reset_rows_view(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.user_selected = false;
        self.hovered = None;
        self.scroll.0.borrow().base_handle.set_offset(Point {
            x: px(0.),
            y: px(0.),
        });
        self.restart_search(cx);
        self.settle_initial_selection(cx);
        cx.notify();
    }

    /// The row of the HEAD commit, once it is among the loaded rows.
    fn head_index(&self) -> Option<usize> {
        let head = self.head_oid()?;
        self.rows().iter().position(|item| item.commit.id == head)
    }

    /// The row a key press lands on when nothing is selected: the HEAD row
    /// when it is loaded, else row 0.
    fn initial_row(&self) -> Option<usize> {
        if self.rows().is_empty() {
            None
        } else {
            Some(self.head_index().unwrap_or(0))
        }
    }

    /// While the user has not picked a row, the selection follows the HEAD row
    /// as it arrives, else row 0. Selecting is not focusing: this never moves
    /// the keyboard focus or scrolls the list.
    fn settle_initial_selection(&mut self, cx: &mut Context<Self>) {
        if self.user_selected || self.rows().is_empty() {
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
        // A pick, a search jump or a reveal is newer than the reload's kept
        // selection, so the reload must not put the old row back.
        self.pending_selection = None;
        if index >= self.rows().len() || self.selected == Some(index) {
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
        // The sidebar target may not be on the filtered day: show the whole
        // history again, then reveal it there.
        if (self.day_rows.is_some() || self.branch_rows.is_some())
            && !self.rows().iter().any(|item| item.commit.id == oid)
        {
            self.set_day_filter(None, cx);
            self.set_branch_filter(None, cx);
        }
        if let Some(index) = self.rows().iter().position(|item| item.commit.id == oid) {
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
            let last = self.rows().len().saturating_sub(1) as isize;
            (current as isize + delta).clamp(0, last) as usize
        };
        self.select_row(next, cx);
    }

    /// Home / End: the first or the last loaded row (`usize::MAX` for End).
    fn jump_selection(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(last) = self.rows().len().checked_sub(1) else {
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
        let last = self.rows().len().saturating_sub(1) as isize;
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
        if let Some(item) = self.rows().get(index) {
            cx.write_to_clipboard(ClipboardItem::new_string(item.commit.id.to_string()));
        }
    }

    /// `B` on the focused list: "Create branch here…" on the selected commit,
    /// the same request the row menu makes.
    fn create_branch_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected.or_else(|| self.initial_row()) else {
            return;
        };
        self.select_row(index, cx);
        let Some(item) = self.rows().get(index) else {
            return;
        };
        let card = CommitCard::from_commit(&item.commit, self.refs().unwrap_or(&[]), self.head());
        // The shell's handler reads this view back for the dialog's context, so
        // it must run after the current update has finished.
        let on_action = self.on_action.clone();
        window.defer(cx, move |window, cx| {
            on_action(RefAction::CreateBranch(card), window, cx);
        });
    }

    /// Double-click: switch to the row's first local branch that is not HEAD,
    /// check out the row's first remote branch as a local tracking branch, or
    /// check the commit out detached when the row has no branch at all (only
    /// tags or a bare commit).
    fn checkout_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.rows().get(index) else {
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
            None => {
                // No local branch: a remote branch is downloaded locally
                // through the sidebar's remote-checkout path, not detached.
                let remote = self
                    .refs()
                    .unwrap_or(&[])
                    .iter()
                    .find(|reference| {
                        reference.kind == git_core::RefKind::RemoteBranch
                            && reference.target == card.oid
                            && !reference.name.ends_with("/HEAD")
                    })
                    .map(|reference| reference.name.clone());
                if let (Some(remote), Some(handler)) = (remote, self.sidebar_action.clone()) {
                    window.defer(cx, move |window, cx| {
                        handler(SidebarAction::CheckoutRemoteBranch(remote), window, cx);
                    });
                    return;
                }
                RefAction::CheckoutCommit(card)
            }
        };
        (self.on_action)(action, window, cx);
    }

    /// Installs what the shell does with a sidebar action chosen from a ref's
    /// menu, so the row menu's "Branch" view and the badge menus work.
    pub(crate) fn set_sidebar_action_handler(&mut self, handler: SidebarActionHandler) {
        self.sidebar_action = Some(handler);
    }

    /// Installs what a click on a pull request badge does.
    pub(crate) fn set_pr_click_handler(&mut self, handler: PrClickHandler) {
        self.pr_click = Some(handler);
    }

    /// Starts the short wait that lets a fast scroll pass before its rows are
    /// reported. A wait already running picks up the latest range.
    fn schedule_visible_rows(&mut self, cx: &mut Context<Self>) {
        if self.visible_debounce.is_some() {
            return;
        }
        self.visible_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CI_VISIBLE_DEBOUNCE).await;
            this.update(cx, |view, cx| view.report_visible_rows(cx))
                .ok();
        }));
    }

    /// Tells the shell the current visible set unless it is the one it has.
    /// An event, not a callback: the shell reads this view from its handler,
    /// which is only safe once this update has finished.
    fn report_visible_rows(&mut self, cx: &mut Context<Self>) {
        self.visible_debounce = None;
        if self.visible_oids == self.visible_reported {
            return;
        }
        self.visible_reported = self.visible_oids.clone();
        cx.emit(HistoryEvent::VisibleRowsChanged(
            self.visible_reported.clone(),
        ));
    }

    /// The commits of the rows in `range`, plus [`CI_VISIBLE_MARGIN`] rows on
    /// each side, in row order.
    fn visible_commits(&self, items: &[Item], range: std::ops::Range<usize>) -> Vec<Oid> {
        let start = range.start.saturating_sub(CI_VISIBLE_MARGIN);
        let end = range.end.saturating_add(CI_VISIBLE_MARGIN).min(items.len());
        items[start.min(end)..end]
            .iter()
            .map(|item| item.commit.id)
            .collect()
    }

    /// Replaces the pull request badges and CI dots the rows draw.
    pub(crate) fn set_hosting(&mut self, hosting: Arc<HostingView>, cx: &mut Context<Self>) {
        self.hosting = hosting;
        cx.notify();
    }

    /// How many commits are loaded.
    pub(crate) fn loaded_len(&self) -> usize {
        self.items.len()
    }

    /// Every loaded commit reachable from `tips` (the tips themselves
    /// included), following parents through the loaded commits only.
    pub(crate) fn reachable_from(&self, tips: &[Oid]) -> HashSet<Oid> {
        let index: HashMap<Oid, &CommitInfo> = self
            .items
            .iter()
            .map(|item| (item.commit.id, item.commit.as_ref()))
            .collect();
        crate::hosting_status::reachable(tips, |oid| {
            index.get(&oid).map(|commit| commit.parents.clone())
        })
    }

    /// How many times HEAD and the refs have been (re)read.
    pub(crate) fn meta_version(&self) -> u64 {
        self.meta_version
    }

    /// Tells the History which remotes the repository has, so its menus can
    /// offer "Open on <host>".
    pub(crate) fn set_remote_targets(&mut self, targets: Vec<RemoteTargets>) {
        self.remote_targets = targets;
    }

    /// Tells the History which operation the repository is stopped in.
    pub(crate) fn set_operation(&mut self, operation: Operation) {
        self.operation = operation;
    }

    /// The hosts the repository's remotes point at, the current branch's
    /// upstream first. Pure: reads the configured accounts, nothing else.
    fn host_links(&self, cx: &App) -> Vec<HostLink> {
        if self.remote_targets.is_empty() {
            return Vec::new();
        }
        let accounts = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.accounts.clone())
            .unwrap_or_default();
        let preferred = web_links::upstream_remote(
            self.head(),
            self.refs().unwrap_or(&[]),
            &self.remote_targets,
        );
        web_links::host_links(
            &crate::remote_accounts::account_refs(&accounts),
            &self.remote_targets,
            preferred.as_deref(),
        )
    }

    /// Right-clicking a row selects it and opens the row menu at the pointer.
    fn open_row_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.rows().get(index) else {
            return;
        };
        let commit = item.commit.id;
        let refs = self.refs().unwrap_or(&[]);
        let card = CommitCard::from_commit(&item.commit, refs, self.head());
        let ctx = self.ref_menu_ctx(cx);
        let rows = ref_menus::commit_ref_rows(commit, self.head(), refs, &self.remote_targets);
        let pr_kinds = rows
            .iter()
            .filter(|row| matches!(row.kind, RefRowKind::Local { .. }))
            .filter_map(|row| Some((row.name.clone(), self.pr_kind(&row.name, cx)?)))
            .collect();
        let menu = RowMenu {
            hash: commit.to_string(),
            open_links: ctx.links.clone(),
            summary: item.commit.summary.clone(),
            author: format!("{} <{}>", item.commit.author_name, item.commit.author_email),
            card,
            reset_branch: match self.head() {
                Some(Head::Branch { name, .. }) => name.clone(),
                _ => "HEAD".to_owned(),
            },
            rows,
            ctx: Rc::new(ctx),
            pr_kinds: Rc::new(pr_kinds),
            on_action: self.on_action.clone(),
            on_sidebar: self.sidebar_action.clone(),
        };
        menu::open_context_menu(position, menu.entries(), window, cx);
    }

    /// Installs how the shell finds the service "Create pull request…" names
    /// for a branch.
    pub(crate) fn set_pr_service(&mut self, hook: PrServiceHook) {
        self.pr_service = Some(hook);
    }

    /// The service "Create pull request…" names for the local branch
    /// `branch`; `None` without a remote that can take one.
    fn pr_kind(&self, branch: &str, cx: &App) -> Option<ServiceKind> {
        let hook = self.pr_service.as_ref()?;
        hook(branch, self.head(), self.refs().unwrap_or(&[]), cx)
    }

    /// What the ref menus are built from: a snapshot of HEAD and the refs, the
    /// hosts, the stopped operation and the active branch filter. `pr_kind` is
    /// per branch and set by the caller.
    fn ref_menu_ctx(&self, cx: &App) -> RefMenuCtx {
        // The remote a branch without upstream is pushed to: `origin`, else
        // the first one (what `git_core::network::default_remote` answers).
        let default_remote = self
            .remote_targets
            .iter()
            .find(|target| target.name == "origin")
            .or_else(|| self.remote_targets.first())
            .map(|target| target.name.clone());
        RefMenuCtx {
            head: self.head().cloned(),
            refs: self.refs().unwrap_or(&[]).to_vec(),
            links: self.host_links(cx),
            targets: self.remote_targets.clone(),
            operation: self.operation,
            filter: self.branch_filter().map(str::to_owned),
            pr_kind: None,
            default_remote,
        }
    }

    /// Right-clicking a branch, tag or remote badge opens that ref's own
    /// menu in place of the row menu, with no ref-row level. A merged
    /// local/remote badge lists both refs' sections.
    fn open_badge_menu(
        &mut self,
        spec: &BadgeSpec,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = badge_row(spec) else {
            return;
        };
        let mut ctx = self.ref_menu_ctx(cx);
        ctx.pr_kind = self.pr_kind(&row.name, cx);
        let entries = ref_menu_entries(
            ref_menus::badge_sections(&ctx, &row),
            self.sidebar_action.clone(),
        );
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

    /// Whether idle lanes fold into gap lanes.
    pub(crate) fn idle_lanes_collapsed(&self) -> bool {
        self.collapse
    }

    /// Every loaded commit, newest first, for the palette's Commits group.
    /// The whole history, whatever the day filter shows.
    pub(crate) fn loaded_commits(&self) -> Vec<Arc<CommitInfo>> {
        self.items.iter().map(|item| item.commit.clone()).collect()
    }

    /// Sets the idle-lane collapse, recomputes the geometry and persists it.
    pub(crate) fn set_collapse(&mut self, collapse: bool, cx: &mut Context<Self>) {
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
    pub(crate) fn fit_graph(&mut self, cx: &mut Context<Self>) {
        let width = fit_width(content_width(self.geometry()));
        self.graph_width = width;
        crate::update_settings(cx, move |settings| settings.graph_width = width);
        cx.notify();
    }

    /// The current width of a resizable column.
    fn column_width(&self, column: ResizeColumn) -> f32 {
        match column {
            ResizeColumn::Graph => self.graph_width,
            ResizeColumn::Author => self.author_width,
            ResizeColumn::Date => self.date_width,
            ResizeColumn::Hash => self.hash_width,
        }
    }

    /// Sets a resizable column's live width without persisting it; the grip
    /// drag persists on release.
    fn set_column_width(&mut self, column: ResizeColumn, width: f32) {
        match column {
            ResizeColumn::Graph => self.graph_width = width,
            ResizeColumn::Author => self.author_width = width,
            ResizeColumn::Date => self.date_width = width,
            ResizeColumn::Hash => self.hash_width = width,
        }
    }

    /// Persists one column's width in the settings.
    fn persist_column_width(&mut self, column: ResizeColumn, width: f32, cx: &mut Context<Self>) {
        crate::update_settings(cx, move |settings| match column {
            ResizeColumn::Graph => settings.graph_width = width,
            ResizeColumn::Author => settings.author_width = width,
            ResizeColumn::Date => settings.date_width = width,
            ResizeColumn::Hash => settings.hash_width = width,
        });
    }

    /// Restores a column's default width on a grip double-click. The graph
    /// keeps its own content-fitting behaviour.
    fn reset_column_width(&mut self, column: ResizeColumn, cx: &mut Context<Self>) {
        if column == ResizeColumn::Graph {
            self.fit_graph(cx);
            return;
        }
        let width = column.default_width();
        self.set_column_width(column, width);
        self.persist_column_width(column, width, cx);
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
        let width = self.column_width(drag.column);
        self.persist_column_width(drag.column, width, cx);
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
                MenuItem::new(ui::t!("history.graph_menu.collapse_idle"))
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
                MenuItem::new(ui::t!("history.graph_menu.fit"))
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

    /// Reads HEAD and the refs off the UI thread: the first load and the
    /// refs-only refresh must read them the same way.
    async fn read_meta(
        repo: Repo,
        cx: &mut AsyncApp,
    ) -> (Result<Head, Error>, Result<Vec<Ref>, Error>) {
        cx.background_executor()
            .spawn(async move { (repo.head(), repo.refs()) })
            .await
    }

    /// Bumps and returns the HEAD/refs read generation.
    fn next_meta_generation(&mut self) -> u64 {
        self.meta_generation = self.meta_generation.wrapping_add(1);
        self.meta_generation
    }

    fn set_meta(
        &mut self,
        generation: u64,
        root: PathBuf,
        head: Result<Head, Error>,
        refs: Result<Vec<Ref>, Error>,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_meta(generation, root, head, refs) {
            return;
        }
        // HEAD may have arrived after the first rows: the initial selection
        // can now move to its row.
        self.settle_initial_selection(cx);
        cx.notify();
    }

    /// Stores a HEAD/refs answer and rebuilds the badges. Returns whether the
    /// answer was still current.
    fn apply_meta(
        &mut self,
        generation: u64,
        root: PathBuf,
        head: Result<Head, Error>,
        refs: Result<Vec<Ref>, Error>,
    ) -> bool {
        if generation != self.meta_generation {
            return false;
        }
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
        self.meta_version = self.meta_version.wrapping_add(1);
        true
    }

    /// Re-reads HEAD and the refs without touching the rows, the selection,
    /// the scroll offset, the hover or the search. A checkout moves HEAD and a
    /// branch ref but cannot change the set of commits reachable from `--all`,
    /// so the walking load is left alone; only the badges and the HEAD ring
    /// change.
    pub(crate) fn refresh_refs(&mut self, cx: &mut Context<Self>) {
        let Ok(git) = self.git.clone() else {
            return;
        };
        // The old refs stay on screen while the read runs: no Loading flash.
        let generation = self.next_meta_generation();
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path)?;
                    let root = repo.root().to_path_buf();
                    Ok::<_, Error>((root, repo))
                })
                .await;
            let (root, repo) = match result {
                Ok(opened) => opened,
                Err(error) => {
                    log::warn!("could not read HEAD and the refs: {error}");
                    return;
                }
            };
            let (head, refs) = Self::read_meta(repo, cx).await;
            this.update(cx, |view, cx| {
                view.refresh_meta(generation, root, head, refs, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Applies a [`Self::refresh_refs`] answer: the badges, HEAD and meta
    /// version, and nothing the list draws.
    fn refresh_meta(
        &mut self,
        generation: u64,
        root: PathBuf,
        head: Result<Head, Error>,
        refs: Result<Vec<Ref>, Error>,
        cx: &mut Context<Self>,
    ) {
        if self.apply_meta(generation, root, head, refs) {
            cx.notify();
        }
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

    /// The query the search field last reported.
    pub fn search_query(&self) -> &str {
        &self.search_text
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
            self.is_loading() || self.search_scanning || self.search_scanned < self.rows().len();
        if self.search_matches.is_empty() {
            return Some(if pending {
                "– / 0+".into()
            } else {
                ui::t!("history.search.no_matches")
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

    /// The selected commit, for the shell's checkout test.
    #[cfg(test)]
    pub(crate) fn selected_oid(&self) -> Option<Oid> {
        self.selected
            .and_then(|index| self.rows().get(index))
            .map(|item| item.commit.id)
    }

    /// The load generation, for the shell's checkout test: an unchanged value
    /// proves the log was not walked again.
    #[cfg(test)]
    pub(crate) fn load_generation(&self) -> u64 {
        self.generation
    }

    /// The HEAD badge's label on `oid`, for the shell's checkout test.
    #[cfg(test)]
    pub(crate) fn head_badge(&self, oid: Oid) -> Option<&str> {
        self.badges
            .get(&oid)?
            .iter()
            .find(|badge| badge.kind == RefKind::Head)
            .map(|badge| badge.label.as_ref())
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
        if self.search_scanning || self.search_scanned >= self.rows().len() {
            return;
        }
        let start = self.search_scanned;
        let end = self.rows().len();
        let rows: Vec<Arc<CommitInfo>> = self.rows()[start..end]
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
        // A selection kept across the reload whose commit is gone falls back
        // to the top and the HEAD (or first) row, as a fresh load would.
        if self.pending_selection.take().is_some() {
            self.selected = None;
            self.user_selected = false;
            self.scroll.0.borrow().base_handle.set_offset(Point {
                x: px(0.),
                y: px(0.),
            });
            self.settle_initial_selection(cx);
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
        // The rows of the abandoned walk may not contain the kept commit.
        if self.pending_selection.take().is_some() {
            self.user_selected = false;
            self.settle_initial_selection(cx);
        }
        cx.notify();
    }

    /// Reloads the history from scratch, e.g. after a commit moved HEAD.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.retry(cx);
    }

    /// Start over: load again, keeping the rows until the first new page lands.
    fn retry(&mut self, cx: &mut Context<Self>) {
        // Rows that are already shown keep their selection and scroll offset
        // when the selected commit survives the walk (the commit count is
        // usually unchanged on a reload). The very first load and an empty
        // list start at the top again.
        let restoring = !self.items.is_empty()
            && self.day_rows.is_none()
            && self.branch_rows.is_none()
            && self.selected.is_some();
        self.pending_selection = if restoring {
            self.selected
                .and_then(|index| self.items.get(index))
                .map(|item| item.commit.id)
        } else {
            None
        };
        self.pending_rows = !self.items.is_empty();
        if self.pending_selection.is_none() {
            self.selected = None;
            self.user_selected = false;
            // The new load starts at the top, so the HEAD selection that
            // settles later is visible instead of being left below the old
            // scroll offset.
            self.scroll.0.borrow().base_handle.set_offset(Point {
                x: px(0.),
                y: px(0.),
            });
        }
        self.hovered = None;
        // The query stays in the field; its matches belong to the old rows.
        self.restart_search(cx);
        self.start(cx);
        // The day's commits may have changed too (a new commit today).
        if let Some(filter) = self.day_rows.take().map(|day| day.filter) {
            self.set_day_filter(Some(filter), cx);
        }
        // So may the branch's.
        if let Some(branch) = self.branch_rows.take().map(|rows| rows.branch) {
            self.set_branch_filter(Some(branch), cx);
        }
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
        self.selected
            .and_then(|ix| self.rows().get(ix))
            .map(|item| {
                let mut hash = item.commit.id.to_string();
                hash.truncate(7);
                hash.into()
            })
    }

    /// The selected commit, for the details dock.
    pub fn selected_commit(&self) -> Option<Arc<CommitInfo>> {
        self.selected
            .and_then(|ix| self.rows().get(ix))
            .map(|item| item.commit.clone())
    }

    /// The graph header cell: the label, the "+N" clipped-lanes pill, the
    /// resize grip and the right-click menu.
    fn render_graph_header(
        &self,
        columns: Columns,
        pane_width: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let accent = colors.accent;
        let clipped = clipped_lanes(self.geometry(), columns.graph);

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
            .child(ui::t!("history.header.graph"));
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
                    .tooltip(Tooltip::text(ui::i18n::t_count(
                        "history.header.lanes_clipped",
                        clipped as u64,
                        &[],
                    )))
                    .on_click(cx.listener(|view, _, _, cx| view.fit_graph(cx)))
                    .child(format!("+{clipped}")),
            );
        }

        cell.child(self.render_grip(ResizeColumn::Graph, pane_width, cx))
    }

    /// One column boundary's resize grip. The graph's grip sits on its right
    /// edge; every other column's grip sits on its left edge and resizes that
    /// column.
    fn render_grip(
        &self,
        column: ResizeColumn,
        pane_width: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let accent = colors.accent;
        let dragging = self
            .grip_drag
            .as_ref()
            .is_some_and(|drag| drag.dragging && drag.column == column);
        let left_edge = column != ResizeColumn::Graph;

        div()
            .id(column.id())
            .absolute()
            .when(left_edge, |this| this.left(px(-8.)))
            .when(!left_edge, |this| this.right(px(-8.)))
            .top(px(3.))
            .bottom(px(3.))
            .w(px(7.))
            .rounded(px(2.))
            .cursor_ew_resize()
            .hover(move |style| style.bg(accent.opacity(0.7)))
            .when(dragging, |this| this.bg(accent.opacity(0.7)))
            // A resting 1px divider, so the grip is visible before hover.
            .when(!dragging, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(3.))
                        .w(px(1.))
                        .bg(colors.border_strong),
                )
            })
            .tooltip(Tooltip::text(ui::t!("history.header.grip_tooltip")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        view.reset_column_width(column, cx);
                    } else {
                        view.grip_drag = Some(GripDrag {
                            column,
                            start_x: f32::from(event.position.x),
                            start_width: view.column_width(column),
                            pane_width,
                            dragging: false,
                        });
                    }
                }),
            )
            .on_drag(GripDragPayload, |_, _, _, cx| cx.new(|_| EmptyView))
            .on_drag_move(
                cx.listener(|view, event: &DragMoveEvent<GripDragPayload>, _, cx| {
                    let Some(drag) = &view.grip_drag else {
                        return;
                    };
                    let (column, start_x, start_width, pane_width) =
                        (drag.column, drag.start_x, drag.start_width, drag.pane_width);
                    let dx = f32::from(event.event.position.x) - start_x;
                    if let Some(drag) = &mut view.grip_drag {
                        drag.dragging = true;
                    }
                    let requested = start_width + column.drag_sign() * dx;
                    let width = clamp_column_width(column, requested, pane_width, view.columns);
                    if view.column_width(column) != width {
                        view.set_column_width(column, width);
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
            )
    }

    fn render_header(
        &self,
        columns: Columns,
        pane_width: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
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
            .child(self.render_graph_header(columns, pane_width, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(ui::t!("history.header.summary")),
            );
        if let Some(author) = columns.author {
            head = head.child(
                div()
                    .relative()
                    .w(px(author))
                    .flex_none()
                    .child(ui::t!("history.header.author"))
                    .child(self.render_grip(ResizeColumn::Author, pane_width, cx)),
            );
        }
        head = head.child(
            div()
                .relative()
                .w(px(columns.date))
                .flex_none()
                .child(ui::t!("history.header.date"))
                .child(self.render_grip(ResizeColumn::Date, pane_width, cx)),
        );
        if let Some(hash) = columns.hash {
            head = head.child(
                div()
                    .relative()
                    .w(px(hash))
                    .flex_none()
                    .child(ui::t!("history.header.hash"))
                    .child(self.render_grip(ResizeColumn::Hash, pane_width, cx)),
            );
        }
        div().w_full().px(px(SIDE_MARGIN)).child(head)
    }

    /// The accent-soft bar under the header while the branch filter is on.
    fn render_branch_bar(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let colors = cx.theme().colors;
        let rows = self.branch_rows.as_ref()?;
        let ready = matches!(rows.state, DayState::Ready);
        let commits = rows.items.len();
        let merges = rows.merged_from.len();
        let counts = if !ready {
            "…".to_owned()
        } else if merges > 0 {
            ui::t!(
                "history.branch_bar.with_merges",
                commits = ui::i18n::t_count("history.branch_bar.commits", commits as u64, &[]),
                merges = ui::i18n::t_count("history.branch_bar.merges", merges as u64, &[])
            )
            .to_string()
        } else {
            ui::i18n::t_count("history.branch_bar.commits", commits as u64, &[]).to_string()
        };
        let into = rows.merged_into.as_ref().map(|(oid, main)| {
            let hash = short_hash(oid);
            let oid = *oid;
            Button::new("branch-filter-into")
                .size(ButtonSize::Small)
                .label(ui::t!(
                    "history.branch_bar.merged_into",
                    main = main,
                    hash = hash
                ))
                .tooltip(ui::t!("history.branch_bar.select_merge", hash = hash))
                .on_click(cx.listener(move |view, _, _, cx| {
                    // Back to the whole history, on that merge.
                    view.set_branch_filter(None, cx);
                    view.reveal_commit(oid, cx);
                }))
        });
        Some(
            div()
                .id("history-branch-filter")
                .w_full()
                .min_h(px(32.))
                .flex_none()
                .py(px(4.))
                .pl(px(8.))
                .pr(px(12.))
                .flex()
                .items_center()
                .gap(px(12.))
                .bg(colors.accent_soft)
                .border_b_1()
                .border_color(colors.border)
                .text_size(FS_SM)
                .text_color(colors.text)
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(colors.muted)
                        .child(
                            Icon::new(IconName::Branch)
                                .size(px(13.))
                                .color(colors.muted),
                        )
                        .child(div().min_w_0().flex().items_center().children(
                            ui::i18n::pieces_children(
                                ui::i18n::t_pieces(
                                    "history.branch_bar.commits_of",
                                    &[],
                                    &["branch"],
                                ),
                                vec![div()
                                .id("history-branch-filter-name")
                                .min_w_0()
                                // The design caps the name at 40ch.
                                .max_w(px(320.))
                                .truncate()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text)
                                .tooltip(Tooltip::text(rows.branch.clone()))
                                .child(rows.branch.clone())
                                .into_any_element()],
                            ),
                        )),
                )
                .child(div().flex_none().text_color(colors.muted).child(counts))
                .children(into)
                .child(div().flex_1())
                .child(
                    Button::new("branch-filter-clear")
                        .size(ButtonSize::Small)
                        .icon(IconName::X)
                        .label(ui::t!("history.branch_bar.show_all"))
                        .on_click(cx.listener(|view, _, _, cx| view.set_branch_filter(None, cx))),
                ),
        )
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
        let loading_text = ui::i18n::t_count(
            "history.load.loading",
            loaded,
            &[("n", &format_count(loaded as usize))],
        );
        let loading_text = if self.collapse && self.geometry().folded_count > 0 {
            ui::t!(
                "history.load.with_folded",
                loading = loading_text,
                folded = ui::i18n::t_count(
                    "history.load.lanes_folded",
                    self.geometry.folded_count as u64,
                    &[]
                )
            )
        } else {
            loading_text
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
                    .child(loading_text),
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
                    .label(ui::t!("history.load.cancel"))
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
        let geometry = self.geometry().clone();
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
        let items: &[Item] = match (&self.day_rows, &self.branch_rows) {
            (Some(day), _) => &day.items,
            (None, Some(branch)) => &branch.items,
            (None, None) => &self.items,
        };
        let branch_rows = self
            .branch_rows
            .as_ref()
            .filter(|_| self.day_rows.is_none());
        // The shell asks for the checks of what is actually on screen; a fast
        // scroll only reports where it stopped.
        let visible = self.visible_commits(items, range.clone());
        if visible != self.visible_oids {
            self.visible_oids = visible;
            cx.defer_in(window, |view, _, cx| view.schedule_visible_rows(cx));
        }
        let badges = &self.badges;
        let hosting = self.hosting.clone();
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
                // Branch filter: the fork-point row, and the branch a merge
                // row brought in (its stub and "from {x}" chip).
                let is_fork = branch_rows.is_some_and(|rows| rows.fork == Some(c.id));
                let merged_from = branch_rows.and_then(|rows| rows.merged_from.get(&c.id));
                let stub = merged_from.is_some();
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
                    .when(is_fork, |row| row.opacity(FORK_ROW_OPACITY))
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
                                // The mask is 1 px taller at top and bottom so
                                // the lanes' vertical overdraw survives.
                                let mask = ContentMask {
                                    bounds: overdraw_mask_bounds(bounds),
                                };
                                window.with_content_mask(Some(mask), |window| {
                                    paint_row(
                                        &row,
                                        ix,
                                        bounds,
                                        background,
                                        &row_geometry,
                                        is_head,
                                        stub,
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
                            let suffix = remote_suffix(&spec.remotes);
                            let width = f32::from(measure_badge(
                                window,
                                &spec.label,
                                spec.kind,
                                suffix.as_ref(),
                                badge_widths,
                            ));
                            // A branch with a pull request also carries its badge.
                            match pr_badge_of(&hosting, spec) {
                                Some(_) => width + PR_BADGE_WIDTH + BADGE_GAP,
                                None => width,
                            }
                        })
                        .collect();
                    // The most hidden badges the "+N" label can stand for.
                    let plus = SharedString::from(format!("+{}", specs.len()));
                    let plus_width = f32::from(measure_badge(
                        window,
                        &plus,
                        RefKind::Overflow,
                        None,
                        badge_widths,
                    ));
                    // The "from {x}" chip is not a ref of the row but takes
                    // room beside the badges.
                    let chip_width = merged_from.map_or(0., |name| {
                        let label = ui::t!("history.badge.from", name = name);
                        f32::from(measure_badge(
                            window,
                            &label,
                            RefKind::Local,
                            None,
                            badge_widths,
                        )) + BADGE_GAP
                    });
                    badge_overflow(&widths, summary_width - chip_width, BADGE_GAP, plus_width)
                };

                let mut summary = div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(BADGE_GAP));
                for (index, spec) in specs.iter().take(fit.shown).enumerate() {
                    // The wrapper takes the right-click: `RefBadge` has no
                    // mouse hooks, and the row's own menu must not also open.
                    let menu_spec = spec.clone();
                    summary = summary.child(
                        div()
                            .id(format!("history-badge-menu-{ix}-{index}"))
                            .flex_none()
                            .flex()
                            .items_center()
                            .debug_selector(|| format!("history-badge-menu-{ix}-{index}"))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                    if badge_row(&menu_spec).is_none() {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    let focus = view.focus.clone();
                                    window.focus(&focus, cx);
                                    view.select_row(ix, cx);
                                    view.open_badge_menu(&menu_spec, event.position, window, cx);
                                    // Keep the menu that just took focus.
                                    window.prevent_default();
                                }),
                            )
                            .child({
                                let mut badge = RefBadge::new(
                                    format!("history-badge-{ix}-{index}"),
                                    spec.label.clone(),
                                )
                                .kind(spec.kind)
                                .max_label_width(px(BADGE_LABEL_MAX))
                                .tooltip(spec.tooltip.clone());
                                if let Some(suffix) = remote_suffix(&spec.remotes) {
                                    badge = badge.remote_suffix(suffix);
                                }
                                badge
                            }),
                    );
                    if let Some((branch, badge)) = pr_badge_of(&hosting, spec) {
                        let branch = branch.to_owned();
                        summary = summary.child(
                            PrBadge::new(
                                format!("history-pr-{ix}-{index}"),
                                badge.state,
                                badge.text.clone(),
                            )
                            .review(badge.review)
                            .tooltip(badge.tooltip.clone())
                            .on_click(cx.listener(
                                move |view, event: &ClickEvent, window, cx| {
                                    let focus = view.focus.clone();
                                    window.focus(&focus, cx);
                                    view.select_row(ix, cx);
                                    if let Some(handler) = view.pr_click.clone() {
                                        handler(branch.clone(), event.position(), window, cx);
                                    }
                                },
                            )),
                        );
                    }
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
                if let Some(name) = merged_from {
                    let target = name.clone();
                    summary = summary.child(
                        RefBadge::new(
                            format!("history-from-{ix}"),
                            ui::t!("history.badge.from", name = name),
                        )
                        .dashed()
                        .max_label_width(px(BADGE_LABEL_MAX))
                        .tooltip(ui::t!("history.badge.from_tooltip", name = name))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            // The row's own click must not also select a
                            // row of the list that is about to change.
                            cx.stop_propagation();
                            view.set_branch_filter(Some(target.clone()), cx);
                        })),
                    );
                }
                if let Some(state) = hosting.ci.get(&c.id) {
                    summary = summary.child(
                        CiGlyph::new(format!("history-ci-{ix}"), *state).tooltip(ci_label(*state)),
                    );
                }
                row_div = row_div.child(
                    summary
                        .child({
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
                        })
                        .when(is_fork, |summary| summary.child(fork_note(colors))),
                );
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
        let list = uniform_list(
            "history",
            self.rows().len(),
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
        .on_action(cx.listener(|view, _: &CreateBranchHere, window, cx| {
            view.create_branch_selected(window, cx);
        }));

        // The scrollbar overlays the rows rather than sitting beside them: an
        // in-flow 10px track would steal width from the columns and push the
        // Hash column out of line with the header above (which spans the full
        // pane). As an absolute overlay the table stays aligned; the opaque
        // track keeps row text from showing through beside the thumb.
        let bg = cx.theme().colors.bg;
        let scrollbar = div().absolute().top_0().bottom_0().right_0();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(list)
            .child(
                scrollbar
                    .child(Scrollbar::uniform("history-bar", self.scroll.clone()).background(bg)),
            )
            .into_any_element()
    }

    fn render_body(&mut self, columns: Columns, cx: &mut Context<Self>) -> AnyElement {
        // The state is small and cloned so the render helpers can take `&mut
        // self` for their click listeners.
        if let Some(day) = &self.day_rows {
            return match &day.state {
                DayState::Loading => self.render_skeletons(columns, cx).into_any_element(),
                DayState::Failed(message) => {
                    let message = SharedString::from(message.clone());
                    let actions = div().flex().gap(px(8.)).child(
                        Button::primary("empty-day-retry")
                            .label(ui::t!("history.action.retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    );
                    centred(
                        EmptyState::new(ui::t!("history.empty.day_failed"))
                            .icon(IconName::Warn)
                            .icon_color(cx.theme().colors.warning)
                            .text(message)
                            .actions(actions),
                    )
                }
                DayState::Ready if day.items.is_empty() => centred(
                    EmptyState::new(ui::t!("history.empty.day_none"))
                        .icon(IconName::Git)
                        .icon_color(cx.theme().colors.muted),
                ),
                DayState::Ready => self.render_list(cx),
            };
        }
        if let Some(branch) = &self.branch_rows {
            let name = SharedString::from(branch.branch.clone());
            return match &branch.state {
                DayState::Loading => self.render_skeletons(columns, cx).into_any_element(),
                DayState::Failed(message) => {
                    let message = SharedString::from(message.clone());
                    let actions = div().flex().gap(px(8.)).child(
                        Button::primary("empty-branch-retry")
                            .label(ui::t!("history.action.retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    );
                    centred(
                        EmptyState::new(ui::t!("history.empty.branch_failed"))
                            .icon(IconName::Warn)
                            .icon_color(cx.theme().colors.warning)
                            .text(message)
                            .actions(actions),
                    )
                }
                DayState::Ready if branch.items.is_empty() => {
                    let actions = div().flex().gap(px(8.)).child(
                        Button::new("empty-branch-clear")
                            .label(ui::t!("history.action.show_all_commits"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_branch_filter(None, cx);
                            })),
                    );
                    centred(
                        EmptyState::new(ui::t!("history.empty.branch_none"))
                            .icon(IconName::Branch)
                            .icon_color(cx.theme().colors.muted)
                            .text(ui::t!("history.empty.branch_none_detail", name = name))
                            .actions(actions),
                    )
                }
                DayState::Ready => self.render_list(cx),
            };
        }
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
                        ui::t!("history.strip.cancelled"),
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
            ui::toast::show(
                ToastKind::Info,
                ui::t!("history.gitignore.exists"),
                window,
                cx,
            );
        } else if let Err(error) = std::fs::write(&path, STARTER_GITIGNORE) {
            ui::toast::show(
                ToastKind::Error,
                ui::t!("history.gitignore.failed", error = error),
                window,
                cx,
            );
            return;
        } else {
            ui::toast::show(ToastKind::Ok, ui::t!("history.gitignore.added"), window, cx);
        }
        self.show_local_changes(window, cx);
    }

    /// Empty repository: an invitation to make the first commit.
    fn render_no_commits(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        let body = BodyText::new().plain(&ui::t!("history.empty.no_commits_body"));
        let actions = div()
            .flex()
            .gap(px(8.))
            .child(
                Button::primary("empty-no-commits-primary")
                    .label(ui::t!("history.empty.go_to_changes"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_local_changes(window, cx);
                    })),
            )
            .child(
                Button::new("empty-no-commits-secondary")
                    .label(ui::t!("history.empty.add_gitignore"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_gitignore(window, cx);
                    })),
            );
        centred(
            EmptyState::new(ui::t!("history.empty.no_commits"))
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
                .label(ui::t!("history.action.retry"))
                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
        );
        centred(
            EmptyState::new(ui::t!("history.empty.cancelled"))
                .icon(IconName::Fetch)
                .icon_color(cx.theme().colors.muted)
                .text(ui::t!("history.empty.cancelled_detail"))
                .actions(actions),
        )
    }

    fn render_failure(&mut self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        match failure {
            Failure::NotARepository(path) => {
                let path = path.display().to_string();
                let body = BodyText::templated(
                    &ui::t!("history.empty.not_a_repository_body"),
                    &[("path", &path)],
                );
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-not-a-repo-init")
                            .icon(IconName::Init)
                            .label(ui::t!("history.empty.init_here"))
                            .on_click(cx.listener(|this, _, _, cx| this.init_repository(cx))),
                    )
                    .child(
                        Button::new("empty-not-a-repo-open")
                            .label(ui::t!("history.action.open_another"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(workspace) = this.workspace.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.open_repository_dialog(window, cx);
                                    });
                                }
                            })),
                    );
                centred(
                    EmptyState::new(ui::t!("history.failure.not_a_repository"))
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::PathNotFound(path) => {
                let shown = path.display().to_string();
                let remove_path = path.clone();
                let body = BodyText::templated(
                    &ui::t!("history.empty.folder_missing_body"),
                    &[("path", &shown)],
                );
                // Removing the dead entry is the way out: the welcome screen
                // shows the rest of the recent list again. Retry only repeats
                // the same answer.
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-path-missing-remove")
                            .label(ui::t!("history.empty.remove_from_recent"))
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
                            .label(ui::t!("history.action.open_another"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(workspace) = this.workspace.upgrade() {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.open_repository_dialog(window, cx);
                                    });
                                }
                            })),
                    );
                centred(
                    EmptyState::new(ui::t!("history.failure.folder_not_found"))
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::GitNotFound => {
                let body = BodyText::templated(
                    &ui::t!("history.empty.git_not_found_body"),
                    &[("git", "git")],
                );
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-not-found-settings")
                            .icon(IconName::Gear)
                            .label(ui::t!("history.empty.open_git_settings"))
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(OpenGitSettings), cx);
                            })),
                    )
                    .child(self.retry_git_button("empty-git-not-found-retry", cx));
                centred(
                    EmptyState::new(ui::t!("history.failure.git_not_found"))
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
                // `{path}` stays in the template: it becomes a code run.
                let body = BodyText::templated(
                    &ui::t!(
                        "history.empty.git_too_old_body",
                        required = required,
                        source = source_label(*source),
                        found = found
                    ),
                    &[("path", &path.display().to_string())],
                );
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-too-old-settings")
                            .icon(IconName::Gear)
                            .label(ui::t!("history.empty.open_git_settings"))
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
                let template = match code {
                    Some(code) => ui::t!("history.empty.git_failed_body", code = code),
                    None => ui::t!("history.empty.git_stopped_body"),
                };
                let body =
                    BodyText::templated(&template, &[("command", &format!("git {command}"))]);
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-error-retry")
                            .icon(IconName::Fetch)
                            .label(ui::t!("history.action.retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("empty-git-error-copy")
                            .icon(IconName::Copy)
                            .label(ui::t!("history.action.copy_details"))
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
                            .label(ui::t!("history.action.retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("empty-other-copy")
                            .icon(IconName::Copy)
                            .label(ui::t!("history.action.copy_details"))
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
            .label(ui::t!("history.action.retry"))
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
                ui::t!("history.strip.git_failed", command = command)
            }
            Failure::Other { message } => ui::t!("history.strip.generic", message = message),
            _ => ui::t!("history.strip.incomplete"),
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
            .child(SelectableText::new(
                gpui::ElementId::Name("history-strip-text".into()),
                text,
            ))
            .child(div().flex_1())
            .child(
                Button::new("history-strip-retry")
                    .label(ui::t!("history.action.retry"))
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
            );
        if copy_details {
            strip = strip.child(
                Button::new("history-strip-copy")
                    .label(ui::t!("history.action.copy_details"))
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
        let columns = columns(
            pane_width,
            self.graph_width,
            self.author_width,
            self.date_width,
            self.hash_width,
        );
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
            // Esc with the list (or anything under it) focused clears the
            // day filter; without one it goes on to the next handler.
            .on_action(cx.listener(|view, _: &Cancel, _, cx| {
                if view.day_rows.is_some() {
                    view.set_day_filter(None, cx);
                } else if view.branch_rows.is_some() {
                    view.set_branch_filter(None, cx);
                } else {
                    cx.propagate();
                }
            }))
            .child(self.render_header(columns, pane_width, cx))
            .children(self.render_branch_bar(cx))
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

    /// A whole translated sentence whose `{name}` placeholders become code runs
    /// with the matching value; the rest is plain. Translators see one
    /// sentence, so word order can differ per language.
    fn templated(template: &str, code: &[(&str, &str)]) -> Self {
        let names: Vec<&str> = code.iter().map(|(name, _)| *name).collect();
        ui::i18n::split_template(template, &names)
            .into_iter()
            .fold(Self::new(), |body, piece| match piece {
                ui::i18n::Piece::Text(text) => body.plain(&text),
                ui::i18n::Piece::Slot(index) => body.code(code[index].1),
            })
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

/// Lays out the commits of one day as a graph of their own: only parents that
/// are on the same day are drawn, like the design's filtered history.
fn layout_day(commits: Vec<CommitInfo>) -> Vec<(CommitInfo, Row<Oid>)> {
    let ids: HashSet<Oid> = commits.iter().map(|c| c.id).collect();
    let mut graph = GraphBuilder::new();
    commits
        .into_iter()
        .map(|c| {
            let parents: Vec<Oid> = c
                .parents
                .iter()
                .copied()
                .filter(|p| ids.contains(p))
                .collect();
            let row = graph.push(c.id, &parents);
            (c, row)
        })
        .collect()
}

/// The dashed "fork point" note after the fork-point row's summary.
fn fork_note(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex_none()
        .ml(px(8.))
        .px(px(6.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(R_SM)
        .border_1()
        .border_dashed()
        .border_color(colors.border_strong)
        .text_color(colors.muted)
        .text_size(FS_XS)
        .child(ui::t!("history.badge.fork_point"))
}

/// Lays out a branch's commits as a graph of their own: only parents in the
/// list are drawn. A merge keeps a hollow dot even when its merged-in parent is
/// outside the list; its stub is painted from `merged_from`.
fn layout_branch(log: &BranchLog) -> Vec<(CommitInfo, Row<Oid>)> {
    layout_day(log.commits.clone())
        .into_iter()
        .map(|(commit, mut row)| {
            row.is_merge |= log.merged_from.contains_key(&commit.id);
            (commit, row)
        })
        .collect()
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

    fn local(name: &str, target: Oid) -> Ref {
        reference(
            git_core::RefKind::LocalBranch,
            &format!("refs/heads/{name}"),
            name,
            target,
        )
    }

    fn remote(name: &str, target: Oid) -> Ref {
        reference(
            git_core::RefKind::RemoteBranch,
            &format!("refs/remotes/{name}"),
            name,
            target,
        )
    }

    fn remote_targets(list: &[(&str, &str)]) -> Vec<RemoteTargets> {
        list.iter()
            .map(|(name, url)| RemoteTargets {
                name: (*name).to_owned(),
                fetch_url: (*url).to_owned(),
                push_urls: vec![(*url).to_owned()],
            })
            .collect()
    }

    /// A row menu over `refs` with HEAD `head`, one remote `origin` on GitHub
    /// and no stopped operation.
    fn row_menu(commit: Oid, head: Option<Head>, refs: Vec<Ref>) -> RowMenu {
        let targets = remote_targets(&[("origin", "https://github.com/o/r.git")]);
        let links = web_links::host_links(&[], &targets, None);
        let rows = ref_menus::commit_ref_rows(commit, head.as_ref(), &refs, &targets);
        let reset_branch = match &head {
            Some(Head::Branch { name, .. }) => name.clone(),
            _ => "HEAD".to_owned(),
        };
        let ctx = RefMenuCtx {
            head,
            refs,
            links: links.clone(),
            targets,
            operation: Operation::None,
            filter: None,
            pr_kind: None,
            default_remote: Some("origin".to_owned()),
        };
        RowMenu {
            hash: commit.to_string(),
            card: CommitCard::bare(commit),
            summary: "Subject".to_owned(),
            author: "Ann <ann@example.com>".to_owned(),
            reset_branch,
            rows,
            ctx: Rc::new(ctx),
            pr_kinds: Rc::default(),
            on_action: Rc::new(|_, _, _| {}),
            on_sidebar: None,
            open_links: links,
        }
    }

    /// Labels of the entries; a rule is `---`, the filter field `<filter>`.
    fn labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => item.label.to_string(),
                MenuEntry::Separator => "---".to_owned(),
                MenuEntry::Header(header) => header.label.to_string(),
                MenuEntry::Filter { .. } => "<filter>".to_owned(),
            })
            .collect()
    }

    fn find<'a>(entries: &'a [MenuEntry], label: &str) -> &'a MenuItem {
        entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label.as_ref() == label => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {label:?} in {:?}", labels(entries)))
    }

    /// The entries of the item's cascading child panel.
    fn open(entries: &[MenuEntry], label: &str) -> Vec<MenuEntry> {
        let submenu = find(entries, label)
            .submenu
            .clone()
            .unwrap_or_else(|| panic!("{label:?} is not a submenu"));
        submenu()
    }

    /// What follows the ref rows: the commit's own actions.
    const COMMIT_BLOCK: [&str; 9] = [
        "Checkout this commit",
        "Create branch here…",
        "Create tag here…",
        "---",
        "Cherry-pick",
        "Revert commit",
        "Reset main to here…",
        "---",
        "Copy",
    ];

    fn main_head(commit: Oid) -> Option<Head> {
        Some(Head::Branch {
            name: "main".to_owned(),
            target: Some(commit),
        })
    }

    #[test]
    fn a_row_without_refs_starts_with_the_commit_block() {
        let menu = row_menu(oid(1), main_head(oid(9)), vec![]);
        let mut expected = COMMIT_BLOCK.map(str::to_owned).to_vec();
        expected.push("Open on GitHub".to_owned());
        assert_eq!(labels(&menu.entries()), expected);
    }

    #[test]
    fn a_row_with_one_ref_still_shows_a_ref_row_with_a_submenu() {
        let commit = oid(1);
        let menu = row_menu(commit, main_head(oid(9)), vec![local("feat", commit)]);
        let entries = menu.entries();

        assert_eq!(labels(&entries)[..2], ["feat", "---"]);
        let row = find(&entries, "feat");
        assert_eq!(row.icon, Some(IconName::Branch));
        assert!(!row.accent_icon);
        assert_eq!(row.hint, None);
        let section = labels(&open(&entries, "feat"));
        assert_eq!(section[..2], ["feat", "Switch to feat"]);
    }

    #[test]
    fn the_head_branch_row_is_accented_and_hints_head_and_its_remote_twin() {
        let commit = oid(1);
        let refs = vec![
            local("main", commit),
            remote("origin/main", commit),
            remote("origin/zed", commit),
            local("feat", commit),
        ];
        let menu = row_menu(commit, main_head(commit), refs);
        let entries = menu.entries();

        // HEAD, other locals, then remote-only branches.
        assert_eq!(labels(&entries)[..4], ["main", "feat", "origin/zed", "---"]);
        let head = find(&entries, "main");
        assert!(head.accent_icon);
        assert_eq!(head.hint.as_deref(), Some("HEAD +origin"));
        assert_eq!(find(&entries, "origin/zed").icon, Some(IconName::Cloud));
        // The merged pair has one section per ref.
        let section = labels(&open(&entries, "main"));
        assert_eq!(section.iter().filter(|l| *l == "main").count(), 1);
        assert!(section.contains(&"origin/main".to_owned()));
        assert!(find(&open(&entries, "main"), "Switch to main").disabled);
    }

    #[test]
    fn more_than_six_refs_keep_five_and_a_filtered_more_refs_row() {
        let commit = oid(1);
        let refs: Vec<Ref> = (0..9).map(|n| local(&format!("b{n}"), commit)).collect();
        let menu = row_menu(commit, main_head(oid(9)), refs);
        let entries = menu.entries();

        assert_eq!(
            labels(&entries)[..7],
            ["b0", "b1", "b2", "b3", "b4", "More refs (4)", "---"]
        );
        let more = open(&entries, "More refs (4)");
        assert_eq!(labels(&more), ["<filter>", "b5", "b6", "b7", "b8"]);
        // Its rows are ref rows with their own submenus.
        assert!(find(&more, "b5").submenu.is_some());

        let six: Vec<Ref> = (0..6).map(|n| local(&format!("b{n}"), commit)).collect();
        let menu = row_menu(commit, main_head(oid(9)), six);
        assert!(!labels(&menu.entries())
            .iter()
            .any(|l| l.starts_with("More refs")));
    }

    #[test]
    fn create_pull_request_follows_the_service_of_each_branch() {
        let commit = oid(1);
        let mut menu = row_menu(commit, main_head(oid(9)), vec![local("feat", commit)]);
        let has_pr = |menu: &RowMenu| {
            labels(&open(&menu.entries(), "feat"))
                .iter()
                .any(|l| l.starts_with("Create "))
        };
        assert!(!has_pr(&menu), "no hook answer, no item");
        menu.pr_kinds = Rc::new(HashMap::from([("feat".to_owned(), ServiceKind::GitLab)]));
        let section = labels(&open(&menu.entries(), "feat"));
        assert!(section.contains(&"Create merge request…".to_owned()));
    }

    #[test]
    fn copy_offers_the_commit_texts_and_a_link_per_host() {
        let commit = oid(1);
        let menu = row_menu(commit, main_head(oid(9)), vec![]);
        let copy = open(&menu.entries(), "Copy");
        assert_eq!(
            labels(&copy),
            [
                "Commit hash",
                "Short hash",
                "Subject",
                "Full message",
                "Author",
                "Link on GitHub"
            ]
        );
        assert_eq!(
            find(&copy, "Commit hash").shortcut.as_deref(),
            Some("Ctrl+C")
        );
    }

    #[test]
    fn commit_menu_offers_open_on_one_host_or_one_per_host() {
        let commit = oid(1);
        let extras = [
            ("origin", "https://github.com/o/r.git"),
            ("publish", "https://gitlab.com/g/r.git"),
        ];
        let menu = |n: usize| {
            let mut menu = row_menu(commit, None, Vec::new());
            menu.open_links = web_links::host_links(&[], &remote_targets(&extras[..n]), None);
            menu
        };
        let tail = |menu: &RowMenu| labels(&menu.entries())[COMMIT_BLOCK.len() - 1..].to_vec();
        assert_eq!(tail(&menu(0)), ["Copy"]);
        assert_eq!(tail(&menu(1)), ["Copy", "Open on GitHub"]);
        assert_eq!(
            tail(&menu(2)),
            [
                "Copy",
                "Open on GitHub (origin)",
                "Open on GitLab (publish)"
            ]
        );
        // The links in "Copy ›" tell the hosts apart the same way.
        let copy = labels(&open(&menu(2).entries(), "Copy"));
        assert_eq!(
            copy[5..],
            ["Link on GitHub (origin)", "Link on GitLab (publish)"]
        );
    }

    #[test]
    fn badges_open_their_ref_menu_but_overflow_and_detached_head_do_not() {
        let spec = |kind, label: &str| BadgeSpec {
            kind,
            label: label.to_owned().into(),
            tooltip: "".into(),
            remotes: Vec::new(),
        };
        let name_of = |spec: BadgeSpec| badge_row(&spec).map(|row| row.name);
        assert_eq!(
            name_of(spec(RefKind::Head, "main")).as_deref(),
            Some("main")
        );
        assert_eq!(name_of(spec(RefKind::Head, "HEAD")), None);
        assert_eq!(name_of(spec(RefKind::Overflow, "+3")), None);
        assert!(matches!(
            badge_row(&spec(RefKind::Remote, "origin/main"))
                .unwrap()
                .kind,
            RefRowKind::Remote
        ));
        assert!(matches!(
            badge_row(&spec(RefKind::Tag, "v1.0")).unwrap().kind,
            RefRowKind::Tag
        ));
        let head = badge_row(&spec(RefKind::Head, "main")).unwrap();
        assert!(head.is_head());

        // A merged badge's menu has one section per ref.
        let merged = BadgeSpec {
            kind: RefKind::Local,
            label: "main".into(),
            tooltip: "refs/heads/main".into(),
            remotes: vec![SharedString::from("origin")],
        };
        let row = badge_row(&merged).unwrap();
        let commit = oid(1);
        let menu = row_menu(
            commit,
            main_head(commit),
            vec![local("main", commit), remote("origin/main", commit)],
        );
        let entries = ref_menu_entries(ref_menus::badge_sections(&menu.ctx, &row), None);
        let headers: Vec<String> = entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Header(header) => Some(header.label.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["main", "origin/main"]);
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
                (&RefKind::Tag, "v1"),
            ]
        );
        // HEAD's branch and `origin/main` are the same commit: one badge.
        assert_eq!(row[0].remotes, vec![SharedString::from("origin")]);
        // The HEAD badge names its branch's full ref.
        assert_eq!(row[0].tooltip.as_ref(), "refs/heads/main");
        // Every other badge points at its own full ref name.
        assert_eq!(row[2].tooltip.as_ref(), "refs/heads/zeta");
        assert_eq!(row[3].tooltip.as_ref(), "refs/tags/v1");
    }

    #[test]
    fn a_local_branch_and_its_remote_on_the_same_commit_merge_into_one_badge() {
        let commit = oid(1);
        let refs = vec![
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/main",
                "main",
                oid(9),
            ),
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/feat",
                "feat",
                commit,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/feat",
                "origin/feat",
                commit,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/HEAD",
                "origin/HEAD",
                commit,
            ),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(9)),
        };

        let badges = build_badges(Some(&head), &refs);

        assert_eq!(
            badges[&commit],
            vec![BadgeSpec {
                kind: RefKind::Local,
                label: "feat".into(),
                tooltip: "refs/heads/feat".into(),
                remotes: vec![SharedString::from("origin")],
            }]
        );
        assert_eq!(
            remote_suffix(&badges[&commit][0].remotes).as_deref(),
            Some("+origin")
        );
    }

    #[test]
    fn a_head_badge_merges_its_remote_on_the_same_commit() {
        let commit = oid(1);
        let refs = vec![
            local("main", commit),
            remote("origin/main", commit),
            remote("origin/HEAD", commit),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(commit),
        };

        let badges = build_badges(Some(&head), &refs);

        assert_eq!(
            badges[&commit],
            vec![BadgeSpec {
                kind: RefKind::Head,
                label: "main".into(),
                tooltip: "refs/heads/main".into(),
                remotes: vec![SharedString::from("origin")],
            }]
        );
    }

    #[test]
    fn a_symbolic_remote_head_is_never_a_badge() {
        let commit = oid(1);
        let refs = vec![remote("origin/HEAD", commit)];

        let badges = build_badges(None, &refs);

        assert!(badges.is_empty(), "{badges:?}");
    }

    #[test]
    fn a_remote_on_another_commit_stays_its_own_badge() {
        let local_commit = oid(1);
        let remote_commit = oid(2);
        let refs = vec![
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/feat",
                "feat",
                local_commit,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/feat",
                "origin/feat",
                remote_commit,
            ),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(9)),
        };

        let badges = build_badges(Some(&head), &refs);

        assert_eq!(badges[&local_commit].len(), 1);
        assert!(badges[&local_commit][0].remotes.is_empty());
        assert_eq!(
            badges[&remote_commit],
            vec![BadgeSpec {
                kind: RefKind::Remote,
                label: "origin/feat".into(),
                tooltip: "refs/remotes/origin/feat".into(),
                remotes: Vec::new(),
            }]
        );
    }

    #[test]
    fn several_remotes_on_the_same_commit_merge_into_one_badge() {
        let commit = oid(1);
        let refs = vec![
            reference(
                git_core::RefKind::LocalBranch,
                "refs/heads/feat",
                "feat",
                commit,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/upstream/feat",
                "upstream/feat",
                commit,
            ),
            reference(
                git_core::RefKind::RemoteBranch,
                "refs/remotes/origin/feat",
                "origin/feat",
                commit,
            ),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(9)),
        };

        let badges = build_badges(Some(&head), &refs);

        assert_eq!(badges[&commit].len(), 1);
        assert_eq!(
            badges[&commit][0].remotes,
            vec![SharedString::from("origin"), SharedString::from("upstream")]
        );
        assert_eq!(
            remote_suffix(&badges[&commit][0].remotes).as_deref(),
            Some("+2")
        );
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
                remotes: Vec::new(),
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

    /// The table for `pane_width` with every column at its default width.
    fn default_columns(pane_width: f32) -> Columns {
        columns(
            pane_width,
            GRAPH_WIDTH_DEFAULT,
            AUTHOR_WIDTH_DEFAULT,
            DATE_WIDTH_DEFAULT,
            HASH_WIDTH_DEFAULT,
        )
    }

    #[test]
    fn summary_width_subtracts_the_fixed_columns_and_gaps() {
        let wide = default_columns(1200.);
        assert_eq!(summary_width(1200., wide), 674.);
        let medium = default_columns(560.);
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
        // 560 px is the design's first breakpoint: Hash goes; the saved Author
        // and Date widths stay.
        let medium = default_columns(560.);
        assert_eq!(medium.hash, None);
        assert_eq!(medium.author, Some(AUTHOR_WIDTH_DEFAULT));
        assert_eq!(medium.date, DATE_WIDTH_DEFAULT);
        assert_eq!(medium.graph, GRAPH_WIDTH_DEFAULT);

        // 460 px is the second: Author goes too.
        let narrow = default_columns(460.);
        assert_eq!(narrow.hash, None);
        assert_eq!(narrow.author, None);
        assert_eq!(narrow.date, DATE_WIDTH_DEFAULT);
        assert_eq!(narrow.graph, GRAPH_WIDTH_DEFAULT);

        // A wide pane keeps all five columns.
        let wide = default_columns(1200.);
        assert_eq!(wide.author, Some(AUTHOR_WIDTH_DEFAULT));
        assert_eq!(wide.date, DATE_WIDTH_DEFAULT);
        assert_eq!(wide.hash, Some(HASH_WIDTH_DEFAULT));
    }

    #[test]
    fn columns_have_no_fixed_graph_max_only_the_summary_minimum() {
        // A 400 px graph fits the pane and is used as saved.
        assert_eq!(
            columns(
                1200.,
                400.,
                AUTHOR_WIDTH_DEFAULT,
                DATE_WIDTH_DEFAULT,
                HASH_WIDTH_DEFAULT
            )
            .graph,
            400.
        );
        // 306 px is the room a 800 px pane leaves after the other columns,
        // the gaps and the 80 px summary minimum.
        assert_eq!(columns(800., 400., 130., 104., 64.).graph, 306.);
        // No 480 px cap: the pane's room alone bounds the graph.
        let huge = columns(1200., 9999., 130., 104., 64.);
        assert_eq!(huge.graph, 706.);
        assert!(huge.graph > 480.);
        assert_eq!(summary_width(1200., huge), SUMMARY_MIN);
        // At 560 / 460 the saved graph is narrowed to keep the summary room.
        assert_eq!(columns(560., 480., 130., 104., 64.).graph, 148.);
        assert_eq!(columns(460., 480., 130., 104., 64.).graph, 196.);
        // The 40 px floor holds whatever the file or the pane says.
        assert_eq!(columns(1200., 1., 130., 104., 64.).graph, GRAPH_MIN);
        assert_eq!(columns(300., 480., 130., 104., 64.).graph, GRAPH_MIN);
        // A saved width that no longer fits a shrunk window is narrowed so the
        // summary keeps its minimum.
        assert_eq!(
            summary_width(600., columns(600., 112., 900., 104., 64.)),
            SUMMARY_MIN
        );
    }

    #[test]
    fn every_column_respects_its_minimum() {
        let tiny = columns(1200., 1., 1., 1., 1.);
        assert_eq!(tiny.graph, GRAPH_MIN);
        assert_eq!(tiny.author, Some(AUTHOR_WIDTH_MIN));
        assert_eq!(tiny.date, DATE_WIDTH_MIN);
        assert_eq!(tiny.hash, Some(HASH_WIDTH_MIN));
    }

    #[test]
    fn a_dragged_column_may_not_squeeze_the_summary_below_its_minimum() {
        let wide = default_columns(1200.);
        // Each column may grow until only the 80 px summary minimum is left.
        assert_eq!(
            clamp_column_width(ResizeColumn::Graph, 9999., 1200., wide),
            706.
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Author, 9999., 1200., wide),
            724.
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Date, 9999., 1200., wide),
            698.
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Hash, 9999., 1200., wide),
            658.
        );
        // Each column also stops at its own minimum.
        assert_eq!(
            clamp_column_width(ResizeColumn::Graph, 0., 1200., wide),
            GRAPH_MIN
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Author, 0., 1200., wide),
            AUTHOR_WIDTH_MIN
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Date, 0., 1200., wide),
            DATE_WIDTH_MIN
        );
        assert_eq!(
            clamp_column_width(ResizeColumn::Hash, 0., 1200., wide),
            HASH_WIDTH_MIN
        );
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
            xs: vec![8., 24., 40., 56.],
            folded: vec![false; 4],
            width: 64.,
            folded_count: 0,
        };
        // A centre exactly on the edge is not clipped.
        // (the painted centres are 10, 26, 42, 58 with the 2 px inset).
        assert_eq!(clipped_lanes(&geometry, 64.), 0);
        assert_eq!(clipped_lanes(&geometry, 58.), 0);
        assert_eq!(clipped_lanes(&geometry, 32.), 2);
        assert_eq!(clipped_lanes(&geometry, 26.), 2);
        assert_eq!(clipped_lanes(&geometry, 10.), 3);
    }

    #[test]
    fn skeleton_widths_are_cell_percentages() {
        let wide = default_columns(1200.);
        assert_eq!(skeleton_width(SkeletonColumn::Summary, 0, wide), px(144.));
        assert_eq!(
            skeleton_width(SkeletonColumn::Summary, 1, wide),
            px(SKELETON_SUMMARY_REF * 0.77)
        );
        assert_eq!(skeleton_width(SkeletonColumn::Author, 0, wide), px(65.));
        assert_eq!(skeleton_width(SkeletonColumn::Date, 4, wide), px(62.4));
        assert_eq!(skeleton_width(SkeletonColumn::Hash, 4, wide), px(51.2));
        // The narrow table uses the saved date column without an author.
        let narrow = default_columns(460.);
        assert_eq!(
            skeleton_width(SkeletonColumn::Date, 4, narrow),
            px(DATE_WIDTH_DEFAULT * (skeleton_percent(SkeletonColumn::Date, 4) / 100.))
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
            .code("git log --all --date-order")
            .plain(" failed with exit code 128.");
        let style = body_style(ui::Theme::light().colors);
        let runs = body.runs(&style);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            (runs[0].len, runs[1].len, runs[2].len),
            (
                "The command ".len(),
                "git log --all --date-order".len(),
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
        // `secondary-c` is Cmd+C on macOS and Ctrl+C elsewhere.
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
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
        use gpui::{Modifiers, MouseButton};

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

        // The commit has no ref, so the last item is "Copy ›": Up selects it,
        // Enter opens it with "Commit hash" highlighted, Enter copies.
        cx.simulate_keystrokes("up enter enter");
        cx.run_until_parked();
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("the row menu's Copy > Commit hash did not write the hash");
        assert_eq!(copied, b, "the row menu copied the wrong commit hash");
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                0,
                "the row menu stayed open after its item was clicked"
            );
        });
    }

    /// Right-clicking a branch badge opens that branch's own sidebar menu (not
    /// the row menu), and its items go to the sidebar action handler.
    #[gpui::test]
    fn right_clicking_a_branch_badge_opens_the_branch_menu(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton};

        let repo = crate::test_support::TempRepo::new("history-badge-menu");
        commit(repo.path(), "a.txt", "A");
        commit(repo.path(), "b.txt", "B");

        let (mut cx, history) = open_history(&repo, cx);
        let seen: Rc<std::cell::RefCell<Vec<SidebarAction>>> = Rc::default();
        let sink = seen.clone();
        history.update(&mut cx, |view, _| {
            view.set_sidebar_action_handler(Rc::new(move |action, _, _| {
                sink.borrow_mut().push(action);
            }));
        });

        let badge = cx
            .debug_bounds("history-badge-menu-0-0")
            .expect("the HEAD badge of the first row was not drawn");
        let position = badge.center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                ui::overlay::layer_count(window, cx),
                1,
                "right-clicking a badge must open exactly one menu"
            );
        });

        // The first enabled item of the HEAD branch's menu (no remote here) is
        // "New branch from main…".
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert!(
            matches!(seen.borrow().as_slice(), [SidebarAction::CreateBranchAt(_)]),
            "the badge menu is not the branch's section: {:?}",
            seen.borrow()
        );
    }

    /// The badge and row menus read the operation the shell hands the History:
    /// Merge is offered while idle and disabled while one is stopped.
    #[gpui::test]
    fn badge_menus_disable_merge_while_an_operation_is_stopped(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("history-badge-merge");
        commit(repo.path(), "a.txt", "A");

        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            let merge_enabled = |view: &HistoryView, cx: &App| {
                let ctx = view.ref_menu_ctx(cx);
                ref_menus::local_section(&ctx, "side")
                    .iter()
                    .find_map(|entry| match entry {
                        RefMenuEntry::Item(item) if item.label.starts_with("Merge ") => {
                            Some(item.enabled)
                        }
                        _ => None,
                    })
            };
            assert_eq!(merge_enabled(view, cx), Some(true));
            view.set_operation(Operation::Merge);
            assert_eq!(merge_enabled(view, cx), Some(false));
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
    fn git_in(repo: &std::path::Path, args: &[&str]) {
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

    /// `main`: c1, then `feat` (f1, f2) forks off, main goes on (m1), merges
    /// `feat` with a merge commit and adds m2. Returns the merge commit's hash.
    fn branch_filter_repo(name: &str) -> (crate::test_support::TempRepo, String) {
        let repo = crate::test_support::TempRepo::new(name);
        let path = repo.path();
        git_in(path, &["checkout", "-q", "-B", "main"]);
        commit(path, "c1", "c1");
        git_in(path, &["checkout", "-q", "-b", "feat"]);
        commit(path, "f1", "f1");
        commit(path, "f2", "f2");
        git_in(path, &["checkout", "-q", "main"]);
        commit(path, "m1", "m1");
        git_in(
            path,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "Merge branch 'feat'",
                "feat",
            ],
        );
        commit(path, "m2", "m2");
        // The merge commit is m2's parent.
        let parent = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD~1"])
            .output()
            .expect("git runs");
        assert!(parent.status.success());
        (
            repo,
            String::from_utf8_lossy(&parent.stdout).trim().to_owned(),
        )
    }

    fn summaries(history: &HistoryView) -> Vec<String> {
        history
            .rows()
            .iter()
            .map(|item| item.commit.summary.to_string())
            .collect()
    }

    /// A branch off main lists its own commits down to the fork point, flags
    /// the fork row, and knows the merge that brought it into main.
    #[gpui::test]
    fn a_feature_branch_filter_stops_at_the_fork_point(cx: &mut gpui::TestAppContext) {
        let (repo, merge) = branch_filter_repo("branch-filter-feature");
        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            view.set_branch_filter(Some("feat".to_owned()), cx)
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert_eq!(view.branch_filter(), Some("feat"));
            assert_eq!(summaries(view), ["f2", "f1", "c1"]);
            let rows = view.branch_rows.as_ref().expect("the filter is on");
            let fork = rows.fork.expect("the branch has a fork point");
            let fork_row = view.rows().iter().find(|item| item.commit.id == fork);
            assert_eq!(fork_row.expect("fork row").commit.summary, "c1");
            let (into, main) = rows.merged_into.clone().expect("feat is merged");
            assert_eq!(into.to_string(), merge);
            assert_eq!(main, "main");
            assert!(rows.merged_from.is_empty());
            // The first row is selected, at the top.
            assert_eq!(view.selected, Some(0));
        });
    }

    /// The main line keeps its merge commits, each flagged with the branch it
    /// brought in and drawn as a merge; it has no fork point or merged-into.
    #[gpui::test]
    fn the_main_line_filter_flags_merges_with_their_branch(cx: &mut gpui::TestAppContext) {
        let (repo, merge) = branch_filter_repo("branch-filter-main");
        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            view.set_branch_filter(Some("main".to_owned()), cx)
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert_eq!(summaries(view), ["m2", "Merge branch 'feat'", "m1", "c1"]);
            let rows = view.branch_rows.as_ref().expect("the filter is on");
            assert!(rows.fork.is_none() && rows.merged_into.is_none());
            let merged: Vec<(String, &String)> = rows
                .merged_from
                .iter()
                .map(|(id, from)| (id.to_string(), from))
                .collect();
            assert_eq!(merged, [(merge.clone(), &"feat".to_owned())]);
            // The merge keeps its hollow dot though only one parent is listed.
            let merge_row = view
                .rows()
                .iter()
                .find(|item| item.commit.id.to_string() == merge)
                .expect("merge row");
            assert!(merge_row.row.is_merge);
        });
    }

    /// Esc on the focused list clears the filter; a branch that is gone shows
    /// the empty state's rows (none) instead of failing.
    #[gpui::test]
    fn escape_clears_the_branch_filter(cx: &mut gpui::TestAppContext) {
        let (repo, _) = branch_filter_repo("branch-filter-esc");
        let (mut cx, history) = open_history(&repo, cx);
        history.update(&mut cx, |view, cx| {
            view.set_branch_filter(Some("nope".to_owned()), cx)
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert!(matches!(
                view.branch_rows.as_ref().map(|rows| &rows.state),
                Some(DayState::Ready)
            ));
            assert!(view.rows().is_empty());
        });
        history.update(&mut cx, |view, cx| {
            view.set_branch_filter(Some("feat".to_owned()), cx)
        });
        cx.run_until_parked();
        let focus = history.read_with(&cx, |view, _| view.list_focus());
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert!(view.branch_filter().is_none(), "Esc left the filter on");
            assert_eq!(view.commit_count(), 6);
        });
    }

    /// The day and branch filters replace each other, and the branch filter
    /// clears the search.
    #[gpui::test]
    fn the_day_and_branch_filters_are_exclusive(cx: &mut gpui::TestAppContext) {
        let (repo, _) = branch_filter_repo("branch-filter-exclusive");
        let (mut cx, history) = open_history(&repo, cx);
        let today = days_today();
        history.update(&mut cx, |view, cx| {
            view.set_search_query("m1".to_owned(), cx);
            view.set_day_filter(
                Some(DayFilter {
                    day: today,
                    me: None,
                }),
                cx,
            );
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| assert!(view.day_filter().is_some()));

        history.update(&mut cx, |view, cx| {
            view.set_branch_filter(Some("feat".to_owned()), cx)
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert!(
                view.day_filter().is_none(),
                "the branch filter kept the day"
            );
            assert_eq!(view.branch_filter(), Some("feat"));
            assert_eq!(view.search_query(), "", "the branch filter kept the search");
        });

        history.update(&mut cx, |view, cx| {
            view.set_day_filter(
                Some(DayFilter {
                    day: today,
                    me: None,
                }),
                cx,
            )
        });
        cx.run_until_parked();
        history.read_with(&cx, |view, _| {
            assert!(
                view.branch_filter().is_none(),
                "the day filter kept the branch"
            );
            assert!(view.day_filter().is_some());
        });
    }

    /// Today's local calendar day, the day the test commits were made on.
    fn days_today() -> i32 {
        let now = unix_now() as i64;
        // Commits land within seconds of now; the local offset is ignored,
        // which only matters within hours of local midnight, and then the day
        // filter is merely empty — the test checks exclusivity, not its rows.
        (now / 86_400) as i32
    }
}
