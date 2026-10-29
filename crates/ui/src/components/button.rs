//! The button, in its five variants and three sizes.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, transparent_black, App, AppContext, BoxShadow, ClickEvent, Div, ElementId,
    FontWeight, IntoElement, ParentElement, Pixels, Render, RenderOnce, SharedString, Styled,
    Window,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::components::counter::Counter;
use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{CTL, CTL_SM, FONT_UI, FS_BASE, FS_SM, R_SM};
use crate::theme::ActiveTheme;

/// Visual variants of [`Button`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Secondary,
    Primary,
    Danger,
    Ghost,
    /// Toolbar button: taller, tighter padding, no border.
    Tool,
}

/// Sizes of [`Button`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Default,
    Small,
    Xs,
}

/// A push button.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    icon_only: Option<IconName>,
    variant: ButtonVariant,
    size: ButtonSize,
    selected: bool,
    disabled: bool,
    min_width: Option<Pixels>,
    counter: Option<Counter>,
    tooltip: Option<SharedString>,
    on_click: Option<crate::components::ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            icon_only: None,
            variant: ButtonVariant::Secondary,
            size: ButtonSize::Default,
            selected: false,
            disabled: false,
            min_width: None,
            counter: None,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn primary(id: impl Into<ElementId>) -> Self {
        Self::new(id).variant(ButtonVariant::Primary)
    }

    pub fn danger(id: impl Into<ElementId>) -> Self {
        Self::new(id).variant(ButtonVariant::Danger)
    }

    pub fn ghost(id: impl Into<ElementId>) -> Self {
        Self::new(id).variant(ButtonVariant::Ghost)
    }

    pub fn tool(id: impl Into<ElementId>) -> Self {
        Self::new(id).variant(ButtonVariant::Tool)
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A leading icon.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Draw only the icon, as a square icon button.
    pub fn icon_only(mut self, icon: IconName) -> Self {
        self.icon_only = Some(icon);
        self
    }

    /// The "on" state of a tool button.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = Some(width);
        self
    }

    /// A trailing count or badge.
    pub fn counter(mut self, counter: Counter) -> Self {
        self.counter = Some(counter);
        self
    }

    /// Built-in GPUI tooltip; required for icon-only buttons.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let primary_shadow = cx.theme().primary_shadow;
        debug_assert!(
            self.icon_only.is_none() || self.tooltip.is_some(),
            "icon-only buttons need a tooltip"
        );

        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(!self.disabled);
        let ring = !self.disabled && focus::focus_visible(&focus, window, cx);

        let (height, padding, font_size, icon_size) = match self.size {
            ButtonSize::Default => (CTL, px(10.), FS_BASE, px(14.)),
            ButtonSize::Small => (CTL_SM, px(8.), FS_SM, px(14.)),
            ButtonSize::Xs => (px(20.), px(0.), FS_SM, px(12.)),
        };
        let tool_height = if self.variant == ButtonVariant::Tool {
            px(28.)
        } else {
            height
        };

        let (mut background, mut foreground, mut border, mut weight) = match self.variant {
            ButtonVariant::Secondary => (
                colors.surface,
                colors.text,
                colors.border_strong,
                FontWeight::NORMAL,
            ),
            ButtonVariant::Primary => (
                colors.accent,
                colors.accent_fg,
                colors.accent,
                FontWeight::SEMIBOLD,
            ),
            ButtonVariant::Danger => (
                colors.surface,
                colors.error,
                colors.error.opacity(0.55),
                FontWeight::NORMAL,
            ),
            ButtonVariant::Ghost | ButtonVariant::Tool => (
                transparent_black(),
                colors.text,
                transparent_black(),
                FontWeight::NORMAL,
            ),
        };

        if self.variant == ButtonVariant::Tool && self.selected && !self.disabled {
            background = colors.selection;
            foreground = colors.selection_fg;
        }

        if self.disabled {
            weight = match self.variant {
                ButtonVariant::Primary => FontWeight::SEMIBOLD,
                _ => FontWeight::NORMAL,
            };
            match self.variant {
                ButtonVariant::Secondary | ButtonVariant::Danger => {
                    background = transparent_black();
                    foreground = colors.disabled;
                    border = colors.border;
                }
                ButtonVariant::Primary => {
                    background = colors.active;
                    foreground = colors.disabled;
                    border = colors.border;
                }
                ButtonVariant::Ghost | ButtonVariant::Tool => {
                    background = transparent_black();
                    foreground = colors.disabled;
                    border = transparent_black();
                }
            }
        }

        let mut root = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(tool_height)
            .text_size(font_size)
            .line_height(font_size)
            .font_family(FONT_UI)
            .font_weight(weight)
            .rounded(R_SM)
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(foreground);

        if let Some(width) = self.min_width {
            root = root.min_w(width);
        }

        if self.icon_only.is_some() {
            root = root.w(tool_height).px(px(0.));
        } else {
            let padding = if self.variant == ButtonVariant::Tool {
                px(8.)
            } else {
                padding
            };
            root = root.px(padding);
        }

        if self.disabled {
            root = root.cursor_not_allowed();
        } else {
            root = root.cursor_pointer().key_context("ui-button");
            let selected_tool = self.variant == ButtonVariant::Tool && self.selected;
            if !selected_tool {
                let hover_background = match self.variant {
                    ButtonVariant::Secondary | ButtonVariant::Ghost | ButtonVariant::Tool => {
                        colors.hover
                    }
                    ButtonVariant::Primary => colors.accent_hover,
                    ButtonVariant::Danger => colors.error.opacity(0.16),
                };
                let hover_border = match self.variant {
                    ButtonVariant::Primary => Some(colors.accent_hover),
                    ButtonVariant::Danger => Some(colors.error.opacity(0.55)),
                    _ => None,
                };
                root = root.hover(move |style| {
                    let style = style.bg(hover_background);
                    match hover_border {
                        Some(color) => style.border_color(color),
                        None => style,
                    }
                });

                let active_background = match self.variant {
                    ButtonVariant::Primary => {
                        let mut color = colors.accent;
                        color.l *= 0.92;
                        color
                    }
                    _ => colors.active,
                };
                root = root.active(move |style| style.bg(active_background));
            }

            if let Some(listener) = self.on_click {
                root = root.on_click(move |event, window, cx| listener(event, window, cx));
            }
            root = root.track_focus(&focus);
        }

        if self.variant == ButtonVariant::Primary {
            if let Some(shadow) = primary_shadow {
                root = root.shadow(vec![BoxShadow {
                    color: shadow.color,
                    offset: point(px(0.), shadow.offset_y),
                    blur_radius: shadow.blur,
                    spread_radius: shadow.spread,
                }]);
            }
        }

        if let Some(text) = self.tooltip {
            root = root.tooltip(move |_, cx| cx.new(|_| TooltipView { text: text.clone() }).into());
        }

        let content: Div = if let Some(icon) = self.icon_only {
            div().child(Icon::new(icon).size(icon_size).color(foreground))
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when_some(self.icon, |this, icon| {
                    this.child(Icon::new(icon).size(icon_size).color(foreground))
                })
                .when_some(self.label, |this, label| this.child(label))
                .when_some(self.counter, |this, counter| this.child(counter))
        };

        root.child(content).when(ring, |this| {
            this.child(focus::focus_ring(RingOffset::Outset1, R_SM, cx))
        })
    }
}

struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .px(px(8.))
            .py(px(4.))
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_SM)
            .bg(colors.surface)
            .text_color(colors.text)
            .text_size(FS_SM)
            .child(self.text.clone())
    }
}
