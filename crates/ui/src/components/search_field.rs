//! A search/filter field: icon, borderless input and optional key hint.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, App, BoxShadow, CursorStyle, ElementId, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, RenderOnce, SharedString, Styled, Window,
};

use super::text_input::TextInput;
use super::Kbd;
use crate::icon::{Icon, IconName};
use crate::metrics::{CTL, R_SM};
use crate::theme::ActiveTheme;

/// A bordered wrapper around a [`TextInput`].
#[derive(IntoElement)]
pub struct SearchField {
    id: ElementId,
    input: Entity<TextInput>,
    hint: Option<SharedString>,
}

impl SearchField {
    pub fn new(id: impl Into<ElementId>, input: Entity<TextInput>) -> Self {
        Self {
            id: id.into(),
            input,
            hint: None,
        }
    }

    /// A trailing key hint, e.g. `Ctrl K`.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn input(&self) -> &Entity<TextInput> {
        &self.input
    }
}

impl RenderOnce for SearchField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        self.input.update(cx, |input, _| input.set_borderless(true));
        let focus_handle = self.input.read(cx).focus_handle().clone();
        let focused = focus_handle.is_focused(window);

        let border = if focused {
            colors.focus
        } else {
            colors.border_strong
        };

        let mut root = div()
            .id(self.id.clone())
            // A field fills its container: with an auto width a flex row only
            // takes its content width (icon + padding), which collapses the
            // input and hides the placeholder.
            .w_full()
            .relative()
            .debug_selector({
                let id = self.id.to_string();
                move || id.clone()
            })
            .h(CTL)
            .pl(px(8.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(colors.bg)
            .border_1()
            .border_color(border)
            .rounded(R_SM)
            .text_color(colors.muted)
            .cursor(CursorStyle::IBeam);

        if focused {
            root = root.shadow(vec![BoxShadow {
                color: colors.focus,
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(1.),
            }]);
        } else {
            let hover_border = colors.muted;
            root = root.hover(move |style| style.border_color(hover_border));
        }

        root = root.on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.focus(&focus_handle);
        });

        root.child(
            Icon::new(IconName::Search)
                .size(px(14.))
                .color(colors.muted),
        )
        .child(div().flex_1().min_w_0().overflow_hidden().child(self.input))
        .when_some(self.hint, |this, hint| this.child(Kbd::new(hint)))
    }
}
