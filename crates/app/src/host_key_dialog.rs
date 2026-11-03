//! The SSH host-key prompt shown before the first connection to a host (and
//! when a known host presents a different key).

use std::sync::mpsc::Sender;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, ParentElement, SharedString, Styled,
    Window,
};

use git_core::ssh::{HostKeyStatus, SshTarget};
use ui::metrics::{FONT_MONO, FS_SM, R_MD};
use ui::theme::ActiveTheme;
use ui::{dialog, overlay, Banner, BannerKind, Button, Dialog, DialogFocus, DialogHost};

use crate::network_service::HostKeyRequest;

pub(crate) struct HostKeyDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    address: SharedString,
    changed: bool,
    old: Vec<SharedString>,
    new: Vec<SharedString>,
    reply: Sender<bool>,
    answered: bool,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

impl HostKeyDialog {
    fn new(request: HostKeyRequest, cx: &mut Context<Self>) -> Self {
        let HostKeyRequest {
            target,
            status,
            reply,
        } = request;
        let (changed, old, new) = match status {
            HostKeyStatus::Changed { old, new } => (
                true,
                old.iter()
                    .map(|key| {
                        if key.kind.is_empty() {
                            SharedString::from(key.fingerprint.clone())
                        } else {
                            SharedString::from(format!("{} ({})", key.fingerprint, key.kind))
                        }
                    })
                    .collect(),
                new.iter()
                    .map(|key| {
                        if key.kind.is_empty() {
                            SharedString::from(key.fingerprint.clone())
                        } else {
                            SharedString::from(format!("{} ({})", key.fingerprint, key.kind))
                        }
                    })
                    .collect(),
            ),
            HostKeyStatus::Unknown { keys } => (
                false,
                Vec::new(),
                keys.into_iter()
                    .map(|key| {
                        if key.kind.is_empty() {
                            SharedString::from(key.fingerprint)
                        } else {
                            SharedString::from(format!("{} ({})", key.fingerprint, key.kind))
                        }
                    })
                    .collect(),
            ),
            _ => (false, Vec::new(), Vec::new()),
        };
        Self {
            layer: None,
            host: None,
            address: format_host(&target).into(),
            changed,
            old,
            new,
            reply,
            answered: false,
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn answer(&mut self, trust: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.answered {
            return;
        }
        self.answered = true;
        let _ = self.reply.send(trust);
        self.close(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    pub(crate) fn layer_gone(&mut self, window: &Window, cx: &App) -> bool {
        let Some(layer) = self.layer else {
            return true;
        };
        if overlay::layer_view(layer, window, cx).is_some() {
            return false;
        }
        if !self.answered {
            self.answered = true;
            let _ = self.reply.send(false);
        }
        true
    }
}

fn format_host(target: &SshTarget) -> String {
    if target.port == 22 {
        target.host.clone()
    } else {
        format!("{}:{}", target.host, target.port)
    }
}

pub(crate) fn open(
    request: HostKeyRequest,
    window: &mut Window,
    cx: &mut App,
) -> Entity<HostKeyDialog> {
    let dialog = cx.new(|cx| HostKeyDialog::new(request, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new(ui::t!("dialogs.host_key.title")),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
    });
    dialog
}

fn build(state: &Entity<HostKeyDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.answer(false, window, cx));
        }
    };
    let trust = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.answer(true, window, cx));
        }
    };

    let fingerprints = |keys: &[SharedString], colors: ui::theme::ThemeColors| {
        let mut list = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(10.))
            .rounded(R_MD)
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface);
        for key in keys {
            list = list.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.text)
                    .child(key.clone()),
            );
        }
        list
    };

    let mut body = div().flex().flex_col().gap(px(12.));
    if dialog.changed {
        body = body.child(
            Banner::new(BannerKind::Error, ui::t!("dialogs.host_key.mitm_title"))
                .body(ui::t!("dialogs.host_key.mitm_body", host = dialog.address)),
        );
        body = body
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("dialogs.host_key.previously_trusted")),
            )
            .child(fingerprints(&dialog.old, colors))
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("dialogs.host_key.presented_now")),
            )
            .child(fingerprints(&dialog.new, colors));
    } else {
        body = body
            .child(
                Banner::new(BannerKind::Info, ui::t!("dialogs.host_key.trust_title"))
                    .body(ui::t!("dialogs.host_key.trust_body", host = dialog.address)),
            )
            .child(fingerprints(&dialog.new, colors))
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("dialogs.host_key.compare_hint")),
            );
    }
    body = body.child(
        div()
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(ui::t!("dialogs.host_key.remembered")),
    );

    let primary_label = if dialog.changed {
        ui::t!("dialogs.host_key.replace_key")
    } else {
        ui::t!("dialogs.host_key.trust_host")
    };
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("host-key-cancel")
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(if dialog.changed {
            Button::danger("host-key-trust")
                .label(primary_label)
                .min_width(px(96.))
                .focus_handle(dialog.primary_focus.clone())
                .on_click(trust)
        } else {
            Button::primary("host-key-trust")
                .label(primary_label)
                .min_width(px(96.))
                .focus_handle(dialog.primary_focus.clone())
                .on_click(trust)
        });

    let title = if dialog.changed {
        ui::t!("dialogs.host_key.changed_title")
    } else {
        ui::t!("dialogs.host_key.title")
    };
    Dialog::new(title)
        .icon(if dialog.changed {
            ui::IconName::Warn
        } else {
            ui::IconName::Lock
        })
        .width(px(500.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.primary_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .default_danger(dialog.changed)
        .on_default({
            let state = state.clone();
            move |window: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| dialog.answer(true, window, cx));
            }
        })
}
