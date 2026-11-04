//! Graph geometry for the history screen. Shared metrics come from the `ui`
//! crate; only what this screen adds on top lives here.

use gpui::{px, Pixels};

pub use ui::metrics::{DOT_R as DOT_RADIUS, LANE_W as LANE_WIDTH};

/// Radius of the ring painted in the row background around a plain dot.
pub const DOT_HALO_RADIUS: Pixels = px(6.5);
/// Inner radius of a merge commit's hollow ring (its outer radius is
/// [`DOT_RADIUS`], so the stroke is 2 px).
pub const MERGE_INNER_RADIUS: Pixels = px(2.5);
/// Radius of the extra ring around the HEAD commit.
pub const HEAD_RING_RADIUS: Pixels = px(8.);
/// Stroke width of the HEAD ring.
pub const HEAD_RING_WIDTH: Pixels = px(1.5);
/// Width of the fade at the graph column's right edge when the graph is wider
/// than the column.
pub const GRAPH_FADE_WIDTH: Pixels = px(20.);
/// Left inset of the graph's lanes, px: lane `n`'s centre is `16n + 8 + 2`, so
/// the HEAD ring (radius 8 plus half its stroke) on lane 0 is not cut by the
/// column's left edge. `git-graph` stays inset-free; this is added when its x
/// is turned into pixels.
pub const GRAPH_INSET: f32 = 2.;
/// How far a straight lane segment runs past the top and bottom of its row, px,
/// so no hairline seam shows between rows at fractional scale factors.
pub const LANE_OVERDRAW: f32 = 1.;
