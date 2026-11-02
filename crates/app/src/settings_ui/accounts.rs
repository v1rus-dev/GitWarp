//! Settings → Accounts: the hosting-service accounts, their live status, and
//! the Rename and Remove dialogs. Add and Reconnect live in `account_dialog`.
//!
//! On opening the page every account is checked in the background against its
//! service (`forge::api::current_user`); rows show "Checking…" until the
//! answer arrives.

use std::sync::atomic::Ordering;

use forge::ServiceKind;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Pixels, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window,
};

use ui::metrics::{FS_BASE, FS_SM, FS_XS, R_MD, R_XS};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Dialog, DialogFocus, DialogHost,
    EmptyState, Icon, IconName, TextInput, TextInputEvent, Tooltip,
};

use super::account_dialog;
use super::{heading, SettingsView};
use crate::accounts::{self as logic, cached_store, AccountStatus, StoreResult};
use crate::settings::{AccountMethod, AccountRecord};

/// The service's icon: the GitHub mark, or a monogram badge for the others.
pub(super) fn service_icon(kind: ServiceKind, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let frame = div()
        .w(px(20.))
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    if kind == ServiceKind::GitHub {
        return frame
            .child(Icon::new(IconName::Github).size(px(16.)).color(colors.text))
            .into_any_element();
    }
    frame
        .child(
            div()
                .min_w(px(20.))
                .h(px(16.))
                .px(px(2.))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(colors.border_strong)
                .rounded(R_XS)
                .text_color(colors.muted)
                .text_size(FS_XS)
                .font_weight(FontWeight::BOLD)
                .line_height(px(11.))
                .child(logic::monogram(kind)),
        )
        .into_any_element()
}

/// A round avatar with the first letter of the login.
pub(super) fn avatar(login: &str, size: Pixels, font: Pixels, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let letter: String = login
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned());
    div()
        .w(size)
        .h(size)
        .flex_none()
        .rounded(size / 2.)
        .bg(colors.active)
        .text_color(colors.muted)
        .text_size(font)
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(letter)
        .into_any_element()
}

impl SettingsView {
    /// Checks every account (page opened) in the background.
    pub(super) fn open_accounts(&mut self, cx: &mut Context<Self>) {
        self.start_checks(None, cx);
    }

    /// Resolves the secret store, then checks the matching accounts, each in
    /// its own background task. `only` limits it to one account (Retry).
    pub(crate) fn start_checks(&mut self, only: Option<String>, cx: &mut Context<Self>) {
        let targets: Vec<AccountRecord> = self
            .settings(cx)
            .accounts
            .into_iter()
            .filter(|a| only.as_ref().is_none_or(|id| *id == a.id))
            .collect();
        for account in &targets {
            self.account_status
                .insert(account.id.clone(), AccountStatus::Checking);
        }
        let cached = cached_store(cx);
        cx.spawn(async move |this, cx| {
            let store = cx
                .background_executor()
                .spawn(async move { cached.unwrap_or_else(logic::resolve_store) })
                .await;
            this.update(cx, |view, cx| {
                if only.is_none() || store.is_err() {
                    view.accounts_error = store.as_ref().err().map(|e| e.to_string().into());
                }
                for account in targets {
                    view.run_check(store.clone(), account, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run_check(&mut self, store: StoreResult, account: AccountRecord, cx: &mut Context<Self>) {
        let counter = self
            .account_checks
            .entry(account.id.clone())
            .or_default()
            .clone();
        let generation = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let git = self.git.clone().ok();
        cx.spawn(async move |this, cx| {
            let id = account.id.clone();
            let current = counter.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    // A newer check or a removal bumps the counter: the legacy
                    // import must not write the keychain for it.
                    let is_current = || current.load(Ordering::SeqCst) == generation;
                    logic::check_account(&store, git.as_deref(), &account, &is_current)
                })
                .await;
            this.update(cx, |view, cx| {
                if view
                    .account_checks
                    .get(&id)
                    .is_none_or(|c| c.load(Ordering::SeqCst) != generation)
                {
                    return;
                }
                if let Some(login) = outcome.new_login {
                    crate::update_settings(cx, |settings| {
                        if let Some(account) = settings.accounts.iter_mut().find(|a| a.id == id) {
                            account.login = login;
                        }
                    });
                }
                view.account_status.insert(id, outcome.status);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        self.account_dialog = Some(account_dialog::open(
            account_dialog::Open::Add,
            view,
            window,
            cx,
        ));
    }

    fn open_reconnect(
        &mut self,
        account: AccountRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        self.account_dialog = Some(account_dialog::open(
            account_dialog::Open::Reconnect(account),
            view,
            window,
            cx,
        ));
    }

    fn open_prompt(
        &mut self,
        kind: PromptKind,
        account: AccountRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        self.account_prompt = Some(open_prompt(kind, account, view, window, cx));
    }

    /// Forgets the account and deletes its token from the keychain.
    fn remove_account(&mut self, account: &AccountRecord, cx: &mut Context<Self>) {
        let id = account.id.clone();
        self.account_status.remove(&id);
        // Bumping the counter tells an in-flight check the account is gone.
        if let Some(counter) = self.account_checks.remove(&id) {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        crate::update_settings(cx, |settings| settings.accounts.retain(|a| a.id != id));
        let cached = cached_store(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = cached.unwrap_or_else(logic::resolve_store)?;
                    store.delete(&id)
                })
                .await;
            this.update(cx, |view, cx| {
                // The account is gone either way; say if its token may remain.
                if let Err(error) = result {
                    view.accounts_error = Some(
                        format!("The account was removed, but its token could not be deleted from the keychain: {error}")
                            .into(),
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn status_view(status: &AccountStatus, id: &str, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let (icon, text, color, tooltip): (Option<IconName>, &str, _, Option<String>) = match status {
        AccountStatus::Connected => (Some(IconName::Check), "Connected", colors.success, None),
        AccountStatus::Checking => (None, "Checking…", colors.muted, None),
        AccountStatus::Invalid(detail) => (
            Some(IconName::Err),
            "Token invalid or revoked",
            colors.error,
            detail.clone(),
        ),
        AccountStatus::Unreachable(detail) => (
            Some(IconName::Err),
            "Instance unreachable",
            colors.error,
            detail.clone(),
        ),
        AccountStatus::Unchecked(why) => (
            Some(IconName::Warn),
            "Not checked",
            colors.muted,
            Some(why.clone()),
        ),
    };
    div()
        .id(ElementId::Name(
            format!("settings-account-{id}-status").into(),
        ))
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(FS_SM)
        .text_color(color)
        .whitespace_nowrap()
        .when(matches!(status, AccountStatus::Checking), |this| {
            this.child(ui::Spinner::new(
                ElementId::Name(format!("settings-account-{id}-spin").into()),
                px(12.),
            ))
        })
        .when_some(icon, |this, icon| {
            this.child(Icon::new(icon).size(px(12.)).color(color))
        })
        .child(text)
        .when_some(tooltip, |this, tooltip| {
            this.tooltip(Tooltip::text(tooltip))
        })
        .into_any_element()
}

fn account_row(
    account: &AccountRecord,
    status: &AccountStatus,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let kind = logic::kind_of(account).unwrap_or(ServiceKind::GitHub);
    let id = account.id.clone();
    let label = account.display_label().to_owned();
    let host = logic::host_of(&account.base_url);
    let (method_icon, method_text) = match account.method {
        AccountMethod::Token => (IconName::Key, "Token"),
        AccountMethod::Browser => (IconName::Link, "Browser"),
    };

    let mut actions = div().flex().items_center().gap(px(6.)).flex_none();
    match status {
        AccountStatus::Invalid(_) => {
            let entity = cx.entity();
            let account = account.clone();
            actions = actions.child(
                Button::primary(format!("settings-account-{id}-reconnect"))
                    .label("Reconnect")
                    .size(ButtonSize::Small)
                    .on_click(move |_: &ClickEvent, window, cx| {
                        let account = account.clone();
                        entity.update(cx, |view, cx| view.open_reconnect(account, window, cx));
                    }),
            );
        }
        AccountStatus::Unreachable(_) | AccountStatus::Unchecked(_) => {
            let entity = cx.entity();
            let retry_id = id.clone();
            actions = actions.child(
                Button::new(format!("settings-account-{id}-retry"))
                    .label("Retry")
                    .icon(IconName::Fetch)
                    .size(ButtonSize::Small)
                    .on_click(move |_: &ClickEvent, _, cx| {
                        let retry_id = retry_id.clone();
                        entity.update(cx, |view, cx| view.start_checks(Some(retry_id), cx));
                    }),
            );
        }
        _ => {}
    }
    let rename = {
        let entity = cx.entity();
        let account = account.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let account = account.clone();
            entity.update(cx, |view, cx| {
                view.open_prompt(PromptKind::Rename, account, window, cx)
            });
        }
    };
    let profile = {
        let url = logic::profile_url(account);
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| cx.open_url(&url)
    };
    let remove = {
        let entity = cx.entity();
        let account = account.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let account = account.clone();
            entity.update(cx, |view, cx| {
                view.open_prompt(PromptKind::Remove, account, window, cx)
            });
        }
    };
    actions = actions
        .child(
            Button::ghost(format!("settings-account-{id}-rename"))
                .icon_only(IconName::Edit)
                .size(ButtonSize::Small)
                .tooltip("Rename label")
                .on_click(rename),
        )
        .child(
            Button::ghost(format!("settings-account-{id}-profile"))
                .icon_only(IconName::Link)
                .size(ButtonSize::Small)
                .tooltip("Open profile in browser")
                .on_click(profile),
        )
        .child(
            Button::ghost(format!("settings-account-{id}-remove"))
                .icon_only(IconName::Trash)
                .size(ButtonSize::Small)
                .tooltip("Remove account")
                .on_click(remove),
        );

    div()
        .id(ElementId::Name(format!("settings-account-{id}").into()))
        .w_full()
        .min_h(px(52.))
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(service_icon(kind, cx))
        .child(avatar(&account.login, px(28.), FS_SM, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(FS_BASE)
                                .truncate()
                                .child(label.clone()),
                        )
                        .when(label != account.login, |this| {
                            this.child(
                                div()
                                    .text_size(FS_SM)
                                    .text_color(colors.muted)
                                    .child(format!("@{}", account.login)),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(host)
                        .child(div().text_color(colors.border_strong).child("·"))
                        .child(Icon::new(method_icon).size(px(12.)).color(colors.muted))
                        .child(method_text),
                ),
        )
        .child(status_view(status, &id, cx))
        .child(actions)
        .into_any_element()
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let settings = view.settings(cx);
    let mut page = div().flex().flex_col();
    page = page.child(heading(
        "Accounts",
        "Sign in to GitHub, GitLab, Gitea / Forgejo, Bitbucket or Azure DevOps, on public or self-hosted instances. Used for private clones, HTTPS push and pull, pull requests and CI status.",
    ));
    if let Some(error) = &view.accounts_error {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Error, "The system keychain cannot be used")
                    .body(error.clone()),
            ),
        );
    }

    let add = {
        let entity = cx.entity();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.open_add_account(window, cx));
        }
    };

    let count = settings.accounts.len();
    let mut list = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .rounded(R_MD)
        .overflow_hidden();
    if settings.accounts.is_empty() {
        list = list.child(
            EmptyState::new("No accounts yet")
                .icon(IconName::Cloud)
                .text("Accounts let GitWarp clone private repositories, push and pull over HTTPS, and show pull requests and CI status. You can add several, even for the same service.")
                .actions(
                    Button::primary("settings-accounts-add-empty")
                        .label("Add account")
                        .icon(IconName::Plus)
                        .on_click(add.clone()),
                ),
        );
    } else {
        for (index, account) in settings.accounts.iter().enumerate() {
            let status = view
                .account_status
                .get(&account.id)
                .cloned()
                .unwrap_or(AccountStatus::Checking);
            let row = account_row(account, &status, cx);
            list = list.child(
                div()
                    .when(index + 1 != count, |this| {
                        this.border_b_1().border_color(colors.border)
                    })
                    .child(row),
            );
        }
    }

    page = page.child(
        div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(FS_XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if count > 0 {
                                format!("ACCOUNTS · {count}")
                            } else {
                                "ACCOUNTS".to_owned()
                            }),
                    )
                    .when(count > 0, |this| {
                        this.child(
                            Button::primary("settings-accounts-add")
                                .label("Add account")
                                .icon(IconName::Plus)
                                .size(ButtonSize::Small)
                                .on_click(add),
                        )
                    }),
            )
            .child(list)
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .mt(px(2.))
                            .child(Icon::new(IconName::Lock).size(px(14.)).color(colors.muted)),
                    )
                    .child("Tokens are kept in the system keychain (libsecret on Linux, Credential Manager on Windows, Keychain on macOS), not in settings.json."),
            ),
    );
    page.into_any_element()
}

/* ------------------------- rename / remove dialogs ------------------------ */

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptKind {
    Rename,
    Remove,
}

pub(crate) struct AccountPrompt {
    kind: PromptKind,
    account: AccountRecord,
    input: Entity<TextInput>,
    _input_events: Subscription,
    layer: Option<overlay::LayerId>,
    _host: Option<Entity<DialogHost>>,
    view: WeakEntity<SettingsView>,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
}

fn open_prompt(
    kind: PromptKind,
    account: AccountRecord,
    view: WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AccountPrompt> {
    let prompt = cx.new(|cx| {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(account.login.clone(), cx);
            input.set_text(account.display_label().to_owned(), cx);
            input
        });
        let events = cx.subscribe(
            &input,
            |_: &mut AccountPrompt, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(_) = event {
                    cx.notify();
                }
            },
        );
        AccountPrompt {
            kind,
            account,
            input,
            _input_events: events,
            layer: None,
            _host: None,
            view,
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    });
    let state = prompt.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build_prompt(&state, cx),
        None => Dialog::new("Account"),
    });
    prompt.update(cx, |prompt, _| {
        prompt.layer = Some(layer);
        prompt._host = Some(host);
    });
    prompt
}

impl AccountPrompt {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn label(&self, cx: &App) -> String {
        self.input.read(cx).text().trim().to_owned()
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.kind {
            PromptKind::Rename => {
                let label = self.label(cx);
                if label.is_empty() {
                    return;
                }
                let id = self.account.id.clone();
                crate::update_settings(cx, |settings| {
                    if let Some(account) = settings.accounts.iter_mut().find(|a| a.id == id) {
                        account.label = label;
                    }
                });
                if let Some(view) = self.view.upgrade() {
                    view.update(cx, |_, cx| cx.notify());
                }
                self.close(window, cx);
            }
            PromptKind::Remove => {
                let account = self.account.clone();
                self.close(window, cx);
                if let Some(view) = self.view.upgrade() {
                    view.update(cx, |view, cx| view.remove_account(&account, cx));
                }
            }
        }
    }
}

fn build_prompt(state: &Entity<AccountPrompt>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let prompt = state.read(cx);
    let account = &prompt.account;
    let kind = logic::kind_of(account).unwrap_or(ServiceKind::GitHub);
    let host = logic::host_of(&account.base_url);
    let remove = prompt.kind == PromptKind::Remove;
    let empty = prompt.label(cx).is_empty();

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |prompt, cx| prompt.close(window, cx));
        }
    };
    let confirm = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |prompt, cx| prompt.confirm(window, cx));
        }
    };

    let body: AnyElement = if remove {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .rounded(R_MD)
                    .bg(colors.bg)
                    .child(service_icon(kind, cx))
                    .child(avatar(&account.login, px(28.), FS_SM, cx))
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(account.display_label().to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(FS_SM)
                                    .text_color(colors.muted)
                                    .child(host.clone()),
                            ),
                    ),
            )
            .child("GitWarp forgets this account and deletes its token from the system keychain. Remotes that used it will ask you to pick another account.")
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(format!(
                        "The token stays valid on {host} until you revoke it there."
                    )),
            )
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(FS_SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Label"),
            )
            .child(div().w_full().h(px(26.)).child(prompt.input.clone()))
            .child(if empty {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(FS_SM)
                    .text_color(colors.error)
                    .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                    .child("A label cannot be empty.")
                    .into_any_element()
            } else {
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(format!(
                        "Shown in clone, push and pull-request menus. The login name stays {}.",
                        account.login
                    ))
                    .into_any_element()
            })
            .into_any_element()
    };

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("account-prompt-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .focus_handle(prompt.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            if remove {
                Button::danger("account-prompt-confirm").label("Remove account")
            } else {
                Button::primary("account-prompt-confirm")
                    .label("Save")
                    .disabled(empty)
            }
            .min_width(px(96.))
            .focus_handle(prompt.primary_focus.clone())
            .on_click(confirm),
        );

    let mut order = Vec::new();
    if !remove {
        order.push(DialogFocus::input(
            prompt.input.read(cx).focus_handle().clone(),
        ));
    }
    order.push(DialogFocus::control(prompt.cancel_focus.clone()));
    order.push(DialogFocus::control(prompt.primary_focus.clone()));

    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |prompt, cx| prompt.confirm(window, cx));
        }
    };
    Dialog::new(if remove {
        "Remove account"
    } else {
        "Rename label"
    })
    .icon(if remove {
        IconName::Trash
    } else {
        IconName::Edit
    })
    .width(if remove { px(460.) } else { px(440.) })
    .compact_body()
    .body(body)
    .footer(footer)
    .focus_order(order)
    .autofocus(if remove {
        prompt.cancel_focus.clone()
    } else {
        prompt.input.read(cx).focus_handle().clone()
    })
    .default_button(prompt.primary_focus.clone())
    .default_danger(remove)
    .on_default(on_default)
}
