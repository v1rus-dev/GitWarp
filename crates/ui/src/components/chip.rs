//! Inline chips: a short label with an optional state icon.

use gpui::prelude::FluentBuilder;
use gpui::InteractiveElement;
use gpui::{
    div, px, App, ElementId, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FS_XS, R_SM};
use crate::theme::ActiveTheme;

/// The state a chip communicates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChipKind {
    #[default]
    None,
    Ok,
    Warn,
    Err,
    Acc,
}

/// A bordered, tinted label.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    kind: ChipKind,
    icon: Option<IconName>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ChipKind::None,
            icon: None,
        }
    }

    pub fn kind(mut self, kind: ChipKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

fn colors_for(
    kind: ChipKind,
    muted: Hsla,
    success: Hsla,
    warning: Hsla,
    error: Hsla,
    accent: Hsla,
) -> (Hsla, Hsla) {
    match kind {
        ChipKind::None => (muted, muted),
        ChipKind::Ok => (success, success.opacity(0.5)),
        ChipKind::Warn => (warning, warning.opacity(0.5)),
        ChipKind::Err => (error, error.opacity(0.5)),
        ChipKind::Acc => (accent, accent.opacity(0.5)),
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (foreground, border) = colors_for(
            self.kind,
            colors.muted,
            colors.success,
            colors.warning,
            colors.error,
            colors.accent,
        );
        let border = if self.kind == ChipKind::None {
            colors.border_strong
        } else {
            border
        };

        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(18.))
            .px(px(6.))
            .rounded(R_SM)
            .border_1()
            .border_color(border)
            .text_color(foreground)
            .text_size(FS_XS)
            .line_height(px(18.))
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(px(10.)).color(foreground))
            })
            .child(self.label)
    }
}
