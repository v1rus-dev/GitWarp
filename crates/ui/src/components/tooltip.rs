//! The floating tooltip view.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyView, App, AppContext, Context, IntoElement, ParentElement, Render, SharedString,
    Styled, Window,
};

use crate::components::Kbd;
use crate::metrics::{FS_SM, R_SM};
use crate::theme::ActiveTheme;

/// A surface panel with a line of text and an optional trailing key cap.
///
/// Build one as the closure GPUI's `tooltip` expects with [`Tooltip::text`] or
/// [`Tooltip::with_kbd`].
#[derive(Clone, Debug)]
pub struct Tooltip {
    label: SharedString,
    kbd: Option<SharedString>,
}

impl Tooltip {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            kbd: None,
        }
    }

    pub fn kbd(mut self, kbd: impl Into<SharedString>) -> Self {
        self.kbd = Some(kbd.into());
        self
    }

    /// A tooltip closure for a plain text hint.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let tooltip = Self::new(text);
        move |_, cx| cx.new(|_| tooltip.clone()).into()
    }

    /// A tooltip closure with a trailing key cap.
    pub fn with_kbd(
        text: impl Into<SharedString>,
        kbd: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let tooltip = Self::new(text).kbd(kbd);
        move |_, cx| cx.new(|_| tooltip.clone()).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .max_w(px(320.))
            .px(px(8.))
            .py(px(4.))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_SM)
            .shadow(cx.theme().pop_shadow())
            // A tooltip is its own root and does not inherit the window's text style, so it
            // needs the CJK fallbacks set explicitly.
            .font(crate::fonts::ui_font())
            .text_size(FS_SM)
            .text_color(colors.text)
            .child(self.label.clone())
            .when_some(self.kbd.clone(), |this, kbd| this.child(Kbd::new(kbd)))
    }
}
