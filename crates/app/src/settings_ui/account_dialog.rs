//! The Add / Reconnect account dialog (token sign-in only).
//!
//! Steps: service, instance (with a probe for self-hosted addresses), sign-in
//! with a token, then the result with an optional label. Reconnect opens on the
//! sign-in step with the service and instance locked. The browser sign-in is
//! drawn disabled: OAuth lands in a later step.
//!
//! The pure rules (step order, button gating, probe and token result mapping,
//! the label rule) live in `crate::accounts` and are tested there.

use std::rc::Rc;

use forge::api::{Client, TokenKind};
use forge::ServiceKind;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use ui::actions::Confirm;
use ui::focus::{self, RingOffset};
use ui::metrics::{FONT_MONO, FS_BASE, FS_MD, FS_SM, FS_XS, R_MD, R_SM};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Dialog, DialogFocus, DialogHost, Icon,
    IconName, TextInput, TextInputEvent, ToastKind, Tooltip,
};

use super::accounts::{avatar, service_icon};
use super::SettingsView;
use crate::accounts::{
    self as logic, cached_store, continue_enabled, continue_label, instance_from_input,
    probe_state, sign_in_outcome, ProbeState, SignInError, Step,
};
use crate::settings::AccountRecord;

/// What activating a card does.
type CardAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the dialog is opened for.
pub(super) enum Open {
    Add,
    /// Re-enter the token of this account.
    Reconnect(AccountRecord),
}

/// A token the service accepted, waiting for the label and Done.
type Connected = logic::Verified;

pub(crate) struct AccountDialog {
    /// The account being reconnected, `None` when adding.
    reconnect: Option<AccountRecord>,
    step: Step,
    kind: Option<ServiceKind>,
    self_hosted: bool,
    /// Accounts that exist already, for the label rule.
    existing: Vec<AccountRecord>,

    url: Entity<TextInput>,
    _url_events: Subscription,
    probe: ProbeState,
    /// Bumped when the address changes or a probe starts; a stale answer is dropped.
    probe_generation: u64,

    token: Entity<TextInput>,
    _token_events: Subscription,
    show_token: bool,
    verifying: bool,
    /// Bumped by every token check and whenever the form changes under one, so a
    /// late answer for an earlier attempt is dropped.
    connect_generation: u64,
    sign_in_error: Option<SignInError>,

    connected: Option<Connected>,
    label: Entity<TextInput>,
    _label_events: Subscription,
    label_error: bool,
    saving: bool,
    save_error: Option<SharedString>,
    /// Why the keychain cannot be used, when it is known not to work.
    store_error: Option<SharedString>,

    layer: Option<overlay::LayerId>,
    _host: Option<Entity<DialogHost>>,
    view: WeakEntity<SettingsView>,

    service_focus: Vec<FocusHandle>,
    public_focus: FocusHandle,
    self_focus: FocusHandle,
    show_focus: FocusHandle,
    create_focus: FocusHandle,
    action_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
}

/// Opens the dialog.
pub(super) fn open(
    what: Open,
    view: WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AccountDialog> {
    let existing = super::current_settings(cx).accounts;
    let store_error = match cached_store(cx) {
        Some(Err(error)) => Some(SharedString::from(error.to_string())),
        _ => None,
    };
    let dialog = cx.new(|cx| {
        let url = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("https://git.example.com", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let token = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Paste the token here", cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_masked(true, cx);
            input
        });
        let label = cx.new(|cx| TextInput::new(window, cx));
        let url_events = cx.subscribe(
            &url,
            |this: &mut AccountDialog, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(_) = event {
                    this.probe = ProbeState::Idle;
                    this.probe_generation = this.probe_generation.wrapping_add(1);
                }
                cx.notify();
            },
        );
        let token_events = cx.subscribe(
            &token,
            |this: &mut AccountDialog, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(_) = event {
                    this.sign_in_error = None;
                    // A check of the old text no longer says anything about the field.
                    this.connect_generation = this.connect_generation.wrapping_add(1);
                    this.verifying = false;
                }
                cx.notify();
            },
        );
        let label_events = cx.subscribe(
            &label,
            |this: &mut AccountDialog, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(_) = event {
                    this.label_error = false;
                }
                cx.notify();
            },
        );
        let (reconnect, step, kind, self_hosted) = match what {
            Open::Add => (None, Step::Service, None, false),
            Open::Reconnect(account) => {
                let kind = logic::kind_of(&account);
                let self_hosted = logic::instance_of(&account)
                    .map(|instance| !instance.is_public())
                    .unwrap_or(true);
                (Some(account), Step::SignIn, kind, self_hosted)
            }
        };
        AccountDialog {
            reconnect,
            step,
            kind,
            self_hosted,
            existing,
            url,
            _url_events: url_events,
            probe: ProbeState::Idle,
            probe_generation: 0,
            token,
            _token_events: token_events,
            show_token: false,
            verifying: false,
            connect_generation: 0,
            sign_in_error: None,
            connected: None,
            label,
            _label_events: label_events,
            label_error: false,
            saving: false,
            save_error: None,
            store_error,
            layer: None,
            _host: None,
            view,
            service_focus: ServiceKind::ALL
                .iter()
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            public_focus: cx.focus_handle().tab_stop(true),
            self_focus: cx.focus_handle().tab_stop(true),
            show_focus: cx.focus_handle().tab_stop(true),
            create_focus: cx.focus_handle().tab_stop(true),
            action_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    });
    let state = dialog.downgrade();
    let (layer, host) =
        dialog::open_with_view(window, cx, move |window, cx| match state.upgrade() {
            Some(state) => build(&state, window, cx),
            None => Dialog::new("Add account"),
        });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog._host = Some(host);
        // Reconnect opens on the token field.
        if dialog.step == Step::SignIn {
            let handle = dialog.token.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
    });
    dialog
}

impl AccountDialog {
    fn reconnecting(&self) -> bool {
        self.reconnect.is_some()
    }

    /// The instance base URL the chosen service and address point at.
    fn base_url(&self, cx: &App) -> Option<String> {
        if let Some(account) = &self.reconnect {
            return Some(account.base_url.clone());
        }
        let kind = self.kind?;
        if self.self_hosted {
            instance_from_input(kind, self.url.read(cx).text()).map(|i| i.base_url)
        } else {
            Some(format!("https://{}", logic::public_host(kind)))
        }
    }

    /// The instance of the verified token once there is one, else the form's.
    fn shown_base_url(&self, cx: &App) -> Option<String> {
        match &self.connected {
            Some(verified) => Some(verified.base_url.clone()),
            None => self.base_url(cx),
        }
    }

    /// `host[:port]` for display; the typed text while the address is not valid yet.
    fn host_text(&self, cx: &App) -> String {
        match self.shown_base_url(cx) {
            Some(base) => logic::host_of(&base),
            None => self.url.read(cx).text().trim().to_owned(),
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Answers arriving after this are for a dialog that is gone.
        self.connect_generation = self.connect_generation.wrapping_add(1);
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn pick_service(&mut self, kind: ServiceKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.verifying || self.saving {
            return;
        }
        self.kind = Some(kind);
        self.self_hosted = false;
        self.probe = ProbeState::Idle;
        self.step = Step::Instance;
        window.focus(&self.public_focus, cx);
        cx.notify();
    }

    fn choose_instance(&mut self, self_hosted: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.verifying || self.saving {
            return;
        }
        self.self_hosted = self_hosted;
        self.probe = ProbeState::Idle;
        self.probe_generation = self.probe_generation.wrapping_add(1);
        if self_hosted {
            let handle = self.url.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn switch_service(&mut self, kind: ServiceKind, cx: &mut Context<Self>) {
        if self.verifying || self.saving {
            return;
        }
        self.kind = Some(kind);
        self.probe = ProbeState::Detected { version: None };
        cx.notify();
    }

    fn go_to_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::SignIn;
        let handle = self.token.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The primary button of the first two steps.
    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Service => {
                if let Some(kind) = self.kind {
                    self.pick_service(kind, window, cx);
                }
            }
            Step::Instance => {
                let address_valid = self.base_url(cx).is_some();
                if !continue_enabled(!self.self_hosted, address_valid, &self.probe) {
                    return;
                }
                if !self.self_hosted || matches!(self.probe, ProbeState::Detected { .. }) {
                    self.go_to_sign_in(window, cx);
                } else {
                    self.run_probe(cx);
                }
            }
            Step::SignIn | Step::Done => {}
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.verifying || self.saving {
            return;
        }
        self.sign_in_error = None;
        self.step = self.step.back();
        if self.step == Step::Instance && self.self_hosted {
            let handle = self.url.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn run_probe(&mut self, cx: &mut Context<Self>) {
        let (Some(kind), Some(base_url)) = (self.kind, self.base_url(cx)) else {
            return;
        };
        self.probe = ProbeState::Checking;
        self.probe_generation = self.probe_generation.wrapping_add(1);
        let generation = self.probe_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { forge::api::probe(&Client::new(), &base_url) })
                .await;
            this.update(cx, |dialog, cx| {
                if generation != dialog.probe_generation {
                    return;
                }
                dialog.probe = probe_state(kind, result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn token_text(&self, cx: &App) -> String {
        self.token.read(cx).text().trim().to_owned()
    }

    fn can_connect(&self, cx: &App) -> bool {
        !self.token_text(cx).is_empty() && !self.verifying && self.store_error.is_none()
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_connect(cx) {
            return;
        }
        let (Some(kind), Some(base_url)) = (self.kind, self.base_url(cx)) else {
            return;
        };
        let token = self.token_text(cx);
        let mut instance = match forge::Instance::new(kind, &base_url) {
            Ok(instance) => instance,
            Err(error) => {
                self.sign_in_error = Some(SignInError::Other(error.to_string()));
                cx.notify();
                return;
            }
        };
        if let Some(account) = &self.reconnect {
            instance.ssh_host = account.ssh_host.clone();
        }
        self.verifying = true;
        self.sign_in_error = None;
        self.connect_generation = self.connect_generation.wrapping_add(1);
        let attempt = self.connect_generation;
        // What the check runs against; the result is saved with exactly this.
        let checked = (kind, instance.base_url.clone(), instance.ssh_host.clone());
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let checked_token = token.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    // GitLab personal access tokens use PRIVATE-TOKEN; OAuth comes later.
                    forge::api::current_user(
                        &Client::new(),
                        &instance,
                        &checked_token,
                        TokenKind::Pat,
                    )
                })
                .await;
            window_handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |dialog, cx| {
                        let open = dialog.layer.is_some();
                        if !logic::attempt_is_current(attempt, dialog.connect_generation, open) {
                            return;
                        }
                        dialog.verifying = false;
                        match sign_in_outcome(kind, result) {
                            Ok(login) => {
                                let (kind, base_url, ssh_host) = checked;
                                let verified = Connected {
                                    kind,
                                    base_url,
                                    ssh_host,
                                    login,
                                    token,
                                };
                                dialog.show_result(verified, window, cx)
                            }
                            Err(error) => dialog.sign_in_error = Some(error),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Moves to the result step. A second account on the same instance has to
    /// be told apart by a label, so the field is required then.
    fn show_result(&mut self, verified: Connected, window: &mut Window, cx: &mut Context<Self>) {
        let login = verified.login.clone();
        self.connected = Some(verified);
        let required = self.label_required(cx);
        let placeholder = if required {
            "e.g. Personal, Acme work".to_owned()
        } else {
            login.clone()
        };
        self.label
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        self.step = Step::Done;
        if !self.reconnecting() {
            let handle = self.label.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
    }

    fn label_required(&self, cx: &App) -> bool {
        !self.reconnecting()
            && self
                .shown_base_url(cx)
                .is_some_and(|base| logic::needs_label(&self.existing, &base))
    }

    /// Done: stores the token in the keychain, then records the account.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let label = self.label.read(cx).text().trim().to_owned();
        if self.label_required(cx) && label.is_empty() {
            self.label_error = true;
            cx.notify();
            return;
        }
        let Some(connected) = self.connected.as_ref() else {
            return;
        };
        let token = connected.token.clone();
        let id = match &self.reconnect {
            Some(account) => account.id.clone(),
            None => logic::new_account_id(),
        };
        let reconnect = self.reconnect.is_some();
        if reconnect && !self.account_exists(&id, cx) {
            self.account_removed(window, cx);
            return;
        }
        // The record comes from the verified result, not from the form.
        let record = connected.record(id.clone(), &label);
        let cached = cached_store(cx);
        self.saving = true;
        self.save_error = None;
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = cached.unwrap_or_else(logic::resolve_store)?;
                    store.set(&id, &token)
                })
                .await;
            window_handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |dialog, cx| {
                        dialog.saved(
                            result.map_err(|e| e.to_string()),
                            record,
                            reconnect,
                            window,
                            cx,
                        )
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn saved(
        &mut self,
        result: Result<(), String>,
        record: AccountRecord,
        reconnect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        if let Err(error) = result {
            self.save_error = Some(error.into());
            cx.notify();
            return;
        }
        if reconnect && !self.account_exists(&record.id, cx) {
            // Removed while the token was being stored: do not leave it behind.
            let id = record.id.clone();
            let cached = cached_store(cx);
            cx.background_executor()
                .spawn(async move {
                    if let Ok(store) = cached.unwrap_or_else(logic::resolve_store) {
                        let _ = store.delete(&id);
                    }
                })
                .detach();
            self.account_removed(window, cx);
            return;
        }
        let id = record.id.clone();
        let message = if reconnect {
            format!("Reconnected {}", record.display_label())
        } else {
            let name = logic::kind_of(&record).map_or("", ServiceKind::display_name);
            format!("Connected {name} account {}", record.display_label())
        };
        crate::update_settings(cx, |settings| {
            match settings.accounts.iter_mut().find(|a| a.id == record.id) {
                // Reconnect keeps the label and the SSH host; the login follows the token.
                Some(existing) => existing.login = record.login.clone(),
                None => settings.accounts.push(record.clone()),
            }
        });
        if let Some(view) = self.view.upgrade() {
            view.update(cx, |view, cx| {
                view.account_status
                    .insert(id, crate::accounts::AccountStatus::Connected);
                cx.notify();
            });
        }
        self.close(window, cx);
        ui::toast::show(ToastKind::Ok, message, window, cx);
    }

    fn account_exists(&self, id: &str, cx: &App) -> bool {
        super::current_settings(cx)
            .accounts
            .iter()
            .any(|a| a.id == id)
    }

    /// The account being reconnected was removed meanwhile: nothing is saved.
    fn account_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.view.upgrade() {
            view.update(cx, |view, cx| {
                view.accounts_error = Some("This account was removed".into());
                cx.notify();
            });
        }
        self.close(window, cx);
    }

    fn default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Service | Step::Instance => self.next(window, cx),
            Step::SignIn => self.connect(window, cx),
            Step::Done => self.finish(window, cx),
        }
    }
}

/// A click handler that runs `f` on the dialog.
fn act(
    state: &Entity<AccountDialog>,
    f: impl Fn(&mut AccountDialog, &mut Window, &mut Context<AccountDialog>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, window, cx| state.update(cx, |dialog, cx| f(dialog, window, cx))
}

/// A selectable card: service icon, title and description. Focusable, and
/// Enter activates it like a button. `on_click` is `None` for a disabled card.
#[allow(clippy::too_many_arguments)]
fn card(
    id: impl Into<ElementId>,
    handle: Option<&FocusHandle>,
    selected: bool,
    icon: AnyElement,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    on_click: Option<CardAction>,
    window: &Window,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let colors = cx.theme().colors;
    let disabled = on_click.is_none();
    let ring = handle.is_some_and(|h| focus::focus_visible(h, window, cx));
    let mut card = div()
        .id(id)
        .relative()
        .w_full()
        .min_h(px(52.))
        .px(px(12.))
        .py(px(10.))
        .flex()
        .gap(px(10.))
        .rounded(R_MD)
        .border_1()
        .text_size(FS_BASE)
        .when(!disabled, |this| {
            this.bg(colors.bg)
                .border_color(colors.border_strong)
                .text_color(colors.text)
                .cursor_pointer()
                .hover(move |style| style.bg(colors.hover))
        })
        .when(disabled, |this| {
            this.border_color(colors.border)
                .text_color(colors.muted)
                .cursor_not_allowed()
        })
        .when(selected, |this| {
            this.border_color(colors.accent).bg(colors.accent_soft)
        })
        .when_some(handle, |this, handle| this.track_focus(handle))
        .child(
            div()
                .w(px(20.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(px(1.))
                .child(icon),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(description.into()),
                ),
        );
    if let Some(on_click) = on_click {
        let on_enter = on_click.clone();
        card = card
            .on_click(move |_: &ClickEvent, window, cx| on_click(window, cx))
            .on_action(move |_: &Confirm, window, cx| on_enter(window, cx));
    }
    if ring {
        card = card.child(focus::focus_ring(RingOffset::Outset1, R_MD, cx));
    }
    card
}

fn field_label(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(FS_SM)
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

fn help_text(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .text_size(FS_SM)
        .text_color(cx.theme().colors.muted)
        .child(text.into())
}

fn stepper(current: usize, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(FS_SM)
        .text_color(colors.muted);
    for (index, name) in ["Service", "Instance", "Sign in"].into_iter().enumerate() {
        let number = index + 1;
        let now = number == current;
        let done = number < current;
        let circle = div()
            .w(px(18.))
            .h(px(18.))
            .rounded(px(9.))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(FS_XS)
            .font_weight(FontWeight::SEMIBOLD)
            .map(|this| {
                if now {
                    this.bg(colors.accent)
                        .border_color(colors.accent)
                        .text_color(colors.accent_fg)
                } else if done {
                    this.bg(colors.success)
                        .border_color(colors.success)
                        .text_color(colors.accent_fg)
                } else {
                    this.border_color(colors.border_strong)
                }
            })
            .child(if done {
                Icon::new(IconName::Check)
                    .size(px(10.))
                    .color(colors.accent_fg)
                    .into_any_element()
            } else {
                number.to_string().into_any_element()
            });
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when(now, |this| {
                    this.text_color(colors.text).font_weight(FontWeight::MEDIUM)
                })
                .child(circle)
                .child(name),
        );
        if index < 2 {
            row = row.child(
                Icon::new(IconName::ChevR)
                    .size(px(12.))
                    .color(colors.border_strong),
            );
        }
    }
    row.into_any_element()
}

/// Service and instance of a reconnected account, locked.
fn lock_line(kind: ServiceKind, host: &str, login: &str, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(kind.display_name()),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(format!("{host} · {login}")),
                ),
        )
        .child(Icon::new(IconName::Lock).size(px(14.)).color(colors.muted))
        .into_any_element()
}

/// Warns when the token would travel over plain HTTP. Connecting stays allowed.
fn http_warning(base_url: Option<&str>) -> Option<AnyElement> {
    base_url.filter(|url| logic::is_plain_http(url)).map(|_| {
        Banner::new(
            BannerKind::Warn,
            "This address uses plain HTTP — the token would be sent unencrypted.",
        )
        .into_any_element()
    })
}

fn probe_banner(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    kind: ServiceKind,
    host: &str,
) -> Option<AnyElement> {
    let name = kind.display_name();
    let public = !dialog.self_hosted;
    match &dialog.probe {
        ProbeState::Idle if public => Some(
            Banner::new(BannerKind::Ok, format!("{name} · {host}"))
                .body("Public instance, reachable.")
                .into_any_element(),
        ),
        ProbeState::Idle => None,
        ProbeState::Checking => Some(
            Banner::new(BannerKind::Info, format!("Checking {host}…"))
                .body("Looking for the service type and API version.")
                .into_any_element(),
        ),
        ProbeState::Detected { version } => Some(
            Banner::new(
                BannerKind::Ok,
                match version {
                    Some(version) => format!("Detected: {name} {version}"),
                    None => format!("Detected: {name}"),
                },
            )
            .body("The address is reachable.")
            .into_any_element(),
        ),
        ProbeState::Other(other) => {
            let other = *other;
            Some(
                Banner::new(
                    BannerKind::Warn,
                    format!("{host} is a {} server, not {name}", other.display_name()),
                )
                .body(format!(
                    "The address answered like {}. Switch the service type to keep going.",
                    other.display_name()
                ))
                .action(
                    Button::primary("account-switch-service")
                        .label(format!("Switch to {}", other.display_name()))
                        .size(ButtonSize::Small)
                        .focus_handle(dialog.action_focus.clone())
                        .on_click(act(state, move |d, _, cx| d.switch_service(other, cx))),
                )
                .into_any_element(),
            )
        }
        ProbeState::Unreachable(text) => Some(
            Banner::new(BannerKind::Error, format!("Can’t reach {host}"))
                .body(format!(
                    "Check the address, your network or VPN, or the proxy settings. {text}"
                ))
                .action(
                    Button::new("account-probe-retry")
                        .label("Try again")
                        .icon(IconName::Fetch)
                        .size(ButtonSize::Small)
                        .focus_handle(dialog.action_focus.clone())
                        .on_click(act(state, |d, _, cx| d.run_probe(cx))),
                )
                .into_any_element(),
        ),
        // The "Trust for this instance" action is not part of this step.
        ProbeState::Tls(text) => Some(
            Banner::new(
                BannerKind::Warn,
                format!("The certificate of {host} is not trusted"),
            )
            .body(text.clone())
            .into_any_element(),
        ),
        ProbeState::NotRecognized => Some(
            Banner::new(
                BannerKind::Error,
                format!("No supported service found at {host}"),
            )
            .body("Check that this is the web address of the service, without a path.")
            .into_any_element(),
        ),
    }
}

fn sign_in_banner(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    kind: ServiceKind,
    host: &str,
    cx: &App,
) -> Option<AnyElement> {
    let colors = cx.theme().colors;
    let name = kind.display_name();
    match dialog.sign_in_error.as_ref()? {
        SignInError::Wrong => Some(
            Banner::new(BannerKind::Error, "That token was rejected")
                .body(format!(
                    "{name} answered 401 Bad credentials. Check that you pasted the whole token and that it has not been revoked or expired."
                ))
                .into_any_element(),
        ),
        SignInError::Permissions(missing) => {
            let mut chips = div().flex().flex_wrap().gap(px(6.)).mt(px(4.));
            for permission in missing {
                chips = chips.child(permission_chip(permission, Some(colors.error), cx));
            }
            Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .p(px(10.))
                    .pl(px(12.))
                    .border_1()
                    .border_color(colors.error.opacity(0.55))
                    .rounded(R_MD)
                    .bg(colors.error.opacity(0.10))
                    .child(
                        div()
                            .mt(px(2.))
                            .child(Icon::new(IconName::Err).size(px(16.)).color(colors.error)),
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
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("The token is missing permissions"),
                            )
                            .child(div().text_color(colors.muted).child(format!(
                                "{name} accepted the token, but it cannot do everything GitWarp needs. Create a new token that includes:"
                            )))
                            .child(chips),
                    )
                    .into_any_element(),
            )
        }
        SignInError::Network(text) => Some(
            Banner::new(BannerKind::Error, format!("Can’t reach {host}"))
                .body(format!(
                    "Check your network connection and try again. {text}"
                ))
                .action(
                    Button::new("account-connect-retry")
                        .label("Try again")
                        .icon(IconName::Fetch)
                        .size(ButtonSize::Small)
                        .focus_handle(dialog.action_focus.clone())
                        .on_click(act(state, |d, window, cx| d.connect(window, cx))),
                )
                .into_any_element(),
        ),
        SignInError::Other(text) => Some(
            Banner::new(BannerKind::Error, "Could not check the token")
                .body(text.clone())
                .into_any_element(),
        ),
    }
}

/// A permission name in a bordered mono chip.
fn permission_chip(text: &str, color: Option<gpui::Hsla>, cx: &App) -> gpui::Div {
    let colors = cx.theme().colors;
    div()
        .px(px(6.))
        .py(px(1.))
        .border_1()
        .border_color(color.unwrap_or(colors.border_strong))
        .rounded(R_SM)
        .bg(colors.bg)
        .font_family(FONT_MONO)
        .text_size(FS_SM)
        .text_color(color.unwrap_or(colors.text))
        .child(text.to_owned())
}

fn service_step(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let cards: Vec<AnyElement> = ServiceKind::ALL
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            let pick = {
                let state = state.clone();
                Rc::new(move |window: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| dialog.pick_service(kind, window, cx));
                }) as CardAction
            };
            card(
                format!("account-service-{}", kind.id()),
                Some(&dialog.service_focus[index]),
                dialog.kind == Some(kind),
                service_icon(kind, cx),
                kind.display_name(),
                logic::service_blurb(kind),
                Some(pick),
                window,
                cx,
            )
            .into_any_element()
        })
        .collect();
    let mut cards = cards.into_iter();
    let mut grid = div().flex().flex_col().gap(px(8.));
    while let Some(first) = cards.next() {
        let mut row = div()
            .flex()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(first));
        if let Some(second) = cards.next() {
            row = row.child(div().flex_1().min_w_0().child(second));
        } else {
            row = row.child(div().flex_1().min_w_0());
        }
        grid = grid.child(row);
    }
    grid.into_any_element()
}

fn instance_step(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    kind: ServiceKind,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let choose = |self_hosted: bool| {
        let state = state.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.choose_instance(self_hosted, window, cx)
            });
        }) as CardAction
    };
    let public_card = card(
        "account-instance-public",
        Some(&dialog.public_focus),
        !dialog.self_hosted,
        service_icon(kind, cx),
        logic::public_host(kind),
        "Public instance",
        Some(choose(false)),
        window,
        cx,
    );
    let self_card = card(
        "account-instance-self",
        Some(&dialog.self_focus),
        dialog.self_hosted,
        Icon::new(IconName::Cloud)
            .size(px(16.))
            .color(colors.text)
            .into_any_element(),
        "Self-hosted",
        format!(
            "{} on your own domain or intranet",
            logic::self_hosted_name(kind)
        ),
        Some(choose(true)),
        window,
        cx,
    );
    let url_text = dialog.url.read(cx).text().trim().to_owned();
    let url_invalid = dialog.self_hosted
        && !url_text.is_empty()
        && instance_from_input(kind, &url_text).is_none();
    let host = dialog.host_text(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(public_card)
                .child(self_card),
        )
        .when(dialog.self_hosted, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field_label("Instance URL"))
                    .child(div().w_full().h(px(26.)).child(dialog.url.clone()))
                    .child(if url_invalid {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(FS_SM)
                            .text_color(colors.error)
                            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                            .child("Enter a web address like https://git.example.com.")
                            .into_any_element()
                    } else {
                        help_text("The address you open in the browser, without a path.", cx)
                            .into_any_element()
                    }),
            )
        })
        .when_some(
            http_warning(
                dialog
                    .base_url(cx)
                    .as_deref()
                    .filter(|_| dialog.self_hosted),
            ),
            |this, banner| this.child(banner),
        )
        .when_some(probe_banner(state, dialog, kind, &host), |this, banner| {
            this.child(banner)
        })
        .into_any_element()
}

fn sign_in_step(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    kind: ServiceKind,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let host = dialog.host_text(cx);
    let base_url = dialog.base_url(cx).unwrap_or_default();
    let token_card = card(
        "account-method-token",
        None,
        true,
        Icon::new(IconName::Key)
            .size(px(16.))
            .color(colors.text)
            .into_any_element(),
        "Token",
        "Paste a personal access token. Works everywhere.",
        Some(Rc::new(|_, _| {})),
        window,
        cx,
    );
    // The browser option is drawn but not selectable: OAuth is a later step.
    let browser_description = if logic::has_oauth(kind) {
        if dialog.self_hosted {
            "Uses an OAuth app registered on this instance.".to_owned()
        } else {
            "Approve GitWarp in your browser, no token to copy.".to_owned()
        }
    } else {
        format!(
            "Not available for {}: it has no sign-in flow for desktop apps. Use a token.",
            kind.display_name()
        )
    };
    let browser_card = card(
        "account-method-browser",
        None,
        false,
        Icon::new(IconName::Link)
            .size(px(16.))
            .color(colors.muted)
            .into_any_element(),
        "Sign in with browser",
        browser_description,
        None,
        window,
        cx,
    )
    .when(logic::has_oauth(kind), |this| {
        this.tooltip(Tooltip::text("Coming soon"))
    });

    let token_url = logic::token_page_url(kind, &base_url);
    let mut perms = div().flex().flex_wrap().gap(px(6.)).mt(px(4.));
    for permission in logic::required_permissions(kind) {
        perms = perms.child(permission_chip(permission, None, cx));
    }
    let permissions_title = if kind == ServiceKind::GitHub {
        "Required permissions (classic token scopes)"
    } else {
        "Required permissions"
    };
    let show = dialog.show_token;
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .when_some(dialog.reconnect.as_ref(), |this, account| {
            this.child(lock_line(kind, &host, &account.login, cx))
        })
        .when_some(http_warning(Some(&base_url)), |this, banner| {
            this.child(banner)
        })
        .when_some(dialog.store_error.clone(), |this, error| {
            this.child(
                Banner::new(BannerKind::Error, "The system keychain is not available").body(
                    format!("{error}. GitWarp does not store tokens anywhere else."),
                ),
            )
        })
        .when_some(
            sign_in_banner(state, dialog, kind, &host, cx),
            |this, banner| this.child(banner),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(token_card)
                .child(browser_card),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(field_label("Access token"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(26.))
                                .child(dialog.token.clone()),
                        )
                        .child(
                            Button::new("account-token-show")
                                .icon_only(IconName::Eye)
                                .selected(show)
                                .tooltip(if show { "Hide token" } else { "Show token" })
                                .focus_handle(dialog.show_focus.clone())
                                .on_click(act(state, |d, _, cx| {
                                    d.show_token = !d.show_token;
                                    let masked = !d.show_token;
                                    d.token.update(cx, |input, cx| input.set_masked(masked, cx));
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(field_label(permissions_title))
                        .child(perms)
                        .child(help_text(logic::token_where(kind), cx).mt(px(6.))),
                )
                .child(
                    Button::new("account-token-create")
                        .label("Create a token")
                        .icon(IconName::Link)
                        .focus_handle(dialog.create_focus.clone())
                        .on_click(move |_: &ClickEvent, _, cx: &mut App| cx.open_url(&token_url)),
                ),
        )
        .into_any_element()
}

fn done_step(dialog: &AccountDialog, kind: ServiceKind, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let login = dialog
        .connected
        .as_ref()
        .map(|c| c.login.clone())
        .unwrap_or_default();
    let required = dialog.label_required(cx);
    let host = dialog.host_text(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .border_1()
                .border_color(colors.success.opacity(0.5))
                .rounded(R_MD)
                .bg(colors.success.opacity(0.08))
                .child(avatar(&login, px(40.), FS_MD, cx))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("Connected as {login}")),
                        )
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(format!("{} · {host} · token", kind.display_name())),
                        ),
                ),
        )
        .when_some(dialog.save_error.clone(), |this, error| {
            this.child(
                Banner::new(BannerKind::Error, "The token could not be saved").body(error),
            )
        })
        .when(!dialog.reconnecting(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field_label(if required {
                        "Label *"
                    } else {
                        "Label (optional)"
                    }))
                    .child(div().w_full().h(px(26.)).child(dialog.label.clone()))
                    .child(if dialog.label_error {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(FS_SM)
                            .text_color(colors.error)
                            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                            .child("Enter a label for this account.")
                            .into_any_element()
                    } else if required {
                        help_text(
                            format!(
                                "You already have an account on {host}. A label tells them apart in clone, push and pull-request menus."
                            ),
                            cx,
                        )
                        .into_any_element()
                    } else {
                        help_text("Defaults to the login name.", cx).into_any_element()
                    }),
            )
        })
        .into_any_element()
}

fn build(state: &Entity<AccountDialog>, window: &Window, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let reconnecting = dialog.reconnecting();
    let title = if reconnecting {
        "Reconnect account"
    } else {
        "Add account"
    };
    let kind = dialog.kind;

    let body_inner = match (dialog.step, kind) {
        (Step::Service, _) | (_, None) => service_step(state, dialog, window, cx),
        (Step::Instance, Some(kind)) => instance_step(state, dialog, kind, window, cx),
        (Step::SignIn, Some(kind)) => sign_in_step(state, dialog, kind, window, cx),
        (Step::Done, Some(kind)) => done_step(
            dialog,
            dialog.connected.as_ref().map_or(kind, |c| c.kind),
            cx,
        ),
    };
    let body = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .when(!reconnecting, |this| {
            this.child(stepper(dialog.step.number(), cx))
        })
        .child(body_inner);

    // Footer: a secondary button (Back / Cancel) and the primary one.
    let (secondary_label, secondary_back) = match dialog.step {
        Step::Service => ("Cancel", false),
        Step::Instance => ("Back", true),
        Step::SignIn if reconnecting => ("Cancel", false),
        Step::SignIn => ("Back", true),
        Step::Done => ("", false),
    };
    let (primary_label, primary_enabled): (SharedString, bool) = match dialog.step {
        Step::Service => ("Continue".into(), dialog.kind.is_some()),
        Step::Instance => (
            continue_label(!dialog.self_hosted, &dialog.probe).into(),
            continue_enabled(
                !dialog.self_hosted,
                dialog.base_url(cx).is_some(),
                &dialog.probe,
            ),
        ),
        Step::SignIn => (
            if dialog.verifying {
                "Checking…"
            } else if reconnecting {
                "Reconnect"
            } else {
                "Connect"
            }
            .into(),
            dialog.can_connect(cx),
        ),
        Step::Done => ("Done".into(), !dialog.saving),
    };
    let mut footer = div().flex().items_center().gap(px(8.));
    if dialog.step != Step::Done {
        footer = footer.child(
            Button::new("account-secondary")
                .label(secondary_label)
                .disabled(dialog.verifying || dialog.saving)
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(act(state, move |d, window, cx| {
                    if secondary_back {
                        d.back(window, cx);
                    } else {
                        d.close(window, cx);
                    }
                })),
        );
    }
    footer = footer.child(
        Button::primary("account-primary")
            .label(primary_label)
            .min_width(px(96.))
            .disabled(!primary_enabled)
            .focus_handle(dialog.primary_focus.clone())
            .on_click(act(state, |d, window, cx| d.default_action(window, cx))),
    );

    let mut order: Vec<DialogFocus> = Vec::new();
    let has_action = match dialog.step {
        Step::Instance => matches!(
            dialog.probe,
            ProbeState::Other(_) | ProbeState::Unreachable(_)
        ),
        Step::SignIn => matches!(dialog.sign_in_error, Some(SignInError::Network(_))),
        _ => false,
    };
    match dialog.step {
        Step::Service => {
            order.extend(
                dialog
                    .service_focus
                    .iter()
                    .cloned()
                    .map(DialogFocus::control),
            );
        }
        Step::Instance => {
            order.push(DialogFocus::control(dialog.public_focus.clone()));
            order.push(DialogFocus::control(dialog.self_focus.clone()));
            if dialog.self_hosted {
                order.push(DialogFocus::input(
                    dialog.url.read(cx).focus_handle().clone(),
                ));
            }
        }
        Step::SignIn => {
            order.push(DialogFocus::input(
                dialog.token.read(cx).focus_handle().clone(),
            ));
            order.push(DialogFocus::control(dialog.show_focus.clone()));
            order.push(DialogFocus::control(dialog.create_focus.clone()));
        }
        Step::Done => {
            if !reconnecting {
                order.push(DialogFocus::input(
                    dialog.label.read(cx).focus_handle().clone(),
                ));
            }
        }
    }
    if has_action {
        order.push(DialogFocus::control(dialog.action_focus.clone()));
    }
    if dialog.step != Step::Done {
        order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    }
    order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.default_action(window, cx));
        }
    };
    Dialog::new(title)
        .icon(IconName::Key)
        .width(px(580.))
        .busy(dialog.verifying || dialog.saving)
        .body(body)
        .footer(footer)
        .focus_order(order)
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default)
}
