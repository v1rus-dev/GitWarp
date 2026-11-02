//! Loading skeletons.
//!
//! GPUI has no animated gradient positions, so the shimmer pulses a `hover`
//! overlay over an `active` base; the base keeps it visible even on the first
//! frame.

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

/// Shimmer opacity, never below 0.4 so the skeleton is visible at any phase.
pub(crate) fn shimmer_opacity(delta: f32) -> f32 {
    let phase = (2. * delta - 1.).abs();
    0.4 + 0.6 * (1. - phase)
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
                |this, delta| this.opacity(shimmer_opacity(delta)),
            );

        div()
            .relative()
            .w(self.width)
            .h(px(10.))
            .rounded(R_XS)
            .bg(colors.active)
            .overflow_hidden()
            .child(pulse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_is_visible_at_every_phase() {
        for step in 0..=20 {
            let delta = step as f32 / 20.;
            let opacity = shimmer_opacity(delta);
            assert!((0.4..=1.0).contains(&opacity), "{delta}: {opacity}");
        }
    }
}
