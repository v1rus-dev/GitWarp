//! Paints one row of the commit graph inside its row bounds.
//!
//! Every row draws only its own two halves (see `git_graph::Row`), so rows
//! are independent and the virtualized list can paint any slice of history.

use gpui::{point, px, size, Bounds, PathBuilder, Pixels, Point, Window};

use git_core::Oid;
use git_graph::{Row, Segment};

use crate::theme::{
    lane_color, DOT_RADIUS, GRAPH_PADDING, LANE_WIDTH, LINE_WIDTH, MAX_GRAPH_LANES,
};

pub fn graph_width(lanes: usize) -> Pixels {
    GRAPH_PADDING * 2. + LANE_WIDTH * lanes.clamp(1, MAX_GRAPH_LANES) as f32
}

pub fn paint_row(row: &Row<Oid>, bounds: Bounds<Pixels>, window: &mut Window) {
    let x = |col: usize| bounds.origin.x + GRAPH_PADDING + LANE_WIDTH * (col as f32 + 0.5);
    let visible = |s: &Segment| s.from < MAX_GRAPH_LANES && s.to < MAX_GRAPH_LANES;
    let top = bounds.origin.y;
    let mid = bounds.origin.y + bounds.size.height / 2.;
    let bottom = bounds.origin.y + bounds.size.height;

    for s in row.above.iter().filter(|s| visible(s)) {
        line(window, point(x(s.from), top), point(x(s.to), mid), s.color);
    }
    for s in row.below.iter().filter(|s| visible(s)) {
        line(
            window,
            point(x(s.from), mid),
            point(x(s.to), bottom),
            s.color,
        );
    }

    if row.column < MAX_GRAPH_LANES {
        let r = if row.is_merge {
            DOT_RADIUS * 0.8
        } else {
            DOT_RADIUS
        };
        let center = point(x(row.column), mid);
        let dot = Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.));
        window.paint_quad(
            gpui::fill(dot, lane_color(row.color))
                .corner_radii(r)
                .border_widths(if row.is_tip { px(0.) } else { px(1.5) })
                .border_color(gpui::rgb(0x1e2025)),
        );
    }
}

/// Straight for vertical lanes, an S-curve for lanes changing column:
/// the curve keeps branch and merge lines readable in dense graphs.
fn line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: u32) {
    let mut path = PathBuilder::stroke(LINE_WIDTH);
    path.move_to(from);
    if from.x == to.x {
        path.line_to(to);
    } else {
        let dy = (to.y - from.y) * 0.6;
        path.cubic_bezier_to(to, point(from.x, from.y + dy), point(to.x, to.y - dy));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, lane_color(color));
    }
}
