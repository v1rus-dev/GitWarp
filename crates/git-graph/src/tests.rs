use super::*;

fn rows(history: &[(&'static str, &[&'static str])]) -> Vec<Row<&'static str>> {
    let mut b = GraphBuilder::new();
    history.iter().map(|(id, p)| b.push(*id, p)).collect()
}

fn seg(from: usize, to: usize) -> (usize, usize) {
    (from, to)
}

fn shape(segments: &[Segment]) -> Vec<(usize, usize)> {
    segments.iter().map(|s| (s.from, s.to)).collect()
}

/// Every segment leaving a row at the bottom must arrive at the next row's top
/// at the same column, and nothing may appear out of nowhere.
fn assert_continuous(rows: &[Row<&str>]) {
    for pair in rows.windows(2) {
        let mut out: Vec<usize> = pair[0].below.iter().map(|s| s.to).collect();
        let mut inc: Vec<usize> = pair[1].above.iter().map(|s| s.from).collect();
        out.sort();
        out.dedup();
        inc.sort();
        inc.dedup();
        assert_eq!(
            out, inc,
            "broken line between {} and {}",
            pair[0].id, pair[1].id
        );
    }
}

#[test]
fn linear_history_is_one_straight_lane() {
    let r = rows(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
    assert!(r.iter().all(|row| row.column == 0 && row.width == 1));
    assert!(r[0].is_tip && !r[1].is_tip && !r[2].is_tip);
    assert!(r[0].above.is_empty());
    assert_eq!(shape(&r[0].below), vec![seg(0, 0)]);
    assert_eq!(shape(&r[1].above), vec![seg(0, 0)]);
    assert!(r[2].below.is_empty(), "root commit has nothing below");
    assert!(r.iter().all(|row| row.color == r[0].color));
    assert_continuous(&r);
}

#[test]
fn branch_and_merge() {
    //  m      merge of b2 into a2
    //  |\
    //  a2 |
    //  |  b2
    //  |  b1
    //  |/
    //  a1
    let r = rows(&[
        ("m", &["a2", "b2"]),
        ("a2", &["a1"]),
        ("b2", &["b1"]),
        ("b1", &["a1"]),
        ("a1", &[]),
    ]);
    assert!(r[0].is_merge);
    assert_eq!(shape(&r[0].below), vec![seg(0, 0), seg(0, 1)]);
    assert_eq!(r[1].column, 0);
    assert_eq!(r[2].column, 1);
    assert_eq!(r[3].column, 1);
    assert_eq!(r[4].column, 0);
    // D1: b1's line bends left into a1's waiting lane on b1's own row.
    assert_eq!(shape(&r[3].below), vec![seg(0, 0), seg(1, 0)]);
    // Both branches are already joined by then, so a1 has one incoming lane.
    assert_eq!(shape(&r[4].above), vec![seg(0, 0)]);
    assert_eq!(r[4].width, 1);
    // The merged branch keeps one color for its whole length.
    assert_eq!(r[2].color, r[3].color);
    assert_ne!(r[2].color, r[1].color);
    assert_continuous(&r);
}

#[test]
fn second_tip_bends_into_the_waiting_lane() {
    // x waits for `base` in column 0, so D1 makes y's line bend into that
    // lane on y's own row; y's column stays free and `base` has one incoming
    // lane.
    let r = rows(&[("x", &["base"]), ("y", &["base"]), ("base", &[])]);
    assert_eq!(r[0].column, 0);
    assert_eq!(r[1].column, 1);
    assert!(r[0].is_tip && r[1].is_tip);
    assert_eq!(shape(&r[1].above), vec![seg(0, 0)]);
    assert_eq!(shape(&r[1].below), vec![seg(0, 0), seg(1, 0)]);
    assert_eq!(r[2].column, 0);
    assert_eq!(shape(&r[2].above), vec![seg(0, 0)]);
    assert_eq!(r[2].width, 1);
    assert_continuous(&r);

    assert_eq!(
        ascii::render(&r, |row| row.id.to_string()),
        "●  x\n│\n│ ●  y\n├─╯\n●  base\n"
    );
}

#[test]
fn merge_opens_a_lane_left_of_the_merge_when_a_column_is_free() {
    // `a` frees column 0 while `m` waits in column 1. D2 opens the merged-in
    // lane for q there, left of the merge commit, instead of appending a
    // third column on the right.
    let r = rows(&[
        ("x", &["a"]),
        ("y", &["m"]),
        ("a", &[]),
        ("m", &["p", "q"]),
        ("p", &[]),
        ("q", &[]),
    ]);
    assert_eq!(r[3].column, 1);
    // The first parent continues in column 1; the new lane opens in column 0.
    assert_eq!(shape(&r[3].below), vec![seg(1, 0), seg(1, 1)]);
    assert_eq!(r[3].width, 2);
    assert_eq!(r[4].column, 1);
    assert_eq!(r[5].column, 0);
    assert_continuous(&r);

    assert_eq!(
        ascii::render(&r, |row| row.id.to_string()),
        "●  x\n│\n│ ●  y\n│ │\n● │  a\n  │\n  ◆  m\n╭─┤\n│ ●  p\n│\n●  q\n"
    );
}

#[test]
fn octopus_merge_opens_a_lane_per_parent() {
    let r = rows(&[
        ("m", &["a", "b", "c"]),
        ("a", &["root"]),
        ("b", &["root"]),
        ("c", &["root"]),
        ("root", &[]),
    ]);
    assert_eq!(shape(&r[0].below), vec![seg(0, 0), seg(0, 1), seg(0, 2)]);
    assert_eq!([r[1].column, r[2].column, r[3].column], [0, 1, 2]);
    // D1: b and c bend into a's waiting lane on their own rows.
    assert_eq!(shape(&r[2].below), vec![seg(0, 0), seg(1, 0), seg(2, 2)]);
    assert_eq!(shape(&r[3].below), vec![seg(0, 0), seg(2, 0)]);
    assert_eq!(shape(&r[4].above), vec![seg(0, 0)]);
    assert_eq!(r[4].width, 1);
    assert_eq!(r[0].width, 3);
    assert_continuous(&r);
}

#[test]
fn merge_parent_already_waited_for_joins_existing_lane() {
    // `side` is reachable from the tip `s` and is merged into `m`:
    // the merge must connect into s's lane instead of opening a third one.
    let r = rows(&[
        ("s", &["side"]),
        ("m", &["main", "side"]),
        ("side", &["main"]),
        ("main", &[]),
    ]);
    assert_eq!(r[0].column, 0);
    assert_eq!(r[1].column, 1);
    assert_eq!(shape(&r[1].below), vec![seg(0, 0), seg(1, 0), seg(1, 1)]);
    assert!(r.iter().all(|row| row.width <= 2));
    assert_continuous(&r);
}

#[test]
fn freed_columns_are_reused() {
    let r = rows(&[
        ("m1", &["a", "b"]),
        ("b", &["a"]),
        ("a", &["m0"]),
        ("m0", &["z", "y"]),
        ("y", &["z"]),
        ("z", &[]),
    ]);
    // After the first branch closes, the second one takes column 1 again.
    assert_eq!(r[1].column, 1);
    assert_eq!(r[4].column, 1);
    assert!(r.iter().all(|row| row.width <= 2));
    assert_continuous(&r);
}

#[test]
fn unrelated_roots_get_their_own_lanes() {
    let r = rows(&[("a2", &["a1"]), ("b1", &[]), ("a1", &[])]);
    assert_eq!(r[1].column, 1);
    assert!(r[1].is_tip);
    assert!(
        r[1].below.iter().all(|s| s.from != 1),
        "orphan root ends its lane"
    );
    assert_eq!(r[2].column, 0);
    assert_continuous(&r);
}

#[test]
fn duplicate_parents_draw_a_single_line() {
    let r = rows(&[("m", &["a", "a"]), ("a", &[])]);
    assert_eq!(shape(&r[0].below), vec![seg(0, 0)]);
    assert_continuous(&r);
}

#[test]
fn open_lanes_stay_bounded_on_long_linear_history() {
    let ids: Vec<u32> = (0..100_000).rev().collect();
    let mut b = GraphBuilder::new();
    for w in ids.windows(2) {
        let row = b.push(w[0], &[w[1]]);
        assert_eq!(row.column, 0);
    }
    b.push(0, &[]);
    assert_eq!(b.len(), 100_000);
    assert_eq!(b.open_lanes(), 0);
}

fn idle(history: &[(&'static str, &[&'static str])]) -> IdleLanes {
    let mut idle = IdleLanes::new();
    for row in rows(history) {
        idle.push(&row);
    }
    idle
}

fn feed(
    b: &mut GraphBuilder<&'static str>,
    idle: &mut IdleLanes,
    id: &'static str,
    parents: &[&'static str],
) {
    let row = b.push(id, parents);
    idle.push(&row);
}

#[test]
fn geometry_without_collapse_is_lane_width_per_column() {
    let g = idle(&[
        ("m", &["a", "b", "c"]),
        ("a", &["root"]),
        ("b", &["root"]),
        ("c", &["root"]),
        ("root", &[]),
    ])
    .geometry(false);
    assert_eq!(g.xs, vec![7.0, 21.0, 35.0]);
    assert_eq!(g.folded, vec![false, false, false]);
    assert_eq!(g.width, 42.0);
    assert_eq!(g.folded_count, 0);
}

#[test]
fn three_idle_lanes_fold_into_one_gap() {
    // Column 0 is the main line (a merge dot, so never idle). b, c and d each
    // occupy one column, passing rows included, and then join column 0 on
    // their own row: one non-merge dot per lane instance. The three adjacent
    // columns therefore fold into one 12 px gap.
    let g = idle(&[
        ("m", &["a", "b", "c", "d"]),
        ("a", &["root"]),
        ("b", &["root"]),
        ("c", &["root"]),
        ("d", &["root"]),
        ("root", &[]),
    ])
    .geometry(true);
    assert_eq!(g.folded, vec![false, true, true, true]);
    assert_eq!(g.xs, vec![7.0, 20.0, 20.0, 20.0]);
    assert_eq!(g.width, 26.0);
    assert_eq!(g.folded_count, 3);
}

#[test]
fn two_idle_lanes_do_not_fold() {
    let g = idle(&[
        ("m", &["a", "b", "c"]),
        ("a", &["root"]),
        ("b", &["root"]),
        ("c", &["root"]),
        ("root", &[]),
    ])
    .geometry(true);
    assert_eq!(g.folded, vec![false, false, false]);
    assert_eq!(g.xs, vec![7.0, 21.0, 35.0]);
    assert_eq!(g.width, 42.0);
    assert_eq!(g.folded_count, 0);
}

#[test]
fn a_column_that_once_held_two_commits_never_folds() {
    // Column 1 first carries y and base (two dots), so it is already
    // non-foldable when it is reused later by the single-commit tip q.
    let g = idle(&[
        ("x", &["x2"]),
        ("y", &["base"]),
        ("x2", &["base"]),
        ("base", &[]),
        ("p", &["root2"]),
        ("q", &["root2"]),
        ("root2", &[]),
    ])
    .geometry(true);
    assert!(!g.folded[1], "a reused non-idle column must not fold");
    assert_eq!(g.folded_count, 0);
    assert_eq!(g.xs, vec![7.0, 21.0]);
    assert_eq!(g.width, 28.0);
}

#[test]
fn a_column_reused_back_to_back_splits_into_separate_instances() {
    // Columns 1..3 each carry one idle instance: m opens them, b, c and d sit
    // on them and close into the main line. Column 1 closes on b's row and the
    // very next row reuses it for the tip t, so the two lanes touch the column
    // back to back. They are two one-dot instances, not one two-dot instance,
    // so the column stays foldable and the run still folds.
    let g = idle(&[
        ("m", &["a", "b", "c", "d"]),
        ("a", &["z"]),
        ("b", &["z"]),
        ("t", &["z"]),
        ("c", &["z"]),
        ("d", &["z"]),
        ("z", &[]),
    ])
    .geometry(true);
    assert_eq!(g.folded, vec![false, true, true, true]);
    assert_eq!(g.xs, vec![7.0, 20.0, 20.0, 20.0]);
    assert_eq!(g.width, 26.0);
    assert_eq!(g.folded_count, 3);
}

#[test]
fn a_reused_column_with_a_two_commit_lane_does_not_fold() {
    // Same shape, but b's lane carries b and z2 down column 1: that one
    // instance holds two dots, so the column is not idle and the fold breaks.
    let g = idle(&[
        ("m", &["a", "b", "c", "d"]),
        ("a", &["z"]),
        ("b", &["z2"]),
        ("z2", &["z"]),
        ("t", &["z"]),
        ("c", &["z"]),
        ("d", &["z"]),
        ("z", &[]),
    ])
    .geometry(true);
    assert!(!g.folded[1], "a two-commit instance is not idle");
    assert_eq!(g.folded_count, 0);
    assert_eq!(g.xs.len(), 4);
    assert_eq!(g.width, 56.0);
}

#[test]
fn a_merge_tip_makes_its_column_non_foldable() {
    // `m` is a merge commit and a tip in column 1: its instance holds one dot,
    // but that dot is a merge, so the column is not idle.
    let r = rows(&[("a", &["p"]), ("m", &["p", "p"]), ("p", &[])]);
    assert!(r[1].is_tip && r[1].is_merge);
    let mut idle = IdleLanes::new();
    for row in &r {
        idle.push(row);
    }
    let g = idle.geometry(true);
    assert!(!g.folded[1], "a merge dot is not idle");
    assert_eq!(g.folded_count, 0);
    assert_eq!(g.width, 28.0);
}

#[test]
fn geometry_follows_new_rows_that_break_a_fold() {
    let mut b = GraphBuilder::new();
    let mut idle = IdleLanes::new();
    feed(&mut b, &mut idle, "m", &["a", "b", "c", "d"]);
    feed(&mut b, &mut idle, "a", &["root"]);
    feed(&mut b, &mut idle, "b", &["root"]);
    let g = idle.geometry(true);
    assert_eq!(g.folded, vec![false, true, true, true]);
    assert_eq!(g.width, 26.0);

    // `c` is a merge, so its column stops being idle and the run breaks.
    feed(&mut b, &mut idle, "c", &["root", "extra"]);
    let g = idle.geometry(true);
    assert_eq!(g.folded, vec![false, false, false, false]);
    assert_eq!(g.folded_count, 0);
    assert_eq!(g.width, 56.0);
}

#[test]
fn a_wide_row_tracks_every_column() {
    // 300 parents in one octopus merge: a 300-lane row. Column 0 is the merge
    // itself (never idle); every merged-in column carries one non-merge dot
    // and must be folded into the same gap.
    const PARENTS: u32 = 300;
    let parents: Vec<u32> = (1..=PARENTS).collect();
    let mut b = GraphBuilder::new();
    let mut idle = IdleLanes::new();
    let row = b.push(0u32, &parents);
    assert_eq!(row.width, PARENTS as usize);
    idle.push(&row);
    for &p in &parents {
        let row = b.push(p, &[]);
        idle.push(&row);
    }

    let g = idle.geometry(true);
    assert_eq!(g.xs.len(), PARENTS as usize);
    assert_eq!(g.xs[0], 7.0);
    assert!(g.xs[1..].iter().all(|&x| x == 20.0));
    assert!(!g.folded[0]);
    assert!(g.folded[1..].iter().all(|&f| f));
    assert_eq!(g.folded_count, PARENTS as usize - 1);
    assert_eq!(g.width, 26.0);
}
