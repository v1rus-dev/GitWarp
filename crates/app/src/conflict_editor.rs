//! The conflict editor: resolves one conflicted file chunk by chunk.
//!
//! The two sides (and optionally the base) sit side by side over one shared
//! vertical scroll; every conflict is a boxed region with a "Use these lines"
//! checkbox per side. The result pane below is rebuilt from the picks; a
//! conflict region of it can be edited by hand (`ConflictResolution::Custom`):
//! the region's rows are then reserved blank and a `TextArea` is laid over them.
//!
//! The first half of the file is the pure model (rows, result lines, pick
//! toggling); the second half is the view.

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, px, relative, uniform_list, AnyElement, App, AppContext, Bounds, Context,
    Div, ElementId, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, ScrollStrategy, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, Window,
};

use git_core::{
    ConflictGuard, ConflictKind, ConflictResolution, ConflictSegment, ConflictSide, Git, Repo,
    SaveOutcome, TextConflict,
};
use ui::actions::{key_binding, Cancel};
use ui::components::menu::{self, MenuEntry, MenuItem};
use ui::components::toast::{self, ToastKind};
use ui::metrics::{FONT_MONO, FS_LG, FS_SM, FS_XS, R_SM};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    Button, ButtonSize, Checkbox, Chip, ChipKind, Icon, IconName, Scrollbar, Skeleton, Switch,
    TextArea, TextAreaEvent, Tooltip,
};

/// Height of every row of both lists (a `uniform_list` needs one height; the
/// design's 22 px code rows would clip the checkbox rows).
const ROW: f32 = 24.;
const GUTTER: f32 = 44.;
const BASE_WIDTH: f32 = 260.;
/// The line height of the result's text area and its border + padding
/// (`ui::TextArea`: 20 px lines, 6 px padding top and bottom, 1 px border).
const EDIT_LINE: f32 = 20.;
const EDIT_CHROME: f32 = 14.;

// ───────────────────────────── pure model ─────────────────────────────

/// The line of each pane shown in a row (an index into that side's lines);
/// `None` is an alignment filler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cells {
    pub ours: Option<usize>,
    pub base: Option<usize>,
    pub theirs: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    Stable,
    /// Only `side` changed the lines: they win without asking.
    Auto(ConflictSide),
    /// Both sides changed the lines the same way.
    Same,
    /// The header row of conflict `index` (no lines).
    ConflictHead {
        index: usize,
    },
    /// A line row of conflict `index`; `last` closes the box.
    Conflict {
        index: usize,
        last: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub kind: RowKind,
    pub cells: Cells,
}

/// One row per aligned line of the side panes. Without `show_base` the base
/// range does not take part in the alignment.
pub(crate) fn rows(conflict: &TextConflict, show_base: bool) -> Vec<Row> {
    let mut out = Vec::new();
    let mut conflict_index = 0;
    for segment in &conflict.segments {
        match segment {
            ConflictSegment::Stable { base, ours, theirs } => {
                aligned(&mut out, base, ours, theirs, show_base, |_| RowKind::Stable)
            }
            ConflictSegment::Auto {
                side,
                base,
                ours,
                theirs,
            } => aligned(&mut out, base, ours, theirs, show_base, |_| {
                RowKind::Auto(*side)
            }),
            ConflictSegment::Same { base, ours, theirs } => {
                aligned(&mut out, base, ours, theirs, show_base, |_| RowKind::Same)
            }
            ConflictSegment::Conflict { base, ours, theirs } => {
                let index = conflict_index;
                conflict_index += 1;
                out.push(Row {
                    kind: RowKind::ConflictHead { index },
                    cells: Cells::default(),
                });
                let before = out.len();
                aligned(&mut out, base, ours, theirs, show_base, |last| {
                    RowKind::Conflict { index, last }
                });
                if out.len() == before {
                    // A box needs a body row to close on.
                    out.push(Row {
                        kind: RowKind::Conflict { index, last: true },
                        cells: Cells::default(),
                    });
                }
            }
        }
    }
    out
}

/// Appends the rows of one segment: line `i` of every pane side by side, the
/// shorter panes padded with fillers. `kind` gets whether the row is the last.
fn aligned(
    out: &mut Vec<Row>,
    base: &Range<usize>,
    ours: &Range<usize>,
    theirs: &Range<usize>,
    show_base: bool,
    kind: impl Fn(bool) -> RowKind,
) {
    let count = ours
        .len()
        .max(theirs.len())
        .max(if show_base { base.len() } else { 0 });
    for offset in 0..count {
        let pick = |range: &Range<usize>| (offset < range.len()).then(|| range.start + offset);
        out.push(Row {
            kind: kind(offset + 1 == count),
            cells: Cells {
                ours: pick(ours),
                base: if show_base { pick(base) } else { None },
                theirs: pick(theirs),
            },
        });
    }
}

/// The row index of each conflict's header row.
pub(crate) fn head_rows(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(ix, row)| matches!(row.kind, RowKind::ConflictHead { .. }).then_some(ix))
        .collect()
}

/// Where the text of a result line comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Ours(usize),
    Theirs(usize),
    /// One line of a hand-edited region.
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResultLine {
    /// A line of the file; `region` is the conflict it belongs to and `last`
    /// closes that conflict's box.
    Text {
        no: usize,
        source: Source,
        region: Option<usize>,
        last: bool,
    },
    /// The small label above an automatically resolved region.
    AutoLabel { side: ConflictSide },
    /// The header row of conflict `index`.
    Head { index: usize },
    /// The body of an unresolved conflict.
    Placeholder { index: usize },
    /// The body of a resolved conflict that has no lines.
    Empty { index: usize },
    /// One reserved row of the region being hand-edited: the text area is laid
    /// over these rows. `last` closes the conflict's box.
    EditRow { index: usize, last: bool },
}

impl ResultLine {
    /// The text of a `Text` line.
    #[cfg(test)]
    pub(crate) fn text<'a>(&'a self, conflict: &'a TextConflict) -> Option<&'a str> {
        match self {
            Self::Text { source, .. } => Some(source.text(conflict)),
            _ => None,
        }
    }
}

impl Source {
    pub(crate) fn text<'a>(&'a self, conflict: &'a TextConflict) -> &'a str {
        match self {
            Self::Ours(ix) => &conflict.ours[*ix],
            Self::Theirs(ix) => &conflict.theirs[*ix],
            Self::Custom(text) => text,
        }
    }
}

/// The file as `resolutions` rebuild it, with the conflicts marked: the rows
/// of the result pane. Mirrors `TextConflict::render` (same lines, same order:
/// a conflict's lines come from `TextConflict::region_lines`); an unresolved
/// conflict contributes a placeholder instead of lines.
pub(crate) fn result_lines(
    conflict: &TextConflict,
    resolutions: &[ConflictResolution],
) -> Vec<ResultLine> {
    let mut out = Vec::new();
    let mut no = 0;
    let mut index = 0;
    for segment in &conflict.segments {
        match segment {
            ConflictSegment::Stable { ours, .. } | ConflictSegment::Same { ours, .. } => {
                for ix in ours.clone() {
                    no += 1;
                    out.push(ResultLine::Text {
                        no,
                        source: Source::Ours(ix),
                        region: None,
                        last: false,
                    });
                }
            }
            ConflictSegment::Auto {
                side, ours, theirs, ..
            } => {
                let range = match side {
                    ConflictSide::Ours => ours,
                    ConflictSide::Theirs => theirs,
                };
                if !range.is_empty() {
                    out.push(ResultLine::AutoLabel { side: *side });
                }
                for ix in range.clone() {
                    no += 1;
                    out.push(ResultLine::Text {
                        no,
                        source: match side {
                            ConflictSide::Ours => Source::Ours(ix),
                            ConflictSide::Theirs => Source::Theirs(ix),
                        },
                        region: None,
                        last: false,
                    });
                }
            }
            ConflictSegment::Conflict { .. } => {
                out.push(ResultLine::Head { index });
                let resolution = resolutions
                    .get(index)
                    .unwrap_or(&ConflictResolution::Unresolved);
                let (body, lines) = region_body(conflict, index, resolution, no, None);
                out.extend(body);
                no += lines;
                index += 1;
            }
        }
    }
    out
}

/// The result rows under the header of conflict `index` for `resolution`, and
/// how many file lines they are. `first_no` is the number of file lines
/// before the region; `edit_rows` reserves that many rows for the text area
/// instead of the region's lines.
fn region_body(
    conflict: &TextConflict,
    index: usize,
    resolution: &ConflictResolution,
    first_no: usize,
    edit_rows: Option<usize>,
) -> (Vec<ResultLine>, usize) {
    let lines = if is_unresolved(resolution) {
        Vec::new()
    } else {
        conflict.region_lines(index, resolution)
    };
    let count = lines.len();
    if let Some(rows) = edit_rows {
        let body = (0..rows)
            .map(|row| ResultLine::EditRow {
                index,
                last: row + 1 == rows,
            })
            .collect();
        return (body, count);
    }
    let mut body: Vec<ResultLine> = Vec::with_capacity(count.max(1));
    if is_unresolved(resolution) {
        body.push(ResultLine::Placeholder { index });
    } else if lines.is_empty() {
        body.push(ResultLine::Empty { index });
    } else {
        for (offset, (text, _)) in lines.into_iter().enumerate() {
            body.push(ResultLine::Text {
                no: first_no + offset + 1,
                source: Source::Custom(text),
                region: Some(index),
                last: offset + 1 == count,
            });
        }
    }
    (body, count)
}

/// The text a text area starts from for a region made of `lines`. A final
/// empty line gets one more `\n`, because a single final newline would be read
/// back as a terminator and drop that line.
fn lines_text(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    if lines.last().is_some_and(|line| line.is_empty()) {
        text.push('\n');
    }
    text
}

/// The text area height of a region of `text`, in result rows (it is laid over
/// that many reserved rows): 3 to 12 lines of the area, then it scrolls.
pub(crate) fn edit_rows(text: &str) -> usize {
    let lines = text.split('\n').count().clamp(3, 12);
    let height = lines as f32 * EDIT_LINE + EDIT_CHROME;
    (height / ROW).ceil() as usize
}

/// Replaces the body of conflict `index` (its text, placeholder or empty-note
/// rows) by `rows` reserved edit rows.
pub(crate) fn with_edit_rows(lines: Vec<ResultLine>, index: usize, rows: usize) -> Vec<ResultLine> {
    let in_body = |line: &ResultLine| match line {
        ResultLine::Text { region, .. } => *region == Some(index),
        ResultLine::Placeholder { index: at } | ResultLine::Empty { index: at } => *at == index,
        _ => false,
    };
    let mut out = Vec::with_capacity(lines.len() + rows);
    let mut inserted = false;
    for line in lines {
        if !in_body(&line) {
            out.push(line);
        } else if !inserted {
            inserted = true;
            out.extend((0..rows).map(|row| ResultLine::EditRow {
                index,
                last: row + 1 == rows,
            }));
        }
    }
    out
}

/// The result row index of each conflict's header row.
pub(crate) fn result_heads(lines: &[ResultLine]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(ix, line)| matches!(line, ResultLine::Head { .. }).then_some(ix))
        .collect()
}

/// Whether `resolution` still needs a decision.
pub(crate) fn is_unresolved(resolution: &ConflictResolution) -> bool {
    match resolution {
        ConflictResolution::Unresolved => true,
        ConflictResolution::Picked(sides) => sides.is_empty(),
        ConflictResolution::Custom(_) => false,
    }
}

pub(crate) fn unresolved_count(resolutions: &[ConflictResolution]) -> usize {
    resolutions.iter().filter(|r| is_unresolved(r)).count()
}

/// Save is live when no conflict is left open.
pub(crate) fn can_save(resolutions: &[ConflictResolution]) -> bool {
    unresolved_count(resolutions) == 0
}

/// Whether the user changed anything (leaving then asks to confirm).
pub(crate) fn is_dirty(resolutions: &[ConflictResolution]) -> bool {
    resolutions
        .iter()
        .any(|r| *r != ConflictResolution::Unresolved)
}

/// Ticks or unticks `side`. Ticking appends to the pick order; unticking the
/// last side makes the conflict unresolved again. A hand edit is replaced by
/// the tick (the view never offers one: ticks are disabled while edited).
pub(crate) fn toggle_pick(current: &ConflictResolution, side: ConflictSide) -> ConflictResolution {
    let mut sides = match current {
        ConflictResolution::Picked(sides) => sides.clone(),
        _ => Vec::new(),
    };
    if let Some(at) = sides.iter().position(|s| *s == side) {
        sides.remove(at);
    } else {
        sides.push(side);
    }
    if sides.is_empty() {
        ConflictResolution::Unresolved
    } else {
        ConflictResolution::Picked(sides)
    }
}

/// Reverses the order of a two-side pick; anything else stays.
pub(crate) fn swap_order(current: &ConflictResolution) -> ConflictResolution {
    match current {
        ConflictResolution::Picked(sides) if sides.len() == 2 => {
            ConflictResolution::Picked(vec![sides[1], sides[0]])
        }
        other => other.clone(),
    }
}

/// Whether any conflict is hand-edited.
pub(crate) fn has_custom(resolutions: &[ConflictResolution]) -> bool {
    resolutions
        .iter()
        .any(|r| matches!(r, ConflictResolution::Custom(_)))
}

/// Every conflict set to the single side `side`.
pub(crate) fn take_all(count: usize, side: ConflictSide) -> Vec<ConflictResolution> {
    vec![ConflictResolution::Picked(vec![side]); count]
}

/// The 0-based tick position of `side` in the pick when both sides are
/// ticked (the badge shows it); `None` when the order does not matter.
pub(crate) fn pick_order(current: &ConflictResolution, side: ConflictSide) -> Option<usize> {
    match current {
        ConflictResolution::Picked(sides) if sides.len() == 2 => {
            sides.iter().position(|s| *s == side)
        }
        _ => None,
    }
}

pub(crate) fn is_picked(current: &ConflictResolution, side: ConflictSide) -> bool {
    matches!(current, ConflictResolution::Picked(sides) if sides.contains(&side))
}

/// The conflict to focus after moving `delta` from `current` (clamped; no
/// wrap-around). `None` when the file has no conflicts.
pub(crate) fn step_focus(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let from = current.map_or(if delta >= 0 { -1 } else { count as isize }, |c| c as isize);
    Some((from + delta).clamp(0, count as isize - 1) as usize)
}

/// The line ranges of each conflict, in order: `(base, ours, theirs)`.
fn conflict_ranges(conflict: &TextConflict) -> Vec<(Range<usize>, Range<usize>, Range<usize>)> {
    conflict
        .segments
        .iter()
        .filter_map(|segment| match segment {
            ConflictSegment::Conflict { base, ours, theirs } => {
                Some((base.clone(), ours.clone(), theirs.clone()))
            }
            _ => None,
        })
        .collect()
}

// ───────────────────────────── view state ─────────────────────────────

/// A conflict that has no lines to merge: only a whole side can be kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whole {
    Binary,
    /// Not UTF-8 text (or a filtered file): not editable line by line.
    NotText,
    Symlink,
    Submodule,
    /// We deleted the file, they changed it.
    DeletedByUs,
    /// They deleted the file, we changed it.
    DeletedByThem,
    /// Only we added the file.
    AddedByUs,
    /// Only they added the file.
    AddedByThem,
    /// Both sides deleted the file.
    DeletedByBoth,
}

impl Whole {
    /// The side that does not have the file (taking it deletes the file);
    /// `None` when both sides have it, or when neither does.
    fn absent_side(self) -> Option<ConflictSide> {
        match self {
            Self::DeletedByUs | Self::AddedByThem => Some(ConflictSide::Ours),
            Self::DeletedByThem | Self::AddedByUs => Some(ConflictSide::Theirs),
            _ => None,
        }
    }
}

/// Where a save stands. A finished job leaves the editor waiting for the
/// status read that follows it, so the file cannot be saved twice; only that
/// read (successful or not) ends the wait. A status read that has nothing to
/// do with the save must not end a save that is still running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SaveState {
    #[default]
    Idle,
    /// The job is running.
    Running,
    /// The job succeeded; the status read is pending.
    SavedWaiting,
}

impl SaveState {
    fn is_busy(self) -> bool {
        self != Self::Idle
    }

    /// A status read finished (`Ok` or `Err`).
    fn status_arrived(self) -> Self {
        match self {
            Self::SavedWaiting => Self::Idle,
            other => other,
        }
    }
}

struct TextState {
    conflict: Arc<TextConflict>,
    ranges: Vec<(Range<usize>, Range<usize>, Range<usize>)>,
    /// What `render` gets: the picks of each conflict, or `Custom` after a
    /// hand edit.
    resolutions: Vec<ConflictResolution>,
    /// The picks of each conflict (never `Custom`), kept under a hand edit so
    /// "Reset to picks" can restore them.
    picks: Vec<ConflictResolution>,
    rows: Vec<Row>,
    head_rows: Vec<usize>,
    result: Vec<ResultLine>,
    result_heads: Vec<usize>,
    /// The number of file lines each conflict's region has now, so a keystroke
    /// can renumber the lines after it without rebuilding the whole result.
    region_counts: Vec<usize>,
    focused: Option<usize>,
    /// The conflict whose result region shows the text area, and the number
    /// of result rows reserved for it.
    editing: Option<(usize, usize)>,
}

impl TextState {
    fn new(conflict: TextConflict, show_base: bool) -> Self {
        let resolutions = conflict.unresolved();
        let focused = (!resolutions.is_empty()).then_some(0);
        let mut state = Self {
            ranges: conflict_ranges(&conflict),
            conflict: Arc::new(conflict),
            picks: resolutions.clone(),
            resolutions,
            rows: Vec::new(),
            head_rows: Vec::new(),
            result: Vec::new(),
            result_heads: Vec::new(),
            region_counts: Vec::new(),
            focused,
            editing: None,
        };
        state.rebuild_rows(show_base);
        state.rebuild_result();
        state
    }

    fn rebuild_rows(&mut self, show_base: bool) {
        self.rows = rows(&self.conflict, show_base);
        self.head_rows = head_rows(&self.rows);
    }

    fn rebuild_result(&mut self) {
        let lines = result_lines(&self.conflict, &self.resolutions);
        self.region_counts = vec![0; self.resolutions.len()];
        for line in &lines {
            if let ResultLine::Text {
                region: Some(index),
                ..
            } = line
            {
                self.region_counts[*index] += 1;
            }
        }
        self.result = match self.editing {
            Some((index, rows)) => with_edit_rows(lines, index, rows),
            None => lines,
        };
        self.result_heads = result_heads(&self.result);
    }

    /// Replaces every conflict's state (take-all): hand edits go too.
    fn set_all(&mut self, resolutions: Vec<ConflictResolution>) {
        self.picks = resolutions.clone();
        self.resolutions = resolutions;
        self.editing = None;
        self.rebuild_result();
    }

    fn is_custom(&self, index: usize) -> bool {
        matches!(
            self.resolutions.get(index),
            Some(ConflictResolution::Custom(_))
        )
    }

    /// Ticks `side` of conflict `index`. Ignored while it is hand-edited
    /// (the checkboxes are disabled until "Reset to picks").
    fn toggle(&mut self, index: usize, side: ConflictSide) {
        if self.is_custom(index) {
            return;
        }
        if let Some(current) = self.picks.get(index) {
            self.picks[index] = toggle_pick(current, side);
            self.resolutions[index] = self.picks[index].clone();
            self.rebuild_result();
        }
    }

    fn swap(&mut self, index: usize) {
        if self.is_custom(index) {
            return;
        }
        if let Some(current) = self.picks.get(index) {
            self.picks[index] = swap_order(current);
            self.resolutions[index] = self.picks[index].clone();
            self.rebuild_result();
        }
    }

    /// The lines conflict `index` renders from its picks (`None` while it has
    /// none: an edit then has nothing to compare with).
    fn picked_lines(&self, index: usize) -> Option<Vec<String>> {
        let picks = self.picks.get(index)?;
        if is_unresolved(picks) {
            return None;
        }
        Some(
            self.conflict
                .region_lines(index, picks)
                .into_iter()
                .map(|(text, _)| text)
                .collect(),
        )
    }

    /// The text the text area of conflict `index` starts from: the hand edit,
    /// else the lines of the picks (empty while unresolved).
    fn edit_text(&self, index: usize) -> String {
        match self.resolutions.get(index) {
            Some(ConflictResolution::Custom(text)) => text.clone(),
            _ => {
                let lines = self.picked_lines(index).unwrap_or_default();
                lines_text(&lines.iter().map(String::as_str).collect::<Vec<_>>())
            }
        }
    }

    /// The text area of conflict `index` now holds `text`. It becomes a hand
    /// edit when it renders differently from the picks; an unresolved conflict
    /// has no picks to compare with, so any change makes it one. (Entering the
    /// edit without typing never gets here.)
    ///
    /// Only the edited region's rows are rebuilt (see [`Self::update_region`]):
    /// this runs on every keystroke, and the result of a big file is big.
    fn apply_edit(&mut self, index: usize, text: String, rows: usize) {
        if index >= self.resolutions.len() {
            return;
        }
        let same = self.picked_lines(index).is_some_and(|picked| {
            let typed = self
                .conflict
                .region_lines(index, &ConflictResolution::Custom(text.clone()));
            picked.len() == typed.len() && picked.iter().zip(&typed).all(|(a, (b, _))| a == b)
        });
        self.resolutions[index] = if same {
            self.picks[index].clone()
        } else {
            ConflictResolution::Custom(text)
        };
        let already_editing = self.editing.is_some_and(|(at, _)| at == index);
        self.editing = Some((index, rows));
        if already_editing {
            self.update_region(index);
        } else {
            self.rebuild_result();
        }
    }

    /// Rebuilds the rows of conflict `index` only, shifting the line numbers
    /// and header rows after it by what its line and row counts changed by.
    fn update_region(&mut self, index: usize) {
        let Some(&head) = self.result_heads.get(index) else {
            return;
        };
        let in_body = |line: &ResultLine| match line {
            ResultLine::Text { region, .. } => *region == Some(index),
            ResultLine::Placeholder { index: at }
            | ResultLine::Empty { index: at }
            | ResultLine::EditRow { index: at, .. } => *at == index,
            _ => false,
        };
        let old_len = self.result[head + 1..]
            .iter()
            .take_while(|line| in_body(line))
            .count();
        let first_no = self.result[..head]
            .iter()
            .rev()
            .find_map(|line| match line {
                ResultLine::Text { no, .. } => Some(*no),
                _ => None,
            })
            .unwrap_or(0);
        let edit_rows = self.editing.filter(|(at, _)| *at == index).map(|(_, r)| r);
        let (body, lines) = region_body(
            &self.conflict,
            index,
            &self.resolutions[index],
            first_no,
            edit_rows,
        );
        let row_delta = body.len() as isize - old_len as isize;
        let line_delta = lines as isize - self.region_counts[index] as isize;
        self.region_counts[index] = lines;
        let end = head + 1 + body.len();
        self.result.splice(head + 1..head + 1 + old_len, body);
        if line_delta != 0 {
            for line in &mut self.result[end..] {
                if let ResultLine::Text { no, .. } = line {
                    *no = (*no as isize + line_delta) as usize;
                }
            }
        }
        if row_delta != 0 {
            for later in &mut self.result_heads[index + 1..] {
                *later = (*later as isize + row_delta) as usize;
            }
        }
    }

    /// "Reset to picks": the picks the conflict had before the edit.
    fn reset(&mut self, index: usize) {
        if self.is_custom(index) {
            self.resolutions[index] = self.picks[index].clone();
            self.rebuild_result();
        }
    }

    /// Reserves `rows` result rows for the text area of conflict `index`
    /// (`None`: back to the plain rows).
    fn set_editing(&mut self, editing: Option<(usize, usize)>) {
        if self.editing != editing {
            self.editing = editing;
            self.rebuild_result();
        }
    }

    /// The border colour of conflict `index`'s box, and whether it is thick.
    fn box_style(&self, index: usize, colors: ThemeColors) -> (Hsla, bool) {
        if self.focused == Some(index) {
            (colors.focus, true)
        } else if self.resolutions.get(index).is_none_or(is_unresolved) {
            (colors.warning, false)
        } else {
            (colors.border_strong, false)
        }
    }
}

enum Load {
    Loading,
    Failed(String),
    Text(Box<TextState>),
    Whole {
        kind: Whole,
        choice: Option<ConflictSide>,
    },
}

/// Where a confirmed "discard your picks" goes (or, for `TakeAll`, a confirmed
/// "replace your edits too"; for `ReplaceDisk`, a confirmed save).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Leave {
    Close,
    File(String),
    TakeAll(ConflictSide),
    /// "Replace the changes on disk?": the file changed after the merge
    /// wrote it, and saving replaces those changes.
    ReplaceDisk,
}

/// The open text area of a hand-edited result region.
struct EditArea {
    index: usize,
    area: Entity<TextArea>,
    _subscription: Subscription,
}

/// What reaches the repository on save.
enum Job {
    /// The resolved text and the state of the conflict it was made from.
    Content(String, ConflictGuard),
    Take(ConflictSide),
}

/// What the editor tells the Changes view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorEvent {
    /// Back / cancel: return to the file lists.
    Close,
    /// The resolution of the open file was written and staged.
    Saved,
    /// Something changed under the editor: re-read the status.
    Refresh,
}

actions!(
    gitwarp,
    [
        PrevConflict,
        NextConflict,
        PickOurs,
        PickTheirs,
        SaveResolution,
        EditResult
    ]
);

/// Binds the editor's keys. Called once at startup.
pub fn bind_keys(cx: &mut App) {
    let editor = Some("conflict-editor");
    let outside_text_area = Some("conflict-editor && !TextArea");
    cx.bind_keys([
        // Not inside the edit text area: its keys are for the text.
        key_binding(cx, "alt-up", PrevConflict, outside_text_area),
        key_binding(cx, "alt-down", NextConflict, outside_text_area),
        key_binding(cx, "alt-1", PickOurs, outside_text_area),
        key_binding(cx, "alt-2", PickTheirs, outside_text_area),
        key_binding(cx, "secondary-s", SaveResolution, editor),
        key_binding(cx, "f2", EditResult, editor),
        // Enter belongs to the focused button / checkbox / text area when
        // there is one; only the editor itself takes it as "edit the result".
        key_binding(
            cx,
            "enter",
            EditResult,
            Some("conflict-editor && !ui-button && !ui-checkbox && !ui-switch && !TextArea"),
        ),
    ]);
}

pub(crate) struct ConflictEditor {
    git: Arc<Git>,
    root: PathBuf,
    /// The conflicted paths, in list order, and the open one.
    files: Vec<String>,
    path: String,
    ours_label: String,
    theirs_label: String,
    load: Load,
    generation: u64,
    show_base: bool,
    confirm: Option<Leave>,
    /// The result region being edited by hand, if any.
    edit: Option<EditArea>,
    /// A save is running (or its status refresh is pending).
    saving: SaveState,
    /// The user agreed to replace the changes made on disk after the merge.
    replace_confirmed: bool,
    focus: FocusHandle,
    needs_focus: bool,
    top_scroll: UniformListScrollHandle,
    result_scroll: UniformListScrollHandle,
    more_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl EventEmitter<EditorEvent> for ConflictEditor {}

impl ConflictEditor {
    pub(crate) fn new(
        git: Arc<Git>,
        root: PathBuf,
        files: Vec<String>,
        path: String,
        ours_label: String,
        theirs_label: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self {
            git,
            root,
            files,
            path,
            ours_label,
            theirs_label,
            load: Load::Loading,
            generation: 0,
            show_base: false,
            confirm: None,
            edit: None,
            saving: SaveState::Idle,
            replace_confirmed: false,
            focus: cx.focus_handle().tab_stop(true),
            needs_focus: true,
            top_scroll: UniformListScrollHandle::new(),
            result_scroll: UniformListScrollHandle::new(),
            more_bounds: Rc::default(),
        };
        editor.start_load(cx);
        editor
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// `(conflicts, still unresolved)` of a loaded text conflict.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> Option<(usize, usize)> {
        match &self.load {
            Load::Text(state) => Some((
                state.resolutions.len(),
                unresolved_count(&state.resolutions),
            )),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn resolution(&self, index: usize) -> Option<&ConflictResolution> {
        match &self.load {
            Load::Text(state) => state.resolutions.get(index),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn confirming(&self) -> bool {
        self.confirm.is_some()
    }

    /// Whether the open text conflict shows the "changed after the merge"
    /// warning.
    #[cfg(test)]
    pub(crate) fn changed_after_merge(&self) -> bool {
        matches!(&self.load, Load::Text(state) if state.conflict.guard.changed_after_merge)
    }

    /// Answers the pending confirm with its destructive button.
    #[cfg(test)]
    pub(crate) fn accept_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(leave) = self.confirm.clone() {
            self.leave(leave, cx);
        }
    }

    /// The conflict whose result region has the text area open.
    #[cfg(test)]
    pub(crate) fn editing(&self) -> Option<usize> {
        self.edit.as_ref().map(|edit| edit.index)
    }

    /// The text area of the region being edited.
    #[cfg(test)]
    pub(crate) fn edit_area(&self) -> Option<Entity<TextArea>> {
        self.edit.as_ref().map(|edit| edit.area.clone())
    }

    fn side_label(&self, side: ConflictSide) -> &str {
        match side {
            ConflictSide::Ours => &self.ours_label,
            ConflictSide::Theirs => &self.theirs_label,
        }
    }

    /// Loads the conflict of the open file off the main thread.
    fn start_load(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.load = Load::Loading;
        self.replace_confirmed = false;
        self.edit = None;
        self.top_scroll = UniformListScrollHandle::new();
        self.result_scroll = UniformListScrollHandle::new();
        let (git, root, path) = (self.git.clone(), self.root.clone(), self.path.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root).map_err(|error| error.to_string())?;
                    repo.conflict(&path).map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |editor, cx| editor.set_loaded(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_loaded(
        &mut self,
        generation: u64,
        result: Result<ConflictKind, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let whole = |kind| Load::Whole { kind, choice: None };
        self.load = match result {
            Ok(ConflictKind::Text(conflict)) => {
                Load::Text(Box::new(TextState::new(conflict, self.show_base)))
            }
            Ok(ConflictKind::Binary) => whole(Whole::Binary),
            Ok(ConflictKind::NotText) => whole(Whole::NotText),
            Ok(ConflictKind::Symlink) => whole(Whole::Symlink),
            Ok(ConflictKind::Submodule) => whole(Whole::Submodule),
            Ok(ConflictKind::DeletedByUs) => whole(Whole::DeletedByUs),
            Ok(ConflictKind::DeletedByThem) => whole(Whole::DeletedByThem),
            Ok(ConflictKind::AddedByUs) => whole(Whole::AddedByUs),
            Ok(ConflictKind::AddedByThem) => whole(Whole::AddedByThem),
            Ok(ConflictKind::DeletedByBoth) => whole(Whole::DeletedByBoth),
            Err(error) => Load::Failed(error),
        };
        cx.notify();
    }

    /// Takes the fresh status of the Changes view: the conflicted paths and
    /// the side labels. When the open file is no longer conflicted (it was
    /// saved, or resolved elsewhere) moves on to the next conflicted file.
    /// Returns `false` when none is left and the editor should close.
    pub(crate) fn sync(
        &mut self,
        files: Vec<String>,
        ours_label: String,
        theirs_label: String,
        cx: &mut Context<Self>,
    ) -> bool {
        self.ours_label = ours_label;
        self.theirs_label = theirs_label;
        self.saving = self.saving.status_arrived();
        if files.is_empty() {
            return false;
        }
        if files.contains(&self.path) {
            self.files = files;
            cx.notify();
            return true;
        }
        // The next file after the old position that is still conflicted,
        // else the first one left.
        let old_at = self.files.iter().position(|f| *f == self.path);
        let next = old_at
            .and_then(|at| {
                self.files[at + 1..]
                    .iter()
                    .find(|file| files.contains(file))
            })
            .unwrap_or(&files[0])
            .clone();
        self.files = files;
        self.open_file(next, cx);
        true
    }

    fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.path = path;
        self.confirm = None;
        self.needs_focus = true;
        self.start_load(cx);
    }

    // ── state queries ──

    fn dirty(&self) -> bool {
        match &self.load {
            Load::Text(state) => is_dirty(&state.resolutions),
            Load::Whole { choice, .. } => choice.is_some(),
            _ => false,
        }
    }

    pub(crate) fn can_save(&self) -> bool {
        !self.saving.is_busy()
            && match &self.load {
                Load::Text(state) => can_save(&state.resolutions),
                Load::Whole { choice, .. } => choice.is_some(),
                _ => false,
            }
    }

    fn file_position(&self) -> Option<usize> {
        self.files.iter().position(|file| *file == self.path)
    }

    // ── actions ──

    pub(crate) fn toggle_pick(&mut self, index: usize, side: ConflictSide, cx: &mut Context<Self>) {
        if let Load::Text(state) = &mut self.load {
            state.focused = Some(index);
            // A hand edit keeps its text until "Reset to picks".
            if !state.is_custom(index) {
                state.toggle(index, side);
                self.close_edit_of(index);
            }
            cx.notify();
        }
    }

    /// Drops the open text area when it belongs to conflict `index` (its
    /// text would no longer match the region).
    fn close_edit_of(&mut self, index: usize) {
        if self.edit.as_ref().is_some_and(|edit| edit.index == index) {
            self.stop_edit();
        }
    }

    fn toggle_focused(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        if let Load::Text(state) = &self.load {
            if let Some(index) = state.focused {
                self.toggle_pick(index, side, cx);
            }
        }
    }

    fn swap_pick(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Load::Text(state) = &mut self.load {
            state.swap(index);
            self.close_edit_of(index);
            cx.notify();
        }
    }

    /// The footer's "Take {branch} for all": asks first when it would replace
    /// hand edits.
    pub(crate) fn request_take_all(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        let custom = matches!(&self.load, Load::Text(state) if has_custom(&state.resolutions));
        if custom {
            self.confirm = Some(Leave::TakeAll(side));
            cx.notify();
        } else {
            self.take_all(side, cx);
        }
    }

    fn take_all(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        if let Load::Text(state) = &mut self.load {
            let count = state.resolutions.len();
            state.set_all(take_all(count, side));
            self.edit = None;
            self.needs_focus = true;
            cx.notify();
        }
    }

    // ── hand edit of the result ──

    /// Opens the text area over the result region of conflict `index`,
    /// prefilled with what the region holds now, and focuses it.
    fn start_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving.is_busy() {
            return;
        }
        if let Some(edit) = &self.edit {
            if edit.index == index {
                let handle = edit.area.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
                return;
            }
        }
        let Load::Text(state) = &mut self.load else {
            return;
        };
        if index >= state.resolutions.len() {
            return;
        }
        let text = state.edit_text(index);
        let rows = edit_rows(&text);
        let placeholder = ui::t!("changes.editor.edit_placeholder");
        let area = cx.new(|cx| {
            let mut area = TextArea::new(window, cx);
            area.set_text(text, cx);
            area.set_placeholder(placeholder, cx);
            area.set_height(px(rows as f32 * ROW), cx);
            area
        });
        let subscription = cx.subscribe(&area, move |editor, area, event: &TextAreaEvent, cx| {
            let TextAreaEvent::Changed(text) = event;
            editor.edit_changed(index, text.clone(), &area, cx);
        });
        state.focused = Some(index);
        state.set_editing(Some((index, rows)));
        if let Some(row) = state.result_heads.get(index) {
            self.result_scroll
                .scroll_to_item_strict(*row, ScrollStrategy::Top);
        }
        let handle = area.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        self.edit = Some(EditArea {
            index,
            area,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// F2 / Enter: edits the focused conflict's result.
    fn edit_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = match &self.load {
            Load::Text(state) => state.focused,
            _ => None,
        };
        if let Some(index) = focused {
            self.start_edit(index, window, cx);
        }
    }

    /// The text area of conflict `index` changed: the region becomes (or stops
    /// being) a hand edit, and its reserved rows follow the text height.
    fn edit_changed(
        &mut self,
        index: usize,
        text: String,
        area: &Entity<TextArea>,
        cx: &mut Context<Self>,
    ) {
        let rows = edit_rows(&text);
        if let Load::Text(state) = &mut self.load {
            state.apply_edit(index, text, rows);
        }
        area.update(cx, |area, cx| area.set_height(px(rows as f32 * ROW), cx));
        cx.notify();
    }

    /// Closes the text area; the region keeps what was typed. The editor
    /// takes the focus back on the next frame.
    fn stop_edit(&mut self) {
        if self.edit.take().is_some() {
            if let Load::Text(state) = &mut self.load {
                state.set_editing(None);
            }
            self.needs_focus = true;
        }
    }

    /// "Reset to picks": the conflict goes back to the picks it had before it
    /// was edited by hand.
    pub(crate) fn reset_picks(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close_edit_of(index);
        if let Load::Text(state) = &mut self.load {
            state.focused = Some(index);
            state.reset(index);
        }
        cx.notify();
    }

    fn choose(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        if let Load::Whole { choice, .. } = &mut self.load {
            *choice = Some(side);
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn focused(&self) -> Option<usize> {
        match &self.load {
            Load::Text(state) => state.focused,
            _ => None,
        }
    }

    fn focus_conflict(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Load::Text(state) = &mut self.load {
            if state.focused != Some(index) {
                state.focused = Some(index);
                cx.notify();
            }
        }
    }

    /// Alt+↑ / Alt+↓: focuses the neighbouring conflict and scrolls it into
    /// view in both panes.
    fn step_conflict(&mut self, delta: isize, cx: &mut Context<Self>) {
        // Moving on leaves the text area (its region keeps what was typed).
        self.stop_edit();
        let Load::Text(state) = &mut self.load else {
            return;
        };
        let Some(index) = step_focus(state.focused, state.resolutions.len(), delta) else {
            return;
        };
        state.focused = Some(index);
        if let Some(row) = state.head_rows.get(index) {
            self.top_scroll
                .scroll_to_item_strict(*row, ScrollStrategy::Top);
        }
        if let Some(row) = state.result_heads.get(index) {
            self.result_scroll
                .scroll_to_item_strict(*row, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn set_show_base(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_base = show;
        if let Load::Text(state) = &mut self.load {
            state.rebuild_rows(show);
        }
        cx.notify();
    }

    /// Leaves for `leave`, asking first when picks would be lost.
    fn request_leave(&mut self, leave: Leave, cx: &mut Context<Self>) {
        if self.saving.is_busy() {
            return;
        }
        if self.dirty() {
            self.confirm = Some(leave);
            cx.notify();
        } else {
            self.leave(leave, cx);
        }
    }

    fn leave(&mut self, leave: Leave, cx: &mut Context<Self>) {
        match leave {
            Leave::Close => cx.emit(EditorEvent::Close),
            Leave::File(path) => self.open_file(path, cx),
            Leave::TakeAll(side) => {
                self.confirm = None;
                self.take_all(side, cx);
            }
            Leave::ReplaceDisk => {
                self.confirm = None;
                self.replace_confirmed = true;
                self.save(cx);
            }
        }
    }

    fn step_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(at) = self.file_position() else {
            return;
        };
        let next = at as isize + delta;
        if let Some(path) = usize::try_from(next).ok().and_then(|ix| self.files.get(ix)) {
            let path = path.clone();
            self.request_leave(Leave::File(path), cx);
        }
    }

    /// Esc / Cancel: stops editing by hand, else dismisses the confirm, else leaves.
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            // Esc in the text area only stops the editing.
            self.stop_edit();
            cx.notify();
        } else if self.confirm.is_some() {
            self.confirm = None;
            cx.notify();
        } else {
            self.request_leave(Leave::Close, cx);
        }
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save() {
            return;
        }
        let job = match &self.load {
            Load::Text(state) => match state.conflict.render(&state.resolutions) {
                Some(content) => {
                    // The file on disk is not what the merge wrote: saving
                    // replaces whatever it holds, so ask once.
                    if state.conflict.guard.changed_after_merge && !self.replace_confirmed {
                        self.confirm = Some(Leave::ReplaceDisk);
                        cx.notify();
                        return;
                    }
                    Job::Content(content, state.conflict.guard.clone())
                }
                None => return,
            },
            Load::Whole {
                choice: Some(side), ..
            } => Job::Take(*side),
            _ => return,
        };
        self.start_job(job, cx);
    }

    /// The whole-file take of a file that failed to load.
    fn take_whole(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        if !self.saving.is_busy() {
            self.start_job(Job::Take(side), cx);
        }
    }

    fn start_job(&mut self, job: Job, cx: &mut Context<Self>) {
        self.saving = SaveState::Running;
        cx.notify();
        let (git, root, path) = (self.git.clone(), self.root.clone(), self.path.clone());
        let notice = ui::t!("changes.editor.resolved", path = path).to_string();
        let changed_on_disk = ui::t!("changes.editor.changed_on_disk", path = path).to_string();
        let not_same = ui::t!("changes.editor.not_same_conflict", path = path).to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root).map_err(|error| error.to_string())?;
                    match job {
                        Job::Content(content, guard) => {
                            repo.save_resolution(&path, &content, &guard)
                        }
                        Job::Take(side) => {
                            repo.resolve_take(&path, side).map(|()| SaveOutcome::Saved)
                        }
                    }
                    .map_err(|error| error.to_string())
                })
                .await;
            this.update_in(cx, |editor, window, cx| {
                editor.finish_job(result, [notice, changed_on_disk, not_same], window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// `notices`: the texts for saved, changed on disk, not the same conflict.
    fn finish_job(
        &mut self,
        result: Result<SaveOutcome, String>,
        notices: [String; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let [saved, changed_on_disk, not_same] = notices;
        match result {
            Ok(SaveOutcome::Saved) => {
                toast::show(ToastKind::Ok, saved, window, cx);
                // The editor waits for the Changes view's status read (it ends
                // in `sync`, or in `status_failed`), so the file cannot be
                // saved twice meanwhile.
                self.saving = SaveState::SavedWaiting;
                cx.emit(EditorEvent::Saved);
            }
            Ok(outcome) => {
                // Nothing was written: show what is there now.
                self.saving = SaveState::Idle;
                let message = match outcome {
                    SaveOutcome::WorkTreeChanged => changed_on_disk,
                    _ => not_same,
                };
                toast::show(ToastKind::Error, message, window, cx);
                self.confirm = None;
                self.replace_confirmed = false;
                self.start_load(cx);
                cx.emit(EditorEvent::Refresh);
            }
            Err(error) => {
                self.saving = SaveState::Idle;
                toast::show(ToastKind::Error, error, window, cx);
                cx.notify();
            }
        }
    }

    /// The status read that follows a save failed: stops waiting for it.
    pub(crate) fn status_failed(&mut self, cx: &mut Context<Self>) {
        let next = self.saving.status_arrived();
        if next != self.saving {
            self.saving = next;
            cx.notify();
        }
    }
}

// ───────────────────────────── view ─────────────────────────────

/// The box drawn around one conflict in every pane.
#[derive(Clone, Copy)]
struct BoxStyle {
    color: Hsla,
    thick: bool,
    top: bool,
    bottom: bool,
}

/// Applies the conflict box (or the plain pane separator) to a cell.
fn boxed<T: Styled>(cell: T, style: Option<BoxStyle>, colors: ThemeColors) -> T {
    let Some(style) = style else {
        return cell.border_r_1().border_color(colors.border);
    };
    let cell = cell.border_color(style.color);
    let cell = if style.thick {
        cell.border_l_2().border_r_2()
    } else {
        cell.border_l_1().border_r_1()
    };
    let cell = match (style.top, style.thick) {
        (true, true) => cell.border_t_2(),
        (true, false) => cell.border_t_1(),
        _ => cell,
    };
    match (style.bottom, style.thick) {
        (true, true) => cell.border_b_2(),
        (true, false) => cell.border_b_1(),
        _ => cell,
    }
}

fn pane_cell() -> Div {
    div().flex_1().min_w_0().h_full().flex().items_center()
}

fn base_cell() -> Div {
    div()
        .w(px(BASE_WIDTH))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
}

/// One code cell: gutter with the line number and the text, or a filler.
fn code_cell(
    shell: Div,
    line: Option<(usize, &str)>,
    tint: Option<Hsla>,
    style: Option<BoxStyle>,
    colors: ThemeColors,
) -> Div {
    let cell = boxed(shell, style, colors).font_family(FONT_MONO);
    match line {
        None => cell
            .child(div().w(px(GUTTER)).flex_none().h_full())
            .child(div().flex_1().h_full().bg(colors.hover)),
        Some((no, text)) => cell
            .child(
                div()
                    .w(px(GUTTER))
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr(px(8.))
                    .when_some(tint, |this, tint| this.bg(tint))
                    .text_color(colors.muted)
                    .child((no + 1).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when_some(tint, |this, tint| this.bg(tint))
                    .text_color(colors.text)
                    .child(SharedString::from(text.to_owned())),
            ),
    }
}

/// A small clickable icon without a button frame (back, file switcher).
fn icon_tool(
    id: impl Into<ElementId>,
    icon: IconName,
    flipped: bool,
    tooltip: SharedString,
    enabled: bool,
    colors: ThemeColors,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let glyph = Icon::new(icon).size(px(12.)).color(if enabled {
        colors.text
    } else {
        colors.disabled
    });
    let glyph = if flipped { glyph.flipped() } else { glyph };
    let hover = colors.hover;
    div()
        .id(id)
        .w(px(24.))
        .h(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(R_SM)
        .child(glyph)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(move |_, window, cx| on_click(window, cx))
        })
        .tooltip(Tooltip::text(tooltip))
}

/// An invisible canvas filling its parent that records the parent's bounds.
fn bounds_probe(slot: Rc<RefCell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |_, bounds, _, _| {
            *slot.borrow_mut() = Some(bounds);
        },
    )
    .absolute()
    .inset_0()
}

impl ConflictEditor {
    fn kind_note(&self, kind: Whole) -> SharedString {
        match kind {
            Whole::Binary => ui::t!("changes.editor.note_binary"),
            Whole::NotText => ui::t!("changes.editor.note_not_text"),
            Whole::Symlink => ui::t!("changes.editor.note_symlink"),
            Whole::Submodule => ui::t!("changes.editor.note_submodule"),
            Whole::DeletedByUs | Whole::DeletedByThem => ui::t!("changes.editor.note_deleted"),
            Whole::AddedByUs | Whole::AddedByThem => ui::t!("changes.editor.note_added"),
            Whole::DeletedByBoth => ui::t!("changes.editor.note_deleted_both"),
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let mut bar = div()
            .h(px(40.))
            .flex_none()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border);

        let back_view = view.clone();
        let hover = colors.hover;
        bar = bar.child(
            div()
                .id("cfe-back")
                .h(px(26.))
                .px(px(6.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(R_SM)
                .text_size(FS_SM)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(move |_, _, cx| {
                    back_view.update(cx, |editor, cx| editor.request_leave(Leave::Close, cx));
                })
                .child(
                    Icon::new(IconName::ChevR)
                        .size(px(12.))
                        .color(colors.muted)
                        .flipped(),
                )
                .child(ui::t!("changes.editor.back")),
        );
        bar = bar.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(FS_SM)
                .child(self.path.clone()),
        );

        if let Load::Text(state) = &self.load {
            let total = state.resolutions.len();
            let left = unresolved_count(&state.resolutions);
            let (left_text, left_color) = if left == 0 {
                (ui::t!("changes.editor.none_left"), colors.success)
            } else {
                (ui::t!("changes.editor.left", n = left), colors.muted)
            };
            bar = bar.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(ui::i18n::t_count("changes.editor.count", total as u64, &[])),
                    )
                    .child("·")
                    .child(div().text_color(left_color).child(left_text)),
            );
            let (prev_view, next_view) = (view.clone(), view.clone());
            bar = bar
                .child(
                    Button::tool("cfe-prev-conflict")
                        .size(ButtonSize::Small)
                        .icon_only(IconName::Up)
                        .tooltip_kbd(ui::t!("changes.editor.prev_conflict"), "Alt+↑")
                        .disabled(total == 0)
                        .on_click(move |_, _, cx| {
                            prev_view.update(cx, |editor, cx| editor.step_conflict(-1, cx));
                        }),
                )
                .child(
                    Button::tool("cfe-next-conflict")
                        .size(ButtonSize::Small)
                        .icon_only(IconName::Down)
                        .tooltip_kbd(ui::t!("changes.editor.next_conflict"), "Alt+↓")
                        .disabled(total == 0)
                        .on_click(move |_, _, cx| {
                            next_view.update(cx, |editor, cx| editor.step_conflict(1, cx));
                        }),
                );
            let base_view = view.clone();
            bar = bar.child(
                Switch::new("cfe-show-base")
                    .label(ui::t!("changes.editor.show_base"))
                    .checked(self.show_base)
                    .on_toggle(move |checked, _, cx| {
                        base_view.update(cx, |editor, cx| editor.set_show_base(checked, cx));
                    }),
            );
        }

        if let Some(at) = self.file_position() {
            let (prev_view, next_view) = (view.clone(), view);
            let last = self.files.len() - 1;
            bar = bar.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!(
                        "changes.editor.file_of",
                        i = at + 1,
                        n = self.files.len()
                    ))
                    .child(icon_tool(
                        "cfe-prev-file",
                        IconName::ChevR,
                        true,
                        ui::t!("changes.editor.prev_file"),
                        at > 0,
                        colors,
                        move |_, cx| {
                            prev_view.update(cx, |editor, cx| editor.step_file(-1, cx));
                        },
                    ))
                    .child(icon_tool(
                        "cfe-next-file",
                        IconName::ChevR,
                        false,
                        ui::t!("changes.editor.next_file"),
                        at < last,
                        colors,
                        move |_, cx| {
                            next_view.update(cx, |editor, cx| editor.step_file(1, cx));
                        },
                    )),
            );
        }
        bar
    }

    /// The warning under the header when the file on disk is not what the
    /// merge wrote (rerere, another editor).
    fn render_disk_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let Load::Text(state) = &self.load else {
            return None;
        };
        if !state.conflict.guard.changed_after_merge {
            return None;
        }
        let colors = cx.theme().colors;
        Some(
            div()
                .id("cfe-disk-banner")
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
                .child(Icon::new(IconName::Warn).color(colors.warning))
                .child(ui::t!("changes.editor.changed_after_merge")),
        )
    }

    fn pane_headers(&self, colors: ThemeColors) -> Div {
        let head = |dot: Hsla, name: String, tag: Option<SharedString>| {
            div()
                .h(px(34.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(colors.surface)
                .border_b_1()
                .border_r_1()
                .border_color(colors.border)
                .text_size(FS_SM)
                .child(div().size(px(8.)).rounded_full().bg(dot).flex_none())
                .child(
                    Icon::new(IconName::Branch)
                        .size(px(12.))
                        .color(colors.muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .when_some(tag, |this, tag| {
                    this.child(div().flex_none().text_color(colors.muted).child(tag))
                })
        };
        let mut row = div().w_full().flex_none().flex().child(
            head(
                colors.st_deleted,
                self.ours_label.clone(),
                Some(ui::t!("changes.editor.head_tag")),
            )
            .flex_1()
            .min_w_0(),
        );
        if self.show_base {
            row = row.child(
                head(
                    colors.muted,
                    ui::t!("changes.editor.base").to_string(),
                    None,
                )
                .w(px(BASE_WIDTH))
                .flex_none(),
            );
        }
        row.child(
            head(
                colors.st_added,
                self.theirs_label.clone(),
                Some(ui::t!("changes.editor.incoming_tag")),
            )
            .flex_1()
            .min_w_0()
            .border_r_0(),
        )
    }

    fn render_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Load::Text(state) = &self.load else {
            return Vec::new();
        };
        let colors = cx.theme().colors;
        range
            .filter_map(|ix| {
                let row = state.rows.get(ix)?;
                Some(self.render_row(ix, row, state, colors, cx))
            })
            .collect()
    }

    fn render_row(
        &self,
        ix: usize,
        row: &Row,
        state: &TextState,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conflict = &state.conflict;
        let view = cx.entity();
        let (index, head, last) = match row.kind {
            RowKind::ConflictHead { index } => (Some(index), true, false),
            RowKind::Conflict { index, last } => (Some(index), false, last),
            _ => (None, false, false),
        };
        let style = index.map(|index| {
            let (color, thick) = state.box_style(index, colors);
            BoxStyle {
                color,
                thick,
                top: head,
                bottom: last,
            }
        });
        let shell = div()
            .id(SharedString::from(format!("cfe-row-{ix}")))
            .w_full()
            .h(px(ROW))
            .flex_none()
            .flex()
            .text_size(cx.theme().mono_font_size)
            .when_some(index, |this, index| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |editor, _, _, cx| editor.focus_conflict(index, cx)),
                )
            });

        if let (RowKind::ConflictHead { index }, Some(style)) = (row.kind, style) {
            let resolution = &state.resolutions[index];
            let (_, ours, theirs) = &state.ranges[index];
            let unresolved = is_unresolved(resolution);
            let custom = matches!(resolution, ConflictResolution::Custom(_));
            let side_head = |side: ConflictSide, lines: usize, shell: Div| {
                let view = view.clone();
                let order = pick_order(resolution, side);
                let mut cell = shell
                    .px(px(8.))
                    .gap(px(8.))
                    .bg(colors.surface)
                    .font_family(ui::fonts::ui_font().family.clone())
                    .text_size(FS_SM);
                if side == ConflictSide::Ours {
                    cell = cell.child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui::t!("changes.editor.conflict", n = index + 1)),
                    );
                }
                cell = cell
                    .child(
                        Checkbox::new(format!("cfe-pick-{index}-{side:?}"))
                            .label(ui::t!("changes.editor.use_lines"))
                            .checked(is_picked(resolution, side))
                            // A hand edit locks the picks until "Reset to picks".
                            .disabled(custom)
                            .on_toggle(move |_, _, cx| {
                                view.update(cx, |editor, cx| editor.toggle_pick(index, side, cx));
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(colors.muted)
                            .child(ui::i18n::t_count("changes.editor.lines", lines as u64, &[])),
                    );
                if custom && side == ConflictSide::Ours {
                    cell = cell.child(
                        div()
                            .flex_none()
                            .text_color(colors.muted)
                            .child(ui::t!("changes.editor.edited_in_result")),
                    );
                }
                if let Some(order) = order {
                    cell = cell.child(
                        div()
                            .flex_none()
                            .min_w(px(16.))
                            .h(px(16.))
                            .px(px(4.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors.accent)
                            .text_color(colors.accent_fg)
                            .text_size(FS_XS)
                            .font_weight(FontWeight::BOLD)
                            .child((order + 1).to_string()),
                    );
                }
                cell
            };
            let mut cells = vec![boxed(
                side_head(ConflictSide::Ours, ours.len(), pane_cell()),
                Some(style),
                colors,
            )];
            if self.show_base {
                let (base, _, _) = &state.ranges[index];
                cells.push(boxed(
                    base_cell()
                        .px(px(8.))
                        .gap(px(8.))
                        .bg(colors.surface)
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("changes.editor.base"))
                        .child(ui::i18n::t_count(
                            "changes.editor.lines",
                            base.len() as u64,
                            &[],
                        )),
                    Some(style),
                    colors,
                ));
            }
            let mut theirs_cell = side_head(ConflictSide::Theirs, theirs.len(), pane_cell());
            if unresolved {
                theirs_cell = theirs_cell.child(div().flex_1()).child(
                    Chip::new(
                        SharedString::from(format!("cfe-unresolved-{index}")),
                        ui::t!("changes.editor.unresolved"),
                    )
                    .kind(ChipKind::Warn),
                );
            } else if custom {
                let reset_view = view.clone();
                theirs_cell = theirs_cell
                    .child(div().flex_1())
                    .child(Chip::new(
                        SharedString::from(format!("cfe-side-edited-{index}")),
                        ui::t!("changes.editor.edited"),
                    ))
                    .child(
                        Button::ghost(format!("cfe-side-reset-{index}"))
                            .size(ButtonSize::Xs)
                            .label(ui::t!("changes.editor.reset_picks"))
                            .on_click(move |_, _, cx| {
                                reset_view.update(cx, |editor, cx| editor.reset_picks(index, cx));
                            }),
                    );
            }
            cells.push(boxed(theirs_cell, Some(style), colors));
            return shell.children(cells).into_any_element();
        }

        let ours_tint = match row.kind {
            RowKind::Auto(ConflictSide::Ours) | RowKind::Conflict { .. } => {
                Some(colors.diff_del_bg)
            }
            RowKind::Same => Some(colors.diff_hunk_bg),
            _ => None,
        };
        let theirs_tint = match row.kind {
            RowKind::Auto(ConflictSide::Theirs) | RowKind::Conflict { .. } => {
                Some(colors.diff_add_bg)
            }
            RowKind::Same => Some(colors.diff_hunk_bg),
            _ => None,
        };
        let ours_line = row.cells.ours.map(|at| (at, conflict.ours[at].as_str()));
        let theirs_line = row
            .cells
            .theirs
            .map(|at| (at, conflict.theirs[at].as_str()));
        let mut cells = vec![code_cell(pane_cell(), ours_line, ours_tint, style, colors)];
        if self.show_base {
            let base_line = row.cells.base.and_then(|at| {
                conflict
                    .base
                    .as_ref()
                    .and_then(|base| base.get(at))
                    .map(|text| (at, text.as_str()))
            });
            cells.push(code_cell(
                base_cell(),
                base_line,
                Some(colors.hover),
                style,
                colors,
            ));
        }
        cells.push(code_cell(
            pane_cell().border_r_0(),
            theirs_line,
            theirs_tint,
            style,
            colors,
        ));
        shell.children(cells).into_any_element()
    }

    fn render_result_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Load::Text(state) = &self.load else {
            return Vec::new();
        };
        let colors = cx.theme().colors;
        let view = cx.entity();
        range
            .filter_map(|ix| {
                let line = state.result.get(ix)?;
                Some(self.render_result_row(ix, line, state, colors, &view, cx))
            })
            .collect()
    }

    fn render_result_row(
        &self,
        ix: usize,
        line: &ResultLine,
        state: &TextState,
        colors: ThemeColors,
        view: &gpui::Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let region = match line {
            ResultLine::Text { region, .. } => *region,
            ResultLine::Head { index }
            | ResultLine::Placeholder { index }
            | ResultLine::Empty { index }
            | ResultLine::EditRow { index, .. } => Some(*index),
            ResultLine::AutoLabel { .. } => None,
        };
        let style = region.map(|index| {
            let (color, thick) = state.box_style(index, colors);
            BoxStyle {
                color,
                thick,
                top: matches!(line, ResultLine::Head { .. }),
                bottom: matches!(
                    line,
                    ResultLine::Placeholder { .. } | ResultLine::Empty { .. }
                ) || matches!(
                    line,
                    ResultLine::Text { last: true, .. } | ResultLine::EditRow { last: true, .. }
                ),
            }
        });
        let shell = div()
            .id(SharedString::from(format!("cfe-result-{ix}")))
            .w_full()
            .h(px(ROW))
            .flex_none()
            .flex()
            .items_center()
            .text_size(cx.theme().mono_font_size);
        // The result has no pane separator: only conflict regions are boxed.
        let shell = match style {
            Some(style) => boxed(shell, Some(style), colors),
            None => shell,
        };
        let shell = shell.when_some(region, |this, index| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, _, _, cx| editor.focus_conflict(index, cx)),
            )
        });
        // A click on the body of a region (not its header) edits it by hand.
        let body_of = match line {
            ResultLine::Text { region, .. } => *region,
            ResultLine::Placeholder { index } | ResultLine::Empty { index } => Some(*index),
            _ => None,
        };
        let shell = shell.when_some(body_of, |this, index| {
            this.cursor_text().on_click(
                cx.listener(move |editor, _, window, cx| editor.start_edit(index, window, cx)),
            )
        });
        let gutter = |no: Option<usize>| {
            div()
                .w(px(GUTTER))
                .flex_none()
                .h_full()
                .flex()
                .items_center()
                .justify_end()
                .pr(px(8.))
                .font_family(FONT_MONO)
                .text_color(colors.muted)
                .child(no.map(|no| no.to_string()).unwrap_or_default())
        };
        match line {
            ResultLine::Text { no, source, .. } => shell
                .child(gutter(Some(*no)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(8.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family(FONT_MONO)
                        .text_color(colors.text)
                        .child(SharedString::from(source.text(&state.conflict).to_owned())),
                )
                .into_any_element(),
            ResultLine::AutoLabel { side } => shell
                .pl(px(GUTTER + 8.))
                .text_size(FS_XS)
                .text_color(colors.muted)
                .child(
                    div()
                        .id(SharedString::from(format!("cfe-auto-{ix}")))
                        .tooltip(Tooltip::text(ui::t!(
                            "changes.editor.auto_note",
                            branch = self.side_label(*side)
                        )))
                        .child(ui::t!(
                            "changes.editor.auto",
                            branch = self.side_label(*side)
                        )),
                )
                .into_any_element(),
            ResultLine::Placeholder { .. } => shell
                .bg(colors.warning.opacity(0.12))
                .child(gutter(None))
                .child(
                    div()
                        .px(px(8.))
                        .text_size(FS_SM)
                        .text_color(colors.warning)
                        .child(ui::t!("changes.editor.pick_a_side")),
                )
                .into_any_element(),
            // Blank: the text area is laid over these rows (see `render_text`).
            ResultLine::EditRow { .. } => {
                shell.bg(colors.bg).child(gutter(None)).into_any_element()
            }
            ResultLine::Empty { .. } => shell
                .child(gutter(None))
                .child(
                    div()
                        .px(px(8.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("changes.editor.no_lines")),
                )
                .into_any_element(),
            ResultLine::Head { index } => {
                let index = *index;
                let resolution = &state.resolutions[index];
                let (chip, first): (SharedString, Option<ConflictSide>) = match resolution {
                    ConflictResolution::Picked(sides) if sides.len() == 2 => {
                        (ui::t!("changes.editor.both"), Some(sides[0]))
                    }
                    ConflictResolution::Picked(sides) if sides.len() == 1 => (
                        SharedString::from(self.side_label(sides[0]).to_owned()),
                        None,
                    ),
                    ConflictResolution::Custom(_) => (ui::t!("changes.editor.edited"), None),
                    _ => (ui::t!("changes.editor.unresolved"), None),
                };
                let kind = if is_unresolved(resolution) {
                    ChipKind::Warn
                } else {
                    ChipKind::None
                };
                let swap_view = view.clone();
                let (edit_view, reset_view) = (view.clone(), view.clone());
                let custom = matches!(resolution, ConflictResolution::Custom(_));
                shell
                    .bg(colors.surface)
                    .px(px(12.))
                    .gap(px(8.))
                    .text_size(FS_SM)
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui::t!("changes.editor.conflict", n = index + 1)),
                    )
                    .child(
                        Chip::new(SharedString::from(format!("cfe-chip-{index}")), chip).kind(kind),
                    )
                    .when_some(first, |this, first| {
                        this.child(div().flex_none().text_color(colors.muted).child(ui::t!(
                            "changes.editor.first",
                            branch = self.side_label(first)
                        )))
                        .child(
                            Button::ghost(format!("cfe-swap-{index}"))
                                .size(ButtonSize::Xs)
                                .icon_only(IconName::Undo)
                                .tooltip(ui::t!("changes.editor.swap"))
                                .on_click(move |_, _, cx| {
                                    swap_view.update(cx, |editor, cx| editor.swap_pick(index, cx));
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .when(custom, |this| {
                        this.child(
                            Button::ghost(format!("cfe-reset-{index}"))
                                .size(ButtonSize::Xs)
                                .label(ui::t!("changes.editor.reset_picks"))
                                .on_click(move |_, _, cx| {
                                    reset_view
                                        .update(cx, |editor, cx| editor.reset_picks(index, cx));
                                }),
                        )
                    })
                    .child(
                        Button::ghost(format!("cfe-edit-{index}"))
                            .size(ButtonSize::Xs)
                            .icon_only(IconName::Edit)
                            .tooltip_kbd(ui::t!("changes.editor.edit"), "F2")
                            .on_click(move |_, window, cx| {
                                edit_view
                                    .update(cx, |editor, cx| editor.start_edit(index, window, cx));
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    /// The text area over the reserved rows of the region being edited.
    fn render_edit_area(&self, state: &TextState, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.edit.as_ref()?;
        let (_, rows) = state.editing?;
        let first = state
            .result
            .iter()
            .position(|line| matches!(line, ResultLine::EditRow { .. }))?;
        let offset = self.result_scroll.0.borrow().base_handle.offset().y;
        let top = offset + px(first as f32 * ROW);
        Some(
            div()
                .id("cfe-edit-area")
                .absolute()
                .left(px(GUTTER + 2.))
                .right(px(2.))
                .top(top)
                .h(px(rows as f32 * ROW))
                .occlude()
                .on_mouse_down_out(cx.listener(|editor, _, _, cx| {
                    editor.stop_edit();
                    cx.notify();
                }))
                .child(edit.area.clone())
                .into_any_element(),
        )
    }

    /// The side panes and the result pane of a text conflict.
    fn render_text(&self, state: &TextState, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let top = uniform_list(
            "cfe-rows",
            state.rows.len(),
            cx.processor(|editor, range, _, cx| editor.render_rows(range, cx)),
        )
        .track_scroll(&self.top_scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let result = uniform_list(
            "cfe-result-rows",
            state.result.len(),
            cx.processor(|editor, range, _, cx| editor.render_result_rows(range, cx)),
        )
        .track_scroll(&self.result_scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let lines = state
            .result
            .iter()
            .filter(|line| matches!(line, ResultLine::Text { .. }))
            .count();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(relative(0.58))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(self.pane_headers(colors))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(top)
                            .child(Scrollbar::uniform("cfe-rows-bar", self.top_scroll.clone())),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(colors.border_strong)
                    .child(
                        div()
                            .h(px(30.))
                            .flex_none()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .bg(colors.surface)
                            .border_b_1()
                            .border_color(colors.border)
                            .text_size(FS_SM)
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(ui::t!("changes.editor.result")),
                            )
                            .child(div().text_color(colors.muted).child(format!(
                                "{} · {}",
                                ui::t!("changes.editor.result_hint"),
                                ui::i18n::t_count("changes.editor.lines", lines as u64, &[])
                            ))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                // The text area of a hand edit is laid over the
                                // rows reserved for it, at the list's scroll offset.
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(result)
                                    .children(self.render_edit_area(state, cx)),
                            )
                            .child(Scrollbar::uniform(
                                "cfe-result-bar",
                                self.result_scroll.clone(),
                            )),
                    ),
            )
    }

    /// The centred card of a conflict that only takes a whole side.
    fn render_whole(
        &self,
        kind: Whole,
        choice: Option<ConflictSide>,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        // The side without the file: its card deletes it.
        let absent_side = kind.absent_side();
        let added = matches!(kind, Whole::AddedByUs | Whole::AddedByThem);
        let title = match kind {
            Whole::DeletedByUs => ui::t!(
                "changes.editor.title_deleted",
                path = self.path,
                branch = self.side_label(ConflictSide::Ours)
            ),
            Whole::DeletedByThem => ui::t!(
                "changes.editor.title_deleted",
                path = self.path,
                branch = self.side_label(ConflictSide::Theirs)
            ),
            Whole::AddedByUs | Whole::AddedByThem => {
                let adder = if kind == Whole::AddedByUs {
                    ConflictSide::Ours
                } else {
                    ConflictSide::Theirs
                };
                ui::t!(
                    "changes.editor.title_added",
                    path = self.path,
                    branch = self.side_label(adder)
                )
            }
            Whole::DeletedByBoth => ui::t!("changes.editor.title_deleted_both", path = self.path),
            _ => ui::t!("changes.editor.title_both", path = self.path),
        };
        let kind_line = |side: ConflictSide| -> SharedString {
            if absent_side == Some(side) || kind == Whole::DeletedByBoth {
                return ui::t!("changes.editor.card_removed");
            }
            match kind {
                Whole::Binary => ui::t!("changes.editor.kind_binary"),
                Whole::NotText => ui::t!("changes.editor.kind_not_text"),
                Whole::Symlink => ui::t!("changes.editor.kind_symlink"),
                Whole::Submodule => ui::t!("changes.editor.kind_submodule"),
                _ => ui::t!("changes.editor.card_kept", branch = self.side_label(side)),
            }
        };
        let card = |side: ConflictSide| {
            let selected = choice == Some(side);
            let branch = self.side_label(side);
            let label = if kind == Whole::DeletedByBoth || (added && absent_side == Some(side)) {
                ui::t!("changes.editor.delete_the_file")
            } else if added {
                ui::t!("changes.editor.keep_the_file")
            } else if absent_side == Some(side) {
                ui::t!("changes.editor.delete_file", branch = branch)
            } else if side == ConflictSide::Ours {
                ui::t!("changes.editor.keep_version", branch = branch)
            } else {
                ui::t!("changes.editor.use_version", branch = branch)
            };
            let view = view.clone();
            let accent = colors.accent;
            div()
                .id(SharedString::from(format!("cfe-card-{side:?}")))
                .flex_1()
                .min_w_0()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(ui::metrics::R_MD)
                .border_1()
                .border_color(if selected {
                    colors.accent
                } else {
                    colors.border_strong
                })
                .bg(if selected {
                    colors.accent_soft
                } else {
                    colors.surface
                })
                .cursor_pointer()
                .hover(move |style| style.border_color(accent))
                .on_click(move |_, _, cx| {
                    view.update(cx, |editor, cx| editor.choose(side, cx));
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(
                            div()
                                .size(px(14.))
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(if selected {
                                    colors.accent
                                } else {
                                    colors.border_strong
                                })
                                .bg(colors.bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(selected, |this| {
                                    this.child(div().size(px(6.)).rounded_full().bg(colors.accent))
                                }),
                        )
                        .child(label),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(kind_line(side)),
                )
        };
        // Deleted by both: only the deletion is left to confirm (any side's
        // pick removes the path).
        let cards = if kind == Whole::DeletedByBoth {
            div().flex().gap(px(12.)).child(card(ConflictSide::Ours))
        } else {
            div()
                .flex()
                .gap(px(12.))
                .child(card(ConflictSide::Ours))
                .child(card(ConflictSide::Theirs))
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .justify_center()
            .overflow_hidden()
            .px(px(16.))
            .py(px(24.))
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .text_size(FS_LG)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .max_w(px(560.))
                            .text_color(colors.muted)
                            .child(self.kind_note(kind)),
                    )
                    .child(cards),
            )
    }

    fn render_failed(&self, message: &str, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let take = |side: ConflictSide| {
            let view = view.clone();
            Button::new(format!("cfe-take-{side:?}"))
                .label(ui::t!(
                    "changes.conflict.take",
                    branch = self.side_label(side)
                ))
                .disabled(self.saving.is_busy())
                .on_click(move |_, _, cx| {
                    view.update(cx, |editor, cx| editor.take_whole(side, cx));
                })
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(
                div()
                    .max_w(px(560.))
                    .text_color(colors.error)
                    .child(ui::t!("changes.editor.load_failed", error = message)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(take(ConflictSide::Ours))
                    .child(take(ConflictSide::Theirs)),
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let mut bar = div()
            .min_h(px(48.))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM);

        if let Some(leave) = self.confirm.clone() {
            let (discard_view, keep_view) = (view.clone(), view);
            let (ask, discard) = match leave {
                Leave::TakeAll(_) => (
                    ui::t!("changes.editor.replace_ask"),
                    ui::t!("changes.editor.replace"),
                ),
                Leave::ReplaceDisk => (
                    ui::t!("changes.editor.replace_disk_ask"),
                    ui::t!("changes.editor.replace"),
                ),
                _ => (
                    ui::t!("changes.editor.discard_ask", path = self.path),
                    ui::t!("changes.editor.discard"),
                ),
            };
            return bar
                .child(Icon::new(IconName::Warn).color(colors.warning))
                .child(ask)
                .child(div().flex_1())
                .child(
                    Button::danger("cfe-discard")
                        .label(discard)
                        .on_click(move |_, _, cx| {
                            discard_view.update(cx, |editor, cx| editor.leave(leave.clone(), cx));
                        }),
                )
                .child(
                    Button::new("cfe-keep")
                        .label(ui::t!("changes.editor.keep_editing"))
                        .on_click(move |_, _, cx| {
                            keep_view.update(cx, |editor, cx| {
                                editor.confirm = None;
                                cx.notify();
                            });
                        }),
                );
        }

        // Status on the left.
        let status = match &self.load {
            Load::Text(state) => {
                let left = unresolved_count(&state.resolutions);
                Some(if left == 0 {
                    (
                        IconName::Check,
                        colors.success,
                        ui::t!("changes.editor.all_resolved"),
                    )
                } else {
                    (
                        IconName::Warn,
                        colors.warning,
                        ui::i18n::t_count("changes.editor.footer_left", left as u64, &[]),
                    )
                })
            }
            Load::Whole { choice, .. } => Some(if choice.is_some() {
                (
                    IconName::Check,
                    colors.success,
                    ui::t!("changes.editor.all_resolved"),
                )
            } else {
                (
                    IconName::Warn,
                    colors.warning,
                    ui::t!("changes.editor.choose"),
                )
            }),
            _ => None,
        };
        if let Some((icon, color, text)) = status {
            bar = bar.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(color)
                    .child(Icon::new(icon).color(color))
                    .child(text),
            );
        }
        bar = bar.child(div().flex_1());

        if matches!(self.load, Load::Text(_)) {
            let more_view = view.clone();
            let slot = self.more_bounds.clone();
            let bounds = self.more_bounds.clone();
            bar = bar.child(
                div().relative().child(bounds_probe(slot)).child(
                    Button::ghost("cfe-more")
                        .icon_only(IconName::ChevD)
                        .tooltip(ui::t!("changes.editor.more"))
                        .on_click(move |_, window, cx| {
                            let entries = [ConflictSide::Ours, ConflictSide::Theirs]
                                .into_iter()
                                .map(|side| {
                                    let view = more_view.clone();
                                    let label = more_view.read(cx).side_label(side).to_owned();
                                    MenuEntry::Item(
                                        MenuItem::new(ui::t!(
                                            "changes.editor.take_all",
                                            branch = label
                                        ))
                                        .icon(IconName::Check)
                                        .action(
                                            move |_, cx| {
                                                view.update(cx, |editor, cx| {
                                                    editor.request_take_all(side, cx)
                                                });
                                            },
                                        ),
                                    )
                                })
                                .collect::<Vec<_>>();
                            match *bounds.borrow() {
                                Some(anchor) => {
                                    menu::open_dropdown(anchor, entries, window, cx);
                                }
                                None => {
                                    menu::open_context_menu(
                                        window.mouse_position(),
                                        entries,
                                        window,
                                        cx,
                                    );
                                }
                            }
                        }),
                ),
            );
        }

        let (cancel_view, save_view) = (view.clone(), view);
        let can_save = self.can_save();
        let saveable = matches!(self.load, Load::Text(_) | Load::Whole { .. });
        bar = bar.child(
            Button::ghost("cfe-cancel")
                .label(ui::t!("changes.editor.cancel"))
                .on_click(move |_, _, cx| {
                    cancel_view.update(cx, |editor, cx| editor.cancel(cx));
                }),
        );
        if saveable {
            let mut save = Button::primary("cfe-save")
                .label(ui::t!("changes.editor.save"))
                .disabled(!can_save)
                .on_click(move |_, _, cx| {
                    save_view.update(cx, |editor, cx| editor.save(cx));
                });
            if !can_save && !self.saving.is_busy() {
                save = save.tooltip(ui::t!("changes.editor.save_blocked"));
            }
            bar = bar.child(save);
        }
        bar
    }
}

impl Render for ConflictEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            window.focus(&self.focus, cx);
        }
        let colors = cx.theme().colors;
        let body = match &self.load {
            Load::Loading => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .child(Skeleton::new("cfe-skeleton-1", px(320.)))
                .child(Skeleton::new("cfe-skeleton-2", px(260.)))
                .child(Skeleton::new("cfe-skeleton-3", px(290.))),
            Load::Failed(message) => self.render_failed(&message.clone(), cx),
            Load::Text(state) => self.render_text(state, cx),
            Load::Whole { kind, choice } => self.render_whole(*kind, *choice, cx),
        };
        div()
            .id("conflict-editor")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .key_context("conflict-editor")
            .track_focus(&self.focus)
            .on_action(cx.listener(|editor, _: &PrevConflict, _, cx| editor.step_conflict(-1, cx)))
            .on_action(cx.listener(|editor, _: &NextConflict, _, cx| editor.step_conflict(1, cx)))
            .on_action(cx.listener(|editor, _: &PickOurs, _, cx| {
                editor.toggle_focused(ConflictSide::Ours, cx)
            }))
            .on_action(cx.listener(|editor, _: &PickTheirs, _, cx| {
                editor.toggle_focused(ConflictSide::Theirs, cx)
            }))
            .on_action(cx.listener(|editor, _: &SaveResolution, _, cx| editor.save(cx)))
            .on_action(
                cx.listener(|editor, _: &EditResult, window, cx| editor.edit_focused(window, cx)),
            )
            .on_action(cx.listener(|editor, _: &Cancel, _, cx| editor.cancel(cx)))
            .child(self.render_header(cx))
            .children(self.render_disk_banner(cx))
            .child(body)
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conflict model over plain strings: `ours` / `theirs` / `base` lines
    /// and hand-written segments.
    fn model(
        base: &[&str],
        ours: &[&str],
        theirs: &[&str],
        segments: Vec<ConflictSegment>,
    ) -> TextConflict {
        let owned = |lines: &[&str]| lines.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        TextConflict {
            base: Some(owned(base)),
            ours: owned(ours),
            theirs: owned(theirs),
            base_endings: vec![git_core::LineEnding::Lf; base.len()],
            ours_endings: vec![git_core::LineEnding::Lf; ours.len()],
            theirs_endings: vec![git_core::LineEnding::Lf; theirs.len()],
            segments,
            guard: ConflictGuard::default(),
        }
    }

    /// Stable `a`, conflict (ours x1 x2 / theirs y1 / base b1 b2 b3), auto
    /// theirs `c`→`C`, same `s`, conflict (ours-only line / nothing).
    fn sample() -> TextConflict {
        model(
            &["a", "b1", "b2", "b3", "c", "s", "m"],
            &["a", "x1", "x2", "c", "s", "o"],
            &["a", "y1", "C", "s"],
            vec![
                ConflictSegment::Stable {
                    base: 0..1,
                    ours: 0..1,
                    theirs: 0..1,
                },
                ConflictSegment::Conflict {
                    base: 1..4,
                    ours: 1..3,
                    theirs: 1..2,
                },
                ConflictSegment::Auto {
                    side: ConflictSide::Theirs,
                    base: 4..5,
                    ours: 3..4,
                    theirs: 2..3,
                },
                ConflictSegment::Same {
                    base: 5..6,
                    ours: 4..5,
                    theirs: 3..4,
                },
                ConflictSegment::Conflict {
                    base: 6..7,
                    ours: 5..6,
                    theirs: 4..4,
                },
            ],
        )
    }

    fn kinds(rows: &[Row]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn rows_align_the_two_sides_with_fillers() {
        let conflict = sample();
        let rows = rows(&conflict, false);
        assert_eq!(
            kinds(&rows),
            [
                RowKind::Stable,
                RowKind::ConflictHead { index: 0 },
                RowKind::Conflict {
                    index: 0,
                    last: false
                },
                RowKind::Conflict {
                    index: 0,
                    last: true
                },
                RowKind::Auto(ConflictSide::Theirs),
                RowKind::Same,
                RowKind::ConflictHead { index: 1 },
                RowKind::Conflict {
                    index: 1,
                    last: true
                },
            ]
        );
        // Conflict 0: two lines of ours against one of theirs plus a filler.
        assert_eq!(
            rows[2].cells,
            Cells {
                ours: Some(1),
                base: None,
                theirs: Some(1)
            }
        );
        assert_eq!(
            rows[3].cells,
            Cells {
                ours: Some(2),
                base: None,
                theirs: None
            }
        );
        // The header row has no lines; line numbers are side indexes.
        assert_eq!(rows[1].cells, Cells::default());
        // Conflict 1: theirs deleted the line, so its cell is a filler.
        assert_eq!(rows[7].cells.ours, Some(5));
        assert_eq!(rows[7].cells.theirs, None);
        assert_eq!(head_rows(&rows), [1, 6]);
    }

    #[test]
    fn the_base_pane_pads_the_conflict_to_its_own_length() {
        let conflict = sample();
        let rows = rows(&conflict, true);
        // Conflict 0 has three base lines now: three body rows.
        let body: Vec<_> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Conflict { index: 0, .. }))
            .collect();
        assert_eq!(body.len(), 3);
        assert_eq!(body[2].cells.base, Some(3));
        assert_eq!(body[2].cells.ours, None);
        assert_eq!(
            body[2].kind,
            RowKind::Conflict {
                index: 0,
                last: true
            }
        );
        // Stable rows carry the base line too.
        assert_eq!(rows[0].cells.base, Some(0));
        assert_eq!(head_rows(&rows).len(), 2);
    }

    #[test]
    fn a_conflict_without_lines_still_gets_a_body_row() {
        let conflict = model(
            &[],
            &[],
            &[],
            vec![ConflictSegment::Conflict {
                base: 0..0,
                ours: 0..0,
                theirs: 0..0,
            }],
        );
        let rows = rows(&conflict, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].kind,
            RowKind::Conflict {
                index: 0,
                last: true
            }
        );
    }

    fn texts(conflict: &TextConflict, lines: &[ResultLine]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|line| line.text(conflict).map(str::to_owned))
            .collect()
    }

    #[test]
    fn the_result_marks_unresolved_regions_and_numbers_the_lines() {
        let conflict = sample();
        let lines = result_lines(&conflict, &conflict.unresolved());
        assert_eq!(
            lines
                .iter()
                .map(|l| match l {
                    ResultLine::Text { no, .. } => format!("t{no}"),
                    ResultLine::AutoLabel { .. } => "auto".into(),
                    ResultLine::Head { index } => format!("h{index}"),
                    ResultLine::Placeholder { index } => format!("p{index}"),
                    ResultLine::Empty { index } => format!("e{index}"),
                    ResultLine::EditRow { index, .. } => format!("w{index}"),
                })
                .collect::<Vec<_>>(),
            ["t1", "h0", "p0", "auto", "t2", "t3", "h1", "p1"]
        );
        assert_eq!(result_heads(&lines), [1, 6]);
        assert_eq!(texts(&conflict, &lines), ["a", "C", "s"]);
    }

    #[test]
    fn the_result_follows_each_kind_of_resolution() {
        let conflict = sample();
        let resolutions = vec![
            ConflictResolution::Picked(vec![ConflictSide::Theirs, ConflictSide::Ours]),
            ConflictResolution::Custom("one\r\ntwo\n".into()),
        ];
        let lines = result_lines(&conflict, &resolutions);
        assert_eq!(
            texts(&conflict, &lines),
            ["a", "y1", "x1", "x2", "C", "s", "one", "two"]
        );
        // The last line of each region closes its box; numbering continues.
        let closing: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                ResultLine::Text {
                    no,
                    last: true,
                    region,
                    ..
                } => Some((*no, *region)),
                _ => None,
            })
            .collect();
        assert_eq!(closing, [(4, Some(0)), (8, Some(1))]);
        // A removed region (empty pick of an empty side) shows a note row.
        let removed = result_lines(
            &conflict,
            &[
                ConflictResolution::Picked(vec![ConflictSide::Ours]),
                ConflictResolution::Picked(vec![ConflictSide::Theirs]),
            ],
        );
        assert!(matches!(
            removed.last(),
            Some(ResultLine::Empty { index: 1 })
        ));
    }

    /// The result pane must show exactly what `render` writes.
    #[test]
    fn the_result_lines_match_the_rendered_file() {
        let conflict = sample();
        let resolutions = vec![
            ConflictResolution::Picked(vec![ConflictSide::Ours, ConflictSide::Theirs]),
            ConflictResolution::Custom("hand\n".into()),
        ];
        let rendered = conflict.render(&resolutions).unwrap();
        let lines = result_lines(&conflict, &resolutions);
        let joined: String = texts(&conflict, &lines)
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(joined, rendered);
    }

    #[test]
    fn ticking_keeps_the_order_and_unticking_the_last_unresolves() {
        use ConflictResolution::{Picked, Unresolved};
        let ours = ConflictSide::Ours;
        let theirs = ConflictSide::Theirs;
        let one = toggle_pick(&Unresolved, ours);
        assert_eq!(one, Picked(vec![ours]));
        let both = toggle_pick(&one, theirs);
        assert_eq!(both, Picked(vec![ours, theirs]));
        assert_eq!(pick_order(&both, ours), Some(0));
        assert_eq!(pick_order(&both, theirs), Some(1));
        assert_eq!(pick_order(&one, ours), None);
        // Untick ours: theirs stays.
        let rest = toggle_pick(&both, ours);
        assert_eq!(rest, Picked(vec![theirs]));
        // Tick theirs first, then ours: the order follows the ticks.
        let reversed = toggle_pick(&toggle_pick(&Unresolved, theirs), ours);
        assert_eq!(reversed, Picked(vec![theirs, ours]));
        assert_eq!(toggle_pick(&rest, theirs), Unresolved);
        // A hand edit gives way to the tick.
        assert_eq!(
            toggle_pick(&ConflictResolution::Custom("x".into()), ours),
            Picked(vec![ours])
        );
    }

    #[test]
    fn swapping_reverses_two_picks_only() {
        use ConflictResolution::{Picked, Unresolved};
        let (o, t) = (ConflictSide::Ours, ConflictSide::Theirs);
        assert_eq!(swap_order(&Picked(vec![o, t])), Picked(vec![t, o]));
        assert_eq!(swap_order(&Picked(vec![o])), Picked(vec![o]));
        assert_eq!(swap_order(&Unresolved), Unresolved);
    }

    #[test]
    fn take_all_sets_every_conflict_to_one_side() {
        let all = take_all(3, ConflictSide::Theirs);
        assert_eq!(all.len(), 3);
        assert!(all
            .iter()
            .all(|r| *r == ConflictResolution::Picked(vec![ConflictSide::Theirs])));
        assert!(can_save(&all));
    }

    #[test]
    fn save_needs_every_conflict_resolved_and_dirty_needs_a_pick() {
        use ConflictResolution::{Custom, Picked, Unresolved};
        let o = ConflictSide::Ours;
        assert!(can_save(&[]));
        assert!(!can_save(&[Unresolved]));
        assert!(!can_save(&[Picked(vec![o]), Unresolved]));
        assert!(!can_save(&[Picked(vec![])]));
        assert!(can_save(&[Picked(vec![o]), Custom(String::new())]));
        assert_eq!(
            unresolved_count(&[Unresolved, Picked(vec![o]), Picked(vec![])]),
            2
        );
        assert!(!is_dirty(&[Unresolved, Unresolved]));
        assert!(is_dirty(&[Unresolved, Picked(vec![o])]));
        assert!(!is_dirty(&[]));
    }

    #[test]
    fn focus_steps_clamp_and_start_from_the_ends() {
        assert_eq!(step_focus(None, 0, 1), None);
        assert_eq!(step_focus(None, 3, 1), Some(0));
        assert_eq!(step_focus(None, 3, -1), Some(2));
        assert_eq!(step_focus(Some(0), 3, -1), Some(0));
        assert_eq!(step_focus(Some(1), 3, 1), Some(2));
        assert_eq!(step_focus(Some(2), 3, 1), Some(2));
    }

    #[test]
    fn the_state_rebuilds_the_result_after_every_change() {
        let mut state = TextState::new(sample(), false);
        assert_eq!(state.focused, Some(0));
        state.toggle(0, ConflictSide::Ours);
        assert!(matches!(
            state.result[state.result_heads[0] + 1],
            ResultLine::Text { no: 2, .. }
        ));
        state.toggle(0, ConflictSide::Theirs);
        state.swap(0);
        assert_eq!(
            state.resolutions[0],
            ConflictResolution::Picked(vec![ConflictSide::Theirs, ConflictSide::Ours])
        );
        state.set_all(take_all(2, ConflictSide::Ours));
        assert!(can_save(&state.resolutions));
        assert!(state
            .result
            .iter()
            .all(|l| !matches!(l, ResultLine::Placeholder { .. })));
    }

    fn picked(side: ConflictSide) -> ConflictResolution {
        ConflictResolution::Picked(vec![side])
    }

    #[test]
    fn a_text_area_edit_that_renders_like_the_picks_is_not_custom() {
        let mut state = TextState::new(sample(), false);
        state.toggle(0, ConflictSide::Ours);
        assert_eq!(state.edit_text(0), "x1\nx2");
        // Entering the edit changes nothing; the same text, with or without
        // the terminating newline, is still the picks.
        state.apply_edit(0, "x1\nx2".into(), 4);
        assert_eq!(state.resolutions[0], picked(ConflictSide::Ours));
        state.apply_edit(0, "x1\nx2\n".into(), 4);
        assert_eq!(state.resolutions[0], picked(ConflictSide::Ours));
        state.apply_edit(0, "x1\nz".into(), 4);
        assert_eq!(
            state.resolutions[0],
            ConflictResolution::Custom("x1\nz".into())
        );
        // Typing the text back ends the hand edit.
        state.apply_edit(0, "x1\nx2".into(), 4);
        assert_eq!(state.resolutions[0], picked(ConflictSide::Ours));
    }

    #[test]
    fn reset_restores_the_picks_the_conflict_had_before_the_edit() {
        let mut state = TextState::new(sample(), false);
        state.toggle(0, ConflictSide::Theirs);
        state.toggle(0, ConflictSide::Ours);
        state.apply_edit(0, "hand".into(), 4);
        assert!(state.is_custom(0));
        // The picks are locked while edited.
        state.toggle(0, ConflictSide::Ours);
        state.swap(0);
        assert_eq!(
            state.resolutions[0],
            ConflictResolution::Custom("hand".into())
        );
        assert_eq!(state.edit_text(0), "hand");
        // (While the text area is open the region's rows are reserved blank.)
        state.set_editing(None);
        assert!(state
            .result
            .iter()
            .any(|l| l.text(&state.conflict) == Some("hand")));
        state.reset(0);
        assert_eq!(
            state.resolutions[0],
            ConflictResolution::Picked(vec![ConflictSide::Theirs, ConflictSide::Ours])
        );
        // A conflict that was unresolved goes back to unresolved.
        state.apply_edit(1, "typed".into(), 4);
        assert!(state.is_custom(1));
        state.reset(1);
        assert_eq!(state.resolutions[1], ConflictResolution::Unresolved);
        // Reset of a conflict that is not edited does nothing.
        state.reset(1);
        assert_eq!(state.resolutions[1], ConflictResolution::Unresolved);
    }

    #[test]
    fn an_empty_hand_edit_resolves_and_deletes_the_region() {
        let mut state = TextState::new(sample(), false);
        state.apply_edit(0, "kept".into(), 4);
        assert!(!can_save(&state.resolutions));
        assert_eq!(state.edit_text(1), "");
        // The unresolved conflict has nothing to compare with: any change is
        // an edit, and an empty one is allowed.
        state.apply_edit(1, String::new(), 4);
        assert_eq!(
            state.resolutions[1],
            ConflictResolution::Custom(String::new())
        );
        assert!(can_save(&state.resolutions));
        assert!(is_dirty(&state.resolutions));
        assert_eq!(
            state.conflict.render(&state.resolutions).unwrap(),
            "a\nkept\nC\ns\n"
        );
    }

    #[test]
    fn a_hand_edit_alone_makes_the_file_dirty() {
        assert!(is_dirty(&[
            ConflictResolution::Unresolved,
            ConflictResolution::Custom(String::new())
        ]));
        assert!(has_custom(&[
            ConflictResolution::Unresolved,
            ConflictResolution::Custom("x".into())
        ]));
        assert!(!has_custom(&[
            ConflictResolution::Unresolved,
            picked(ConflictSide::Ours)
        ]));
    }

    #[test]
    fn take_all_replaces_hand_edits_and_forgets_their_picks() {
        let mut state = TextState::new(sample(), false);
        state.toggle(0, ConflictSide::Ours);
        state.apply_edit(0, "hand".into(), 4);
        state.set_all(take_all(2, ConflictSide::Theirs));
        assert!(!has_custom(&state.resolutions));
        assert_eq!(state.picks, state.resolutions);
        // Reset after take-all has nothing to restore.
        state.reset(0);
        assert_eq!(state.resolutions[0], picked(ConflictSide::Theirs));
    }

    #[test]
    fn a_region_with_a_final_empty_line_survives_an_untouched_edit() {
        assert_eq!(lines_text(&[]), "");
        assert_eq!(lines_text(&["a"]), "a");
        assert_eq!(lines_text(&["a", ""]), "a\n\n");
        assert_eq!(lines_text(&[""]), "\n");
        // What the text area gives back reads as the same lines again.
        let conflict = sample();
        for lines in [vec![], vec!["a"], vec!["a", ""], vec![""], vec!["", ""]] {
            let back: Vec<String> = conflict
                .region_lines(0, &ConflictResolution::Custom(lines_text(&lines)))
                .into_iter()
                .map(|(text, _)| text)
                .collect();
            assert_eq!(back, lines);
        }
    }

    #[test]
    fn the_edit_region_reserves_three_to_twelve_text_lines_of_rows() {
        assert_eq!(edit_rows(""), 4);
        assert_eq!(edit_rows("a\nb\nc"), 4);
        assert_eq!(edit_rows("a\nb\nc\nd\ne"), 5);
        assert_eq!(edit_rows(&"x\n".repeat(40)), 11);

        let conflict = sample();
        let lines = result_lines(
            &conflict,
            &[picked(ConflictSide::Ours), ConflictResolution::Unresolved],
        );
        let edited = with_edit_rows(lines.clone(), 0, 3);
        assert_eq!(edited.len(), lines.len() - 2 + 3);
        let at = edited
            .iter()
            .position(|l| matches!(l, ResultLine::EditRow { .. }))
            .unwrap();
        assert!(matches!(edited[at - 1], ResultLine::Head { index: 0 }));
        assert_eq!(
            edited[at..at + 3],
            [
                ResultLine::EditRow {
                    index: 0,
                    last: false
                },
                ResultLine::EditRow {
                    index: 0,
                    last: false
                },
                ResultLine::EditRow {
                    index: 0,
                    last: true
                },
            ]
        );
        // The other conflict keeps its placeholder; the edited unresolved one
        // swaps its own for the reserved rows.
        assert!(edited
            .iter()
            .any(|l| matches!(l, ResultLine::Placeholder { index: 1 })));
        let edited = with_edit_rows(lines, 1, 4);
        assert!(!edited
            .iter()
            .any(|l| matches!(l, ResultLine::Placeholder { .. })));
    }

    #[test]
    fn the_state_reserves_rows_while_editing_and_drops_them_after() {
        let mut state = TextState::new(sample(), false);
        state.toggle(0, ConflictSide::Ours);
        let plain = state.result.len();
        state.set_editing(Some((0, 5)));
        assert_eq!(state.result.len(), plain - 2 + 5);
        state.apply_edit(0, "one".into(), 4);
        assert_eq!(
            state
                .result
                .iter()
                .filter(|l| matches!(l, ResultLine::EditRow { .. }))
                .count(),
            4
        );
        state.set_editing(None);
        assert!(!state
            .result
            .iter()
            .any(|l| matches!(l, ResultLine::EditRow { .. })));
    }
    #[test]
    fn only_the_status_read_after_a_save_ends_the_wait() {
        // An unrelated status result must not end a save that is still running.
        assert_eq!(SaveState::Running.status_arrived(), SaveState::Running);
        assert_eq!(SaveState::Idle.status_arrived(), SaveState::Idle);
        // After a finished save the next status result ends the wait, whether
        // it was read or failed (the view calls `status_arrived` for both).
        assert_eq!(SaveState::SavedWaiting.status_arrived(), SaveState::Idle);
        assert!(SaveState::Running.is_busy());
        assert!(SaveState::SavedWaiting.is_busy());
        assert!(!SaveState::Idle.is_busy());
    }

    #[test]
    fn typing_rebuilds_only_the_edited_region_and_matches_a_full_rebuild() {
        let full = |state: &TextState| {
            let lines = result_lines(&state.conflict, &state.resolutions);
            let lines = match state.editing {
                Some((index, rows)) => with_edit_rows(lines, index, rows),
                None => lines,
            };
            (result_heads(&lines), lines)
        };
        let mut state = TextState::new(sample(), false);
        // First edit enters the edit (a full rebuild), the rest are keystrokes.
        for (index, text, rows) in [
            (0, "x", 4),
            (0, "x\ny\nz", 4),
            (0, "x\ny\nz\nw\nv\nu", 5),
            (0, "", 4),
            (0, "one\ntwo", 4),
        ] {
            state.apply_edit(index, text.to_owned(), rows);
            let (heads, lines) = full(&state);
            assert_eq!(state.result, lines, "after {text:?}");
            assert_eq!(state.result_heads, heads, "after {text:?}");
        }
        // Another region after the first: its numbers follow the first's size.
        state.set_editing(None);
        for (text, rows) in [("p", 4), ("p\nq\nr\ns", 4), ("", 4)] {
            state.apply_edit(1, text.to_owned(), rows);
            let (heads, lines) = full(&state);
            assert_eq!(state.result, lines, "region 1 after {text:?}");
            assert_eq!(state.result_heads, heads);
        }
    }

    #[test]
    fn the_result_takes_its_conflict_lines_from_the_model() {
        let conflict = sample();
        let resolutions = vec![
            ConflictResolution::Picked(vec![ConflictSide::Theirs, ConflictSide::Ours]),
            ConflictResolution::Custom("typed\nlines".into()),
        ];
        let from_model: Vec<String> = (0..2)
            .flat_map(|index| conflict.region_lines(index, &resolutions[index]))
            .map(|(text, _)| text)
            .collect();
        let lines = result_lines(&conflict, &resolutions);
        let shown: Vec<&str> = lines
            .iter()
            .filter_map(|line| match line {
                ResultLine::Text {
                    source: Source::Custom(text),
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(shown, from_model);
    }

    #[test]
    fn deleting_kinds_know_which_side_has_no_file() {
        assert_eq!(Whole::DeletedByUs.absent_side(), Some(ConflictSide::Ours));
        assert_eq!(
            Whole::DeletedByThem.absent_side(),
            Some(ConflictSide::Theirs)
        );
        assert_eq!(Whole::AddedByUs.absent_side(), Some(ConflictSide::Theirs));
        assert_eq!(Whole::AddedByThem.absent_side(), Some(ConflictSide::Ours));
        assert_eq!(Whole::DeletedByBoth.absent_side(), None);
        assert_eq!(Whole::NotText.absent_side(), None);
    }
}
