//! Paints one row of the commit graph inside its row bounds.
//!
//! Every row draws only its own two halves (see `git_graph::Row`), so rows
//! are independent and the virtualized list can paint any slice of history.
//! Lane positions come from the [`ColumnGeometry`] snapshot the view keeps
//! beside its [`git_graph::IdleLanes`] accumulator, so folded columns paint at
//! their shared gap x.

use git_core::Oid;
use git_graph::{ColorId, ColumnGeometry, Row, Segment};
use gpui::{
    fill, linear_color_stop, linear_gradient, point, px, Bounds, Hsla, PathBuilder, Pixels, Point,
    Window,
};
use ui::Theme;

use crate::theme::{
    DOT_HALO_RADIUS, DOT_RADIUS, GRAPH_FADE_WIDTH, GRAPH_INSET, HEAD_RING_RADIUS, HEAD_RING_WIDTH,
    LANE_OVERDRAW, MERGE_INNER_RADIUS,
};

/// A dot whose centre is further than this past the column edge is not drawn.
const DOT_CLIP_MARGIN: f32 = 6.;
/// Dash pattern of a folded lane: 2 px on, 3 px off.
const DASH_ON: f32 = 2.;
const DASH_PERIOD: f32 = 5.;
/// Where a merge stub ends, relative to its dot.
const STUB_DX: f32 = 9.;
const STUB_DY: f32 = 8.;

/// Pixel x of a column's centre inside the graph column: the geometry's x plus
/// the left inset.
pub fn lane_centre(x: f32) -> f32 {
    x + GRAPH_INSET
}

/// Content width of the graph: the geometry's width plus the left inset.
pub fn content_width(geometry: &ColumnGeometry) -> f32 {
    geometry.width + GRAPH_INSET
}

/// The graph cell's paint mask: the cell's bounds extended vertically by the
/// lane overdraw, so the 1 px a straight lane runs past its row is not clipped.
/// Horizontally the mask stays at the cell's edges (the right-edge clip/fade).
pub fn overdraw_mask_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(bounds.origin.x, bounds.origin.y - px(LANE_OVERDRAW)),
        gpui::size(
            bounds.size.width,
            bounds.size.height + px(2. * LANE_OVERDRAW),
        ),
    )
}

/// Paints one row. `background` is the row's current fill (`bg`, `hover` or
/// `selection`): it colours the dot halo and the merge ring's interior, and it
/// is what the clipped edge fades into. `stub` adds the History branch
/// filter's dashed stub: the merged-in branch's line, cut short.
#[allow(clippy::too_many_arguments)]
pub fn paint_row(
    row: &Row<Oid>,
    row_index: usize,
    bounds: Bounds<Pixels>,
    background: Hsla,
    geometry: &ColumnGeometry,
    is_head: bool,
    stub: bool,
    theme: &Theme,
    colored: bool,
    line_width: Pixels,
    window: &mut Window,
) {
    let cw = f32::from(bounds.size.width);
    let top = bounds.origin.y;
    let mid = top + bounds.size.height / 2.;
    let bottom = top + bounds.size.height;
    // Straight lane segments overdraw their row by 1 px at both ends.
    let overdraw = px(LANE_OVERDRAW);
    let origin_x = bounds.origin.x;
    let origin_x_f = f32::from(origin_x);
    // Snap lane centres to whole pixels: an even line width centred on an
    // integer x covers exactly two pixel columns. Dots round their centre too,
    // so curved endpoints land on the same x as the straight lanes and dots.
    let x_of = |col: usize| {
        geometry
            .xs
            .get(col)
            .map(|x| px((origin_x_f + lane_centre(*x)).round()))
    };
    let local_x = |x: Pixels| f32::from(x - origin_x);
    let lane = |color: ColorId| lane_color(theme, colored, color);
    let folded = |col: usize| geometry.folded.get(col).copied().unwrap_or(false);
    // The dash phase follows the row's absolute content y, so the pattern runs
    // on across row boundaries instead of restarting in every row.
    let dash_offset = row_index as f32 * f32::from(bounds.size.height);

    // Straight-through passing lanes first: one vertical per distinct x, so a
    // folded group draws its dashed gap lane once. `xs` never decreases with
    // the column, so the lanes arrive in x order: an equal x is the same folded
    // group, and the first lane past the column edge ends the row.
    let through: Vec<&Segment> = passing_lanes(row).collect();
    let mut last_x: Option<f32> = None;
    for segment in &through {
        let col = segment.from;
        let Some(x) = x_of(col) else {
            continue;
        };
        let x_local = local_x(x);
        if !vertical_visible(x_local, cw) {
            break;
        }
        if last_x == Some(x_local) {
            continue;
        }
        last_x = Some(x_local);
        if folded(col) {
            let color = theme.colors.muted.opacity(0.6);
            for (from, to) in dash_segments(0., f32::from(bounds.size.height), dash_offset) {
                straight(
                    window,
                    line_width,
                    point(x, top + px(from)),
                    point(x, top + px(to)),
                    color,
                );
            }
        } else {
            straight(
                window,
                line_width,
                point(x, top - overdraw),
                point(x, bottom + overdraw),
                lane(segment.color),
            );
        }
    }

    // Incoming lanes: a straight run into the dot, or a curve from the column
    // the line comes from. A passing lane that the lower half compacts left
    // stops at the center line, so only its upper half is drawn here.
    for segment in &row.above {
        let Some(x0) = x_of(segment.from) else {
            continue;
        };
        let color = lane(segment.color);
        if segment.from == segment.to {
            if segment.from != row.column && through.iter().any(|s| s.from == segment.from) {
                continue; // a straight-through passing lane, drawn above
            }
            if !vertical_visible(local_x(x0), cw) {
                continue;
            }
            if segment.from != row.column && folded(segment.from) {
                let color = theme.colors.muted.opacity(0.6);
                let end = f32::from(bounds.size.height) / 2.;
                for (from, to) in dash_segments(0., end, dash_offset) {
                    straight(
                        window,
                        line_width,
                        point(x0, top + px(from)),
                        point(x0, top + px(to)),
                        color,
                    );
                }
            } else {
                straight(
                    window,
                    line_width,
                    point(x0, top - overdraw),
                    point(x0, mid),
                    color,
                );
            }
        } else {
            let Some(x1) = x_of(segment.to) else {
                continue;
            };
            if curve_visible(local_x(x0), local_x(x1), cw) {
                curve_into_dot(window, line_width, point(x0, top), point(x1, mid), color);
            }
        }
    }

    // Outgoing lanes: a straight run out of the dot, or a curve to the column
    // the line leads to. A compacted passing lane curves from its old column at
    // the center line to its new column at the bottom edge.
    for segment in &row.below {
        let Some(x0) = x_of(segment.from) else {
            continue;
        };
        let color = lane(segment.color);
        if segment.from == segment.to {
            if segment.from != row.column && through.iter().any(|s| s.from == segment.from) {
                continue; // a straight-through passing lane, drawn above
            }
            if !vertical_visible(local_x(x0), cw) {
                continue;
            }
            if segment.from != row.column && folded(segment.from) {
                let color = theme.colors.muted.opacity(0.6);
                let end = f32::from(bounds.size.height);
                for (from, to) in dash_segments(end / 2., end, dash_offset) {
                    straight(
                        window,
                        line_width,
                        point(x0, top + px(from)),
                        point(x0, top + px(to)),
                        color,
                    );
                }
            } else {
                straight(
                    window,
                    line_width,
                    point(x0, mid),
                    point(x0, bottom + overdraw),
                    color,
                );
            }
        } else {
            let Some(x1) = x_of(segment.to) else {
                continue;
            };
            if curve_visible(local_x(x0), local_x(x1), cw) {
                if segment.from == row.column {
                    curve_out_of_dot(window, line_width, point(x0, mid), point(x1, bottom), color);
                } else {
                    // A compacted passing lane shifts sideways while it keeps
                    // running down: a symmetric S with vertical tangents, so it
                    // joins the straight lanes above and below without a kink.
                    curve_shift(window, line_width, point(x0, mid), point(x1, bottom), color);
                }
            }
        }
    }

    if let Some(xc) = x_of(row.column) {
        if dot_visible(local_x(xc), cw) {
            let center = point(xc, mid);

            if stub {
                paint_stub(window, line_width, center, lane(row.color));
            }

            // The HEAD ring goes under the dot.
            if is_head {
                ring(
                    window,
                    center,
                    HEAD_RING_RADIUS,
                    HEAD_RING_WIDTH,
                    lane(row.color),
                );
            }

            if row.is_merge {
                // Hollow ring: outer 5, inner 2.5, so the stroke is 2.5 px. No halo.
                circle(window, center, DOT_RADIUS, lane(row.color));
                circle(window, center, MERGE_INNER_RADIUS, background);
            } else {
                // Halo first, then the dot: the ring never touches the lines.
                circle(window, center, DOT_HALO_RADIUS, background);
                circle(window, center, DOT_RADIUS, lane(row.color));
            }
        }
    }

    // A graph wider than the column fades out over its last 20 px.
    if content_width(geometry) > cw {
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

/// The stub of a merge in the branch filter: a 2-on / 2-off dashed line from
/// the dot up and to the right, ending in a small dot (the design's
/// `M xc 13 L xc+9 5` with a 1.8 radius end dot).
fn paint_stub(window: &mut Window, width: Pixels, centre: Point<Pixels>, color: Hsla) {
    let end = point(centre.x + px(STUB_DX), centre.y - px(STUB_DY));
    let length = STUB_DX.hypot(STUB_DY);
    let mut at = 0.;
    while at < length {
        let to = (at + 2.).min(length);
        let along = |t: f32| {
            point(
                centre.x + px(STUB_DX * t / length),
                centre.y - px(STUB_DY * t / length),
            )
        };
        straight(window, width, along(at), along(to), color);
        at += 4.;
    }
    circle(window, end, px(1.8), color);
}

/// The row's straight-through passing lanes: a column that carries a lane
/// untouched through both halves and is not the dot's. They are returned in
/// column order, so the painter's last-x dedup draws each column once.
///
/// A lower-half passing lane that compaction shifts left (`from != to`) is not
/// straight through: the upper half is drawn to the center line and the lower
/// half as a curve, so it is left out here.
fn passing_lanes<'a>(row: &'a Row<Oid>) -> impl Iterator<Item = &'a Segment> + 'a {
    let column = row.column;
    row.above.iter().filter(move |s| {
        s.from == s.to
            && s.from != column
            && row.below.iter().any(|b| b.from == b.to && b.from == s.from)
    })
}

/// The colour of a lane: its own colour id from `git-graph`, or the muted
/// fallback when the graph is not coloured. Columns are only positions: a lane
/// that compaction shifts keeps its colour, so the painter must never index
/// the palette by column.
fn lane_color(theme: &Theme, colored: bool, color: ColorId) -> Hsla {
    if colored {
        theme.lane_color(color)
    } else {
        theme.colors.muted
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

fn straight(
    window: &mut Window,
    width: Pixels,
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
) {
    let mut path = PathBuilder::stroke(width);
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// An upper-half lane change: the line leaves the row's top edge vertically
/// and arrives at the dot horizontally, a long quarter-ellipse rather than a
/// steep S squeezed into the row. The vertical tangent at the top edge matches
/// the straight lane above; the horizontal one matches the dot.
fn curve_into_dot(
    window: &mut Window,
    width: Pixels,
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let mut path = PathBuilder::stroke(width);
    path.move_to(from);
    path.cubic_bezier_to(
        to,
        point(from.x, from.y + dy * 0.45),
        point(to.x - dx * 0.55, to.y),
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A lower-half lane change out of the dot: it leaves the dot horizontally and
/// arrives at the row's bottom edge vertically, mirroring [`curve_into_dot`].
fn curve_out_of_dot(
    window: &mut Window,
    width: Pixels,
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let mut path = PathBuilder::stroke(width);
    path.move_to(from);
    path.cubic_bezier_to(
        to,
        point(from.x + dx * 0.55, from.y),
        point(to.x, from.y + dy * 0.45),
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A passing lane that compaction shifts sideways: a symmetric S with vertical
/// tangents at both ends, so it meets the straight lane above and below cleanly.
fn curve_shift(
    window: &mut Window,
    width: Pixels,
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
) {
    let mid_y = from.y + (to.y - from.y) * 0.5;
    let mut path = PathBuilder::stroke(width);
    path.move_to(from);
    path.cubic_bezier_to(to, point(from.x, mid_y), point(to.x, mid_y));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: Pixels, color: Hsla) {
    // A rounded quad is tessellated as a box, so a small one lands between
    // pixels and looks faceted. A filled cubic circle, snapped to the pixel
    // grid, stays round at the graph's 4–6 px sizes.
    let r = f32::from(radius);
    let k = r * 0.552_284_8;
    let cx = f32::from(center.x).round();
    let cy = f32::from(center.y).round();
    let at = |x: f32, y: f32| point(px(cx + x), px(cy + y));
    let mut path = PathBuilder::fill();
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

/// A stroked circle, like the design's `<circle fill="none" stroke=…>`: the
/// stroke is centred on `radius`, so it covers `radius ± width / 2`.
fn ring(window: &mut Window, center: Point<Pixels>, radius: Pixels, width: Pixels, color: Hsla) {
    let r = f32::from(radius);
    let k = r * 0.552_284_8;
    let cx = f32::from(center.x).round();
    let cy = f32::from(center.y).round();
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

    fn geometry(xs: Vec<f32>, width: f32) -> ColumnGeometry {
        let lanes = xs.len();
        ColumnGeometry {
            xs,
            folded: vec![false; lanes],
            width,
            folded_count: 0,
        }
    }

    #[test]
    fn lane_zero_centre_is_ten() {
        // git-graph puts lane 0 at 8; the 2 px inset makes it 10.
        assert_eq!(lane_centre(8.), 10.);
        assert_eq!(lane_centre(24.), 26.);
    }

    #[test]
    fn content_width_is_lanes_times_16_plus_inset() {
        for n in [1_usize, 3, 10] {
            let g = geometry(
                (0..n).map(|i| 16. * i as f32 + 8.).collect(),
                n as f32 * 16.,
            );
            assert_eq!(content_width(&g), n as f32 * 16. + 2.);
        }
    }

    #[test]
    fn fit_width_includes_the_inset() {
        // Three lanes: 48 + 2 inset + 8 pad.
        let g = geometry(vec![8., 24., 40.], 48.);
        assert_eq!(crate::history_view::fit_width(content_width(&g)), 58.);
    }

    #[test]
    fn mask_extends_one_px_vertically_only() {
        let b = Bounds::new(point(px(10.), px(20.)), gpui::size(px(100.), px(26.)));
        let m = overdraw_mask_bounds(b);
        assert_eq!(m.origin, point(px(10.), px(19.)));
        assert_eq!(m.size, gpui::size(px(100.), px(28.)));
    }

    #[test]
    fn a_shifted_passing_lane_keeps_its_own_colour() {
        let theme = Theme::dark();
        // Compaction moved this passing lane from column 1 to column 0: its
        // colour must come from the lane itself, not from either column's
        // palette entry.
        let shifted = Segment {
            from: 1,
            to: 0,
            color: 3,
        };
        assert_ne!(shifted.from, shifted.to);
        let colour = lane_color(&theme, true, shifted.color);
        assert_eq!(colour, theme.lane_color(shifted.color));
        assert_ne!(colour, theme.lane_color(shifted.from as u32));
        assert_ne!(colour, theme.lane_color(shifted.to as u32));
        // Uncoloured mode keeps the muted fallback.
        assert_eq!(lane_color(&theme, false, shifted.color), theme.colors.muted);
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
    fn passing_lanes_are_only_lanes_that_run_straight_through() {
        let row = Row {
            id: Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap(),
            column: 2,
            color: 0,
            // A lane change, the dot's own column and a lane that only exists
            // above are not straight-through passing lanes.
            above: vec![segment(0, 0), segment(1, 2), segment(3, 3), segment(4, 4)],
            below: vec![segment(0, 0), segment(2, 1), segment(3, 1), segment(4, 4)],
            width: 5,
            is_tip: true,
            is_merge: false,
        };
        let froms: Vec<usize> = passing_lanes(&row).map(|s| s.from).collect();
        assert_eq!(
            froms,
            vec![0, 4],
            "only straight-through lanes, column order"
        );
        assert!(
            froms.windows(2).all(|w| w[0] <= w[1]),
            "the painter's early cull relies on non-decreasing columns"
        );
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
}
