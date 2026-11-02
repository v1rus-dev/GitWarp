//! Ref badges: branch, remote, tag, stash and HEAD labels.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, ClickEvent, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FS_XS, R_SM};
use crate::theme::{ActiveTheme, ThemeColors};
use crate::Tooltip;

/// What a badge points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefKind {
    #[default]
    Local,
    Remote,
    Tag,
    Stash,
    Head,
    /// The "+N" badge that stands for collapsed refs.
    Overflow,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A small tinted ref label.
#[derive(IntoElement)]
pub struct RefBadge {
    id: ElementId,
    label: SharedString,
    kind: RefKind,
    max_label_width: Option<Pixels>,
    tooltip: Option<SharedString>,
    dashed: bool,
    on_click: Option<ClickHandler>,
}

impl RefBadge {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: RefKind::Local,
            max_label_width: None,
            tooltip: None,
            dashed: false,
            on_click: None,
        }
    }

    pub fn kind(mut self, kind: RefKind) -> Self {
        self.kind = kind;
        self
    }

    /// Cap the label at `width`; a longer label is ellipsized inside the badge.
    pub fn max_label_width(mut self, width: Pixels) -> Self {
        self.max_label_width = Some(width);
        self
    }

    /// A transparent badge with a dashed border: a reference that is not a ref
    /// of this commit (the History branch filter's "from {branch}" chip).
    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }

    /// Makes the badge a button.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Show `text` when the badge is hovered.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

fn kind_style(kind: RefKind, colors: ThemeColors) -> (Hsla, Option<IconName>, FontWeight) {
    match kind {
        RefKind::Local => (colors.accent, Some(IconName::Branch), FontWeight::SEMIBOLD),
        RefKind::Remote => (colors.muted, Some(IconName::Cloud), FontWeight::MEDIUM),
        RefKind::Tag => (colors.warning, Some(IconName::Tag), FontWeight::SEMIBOLD),
        RefKind::Stash => (
            colors.st_untracked,
            Some(IconName::Stash),
            FontWeight::SEMIBOLD,
        ),
        RefKind::Head => (colors.accent, Some(IconName::Branch), FontWeight::SEMIBOLD),
        // Like a remote badge, but heavier and without an icon.
        RefKind::Overflow => (colors.muted, None, FontWeight::SEMIBOLD),
    }
}

impl RenderOnce for RefBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (color, icon, weight) = kind_style(self.kind, colors);

        let (mut background, border, foreground) = if self.kind == RefKind::Head {
            (colors.accent, colors.accent, colors.accent_fg)
        } else {
            (color.opacity(0.10), color.opacity(0.28), color)
        };
        let weight = if self.dashed {
            FontWeight::MEDIUM
        } else {
            weight
        };
        if self.dashed {
            background = gpui::transparent_black();
        }
        let hover_background = color.opacity(0.14);
        let dashed = self.dashed;

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .h(px(18.))
            .px(px(6.))
            .rounded(R_SM)
            .bg(background)
            .border_1()
            .when(dashed, |this| this.border_dashed())
            .border_color(border)
            .text_color(foreground)
            .text_size(FS_XS)
            .line_height(px(18.))
            .font_weight(weight)
            .when_some(icon, |this, icon| {
                this.child(Icon::new(icon).size(px(11.)).color(foreground))
            })
            .child(
                div()
                    .when_some(self.max_label_width, |this, width| {
                        this.max_w(width).overflow_hidden().truncate()
                    })
                    .child(self.label),
            )
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(Tooltip::text(tooltip))
            })
            .when_some(self.on_click, |this, handler| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover_background))
                    .on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}
