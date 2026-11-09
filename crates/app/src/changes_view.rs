//! Local changes: the two file lists, the selected file's diff with stage
//! controls, and the commit box.
//!
//! All repository work runs on the background executor. The view only holds
//! the answer: the status snapshot, the selected file's diff and the picks the
//! user made for line-level staging.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, px, uniform_list, AnyElement, App, AppContext, Bounds, Context, Div,
    Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription,
    TextRun, UniformListScrollHandle, Window,
};

use git_core::{
    ChangeKind, ConflictSide, Error, FileDiff, Git, HunkPick, Operation, Repo, WorkingFile,
    WorkingStatus, WorktreeFile,
};
use ui::actions::{key_binding, Cancel, SelectNext, SelectPrev};
use ui::components::toast::{self, ToastKind};
use ui::metrics::{FONT_MONO, FS_SM, FS_XS, MONO};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    Banner, BannerKind, Button, ButtonSize, CountPill, FileRow, IconName, ScrollArea, Scrollbar,
    SegmentItem, Segmented, SelectableText, Skeleton, Switch, TextArea, TextAreaEvent,
};

use crate::conflict_editor::{self, ConflictEditor, EditorEvent};
use crate::diff_model::{
    build_rows_with, build_split_rows, max_columns, DiffRow, RowKind, SplitCell, SplitCellKind,
    SplitRow,
};
use crate::settings::DiffLayout;

/// The width of the two file lists.
pub const FILES_WIDTH: f32 = 310.;
const DIFF_ROW: f32 = 22.;
const DIFF_GUTTER: f32 = 44.;
const DIFF_MARKER: f32 = 16.;
/// The subject line's soft limit, for the character guides and counter.
pub const SUBJECT_LIMIT: usize = 50;
/// The body's soft line-length limit.
pub const BODY_LIMIT: usize = 72;

/// Emitted when a commit succeeded, so the shell can reload the history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesEvent {
    Committed,
    /// A commit the user asked to push succeeded; the shell starts the push.
    PushAfterCommit,
    /// The merge in progress was aborted. `merge --abort` does not move HEAD,
    /// but the index, the work tree and the sidebar state changed.
    MergeAborted,
}

/// Which side of the index a file row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Unmerged paths of a merge in progress.
    Conflicted,
    Unstaged,
    Staged,
}

impl Side {
    fn staged(self) -> bool {
        self == Self::Staged
    }
}

/// One selected row: its side and index in that side's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub side: Side,
    pub index: usize,
}

/// What the view knows about the work tree.
#[derive(Clone, Debug)]
pub enum StatusState {
    /// A read is running; carries the last loaded status so counts and the
    /// banner keep their values instead of flashing to zero during a reload.
    Loading(Option<Arc<WorkingStatus>>),
    Loaded(Arc<WorkingStatus>),
    /// The read failed; keeps the last loaded status (as `Loading` does) so a
    /// merge's conflict count and banner do not reset to "all resolved".
    Failed(String, Option<Arc<WorkingStatus>>),
}

/// One file's diff, flattened into the rows the two layouts paint.
#[derive(Clone, Debug)]
pub struct ChangesDiff {
    pub diff: FileDiff,
    pub rows: Vec<DiffRow>,
    pub split_rows: Vec<SplitRow>,
    pub columns: usize,
}

/// What the diff pane knows about the selected file.
#[derive(Clone, Debug, Default)]
pub enum DiffState {
    #[default]
    Empty,
    Loading,
    Loaded(Arc<ChangesDiff>),
    /// The work-tree file of a conflicted path, markers and all.
    Conflict(Arc<ConflictFile>),
    Failed(String),
}

/// How a line of a conflicted file is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    Plain,
    /// `<<<<<<<`, `|||||||`, `=======` or `>>>>>>>`.
    Marker,
    /// Between `<<<<<<<` and `=======` (the checked-out side).
    Ours,
    /// Between `=======` and `>>>>>>>` (the merged side).
    Theirs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictLine {
    pub text: SharedString,
    pub kind: ConflictKind,
}

/// The work-tree content of a conflicted path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictFile {
    pub lines: Vec<ConflictLine>,
    /// Longest line, in characters, for the pane width.
    pub columns: usize,
    pub binary: bool,
    /// The path is not in the work tree (both sides deleted it).
    pub missing: bool,
}

/// What a stopped merge adds to the view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeInfo {
    /// The branch being merged, from the merge message (a squash has none).
    pub branch: Option<String>,
    /// The prepared commit message (`MERGE_MSG` or `SQUASH_MSG`).
    pub message: Option<String>,
}

/// Whether the merge info of a fresh quiet read matches the shown one. The
/// commit message is only read for the first status of a merge (the later reads
/// carry `None`), so it counts only when the new read has one.
fn merge_unchanged(shown: &Option<MergeInfo>, new: &Option<MergeInfo>) -> bool {
    match (shown, new) {
        (None, None) => true,
        (Some(shown), Some(new)) => {
            shown.branch == new.branch && (new.message.is_none() || shown.message == new.message)
        }
        _ => false,
    }
}

/// Whether the box still holds the untouched prefill text, so it can be
/// cleared when the merge ended without a commit. A prefill only goes into an
/// empty box, so "undo" is always "empty it".
fn prefill_untouched(current: &str, prefill: &str) -> bool {
    current.trim_end() == prefill.trim_end()
}

/// Where the conflict menu opens.
#[derive(Clone, Copy)]
enum MenuAnchor {
    /// At the pointer (right click).
    Pointer(gpui::Point<Pixels>),
    /// Under a control or row (keyboard, the "…" button).
    Bounds(Bounds<Pixels>),
}

/// Anchors a menu to `bounds`; before the first paint there are none, and the
/// pointer is the fallback.
fn anchor_or_pointer(bounds: Option<Bounds<Pixels>>, window: &Window) -> MenuAnchor {
    bounds.map_or_else(
        || MenuAnchor::Pointer(window.mouse_position()),
        MenuAnchor::Bounds,
    )
}

/// An invisible canvas filling its parent that records the parent's painted
/// bounds in `slot` under `index`.
fn bounds_probe(
    slot: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    index: usize,
) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |_, bounds, _, _| {
            slot.borrow_mut().insert(index, bounds);
        },
    )
    .absolute()
    .inset_0()
}

/// One consistent read of the work tree.
#[derive(Debug)]
pub struct Snapshot {
    pub status: WorkingStatus,
    /// `Some` while a merge or squash is in progress.
    pub merge: Option<MergeInfo>,
    /// Whatever operation the repository is stopped in.
    pub operation: Operation,
}

/// How a conflicted path is resolved from the row menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resolve {
    Take(ConflictSide),
    MarkResolved,
}

pub struct ChangesView {
    path: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    state: StatusState,
    generation: u64,
    selection: Option<Selection>,
    diff: DiffState,
    diff_generation: u64,
    /// Picked `(hunk, line)` pairs for line-level staging.
    picked: HashSet<(usize, usize)>,
    pick_anchor: Option<(usize, usize)>,
    /// The row whose discard button is waiting for confirmation.
    confirming_discard: Option<Selection>,
    message: Entity<TextArea>,
    _message_events: Subscription,
    amend: bool,
    /// The message the user typed before amend replaced it.
    draft: Option<String>,
    busy: bool,
    /// The merge in progress, if any; read with every status.
    merge: Option<MergeInfo>,
    /// The operation the repository is stopped in, read with every status.
    operation: Operation,
    /// What the merge prefill put in the box, to take it out again if the
    /// merge ends without a commit.
    prefill: Option<String>,
    /// The banner asks to confirm the abort.
    confirming_abort: bool,
    /// "Keep merging" of the abort confirm: focused when the confirm opens.
    keep_focus: FocusHandle,
    /// Painted bounds of the conflicted rows (by index) and of their "…"
    /// buttons, to anchor the resolve menu.
    conflict_row_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    conflict_button_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// The conflict editor, while it replaces the lists and the diff.
    editor: Option<(Entity<ConflictEditor>, Subscription)>,
    /// The editor went away without a window at hand: focus the view on the
    /// next paint so its keys keep working.
    refocus: bool,
    focus: FocusHandle,
    conflicts_scroll: ScrollHandle,
    unstaged_scroll: ScrollHandle,
    staged_scroll: ScrollHandle,
    diff_scroll: UniformListScrollHandle,
    mono_cell: Pixels,
    layout: DiffLayout,
}

impl ChangesView {
    pub fn new(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let message = cx.new(|cx| {
            let mut input = TextArea::new(window, cx);
            input.set_placeholder("feat(graph): …", cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_height(px(104.), cx);
            input
        });
        let message_events = cx.subscribe(&message, |_, _, event: &TextAreaEvent, cx| {
            let TextAreaEvent::Changed(_) = event;
            cx.notify();
        });
        let mono_size = cx
            .try_global::<crate::AppSettings>()
            .map(|state| px(f32::from(state.settings.mono_font_size)))
            .unwrap_or(MONO);
        let mono_cell = window
            .text_system()
            .ch_advance(
                window.text_system().resolve_font(&gpui::font(FONT_MONO)),
                mono_size,
            )
            .unwrap_or(px(7.2));
        let layout = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_layout)
            .unwrap_or_default();
        let mut view = Self {
            path,
            git,
            state: StatusState::Loading(None),
            generation: 0,
            selection: None,
            diff: DiffState::Empty,
            diff_generation: 0,
            picked: HashSet::new(),
            pick_anchor: None,
            confirming_discard: None,
            message,
            _message_events: message_events,
            amend: false,
            draft: None,
            busy: false,
            merge: None,
            operation: Operation::None,
            prefill: None,
            confirming_abort: false,
            keep_focus: cx.focus_handle().tab_stop(true),
            conflict_row_bounds: Rc::default(),
            conflict_button_bounds: Rc::default(),
            editor: None,
            refocus: false,
            focus: cx.focus_handle().tab_stop(false),
            conflicts_scroll: ScrollHandle::new(),
            unstaged_scroll: ScrollHandle::new(),
            staged_scroll: ScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            mono_cell,
            layout,
        };
        view.start_status(cx);
        view
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The status once loaded; `None` while loading or after a failure, so the
    /// lists, the diff selection and the commit box show their empty state.
    pub fn status(&self) -> Option<&WorkingStatus> {
        match &self.state {
            StatusState::Loaded(status) => Some(status),
            _ => None,
        }
    }

    fn last_arc(&self) -> Option<Arc<WorkingStatus>> {
        match &self.state {
            StatusState::Loaded(status)
            | StatusState::Loading(Some(status))
            | StatusState::Failed(_, Some(status)) => Some(status.clone()),
            _ => None,
        }
    }

    /// The last loaded status, kept through a reload or a failed read. Only the
    /// data accessors (counts, the banner and commit rules) go by it, never
    /// what is rendered.
    fn last_status(&self) -> Option<&WorkingStatus> {
        match &self.state {
            StatusState::Loaded(status)
            | StatusState::Loading(Some(status))
            | StatusState::Failed(_, Some(status)) => Some(status),
            _ => None,
        }
    }

    /// Unstaged paths, conflicted ones not included (see [`Self::conflicted_count`]).
    pub fn unstaged_count(&self) -> usize {
        self.last_status().map_or(0, WorkingStatus::unstaged_count)
    }

    pub fn staged_count(&self) -> usize {
        self.last_status().map_or(0, WorkingStatus::staged_count)
    }

    pub fn conflicted_count(&self) -> usize {
        self.last_status()
            .map_or(0, WorkingStatus::conflicted_count)
    }

    /// Every path with local work: staged, unstaged and conflicted.
    pub fn dirty_count(&self) -> usize {
        self.staged_count() + self.unstaged_count() + self.conflicted_count()
    }

    /// The operation the repository is stopped in, as of the last status.
    pub fn operation(&self) -> Operation {
        self.operation
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, StatusState::Loading(_))
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            StatusState::Failed(error, _) => Some(error),
            _ => None,
        }
    }

    /// Reloads the status, e.g. when the view becomes visible or the work tree
    /// changed outside the app.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // The view becoming visible again drops a half-answered abort confirm.
        self.confirming_abort = false;
        self.start_status(cx);
    }

    fn file(&self, selection: Selection) -> Option<&WorkingFile> {
        files_of(self.status()?, selection.side).get(selection.index)
    }

    fn selection_path(&self) -> Option<(Side, String)> {
        let selection = self.selection?;
        self.file(selection)
            .map(|file| (selection.side, file.path.clone()))
    }

    /// Whether an operation of this view (stage, commit, discard...) is running.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// [`Self::refresh`] for changes made outside the app: the loaded state
    /// stays on screen (no `Loading` flash, no empty list) and a status equal
    /// to the shown one is dropped. Skipped while an operation of this view is
    /// running (it reads the status itself when it finishes).
    pub fn refresh_quiet(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.start_status_with(true, cx);
    }

    fn start_status(&mut self, cx: &mut Context<Self>) {
        self.start_status_with(false, cx);
    }

    fn start_status_with(&mut self, quiet: bool, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let previous = self.selection_path();
        if !quiet {
            self.state = StatusState::Loading(self.last_arc());
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.state = StatusState::Failed(error.to_string(), None);
                cx.notify();
                return;
            }
        };
        let path = self.path.clone();
        let need_message = self.merge.is_none();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    read_snapshot(&repo, need_message)
                })
                .await;
            this.update(cx, |view, cx| {
                if quiet {
                    view.set_status_quiet(generation, previous, result, cx)
                } else {
                    view.set_status(generation, previous, result, cx)
                }
            })
            .ok();
        })
        .detach();
        if !quiet {
            cx.notify();
        }
    }

    /// [`Self::set_status`] that leaves the list alone when the read equals the
    /// status on screen, so a no-op refresh does not reset the selection or the
    /// picked rows. The selected file's diff is still re-read quietly.
    fn set_status_quiet(
        &mut self,
        generation: u64,
        previous: Option<(Side, String)>,
        result: Result<Snapshot, String>,
        cx: &mut Context<Self>,
    ) {
        if generation == self.generation {
            if let (Ok(snapshot), StatusState::Loaded(shown)) = (&result, &self.state) {
                if **shown == snapshot.status
                    && self.operation == snapshot.operation
                    && merge_unchanged(&self.merge, &snapshot.merge)
                {
                    // Same list, but a file can change under the same counts (one line
                    // swapped for another), so the open diff is re-read, in place.
                    self.start_diff_with(true, cx);
                    return;
                }
            }
        }
        self.set_status(generation, previous, result, cx);
    }

    fn set_status(
        &mut self,
        generation: u64,
        previous: Option<(Side, String)>,
        result: Result<Snapshot, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let (status, merge, operation) = match result {
            Ok(snapshot) => (
                Arc::new(snapshot.status),
                snapshot.merge,
                snapshot.operation,
            ),
            Err(error) => {
                self.state = StatusState::Failed(error, self.last_arc());
                self.diff = DiffState::Empty;
                // An editor waiting for the status of its save must not wait
                // for ever.
                if let Some((editor, _)) = &self.editor {
                    editor.update(cx, |editor, cx| editor.status_failed(cx));
                }
                cx.notify();
                return;
            }
        };
        let selection = previous
            .and_then(|(side, path)| {
                // The file may have moved to another list (resolved, staged).
                let others = SIDES.into_iter().filter(|other| *other != side);
                std::iter::once(side).chain(others).find_map(|side| {
                    files_of(&status, side)
                        .iter()
                        .position(|file| file.path == path)
                        .map(|index| Selection { side, index })
                })
            })
            .or_else(|| {
                SIDES.into_iter().find_map(|side| {
                    (!files_of(&status, side).is_empty()).then_some(Selection { side, index: 0 })
                })
            });
        self.selection = selection;
        self.picked.clear();
        self.pick_anchor = None;
        self.confirming_discard = None;
        self.operation = operation;
        self.state = StatusState::Loaded(status);
        self.apply_merge(merge, cx);
        self.start_diff(cx);
        self.sync_editor(cx);
        cx.notify();
    }

    /// Takes the merge state of a fresh status: a stopped merge pre-fills the
    /// commit box once and rules amend out.
    fn apply_merge(&mut self, merge: Option<MergeInfo>, cx: &mut Context<Self>) {
        match &merge {
            Some(info) => {
                if self.amend {
                    self.set_amend(false, cx);
                }
                let current = self.message.read(cx).text().to_string();
                // Only the first status of a merge prefills (`self.merge` is
                // still `None` then); later reloads leave the box alone.
                if self.merge.is_none() && current.trim().is_empty() {
                    if let Some(message) = info.message.clone().filter(|text| !text.is_empty()) {
                        self.prefill = Some(message.clone());
                        self.message
                            .update(cx, |input, cx| input.set_text(message, cx));
                    }
                }
            }
            None => {
                // The merge ended: after a commit the box is already empty; after
                // an abort (here or outside the app) an untouched prefill goes.
                if let Some(prefill) = self.prefill.take() {
                    let current = self.message.read(cx).text().to_string();
                    if prefill_untouched(&current, &prefill) {
                        self.message.update(cx, |input, cx| input.set_text("", cx));
                    }
                }
                self.confirming_abort = false;
            }
        }
        self.merge = merge;
    }

    /// Moves the file selection through both lists with the arrow keys.
    fn step_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        // The lists are hidden behind the conflict editor.
        if self.editor.is_some() {
            return;
        }
        let Some(status) = self.status() else {
            return;
        };
        let mut rows = Vec::new();
        for side in SIDES {
            rows.extend((0..files_of(status, side).len()).map(|index| Selection { side, index }));
        }
        if rows.is_empty() {
            return;
        }
        let current = self
            .selection
            .and_then(|selection| rows.iter().position(|row| *row == selection))
            .unwrap_or(0) as isize;
        let next = current
            .saturating_add(step)
            .clamp(0, rows.len() as isize - 1) as usize;
        let selection = rows[next];
        self.select(selection.side, selection.index, cx);
    }

    /// Selects a file row and loads its diff.
    fn select(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        let selection = Selection { side, index };
        if self.selection == Some(selection) {
            return;
        }
        self.selection = Some(selection);
        self.picked.clear();
        self.pick_anchor = None;
        self.confirming_discard = None;
        self.confirming_abort = false;
        self.start_diff(cx);
        cx.notify();
    }

    fn start_diff(&mut self, cx: &mut Context<Self>) {
        self.start_diff_with(false, cx);
    }

    /// `quiet` keeps the shown diff and its scroll position until the new one
    /// arrives (a refresh for an outside change, not a new selection).
    fn start_diff_with(&mut self, quiet: bool, cx: &mut Context<Self>) {
        // The conflict editor covers the diff pane: leave the hidden pane alone
        // (the editor's close reloads it).
        if self.editor.is_some() {
            return;
        }
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        if !quiet {
            self.diff_scroll = UniformListScrollHandle::new();
        }
        let Some(selection) = self.selection else {
            self.diff = DiffState::Empty;
            return;
        };
        let Some(file) = self.file(selection).cloned() else {
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
        if !quiet {
            self.diff = DiffState::Loading;
        }
        let path = self.path.clone();
        if selection.side == Side::Conflicted {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { read_conflict(git, &path, &file.path) })
                    .await;
                this.update(cx, |view, cx| view.set_conflict(generation, result, cx))
                    .ok();
            })
            .detach();
            if !quiet {
                cx.notify();
            }
            return;
        }
        let staged = selection.side.staged();
        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let options = settings.diff_options();
        let word_highlight = settings.diff_word_highlight;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_diff(git, &path, &file, staged, options, word_highlight) })
                .await;
            this.update(cx, |view, cx| view.set_diff(generation, result, cx))
                .ok();
        })
        .detach();
        if !quiet {
            cx.notify();
        }
    }

    fn set_diff(
        &mut self,
        generation: u64,
        result: Result<ChangesDiff, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.diff_generation {
            return;
        }
        self.diff = match result {
            Ok(data) => DiffState::Loaded(Arc::new(data)),
            Err(error) => DiffState::Failed(error),
        };
        cx.notify();
    }

    fn set_conflict(
        &mut self,
        generation: u64,
        result: Result<ConflictFile, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.diff_generation {
            return;
        }
        self.diff = match result {
            Ok(data) => DiffState::Conflict(Arc::new(data)),
            Err(error) => DiffState::Failed(error),
        };
        cx.notify();
    }

    fn diff_data(&self) -> Option<Arc<ChangesDiff>> {
        match &self.diff {
            DiffState::Loaded(data) => Some(data.clone()),
            _ => None,
        }
    }

    fn set_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.diff_scroll = UniformListScrollHandle::new();
        crate::update_settings(cx, |settings| settings.diff_layout = layout);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_message_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.message
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    fn set_amend(&mut self, amend: bool, cx: &mut Context<Self>) {
        if self.amend == amend || (amend && self.merge.is_some()) {
            return;
        }
        self.amend = amend;
        let current = self.message.read(cx).text().to_string();
        if amend {
            self.draft = Some(current);
            if let Some(head) = self.status().and_then(|status| status.head.as_ref()) {
                let message = head.message.trim_end().to_string();
                self.message
                    .update(cx, |input, cx| input.set_text(message, cx));
            }
        } else if let Some(draft) = self.draft.take() {
            self.message
                .update(cx, |input, cx| input.set_text(draft, cx));
        }
        cx.notify();
    }

    fn toggle_pick(&mut self, hunk: usize, line: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        if !self.can_pick(selection) {
            return;
        }
        if shift {
            if let Some((anchor_hunk, anchor_line)) = self.pick_anchor {
                if anchor_hunk == hunk {
                    let (from, to) = if anchor_line <= line {
                        (anchor_line, line)
                    } else {
                        (line, anchor_line)
                    };
                    for index in from..=to {
                        self.picked.insert((hunk, index));
                    }
                    cx.notify();
                    return;
                }
            }
        }
        let key = (hunk, line);
        if !self.picked.remove(&key) {
            self.picked.insert(key);
        }
        self.pick_anchor = Some(key);
        cx.notify();
    }

    /// Line picking works on the unstaged side for additions, modifications
    /// and deletions, and on the staged side only for modifications: those
    /// are the patches that can be reversed line by line.
    fn can_pick(&self, selection: Selection) -> bool {
        let Some(file) = self.file(selection) else {
            return false;
        };
        if selection.side == Side::Unstaged {
            matches!(
                file.kind,
                ChangeKind::Added | ChangeKind::Modified | ChangeKind::Deleted
            )
        } else {
            file.kind == ChangeKind::Modified
        }
    }

    fn picked_in(&self, hunk: usize) -> usize {
        self.picked
            .iter()
            .filter(|(picked_hunk, _)| *picked_hunk == hunk)
            .count()
    }

    fn picks(&self, hunk: usize) -> Vec<HunkPick> {
        let mut lines: Vec<usize> = self
            .picked
            .iter()
            .filter(|(picked_hunk, _)| *picked_hunk == hunk)
            .map(|(_, line)| *line)
            .collect();
        lines.sort_unstable();
        if lines.is_empty() {
            vec![HunkPick::whole(hunk)]
        } else {
            vec![HunkPick::lines(hunk, lines)]
        }
    }

    fn stage_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(file) = self.file(selection).cloned() else {
            return;
        };
        let Some(data) = self.diff_data() else {
            return;
        };
        let picks = self.picks(hunk);
        let picked = picks[0].lines.as_ref().map_or(0, Vec::len);
        let staged = selection.side.staged();
        let notice = match (picked > 0, staged) {
            (true, true) => ui::i18n::t_count("changes.notice.unstaged_lines", picked as u64, &[]),
            (true, false) => ui::i18n::t_count("changes.notice.staged_lines", picked as u64, &[]),
            (false, true) => ui::t!(
                "changes.notice.unstaged_hunk",
                n = hunk + 1,
                path = file.path
            ),
            (false, false) => ui::t!("changes.notice.staged_hunk", n = hunk + 1, path = file.path),
        }
        .to_string();
        self.picked.clear();
        self.pick_anchor = None;
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage_selection(&file, &data.diff, &picks)?;
            } else {
                repo.stage_selection(&file, &data.diff, &picks)?;
            }
            Ok(notice)
        });
    }

    /// Space on a file row: stages the selected unstaged file, or unstages the
    /// selected staged one.
    fn stage_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(selection) = self.selection {
            self.stage_file(selection.side, selection.index, cx);
        }
    }

    /// Ctrl+Shift+Z on a file row: asks to discard the selected unstaged file
    /// through the inline confirm bar, whatever the "confirm discard" setting
    /// says — a key press never discards on its own.
    fn ask_discard_selected(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(selection) = self.selection.filter(|s| s.side == Side::Unstaged) {
            if self.file(selection).is_some() {
                self.confirming_discard = Some(selection);
                cx.notify();
            }
        }
    }

    fn stage_file(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        // A conflicted path is only resolved on purpose, from its menu: Space
        // must not mark it resolved with the markers still in it.
        if side == Side::Conflicted {
            return;
        }
        let Some(file) = self
            .status()
            .and_then(|status| files_of(status, side).get(index))
            .cloned()
        else {
            return;
        };
        let path = file.path.clone();
        let staged = side.staged();
        let notice = if staged {
            ui::t!("changes.notice.unstaged_file", path = path)
        } else {
            ui::t!("changes.notice.staged_file", path = path)
        }
        .to_string();
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage(std::slice::from_ref(&path))?;
            } else {
                repo.stage(std::slice::from_ref(&path))?;
            }
            Ok(notice)
        });
    }

    pub(crate) fn stage_all(&mut self, side: Side, cx: &mut Context<Self>) {
        let Some(status) = self.status() else {
            return;
        };
        if side == Side::Conflicted {
            return;
        }
        let paths: Vec<String> = files_of(status, side)
            .iter()
            .map(|file| file.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let staged = side.staged();
        let notice = if staged {
            ui::i18n::t_count("changes.notice.unstaged_files", paths.len() as u64, &[])
        } else {
            ui::i18n::t_count("changes.notice.staged_files", paths.len() as u64, &[])
        }
        .to_string();
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage(&paths)?;
            } else {
                repo.stage(&paths)?;
            }
            Ok(notice)
        });
    }

    fn discard(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        if side == Side::Conflicted {
            return;
        }
        let Some(file) = self
            .status()
            .and_then(|status| files_of(status, side).get(index))
            .cloned()
        else {
            return;
        };
        let path = file.path.clone();
        let untracked = file.untracked;
        let notice = ui::t!("changes.notice.discarded", path = path).to_string();
        self.confirming_discard = None;
        self.start_action(cx, move |repo| {
            if untracked {
                repo.discard(&[], std::slice::from_ref(&path))?;
            } else {
                repo.discard(std::slice::from_ref(&path), &[])?;
            }
            Ok(notice)
        });
    }

    /// Runs one repository mutation, then reloads the status.
    fn start_action<F>(&mut self, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(&Repo) -> git_core::Result<String> + Send + 'static,
    {
        self.start_action_emitting(None, cx, action);
    }

    /// [`Self::start_action`] that also tells the shell what happened once the
    /// action succeeded.
    fn start_action_emitting<F>(
        &mut self,
        event: Option<ChangesEvent>,
        cx: &mut Context<Self>,
        action: F,
    ) where
        F: FnOnce(&Repo) -> git_core::Result<String> + Send + 'static,
    {
        if self.busy {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("cannot change the work tree: {error}");
                return;
            }
        };
        self.busy = true;
        let path = self.path.clone();
        let need_message = self.merge.is_none();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    let notice = action(&repo).map_err(|error| error.to_string())?;
                    let snapshot = read_snapshot(&repo, need_message)?;
                    Ok::<_, String>((notice, snapshot))
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_action(result, event, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_action(
        &mut self,
        result: Result<(String, Snapshot), String>,
        event: Option<ChangesEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        match result {
            Ok((notice, snapshot)) => {
                toast::show(ToastKind::Ok, notice, window, cx);
                let previous = self.selection_path();
                self.apply_status(snapshot, previous, cx);
                if let Some(event) = event {
                    cx.emit(event);
                }
            }
            Err(error) => {
                toast::show(ToastKind::Error, error, window, cx);
                cx.notify();
            }
        }
    }

    fn apply_status(
        &mut self,
        status: Snapshot,
        previous: Option<(Side, String)>,
        cx: &mut Context<Self>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.set_status(generation, previous, Ok(status), cx);
    }

    /// The label of the checked-out branch, or `HEAD` while detached.
    fn head_label(&self) -> String {
        self.status()
            .and_then(|status| status.branch.clone())
            .unwrap_or_else(|| "HEAD".to_string())
    }

    /// The name of the merged side: the branch from the merge message, or a
    /// neutral phrase when git's message has an unknown shape.
    fn incoming_label(&self) -> String {
        self.merge
            .as_ref()
            .and_then(|merge| merge.branch.clone())
            .unwrap_or_else(|| ui::t!("changes.conflict.incoming").to_string())
    }

    /// The name of one side of a conflict: the checked-out branch for ours,
    /// the merged one for theirs.
    fn side_label(&self, side: ConflictSide) -> String {
        match side {
            ConflictSide::Ours => self.head_label(),
            ConflictSide::Theirs => self.incoming_label(),
        }
    }

    /// Opens the conflict editor on the conflicted `path`. Only while an
    /// operation is stopped with conflicts.
    fn open_conflict_editor(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(git) = &self.git else {
            return;
        };
        let git = git.clone();
        let Some(status) = self.status() else {
            return;
        };
        if self.operation == Operation::None
            || !status.conflicted.iter().any(|file| file.path == path)
        {
            return;
        }
        let files = status
            .conflicted
            .iter()
            .map(|file| file.path.clone())
            .collect();
        let root = self.path.clone();
        let (ours, theirs) = (
            self.side_label(ConflictSide::Ours),
            self.side_label(ConflictSide::Theirs),
        );
        let editor = cx.new(|cx| ConflictEditor::new(git, root, files, path, ours, theirs, cx));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |view, _, event: &EditorEvent, window, cx| {
                match event {
                    EditorEvent::Close => {
                        view.editor = None;
                        // The diff pane was not following the status while the
                        // editor covered it: catch up.
                        view.start_diff(cx);
                        window.focus(&view.focus, cx);
                        cx.notify();
                    }
                    // The status read that follows tells the editor what is
                    // left (see `sync_editor`).
                    EditorEvent::Saved | EditorEvent::Refresh => view.start_status(cx),
                }
            },
        );
        self.editor = Some((editor, subscription));
        cx.notify();
    }

    fn open_selected_conflict_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((Side::Conflicted, path)) = self.selection_path() {
            self.open_conflict_editor(path, window, cx);
        }
    }

    /// Hands a fresh status to the open editor: it moves to the next
    /// conflicted file when its own was resolved, and the view takes the
    /// lists back when nothing is left.
    fn sync_editor(&mut self, cx: &mut Context<Self>) {
        let Some((editor, _)) = &self.editor else {
            return;
        };
        let editor = editor.clone();
        let files = self
            .status()
            .map(|status| status.conflicted.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default();
        let (ours, theirs) = (
            self.side_label(ConflictSide::Ours),
            self.side_label(ConflictSide::Theirs),
        );
        let keep = self.operation != Operation::None
            && editor.update(cx, |editor, cx| editor.sync(files, ours, theirs, cx));
        if !keep {
            self.editor = None;
            self.refocus = true;
            self.start_diff(cx);
        }
    }

    /// Resolves the conflicted `path` (see the row menu). The menu captures the
    /// path, not the row index: the list may have changed while the menu was
    /// open, and then the path is simply no longer conflicted.
    fn resolve_conflict(&mut self, path: String, how: Resolve, cx: &mut Context<Self>) {
        let still_conflicted = self
            .status()
            .is_some_and(|status| status.conflicted.iter().any(|file| file.path == path));
        if !still_conflicted {
            self.start_status(cx);
            return;
        }
        let notice = match how {
            Resolve::Take(side) => {
                ui::t!(
                    "changes.notice.took",
                    branch = self.side_label(side),
                    path = path
                )
            }
            Resolve::MarkResolved => ui::t!("changes.notice.marked_resolved", path = path),
        }
        .to_string();
        self.start_action(cx, move |repo| {
            match how {
                Resolve::Take(side) => repo.resolve_take(&path, side)?,
                Resolve::MarkResolved => repo.stage(std::slice::from_ref(&path))?,
            }
            Ok(notice)
        });
    }

    /// Abandons the merge (after the banner's inline confirm).
    fn abort_merge(&mut self, cx: &mut Context<Self>) {
        self.confirming_abort = false;
        let notice = if self.squashing() {
            ui::t!("changes.notice.squash_aborted")
        } else {
            ui::t!("changes.notice.merge_aborted")
        }
        .to_string();
        self.start_action_emitting(Some(ChangesEvent::MergeAborted), cx, move |repo| {
            repo.merge_abort()?;
            Ok(notice)
        });
        cx.notify();
    }

    /// Opens the menu of the selected conflicted row from the keyboard
    /// (Enter, Menu, Shift+F10), anchored under the row.
    fn open_selected_conflict_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.selection.filter(|s| s.side == Side::Conflicted) else {
            return;
        };
        let Some(path) = self.file(selection).map(|file| file.path.clone()) else {
            return;
        };
        let bounds = self
            .conflict_row_bounds
            .borrow()
            .get(&selection.index)
            .copied();
        let anchor = anchor_or_pointer(bounds, window);
        self.open_conflict_menu(path, anchor, window, cx);
    }

    /// The menu of the conflicted `path`: take a side, or mark it resolved.
    fn open_conflict_menu(
        &mut self,
        path: String,
        anchor: MenuAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use ui::components::menu::{self, MenuEntry, MenuItem};

        let busy = self.busy;
        let view = cx.entity();
        let item =
            |label: SharedString, icon: IconName, how: Resolve| {
                let view = view.clone();
                let path = path.clone();
                MenuEntry::Item(MenuItem::new(label).icon(icon).disabled(busy).action(
                    move |_, cx| {
                        view.update(cx, |view, cx| view.resolve_conflict(path.clone(), how, cx));
                    },
                ))
            };
        let mut entries = Vec::new();
        if self.operation != Operation::None {
            let editor_view = view.clone();
            let editor_path = path.clone();
            entries.push(MenuEntry::Item(
                MenuItem::new(ui::t!("changes.editor.open_menu"))
                    .icon(IconName::Merge)
                    .disabled(busy)
                    .action(move |window, cx| {
                        editor_view.update(cx, |view, cx| {
                            view.open_conflict_editor(editor_path.clone(), window, cx);
                        });
                    }),
            ));
            entries.push(MenuEntry::Separator);
        }
        entries.extend(vec![
            item(
                ui::t!(
                    "changes.conflict.take",
                    branch = self.side_label(ConflictSide::Ours)
                ),
                IconName::Check,
                Resolve::Take(ConflictSide::Ours),
            ),
            item(
                ui::t!(
                    "changes.conflict.take",
                    branch = self.side_label(ConflictSide::Theirs)
                ),
                IconName::Check,
                Resolve::Take(ConflictSide::Theirs),
            ),
            MenuEntry::Separator,
            item(
                ui::t!("changes.conflict.mark_resolved"),
                IconName::Plus,
                Resolve::MarkResolved,
            ),
        ]);
        match anchor {
            MenuAnchor::Pointer(position) => menu::open_context_menu(position, entries, window, cx),
            MenuAnchor::Bounds(bounds) => menu::open_dropdown(bounds, entries, window, cx),
        };
    }

    /// The operation banner of a merge or squash in progress.
    fn render_merge_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let merge = self.merge.as_ref()?;
        let squash = self.squashing();
        let left = self.conflicted_count();
        let head = self.head_label();
        let busy = self.busy;
        let view = cx.entity();
        // A squash message does not name the branch, so it has no branch title.
        let title = match &merge.branch {
            _ if squash => ui::t!("changes.squash.title_unknown", head = head),
            Some(branch) => ui::t!("changes.merge.title", branch = branch, head = head),
            None => ui::t!("changes.merge.title_unknown", head = head),
        };
        let abort_label = if squash {
            ui::t!("changes.squash.abort")
        } else {
            ui::t!("changes.merge.abort")
        };
        let commit_label = if squash {
            ui::t!("changes.squash.commit")
        } else {
            ui::t!("changes.merge.commit")
        };
        let kind = merge_banner_kind(left);
        let actions = if self.confirming_abort {
            let keep_view = view.clone();
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::danger("merge-abort-confirm")
                        .size(ButtonSize::Small)
                        .label(abort_label.clone())
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| view.abort_merge(cx));
                        }),
                )
                .child(
                    Button::new("merge-abort-keep")
                        .size(ButtonSize::Small)
                        .label(ui::t!("changes.merge.keep"))
                        .focus_handle(self.keep_focus.clone())
                        .on_click(move |_, _, cx| {
                            keep_view.update(cx, |view, cx| {
                                view.confirming_abort = false;
                                cx.notify();
                            });
                        }),
                )
        } else {
            let abort_view = view.clone();
            let can = can_commit(!squash, false, self.staged_count(), left) && !busy;
            let mut commit = Button::primary("merge-commit")
                .size(ButtonSize::Small)
                .label(commit_label)
                .disabled(!can);
            if left > 0 {
                commit = commit.tooltip(ui::t!("changes.merge.resolve_first"));
            }
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::danger("merge-abort")
                        .size(ButtonSize::Small)
                        .label(abort_label)
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            abort_view.update(cx, |view, cx| {
                                view.confirming_abort = true;
                                // Safe choice first: Enter / Space keep merging.
                                window.focus(&view.keep_focus, cx);
                                cx.notify();
                            });
                        }),
                )
                .child(commit.on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| view.commit(window, cx));
                }))
        };
        let detail = if self.confirming_abort {
            match &merge.branch {
                _ if squash => ui::t!("changes.squash.abort_confirm", head = head),
                Some(branch) => {
                    ui::t!("changes.merge.abort_confirm", branch = branch, head = head)
                }
                None => ui::t!("changes.merge.abort_confirm_unknown", head = head),
            }
        } else if left == 0 {
            if squash {
                ui::t!("changes.squash.resolved")
            } else {
                ui::t!("changes.merge.resolved")
            }
        } else {
            ui::i18n::t_count("changes.merge.left", left as u64, &[])
        };
        Some(
            div()
                .w_full()
                .flex_none()
                .debug_selector(|| "merge-banner".to_string())
                .child(
                    Banner::new(kind, title)
                        .body(detail)
                        .flat()
                        .trailing(actions),
                )
                .into_any_element(),
        )
    }

    /// A merge (not a squash) is stopped: its commit needs nothing newly staged.
    /// A squash commits like a plain commit, from what the squash staged.
    fn merging(&self) -> bool {
        self.merge.is_some() && !self.squashing()
    }

    /// A `--squash` merge is stopped: the changes are staged, there is no
    /// `MERGE_HEAD` and the commit has a single parent.
    fn squashing(&self) -> bool {
        self.operation == Operation::Squash
    }

    /// Commits the index, or amends HEAD, from the commit box.
    pub(crate) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_maybe_push(false, window, cx);
    }

    /// Commits and asks the shell to push the branch once the commit lands.
    fn commit_and_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_maybe_push(true, window, cx);
    }

    fn commit_maybe_push(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let message = self.message.read(cx).text().trim_end().to_string();
        if message.trim().is_empty() {
            toast::show(
                ToastKind::Warn,
                ui::t!("changes.commit.message_required"),
                window,
                cx,
            );
            return;
        }
        let staged = self.staged_count();
        let amend = self.amend;
        if !can_commit(self.merging(), amend, staged, self.conflicted_count()) {
            return;
        }
        let branch = self.head_label();
        let notice = if let Some(merge) = self.merge.as_ref().filter(|_| !self.squashing()) {
            match &merge.branch {
                Some(merged) => {
                    ui::t!("changes.notice.merged", branch = merged, head = branch).to_string()
                }
                None => ui::t!("changes.notice.merge_finished").to_string(),
            }
        } else if amend {
            ui::t!("changes.notice.amended").to_string()
        } else {
            ui::i18n::t_count(
                "changes.notice.committed",
                staged as u64,
                &[("branch", &branch)],
            )
            .to_string()
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("cannot commit: {error}");
                return;
            }
        };
        self.busy = true;
        let path = self.path.clone();
        let need_message = self.merge.is_none();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    repo.commit(&message, amend)
                        .map_err(|error| error.to_string())?;
                    let snapshot = read_snapshot(&repo, need_message)?;
                    Ok::<_, String>((notice, snapshot))
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_commit(result, push, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_commit(
        &mut self,
        result: Result<(String, Snapshot), String>,
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        match result {
            Ok((notice, snapshot)) => {
                toast::show(ToastKind::Ok, notice, window, cx);
                self.message.update(cx, |input, cx| input.set_text("", cx));
                self.amend = false;
                self.draft = None;
                let previous = self.selection_path();
                self.apply_status(snapshot, previous, cx);
                cx.emit(ChangesEvent::Committed);
                if push {
                    cx.emit(ChangesEvent::PushAfterCommit);
                }
            }
            Err(error) => {
                toast::show(ToastKind::Error, error, window, cx);
                cx.notify();
            }
        }
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        if self.refocus {
            self.refocus = false;
            window.focus(&self.focus, cx);
        }
        let pane_width = (f32::from(window.viewport_size().width) - FILES_WIDTH - 560.).max(320.);
        div()
            .size_full()
            .flex()
            .flex_col()
            .debug_selector(|| "changes-view".to_string())
            .key_context("changes")
            .track_focus(&self.focus)
            // While the conflict editor covers the lists, their keys do nothing.
            .on_action(cx.listener(|view, _: &CommitChanges, window, cx| {
                if view.editor.is_none() {
                    view.commit(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &StageSelected, _, cx| {
                if view.editor.is_none() {
                    view.stage_selected(cx);
                }
            }))
            .on_action(cx.listener(|view, _: &DiscardSelected, _, cx| {
                if view.editor.is_none() {
                    view.ask_discard_selected(cx);
                }
            }))
            .on_action(cx.listener(|view, _: &OpenConflictMenu, window, cx| {
                if view.editor.is_none() {
                    view.open_selected_conflict_menu(window, cx);
                }
            }))
            // Esc in the abort confirm is "Keep merging"; otherwise it goes on.
            .on_action(cx.listener(|view, _: &Cancel, _, cx| {
                if view.confirming_abort {
                    view.confirming_abort = false;
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|view, _: &SelectNext, _, cx| {
                view.step_selection(1, cx);
            }))
            .on_action(cx.listener(|view, _: &SelectPrev, _, cx| {
                view.step_selection(-1, cx);
            }))
            .children(self.render_merge_banner(cx))
            .map(
                |this| match self.editor.as_ref().map(|(editor, _)| editor.clone()) {
                    // The editor takes the whole centre area, commit box included.
                    Some(editor) => this.child(div().flex_1().min_h_0().w_full().child(editor)),
                    None => this
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .flex()
                                .child(self.render_files(cx))
                                .child(self.render_diff(pane_width, colors, cx)),
                        )
                        .child(self.render_commit_box(cx)),
                },
            )
    }

    /// The primary "Resolve…" button of a conflicted file's diff pane.
    fn resolve_button(&self, cx: &mut Context<Self>) -> Option<Button> {
        let conflicted = self
            .selection
            .is_some_and(|selection| selection.side == Side::Conflicted);
        if !conflicted || self.operation == Operation::None || self.git.is_err() {
            return None;
        }
        let view = cx.entity();
        Some(
            Button::primary("resolve-conflict")
                .size(ButtonSize::Small)
                .label(ui::t!("changes.editor.open_button"))
                .on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| {
                        view.open_selected_conflict_editor(window, cx);
                    });
                }),
        )
    }

    fn render_files(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let confirm = self.confirming_discard.and_then(|selection| {
            self.file(selection)
                .map(|file| (selection, file.path.clone()))
        });
        let mut panel = div()
            .w(px(FILES_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_start()
            .border_r_1()
            .border_color(colors.border);
        if let Some((selection, path)) = confirm {
            let discard_view = view.clone();
            let cancel_view = view.clone();
            panel = panel.child(
                div()
                    .w_full()
                    .flex_none()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(ui::t!("changes.discard.confirm", path = path)),
                    )
                    .child(
                        Button::danger("confirm-discard")
                            .size(ButtonSize::Small)
                            .label(ui::t!("changes.discard.button"))
                            .on_click(move |_, _, cx| {
                                discard_view.update(cx, |view, cx| {
                                    view.discard(selection.side, selection.index, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("cancel-discard")
                            .size(ButtonSize::Small)
                            .label(ui::t!("changes.discard.cancel"))
                            .on_click(move |_, _, cx| {
                                cancel_view.update(cx, |view, cx| {
                                    view.confirming_discard = None;
                                    cx.notify();
                                });
                            }),
                    ),
            );
        }

        // Empty lists while loading or failed; one refcount bump when loaded.
        let status = match &self.state {
            StatusState::Loaded(status) => status.clone(),
            _ => Arc::default(),
        };
        let (conflicted, unstaged, staged) = (&status.conflicted, &status.unstaged, &status.staged);
        if !conflicted.is_empty() {
            panel = panel.child(self.render_group(
                Side::Conflicted,
                ui::t!("changes.group.conflicts"),
                conflicted,
                SharedString::default(),
                self.conflicts_scroll.clone(),
                view.clone(),
                cx,
            ));
        }
        panel
            .child(self.render_group(
                Side::Unstaged,
                ui::t!("changes.group.unstaged"),
                unstaged,
                ui::t!("changes.group.unstaged_empty"),
                self.unstaged_scroll.clone(),
                view.clone(),
                cx,
            ))
            .child(self.render_group(
                Side::Staged,
                ui::t!("changes.group.staged"),
                staged,
                ui::t!("changes.group.staged_empty"),
                self.staged_scroll.clone(),
                view,
                cx,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_group(
        &mut self,
        side: Side,
        title: SharedString,
        files: &[WorkingFile],
        empty: SharedString,
        scroll: ScrollHandle,
        view: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let busy = self.busy;
        let has_conflicts = self.conflicted_count() > 0;
        let conflict_row_bounds = self.conflict_row_bounds.clone();
        let conflict_button_bounds = self.conflict_button_bounds.clone();
        let all_view = view.clone();
        let all_label = match side {
            Side::Unstaged | Side::Conflicted => ui::t!("changes.group.stage_all"),
            Side::Staged => ui::t!("changes.group.unstage_all"),
        };
        let side_name = side_id(side);
        let mut rows = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let selected = self.selection == Some(Selection { side, index });
            let status = status_letter(file);
            let path = file.path.clone();
            let mut row = FileRow::new(format!("changes-{side_name}-{index}"), status, path)
                .added(file.added)
                .deleted(file.deleted)
                .selected(selected);
            let select_view = view.clone();
            let stage_view = view.clone();
            let discard_view = view.clone();
            let stage_icon = if side == Side::Unstaged {
                IconName::Plus
            } else {
                IconName::Minus
            };
            let stage_tip = match (
                side == Side::Unstaged,
                crate::settings_ui::keybindings::key_cap(cx, "stage"),
            ) {
                (true, Some(cap)) => ui::t!("changes.row.stage_with_key", cap = cap),
                (false, Some(cap)) => ui::t!("changes.row.unstage_with_key", cap = cap),
                (true, None) => ui::t!("changes.row.stage"),
                (false, None) => ui::t!("changes.row.unstage"),
            };
            if side == Side::Conflicted {
                let menu_view = view.clone();
                let path = file.path.clone();
                let button_bounds = conflict_button_bounds.clone();
                let slot = conflict_button_bounds.clone();
                row = row.action(
                    div()
                        .relative()
                        .child(
                            Button::ghost(format!("resolve-{index}"))
                                .size(ButtonSize::Xs)
                                // No tooltip: it would stay on top of the menu this opens.
                                .label("…")
                                .disabled(busy)
                                .on_click(move |_, window, cx| {
                                    let path = path.clone();
                                    // Anchored to the button itself, not the mouse.
                                    let anchor = button_bounds.borrow().get(&index).copied();
                                    menu_view.update(cx, |view, cx| {
                                        view.select(Side::Conflicted, index, cx);
                                        let anchor = anchor_or_pointer(anchor, window);
                                        view.open_conflict_menu(path, anchor, window, cx);
                                    });
                                }),
                        )
                        .child(bounds_probe(slot, index)),
                );
            } else {
                row = row.action(
                    Button::ghost(format!("stage-{side:?}-{index}"))
                        .size(ButtonSize::Xs)
                        .icon_only(stage_icon)
                        .tooltip(stage_tip)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            stage_view.update(cx, |view, cx| view.stage_file(side, index, cx));
                        }),
                );
            }
            if side == Side::Unstaged {
                row = row.action(
                    Button::ghost(format!("discard-{side:?}-{index}"))
                        .size(ButtonSize::Xs)
                        .icon_only(IconName::Undo)
                        .tooltip(
                            match crate::settings_ui::keybindings::key_cap(cx, "discard") {
                                Some(cap) => ui::t!(
                                    "changes.row.discard_with_key",
                                    cap = cap.replace(' ', "+")
                                ),
                                None => ui::t!("changes.row.discard"),
                            },
                        )
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let confirm = cx
                                .try_global::<crate::AppSettings>()
                                .map(|state| state.settings.confirm_discard)
                                .unwrap_or(true);
                            discard_view.update(cx, |view, cx| {
                                if confirm {
                                    view.confirming_discard = Some(Selection { side, index });
                                    cx.notify();
                                } else {
                                    view.discard(side, index, cx);
                                }
                            });
                        }),
                );
            }
            let debug_id = format!("changes-{side_name}-{index}");
            let menu_view = view.clone();
            let menu_path = file.path.clone();
            let row_slot = conflict_row_bounds.clone();
            rows.push(
                div()
                    .debug_selector(move || debug_id.clone())
                    // A conflicted row also gets `changes-conflict`, for the
                    // keys that open its resolve menu.
                    .key_context(if side == Side::Conflicted {
                        "changes-file changes-conflict"
                    } else {
                        "changes-file"
                    })
                    .when(side == Side::Conflicted, |this| {
                        this.relative()
                            .child(bounds_probe(row_slot, index))
                            .on_mouse_down(
                                MouseButton::Right,
                                move |event: &MouseDownEvent, window, cx| {
                                    let position = event.position;
                                    let path = menu_path.clone();
                                    menu_view.update(cx, |view, cx| {
                                        view.select(Side::Conflicted, index, cx);
                                        view.open_conflict_menu(
                                            path,
                                            MenuAnchor::Pointer(position),
                                            window,
                                            cx,
                                        );
                                    });
                                },
                            )
                    })
                    .child(row.on_click(move |_, _, cx| {
                        select_view.update(cx, |view, cx| view.select(side, index, cx));
                    })),
            );
        }

        let list = if files.is_empty() {
            div()
                .px(px(16.))
                .py(px(10.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(empty)
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        let count = files.len();
        let conflicts = side == Side::Conflicted;
        let stage_all = Button::new(format!("stage-all-{side:?}"))
            .size(ButtonSize::Small)
            .label(all_label)
            .disabled(busy || count == 0)
            .on_click(move |_, _, cx| {
                all_view.update(cx, |view, cx| view.stage_all(side, cx));
            });
        // The conflicts group keeps its natural height (up to a few rows, then
        // it scrolls) so the two lists below stay visible.
        let conflicts_height = px(34. + count.min(6) as f32 * f32::from(cx.theme().row_height));
        div()
            .w_full()
            .when(conflicts, |this| this.flex_none().h(conflicts_height))
            .when(!conflicts, |this| this.flex_1().min_h_0())
            .flex()
            .flex_col()
            .when(
                side == Side::Staged || (side == Side::Unstaged && has_conflicts),
                |this| this.border_t_1().border_color(colors.border),
            )
            .child(
                div()
                    .w_full()
                    .h(px(34.))
                    .flex_none()
                    .pl(px(16.))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(CountPill::new(count.to_string()))
                    .child(div().flex_1())
                    .when(!conflicts, |this| this.child(stage_all)),
            )
            .child(ScrollArea::new(format!("changes-scroll-{side:?}"), scroll).child(list))
    }

    fn render_diff(&mut self, pane_width: f32, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let selection = self.selection;
        let path = selection
            .and_then(|selection| self.file(selection))
            .map(|file| match &file.old_path {
                Some(old) => format!("{} ← {old}", file.path),
                None => file.path.clone(),
            });
        let hunks = self
            .diff_data()
            .map(|data| data.diff.hunks.len())
            .unwrap_or(0);
        let layout = self.layout;
        let view = cx.entity();
        let mut toolbar = div()
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border);
        toolbar = toolbar.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(FS_SM)
                .text_color(colors.text)
                .child(path.unwrap_or_default()),
        );
        if let Some(selection) = selection {
            let label = match selection.side {
                Side::Conflicted => ui::t!("changes.conflict.label"),
                side => {
                    let side = match side {
                        Side::Unstaged => ui::t!("changes.side.unstaged"),
                        _ => ui::t!("changes.side.staged"),
                    };
                    ui::i18n::t_count("changes.diff.side_hunks", hunks as u64, &[("side", &side)])
                }
            };
            toolbar = toolbar.child(
                div()
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(label),
            );
        }
        toolbar = toolbar.child(
            Segmented::new("changes-layout")
                .small()
                .item(
                    SegmentItem::new(ui::t!("changes.layout.unified"))
                        .with_icon(IconName::Unified)
                        .id("changes-unified"),
                )
                .item(
                    SegmentItem::new(ui::t!("changes.layout.split"))
                        .with_icon(IconName::Split)
                        .id("changes-split"),
                )
                .selected(if layout == DiffLayout::Split { 1 } else { 0 })
                .on_select(move |index, _, cx| {
                    let layout = if index == 1 {
                        DiffLayout::Split
                    } else {
                        DiffLayout::Unified
                    };
                    view.update(cx, |view, cx| view.set_layout(layout, cx));
                }),
        );

        let body = match &self.diff {
            DiffState::Empty => diff_note(ui::t!("changes.diff.select_file"), colors),
            DiffState::Loading => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .child(Skeleton::new("changes-diff-skeleton-1", px(320.)))
                .child(Skeleton::new("changes-diff-skeleton-2", px(260.)))
                .child(Skeleton::new("changes-diff-skeleton-3", px(290.))),
            DiffState::Failed(message) => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .text_color(colors.error)
                .child(SelectableText::new(
                    gpui::ElementId::Name("changes-diff-error".into()),
                    message.clone(),
                )),
            DiffState::Conflict(data) => {
                let resolve = self.resolve_button(cx);
                if data.binary {
                    diff_note(ui::t!("changes.diff.binary"), colors)
                        .flex_col()
                        .gap(px(12.))
                        .children(resolve)
                } else if data.missing {
                    diff_note(ui::t!("changes.conflict.no_file"), colors)
                        .flex_col()
                        .gap(px(12.))
                        .children(resolve)
                } else {
                    let row_width = conflict_width(self.mono_cell, data.columns, pane_width);
                    let data = data.clone();
                    let list = uniform_list(
                        "changes-conflict",
                        data.lines.len(),
                        cx.processor(move |view, range, window, cx| {
                            view.render_conflict_rows(data.clone(), range, row_width, window, cx)
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_none()
                                .px(px(16.))
                                .py(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .border_b_1()
                                .border_color(colors.border)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(ui::t!("changes.conflict.note")),
                                )
                                .children(resolve),
                        )
                        .child(div().flex_1().min_h_0().flex().child(list).child(
                            Scrollbar::uniform("changes-diff-bar", self.diff_scroll.clone()),
                        ))
                }
            }
            DiffState::Loaded(data) => {
                if data.diff.binary {
                    diff_note(ui::t!("changes.diff.binary"), colors)
                } else if data.rows.is_empty() {
                    diff_note(ui::t!("changes.diff.no_changes"), colors)
                } else if layout == DiffLayout::Split {
                    let row_width = split_width(pane_width);
                    let data = data.clone();
                    let list = uniform_list(
                        "changes-diff",
                        data.split_rows.len(),
                        cx.processor(move |view, range, window, cx| {
                            view.render_split_rows(data.clone(), range, row_width, window, cx)
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "changes-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                } else {
                    let row_width = diff_width(self.mono_cell, data.columns, pane_width);
                    let data = data.clone();
                    let list = uniform_list(
                        "changes-diff",
                        data.rows.len(),
                        cx.processor(move |view, range, window, cx| {
                            view.render_unified_rows(data.clone(), range, row_width, window, cx)
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "changes-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                }
            }
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(cx.theme().mono_font_size)
            .child(toolbar)
            .child(body)
    }

    fn render_unified_rows(
        &mut self,
        data: Arc<ChangesDiff>,
        range: std::ops::Range<usize>,
        row_width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let pane_width = px(f32::from(row_width));
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        let selection = self.selection;
        let staged = selection.is_some_and(|selection| selection.side.staged());
        let can_pick = selection.is_some_and(|selection| self.can_pick(selection));
        let hunk_button_allowed = selection.is_some_and(|selection| {
            self.file(selection).is_some_and(|file| {
                matches!(
                    file.kind,
                    ChangeKind::Added | ChangeKind::Modified | ChangeKind::Deleted
                ) && !(selection.side == Side::Staged && file.kind == ChangeKind::Added)
            })
        });
        range
            .filter_map(|index| {
                let row = data.rows.get(index)?;
                let row = row.clone();
                match row.kind {
                    RowKind::Hunk => {
                        let hunk = row.hunk.unwrap_or(0);
                        let picked = self.picked_in(hunk);
                        let label = match (picked, staged) {
                            (0, false) => ui::t!("changes.hunk.stage"),
                            (0, true) => ui::t!("changes.hunk.unstage"),
                            (n, false) => {
                                ui::i18n::t_count("changes.hunk.stage_lines", n as u64, &[])
                            }
                            (n, true) => {
                                ui::i18n::t_count("changes.hunk.unstage_lines", n as u64, &[])
                            }
                        };
                        let stage_view = cx.entity();
                        let button = Button::new(format!("changes-hunk-{hunk}"))
                            .size(ButtonSize::Small)
                            .label(label)
                            .disabled(self.busy || !hunk_button_allowed)
                            .on_click(move |_, _, cx| {
                                stage_view.update(cx, |view, cx| view.stage_hunk(hunk, cx));
                            });
                        Some(
                            div()
                                .id(SharedString::from(format!("changes-hunk-row-{index}")))
                                .h(px(DIFF_ROW))
                                .w(pane_width)
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .pl(px(8.))
                                .pr(px(6.))
                                .bg(colors.diff_hunk_bg)
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(div().flex_1().min_w_0().truncate().child(row.text.clone()))
                                .child(button)
                                .into_any_element(),
                        )
                    }
                    RowKind::Note => Some(
                        div()
                            .h(px(DIFF_ROW))
                            .w(row_width)
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(8.))
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(row.text.clone())
                            .into_any_element(),
                    ),
                    RowKind::Context | RowKind::Added | RowKind::Removed => {
                        let (old_background, new_background, line_background, strong) =
                            match row.kind {
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
                        let picked = row
                            .line
                            .zip(row.hunk)
                            .is_some_and(|(line, hunk)| self.picked.contains(&(hunk, line)));
                        let target = row
                            .line
                            .zip(row.hunk)
                            .filter(|_| matches!(row.kind, RowKind::Added | RowKind::Removed));
                        let pick_view = cx.entity();
                        let gutter = div()
                            .id(SharedString::from(format!("changes-gutter-{index}")))
                            .flex()
                            .flex_none()
                            .h_full()
                            .when(can_pick && target.is_some(), |this| this.cursor_pointer())
                            .on_click(move |event, _, cx| {
                                let Some((line, hunk)) = target else {
                                    return;
                                };
                                let shift = event.modifiers().shift;
                                pick_view.update(cx, |view, cx| {
                                    view.toggle_pick(hunk, line, shift, cx);
                                });
                            })
                            .when(show_numbers, |this| {
                                this.child(diff_gutter(row.old_line, old_background, colors))
                                    .child(diff_gutter(row.new_line, new_background, colors))
                            });
                        let mut element = div()
                            .id(SharedString::from(format!("changes-row-{index}")))
                            .relative()
                            .h(px(DIFF_ROW))
                            .w(row_width)
                            .flex_none()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .child(gutter)
                            .child(diff_marker(row.kind, line_background, colors));
                        if picked {
                            element = element.child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(0.))
                                    .w(px(3.))
                                    .h_full()
                                    .bg(colors.accent),
                            );
                        }
                        Some(
                            element
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .pl(px(6.))
                                        .pr(px(6.))
                                        .when_some(line_background, |this, background| {
                                            this.bg(background)
                                        })
                                        .text_color(colors.text)
                                        .child(emphasized_text(
                                            &row.text,
                                            &row.emphasis,
                                            strong,
                                            colors,
                                        )),
                                )
                                .into_any_element(),
                        )
                    }
                }
            })
            .collect()
    }

    /// Rows of a conflicted file: line number, then the text, with the marker
    /// lines tinted as a warning and the two sides like removed / added lines.
    fn render_conflict_rows(
        &mut self,
        data: Arc<ConflictFile>,
        range: std::ops::Range<usize>,
        row_width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        range
            .filter_map(|index| {
                let line = data.lines.get(index)?;
                let (background, color, weight) = match line.kind {
                    ConflictKind::Marker => (
                        Some(colors.warning.opacity(0.18)),
                        colors.warning,
                        FontWeight::SEMIBOLD,
                    ),
                    ConflictKind::Ours => {
                        (Some(colors.diff_del_bg), colors.text, FontWeight::NORMAL)
                    }
                    ConflictKind::Theirs => {
                        (Some(colors.diff_add_bg), colors.text, FontWeight::NORMAL)
                    }
                    ConflictKind::Plain => (None, colors.text, FontWeight::NORMAL),
                };
                Some(
                    div()
                        .id(SharedString::from(format!("changes-conflict-row-{index}")))
                        .h(px(DIFF_ROW))
                        .w(row_width)
                        .flex_none()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .child(diff_gutter(Some(index as u32 + 1), None, colors))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .px(px(6.))
                                .when_some(background, |this, background| this.bg(background))
                                .text_color(color)
                                .font_weight(weight)
                                .child(line.text.clone()),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_split_rows(
        &mut self,
        data: Arc<ChangesDiff>,
        range: std::ops::Range<usize>,
        row_width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        let gutters = if show_numbers { 2. * DIFF_GUTTER } else { 0. };
        let half = px(((f32::from(row_width) - gutters) / 2.).max(0.));
        range
            .filter_map(|index| {
                let row = data.split_rows.get(index)?;
                Some(split_row(row, index, row_width, half, colors, show_numbers))
            })
            .collect()
    }

    fn render_commit_box(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let text = self.message.read(cx).text().to_string();
        let (subject, longest_body) = message_counters(&text);
        let head = self.status().and_then(|status| status.head.clone());
        let branch = self.head_label();
        let staged = self.staged_count();
        let busy = self.busy;
        let merging = self.merging();
        let commit_label = if merging {
            ui::t!("changes.merge.commit")
        } else if self.amend {
            ui::t!("changes.commit.amend")
        } else {
            ui::i18n::t_count(
                "changes.commit.button",
                staged as u64,
                &[("branch", &branch)],
            )
        };
        let can_commit = !busy
            && !commit_label.is_empty()
            && can_commit(merging, self.amend, staged, self.conflicted_count());
        let commit_view = cx.entity();
        let amend_view = cx.entity();
        let switch = Switch::new("amend-switch")
            .label(ui::t!("changes.commit.amend_switch"))
            .checked(self.amend)
            .disabled(busy || head.is_none() || self.merge.is_some())
            .on_toggle(move |checked, _, cx| {
                amend_view.update(cx, |view, cx| view.set_amend(checked, cx));
            });
        let help = match &head {
            Some(head) => ui::t!("changes.commit.amend_help", short = head.short),
            None => ui::t!("changes.commit.amend_none"),
        };
        let guide_50 = 10. + SUBJECT_LIMIT as f32 * f32::from(self.mono_cell);
        let guide_72 = 10. + BODY_LIMIT as f32 * f32::from(self.mono_cell);
        let counter = |value: usize, limit: usize, label: SharedString| {
            div()
                .text_size(FS_XS)
                .text_color(if value > limit {
                    colors.warning
                } else {
                    colors.muted
                })
                .child(format!("{label} {value} / {limit}"))
        };
        div()
            .w_full()
            .flex_none()
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .p(px(12.))
            .flex()
            .gap(px(16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .mb(px(4.))
                            .child(ui::t!("changes.commit.message_help")),
                    )
                    .child(
                        div()
                            .relative()
                            .child(self.message.clone())
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .left(px(guide_50))
                                    .w(px(1.))
                                    .bg(colors.border_strong)
                                    .opacity(0.7),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .left(px(guide_72))
                                    .w(px(1.))
                                    .bg(colors.border_strong),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .child(counter(
                                subject,
                                SUBJECT_LIMIT,
                                ui::t!("changes.commit.subject"),
                            ))
                            .child(counter(
                                longest_body,
                                BODY_LIMIT,
                                ui::t!("changes.commit.longest_body"),
                            ))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(FS_XS)
                                    .text_color(colors.muted)
                                    .child(ui::t!(
                                        "changes.commit.hint",
                                        key = ui::kbd::platform_label("Ctrl+Enter")
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(260.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(switch)
                            .child(div().text_size(FS_XS).text_color(colors.muted).child(help)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child({
                                let commit_view = commit_view.clone();
                                Button::primary("commit-button")
                                    .label(commit_label)
                                    .disabled(!can_commit)
                                    .on_click(move |_, window, cx| {
                                        commit_view.update(cx, |view, cx| view.commit(window, cx));
                                    })
                            })
                            .child({
                                let commit_view = commit_view.clone();
                                Button::new("commit-push-button")
                                    .label(ui::t!("changes.commit.commit_and_push"))
                                    .disabled(!can_commit)
                                    .on_click(move |_, window, cx| {
                                        commit_view.update(cx, |view, cx| {
                                            view.commit_and_push(window, cx)
                                        });
                                    })
                            }),
                    ),
            )
    }
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render(window, cx)
    }
}

impl EventEmitter<ChangesEvent> for ChangesView {}

/// Binds Ctrl+Enter for the commit box. Called once at startup.
pub fn bind_keys(cx: &mut App, settings: &crate::settings::Settings) {
    let key = settings
        .keybindings
        .get("commit")
        .filter(|keys| !keys.is_empty())
        .cloned()
        .unwrap_or_else(|| "secondary-enter".to_owned());
    let keys = |id: &str, default: &str| {
        settings
            .keybindings
            .get(id)
            .filter(|keys| !keys.is_empty())
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    // The file-row keys live in their own context: it is active on a row and
    // its buttons only, so Space still types in the message editor and a
    // focused button keeps its own Space.
    let file = Some("changes-file && !ui-button");
    cx.bind_keys([
        key_binding(cx, &key, CommitChanges, None),
        key_binding(cx, &keys("stage", "space"), StageSelected, file),
        key_binding(
            cx,
            &keys("discard", "secondary-shift-z"),
            DiscardSelected,
            file,
        ),
    ]);
    // A conflicted row has no stage/discard keys; its resolve menu opens on
    // Enter, the Menu key and Shift+F10 (not on a focused button inside it).
    let conflict = Some("changes-conflict && !ui-button");
    cx.bind_keys([
        key_binding(cx, "enter", OpenConflictMenu, conflict),
        key_binding(cx, "menu", OpenConflictMenu, conflict),
        key_binding(cx, "shift-f10", OpenConflictMenu, conflict),
    ]);
    conflict_editor::bind_keys(cx);
}

actions!(
    gitwarp,
    [
        CommitChanges,
        StageSelected,
        DiscardSelected,
        OpenConflictMenu
    ]
);

/// Reads one file's diff and flattens it into rows off the UI thread.
fn read_diff(
    git: Arc<Git>,
    root: &Path,
    file: &WorkingFile,
    staged: bool,
    options: git_core::DiffOptions,
    word_highlight: bool,
) -> Result<ChangesDiff, String> {
    let repo = Repo::open(git, root).map_err(|error| error.to_string())?;
    let diff = if staged {
        repo.staged_diff_with(&file.path, file.old_path.as_deref(), options)
            .map_err(|error| error.to_string())?
    } else if file.untracked {
        repo.untracked_diff(&file.path)
            .map_err(|error| error.to_string())?
    } else {
        repo.unstaged_diff_with(&file.path, file.old_path.as_deref(), options)
            .map_err(|error| error.to_string())?
    };
    let rows = build_rows_with(&diff.hunks, word_highlight);
    let split_rows = build_split_rows(&rows);
    let columns = max_columns(&rows);
    Ok(ChangesDiff {
        diff,
        rows,
        split_rows,
        columns,
    })
}

/// The lists top to bottom, as the arrow keys walk them.
const SIDES: [Side; 3] = [Side::Conflicted, Side::Unstaged, Side::Staged];

fn files_of(status: &WorkingStatus, side: Side) -> &[WorkingFile] {
    match side {
        Side::Conflicted => &status.conflicted,
        Side::Unstaged => &status.unstaged,
        Side::Staged => &status.staged,
    }
}

fn side_id(side: Side) -> &'static str {
    match side {
        Side::Conflicted => "conflicted",
        Side::Unstaged => "unstaged",
        Side::Staged => "staged",
    }
}

/// Whether the commit button is live. A merge commit is valid with nothing
/// newly staged (the merge result is in the index already) but never while a
/// path is unmerged, and a merge cannot be amended into.
fn can_commit(merging: bool, amend: bool, staged: usize, conflicts: usize) -> bool {
    if conflicts > 0 {
        return false;
    }
    if merging {
        return !amend;
    }
    amend || staged > 0
}

/// A merge banner is a warning while conflicts remain, plain info after.
fn merge_banner_kind(conflicts: usize) -> BannerKind {
    if conflicts > 0 {
        BannerKind::Warn
    } else {
        BannerKind::Info
    }
}

/// Reads the status and, while a merge is stopped, its branch and (only when
/// `need_message`) its prepared message, which only the first status of a
/// merge puts into the commit box.
fn read_snapshot(repo: &Repo, need_message: bool) -> Result<Snapshot, String> {
    let status = repo.working_status().map_err(|error| error.to_string())?;
    let operation = repo.operation().map_err(|error| error.to_string())?;
    let merge = match operation {
        Operation::Merge | Operation::Squash => {
            // A missing or odd message only costs the branch name / the prefill.
            let (branch, message) = if need_message {
                repo.merge_info(operation).unwrap_or_default()
            } else if operation == Operation::Merge {
                (repo.merge_heading().ok().flatten(), None)
            } else {
                (None, None)
            };
            Some(MergeInfo { branch, message })
        }
        _ => None,
    };
    Ok(Snapshot {
        status,
        merge,
        operation,
    })
}

/// Reads the work-tree file of a conflicted path off the UI thread.
fn read_conflict(git: Arc<Git>, root: &Path, path: &str) -> Result<ConflictFile, String> {
    let repo = Repo::open(git, root).map_err(|error| error.to_string())?;
    let text = match repo.worktree_file(path) {
        Ok(WorktreeFile::Text(text)) => text,
        Ok(WorktreeFile::Binary) => {
            return Ok(ConflictFile {
                binary: true,
                ..ConflictFile::default()
            })
        }
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConflictFile {
                missing: true,
                ..ConflictFile::default()
            })
        }
        Err(error) => return Err(error.to_string()),
    };
    let lines = conflict_lines(text.lines.iter().map(String::as_str));
    let columns = lines
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    Ok(ConflictFile {
        lines,
        columns,
        ..ConflictFile::default()
    })
}

/// Classifies the lines of a file with conflict markers. A `=======` outside
/// a `<<<<<<<` block is ordinary text; the diff3 base block (`|||||||` to
/// `=======`) is left plain.
fn conflict_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<ConflictLine> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let is_marker = |line: &str, marker: &str| {
        line.strip_prefix(marker)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
    };
    let mut state = State::Outside;
    lines
        .into_iter()
        .map(|line| {
            let kind = if is_marker(line, "<<<<<<<") && state == State::Outside {
                state = State::Ours;
                ConflictKind::Marker
            } else if is_marker(line, "|||||||") && state == State::Ours {
                state = State::Base;
                ConflictKind::Marker
            } else if is_marker(line, "=======") && matches!(state, State::Ours | State::Base) {
                state = State::Theirs;
                ConflictKind::Marker
            } else if is_marker(line, ">>>>>>>") && state == State::Theirs {
                state = State::Outside;
                ConflictKind::Marker
            } else {
                match state {
                    State::Ours => ConflictKind::Ours,
                    State::Theirs => ConflictKind::Theirs,
                    State::Outside | State::Base => ConflictKind::Plain,
                }
            };
            ConflictLine {
                text: line.to_string().into(),
                kind,
            }
        })
        .collect()
}

fn conflict_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let text = columns as f32 * f32::from(cell) + 24.;
    px((DIFF_GUTTER + text).max(pane))
}

/// The glyph a working-tree file shows: untracked and conflicted files keep
/// git's own letters instead of the change kind.
fn status_letter(file: &WorkingFile) -> char {
    if file.untracked {
        '?'
    } else {
        file.kind.letter()
    }
}

/// Changed paths of the work tree, grouped the way the sidebar's view nav
/// shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    /// Added and untracked paths.
    pub added: usize,
    /// Modified, renamed, copied, type-changed and conflicted paths.
    pub modified: usize,
    pub deleted: usize,
}

impl ChangeCounts {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.modified == 0 && self.deleted == 0
    }
}

/// Counts the unique changed paths in `status`: a path on both sides of the
/// index counts once, with the unstaged kind when the two differ. `None`
/// (status loading or failed) counts nothing.
pub fn change_counts(status: Option<&WorkingStatus>) -> ChangeCounts {
    let Some(status) = status else {
        return ChangeCounts::default();
    };
    // The index is the older state of a path; the work tree overrides it.
    let mut files: HashMap<&str, &WorkingFile> = HashMap::new();
    for file in &status.staged {
        files.insert(file.path.as_str(), file);
    }
    for file in &status.unstaged {
        files.insert(file.path.as_str(), file);
    }
    for file in &status.conflicted {
        files.insert(file.path.as_str(), file);
    }
    let mut counts = ChangeCounts::default();
    for file in files.values() {
        if file.untracked || file.kind == ChangeKind::Added {
            counts.added += 1;
        } else if file.kind == ChangeKind::Deleted {
            counts.deleted += 1;
        } else {
            counts.modified += 1;
        }
    }
    counts
}

/// The subject length and the longest body line, for the counters.
fn message_counters(text: &str) -> (usize, usize) {
    let mut lines = text.split('\n');
    let subject = lines.next().unwrap_or("").chars().count();
    let body = lines.map(|line| line.chars().count()).max().unwrap_or(0);
    (subject, body)
}

/// A muted, centred note for a pane with nothing to paint.
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

fn diff_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let text = columns as f32 * f32::from(cell) + 24.;
    px((DIFF_GUTTER * 2. + DIFF_MARKER + text).max(pane))
}

fn split_width(pane: f32) -> Pixels {
    // Two equal halves that fill the pane whatever the content width; each
    // half clips its own lines (design: `.sbs { overflow: hidden }`).
    px(pane)
}

/// One 44 px line-number gutter.
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

/// Text with the changed words drawn on their stronger background.
fn emphasized_text(
    text: &str,
    emphasis: &[std::ops::Range<usize>],
    strong: Hsla,
    colors: ThemeColors,
) -> StyledText {
    if emphasis.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    let font = gpui::font(FONT_MONO);
    let mut runs = Vec::new();
    let mut position = 0;
    let push = |runs: &mut Vec<TextRun>, len: usize, background: Option<Hsla>| {
        if len == 0 {
            return;
        }
        runs.push(TextRun {
            len,
            font: font.clone(),
            color: colors.text,
            background_color: background,
            underline: None,
            strikethrough: None,
        });
    };
    for range in emphasis {
        let start = range.start.min(text.len());
        let end = range.end.min(text.len());
        if start > position {
            push(&mut runs, start - position, None);
        }
        push(&mut runs, end.saturating_sub(start), Some(strong));
        position = end;
    }
    if position < text.len() {
        push(&mut runs, text.len() - position, None);
    }
    if runs.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
}

/// One side-by-side row.
fn split_row(
    row: &SplitRow,
    index: usize,
    width: Pixels,
    half: Pixels,
    colors: ThemeColors,
    show_numbers: bool,
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("changes-split-row-{index}")))
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
            base = base.child(split_content(old.as_ref(), half, colors));
            if show_numbers {
                base = base.child(
                    split_gutter(new.as_ref(), colors)
                        .border_l_1()
                        .border_color(colors.border),
                );
            }
            base.child(split_content(new.as_ref(), half, colors))
                .into_any_element()
        }
    }
}

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

fn split_content(cell: Option<&SplitCell>, width: Pixels, colors: ThemeColors) -> Div {
    let (background, strong) = match cell.map(|cell| cell.kind) {
        Some(SplitCellKind::Added) => (Some(colors.diff_add_bg), colors.diff_add_strong),
        Some(SplitCellKind::Removed) => (Some(colors.diff_del_bg), colors.diff_del_strong),
        _ => (None, colors.diff_add_strong),
    };
    let content = div()
        .w(width)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .pl(px(6.))
        .pr(px(6.))
        .overflow_hidden()
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.text);
    match cell {
        Some(cell) => content.child(emphasized_text(&cell.text, &cell.emphasis, strong, colors)),
        None => content.bg(colors.hover),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_split_the_subject_from_the_body() {
        assert_eq!(message_counters(""), (0, 0));
        assert_eq!(message_counters("subject"), (7, 0));
        assert_eq!(message_counters("subject\n\nbody line\nx"), (7, 9));
        assert_eq!(message_counters("привет"), (6, 0));
    }

    #[test]
    fn untracked_files_keep_gits_question_mark() {
        let file = WorkingFile {
            kind: ChangeKind::Added,
            old_path: None,
            path: "new.txt".into(),
            added: 1,
            deleted: 0,
            binary: false,
            untracked: true,
        };
        assert_eq!(status_letter(&file), '?');
    }

    #[test]
    fn change_counts_are_empty_without_a_status() {
        assert_eq!(change_counts(None), ChangeCounts::default());
        assert!(change_counts(Some(&WorkingStatus::default())).is_empty());
    }

    #[test]
    fn change_counts_group_paths_by_kind() {
        let status = WorkingStatus {
            staged: vec![
                working_file("a.txt", ChangeKind::Added, false),
                working_file("b.txt", ChangeKind::Deleted, false),
            ],
            unstaged: vec![
                working_file("c.txt", ChangeKind::Added, true),
                working_file("d.txt", ChangeKind::Modified, false),
                working_file("e.txt", ChangeKind::Renamed, false),
                working_file("f.txt", ChangeKind::Other('U'), false),
            ],
            ..WorkingStatus::default()
        };
        assert_eq!(
            change_counts(Some(&status)),
            ChangeCounts {
                added: 2,
                modified: 3,
                deleted: 1,
            }
        );
    }

    #[test]
    fn change_counts_count_a_path_on_both_sides_once() {
        let status = WorkingStatus {
            staged: vec![
                working_file("a.txt", ChangeKind::Added, false),
                working_file("b.txt", ChangeKind::Deleted, false),
            ],
            unstaged: vec![
                working_file("a.txt", ChangeKind::Modified, false),
                working_file("b.txt", ChangeKind::Modified, false),
            ],
            ..WorkingStatus::default()
        };
        // The work tree's kind wins: both paths count as modified, once each.
        assert_eq!(
            change_counts(Some(&status)),
            ChangeCounts {
                added: 0,
                modified: 2,
                deleted: 0,
            }
        );
    }

    #[test]
    fn conflicted_paths_count_once_as_modified() {
        let status = WorkingStatus {
            conflicted: vec![working_file("c.txt", ChangeKind::Other('U'), false)],
            ..WorkingStatus::default()
        };
        assert_eq!(
            change_counts(Some(&status)),
            ChangeCounts {
                added: 0,
                modified: 1,
                deleted: 0,
            }
        );
    }

    #[test]
    fn the_merge_banner_warns_until_the_last_conflict_is_gone() {
        assert_eq!(merge_banner_kind(2), BannerKind::Warn);
        assert_eq!(merge_banner_kind(1), BannerKind::Warn);
        assert_eq!(merge_banner_kind(0), BannerKind::Info);
    }

    #[test]
    fn an_untouched_prefill_is_cleared_but_an_edited_one_is_kept() {
        let prefill = "Merge branch 'side'";
        assert!(prefill_untouched("Merge branch 'side'\n", prefill));
        assert!(!prefill_untouched("Merge branch 'side' + notes", prefill));
        assert!(!prefill_untouched("", prefill));
    }

    #[test]
    fn commit_rules_for_plain_amend_and_merge() {
        // Plain: needs something staged, or an amend.
        assert!(!can_commit(false, false, 0, 0));
        assert!(can_commit(false, false, 2, 0));
        assert!(can_commit(false, true, 0, 0));
        // A merge commits with nothing newly staged, but not amended.
        assert!(can_commit(true, false, 0, 0));
        assert!(!can_commit(true, true, 0, 0));
        // Unmerged paths block everything.
        assert!(!can_commit(true, false, 3, 1));
        assert!(!can_commit(false, false, 3, 1));
    }

    #[test]
    fn conflict_markers_and_both_sides_are_classified() {
        use ConflictKind::*;
        let text = "top\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> side\nbottom\n=======\n";
        let kinds: Vec<_> = conflict_lines(text.lines())
            .into_iter()
            .map(|l| l.kind)
            .collect();
        assert_eq!(
            kinds,
            [Plain, Marker, Ours, Marker, Plain, Marker, Theirs, Marker, Plain, Plain]
        );
        assert!(conflict_lines("".lines()).is_empty());
    }

    #[test]
    fn a_single_line_diff_is_never_narrower_than_the_pane() {
        assert!(diff_width(px(7.), 4, 400.) >= px(400.));
        assert!(split_width(400.) >= px(400.));
    }

    /// A changed file of `path`, for the counter tests.
    fn working_file(path: &str, kind: ChangeKind, untracked: bool) -> WorkingFile {
        WorkingFile {
            kind,
            old_path: None,
            path: path.into(),
            added: 1,
            deleted: 0,
            binary: false,
            untracked,
        }
    }

    /// Writes `content` to `file` under `dir`, creating parent directories.
    fn write(dir: &Path, file: &str, content: impl AsRef<[u8]>) {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// Runs git in `dir` with a throw-away identity.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git is installed");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Opens the changes view as the window root and waits for its status.
    fn open_changes(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, Entity<ChangesView>) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        let git = crate::test_support::git();
        cx.update(ui::init).expect("ui::init");
        cx.update(|cx| bind_keys(cx, &crate::settings::Settings::default()));
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
                    cx.new(|cx| {
                        ChangesView::new(repo.path().to_path_buf(), Ok(git.clone()), window, cx)
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let view = cx
            .update(|window, _| window.root::<ChangesView>().flatten())
            .expect("the changes view is the window root");
        (cx, view)
    }

    /// While a reload runs the last loaded status stays readable, so counts and
    /// the merge banner do not flash to zero ("all resolved").
    #[gpui::test]
    fn counts_survive_a_reload(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-reload");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "two\n");

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, _| assert_eq!(view.unstaged_count(), 1));
        view.update(&mut cx, |view, cx| view.start_status(cx));
        view.read_with(&cx, |view, _| {
            assert!(view.is_loading());
            assert_eq!(view.unstaged_count(), 1);
            assert_eq!(view.dirty_count(), 1);
            // Rendered lists and selection stay empty while loading.
            assert!(view.status().is_none());
        });
        cx.update(|window, _| window.refresh());
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_none(),
            "the file list showed rows while loading"
        );
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert!(!view.is_loading()));

        // A failed reload keeps the last snapshot too.
        view.update(&mut cx, |view, cx| {
            view.set_status(view.generation, None, Err("boom".into()), cx)
        });
        view.read_with(&cx, |view, _| {
            assert_eq!(view.failure(), Some("boom"));
            assert_eq!(view.unstaged_count(), 1);
            assert!(view.status().is_none());
        });
        cx.update(|window, _| window.refresh());
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_none(),
            "the file list showed rows after a failed read"
        );
    }

    /// A quiet refresh never shows the `Loading` state, and picks up a change
    /// made outside the app.
    #[gpui::test]
    fn quiet_refresh_keeps_the_list_and_applies_changes(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-quiet");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "two\n");

        let (mut cx, view) = open_changes(&repo, cx);
        view.update(&mut cx, |view, cx| view.refresh_quiet(cx));
        view.read_with(&cx, |view, _| {
            assert!(!view.is_loading());
            assert!(
                view.status().is_some(),
                "the list blanked during a quiet refresh"
            );
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.unstaged_count(), 1));

        git(repo.path(), &["add", "a.txt"]);
        view.update(&mut cx, |view, cx| view.refresh_quiet(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 1);
            assert_eq!(view.unstaged_count(), 0);
        });
    }

    /// An outside edit that keeps a file's line counts must still reload the
    /// open diff on a quiet refresh.
    #[gpui::test]
    fn quiet_refresh_reloads_the_diff_when_the_status_is_equal(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-quiet-diff");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "two\n");

        let (mut cx, view) = open_changes(&repo, cx);
        let added = |view: &ChangesView| {
            let data = view.diff_data().expect("the diff loaded");
            data.diff.hunks[0]
                .lines
                .iter()
                .filter(|line| line.kind == git_core::LineKind::Added)
                .map(|line| line.text.clone())
                .collect::<Vec<_>>()
        };
        view.read_with(&cx, |view, _| assert_eq!(added(view), ["two"]));

        // Still one added and one removed line: the status is equal.
        write(repo.path(), "a.txt", "three\n");
        view.update(&mut cx, |view, cx| view.refresh_quiet(cx));
        view.read_with(&cx, |view, _| {
            assert!(
                view.diff_data().is_some(),
                "the diff blanked during a quiet reload"
            );
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(added(view), ["three"]));
    }

    #[test]
    fn merge_info_comparison_ignores_a_message_that_was_not_read() {
        let info = |branch: Option<&str>, message: Option<&str>| {
            Some(MergeInfo {
                branch: branch.map(str::to_string),
                message: message.map(str::to_string),
            })
        };
        assert!(merge_unchanged(&None, &None));
        assert!(!merge_unchanged(&None, &info(Some("x"), None)));
        assert!(!merge_unchanged(&info(Some("x"), None), &None));
        // Later reads of a merge carry no message.
        assert!(merge_unchanged(
            &info(Some("x"), Some("m")),
            &info(Some("x"), None)
        ));
        assert!(!merge_unchanged(
            &info(Some("x"), Some("m")),
            &info(Some("y"), None)
        ));
        assert!(!merge_unchanged(
            &info(Some("x"), Some("m")),
            &info(Some("x"), Some("n"))
        ));
    }

    /// Space on a focused file row stages the selected file; Ctrl+Shift+Z asks
    /// to discard it through the inline confirm bar and discards nothing.
    #[gpui::test]
    fn space_stages_and_ctrl_shift_z_asks_to_discard(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("changes-keys");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "two\n");

        let (mut cx, view) = open_changes(&repo, cx);
        let row = cx
            .debug_bounds("changes-unstaged-0")
            .expect("the unstaged row was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();

        cx.simulate_keystrokes("secondary-shift-z");
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.confirming_discard,
                Some(Selection {
                    side: Side::Unstaged,
                    index: 0
                })
            );
            assert_eq!(view.unstaged_count(), 1, "the key discarded without asking");
        });
        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "two\n"
        );

        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 1, "Space did not stage the file");
            assert_eq!(view.unstaged_count(), 0);
        });
    }

    #[gpui::test]
    fn loads_the_status_selects_the_first_file_and_stages(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-stage");
        write(repo.path(), "a.txt", "one\ntwo\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "one\nTWO\nthree\n");
        write(repo.path(), "new.txt", "new\n");

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, _| {
            assert_eq!(view.unstaged_count(), 2, "{:?}", view.state);
            assert_eq!(view.staged_count(), 0);
            assert_eq!(
                view.selection.map(|selection| selection.side),
                Some(Side::Unstaged)
            );
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the first file's diff did not load");
            };
            assert!(diff.rows.iter().any(|row| row.text == "TWO"));
        });
        assert!(
            cx.debug_bounds("changes-view").is_some(),
            "the view was not drawn"
        );
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_some(),
            "the first unstaged row was not drawn"
        );
        assert!(
            cx.debug_bounds("changes-unstaged-1").is_some(),
            "the untracked row was not drawn"
        );

        view.update(&mut cx, |view, cx| view.stage_file(Side::Unstaged, 0, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged.len(), 1);
            assert_eq!(status.staged[0].path, "a.txt");
            assert_eq!(status.unstaged.len(), 1);
            assert_eq!(status.unstaged[0].path, "new.txt");
        });

        view.update(&mut cx, |view, cx| view.stage_all(Side::Unstaged, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 2);
            assert_eq!(view.unstaged_count(), 0);
        });

        view.update(&mut cx, |view, cx| view.stage_all(Side::Staged, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 0);
            assert_eq!(view.unstaged_count(), 2);
        });
    }

    /// A stopped merge: each conflicted path is listed once in its own group,
    /// the merge message fills the commit box, resolving every path lets the
    /// merge commit through, and the banner is gone afterwards.
    #[gpui::test]
    fn a_stopped_merge_lists_conflicts_once_and_commits_after_resolving(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("changes-merge");
        let dir = repo.path();
        write(dir, "a.txt", "base\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", "main\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", "side\n");
        write(dir, "new.txt", "new\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "side"]);
        git(dir, &["switch", "-q", "main"]);
        let outcome = Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();
        assert!(matches!(outcome, git_core::MergeOutcome::Stopped { .. }));

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, cx| {
            let status = view.status().expect("loaded");
            let conflicted: Vec<_> = status.conflicted.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(conflicted, ["a.txt"]);
            assert!(status.unstaged.is_empty(), "{:?}", status.unstaged);
            let staged: Vec<_> = status.staged.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(staged, ["new.txt"]);
            assert_eq!(
                view.selection,
                Some(Selection {
                    side: Side::Conflicted,
                    index: 0
                })
            );
            assert_eq!(
                view.merge.as_ref().and_then(|m| m.branch.as_deref()),
                Some("side")
            );
            assert!(view
                .message
                .read(cx)
                .text()
                .starts_with("Merge branch 'side'"));
            let DiffState::Conflict(file) = &view.diff else {
                panic!("the conflicted file did not load: {:?}", view.diff);
            };
            assert!(file
                .lines
                .iter()
                .any(|line| line.kind == ConflictKind::Marker));
        });
        assert!(cx.debug_bounds("merge-banner").is_some(), "no banner");
        assert!(cx.debug_bounds("changes-conflicted-0").is_some());

        // A menu opened on a row that is gone by click time must not act on
        // whatever now sits at that index: a stale path does nothing.
        view.update(&mut cx, |view, cx| {
            view.resolve_conflict("gone.txt".into(), Resolve::MarkResolved, cx)
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.conflicted_count(), 1);
            assert_eq!(view.staged_count(), 1, "the stale path staged something");
        });

        // Space must not mark a conflicted path resolved.
        view.update(&mut cx, |view, cx| view.stage_selected(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.conflicted_count(), 1));

        view.update(&mut cx, |view, cx| {
            view.resolve_conflict("a.txt".into(), Resolve::Take(ConflictSide::Theirs), cx)
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.conflicted_count(), 0);
            assert!(view.merge.is_some());
            assert!(can_commit(true, false, view.staged_count(), 0));
        });
        view.update_in(&mut cx, |view, window, cx| view.commit(window, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert!(view.merge.is_none());
            assert_eq!(view.staged_count(), 0);
        });
        assert!(cx.debug_bounds("merge-banner").is_none());
        let repo = Repo::open(crate::test_support::git(), dir).unwrap();
        assert_eq!(repo.operation().unwrap(), Operation::None);
    }

    /// While the conflict editor covers the lists, the hidden lists do not
    /// react: a status refresh loads no diff, the arrow-key selection does not
    /// move; closing the editor catches the diff pane up with the selection.
    #[gpui::test]
    fn the_hidden_lists_stay_still_while_the_conflict_editor_is_open(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = crate::test_support::TempRepo::new("changes-editor-hidden");
        let dir = repo.path();
        write(dir, "a.txt", "1\n2\n3\n");
        write(dir, "z.txt", "z\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", "1\nmain\n3\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", "1\nside\n3\n");
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        let outcome = Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();
        assert!(matches!(outcome, git_core::MergeOutcome::Stopped { .. }));
        // An unstaged change gives the lists a second row to move to.
        write(dir, "z.txt", "z2\n");

        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let (selection, generation) =
            view.read_with(&cx, |view, _| (view.selection, view.diff_generation));
        assert_eq!(selection.map(|s| s.side), Some(Side::Conflicted));

        view.update(&mut cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.step_selection(1, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert!(view.editor.is_some());
            assert_eq!(view.diff_generation, generation, "no diff while hidden");
            assert_eq!(view.selection, selection, "the selection did not move");
        });

        // Closing the editor loads the diff of the (unchanged) selection.
        let editor = view.read_with(&cx, |view, _| view.editor.as_ref().unwrap().0.clone());
        editor.update(&mut cx, |editor, cx| editor.cancel(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert!(view.editor.is_none());
            assert_eq!(view.selection, selection);
            assert_ne!(view.diff_generation, generation, "the diff pane caught up");
        });
    }

    /// A merge of `a.txt` (`one` on main, `two` on side) that stops with a conflict.
    fn conflicted_a_txt(name: &str) -> crate::test_support::TempRepo {
        let repo = crate::test_support::TempRepo::new(name);
        let dir = repo.path();
        git(dir, &["config", "core.autocrlf", "false"]);
        write(dir, "a.txt", "1\nbase\n3\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", "1\none\n3\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", "1\ntwo\n3\n");
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        let outcome = Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();
        assert!(matches!(outcome, git_core::MergeOutcome::Stopped { .. }));
        repo
    }

    /// Work done outside the editor is not overwritten silently: a file that
    /// changed after the merge warns at load and asks before saving; one that
    /// changed while the editor was open is not written and is reloaded; one
    /// whose merge was aborted is not written either.
    #[gpui::test]
    fn the_conflict_editor_protects_work_done_elsewhere(cx: &mut gpui::TestAppContext) {
        let repo = conflicted_a_txt("changes-editor-outside");
        let dir = repo.path();
        let ours = git_core::ConflictResolution::Picked(vec![ConflictSide::Ours]);

        // rerere / an IDE changed the file after the merge, before the editor.
        write(dir, "a.txt", "1\nresolved by rerere\n3\n");
        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let editor = view.read_with(&cx, |view, _| view.editor.as_ref().unwrap().0.clone());
        editor.read_with(&cx, |editor, _| assert!(editor.changed_after_merge()));
        cx.simulate_keystrokes("alt-1");
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.resolution(0), Some(&ours))
        });
        // Save asks first and writes nothing yet.
        cx.simulate_keystrokes("secondary-s");
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| assert!(editor.confirming()));
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\nresolved by rerere\n3\n"
        );
        editor.update(&mut cx, |editor, cx| editor.accept_confirm(cx));
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\none\n3\n"
        );
        view.read_with(&cx, |view, _| assert!(view.editor.is_none()));
    }

    #[gpui::test]
    fn a_file_changed_while_the_editor_is_open_is_not_overwritten(cx: &mut gpui::TestAppContext) {
        let repo = conflicted_a_txt("changes-editor-changed-open");
        let dir = repo.path();
        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let editor = view.read_with(&cx, |view, _| view.editor.as_ref().unwrap().0.clone());
        editor.read_with(&cx, |editor, _| assert!(!editor.changed_after_merge()));
        cx.simulate_keystrokes("alt-1");
        write(dir, "a.txt", "1\nedited meanwhile\n3\n");
        cx.simulate_keystrokes("secondary-s");
        cx.run_until_parked();
        // Nothing was written; the editor is open again on the file as it is.
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\nedited meanwhile\n3\n"
        );
        view.read_with(&cx, |view, _| {
            assert!(view.editor.is_some());
            assert_eq!(view.conflicted_count(), 1);
        });
        editor.read_with(&cx, |editor, _| {
            assert!(editor.changed_after_merge(), "reloaded: now it warns");
            assert!(!editor.can_save(), "the picks were dropped by the reload");
        });
    }

    #[gpui::test]
    fn saving_after_the_merge_was_aborted_writes_nothing(cx: &mut gpui::TestAppContext) {
        let repo = conflicted_a_txt("changes-editor-aborted");
        let dir = repo.path();
        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let editor = view.read_with(&cx, |view, _| view.editor.as_ref().unwrap().0.clone());
        cx.simulate_keystrokes("alt-2");
        git(dir, &["merge", "--abort"]);
        let after_abort = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        cx.simulate_keystrokes("secondary-s");
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            after_abort
        );
        // No merge left: the status read closes the editor.
        view.read_with(&cx, |view, _| assert!(view.editor.is_none()));
        drop(editor);
    }

    /// The conflict editor end to end on a real merge: two conflicts in
    /// `a.txt`, one in `b.txt`. Picks rebuild the file, save writes and stages
    /// it and moves to the next conflicted file, Esc with picks asks first, and
    /// resolving the last file hands the view back.
    #[gpui::test]
    fn the_conflict_editor_resolves_files_in_turn(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-editor");
        let dir = repo.path();
        let base = "a1\none\na2\na3\na4\na5\na6\ntwo\na7\n";
        write(dir, "a.txt", base);
        write(dir, "b.txt", "x\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        let edit = |one: &str, two: &str| base.replace("one", one).replace("two", two);
        write(dir, "a.txt", edit("one-main", "two-main"));
        write(dir, "b.txt", "x-main\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", edit("one-side", "two-side"));
        write(dir, "b.txt", "x-side\n");
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        let outcome = Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();
        assert!(matches!(outcome, git_core::MergeOutcome::Stopped { .. }));

        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let editor = view.read_with(&cx, |view, _| {
            view.editor.as_ref().expect("the editor opened").0.clone()
        });
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.path(), "a.txt");
            assert_eq!(editor.counts(), Some((2, 2)));
            assert!(!editor.can_save(), "save must wait for every conflict");
        });

        // Save is a no-op while a conflict is open.
        editor.update(&mut cx, |editor, cx| editor.save(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.conflicted_count(), 2));

        // Alt+1 toggles ours on the focused (first) conflict.
        cx.simulate_keystrokes("alt-1");
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.focused(), Some(0));
            assert_eq!(
                editor.resolution(0),
                Some(&git_core::ConflictResolution::Picked(vec![
                    ConflictSide::Ours
                ]))
            );
        });
        // Second conflict: theirs ticked first, then ours.
        editor.update(&mut cx, |editor, cx| {
            editor.toggle_pick(1, ConflictSide::Theirs, cx);
            editor.toggle_pick(1, ConflictSide::Ours, cx);
        });
        editor.read_with(&cx, |editor, _| assert!(editor.can_save()));
        cx.simulate_keystrokes("secondary-s");
        cx.run_until_parked();

        let saved = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        assert_eq!(
            saved,
            "a1\none-main\na2\na3\na4\na5\na6\ntwo-side\ntwo-main\na7\n"
        );
        view.read_with(&cx, |view, _| {
            let status = view.status().expect("loaded");
            let conflicted: Vec<_> = status.conflicted.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(conflicted, ["b.txt"]);
            assert!(status.staged.iter().any(|f| f.path == "a.txt"));
        });
        // The editor moved on to the next conflicted file by itself.
        let editor = view.read_with(&cx, |view, _| {
            view.editor.as_ref().expect("still open").0.clone()
        });
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.path(), "b.txt");
            assert_eq!(editor.counts(), Some((1, 1)));
        });

        // Esc without picks would leave at once; with a pick it asks first.
        editor.update(&mut cx, |editor, cx| {
            editor.toggle_pick(0, ConflictSide::Theirs, cx)
        });
        cx.simulate_keystrokes("escape");
        editor.read_with(&cx, |editor, _| assert!(editor.confirming()));
        view.read_with(&cx, |view, _| assert!(view.editor.is_some()));
        // A second Esc is "Keep editing".
        cx.simulate_keystrokes("escape");
        editor.read_with(&cx, |editor, _| assert!(!editor.confirming()));

        editor.update(&mut cx, |editor, cx| editor.save(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert!(view.editor.is_none(), "the last file closes the editor");
            assert_eq!(view.conflicted_count(), 0);
        });
        assert_eq!(
            std::fs::read_to_string(dir.join("b.txt")).unwrap(),
            "x-side\n"
        );
    }

    /// Hand-editing the result: F2 opens the text area (untouched, the region
    /// stays a pick), typing makes it a hand edit that locks the picks, Esc
    /// stops editing without leaving the editor, Reset restores the picks,
    /// take-all asks before replacing an edit, and save writes the edit with
    /// the file's own line endings.
    #[gpui::test]
    fn the_conflict_editor_edits_the_result_by_hand(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-editor-custom");
        let dir = repo.path();
        git(dir, &["config", "core.autocrlf", "false"]);
        let file = |middle: &str| format!("a\r\n{middle}\r\nb\r\n");
        write(dir, "a.txt", file("one"));
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", file("one-main"));
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", file("one-side"));
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        let outcome = Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();
        assert!(matches!(outcome, git_core::MergeOutcome::Stopped { .. }));

        let (mut cx, view) = open_changes(&repo, cx);
        view.update_in(&mut cx, |view, window, cx| {
            view.open_conflict_editor("a.txt".into(), window, cx)
        });
        cx.run_until_parked();
        let editor = view.read_with(&cx, |view, _| {
            view.editor.as_ref().expect("the editor opened").0.clone()
        });
        let ours = git_core::ConflictResolution::Picked(vec![ConflictSide::Ours]);
        let custom = |text: &str| git_core::ConflictResolution::Custom(text.to_owned());

        // Alt+1, then F2: the text area opens on the picked lines; opening it
        // does not make the region a hand edit.
        cx.simulate_keystrokes("alt-1");
        cx.simulate_keystrokes("f2");
        cx.run_until_parked();
        editor.read_with(&cx, |editor, cx| {
            assert_eq!(editor.editing(), Some(0));
            assert_eq!(editor.resolution(0), Some(&ours));
            assert_eq!(editor.edit_area().unwrap().read(cx).text(), "one-main");
        });

        // The editor's Alt keys are not for the text area: pressing one while
        // typing neither toggles a pick nor leaves the text area.
        cx.simulate_keystrokes("alt-2 alt-down alt-up");
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.editing(), Some(0));
            assert_eq!(editor.resolution(0), Some(&ours));
        });

        // Typing makes the region a hand edit: resolved, picks locked.
        let area = editor.read_with(&cx, |editor, _| editor.edit_area().unwrap());
        area.update(&mut cx, |area, cx| area.set_text("first\nsecond", cx));
        cx.run_until_parked();
        editor.update(&mut cx, |editor, cx| {
            editor.toggle_pick(0, ConflictSide::Theirs, cx)
        });
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.resolution(0), Some(&custom("first\nsecond")));
            assert!(editor.can_save());
        });

        // Esc in the text area only stops editing: the editor stays open and
        // nothing asks for a confirmation.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.editing(), None);
            assert!(!editor.confirming());
            assert_eq!(editor.resolution(0), Some(&custom("first\nsecond")));
        });
        view.read_with(&cx, |view, _| assert!(view.editor.is_some()));

        // Reset to picks brings the tick back.
        editor.update(&mut cx, |editor, cx| editor.reset_picks(0, cx));
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.resolution(0), Some(&ours));
        });

        // Edit again; "take theirs for all" must ask before replacing it.
        cx.simulate_keystrokes("f2");
        cx.run_until_parked();
        let area = editor.read_with(&cx, |editor, _| editor.edit_area().unwrap());
        area.update(&mut cx, |area, cx| area.set_text("hand edit", cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        editor.update(&mut cx, |editor, cx| {
            editor.request_take_all(ConflictSide::Theirs, cx)
        });
        editor.read_with(&cx, |editor, _| {
            assert!(editor.confirming());
            assert_eq!(editor.resolution(0), Some(&custom("hand edit")));
        });
        // Esc is "Keep editing".
        cx.simulate_keystrokes("escape");
        editor.read_with(&cx, |editor, _| {
            assert!(!editor.confirming());
            assert_eq!(editor.resolution(0), Some(&custom("hand edit")));
        });

        // Ctrl+S saves the edit with the file's CRLF endings.
        cx.simulate_keystrokes("secondary-s");
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "a\r\nhand edit\r\nb\r\n"
        );
        view.read_with(&cx, |view, _| {
            assert!(view.editor.is_none(), "the last file closes the editor");
            assert_eq!(view.conflicted_count(), 0);
        });
    }

    /// A conflicted squash has no `MERGE_HEAD`: it still gets the banner, the
    /// squash message prefill and an abort that works.
    #[gpui::test]
    fn a_conflicted_squash_shows_the_banner_and_aborts(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-squash");
        let dir = repo.path();
        write(dir, "a.txt", "base\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", "main\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", "side\n");
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::Squash)
            .unwrap();

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.operation(), Operation::Squash);
            assert!(view.merge.is_some() && view.squashing());
            assert!(view
                .message
                .read(cx)
                .text()
                .starts_with("Squashed commit of the following"));
            // A squash commits like a plain commit, not like a merge.
            assert!(!view.merging());
        });
        assert!(cx.debug_bounds("merge-banner").is_some(), "no banner");
        view.update(&mut cx, |view, cx| view.abort_merge(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert!(view.merge.is_none());
            assert_eq!(view.operation(), Operation::None);
            assert_eq!(view.message.read(cx).text(), "");
        });
        assert!(!dir.join(".git/SQUASH_MSG").exists());
    }

    /// Aborting a merge removes the untouched prefilled message again.
    #[gpui::test]
    fn aborting_a_merge_clears_the_prefilled_message(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-abort");
        let dir = repo.path();
        write(dir, "a.txt", "base\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        git(dir, &["branch", "side"]);
        write(dir, "a.txt", "main\n");
        git(dir, &["commit", "-q", "-am", "main"]);
        git(dir, &["switch", "-q", "side"]);
        write(dir, "a.txt", "side\n");
        git(dir, &["commit", "-q", "-am", "side"]);
        git(dir, &["switch", "-q", "main"]);
        Repo::open(crate::test_support::git(), dir)
            .unwrap()
            .merge("side", git_core::MergeType::FastForwardIfPossible)
            .unwrap();

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, cx| {
            assert!(view.message.read(cx).text().starts_with("Merge branch"));
        });
        view.update(&mut cx, |view, cx| view.abort_merge(cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert!(view.merge.is_none());
            assert_eq!(view.message.read(cx).text().to_string(), "");
        });
    }

    /// Picking a line in the diff stages only that line of the hunk.
    #[gpui::test]
    fn stages_a_picked_line(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-lines");
        let mut original = String::new();
        for line in 1..=20 {
            original.push_str(&format!("line {line}\n"));
        }
        write(repo.path(), "a.txt", &original);
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        let changed = original
            .replace("line 2\n", "line two\n")
            .replace("line 18\n", "line eighteen\n");
        write(repo.path(), "a.txt", &changed);

        let (mut cx, view) = open_changes(&repo, cx);
        let target = view.read_with(&cx, |view, _| {
            let data = view.diff_data().expect("the diff loaded");
            let row = data
                .rows
                .iter()
                .find(|row| row.text == "line eighteen")
                .expect("the second hunk's added line");
            (row.hunk.unwrap(), row.line.unwrap())
        });
        let picked_count = view.update(&mut cx, |view, cx| {
            view.toggle_pick(target.0, target.1, false, cx);
            assert_eq!(view.picked.len(), 1);
            assert_eq!(view.picked_in(target.0), 1);
            view.picks(target.0)
        });
        assert_eq!(picked_count.len(), 1);
        assert!(picked_count[0].lines.is_some());
        view.update(&mut cx, |view, cx| view.stage_hunk(target.0, cx));
        cx.run_until_parked();

        view.read_with(&cx, |view, _| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged.len(), 1);
            assert_eq!(status.unstaged.len(), 1);
        });
        let cached = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["diff", "--cached", "-U0"])
            .output()
            .unwrap();
        let cached = String::from_utf8_lossy(&cached.stdout);
        assert!(cached.contains("+line eighteen"), "{cached}");
        assert!(!cached.contains("+line two"), "{cached}");
    }

    /// Ctrl+Enter in the message editor reaches the view root and commits.
    #[gpui::test]
    fn ctrl_enter_commits_from_the_message_editor(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-ctrl-enter");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);

        let (mut cx, view) = open_changes(&repo, cx);
        write(repo.path(), "a.txt", "two\n");
        view.update(&mut cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.stage_all(Side::Unstaged, cx));
        cx.run_until_parked();

        let handle = cx.update(|window, cx| {
            let handle = view.read(cx).message.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
            handle
        });
        let _ = handle;
        cx.simulate_input("first commit");
        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.head.as_ref().unwrap().summary, "first commit");
            assert_eq!(status.unstaged_count(), 0);
            assert_eq!(view.message.read(cx).text(), "");
        });
    }

    #[gpui::test]
    fn commits_the_index_and_prefills_the_amend_message(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-commit");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);

        let (mut cx, view) = open_changes(&repo, cx);
        write(repo.path(), "a.txt", "two\n");
        view.update(&mut cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.message.update(cx, |message, cx| {
                message.set_text("feat: two\n\nWhy it exists.\n", cx)
            });
            view.stage_all(Side::Unstaged, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.commit(window, cx));
        });
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged_count(), 0);
            assert_eq!(status.unstaged_count(), 0);
            assert_eq!(status.head.as_ref().unwrap().summary, "feat: two");
            assert_eq!(view.message.read(cx).text(), "");
        });
        // The amend hint uses HEAD's full message.
        view.update(&mut cx, |view, cx| view.set_amend(true, cx));
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.message.read(cx).text(), "feat: two\n\nWhy it exists.");
        });
    }
}
