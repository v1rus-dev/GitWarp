//! The rotating spinner.

use std::time::Duration;

use gpui::{
    percentage, svg, Animation, AnimationExt, App, ElementId, IntoElement, Pixels, RenderOnce,
    Styled, Transformation, Window,
};

use crate::icon::IconName;
use crate::theme::ActiveTheme;

/// The fetch icon rotating once per second.
#[derive(IntoElement)]
pub struct Spinner {
    id: ElementId,
    size: Pixels,
}

impl Spinner {
    pub fn new(id: impl Into<ElementId>, size: Pixels) -> Self {
        Self {
            id: id.into(),
            size,
        }
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = cx.theme().colors.text;
        svg()
            .path(IconName::Fetch.path())
            .w(self.size)
            .h(self.size)
            .text_color(color)
            .with_animation(
                self.id,
                Animation::new(Duration::from_secs(1)).repeat(),
                |this, delta| this.with_transformation(Transformation::rotate(percentage(delta))),
            )
    }
}
