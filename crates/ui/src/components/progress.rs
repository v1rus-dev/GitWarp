//! Determinate and indeterminate progress bars.

use std::time::Duration;

use gpui::{
    div, ease_in_out, px, Animation, AnimationExt, App, ElementId, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, Styled, Window,
};

use crate::theme::ActiveTheme;

const TRACK_WIDTH: f32 = 140.;

/// A progress bar, 4px high or 6px in dialogs.
#[derive(IntoElement)]
pub struct Progress {
    id: ElementId,
    value: f32,
    dialog: bool,
    indeterminate: bool,
}

impl Progress {
    /// `value` is clamped to `0.0..=1.0`.
    pub fn new(id: impl Into<ElementId>, value: f32) -> Self {
        Self {
            id: id.into(),
            value,
            dialog: false,
            indeterminate: false,
        }
    }

    /// The thicker dialog variant.
    pub fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }

    /// A 35% segment sliding across the track.
    pub fn indeterminate(mut self) -> Self {
        self.indeterminate = true;
        self
    }
}

impl RenderOnce for Progress {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let height = if self.dialog { px(6.) } else { px(4.) };
        let radius = if self.dialog { px(3.) } else { px(2.) };

        let track_debug = format!("{}-track", self.id);
        let track = div()
            .relative()
            .w(px(TRACK_WIDTH))
            .h(height)
            .rounded(radius)
            .bg(colors.active)
            .debug_selector(move || track_debug)
            .overflow_hidden();

        if self.indeterminate {
            let width = TRACK_WIDTH * 0.35;
            let fill_debug = format!("{}-fill", self.id);
            let fill = div()
                .absolute()
                .top(px(0.))
                .w(px(width))
                .h_full()
                .rounded(radius)
                .bg(colors.accent)
                .debug_selector(move || fill_debug)
                .with_animation(
                    self.id,
                    Animation::new(Duration::from_millis(1200))
                        .repeat()
                        .with_easing(ease_in_out),
                    move |this, delta| {
                        // Stay inside the track so the segment is visible even
                        // on the first frame and at the loop boundaries.
                        this.left(px(delta * (TRACK_WIDTH - width)))
                    },
                );
            track.child(fill)
        } else {
            let width = TRACK_WIDTH * self.value.clamp(0., 1.);
            track.child(
                div()
                    .w(px(width))
                    .h_full()
                    .rounded(radius)
                    .bg(colors.accent),
            )
        }
    }
}
