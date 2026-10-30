//! Plain-text rendering of laid-out rows, for tests, debugging and the CLI
//! benchmark. Two text lines per commit: the commit line and the connector
//! line below it.

use crate::Row;

/// Render rows like `git log --graph`, with a label after each commit.
pub fn render<Id>(rows: &[Row<Id>], mut label: impl FnMut(&Row<Id>) -> String) -> String {
    let mut out = String::new();
    for row in rows {
        let width = row.width;
        // Children from other columns converging into this commit.
        if row.above.iter().any(|s| s.from != s.to) {
            let mut join = vec![' '; width * 2];
            for s in &row.above {
                if s.from == s.to {
                    put(&mut join, s.from * 2, '│');
                }
            }
            for s in row.above.iter().filter(|s| s.from != s.to) {
                let (lo, hi) = (s.from.min(s.to), s.from.max(s.to));
                for c in lo * 2 + 1..hi * 2 {
                    put(&mut join, c, '─');
                }
                put(&mut join, s.from * 2, if s.from > s.to { '╯' } else { '╰' });
            }
            let has_straight = row
                .above
                .iter()
                .any(|s| s.from == row.column && s.to == row.column);
            let c = row.column * 2;
            join[c] = match (has_straight, row.above.iter().any(|s| s.from > s.to)) {
                (true, _) => '├',
                (false, true) => '╭',
                (false, false) => '╮',
            };
            out.push_str(join.iter().collect::<String>().trim_end());
            out.push('\n');
        }
        // Commit line: the dot and every lane passing by.
        let mut line = vec![' '; width * 2];
        for s in &row.above {
            if s.from == s.to && s.from != row.column {
                line[s.from * 2] = '│';
            }
        }
        line[row.column * 2] = if row.is_merge { '◆' } else { '●' };
        out.extend(line.iter());
        out.push(' ');
        out.push_str(&label(row));
        out.push('\n');

        // Connector line: where lines go on the way to the next row.
        if row.below.is_empty() {
            continue;
        }
        let mut conn = vec![' '; width * 2];
        for s in &row.below {
            use std::cmp::Ordering::*;
            match s.to.cmp(&s.from) {
                Equal => put(&mut conn, s.from * 2, '│'),
                Greater => {
                    for c in s.from * 2 + 1..s.to * 2 {
                        put(&mut conn, c, '─');
                    }
                    put(&mut conn, s.to * 2, '╮');
                }
                Less => {
                    for c in s.to * 2 + 1..s.from * 2 {
                        put(&mut conn, c, '─');
                    }
                    put(&mut conn, s.to * 2, '╭');
                }
            }
        }
        // Lines from the dot sideways start at the dot itself. A lane bends
        // left when it joins a waiting lane further left, so the corner
        // depends on the direction, not just on whether a line also goes down.
        let mut straight = false;
        let mut left = false;
        let mut right = false;
        for s in &row.below {
            if s.from != row.column {
                continue;
            }
            match s.to.cmp(&row.column) {
                std::cmp::Ordering::Equal => straight = true,
                std::cmp::Ordering::Less => left = true,
                std::cmp::Ordering::Greater => right = true,
            }
        }
        if left || right {
            conn[row.column * 2] = dot_connector(straight, left, right);
        }
        out.push_str(conn.iter().collect::<String>().trim_end());
        out.push('\n');
    }
    out
}

fn put(line: &mut [char], at: usize, c: char) {
    let cur = line[at];
    line[at] = match (cur, c) {
        (' ', c) => c,
        ('│', '─') | ('─', '│') => '┼',
        ('│', '╮') => '┤',
        ('│', '╭') => '├',
        (cur, '─') => cur,
        (_, c) => c,
    };
}

/// Character at the commit dot on the connector line. `straight` is a lane
/// going down, `left`/`right` are lanes going sideways from the dot.
fn dot_connector(straight: bool, left: bool, right: bool) -> char {
    match (straight, left, right) {
        (true, true, true) => '┼',
        (true, true, false) => '┤',
        (true, false, true) => '├',
        (false, true, true) => '┴',
        (true, false, false) => '│',
        (false, true, false) => '╯',
        (false, false, true) => '╰',
        (false, false, false) => '│',
    }
}
