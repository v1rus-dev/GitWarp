//! Graph geometry for the history screen. Shared metrics come from the `ui`
//! crate; only what this screen adds on top lives here.

use gpui::{px, Pixels};

pub use ui::metrics::{
    DOT_R as DOT_RADIUS, LANE_W as LANE_WIDTH, LINE_W as LINE_WIDTH, ROW as ROW_HEIGHT,
};

pub const GRAPH_PADDING: Pixels = px(10.);
/// Wider graphs are clipped; the commit text must stay readable.
pub const MAX_GRAPH_LANES: usize = 24;
