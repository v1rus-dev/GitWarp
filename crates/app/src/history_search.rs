//! The history search's matching rules.
//!
//! The view owns the debounce, the background scan, the selection and the
//! highlight. Everything here is pure and unit-tested: a query is a
//! case-insensitive substring of the summary, the author name and the author
//! e-mail, plus a prefix of the commit hash when the trimmed query is 4–40 hex
//! characters.
//!
//! Ranges are byte ranges into the original strings so the renderer can hand
//! them to GPUI text runs. Folding a character with `char::to_lowercase` can
//! change its byte length (`İ` → `i̇`), so the matcher compares characters and
//! keeps the original byte offsets instead of searching a lowercased copy.

use std::ops::Range;

use git_core::CommitInfo;

/// The shortest query that is tried as a hash prefix.
const HASH_MIN: usize = 4;
/// A full git hash; longer hex strings are only searched as text.
const HASH_MAX: usize = 40;

/// A trimmed, case-folded query. `None` from [`Query::parse`] means the field
/// is empty and the history is in its normal, unsearched state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The trimmed query, lowercased and split into characters.
    chars: Vec<char>,
    /// The prefix to match against the commit hash, when the query is hex.
    hash: Option<String>,
}

impl Query {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let hash = if (HASH_MIN..=HASH_MAX).contains(&trimmed.len())
            && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            Some(trimmed.to_ascii_lowercase())
        } else {
            None
        };
        Some(Self {
            // Fold character by character, like `match_at` folds the haystack:
            // `str::to_lowercase` uses the final-sigma rule (`Σ` at the end of
            // a word becomes `ς`), which the per-character haystack never
            // produces.
            chars: trimmed.chars().flat_map(char::to_lowercase).collect(),
            hash,
        })
    }
}

/// The matched byte ranges of one commit, per field. The author cell only
/// shows the name; an e-mail match still makes the row a match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldMatches {
    /// Every non-overlapping occurrence in the summary.
    pub summary: Vec<Range<usize>>,
    /// Every non-overlapping occurrence in the author name.
    pub author_name: Vec<Range<usize>>,
    /// Every non-overlapping occurrence in the author e-mail.
    pub author_email: Vec<Range<usize>>,
    /// The prefix of the full hash, starting at byte 0.
    pub hash: Option<Range<usize>>,
}

/// One matching row and where the query matches in its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMatch {
    pub row: usize,
    pub fields: FieldMatches,
}

/// Every matching row in order. `rows` yields `(row index, commit)` pairs so
/// the caller can scan a page starting at any offset.
pub fn search<'a>(
    query: &Query,
    rows: impl IntoIterator<Item = (usize, &'a CommitInfo)>,
) -> Vec<HistoryMatch> {
    rows.into_iter()
        .filter_map(|(row, commit)| match_commit(query, row, commit))
        .collect()
}

/// One row, or `None` when no field matches.
pub fn match_commit(query: &Query, row: usize, commit: &CommitInfo) -> Option<HistoryMatch> {
    let summary = occurrences(&commit.summary, &query.chars);
    let author_name = occurrences(&commit.author_name, &query.chars);
    let author_email = occurrences(&commit.author_email, &query.chars);
    let hash = query.hash.as_ref().and_then(|prefix| {
        commit
            .id
            .to_string()
            .starts_with(prefix)
            .then_some(0..prefix.len())
    });
    if summary.is_empty() && author_name.is_empty() && author_email.is_empty() && hash.is_none() {
        return None;
    }
    Some(HistoryMatch {
        row,
        fields: FieldMatches {
            summary,
            author_name,
            author_email,
            hash,
        },
    })
}

/// The first match at or after `selected`, wrapping at the end. Returns the
/// position in `matches`, or `None` when there is nothing to jump to.
pub fn first_from(matches: &[HistoryMatch], selected: Option<usize>) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let start = selected
        .map(|row| matches.partition_point(|m| m.row < row))
        .unwrap_or(0);
    Some(start % matches.len())
}

/// The next match after `selected` (Enter / F3), or the previous one before it
/// (Shift+Enter / Shift+F3), both wrapping. A selection that is not on a match
/// counts as sitting between the two matches around it.
pub fn step(matches: &[HistoryMatch], selected: Option<usize>, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let len = matches.len();
    match selected {
        Some(row) if forward => Some(matches.partition_point(|m| m.row <= row) % len),
        Some(row) => Some((matches.partition_point(|m| m.row < row) + len - 1) % len),
        None if forward => Some(0),
        None => Some(len - 1),
    }
}

/// The position of the selected row among the matches, for the "3 / 17"
/// count; `None` when the selection is not on a match.
pub fn position_of(matches: &[HistoryMatch], row: usize) -> Option<usize> {
    matches.binary_search_by_key(&row, |m| m.row).ok()
}

/// Every non-overlapping occurrence of `needle` in `haystack`, left to right.
fn occurrences(haystack: &str, needle: &[char]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut from = 0;
    while from < haystack.len() {
        let mut hit = None;
        for (offset, _) in haystack[from..].char_indices() {
            let start = from + offset;
            if let Some(end) = match_at(haystack, start, needle) {
                hit = Some(start..end);
                break;
            }
        }
        let Some(range) = hit else {
            break;
        };
        if range.is_empty() {
            break;
        }
        from = range.end;
        found.push(range);
    }
    found
}

/// Whether the query starts at `start`, compared character by character with
/// case folding. Returns the byte offset just past the match in the original
/// string.
///
/// A character may lowercase into several characters (`İ` → `i̇`): the query
/// ending inside that expansion still counts as a match and covers the whole
/// original character, so the range stays on a UTF-8 boundary.
fn match_at(haystack: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut needle_pos = 0;
    let mut pos = start;
    while needle_pos < needle.len() {
        let character = haystack[pos..].chars().next()?;
        // The common path: one ASCII character folds to one ASCII character.
        if character.is_ascii() {
            if character.to_ascii_lowercase() != needle[needle_pos] {
                return None;
            }
            needle_pos += 1;
            pos += 1;
            continue;
        }
        for lower in character.to_lowercase() {
            if needle_pos == needle.len() {
                // The query ended inside this character's lowercase form.
                return Some(pos + character.len_utf8());
            }
            if needle[needle_pos] != lower {
                return None;
            }
            needle_pos += 1;
        }
        pos += character.len_utf8();
    }
    Some(pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_core::Oid;

    fn commit(summary: &str, name: &str, email: &str, hash: &str) -> CommitInfo {
        CommitInfo {
            id: Oid::from_hex(hash).expect("a 40-char hex hash"),
            parents: Vec::new(),
            author_name: name.to_owned(),
            author_email: email.to_owned(),
            author_time: 0,
            summary: summary.to_owned(),
        }
    }

    fn matched(query: &str, commit: &CommitInfo) -> Option<HistoryMatch> {
        let query = Query::parse(query).expect("a non-empty query");
        match_commit(&query, 7, commit)
    }

    #[test]
    fn empty_or_whitespace_queries_are_not_a_search() {
        assert_eq!(Query::parse(""), None);
        assert_eq!(Query::parse(" \t\n "), None);
        assert!(Query::parse("  a  ").is_some());
    }

    #[test]
    fn the_summary_author_name_and_email_match_case_insensitively() {
        let c = commit(
            "Fix the Parser",
            "Mira Okonkwo",
            "Mira@Example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );

        let summary = matched("PARSER", &c).expect("the summary matches");
        assert_eq!(summary.row, 7);
        assert_eq!(summary.fields.summary, vec![8..14]);
        assert_eq!(&c.summary[summary.fields.summary[0].clone()], "Parser");
        assert!(summary.fields.author_name.is_empty());

        let name = matched("okonkwo", &c).expect("the author name matches");
        assert_eq!(name.fields.author_name, vec![5..12]);
        assert_eq!(
            &c.author_name[name.fields.author_name[0].clone()],
            "Okonkwo"
        );

        let email = matched("EXAMPLE", &c).expect("the e-mail matches");
        assert_eq!(email.fields.author_email, vec![5..12]);
    }

    #[test]
    fn every_occurrence_in_a_field_is_reported() {
        let c = commit(
            "ab one ab",
            "ab",
            "x@y",
            "0123456789abcdef0123456789abcdef01234567",
        );
        let found = matched("ab", &c).expect("the summary matches");
        // No hash match for a two-character query.
        assert_eq!(found.fields.summary, vec![0..2, 7..9]);
        assert_eq!(found.fields.author_name, vec![0..2]);
        assert_eq!(found.fields.hash, None);
    }

    #[test]
    fn a_hash_prefix_matches_only_for_four_to_forty_hex_characters() {
        let c = commit(
            "Release 012 and 4567",
            "Test User",
            "test@example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );

        // Three hex characters are text, not a hash.
        let three = matched("012", &c).expect("three hex chars still match the text");
        assert_eq!(three.fields.summary, vec![8..11]);
        assert_eq!(three.fields.hash, None);
        // Four do match the start of the full hash.
        let four = matched("0123", &c).expect("the hash prefix matches");
        assert_eq!(four.fields.hash, Some(0..4));
        assert!(four.fields.summary.is_empty());
        // Uppercase hex still matches, case-insensitively.
        let hex = commit(
            "A commit",
            "Test User",
            "test@example.com",
            "abcdef0123456789abcdef0123456789abcdef01",
        );
        assert_eq!(matched("ABCDEF", &hex).unwrap().fields.hash, Some(0..6));
        // Only the start of the hash: a later substring is not a prefix, but
        // it is still text.
        assert_eq!(matched("4567", &c).unwrap().fields.hash, None);
        // The full hash is a 40-character prefix; 41 characters are text.
        let full = c.id.to_string();
        assert_eq!(
            matched(&full, &c)
                .expect("the full hash matches")
                .fields
                .hash,
            Some(0..40)
        );
        assert_eq!(
            matched(&format!("{full}0"), &c).map(|m| m.fields.hash),
            None
        );
        // A hex-looking query that is not in the hash does not match either.
        assert_eq!(matched("dead", &c), None);
    }

    #[test]
    fn utf8_match_ranges_stay_on_character_boundaries() {
        let c = commit(
            "Ünïcode parser",
            "Élodie Ünïcode",
            "elodie@example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );

        // Seven characters, nine bytes: the range covers the whole "Ünïcode".
        let found = matched("ünïcode", &c).expect("the summary matches");
        let range = found.fields.summary[0].clone();
        assert_eq!(range, 0..9);
        assert_eq!(&c.summary[range], "Ünïcode");

        let name = matched("ÉLODIE", &c).expect("the author name matches");
        let range = name.fields.author_name[0].clone();
        assert_eq!(&c.author_name[range], "Élodie");

        for range in &found.fields.summary {
            assert!(c.summary.is_char_boundary(range.start));
            assert!(c.summary.is_char_boundary(range.end));
        }
        for range in &name.fields.author_name {
            assert!(c.author_name.is_char_boundary(range.start));
            assert!(c.author_name.is_char_boundary(range.end));
        }
    }

    #[test]
    fn a_query_ending_inside_a_lowercase_expansion_covers_the_character() {
        // `İ` lowercases to "i" + combining dot: the query "i" matches the
        // whole two-byte character and the range still splits it cleanly.
        let c = commit(
            "İstanbul",
            "Name",
            "mail@example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );
        let found = matched("i", &c).expect("the summary matches");
        assert_eq!(found.fields.summary, vec![0..2]);
        assert_eq!(&c.summary[found.fields.summary[0].clone()], "İ");
    }

    #[test]
    fn a_final_capital_sigma_matches_the_per_character_fold() {
        // `str::to_lowercase("ΟΔΟΣ")` ends in the final sigma `ς`, while the
        // haystack folds each `Σ` to `σ`; the needle must fold the same way.
        let c = commit(
            "ΟΔΟΣ parser",
            "Name",
            "mail@example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );
        let found = matched("ΟΔΟΣ", &c).expect("the summary matches");
        assert_eq!(found.fields.summary, vec![0..8]);
    }

    #[test]
    fn an_unrelated_query_does_not_match() {
        let c = commit(
            "Fix the parser",
            "Mira",
            "mira@example.com",
            "0123456789abcdef0123456789abcdef01234567",
        );
        assert_eq!(matched("zzz", &c), None);
    }

    fn matches_at(rows: &[usize]) -> Vec<HistoryMatch> {
        rows.iter()
            .map(|&row| HistoryMatch {
                row,
                fields: FieldMatches::default(),
            })
            .collect()
    }

    #[test]
    fn typing_takes_the_first_match_at_or_after_the_selection() {
        let matches = matches_at(&[1, 4, 7]);
        assert_eq!(first_from(&matches, None), Some(0));
        assert_eq!(first_from(&matches, Some(0)), Some(0));
        assert_eq!(first_from(&matches, Some(2)), Some(1));
        // A selection that is itself a match stays on it.
        assert_eq!(first_from(&matches, Some(4)), Some(1));
        assert_eq!(first_from(&matches, Some(8)), Some(0));
        assert_eq!(first_from(&[], Some(3)), None);
    }

    #[test]
    fn next_and_previous_step_through_the_matches_and_wrap() {
        let matches = matches_at(&[1, 4, 7]);
        // From no selection: the first match forward, the last one back.
        assert_eq!(
            step(&matches, None, true).map(|pos| matches[pos].row),
            Some(1)
        );
        assert_eq!(
            step(&matches, None, false).map(|pos| matches[pos].row),
            Some(7)
        );
        // On a match: the next / previous one, both wrapping.
        assert_eq!(
            step(&matches, Some(1), true).map(|pos| matches[pos].row),
            Some(4)
        );
        assert_eq!(
            step(&matches, Some(7), true).map(|pos| matches[pos].row),
            Some(1)
        );
        assert_eq!(
            step(&matches, Some(4), false).map(|pos| matches[pos].row),
            Some(1)
        );
        assert_eq!(
            step(&matches, Some(1), false).map(|pos| matches[pos].row),
            Some(7)
        );
        // Between matches: the one after / before, wrapping at the ends.
        assert_eq!(
            step(&matches, Some(2), true).map(|pos| matches[pos].row),
            Some(4)
        );
        assert_eq!(
            step(&matches, Some(2), false).map(|pos| matches[pos].row),
            Some(1)
        );
        assert_eq!(
            step(&matches, Some(8), true).map(|pos| matches[pos].row),
            Some(1)
        );
        assert_eq!(
            step(&matches, Some(0), false).map(|pos| matches[pos].row),
            Some(7)
        );
        assert_eq!(step(&[], Some(2), true), None);
    }

    #[test]
    fn the_count_position_is_only_on_a_match() {
        let matches = matches_at(&[1, 4, 7]);
        assert_eq!(position_of(&matches, 1), Some(0));
        assert_eq!(position_of(&matches, 7), Some(2));
        assert_eq!(position_of(&matches, 2), None);
    }

    #[test]
    fn search_scans_a_slice_starting_at_an_offset() {
        let commits = [
            commit(
                "needle one",
                "a",
                "a@x",
                "0123456789abcdef0123456789abcdef01234567",
            ),
            commit(
                "needle two",
                "b",
                "b@x",
                "1123456789abcdef0123456789abcdef01234567",
            ),
            commit(
                "nothing",
                "c",
                "c@x",
                "2123456789abcdef0123456789abcdef01234567",
            ),
        ];
        let query = Query::parse("needle").unwrap();
        let found = search(&query, commits.iter().enumerate().skip(1));
        assert_eq!(
            found.iter().map(|m| m.row).collect::<Vec<_>>(),
            vec![1],
            "the scan must not renumber the rows of a later page"
        );
    }

    /// The brief asks for a measurement on 100k rows. Ignored so the normal
    /// suite stays fast; run with `cargo test -p app -- --ignored --nocapture
    /// a_hundred_thousand_rows`.
    #[test]
    #[ignore = "measurement, not a correctness gate"]
    fn a_hundred_thousand_rows_scan_quickly() {
        let commits: Vec<CommitInfo> = (0..100_000)
            .map(|i| CommitInfo {
                id: Oid::from_hex(&format!("{i:040x}")).unwrap(),
                parents: Vec::new(),
                author_name: format!("Author {i}"),
                author_email: format!("author{i}@example.com"),
                author_time: 0,
                summary: format!("Commit {i} fixes the history parser"),
            })
            .collect();
        let query = Query::parse("parser").unwrap();
        let start = std::time::Instant::now();
        let found = search(&query, commits.iter().enumerate());
        eprintln!(
            "search of {} rows took {:?} and found {} matches",
            commits.len(),
            start.elapsed(),
            found.len()
        );
        assert_eq!(found.len(), commits.len());
    }
}
