//! Settings → Identity: the commit author (global or repository), commit
//! signing, and the GitHub/GitLab credentials the configured helper stores.

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Subscription, WeakEntity, Window,
};

use git_core::{ConfigScope, Git, Identity, Repo};
use ui::metrics::{FS_BASE, FS_SM, R_SM};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Dialog, DialogFocus, DialogHost, Icon,
    IconName, TextInput, TextInputEvent,
};

use super::form;
use super::{group, heading, SettingsView};
use crate::settings::{IdentityScope, Settings};

const HOSTS: [(&str, &str, &str); 2] = [
    ("github", "GitHub", "github.com"),
    ("gitlab", "GitLab", "gitlab.com"),
];

/// The accounts found in the credential helper.
#[derive(Default, Clone)]
pub(crate) struct Accounts {
    pub github: Option<String>,
    pub gitlab: Option<String>,
}

fn scope_of(settings: &Settings) -> ConfigScope {
    settings.identity_scope.into()
}

fn host_of(key: &str) -> &'static str {
    if key == "gitlab" {
        "gitlab.com"
    } else {
        "github.com"
    }
}

fn account_of<'a>(settings: &'a Settings, key: &str) -> &'a str {
    if key == "gitlab" {
        &settings.account_gitlab
    } else {
        &settings.account_github
    }
}

impl SettingsView {
    pub(super) fn reload_identity(&mut self, cx: &mut Context<Self>) {
        self.reload_accounts(cx);
        if self.ssh_keys.is_empty() {
            self.reload_ssh(cx);
        }
        let settings = self.settings(cx);
        let scope = scope_of(&settings);
        let root = self.root.clone();
        if root.is_none() && scope == ConfigScope::Local {
            self.identity = None;
            self.identity_error = Some("Open a repository to edit its identity.".into());
            self.identity_busy = false;
            cx.notify();
            return;
        }
        let Ok(git) = self.git.clone() else {
            self.identity = None;
            self.identity_error = Some("Git is not available.".into());
            self.identity_busy = false;
            cx.notify();
            return;
        };
        self.identity_generation = self.identity_generation.wrapping_add(1);
        let generation = self.identity_generation;
        self.identity_busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => Repo::open(git, &root).and_then(|repo| repo.identity(scope)),
                        None => git_core::global_identity(&git),
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if generation != view.identity_generation {
                    return;
                }
                view.identity_busy = false;
                match result {
                    Ok(identity) => {
                        view.identity_error = None;
                        view.apply_identity(&identity, cx);
                    }
                    Err(error) => {
                        view.identity_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn apply_identity(&mut self, identity: &Identity, cx: &mut Context<Self>) {
        self.identity = Some(identity.clone());
        self.identity_touched = false;
        // The Signing controls mirror git's configuration, not a separate
        // app-level preference.
        let sign = identity.sign_commits;
        let key = identity.signing_key.clone();
        crate::update_settings(cx, |settings| {
            settings.sign_commits = sign;
            settings.signing_key = key;
        });
        self.identity_syncing = true;
        self.name_input
            .update(cx, |input, cx| input.set_text(identity.name.clone(), cx));
        self.email_input
            .update(cx, |input, cx| input.set_text(identity.email.clone(), cx));
        self.identity_syncing = false;
    }

    pub(super) fn on_author_changed(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        if self.identity_syncing {
            return;
        }
        if let TextInputEvent::Changed(_) = event {
            self.identity_touched = true;
            self.schedule_identity_save(cx);
        }
        if let TextInputEvent::Submit = event {
            self.save_identity_now(cx);
        }
        cx.notify();
    }

    fn schedule_identity_save(&mut self, cx: &mut Context<Self>) {
        self.identity_save_generation = self.identity_save_generation.wrapping_add(1);
        let generation = self.identity_save_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            this.update(cx, |view, cx| {
                if generation == view.identity_save_generation {
                    view.save_identity_now(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn save_identity_now(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let name = self.name_input.read(cx).text().trim().to_owned();
        let email = self.email_input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.identity_touched = true;
            self.identity_error =
                Some("Name is required — git refuses to commit without it.".into());
            cx.notify();
            return;
        }
        if !valid_email(&email) {
            self.identity_touched = true;
            self.identity_error =
                Some("Enter a valid email address, like name@example.com.".into());
            cx.notify();
            return;
        }
        let root = self.root.clone();
        let Ok(git) = self.git.clone() else {
            self.identity_error = Some("Git is not available.".into());
            cx.notify();
            return;
        };
        let scope = scope_of(&settings);
        let sign = settings.sign_commits;
        let key = settings.signing_key.clone();
        self.identity_busy = true;
        self.identity_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => {
                            let repo = Repo::open(git, &root)?;
                            repo.set_identity(scope, &name, &email)?;
                            repo.set_signing(scope, sign, &key)?;
                            repo.identity(scope)
                        }
                        None => {
                            git_core::set_global_identity(&git, &name, &email)?;
                            git_core::set_global_signing(&git, sign, &key)?;
                            git_core::global_identity(&git)
                        }
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                view.identity_busy = false;
                match result {
                    Ok(identity) => {
                        view.identity_error = None;
                        view.apply_identity(&identity, cx);
                    }
                    Err(error) => {
                        view.identity_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save_signing(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let root = self.root.clone();
        let Ok(git) = self.git.clone() else {
            return;
        };
        let scope = scope_of(&settings);
        let sign = settings.sign_commits;
        let key = settings.signing_key.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => {
                            let repo = Repo::open(git, &root)?;
                            repo.set_signing(scope, sign, &key)
                        }
                        None => git_core::set_global_signing(&git, sign, &key),
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.identity_error = Some(error.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn reload_accounts(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let mut accounts = Accounts {
            github: (!settings.account_github.is_empty()).then(|| settings.account_github.clone()),
            gitlab: (!settings.account_gitlab.is_empty()).then(|| settings.account_gitlab.clone()),
        };
        let Ok(git) = self.git.clone() else {
            self.accounts = accounts;
            return;
        };
        self.accounts = accounts.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let mut found = Vec::new();
                    for (key, _, host) in HOSTS {
                        if let Ok(Some(credential)) = git_core::credential::fill(&git, host) {
                            found.push((key, credential.username));
                        }
                    }
                    found
                })
                .await;
            this.update(cx, |view, cx| {
                for (key, username) in found {
                    if key == "github" {
                        accounts.github = Some(username);
                    } else {
                        accounts.gitlab = Some(username);
                    }
                }
                view.accounts = accounts;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_account_dialog(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let git = self.git.clone().ok();
        let view = cx.entity().downgrade();
        self.account_dialog = Some(open_account_dialog(key, git, view, window, cx));
    }

    pub(super) fn disconnect_account(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let username = account_of(&settings, key).to_owned();
        let host = host_of(key);
        self.change(cx, |settings| {
            if key == "github" {
                settings.account_github.clear();
            } else {
                settings.account_gitlab.clear();
            }
        });
        if key == "github" {
            self.accounts.github = None;
        } else {
            self.accounts.gitlab = None;
        }
        let Ok(git) = self.git.clone() else {
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git_core::credential::reject(&git, host, &username) })
                .await;
            this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.identity_error = Some(error.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        "Identity",
        "Who your commits are attributed to, how they are signed, and which hosting accounts GitWarp can use.",
    ));
    if let Some(error) = &view.identity_error {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Error, "Could not read or save the identity")
                    .body(error.clone()),
            ),
        );
    }

    let scope = {
        let entity = cx.entity();
        let control = form::segmented(
            "settings-id-scope".into(),
            ["All repositories", "This repository only"],
            usize::from(settings.identity_scope == IdentityScope::Repository),
            false,
            move |index, _, cx| {
                let scope = if index == 1 {
                    IdentityScope::Repository
                } else {
                    IdentityScope::Global
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.identity_scope = scope);
                    view.reload_identity(cx);
                });
            },
        );
        div().w(px(330.)).child(control).into_any_element()
    };
    let scope_help = if settings.identity_scope == IdentityScope::Global {
        "Written to ~/.gitconfig. A repository can still override it."
    } else {
        "Written to .git/config of the open repository."
    };

    let name = div().w_full().h(px(26.)).child(view.name_input.clone());
    let email = div().w_full().h(px(26.)).child(view.email_input.clone());
    let name_empty = view.name_input.read(cx).text().trim().is_empty();
    let name_error = (view.identity_touched && name_empty)
        .then_some("Name is required — git refuses to commit without it.");
    let email_text = view.email_input.read(cx).text().trim().to_owned();
    let email_error = (view.identity_touched && !valid_email(&email_text))
        .then_some("Enter a valid email address, like name@example.com.");

    let mut author_rows = vec![
        form::field_row("Apply to", None, scope).into_any_element(),
        div()
            .px(px(12.))
            .pb(px(6.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(scope_help)
            .into_any_element(),
        div()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_t_1()
            .child(div().text_size(FS_SM).child("Name"))
            .child(name)
            .when_some(name_error, |this, error| this.child(form::error(error, cx)))
            .into_any_element(),
        div()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_t_1()
            .child(div().text_size(FS_SM).child("Email"))
            .child(email)
            .when_some(email_error, |this, error| {
                this.child(form::error(error, cx))
            })
            .when(view.identity_touched && email_error.is_none(), |this| {
                this.child(form::help("Visible in every commit you create.", cx))
            })
            .into_any_element(),
    ];
    if view.identity_busy {
        author_rows.push(
            div()
                .px(px(12.))
                .pb(px(8.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child("Saving…")
                .into_any_element(),
        );
    }
    page = page.child(group("Commit author", author_rows));

    let sign = {
        let entity = cx.entity();
        form::switch(
            "settings-id-sign".into(),
            settings.sign_commits,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.sign_commits = checked);
                    view.save_signing(cx);
                });
            },
        )
    };
    let key_names: Vec<String> = {
        let mut names: Vec<String> = view.ssh_keys.iter().map(|key| key.name.clone()).collect();
        if !settings.signing_key.is_empty() && !names.contains(&settings.signing_key) {
            names.insert(0, settings.signing_key.clone());
        }
        names
    };
    let key_index = key_names
        .iter()
        .position(|name| *name == settings.signing_key)
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut key_options = vec!["No signing key".to_owned()];
    key_options.extend(key_names.clone());
    let signing_key = {
        let entity = cx.entity();
        let names = key_names.clone();
        form::select(
            "settings-id-key".into(),
            key_options,
            key_index,
            !settings.sign_commits,
            move |index, _, cx| {
                let key = if index == 0 {
                    String::new()
                } else {
                    names.get(index - 1).cloned().unwrap_or_default()
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.signing_key = key);
                    view.save_signing(cx);
                });
            },
        )
    };
    page = page.child(group(
        "Signing",
        vec![
            form::row(
                "settings-id-sign-row",
                "Sign commits",
                "Adds a verified badge on hosts that know your key.",
                sign,
                false,
            ),
            form::row(
                "settings-id-key-row",
                "Signing key",
                "Used for commits and tags.",
                signing_key,
                !settings.sign_commits,
            ),
        ],
    ));

    let mut accounts = div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .border_1()
        .rounded(R_SM)
        .overflow_hidden();
    for (key, label, host) in HOSTS {
        let username = if key == "github" {
            view.accounts.github.clone()
        } else {
            view.accounts.gitlab.clone()
        };
        let connect = {
            let entity = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                entity.update(cx, |view, cx| {
                    view.open_account_dialog(key, window, cx);
                });
            }
        };
        let disconnect = {
            let entity = cx.entity();
            move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                entity.update(cx, |view, cx| view.disconnect_account(key, cx));
            }
        };
        let connected = username.is_some();
        accounts = accounts.child(
            div()
                .w_full()
                .px(px(12.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .w(px(32.))
                        .h(px(32.))
                        .rounded(px(16.))
                        .bg(colors.active)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Cloud).size(px(16.)).color(colors.muted)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(FS_BASE)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(FS_SM)
                                .when(connected, |this| this.text_color(colors.success))
                                .when(!connected, |this| this.text_color(colors.muted))
                                .child(
                                    Icon::new(if connected {
                                        IconName::Check
                                    } else {
                                        IconName::Cloud
                                    })
                                    .size(px(12.))
                                    .color(if connected {
                                        colors.success
                                    } else {
                                        colors.muted
                                    }),
                                )
                                .child(match &username {
                                    Some(user) => format!("Connected as {user}"),
                                    None => "Not connected".to_owned(),
                                }),
                        ),
                )
                .child(if connected {
                    Button::new(format!("settings-account-{key}"))
                        .label("Disconnect")
                        .size(ButtonSize::Small)
                        .on_click(disconnect)
                } else {
                    Button::new(format!("settings-account-{key}"))
                        .label("Connect")
                        .size(ButtonSize::Small)
                        .on_click(connect)
                })
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child(host),
                ),
        );
    }
    page = page.child(
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
                    .child("HOSTING ACCOUNTS"),
            )
            .child(accounts)
            .child(form::help(
                "Used for cloning private repositories and for push credentials. Tokens are stored by your git credential helper, not in settings.json.",
                cx,
            )),
    );
    page.into_any_element()
}

/* ----------------------------- account dialog ---------------------------- */

pub(crate) struct AccountDialog {
    host: &'static str,
    key: &'static str,
    layer: Option<overlay::LayerId>,
    _host: Option<Entity<DialogHost>>,
    username: Entity<TextInput>,
    token: Entity<TextInput>,
    _username_events: Subscription,
    _token_events: Subscription,
    git: Option<Arc<Git>>,
    view: WeakEntity<SettingsView>,
    error: Option<SharedString>,
    username_focus: gpui::FocusHandle,
    token_focus: gpui::FocusHandle,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

/// Opens the "Connect GitHub/GitLab" dialog.
pub(super) fn open_account_dialog(
    key: &'static str,
    git: Option<Arc<Git>>,
    view: WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AccountDialog> {
    let host = host_of(key);
    let dialog = cx.new(|cx| {
        let username = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Your user name", cx);
            input
        });
        let token = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Personal access token", cx);
            input.set_masked(true, cx);
            input
        });
        let username_focus = username.read(cx).focus_handle().clone();
        let token_focus = token.read(cx).focus_handle().clone();
        let username_events = cx.subscribe(&username, |_, _, _: &TextInputEvent, cx| cx.notify());
        let token_events = cx.subscribe(&token, |_, _, _: &TextInputEvent, cx| cx.notify());
        AccountDialog {
            host,
            key,
            layer: None,
            _host: None,
            username,
            token,
            _username_events: username_events,
            _token_events: token_events,
            git,
            view,
            error: None,
            username_focus,
            token_focus,
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    });
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Connect"),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog._host = Some(host);
    });
    dialog
}

impl AccountDialog {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).text().trim().to_owned();
        let token = self.token.read(cx).text().to_owned();
        if username.is_empty() || token.is_empty() {
            self.error = Some("Enter both the user name and a token.".into());
            cx.notify();
            return;
        }
        let host = self.host;
        let key = self.key;
        let git = self.git.clone();
        let view = self.view.clone();
        self.close(window, cx);
        cx.spawn(async move |_, cx| {
            if let Some(git) = git {
                let approve_user = username.clone();
                let approve_token = token.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        git_core::credential::approve(&git, host, &approve_user, &approve_token)?;
                        // Confirm the helper really stored it: `git credential
                        // approve` succeeds even when no helper is configured.
                        git_core::credential::fill(&git, host)
                    })
                    .await;
                let stored = match result {
                    Ok(stored) => stored.is_some(),
                    Err(error) => {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |view, cx| {
                                view.identity_error = Some(error.to_string().into());
                                cx.notify();
                            });
                        }
                        return;
                    }
                };
                if !stored {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |view, cx| {
                            view.identity_error = Some(
                                "Your git credential helper did not store the token. Configure one (for example `credential.helper=store`) and try again."
                                    .into(),
                            );
                            cx.notify();
                        });
                    }
                    return;
                }
            }
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.change(cx, |settings| {
                        if key == "github" {
                            settings.account_github = username.clone();
                        } else {
                            settings.account_gitlab = username.clone();
                        }
                    });
                    if key == "github" {
                        view.accounts.github = Some(username.clone());
                    } else {
                        view.accounts.gitlab = Some(username.clone());
                    }
                    view.reload_accounts(cx);
                });
            }
        })
        .detach();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }
}

fn build(state: &Entity<AccountDialog>, cx: &App) -> Dialog {
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
    let can_submit =
        !dialog.username.read(cx).text().is_empty() && !dialog.token.read(cx).text().is_empty();

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            Banner::new(
                BannerKind::Info,
                format!("Connect {}", if dialog.key == "github" { "GitHub" } else { "GitLab" }),
            )
            .body(format!(
                "The token is handed to your git credential helper for {}. It is not written to settings.json.",
                dialog.host
            )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(FS_SM).child("User name"))
                .child(dialog.username.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(FS_SM).child("Personal access token"))
                .child(dialog.token.clone()),
        )
        .when_some(dialog.error.clone(), |this, error| {
            this.child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.error)
                    .child(error),
            )
        });

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("account-cancel")
                .label("Cancel")
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("account-connect")
                .label("Connect")
                .disabled(!can_submit)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(submit),
        );

    Dialog::new("Connect an account")
        .icon(IconName::Cloud)
        .width(px(520.))
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::input(dialog.username_focus.clone()),
            DialogFocus::input(dialog.token_focus.clone()),
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.username_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_addresses_only() {
        assert!(valid_email("mira@kovalenko.dev"));
        assert!(valid_email("mira.k@users.noreply.github.com"));
        assert!(!valid_email(""));
        assert!(!valid_email("mira"));
        assert!(!valid_email("mira@localhost"));
        assert!(!valid_email("mira@.dev"));
        assert!(!valid_email("@kovalenko.dev"));
    }
}
