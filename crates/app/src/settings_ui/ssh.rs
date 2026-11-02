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
                "This key has no public half to copy",
                window,
                cx,
            );
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(key.public_key.clone()));
        ui::toast::show(
            ToastKind::Ok,
            format!("Public key of {} copied", key.name),
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
        self.ssh_tests
            .insert(slot.clone(), TestState::Running(host.clone()));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let host = host.clone();
                    async move { ssh::test_connection(&host, 8) }
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
        "SSH keys",
        "Keys GitWarp can use to clone, fetch and push over SSH. Private keys stay in ~/.ssh; only fingerprints and public keys are shown here.",
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
                    .label("Generate key…")
                    .icon(IconName::Plus)
                    .on_click(generate),
            )
            .child(
                Button::new("settings-ssh-import")
                    .label("Import…")
                    .icon(IconName::Folder)
                    .on_click(import),
            )
            .child(div().flex_1())
            .child(
                Button::new("settings-ssh-refresh")
                    .icon(IconName::Fetch)
                    .label("Refresh")
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
                Banner::new(BannerKind::Info, "ssh-agent is running").body(format!(
                    "{loaded} of {} keys loaded. Loaded keys are offered to hosts automatically.",
                    view.ssh_keys.len()
                )),
            ));
        }
        Some(false) => {
            page = page.child(div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Error, "ssh-agent is not running").body(
                    "Keys protected by a passphrase cannot be used until it starts. Start it with eval \"$(ssh-agent -s)\" or enable the ssh-agent user service, then retry.",
                ),
            ));
            page = page.child(
                div().max_w(px(760.)).mb(px(12.)).child(
                    Button::new("settings-ssh-agent-retry")
                        .label("Check again")
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
        page =
            page.child(div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Error, "Could not read ~/.ssh").body(error.clone()),
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
                        .child("No SSH keys yet"),
                )
                .child(
                    div()
                        .max_w(px(420.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child("Create a key and add its public half to GitHub, GitLab or your own server to clone private repositories without typing a password."),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Button::primary("settings-ssh-empty-generate")
                                .label("Generate key…")
                                .icon(IconName::Plus)
                                .on_click({
                                    let entity = cx.entity();
                                    move |_, window, cx: &mut App| {
                                        entity.update(cx, |view, cx| {
                                            view.open_ssh_dialog(SshDialogKind::Generate, window, cx)
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("settings-ssh-empty-import")
                                .label("Import existing key…")
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
    page = page.child(section("YOUR KEYS", keys));

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
                        .child("No trusted hosts"),
                )
                .child(div().text_size(FS_SM).text_color(colors.muted).child(
                    "Hosts appear here after you confirm their fingerprint on first connection.",
                )),
        );
    } else {
        for (index, host) in view.ssh_hosts.iter().enumerate() {
            hosts = hosts.child(host_row(view, index, host, cx));
        }
    }
    page = page.child(section("TRUSTED HOSTS", hosts));
    page = page.child(
        div()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Icon::new(IconName::Info).size(px(12.)).color(colors.muted))
            .child("Stored in ~/.ssh/known_hosts."),
    );
    page.into_any_element()
}

fn section(label: &'static str, body: impl IntoElement) -> AnyElement {
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
            Chip::new("ssh-agent", "in ssh-agent")
                .kind(ChipKind::Ok)
                .icon(IconName::Check),
        );
    }
    chips = chips.child(if key.encrypted {
        Chip::new("ssh-pass", "passphrase").icon(IconName::Lock)
    } else {
        Chip::new("ssh-nopass", "no passphrase")
            .kind(ChipKind::Warn)
            .icon(IconName::Warn)
    });
    if key.kind == "rsa-2048" {
        chips = chips.child(
            Chip::new("ssh-weak", "weak: RSA 2048")
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
                .label("Copy public key")
                .on_click(copy),
        )
        .child(
            Button::new(format!("ssh-test-{}", key.name))
                .size(ButtonSize::Small)
                .icon(IconName::Fetch)
                .label("Test connection")
                .disabled(test_host.is_none() || matches!(test, Some(TestState::Running(_))))
                .on_click(test_button),
        )
        .child(
            Button::new(format!("ssh-agent-{}", key.name))
                .size(ButtonSize::Small)
                .label(if key.agent {
                    "Remove from agent"
                } else {
                    "Add to agent"
                })
                .disabled(view.ssh_agent != Some(true))
                .on_click(agent),
        )
        .child(
            Button::danger(format!("ssh-delete-{}", key.name))
                .size(ButtonSize::Small)
                .icon(IconName::Trash)
                .label("Delete…")
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
                            "fingerprint unavailable (key needs its passphrase)".to_owned()
                        } else {
                            key.fingerprint.clone()
                        }),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(format!(
                            "{} · {}",
                            if created.is_empty() {
                                "Created date unknown".to_owned()
                            } else {
                                format!("Created {created}")
                            },
                            match test {
                                Some(_) => test_host
                                    .as_ref()
                                    .map(|host| format!("tested against {host}"))
                                    .unwrap_or_else(|| "not tested against any host".to_owned()),
                                None => "not tested against any host".to_owned(),
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
            .child(format!("Connecting to {host}…"))
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
            .child(format!("{host} — {message}"))
            .into_any_element(),
        TestState::Failed(host, message) => div()
            .flex()
            .items_start()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.error)
            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
            .child(format!("{host} — {message}"))
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
                .tooltip(format!("Forget {}", host.host))
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
            input.set_placeholder("you@example.com", cx);
            input.set_text(std::env::var("GIT_AUTHOR_EMAIL").unwrap_or_default(), cx);
            input
        });
        let passphrase = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Passphrase", cx);
            input.set_masked(true, cx);
            input
        });
        let repeat = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Repeat passphrase", cx);
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
            self.error = Some("Choose a file name without spaces or slashes.".into());
            cx.notify();
            return;
        }
        let pass = self.passphrase.read(cx).text().to_owned();
        let repeat = self.repeat.read(cx).text().to_owned();
        if self.use_passphrase && (pass.len() < 5 || pass != repeat) {
            self.error = Some("The passphrases must match and be at least 5 characters.".into());
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
            self.error = Some("Choose a private key file.".into());
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
        SshDialogKind::Generate => ("Generate SSH key", IconName::Key),
        SshDialogKind::Import => ("Import SSH key", IconName::Key),
        SshDialogKind::Delete(_) => ("Delete SSH key", IconName::Trash),
    };
    if let Some(key) = &dialog.result {
        return result_dialog(state, key, title.into(), icon, cx);
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
                        .label("Cancel")
                        .focus_handle(dialog.cancel_focus.clone())
                        .on_click(cancel),
                )
                .child({
                    let state = state.clone();
                    Button::primary("ssh-dialog-generate")
                        .label("Generate key")
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
                        .label("Cancel")
                        .focus_handle(dialog.cancel_focus.clone())
                        .on_click(cancel),
                )
                .child({
                    let state = state.clone();
                    Button::primary("ssh-dialog-import")
                        .label("Import key")
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
                            .label("Cancel")
                            .focus_handle(dialog.cancel_focus.clone())
                            .on_click(cancel),
                    )
                    .child({
                        let state = state.clone();
                        Button::danger("ssh-dialog-delete")
                            .label("Delete key")
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
            ui::toast::show(ToastKind::Ok, "Public key copied to clipboard", window, cx);
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };
    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            Banner::new(BannerKind::Ok, "Add the public key to your Git host").body(
                "Paste it in GitHub → Settings → SSH and GPG keys, or the same page on your host. The private key never leaves this machine.",
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(FS_SM).child("Public key"))
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
                .child(format!("Fingerprint: {}", key.fingerprint)),
        )
        .when(!key.agent && key.encrypted, |this| {
            this.child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("The key was not added to ssh-agent: it needs its passphrase first."),
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
                        .label("Copy public key")
                        .on_click(copy),
                )
                .child(
                    Button::primary("ssh-result-done")
                        .label("Done")
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
            .child(div().text_size(FS_SM).child("File name"))
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
                    .child("The public key is saved next to it with a .pub extension."),
            ),
    );
    let type_select = {
        let state = state.clone();
        Select::new("ssh-key-type")
            .options(["Ed25519", "RSA 4096", "ECDSA"])
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
            .child(div().text_size(FS_SM).child("Type"))
            .child(type_select)
            .child(div().text_size(FS_SM).text_color(colors.muted).child(
                if dialog.key_type == 0 {
                    "Recommended: short, fast, supported by GitHub, GitLab and Bitbucket."
                } else if dialog.key_type == 1 {
                    "Compatible with old servers; slower and larger."
                } else {
                    "Supported by most hosts; prefer Ed25519 when you can."
                },
            )),
    );
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().text_size(FS_SM).child("Comment"))
            .child(dialog.comment.clone())
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("Helps you recognise the key on the host, usually your email."),
            ),
    );
    let use_pass = {
        let state = state.clone();
        Checkbox::new("ssh-use-pass")
            .label("Protect the key with a passphrase (recommended)")
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
                    .child(div().text_size(FS_SM).child("Passphrase"))
                    .child(dialog.passphrase.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(FS_SM).child("Repeat passphrase"))
                    .child(dialog.repeat.clone()),
            );
    } else {
        body = body.child(
            Banner::new(
                BannerKind::Warn,
                "Anyone who can read the file can use this key",
            )
            .body("Without a passphrase, a stolen backup or laptop gives access to every host this key is registered on."),
        );
    }
    let add_agent = {
        let state = state.clone();
        Checkbox::new("ssh-add-agent")
            .label("Add the key to ssh-agent now")
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
            .child(div().text_size(FS_SM).child("Private key file"))
            .child(dialog.path.clone())
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("Choose the private file, for example id_ed25519, not the .pub file."),
            ),
    );
    let copy = {
        let state = state.clone();
        Checkbox::new("ssh-import-copy")
            .label("Copy into ~/.ssh (the original stays where it is)")
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
                    .child(div().w(px(90.)).text_color(colors.muted).child("Key"))
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
                            .child("Fingerprint"),
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
            "The private key file is deleted from ~/.ssh",
        )
        .body(
            "Hosts that use it will reject you until you add another key. This cannot be undone.",
        ),
    );
    let ack = {
        let state = state.clone();
        Checkbox::new("ssh-delete-ack")
            .label("I have a backup or no longer need this key")
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
