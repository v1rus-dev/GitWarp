//! Modal dialogs: chrome, focus trap and default-button handling.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, Context, ElementId, Entity, FocusHandle, FontWeight,
    IntoElement, ParentElement, Pixels, Render, SharedString, Styled, Window,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::actions::{Cancel, Confirm, FocusNext, FocusPrev};
use crate::components::{Button, ButtonSize, TextInputSubmit};
use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_UI, FS_MD, FS_SM, R_LG};
use crate::overlay::{self, LayerId};
use crate::theme::ActiveTheme;

/// What a dialog focus stop is, for Enter and Tab handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogFocusKind {
    /// A text input: Enter submits the default button.
    Input,
    /// A button, checkbox or similar: Enter activates the control itself.
    Control,
}

/// One stop of a dialog's focus trap.
#[derive(Clone, Debug)]
pub struct DialogFocus {
    pub handle: FocusHandle,
    pub kind: DialogFocusKind,
}

impl DialogFocus {
    pub fn input(handle: FocusHandle) -> Self {
        Self {
            handle,
            kind: DialogFocusKind::Input,
        }
    }

    pub fn control(handle: FocusHandle) -> Self {
        Self {
            handle,
            kind: DialogFocusKind::Control,
        }
    }
}

/// A dialog callback.
type DialogAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Builds a dialog; called once when it opens and again on every frame.
type DialogBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Dialog>;

/// A dialog panel, rendered inside [`open`].
///
/// The builder runs on every frame, so it must rebuild elements from state
/// that already exists rather than create new entities.
pub struct Dialog {
    title: SharedString,
    icon: Option<IconName>,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
    hint: Option<SharedString>,
    width: Pixels,
    busy: bool,
    compact_body: bool,
    focus: Vec<DialogFocus>,
    autofocus: Option<FocusHandle>,
    default_button: Option<FocusHandle>,
    default_danger: bool,
    on_default: Option<DialogAction>,
}

impl Dialog {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            icon: None,
            body: None,
            footer: None,
            hint: None,
            width: px(520.),
            busy: false,
            compact_body: false,
            focus: Vec::new(),
            autofocus: None,
            default_button: None,
            default_danger: false,
            on_default: None,
        }
    }

    /// A 16px header icon, drawn in `muted`.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The right-aligned footer content.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// The left footer hint, in `fs-sm` `muted`.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Disables Esc, the scrim, Enter and the close button.
    ///
    /// The builder runs on every frame, so `.busy(state.running)` takes effect
    /// as soon as `state.running` changes; [`set_busy`] remains available for
    /// callers that do not rebuild the dialog from state.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Body gap 14 instead of 16.
    pub fn compact_body(mut self) -> Self {
        self.compact_body = true;
        self
    }

    /// The dialog's focus stops, in Tab order.
    ///
    /// Tab and Shift-Tab cycle **only** through these handles. Any other
    /// focusable content placed in the body or footer (`Select`, `Tree`, …)
    /// is not reachable with the keyboard unless it is listed here; add its
    /// focus handles, with [`DialogFocus::control`] for controls and
    /// [`DialogFocus::input`] for text inputs.
    pub fn focus_order(mut self, focus: impl IntoIterator<Item = DialogFocus>) -> Self {
        self.focus = focus.into_iter().collect();
        self
    }

    /// Focused when the dialog opens; otherwise the first input is used.
    pub fn autofocus(mut self, handle: FocusHandle) -> Self {
        self.autofocus = Some(handle);
        self
    }

    /// The caller's visual default and the initial-focus fallback.
    ///
    /// Enter does **not** activate this button by itself: set
    /// [`on_default`](Self::on_default) for Enter to do anything. Danger
    /// defaults are never activated.
    pub fn default_button(mut self, handle: FocusHandle) -> Self {
        self.default_button = Some(handle);
        self
    }

    pub fn default_danger(mut self, danger: bool) -> Self {
        self.default_danger = danger;
        self
    }

    /// What Enter runs when it reaches the dialog rather than a control.
    ///
    /// This is the only thing Enter runs: without it, Enter does nothing in
    /// the dialog, even when [`default_button`](Self::default_button) is set.
    /// Enter inside a [`TextInput`](crate::TextInput) is consumed by the
    /// dialog: `on_default` runs and no `TextInputEvent::Submit` reaches other
    /// subscribers.
    pub fn on_default(mut self, listener: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_default = Some(Rc::new(listener));
        self
    }

    fn initial_focus(&self) -> Option<FocusHandle> {
        self.autofocus
            .clone()
            .or_else(|| {
                self.focus
                    .iter()
                    .find(|focus| focus.kind == DialogFocusKind::Input)
                    .map(|focus| focus.handle.clone())
            })
            .or_else(|| self.default_button.clone())
            .or_else(|| self.focus.first().map(|focus| focus.handle.clone()))
    }
}

/// The persistent host of a dialog: the layer's view.
pub struct DialogHost {
    build: DialogBuilder,
    focus: FocusHandle,
    layer: Option<LayerId>,
    /// Imperative override set by [`set_busy`]; when `None` the builder's
    /// `Dialog::busy` is authoritative on every render.
    busy_override: Option<bool>,
    /// Frames rendered so far; the focus check needs the dispatch tree of an
    /// earlier frame that already contains the dialog.
    frames: u32,
}

/// Opens a dialog and returns its layer.
pub fn open(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(&mut Window, &mut App) -> Dialog + 'static,
) -> LayerId {
    open_with_view(window, cx, build).0
}

/// Opens a dialog and also returns its host, so callers can change `busy`.
pub fn open_with_view(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(&mut Window, &mut App) -> Dialog + 'static,
) -> (LayerId, Entity<DialogHost>) {
    let dialog = build(window, cx);
    let busy = dialog.busy;
    let initial = dialog.initial_focus();
    let build = Rc::new(build);
    let host_build = build.clone();
    let host = cx.new(move |cx| DialogHost {
        build: host_build,
        focus: cx.focus_handle().tab_stop(false),
        layer: None,
        busy_override: None,
        frames: 0,
    });
    let layer = overlay::open_modal(host.clone().into(), window, cx);
    host.update(cx, |host, _| host.layer = Some(layer));
    overlay::set_dismiss_on_outside_click(layer, !busy, window, cx);
    let target = initial.unwrap_or_else(|| host.read(cx).focus.clone());
    window.focus(&target, cx);
    (layer, host)
}

/// Closes the dialog layer `id`.
pub fn close(id: LayerId, window: &mut Window, cx: &mut App) {
    overlay::close(id, window, cx);
}

/// Switches a dialog in and out of the busy state.
pub fn set_busy(id: LayerId, busy: bool, window: &mut Window, cx: &mut App) {
    let Some(view) = overlay::layer_view(id, window, cx) else {
        return;
    };
    let Ok(host) = view.downcast::<DialogHost>() else {
        return;
    };
    host.update(cx, |host, _| host.busy_override = Some(busy));
    overlay::set_dismiss_on_outside_click(id, !busy, window, cx);
    window.refresh();
}

impl DialogHost {
    /// Makes sure the dialog owns keyboard focus.
    ///
    /// The initial focus target may not be focusable: a disabled button does
    /// not track its handle, so focusing it leaves the focus on nothing and
    /// Esc, Tab and Enter never reach the dialog. A control that disables
    /// itself while it has focus does the same. Whenever the dialog is the top
    /// layer and focus is not inside it, focus moves to the dialog container;
    /// Tab then enters the focus order at its first stop.
    fn keep_focus_inside(&mut self, layer: Option<LayerId>, window: &mut Window, cx: &mut App) {
        self.frames = self.frames.saturating_add(1);
        if self.frames == 1 {
            // Check on the next frame, once the dialog is in the dispatch tree.
            window.refresh();
            return;
        }
        let on_top = layer.is_some_and(|layer| overlay::is_top_layer(layer, window, cx));
        // A menu closed by Enter parks focus on the overlay root until the
        // key-up hands it back to its opener; pulling it into the dialog now
        // would lose that restore.
        if on_top
            && !overlay::restore_pending(window, cx)
            && !self.focus.contains_focused(window, cx)
        {
            window.focus(&self.focus, cx);
        }
    }
}

impl Render for DialogHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog = (self.build)(window, cx);
        let colors = cx.theme().colors;
        // The builder is the source of truth: stage 2 writes `.busy(running)`
        // and the state changes without ever calling `set_busy`. An explicit
        // `set_busy` overrides the builder until called again.
        let busy = self.busy_override.unwrap_or(dialog.busy);
        let layer = self.layer;
        if let Some(layer) = layer {
            overlay::set_dismiss_on_outside_click(layer, !busy, window, cx);
        }
        self.keep_focus_inside(layer, window, cx);
        let handles: Vec<FocusHandle> = dialog
            .focus
            .iter()
            .map(|focus| focus.handle.clone())
            .collect();
        let controls: Vec<FocusHandle> = dialog
            .focus
            .iter()
            .filter(|focus| focus.kind == DialogFocusKind::Control)
            .map(|focus| focus.handle.clone())
            .collect();
        let has_hint = dialog.hint.is_some();

        let run_default: DialogAction = {
            let on_default = dialog.on_default.clone();
            let danger = dialog.default_danger;
            let controls = controls.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if busy || danger {
                    return;
                }
                if let Some(focused) = window.focused(cx) {
                    if controls.iter().any(|handle| handle == &focused) {
                        return;
                    }
                }
                if let Some(on_default) = &on_default {
                    on_default(window, cx);
                }
            })
        };

        let mut close_button =
            Button::ghost(ElementId::Name(format!("dialog-close-{layer:?}").into()))
                .icon_only(IconName::X)
                .size(ButtonSize::Xs)
                .tooltip(crate::i18n::t("kit.dialog.close"))
                .disabled(busy);
        if !busy {
            close_button = close_button.on_click(move |_, window, cx| {
                if let Some(layer) = layer {
                    close(layer, window, cx);
                }
            });
        }

        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(colors.border)
            .when_some(dialog.icon, |this, icon| {
                this.child(Icon::new(icon).size(px(16.)).color(colors.muted))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(FS_MD)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(dialog.title.clone()),
            )
            .child(close_button);

        let mut body = div()
            .id(ElementId::Name(format!("dialog-body-{layer:?}").into()))
            .flex()
            .flex_col()
            .px(px(20.))
            .py(px(20.))
            .gap(if dialog.compact_body {
                px(14.)
            } else {
                px(16.)
            })
            .max_h(px(560.))
            // The body is the dialog's scroll container: it keeps its capped
            // height instead of being compressed by the head and footer.
            .flex_none()
            .overflow_y_scroll();
        if let Some(content) = dialog.body {
            // The content is one flex item of the scrolling column: a fixed
            // height here would otherwise shrink every field in it.
            body = body.child(div().flex_none().child(content));
        }

        let mut footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(16.))
            .border_t_1()
            .border_color(colors.border)
            .when(!has_hint, |this| this.justify_end());
        if let Some(hint) = dialog.hint.clone() {
            footer = footer.child(
                div()
                    .mr_auto()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(hint),
            );
        }
        if let Some(content) = dialog.footer {
            footer = footer.child(content);
        }

        let default_for_confirm = run_default.clone();
        let default_for_submit = run_default;
        let next_handles = handles.clone();
        let prev_handles = handles;

        div()
            .w(dialog.width)
            .flex()
            .flex_col()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_LG)
            .shadow(cx.theme().pop_shadow())
            .overflow_hidden()
            .font_family(FONT_UI)
            .text_color(colors.text)
            .track_focus(&self.focus)
            .key_context("ui-dialog")
            .on_action(move |_: &Confirm, window: &mut Window, cx: &mut App| {
                // Enter closes on key-down; defer the focus restore so the
                // paired key-up cannot activate the restored opener.
                overlay::defer_focus_restore(window, cx);
                default_for_confirm(window, cx);
                overlay::clear_deferred_focus_restore(window, cx);
            })
            .capture_action(
                move |_: &TextInputSubmit, window: &mut Window, cx: &mut App| {
                    overlay::defer_focus_restore(window, cx);
                    default_for_submit(window, cx);
                    overlay::clear_deferred_focus_restore(window, cx);
                    // The dialog owns Enter inside its inputs: do not also
                    // emit `TextInputEvent::Submit` to other subscribers.
                    cx.stop_propagation();
                },
            )
            .on_action(move |_: &FocusNext, window: &mut Window, cx: &mut App| {
                cycle_focus(&next_handles, true, window, cx);
            })
            .on_action(move |_: &FocusPrev, window: &mut Window, cx: &mut App| {
                cycle_focus(&prev_handles, false, window, cx);
            })
            .on_action(move |_: &Cancel, window: &mut Window, cx: &mut App| {
                if busy {
                    return;
                }
                if let Some(layer) = layer {
                    close(layer, window, cx);
                }
            })
            .child(head)
            .child(body)
            .child(footer)
    }
}

fn cycle_focus(handles: &[FocusHandle], forward: bool, window: &mut Window, cx: &mut App) {
    let current = window
        .focused(cx)
        .and_then(|focused| handles.iter().position(|handle| handle == &focused));
    if let Some(index) = next_focus_index(handles.len(), current, forward) {
        window.focus(&handles[index], cx);
    }
}

/// The next index, wrapping; `None` when there are no stops.
fn next_focus_index(len: usize, current: Option<usize>, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, forward) {
        (Some(index), true) => (index + 1) % len,
        (Some(index), false) => (index + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_wraps_forward_and_backward() {
        assert_eq!(next_focus_index(3, Some(0), true), Some(1));
        assert_eq!(next_focus_index(3, Some(2), true), Some(0));
        assert_eq!(next_focus_index(3, Some(0), false), Some(2));
        assert_eq!(next_focus_index(3, None, true), Some(0));
        assert_eq!(next_focus_index(3, None, false), Some(2));
        assert_eq!(next_focus_index(0, None, true), None);
    }

    #[test]
    fn out_of_range_current_index_wraps() {
        assert_eq!(next_focus_index(3, Some(5), true), Some(0));
        assert_eq!(next_focus_index(3, Some(5), false), Some(1));
    }
}
