//! Settings → Accounts: the hosting-service accounts, their live status, and
//! the Rename and Remove dialogs. Add and Reconnect live in `account_dialog`.
//!
//! On opening the page every account is checked in the background against its
//! service (`forge::api::current_user`); rows show "Checking…" until the
//! answer arrives.

use std::rc::Rc;
use std::sync::atomic::Ordering;

use forge::ServiceKind;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FocusHandle,
    FontWeight, HighlightStyle, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, WeakEntity, Window,
};

use ui::actions::Confirm;
use ui::focus::{self, RingOffset};

use ui::metrics::{FS_BASE, FS_SM, FS_XS, R_MD, R_XS};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Dialog, DialogFocus, DialogHost,
    EmptyState, Icon, IconName, TextInput, TextInputEvent, ToastKind, Tooltip,
};

use super::account_dialog;
use super::{heading, SettingsView};
use crate::accounts::{self as logic, cached_store, AccountStatus, StoreResult};
use crate::cli_import::{self, Candidate, CliError};
use crate::settings::{AccountMethod, AccountRecord};

/// The service's icon: the GitHub mark, or a monogram badge for the others.
pub(crate) fn service_icon(kind: ServiceKind, cx: &App) -> AnyElement {
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
pub(crate) fn avatar(login: &str, size: Pixels, font: Pixels, cx: &App) -> AnyElement {
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
    /// Checks every account (page opened) in the background, and looks for
    /// accounts the `gh` / `glab` CLIs are signed in to.
    pub(super) fn open_accounts(&mut self, cx: &mut Context<Self>) {
        self.start_checks(None, cx);
        self.start_cli_detect(cx);
    }

    /// Runs the CLI detection off the UI thread, once per page open. An answer
    /// that arrives after the page was left or opened again is ignored.
    fn start_cli_detect(&mut self, cx: &mut Context<Self>) {
        self.cli_detect_generation += 1;
        let generation = self.cli_detect_generation;
        self.cli_imports.clear();
        self.cli_import_rows.clear();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async { crate::cli_import::detect() })
                .await;
            this.update(cx, |view, cx| {
                if view.cli_detect_generation != generation || view.page != super::Page::Accounts {
                    return;
                }
                view.cli_imports = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Dismiss: never offer this login again.
    fn dismiss_cli_import(&mut self, candidate: &Candidate, cx: &mut Context<Self>) {
        let key = candidate.key();
        self.cli_import_rows.remove(&key);
        crate::update_settings(cx, |settings| {
            if !settings.dismissed_imports.contains(&key) {
                settings.dismissed_imports.push(key);
            }
        });
        cx.notify();
    }

    /// Import: reads the token from the tool, verifies it, saves it like the
    /// Add dialog's token path. A failure shows its reason on the row.
    fn import_cli_account(
        &mut self,
        candidate: Candidate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = candidate.key();
        if matches!(self.cli_import_rows.get(&key), Some(ImportRow::Importing)) {
            return;
        }
        self.cli_import_rows
            .insert(key.clone(), ImportRow::Importing);
        let cached = cached_store(cx);
        let window_handle = window.window_handle();
        let background = candidate.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = cached.unwrap_or_else(logic::resolve_store);
                    logic::import_cli_account(&store, &background)
                })
                .await;
            // The account is saved here, not in the view: the keychain already
            // holds the token, and closing Settings mid-import must not leave
            // it without a record.
            let outcome = match result {
                Ok(record) => {
                    let to_save = record.clone();
                    let saved = cx.update(|cx| persist_imported(cx, to_save));
                    if !saved {
                        // The same login was added meanwhile: drop the copy just stored.
                        let id = record.id.clone();
                        let cached = cx.update(|cx| cached_store(cx));
                        cx.background_executor()
                            .spawn(async move {
                                if let Ok(store) = cached.unwrap_or_else(logic::resolve_store) {
                                    let _ = logic::forget_secret(store.as_ref(), &id);
                                }
                            })
                            .await;
                    }
                    Ok((record, saved))
                }
                Err(failure) => Err(failure),
            };
            window_handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |view, cx| {
                        view.cli_import_done(candidate, outcome, window, cx)
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Row state, status and toast for a finished import; the account itself
    /// was already saved by [`persist_imported`].
    fn cli_import_done(
        &mut self,
        candidate: Candidate,
        result: Result<(AccountRecord, bool), logic::ImportFailure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = candidate.key();
        let (record, saved) = match result {
            Ok(done) => done,
            Err(failure) => {
                let message = import_failure_text(&candidate, &failure);
                self.cli_import_rows.insert(key, ImportRow::Failed(message));
                cx.notify();
                return;
            }
        };
        self.cli_import_rows.remove(&key);
        if saved {
            let message = ui::t!(
                "settings.accounts.import.toast",
                login = record.login,
                tool = candidate.tool.name()
            );
            self.account_status
                .insert(record.id, AccountStatus::Connected);
            ui::toast::show(ToastKind::Ok, message, window, cx);
        }
        cx.notify();
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
            Some(view),
            None,
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
            Some(view),
            None,
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
        crate::update_settings(cx, |settings| {
            settings.accounts.retain(|a| a.id != id);
            settings.forget_account(&id);
        });
        let cached = cached_store(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = cached.unwrap_or_else(logic::resolve_store)?;
                    logic::forget_secret(store.as_ref(), &id)
                })
                .await;
            this.update(cx, |view, cx| {
                // The account is gone either way; say if its token may remain.
                if let Err(error) = result {
                    view.accounts_error = Some(ui::t!(
                        "settings.accounts.remove.keychain_failed",
                        error = error
                    ));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// Adds an imported account unless the same login (service, instance, login)
/// exists already. Returns whether it was added.
fn persist_imported(cx: &mut App, record: AccountRecord) -> bool {
    let duplicate = super::current_settings(cx).accounts.iter().any(|a| {
        a.kind == record.kind
            && a.base_url.eq_ignore_ascii_case(&record.base_url)
            && a.login.eq_ignore_ascii_case(&record.login)
    });
    if duplicate {
        return false;
    }
    crate::update_settings(cx, |settings| settings.accounts.push(record));
    true
}

/// The state of one Import row while it is not idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportRow {
    Importing,
    Failed(SharedString),
}

/// The reason shown on a row whose import failed. Never contains the token.
fn import_failure_text(candidate: &Candidate, failure: &logic::ImportFailure) -> SharedString {
    let tool = candidate.tool.name();
    match failure {
        logic::ImportFailure::Cli(CliError::Timeout) => {
            ui::t!("settings.accounts.import.error.timeout", tool = tool)
        }
        logic::ImportFailure::Cli(CliError::Missing) => {
            ui::t!("settings.accounts.import.error.missing", tool = tool)
        }
        logic::ImportFailure::Cli(CliError::Failed) => {
            ui::t!("settings.accounts.import.error.failed", tool = tool)
        }
        logic::ImportFailure::Cli(CliError::Empty) => {
            ui::t!("settings.accounts.import.error.empty", tool = tool)
        }
        logic::ImportFailure::Rejected => ui::t!("settings.accounts.import.error.rejected"),
        logic::ImportFailure::Scopes(scopes) => ui::t!(
            "settings.accounts.import.error.scopes",
            scopes = scopes.join(", ")
        ),
        logic::ImportFailure::Api(error) => {
            ui::t!("settings.accounts.import.error.api", error = error)
        }
        logic::ImportFailure::Keychain(error) => {
            ui::t!("settings.accounts.import.error.keychain", error = error)
        }
    }
}

/// The banner above the list: one row per login the CLIs are signed in to.
fn import_banner(
    view: &SettingsView,
    candidates: &[Candidate],
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let mut banner = div()
        .max_w(px(760.))
        .mb(px(16.))
        .px(px(12.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .border_1()
        .border_color(colors.info.opacity(0.5))
        .rounded(R_MD)
        .bg(colors.info.opacity(0.09))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .mt(px(2.))
                        .child(Icon::new(IconName::Info).size(px(16.)).color(colors.info)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(FS_BASE)
                                .child(ui::t!("settings.accounts.import.title")),
                        )
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(ui::t!("settings.accounts.import.text")),
                        ),
                ),
        );
    for (index, candidate) in candidates.iter().enumerate() {
        let key = candidate.key();
        let row_state = view.cli_import_rows.get(&key);
        let importing = matches!(row_state, Some(ImportRow::Importing));
        let import = {
            let entity = cx.entity();
            let candidate = candidate.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let candidate = candidate.clone();
                entity.update(cx, |view, cx| {
                    view.import_cli_account(candidate, window, cx)
                });
            }
        };
        let dismiss = {
            let entity = cx.entity();
            let candidate = candidate.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let candidate = candidate.clone();
                entity.update(cx, |view, cx| view.dismiss_cli_import(&candidate, cx));
            }
        };
        banner = banner.child(
            div()
                .id(ElementId::Name(format!("settings-import-{key}").into()))
                .flex()
                .flex_col()
                .py(px(6.))
                .border_t_1()
                .border_color(colors.border)
                .when(index == 0, |this| this.mt(px(6.)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(service_icon(candidate.kind, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_baseline()
                                .gap(px(6.))
                                .text_size(FS_BASE)
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(candidate.login.clone()),
                                )
                                .child(div().text_color(colors.muted).truncate().child(ui::t!(
                                    "settings.accounts.import.source",
                                    host = candidate.host.clone(),
                                    tool = candidate.tool.name()
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .flex_none()
                                .child(
                                    Button::primary(format!("settings-import-{key}-import"))
                                        .label(if importing {
                                            ui::t!("settings.accounts.import.importing")
                                        } else {
                                            ui::t!("settings.accounts.import.import")
                                        })
                                        .size(ButtonSize::Small)
                                        .disabled(importing)
                                        .on_click(import),
                                )
                                .child(
                                    Button::new(format!("settings-import-{key}-dismiss"))
                                        .label(ui::t!("settings.accounts.import.dismiss"))
                                        .size(ButtonSize::Small)
                                        .disabled(importing)
                                        .on_click(dismiss),
                                ),
                        ),
                )
                .when_some(
                    match row_state {
                        Some(ImportRow::Failed(reason)) => Some(reason.clone()),
                        _ => None,
                    },
                    |this, reason| {
                        this.child(
                            div()
                                .mt(px(4.))
                                .pl(px(30.))
                                .text_size(FS_SM)
                                .text_color(colors.error)
                                .child(reason),
                        )
                    },
                ),
        );
    }
    banner.into_any_element()
}

fn status_view(status: &AccountStatus, id: &str, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let (icon, text, color, tooltip): (Option<IconName>, SharedString, _, Option<String>) =
        match status {
            AccountStatus::Connected => (
                Some(IconName::Check),
                ui::t!("settings.accounts.status.connected"),
                colors.success,
                None,
            ),
            // The refresh token of a browser sign-in is about to end.
            AccountStatus::Expiring { days } => (
                Some(IconName::Warn),
                ui::i18n::t_count("settings.accounts.status.expiring", *days, &[]),
                colors.warning,
                None,
            ),
            AccountStatus::Checking => (
                None,
                ui::t!("settings.accounts.status.checking"),
                colors.muted,
                None,
            ),
            AccountStatus::Invalid(detail) => (
                Some(IconName::Err),
                ui::t!("settings.accounts.status.invalid"),
                colors.error,
                detail.clone(),
            ),
            AccountStatus::Unreachable(detail) => (
                Some(IconName::Err),
                ui::t!("settings.accounts.status.unreachable"),
                colors.error,
                detail.clone(),
            ),
            AccountStatus::Unchecked(why) => (
                Some(IconName::Warn),
                ui::t!("settings.accounts.status.unchecked"),
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
        AccountMethod::Token => (IconName::Key, ui::t!("settings.accounts.method.token")),
        AccountMethod::Browser => (IconName::Link, ui::t!("settings.accounts.method.browser")),
    };

    let mut actions = div().flex().items_center().gap(px(6.)).flex_none();
    match status {
        AccountStatus::Expiring { .. } => {
            let entity = cx.entity();
            let account = account.clone();
            actions = actions.child(
                Button::new(format!("settings-account-{id}-reconnect"))
                    .label(ui::t!("settings.accounts.action.reconnect"))
                    .size(ButtonSize::Small)
                    .on_click(move |_: &ClickEvent, window, cx| {
                        let account = account.clone();
                        entity.update(cx, |view, cx| view.open_reconnect(account, window, cx));
                    }),
            );
        }
        AccountStatus::Invalid(_) => {
            let entity = cx.entity();
            let account = account.clone();
            actions = actions.child(
                Button::primary(format!("settings-account-{id}-reconnect"))
                    .label(ui::t!("settings.accounts.action.reconnect"))
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
                    .label(ui::t!("settings.accounts.action.retry"))
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
                .tooltip(ui::t!("settings.accounts.tooltip.rename"))
                .on_click(rename),
        )
        .child(
            Button::ghost(format!("settings-account-{id}-profile"))
                .icon_only(IconName::Link)
                .size(ButtonSize::Small)
                .tooltip(ui::t!("settings.accounts.tooltip.profile"))
                .on_click(profile),
        )
        .child(
            Button::ghost(format!("settings-account-{id}-remove"))
                .icon_only(IconName::Trash)
                .size(ButtonSize::Small)
                .tooltip(ui::t!("settings.accounts.tooltip.remove"))
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
        // No warning-subtle theme token exists; tints elsewhere (Done card,
        // banners) are the semantic colour at low opacity.
        .when(matches!(status, AccountStatus::Expiring { .. }), |this| {
            this.bg(colors.warning.opacity(0.08))
        })
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
        ui::t!("settings.accounts.page.title"),
        ui::t!("settings.accounts.page.subtitle"),
    ));
    if let Some(error) = &view.accounts_error {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(
                    BannerKind::Error,
                    ui::t!("settings.accounts.keychain.unusable.title"),
                )
                .body(error.clone()),
            ),
        );
    }

    // Above the list, and above the empty state, as the design shows.
    let offers = cli_import::visible(
        &view.cli_imports,
        &settings.accounts,
        &settings.dismissed_imports,
    );
    if !offers.is_empty() {
        page = page.child(import_banner(view, &offers, cx));
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
            EmptyState::new(ui::t!("settings.accounts.empty.title"))
                .icon(IconName::Cloud)
                .text(ui::t!("settings.accounts.empty.text"))
                .actions(
                    Button::primary("settings-accounts-add-empty")
                        .label(ui::t!("settings.accounts.add"))
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
                                ui::i18n::t_count(
                                    "settings.accounts.list.heading",
                                    count as u64,
                                    &[],
                                )
                            } else {
                                ui::t!("settings.accounts.list.heading_empty")
                            }),
                    )
                    .when(count > 0, |this| {
                        this.child(
                            Button::primary("settings-accounts-add")
                                .label(ui::t!("settings.accounts.add"))
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
                    .child(ui::t!("settings.accounts.note.keychain")),
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
        None => Dialog::new(ui::t!("settings.accounts.prompt.fallback_title")),
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
            .child(ui::t!("settings.accounts.prompt.remove.body"))
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!("settings.accounts.prompt.remove.note", host = host)),
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
                    .child(ui::t!("settings.accounts.prompt.label")),
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
                    .child(ui::t!("settings.accounts.prompt.label_empty"))
                    .into_any_element()
            } else {
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::t!(
                        "settings.accounts.prompt.label_help",
                        login = account.login
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
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .focus_handle(prompt.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            if remove {
                Button::danger("account-prompt-confirm")
                    .label(ui::t!("settings.accounts.prompt.remove.confirm"))
            } else {
                Button::primary("account-prompt-confirm")
                    .label(ui::t!("settings.accounts.prompt.save"))
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
        ui::t!("settings.accounts.prompt.remove.title")
    } else {
        ui::t!("settings.accounts.prompt.rename.title")
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

/* --------------------- "Which account for <remote>?" --------------------- */

/// Runs when the user confirms the picker, with the chosen account.
pub(crate) type OnPicked = Rc<dyn Fn(&AccountRecord, &mut Window, &mut App)>;

/// The picker opened from a remote's menu when several accounts match its
/// host. The choice is remembered per repository by the caller.
pub(crate) struct AccountPicker {
    remote: String,
    host: String,
    accounts: Vec<AccountRecord>,
    selected: Option<String>,
    on_picked: OnPicked,
    layer: Option<overlay::LayerId>,
    _host: Option<Entity<DialogHost>>,
    row_focus: Vec<FocusHandle>,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
}

/// Opens the picker with `current` preselected.
pub(crate) fn open_picker(
    remote: String,
    host: String,
    accounts: Vec<AccountRecord>,
    current: Option<String>,
    on_picked: OnPicked,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AccountPicker> {
    let picker = cx.new(|cx| AccountPicker {
        row_focus: accounts
            .iter()
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect(),
        remote,
        host,
        selected: current.filter(|id| accounts.iter().any(|a| a.id == *id)),
        accounts,
        on_picked,
        layer: None,
        _host: None,
        cancel_focus: cx.focus_handle().tab_stop(true),
        primary_focus: cx.focus_handle().tab_stop(true),
    });
    let state = picker.downgrade();
    let (layer, host) =
        dialog::open_with_view(window, cx, move |window, cx| match state.upgrade() {
            Some(state) => build_picker(&state, window, cx),
            None => Dialog::new(ui::t!("dialogs.account_picker.fallback_title")),
        });
    picker.update(cx, |picker, _| {
        picker.layer = Some(layer);
        picker._host = Some(host);
    });
    picker
}

impl AccountPicker {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = Some(id.to_owned());
        cx.notify();
    }

    /// Use account: closes the dialog, then hands the choice to the opener.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self
            .selected
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| a.id == *id))
            .cloned()
        else {
            return;
        };
        let on_picked = self.on_picked.clone();
        self.close(window, cx);
        on_picked(&account, window, cx);
    }
}

/// The second line of a picker row: `@login · ` (only when the label hides
/// the login) and how the account signs in.
fn account_detail(account: &AccountRecord) -> String {
    let method = match account.method {
        AccountMethod::Token => ui::t!("dialogs.account_picker.method.token"),
        AccountMethod::Browser => ui::t!("dialogs.account_picker.method.browser"),
    };
    if account.display_label() == account.login {
        method.to_string()
    } else {
        ui::t!(
            "dialogs.account_picker.detail",
            login = account.login,
            method = method
        )
        .to_string()
    }
}

fn build_picker(state: &Entity<AccountPicker>, window: &Window, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let picker = state.read(cx);

    // One translated sentence with the host in bold: the translator controls
    // the word order, so the highlight range follows the host's position.
    let mut text = String::new();
    let mut host_range = None;
    for piece in ui::i18n::t_count_pieces(
        "dialogs.account_picker.intro",
        picker.accounts.len() as u64,
        &[("remote", &picker.remote)],
        &["host"],
    ) {
        match piece {
            ui::i18n::Piece::Text(part) => text.push_str(&part),
            ui::i18n::Piece::Slot(_) => {
                host_range = Some(text.len()..text.len() + picker.host.len());
                text.push_str(&picker.host);
            }
        }
    }
    let intro =
        StyledText::new(SharedString::from(text)).with_highlights(host_range.map(|range| {
            (
                range,
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            )
        }));

    let mut list = div().flex().flex_col().gap(px(4.));
    for (index, account) in picker.accounts.iter().enumerate() {
        let selected = picker.selected.as_deref() == Some(account.id.as_str());
        let handle = picker.row_focus[index].clone();
        let ring = focus::focus_visible(&handle, window, cx);
        let kind = logic::kind_of(account).unwrap_or(ServiceKind::GitHub);
        let id = account.id.clone();
        let choose = {
            let state = state.clone();
            let id = id.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                state.update(cx, |picker, cx| picker.select(&id, cx));
            }
        };
        let choose_key = {
            let state = state.clone();
            move |_: &Confirm, _: &mut Window, cx: &mut App| {
                state.update(cx, |picker, cx| picker.select(&id, cx));
            }
        };
        list = list.child(
            div()
                .id(ElementId::named_usize("account-picker-row", index))
                .relative()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(8.))
                .rounded(R_MD)
                .border_1()
                .border_color(if selected {
                    colors.accent
                } else {
                    colors.border_strong
                })
                .bg(if selected {
                    colors.accent_soft
                } else {
                    colors.bg
                })
                .text_color(colors.text)
                .cursor_pointer()
                .when(!selected, |this| {
                    let hover = colors.hover;
                    this.hover(move |style| style.bg(hover))
                })
                .track_focus(&handle)
                .on_click(choose)
                .on_action(choose_key)
                .child(service_icon(kind, cx))
                .child(avatar(&account.login, px(28.), FS_SM, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(account.display_label().to_owned()),
                        )
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .truncate()
                                .child(account_detail(account)),
                        ),
                )
                // The account's status is not known here, so the design's status
                // column is skipped; only the check of the selected row is drawn.
                .child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .flex_none()
                        .when(selected, |this| {
                            this.child(Icon::new(IconName::Check).size(px(14.)).color(colors.text))
                        }),
                )
                .when(ring, |this| {
                    this.child(focus::focus_ring(RingOffset::Outset1, R_MD, cx))
                }),
        );
    }

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(intro)
        .child(list)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(6.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(
                    div()
                        .mt(px(2.))
                        .child(Icon::new(IconName::Info).size(px(12.)).color(colors.muted)),
                )
                .child(ui::t!("dialogs.account_picker.help")),
        );

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |picker, cx| picker.close(window, cx));
        }
    };
    let confirm = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |picker, cx| picker.confirm(window, cx));
        }
    };
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("account-picker-cancel")
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .focus_handle(picker.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("account-picker-use")
                .label(ui::t!("dialogs.account_picker.use"))
                .min_width(px(96.))
                .disabled(picker.selected.is_none())
                .focus_handle(picker.primary_focus.clone())
                .on_click(confirm),
        );

    let mut order: Vec<DialogFocus> = picker
        .row_focus
        .iter()
        .map(|handle| DialogFocus::control(handle.clone()))
        .collect();
    order.push(DialogFocus::control(picker.cancel_focus.clone()));
    order.push(DialogFocus::control(picker.primary_focus.clone()));

    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |picker, cx| picker.confirm(window, cx));
        }
    };
    let first_row = picker
        .row_focus
        .first()
        .cloned()
        .unwrap_or_else(|| picker.cancel_focus.clone());
    Dialog::new(ui::t!(
        "dialogs.account_picker.title",
        remote = picker.remote
    ))
    .icon(IconName::User)
    .width(px(500.))
    .compact_body()
    .body(body)
    .footer(footer)
    .focus_order(order)
    .autofocus(first_row)
    .default_button(picker.primary_focus.clone())
    .on_default(on_default)
}

#[cfg(test)]
mod picker_tests {
    use super::*;

    fn record(login: &str, label: &str, method: AccountMethod) -> AccountRecord {
        AccountRecord {
            id: "a".into(),
            kind: "github".into(),
            base_url: "https://github.com".into(),
            login: login.into(),
            label: label.into(),
            method,
            ssh_host: None,
            imported_from: None,
        }
    }

    #[test]
    fn the_detail_line_shows_the_login_only_when_the_label_differs() {
        assert_eq!(
            account_detail(&record("mira", "mira", AccountMethod::Token)),
            "Token"
        );
        assert_eq!(
            account_detail(&record("mira-k", "Acme work", AccountMethod::Browser)),
            "@mira-k · Browser sign-in"
        );
    }
}
