//! The "Open from folder" dialog: the folder field, the live probe banner, the
//! recent folders and the optional initialization of a plain folder.
//!
//! The probe and `git init` live in [`crate::init_flow`], shared with the
//! welcome screen's Initialize dialog; this module owns the fields and the
//! recent list.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use git_core::{Error, FolderProbe, Git};
use ui::metrics::{FONT_MONO, FS_BASE, FS_SM, MONO, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    Icon, IconName, TextInput, TextInputEvent, ToastKind,
};

use crate::format::{display_path, expand_home, home_dir};
use crate::init_flow::{banner_body, details, DialogKind, InitFlow, StatusKind};
use crate::settings::RecentRepo;
use crate::workspace::{repo_name, Workspace};

/// How many recent folders the list shows.
const RECENT_SHOWN: usize = 6;

pub(crate) struct OpenRepoDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    workspace: WeakEntity<Workspace>,
    /// The probe and the initialize state, shared with the welcome dialog.
    flow: Entity<InitFlow>,
    folder: Entity<TextInput>,
    branch: Entity<TextInput>,
    /// For `~` in the field and in the recent folders.
    home: Option<PathBuf>,
    /// Set once the user typed in the field or picked a folder there.
    touched: bool,
    /// Set while the dialog fills the field itself: the change event that
    /// queues behind it must not restart the probe with the debounce.
    ignore_change: bool,
    folder_focus: FocusHandle,
    browse_focus: FocusHandle,
    branch_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _flow_observer: Subscription,
}

/// What a folder picker answer means for the dialog.
pub(crate) enum Browse {
    Picked(Option<PathBuf>),
    Cancelled,
    Failed(String),
}

/// Opens the native folder picker; shared with the Clone dialog's Browse button.
pub(crate) fn pick_folder(cx: &mut App) -> impl std::future::Future<Output = Browse> {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(ui::t!("welcome.init.open")),
    });
    async move {
        match receiver.await {
            Ok(Ok(Some(paths))) => Browse::Picked(paths.into_iter().next()),
            Ok(Ok(None)) => Browse::Cancelled,
            Ok(Err(error)) => Browse::Failed(error.to_string()),
            Err(error) => Browse::Failed(error.to_string()),
        }
    }
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
        let flow = cx.new(|_| InitFlow::new(git));

        // Opening a repository (or finishing an init) closes this dialog and
        // hands the root to the workspace.
        let state = cx.entity().downgrade();
        flow.update(cx, |flow, _| {
            flow.set_on_open(Rc::new(move |root: PathBuf, created: bool, window, cx| {
                let Some(dialog) = state.upgrade() else {
                    return;
                };
                dialog.update(cx, |dialog, cx| {
                    let label = display_path(&root, dialog.home.as_deref());
                    let name = repo_name(&root);
                    dialog.close(window, cx);
                    if let Some(workspace) = dialog.workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_repository(root.clone(), window, cx);
                        });
                    }
                    if created {
                        toast::show(
                            ToastKind::Ok,
                            ui::t!("welcome.open.toast_initialized", path = label),
                            window,
                            cx,
                        );
                    } else {
                        toast::show(
                            ToastKind::Ok,
                            ui::t!("welcome.open.toast_opened", name = name),
                            window,
                            cx,
                        );
                    }
                });
            }));
        });

        let subscriptions = vec![
            cx.subscribe(&folder, |this, _, event: &TextInputEvent, cx| {
                this.on_folder_changed(event, cx)
            }),
            cx.subscribe(&branch, |this, _, _: &TextInputEvent, cx| {
                this.notify_host(cx);
                cx.notify();
            }),
        ];
        let flow_observer = cx.observe(&flow, |this, _, cx| {
            this.sync_error_border(cx);
            this.sync_busy(cx);
            this.notify_host(cx);
            cx.notify();
        });

        Self {
            layer: None,
            host: None,
            workspace,
            flow,
            folder,
            branch,
            home: home_dir(),
            touched: false,
            ignore_change: false,
            folder_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            branch_focus,
            checkbox_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
            _flow_observer: flow_observer,
        }
    }

    /// The overlay layer of this dialog, while it is open.
    pub(crate) fn layer_id(&self) -> Option<overlay::LayerId> {
        self.layer
    }

    /// The state of the dialog as the banner and the footer show it.
    fn spec(&self, cx: &App) -> crate::init_flow::StatusSpec {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let branch_blank = self.branch.read(cx).text().trim().is_empty();
        self.flow
            .read(cx)
            .spec(DialogKind::Open, empty, branch_blank)
    }

    fn busy(&self, cx: &App) -> bool {
        self.flow.read(cx).busy()
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
        let text = self.folder.read(cx).text().trim().to_owned();
        if text.is_empty() {
            self.flow.update(cx, |flow, cx| flow.reset(cx));
        } else {
            let path = expand_home(&text, self.home.as_deref());
            self.flow.update(cx, |flow, cx| flow.probe(path, true, cx));
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

    /// The folder and branch fields show the error border only for input the
    /// dialog itself cannot act on.
    fn sync_error_border(&mut self, cx: &mut Context<Self>) {
        let branch_blank = self.branch.read(cx).text().trim().is_empty();
        let flow = self.flow.read(cx);
        let missing = matches!(flow.result(), Some(Ok(FolderProbe::Missing)));
        // The last probe decides, not the status: while the next probe runs
        // the controls stay and their borders must not reset mid-keystroke.
        let plain = matches!(flow.result(), Some(Ok(FolderProbe::Plain)));
        let folder_error = self.touched && missing;
        let branch_error = plain && branch_blank;
        self.folder
            .update(cx, |input, cx| input.set_error(folder_error, cx));
        self.branch
            .update(cx, |input, cx| input.set_error(branch_error, cx));
    }

    /// The folder and branch fields are read-only while `git init` runs, so
    /// the path the init started with cannot change under it.
    fn sync_busy(&self, cx: &mut Context<Self>) {
        let busy = self.flow.read(cx).busy();
        self.folder
            .update(cx, |input, cx| input.set_disabled(busy, cx));
        self.branch
            .update(cx, |input, cx| input.set_disabled(busy, cx));
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
        // A picker that was already open when `git init` started must not
        // change the folder the init is running for.
        if self.flow.read(cx).busy() {
            return;
        }
        match outcome {
            Browse::Picked(Some(path)) => {
                let label = display_path(&path, home.as_deref());
                self.touched = true;
                self.set_folder_text(label, cx);
                // The picker is an explicit choice: probe at once.
                self.flow.update(cx, |flow, cx| flow.probe(path, false, cx));
                self.sync_error_border(cx);
                self.notify_host(cx);
                cx.notify();
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

    /// A recent row: fill the field, mark it touched and probe immediately.
    fn choose_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The field is read-only while `git init` runs; the list follows it.
        if self.busy(cx) {
            return;
        }
        let label = display_path(&path, self.home.as_deref());
        self.touched = true;
        self.set_folder_text(label, cx);
        self.flow.update(cx, |flow, cx| flow.probe(path, false, cx));
        self.sync_error_border(cx);
        self.notify_host(cx);
        cx.notify();
    }

    /// The primary button and Enter: open the repository or initialize one.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        let spec = self.spec(cx);
        if !spec.enabled {
            return;
        }
        match spec.kind {
            StatusKind::Repo | StatusKind::Nested => {
                let root = match self.flow.read(cx).result() {
                    Some(Ok(FolderProbe::Repo { root, .. }))
                    | Some(Ok(FolderProbe::Nested { root })) => root.clone(),
                    _ => return,
                };
                self.flow
                    .update(cx, |flow, cx| flow.open_root(root, window, cx));
            }
            StatusKind::Plain => self.start_init(window, cx),
            _ => {}
        }
    }

    /// Runs `init_with` off the UI thread; the dialog stays busy until it
    /// answers.
    fn start_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder_text = self.folder.read(cx).text().trim().to_owned();
        let branch_text = self.branch.read(cx).text().trim().to_owned();
        if folder_text.is_empty() {
            return;
        }
        let path = expand_home(&folder_text, self.home.as_deref());
        let branch = (!branch_text.is_empty()).then_some(branch_text);
        self.flow
            .update(cx, |flow, cx| flow.start_init(path, branch, window, cx));
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
        None => Dialog::new(ui::t!("welcome.open.title"))
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
    let flow = dialog.flow.read(cx);
    let busy = flow.busy();
    let initial_commit = flow.initial_commit();
    let init_error = flow.error().cloned();
    let recent = read_recent(cx);

    let label = |text: SharedString| div().text_size(FS_SM).text_color(colors.muted).child(text);

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
                .child(label(ui::t!("welcome.open.folder")))
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
                        .label(ui::t!("welcome.open.browse"))
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
        if let Some(body) = banner_body(flow, DialogKind::Open) {
            banner = banner.body(body);
        }
        status = status.child(banner);
        has_status = true;
    }
    if let Some(extra) = details(flow, dialog.home.as_deref(), colors) {
        status = status.child(extra);
        has_status = true;
    }
    if let Some(error) = init_error {
        status = status
            .child(Banner::new(BannerKind::Error, ui::t!("welcome.open.init_failed")).body(error));
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
    if matches!(flow.result(), Some(Ok(FolderProbe::Plain))) {
        body = body.child(init_controls(
            state,
            dialog.checkbox_focus.clone(),
            dialog.branch.clone(),
            initial_commit,
            busy,
            colors,
        ));
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
                .child(label(ui::t!("welcome.open.recent")))
                .child(list),
        );
    }

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("open-cancel")
                .label(ui::t!("welcome.common.cancel"))
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
    if matches!(flow.result(), Some(Ok(FolderProbe::Plain))) {
        focus_order.push(DialogFocus::input(dialog.branch_focus.clone()));
        focus_order.push(DialogFocus::control(dialog.checkbox_focus.clone()));
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let mut panel = Dialog::new(ui::t!("welcome.open.title"))
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

/// The initialize controls of a plain folder: the initial-commit checkbox and
/// the default branch field.
#[allow(clippy::too_many_arguments)]
fn init_controls(
    state: &Entity<OpenRepoDialog>,
    checkbox_focus: FocusHandle,
    branch: Entity<TextInput>,
    initial_commit: bool,
    busy: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    let toggle = {
        let state = state.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog
                    .flow
                    .update(cx, |flow, cx| flow.set_initial_commit(next, cx));
                dialog.notify_host(cx);
            });
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            Checkbox::new("open-initial-commit")
                .label(ui::t!("welcome.init.initial_commit"))
                .checked(initial_commit)
                .disabled(busy)
                .focus_handle(checkbox_focus)
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
                        .child(ui::t!("welcome.init.default_branch")),
                )
                .child(div().w(px(220.)).child(branch)),
        )
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

    /// Opening a repository the folder field already points at runs from the
    /// dialog's own update; the flow re-enters the dialog only after that
    /// returned, so no double entity lease.
    #[gpui::test]
    fn opening_an_existing_repository_closes_the_dialog(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("open-dialog");
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);

        let dialog = cx.update(|window, cx| {
            workspace.update(cx, |_, cx| {
                let weak = cx.weak_entity();
                super::open(window, cx, Ok(git.clone()), weak)
            })
        });
        dialog.update(&mut cx, |dialog, cx| {
            dialog.choose_recent(repo.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            dialog.update(&mut cx, |dialog, cx| dialog.spec(cx).kind),
            StatusKind::Repo,
            "the folder was not classified as a repository"
        );

        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| dialog.activate(window, cx));
        });
        cx.run_until_parked();

        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            0,
            "the dialog stayed open"
        );
        assert!(
            cx.debug_bounds("sidebar-filter").is_some(),
            "the workspace did not open the repository"
        );
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
