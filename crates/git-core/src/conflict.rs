//! The conflict model behind the conflict editor: each side of a conflicted
//! file, the file split into segments over the base, and the text a set of
//! resolutions renders to.
//!
//! Line diffs come from the git CLI (base to each side, histogram); the
//! three-way chunking is plain Rust, written to give the same regions as the
//! file `git merge` (ort) writes to the work tree: git's `ll_merge` runs the
//! histogram diff at the "zealous" level, so conflicts are refined by diffing
//! the two sides against each other (histogram again, see [`diff_lines`]) and
//! conflicts separated by at most three lines are joined. (`git merge-file`
//! uses the stricter "zealous alnum" level, which also joins conflicts
//! separated by lines without letters and digits; `git merge` does not.)

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::ops::Range;
use std::path::{Component, Path};
use std::process::Stdio;

use crate::status::{unmerged, ConflictSide, UNTRACKED_COUNT_CAP};
use crate::{check_output, Error, Repo, Result};

/// How one line of a side ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// `\n`
    Lf,
    /// `\r\n`
    CrLf,
    /// No terminator: only the last line of a file can have this.
    None,
}

/// What kind of conflict a path is in, with the data to resolve it.
// One value per file load, moved straight into the editor: boxing the text
// conflict would only add a deref to every caller.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    Text(TextConflict),
    /// A side has a NUL byte: only whole-side picking is possible.
    Binary,
    /// Not text GitWarp can merge line by line: a side is not valid UTF-8,
    /// or the path goes through a git filter (LFS: the index holds pointers,
    /// not the file's content). Only whole-side picking is possible.
    NotText,
    /// We deleted the file (no stage 2), they changed it.
    DeletedByUs,
    /// They deleted the file (no stage 3), we changed it.
    DeletedByThem,
    /// Added on our side only (stage 2 without a base).
    AddedByUs,
    /// Added on their side only (stage 3 without a base).
    AddedByThem,
    /// Deleted on both sides (a base and nothing else).
    DeletedByBoth,
    /// Either side is a symlink (mode 120000).
    Symlink,
    /// Either side is a submodule (mode 160000).
    Submodule,
}

/// One unmerged index entry of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStage {
    /// 1 base, 2 ours, 3 theirs.
    pub stage: u8,
    pub mode: String,
    pub oid: String,
}

/// What a conflict looked like when it was loaded; [`Repo::save_resolution`]
/// refuses to write when the repository no longer matches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictGuard {
    /// The path's unmerged entries at load.
    pub stages: Vec<IndexStage>,
    /// A hash of the work-tree file at load (`None`: it did not exist).
    pub work_tree_digest: Option<u64>,
    /// The work-tree file differed from the marker file `git merge` writes
    /// (rerere resolved it, or it was edited elsewhere).
    pub changed_after_merge: bool,
}

/// What [`Repo::save_resolution`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written and staged; the conflict is over.
    Saved,
    /// Like `Saved`, but both sides changed the file mode differently: ours
    /// was kept (git would have left the path conflicted).
    SavedModeConflict,
    /// The path is not in the conflict that was loaded (merge aborted,
    /// resolved elsewhere, or the sides changed). Nothing was written.
    StagesChanged,
    /// The work-tree file changed since the load. Nothing was written.
    WorkTreeChanged,
}

/// A text conflict: the three versions and the file cut into segments.
///
/// Lines are stored without their terminator (`\n` or `\r\n`); the
/// `*_endings` vectors hold the terminator of each line, so rendering
/// reproduces the bytes of mixed-ending files and a missing final newline.
/// Only valid UTF-8 gets here (anything else is [`ConflictKind::NotText`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextConflict {
    /// `None` when both sides added the file (an empty base).
    pub base: Option<Vec<String>>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    /// One per base line; empty when `base` is `None`.
    pub base_endings: Vec<LineEnding>,
    pub ours_endings: Vec<LineEnding>,
    pub theirs_endings: Vec<LineEnding>,
    pub segments: Vec<ConflictSegment>,
    pub guard: ConflictGuard,
}

/// One region of the file. All ranges are half-open and index the line
/// vector of their side; `Stable` ranges have equal lengths on all sides.
///
/// A conflict that git's refinement splits into several (common lines inside
/// it become `Same`) cannot attribute base lines to the pieces: the first
/// `Conflict` piece gets the base range of the whole region, the other pieces
/// an empty range (before the first `Conflict`: at the region's start; after
/// it: at its end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictSegment {
    /// Unchanged on both sides.
    Stable {
        base: Range<usize>,
        ours: Range<usize>,
        theirs: Range<usize>,
    },
    /// Changed on one side only: `side` wins without asking.
    Auto {
        side: ConflictSide,
        base: Range<usize>,
        ours: Range<usize>,
        theirs: Range<usize>,
    },
    /// Both sides have the same text here (changed, or equal after git's
    /// refinement of a bigger conflict).
    Same {
        base: Range<usize>,
        ours: Range<usize>,
        theirs: Range<usize>,
    },
    /// Both sides changed the region differently.
    Conflict {
        base: Range<usize>,
        ours: Range<usize>,
        theirs: Range<usize>,
    },
}

/// How one conflict segment is resolved; one per `Conflict`, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    Unresolved,
    /// One or both sides, in pick order (no duplicates; an empty list counts
    /// as unresolved).
    Picked(Vec<ConflictSide>),
    /// Hand-edited text for the region: lines separated by `\n` (a `\r\n`
    /// is accepted), a final newline optional. Every line is written with
    /// the file's line ending, except the last line of the file's last
    /// region when the text has no final newline and neither side had one
    /// (the file then stays unterminated). May be empty (the region is
    /// removed).
    Custom(String),
}

impl TextConflict {
    /// The number of `Conflict` segments, i.e. the length `resolutions` needs.
    pub fn conflict_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|segment| matches!(segment, ConflictSegment::Conflict { .. }))
            .count()
    }

    /// Every conflict unresolved.
    pub fn unresolved(&self) -> Vec<ConflictResolution> {
        vec![ConflictResolution::Unresolved; self.conflict_count()]
    }

    /// The lines `resolution` produces for conflict `conflict_index`, each
    /// with its terminator; empty while the conflict is unresolved. The one
    /// definition of "what a resolution writes": `render`, `render_markers`
    /// and the editor's result pane all go through it.
    pub fn region_lines(
        &self,
        conflict_index: usize,
        resolution: &ConflictResolution,
    ) -> Vec<(String, LineEnding)> {
        let mut seen = 0;
        for (at, segment) in self.segments.iter().enumerate() {
            if let ConflictSegment::Conflict { ours, theirs, .. } = segment {
                if seen == conflict_index {
                    return self.conflict_lines(at, ours, theirs, resolution);
                }
                seen += 1;
            }
        }
        Vec::new()
    }

    /// [`Self::region_lines`] for the conflict at `segments[at]`.
    fn conflict_lines(
        &self,
        at: usize,
        ours: &Range<usize>,
        theirs: &Range<usize>,
        resolution: &ConflictResolution,
    ) -> Vec<(String, LineEnding)> {
        let side_lines = |side: &ConflictSide| -> Vec<(String, LineEnding)> {
            let (lines, endings, range) = match side {
                ConflictSide::Ours => (&self.ours, &self.ours_endings, ours),
                ConflictSide::Theirs => (&self.theirs, &self.theirs_endings, theirs),
            };
            range
                .clone()
                .map(|ix| (lines[ix].clone(), endings[ix]))
                .collect()
        };
        match resolution {
            ConflictResolution::Unresolved => Vec::new(),
            ConflictResolution::Picked(sides) => {
                let mut seen: Vec<ConflictSide> = Vec::new();
                let mut out = Vec::new();
                for side in sides {
                    if seen.contains(side) {
                        continue;
                    }
                    seen.push(*side);
                    out.extend(side_lines(side));
                }
                out
            }
            ConflictResolution::Custom(text) => {
                if text.is_empty() {
                    return Vec::new();
                }
                let default_ending = self.default_ending();
                let body = text.strip_suffix('\n').unwrap_or(text);
                let mut out: Vec<(String, LineEnding)> = body
                    .split('\n')
                    .map(|line| {
                        (
                            line.strip_suffix('\r').unwrap_or(line).to_owned(),
                            default_ending,
                        )
                    })
                    .collect();
                // The file stays unterminated when the typed text is its last
                // content, has no final newline, and neither side had one.
                let unterminated = |endings: &[LineEnding]| {
                    endings
                        .last()
                        .is_none_or(|ending| *ending == LineEnding::None)
                };
                if !text.ends_with('\n')
                    && unterminated(&self.ours_endings)
                    && unterminated(&self.theirs_endings)
                    && self.segments[at + 1..].iter().all(|s| self.is_empty(s))
                {
                    if let Some(last) = out.last_mut() {
                        last.1 = LineEnding::None;
                    }
                }
                out
            }
        }
    }

    /// Whether a segment writes no lines at all.
    fn is_empty(&self, segment: &ConflictSegment) -> bool {
        match segment {
            ConflictSegment::Stable { ours, .. } | ConflictSegment::Same { ours, .. } => {
                ours.is_empty()
            }
            ConflictSegment::Auto {
                side, ours, theirs, ..
            } => match side {
                ConflictSide::Ours => ours.is_empty(),
                ConflictSide::Theirs => theirs.is_empty(),
            },
            ConflictSegment::Conflict { .. } => false,
        }
    }

    /// The file text for `resolutions`, or `None` while a conflict is
    /// unresolved (or `resolutions` has the wrong length).
    pub fn render(&self, resolutions: &[ConflictResolution]) -> Option<String> {
        if resolutions.len() != self.conflict_count() {
            return None;
        }
        let mut out = Emitter::default();
        let mut next = resolutions.iter();
        for (at, segment) in self.segments.iter().enumerate() {
            match segment {
                ConflictSegment::Conflict { ours, theirs, .. } => {
                    let resolution = next.next()?;
                    if is_open(resolution) {
                        return None;
                    }
                    for (line, ending) in self.conflict_lines(at, ours, theirs, resolution) {
                        out.line(&line, ending);
                    }
                }
                other => self.emit_plain(&mut out, other),
            }
        }
        Some(out.finish())
    }

    /// The file as git writes it for an unresolved merge
    /// (`merge.conflictStyle=merge`): every conflict as a marker block
    /// `<<<<<<< ours_label` / `=======` / `>>>>>>> theirs_label`. Marker lines
    /// end with CRLF when the lines around the conflict do, like git's.
    pub fn render_markers(&self, ours_label: &str, theirs_label: &str) -> String {
        let mut out = Emitter::default();
        for segment in &self.segments {
            let ConflictSegment::Conflict { ours, theirs, .. } = segment else {
                self.emit_plain(&mut out, segment);
                continue;
            };
            let cr = self.markers_need_cr(ours, theirs);
            out.marker("<<<<<<<", ours_label, cr);
            out.side(&self.ours, &self.ours_endings, ours);
            out.close(cr);
            out.marker("=======", "", cr);
            out.side(&self.theirs, &self.theirs_endings, theirs);
            out.close(cr);
            out.marker(">>>>>>>", theirs_label, cr);
        }
        out.finish()
    }

    /// Writes a segment that is not a conflict.
    fn emit_plain(&self, out: &mut Emitter, segment: &ConflictSegment) {
        match segment {
            ConflictSegment::Stable { ours, .. } | ConflictSegment::Same { ours, .. } => {
                out.side(&self.ours, &self.ours_endings, ours);
            }
            ConflictSegment::Auto {
                side, ours, theirs, ..
            } => match side {
                ConflictSide::Ours => out.side(&self.ours, &self.ours_endings, ours),
                ConflictSide::Theirs => out.side(&self.theirs, &self.theirs_endings, theirs),
            },
            ConflictSegment::Conflict { .. } => {}
        }
    }

    /// git's `is_cr_needed`: the marker lines of a conflict are CRLF when the
    /// line before it (or the first line) is CRLF in both ours and theirs,
    /// and the base's first line is CRLF too.
    fn markers_need_cr(&self, ours: &Range<usize>, theirs: &Range<usize>) -> bool {
        // `Some(false)` LF, `Some(true)` CRLF, `None` cannot tell.
        fn eol_crlf(endings: &[LineEnding], i: usize) -> Option<bool> {
            if endings.is_empty() {
                return None;
            }
            let i = i.min(endings.len() - 1);
            let crlf = |ending: LineEnding| ending == LineEnding::CrLf;
            if i + 1 < endings.len() || endings[i] != LineEnding::None {
                return Some(crlf(endings[i]));
            }
            if i == 0 {
                return None;
            }
            Some(crlf(endings[i - 1]))
        }
        let before = |range: &Range<usize>| range.start.saturating_sub(1);
        // git tests each answer for "truthy", which includes "cannot tell";
        // the last answer stands, and "cannot tell" ends up as LF.
        let mut answer = eol_crlf(&self.ours_endings, before(ours));
        if answer != Some(false) {
            answer = eol_crlf(&self.theirs_endings, before(theirs));
        }
        if answer != Some(false) {
            answer = eol_crlf(&self.base_endings, 0);
        }
        answer == Some(true)
    }

    /// The ending used for lines the user typed: CRLF when every terminated
    /// line of ours (or, without any, of theirs) is CRLF, else LF.
    fn default_ending(&self) -> LineEnding {
        for endings in [&self.ours_endings, &self.theirs_endings] {
            let terminated = endings.iter().filter(|e| **e != LineEnding::None);
            let (mut crlf, mut lf) = (0, 0);
            for ending in terminated {
                match ending {
                    LineEnding::CrLf => crlf += 1,
                    _ => lf += 1,
                }
            }
            if crlf + lf > 0 {
                return if lf == 0 {
                    LineEnding::CrLf
                } else {
                    LineEnding::Lf
                };
            }
        }
        LineEnding::Lf
    }

    /// Builds the model from the three sides and the line diffs base→ours
    /// and base→theirs.
    fn from_sides(
        base: Option<SideText>,
        ours: SideText,
        theirs: SideText,
        ours_hunks: &[DiffHunk],
        theirs_hunks: &[DiffHunk],
    ) -> Self {
        let empty = SideText::default();
        let segments = build_segments(
            &base.as_ref().unwrap_or(&empty).keys(),
            &ours.keys(),
            &theirs.keys(),
            ours_hunks,
            theirs_hunks,
        );
        Self {
            base_endings: base
                .as_ref()
                .map(|side| side.endings.clone())
                .unwrap_or_default(),
            base: base.map(|side| side.lines),
            ours: ours.lines,
            theirs: theirs.lines,
            ours_endings: ours.endings,
            theirs_endings: theirs.endings,
            segments,
            guard: ConflictGuard::default(),
        }
    }
}

/// Whether `resolution` leaves its conflict open.
fn is_open(resolution: &ConflictResolution) -> bool {
    match resolution {
        ConflictResolution::Unresolved => true,
        ConflictResolution::Picked(sides) => sides.is_empty(),
        ConflictResolution::Custom(_) => false,
    }
}

/// Appends lines to the result text. A line without a terminator that is
/// followed by more text gets one, like git does for a union merge.
#[derive(Default)]
struct Emitter {
    out: String,
    /// The last line had no terminator yet.
    open: bool,
    /// The last terminated line was CRLF.
    crlf: bool,
}

impl Emitter {
    fn line(&mut self, text: &str, ending: LineEnding) {
        self.close(self.crlf);
        self.out.push_str(text);
        match ending {
            LineEnding::Lf => {
                self.out.push('\n');
                self.crlf = false;
            }
            LineEnding::CrLf => {
                self.out.push_str("\r\n");
                self.crlf = true;
            }
            LineEnding::None => self.open = true,
        }
    }

    /// Terminates an unterminated last line.
    fn close(&mut self, crlf: bool) {
        if self.open {
            self.out.push_str(if crlf { "\r\n" } else { "\n" });
            self.open = false;
        }
    }

    /// A conflict marker line (`label` empty: no label, no space).
    fn marker(&mut self, marker: &str, label: &str, crlf: bool) {
        self.close(crlf);
        self.out.push_str(marker);
        if !label.is_empty() {
            self.out.push(' ');
            self.out.push_str(label);
        }
        self.out.push_str(if crlf { "\r\n" } else { "\n" });
    }

    fn side(&mut self, lines: &[String], endings: &[LineEnding], range: &Range<usize>) {
        for index in range.clone() {
            self.line(&lines[index], endings[index]);
        }
    }

    fn finish(self) -> String {
        self.out
    }
}

/// A side's lines and their terminators.
#[derive(Debug, Default, Clone)]
struct SideText {
    lines: Vec<String>,
    endings: Vec<LineEnding>,
}

impl SideText {
    /// Splits on `\n`; a `\r` before it is the CRLF ending, not line text.
    fn parse(text: &str) -> Self {
        let mut side = Self::default();
        for line in text.split_inclusive('\n') {
            let (content, ending) = match line.strip_suffix('\n') {
                Some(content) => match content.strip_suffix('\r') {
                    Some(content) => (content, LineEnding::CrLf),
                    None => (content, LineEnding::Lf),
                },
                None => (line, LineEnding::None),
            };
            side.lines.push(content.to_string());
            side.endings.push(ending);
        }
        side
    }

    fn keys(&self) -> Vec<Line<'_>> {
        self.lines
            .iter()
            .zip(&self.endings)
            .map(|(text, ending)| Line {
                text,
                ending: *ending,
            })
            .collect()
    }
}

/// A line as git compares it: the text together with its terminator
/// (`a\r\n` differs from `a\n`, a last line without newline from one with).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Line<'a> {
    text: &'a str,
    ending: LineEnding,
}

/// One change of a side against the base, as ranges of base and side lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffHunk {
    pub base: Range<usize>,
    pub side: Range<usize>,
}

/// The classic diff3 region walk over two hunk lists (both sorted by base
/// position), followed by git's refinement and simplification of conflicts.
fn build_segments(
    base: &[Line<'_>],
    ours: &[Line<'_>],
    theirs: &[Line<'_>],
    ours_hunks: &[DiffHunk],
    theirs_hunks: &[DiffHunk],
) -> Vec<ConflictSegment> {
    let mut segments = Vec::new();
    let (mut oi, mut ti) = (0, 0);
    // Where the next base line is, and how far ours/theirs are shifted from
    // the base by the hunks before it.
    let mut pos = 0usize;
    let (mut ours_shift, mut theirs_shift) = (0isize, 0isize);
    let shifted = |index: usize, shift: isize| (index as isize + shift) as usize;

    while oi < ours_hunks.len() || ti < theirs_hunks.len() {
        let start = match (ours_hunks.get(oi), theirs_hunks.get(ti)) {
            (Some(o), Some(t)) => o.base.start.min(t.base.start),
            (Some(o), None) => o.base.start,
            (None, Some(t)) => t.base.start,
            (None, None) => unreachable!(),
        };
        if start > pos {
            segments.push(ConflictSegment::Stable {
                base: pos..start,
                ours: shifted(pos, ours_shift)..shifted(start, ours_shift),
                theirs: shifted(pos, theirs_shift)..shifted(start, theirs_shift),
            });
        }
        // Grow the region over every hunk that overlaps or touches it
        // (git: two changes on adjacent lines conflict).
        let (ours_before, theirs_before) = (ours_shift, theirs_shift);
        let mut end = start;
        let (mut ours_used, mut theirs_used) = (false, false);
        loop {
            let mut grew = false;
            if let Some(hunk) = ours_hunks.get(oi) {
                if hunk.base.start <= end.max(start) {
                    end = end.max(hunk.base.end);
                    ours_shift += hunk.side.len() as isize - hunk.base.len() as isize;
                    ours_used = true;
                    oi += 1;
                    grew = true;
                }
            }
            if let Some(hunk) = theirs_hunks.get(ti) {
                if hunk.base.start <= end.max(start) {
                    end = end.max(hunk.base.end);
                    theirs_shift += hunk.side.len() as isize - hunk.base.len() as isize;
                    theirs_used = true;
                    ti += 1;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let base_range = start..end;
        let ours_range = shifted(start, ours_before)..shifted(end, ours_shift);
        let theirs_range = shifted(start, theirs_before)..shifted(end, theirs_shift);
        segments.push(match (ours_used, theirs_used) {
            (true, true) => {
                if ours[ours_range.clone()] == theirs[theirs_range.clone()] {
                    ConflictSegment::Same {
                        base: base_range,
                        ours: ours_range,
                        theirs: theirs_range,
                    }
                } else {
                    ConflictSegment::Conflict {
                        base: base_range,
                        ours: ours_range,
                        theirs: theirs_range,
                    }
                }
            }
            (true, false) => ConflictSegment::Auto {
                side: ConflictSide::Ours,
                base: base_range,
                ours: ours_range,
                theirs: theirs_range,
            },
            _ => ConflictSegment::Auto {
                side: ConflictSide::Theirs,
                base: base_range,
                ours: ours_range,
                theirs: theirs_range,
            },
        });
        pos = end;
    }
    if pos < base.len() {
        segments.push(ConflictSegment::Stable {
            base: pos..base.len(),
            ours: shifted(pos, ours_shift)..ours.len(),
            theirs: shifted(pos, theirs_shift)..theirs.len(),
        });
    }
    let segments = refine_conflicts(segments, ours, theirs);
    simplify_conflicts(segments, ours)
}

/// Above this many line pairs the classic fallback of [`diff_lines`] does not
/// diff a region (the table would be too big) and keeps it as one change.
const REFINE_CELL_CAP: usize = 4_000_000;

/// Splits each conflict along the lines its two sides share: git diffs the
/// two sides against each other and only the differing parts stay in
/// conflict.
fn refine_conflicts(
    segments: Vec<ConflictSegment>,
    ours: &[Line<'_>],
    theirs: &[Line<'_>],
) -> Vec<ConflictSegment> {
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let ConflictSegment::Conflict {
            base,
            ours: o,
            theirs: t,
        } = &segment
        else {
            out.push(segment);
            continue;
        };
        let changes = diff_lines(&ours[o.clone()], &theirs[t.clone()]);
        // The whole base range goes to the first `Conflict` piece; the other
        // pieces get an empty range (`Same` ones before it at the start of the
        // region, after it at the end). No change at all: one `Same` piece.
        let mut base_for_conflict = Some(base.clone());
        let mut after_conflict = false;
        let mut piece_base = |conflict: bool| {
            if conflict {
                after_conflict = true;
                base_for_conflict.take().unwrap_or(base.end..base.end)
            } else if after_conflict {
                base.end..base.end
            } else {
                base.start..base.start
            }
        };
        let (mut oc, mut tc) = (o.start, t.start);
        for (co, ct) in changes {
            let (co, ct) = (
                o.start + co.start..o.start + co.end,
                t.start + ct.start..t.start + ct.end,
            );
            if co.start > oc {
                out.push(ConflictSegment::Same {
                    base: piece_base(false),
                    ours: oc..co.start,
                    theirs: tc..ct.start,
                });
            }
            oc = co.end;
            tc = ct.end;
            out.push(ConflictSegment::Conflict {
                base: piece_base(true),
                ours: co,
                theirs: ct,
            });
        }
        if oc < o.end || !after_conflict {
            let base = if after_conflict {
                base.end..base.end
            } else {
                base.clone()
            };
            out.push(ConflictSegment::Same {
                base,
                ours: oc..o.end,
                theirs: tc..t.end,
            });
        }
    }
    out
}

/// Joins conflicts that are close together, like git's zealous level: when
/// at most three lines lie between two conflicts they are one conflict. Only
/// unchanged and `Same` text may lie between; a one-sided change keeps
/// conflicts apart. (`git merge-file`'s "zealous alnum" level also joins over
/// any number of lines without letters and digits; `git merge` does not.)
fn simplify_conflicts(segments: Vec<ConflictSegment>, ours: &[Line<'_>]) -> Vec<ConflictSegment> {
    let mut out: Vec<ConflictSegment> = Vec::with_capacity(segments.len());
    for segment in segments {
        let ConflictSegment::Conflict {
            base,
            ours: o,
            theirs: t,
        } = &segment
        else {
            out.push(segment);
            continue;
        };
        let previous = out.iter().rposition(|s| {
            !matches!(
                s,
                ConflictSegment::Stable { .. } | ConflictSegment::Same { .. }
            )
        });
        let joinable =
            previous.filter(|&index| matches!(out[index], ConflictSegment::Conflict { .. }));
        if let Some(index) = joinable {
            let ConflictSegment::Conflict {
                base: pb,
                ours: po,
                theirs: pt,
            } = &out[index]
            else {
                unreachable!()
            };
            if ours[po.end..o.start].len() <= 3 {
                let joined = ConflictSegment::Conflict {
                    base: pb.start.min(base.start)..pb.end.max(base.end),
                    ours: po.start..o.end,
                    theirs: pt.start..t.end,
                };
                out.truncate(index);
                out.push(joined);
                continue;
            }
        }
        out.push(segment);
    }
    out
}

/// The changes turning `a` into `b`, as ranges of `a` and of `b`: git's
/// histogram diff (`xdl_do_histogram_diff`) followed by the same sliding of
/// changes (`xdl_change_compact`, without the indent heuristic) that git
/// applies, so a change inside a run of equal lines lands where git puts it.
/// Regions where every common line is too frequent for the histogram fall
/// back to a plain LCS diff (git: Myers).
fn diff_lines(a: &[Line<'_>], b: &[Line<'_>]) -> Vec<(Range<usize>, Range<usize>)> {
    let (mut changed_a, mut changed_b) = (vec![false; a.len()], vec![false; b.len()]);
    histogram(a, b, 0..a.len(), 0..b.len(), &mut changed_a, &mut changed_b);
    compact(a, &mut changed_a, &changed_b);
    compact(b, &mut changed_b, &changed_a);
    let mut changes = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let (ca, cb) = (i < a.len() && changed_a[i], j < b.len() && changed_b[j]);
        if !ca && !cb {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while i < a.len() && changed_a[i] {
            i += 1;
        }
        while j < b.len() && changed_b[j] {
            j += 1;
        }
        changes.push((si..i, sj..j));
    }
    changes
}

/// A line of `a` may occur at most this often for the histogram to anchor on
/// it (git: `MAX_CHAIN_LENGTH`).
const HISTOGRAM_CHAIN: usize = 64;

/// A common run found by the histogram: `a[a_start..=a_end]` equals
/// `b[b_start..=b_end]` (inclusive ends, like git's region).
struct Region {
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
}

/// git's `histogram_diff`: marks the lines of `a[ar]` / `b[br]` that are
/// not part of the longest low-frequency common run, recursing on both sides
/// of that run.
fn histogram(
    a: &[Line<'_>],
    b: &[Line<'_>],
    mut ar: Range<usize>,
    mut br: Range<usize>,
    changed_a: &mut [bool],
    changed_b: &mut [bool],
) {
    loop {
        if ar.is_empty() && br.is_empty() {
            return;
        }
        if ar.is_empty() {
            changed_b[br.clone()].fill(true);
            return;
        }
        if br.is_empty() {
            changed_a[ar.clone()].fill(true);
            return;
        }
        match find_region(a, b, &ar, &br) {
            Lcs::Fallback => {
                for (ca, cb) in classic_diff(&a[ar.clone()], &b[br.clone()]) {
                    changed_a[ar.start + ca.start..ar.start + ca.end].fill(true);
                    changed_b[br.start + cb.start..br.start + cb.end].fill(true);
                }
                return;
            }
            Lcs::None => {
                changed_a[ar.clone()].fill(true);
                changed_b[br.clone()].fill(true);
                return;
            }
            Lcs::Found(region) => {
                histogram(
                    a,
                    b,
                    ar.start..region.a_start,
                    br.start..region.b_start,
                    changed_a,
                    changed_b,
                );
                ar = region.a_end + 1..ar.end;
                br = region.b_end + 1..br.end;
            }
        }
    }
}

enum Lcs {
    Found(Region),
    /// No line of `a` occurs in `b`.
    None,
    /// Common lines exist but all occur too often: use the classic diff.
    Fallback,
}

/// git's `find_lcs` over `a[ar]` and `b[br]`.
fn find_region(a: &[Line<'_>], b: &[Line<'_>], ar: &Range<usize>, br: &Range<usize>) -> Lcs {
    // One record per distinct line of `a[ar]`: its occurrence count (capped)
    // and its first position; `next[i - ar.start]` chains the occurrences.
    const NONE: usize = usize::MAX;
    struct Record {
        count: usize,
        first: usize,
    }
    let mut records: Vec<Record> = Vec::new();
    let mut by_line: HashMap<Line<'_>, usize> = HashMap::new();
    let mut record_of = vec![0usize; ar.len()];
    let mut next = vec![NONE; ar.len()];
    for ptr in ar.clone().rev() {
        let id = *by_line.entry(a[ptr]).or_insert_with(|| {
            records.push(Record {
                count: 0,
                first: NONE,
            });
            records.len() - 1
        });
        let record = &mut records[id];
        next[ptr - ar.start] = record.first;
        record.first = ptr;
        record.count = (record.count + 1).min(HISTOGRAM_CHAIN);
        record_of[ptr - ar.start] = id;
    }
    let count_at = |ptr: usize| records[record_of[ptr - ar.start]].count;

    let mut best: Option<Region> = None;
    let mut cnt = HISTOGRAM_CHAIN + 1;
    let mut has_common = false;
    let mut b_ptr = br.start;
    while b_ptr < br.end {
        let mut b_next = b_ptr + 1;
        if let Some(&id) = by_line.get(&b[b_ptr]) {
            let record = &records[id];
            if record.count > cnt {
                has_common = true;
            } else {
                has_common = true;
                let mut a_ptr = record.first;
                loop {
                    let mut rc = record.count;
                    let (mut a_start, mut b_start) = (a_ptr, b_ptr);
                    let (mut a_end, mut b_end) = (a_ptr, b_ptr);
                    while ar.start < a_start
                        && br.start < b_start
                        && a[a_start - 1] == b[b_start - 1]
                    {
                        a_start -= 1;
                        b_start -= 1;
                        if rc > 1 {
                            rc = rc.min(count_at(a_start));
                        }
                    }
                    while a_end + 1 < ar.end && b_end + 1 < br.end && a[a_end + 1] == b[b_end + 1] {
                        a_end += 1;
                        b_end += 1;
                        if rc > 1 {
                            rc = rc.min(count_at(a_end));
                        }
                    }
                    if b_next <= b_end {
                        b_next = b_end + 1;
                    }
                    let best_len = best.as_ref().map_or(0, |r| r.a_end - r.a_start);
                    if best_len < a_end - a_start || rc < cnt {
                        best = Some(Region {
                            a_start,
                            a_end,
                            b_start,
                            b_end,
                        });
                        cnt = rc;
                    }
                    // Skip the occurrences inside the run just found.
                    let mut np = next[a_ptr - ar.start];
                    while np != NONE && np <= a_end {
                        np = next[np - ar.start];
                    }
                    if np == NONE {
                        break;
                    }
                    a_ptr = np;
                }
            }
        }
        b_ptr = b_next;
    }
    match best {
        Some(region) => Lcs::Found(region),
        None if has_common && HISTOGRAM_CHAIN < cnt => Lcs::Fallback,
        None => Lcs::None,
    }
}

/// A plain LCS diff of two slices (the stand-in for git's Myers fallback).
fn classic_diff(a: &[Line<'_>], b: &[Line<'_>]) -> Vec<(Range<usize>, Range<usize>)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let am = &a[prefix..a.len() - suffix];
    let bm = &b[prefix..b.len() - suffix];
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }
    let whole = vec![(prefix..prefix + am.len(), prefix..prefix + bm.len())];
    if am.is_empty() || bm.is_empty() || am.len().saturating_mul(bm.len()) > REFINE_CELL_CAP {
        return whole;
    }
    let (n, m) = (am.len(), bm.len());
    let width = m + 1;
    // lcs[i * width + j]: length of the longest common subsequence of
    // am[i..] and bm[j..].
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if am[i] == bm[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let mut changes = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<(usize, usize)> = None;
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            if let Some((si, sj)) = open.take() {
                changes.push((prefix + si..prefix + i, prefix + sj..prefix + j));
            }
            i += 1;
            j += 1;
            continue;
        }
        open.get_or_insert((i, j));
        if j == m || (i < n && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
            i += 1;
        } else {
            j += 1;
        }
    }
    if let Some((si, sj)) = open {
        changes.push((prefix + si..prefix + n, prefix + sj..prefix + m));
    }
    changes
}

/// A run of changed lines `start..end` of one file.
#[derive(Clone, Copy)]
struct Group {
    start: usize,
    end: usize,
}

fn flag(changed: &[bool], i: usize) -> bool {
    changed.get(i).copied().unwrap_or(false)
}

fn group_init(changed: &[bool]) -> Group {
    let mut end = 0;
    while flag(changed, end) {
        end += 1;
    }
    Group { start: 0, end }
}

/// Moves to the next group; `false` when there is none.
fn group_next(changed: &[bool], g: &mut Group) -> bool {
    if g.end == changed.len() {
        return false;
    }
    g.start = g.end + 1;
    g.end = g.start;
    while flag(changed, g.end) {
        g.end += 1;
    }
    true
}

fn group_previous(changed: &[bool], g: &mut Group) -> bool {
    if g.start == 0 {
        return false;
    }
    g.end = g.start - 1;
    g.start = g.end;
    while g.start > 0 && changed[g.start - 1] {
        g.start -= 1;
    }
    true
}

fn slide_up(lines: &[Line<'_>], changed: &mut [bool], g: &mut Group) -> bool {
    if g.start > 0 && lines[g.start - 1] == lines[g.end - 1] {
        g.start -= 1;
        g.end -= 1;
        changed[g.start] = true;
        changed[g.end] = false;
        while g.start > 0 && changed[g.start - 1] {
            g.start -= 1;
        }
        true
    } else {
        false
    }
}

fn slide_down(lines: &[Line<'_>], changed: &mut [bool], g: &mut Group) -> bool {
    if g.end < lines.len() && lines[g.start] == lines[g.end] {
        changed[g.start] = false;
        changed[g.end] = true;
        g.start += 1;
        g.end += 1;
        while flag(changed, g.end) {
            g.end += 1;
        }
        true
    } else {
        false
    }
}

/// git's `xdl_change_compact` for one file: slides every group of changed
/// lines of `lines` as far down as it goes, merging with groups it meets,
/// then back up to line up with the last group of the other file it can
/// match.
fn compact(lines: &[Line<'_>], changed: &mut [bool], other: &[bool]) {
    let mut g = group_init(changed);
    let mut go = group_init(other);
    loop {
        if g.end != g.start {
            let mut end_matching_other;
            let mut earliest_end;
            loop {
                let group_size = g.end - g.start;
                end_matching_other = None::<usize>;
                while slide_up(lines, changed, &mut g) {
                    group_previous(other, &mut go);
                }
                earliest_end = g.end;
                if go.end > go.start {
                    end_matching_other = Some(g.end);
                }
                while slide_down(lines, changed, &mut g) {
                    group_next(other, &mut go);
                    if go.end > go.start {
                        end_matching_other = Some(g.end);
                    }
                }
                if group_size == g.end - g.start {
                    break;
                }
            }
            if g.end != earliest_end && end_matching_other.is_some() {
                while go.end == go.start {
                    if !slide_up(lines, changed, &mut g) || !group_previous(other, &mut go) {
                        break;
                    }
                }
            }
        }
        if !group_next(changed, &mut g) {
            break;
        }
        group_next(other, &mut go);
    }
}

// ---------------------------------------------------------------------------
// Reading a conflicted path from the repository
// ---------------------------------------------------------------------------

/// The blobs `oids` through one `git cat-file --batch` process, in order;
/// one over the display cap is [`Error::TooLarge`].
fn read_blobs(repo: &Repo, oids: &[&str]) -> Result<Vec<Vec<u8>>> {
    let mut child = repo
        .command()
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::Spawn)?;
    // A handful of object names: far below the pipe buffer, so writing them
    // all before reading cannot block.
    let mut stdin = child.stdin.take().ok_or(Error::Parse("cat-file stdin"))?;
    let request: String = oids.iter().map(|oid| format!("{oid}\n")).collect();
    let written = stdin.write_all(request.as_bytes());
    drop(stdin);
    written.map_err(Error::Io)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or(Error::Parse("cat-file stdout"))?);
    let mut blobs = Vec::with_capacity(oids.len());
    for _ in oids {
        // `<oid> SP <type> SP <size> LF` (or `<oid> SP missing LF`).
        let mut header = String::new();
        stdout.read_line(&mut header).map_err(Error::Io)?;
        let mut fields = header.split_whitespace().skip(1);
        let (Some("blob"), Some(size)) = (fields.next(), fields.next()) else {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Parse("cat-file --batch header"));
        };
        let size: u64 = size.parse().map_err(|_| Error::Parse("cat-file size"))?;
        if size > UNTRACKED_COUNT_CAP {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::TooLarge { what: "diff" });
        }
        let mut bytes = vec![0u8; size as usize];
        stdout.read_exact(&mut bytes).map_err(Error::Io)?;
        // The LF after the content.
        let mut lf = [0u8; 1];
        stdout.read_exact(&mut lf).map_err(Error::Io)?;
        blobs.push(bytes);
    }
    drop(stdout);
    let out = child.wait_with_output().map_err(Error::Io)?;
    check_output(out, "cat-file --batch")?;
    Ok(blobs)
}

/// The line changes from blob `base` to blob `side`, from `git diff -U0`
/// with everything that could vary with user configuration pinned. The
/// histogram algorithm is the one `git merge` merges file contents with.
fn blob_hunks(repo: &Repo, base: &IndexStage, side: &IndexStage) -> Result<Vec<DiffHunk>> {
    if base.oid == side.oid {
        return Ok(Vec::new());
    }
    let out = repo
        .command()
        .args([
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--text",
            "--diff-algorithm=histogram",
            "--no-indent-heuristic",
            "--inter-hunk-context=0",
        ])
        .arg(&base.oid)
        .arg(&side.oid)
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "diff")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut hunks = Vec::new();
    for line in text.lines().filter(|line| line.starts_with("@@ ")) {
        let (old_start, old_count, new_start, new_count) =
            crate::diff::parse_hunk_header(line).ok_or(Error::Parse("diff hunk header"))?;
        // `-U0` numbers an empty range by the line before it.
        let start = |start: u32, count: u32| (if count == 0 { start } else { start - 1 }) as usize;
        let (base_start, side_start) = (start(old_start, old_count), start(new_start, new_count));
        hunks.push(DiffHunk {
            base: base_start..base_start + old_count as usize,
            side: side_start..side_start + new_count as usize,
        });
    }
    Ok(hunks)
}

/// Whether a git filter (LFS, ...) is set for `path`: the index then holds
/// the clean form (or a pointer), not what the work tree shows.
fn has_filter(repo: &Repo, path: &str) -> Result<bool> {
    let out = repo
        .command()
        .args(["check-attr", "-z", "filter", "--"])
        .arg(path)
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "check-attr")?;
    // `<path> NUL <attr> NUL <value> NUL`
    let value = out.stdout.split(|&byte| byte == 0).nth(2).unwrap_or(b"");
    Ok(!matches!(value, b"" | b"unspecified" | b"unset"))
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Brings marker lines to a label-free, LF-only form so the file `git merge`
/// wrote can be compared with [`TextConflict::render_markers`] whatever the
/// labels (merge, rebase, cherry-pick, squash all name the sides differently)
/// and the work-tree line endings (`core.autocrlf`, `eol` attributes) are.
fn normalize_markers(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| {
            if line.starts_with("<<<<<<<") {
                "<<<<<<<"
            } else if line.starts_with(">>>>>>>") {
                ">>>>>>>"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The two sides of a marker file: the text with every conflict block
/// replaced by its ours part, and by its theirs part. The `|||||||` base
/// sections of `diff3`/`zdiff3` are dropped. `zdiff3` moves lines common to
/// both sides out of the blocks, which leaves both results unchanged, so they
/// compare equal whatever the conflict style.
fn marker_sides(text: &str) -> (String, String) {
    let (mut ours, mut theirs) = (String::new(), String::new());
    // 0 outside, 1 ours, 2 base, 3 theirs
    let mut state = 0;
    for line in text.replace("\r\n", "\n").split_inclusive('\n') {
        match state {
            0 if line.starts_with("<<<<<<<") => state = 1,
            1 | 2 if line.starts_with("|||||||") => state = 2,
            1 | 2 if line.starts_with("=======") => state = 3,
            3 if line.starts_with(">>>>>>>") => state = 0,
            1 => ours.push_str(line),
            3 => theirs.push_str(line),
            0 => {
                ours.push_str(line);
                theirs.push_str(line);
            }
            _ => {}
        }
    }
    (ours, theirs)
}

/// Whether the file git left in the work tree is not what an unresolved merge
/// writes (rerere, an IDE, a half-done manual resolution). For the default
/// `merge` style the marker file is compared as a whole; for `diff3` and
/// `zdiff3` (different block layout) by what each side's text comes to.
fn changed_after_merge(repo: &Repo, model: &TextConflict, disk: Option<&[u8]>) -> bool {
    let style = repo
        .command()
        .args(["config", "--get", "merge.conflictStyle"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default();
    let Some(disk) = disk else {
        return true;
    };
    let disk = String::from_utf8_lossy(disk);
    let expected = model.render_markers("", "");
    if style.is_empty() || style == "merge" {
        normalize_markers(&disk) != normalize_markers(&expected)
    } else {
        marker_sides(&disk) != marker_sides(&expected)
    }
}

/// Loads the conflict at `path` (repo-relative, literal).
pub(crate) fn conflict(repo: &Repo, path: &str) -> Result<ConflictKind> {
    let stages = unmerged(repo, path)?;
    let entry = |stage: u8| stages.iter().find(|entry| entry.stage == stage).cloned();
    let (base, ours, theirs) = (entry(1), entry(2), entry(3));
    if stages.is_empty() {
        return Err(Error::Parse("path has no conflict entries"));
    }
    let mode_of = |entry: &Option<IndexStage>, mode: &str| {
        entry.as_ref().is_some_and(|entry| entry.mode == mode)
    };
    if mode_of(&ours, "120000") || mode_of(&theirs, "120000") {
        return Ok(ConflictKind::Symlink);
    }
    if mode_of(&ours, "160000") || mode_of(&theirs, "160000") {
        return Ok(ConflictKind::Submodule);
    }
    let (ours, theirs) = match (ours, theirs) {
        (Some(ours), Some(theirs)) => (ours, theirs),
        (Some(_), None) if base.is_some() => return Ok(ConflictKind::DeletedByThem),
        (Some(_), None) => return Ok(ConflictKind::AddedByUs),
        (None, Some(_)) if base.is_some() => return Ok(ConflictKind::DeletedByUs),
        (None, Some(_)) => return Ok(ConflictKind::AddedByThem),
        (None, None) => return Ok(ConflictKind::DeletedByBoth),
    };
    if has_filter(repo, path)? {
        return Ok(ConflictKind::NotText);
    }
    let mut oids = vec![ours.oid.as_str(), theirs.oid.as_str()];
    oids.extend(base.as_ref().map(|base| base.oid.as_str()));
    let mut blobs = read_blobs(repo, &oids)?.into_iter();
    let (ours_bytes, theirs_bytes) = (blobs.next().unwrap(), blobs.next().unwrap());
    let base_bytes = blobs.next();
    let binary = |bytes: &[u8]| bytes.contains(&0);
    if binary(&ours_bytes) || binary(&theirs_bytes) || base_bytes.as_deref().is_some_and(binary) {
        return Ok(ConflictKind::Binary);
    }
    // Invalid UTF-8 would be rewritten as U+FFFD on save: not editable as text.
    let utf8 = |bytes: Vec<u8>| String::from_utf8(bytes).ok();
    let (Some(ours_text), Some(theirs_text)) = (utf8(ours_bytes), utf8(theirs_bytes)) else {
        return Ok(ConflictKind::NotText);
    };
    let base_text = match base_bytes {
        Some(bytes) => match utf8(bytes) {
            Some(text) => Some(text),
            None => return Ok(ConflictKind::NotText),
        },
        None => None,
    };
    let (ours_side, theirs_side) = (SideText::parse(&ours_text), SideText::parse(&theirs_text));
    let base_side = base_text.as_deref().map(SideText::parse);
    let (ours_hunks, theirs_hunks) = match &base {
        Some(base) => (
            blob_hunks(repo, base, &ours)?,
            blob_hunks(repo, base, &theirs)?,
        ),
        // Both added the file: an empty base, every line an insertion.
        None => (insertion(&ours_side), insertion(&theirs_side)),
    };
    let mut model = TextConflict::from_sides(
        base_side,
        ours_side,
        theirs_side,
        &ours_hunks,
        &theirs_hunks,
    );
    let disk = fs::read(repo.root().join(path)).ok();
    model.guard = ConflictGuard {
        stages,
        work_tree_digest: disk.as_deref().map(digest),
        changed_after_merge: changed_after_merge(repo, &model, disk.as_deref()),
    };
    Ok(ConflictKind::Text(model))
}

/// The hunk list of a side against an empty base.
fn insertion(side: &SideText) -> Vec<DiffHunk> {
    if side.lines.is_empty() {
        Vec::new()
    } else {
        vec![DiffHunk {
            base: 0..0,
            side: 0..side.lines.len(),
        }]
    }
}

/// Writes `content` (the clean, index form of the resolved file) as the
/// resolution of `path` and stages it, which ends the conflict for that path.
///
/// Goes through git like a manual resolution does: the content is hashed
/// without filters, put in the index (clearing the conflict stages) and
/// checked out, so smudge filters, `eol` and `core.autocrlf` shape the
/// work-tree file. Nothing is written when the path is no longer in the
/// conflict `guard` describes, or its work-tree file changed since.
pub(crate) fn save_resolution(
    repo: &Repo,
    path: &str,
    content: &str,
    guard: &ConflictGuard,
) -> Result<SaveOutcome> {
    // The path is written to, so it must stay inside the work tree, and out
    // of `.git` (a `.git` entry in the work tree is how a checkout runs code).
    let safe = Path::new(path).components().all(|part| match part {
        Component::Normal(name) => !name.to_string_lossy().eq_ignore_ascii_case(".git"),
        _ => false,
    });
    if !safe {
        return Err(Error::Parse("path is not inside the work tree"));
    }
    let current = unmerged(repo, path)?;
    if current.is_empty() || current != guard.stages {
        return Ok(SaveOutcome::StagesChanged);
    }
    let disk = fs::read(repo.root().join(path)).ok();
    if disk.as_deref().map(digest) != guard.work_tree_digest {
        return Ok(SaveOutcome::WorkTreeChanged);
    }
    let (mode, mode_conflict) =
        resolved_mode(&current).ok_or(Error::Parse("path has no side to take the mode from"))?;

    let mut child = repo
        .command()
        .args(["hash-object", "-w", "--no-filters", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::Spawn)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(Error::Parse("hash-object stdin"))?;
    let written = stdin.write_all(content.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().map_err(Error::Io)?;
    let out = check_output(out, "hash-object")?;
    written.map_err(Error::Io)?;
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_owned();

    // `--cacheinfo` replaces the path's conflict stages with one entry.
    let out = repo
        .command()
        .args(["update-index", "--add", "--cacheinfo", &mode, &oid, path])
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "update-index")?;
    let checkout = repo
        .command()
        .args(["checkout-index", "-f", "--", path])
        .output()
        .map_err(Error::Spawn)
        .and_then(|out| check_output(out, "checkout-index"));
    if let Err(error) = checkout {
        // The work tree could not be written: put the conflict back, so the
        // path is not left staged with content that never reached the file.
        restore_stages(repo, path, &current)?;
        return Err(error);
    }
    Ok(if mode_conflict {
        SaveOutcome::SavedModeConflict
    } else {
        SaveOutcome::Saved
    })
}

/// The mode of the resolved file, picked like git: the same on both sides, or
/// the side that changed it from the base. `true` when both sides changed it
/// differently (ours is kept). `None` without a side to look at.
fn resolved_mode(stages: &[IndexStage]) -> Option<(String, bool)> {
    let mode = |stage: u8| {
        stages
            .iter()
            .find(|entry| entry.stage == stage)
            .map(|entry| entry.mode.clone())
    };
    let (base, ours, theirs) = (mode(1), mode(2), mode(3));
    let (ours, theirs) = match (ours, theirs) {
        (Some(ours), Some(theirs)) => (ours, theirs),
        (one, other) => return one.or(other).map(|mode| (mode, false)),
    };
    if ours == theirs {
        return Some((ours, false));
    }
    match base {
        Some(base) if ours == base => Some((theirs, false)),
        Some(base) if theirs == base => Some((ours, false)),
        _ => Some((ours, true)),
    }
}

/// Puts the unmerged entries `stages` back into the index for `path`.
fn restore_stages(repo: &Repo, path: &str, stages: &[IndexStage]) -> Result<()> {
    let out = repo
        .command()
        .args(["update-index", "--force-remove", "--"])
        .arg(path)
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "update-index --force-remove")?;
    let mut child = repo
        .command()
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::Spawn)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(Error::Parse("update-index stdin"))?;
    let info: String = stages
        .iter()
        .map(|entry| format!("{} {} {}\t{path}\n", entry.mode, entry.oid, entry.stage))
        .collect();
    let written = stdin.write_all(info.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().map_err(Error::Io)?;
    check_output(out, "update-index --index-info")?;
    written.map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(text: &str) -> SideText {
        SideText::parse(text)
    }

    /// The model for three texts, with the line diffs computed by our own
    /// diff (no git).
    fn model(base: Option<&str>, ours: &str, theirs: &str) -> TextConflict {
        let (b, o, t) = (base.map(side), side(ours), side(theirs));
        let empty = SideText::default();
        let hunks = |s: &SideText| -> Vec<DiffHunk> {
            diff_lines(&b.as_ref().unwrap_or(&empty).keys(), &s.keys())
                .into_iter()
                .map(|(base, side)| DiffHunk { base, side })
                .collect()
        };
        TextConflict::from_sides(b.clone(), o.clone(), t.clone(), &hunks(&o), &hunks(&t))
    }

    fn keys(lines: &[&'static str]) -> Vec<Line<'static>> {
        lines
            .iter()
            .map(|text| Line {
                text,
                ending: LineEnding::Lf,
            })
            .collect()
    }

    fn hunk(base: Range<usize>, side: Range<usize>) -> DiffHunk {
        DiffHunk { base, side }
    }

    use ConflictSide::{Ours, Theirs};

    fn picked(sides: &[ConflictSide]) -> ConflictResolution {
        ConflictResolution::Picked(sides.to_vec())
    }

    #[test]
    fn overlapping_changes_conflict() {
        let base = keys(&["a", "b", "c", "d", "e", "f", "g"]);
        let ours = keys(&["a", "X", "Y", "d", "e", "f", "g"]);
        let theirs = keys(&["a", "b", "P", "d", "e", "f", "g"]);
        let segments = build_segments(
            &base,
            &ours,
            &theirs,
            &[hunk(1..3, 1..3)],
            &[hunk(2..3, 2..3)],
        );
        assert_eq!(
            segments,
            vec![
                ConflictSegment::Stable {
                    base: 0..1,
                    ours: 0..1,
                    theirs: 0..1
                },
                ConflictSegment::Conflict {
                    base: 1..3,
                    ours: 1..3,
                    theirs: 1..3
                },
                ConflictSegment::Stable {
                    base: 3..7,
                    ours: 3..7,
                    theirs: 3..7
                },
            ]
        );
    }

    #[test]
    fn adjacent_changes_conflict() {
        // Lines 5 and 6 of 7 changed on different sides: git conflicts.
        let base = keys(&["1", "2", "3", "4", "5", "6", "7"]);
        let ours = keys(&["1", "2", "3", "4", "X", "6", "7"]);
        let theirs = keys(&["1", "2", "3", "4", "5", "Y", "7"]);
        let segments = build_segments(
            &base,
            &ours,
            &theirs,
            &[hunk(4..5, 4..5)],
            &[hunk(5..6, 5..6)],
        );
        assert!(segments.contains(&ConflictSegment::Conflict {
            base: 4..6,
            ours: 4..6,
            theirs: 4..6
        }));
    }

    #[test]
    fn separate_changes_are_auto_resolved() {
        let base = keys(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        let ours = keys(&["X", "2", "3", "4", "5", "6", "7", "8"]);
        let theirs = keys(&["1", "2", "3", "4", "5", "6", "7"]);
        let segments = build_segments(
            &base,
            &ours,
            &theirs,
            &[hunk(0..1, 0..1)],
            &[hunk(7..8, 7..7)],
        );
        assert_eq!(
            segments,
            vec![
                ConflictSegment::Auto {
                    side: Ours,
                    base: 0..1,
                    ours: 0..1,
                    theirs: 0..1
                },
                ConflictSegment::Stable {
                    base: 1..7,
                    ours: 1..7,
                    theirs: 1..7
                },
                ConflictSegment::Auto {
                    side: Theirs,
                    base: 7..8,
                    ours: 7..8,
                    theirs: 7..7
                },
            ]
        );
    }

    #[test]
    fn same_change_on_both_sides_is_same() {
        let base = keys(&["a", "b", "c"]);
        let both = keys(&["a", "X", "c"]);
        let segments = build_segments(
            &base,
            &both,
            &both,
            &[hunk(1..2, 1..2)],
            &[hunk(1..2, 1..2)],
        );
        assert!(segments.contains(&ConflictSegment::Same {
            base: 1..2,
            ours: 1..2,
            theirs: 1..2
        }));
    }

    #[test]
    fn insertions_at_the_same_point() {
        let base = keys(&["a", "b"]);
        let ours = keys(&["a", "X", "b"]);
        let theirs = keys(&["a", "Y", "b"]);
        let segments = build_segments(
            &base,
            &ours,
            &theirs,
            &[hunk(1..1, 1..2)],
            &[hunk(1..1, 1..2)],
        );
        assert!(segments.contains(&ConflictSegment::Conflict {
            base: 1..1,
            ours: 1..2,
            theirs: 1..2
        }));
        let segments = build_segments(
            &base,
            &ours,
            &ours,
            &[hunk(1..1, 1..2)],
            &[hunk(1..1, 1..2)],
        );
        assert!(segments.contains(&ConflictSegment::Same {
            base: 1..1,
            ours: 1..2,
            theirs: 1..2
        }));
    }

    #[test]
    fn deletion_against_edit_conflicts() {
        let m = model(
            Some("a\nb\nc\nd\ne\nf\ng\n"),
            "a\nb\nc\nd\ne\nf\n",
            "a\nb\nc\nX\ne\nf\ng\n",
        );
        // Not a conflict: the changes are far apart.
        assert_eq!(m.conflict_count(), 0);
        let m = model(Some("a\nb\nc\n"), "a\nc\n", "a\nX\nc\n");
        assert_eq!(m.conflict_count(), 1);
    }

    #[test]
    fn changes_at_file_start_and_end() {
        let m = model(Some("a\nb\nc\n"), "X\nb\nc\n", "Y\nb\nc\n");
        assert_eq!(m.conflict_count(), 1);
        assert_eq!(
            m.render(&[picked(&[Ours])]).unwrap(),
            "X\nb\nc\n".to_string()
        );
        let m = model(Some("a\nb\nc\n"), "a\nb\nX\n", "a\nb\nY\n");
        assert_eq!(
            m.render(&[picked(&[Theirs])]).unwrap(),
            "a\nb\nY\n".to_string()
        );
    }

    #[test]
    fn both_added_without_a_base() {
        let m = model(None, "a\nb\n", "a\nc\n");
        assert_eq!(m.base, None);
        assert_eq!(m.conflict_count(), 1);
        // Refinement keeps the shared first line out of the conflict.
        assert_eq!(
            m.render(&[picked(&[Ours, Theirs])]).unwrap(),
            "a\nb\nc\n".to_string()
        );
        let same = model(None, "a\n", "a\n");
        assert_eq!(same.conflict_count(), 0);
    }

    #[test]
    fn refinement_trims_shared_lines_inside_a_conflict() {
        // Both changed lines 2..5 but share "x" in the middle.
        let m = model(
            Some("0\nb\nc\nd\ne\n9\n"),
            "0\nA\nx\nB\ne\n9\n",
            "0\nC\nx\nD\ne\n9\n",
        );
        // The shared "x" is only one line between two conflicts, so git
        // joins them again.
        assert_eq!(m.conflict_count(), 1);
        let far = model(
            Some("0\nb\nc\nd\ne\nf\ng\nh\n9\n"),
            "0\nA\nx\ny\nz\nw\nB\nh\n9\n",
            "0\nC\nx\ny\nz\nw\nD\nh\n9\n",
        );
        // Four shared lines with letters: two conflicts.
        assert_eq!(far.conflict_count(), 2);
    }

    #[test]
    fn conflicts_with_a_gap_without_alnum_stay_apart() {
        // `git merge-file` joins these (zealous alnum); `git merge` writes
        // two blocks (zealous), and the model follows `git merge`.
        let m = model(
            Some("0\nb\n-\n-\n-\n-\nf\n9\n"),
            "0\nA\n-\n-\n-\n-\nB\n9\n",
            "0\nC\n-\n-\n-\n-\nD\n9\n",
        );
        assert_eq!(m.conflict_count(), 2);
    }

    #[test]
    fn a_refined_conflict_keeps_its_base_lines_in_the_conflict_pieces() {
        let m = model(
            Some("0\nb\nc\nd\ne\nf\ng\nh\n9\n"),
            "0\nA\nx\ny\nz\nw\nB\nh\n9\n",
            "0\nC\nx\ny\nz\nw\nD\nh\n9\n",
        );
        let pieces: Vec<_> = m
            .segments
            .iter()
            .filter_map(|segment| match segment {
                ConflictSegment::Conflict { base, .. } => Some(base.clone()),
                _ => None,
            })
            .collect();
        // The first conflict shows the whole region's base (lines 1..7), the
        // second has none of its own.
        assert_eq!(pieces, vec![1..7, 7..7]);
        for segment in &m.segments {
            if let ConflictSegment::Same { base, .. } = segment {
                assert!(base.is_empty(), "a Same piece must not hold base lines");
            }
        }
    }

    #[test]
    fn region_lines_is_what_render_and_markers_use() {
        let m = model(Some("a\nb\nc\n"), "a\nX\nc\n", "a\nY\nc\n");
        let lines = |resolution: ConflictResolution| {
            m.region_lines(0, &resolution)
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(ConflictResolution::Unresolved), Vec::<String>::new());
        assert_eq!(lines(picked(&[Theirs, Ours])), vec!["Y", "X"]);
        assert_eq!(lines(picked(&[Ours, Ours])), vec!["X"]);
        assert_eq!(
            lines(ConflictResolution::Custom("p\nq\n".into())),
            vec!["p", "q"]
        );
        assert!(m.region_lines(5, &picked(&[Ours])).is_empty());
        assert_eq!(
            m.render_markers("HEAD", "side"),
            "a\n<<<<<<< HEAD\nX\n=======\nY\n>>>>>>> side\nc\n"
        );
    }

    #[test]
    fn typed_text_ending_the_file_stays_unterminated_only_when_neither_side_had_a_newline() {
        let custom = |text: &str| ConflictResolution::Custom(text.to_string());
        // Neither side ends with a newline, the region is the file's end.
        let m = model(Some("a\nb\nc"), "a\nb\nX", "a\nb\nY");
        assert_eq!(m.render(&[custom("Z")]).unwrap(), "a\nb\nZ");
        assert_eq!(m.render(&[custom("Z\n")]).unwrap(), "a\nb\nZ\n");
        assert_eq!(m.render(&[custom("Z\nW")]).unwrap(), "a\nb\nZ\nW");
        // One side ends with a newline: the typed line gets a terminator.
        let m = model(Some("a\nb\nc\n"), "a\nb\nX", "a\nb\nY\n");
        assert_eq!(m.render(&[custom("Z")]).unwrap(), "a\nb\nZ\n");
        // Not the last content of the file: always terminated.
        let m = model(
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni"),
            "a\nX\nc\nd\ne\nf\ng\nh\ni",
            "a\nY\nc\nd\ne\nf\ng\nh\ni",
        );
        assert_eq!(
            m.render(&[custom("Z")]).unwrap(),
            "a\nZ\nc\nd\ne\nf\ng\nh\ni"
        );
        // CRLF files: the typed lines take CRLF, the unterminated last one none.
        let m = model(Some("a\r\nb\r\nc"), "a\r\nb\r\nX", "a\r\nb\r\nY");
        assert_eq!(m.render(&[custom("Z\nW")]).unwrap(), "a\r\nb\r\nZ\r\nW");
    }

    #[test]
    fn render_picks_in_pick_order() {
        let m = model(Some("a\nb\nc\n"), "a\nX\nc\n", "a\nY\nc\n");
        assert_eq!(m.render(&[picked(&[Ours])]).unwrap(), "a\nX\nc\n");
        assert_eq!(m.render(&[picked(&[Theirs])]).unwrap(), "a\nY\nc\n");
        assert_eq!(
            m.render(&[picked(&[Ours, Theirs])]).unwrap(),
            "a\nX\nY\nc\n"
        );
        assert_eq!(
            m.render(&[picked(&[Theirs, Ours])]).unwrap(),
            "a\nY\nX\nc\n"
        );
        assert_eq!(m.render(&[picked(&[Ours, Ours])]).unwrap(), "a\nX\nc\n");
    }

    #[test]
    fn render_custom_and_unresolved() {
        let m = model(Some("a\nb\nc\n"), "a\nX\nc\n", "a\nY\nc\n");
        assert_eq!(m.render(&m.unresolved()), None);
        assert_eq!(m.render(&[picked(&[])]), None);
        assert_eq!(m.render(&[]), None);
        let custom = |text: &str| m.render(&[ConflictResolution::Custom(text.to_string())]);
        assert_eq!(custom("Z").unwrap(), "a\nZ\nc\n");
        assert_eq!(custom("Z\n").unwrap(), "a\nZ\nc\n");
        assert_eq!(custom("Z\nW").unwrap(), "a\nZ\nW\nc\n");
        assert_eq!(custom("").unwrap(), "a\nc\n");
        assert_eq!(custom("\n").unwrap(), "a\n\nc\n");
    }

    #[test]
    fn crlf_round_trips() {
        let m = model(
            Some("a\r\nb\r\nc\r\n"),
            "a\r\nX\r\nc\r\n",
            "a\r\nY\r\nc\r\n",
        );
        assert_eq!(m.ours, vec!["a", "X", "c"]);
        assert_eq!(m.render(&[picked(&[Ours])]).unwrap(), "a\r\nX\r\nc\r\n");
        assert_eq!(
            m.render(&[picked(&[Theirs, Ours])]).unwrap(),
            "a\r\nY\r\nX\r\nc\r\n"
        );
        // Typed lines take the file's ending.
        assert_eq!(
            m.render(&[ConflictResolution::Custom("Z\nW".into())])
                .unwrap(),
            "a\r\nZ\r\nW\r\nc\r\n"
        );
    }

    #[test]
    fn mixed_endings_keep_their_own() {
        let m = model(Some("a\nb\r\nc\n"), "a\nX\r\nc\n", "a\nY\nc\n");
        assert_eq!(m.render(&[picked(&[Ours])]).unwrap(), "a\nX\r\nc\n");
        assert_eq!(m.render(&[picked(&[Theirs])]).unwrap(), "a\nY\nc\n");
    }

    #[test]
    fn a_missing_final_newline_is_kept_and_repaired_in_the_middle() {
        let m = model(Some("a\nb\nc"), "a\nX\nc", "a\nY\nc");
        assert_eq!(m.render(&[picked(&[Ours])]).unwrap(), "a\nX\nc");
        let m = model(Some("a\nb\nc\n"), "a\nX", "a\nY\nc\n");
        assert_eq!(m.render(&[picked(&[Ours])]).unwrap(), "a\nX");
        // A union puts text after the unterminated line: git terminates it.
        assert_eq!(
            m.render(&[picked(&[Ours, Theirs])]).unwrap(),
            "a\nX\nY\nc\n"
        );
    }

    #[test]
    fn diff_lines_slides_changes_down() {
        let a = keys(&["x", "x"]);
        let b = keys(&["x"]);
        assert_eq!(diff_lines(&a, &b), vec![(1..2, 1..1)]);
        assert_eq!(diff_lines(&a, &a), vec![]);
    }

    #[test]
    fn diff_lines_anchors_on_rare_lines_like_histogram() {
        // Myers would match the many "x" lines; the histogram anchors on the
        // unique "u" and treats the rest as changed around it.
        let a = keys(&["x", "x", "u", "x", "x"]);
        let b = keys(&["y", "u", "y"]);
        assert_eq!(diff_lines(&a, &b), vec![(0..2, 0..1), (3..5, 2..3)]);
    }

    #[test]
    fn the_resolved_mode_follows_the_side_that_changed_it() {
        let stages = |modes: &[(u8, &str)]| -> Vec<IndexStage> {
            modes
                .iter()
                .map(|(stage, mode)| IndexStage {
                    stage: *stage,
                    mode: (*mode).into(),
                    oid: String::new(),
                })
                .collect()
        };
        let pick = |modes: &[(u8, &str)]| resolved_mode(&stages(modes));
        let some = |mode: &str, conflict: bool| Some((mode.to_string(), conflict));
        assert_eq!(
            pick(&[(1, "100644"), (2, "100644"), (3, "100755")]),
            some("100755", false)
        );
        assert_eq!(
            pick(&[(1, "100644"), (2, "100755"), (3, "100644")]),
            some("100755", false)
        );
        assert_eq!(
            pick(&[(1, "100644"), (2, "100755"), (3, "100755")]),
            some("100755", false)
        );
        // Both changed it, differently: ours stays, and the caller is told.
        assert_eq!(
            pick(&[(1, "100644"), (2, "100755"), (3, "100664")]),
            some("100755", true)
        );
        assert_eq!(pick(&[(2, "100755"), (3, "100644")]), some("100755", true));
        assert_eq!(pick(&[(3, "100644")]), some("100644", false));
        assert_eq!(pick(&[]), None);
    }
}
