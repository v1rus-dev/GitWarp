//! The "Open from folder" dialog: the folder field, the live probe banner, the
//! recent folders and the optional initialization of a plain folder.
//!
//! The probe runs in the background and is debounced; every keystroke bumps a
//! generation counter and a result for an older generation is dropped, so the
//! banner never describes a previous value of the field.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use git_core::{probe_folder, Error, FolderProbe, Git, Repo};
use ui::metrics::{FONT_MONO, FS_BASE, FS_SM, MONO, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    Icon, IconName, TextInput, TextInputEvent, ToastKind,
};

use crate::format::{display_path, expand_home, format_count, home_dir};
use crate::settings::RecentRepo;
use crate::workspace::{repo_name, Workspace};

/// The field's debounce: a probe starts this long after the last keystroke.
const PROBE_DEBOUNCE: Duration = Duration::from_millis(250);
/// How many recent folders the list shows.
const RECENT_SHOWN: usize = 6;

/// The outcome of a probe, with git's message flattened for the banner.
type ProbeResult = Result<FolderProbe, String>;

/// The dialog's border state, before the labels and texts are looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusKind {
    Empty,
    Probing,
    Repo,
    Nested,
    Plain,
    Missing,
    Failed,
}

/// Everything the footer and the banner need for one status. `banner` is the
/// kind and title; the body text is built separately because it contains mono
/// spans and, for a plain folder, the initialize controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusSpec {
    kind: StatusKind,
    banner: Option<(BannerKind, &'static str)>,
    primary: &'static str,
    enabled: bool,
    hint: Option<&'static str>,
}

const OPEN: &str = "Open";
const INITIALIZE: &str = "Initialize and open";

/// The status of a folder field with `text`, `probing` and `result`.
///
/// While a probe runs the previous banner stays (`result`), so typing does not
/// flash an empty dialog; only the primary button is disabled.
fn status_spec(
    field_empty: bool,
    probing: bool,
    result: Option<&ProbeResult>,
    branch_blank: bool,
) -> StatusSpec {
    if field_empty {
        return StatusSpec {
            kind: StatusKind::Empty,
            banner: None,
            primary: OPEN,
            enabled: false,
            hint: Some("Choose a folder to continue"),
        };
    }
    let previous = |result: Option<&ProbeResult>| match result {
        Some(result) => result_spec(result, branch_blank),
        None => StatusSpec {
            kind: StatusKind::Empty,
            banner: None,
            primary: OPEN,
            enabled: false,
            hint: None,
        },
    };
    if probing {
        let mut spec = previous(result);
        spec.kind = StatusKind::Probing;
        spec.enabled = false;
        spec.hint = None;
        return spec;
    }
    previous(result)
}

fn result_spec(result: &ProbeResult, branch_blank: bool) -> StatusSpec {
    match result {
        Ok(FolderProbe::Repo { .. }) => StatusSpec {
            kind: StatusKind::Repo,
            banner: Some((BannerKind::Ok, "Git repository found")),
            primary: OPEN,
            enabled: true,
            hint: None,
        },
        Ok(FolderProbe::Nested { .. }) => StatusSpec {
            kind: StatusKind::Nested,
            banner: Some((BannerKind::Warn, "This folder is inside a repository")),
            primary: OPEN,
            enabled: true,
            hint: None,
        },
        Ok(FolderProbe::Plain) => StatusSpec {
            kind: StatusKind::Plain,
            banner: Some((BannerKind::Info, "Not a Git repository yet")),
            primary: INITIALIZE,
            enabled: !branch_blank,
            hint: None,
        },
        Ok(FolderProbe::Missing) => StatusSpec {
            kind: StatusKind::Missing,
            banner: Some((BannerKind::Error, "Folder does not exist")),
            primary: OPEN,
            enabled: false,
            hint: Some("Choose an existing folder"),
        },
        Err(_) => StatusSpec {
            kind: StatusKind::Failed,
            banner: Some((BannerKind::Error, "Could not read this folder")),
            primary: OPEN,
            enabled: false,
            hint: None,
        },
    }
}

/// Whether a probe result belongs to an older state of the field.
fn is_stale(result_generation: u64, current_generation: u64) -> bool {
    result_generation != current_generation
}

pub(crate) struct OpenRepoDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    workspace: WeakEntity<Workspace>,
    git: Result<Arc<Git>, Arc<Error>>,
    folder: Entity<TextInput>,
    branch: Entity<TextInput>,
    /// For `~` in the field and in the recent folders.
    home: Option<PathBuf>,
    /// Set once the user typed in the field or picked a folder there.
    touched: bool,
    /// A probe is scheduled or running; the banner keeps `result`.
    probing: bool,
    /// The last finished probe.
    result: Option<ProbeResult>,
    /// Bumped on every change of the field; older probes are dropped.
    generation: u64,
    /// Set while the dialog fills the field itself: the change event that
    /// queues behind it must not restart the probe with the debounce.
    ignore_change: bool,
    initial_commit: bool,
    busy: bool,
    init_error: Option<SharedString>,
    folder_focus: FocusHandle,
    browse_focus: FocusHandle,
    branch_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// What a folder picker answer means for the dialog.
enum Browse {
    Picked(Option<PathBuf>),
    Cancelled,
    Failed(String),
}

impl OpenRepoDialog {
    fn new(
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("~/develop/my-project", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let branch = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("main", cx);
            input.set_text("main", cx);
            input.set_font_family(FONT_MONO, cx);
            input
        });
        let folder_focus = folder.read(cx).focus_handle().clone();
        let branch_focus = branch.read(cx).focus_handle().clone();
        let subscriptions = vec![
            cx.subscribe(&folder, |this, _, event: &TextInputEvent, cx| {
                this.on_folder_changed(event, cx)
            }),
            cx.subscribe(&branch, |this, _, _: &TextInputEvent, cx| {
                this.notify_host(cx);
            }),
        ];

        Self {
            layer: None,
            host: None,
            workspace,
            git,
            folder,
            branch,
            home: home_dir(),
            touched: false,
            probing: false,
            result: None,
            generation: 0,
            ignore_change: false,
            initial_commit: true,
            busy: false,
            init_error: None,
            folder_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            branch_focus,
            checkbox_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        }
    }

    /// The overlay layer of this dialog, while it is open.
    pub(crate) fn layer_id(&self) -> Option<overlay::LayerId> {
        self.layer
    }

    /// The state of the dialog as the banner and the footer show it.
    fn spec(&self, cx: &App) -> StatusSpec {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let branch_blank = self.branch.read(cx).text().trim().is_empty();
        status_spec(empty, self.probing, self.result.as_ref(), branch_blank)
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    fn on_folder_changed(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        if !matches!(event, TextInputEvent::Changed(_)) {
            return;
        }
        if self.ignore_change {
            // The dialog filled the field and probed at once already.
            self.ignore_change = false;
            return;
        }
        self.touched = true;
        self.init_error = None;
        let text = self.folder.read(cx).text().trim().to_owned();
        if text.is_empty() {
            self.reset_probe();
        } else {
            let path = expand_home(&text, self.home.as_deref());
            self.start_probe(path, true, cx);
        }
        self.sync_error_border(cx);
        self.notify_host(cx);
        cx.notify();
    }

    /// Fills the field from the dialog itself. The input still queues a
    /// change event for it; `ignore_change` keeps that from restarting the
    /// probe with the debounce, because the caller probes at once.
    fn set_folder_text(&mut self, label: String, cx: &mut Context<Self>) {
        if self.folder.read(cx).text() == label {
            return;
        }
        self.ignore_change = true;
        self.folder
            .update(cx, |input, cx| input.set_text(label, cx));
    }

    /// An empty field has no status: drop any result and any probe in flight.
    fn reset_probe(&mut self) {
        self.generation += 1;
        self.probing = false;
        self.result = None;
    }

    /// Starts a probe for `path`, after the debounce or at once.
    fn start_probe(&mut self, path: PathBuf, debounced: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.probing = true;
        let git = self.git.clone();
        cx.spawn(async move |this, cx| {
            if debounced {
                cx.background_executor().timer(PROBE_DEBOUNCE).await;
            }
            // A keystroke after this probe was scheduled made it obsolete.
            let current = this.update(cx, |dialog, _| dialog.generation).ok();
            if current != Some(generation) {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    match git {
                        Ok(git) => probe_folder(git, &path).map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                })
                .await;
            this.update(cx, |dialog, cx| dialog.finish_probe(generation, result, cx))
                .ok();
        })
        .detach();
    }

    fn finish_probe(&mut self, generation: u64, result: ProbeResult, cx: &mut Context<Self>) {
        if is_stale(generation, self.generation) {
            return;
        }
        self.probing = false;
        self.result = Some(result);
        self.sync_error_border(cx);
        self.notify_host(cx);
        cx.notify();
    }

    /// The field shows the error border only for a missing folder the user
    /// actually typed or picked.
    fn sync_error_border(&mut self, cx: &mut Context<Self>) {
        let missing = matches!(self.result, Some(Ok(FolderProbe::Missing)));
        let error = self.touched && missing;
        self.folder
            .update(cx, |input, cx| input.set_error(error, cx));
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = match receiver.await {
                Ok(Ok(Some(paths))) => Browse::Picked(paths.into_iter().next()),
                Ok(Ok(None)) => Browse::Cancelled,
                Ok(Err(error)) => Browse::Failed(error.to_string()),
                Err(error) => Browse::Failed(error.to_string()),
            };
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
        match outcome {
            Browse::Picked(Some(path)) => {
                let label = display_path(&path, home.as_deref());
                self.touched = true;
                self.init_error = None;
                self.set_folder_text(label, cx);
                // The picker is an explicit choice: probe at once.
                self.start_probe(path, false, cx);
                self.sync_error_border(cx);
                self.notify_host(cx);
                cx.notify();
            }
            Browse::Picked(None) | Browse::Cancelled => {}
            Browse::Failed(message) => {
                // No xdg portal, no picker: the field stays the way in.
                toast::show(
                    ToastKind::Error,
                    format!("Could not open the folder picker: {message}"),
                    window,
                    cx,
                );
            }
        }
    }

    /// A recent row: fill the field, mark it touched and probe immediately.
    fn choose_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let label = display_path(&path, self.home.as_deref());
        self.touched = true;
        self.init_error = None;
        self.set_folder_text(label, cx);
        self.start_probe(path, false, cx);
        self.sync_error_border(cx);
        self.notify_host(cx);
        cx.notify();
    }

    /// The primary button and Enter: open the repository or initialize one.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let spec = self.spec(cx);
        if !spec.enabled {
            return;
        }
        match spec.kind {
            StatusKind::Repo | StatusKind::Nested => {
                let root = match self.result.as_ref() {
                    Some(Ok(FolderProbe::Repo { root, .. }))
                    | Some(Ok(FolderProbe::Nested { root })) => root.clone(),
                    _ => return,
                };
                self.open_root(root, window, cx);
            }
            StatusKind::Plain => self.start_init(window, cx),
            _ => {}
        }
    }

    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = repo_name(&root);
        self.close(window, cx);
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(root, window, cx);
            });
        }
        toast::show(ToastKind::Ok, format!("Opened {name}"), window, cx);
    }

    /// Runs `init_with` off the UI thread; the dialog stays busy until it
    /// answers.
    fn start_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder_text = self.folder.read(cx).text().trim().to_owned();
        let branch_text = self.branch.read(cx).text().trim().to_owned();
        if self.busy || folder_text.is_empty() {
            return;
        }
        let path = expand_home(&folder_text, self.home.as_deref());
        let branch = (!branch_text.is_empty()).then_some(branch_text);
        let initial_commit = self.initial_commit;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.set_init_error(error.to_string(), cx);
                return;
            }
        };

        self.busy = true;
        self.init_error = None;
        self.notify_host(cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Repo::init_with(git, &path, branch.as_deref(), initial_commit)
                        .map(|repo| repo.root().to_path_buf())
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update_in(cx, |dialog, window, cx| {
                dialog.finish_init(result, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_init(
        &mut self,
        result: Result<PathBuf, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(path) => {
                let label = display_path(&path, self.home.as_deref());
                self.close(window, cx);
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.open_repository(path, window, cx);
                    });
                }
                toast::show(
                    ToastKind::Ok,
                    format!("Initialized repository in {label}"),
                    window,
                    cx,
                );
            }
            Err(message) => self.set_init_error(message, cx),
        }
    }

    fn set_init_error(&mut self, message: String, cx: &mut Context<Self>) {
        let message = message.trim();
        let message = if message.is_empty() {
            "git failed without a message"
        } else {
            message
        };
        self.busy = false;
        self.init_error = Some(message.to_owned().into());
        self.notify_host(cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }
}

/// Opens the dialog and returns its state, kept by the workspace.
pub(crate) fn open(
    window: &mut Window,
    cx: &mut Context<Workspace>,
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
) -> Entity<OpenRepoDialog> {
    let dialog = cx.new(|cx| OpenRepoDialog::new(git, workspace, window, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new("Open from folder")
            .icon(IconName::Folder)
            .width(px(540.)),
    });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
        cx.notify();
    });
    dialog
}

/// Builds the dialog from its state. The builder runs on every frame, so it
/// only reads state that already exists.
fn build(state: &Entity<OpenRepoDialog>, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let spec = dialog.spec(cx);
    let busy = dialog.busy;
    let recent = read_recent(cx);

    let label = |text: &'static str| div().text_size(FS_SM).text_color(colors.muted).child(text);

    let browse: ClickAction = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window, cx| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        })
    };
    let primary: ClickAction = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window, cx| {
            state.update(cx, |dialog, cx| dialog.activate(window, cx));
        })
    };
    let cancel: ClickAction = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window, cx| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        })
    };
    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.activate(window, cx));
        }
    };

    let field = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(label("Folder"))
                .child(div().text_size(FS_SM).text_color(colors.error).child("*")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().child(dialog.folder.clone()))
                .child(
                    Button::new("open-browse")
                        .label("Browse…")
                        .icon(IconName::Folder)
                        .disabled(busy)
                        .focus_handle(dialog.browse_focus.clone())
                        .on_click(browse),
                ),
        );

    let mut status = div().flex().flex_col().gap(px(8.));
    let mut has_status = false;
    if let Some((kind, title)) = spec.banner {
        let mut banner = Banner::new(kind, title);
        if let Some(body) = banner_body(dialog) {
            banner = banner.body(body);
        }
        status = status.child(banner);
        has_status = true;
    }
    if let Some(extra) = status_details(state, spec.kind, cx) {
        status = status.child(extra);
        has_status = true;
    }
    if let Some(error) = dialog.init_error.clone() {
        status = status.child(
            Banner::new(BannerKind::Error, "Could not initialize the repository").body(error),
        );
        has_status = true;
    }

    let primary_enabled = spec.enabled && !busy;
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .debug_selector(|| "open-dialog-body".to_owned())
        .child(field);
    if has_status {
        body = body.child(status);
    }

    if !recent.is_empty() {
        let mut list = div()
            .w_full()
            .flex()
            .flex_col()
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .overflow_hidden();
        let count = recent.len().min(RECENT_SHOWN);
        for (index, repo) in recent.iter().take(RECENT_SHOWN).enumerate() {
            list = list.child(recent_row(
                index,
                repo,
                index + 1 == count,
                dialog.home.as_deref(),
                state.clone(),
                colors,
            ));
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label("Recent folders"))
                .child(list),
        );
    }

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("open-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .disabled(busy)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("open-primary")
                .label(spec.primary)
                .min_width(px(72.))
                .disabled(!primary_enabled)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(primary),
        );

    let mut focus_order = vec![
        DialogFocus::input(dialog.folder_focus.clone()),
        DialogFocus::control(dialog.browse_focus.clone()),
    ];
    if matches!(spec.kind, StatusKind::Plain) {
        focus_order.push(DialogFocus::input(dialog.branch_focus.clone()));
        focus_order.push(DialogFocus::control(dialog.checkbox_focus.clone()));
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let mut panel = Dialog::new("Open from folder")
        .icon(IconName::Folder)
        .width(px(540.))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(busy)
        .focus_order(focus_order)
        .autofocus(dialog.folder_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default);
    if let Some(hint) = spec.hint {
        panel = panel.hint(hint);
    }
    panel
}

/// The plain part of the banner body: the sentences without mono spans.
fn banner_body(dialog: &OpenRepoDialog) -> Option<SharedString> {
    match dialog.result.as_ref()? {
        Ok(FolderProbe::Plain) => Some("You can initialize one here and open it.".into()),
        Ok(FolderProbe::Missing) => Some("Check the path, or choose a folder with Browse.".into()),
        Err(message) => Some(message.clone().into()),
        _ => None,
    }
}

/// The banner's rich body (mono spans) and, for a plain folder, the
/// initialize controls. `Banner` takes plain text, so this element sits after
/// it instead of re-implementing the banner.
fn status_details(
    state: &Entity<OpenRepoDialog>,
    kind: StatusKind,
    cx: &App,
) -> Option<AnyElement> {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    match dialog.result.as_ref()? {
        Ok(FolderProbe::Repo {
            root,
            branch,
            commits,
        }) => {
            let branch = branch.clone().unwrap_or_else(|| "detached HEAD".to_owned());
            let commits = usize::try_from(*commits).unwrap_or(usize::MAX);
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .text_size(FS_BASE)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .child(repo_name(root.as_path())),
                    )
                    .child("· branch")
                    .child(div().font_family(FONT_MONO).child(branch))
                    .child(format!("· {} commits", format_count(commits)))
                    .into_any_element(),
            )
        }
        Ok(FolderProbe::Nested { root }) => {
            let root = display_path(root.as_path(), dialog.home.as_deref());
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(2.))
                    .text_size(FS_BASE)
                    .text_color(colors.muted)
                    .child("The repository root is ")
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .max_w(px(420.))
                            .truncate()
                            .child(root),
                    )
                    .child(". GitWarp will open the root, not this subfolder.")
                    .into_any_element(),
            )
        }
        Ok(FolderProbe::Plain) if matches!(kind, StatusKind::Plain | StatusKind::Probing) => {
            let busy = dialog.busy;
            let toggle = {
                let state = state.clone();
                move |next: bool, _: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| {
                        dialog.initial_commit = next;
                        dialog.notify_host(cx);
                    });
                }
            };
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        Checkbox::new("open-initial-commit")
                            .label("Create an initial commit with a .gitignore")
                            .checked(dialog.initial_commit)
                            .disabled(busy)
                            .focus_handle(dialog.checkbox_focus.clone())
                            .on_toggle(toggle),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(FS_SM)
                                    .text_color(colors.muted)
                                    .child("Default branch"),
                            )
                            .child(div().w(px(220.)).child(dialog.branch.clone())),
                    )
                    .into_any_element(),
            )
        }
        _ => None,
    }
}

/// One row of the recent folders list: `Folder` 14 · name · path (mono).
fn recent_row(
    index: usize,
    repo: &RecentRepo,
    last: bool,
    home: Option<&std::path::Path>,
    state: Entity<OpenRepoDialog>,
    colors: ThemeColors,
) -> impl IntoElement {
    let path = repo.path.clone();
    let name = repo
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.path.display().to_string());
    let path_label = display_path(&repo.path, home);

    let mut row = div()
        .id(ElementId::named_usize("open-recent", index))
        .w_full()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .min_h(px(32.))
        .px(px(10.))
        .py(px(4.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, _, cx| {
            state.update(cx, |dialog, cx| dialog.choose_recent(path.clone(), cx));
        })
        .child(
            div().w(px(20.)).flex_none().flex().items_center().child(
                Icon::new(IconName::Folder)
                    .size(px(14.))
                    .color(colors.muted),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(FS_BASE)
                .text_color(colors.text)
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .max_w(px(220.))
                .truncate()
                .font_family(FONT_MONO)
                .text_size(MONO)
                .text_color(colors.muted)
                .child(path_label),
        );
    if !last {
        row = row.border_b_1().border_color(colors.border);
    }
    row
}

type ClickAction = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn read_recent(cx: &App) -> Vec<RecentRepo> {
    cx.try_global::<crate::AppSettings>()
        .map(|state| state.settings.recent.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(probe: FolderProbe) -> ProbeResult {
        Ok(probe)
    }

    fn repo() -> ProbeResult {
        result(FolderProbe::Repo {
            root: PathBuf::from("/home/user/gitwarp"),
            branch: Some("main".to_owned()),
            commits: 21,
        })
    }

    #[test]
    fn an_empty_field_asks_for_a_folder_and_disables_open() {
        let spec = status_spec(true, false, None, false);
        assert_eq!(spec.kind, StatusKind::Empty);
        assert_eq!(spec.banner, None);
        assert_eq!(spec.primary, "Open");
        assert!(!spec.enabled);
        assert_eq!(spec.hint, Some("Choose a folder to continue"));
    }

    #[test]
    fn a_repository_root_keeps_the_previous_banner_while_probing() {
        let repo = repo();
        let ready = status_spec(false, false, Some(&repo), false);
        assert_eq!(
            (ready.kind, ready.banner),
            (
                StatusKind::Repo,
                Some((BannerKind::Ok, "Git repository found"))
            )
        );
        assert_eq!(
            (ready.primary, ready.enabled, ready.hint),
            ("Open", true, None)
        );

        // Typing again: the same banner stays, the button goes dead.
        let probing = status_spec(false, true, Some(&repo), false);
        assert_eq!(probing.kind, StatusKind::Probing);
        assert_eq!(probing.banner, ready.banner);
        assert_eq!(probing.primary, "Open");
        assert!(!probing.enabled);
        assert_eq!(probing.hint, None);
    }

    #[test]
    fn a_nested_folder_opens_the_root() {
        let nested = result(FolderProbe::Nested {
            root: PathBuf::from("/home/user/gitwarp"),
        });
        let spec = status_spec(false, false, Some(&nested), false);
        assert_eq!(spec.kind, StatusKind::Nested);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Warn, "This folder is inside a repository"))
        );
        assert_eq!(
            (spec.primary, spec.enabled, spec.hint),
            ("Open", true, None)
        );
    }

    #[test]
    fn a_plain_folder_initializes_and_needs_a_branch() {
        let plain = result(FolderProbe::Plain);
        let ready = status_spec(false, false, Some(&plain), false);
        assert_eq!(ready.kind, StatusKind::Plain);
        assert_eq!(
            ready.banner,
            Some((BannerKind::Info, "Not a Git repository yet"))
        );
        assert_eq!(
            (ready.primary, ready.enabled, ready.hint),
            ("Initialize and open", true, None)
        );

        let blank = status_spec(false, false, Some(&plain), true);
        assert_eq!(blank.primary, "Initialize and open");
        assert!(!blank.enabled);
    }

    #[test]
    fn a_missing_folder_reports_the_error_and_hints_at_browse() {
        let missing = result(FolderProbe::Missing);
        let spec = status_spec(false, false, Some(&missing), false);
        assert_eq!(spec.kind, StatusKind::Missing);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Error, "Folder does not exist"))
        );
        assert_eq!((spec.primary, spec.enabled), ("Open", false));
        assert_eq!(spec.hint, Some("Choose an existing folder"));
    }

    #[test]
    fn a_failed_probe_shows_gits_message_in_an_error_banner() {
        let failed: ProbeResult = Err("git rev-parse failed (exit 128)".to_owned());
        let spec = status_spec(false, false, Some(&failed), false);
        assert_eq!(spec.kind, StatusKind::Failed);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Error, "Could not read this folder"))
        );
        assert_eq!(
            (spec.primary, spec.enabled, spec.hint),
            ("Open", false, None)
        );
    }

    #[test]
    fn only_the_latest_probe_generation_is_accepted() {
        assert!(!is_stale(7, 7));
        assert!(is_stale(7, 8));
        assert!(is_stale(8, 7));
    }

    /// Opens the real window and drives the dialog through the workspace
    /// action, so the builder, its focus trap and Esc are exercised.
    #[gpui::test]
    fn ctrl_o_opens_and_escape_closes_the_open_dialog(cx: &mut gpui::TestAppContext) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        cx.update(ui::init).expect("ui::init");
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(960.), px(620.)),
                    ))),
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        Workspace::new(
                            None,
                            Err(Arc::new(Error::PathNotFound {
                                path: PathBuf::from("/nonexistent"),
                            })),
                            git_core::GitChoice::Auto,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });

        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        cx.update(|window, _| window.activate_window());
        assert!(
            cx.debug_bounds("recent-filter").is_some(),
            "the welcome screen did not render"
        );

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(super::super::workspace::OpenRepositoryDialog), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            1,
            "Ctrl O did not open the Open dialog"
        );
        assert!(
            cx.debug_bounds("open-dialog-body").is_some(),
            "the dialog body did not render"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            0,
            "Escape did not close the dialog"
        );
    }
}
