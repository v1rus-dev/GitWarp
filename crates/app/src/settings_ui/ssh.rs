//! Settings → SSH keys: the private keys in `~/.ssh`, the agent and the
//! trusted hosts, with generate / import / delete dialogs.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window,
};

use git_core::ssh::{self, KeyType, SshKey, TrustedHost};
use ui::metrics::{FONT_MONO, FS_SM, R_SM};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Checkbox, Chip, ChipKind, Dialog,
    DialogFocus, DialogHost, Icon, IconName, Select, TextInput, TextInputEvent, ToastKind,
};

use super::{heading, SettingsView, TestState};
use crate::format::{absolute_time, home_dir};

impl SettingsView {
    pub(super) fn reload_ssh(&mut self, cx: &mut Context<Self>) {
        self.ssh_generation = self.ssh_generation.wrapping_add(1);
        let generation = self.ssh_generation;
        self.ssh_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let keys = ssh::list_keys();
                    let agent = ssh::agent_running();
                    let hosts = ssh::trusted_hosts();
                    (keys, agent, hosts)
                })
                .await;
            this.update(cx, |view, cx| {
                if generation != view.ssh_generation {
                    return;
                }
                view.ssh_loading = false;
                match result.0 {
                    Ok(keys) => {
                        view.ssh_keys = keys;
                        view.ssh_error = None;
                    }
                    Err(error) => view.ssh_error = Some(error.to_string()),
                }
                view.ssh_agent = Some(result.1);
                match result.2 {
                    Ok(hosts) => view.ssh_hosts = hosts,
                    Err(error) => view.ssh_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn copy_key(&mut self, key: &SshKey, window: &mut Window, cx: &mut Context<Self>) {
        if key.public_key.is_empty() {
            ui::toast::show(
                ToastKind::Warn,
                ui::t!("settings.ssh.no_public"),
                window,
                cx,
            );
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(key.public_key.clone()));
        ui::toast::show(
            ToastKind::Ok,
            ui::t!("settings.ssh.copied", name = key.name),
            window,
            cx,
        );
    }

    fn toggle_agent(&mut self, key: &SshKey, cx: &mut Context<Self>) {
        let path = key.path.clone();
        let add = !key.agent;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if add {
                        ssh::add_to_agent(&path)
                    } else {
                        ssh::remove_from_agent(&path)
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.ssh_error = Some(error.to_string());
                }
                view.reload_ssh(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn test_key(&mut self, key: &SshKey, cx: &mut Context<Self>) {
        let host = self
            .ssh_hosts
            .iter()
            .find(|host| host.host.contains("github") || host.host.contains("gitlab"))
            .or_else(|| self.ssh_hosts.first())
            .map(|host| host.host.clone());
        let Some(host) = host else {
            return;
        };
        let slot = format!("{}|{}", key.name, host);
        let key_path = key.path.clone();
        self.ssh_tests
            .insert(slot.clone(), TestState::Running(host.clone()));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let host = host.clone();
                    let key_path = key_path.clone();
                    async move { ssh::test_connection(&host, &key_path, 8) }
                })
                .await;
            this.update(cx, |view, cx| {
                let state = match result {
                    Ok(message) => TestState::Ok(host.clone(), message),
                    Err(error) => TestState::Failed(host.clone(), error.to_string()),
                };
                view.ssh_tests.insert(slot.clone(), state);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn forget_host(&mut self, host: &str, cx: &mut Context<Self>) {
        let host = host.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let host = host.clone();
                    async move { ssh::forget_host(&host) }
                })
                .await;
            this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.ssh_error = Some(error.to_string());
                }
                view.reload_ssh(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_ssh_dialog(
        &mut self,
        kind: SshDialogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        self.ssh_dialog = Some(open_dialog(kind, view, window, cx));
    }
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let mut page = div().flex().flex_col();
    page = page.child(heading(
        ui::t!("settings.page.ssh"),
        ui::t!("settings.ssh.subtitle"),
    ));

    let generate = {
        let entity = cx.entity();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| {
                view.open_ssh_dialog(SshDialogKind::Generate, window, cx)
            });
        }
    };
    let import = {
        let entity = cx.entity();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| {
                view.open_ssh_dialog(SshDialogKind::Import, window, cx)
            });
        }
    };
    page = page.child(
        div()
            .w_full()
            .max_w(px(760.))
            .mb(px(16.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::primary("settings-ssh-generate")
                    .label(ui::t!("settings.ssh.generate"))
                    .icon(IconName::Plus)
                    .on_click(generate),
            )
            .child(
                Button::new("settings-ssh-import")
                    .label(ui::t!("settings.ssh.import"))
                    .icon(IconName::Folder)
                    .on_click(import),
            )
            .child(div().flex_1())
            .child(
                Button::new("settings-ssh-refresh")
                    .icon(IconName::Fetch)
                    .label(ui::t!("settings.ssh.refresh"))
                    .disabled(view.ssh_loading)
                    .on_click({
                        let entity = cx.entity();
                        move |_, _, cx: &mut App| {
                            entity.update(cx, |view, cx| view.reload_ssh(cx));
                        }
                    }),
            ),
    );

    let loaded = view.ssh_keys.iter().filter(|key| key.agent).count();
    match view.ssh_agent {
        Some(true) => {
            page = page.child(div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Info, ui::t!("settings.ssh.agent_running")).body(ui::t!(
                    "settings.ssh.agent_loaded",
                    loaded = loaded,
                    total = view.ssh_keys.len()
                )),
            ));
        }
        Some(false) => {
            page = page.child(
                div().max_w(px(760.)).mb(px(12.)).child(
                    Banner::new(BannerKind::Error, ui::t!("settings.ssh.agent_stopped"))
                        .body(ui::t!("settings.ssh.agent_stopped_body")),
                ),
            );
            page = page.child(
                div().max_w(px(760.)).mb(px(12.)).child(
                    Button::new("settings-ssh-agent-retry")
                        .label(ui::t!("settings.ssh.check_again"))
                        .icon(IconName::Fetch)
                        .on_click({
                            let entity = cx.entity();
                            move |_, _, cx: &mut App| {
                                entity.update(cx, |view, cx| view.reload_ssh(cx));
                            }
                        }),
                ),
            );
        }
        None => {}
    }
    if let Some(error) = &view.ssh_error {
        page = page.child(div().max_w(px(760.)).mb(px(12.)).child(
            Banner::new(BannerKind::Error, ui::t!("settings.ssh.read_failed")).body(error.clone()),
        ));
    }

    let mut keys = div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .border_1()
        .rounded(R_SM)
        .overflow_hidden();
    if view.ssh_keys.is_empty() {
        keys = keys.child(
            div()
                .p(px(24.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .text_center()
                .child(Icon::new(IconName::Key).size(px(24.)).color(colors.muted))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(ui::t!("settings.ssh.empty_title")),
                )
                .child(
                    div()
                        .max_w(px(420.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("settings.ssh.empty_body")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Button::primary("settings-ssh-empty-generate")
                                .label(ui::t!("settings.ssh.generate"))
                                .icon(IconName::Plus)
                                .on_click({
                                    let entity = cx.entity();
                                    move |_, window, cx: &mut App| {
                                        entity.update(cx, |view, cx| {
                                            view.open_ssh_dialog(
                                                SshDialogKind::Generate,
                                                window,
                                                cx,
                                            )
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("settings-ssh-empty-import")
                                .label(ui::t!("settings.ssh.import_existing"))
                                .icon(IconName::Folder)
                                .on_click({
                                    let entity = cx.entity();
                                    move |_, window, cx: &mut App| {
                                        entity.update(cx, |view, cx| {
                                            view.open_ssh_dialog(SshDialogKind::Import, window, cx)
                                        });
                                    }
                                }),
                        ),
                ),
        );
    } else {
        for key in &view.ssh_keys {
            let card = key_card(view, key, cx);
            keys = keys.child(card);
        }
    }
    page = page.child(section(ui::t!("settings.ssh.keys_heading"), keys));

    let mut hosts = div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .border_1()
        .rounded(R_SM)
        .overflow_hidden();
    if view.ssh_hosts.is_empty() {
        hosts = hosts.child(
            div()
                .p(px(24.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .text_center()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(ui::t!("settings.ssh.no_hosts")),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("settings.ssh.no_hosts_body")),
                ),
        );
    } else {
        for (index, host) in view.ssh_hosts.iter().enumerate() {
            hosts = hosts.child(host_row(view, index, host, cx));
        }
    }
    page = page.child(section(ui::t!("settings.ssh.hosts_heading"), hosts));
    page = page.child(
        div()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Icon::new(IconName::Info).size(px(12.)).color(colors.muted))
            .child(ui::t!("settings.ssh.known_hosts")),
    );
    page.into_any_element()
}

fn section(label: gpui::SharedString, body: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .mb(px(20.))
        .max_w(px(760.))
        .child(
            div()
                .text_size(ui::metrics::FS_XS)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(label),
        )
        .child(body)
        .into_any_element()
}

fn key_card(view: &SettingsView, key: &SshKey, cx: &Context<SettingsView>) -> AnyElement {
    let colors = cx.theme().colors;
    let test_host = view
        .ssh_hosts
        .iter()
        .find(|host| host.host.contains("github") || host.host.contains("gitlab"))
        .or_else(|| view.ssh_hosts.first())
        .map(|host| host.host.clone());
    let test = test_host
        .as_ref()
        .and_then(|host| view.ssh_tests.get(&format!("{}|{}", key.name, host)));

    let copy = {
        let entity = cx.entity();
        let key = key.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.copy_key(&key, window, cx));
        }
    };
    let test_button = {
        let entity = cx.entity();
        let key = key.clone();
        move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.test_key(&key, cx));
        }
    };
    let agent = {
        let entity = cx.entity();
        let key = key.clone();
        move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.toggle_agent(&key, cx));
        }
    };
    let delete = {
        let entity = cx.entity();
        let key = key.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| {
                view.open_ssh_dialog(SshDialogKind::Delete(key.clone()), window, cx)
            });
        }
    };
    let created = if key.created == 0 {
        String::new()
    } else {
        absolute_time(key.created as i64)
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_owned()
    };

    let mut chips = div().flex().items_center().gap(px(4.));
    chips = chips.child(Chip::new("ssh-type", key.kind.clone()).icon(IconName::Key));
    if key.agent {
        chips = chips.child(
            Chip::new("ssh-agent", ui::t!("settings.ssh.chip_agent"))
                .kind(ChipKind::Ok)
                .icon(IconName::Check),
        );
    }
    chips = chips.child(if key.encrypted {
        Chip::new("ssh-pass", ui::t!("settings.ssh.chip_passphrase")).icon(IconName::Lock)
    } else {
        Chip::new("ssh-nopass", ui::t!("settings.ssh.chip_no_passphrase"))
            .kind(ChipKind::Warn)
            .icon(IconName::Warn)
    });
    if key.kind == "rsa-2048" {
        chips = chips.child(
            Chip::new("ssh-weak", ui::t!("settings.ssh.chip_weak"))
                .kind(ChipKind::Warn)
                .icon(IconName::Warn),
        );
    }

    let actions = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            Button::new(format!("ssh-copy-{}", key.name))
                .size(ButtonSize::Small)
                .icon(IconName::Copy)
                .label(ui::t!("settings.ssh.copy_public"))
                .on_click(copy),
        )
        .child(
            Button::new(format!("ssh-test-{}", key.name))
                .size(ButtonSize::Small)
                .icon(IconName::Fetch)
                .label(ui::t!("settings.ssh.test_connection"))
                .disabled(test_host.is_none() || matches!(test, Some(TestState::Running(_))))
                .on_click(test_button),
        )
        .child(
            Button::new(format!("ssh-agent-{}", key.name))
                .size(ButtonSize::Small)
                .label(if key.agent {
                    ui::t!("settings.ssh.agent_remove")
                } else {
                    ui::t!("settings.ssh.agent_add")
                })
                .disabled(view.ssh_agent != Some(true))
                .on_click(agent),
        )
        .child(
            Button::danger(format!("ssh-delete-{}", key.name))
                .size(ButtonSize::Small)
                .icon(IconName::Trash)
                .label(ui::t!("settings.ssh.delete"))
                .on_click(delete),
        );

    let card = div()
        .w_full()
        .p(px(12.))
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .rounded(px(14.))
                .bg(colors.active)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Key).size(px(16.)).color(colors.muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .flex_wrap()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(key.name.clone()),
                        )
                        .child(chips),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .truncate()
                        .child(if key.fingerprint.is_empty() {
                            ui::t!("settings.ssh.fingerprint_unavailable").to_string()
                        } else {
                            key.fingerprint.clone()
                        }),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!(
                            "settings.ssh.created_tested",
                            created = if created.is_empty() {
                                ui::t!("settings.ssh.created_unknown").to_string()
                            } else {
                                ui::t!("settings.ssh.created", date = created).to_string()
                            },
                            tested = match test {
                                Some(_) => test_host
                                    .as_ref()
                                    .map(|host| ui::t!("settings.ssh.tested_against", host = host)
                                        .to_string())
                                    .unwrap_or_else(
                                        || ui::t!("settings.ssh.not_tested").to_string()
                                    ),
                                None => ui::t!("settings.ssh.not_tested").to_string(),
                            }
                        )),
                )
                .when_some(test, |this, test| this.child(test_line(test, colors)))
                .child(actions),
        );
    card.into_any_element()
}

fn test_line(test: &TestState, colors: ui::ThemeColors) -> AnyElement {
    match test {
        TestState::Running(host) => div()
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(ui::t!("settings.ssh.connecting", host = host))
            .into_any_element(),
        TestState::Ok(host, message) => div()
            .flex()
            .items_start()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.success)
            .child(
                Icon::new(IconName::Check)
                    .size(px(12.))
                    .color(colors.success),
            )
            .child(ui::t!(
                "settings.ssh.test_result",
                host = host,
                message = message
            ))
            .into_any_element(),
        TestState::Failed(host, message) => div()
            .flex()
            .items_start()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.error)
            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
            .child(ui::t!(
                "settings.ssh.test_result",
                host = host,
                message = message
            ))
            .into_any_element(),
    }
}

fn host_row(
    _view: &SettingsView,
    index: usize,
    host: &TrustedHost,
    cx: &Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let forget = {
        let entity = cx.entity();
        let host = host.host.clone();
        move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.forget_host(&host, cx));
        }
    };
    div()
        .w_full()
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(host.host.clone()),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .truncate()
                        .child(format!(
                            "{} {}",
                            host.kind,
                            if host.fingerprint.is_empty() {
                                "—"
                            } else {
                                host.fingerprint.as_str()
                            }
                        )),
                ),
        )
        .child(
            Button::ghost(format!("settings-ssh-forget-{index}"))
                .icon_only(IconName::Trash)
                .size(ButtonSize::Small)
                .tooltip(ui::t!("settings.ssh.forget", host = host.host))
                .on_click(forget),
        )
        .into_any_element()
}

/* -------------------------------- dialogs -------------------------------- */

/// What the SSH dialog is for.
pub(crate) enum SshDialogKind {
    Generate,
    Import,
    Delete(SshKey),
}

pub(crate) struct SshDialog {
    kind: SshDialogKind,
    layer: Option<overlay::LayerId>,
    _host: Option<Entity<DialogHost>>,
    view: gpui::WeakEntity<SettingsView>,
    name: Entity<TextInput>,
    comment: Entity<TextInput>,
    passphrase: Entity<TextInput>,
    repeat: Entity<TextInput>,
    path: Entity<TextInput>,
    _events: Vec<Subscription>,
    key_type: usize,
    use_passphrase: bool,
    add_agent: bool,
    copy_import: bool,
    ack_delete: bool,
    error: Option<SharedString>,
    result: Option<SshKey>,
    busy: bool,
    name_focus: gpui::FocusHandle,
    comment_focus: gpui::FocusHandle,
    pass_focus: gpui::FocusHandle,
    repeat_focus: gpui::FocusHandle,
    path_focus: gpui::FocusHandle,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

pub(super) fn open_dialog(
    kind: SshDialogKind,
    view: gpui::WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SshDialog> {
    let dialog = cx.new(|cx| {
        let mut events = Vec::new();
        let name = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_text("id_ed25519_gitwarp", cx);
            input.set_font_family(ui::metrics::FONT_MONO, cx);
            input
        });
        let comment = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("settings.placeholder.email"), cx);
            input.set_text(std::env::var("GIT_AUTHOR_EMAIL").unwrap_or_default(), cx);
            input
        });
        let passphrase = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("settings.ssh.dialog.passphrase"), cx);
            input.set_masked(true, cx);
            input
        });
        let repeat = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("settings.ssh.dialog.repeat"), cx);
            input.set_masked(true, cx);
            input
        });
        let path = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("~/.ssh/id_ed25519_old", cx);
            input.set_font_family(ui::metrics::FONT_MONO, cx);
            input
        });
        for input in [&name, &comment, &passphrase, &repeat, &path] {
            events.push(cx.subscribe(input, |_, _, _: &TextInputEvent, cx| cx.notify()));
        }
        let name_focus = name.read(cx).focus_handle().clone();
        let comment_focus = comment.read(cx).focus_handle().clone();
        let pass_focus = passphrase.read(cx).focus_handle().clone();
        let repeat_focus = repeat.read(cx).focus_handle().clone();
        let path_focus = path.read(cx).focus_handle().clone();
        SshDialog {
            kind,
            layer: None,
            _host: None,
            view,
            name,
            comment,
            passphrase,
            repeat,
            path,
            _events: events,
            key_type: 0,
            use_passphrase: true,
            add_agent: true,
            copy_import: true,
            ack_delete: false,
            error: None,
            result: None,
            busy: false,
            name_focus,
            comment_focus,
            pass_focus,
            repeat_focus,
            path_focus,
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    });
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("SSH key"),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog._host = Some(host);
    });
    dialog
}

impl SshDialog {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_owned();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.starts_with('-')
            || name.contains('/')
            || name.contains('\\')
            || name.contains(' ')
        {
            self.error = Some(ui::t!("settings.ssh.dialog.bad_name"));
            cx.notify();
            return;
        }
        let pass = self.passphrase.read(cx).text().to_owned();
        let repeat = self.repeat.read(cx).text().to_owned();
        if self.use_passphrase && (pass.len() < 5 || pass != repeat) {
            self.error = Some(ui::t!("settings.ssh.dialog.bad_passphrase"));
            cx.notify();
            return;
        }
        let kind = match self.key_type {
            1 => KeyType::Rsa4096,
            2 => KeyType::Ecdsa,
            _ => KeyType::Ed25519,
        };
        let comment = self.comment.read(cx).text().to_owned();
        let add_agent = self.add_agent;
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh::generate_key(&name, kind, &comment, &pass, add_agent) })
                .await;
            this.update(cx, |dialog, cx| {
                dialog.busy = false;
                match result {
                    Ok(key) => {
                        let view = dialog.view.clone();
                        dialog.result = Some(key);
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| view.reload_ssh(cx));
                        }
                    }
                    Err(error) => dialog.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let _ = window;
        cx.notify();
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.path.read(cx).text().trim().to_owned();
        if text.is_empty() {
            self.error = Some(ui::t!("settings.ssh.dialog.no_file"));
            cx.notify();
            return;
        }
        let path = crate::format::expand_home(&text, home_dir().as_deref());
        let copy = self.copy_import;
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh::import_key(&path, copy) })
                .await;
            this.update(cx, |dialog, cx| {
                dialog.busy = false;
                match result {
                    Ok(key) => {
                        let view = dialog.view.clone();
                        dialog.result = Some(key);
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| view.reload_ssh(cx));
                        }
                    }
                    Err(error) => dialog.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let _ = window;
        cx.notify();
    }

    fn delete(&mut self, key: SshKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ack_delete {
            return;
        }
        // Removing two files is fast enough to stay on the UI thread; a
        // failure has to be shown in the dialog before it closes.
        match ssh::delete_key(&key.path) {
            Ok(()) => {
                if let Some(view) = self.view.upgrade() {
                    view.update(cx, |view, cx| view.reload_ssh(cx));
                }
                self.close(window, cx);
            }
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }
}

fn build(state: &Entity<SshDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);
    let (title, icon) = match &dialog.kind {
        SshDialogKind::Generate => (ui::t!("settings.ssh.dialog.generate_title"), IconName::Key),
        SshDialogKind::Import => (ui::t!("settings.ssh.dialog.import_title"), IconName::Key),
        SshDialogKind::Delete(_) => (ui::t!("settings.ssh.dialog.delete_title"), IconName::Trash),
    };
    if let Some(key) = &dialog.result {
        return result_dialog(state, key, title, icon, cx);
    }
    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };
    let footer = {
        let state = state.clone();
        match &dialog.kind {
            SshDialogKind::Generate => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("ssh-dialog-cancel")
                        .label(ui::t!("settings.ssh.dialog.cancel"))
                        .focus_handle(dialog.cancel_focus.clone())
                        .on_click(cancel),
                )
                .child({
                    let state = state.clone();
                    Button::primary("ssh-dialog-generate")
                        .label(ui::t!("settings.ssh.dialog.generate"))
                        .disabled(dialog.busy)
                        .focus_handle(dialog.primary_focus.clone())
                        .on_click(move |_, window, cx| {
                            state.update(cx, |dialog, cx| dialog.generate(window, cx));
                        })
                }),
            SshDialogKind::Import => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("ssh-dialog-cancel")
                        .label(ui::t!("settings.ssh.dialog.cancel"))
                        .focus_handle(dialog.cancel_focus.clone())
                        .on_click(cancel),
                )
                .child({
                    let state = state.clone();
                    Button::primary("ssh-dialog-import")
                        .label(ui::t!("settings.ssh.dialog.import"))
                        .disabled(dialog.busy)
                        .focus_handle(dialog.primary_focus.clone())
                        .on_click(move |_, window, cx| {
                            state.update(cx, |dialog, cx| dialog.import(window, cx));
                        })
                }),
            SshDialogKind::Delete(key) => {
                let key = key.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("ssh-dialog-cancel")
                            .label(ui::t!("settings.ssh.dialog.cancel"))
                            .focus_handle(dialog.cancel_focus.clone())
                            .on_click(cancel),
                    )
                    .child({
                        let state = state.clone();
                        Button::danger("ssh-dialog-delete")
                            .label(ui::t!("settings.ssh.dialog.delete"))
                            .disabled(!dialog.ack_delete || dialog.busy)
                            .focus_handle(dialog.primary_focus.clone())
                            .on_click(move |_, window, cx| {
                                let key = key.clone();
                                state.update(cx, |dialog, cx| {
                                    dialog.delete(key.clone(), window, cx)
                                });
                            })
                    })
            }
        }
    };

    let body: AnyElement = match &dialog.kind {
        SshDialogKind::Generate => generate_body(state, cx, colors),
        SshDialogKind::Import => import_body(state, cx, colors),
        SshDialogKind::Delete(key) => delete_body(state, cx, key, colors),
    };
    let mut builder = Dialog::new(title)
        .icon(icon)
        .width(px(520.))
        .body(body)
        .footer(footer)
        .busy(dialog.busy)
        .focus_order(vec![
            DialogFocus::input(dialog.name_focus.clone()),
            DialogFocus::input(dialog.comment_focus.clone()),
            DialogFocus::input(dialog.pass_focus.clone()),
            DialogFocus::input(dialog.repeat_focus.clone()),
            DialogFocus::input(dialog.path_focus.clone()),
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(match &dialog.kind {
            SshDialogKind::Generate => dialog.name_focus.clone(),
            SshDialogKind::Import => dialog.path_focus.clone(),
            SshDialogKind::Delete(_) => dialog.cancel_focus.clone(),
        });
    if matches!(dialog.kind, SshDialogKind::Delete(_)) {
        builder = builder.default_button(dialog.cancel_focus.clone());
    }
    builder
}

fn result_dialog(
    state: &Entity<SshDialog>,
    key: &SshKey,
    title: SharedString,
    icon: IconName,
    cx: &App,
) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);
    let close = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };
    let copy = {
        let state = state.clone();
        let public = key.public_key.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(public.clone()));
            ui::toast::show(
                ToastKind::Ok,
                ui::t!("settings.ssh.dialog.copied"),
                window,
                cx,
            );
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };
    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            Banner::new(BannerKind::Ok, ui::t!("settings.ssh.dialog.added_title"))
                .body(ui::t!("settings.ssh.dialog.added_body")),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(FS_SM)
                        .child(ui::t!("settings.ssh.dialog.public_key")),
                )
                .child(
                    div()
                        .p(px(8.))
                        .rounded(R_SM)
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.bg)
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .child(key.public_key.clone()),
                ),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!(
                    "settings.ssh.dialog.fingerprint_line",
                    fingerprint = key.fingerprint
                )),
        )
        .when(!key.agent && key.encrypted, |this| {
            this.child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("settings.ssh.dialog.not_in_agent")),
            )
        });
    Dialog::new(title)
        .icon(icon)
        .width(px(560.))
        .body(body)
        .footer(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("ssh-result-copy")
                        .icon(IconName::Copy)
                        .label(ui::t!("settings.ssh.copy_public"))
                        .on_click(copy),
                )
                .child(
                    Button::primary("ssh-result-done")
                        .label(ui::t!("settings.ssh.dialog.done"))
                        .focus_handle(dialog.primary_focus.clone())
                        .on_click(close),
                ),
        )
        .default_button(dialog.primary_focus.clone())
        .focus_order(vec![DialogFocus::control(dialog.primary_focus.clone())])
        .autofocus(dialog.primary_focus.clone())
}

fn generate_body(state: &Entity<SshDialog>, cx: &App, colors: ui::ThemeColors) -> AnyElement {
    let dialog = state.read(cx);
    let mut body = div().flex().flex_col().gap(px(10.));
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(FS_SM)
                    .child(ui::t!("settings.ssh.dialog.file_name")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .child("~/.ssh/"),
                    )
                    .child(div().flex_1().min_w_0().child(dialog.name.clone())),
            )
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("settings.ssh.dialog.file_name_help")),
            ),
    );
    let type_select = {
        let state = state.clone();
        Select::new("ssh-key-type")
            .options(["Ed25519", "RSA 4096", "ECDSA"]) // algorithm names, not translated
            .selected(dialog.key_type)
            .on_change(move |index, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.key_type = index.min(2);
                    cx.notify();
                });
            })
    };
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(FS_SM)
                    .child(ui::t!("settings.ssh.dialog.type")),
            )
            .child(type_select)
            .child(div().text_size(FS_SM).text_color(colors.muted).child(
                if dialog.key_type == 0 {
                    ui::t!("settings.ssh.dialog.type_ed25519")
                } else if dialog.key_type == 1 {
                    ui::t!("settings.ssh.dialog.type_rsa")
                } else {
                    ui::t!("settings.ssh.dialog.type_ecdsa")
                },
            )),
    );
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(FS_SM)
                    .child(ui::t!("settings.ssh.dialog.comment")),
            )
            .child(dialog.comment.clone())
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("settings.ssh.dialog.comment_help")),
            ),
    );
    let use_pass = {
        let state = state.clone();
        Checkbox::new("ssh-use-pass")
            .label(ui::t!("settings.ssh.dialog.use_passphrase"))
            .checked(dialog.use_passphrase)
            .on_toggle(move |checked, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.use_passphrase = checked;
                    cx.notify();
                });
            })
    };
    body = body.child(use_pass);
    if dialog.use_passphrase {
        body = body
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(FS_SM)
                            .child(ui::t!("settings.ssh.dialog.passphrase")),
                    )
                    .child(dialog.passphrase.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(FS_SM)
                            .child(ui::t!("settings.ssh.dialog.repeat")),
                    )
                    .child(dialog.repeat.clone()),
            );
    } else {
        body = body.child(
            Banner::new(
                BannerKind::Warn,
                ui::t!("settings.ssh.dialog.no_pass_title"),
            )
            .body(ui::t!("settings.ssh.dialog.no_pass_body")),
        );
    }
    let add_agent = {
        let state = state.clone();
        Checkbox::new("ssh-add-agent")
            .label(ui::t!("settings.ssh.dialog.add_agent"))
            .checked(dialog.add_agent)
            .on_toggle(move |checked, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.add_agent = checked;
                    cx.notify();
                });
            })
    };
    body = body.child(add_agent);
    if let Some(error) = &dialog.error {
        body = body.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(error.clone()),
        );
    }
    body.into_any_element()
}

fn import_body(state: &Entity<SshDialog>, cx: &App, colors: ui::ThemeColors) -> AnyElement {
    let dialog = state.read(cx);
    let mut body = div().flex().flex_col().gap(px(10.));
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(FS_SM)
                    .child(ui::t!("settings.ssh.dialog.private_file")),
            )
            .child(dialog.path.clone())
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("settings.ssh.dialog.private_file_help")),
            ),
    );
    let copy = {
        let state = state.clone();
        Checkbox::new("ssh-import-copy")
            .label(ui::t!("settings.ssh.dialog.copy_into"))
            .checked(dialog.copy_import)
            .on_toggle(move |checked, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.copy_import = checked;
                    cx.notify();
                });
            })
    };
    body = body.child(copy);
    if let Some(error) = &dialog.error {
        body = body.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(error.clone()),
        );
    }
    body.into_any_element()
}

fn delete_body(
    state: &Entity<SshDialog>,
    cx: &App,
    key: &SshKey,
    colors: ui::ThemeColors,
) -> AnyElement {
    let dialog = state.read(cx);
    let mut body = div().flex().flex_col().gap(px(10.));
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(90.))
                            .text_color(colors.muted)
                            .child(ui::t!("settings.ssh.dialog.key")),
                    )
                    .child(key.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(90.))
                            .text_color(colors.muted)
                            .child(ui::t!("settings.ssh.dialog.fingerprint")),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .child(key.fingerprint.clone()),
                    ),
            ),
    );
    body = body.child(
        Banner::new(
            BannerKind::Warn,
            ui::t!("settings.ssh.dialog.delete_warn_title"),
        )
        .body(ui::t!("settings.ssh.dialog.delete_warn_body")),
    );
    let ack = {
        let state = state.clone();
        Checkbox::new("ssh-delete-ack")
            .label(ui::t!("settings.ssh.dialog.ack"))
            .checked(dialog.ack_delete)
            .on_toggle(move |checked, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.ack_delete = checked;
                    cx.notify();
                });
            })
    };
    body = body.child(ack);
    if let Some(error) = &dialog.error {
        body = body.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(error.clone()),
        );
    }
    body.into_any_element()
}
