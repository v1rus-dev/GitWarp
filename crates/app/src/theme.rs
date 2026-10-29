//! Dark theme constants. A real theme system comes later; keeping the values
//! in one place makes that move easy.

use gpui::{hsla, px, rgb, Hsla, Pixels, Rgba};

pub const ROW_HEIGHT: Pixels = px(26.);
pub const LANE_WIDTH: Pixels = px(14.);
pub const GRAPH_PADDING: Pixels = px(10.);
pub const LINE_WIDTH: Pixels = px(2.);
pub const DOT_RADIUS: Pixels = px(4.5);
/// Wider graphs are clipped; the commit text must stay readable.
pub const MAX_GRAPH_LANES: usize = 24;

pub fn background() -> Rgba {
    rgb(0x1e2025)
}
pub fn surface() -> Rgba {
    rgb(0x25282e)
}
pub fn selection() -> Rgba {
    rgb(0x2f4a6d)
}
pub fn hover() -> Rgba {
    rgb(0x2a2d34)
}
pub fn text() -> Rgba {
    rgb(0xdcdfe4)
}
pub fn text_muted() -> Rgba {
    rgb(0x8b919c)
}
pub fn border() -> Rgba {
    rgb(0x33373e)
}
pub fn error() -> Rgba {
    rgb(0xe06c75)
}

/// Lane colors: hues spread by the golden angle, so neighbours never clash
/// and the palette never runs out.
pub fn lane_color(id: u32) -> Hsla {
    let hue = (id as f32 * 0.381_966).fract();
    hsla(hue, 0.65, 0.62, 1.)
}
