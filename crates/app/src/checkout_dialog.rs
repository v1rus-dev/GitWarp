//! Asks what to do with uncommitted changes before a checkout.
//!
//! A clean work tree never opens this. A dirty one offers three outcomes:
//! stash the changes and switch, carry them along (git refuses if they would
//! be overwritten), or discard them and switch.

use std::rc::Rc;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, ParentElement, SharedString, Styled,
    Window,
};

use ui::theme::ActiveTheme;
use ui::{dialog, overlay, Button, Dialog, DialogFocus, IconName};

type Choice = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct CheckoutDialog {
    layer: Option<overlay::LayerId>,
    title: SharedString,
    body: SharedString,
    stash: Choice,
    keep: Choice,
    discard: Choice,
    stash_focus: gpui::FocusHandle,
    keep_focus: gpui::FocusHandle,
    discard_focus: gpui::FocusHandle,
    cancel_focus: gpui::FocusHandle,
}

impl CheckoutDialog {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn choose(&mut self, choice: &Choice, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
        choice(window, cx);
    }
}

/// Opens the dialog. Each choice runs after the dialog has closed.
pub(crate) fn open(
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    stash: impl Fn(&mut Window, &mut App) + 'static,
    keep: impl Fn(&mut Window, &mut App) + 'static,
    discard: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CheckoutDialog> {
    let dialog = cx.new(|cx| CheckoutDialog {
        layer: None,
        title: title.into(),
        body: body.into(),
        stash: Rc::new(stash),
        keep: Rc::new(keep),
        discard: Rc::new(discard),
        stash_focus: cx.focus_handle().tab_stop(true),
        keep_focus: cx.focus_handle().tab_stop(true),
        discard_focus: cx.focus_handle().tab_stop(true),
        cancel_focus: cx.focus_handle().tab_stop(true),
    });
    let state = dialog.downgrade();
    let (layer, _host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Switch branch"),
    });
    dialog.update(cx, |dialog, _| dialog.layer = Some(layer));
    dialog
}

fn build(state: &Entity<CheckoutDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let button = |id: &'static str, label: &'static str, focus, choice: Choice, danger: bool| {
        let state = state.clone();
        let click = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.choose(&choice, window, cx));
        };
        let button = if danger {
            Button::danger(id)
        } else {
            Button::new(id)
        };
        button.label(label).focus_handle(focus).on_click(click)
    };

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .text_color(colors.text)
        .child(dialog.body.clone())
        .child(
            div()
                .text_size(ui::metrics::FS_SM)
                .text_color(colors.muted)
                .child("Carrying the changes fails when the other branch edits the same lines."),
        );

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("checkout-cancel")
                .label("Cancel")
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(button(
            "checkout-discard",
            "Discard",
            dialog.discard_focus.clone(),
            dialog.discard.clone(),
            true,
        ))
        .child(button(
            "checkout-keep",
            "Bring changes",
            dialog.keep_focus.clone(),
            dialog.keep.clone(),
            false,
        ))
        .child(button(
            "checkout-stash",
            "Stash and switch",
            dialog.stash_focus.clone(),
            dialog.stash.clone(),
            false,
        ));

    Dialog::new(dialog.title.clone())
        .icon(IconName::Warn)
        .width(px(520.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.discard_focus.clone()),
            DialogFocus::control(dialog.keep_focus.clone()),
            DialogFocus::control(dialog.stash_focus.clone()),
        ])
        .autofocus(dialog.stash_focus.clone())
        .default_button(dialog.stash_focus.clone())
        .on_default({
            let state = state.clone();
            move |window, cx| {
                let choice = state.read(cx).stash.clone();
                state.update(cx, |dialog, cx| dialog.choose(&choice, window, cx));
            }
        })
}
