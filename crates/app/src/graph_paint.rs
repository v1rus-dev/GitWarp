//! Paints one row of the commit graph inside its row bounds.
//!
//! Every row draws only its own two halves (see `git_graph::Row`), so rows
//! are independent and the virtualized list can paint any slice of history.
//! Lane positions come from the [`ColumnGeometry`] snapshot the view keeps
//! beside its [`git_graph::IdleLanes`] accumulator, so folded columns paint at
//! their shared gap x.

use git_core::Oid;
use git_graph::{ColumnGeometry, Row, Segment};
use gpui::{
    fill, linear_color_stop, linear_gradient, point, px, size, Bounds, Hsla, PathBuilder, Pixels,
    Point, Window,
};
use ui::Theme;

use crate::theme::{
    DOT_HALO_RADIUS, DOT_RADIUS, GRAPH_FADE_WIDTH, HEAD_RING_RADIUS, HEAD_RING_WIDTH, LANE_WIDTH,
    LINE_WIDTH, MERGE_INNER_RADIUS, ROW_HEIGHT,
};

/// A dot whose centre is further than this past the column edge is not drawn.
const DOT_CLIP_MARGIN: f32 = 6.;
/// Dash pattern of a folded lane: 2 px on, 3 px off.
const DASH_ON: f32 = 2.;
const DASH_PERIOD: f32 = 5.;

/// Width the graph column needs for `lanes` unfolded lanes.
pub fn graph_width(lanes: usize) -> Pixels {
    LANE_WIDTH * lanes.max(1) as f32
}

/// Paints one row. `background` is the row's current fill (`bg`, `hover` or
/// `selection`): it colours the dot halo and the merge ring's interior, and it
/// is what the clipped edge fades into.
#[allow(clippy::too_many_arguments)]
pub fn paint_row(
    row: &Row<Oid>,
    row_index: usize,
    bounds: Bounds<Pixels>,
    background: Hsla,
    geometry: &ColumnGeometry,
    is_head: bool,
    theme: &Theme,
    window: &mut Window,
) {
    let cw = f32::from(bounds.size.width);
    let top = bounds.origin.y;
    let mid = top + bounds.size.height / 2.;
    let bottom = top + bounds.size.height;
    let origin_x = bounds.origin.x;
    let x_of = |col: usize| geometry.xs.get(col).map(|x| origin_x + px(*x));
    let local_x = |x: Pixels| f32::from(x - origin_x);
    let lane = |col: usize| theme.lane_color(col as u32);
    let folded = |col: usize| geometry.folded.get(col).copied().unwrap_or(false);
    // The dash phase follows the row's absolute content y, so the pattern runs
    // on across row boundaries instead of restarting in every row.
    let dash_offset = row_index as f32 * f32::from(ROW_HEIGHT);

    // Passing lanes first: one vertical per distinct x, so a folded group draws
    // its dashed gap lane once.
    let mut seen: Vec<f32> = Vec::new();
    for segment in row
        .above
        .iter()
        .chain(row.below.iter())
        .filter(|s| s.from == s.to && s.from != row.column)
    {
        let Some(x) = x_of(segment.from) else {
            continue;
        };
        let x_local = local_x(x);
        if seen.contains(&x_local) {
            continue;
        }
        seen.push(x_local);
        if !vertical_visible(x_local, cw) {
            continue;
        }
        if folded(segment.from) {
            let color = theme.colors.muted.opacity(0.6);
            for (from, to) in dash_segments(0., f32::from(bounds.size.height), dash_offset) {
                straight(
                    window,
                    point(x, top + px(from)),
                    point(x, top + px(to)),
                    color,
                );
            }
        } else {
            straight(window, point(x, top), point(x, bottom), lane(segment.from));
        }
    }

    // Incoming lanes: a straight run into the dot, or a curve from the column
    // the line comes from.
    for segment in &row.above {
        if segment.from == segment.to && segment.from != row.column {
            continue; // a passing lane, drawn above
        }
        let Some(x0) = x_of(segment.from) else {
            continue;
        };
        let color = lane(segment_column(true, segment));
        if segment.from == segment.to {
            if vertical_visible(local_x(x0), cw) {
                straight(window, point(x0, top), point(x0, mid), color);
            }
        } else {
            let Some(x1) = x_of(segment.to) else {
                continue;
            };
            if curve_visible(local_x(x0), local_x(x1), cw) {
                curve(window, point(x0, top), point(x1, mid), color);
            }
        }
    }

    // Outgoing lanes: a straight run out of the dot, or a curve to the column
    // the line leads to.
    for segment in &row.below {
        if segment.from == segment.to && segment.from != row.column {
            continue; // a passing lane, drawn above
        }
        let Some(x0) = x_of(segment.from) else {
            continue;
        };
        let color = lane(segment_column(false, segment));
        if segment.from == segment.to {
            if vertical_visible(local_x(x0), cw) {
                straight(window, point(x0, mid), point(x0, bottom), color);
            }
        } else {
            let Some(x1) = x_of(segment.to) else {
                continue;
            };
            if curve_visible(local_x(x0), local_x(x1), cw) {
                curve(window, point(x0, mid), point(x1, bottom), color);
            }
        }
    }

    if let Some(xc) = x_of(row.column) {
        if dot_visible(local_x(xc), cw) {
            let center = point(xc, mid);

            // The HEAD ring goes under the dot.
            if is_head {
                ring(
                    window,
                    center,
                    HEAD_RING_RADIUS,
                    HEAD_RING_WIDTH,
                    theme.colors.text,
                );
            }

            if row.is_merge {
                // Hollow ring: outer 4.5, inner 2.5, so the stroke is 2 px. No halo.
                circle(window, center, DOT_RADIUS, lane(row.column));
                circle(window, center, MERGE_INNER_RADIUS, background);
            } else {
                // Halo first, then the dot: the ring never touches the lines.
                circle(window, center, DOT_HALO_RADIUS, background);
                circle(window, center, DOT_RADIUS, lane(row.column));
            }
        }
    }

    // A graph wider than the column fades out over its last 20 px.
    if geometry.width > cw {
        let fade = f32::from(GRAPH_FADE_WIDTH).min(cw);
        let fade_bounds = Bounds::from_corners(
            point(origin_x + px(cw - fade), top),
            point(origin_x + px(cw), bottom),
        );
        window.paint_quad(fill(
            fade_bounds,
            linear_gradient(
                90.,
                linear_color_stop(background.opacity(0.), 0.),
                linear_color_stop(background, 1.),
            ),
        ));
    }
}

/// The column whose colour a segment takes: `above` segments are coloured by
/// their source column (the lane the line comes from), `below` segments by
/// their target column (the lane the line leads to). A passing lane has the
/// same column on both ends.
fn segment_column(above: bool, segment: &Segment) -> usize {
    if above {
        segment.from
    } else {
        segment.to
    }
}

/// Verticals are clipped at the column's right edge.
fn vertical_visible(x: f32, cw: f32) -> bool {
    x <= cw
}

/// A curve is kept when either end is inside the column; the content mask cuts
/// the rest.
fn curve_visible(x0: f32, x1: f32, cw: f32) -> bool {
    x0.min(x1) <= cw
}

/// A dot is still drawn when its centre is just past the edge, so the half
/// that is inside the column stays visible.
fn dot_visible(x: f32, cw: f32) -> bool {
    x <= cw + DOT_CLIP_MARGIN
}

/// The visible stretches of a 2-on / 3-off dash pattern between `y0` and `y1`
/// (row-local), with the pattern phase taken from `offset`, the row's absolute
/// content y. Painting the dashes as short segments keeps the pattern running
/// across row boundaries.
fn dash_segments(y0: f32, y1: f32, offset: f32) -> Vec<(f32, f32)> {
    let mut dashes = Vec::new();
    let mut y = y0;
    while y < y1 {
        let phase = (offset + y).rem_euclid(DASH_PERIOD);
        if phase < DASH_ON {
            let end = (y + (DASH_ON - phase)).min(y1);
            dashes.push((y, end));
            y = end;
        } else {
            y = (y + (DASH_PERIOD - phase)).min(y1);
        }
    }
    dashes
}

fn straight(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla) {
    let mut path = PathBuilder::stroke(LINE_WIDTH);
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A lane change with the design's control points: 7 px along from the start
/// and 7 px back from the end, so the bend stays inside the row.
fn curve(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla) {
    let mut path = PathBuilder::stroke(LINE_WIDTH);
    path.move_to(from);
    path.cubic_bezier_to(
        to,
        point(from.x, from.y + px(7.)),
        point(to.x, to.y - px(7.)),
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: Pixels, color: Hsla) {
    let bounds = Bounds::new(
        point(center.x - radius, center.y - radius),
        size(radius * 2., radius * 2.),
    );
    window.paint_quad(fill(bounds, color).corner_radii(radius));
}

/// A stroked circle, like the design's `<circle fill="none" stroke=…>`: the
/// stroke is centred on `radius`, so it covers `radius ± width / 2`.
fn ring(window: &mut Window, center: Point<Pixels>, radius: Pixels, width: Pixels, color: Hsla) {
    let r = f32::from(radius);
    let k = r * 0.552_284_8;
    let (cx, cy) = (f32::from(center.x), f32::from(center.y));
    let at = |x: f32, y: f32| point(px(cx + x), px(cy + y));
    let mut path = PathBuilder::stroke(width);
    path.move_to(at(r, 0.));
    path.cubic_bezier_to(at(0., r), at(r, k), at(k, r));
    path.cubic_bezier_to(at(-r, 0.), at(-k, r), at(-r, k));
    path.cubic_bezier_to(at(0., -r), at(-r, -k), at(-k, -r));
    path.cubic_bezier_to(at(r, 0.), at(k, -r), at(r, -k));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(from: usize, to: usize) -> Segment {
        Segment { from, to, color: 0 }
    }

    #[test]
    fn above_segments_take_the_source_column_and_below_the_target() {
        // A lane change: the incoming half is the child's column, the outgoing
        // half is the parent's.
        let change = segment(2, 5);
        assert_eq!(segment_column(true, &change), 2);
        assert_eq!(segment_column(false, &change), 5);
        // A passing lane has the same column on both ends.
        let passing = segment(3, 3);
        assert_eq!(segment_column(true, &passing), 3);
        assert_eq!(segment_column(false, &passing), 3);
    }

    #[test]
    fn culling_keeps_only_what_touches_the_graph_column() {
        // Verticals and curves must start at or left of the edge.
        assert!(vertical_visible(112., 112.));
        assert!(!vertical_visible(112.5, 112.));
        assert!(curve_visible(112., 200., 112.));
        assert!(!curve_visible(112.5, 200., 112.));
        // Dots may hang up to 6 px past the edge.
        assert!(dot_visible(118., 112.));
        assert!(!dot_visible(118.5, 112.));
    }

    #[test]
    fn dashes_keep_their_phase_across_rows() {
        // A row on 2 / off 3, starting at the top of the content.
        assert_eq!(
            dash_segments(0., 26., 0.),
            vec![
                (0., 2.),
                (5., 7.),
                (10., 12.),
                (15., 17.),
                (20., 22.),
                (25., 26.),
            ]
        );
        // The next row starts at absolute y 26, which is one pixel into a
        // dash: the first dash of the row only lasts one more pixel.
        assert_eq!(dash_segments(0., 5., 26.), vec![(0., 1.), (4., 5.)]);
    }

    #[test]
    fn graph_width_is_lane_width_times_lanes() {
        assert_eq!(f32::from(graph_width(1)), 14.);
        // No cap and no side padding: the full design width of a wide graph.
        assert_eq!(f32::from(graph_width(241)), 3374.);
    }
}
