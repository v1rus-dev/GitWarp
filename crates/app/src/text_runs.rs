//! Styled inline runs for translated sentences (see `ui::i18n::sentence`).

use gpui::prelude::*;
use gpui::{div, Div, FontWeight, Hsla, SharedString};
use ui::metrics::FONT_MONO;

/// A semibold run in `color`.
pub fn strong(color: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
}

/// A monospace run in `color`.
pub fn mono(color: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_color(color)
        .child(text.into())
}
