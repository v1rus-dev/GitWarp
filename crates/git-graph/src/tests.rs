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
    // Both lanes converge into the fork point.
    assert_eq!(shape(&r[4].above), vec![seg(0, 0), seg(1, 0)]);
    // The merged branch keeps one color for its whole length.
    assert_eq!(r[2].color, r[3].color);
    assert_ne!(r[2].color, r[1].color);
    assert_continuous(&r);
}

#[test]
fn two_tips_on_same_base_meet_at_the_base() {
    let r = rows(&[("x", &["base"]), ("y", &["base"]), ("base", &[])]);
    assert_eq!(r[0].column, 0);
    assert_eq!(r[1].column, 1);
    assert!(r[0].is_tip && r[1].is_tip);
    // x's lane passes by y.
    assert_eq!(shape(&r[1].above), vec![seg(0, 0)]);
    assert_eq!(shape(&r[1].below), vec![seg(0, 0), seg(1, 1)]);
    assert_eq!(r[2].column, 0);
    assert_eq!(shape(&r[2].above), vec![seg(0, 0), seg(1, 0)]);
    assert_continuous(&r);
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
    assert_eq!(shape(&r[4].above), vec![seg(0, 0), seg(1, 0), seg(2, 0)]);
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
