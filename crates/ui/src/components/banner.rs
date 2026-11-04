//! Inline banners.

use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, ElementId, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window,
};

use crate::components::SelectableText;
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
    action: Option<AnyElement>,
    trailing: Option<AnyElement>,
    flat: bool,
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
            action: None,
            trailing: None,
            flat: false,
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

    /// An action (a button) under the text, inside the banner.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// Controls on the right edge of the banner, vertically centred (an
    /// operation banner's Abort / Commit buttons).
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// A full-width bar: square corners, only the bottom edge drawn.
    pub fn flat(mut self) -> Self {
        self.flat = true;
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

fn kind_tag(kind: BannerKind) -> u8 {
    match kind {
        BannerKind::Warn => 0,
        BannerKind::Error => 1,
        BannerKind::Info => 2,
        BannerKind::Ok => 3,
    }
}

/// A stable id for the banner's text state; banners have no id of their own,
/// and two identical banners may share their (read-only) selection harmlessly.
fn banner_fingerprint(kind: BannerKind, title: &str, body: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    kind_tag(kind).hash(&mut hasher);
    title.hash(&mut hasher);
    body.hash(&mut hasher);
    hasher.finish()
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (color, border_alpha, background_alpha, icon) = style_for(self.kind, colors);
        let fingerprint = banner_fingerprint(self.kind, &self.title, self.body.as_deref());
        let title_id = ElementId::Name(format!("banner-title-{fingerprint}").into());
        let body_id = ElementId::Name(format!("banner-body-{fingerprint}").into());

        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(10.))
            .pl(px(12.))
            .pr(px(12.))
            .w_full()
            .when(self.flat, |this| this.border_b_1())
            .when(!self.flat, |this| this.border_1().rounded(R_MD))
            .when(self.trailing.is_some(), |this| this.items_center())
            .border_color(color.opacity(border_alpha))
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
                            .child(SelectableText::new(title_id, self.title.clone())),
                    )
                    .when_some(self.body, |this, body| {
                        this.child(
                            div()
                                .when(self.mono_body, |this| this.font_family(FONT_MONO))
                                .text_color(colors.muted)
                                .child(SelectableText::new(body_id, body)),
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
                                .children(self.list.into_iter().enumerate().map(
                                    |(index, item)| {
                                        let text: SharedString = if self.plain_list {
                                            item
                                        } else {
                                            SharedString::from(format!("• {item}"))
                                        };
                                        div().child(SelectableText::new(
                                            ElementId::Name(
                                                format!("banner-list-{fingerprint}-{index}").into(),
                                            ),
                                            text,
                                        ))
                                    },
                                )),
                        )
                    })
                    .when_some(self.action, |this, action| {
                        this.child(div().mt(px(4.)).child(action))
                    }),
            )
            .when_some(self.trailing, |this, trailing| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(trailing),
                )
            })
    }
}
