//! Shared geometry: fonts, type sizes, spacing, radii and component metrics.
//!
//! Both themes use the same numbers; only colours differ.

use gpui::{px, Pixels};

/// UI font family; bundled in a later step, GPUI falls back until then.
pub const FONT_UI: &str = "Inter";
/// Mono font family; bundled in a later step, GPUI falls back until then.
pub const FONT_MONO: &str = "JetBrains Mono";
/// CJK fallback family (bundled Noto Sans JP) for both UI and mono text.
pub const FONT_CJK: &str = "Noto Sans JP";

/// Section labels, counters, key caps.
pub const FS_XS: Pixels = px(11.);
/// Secondary meta, tooltips, columns.
pub const FS_SM: Pixels = px(12.);
/// Default UI, rows, inputs.
pub const FS_BASE: Pixels = px(13.);
/// Dialog titles, palette input.
pub const FS_MD: Pixels = px(14.);
/// Page headings.
pub const FS_LG: Pixels = px(16.);
/// Welcome title, settings pane headings.
pub const FS_XL: Pixels = px(20.);
/// Diff and other monospaced text.
pub const MONO: Pixels = px(12.);

/// 2px.
pub const SP_1: Pixels = px(2.);
/// 4px.
pub const SP_2: Pixels = px(4.);
/// 6px.
pub const SP_3: Pixels = px(6.);
/// 8px.
pub const SP_4: Pixels = px(8.);
/// 12px.
pub const SP_5: Pixels = px(12.);
/// 16px.
pub const SP_6: Pixels = px(16.);
/// 24px.
pub const SP_7: Pixels = px(24.);
/// 32px.
pub const SP_8: Pixels = px(32.);

/// Small radius: checkboxes, swatches.
pub const R_XS: Pixels = px(3.);
/// Buttons, inputs, ref badges, menus items.
pub const R_SM: Pixels = px(5.);
/// History rows, menu panels, dialogs' inner cards.
pub const R_MD: Pixels = px(7.);
/// Dialog panels.
pub const R_LG: Pixels = px(12.);
/// Window frame.
pub const R_WINDOW: Pixels = px(8.);
/// Pills and counters.
pub const R_PILL: Pixels = px(8.);

/// One history row, and the base control height.
pub const ROW: Pixels = px(26.);
/// Standard control height.
pub const CTL: Pixels = px(26.);
/// Compact control height.
pub const CTL_SM: Pixels = px(22.);
/// Titlebar and toolbar.
pub const BAR: Pixels = px(40.);
/// Status bar.
pub const STATUS: Pixels = px(24.);
/// The details panel's header.
pub const DOCK_HEADER: Pixels = px(34.);

/// One graph lane.
pub const LANE_W: Pixels = px(16.);
/// Radius of a commit dot.
pub const DOT_R: Pixels = px(5.);
/// Width of a graph edge.
pub const LINE_W: Pixels = px(2.);
