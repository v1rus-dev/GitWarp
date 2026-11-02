//! Pull request badges and CI glyphs: the `#142` pill that marks a branch
//! with a pull / merge request, and the state glyph of its checks.

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, percentage, px, svg, Animation, AnimationExt, App, ClickEvent, ElementId, Hsla,
    InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Transformation, Window,
};

use crate::icon::{Icon, IconName};
use crate::metrics::{FS_XS, R_SM};
use crate::theme::{ActiveTheme, ThemeColors};
use crate::Tooltip;

/// Lifecycle of a pull / merge request, as the badge draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrBadgeState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrBadgeState {
    fn icon(self) -> IconName {
        match self {
            Self::Open => IconName::Pr,
            Self::Draft => IconName::PrDraft,
            Self::Merged => IconName::Merge,
            Self::Closed => IconName::PrClosed,
        }
    }

    /// The text colour and the border colour of the badge.
    fn colors(self, colors: ThemeColors) -> (Hsla, Hsla) {
        match self {
            Self::Open => (colors.success, colors.success.opacity(0.5)),
            Self::Draft => (colors.muted, colors.border_strong),
            Self::Merged => (colors.st_untracked, colors.st_untracked.opacity(0.5)),
            Self::Closed => (colors.error, colors.error.opacity(0.5)),
        }
    }
}

/// The review outcome mark on a badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReviewMark {
    Approved,
    ChangesRequested,
}

impl ReviewMark {
    pub fn icon(self) -> IconName {
        match self {
            Self::Approved => IconName::Check,
            Self::ChangesRequested => IconName::Comment,
        }
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A small pill: state icon, text (`#142`, or a state name) and optionally a
/// review mark. A button when given a click handler.
#[derive(IntoElement)]
pub struct PrBadge {
    id: ElementId,
    state: PrBadgeState,
    text: SharedString,
    review: Option<ReviewMark>,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl PrBadge {
    pub fn new(
        id: impl Into<ElementId>,
        state: PrBadgeState,
        text: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            state,
            text: text.into(),
            review: None,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn review(mut self, review: Option<ReviewMark>) -> Self {
        self.review = review;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Makes the badge a button. The row it sits in does not also see the
    /// press.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for PrBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (foreground, border) = self.state.colors(colors);
        let hover = colors.hover;
        let debug_name = self.id.to_string();
        div()
            .id(self.id)
            .debug_selector(move || debug_name)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .h(px(18.))
            .px(px(5.))
            .rounded(R_SM)
            .border_1()
            .border_color(border)
            .text_color(foreground)
            .text_size(FS_XS)
            .line_height(px(18.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(Icon::new(self.state.icon()).size(px(11.)).color(foreground))
            .child(self.text)
            .when_some(self.review, |this, review| {
                this.child(
                    div()
                        .ml(px(2.))
                        .child(Icon::new(review.icon()).size(px(10.)).color(foreground)),
                )
            })
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(Tooltip::text(tooltip))
            })
            .when_some(self.on_click, |this, handler| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    // The tree and history rows act on mouse-down; keep
                    // the press for the badge.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        handler(event, window, cx)
                    })
            })
    }
}

/// The state of the checks of a commit, as the glyph draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CiGlyphState {
    Passed,
    Failed,
    Running,
    Pending,
    Cancelled,
}

impl CiGlyphState {
    fn icon(self) -> IconName {
        match self {
            Self::Passed => IconName::CheckFill,
            Self::Failed => IconName::XFill,
            Self::Running => IconName::Sync,
            Self::Pending => IconName::Hourglass,
            Self::Cancelled => IconName::Skip,
        }
    }

    fn color(self, colors: ThemeColors) -> Hsla {
        match self {
            Self::Passed => colors.success,
            Self::Failed => colors.error,
            Self::Running => colors.warning,
            Self::Pending | Self::Cancelled => colors.muted,
        }
    }
}

/// One state glyph; a running one rotates.
#[derive(IntoElement)]
pub struct CiGlyph {
    id: ElementId,
    state: CiGlyphState,
    tooltip: Option<SharedString>,
}

impl CiGlyph {
    pub fn new(id: impl Into<ElementId>, state: CiGlyphState) -> Self {
        Self {
            id: id.into(),
            state,
            tooltip: None,
        }
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl RenderOnce for CiGlyph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.state.color(cx.theme().colors);
        let debug_name = self.id.to_string();
        let size = px(12.);
        let glyph = if self.state == CiGlyphState::Running {
            svg()
                .path(self.state.icon().path())
                .w(size)
                .h(size)
                .text_color(color)
                .with_animation(
                    ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "spin".into()),
                    Animation::new(Duration::from_secs(1)).repeat(),
                    |this, delta| {
                        this.with_transformation(Transformation::rotate(percentage(delta)))
                    },
                )
                .into_any_element()
        } else {
            Icon::new(self.state.icon())
                .size(size)
                .color(color)
                .into_any_element()
        };
        div()
            .id(self.id)
            .debug_selector(move || debug_name)
            .flex()
            .flex_none()
            .items_center()
            .child(glyph)
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(Tooltip::text(tooltip))
            })
    }
}
