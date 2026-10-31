//! A small destructive-action confirmation, used by the remote branch delete.

use std::rc::Rc;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, ParentElement, SharedString, Styled,
    Window,
};

use ui::theme::ActiveTheme;
use ui::{dialog, overlay, Button, Dialog, DialogFocus, DialogHost, IconName};

type ConfirmAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct ConfirmDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    title: SharedString,
    body: SharedString,
    primary: SharedString,
    action: ConfirmAction,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

impl ConfirmDialog {
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
        let action = self.action.clone();
        action(window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }
}

/// Opens a modal with a danger-styled primary button. `action` runs after the
/// dialog closes.
pub(crate) fn open(
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    primary: impl Into<SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ConfirmDialog> {
    let dialog = cx.new(|cx| ConfirmDialog {
        layer: None,
        host: None,
        title: title.into(),
        body: body.into(),
        primary: primary.into(),
        action: Rc::new(action),
        cancel_focus: cx.focus_handle().tab_stop(true),
        primary_focus: cx.focus_handle().tab_stop(true),
    });
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Confirm"),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
    });
    dialog
}

fn build(state: &Entity<ConfirmDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.cancel(window, cx));
        }
    };
    let confirm = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.confirm(window, cx));
        }
    };

    let body = div().text_color(colors.text).child(dialog.body.clone());

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("confirm-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::danger("confirm-primary")
                .label(dialog.primary.clone())
                .min_width(px(96.))
                .focus_handle(dialog.primary_focus.clone())
                .on_click(confirm),
        );

    Dialog::new(dialog.title.clone())
        .icon(IconName::Warn)
        .width(px(440.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.cancel_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .default_danger(true)
}
