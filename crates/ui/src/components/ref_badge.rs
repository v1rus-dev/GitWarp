//! Ref badges: branch, remote, tag, stash and HEAD labels.

use gpui::prelude::FluentBuilder;
use gpui::InteractiveElement;
use gpui::{
    div, px, App, ElementId, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FS_XS, R_SM};
use crate::theme::{ActiveTheme, ThemeColors};

/// What a badge points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RefKind {
    #[default]
    Local,
    Remote,
    Tag,
    Stash,
    Head,
}

/// A small tinted ref label.
#[derive(IntoElement)]
pub struct RefBadge {
    id: ElementId,
    label: SharedString,
    kind: RefKind,
}

impl RefBadge {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: RefKind::Local,
        }
    }

    pub fn kind(mut self, kind: RefKind) -> Self {
        self.kind = kind;
        self
    }
}

fn kind_style(kind: RefKind, colors: ThemeColors) -> (Hsla, IconName, FontWeight) {
    match kind {
        RefKind::Local => (colors.accent, IconName::Branch, FontWeight::SEMIBOLD),
        RefKind::Remote => (colors.muted, IconName::Cloud, FontWeight::MEDIUM),
        RefKind::Tag => (colors.warning, IconName::Tag, FontWeight::SEMIBOLD),
        RefKind::Stash => (colors.st_untracked, IconName::Stash, FontWeight::SEMIBOLD),
        RefKind::Head => (colors.accent, IconName::Branch, FontWeight::SEMIBOLD),
    }
}

impl RenderOnce for RefBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (color, icon, weight) = kind_style(self.kind, colors);

        let (background, border, foreground) = if self.kind == RefKind::Head {
            (colors.accent, colors.accent, colors.accent_fg)
        } else {
            (color.opacity(0.10), color.opacity(0.28), color)
        };
        let icon = if self.kind == RefKind::Head {
            None
        } else {
            Some(icon)
        };

        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(3.))
            .h(px(18.))
            .px(px(6.))
            .rounded(R_SM)
            .bg(background)
            .border_1()
            .border_color(border)
            .text_color(foreground)
            .text_size(FS_XS)
            .line_height(px(18.))
            .font_weight(weight)
            .when_some(icon, |this, icon| {
                this.child(Icon::new(icon).size(px(11.)).color(foreground))
            })
            .child(self.label)
    }
}
