//! The conflict model behind the conflict editor: each side of a conflicted
//! file, the file split into segments over the base, and the text a set of
//! resolutions renders to.
//!
//! Line diffs come from the git CLI (base to each side); the three-way
//! chunking is plain Rust, written to give the same regions as
//! `git merge-file` (git's "zealous alnum" level): conflicts are refined by
//! diffing the two sides against each other, and conflicts separated by at
//! most three lines, or by lines without letters and digits, are joined.

use std::fs;
use std::ops::Range;
use std::path::{Component, Path};

use crate::status::{literal_command, stage, ConflictSide, UNTRACKED_COUNT_CAP};
use crate::{check_output, Error, Repo, Result};

/// How one line of a side ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n`
    Lf,
    /// `\r\n`
    CrLf,
    /// No terminator: only the last line of a file can have this.
    None,
}

/// What kind of conflict a path is in, with the data to resolve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    Text(TextConflict),
    /// A side has a NUL byte: only whole-side picking is possible.
    Binary,
    /// We deleted the file (no stage 2), they changed it.
    DeletedByUs,
    /// They deleted the file (no stage 3), we changed it.
    DeletedByThem,
    /// Either side is a symlink (mode 120000).
    Symlink,
    /// Either side is a submodule (mode 160000).
    Submodule,
}

/// A text conflict: the three versions and the file cut into segments.
///
/// Lines are stored without their terminator (`\n` or `\r\n`); the
/// `*_endings` vectors hold the terminator of each line, so rendering
/// reproduces the bytes of mixed-ending files and a missing final newline.
/// Invalid UTF-8 was replaced with U+FFFD when the blobs were read.
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
}

/// One region of the file. All ranges are half-open and index the line
/// vector of their side; `Stable` ranges have equal lengths on all sides.
///
/// A conflict that git's refinement splits into several (common lines inside
/// it become `Same`) cannot attribute base lines to the pieces: the first
/// piece gets the base range of the whole region, the later ones an empty
/// range at its end.
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
    /// the file's line ending. May be empty (the region is removed).
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

    /// The file text for `resolutions`, or `None` while a conflict is
    /// unresolved (or `resolutions` has the wrong length).
    pub fn render(&self, resolutions: &[ConflictResolution]) -> Option<String> {
        if resolutions.len() != self.conflict_count() {
            return None;
        }
        let default_ending = self.default_ending();
        let mut out = Emitter::default();
        let mut next = resolutions.iter();
        for segment in &self.segments {
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
                ConflictSegment::Conflict { ours, theirs, .. } => match next.next()? {
                    ConflictResolution::Unresolved => return None,
                    ConflictResolution::Picked(sides) => {
                        if sides.is_empty() {
                            return None;
                        }
                        let mut seen = Vec::new();
                        for side in sides {
                            if seen.contains(side) {
                                continue;
                            }
                            seen.push(*side);
                            match side {
                                ConflictSide::Ours => {
                                    out.side(&self.ours, &self.ours_endings, ours);
                                }
                                ConflictSide::Theirs => {
                                    out.side(&self.theirs, &self.theirs_endings, theirs);
                                }
                            }
                        }
                    }
                    ConflictResolution::Custom(text) => {
                        let body = text.strip_suffix('\n').unwrap_or(text);
                        if !text.is_empty() {
                            for line in body.split('\n') {
                                let line = line.strip_suffix('\r').unwrap_or(line);
                                out.line(line, default_ending);
                            }
                        }
                    }
                },
            }
        }
        Some(out.finish())
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
        base: Option<&SideText>,
        ours: &SideText,
        theirs: &SideText,
        ours_hunks: &[DiffHunk],
        theirs_hunks: &[DiffHunk],
    ) -> Self {
        let empty = SideText::default();
        let base_side = base.unwrap_or(&empty);
        let segments = build_segments(
            &base_side.keys(),
            &ours.keys(),
            &theirs.keys(),
            ours_hunks,
            theirs_hunks,
        );
        Self {
            base: base.map(|side| side.lines.clone()),
            ours: ours.lines.clone(),
            theirs: theirs.lines.clone(),
            base_endings: base.map(|side| side.endings.clone()).unwrap_or_default(),
            ours_endings: ours.endings.clone(),
            theirs_endings: theirs.endings.clone(),
            segments,
        }
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
        if self.open {
            self.out.push_str(if self.crlf { "\r\n" } else { "\n" });
            self.open = false;
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Above this many line pairs the refinement does not diff a conflict's two
/// sides (the table would be too big) and keeps the conflict whole.
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
        // The base range goes to the first piece, the rest get an empty one.
        let first = std::cell::Cell::new(true);
        let piece_base = || {
            if first.replace(false) {
                base.clone()
            } else {
                base.end..base.end
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
                    base: piece_base(),
                    ours: oc..co.start,
                    theirs: tc..ct.start,
                });
            }
            oc = co.end;
            tc = ct.end;
            out.push(ConflictSegment::Conflict {
                base: piece_base(),
                ours: co,
                theirs: ct,
            });
        }
        if oc < o.end || first.get() {
            out.push(ConflictSegment::Same {
                base: piece_base(),
                ours: oc..o.end,
                theirs: tc..t.end,
            });
        }
    }
    out
}

/// Joins conflicts that are close together, like git's zealous alnum level:
/// when the lines between two conflicts number at most three, or contain no
/// letter or digit, they are one conflict. Only unchanged and `Same` text may
/// lie between; a one-sided change keeps conflicts apart.
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
            let gap = &ours[po.end..o.start];
            let close = gap.len() <= 3
                || !gap
                    .iter()
                    .any(|line| line.text.bytes().any(|b| b.is_ascii_alphanumeric()));
            if close {
                let joined = ConflictSegment::Conflict {
                    base: pb.start..base.end,
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

/// The changes turning `a` into `b`, as ranges of `a` and of `b`. Common
/// lines are matched as early as possible, so a change inside a run of equal
/// lines lands at the end of the run (git slides changes down too).
fn diff_lines(a: &[Line<'_>], b: &[Line<'_>]) -> Vec<(Range<usize>, Range<usize>)> {
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

// ---------------------------------------------------------------------------
// Reading a conflicted path from the repository
// ---------------------------------------------------------------------------

/// One index entry of a conflicted path.
struct StageEntry {
    mode: String,
    oid: String,
}

/// The index entries (stage 1 base, 2 ours, 3 theirs) of `path`.
fn stages(repo: &Repo, path: &str) -> Result<[Option<StageEntry>; 3]> {
    let out = literal_command(repo)
        .args(["ls-files", "-u", "-z", "--"])
        .arg(path)
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "ls-files -u")?;
    let mut entries: [Option<StageEntry>; 3] = [None, None, None];
    // `<mode> SP <oid> SP <stage> TAB <path> NUL`
    for record in out.stdout.split(|&byte| byte == 0) {
        let Some(tab) = record.iter().position(|&b| b == b'\t') else {
            continue;
        };
        if &record[tab + 1..] != path.as_bytes() {
            continue;
        }
        let head = String::from_utf8_lossy(&record[..tab]);
        let mut fields = head.split(' ');
        let (Some(mode), Some(oid), Some(stage)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(Error::Parse("ls-files -u entry"));
        };
        let slot = match stage {
            "1" => 0,
            "2" => 1,
            "3" => 2,
            _ => return Err(Error::Parse("ls-files -u stage")),
        };
        entries[slot] = Some(StageEntry {
            mode: mode.to_string(),
            oid: oid.to_string(),
        });
    }
    Ok(entries)
}

/// A blob's bytes; over the display cap is [`Error::TooLarge`].
fn read_blob(repo: &Repo, oid: &str) -> Result<Vec<u8>> {
    let out = repo
        .command()
        .args(["cat-file", "-s", oid])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "cat-file -s")?;
    let size: u64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| Error::Parse("cat-file -s size"))?;
    if size > UNTRACKED_COUNT_CAP {
        return Err(Error::TooLarge { what: "diff" });
    }
    let out = repo
        .command()
        .args(["cat-file", "blob", oid])
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "cat-file blob").map(|out| out.stdout)
}

/// The line changes from blob `base` to blob `side`, from `git diff -U0`
/// with everything that could vary with user configuration pinned.
fn blob_hunks(repo: &Repo, base: &StageEntry, side: &StageEntry) -> Result<Vec<DiffHunk>> {
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
            "--diff-algorithm=myers",
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

/// Loads the conflict at `path` (repo-relative, literal).
pub(crate) fn conflict(repo: &Repo, path: &str) -> Result<ConflictKind> {
    let [base, ours, theirs] = stages(repo, path)?;
    if ours.is_none() && theirs.is_none() {
        return Err(Error::Parse("path has no conflict entries"));
    }
    let mode_of = |entry: &Option<StageEntry>, mode: &str| {
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
        (Some(_), None) => return Ok(ConflictKind::DeletedByThem),
        _ => return Ok(ConflictKind::DeletedByUs),
    };
    let ours_bytes = read_blob(repo, &ours.oid)?;
    let theirs_bytes = read_blob(repo, &theirs.oid)?;
    let base_bytes = match &base {
        Some(base) => Some(read_blob(repo, &base.oid)?),
        None => None,
    };
    let binary = |bytes: &[u8]| bytes.contains(&0);
    if binary(&ours_bytes) || binary(&theirs_bytes) || base_bytes.as_deref().is_some_and(binary) {
        return Ok(ConflictKind::Binary);
    }
    let parse = |bytes: &[u8]| SideText::parse(&String::from_utf8_lossy(bytes));
    let (ours_text, theirs_text) = (parse(&ours_bytes), parse(&theirs_bytes));
    let base_text = base_bytes.as_deref().map(parse);
    let (ours_hunks, theirs_hunks) = match (&base, &base_text) {
        (Some(base), Some(_)) => (
            blob_hunks(repo, base, &ours)?,
            blob_hunks(repo, base, &theirs)?,
        ),
        // Both added the file: an empty base, every line an insertion.
        _ => (insertion(&ours_text), insertion(&theirs_text)),
    };
    Ok(ConflictKind::Text(TextConflict::from_sides(
        base_text.as_ref(),
        &ours_text,
        &theirs_text,
        &ours_hunks,
        &theirs_hunks,
    )))
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

/// Writes `content` to the work-tree file `path` and stages it, which ends
/// the conflict for that path.
pub(crate) fn save_resolution(repo: &Repo, path: &str, content: &str) -> Result<()> {
    // The path is written to, so it must stay inside the work tree.
    let inside = Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if !inside {
        return Err(Error::Parse("path is not inside the work tree"));
    }
    fs::write(repo.root().join(path), content).map_err(Error::Io)?;
    stage(repo, &[path.to_string()])
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
        TextConflict::from_sides(b.as_ref(), &o, &t, &hunks(&o), &hunks(&t))
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
    fn conflicts_with_a_gap_without_alnum_are_joined() {
        let m = model(
            Some("0\nb\n-\n-\n-\n-\nf\n9\n"),
            "0\nA\n-\n-\n-\n-\nB\n9\n",
            "0\nC\n-\n-\n-\n-\nD\n9\n",
        );
        assert_eq!(m.conflict_count(), 1);
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
}
