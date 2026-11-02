//! The Clone repository dialog: form, live validation, host-key/credential
//! prompts through the network service, progress, cancellation and the result.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, ParentElement, SharedString, Styled,
    Subscription, WeakEntity, Window,
};

use git_core::network::{self, CancelToken, CloneKind, CloneOptions, CloneTarget, NetworkContext};
use git_core::{Error, Git, Repo};
use ui::metrics::{FONT_MONO, FS_SM};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    Icon, IconName, Progress, Spinner, TextInput, TextInputEvent, ToastKind,
};

use crate::format::{display_path, expand_home, home_dir};
use crate::network_ops::{self, friendly_error};
use crate::network_service::{HostKeyAsker, NetworkService};
use crate::workspace::{repo_name, Workspace};

const CLONE_TIMEOUT: Duration = Duration::from_millis(80);

/// What the dialog is doing.
enum Phase {
    Form,
    Cloning,
    Done(CloneDone),
    Failed(CloneFailure),
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

    phase: Phase,
    progress: Option<f32>,
    phase_text: SharedString,
    cancel: CancelToken,
    generation: u64,

    url_focus: gpui::FocusHandle,
    dest_focus: gpui::FocusHandle,
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

        Self {
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
            phase: Phase::Form,
            progress: None,
            phase_text: SharedString::default(),
            cancel: CancelToken::new(),
            generation: 0,
            url_focus,
            dest_focus,
            name_focus,
            branch_focus,
            depth_focus,
            checkbox_focus: cx.focus_handle().tab_stop(true),
            shallow_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            secondary_focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn apply_suggested_name(&mut self, cx: &mut Context<Self>) {
        let suggested = network::suggested_folder(&self.url_text).unwrap_or_default();
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
        network::clone_target(&self.url_text)
    }

    fn url_error(&self) -> Option<SharedString> {
        if self.url_text.trim().is_empty() {
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
        if !self.shallow {
            return Ok(None);
        }
        match self.depth_text.trim().parse::<u32>() {
            Ok(depth) if depth > 0 => Ok(Some(depth)),
            _ => Err(()),
        }
    }

    fn ready(&self) -> bool {
        !self.url_text.trim().is_empty()
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
        let raw_url = self.url_text.trim();
        let url = if raw_url.starts_with('~') {
            expand_home(raw_url, self.home.as_deref())
                .display()
                .to_string()
        } else {
            raw_url.to_owned()
        };
        let options = CloneOptions {
            url,
            dest: self.destination(),
            branch: {
                let branch = self.branch_text.trim();
                (!branch.is_empty()).then(|| branch.to_owned())
            },
            depth: self.depth_value().ok().flatten(),
            recurse_submodules: self.recurse,
        };
        let (context, asker) = match cx.try_global::<NetworkService>() {
            Some(service) => (service.context(true), service.host_keys()),
            None => (NetworkContext::default(), HostKeyAsker::unavailable()),
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
                        .child(dialog.url_text.clone()),
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
                    .child(dialog.dest.clone()),
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

    let focus_order = vec![
        DialogFocus::input(dialog.url_focus.clone()),
        DialogFocus::input(dialog.dest_focus.clone()),
        DialogFocus::input(dialog.name_focus.clone()),
        DialogFocus::input(dialog.branch_focus.clone()),
        DialogFocus::input(dialog.depth_focus.clone()),
        DialogFocus::control(dialog.checkbox_focus.clone()),
        DialogFocus::control(dialog.shallow_focus.clone()),
        DialogFocus::control(dialog.cancel_focus.clone()),
        DialogFocus::control(dialog.primary_focus.clone()),
    ];

    Dialog::new(ui::t!("welcome.clone.title"))
        .icon(IconName::Clone)
        .width(px(560.))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(false)
        .focus_order(focus_order)
        .autofocus(dialog.url_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default({
            let state = state.clone();
            move |window: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| dialog.start(window, cx));
            }
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
}
