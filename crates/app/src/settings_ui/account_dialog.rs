//! The Add / Reconnect account dialog.
//!
//! Steps: service, instance (with a probe for self-hosted addresses), sign-in
//! with a token or in the browser, then the result with an optional label.
//! Reconnect opens on the sign-in step with the service and instance locked;
//! for a browser account it goes straight to the device-code view. The browser
//! sign-in (GitHub device flow) is offered for github.com only; the other
//! services show it disabled.
//!
//! The pure rules (step order, button gating, probe and token result mapping,
//! the label rule, the device-flow schedule) live in `crate::accounts` and are
//! tested there.

use std::rc::Rc;
use std::time::{Duration, Instant};

use forge::api::{Client, TokenKind};
use forge::ServiceKind;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, ClipboardItem, Context, ElementId, Entity,
    FocusHandle, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use ui::actions::Confirm;
use ui::focus::{self, RingOffset};
use ui::metrics::{FONT_MONO, FS_BASE, FS_MD, FS_SM, FS_XL, FS_XS, R_MD, R_SM};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, Banner, BannerKind, Button, ButtonSize, Dialog, DialogFocus, DialogHost, Icon,
    IconName, Spinner, TextInput, TextInputEvent, ToastKind, Tooltip,
};

use super::accounts::{avatar, service_icon};
use super::SettingsView;
use crate::accounts::{
    self as logic, cached_store, continue_enabled, continue_label, instance_from_input,
    probe_state, sign_in_outcome, DevicePoll, PollOutcome, ProbeState, SignInError, Step, Tick,
};
use crate::settings::{AccountMethod, AccountRecord};

/// What activating a card does.
type CardAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the dialog is opened for.
pub(crate) enum Open {
    Add,
    /// Add an account for a service and instance known from a remote: starts
    /// at the sign-in step. `base_url` is the public instance's URL or a
    /// self-hosted one (probed like a typed address).
    AddFor {
        kind: ServiceKind,
        base_url: String,
    },
    /// Re-enter the token of this account.
    Reconnect(AccountRecord),
}

/// Runs after an account is saved, with its record: a new account (when the
/// dialog is opened from a remote instead of Settings; the opener then replaces
/// the toast) or a Reconnect (the toast stays).
pub(crate) type OnSaved = Rc<dyn Fn(&AccountRecord, &mut Window, &mut App)>;

/// A token the service accepted, waiting for the label and Done.
type Connected = logic::Verified;

/// The device-code view: a code issued by GitHub, being polled.
struct Device {
    poll: DevicePoll,
    started: Instant,
    /// The short code shown to the user (the device code stays in `poll`).
    user_code: String,
    /// The page where the code is entered.
    verification_uri: String,
}

/// What the countdown timer found.
enum DeviceTick {
    Stop,
    Wait,
    Poll(String),
}

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
    /// How the sign-in step signs in.
    method: AccountMethod,
    /// The device-code view, while a browser sign-in waits for approval.
    device: Option<Device>,
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
    /// The Settings page that opened the dialog; `None` when opened from a remote.
    view: Option<WeakEntity<SettingsView>>,
    on_saved: Option<OnSaved>,

    service_focus: Vec<FocusHandle>,
    public_focus: FocusHandle,
    self_focus: FocusHandle,
    show_focus: FocusHandle,
    create_focus: FocusHandle,
    method_token_focus: FocusHandle,
    method_browser_focus: FocusHandle,
    open_focus: FocusHandle,
    copy_focus: FocusHandle,
    action_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
}

/// Opens the dialog.
pub(crate) fn open(
    what: Open,
    view: Option<WeakEntity<SettingsView>>,
    on_saved: Option<OnSaved>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AccountDialog> {
    // A self-hosted address to fill in and probe once the dialog exists.
    let prefill = match &what {
        Open::AddFor { kind, base_url }
            if forge::Instance::new(*kind, base_url).is_ok_and(|i| !i.is_public()) =>
        {
            Some(base_url.clone())
        }
        _ => None,
    };
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
            input.set_placeholder(ui::t!("settings.accounts.dialog.token.placeholder"), cx);
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
            Open::AddFor { kind, base_url } => {
                let self_hosted = forge::Instance::new(kind, &base_url)
                    .map(|instance| !instance.is_public())
                    .unwrap_or(false);
                (None, Step::SignIn, Some(kind), self_hosted)
            }
            Open::Reconnect(account) => {
                let kind = logic::kind_of(&account);
                let self_hosted = logic::instance_of(&account)
                    .map(|instance| !instance.is_public())
                    .unwrap_or(true);
                (Some(account), Step::SignIn, kind, self_hosted)
            }
        };
        let method = kind.map_or(AccountMethod::Token, |kind| {
            logic::default_method(kind, self_hosted, reconnect.as_ref().map(|a| a.method))
        });
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
            method,
            device: None,
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
            on_saved,
            service_focus: ServiceKind::ALL
                .iter()
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            public_focus: cx.focus_handle().tab_stop(true),
            self_focus: cx.focus_handle().tab_stop(true),
            show_focus: cx.focus_handle().tab_stop(true),
            create_focus: cx.focus_handle().tab_stop(true),
            method_token_focus: cx.focus_handle().tab_stop(true),
            method_browser_focus: cx.focus_handle().tab_stop(true),
            open_focus: cx.focus_handle().tab_stop(true),
            copy_focus: cx.focus_handle().tab_stop(true),
            action_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
        }
    });
    let state = dialog.downgrade();
    let (layer, host) =
        dialog::open_with_view(window, cx, move |window, cx| match state.upgrade() {
            Some(state) => build(&state, window, cx),
            None => Dialog::new(ui::t!("settings.accounts.dialog.add.title")),
        });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog._host = Some(host);
        // Reconnect and Add-for-a-remote open on the sign-in method.
        if dialog.step == Step::SignIn {
            dialog.focus_sign_in(window, cx);
            // A browser account goes straight to the device code.
            if dialog.reconnecting() && dialog.method == AccountMethod::Browser {
                dialog.connect(window, cx);
            }
        }
    });
    if let Some(base_url) = prefill {
        dialog.update(cx, |dialog, cx| {
            dialog
                .url
                .update(cx, |input, cx| input.set_text(base_url, cx));
        });
        // Setting the text emits Changed, which resets the probe state; start
        // the probe after that event has been handled.
        let weak = dialog.downgrade();
        cx.defer(move |cx| {
            weak.update(cx, |dialog, cx| dialog.run_probe(cx)).ok();
        });
    }
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
        self.method = self.kind.map_or(AccountMethod::Token, |kind| {
            logic::default_method(kind, self.self_hosted, None)
        });
        self.focus_sign_in(window, cx);
        cx.notify();
    }

    /// Focus for the sign-in step: the token field, or the primary button
    /// when the browser is the method (there is no field then).
    fn focus_sign_in(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.method {
            AccountMethod::Token => {
                let handle = self.token.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            AccountMethod::Browser => window.focus(&self.primary_focus, cx),
        }
    }

    fn set_method(&mut self, method: AccountMethod, window: &mut Window, cx: &mut Context<Self>) {
        if self.verifying || self.saving || self.device.is_some() || self.method == method {
            return;
        }
        self.method = method;
        self.sign_in_error = None;
        self.focus_sign_in(window, cx);
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
        let has_credentials = match self.method {
            AccountMethod::Token => !self.token_text(cx).is_empty(),
            AccountMethod::Browser => true,
        };
        has_credentials && !self.verifying && self.store_error.is_none() && self.device.is_none()
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_connect(cx) {
            return;
        }
        let (Some(kind), Some(base_url)) = (self.kind, self.base_url(cx)) else {
            return;
        };
        if self.method == AccountMethod::Browser {
            self.start_device(kind, &base_url, window, cx);
            return;
        }
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
                                    method: AccountMethod::Token,
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

    /// Browser sign-in, step 1: asks GitHub for a device code, then keeps
    /// polling until the user approves, declines or the code expires. One
    /// task per attempt; closing the dialog or Cancel bumps the generation and
    /// the task ends at its next wake-up.
    fn start_device(
        &mut self,
        kind: ServiceKind,
        base_url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut instance = match forge::Instance::new(kind, base_url) {
            Ok(instance) => instance,
            Err(error) => {
                self.sign_in_error = Some(SignInError::Other(error.to_string()));
                cx.notify();
                return;
            }
        };
        // The OAuth app exists for github.com only: never start (or later
        // send a token) for any other host, whatever the form or settings say.
        if !logic::browser_available(kind, self.self_hosted) || !logic::oauth_allowed(&instance) {
            self.sign_in_error = Some(SignInError::Other(
                ui::t!("settings.accounts.dialog.signin.browser_unavailable").to_string(),
            ));
            cx.notify();
            return;
        }
        if let Some(account) = &self.reconnect {
            instance.ssh_host = account.ssh_host.clone();
        }
        self.verifying = true;
        self.sign_in_error = None;
        self.device = None;
        self.connect_generation = self.connect_generation.wrapping_add(1);
        let attempt = self.connect_generation;
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let start = cx
                .background_executor()
                .spawn(async {
                    forge::auth::github_device_start(&Client::new(), forge::auth::GITHUB_OAUTH_URL)
                })
                .await;
            let code = match start {
                Ok(code) => code,
                Err(error) => {
                    this.update(cx, |dialog, cx| {
                        if dialog.is_current(attempt) {
                            dialog.verifying = false;
                            dialog.sign_in_error = Some(logic::browser_error(error));
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
            };
            let shown = this.update(cx, |dialog, cx| {
                if !dialog.is_current(attempt) {
                    return false;
                }
                dialog.verifying = false;
                dialog.device = Some(Device {
                    poll: DevicePoll::new(&code),
                    started: Instant::now(),
                    user_code: code.user_code.clone(),
                    verification_uri: code.verification_uri.clone(),
                });
                cx.notify();
                true
            });
            if !matches!(shown, Ok(true)) {
                return;
            }
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(tick) = this.update(cx, |dialog, cx| dialog.device_tick(attempt, cx)) else {
                    return;
                };
                let device_code = match tick {
                    DeviceTick::Stop => return,
                    DeviceTick::Wait => continue,
                    DeviceTick::Poll(device_code) => device_code,
                };
                let polled = cx
                    .background_executor()
                    .spawn(async move {
                        forge::auth::github_device_poll(
                            &Client::new(),
                            forge::auth::GITHUB_OAUTH_URL,
                            &device_code,
                        )
                    })
                    .await;
                let instance = instance.clone();
                let keep_going = window_handle
                    .update(cx, |_, window, cx| {
                        this.update(cx, |dialog, cx| {
                            dialog.device_polled(attempt, polled, kind, instance, window, cx)
                        })
                        .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Whether `attempt` is the latest and the dialog still open.
    fn is_current(&self, attempt: u64) -> bool {
        logic::attempt_is_current(attempt, self.connect_generation, self.layer.is_some())
    }

    /// One second of the countdown: redraws it, and says whether to poll.
    fn device_tick(&mut self, attempt: u64, cx: &mut Context<Self>) -> DeviceTick {
        if !self.is_current(attempt) {
            return DeviceTick::Stop;
        }
        let Some(device) = &self.device else {
            return DeviceTick::Stop;
        };
        match device.poll.tick(device.started.elapsed().as_secs()) {
            Tick::Expired => {
                self.device = None;
                self.sign_in_error = Some(SignInError::Expired);
                cx.notify();
                DeviceTick::Stop
            }
            Tick::Wait => {
                cx.notify();
                DeviceTick::Wait
            }
            Tick::Poll => DeviceTick::Poll(device.poll.device_code().to_owned()),
        }
    }

    /// Applies one poll answer; `false` ends the polling task.
    fn device_polled(
        &mut self,
        attempt: u64,
        result: Result<forge::auth::PollStep, forge::auth::AuthError>,
        kind: ServiceKind,
        instance: forge::Instance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_current(attempt) {
            return false;
        }
        let Some(device) = self.device.as_mut() else {
            return false;
        };
        let elapsed = device.started.elapsed().as_secs();
        let keep_going = match device.poll.apply(elapsed, result) {
            PollOutcome::Continue => return true,
            PollOutcome::Denied => {
                self.sign_in_error = Some(SignInError::Denied);
                false
            }
            PollOutcome::Expired => {
                self.sign_in_error = Some(SignInError::Expired);
                false
            }
            PollOutcome::Failed(error) => {
                self.sign_in_error = Some(logic::browser_error(error));
                false
            }
            PollOutcome::Done(tokens) => {
                self.verify_browser(tokens, kind, instance, window, cx);
                false
            }
        };
        if !keep_going {
            self.device = None;
        }
        cx.notify();
        keep_going
    }

    /// Browser sign-in, last step: asks the service who the new token belongs
    /// to, like the token path does, then goes to the result.
    fn verify_browser(
        &mut self,
        tokens: forge::auth::OAuthTokens,
        kind: ServiceKind,
        instance: forge::Instance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !logic::oauth_allowed(&instance) {
            self.sign_in_error = Some(SignInError::Other(
                ui::t!("settings.accounts.dialog.signin.browser_unavailable").to_string(),
            ));
            cx.notify();
            return;
        }
        self.verifying = true;
        let attempt = self.connect_generation;
        let checked = (kind, instance.base_url.clone(), instance.ssh_host.clone());
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let secret = tokens.to_secret();
            let result = cx
                .background_executor()
                .spawn(async move {
                    forge::api::current_user(
                        &Client::new(),
                        &instance,
                        &tokens.access,
                        TokenKind::OAuth,
                    )
                })
                .await;
            window_handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |dialog, cx| {
                        if !dialog.is_current(attempt) {
                            return;
                        }
                        dialog.verifying = false;
                        match sign_in_outcome(kind, result) {
                            Ok(login) => {
                                let (kind, base_url, ssh_host) = checked;
                                dialog.show_result(
                                    Connected {
                                        kind,
                                        base_url,
                                        ssh_host,
                                        login,
                                        method: AccountMethod::Browser,
                                        token: secret,
                                    },
                                    window,
                                    cx,
                                );
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
    }

    /// Cancel on the device-code view: back to the method choice.
    fn cancel_device(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The polling task ends at its next wake-up.
        self.connect_generation = self.connect_generation.wrapping_add(1);
        self.device = None;
        self.verifying = false;
        self.sign_in_error = None;
        self.focus_sign_in(window, cx);
        cx.notify();
    }

    fn open_verification_page(&self, cx: &mut App) {
        if let Some(device) = &self.device {
            cx.open_url(&device.verification_uri);
        }
    }

    fn copy_user_code(&self, window: &mut Window, cx: &mut App) {
        if let Some(device) = &self.device {
            cx.write_to_clipboard(ClipboardItem::new_string(device.user_code.clone()));
            ui::toast::show(
                ToastKind::Ok,
                ui::t!("settings.accounts.dialog.device.copied"),
                window,
                cx,
            );
        }
    }

    /// Moves to the result step. A second account on the same instance has to
    /// be told apart by a label, so the field is required then.
    fn show_result(&mut self, verified: Connected, window: &mut Window, cx: &mut Context<Self>) {
        let login = verified.login.clone();
        self.connected = Some(verified);
        let required = self.label_required(cx);
        let placeholder = if required {
            ui::t!("settings.accounts.dialog.label.placeholder_required")
        } else {
            SharedString::from(login.clone())
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
                    logic::store_secret(store.as_ref(), &id, &token)
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
                        let _ = logic::forget_secret(store.as_ref(), &id);
                    }
                })
                .detach();
            self.account_removed(window, cx);
            return;
        }
        let id = record.id.clone();
        let message = if reconnect {
            ui::t!(
                "settings.accounts.dialog.toast.reconnected",
                label = record.display_label()
            )
        } else {
            let name = logic::kind_of(&record).map_or("", ServiceKind::display_name);
            ui::t!(
                "settings.accounts.dialog.toast.connected",
                name = name,
                label = record.display_label()
            )
        };
        crate::update_settings(cx, |settings| {
            match settings.accounts.iter_mut().find(|a| a.id == record.id) {
                // Reconnect keeps the label and the SSH host; the login and the
                // sign-in method follow the new credentials.
                Some(existing) => {
                    existing.login = record.login.clone();
                    existing.method = record.method;
                }
                None => settings.accounts.push(record.clone()),
            }
        });
        if let Some(view) = self.view.as_ref().and_then(WeakEntity::upgrade) {
            view.update(cx, |view, cx| {
                view.account_status
                    .insert(id, crate::accounts::AccountStatus::Connected);
                cx.notify();
            });
        }
        let on_saved = self.on_saved.clone();
        self.close(window, cx);
        match &on_saved {
            // The opener says what happened in its own words.
            Some(on_saved) if !reconnect => on_saved(&record, window, cx),
            _ => {
                ui::toast::show(ToastKind::Ok, message, window, cx);
            }
        }
        // A Reconnect keeps its toast; the opener only learns the token works
        // again (e.g. to reload a list that failed with the old one).
        if let Some(on_saved) = on_saved.filter(|_| reconnect) {
            on_saved(&record, window, cx);
        }
    }

    fn account_exists(&self, id: &str, cx: &App) -> bool {
        super::current_settings(cx)
            .accounts
            .iter()
            .any(|a| a.id == id)
    }

    /// The account being reconnected was removed meanwhile: nothing is saved.
    fn account_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.view.as_ref().and_then(WeakEntity::upgrade) {
            view.update(cx, |view, cx| {
                view.accounts_error = Some(ui::t!("settings.accounts.dialog.removed"));
                cx.notify();
            });
        }
        self.close(window, cx);
    }

    fn default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Service | Step::Instance => self.next(window, cx),
            Step::SignIn if self.device.is_some() => self.open_verification_page(cx),
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
    let names = [
        ui::t!("settings.accounts.dialog.step.service"),
        ui::t!("settings.accounts.dialog.step.instance"),
        ui::t!("settings.accounts.dialog.step.sign_in"),
    ];
    for (index, name) in names.into_iter().enumerate() {
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
            ui::t!("settings.accounts.dialog.http_warning"),
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
                .body(ui::t!("settings.accounts.dialog.probe.public.body"))
                .into_any_element(),
        ),
        ProbeState::Idle => None,
        ProbeState::Checking => Some(
            Banner::new(
                BannerKind::Info,
                ui::t!("settings.accounts.dialog.probe.checking.title", host = host),
            )
            .body(ui::t!("settings.accounts.dialog.probe.checking.body"))
            .into_any_element(),
        ),
        ProbeState::Detected { version } => Some(
            Banner::new(
                BannerKind::Ok,
                match version {
                    Some(version) => ui::t!(
                        "settings.accounts.dialog.probe.detected.title_version",
                        name = name,
                        version = version
                    ),
                    None => ui::t!("settings.accounts.dialog.probe.detected.title", name = name),
                },
            )
            .body(ui::t!("settings.accounts.dialog.probe.detected.body"))
            .into_any_element(),
        ),
        ProbeState::Other(other) => {
            let other = *other;
            Some(
                Banner::new(
                    BannerKind::Warn,
                    ui::t!(
                        "settings.accounts.dialog.probe.other.title",
                        host = host,
                        other = other.display_name(),
                        name = name
                    ),
                )
                .body(ui::t!(
                    "settings.accounts.dialog.probe.other.body",
                    other = other.display_name()
                ))
                .action(
                    Button::primary("account-switch-service")
                        .label(ui::t!(
                            "settings.accounts.dialog.probe.other.switch",
                            other = other.display_name()
                        ))
                        .size(ButtonSize::Small)
                        .focus_handle(dialog.action_focus.clone())
                        .on_click(act(state, move |d, _, cx| d.switch_service(other, cx))),
                )
                .into_any_element(),
            )
        }
        ProbeState::Unreachable(text) => Some(
            Banner::new(
                BannerKind::Error,
                ui::t!(
                    "settings.accounts.dialog.probe.unreachable.title",
                    host = host
                ),
            )
            .body(ui::t!(
                "settings.accounts.dialog.probe.unreachable.body",
                error = text
            ))
            .action(
                Button::new("account-probe-retry")
                    .label(ui::t!("settings.accounts.dialog.try_again"))
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
                ui::t!("settings.accounts.dialog.probe.tls.title", host = host),
            )
            .body(text.clone())
            .into_any_element(),
        ),
        ProbeState::NotRecognized => Some(
            Banner::new(
                BannerKind::Error,
                ui::t!(
                    "settings.accounts.dialog.probe.not_recognized.title",
                    host = host
                ),
            )
            .body(ui::t!("settings.accounts.dialog.probe.not_recognized.body"))
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
            Banner::new(
                BannerKind::Error,
                ui::t!("settings.accounts.dialog.signin.wrong.title"),
            )
            .body(ui::t!(
                "settings.accounts.dialog.signin.wrong.body",
                name = name
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
                                div().font_weight(FontWeight::SEMIBOLD).child(ui::t!(
                                    "settings.accounts.dialog.signin.permissions.title"
                                )),
                            )
                            .child(div().text_color(colors.muted).child(ui::t!(
                                "settings.accounts.dialog.signin.permissions.body",
                                name = name
                            )))
                            .child(chips),
                    )
                    .into_any_element(),
            )
        }
        SignInError::Network(text) => Some(
            Banner::new(
                BannerKind::Error,
                ui::t!("settings.accounts.dialog.signin.network.title", host = host),
            )
            .body(ui::t!(
                "settings.accounts.dialog.signin.network.body",
                error = text
            ))
            .action(
                Button::new("account-connect-retry")
                    .label(ui::t!("settings.accounts.dialog.try_again"))
                    .icon(IconName::Fetch)
                    .size(ButtonSize::Small)
                    .focus_handle(dialog.action_focus.clone())
                    .on_click(act(state, |d, window, cx| d.connect(window, cx))),
            )
            .into_any_element(),
        ),
        SignInError::Denied => Some(
            Banner::new(
                BannerKind::Warn,
                ui::t!("settings.accounts.dialog.signin.denied.title"),
            )
            .body(ui::t!("settings.accounts.dialog.signin.denied.body"))
            .action(
                Button::new("account-denied-retry")
                    .label(ui::t!("settings.accounts.dialog.try_again"))
                    .icon(IconName::Fetch)
                    .size(ButtonSize::Small)
                    .focus_handle(dialog.action_focus.clone())
                    .on_click(act(state, |d, window, cx| d.connect(window, cx))),
            )
            .into_any_element(),
        ),
        SignInError::Expired => Some(
            Banner::new(
                BannerKind::Warn,
                ui::t!("settings.accounts.dialog.signin.expired.title"),
            )
            .body(ui::t!("settings.accounts.dialog.signin.expired.body"))
            .action(
                Button::primary("account-expired-restart")
                    .label(ui::t!("settings.accounts.dialog.signin.expired.restart"))
                    .icon(IconName::Fetch)
                    .size(ButtonSize::Small)
                    .focus_handle(dialog.action_focus.clone())
                    .on_click(act(state, |d, window, cx| d.connect(window, cx))),
            )
            .into_any_element(),
        ),
        SignInError::Other(text) => Some(
            Banner::new(
                BannerKind::Error,
                ui::t!("settings.accounts.dialog.signin.other.title"),
            )
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
        ui::t!("settings.accounts.dialog.instance.public.title"),
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
        ui::t!("settings.accounts.dialog.instance.self.title"),
        ui::t!(
            "settings.accounts.dialog.instance.self.description",
            name = logic::self_hosted_name(kind)
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
                    .child(field_label(ui::t!(
                        "settings.accounts.dialog.instance.url.label"
                    )))
                    .child(div().w_full().h(px(26.)).child(dialog.url.clone()))
                    .child(if url_invalid {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(FS_SM)
                            .text_color(colors.error)
                            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                            .child(ui::t!("settings.accounts.dialog.instance.url.invalid"))
                            .into_any_element()
                    } else {
                        help_text(ui::t!("settings.accounts.dialog.instance.url.help"), cx)
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
    if let Some(device) = &dialog.device {
        return device_step(state, dialog, device, &host, cx);
    }
    let base_url = dialog.base_url(cx).unwrap_or_default();
    let browser_ok = logic::browser_available(kind, dialog.self_hosted);
    let method = dialog.method;
    let pick = |target: AccountMethod| {
        let state = state.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.set_method(target, window, cx));
        }) as CardAction
    };
    let token_card = card(
        "account-method-token",
        Some(&dialog.method_token_focus),
        method == AccountMethod::Token,
        Icon::new(IconName::Key)
            .size(px(16.))
            .color(colors.text)
            .into_any_element(),
        ui::t!("settings.accounts.method.token"),
        ui::t!("settings.accounts.dialog.method.token.description"),
        Some(pick(AccountMethod::Token)),
        window,
        cx,
    );
    // The browser option is selectable for github.com only; the other services
    // keep it drawn disabled (their OAuth client IDs come later).
    let browser_description = if logic::has_oauth(kind) {
        if dialog.self_hosted {
            ui::t!("settings.accounts.dialog.method.browser.description_self")
        } else {
            ui::t!("settings.accounts.dialog.method.browser.description_public")
        }
    } else {
        ui::t!(
            "settings.accounts.dialog.method.browser.unavailable",
            name = kind.display_name()
        )
    };
    let browser_card = card(
        "account-method-browser",
        browser_ok.then_some(&dialog.method_browser_focus),
        browser_ok && method == AccountMethod::Browser,
        Icon::new(IconName::Link)
            .size(px(16.))
            .color(if browser_ok {
                colors.text
            } else {
                colors.muted
            })
            .into_any_element(),
        ui::t!("settings.accounts.dialog.method.browser.title"),
        browser_description,
        browser_ok.then(|| pick(AccountMethod::Browser)),
        window,
        cx,
    )
    .when(logic::has_oauth(kind) && !browser_ok, |this| {
        this.tooltip(Tooltip::text(ui::t!(
            "settings.accounts.dialog.method.browser.coming_soon"
        )))
    });

    let token_url = logic::token_page_url(kind, &base_url);
    let mut perms = div().flex().flex_wrap().gap(px(6.)).mt(px(4.));
    for permission in logic::required_permissions(kind) {
        perms = perms.child(permission_chip(&permission, None, cx));
    }
    let permissions_title = if kind == ServiceKind::GitHub {
        ui::t!("settings.accounts.dialog.permissions.title_github")
    } else {
        ui::t!("settings.accounts.dialog.permissions.title")
    };
    let show = dialog.show_token;
    let mut column = div()
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
                Banner::new(
                    BannerKind::Error,
                    ui::t!("settings.accounts.dialog.keychain.title"),
                )
                .body(ui::t!(
                    "settings.accounts.dialog.keychain.body",
                    error = error
                )),
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
        );
    if method == AccountMethod::Token {
        column = column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field_label(ui::t!("settings.accounts.dialog.token.label")))
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
                                    .tooltip(if show {
                                        ui::t!("settings.accounts.dialog.token.hide")
                                    } else {
                                        ui::t!("settings.accounts.dialog.token.show")
                                    })
                                    .focus_handle(dialog.show_focus.clone())
                                    .on_click(act(state, |d, _, cx| {
                                        d.show_token = !d.show_token;
                                        let masked = !d.show_token;
                                        d.token
                                            .update(cx, |input, cx| input.set_masked(masked, cx));
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
                            .label(ui::t!("settings.accounts.dialog.token.create"))
                            .icon(IconName::Link)
                            .focus_handle(dialog.create_focus.clone())
                            .on_click(move |_: &ClickEvent, _, cx: &mut App| {
                                cx.open_url(&token_url)
                            }),
                    ),
            );
    } else {
        column = column.child(help_text(
            ui::t!("settings.accounts.dialog.browser.help", host = host),
            cx,
        ));
    }
    column.into_any_element()
}

/// The device-code view of a browser sign-in: the code to enter, buttons to
/// open the page and copy the code, and the countdown.
fn device_step(
    state: &Entity<AccountDialog>,
    dialog: &AccountDialog,
    device: &Device,
    host: &str,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let remaining = device.poll.remaining(device.started.elapsed().as_secs());
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            Banner::new(
                BannerKind::Info,
                ui::t!("settings.accounts.dialog.device.title"),
            )
            .body(ui::t!("settings.accounts.dialog.device.body", host = host)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(16.))
                .py(px(14.))
                .border_1()
                .border_color(colors.border_strong)
                .rounded(R_MD)
                .bg(colors.bg)
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(FS_XL)
                        .font_weight(FontWeight::BOLD)
                        .child(device.user_code.clone()),
                )
                .child(
                    Button::new("account-device-copy")
                        .label(ui::t!("settings.accounts.dialog.device.copy"))
                        .icon(IconName::Copy)
                        .focus_handle(dialog.copy_focus.clone())
                        .on_click({
                            let state = state.clone();
                            move |_: &ClickEvent, window, cx: &mut App| {
                                state.update(cx, |d, cx| d.copy_user_code(window, cx))
                            }
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::primary("account-device-open")
                        .label(ui::t!("settings.accounts.dialog.device.open"))
                        .icon(IconName::Link)
                        .focus_handle(dialog.open_focus.clone())
                        .on_click({
                            let state = state.clone();
                            move |_: &ClickEvent, _, cx: &mut App| {
                                state.update(cx, |d, cx| d.open_verification_page(cx))
                            }
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .min_w_0()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("settings.accounts.dialog.device.or_go_to"))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .child(device.verification_uri.clone()),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_color(colors.muted)
                .child(Spinner::new(
                    ElementId::Name("account-device-spin".into()),
                    px(14.),
                ))
                .child(ui::t!(
                    "settings.accounts.dialog.device.waiting",
                    time = logic::format_countdown(remaining)
                )),
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
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(ui::t!(
                            "settings.accounts.dialog.done.connected_as",
                            login = login
                        )))
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(ui::t!(
                                    "settings.accounts.dialog.done.detail",
                                    service = kind.display_name(),
                                    host = host
                                )),
                        ),
                ),
        )
        .when_some(dialog.save_error.clone(), |this, error| {
            this.child(
                Banner::new(
                    BannerKind::Error,
                    ui::t!("settings.accounts.dialog.done.save_error"),
                )
                .body(error),
            )
        })
        .when(!dialog.reconnecting(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field_label(if required {
                        ui::t!("settings.accounts.dialog.label.required")
                    } else {
                        ui::t!("settings.accounts.dialog.label.optional")
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
                            .child(ui::t!("settings.accounts.dialog.label.error"))
                            .into_any_element()
                    } else if required {
                        help_text(
                            ui::t!("settings.accounts.dialog.label.help_required", host = host),
                            cx,
                        )
                        .into_any_element()
                    } else {
                        help_text(ui::t!("settings.accounts.dialog.label.help_default"), cx)
                            .into_any_element()
                    }),
            )
        })
        .into_any_element()
}

fn build(state: &Entity<AccountDialog>, window: &Window, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let reconnecting = dialog.reconnecting();
    let title = if reconnecting {
        ui::t!("settings.accounts.dialog.reconnect.title")
    } else {
        ui::t!("settings.accounts.dialog.add.title")
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
    let waiting = dialog.device.is_some();
    let (secondary_label, secondary_back) = match dialog.step {
        Step::Service => (ui::t!("dialogs.common.cancel"), false),
        Step::Instance => (ui::t!("settings.accounts.dialog.back"), true),
        Step::SignIn if reconnecting => (ui::t!("dialogs.common.cancel"), false),
        Step::SignIn => (ui::t!("settings.accounts.dialog.back"), true),
        Step::Done => (SharedString::default(), false),
    };
    let (primary_label, primary_enabled): (SharedString, bool) = match dialog.step {
        Step::Service => (
            ui::t!("settings.accounts.dialog.continue"),
            dialog.kind.is_some(),
        ),
        Step::Instance => (
            continue_label(!dialog.self_hosted, &dialog.probe),
            continue_enabled(
                !dialog.self_hosted,
                dialog.base_url(cx).is_some(),
                &dialog.probe,
            ),
        ),
        Step::SignIn => (
            if dialog.verifying {
                ui::t!("settings.accounts.dialog.checking")
            } else if dialog.method == AccountMethod::Browser {
                ui::t!("settings.accounts.dialog.get_code")
            } else if reconnecting {
                ui::t!("settings.accounts.dialog.reconnect")
            } else {
                ui::t!("settings.accounts.dialog.connect")
            },
            dialog.can_connect(cx),
        ),
        Step::Done => (ui::t!("settings.accounts.dialog.done"), !dialog.saving),
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
                    if d.device.is_some() {
                        d.cancel_device(window, cx);
                    } else if secondary_back {
                        d.back(window, cx);
                    } else {
                        d.close(window, cx);
                    }
                })),
        );
    }
    // While waiting for approval the only footer button is Cancel.
    if !waiting {
        footer = footer.child(
            Button::primary("account-primary")
                .label(primary_label)
                .min_width(px(96.))
                .disabled(!primary_enabled)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(act(state, |d, window, cx| d.default_action(window, cx))),
        );
    }

    let mut order: Vec<DialogFocus> = Vec::new();
    let has_action = match dialog.step {
        Step::Instance => matches!(
            dialog.probe,
            ProbeState::Other(_) | ProbeState::Unreachable(_)
        ),
        Step::SignIn => matches!(
            dialog.sign_in_error,
            Some(SignInError::Network(_) | SignInError::Denied | SignInError::Expired)
        ),
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
        Step::SignIn if waiting => {
            order.push(DialogFocus::control(dialog.open_focus.clone()));
            order.push(DialogFocus::control(dialog.copy_focus.clone()));
        }
        Step::SignIn => {
            order.push(DialogFocus::control(dialog.method_token_focus.clone()));
            if logic::browser_available(kind.unwrap_or(ServiceKind::GitLab), dialog.self_hosted) {
                order.push(DialogFocus::control(dialog.method_browser_focus.clone()));
            }
            if dialog.method == AccountMethod::Token {
                order.push(DialogFocus::input(
                    dialog.token.read(cx).focus_handle().clone(),
                ));
                order.push(DialogFocus::control(dialog.show_focus.clone()));
                order.push(DialogFocus::control(dialog.create_focus.clone()));
            }
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
    if !waiting {
        order.push(DialogFocus::control(dialog.primary_focus.clone()));
    }

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
        .default_button(if waiting {
            dialog.open_focus.clone()
        } else {
            dialog.primary_focus.clone()
        })
        .on_default(on_default)
}
