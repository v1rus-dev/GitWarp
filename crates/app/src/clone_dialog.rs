//! The Clone repository dialog: form, live validation, host-key/credential
//! prompts through the network service, progress, cancellation and the result.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use forge::api::{RepoListing, RepoSummary};

use git_core::network::{self, CancelToken, CloneKind, CloneOptions, CloneTarget, NetworkContext};
use git_core::{Error, Git, Repo};
use ui::actions::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::metrics::{FONT_MONO, FS_SM, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Chip, Dialog, DialogFocus,
    DialogHost, Icon, IconName, Progress, SegmentItem, Segmented, Select, Skeleton, Spinner,
    TextInput, TextInputEvent, ToastKind,
};

use crate::accounts::{self, CloneTab, RepoListError};
use crate::format::{display_path, expand_home, home_dir, relative_time};
use crate::network_ops::{self, friendly_error};
use crate::network_service::{HostKeyAsker, JobAuth, NetworkService};
use crate::open_dialog::{pick_folder, Browse};
use crate::settings::AccountRecord;
use crate::settings_ui::account_dialog::{self, AccountDialog, Open as AccountOpen};
use crate::workspace::{repo_name, Workspace};

const CLONE_TIMEOUT: Duration = Duration::from_millis(80);

/// How the keyboard moves the selection in the repository list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Next,
    Prev,
    First,
    Last,
}

/// The repository selected after `movement`, from the indexes the search
/// shows. A selection the search hides counts as none; the ends clamp.
fn next_selection(visible: &[usize], current: Option<usize>, movement: Move) -> Option<usize> {
    let last = visible.len().checked_sub(1)?;
    let position = current.and_then(|c| visible.iter().position(|i| *i == c));
    let target = match (movement, position) {
        (Move::First, _) | (Move::Next, None) => 0,
        (Move::Last, _) | (Move::Prev, None) => last,
        (Move::Next, Some(p)) => (p + 1).min(last),
        (Move::Prev, Some(p)) => p.saturating_sub(1),
    };
    visible.get(target).copied()
}

/// What the dialog is doing.
enum Phase {
    Form,
    Cloning,
    Done(CloneDone),
    Failed(CloneFailure),
}

/// What the "From account" list shows for the selected account.
enum RepoList {
    Loading,
    Loaded {
        repos: Vec<RepoSummary>,
        /// The account has more repositories than were fetched.
        truncated: bool,
    },
    Failed(RepoListError),
}

struct CloneFailure {
    message: SharedString,
    cancelled: bool,
}

struct CloneDone {
    path: PathBuf,
    branch: Option<String>,
    remote: Option<String>,
    commits: u64,
    branches: usize,
    tags: usize,
}

enum Event {
    Progress(network::Progress),
    Done(Result<CloneDone, CloneFailure>),
}

pub(crate) struct CloneDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    workspace: WeakEntity<Workspace>,
    git: Result<Arc<Git>, Arc<Error>>,
    home: Option<PathBuf>,

    url: Entity<TextInput>,
    dest: Entity<TextInput>,
    name: Entity<TextInput>,
    branch: Entity<TextInput>,
    depth: Entity<TextInput>,
    _url_events: Subscription,
    _dest_events: Subscription,
    _name_events: Subscription,
    _branch_events: Subscription,
    _depth_events: Subscription,

    url_text: String,
    dest_text: String,
    name_text: String,
    branch_text: String,
    depth_text: String,
    url_touched: bool,
    name_edited: bool,
    updating_name: bool,
    recurse: bool,
    shallow: bool,

    tab: CloneTab,
    accounts: Vec<AccountRecord>,
    account_index: usize,
    repos: RepoList,
    /// Indexes of the repositories the search shows; recomputed when the
    /// query changes or a list loads, not on every render.
    visible: Vec<usize>,
    /// Index into the loaded repositories (not into the filtered list).
    repo_selected: Option<usize>,
    query: Entity<TextInput>,
    _query_events: Subscription,
    query_text: String,
    /// Bumped by every load and account switch; a late answer is dropped.
    list_generation: u64,
    /// Keeps the Reconnect dialog alive; its layer holds only a weak handle.
    account_dialog: Option<Entity<AccountDialog>>,
    list_scroll: ScrollHandle,

    phase: Phase,
    progress: Option<f32>,
    phase_text: SharedString,
    cancel: CancelToken,
    generation: u64,

    url_tab_focus: gpui::FocusHandle,
    account_tab_focus: gpui::FocusHandle,
    account_focus: gpui::FocusHandle,
    query_focus: gpui::FocusHandle,
    list_focus: gpui::FocusHandle,
    url_focus: gpui::FocusHandle,
    dest_focus: gpui::FocusHandle,
    browse_focus: gpui::FocusHandle,
    name_focus: gpui::FocusHandle,
    branch_focus: gpui::FocusHandle,
    depth_focus: gpui::FocusHandle,
    checkbox_focus: gpui::FocusHandle,
    shallow_focus: gpui::FocusHandle,
    cancel_focus: gpui::FocusHandle,
    primary_focus: gpui::FocusHandle,
    secondary_focus: gpui::FocusHandle,
}

impl CloneDialog {
    fn new(
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let home = home_dir();
        let default_dest = home
            .as_ref()
            .map(|home| display_path(home, Some(home)))
            .unwrap_or_default();
        let url = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("https://host/user/repo.git", cx);
            input
        });
        let dest = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_text(default_dest.clone(), cx);
            input.set_placeholder("~/develop", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let name = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("repo", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let branch = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("welcome.init.default_branch"), cx);
            input
        });
        let depth = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_text("1", cx);
            input.set_placeholder("1", cx);
            input
        });
        let query = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder(ui::t!("welcome.clone.search_placeholder"), cx);
            input.set_disabled(true, cx);
            input
        });
        let query_focus = query.read(cx).focus_handle().clone();
        let query_events = cx.subscribe(&query, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.query_text = text.clone();
                this.refresh_visible();
                cx.notify();
            }
        });
        let accounts = crate::settings_ui::current_settings(cx).accounts;
        let tab = accounts::default_clone_tab(accounts.len());
        let url_focus = url.read(cx).focus_handle().clone();
        let dest_focus = dest.read(cx).focus_handle().clone();
        let name_focus = name.read(cx).focus_handle().clone();
        let branch_focus = branch.read(cx).focus_handle().clone();
        let depth_focus = depth.read(cx).focus_handle().clone();

        let url_events = cx.subscribe(&url, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.url_text = text.clone();
                this.url_touched = true;
                if !this.name_edited {
                    this.apply_suggested_name(cx);
                }
                cx.notify();
            }
        });
        let dest_events = cx.subscribe(&dest, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.dest_text = text.clone();
                cx.notify();
            }
        });
        let name_events = cx.subscribe(&name, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.name_text = text.clone();
                if !this.updating_name {
                    this.name_edited = true;
                }
                cx.notify();
            }
        });
        let branch_events = cx.subscribe(&branch, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.branch_text = text.clone();
                cx.notify();
            }
        });
        let depth_events = cx.subscribe(&depth, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.depth_text = text.clone();
                cx.notify();
            }
        });

        let mut this = Self {
            layer: None,
            host: None,
            workspace,
            git,
            home,
            url,
            dest,
            name,
            branch,
            depth,
            _url_events: url_events,
            _dest_events: dest_events,
            _name_events: name_events,
            _branch_events: branch_events,
            _depth_events: depth_events,
            url_text: String::new(),
            dest_text: default_dest,
            name_text: String::new(),
            branch_text: String::new(),
            depth_text: "1".to_owned(),
            url_touched: false,
            name_edited: false,
            updating_name: false,
            recurse: false,
            shallow: false,
            tab,
            accounts,
            account_index: 0,
            repos: RepoList::Loading,
            visible: Vec::new(),
            repo_selected: None,
            query,
            _query_events: query_events,
            query_text: String::new(),
            list_generation: 0,
            account_dialog: None,
            list_scroll: ScrollHandle::new(),
            phase: Phase::Form,
            progress: None,
            phase_text: SharedString::default(),
            cancel: CancelToken::new(),
            generation: 0,
            url_tab_focus: cx.focus_handle().tab_stop(true),
            account_tab_focus: cx.focus_handle().tab_stop(true),
            account_focus: cx.focus_handle().tab_stop(true),
            query_focus,
            list_focus: cx.focus_handle().tab_stop(true),
            url_focus,
            dest_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            name_focus,
            branch_focus,
            depth_focus,
            checkbox_focus: cx.focus_handle().tab_stop(true),
            shallow_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            secondary_focus: cx.focus_handle().tab_stop(true),
        };
        if this.tab == CloneTab::Account {
            this.load_repos(cx);
        }
        this
    }

    /// The address that will be cloned: the typed one on the URL tab, the
    /// selected repository's HTTPS address on the account tab.
    fn current_url(&self) -> String {
        match self.tab {
            CloneTab::Url => self.url_text.clone(),
            CloneTab::Account => self
                .selected_repo()
                .map(|repo| repo.clone_https.clone())
                .unwrap_or_default(),
        }
    }

    fn selected_repo(&self) -> Option<&RepoSummary> {
        match &self.repos {
            RepoList::Loaded { repos, .. } => repos.get(self.repo_selected?),
            _ => None,
        }
    }

    /// Indexes of the repositories the search lets through.
    fn visible_repos(&self) -> &[usize] {
        &self.visible
    }

    fn refresh_visible(&mut self) {
        self.visible = match &self.repos {
            RepoList::Loaded { repos, .. } => accounts::filter_repos(repos, &self.query_text),
            _ => Vec::new(),
        };
    }

    /// Whether the list is shown with rows to pick from.
    fn list_has_rows(&self) -> bool {
        !self.visible.is_empty()
    }

    fn current_account(&self) -> Option<&AccountRecord> {
        self.accounts.get(self.account_index)
    }

    /// Starts loading the repositories of the selected account. The token is
    /// read and used on the background executor only.
    fn load_repos(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.current_account().cloned() else {
            return;
        };
        self.list_generation = self.list_generation.wrapping_add(1);
        let generation = self.list_generation;
        self.repos = RepoList::Loading;
        self.refresh_visible();
        self.repo_selected = None;
        self.query
            .update(cx, |input, cx| input.set_disabled(true, cx));
        let store = accounts::cached_store(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = store.unwrap_or_else(accounts::resolve_store);
                    accounts::list_account_repos(&store, &account)
                })
                .await;
            this.update(cx, |dialog, cx| dialog.finish_load(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_load(
        &mut self,
        generation: u64,
        result: Result<RepoListing, RepoListError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.list_generation {
            return;
        }
        let loaded = result.is_ok();
        self.repos = match result {
            Ok(listing) => RepoList::Loaded {
                repos: listing.repos,
                truncated: listing.truncated,
            },
            Err(error) => RepoList::Failed(error),
        };
        self.refresh_visible();
        self.query
            .update(cx, |input, cx| input.set_disabled(!loaded, cx));
        cx.notify();
    }

    /// Switches account: selection and search start over, the list reloads.
    fn select_account(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.account_index || index >= self.accounts.len() {
            return;
        }
        self.account_index = index;
        self.reset_selection(cx);
        self.load_repos(cx);
    }

    fn reset_selection(&mut self, cx: &mut Context<Self>) {
        self.repo_selected = None;
        self.query_text.clear();
        self.refresh_visible();
        self.query.update(cx, |input, cx| input.set_text("", cx));
        self.name_edited = false;
        self.apply_suggested_name(cx);
    }

    fn set_tab(&mut self, tab: CloneTab, cx: &mut Context<Self>) {
        if tab == self.tab || (tab == CloneTab::Account && self.accounts.is_empty()) {
            return;
        }
        self.tab = tab;
        if !self.name_edited {
            self.apply_suggested_name(cx);
        }
        cx.notify();
    }

    fn select_repo(&mut self, index: usize, cx: &mut Context<Self>) {
        if !matches!(&self.repos, RepoList::Loaded { repos, .. } if index < repos.len()) {
            return;
        }
        self.repo_selected = Some(index);
        // A new repository brings its own folder name.
        self.name_edited = false;
        self.apply_suggested_name(cx);
        if let Some(position) = self.visible.iter().position(|i| *i == index) {
            self.list_scroll.scroll_to_item(position);
        }
        cx.notify();
    }

    /// ↑ / ↓ / Home / End in the list move the selection through the
    /// repositories the search shows.
    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        if let Some(index) = next_selection(&self.visible, self.repo_selected, movement) {
            self.select_repo(index, cx);
        }
    }

    /// Opens the account dialog on the token step; the list reloads when the
    /// new token is saved.
    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.current_account().cloned() else {
            return;
        };
        let this = cx.weak_entity();
        let on_saved: account_dialog::OnSaved = std::rc::Rc::new(move |record, _, cx| {
            let id = record.id.clone();
            this.update(cx, |dialog, cx| dialog.reconnected(&id, cx))
                .ok();
        });
        self.account_dialog = Some(account_dialog::open(
            AccountOpen::Reconnect(account),
            None,
            Some(on_saved),
            window,
            cx,
        ));
    }

    /// The token of account `id` was replaced: reload its list.
    fn reconnected(&mut self, id: &str, cx: &mut Context<Self>) {
        // The login follows the token on Reconnect: take the saved record.
        let saved = crate::settings_ui::current_settings(cx).accounts;
        if let Some(index) = saved.iter().position(|account| account.id == id) {
            self.accounts = saved;
            self.account_index = index;
            self.load_repos(cx);
        }
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.clone();
        let picked = pick_folder(cx);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = picked.await;
            this.update_in(cx, |dialog, window, cx| {
                dialog.finish_browse(outcome, home, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_browse(
        &mut self,
        outcome: Browse,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A picker that was already open when the clone started must not
        // change the folder the clone is running for.
        if matches!(self.phase, Phase::Cloning | Phase::Done(_)) {
            return;
        }
        match outcome {
            // `set_text` emits `Changed`, which the `dest_events` subscription
            // turns into `dest_text` and a re-render.
            Browse::Picked(Some(path)) => {
                let label = display_path(&path, home.as_deref());
                self.dest.update(cx, |input, cx| input.set_text(label, cx));
            }
            Browse::Picked(None) | Browse::Cancelled => {}
            Browse::Failed(message) => {
                // No xdg portal, no picker: the field stays the way in.
                toast::show(
                    ToastKind::Error,
                    ui::t!("welcome.open.picker_failed", message = message),
                    window,
                    cx,
                );
            }
        }
    }

    fn apply_suggested_name(&mut self, cx: &mut Context<Self>) {
        let suggested = network::suggested_folder(&self.current_url()).unwrap_or_default();
        if suggested == self.name_text {
            return;
        }
        self.name_text = suggested.clone();
        self.updating_name = true;
        self.name
            .update(cx, |input, cx| input.set_text(suggested, cx));
        self.updating_name = false;
    }

    fn target(&self) -> Option<CloneTarget> {
        network::clone_target(&self.current_url())
    }

    fn url_error(&self) -> Option<SharedString> {
        if self.current_url().trim().is_empty() {
            return None;
        }
        if self.target().is_none() {
            return Some(ui::t!("welcome.clone.bad_url"));
        }
        None
    }

    fn destination(&self) -> PathBuf {
        let parent = expand_home(self.dest_text.trim(), self.home.as_deref());
        let name = self.name_text.trim();
        if name.is_empty() {
            parent
        } else {
            parent.join(name)
        }
    }

    fn name_error(&self) -> Option<SharedString> {
        let name = self.name_text.trim();
        if name.is_empty() {
            return None;
        }
        if name.contains('/') || name.contains('\\') {
            return Some(ui::t!("welcome.clone.name_slashes"));
        }
        let path = self.destination();
        if path.exists() {
            return Some(ui::t!("welcome.clone.path_exists", path = path.display()));
        }
        None
    }

    fn depth_value(&self) -> Result<Option<u32>, ()> {
        // The account tab has no depth field: a shallow flag left over from
        // the URL tab must not apply to it.
        if !self.shallow || self.tab == CloneTab::Account {
            return Ok(None);
        }
        match self.depth_text.trim().parse::<u32>() {
            Ok(depth) if depth > 0 => Ok(Some(depth)),
            _ => Err(()),
        }
    }

    fn ready(&self) -> bool {
        !self.current_url().trim().is_empty()
            && self.target().is_some()
            && !self.name_text.trim().is_empty()
            && self.name_error().is_none()
            && !self.dest_text.trim().is_empty()
            && self.depth_value().is_ok()
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready() || !matches!(self.phase, Phase::Form | Phase::Failed(_)) {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.fail(friendly_error(error), false, window, cx);
                return;
            }
        };
        let current = self.current_url();
        let raw_url = current.trim();
        let url = if raw_url.starts_with('~') {
            expand_home(raw_url, self.home.as_deref())
                .display()
                .to_string()
        } else {
            raw_url.to_owned()
        };
        // The account tab clones with default options: its fields are only
        // the repository, the folder and the folder name.
        let from_account = self.tab == CloneTab::Account;
        let options = CloneOptions {
            url,
            dest: self.destination(),
            branch: {
                let branch = self.branch_text.trim();
                (!branch.is_empty() && !from_account).then(|| branch.to_owned())
            },
            depth: self.depth_value().ok().flatten(),
            recurse_submodules: self.recurse && !from_account,
        };
        // The selected account answers git's credential prompts for its host,
        // so a private repository clones without a password dialog.
        let credential = self
            .current_account()
            .filter(|_| from_account)
            .and_then(|account| accounts::clone_credential(account, &options.url));
        let (context, asker, guard) = match cx.try_global::<NetworkService>() {
            Some(service) => {
                let (context, guard) = match credential {
                    Some((host, credential)) => service.job_context(JobAuth {
                        interactive: true,
                        accounts: std::collections::HashMap::from([(host, credential)]),
                        store: match accounts::cached_store(cx) {
                            Some(Ok(store)) => Some(store),
                            _ => None,
                        },
                    }),
                    None => (service.context(true), None),
                };
                (context, service.host_keys(), guard)
            }
            None => (NetworkContext::default(), HostKeyAsker::unavailable(), None),
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        self.phase = Phase::Cloning;
        self.progress = None;
        self.phase_text = ui::t!("welcome.clone.connecting");
        cx.notify();

        let (tx, rx) = mpsc::channel();
        let progress_tx = tx.clone();
        let worker = cx.background_executor().spawn(async move {
            // Keeps the account registered until the clone has ended.
            let _guard = guard;
            let result =
                network_ops::clone(&git, &options, &context, &asker, &cancel, |progress| {
                    let _ = progress_tx.send(Event::Progress(progress));
                })
                .map_err(|error| CloneFailure {
                    message: friendly_error(&error).into(),
                    cancelled: matches!(error, Error::Cancelled { .. }),
                })
                .and_then(|outcome| {
                    let _ = outcome;
                    summarize(&git, &options)
                });
            let _ = tx.send(Event::Done(result));
        });
        worker.detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(CLONE_TIMEOUT).await;
            let done = this
                .update_in(cx, |dialog, window, cx| {
                    dialog.poll_events(&rx, generation, window, cx)
                })
                .unwrap_or(true);
            if done {
                break;
            }
        })
        .detach();
    }

    /// Returns whether the dialog has left the cloning phase.
    fn poll_events(
        &mut self,
        rx: &Receiver<Event>,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.generation {
            return true;
        }
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Progress(progress) => {
                    if let Some(fraction) = progress.fraction() {
                        self.progress = Some(fraction);
                    }
                    let text = progress.text.trim();
                    if !text.is_empty() {
                        self.phase_text = text.to_owned().into();
                    }
                }
                Event::Done(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            match result {
                Ok(done) => {
                    self.progress = Some(1.0);
                    let path = done.path.clone();
                    crate::update_settings(cx, |settings| {
                        settings.touch_recent(path, crate::history_view::unix_now())
                    });
                    self.phase = Phase::Done(done);
                }
                Err(failure) => {
                    if failure.cancelled {
                        toast::show(
                            ToastKind::Info,
                            ui::t!("welcome.clone.cancelled"),
                            window,
                            cx,
                        );
                        self.generation = self.generation.wrapping_add(1);
                        self.phase = Phase::Form;
                        self.progress = None;
                    } else {
                        self.phase = Phase::Failed(failure);
                    }
                }
            }
            cx.notify();
            return true;
        }
        cx.notify();
        false
    }

    fn fail(
        &mut self,
        message: impl Into<SharedString>,
        cancelled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cancelled {
            toast::show(
                ToastKind::Info,
                ui::t!("welcome.clone.cancelled"),
                window,
                cx,
            );
        }
        self.phase = Phase::Failed(CloneFailure {
            message: message.into(),
            cancelled,
        });
        cx.notify();
    }

    fn back_to_form(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.phase = Phase::Form;
        self.progress = None;
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.phase {
            Phase::Cloning => {
                self.cancel.cancel();
                self.phase_text = ui::t!("welcome.clone.cancelling");
                cx.notify();
            }
            Phase::Done(_) => self.close(window, cx),
            Phase::Failed(_) => self.back_to_form(cx),
            Phase::Form => self.close(window, cx),
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn open_done(&mut self, done: &CloneDone, window: &mut Window, cx: &mut Context<Self>) {
        let path = done.path.clone();
        let workspace = self.workspace.clone();
        self.close(window, cx);
        if let Some(workspace) = workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx);
            });
        }
    }

    pub(crate) fn layer_gone(&mut self, window: &Window, cx: &App) -> bool {
        let Some(layer) = self.layer else {
            return true;
        };
        overlay::layer_view(layer, window, cx).is_none()
    }
}

fn summarize(git: &Arc<Git>, options: &CloneOptions) -> Result<CloneDone, CloneFailure> {
    let repo = Repo::open(git.clone(), &options.dest).map_err(|error| CloneFailure {
        message: friendly_error(&error).into(),
        cancelled: false,
    })?;
    let refs = repo.refs().unwrap_or_default();
    let branches = refs
        .iter()
        .filter(|reference| reference.kind == git_core::RefKind::LocalBranch)
        .count();
    let tags = refs
        .iter()
        .filter(|reference| reference.kind == git_core::RefKind::Tag)
        .count();
    let branch = match repo.head() {
        Ok(git_core::Head::Branch { name, .. }) => Some(name),
        _ => None,
    };
    Ok(CloneDone {
        path: options.dest.clone(),
        branch,
        remote: repo.remote_url("origin").ok().flatten(),
        commits: repo.commit_count().unwrap_or(0),
        branches,
        tags,
    })
}

pub(crate) fn open(
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CloneDialog> {
    let dialog = cx.new(|cx| CloneDialog::new(git, workspace, window, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new(ui::t!("welcome.clone.title")),
    });
    dialog.update(cx, |dialog, _| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
    });
    dialog
}

fn build(state: &Entity<CloneDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.cancel(window, cx));
        }
    };
    let start = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(window, cx));
        }
    };

    match &dialog.phase {
        Phase::Form | Phase::Failed(_) => build_form(state, colors, start, cancel, cx),
        Phase::Cloning => {
            let primary = {
                let state = state.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| dialog.cancel(window, cx));
                }
            };
            let body = div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(Icon::new(IconName::Folder).size(px(14.)))
                        .child(dialog.destination().display().to_string()),
                )
                .child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(dialog.current_url()),
                )
                .child(match dialog.progress {
                    Some(value) => Progress::new("clone-progress", value).dialog().fill(),
                    None => Progress::new("clone-progress", 0.)
                        .dialog()
                        .fill()
                        .indeterminate(),
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(Spinner::new("clone-spinner", px(12.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(dialog.phase_text.clone()),
                        ),
                );

            let footer = div().flex().items_center().gap(px(8.)).child(
                Button::new("clone-cancel-running")
                    .label(ui::t!("welcome.clone.cancel_running"))
                    .min_width(px(96.))
                    .focus_handle(dialog.primary_focus.clone())
                    .on_click(primary),
            );
            Dialog::new(ui::t!("welcome.clone.title_running"))
                .icon(IconName::Clone)
                .width(px(520.))
                .compact_body()
                .body(body)
                .footer(footer)
                .busy(true)
                .focus_order(vec![DialogFocus::control(dialog.primary_focus.clone())])
                .autofocus(dialog.primary_focus.clone())
        }
        Phase::Done(done) => {
            let open = {
                let state = state.clone();
                let done = CloneDone {
                    path: done.path.clone(),
                    branch: done.branch.clone(),
                    remote: done.remote.clone(),
                    commits: done.commits,
                    branches: done.branches,
                    tags: done.tags,
                };
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| dialog.open_done(&done, window, cx));
                }
            };
            let close = {
                let state = state.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| dialog.close(window, cx));
                }
            };
            let details = |label: SharedString, value: String, colors: ThemeColors| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .text_size(FS_SM)
                    .child(
                        div()
                            .w(px(84.))
                            .flex_none()
                            .text_color(colors.muted)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(FONT_MONO)
                            .text_color(colors.text)
                            .child(value),
                    )
            };
            let body = div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    Banner::new(
                        BannerKind::Ok,
                        ui::t!("welcome.clone.done_title", name = repo_name(&done.path)),
                    )
                    .body(ui::t!(
                        "welcome.clone.done_summary",
                        commits = ui::i18n::t_count("welcome.clone.commits", done.commits, &[]),
                        branches = ui::i18n::t_count(
                            "welcome.clone.branches",
                            u64::try_from(done.branches).unwrap_or(u64::MAX),
                            &[]
                        ),
                        tags = ui::i18n::t_count(
                            "welcome.clone.tags",
                            u64::try_from(done.tags).unwrap_or(u64::MAX),
                            &[]
                        )
                    )),
                )
                .child(details(
                    ui::t!("welcome.clone.location"),
                    done.path.display().to_string(),
                    colors,
                ))
                .child(details(
                    ui::t!("welcome.clone.branch"),
                    done.branch.clone().unwrap_or_else(|| "HEAD".to_owned()),
                    colors,
                ))
                .child(details(
                    ui::t!("welcome.clone.remote"),
                    done.remote
                        .clone()
                        .map(|url| format!("origin → {url}"))
                        .unwrap_or_else(|| "origin".to_owned()),
                    colors,
                ));
            let footer = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("clone-close")
                        .label(ui::t!("welcome.clone.close"))
                        .min_width(px(72.))
                        .focus_handle(dialog.secondary_focus.clone())
                        .on_click(close),
                )
                .child(
                    Button::primary("clone-open")
                        .label(ui::t!("welcome.init.open"))
                        .min_width(px(72.))
                        .focus_handle(dialog.primary_focus.clone())
                        .on_click(open),
                );
            Dialog::new(ui::t!("welcome.clone.title_done"))
                .icon(IconName::Check)
                .width(px(520.))
                .compact_body()
                .body(body)
                .footer(footer)
                .focus_order(vec![
                    DialogFocus::control(dialog.secondary_focus.clone()),
                    DialogFocus::control(dialog.primary_focus.clone()),
                ])
                .autofocus(dialog.primary_focus.clone())
                .default_button(dialog.primary_focus.clone())
                .on_default({
                    let state = state.clone();
                    let done = CloneDone {
                        path: done.path.clone(),
                        branch: done.branch.clone(),
                        remote: done.remote.clone(),
                        commits: done.commits,
                        branches: done.branches,
                        tags: done.tags,
                    };
                    move |window: &mut Window, cx: &mut App| {
                        state.update(cx, |dialog, cx| dialog.open_done(&done, window, cx));
                    }
                })
        }
    }
}

fn build_form(
    state: &Entity<CloneDialog>,
    colors: ThemeColors,
    start: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Dialog {
    let dialog = state.read(cx);
    let has_tabs = !dialog.accounts.is_empty();
    let account_tab_active = dialog.tab == CloneTab::Account;

    let mut body = div().flex().flex_col().gap(px(12.));
    if has_tabs {
        body = body.child(source_tabs(state, dialog));
    }
    body = body.child(if account_tab_active {
        account_tab(state, dialog, colors).into_any_element()
    } else {
        url_tab(state, dialog, colors).into_any_element()
    });

    if let Phase::Failed(failure) = &dialog.phase {
        body = body.child(
            Banner::new(
                if failure.cancelled {
                    BannerKind::Info
                } else {
                    BannerKind::Error
                },
                ui::t!("welcome.clone.failed"),
            )
            .body(failure.message.clone()),
        );
    }

    let ready = dialog.ready();
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("clone-cancel")
                .label(ui::t!("welcome.common.cancel"))
                .min_width(px(72.))
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("clone-start")
                .label(if matches!(dialog.phase, Phase::Failed(_)) {
                    ui::t!("welcome.clone.retry")
                } else {
                    ui::t!("welcome.clone.start")
                })
                .min_width(px(72.))
                .disabled(!ready)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(start),
        );

    let mut focus_order = Vec::new();
    if has_tabs {
        focus_order.push(DialogFocus::control(if account_tab_active {
            dialog.account_tab_focus.clone()
        } else {
            dialog.url_tab_focus.clone()
        }));
    }
    if account_tab_active {
        focus_order.push(DialogFocus::control(dialog.account_focus.clone()));
        // Only what is rendered and enabled: the search once a list is
        // loaded, the list once it has rows.
        if matches!(dialog.repos, RepoList::Loaded { .. }) {
            focus_order.push(DialogFocus::input(dialog.query_focus.clone()));
        }
        if dialog.list_has_rows() {
            focus_order.push(DialogFocus::control(dialog.list_focus.clone()));
        }
        if dialog.selected_repo().is_some() {
            focus_order.push(DialogFocus::input(dialog.dest_focus.clone()));
            focus_order.push(DialogFocus::control(dialog.browse_focus.clone()));
            focus_order.push(DialogFocus::input(dialog.name_focus.clone()));
        }
    } else {
        focus_order.extend([
            DialogFocus::input(dialog.url_focus.clone()),
            DialogFocus::input(dialog.dest_focus.clone()),
            DialogFocus::control(dialog.browse_focus.clone()),
            DialogFocus::input(dialog.name_focus.clone()),
            DialogFocus::input(dialog.branch_focus.clone()),
            DialogFocus::input(dialog.depth_focus.clone()),
            DialogFocus::control(dialog.checkbox_focus.clone()),
            DialogFocus::control(dialog.shallow_focus.clone()),
        ]);
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    Dialog::new(ui::t!("welcome.clone.title"))
        .icon(IconName::Clone)
        // The design's dialog is 600 wide once it has the source tabs.
        .width(px(if has_tabs { 600. } else { 560. }))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(false)
        .focus_order(focus_order)
        .autofocus(match (account_tab_active, has_tabs) {
            (true, _) => dialog.account_focus.clone(),
            (false, _) => dialog.url_focus.clone(),
        })
        .default_button(dialog.primary_focus.clone())
        .on_default({
            let state = state.clone();
            move |window: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| dialog.start(window, cx));
            }
        })
}

/// The "URL" tab: the dialog as it was before accounts.
fn url_tab(state: &Entity<CloneDialog>, dialog: &CloneDialog, colors: ThemeColors) -> gpui::Div {
    let browse = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        }
    };
    let target = dialog.target();
    let url_error = dialog.url_error().filter(|_| dialog.url_touched);
    let name_error = dialog.name_error();
    let depth_error = dialog.depth_value().is_err();

    let label = |text: SharedString| div().text_size(FS_SM).text_color(colors.muted).child(text);
    let error = |text: SharedString, colors: ThemeColors| {
        div().text_size(FS_SM).text_color(colors.error).child(text)
    };

    let protocol = match target.as_ref().map(|target| target.kind) {
        Some(CloneKind::Https) => Some(("ok", IconName::Link, "HTTPS".to_owned())),
        Some(CloneKind::Ssh) => Some(("key", IconName::Key, "SSH".to_owned())),
        Some(CloneKind::Local) => Some((
            "folder",
            IconName::Folder,
            ui::t!("welcome.clone.local").to_string(),
        )),
        None => None,
    };
    let protocol = protocol.map(|(_, icon, text)| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Icon::new(icon).size(px(12.)).color(colors.muted))
            .child(text)
    });

    let mut body = div().flex().flex_col().gap(px(12.));

    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(label(ui::t!("welcome.clone.url")))
            .child(dialog.url.clone())
            .when_some(url_error, |this, message| {
                this.child(error(message, colors))
            })
            .when_some(protocol, |this, protocol| this.child(protocol)),
    );

    body = body.child(
        div()
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.open.folder")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(dialog.dest.clone()))
                            .child(
                                Button::new("clone-browse")
                                    .label(ui::t!("welcome.open.browse"))
                                    .icon(IconName::Folder)
                                    .focus_handle(dialog.browse_focus.clone())
                                    .on_click(browse),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(200.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.clone.folder_name")))
                    .child(dialog.name.clone())
                    .when_some(name_error, |this, message| {
                        this.child(error(message, colors))
                    }),
            ),
    );

    body = body.child(
        div()
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.clone.branch_optional")))
                    .child(dialog.branch.clone()),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.clone.depth")))
                    .child(dialog.depth.clone())
                    .when(depth_error, |this| {
                        this.child(error(ui::t!("welcome.clone.depth_error"), colors))
                    }),
            ),
    );

    let shallow = {
        let state = state.clone();
        move |value: bool, _window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.shallow = value;
                if !value {
                    dialog.depth_text = "1".to_owned();
                    dialog.depth.update(cx, |input, cx| input.set_text("1", cx));
                }
                cx.notify();
            });
        }
    };
    let recurse = {
        let state = state.clone();
        move |value: bool, _window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.recurse = value;
                cx.notify();
            });
        }
    };

    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                Checkbox::new("clone-submodules")
                    .label(ui::t!("welcome.clone.submodules"))
                    .checked(dialog.recurse)
                    .focus_handle(dialog.checkbox_focus.clone())
                    .on_toggle(recurse),
            )
            .child(
                Checkbox::new("clone-shallow")
                    .label(ui::t!("welcome.clone.shallow"))
                    .checked(dialog.shallow)
                    .focus_handle(dialog.shallow_focus.clone())
                    .on_toggle(shallow),
            ),
    );

    body
}

/// The "URL | From account" switch, shown when there is an account to list.
fn source_tabs(state: &Entity<CloneDialog>, dialog: &CloneDialog) -> gpui::Div {
    let on_select = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            let tab = if index == 0 {
                CloneTab::Url
            } else {
                CloneTab::Account
            };
            state.update(cx, |dialog, cx| dialog.set_tab(tab, cx));
        }
    };
    div().flex().child(
        Segmented::new("clone-source")
            .item(
                SegmentItem::new(ui::t!("welcome.clone.tab_url"))
                    .id("clone-source-url")
                    .with_icon(IconName::Link)
                    .focus_handle(dialog.url_tab_focus.clone()),
            )
            .item(
                SegmentItem::new(ui::t!("welcome.clone.tab_account"))
                    .id("clone-source-account")
                    .with_icon(IconName::User)
                    .focus_handle(dialog.account_tab_focus.clone()),
            )
            .selected(match dialog.tab {
                CloneTab::Url => 0,
                CloneTab::Account => 1,
            })
            .on_select(on_select),
    )
}

/// The "From account" tab: account, search, the repository list and, once a
/// repository is selected, its address, folder and folder name.
fn account_tab(
    state: &Entity<CloneDialog>,
    dialog: &CloneDialog,
    colors: ThemeColors,
) -> gpui::Div {
    let label = |text: SharedString| div().text_size(FS_SM).text_color(colors.muted).child(text);
    let field = |text: SharedString, control: AnyElement| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(label(text))
            .child(control)
    };

    let account_select = Select::new("clone-account")
        .options(
            dialog
                .accounts
                .iter()
                .map(accounts::account_option_label)
                .collect::<Vec<_>>(),
        )
        .selected(dialog.account_index)
        .focus_handle(dialog.account_focus.clone())
        .on_change({
            let state = state.clone();
            move |index, _, cx| state.update(cx, |dialog, cx| dialog.select_account(index, cx))
        });

    let mut tab = div().flex().flex_col().gap(px(12.)).child(
        div()
            .flex()
            .gap(px(12.))
            .child(field(
                ui::t!("welcome.clone.account"),
                account_select.into_any_element(),
            ))
            .child(field(
                ui::t!("welcome.clone.search"),
                dialog.query.clone().into_any_element(),
            )),
    );
    tab = tab.child(repo_list(state, dialog, colors));

    let Some(repo) = dialog.selected_repo() else {
        return tab.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!("welcome.clone.select_hint")),
        );
    };
    let browse = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        }
    };
    let name_error = dialog.name_error();
    tab.child(
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(label(ui::t!("welcome.clone.url")))
            .child(
                // The address comes from the selection; it is shown, not edited.
                div()
                    .h(ui::metrics::CTL)
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(ui::metrics::R_SM)
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.bg)
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.text)
                    .child(div().min_w_0().truncate().child(repo.clone_https.clone())),
            ),
    )
    .child(
        div()
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.open.folder")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(dialog.dest.clone()))
                            .child(
                                Button::new("clone-browse")
                                    .label(ui::t!("welcome.open.browse"))
                                    .icon(IconName::Folder)
                                    .focus_handle(dialog.browse_focus.clone())
                                    .on_click(browse),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(200.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(label(ui::t!("welcome.clone.folder_name")))
                    .child(dialog.name.clone())
                    .when_some(name_error, |this, message| {
                        this.child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.error)
                                .child(message),
                        )
                    }),
            ),
    )
}

/// A bordered, centred note inside the list frame.
fn list_note(colors: ThemeColors, title: SharedString, text: SharedString) -> AnyElement {
    list_frame(colors)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .px(px(12.))
                .py(px(24.))
                .text_size(FS_SM)
                .text_center()
                .when(!title.is_empty(), |this| {
                    this.child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                })
                .child(div().text_color(colors.muted).child(text)),
        )
        .into_any_element()
}

fn list_frame(colors: ThemeColors) -> gpui::Div {
    div()
        .border_1()
        .border_color(colors.border)
        .rounded(R_MD)
        .bg(colors.bg)
        .overflow_hidden()
}

/// The repository list of the selected account in its current state.
fn repo_list(state: &Entity<CloneDialog>, dialog: &CloneDialog, colors: ThemeColors) -> AnyElement {
    let Some(account) = dialog.current_account() else {
        return div().into_any_element();
    };
    let host = accounts::host_of(&account.base_url);
    match &dialog.repos {
        RepoList::Loading => {
            let widths = [230., 300., 180., 270.];
            let rows = widths.iter().enumerate().map(|(index, width)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(40.))
                    .px(px(10.))
                    .child(Skeleton::new(("clone-skel-icon", index), px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .child(Skeleton::new(("clone-skel", index), px(*width))),
                    )
                    .child(Skeleton::new(("clone-skel-when", index), px(48.)))
            });
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(list_frame(colors).children(rows))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(Spinner::new("clone-repos-spinner", px(14.)))
                        .child(ui::t!("welcome.clone.repos_loading", host = host)),
                )
                .into_any_element()
        }
        RepoList::Failed(RepoListError::Unsupported) => list_note(
            colors,
            SharedString::default(),
            ui::t!(
                "welcome.clone.repos_unsupported",
                service = accounts::kind_of(account).map_or("", forge::ServiceKind::display_name)
            ),
        ),
        RepoList::Failed(error) => {
            let (body, action) = match error {
                RepoListError::Revoked => (
                    ui::t!(
                        "welcome.clone.repos_revoked",
                        login = account.login,
                        host = host
                    ),
                    Button::primary("clone-reconnect")
                        .label(ui::t!("welcome.clone.repos_reconnect"))
                        .on_click({
                            let state = state.clone();
                            move |_, window, cx| {
                                state.update(cx, |dialog, cx| dialog.reconnect(window, cx));
                            }
                        }),
                ),
                other => (
                    match other {
                        RepoListError::Other(text) => text.clone().into(),
                        _ => ui::t!("welcome.clone.repos_network", host = host),
                    },
                    Button::primary("clone-reload")
                        .label(ui::t!("welcome.clone.retry"))
                        .on_click({
                            let state = state.clone();
                            move |_, _, cx| state.update(cx, |dialog, cx| dialog.load_repos(cx))
                        }),
                ),
            };
            Banner::new(BannerKind::Error, ui::t!("welcome.clone.repos_error_title"))
                .body(body)
                .action(action)
                .into_any_element()
        }
        RepoList::Loaded { repos, .. } if repos.is_empty() => list_note(
            colors,
            ui::t!("welcome.clone.repos_empty_title"),
            ui::t!(
                "welcome.clone.repos_empty",
                login = account.login,
                host = host
            ),
        ),
        RepoList::Loaded { repos, truncated } => {
            let visible = dialog.visible_repos();
            if visible.is_empty() {
                return list_note(
                    colors,
                    ui::t!(
                        "welcome.clone.repos_none_title",
                        query = dialog.query_text.trim()
                    ),
                    ui::t!("welcome.clone.repos_none", n = repos.len()),
                );
            }
            let now = crate::history_view::unix_now();
            let count = visible.len();
            let rows = visible
                .iter()
                .copied()
                .enumerate()
                .map(|(position, index)| {
                    repo_row(
                        state,
                        &repos[index],
                        index,
                        dialog.repo_selected == Some(index),
                        position + 1 < count,
                        now,
                        colors,
                    )
                });
            let list = list_frame(colors)
                .id("clone-repolist")
                .key_context("clone-repolist")
                .track_focus(&dialog.list_focus)
                .max_h(px(250.))
                .overflow_y_scroll()
                .track_scroll(&dialog.list_scroll)
                .on_action(move_handler::<SelectNext>(state, Move::Next))
                .on_action(move_handler::<SelectPrev>(state, Move::Prev))
                .on_action(move_handler::<SelectFirst>(state, Move::First))
                .on_action(move_handler::<SelectLast>(state, Move::Last))
                .children(rows);
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(list)
                .when(*truncated, |this| {
                    this.child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(ui::t!("welcome.clone.repos_truncated", n = repos.len())),
                    )
                })
                .into_any_element()
        }
    }
}

/// A list action that moves the selection.
fn move_handler<A: gpui::Action>(
    state: &Entity<CloneDialog>,
    movement: Move,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, _, cx| state.update(cx, |dialog, cx| dialog.move_selection(movement, cx))
}

fn repo_row(
    state: &Entity<CloneDialog>,
    repo: &RepoSummary,
    index: usize,
    selected: bool,
    divider: bool,
    now: u64,
    colors: ThemeColors,
) -> impl IntoElement {
    let icon = if repo.archived {
        IconName::Archive
    } else if repo.fork {
        IconName::RepoForked
    } else {
        IconName::Repo
    };
    let id = |name: &'static str| ElementId::NamedInteger(name.into(), index as u64);
    let chips = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .when(repo.private, |this| {
            this.child(
                Chip::new(
                    id("clone-chip-private"),
                    ui::t!("welcome.clone.repo_private"),
                )
                .icon(IconName::Lock),
            )
        })
        .when(repo.fork, |this| {
            this.child(
                Chip::new(id("clone-chip-fork"), ui::t!("welcome.clone.repo_fork"))
                    .icon(IconName::RepoForked),
            )
        })
        .when(repo.archived, |this| {
            this.child(
                Chip::new(
                    id("clone-chip-archived"),
                    ui::t!("welcome.clone.repo_archived"),
                )
                .icon(IconName::Archive),
            )
        });
    let state = state.clone();
    div()
        .id(id("clone-repo"))
        .flex()
        .items_center()
        .gap(px(8.))
        .min_h(px(40.))
        .px(px(10.))
        .py(px(4.))
        .cursor_pointer()
        .when(divider, |this| {
            this.border_b_1().border_color(colors.border)
        })
        .when(selected, |this| this.bg(colors.accent_soft))
        .when(!selected, |this| {
            this.hover(move |style| style.bg(colors.hover))
        })
        .on_click(move |_, _, cx| state.update(cx, |dialog, cx| dialog.select_repo(index, cx)))
        .child(Icon::new(icon).size(px(16.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            div()
                                .flex()
                                .min_w_0()
                                .truncate()
                                .child(
                                    div()
                                        .text_color(colors.muted)
                                        .font_weight(FontWeight::NORMAL)
                                        .child(format!("{}/", repo.owner)),
                                )
                                .child(repo.name.clone()),
                        )
                        .child(chips),
                )
                .when_some(repo.description.clone(), |this, description| {
                    this.child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .truncate()
                            .child(description),
                    )
                }),
        )
        .when(repo.updated_at > 0, |this| {
            this.child(
                div()
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(relative_time(now, repo.updated_at)),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_and_depths() {
        assert_eq!(
            network::suggested_folder("https://github.com/a/b.git").as_deref(),
            Some("b")
        );
    }

    /// Typing a URL fills the folder name until the user edits it, and the
    /// form only becomes valid when that folder does not exist yet.
    #[gpui::test]
    fn the_form_derives_the_folder_name_and_validates_it(cx: &mut gpui::TestAppContext) {
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);
        let dialog = cx.update(|window, cx| {
            cx.new(|cx| CloneDialog::new(Ok(git), workspace.downgrade(), window, cx))
        });
        assert!(!dialog.read_with(&cx, |dialog, _| dialog.ready()));

        dialog.update(&mut cx, |dialog, cx| {
            dialog.url.update(cx, |input, cx| {
                input.set_text("https://github.com/acme/tool.git", cx)
            });
        });
        cx.run_until_parked();
        assert_eq!(
            dialog.read_with(&cx, |dialog, _| dialog.name_text.clone()),
            "tool"
        );
        assert!(dialog.read_with(&cx, |dialog, _| dialog.ready()));

        // A folder that already exists blocks the clone.
        let existing = crate::test_support::TempRepo::new("clone-dialog-existing");
        let parent = existing.path().parent().unwrap().to_path_buf();
        let manual = existing
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        dialog.update(&mut cx, |dialog, cx| {
            dialog.dest.update(cx, |input, cx| {
                input.set_text(parent.display().to_string(), cx)
            });
            dialog
                .name
                .update(cx, |input, cx| input.set_text(manual.clone(), cx));
        });
        cx.run_until_parked();
        assert!(!dialog.read_with(&cx, |dialog, _| dialog.ready()));
        assert!(dialog.read_with(&cx, |dialog, _| dialog.name_error().is_some()));

        // Editing the name stops following the URL.
        dialog.update(&mut cx, |dialog, cx| {
            dialog.url.update(cx, |input, cx| {
                input.set_text("https://github.com/acme/other.git", cx)
            });
        });
        cx.run_until_parked();
        assert_eq!(
            dialog.read_with(&cx, |dialog, _| dialog.name_text.clone()),
            manual
        );
    }

    #[test]
    fn the_keyboard_moves_through_the_visible_repositories() {
        let visible = [2, 5, 7];
        let go = |current, movement| next_selection(&visible, current, movement);
        assert_eq!(go(None, Move::Next), Some(2));
        assert_eq!(go(None, Move::Prev), Some(7));
        assert_eq!(go(Some(2), Move::Next), Some(5));
        assert_eq!(go(Some(7), Move::Next), Some(7));
        assert_eq!(go(Some(2), Move::Prev), Some(2));
        assert_eq!(go(Some(7), Move::Prev), Some(5));
        assert_eq!(go(Some(5), Move::First), Some(2));
        assert_eq!(go(Some(2), Move::Last), Some(7));
        // A selection the search hides counts as none.
        assert_eq!(go(Some(3), Move::Next), Some(2));
        assert_eq!(next_selection(&[], Some(1), Move::Next), None);
    }

    fn account(id: &str, kind: &str, url: &str) -> AccountRecord {
        AccountRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            base_url: url.to_owned(),
            login: "mira-k".to_owned(),
            label: "mira-k".to_owned(),
            method: crate::settings::AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    fn summary(owner: &str, name: &str) -> RepoSummary {
        RepoSummary {
            owner: owner.to_owned(),
            name: name.to_owned(),
            description: None,
            private: false,
            fork: false,
            archived: false,
            updated_at: 0,
            clone_https: format!("https://github.com/{owner}/{name}.git"),
        }
    }

    fn with_accounts(cx: &mut gpui::TestAppContext, accounts: Vec<AccountRecord>) {
        cx.update(|cx| {
            crate::accounts::set_test_store(cx);
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings {
                    accounts,
                    ..Default::default()
                },
                save_in_flight: false,
                dirty: false,
            });
        });
    }

    /// With accounts the dialog opens on "From account"; picking a repository
    /// fills the address and the folder name, and switching account starts
    /// the selection and the search over.
    #[gpui::test]
    fn the_account_tab_selects_a_repository_and_resets_on_account_switch(
        cx: &mut gpui::TestAppContext,
    ) {
        with_accounts(
            cx,
            vec![
                account("a1", "github", "https://github.com"),
                account("a2", "gitlab", "https://gitlab.com"),
            ],
        );
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);
        let dialog = cx.update(|window, cx| {
            cx.new(|cx| CloneDialog::new(Ok(git), workspace.downgrade(), window, cx))
        });
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.tab, CloneTab::Account);
            // The empty in-memory keychain holds no token.
            assert!(matches!(
                dialog.repos,
                RepoList::Failed(RepoListError::Revoked)
            ));
            assert!(!dialog.ready());
        });

        dialog.update(&mut cx, |dialog, cx| {
            let generation = dialog.list_generation;
            dialog.finish_load(
                generation,
                Ok(RepoListing {
                    repos: vec![summary("acme", "one"), summary("acme", "two")],
                    truncated: false,
                }),
                cx,
            );
            dialog.select_repo(1, cx);
            dialog
                .query
                .update(cx, |input, cx| input.set_text("tw", cx));
        });
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.current_url(), "https://github.com/acme/two.git");
            assert_eq!(dialog.name_text, "two");
            assert_eq!(dialog.visible_repos(), &[1]);
            assert!(dialog.ready());
        });

        dialog.update(&mut cx, |dialog, cx| dialog.select_account(1, cx));
        cx.run_until_parked();
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.account_index, 1);
            assert_eq!(dialog.repo_selected, None);
            assert_eq!(dialog.query_text, "");
            assert_eq!(dialog.name_text, "");
            assert!(!dialog.ready());
        });

        // The URL tab keeps its own, untouched address.
        dialog.update(&mut cx, |dialog, cx| dialog.set_tab(CloneTab::Url, cx));
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.current_url(), "");
        });
    }

    /// Without an account there is no "From account" tab.
    #[gpui::test]
    fn without_accounts_the_dialog_is_the_url_form(cx: &mut gpui::TestAppContext) {
        with_accounts(cx, Vec::new());
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);
        let dialog = cx.update(|window, cx| {
            cx.new(|cx| CloneDialog::new(Ok(git), workspace.downgrade(), window, cx))
        });
        dialog.update(&mut cx, |dialog, cx| {
            assert_eq!(dialog.tab, CloneTab::Url);
            dialog.set_tab(CloneTab::Account, cx);
            assert_eq!(dialog.tab, CloneTab::Url);
        });
    }
}
