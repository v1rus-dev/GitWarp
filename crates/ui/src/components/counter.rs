//! Counters, key caps and count pills.

use gpui::ParentElement;
use gpui::{div, px, App, IntoElement, RenderOnce, SharedString, Styled, Window};

use crate::metrics::{FONT_UI, FS_XS, R_PILL, R_SM};
use crate::theme::ActiveTheme;

/// A number badge, optionally accent coloured.
#[derive(IntoElement)]
pub struct Counter {
    value: SharedString,
    accent: bool,
}

impl Counter {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            accent: false,
        }
    }

    /// Accent fill, used for actionable counts.
    pub fn accent(mut self) -> Self {
        self.accent = true;
        self
    }
}

impl RenderOnce for Counter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .h(px(16.))
            .min_w(px(18.))
            .px(px(5.))
            .rounded(R_PILL)
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.accent {
                colors.accent
            } else {
                colors.active
            })
            .text_color(if self.accent {
                colors.accent_fg
            } else {
                colors.text
            })
            .text_size(FS_XS)
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .line_height(FS_XS)
            .child(self.value)
    }
}

/// A key cap.
#[derive(IntoElement)]
pub struct Kbd {
    text: SharedString,
}

impl Kbd {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(18.))
            .px(px(5.))
            .py(px(1.))
            .border_1()
            .border_b_2()
            .border_color(colors.border_strong)
            .rounded(R_SM)
            .bg(colors.bg)
            .text_color(colors.muted)
            .font_family(FONT_UI)
            .text_size(px(10.))
            .line_height(px(14.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(self.text)
    }
}

/// A quiet count pill, e.g. a section's item count.
#[derive(IntoElement)]
pub struct CountPill {
    value: SharedString,
    selected: bool,
}

impl CountPill {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            selected: false,
        }
    }

    /// On a selected tree row: transparent fill, `selection_sub` text.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl RenderOnce for CountPill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .text_size(FS_XS)
            .text_color(if self.selected {
                colors.selection_sub
            } else {
                colors.muted
            })
            .bg(if self.selected {
                gpui::transparent_black()
            } else {
                colors.active
            })
            .rounded(R_PILL)
            .px(px(6.))
            .child(self.value)
    }
}
