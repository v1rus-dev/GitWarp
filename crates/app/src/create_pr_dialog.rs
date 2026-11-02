//! "Create pull request" (GitLab: "Create merge request").
//!
//! The dialog reads the branch against the chosen base in the background
//! (commit subjects for the prefill, whether the branch is on the remote),
//! pushes the branch first when the remote lacks commits, then creates the
//! request through the service's API with the account's token. The token is
//! read from the keychain on the background executor only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use forge::api::{Auth, CreatedPullRequest, NewPullRequest};
use git_core::network::NetworkOutcome;
use git_core::{Error, Git, Repo};
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, FocusHandle, IntoElement,
    ParentElement, SharedString, Styled, Subscription, Window,
};

use ui::metrics::{FONT_MONO, FS_SM, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost, Icon,
    IconName, Select, TextArea, TextAreaEvent, TextInput, TextInputEvent,
};

use crate::accounts::TokenError;
use crate::pr_logic::{self, CreateFailure, PrTarget, PushFailure, PushNeed};

/// A push prepared by the repository view (credentials, host-key asker): runs
/// on the background executor, blocking.
pub(crate) type PushRun = Box<dyn FnOnce() -> Result<NetworkOutcome, Error> + Send>;

type PreparePush = Rc<dyn Fn(&str, &str, &App) -> Option<PushRun>>;
type Reconnect = Rc<dyn Fn(String, &mut Window, &mut App)>;
type OpenUrl = Rc<dyn Fn(String, String, &mut Window, &mut App)>;
type Changed = Rc<dyn Fn(bool, bool, &mut Window, &mut App)>;
type Closed = Rc<dyn Fn(&mut App)>;

/// What the repository view hands the dialog.
pub(crate) struct PrDialogContext {
    pub root: PathBuf,
    pub git: Result<Arc<Git>, Arc<Error>>,
    /// The remotes that can take a request; never empty.
    pub targets: Vec<PrTarget>,
    /// Index of the preselected target.
    pub target: usize,
    /// The branch the request is made from.
    pub head: String,
    /// The branches each remote has, as the last fetch saw them.
    pub remote_branches: HashMap<String, Vec<String>>,
    /// Prepares the push of `branch` to `remote` with the remote's account.
    pub prepare_push: PreparePush,
    /// Opens the account dialog on the token step of an account.
    pub reconnect: Reconnect,
    /// Opens `url` in the browser (second value: the host for the toast).
    pub open_url: OpenUrl,
    /// The branch was pushed (first value) and / or the request created
    /// (second): refresh the views.
    pub changed: Changed,
    /// The dialog is gone.
    pub closed: Closed,
}

/// What reading the branch against the base found.
#[derive(Debug, Clone)]
enum Range {
    Loading,
    Ready {
        subjects: Vec<String>,
        need: PushNeed,
    },
    Failed(String),
}

/// Why the last attempt did not create a request.
#[derive(Debug, Clone)]
enum Failure {
    Push(PushFailure),
    Create(CreateFailure),
}

/// The finished request.
#[derive(Debug, Clone)]
struct Done {
    number: u64,
    title: String,
    url: String,
    draft: bool,
    base: String,
    pushed: bool,
    reviewers_error: Option<String>,
}

#[derive(Debug, Clone)]
enum Phase {
    Form,
    /// `pushing`: the branch is pushed before the request is created.
    Creating {
        pushing: bool,
    },
    Done(Done),
}

/// What the background work of "Create" answers.
struct Outcome {
    /// The push ran and went through.
    pushed: bool,
    result: Result<CreatedPullRequest, Failure>,
}

pub(crate) struct CreatePrDialog {
    layer: Option<overlay::LayerId>,
    host: Option<gpui::WeakEntity<DialogHost>>,
    ctx: PrDialogContext,
    target: usize,
    base: String,
    base_choices: Vec<String>,
    range: Range,
    /// Bumped per read; an older answer is dropped.
    range_generation: u64,
    title: Entity<TextInput>,
    body: Entity<TextArea>,
    reviewers: Entity<TextInput>,
    /// What the fields were last filled with, to tell whether the user typed.
    prefilled_title: String,
    prefilled_body: String,
    draft: bool,
    /// Set once Create was tried, so an empty title is flagged.
    touched: bool,
    phase: Phase,
    failure: Option<Failure>,
    remote_focus: FocusHandle,
    base_focus: FocusHandle,
    title_focus: FocusHandle,
    body_focus: FocusHandle,
    reviewers_focus: FocusHandle,
    draft_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl CreatePrDialog {
    fn new(ctx: PrDialogContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = ctx.target.min(ctx.targets.len().saturating_sub(1));
        let mut subscriptions = Vec::new();
        let title = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("dialogs.pr.title_placeholder"), cx);
            input
        });
        subscriptions.push(cx.subscribe(&title, |this, _, _: &TextInputEvent, cx| {
            this.notify_host(cx);
            cx.notify();
        }));
        let body = cx.new(|cx| {
            let mut area = TextArea::new(window, cx);
            area.set_placeholder(ui::t!("dialogs.pr.body_placeholder"), cx);
            area.set_font_family(FONT_MONO, cx);
            area
        });
        subscriptions.push(cx.subscribe(&body, |this, _, _: &TextAreaEvent, cx| {
            this.notify_host(cx);
            cx.notify();
        }));
        let reviewers = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("dialogs.pr.reviewers_placeholder"), cx);
            input
        });
        let title_focus = title.read(cx).focus_handle().clone();
        let body_focus = body.read(cx).focus_handle().clone();
        let reviewers_focus = reviewers.read(cx).focus_handle().clone();
        let mut dialog = Self {
            layer: None,
            host: None,
            ctx,
            target,
            base: String::new(),
            base_choices: Vec::new(),
            range: Range::Loading,
            range_generation: 0,
            title,
            body,
            reviewers,
            prefilled_title: String::new(),
            prefilled_body: String::new(),
            draft: false,
            touched: false,
            phase: Phase::Form,
            failure: None,
            remote_focus: cx.focus_handle().tab_stop(true),
            base_focus: cx.focus_handle().tab_stop(true),
            title_focus,
            body_focus,
            reviewers_focus,
            draft_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        };
        dialog.read_range(None, cx);
        dialog
    }

    fn current(&self) -> &PrTarget {
        &self.ctx.targets[self.target]
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = self.host.as_ref().and_then(gpui::WeakEntity::upgrade) {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Creating { .. })
    }

    fn title_text(&self, cx: &App) -> String {
        self.title.read(cx).text().trim().to_owned()
    }

    fn set_remote(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.target || index >= self.ctx.targets.len() || self.running() {
            return;
        }
        self.target = index;
        self.failure = None;
        // A different remote has different branches: the base is chosen again.
        self.read_range(None, cx);
    }

    fn set_base(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(base) = self.base_choices.get(index).cloned() else {
            return;
        };
        if base == self.base || self.running() {
            return;
        }
        self.failure = None;
        self.read_range(Some(base), cx);
    }

    /// Reads the commits of the head against the base (chosen when none is
    /// given) and how the head compares to the remote's copy of it.
    fn read_range(&mut self, base: Option<String>, cx: &mut Context<Self>) {
        self.range_generation += 1;
        let generation = self.range_generation;
        self.range = Range::Loading;
        let remote = self.current().remote.clone();
        let branches = self
            .ctx
            .remote_branches
            .get(&remote)
            .cloned()
            .unwrap_or_default();
        self.base_choices = pr_logic::base_choices(&branches, &self.ctx.head);
        let git = match &self.ctx.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.range = Range::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let root = self.ctx.root.clone();
        let head = self.ctx.head.clone();
        if let Some(base) = &base {
            self.base = base.clone();
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_range(git, &root, &remote, &branches, base, &head) })
                .await;
            this.update_in(cx, |dialog, _window, cx| {
                dialog.range_read(generation, result, cx)
            })
            .ok();
        })
        .detach();
        self.notify_host(cx);
        cx.notify();
    }

    fn range_read(
        &mut self,
        generation: u64,
        result: Result<RangeRead, Error>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.range_generation {
            return;
        }
        match result {
            Ok(read) => {
                self.base = read.base.unwrap_or_default();
                if !self.base.is_empty() && !self.base_choices.contains(&self.base) {
                    // The remote's list lacks it (a default never fetched):
                    // still offer it, it is what the request goes into.
                    self.base_choices.insert(0, self.base.clone());
                }
                let (title, body) = pr_logic::prefill(&read.subjects, &self.ctx.head);
                // Text the user typed is kept; text this dialog filled in is
                // replaced when the base (and so the commits) changed.
                if self.title.read(cx).text() == self.prefilled_title {
                    self.title
                        .update(cx, |input, cx| input.set_text(title.clone(), cx));
                    self.prefilled_title = title;
                }
                if self.body.read(cx).text() == self.prefilled_body {
                    self.body
                        .update(cx, |area, cx| area.set_text(body.clone(), cx));
                    self.prefilled_body = body;
                }
                self.range = Range::Ready {
                    subjects: read.subjects,
                    need: read.need,
                };
            }
            Err(error) => self.range = Range::Failed(error.to_string()),
        }
        self.notify_host(cx);
        cx.notify();
    }

    /// Whether there is a base the head can be merged into.
    fn has_base(&self) -> bool {
        !self.base.is_empty() && self.base != self.ctx.head
    }

    fn can_create(&self) -> bool {
        matches!(self.phase, Phase::Form)
            && self.has_base()
            && matches!(&self.range, Range::Ready { need, .. } if !need.blocks())
    }

    /// Pushes the branch when needed, then creates the request.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create() {
            return;
        }
        self.touched = true;
        let title = self.title_text(cx);
        if title.is_empty() {
            self.notify_host(cx);
            cx.notify();
            return;
        }
        let Range::Ready { need, .. } = &self.range else {
            return;
        };
        let need = *need;
        let target = self.current().clone();
        let head = self.ctx.head.clone();
        let push = if need.pushes() {
            match (self.ctx.prepare_push)(&target.remote, &head, cx) {
                Some(run) => Some(run),
                None => {
                    self.failure = Some(Failure::Push(PushFailure {
                        text: ui::t!("dialogs.pr.no_push").to_string(),
                        token_rejected: false,
                    }));
                    self.notify_host(cx);
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let new = NewPullRequest {
            head: head.clone(),
            base: self.base.clone(),
            title: title.clone(),
            body: self.body.read(cx).text().trim_end().to_owned(),
            draft: self.draft,
            reviewers: pr_logic::parse_reviewers(self.reviewers.read(cx).text()),
        };
        let store = crate::accounts::cached_store(cx);
        let _ = window;
        self.failure = None;
        self.phase = Phase::Creating {
            pushing: push.is_some(),
        };
        let draft = new.draft;
        let base = new.base.clone();
        self.notify_host(cx);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { run_create(push, store, &target, &new) })
                .await;
            this.update_in(cx, |dialog, window, cx| {
                dialog.finish(outcome, title, draft, base, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        outcome: Outcome,
        title: String,
        draft: bool,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let created = outcome.result.is_ok();
        let changed = self.ctx.changed.clone();
        if outcome.pushed {
            // The remote has the commits now: a retry must not push again.
            if let Range::Ready { need, .. } = &mut self.range {
                *need = PushNeed::UpToDate;
            }
        }
        match outcome.result {
            Ok(created) => {
                self.phase = Phase::Done(Done {
                    number: created.pull_request.number,
                    title,
                    url: created.pull_request.web_url,
                    draft,
                    base,
                    pushed: outcome.pushed,
                    reviewers_error: created.reviewers_error.map(|error| error.to_string()),
                });
            }
            Err(failure) => {
                self.phase = Phase::Form;
                self.failure = Some(failure);
            }
        }
        self.notify_host(cx);
        cx.notify();
        if outcome.pushed || created {
            changed(outcome.pushed, created, window, cx);
        }
    }

    /// Opens the existing request (or the new one) and closes the dialog.
    fn open_page(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.current().host().to_owned();
        let open = self.ctx.open_url.clone();
        self.close(window, cx);
        open(url, host, window, cx);
    }

    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.current().account_id.clone();
        (self.ctx.reconnect)(id, window, cx);
    }
}

/// The base, commits and push decision of one read.
struct RangeRead {
    /// `None` when the remote has no branch other than the head.
    base: Option<String>,
    subjects: Vec<String>,
    need: PushNeed,
}

/// Blocking git reads for [`CreatePrDialog::read_range`].
fn read_range(
    git: Arc<Git>,
    root: &Path,
    remote: &str,
    branches: &[String],
    base: Option<String>,
    head: &str,
) -> Result<RangeRead, Error> {
    let repo = Repo::open(git, root)?;
    let base = match base {
        Some(base) => Some(base),
        None => {
            let default = repo.remote_default_branch(remote)?;
            pr_logic::default_base(default.as_deref(), branches, head)
        }
    };
    let on_remote = branches.iter().any(|b| b == head);
    let (ahead, behind) = if on_remote {
        repo.ahead_behind(&format!("{remote}/{head}"), head)?
    } else {
        (0, 0)
    };
    // A request is made from what the remote has: when the local branch has
    // nothing to push, that is the remote's copy (which may be ahead of it).
    let head_ref = if on_remote && ahead == 0 {
        format!("{remote}/{head}")
    } else {
        head.to_owned()
    };
    // The remote's copy of the base when it is known, else a local branch of
    // that name; neither means no commits can be listed.
    let subjects = base
        .as_deref()
        .map(|base| {
            repo.commit_subjects(&format!("{remote}/{base}"), &head_ref)
                .or_else(|_| repo.commit_subjects(base, &head_ref))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Ok(RangeRead {
        base,
        subjects,
        need: pr_logic::push_need(on_remote, ahead, behind),
    })
}

/// Pushes (when asked) and creates the request. Blocking.
fn run_create(
    push: Option<PushRun>,
    store: Option<crate::accounts::StoreResult>,
    target: &PrTarget,
    new: &NewPullRequest,
) -> Outcome {
    let mut pushed = false;
    if let Some(run) = push {
        match run() {
            Ok(outcome) => match pr_logic::failed_target(&outcome) {
                Some(failure) => {
                    return Outcome {
                        pushed: false,
                        result: Err(Failure::Push(failure)),
                    }
                }
                None => pushed = true,
            },
            Err(error) => {
                return Outcome {
                    pushed: false,
                    result: Err(Failure::Push(pr_logic::push_failure(&error))),
                }
            }
        }
    }
    let result = create_request(store, target, new).map_err(Failure::Create);
    Outcome { pushed, result }
}

fn create_request(
    store: Option<crate::accounts::StoreResult>,
    target: &PrTarget,
    new: &NewPullRequest,
) -> Result<CreatedPullRequest, CreateFailure> {
    let store = store
        .unwrap_or_else(crate::accounts::resolve_store)
        .map_err(|error| CreateFailure::Other(error.to_string()))?;
    let (token, kind) = crate::accounts::access_credential_for_id(
        store.as_ref(),
        &target.account_id,
        crate::accounts::oauth_allowed(&target.instance),
    )
    .map_err(|error| match error {
        TokenError::Missing | TokenError::NeedsReconnect => CreateFailure::TokenRejected,
        TokenError::Unreachable(why) => CreateFailure::Offline(why),
        TokenError::Keychain(why) => CreateFailure::Other(why),
    })?;
    forge::api::create_pull_request(
        pr_logic::shared_client(),
        &target.instance,
        &target.coords,
        &Auth::new(Some(&token), kind),
        new,
    )
    .map_err(|error| pr_logic::create_failure(&error))
}

/// Opens the dialog.
pub(crate) fn open(
    ctx: PrDialogContext,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CreatePrDialog> {
    let dialog = cx.new(|cx| CreatePrDialog::new(ctx, window, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new(ui::t!("dialogs.pr.title_fallback")).icon(IconName::Pr),
    });
    // The overlay owns the host: it is released when the dialog goes away by
    // any route (buttons, Esc, the scrim), which tells the view to forget it.
    let closed = dialog.read(cx).ctx.closed.clone();
    cx.observe_release(&host, move |_, cx| closed(cx)).detach();
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog.host = Some(host.downgrade());
        cx.notify();
    });
    dialog
}

fn build(state: &Entity<CreatePrDialog>, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let target = dialog.current();
    let kind = target.kind();
    let noun = pr_logic::noun(kind, false);

    if let Phase::Done(done) = &dialog.phase {
        return build_done(state, dialog, done, colors, cx);
    }

    let running = dialog.running();
    let pushing = matches!(dialog.phase, Phase::Creating { pushing: true });
    let title_text = dialog.title.read(cx).text().trim().to_owned();
    let title_error = (dialog.touched && title_text.is_empty())
        .then(|| ui::t!("dialogs.pr.title_error").to_string());

    let mut body = div().flex().flex_col().gap(px(14.));
    if let Some(failure) = &dialog.failure {
        body = body.child(failure_banner(state, dialog, failure, colors));
    }
    match &dialog.range {
        Range::Ready { need, .. }
            if need.pushes() && !matches!(dialog.failure, Some(Failure::Push(_))) =>
        {
            let (title, text) = match need {
                PushNeed::NotOnRemote => (
                    ui::t!("dialogs.pr.not_pushed_title"),
                    ui::t!(
                        "dialogs.pr.not_pushed_body",
                        head = dialog.ctx.head.clone(),
                        remote = target.remote.clone(),
                        noun = noun.clone()
                    ),
                ),
                _ => (
                    ui::t!("dialogs.pr.unpushed_title"),
                    ui::t!(
                        "dialogs.pr.unpushed_body",
                        head = dialog.ctx.head.clone(),
                        remote = target.remote.clone(),
                        noun = noun.clone()
                    ),
                ),
            };
            body = body.child(Banner::new(BannerKind::Info, title).body(text));
        }
        Range::Ready { need, .. } if need.blocks() => {
            body = body.child(
                Banner::new(
                    BannerKind::Error,
                    ui::t!("dialogs.pr.diverged_title", remote = target.remote.clone()),
                )
                .body(ui::t!(
                    "dialogs.pr.diverged_body",
                    head = dialog.ctx.head.clone(),
                    remote = target.remote.clone(),
                    noun = noun.clone()
                )),
            );
        }
        Range::Failed(error) => {
            body = body.child(
                Banner::new(BannerKind::Error, ui::t!("dialogs.pr.read_failed"))
                    .body(error.clone())
                    .mono_body(),
            );
        }
        _ => {}
    }

    // From / Into.
    let commits_help: SharedString = match &dialog.range {
        // Without a base there is nothing to compare with: the note below says so.
        Range::Ready { .. } if !dialog.has_base() => SharedString::default(),
        Range::Ready { subjects, .. } if subjects.is_empty() => {
            ui::t!("dialogs.pr.no_commits", base = dialog.base.clone())
        }
        Range::Ready { subjects, .. } => {
            ui::i18n::t_count("dialogs.pr.commits_included", subjects.len() as u64, &[])
        }
        Range::Loading => ui::t!("dialogs.pr.reading"),
        Range::Failed(_) => SharedString::default(),
    };
    let from = field(
        ui::t!("dialogs.pr.field_from"),
        false,
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(8.))
            .rounded(R_MD)
            .border_1()
            .border_color(colors.border)
            .bg(colors.bg)
            .child(
                Icon::new(IconName::Branch)
                    .size(px(14.))
                    .color(colors.muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .child(dialog.ctx.head.clone()),
            )
            .into_any_element(),
        None,
        Some(commits_help),
        colors,
    );
    let remote_labels: Vec<String> = dialog
        .ctx
        .targets
        .iter()
        .map(|t| format!("{} · {}", t.remote, t.host()))
        .collect();
    let base_index = dialog
        .base_choices
        .iter()
        .position(|b| *b == dialog.base)
        .unwrap_or(0);
    let into = field(
        ui::t!("dialogs.pr.field_into"),
        false,
        div()
            .flex()
            .gap(px(8.))
            .child(
                div().flex_1().child(
                    Select::new("pr-remote")
                        .options(remote_labels)
                        .selected(dialog.target)
                        .disabled(running || dialog.ctx.targets.len() < 2)
                        .focus_handle(dialog.remote_focus.clone())
                        .on_change({
                            let state = state.clone();
                            move |index, _, cx| {
                                state.update(cx, |d, cx| d.set_remote(index, cx));
                            }
                        }),
                ),
            )
            .child(
                div().flex_1().child(
                    Select::new("pr-base")
                        .options(dialog.base_choices.clone())
                        .selected(base_index)
                        .disabled(running || dialog.base_choices.len() < 2)
                        .focus_handle(dialog.base_focus.clone())
                        .on_change({
                            let state = state.clone();
                            move |index, _, cx| {
                                state.update(cx, |d, cx| d.set_base(index, cx));
                            }
                        }),
                ),
            )
            .into_any_element(),
        None,
        Some(ui::t!(
            "dialogs.pr.help_as",
            login = target.login.clone(),
            host = target.host().to_owned()
        )),
        colors,
    );
    body = body.child(
        div()
            .flex()
            .gap(px(12.))
            .child(div().flex_1().min_w_0().child(from))
            .child(div().flex_1().min_w_0().child(into)),
    );
    // No branch to merge the head into: Create stays disabled, say why. The
    // note spans the full row, under both fields, so it wraps inside the dialog.
    if !dialog.has_base() && matches!(dialog.range, Range::Ready { .. }) {
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(
                    div()
                        .flex_none()
                        .mt(px(2.))
                        .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error)),
                )
                .child(div().flex_1().min_w_0().child(ui::t!(
                    "dialogs.pr.no_base",
                    head = dialog.ctx.head.clone(),
                    remote = target.remote.clone()
                ))),
        );
    }
    body = body.child(field(
        ui::t!("dialogs.pr.field_title"),
        true,
        dialog.title.clone().into_any_element(),
        title_error,
        None,
        colors,
    ));
    body = body.child(field(
        ui::t!("dialogs.pr.field_description"),
        false,
        dialog.body.clone().into_any_element(),
        None,
        Some(ui::t!("dialogs.pr.help_description")),
        colors,
    ));
    body = body.child(field(
        ui::t!("dialogs.pr.field_reviewers"),
        false,
        dialog.reviewers.clone().into_any_element(),
        None,
        Some(ui::t!("dialogs.pr.help_reviewers")),
        colors,
    ));
    body = body.child(
        Checkbox::new("pr-draft")
            .label(ui::t!("dialogs.pr.draft"))
            .checked(dialog.draft)
            .disabled(running)
            .focus_handle(dialog.draft_focus.clone())
            .on_toggle({
                let state = state.clone();
                move |checked, _, cx| {
                    state.update(cx, |d, cx| {
                        d.draft = checked;
                        d.notify_host(cx);
                        cx.notify();
                    });
                }
            }),
    );

    let primary_label: SharedString = if running {
        if pushing {
            ui::t!("dialogs.pr.pushing_creating")
        } else {
            ui::t!("dialogs.pr.creating")
        }
    } else if dialog.draft {
        ui::t!("dialogs.pr.create_draft", noun = noun.clone())
    } else {
        ui::t!("dialogs.pr.create", noun = noun.clone())
    };
    let can_submit = dialog.can_create() && !(dialog.touched && title_text.is_empty());
    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |d, cx| d.close(window, cx));
        }
    };
    let submit = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |d, cx| d.create(window, cx));
        }
    };
    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |d, cx| d.create(window, cx));
        }
    };
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("pr-cancel")
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .disabled(running)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("pr-create")
                .label(primary_label)
                .min_width(px(72.))
                .disabled(!can_submit)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(submit),
        );

    Dialog::new(ui::t!("dialogs.pr.title_create", noun = noun))
        .icon(if dialog.draft {
            IconName::PrDraft
        } else {
            IconName::Pr
        })
        .width(px(600.))
        .compact_body()
        .body(body)
        .footer(footer)
        .hint(ui::t!(
            "dialogs.pr.foot_hint",
            login = target.login.clone(),
            host = target.host().to_owned()
        ))
        .busy(running)
        .focus_order([
            DialogFocus::control(dialog.remote_focus.clone()),
            DialogFocus::control(dialog.base_focus.clone()),
            DialogFocus::input(dialog.title_focus.clone()),
            DialogFocus::input(dialog.body_focus.clone()),
            DialogFocus::input(dialog.reviewers_focus.clone()),
            DialogFocus::control(dialog.draft_focus.clone()),
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.title_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default)
}

/// The success state: what was created and "Open on <Service>".
fn build_done(
    state: &Entity<CreatePrDialog>,
    dialog: &CreatePrDialog,
    done: &Done,
    colors: ThemeColors,
    _cx: &App,
) -> Dialog {
    let target = dialog.current();
    let kind = target.kind();
    let noun = pr_logic::noun(kind, false);
    let detail = if done.draft {
        ui::t!(
            "dialogs.pr.done_detail_draft",
            noun = noun.clone(),
            host = target.host().to_owned(),
            head = dialog.ctx.head.clone(),
            base = done.base.clone()
        )
    } else {
        ui::t!(
            "dialogs.pr.done_detail",
            noun = noun.clone(),
            host = target.host().to_owned(),
            head = dialog.ctx.head.clone(),
            base = done.base.clone()
        )
    };
    let mut body = div().flex().flex_col().gap(px(14.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(12.))
            .rounded(R_MD)
            .border_1()
            .border_color(colors.success)
            .child(
                Icon::new(if done.draft {
                    IconName::PrDraft
                } else {
                    IconName::Pr
                })
                .size(px(24.))
                .color(colors.success),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("#{} {}", done.number, done.title)),
                    )
                    .child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(detail),
                    ),
            ),
    );
    if done.pushed {
        body = body.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!(
                    "dialogs.pr.pushed_note",
                    remote = target.remote.clone()
                )),
        );
    }
    if let Some(error) = &done.reviewers_error {
        body = body.child(
            Banner::new(
                BannerKind::Warn,
                ui::t!("dialogs.pr.reviewers_failed", noun = noun.clone()),
            )
            .body(error.clone()),
        );
    }
    let close = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |d, cx| d.close(window, cx));
        }
    };
    let open = {
        let state = state.clone();
        let url = done.url.clone();
        move |window: &mut Window, cx: &mut App| {
            let url = url.clone();
            state.update(cx, |d, cx| d.open_page(url, window, cx));
        }
    };
    let open_click = {
        let open = open.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| open(window, cx)
    };
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("pr-close")
                .label(ui::t!("dialogs.pr.close"))
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(close),
        )
        .child(
            Button::primary("pr-open")
                .icon(IconName::External)
                .label(ui::t!(
                    "dialogs.pr.open_on",
                    service = target.service_name()
                ))
                .focus_handle(dialog.primary_focus.clone())
                .on_click(open_click),
        );
    Dialog::new(ui::t!(
        "dialogs.pr.title_done",
        noun = pr_logic::noun(kind, true)
    ))
    .icon(IconName::Check)
    .width(px(600.))
    .compact_body()
    .body(body)
    .footer(footer)
    .focus_order([
        DialogFocus::control(dialog.cancel_focus.clone()),
        DialogFocus::control(dialog.primary_focus.clone()),
    ])
    .autofocus(dialog.primary_focus.clone())
    .default_button(dialog.primary_focus.clone())
    .on_default(open)
}

/// The banner for why the last attempt created nothing.
fn failure_banner(
    state: &Entity<CreatePrDialog>,
    dialog: &CreatePrDialog,
    failure: &Failure,
    colors: ThemeColors,
) -> AnyElement {
    let target = dialog.current();
    let noun = pr_logic::noun(target.kind(), false);
    let reconnect = |id: &'static str| {
        let state = state.clone();
        Button::new(id)
            .size(ui::ButtonSize::Small)
            .label(ui::t!("dialogs.pr.reconnect"))
            .on_click(move |_, window, cx| {
                state.update(cx, |d, cx| d.reconnect(window, cx));
            })
    };
    match failure {
        Failure::Push(push) => {
            let mut action = div().flex().flex_col().gap(px(8.)).child(
                div()
                    .max_w_full()
                    .p(px(8.))
                    .rounded(R_MD)
                    .border_1()
                    .border_color(colors.border_strong)
                    .bg(colors.bg)
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.text)
                    .child(push.text.clone()),
            );
            if push.token_rejected {
                action = action.child(div().flex().child(reconnect("pr-push-reconnect")));
            }
            Banner::new(
                BannerKind::Error,
                ui::t!("dialogs.pr.push_failed", head = dialog.ctx.head.clone()),
            )
            .body(ui::t!("dialogs.pr.push_failed_body"))
            .action(action)
            .into_any_element()
        }
        Failure::Create(CreateFailure::Exists { number, web_url }) => {
            let state = state.clone();
            let url = web_url.clone();
            Banner::new(
                BannerKind::Error,
                ui::t!("dialogs.pr.exists_title", noun = noun),
            )
            .body(ui::t!(
                "dialogs.pr.exists_body",
                number = *number,
                head = dialog.ctx.head.clone(),
                base = dialog.base.clone()
            ))
            .action(
                div().flex().child(
                    Button::new("pr-open-existing")
                        .size(ui::ButtonSize::Small)
                        .icon(IconName::External)
                        .label(ui::t!(
                            "dialogs.pr.open_existing",
                            number = *number,
                            service = target.service_name()
                        ))
                        .on_click(move |_, window, cx| {
                            let url = url.clone();
                            state.update(cx, |d, cx| d.open_page(url, window, cx));
                        }),
                ),
            )
            .into_any_element()
        }
        Failure::Create(CreateFailure::TokenRejected) => {
            Banner::new(BannerKind::Error, ui::t!("dialogs.pr.token_title"))
                .body(ui::t!(
                    "dialogs.pr.token_body",
                    login = target.login.clone(),
                    host = target.host().to_owned()
                ))
                .action(div().flex().child(reconnect("pr-reconnect")))
                .into_any_element()
        }
        Failure::Create(CreateFailure::Rejected(why)) => Banner::new(
            BannerKind::Error,
            ui::t!(
                "dialogs.pr.rejected_title",
                service = target.service_name(),
                noun = noun
            ),
        )
        .body(why.clone())
        .into_any_element(),
        Failure::Create(CreateFailure::RateLimited) => {
            Banner::new(BannerKind::Error, ui::t!("dialogs.pr.rate_title"))
                .body(ui::t!(
                    "dialogs.pr.rate_body",
                    host = target.host().to_owned()
                ))
                .into_any_element()
        }
        Failure::Create(CreateFailure::Offline(detail)) => Banner::new(
            BannerKind::Error,
            ui::t!("dialogs.pr.offline_title", host = target.host().to_owned()),
        )
        .body(detail.clone())
        .into_any_element(),
        Failure::Create(CreateFailure::Other(detail)) => Banner::new(
            BannerKind::Error,
            ui::t!("dialogs.pr.failed_title", noun = noun),
        )
        .body(detail.clone())
        .into_any_element(),
    }
}

/// A labelled field with an optional error or help line under it.
fn field(
    label: SharedString,
    required: bool,
    control: AnyElement,
    error: Option<String>,
    help: Option<SharedString>,
    colors: ThemeColors,
) -> gpui::Div {
    let mut field = div().flex().flex_col().gap(px(4.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(label)
            .when_some(required.then_some(()), |this, ()| {
                this.child(div().text_size(FS_SM).text_color(colors.error).child("*"))
            }),
    );
    field = field.child(control);
    if let Some(error) = error {
        field = field.child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                .child(error),
        );
    } else if let Some(help) = help.filter(|help| !help.is_empty()) {
        field = field.child(div().text_size(FS_SM).text_color(colors.muted).child(help));
    }
    field
}

use gpui::prelude::FluentBuilder;

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use forge::secrets::{MemoryStore, SecretStore};
    use forge::{Instance, RepoCoords, ServiceKind};

    use super::*;
    use crate::test_support::{git, open_workspace, TempRepo};

    fn run_git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit(repo: &Path, file: &str, message: &str) {
        std::fs::write(repo.join(file), message).unwrap();
        run_git(repo, &["add", file]);
        run_git(repo, &["commit", "-q", "-m", message]);
    }

    /// `main` with one commit and `feat/x` two commits ahead of it.
    fn repo_with_feature_branch(name: &str) -> TempRepo {
        let repo = TempRepo::new(name);
        run_git(repo.path(), &["checkout", "-q", "-B", "main"]);
        commit(repo.path(), "a.txt", "initial");
        run_git(repo.path(), &["checkout", "-q", "-b", "feat/x"]);
        commit(repo.path(), "b.txt", "feat: first");
        commit(repo.path(), "c.txt", "fix: second");
        repo
    }

    /// A Gitea target on a port nothing listens on: its API calls fail fast.
    fn dead_target() -> PrTarget {
        PrTarget {
            remote: "origin".to_owned(),
            account_id: "acc".to_owned(),
            login: "mira-k".to_owned(),
            instance: Instance::new(ServiceKind::Gitea, "http://127.0.0.1:9").unwrap(),
            coords: RepoCoords {
                namespace: vec!["o".to_owned()],
                repo: "r".to_owned(),
            },
        }
    }

    type Pushes = Rc<RefCell<Vec<(String, String)>>>;

    fn context(
        repo: &TempRepo,
        pushes: &Pushes,
        push_result: fn() -> Result<NetworkOutcome, Error>,
        changed: &Rc<Cell<Option<(bool, bool)>>>,
    ) -> PrDialogContext {
        PrDialogContext {
            root: repo.path().to_path_buf(),
            git: Ok(git()),
            targets: vec![dead_target()],
            target: 0,
            head: "feat/x".to_owned(),
            remote_branches: HashMap::from([(
                "origin".to_owned(),
                vec!["main".to_owned(), "release/0.3".to_owned()],
            )]),
            prepare_push: Rc::new({
                let pushes = pushes.clone();
                move |remote, branch, _| {
                    pushes
                        .borrow_mut()
                        .push((remote.to_owned(), branch.to_owned()));
                    Some(Box::new(push_result) as PushRun)
                }
            }),
            reconnect: Rc::new(|_, _, _| {}),
            open_url: Rc::new(|_, _, _, _| {}),
            changed: Rc::new({
                let changed = changed.clone();
                move |pushed, created, _, _| changed.set(Some((pushed, created)))
            }),
            closed: Rc::new(|_| {}),
        }
    }

    fn pushed_ok() -> Result<NetworkOutcome, Error> {
        Ok(NetworkOutcome::default())
    }

    fn push_rejected() -> Result<NetworkOutcome, Error> {
        Err(Error::Git {
            command: "push",
            code: Some(1),
            stderr: "! [rejected] feat/x -> feat/x (non-fast-forward)\n".to_owned(),
        })
    }

    #[gpui::test]
    fn the_dialog_prefills_from_the_commits_and_decides_to_push(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("pr-prefill");
        let (mut cx, _workspace) = open_workspace(git(), cx);
        let pushes: Pushes = Rc::default();
        let changed = Rc::new(Cell::new(None));
        let ctx = context(&repo, &pushes, pushed_ok, &changed);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();

        dialog.read_with(&cx, |dialog, cx| {
            // The remote's default is `main`: no `origin/HEAD`, so main wins.
            assert_eq!(dialog.base, "main");
            assert_eq!(dialog.base_choices, ["main", "release/0.3"]);
            assert_eq!(dialog.title.read(cx).text(), "Feat: x");
            assert_eq!(dialog.body.read(cx).text(), "- feat: first\n- fix: second");
            // `feat/x` is not among the remote's branches.
            assert!(matches!(
                dialog.range,
                Range::Ready {
                    need: PushNeed::NotOnRemote,
                    ..
                }
            ));
            assert!(dialog.can_create());
        });
    }

    #[gpui::test]
    fn an_empty_title_blocks_the_create(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("pr-title");
        let (mut cx, _workspace) = open_workspace(git(), cx);
        let pushes: Pushes = Rc::default();
        let changed = Rc::new(Cell::new(None));
        let ctx = context(&repo, &pushes, pushed_ok, &changed);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();

        dialog.update(&mut cx, |dialog, cx| {
            dialog
                .title
                .update(cx, |input, cx| input.set_text("  ", cx));
        });
        cx.update(|window, cx| dialog.update(cx, |dialog, cx| dialog.create(window, cx)));
        cx.run_until_parked();

        dialog.read_with(&cx, |dialog, _| {
            assert!(dialog.touched);
            assert!(matches!(dialog.phase, Phase::Form));
        });
        // Nothing was pushed, nothing was reported.
        assert!(pushes.borrow().is_empty());
        assert_eq!(changed.get(), None);
    }

    #[gpui::test]
    fn a_rejected_push_creates_nothing_and_shows_git_s_text(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("pr-push-fail");
        let (mut cx, _workspace) = open_workspace(git(), cx);
        let pushes: Pushes = Rc::default();
        let changed = Rc::new(Cell::new(None));
        let ctx = context(&repo, &pushes, push_rejected, &changed);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();

        cx.update(|window, cx| dialog.update(cx, |dialog, cx| dialog.create(window, cx)));
        cx.run_until_parked();
        cx.run_until_parked();

        assert_eq!(
            *pushes.borrow(),
            [("origin".to_owned(), "feat/x".to_owned())]
        );
        dialog.read_with(&cx, |dialog, _| {
            assert!(matches!(dialog.phase, Phase::Form));
            let Some(Failure::Push(failure)) = &dialog.failure else {
                panic!("expected a push failure, got {:?}", dialog.failure)
            };
            assert_eq!(
                failure.text,
                "! [rejected] feat/x -> feat/x (non-fast-forward)"
            );
        });
        assert_eq!(changed.get(), None, "a failed push changes nothing");
    }

    #[test]
    fn a_push_that_went_through_is_reported_even_when_the_create_fails() {
        let target = dead_target();
        let store = MemoryStore::new();
        store.set("acc", "token").unwrap();
        let store: crate::accounts::StoreResult = Ok(Arc::new(store));
        let new = NewPullRequest {
            head: "feat/x".to_owned(),
            base: "main".to_owned(),
            title: "Feat".to_owned(),
            body: String::new(),
            draft: false,
            reviewers: Vec::new(),
        };
        let outcome = run_create(Some(Box::new(pushed_ok)), Some(store), &target, &new);
        assert!(outcome.pushed);
        // Port 9 refuses the connection: the service cannot be reached.
        assert!(matches!(
            outcome.result,
            Err(Failure::Create(CreateFailure::Offline(_)))
        ));
    }

    #[test]
    fn without_a_token_the_create_asks_to_reconnect() {
        let target = dead_target();
        let store: crate::accounts::StoreResult = Ok(Arc::new(MemoryStore::new()));
        let new = NewPullRequest {
            head: "feat/x".to_owned(),
            base: "main".to_owned(),
            title: "Feat".to_owned(),
            body: String::new(),
            draft: false,
            reviewers: Vec::new(),
        };
        let outcome = run_create(None, Some(store), &target, &new);
        assert!(!outcome.pushed);
        assert!(matches!(
            outcome.result,
            Err(Failure::Create(CreateFailure::TokenRejected))
        ));
    }

    #[gpui::test]
    fn on_the_default_branch_the_base_is_another_branch_or_none(cx: &mut gpui::TestAppContext) {
        let repo = repo_with_feature_branch("pr-base-main");
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        let (mut cx, _workspace) = open_workspace(git(), cx);
        let pushes: Pushes = Rc::default();
        let changed = Rc::new(Cell::new(None));

        // The user is on main; the remote also has develop.
        let mut ctx = context(&repo, &pushes, pushed_ok, &changed);
        ctx.head = "main".to_owned();
        ctx.remote_branches = HashMap::from([(
            "origin".to_owned(),
            vec!["main".to_owned(), "develop".to_owned()],
        )]);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.base, "develop");
            assert!(dialog.has_base());
        });

        // Main is the only branch: nothing to merge into, Create is off.
        let mut ctx = context(&repo, &pushes, pushed_ok, &changed);
        ctx.head = "main".to_owned();
        ctx.remote_branches = HashMap::from([("origin".to_owned(), vec!["main".to_owned()])]);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert!(!dialog.has_base());
            assert!(!dialog.can_create());
        });
    }

    #[gpui::test]
    fn a_diverged_branch_cannot_be_created_and_a_pushed_one_is_not_pushed_again(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = repo_with_feature_branch("pr-diverged");
        // main + one commit, unrelated to feat/x's own two.
        run_git(repo.path(), &["checkout", "-q", "-b", "other", "main"]);
        commit(repo.path(), "d.txt", "remote only");
        run_git(repo.path(), &["checkout", "-q", "feat/x"]);
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/feat/x", "other"],
        );
        let (mut cx, _workspace) = open_workspace(git(), cx);
        let pushes: Pushes = Rc::default();
        let changed = Rc::new(Cell::new(None));
        let mut ctx = context(&repo, &pushes, pushed_ok, &changed);
        ctx.remote_branches = HashMap::from([(
            "origin".to_owned(),
            vec!["main".to_owned(), "feat/x".to_owned()],
        )]);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert!(matches!(
                dialog.range,
                Range::Ready {
                    need: PushNeed::Diverged,
                    ..
                }
            ));
            assert!(!dialog.can_create());
        });
        cx.update(|window, cx| dialog.update(cx, |dialog, cx| dialog.create(window, cx)));
        cx.run_until_parked();
        assert!(pushes.borrow().is_empty(), "a doomed push is not tried");

        // A remote copy that is only ahead: no push; one that is behind the
        // local branch: pushed once, and a failed create keeps it pushed.
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/feat/x", "feat/x~1"],
        );
        let mut ctx = context(&repo, &pushes, pushed_ok, &changed);
        ctx.remote_branches = HashMap::from([(
            "origin".to_owned(),
            vec!["main".to_owned(), "feat/x".to_owned()],
        )]);
        let dialog = cx.update(|window, cx| open(ctx, window, cx));
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert!(matches!(
                dialog.range,
                Range::Ready {
                    need: PushNeed::Ahead(1),
                    ..
                }
            ));
        });
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.finish(
                    Outcome {
                        pushed: true,
                        result: Err(Failure::Create(CreateFailure::RateLimited)),
                    },
                    "t".to_owned(),
                    false,
                    "main".to_owned(),
                    window,
                    cx,
                )
            })
        });
        dialog.read_with(&cx, |dialog, _| {
            assert!(matches!(
                dialog.range,
                Range::Ready {
                    need: PushNeed::UpToDate,
                    ..
                }
            ));
        });
        assert_eq!(changed.get(), Some((true, false)));
    }

    #[test]
    fn a_remote_that_is_ahead_is_read_from_its_own_copy() {
        let repo = repo_with_feature_branch("pr-behind");
        run_git(repo.path(), &["checkout", "-q", "-b", "tmp"]);
        commit(repo.path(), "e.txt", "remote extra");
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/feat/x", "tmp"],
        );
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        run_git(repo.path(), &["checkout", "-q", "feat/x"]);
        let branches = vec!["main".to_owned(), "feat/x".to_owned()];
        let read = read_range(git(), repo.path(), "origin", &branches, None, "feat/x").unwrap();
        assert_eq!(read.need, PushNeed::Behind(1));
        assert_eq!(
            read.subjects,
            ["feat: first", "fix: second", "remote extra"]
        );
    }

    #[test]
    fn reading_the_range_finds_the_commits_and_a_pushed_branch() {
        let repo = repo_with_feature_branch("pr-range");
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        run_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/feat/x", "feat/x~1"],
        );
        let branches = vec!["main".to_owned(), "feat/x".to_owned()];
        let read = read_range(git(), repo.path(), "origin", &branches, None, "feat/x").unwrap();
        assert_eq!(read.base.as_deref(), Some("main"));
        // Both commits are ahead of main, one of them is not on the remote.
        assert_eq!(read.subjects, ["feat: first", "fix: second"]);
        assert_eq!(read.need, PushNeed::Ahead(1));
    }
}
