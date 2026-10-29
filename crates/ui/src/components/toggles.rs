//! Checkbox, radio and switch.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, Animation, AnimationExt, AnyElement, App, ClickEvent, ElementId, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{FS_BASE, R_PILL, R_XS};
use crate::theme::ActiveTheme;

/// A checkbox with a label; supports the mixed state.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    mixed: bool,
    disabled: bool,
    on_toggle: Option<crate::components::ToggleHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            checked: false,
            mixed: false,
            disabled: false,
            on_toggle: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_toggle(mut self, listener: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(!self.disabled);
        let ring = !self.disabled && focus::focus_visible(&focus, window, cx);
        let on = self.checked || self.mixed;

        let mut box_ = div()
            .relative()
            .w(px(14.))
            .h(px(14.))
            .rounded(R_XS)
            .border_1()
            .border_color(if on {
                colors.accent
            } else {
                colors.border_strong
            })
            .bg(if on { colors.accent } else { colors.bg })
            .flex()
            .items_center()
            .justify_center();

        if !self.disabled && !on {
            let border = colors.muted;
            box_ = box_.hover(move |style| style.border_color(border));
        }
        if on {
            box_ = box_.child(
                Icon::new(if self.mixed {
                    IconName::Minus
                } else {
                    IconName::Check
                })
                .size(px(10.))
                .color(colors.accent_fg),
            );
        }
        if ring {
            box_ = box_.child(focus::focus_ring(RingOffset::Outset2, R_XS, cx));
        }

        let mut root = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(26.))
            .key_context("ui-checkbox")
            .child(box_)
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .text_size(FS_BASE)
                        .text_color(colors.text)
                        .child(label),
                )
            });

        if self.disabled {
            root = root.opacity(0.45).cursor_not_allowed();
        } else {
            root = root.cursor_pointer().track_focus(&focus);
            if let Some(listener) = self.on_toggle {
                let next = !self.checked;
                root = root.on_click(move |_, window, cx| listener(next, window, cx));
            }
        }
        root
    }
}

/// A radio button with a label.
#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_click: Option<crate::components::ClickHandler>,
}

impl Radio {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            checked: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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

impl RenderOnce for Radio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(!self.disabled);
        let ring = !self.disabled && focus::focus_visible(&focus, window, cx);

        let mut dot = div()
            .relative()
            .w(px(14.))
            .h(px(14.))
            .rounded(px(7.))
            .border_1()
            .border_color(if self.checked {
                colors.accent
            } else {
                colors.border_strong
            })
            .bg(colors.bg)
            .flex()
            .items_center()
            .justify_center();
        if self.checked {
            dot = dot.child(div().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(colors.accent));
        }
        if ring {
            dot = dot.child(focus::focus_ring(RingOffset::Outset2, px(7.), cx));
        }

        let mut root = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(26.))
            .key_context("ui-radio")
            .child(dot)
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .text_size(FS_BASE)
                        .text_color(colors.text)
                        .child(label),
                )
            });

        if self.disabled {
            root = root.opacity(0.45).cursor_not_allowed();
        } else {
            root = root.cursor_pointer().track_focus(&focus);
            if let Some(listener) = self.on_click {
                root = root.on_click(move |event, window, cx| listener(event, window, cx));
            }
        }
        root
    }
}

struct SwitchState {
    focus: FocusHandle,
    checked: bool,
    generation: u64,
}

/// A switch with a 150 ms knob animation.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_toggle: Option<crate::components::ToggleHandler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            checked: false,
            disabled: false,
            on_toggle: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_toggle(mut self, listener: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| SwitchState {
            focus: cx.focus_handle(),
            checked: self.checked,
            generation: 0,
        });
        let (focus, generation) = state.update(cx, |state, _| {
            if state.checked != self.checked {
                state.checked = self.checked;
                state.generation += 1;
            }
            (state.focus.clone(), state.generation)
        });
        let focus = focus.tab_stop(!self.disabled);
        let ring = !self.disabled && focus::focus_visible(&focus, window, cx);
        let checked = self.checked;

        let knob_debug = format!("{}-knob", self.id);
        let knob = div()
            .absolute()
            .top(px(2.))
            .w(px(10.))
            .h(px(10.))
            .debug_selector(move || knob_debug)
            .rounded(px(5.))
            .bg(if checked {
                colors.accent_fg
            } else {
                colors.muted
            });

        let knob: AnyElement = if generation == 0 {
            // First paint: jump straight to the final position.
            knob.left(px(if checked { 14. } else { 2. }))
                .into_any_element()
        } else {
            let animation_id = ElementId::Name(format!("{}-knob-{generation}", self.id).into());
            knob.left(px(if checked { 2. } else { 14. }))
                .with_animation(
                    animation_id,
                    Animation::new(Duration::from_millis(150)),
                    move |this, delta| {
                        let left = if checked {
                            2. + 12. * delta
                        } else {
                            14. - 12. * delta
                        };
                        this.left(px(left))
                    },
                )
                .into_any_element()
        };

        let track_debug = format!("{}-track", self.id);
        let mut track = div()
            .relative()
            .w(px(28.))
            .h(px(16.))
            .debug_selector(move || track_debug)
            .rounded(R_PILL)
            .border_1()
            .border_color(if checked {
                colors.accent
            } else {
                colors.border_strong
            })
            .bg(if checked {
                colors.accent
            } else {
                colors.active
            })
            .child(knob);
        if ring {
            track = track.child(focus::focus_ring(RingOffset::Outset2, R_PILL, cx));
        }

        let mut root = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(26.))
            .key_context("ui-switch")
            .child(track)
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .text_size(FS_BASE)
                        .text_color(colors.text)
                        .child(label),
                )
            });

        if self.disabled {
            root = root.opacity(0.45).cursor_not_allowed();
        } else {
            root = root.cursor_pointer().track_focus(&focus);
            if let Some(listener) = self.on_toggle {
                let next = !checked;
                root = root.on_click(move |_, window, cx| listener(next, window, cx));
            }
        }
        root
    }
}
