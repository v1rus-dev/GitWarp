//! Loading skeletons.
//!
//! GPUI 0.2.2 has no animated gradient positions, so the shimmer pulses the
//! `hover` overlay between transparent and opaque instead of moving it.

use std::time::Duration;

use gpui::{
    div, ease_in_out, px, Animation, AnimationExt, App, ElementId, IntoElement, ParentElement,
    Pixels, RenderOnce, Styled, Window,
};

use crate::metrics::R_XS;
use crate::theme::ActiveTheme;

/// A 10px-high shimmering placeholder.
#[derive(IntoElement)]
pub struct Skeleton {
    id: ElementId,
    width: Pixels,
}

impl Skeleton {
    pub fn new(id: impl Into<ElementId>, width: Pixels) -> Self {
        Self {
            id: id.into(),
            width,
        }
    }
}

impl RenderOnce for Skeleton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let pulse = div()
            .absolute()
            .inset_0()
            .rounded(R_XS)
            .bg(colors.hover)
            .with_animation(
                self.id,
                Animation::new(Duration::from_millis(1400))
                    .repeat()
                    .with_easing(ease_in_out),
                |this, delta| {
                    let phase = (2. * delta - 1.).abs();
                    this.opacity(1. - phase)
                },
            );

        div()
            .relative()
            .w(self.width)
            .h(px(10.))
            .rounded(R_XS)
            .bg(colors.surface)
            .overflow_hidden()
            .child(pulse)
    }
}
