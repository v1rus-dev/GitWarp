//! Determinate and indeterminate progress bars.

use std::time::Duration;

use gpui::{
    div, ease_in_out, px, relative, Animation, AnimationExt, App, DefiniteLength, ElementId,
    InteractiveElement, IntoElement, ParentElement, RenderOnce, Styled, Window,
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
    fill: bool,
}

impl Progress {
    /// `value` is clamped to `0.0..=1.0`.
    pub fn new(id: impl Into<ElementId>, value: f32) -> Self {
        Self {
            id: id.into(),
            value,
            dialog: false,
            indeterminate: false,
            fill: false,
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

    /// Stretch the track to the width its flex parent gives it instead of the
    /// fixed 140px, e.g. for a load bar spanning a pane.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl RenderOnce for Progress {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let height = if self.dialog { px(6.) } else { px(4.) };
        let radius = if self.dialog { px(3.) } else { px(2.) };

        let track_debug = format!("{}-track", self.id);
        let mut track = div()
            .relative()
            .h(height)
            .rounded(radius)
            .bg(colors.active)
            .debug_selector(move || track_debug)
            .overflow_hidden();
        track = if self.fill {
            // `flex_1` also needs a full width for the case where the parent
            // lays the bar out on its own.
            track.w_full().flex_1()
        } else {
            track.w(px(TRACK_WIDTH))
        };

        if self.indeterminate {
            // Relative geometry: the segment and its travel are fractions of
            // the track, so the animation also works on a stretched bar. On
            // the 140px track this is the same 49px segment moving 91px.
            let fill_debug = format!("{}-fill", self.id);
            let fill = div()
                .absolute()
                .top(px(0.))
                .w(relative(0.35))
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
                        this.left(relative(delta * 0.65))
                    },
                );
            track.child(fill)
        } else {
            let width: DefiniteLength = if self.fill {
                relative(self.value.clamp(0., 1.))
            } else {
                px(TRACK_WIDTH * self.value.clamp(0., 1.)).into()
            };
            track.child(div().w(width).h_full().rounded(radius).bg(colors.accent))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_is_off_by_default() {
        assert!(!Progress::new("progress", 0.5).fill);
        assert!(Progress::new("progress", 0.5).fill().fill);
    }
}
