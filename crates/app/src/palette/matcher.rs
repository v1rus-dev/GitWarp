//! The palette's matcher: a case-insensitive fuzzy subsequence match with a
//! score, and the hash-prefix match used for commits. Pure, so it is unit
//! tested without a window.

use std::ops::Range;

/// A successful match: how good it is and which characters matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Higher is better; only comparable between matches of the same query.
    pub score: i32,
    /// Character (not byte) indices into the matched text, ascending.
    pub positions: Vec<usize>,
}

/// Every matched character.
const CHAR: i32 = 10;
/// The character starts a word (after a separator, or a camelCase hump).
const WORD_START: i32 = 8;
/// The character directly follows the previous matched one.
const CONSECUTIVE: i32 = 6;
/// The match begins at the first character of the text.
const TEXT_START: i32 = 6;
/// The whole query is a prefix of the text.
const PREFIX: i32 = 20;
/// Per skipped character between two matched ones (and before the first).
const GAP: i32 = 1;
/// The most the characters before the first match can cost.
const LEADING_GAP_CAP: i32 = 8;
/// A hash prefix beats every fuzzy score.
const HASH_PREFIX: i32 = 10_000;
/// Shorter hash queries match too many commits to mean "a hash".
pub const MIN_HASH_QUERY: usize = 4;
/// With the explicit `#` scope the user means a commit, so three digits do.
/// (The design states no minimum.)
pub const MIN_HASH_QUERY_SCOPED: usize = 3;

const UNREACHABLE: i32 = i32::MIN / 2;

/// Matches `query` as a subsequence of `text`, ignoring case and whitespace
/// in the query. `None` when some query character has no place left in
/// `text`. An empty query matches everything with score 0.
///
/// The score is the best alignment: it rewards characters that start a word
/// and runs of consecutive characters, and charges for gaps.
pub fn fuzzy(query: &str, text: &str) -> Option<Match> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();
    if !is_subsequence(&query, &folded) {
        return None;
    }

    let n = chars.len();
    let m = query.len();
    // score[i * n + j]: the best score with query[i] matched at text[j].
    // parent[i * n + j]: where query[i - 1] sat in that best alignment.
    let mut score = vec![UNREACHABLE; m * n];
    let mut parent = vec![usize::MAX; m * n];
    // The best way to end query[i - 1] somewhere before j, with the gap since
    // then already charged, and where that was.
    let mut gap_best = vec![UNREACHABLE; n];
    let mut gap_from = vec![usize::MAX; n];

    for i in 0..m {
        if i > 0 {
            let mut best = UNREACHABLE;
            let mut from = usize::MAX;
            for j in 0..n {
                // `best` covers positions before j: age it by one skipped
                // character, then admit the previous column.
                if best > UNREACHABLE {
                    best -= GAP;
                }
                if j > 0 {
                    let previous = score[(i - 1) * n + (j - 1)];
                    if previous > best {
                        best = previous;
                        from = j - 1;
                    }
                }
                gap_best[j] = best;
                gap_from[j] = from;
            }
        }
        for j in i..n {
            if folded[j] != query[i] {
                continue;
            }
            let mut here = CHAR;
            if starts_word(&chars, j) {
                here += WORD_START;
            }
            if i == 0 {
                if j == 0 {
                    here += TEXT_START;
                }
                here -= (j as i32 * GAP).min(LEADING_GAP_CAP);
                score[i * n + j] = here;
            } else {
                let mut best = gap_best[j];
                let mut from = gap_from[j];
                if j > 0 {
                    let run = score[(i - 1) * n + (j - 1)];
                    if run > UNREACHABLE && run + CONSECUTIVE > best {
                        best = run + CONSECUTIVE;
                        from = j - 1;
                    }
                }
                if best <= UNREACHABLE {
                    continue;
                }
                here += best;
                parent[i * n + j] = from;
                score[i * n + j] = here;
            }
        }
    }

    let last = m - 1;
    let (end, best) = (0..n)
        .map(|j| (j, score[last * n + j]))
        .filter(|&(_, s)| s > UNREACHABLE)
        // Ties keep the earliest end.
        .fold(None, |top: Option<(usize, i32)>, (j, s)| match top {
            Some((_, t)) if t >= s => top,
            _ => Some((j, s)),
        })?;

    let mut positions = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = j;
        if i > 0 {
            j = parent[i * n + j];
        }
    }
    let mut total = best;
    if positions.iter().enumerate().all(|(i, &p)| p == i) {
        total += PREFIX;
    }
    Some(Match {
        score: total,
        positions,
    })
}

/// Matches a commit hash typed as a prefix (`e4f2a`). `None` unless the
/// query is at least [`MIN_HASH_QUERY`] hex digits ([`MIN_HASH_QUERY_SCOPED`]
/// when `scoped`, i.e. typed after `#`) and `hash` starts with it.
/// The score beats any fuzzy score, so a hash hit leads the Commits group.
pub fn hash_prefix(query: &str, hash: &str, scoped: bool) -> Option<Match> {
    let query = query.trim();
    let min = if scoped {
        MIN_HASH_QUERY_SCOPED
    } else {
        MIN_HASH_QUERY
    };
    if query.len() < min || !query.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let starts = hash.len() >= query.len()
        && hash.as_bytes()[..query.len()].eq_ignore_ascii_case(query.as_bytes());
    starts.then(|| Match {
        score: HASH_PREFIX,
        positions: (0..query.len()).collect(),
    })
}

/// Turns matched character indices into merged byte ranges of `text`, for
/// painting the highlight.
pub fn byte_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &position in positions {
        let (Some(&start), Some(&end)) = (offsets.get(position), offsets.get(position + 1)) else {
            continue;
        };
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_subsequence(query: &[char], text: &[char]) -> bool {
    let mut next = query.iter().peekable();
    for c in text {
        if next.peek() == Some(&c) {
            next.next();
        }
    }
    next.peek().is_none()
}

/// A word starts at the text's start, after anything that is not a letter or
/// digit, and at a lower-to-upper (camelCase) or letter-to-digit boundary.
fn starts_word(chars: &[char], index: usize) -> bool {
    let Some(before) = index.checked_sub(1).map(|i| chars[i]) else {
        return true;
    };
    let c = chars[index];
    !before.is_alphanumeric()
        || (before.is_lowercase() && c.is_uppercase())
        || (before.is_alphabetic() && c.is_numeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, text: &str) -> i32 {
        fuzzy(query, text)
            .unwrap_or_else(|| panic!("{query:?} should match {text:?}"))
            .score
    }

    #[test]
    fn a_subsequence_matches_case_insensitively_and_reports_positions() {
        let m = fuzzy("psh", "Push current branch").expect("matches");
        assert_eq!(m.positions, vec![0, 2, 3]);
        assert_eq!(
            fuzzy("PUSH", "push").map(|m| m.positions),
            Some(vec![0, 1, 2, 3])
        );
        // Whitespace in the query is ignored, so words may be typed apart.
        assert_eq!(
            fuzzy("push cur", "Push current branch").map(|m| m.positions),
            Some(vec![0, 1, 2, 3, 5, 6, 7])
        );
    }

    #[test]
    fn a_missing_or_misordered_character_is_no_match() {
        assert_eq!(fuzzy("xyz", "Push"), None);
        assert_eq!(fuzzy("hsp", "Push"), None);
        assert_eq!(fuzzy("pushh", "Push"), None);
    }

    #[test]
    fn an_empty_query_matches_everything() {
        let m = fuzzy("  ", "anything").expect("matches");
        assert_eq!((m.score, m.positions), (0, Vec::new()));
    }

    #[test]
    fn word_starts_beat_the_middle_of_a_word() {
        // Both texts contain f…b; only the first has them at word starts.
        assert!(score("fb", "Fetch branch") > score("fb", "Afterbirth"));
        // The camelCase hump counts as a word start.
        assert!(score("sb", "toggleSidebar") > score("sb", "absorb"));
    }

    #[test]
    fn consecutive_runs_beat_scattered_characters() {
        assert!(score("bra", "abranch") > score("bra", "abxrxa"));
    }

    #[test]
    fn a_prefix_beats_the_same_letters_later() {
        assert!(score("push", "Push current branch") > score("push", "Do not push"));
    }

    #[test]
    fn the_best_alignment_is_found_not_the_leftmost() {
        // The leftmost `b` is inside "ab"; the better one starts "bar".
        let m = fuzzy("ba", "ab bar").expect("matches");
        assert_eq!(m.positions, vec![3, 4]);
    }

    #[test]
    fn a_hash_prefix_matches_and_beats_any_fuzzy_score() {
        let hash = "e4f2a91c0d3b5a6978e1f2d3c4b5a69788776655";
        let m = hash_prefix("E4F2a", hash, false).expect("prefix");
        assert_eq!(m.positions, vec![0, 1, 2, 3, 4]);
        assert!(m.score > score("e4f2a", "e4f2a91  Merge branch"));
    }

    #[test]
    fn only_a_long_enough_hex_prefix_counts_as_a_hash() {
        let hash = "e4f2a91c0d3b5a6978e1f2d3c4b5a69788776655";
        assert_eq!(hash_prefix("e4f", hash, false), None);
        assert_eq!(hash_prefix("f2a9", hash, false), None);
        assert_eq!(hash_prefix("e4g2", hash, false), None);
        assert_eq!(hash_prefix("", hash, false), None);
        // After `#` three digits are enough, two are not.
        assert!(hash_prefix("e4f", hash, true).is_some());
        assert_eq!(hash_prefix("e4", hash, true), None);
        assert_eq!(hash_prefix("e4g", hash, true), None);
    }

    #[test]
    fn matched_characters_become_merged_byte_ranges() {
        assert_eq!(byte_ranges("Push now", &[0, 1, 2, 5]), vec![0..3, 5..6]);
        // Multi-byte characters: positions count characters, ranges bytes.
        assert_eq!(byte_ranges("ünï x", &[0, 2]), vec![0..2, 3..5]);
        assert_eq!(byte_ranges("ab", &[5]), Vec::<Range<usize>>::new());
    }
}
