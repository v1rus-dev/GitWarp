//! Turns parsed diff hunks into the flat rows the unified view paints, and the
//! word-level highlight inside a paired removed/added line.
//!
//! Everything here is a pure function of the hunk data, so both the row order
//! and the word diff are unit tested without a window.

use std::ops::Range;

use git_core::{DiffLine, Hunk, LineKind};

/// Expanded width of a tab, in spaces.
const TAB: usize = 4;
/// Above this many token pairs the word diff falls back to the common
/// prefix/suffix, keeping the work bounded for minified files.
const WORD_DIFF_LIMIT: usize = 40_000;

/// What one row of the unified view is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A `@@ … @@` header.
    Hunk,
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` after the line it applies to.
    Note,
}

/// One painted row: a hunk header, a numbered line, or a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The row's text, with tabs already expanded.
    pub text: String,
    /// Byte ranges of `text` that changed against its paired line, only on a
    /// removed/added row of a pair.
    pub emphasis: Vec<Range<usize>>,
    pub no_newline: bool,
}

impl DiffRow {
    fn header(header: &str) -> Self {
        Self {
            kind: RowKind::Hunk,
            old_line: None,
            new_line: None,
            text: header.to_string(),
            emphasis: Vec::new(),
            no_newline: false,
        }
    }

    fn line(line: &DiffLine, text: String, emphasis: Vec<Range<usize>>) -> Self {
        Self {
            kind: match line.kind {
                LineKind::Context => RowKind::Context,
                LineKind::Added => RowKind::Added,
                LineKind::Removed => RowKind::Removed,
            },
            old_line: line.old_line,
            new_line: line.new_line,
            text,
            emphasis,
            no_newline: line.no_newline,
        }
    }

    fn note() -> Self {
        Self {
            kind: RowKind::Note,
            old_line: None,
            new_line: None,
            text: "\\ No newline at end of file".to_string(),
            emphasis: Vec::new(),
            no_newline: false,
        }
    }
}

/// Flattens hunks into rows, pairing each run of removed lines with the run of
/// added lines that follows it.
///
/// A pair is painted removed-then-added, as the design's unified view does;
/// unpaired lines keep the plain line background. Runs of context lines stay
/// in order.
pub fn build_rows(hunks: &[Hunk]) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for hunk in hunks {
        rows.push(DiffRow::header(&hunk.header));
        let lines = &hunk.lines;
        let mut index = 0;
        while index < lines.len() {
            match lines[index].kind {
                LineKind::Context => {
                    let text = expand_tabs(&lines[index].text);
                    rows.push(DiffRow::line(&lines[index], text, Vec::new()));
                    push_note(&mut rows, &lines[index]);
                    index += 1;
                }
                LineKind::Removed => {
                    let removed_start = index;
                    while index < lines.len() && lines[index].kind == LineKind::Removed {
                        index += 1;
                    }
                    let removed = &lines[removed_start..index];
                    let added_start = index;
                    while index < lines.len() && lines[index].kind == LineKind::Added {
                        index += 1;
                    }
                    let added = &lines[added_start..index];
                    for pair in 0..removed.len().max(added.len()) {
                        match (removed.get(pair), added.get(pair)) {
                            (Some(old), Some(new)) => {
                                let old_text = expand_tabs(&old.text);
                                let new_text = expand_tabs(&new.text);
                                let (old_emphasis, new_emphasis) = word_diff(&old_text, &new_text);
                                rows.push(DiffRow::line(old, old_text, old_emphasis));
                                push_note(&mut rows, old);
                                rows.push(DiffRow::line(new, new_text, new_emphasis));
                                push_note(&mut rows, new);
                            }
                            (Some(old), None) => {
                                let text = expand_tabs(&old.text);
                                rows.push(DiffRow::line(old, text, Vec::new()));
                                push_note(&mut rows, old);
                            }
                            (None, Some(new)) => {
                                let text = expand_tabs(&new.text);
                                rows.push(DiffRow::line(new, text, Vec::new()));
                                push_note(&mut rows, new);
                            }
                            (None, None) => {}
                        }
                    }
                }
                LineKind::Added => {
                    let text = expand_tabs(&lines[index].text);
                    rows.push(DiffRow::line(&lines[index], text, Vec::new()));
                    push_note(&mut rows, &lines[index]);
                    index += 1;
                }
            }
        }
    }
    rows
}

/// The note row that follows a line whose file has no trailing newline.
fn push_note(rows: &mut Vec<DiffRow>, line: &DiffLine) {
    if line.no_newline {
        rows.push(DiffRow::note());
    }
}

/// The widest row in display columns, for the horizontal scroll width.
pub fn max_columns(rows: &[DiffRow]) -> usize {
    rows.iter()
        .map(|row| row.text.chars().count())
        .max()
        .unwrap_or(0)
}

/// Expands tabs to the next multiple of [`TAB`] spaces.
pub fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut column = 0;
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = TAB - column % TAB;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    out
}

/// Byte ranges that changed between two lines, one list per side.
pub fn word_diff(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let old_tokens = tokens(old);
    let new_tokens = tokens(new);
    let old_words: Vec<&str> = old_tokens.iter().map(|(text, _)| *text).collect();
    let new_words: Vec<&str> = new_tokens.iter().map(|(text, _)| *text).collect();

    let keep = if old_words.len().saturating_mul(new_words.len()) <= WORD_DIFF_LIMIT {
        lcs_keep(&old_words, &new_words)
    } else {
        prefix_suffix_keep(&old_words, &new_words)
    };

    (
        emphasis_ranges(old, &old_tokens, &keep.0),
        emphasis_ranges(new, &new_tokens, &keep.1),
    )
}

/// Splits `text` into word, whitespace and single-punctuation tokens, with
/// their byte ranges.
fn tokens(text: &str) -> Vec<(&str, Range<usize>)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        if !word(ch) && !ch.is_whitespace() {
            if start < index {
                out.push((&text[start..index], start..index));
            }
            out.push((
                &text[index..index + ch.len_utf8()],
                index..index + ch.len_utf8(),
            ));
            start = index + ch.len_utf8();
            continue;
        }
        let same = |c: char| {
            if ch.is_whitespace() {
                c.is_whitespace()
            } else {
                word(c)
            }
        };
        while let Some(&(_, next)) = chars.peek() {
            if !same(next) {
                break;
            }
            chars.next();
        }
        let end = chars.peek().map(|(i, _)| *i).unwrap_or(text.len());
        out.push((&text[start..end], start..end));
        start = end;
    }
    out
}

/// Which tokens of each side survive the longest common subsequence.
fn lcs_keep(old: &[&str], new: &[&str]) -> (Vec<bool>, Vec<bool>) {
    let n = old.len();
    let m = new.len();
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut keep_old, mut keep_new) = (vec![false; n], vec![false; m]);
    while i < n && j < m {
        if old[i] == new[j] {
            keep_old[i] = true;
            keep_new[j] = true;
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (keep_old, keep_new)
}

/// The cheap fallback: only the equal prefix and suffix stay unmarked.
fn prefix_suffix_keep(old: &[&str], new: &[&str]) -> (Vec<bool>, Vec<bool>) {
    let mut keep_old = vec![false; old.len()];
    let mut keep_new = vec![false; new.len()];
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        keep_old[prefix] = true;
        keep_new[prefix] = true;
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        keep_old[old.len() - 1 - suffix] = true;
        keep_new[new.len() - 1 - suffix] = true;
        suffix += 1;
    }
    (keep_old, keep_new)
}

/// Merges the ranges of dropped tokens into contiguous highlight ranges.
fn emphasis_ranges(
    text: &str,
    tokens: &[(&str, Range<usize>)],
    keep: &[bool],
) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (token, kept) in tokens.iter().zip(keep) {
        if *kept {
            continue;
        }
        let range = token.1.clone();
        match ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => ranges.push(range),
        }
    }
    debug_assert!(ranges
        .iter()
        .all(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end)));
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_line: old,
            new_line: new,
            text: text.to_string(),
            no_newline: false,
        }
    }

    #[test]
    fn expands_tabs_to_the_next_stop() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("abcd\te"), "abcd    e");
        assert_eq!(expand_tabs("plain"), "plain");
    }

    #[test]
    fn word_diff_marks_only_the_changed_words() {
        let (old, new) = word_diff("let value = old + 1;", "let value = new + 1;");
        let marked = |text: &str, ranges: &[Range<usize>]| {
            ranges
                .iter()
                .map(|range| text[range.clone()].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(marked("let value = old + 1;", &old), ["old"]);
        assert_eq!(marked("let value = new + 1;", &new), ["new"]);
    }

    #[test]
    fn word_diff_keeps_the_common_prefix_and_suffix() {
        let (old, new) = word_diff(
            "paint_line(cx, x0, x1, 2.0);",
            "paint_curve(cx, x0, x1, LINE_W);",
        );
        let text = |ranges: &[Range<usize>], text: &str| {
            ranges
                .iter()
                .map(|r| text[r.clone()].to_string())
                .collect::<Vec<_>>()
                .join("|")
        };
        assert_eq!(text(&old, "paint_line(cx, x0, x1, 2.0);"), "paint_line|2.0");
        assert_eq!(
            text(&new, "paint_curve(cx, x0, x1, LINE_W);"),
            "paint_curve|LINE_W"
        );
    }

    #[test]
    fn builds_interleaved_rows_for_a_paired_run() {
        let hunk = Hunk {
            header: "@@ -1,3 +1,4 @@ fn main".to_string(),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![
                line(LineKind::Context, Some(1), Some(1), "one"),
                line(LineKind::Removed, Some(2), None, "two"),
                line(LineKind::Added, None, Some(2), "TWO"),
                line(LineKind::Added, None, Some(3), "three"),
                line(LineKind::Context, Some(3), Some(4), "four"),
            ],
        };
        let rows = build_rows(&[hunk]);
        let kinds: Vec<_> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Hunk,
                RowKind::Context,
                RowKind::Removed,
                RowKind::Added,
                RowKind::Added,
                RowKind::Context,
            ]
        );
        assert_eq!(rows[1].text, "one");
        assert_eq!(rows[2].old_line, Some(2));
        assert_eq!(rows[2].new_line, None);
        assert_eq!(rows[3].new_line, Some(2));
        assert_eq!(rows[3].old_line, None);
        assert_eq!(rows[4].new_line, Some(3));
        // The paired lines highlight the changed words; the extra addition is
        // unpaired and keeps the plain background.
        assert!(!rows[2].emphasis.is_empty());
        assert!(!rows[3].emphasis.is_empty());
        assert!(rows[4].emphasis.is_empty());
    }

    #[test]
    fn a_no_newline_marker_becomes_its_own_row() {
        let mut last = line(LineKind::Added, None, Some(2), "two");
        last.no_newline = true;
        let hunk = Hunk {
            header: "@@ -1 +1,2 @@".to_string(),
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 2,
            lines: vec![line(LineKind::Context, Some(1), Some(1), "one"), last],
        };
        let rows = build_rows(&[hunk]);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2].kind, RowKind::Added);
        assert!(rows[2].no_newline);
        assert_eq!(rows[3].kind, RowKind::Note);
        assert_eq!(rows[3].text, "\\ No newline at end of file");
    }

    #[test]
    fn max_columns_ignores_the_display_width_of_wide_characters() {
        let rows = vec![
            DiffRow::header("@@ -1 +1 @@"),
            DiffRow::line(
                &line(LineKind::Context, Some(1), Some(1), "ab\tc"),
                "ab  c".into(),
                vec![],
            ),
        ];
        assert_eq!(max_columns(&rows), 11);
    }
}
