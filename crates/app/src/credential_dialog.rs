//! The password / user-name prompt for HTTPS git operations.
//!
//! Git's askpass helper runs as a child process and blocks until the answer
//! arrives; this dialog is the UI end of that exchange.

use std::sync::mpsc::Sender;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, ParentElement, SharedString, Styled,
    Subscription, Window,
};

use ui::metrics::FS_SM;
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, Dialog, DialogFocus, DialogHost, TextInput,
    TextInputEvent,
};

use crate::network_service::CredentialPrompt;

pub(crate) struct CredentialDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    prompt: SharedString,
    label: &'static str,
    input: Entity<TextInput>,
    _input_events: Subscription,
    reply: Sender<Option<String>>,
    answered: bool,
    input_focus: gpui::FocusHandle,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

impl CredentialDialog {
    fn new(prompt: CredentialPrompt, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let secret = prompt.secret();
        let label = prompt.label();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(if secret { "Password" } else { "User name" }, cx);
            input.set_masked(secret, cx);
            input
        });
        let input_focus = input.read(cx).focus_handle().clone();
        let input_events = cx.subscribe(&input, |_, _, _: &TextInputEvent, cx| cx.notify());
        Self {
            layer: None,
            host: None,
            prompt: prompt.prompt.into(),
            label,
            input,
            _input_events: input_events,
            reply: prompt.reply,
            answered: false,
            input_focus,
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.answered {
            return;
        }
        let text = self.input.read(cx).text().to_owned();
        if text.is_empty() {
            return;
        }
        self.answered = true;
        let _ = self.reply.send(Some(text));
        self.close(window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.answered {
            self.answered = true;
            let _ = self.reply.send(None);
        }
        self.close(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    /// The layer can be dismissed with Esc or the scrim without `cancel`;
    /// then the waiting git child must still learn that no answer is coming.
    pub(crate) fn layer_gone(&mut self, window: &Window, cx: &App) -> bool {
        let Some(layer) = self.layer else {
            return true;
        };
        if overlay::layer_view(layer, window, cx).is_some() {
            return false;
        }
        if !self.answered {
            self.answered = true;
            let _ = self.reply.send(None);
        }
        true
    }
}

pub(crate) fn open(
    prompt: CredentialPrompt,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CredentialDialog> {
    let dialog = cx.new(|cx| CredentialDialog::new(prompt, window, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Authentication required"),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
    });
    dialog
}

fn build(state: &Entity<CredentialDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.cancel(window, cx));
        }
    };
    let submit = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.submit(window, cx));
        }
    };
    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.submit(window, cx));
        }
    };
    let can_submit = !dialog.input.read(cx).text().is_empty();

    let body = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            Banner::new(BannerKind::Info, "The Git server asked for credentials")
                .body(dialog.prompt.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(dialog.label),
                )
                .child(dialog.input.clone()),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child("The answer is passed to git and is not saved by GitWarp."),
        );

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("credential-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("credential-submit")
                .label("Continue")
                .min_width(px(72.))
                .disabled(!can_submit)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(submit),
        );

    Dialog::new("Authentication required")
        .icon(ui::IconName::Key)
        .width(px(460.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::input(dialog.input_focus.clone()),
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.input_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default)
}
