//! Inline banners.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_MONO, FS_BASE, FS_SM, R_MD};
use crate::theme::{ActiveTheme, ThemeColors};

/// What a banner communicates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    Warn,
    Error,
    Info,
    Ok,
}

/// A tinted message box with an icon, title, body and optional list.
#[derive(IntoElement)]
pub struct Banner {
    kind: BannerKind,
    title: SharedString,
    body: Option<SharedString>,
    list: Vec<SharedString>,
    plain_list: bool,
    mono_body: bool,
}

impl Banner {
    pub fn new(kind: BannerKind, title: impl Into<SharedString>) -> Self {
        Self {
            kind,
            title: title.into(),
            body: None,
            list: Vec::new(),
            plain_list: false,
            mono_body: false,
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Draws the body in the mono font (raw git output).
    pub fn mono_body(mut self) -> Self {
        self.mono_body = true;
        self
    }

    /// Draws the list as plain mono rows: no bullets, no indent (file names).
    pub fn plain_list(mut self) -> Self {
        self.plain_list = true;
        self
    }

    pub fn list(mut self, items: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.list = items.into_iter().map(Into::into).collect();
        self
    }
}

fn style_for(kind: BannerKind, colors: ThemeColors) -> (Hsla, f32, f32, IconName) {
    match kind {
        BannerKind::Warn => (colors.warning, 0.55, 0.10, IconName::Warn),
        BannerKind::Error => (colors.error, 0.55, 0.10, IconName::Err),
        BannerKind::Info => (colors.info, 0.50, 0.09, IconName::Info),
        BannerKind::Ok => (colors.success, 0.50, 0.09, IconName::Check),
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (color, border_alpha, background_alpha, icon) = style_for(self.kind, colors);

        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(10.))
            .pl(px(12.))
            .pr(px(12.))
            .w_full()
            .border_1()
            .border_color(color.opacity(border_alpha))
            .rounded(R_MD)
            .bg(color.opacity(background_alpha))
            .text_size(FS_BASE)
            .child(
                div()
                    .mt(px(2.))
                    .child(Icon::new(icon).size(px(16.)).color(color)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(self.title),
                    )
                    .when_some(self.body, |this, body| {
                        this.child(
                            div()
                                .when(self.mono_body, |this| this.font_family(FONT_MONO))
                                .text_color(colors.muted)
                                .child(body),
                        )
                    })
                    .when(!self.list.is_empty(), |this| {
                        this.child(
                            div()
                                .when(!self.plain_list, |this| this.pl(px(18.)))
                                .when(self.plain_list, |this| this.font_family(FONT_MONO))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .children(self.list.into_iter().map(|item| {
                                    if self.plain_list {
                                        div().child(item)
                                    } else {
                                        div().child(format!("• {item}"))
                                    }
                                })),
                        )
                    }),
            )
    }
}
