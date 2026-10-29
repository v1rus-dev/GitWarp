//! Empty and error states.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_MONO, FS_MD, MONO, R_MD};
use crate::theme::ActiveTheme;

/// A centred placeholder with an icon, title, body, optional actions and an
/// optional monospace error box.
#[derive(IntoElement)]
pub struct EmptyState {
    title: SharedString,
    text: Option<SharedString>,
    icon: IconName,
    icon_color: Option<Hsla>,
    error: Option<SharedString>,
    actions: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            text: None,
            icon: IconName::Folder,
            icon_color: None,
            error: None,
            actions: None,
        }
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    pub fn icon_color(mut self, color: Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Show an error box and use the error icon in the error colour.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self.icon = IconName::Err;
        self
    }

    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let icon_color = self.icon_color.unwrap_or(if self.error.is_some() {
            colors.error
        } else {
            colors.muted
        });

        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .p(px(24.))
            .text_color(colors.muted)
            .child(
                div()
                    .w(px(44.))
                    .h(px(44.))
                    .rounded(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(colors.surface)
                    .border_1()
                    .border_color(colors.border)
                    .child(Icon::new(self.icon).size(px(22.)).color(icon_color)),
            )
            .child(
                div()
                    .mt(px(4.))
                    .text_size(FS_MD)
                    .text_color(colors.text)
                    .child(self.title),
            )
            .when_some(self.text, |this, text| {
                this.child(div().max_w(px(46. * 7.)).text_center().child(text))
            })
            .when_some(self.error, |this, error| {
                this.child(
                    div()
                        .max_w(px(56. * 7.))
                        .p(px(8.))
                        .pl(px(10.))
                        .pr(px(10.))
                        .rounded(R_MD)
                        .border_1()
                        .border_color(colors.border_strong)
                        .bg(colors.bg)
                        .font_family(FONT_MONO)
                        .text_size(MONO)
                        .line_height(px(18.))
                        .text_color(colors.text)
                        .child(error),
                )
            })
            .when_some(self.actions, |this, actions| {
                this.child(div().mt(px(4.)).flex().gap(px(8.)).children([actions]))
            })
    }
}
