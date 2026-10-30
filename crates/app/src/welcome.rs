//! The welcome screen: the three actions on the left, the recent repositories
//! on the right and the Initialize dialog.
//!
//! Branches of the recent rows are read in the background, one repository at a
//! time, and arrive after the list is already on screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, px, transparent_black, uniform_list, App, AppContext, ClickEvent, Context,
    Decorations, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::{Error, Git, Head, Repo};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_SM, FS_XL, MONO, R_MD, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, focus, overlay, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    EmptyState, Icon, IconName, Kbd, Scrollbar, SearchField, TextInput, TextInputEvent,
    TextInputSubmit, Tooltip,
};

use crate::format::{display_path, expand_home, home_dir, relative_time};
use crate::history_view::unix_now;
use crate::settings::RecentRepo;
use crate::workspace::{git_label, titlebar, titlebar_end, titlebar_fill, Workspace};

actions!(welcome, [NewRepository, RemoveSelected]);

/// The left column: the design's fixed width.
const LEFT_W: f32 = 300.;
/// One action row.
const ACTION_H: f32 = 36.;
/// One recent repository row.
const ROW_H: f32 = 44.;
/// The branch column.
const BRANCH_W: f32 = 150.;
/// The relative-time column.
const TIME_W: f32 = 110.;

/// A recent repository row, resolved for one frame.
struct RecentRow {
    path: PathBuf,
    name: SharedString,
    path_label: SharedString,
    branch: Option<SharedString>,
    time: SharedString,
}

/// What is known about a row's branch.
enum BranchState {
    /// The background `Repo::open(..).head()` is still running.
    Loading,
    /// `Some` for a branch, `None` for detached HEAD or a failed read.
    Known(Option<SharedString>),
}

pub struct WelcomeView {
    /// Resolved once per process by the caller; shown in the status bar and
    /// used for the row branches and the Initialize dialog.
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    /// The recent list, re-read from the settings global when it changes.
    recent: Vec<RecentRepo>,
    /// For `~` in the rows and in the dialog's folder field.
    home: Option<PathBuf>,
    filter: Entity<TextInput>,
    filter_text: String,
    /// Index into the filtered rows, never past the end.
    selected: usize,
    branches: HashMap<PathBuf, BranchState>,
    list_scroll: UniformListScrollHandle,
    /// The list's focus: the screen starts here so ↑/↓ work at once.
    focus: FocusHandle,
    open_focus: FocusHandle,
    clone_focus: FocusHandle,
    init_focus: FocusHandle,
    /// The open Initialize dialog, if any.
    dialog: Option<Entity<InitDialog>>,
    _filter_events: Subscription,
}

impl WelcomeView {
    pub fn new(
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Filter recent…", cx);
            input
        });
        let filter_events = cx.subscribe(&filter, |this, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(text) = event {
                this.filter_text = text.clone();
                // Typing always starts again at the first match.
                this.selected = 0;
                cx.notify();
            }
        });

        Self {
            git,
            workspace,
            recent: read_recent(cx),
            home: home_dir(),
            filter,
            filter_text: String::new(),
            selected: 0,
            branches: HashMap::new(),
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            open_focus: cx.focus_handle().tab_stop(true),
            clone_focus: cx.focus_handle().tab_stop(false),
            init_focus: cx.focus_handle().tab_stop(true),
            dialog: None,
            _filter_events: filter_events,
        }
    }

    /// The focus handle of the recent list.
    pub fn list_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn open_repository_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository_dialog(window, cx);
            });
        }
    }

    /// Re-reads the settings' recent list; a changed list clamps the
    /// selection instead of leaving it past the end.
    fn sync_recent(&mut self, cx: &App) {
        let recent = read_recent(cx);
        if recent != self.recent {
            self.recent = recent;
            let len = self.filtered_len();
            self.selected = clamp_selection(self.selected, len);
        }
    }

    /// Starts one background HEAD read per row that has none yet.
    fn ensure_branches(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = &self.git {
            log::debug!("recent branches are unavailable: {error}");
            for repo in &self.recent {
                self.branches
                    .entry(repo.path.clone())
                    .or_insert(BranchState::Known(None));
            }
            return;
        }
        let missing: Vec<PathBuf> = self
            .recent
            .iter()
            .filter(|repo| !self.branches.contains_key(&repo.path))
            .map(|repo| repo.path.clone())
            .collect();
        for path in missing {
            let git = match &self.git {
                Ok(git) => git.clone(),
                Err(_) => return,
            };
            self.branches.insert(path.clone(), BranchState::Loading);
            cx.spawn(async move |this, cx| {
                let read_path = path.clone();
                let head = cx
                    .background_executor()
                    .spawn(async move {
                        Repo::open(git, &read_path)
                            .and_then(|repo| repo.head())
                            .ok()
                    })
                    .await;
                let name = match head {
                    Some(Head::Branch { name, .. }) => Some(SharedString::from(name)),
                    Some(Head::Detached(_)) | None => None,
                };
                this.update(cx, |view, cx| {
                    view.branches.insert(path, BranchState::Known(name));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn matches(&self, repo: &RecentRepo) -> bool {
        matches_filter(
            &recent_name(&repo.path),
            &display_path(&repo.path, self.home.as_deref()),
            &self.filter_text,
        )
    }

    fn filtered_len(&self) -> usize {
        self.recent.iter().filter(|repo| self.matches(repo)).count()
    }

    fn filtered_rows(&self) -> Vec<RecentRow> {
        let now = unix_now();
        self.recent
            .iter()
            .filter(|repo| self.matches(repo))
            .map(|repo| {
                let branch = match self.branches.get(&repo.path) {
                    Some(BranchState::Known(Some(name))) => Some(name.clone()),
                    _ => None,
                };
                RecentRow {
                    path: repo.path.clone(),
                    name: recent_name(&repo.path).into(),
                    path_label: display_path(&repo.path, self.home.as_deref()).into(),
                    branch,
                    time: relative_time(now, repo.last_opened).into(),
                }
            })
            .collect()
    }

    fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.filtered_len() && index != self.selected {
            self.selected = index;
            cx.notify();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.filtered_len();
        if len == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
        if next != self.selected {
            self.selected = next;
            self.list_scroll
                .scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_row(self.selected, window, cx);
    }

    fn open_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.filtered_rows();
        let Some(row) = rows.get(index) else {
            return;
        };
        let path = row.path.clone();
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx);
            });
        }
    }

    /// Drops the selected row from the recent list and keeps a valid
    /// selection.
    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let rows = self.filtered_rows();
        let Some(row) = rows.get(self.selected) else {
            return;
        };
        let path = row.path.clone();
        crate::update_settings(cx, |settings| settings.remove_recent(&path));
        self.sync_recent(cx);
        let len = self.filtered_len();
        self.selected = clamp_selection(self.selected, len);
        cx.notify();
    }

    fn render_left(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let git_icon = Icon::new(IconName::Git).size(px(26.)).color(colors.text);

        let open = row_action(cx, |view, window, cx| {
            view.open_repository_dialog(window, cx)
        });
        let init = row_action(cx, |view, window, cx| view.open_init_dialog(window, cx));

        let open_ring = focus::focus_visible(&self.open_focus, window, cx);
        let init_ring = focus::focus_visible(&self.init_focus, window, cx);

        div()
            .w(px(LEFT_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(24.))
            .py(px(28.))
            .bg(colors.surface)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mb(px(4.))
                    .child(git_icon)
                    .child(
                        div()
                            .text_size(FS_XL)
                            .font_weight(FontWeight::BOLD)
                            .child("GitWarp"),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .mb(px(16.))
                    .text_color(colors.muted)
                    .child("A fast, keyboard-first git client."),
            )
            .child(action_row(
                ActionSpec {
                    id: "welcome-open",
                    icon: IconName::Folder,
                    label: "Open repository…",
                    kbd: "Ctrl O",
                    disabled: false,
                    tooltip: "Open an existing repository",
                },
                self.open_focus.clone(),
                open_ring,
                cx,
                Some(open),
            ))
            .child(action_row(
                ActionSpec {
                    id: "welcome-clone",
                    icon: IconName::Clone,
                    label: "Clone repository…",
                    kbd: "Ctrl Shift O",
                    disabled: true,
                    tooltip: "Clone arrives in a later version",
                },
                self.clone_focus.clone(),
                false,
                cx,
                None,
            ))
            .child(action_row(
                ActionSpec {
                    id: "welcome-init",
                    icon: IconName::Init,
                    label: "Init new repository…",
                    kbd: "Ctrl N",
                    disabled: false,
                    tooltip: "Create a new repository",
                },
                self.init_focus.clone(),
                init_ring,
                cx,
                Some(init),
            ))
    }

    fn render_right(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let rows = self.filtered_rows();

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(FS_LG)
                    .font_weight(FontWeight::BOLD)
                    .child("Recent repositories"),
            )
            .child(
                div()
                    .w(px(240.))
                    .flex_none()
                    .child(SearchField::new("recent-filter", self.filter.clone())),
            );

        let panel = div()
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .track_focus(&self.focus)
            .bg(colors.bg)
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .overflow_hidden();

        let empty = if self.recent.is_empty() {
            Some(
                EmptyState::new("No recent repositories")
                    .icon(IconName::Folder)
                    .text("Open or clone a repository to see it here.")
                    .into_any_element(),
            )
        } else if rows.is_empty() {
            Some(
                EmptyState::new("No matches")
                    .icon(IconName::Folder)
                    .text(format!(
                        "No recent repository matches “{}”.",
                        self.filter_text
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };

        let panel = if let Some(empty) = empty {
            panel.items_center().justify_center().child(empty)
        } else {
            let weak = cx.entity().downgrade();
            let focus = self.focus.clone();
            let selected = self.selected;
            let count = rows.len();
            let list = uniform_list("recent-list", count, move |range, _, _| {
                range
                    .map(|index| {
                        render_recent_row(
                            index,
                            &rows[index],
                            index == selected,
                            index + 1 == count,
                            weak.clone(),
                            focus.clone(),
                            colors,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.list_scroll)
            .flex_1()
            .min_h_0();
            panel.child(list).child(Scrollbar::uniform(
                "recent-scrollbar",
                self.list_scroll.clone(),
            ))
        };

        let hint = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Kbd::new("↑"))
            .child(Kbd::new("↓"))
            .child("select ·")
            .child(Kbd::new("Enter"))
            .child("open ·")
            .child(Kbd::new("Del"))
            .child("remove from list");

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(24.))
            .py(px(20.))
            .child(header)
            .child(panel)
            .child(hint)
    }

    /// Clears a dialog state whose layer was closed with Esc or the scrim.
    fn prune_dialog(&mut self, window: &Window, cx: &App) {
        let Some(dialog) = &self.dialog else {
            return;
        };
        let open = dialog
            .read(cx)
            .layer
            .is_some_and(|layer| overlay::layer_view(layer, window, cx).is_some());
        if !open {
            self.dialog = None;
        }
    }

    fn notify_dialog(&self, cx: &mut App) {
        if let Some(dialog) = &self.dialog {
            if let Some(host) = dialog.read(cx).host.clone() {
                host.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// Opens the Initialize dialog; reopens it when the old layer is gone.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.dialog {
            if let Some(layer) = dialog.read(cx).layer {
                if overlay::layer_view(layer, window, cx).is_some() {
                    return;
                }
            }
        }

        let folder = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("~/develop/new-project", cx);
            input
        });
        let branch = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("main", cx);
            input.set_text("main", cx);
            input
        });
        let folder_focus = folder.read(cx).focus_handle().clone();
        let branch_focus = branch.read(cx).focus_handle().clone();

        let dialog = cx.new(|cx| InitDialog {
            layer: None,
            host: None,
            folder,
            branch,
            folder_focus,
            branch_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            checkbox_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            init_focus: cx.focus_handle().tab_stop(true),
            initial_commit: false,
            busy: false,
            error: None,
            _subscriptions: Vec::new(),
        });

        let state = dialog.downgrade();
        let view = cx.entity().downgrade();
        let (layer, host) =
            dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
                Some(state) => build_init_dialog(&state, &view, cx),
                None => Dialog::new("Initialize repository"),
            });

        let subscriptions = {
            let folder = dialog.read(cx).folder.clone();
            let branch = dialog.read(cx).branch.clone();
            let folder_host = host.clone();
            let branch_host = host.clone();
            vec![
                cx.subscribe(&folder, move |_, _, _: &TextInputEvent, cx| {
                    folder_host.update(cx, |_, cx| cx.notify());
                }),
                cx.subscribe(&branch, move |_, _, _: &TextInputEvent, cx| {
                    branch_host.update(cx, |_, cx| cx.notify());
                }),
            ]
        };
        dialog.update(cx, |dialog, _| {
            dialog.layer = Some(layer);
            dialog.host = Some(host);
            dialog._subscriptions = subscriptions;
        });
        self.dialog = Some(dialog);
        cx.notify();
    }

    fn update_init(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut InitDialog)) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        dialog.update(cx, |dialog, cx| {
            f(dialog);
            if let Some(host) = &dialog.host {
                host.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        });
    }

    /// Runs `git init` off the UI thread, then opens the new repository.
    fn start_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let (folder_text, branch_text, initial_commit, busy) = {
            let dialog = dialog.read(cx);
            (
                dialog.folder.read(cx).text().trim().to_owned(),
                dialog.branch.read(cx).text().trim().to_owned(),
                dialog.initial_commit,
                dialog.busy,
            )
        };
        if busy || folder_text.is_empty() {
            return;
        }
        let path = expand_home(&folder_text, self.home.as_deref());
        let branch = (!branch_text.is_empty()).then_some(branch_text);
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.set_init_error(error.to_string(), cx);
                return;
            }
        };

        self.update_init(cx, |dialog| {
            dialog.busy = true;
            dialog.error = None;
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Repo::init_with(git, &path, branch.as_deref(), initial_commit)
                        .map(|repo| repo.root().to_path_buf())
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update_in(cx, |view, window, cx| view.finish_init(result, window, cx))
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
                if let Some(dialog) = self.dialog.take() {
                    if let Some(layer) = dialog.read(cx).layer {
                        dialog::close(layer, window, cx);
                    }
                }
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.open_repository(path, window, cx);
                    });
                }
            }
            Err(message) => self.set_init_error(message, cx),
        }
    }

    fn set_init_error(&mut self, message: String, cx: &mut Context<Self>) {
        let message = message.trim().to_owned();
        let message = if message.is_empty() {
            "git failed without a message".to_owned()
        } else {
            message
        };
        self.update_init(cx, |dialog| {
            dialog.busy = false;
            dialog.error = Some(message.into());
        });
    }

    fn browse_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let folder = dialog.read(cx).folder.clone();
        let home = self.home.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a folder to initialize".into()),
        });
        cx.spawn(async move |this, cx| {
            let response = receiver.await;
            this.update(cx, |view, cx| {
                match response {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            let label = display_path(&path, home.as_deref());
                            folder.update(cx, |input, cx| input.set_text(label, cx));
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => log::warn!("folder picker failed: {error}"),
                    Err(error) => log::warn!("folder picker did not answer: {error}"),
                }
                view.notify_dialog(cx);
            })
            .ok();
        })
        .detach();
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_dialog(window, cx);
        self.sync_recent(cx);
        self.ensure_branches(cx);

        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let colors = cx.theme().colors;
        let git = git_label(&self.git);

        let bar = titlebar(cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::Git).size(px(16.)))
                    .child("GitWarp"),
            )
            .child(titlebar_fill());

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .key_context("welcome")
            .on_action(
                cx.listener(|view, _: &NewRepository, window, cx| {
                    view.open_init_dialog(window, cx)
                }),
            )
            .on_action(cx.listener(|view, _: &RemoveSelected, _, cx| view.remove_selected(cx)))
            .on_action(
                cx.listener(|view, _: &ui::actions::SelectNext, _, cx| view.move_selection(1, cx)),
            )
            .on_action(
                cx.listener(|view, _: &ui::actions::SelectPrev, _, cx| view.move_selection(-1, cx)),
            )
            .on_action(cx.listener(|view, _: &ui::actions::Confirm, window, cx| {
                view.open_selected(window, cx)
            }))
            // Enter in the filter belongs to the list, not to the text field.
            .capture_action(
                cx.listener(|view, _: &TextInputSubmit, window, cx| view.open_selected(window, cx)),
            )
            .child(titlebar_end(bar, client))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(self.render_left(window, cx))
                    .child(self.render_right(cx)),
            )
            .child(
                div()
                    .h(STATUS)
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.))
                    .bg(colors.sidebar)
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                    .child(git),
            )
    }
}

/// The Initialize dialog's state. The dialog builder reads it on every frame,
/// so `busy` and `error` take effect as soon as they change.
struct InitDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    folder: Entity<TextInput>,
    branch: Entity<TextInput>,
    folder_focus: FocusHandle,
    branch_focus: FocusHandle,
    browse_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    cancel_focus: FocusHandle,
    init_focus: FocusHandle,
    initial_commit: bool,
    busy: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

/// Builds the Initialize dialog from its state. The builder runs on every
/// frame, so it only reads state that already exists.
fn build_init_dialog(
    state: &Entity<InitDialog>,
    view: &WeakEntity<WelcomeView>,
    cx: &App,
) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let empty = dialog.folder.read(cx).text().trim().is_empty();
    let busy = dialog.busy;

    let browse: ClickAction = {
        let view = view.clone();
        Box::new(move |_, _, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| view.browse_folder(cx));
            }
        })
    };
    let toggle: ToggleAction = {
        let state = state.clone();
        Box::new(move |next, _, cx| {
            state.update(cx, |dialog, cx| {
                dialog.initial_commit = next;
                if let Some(host) = &dialog.host {
                    host.update(cx, |_, cx| cx.notify());
                }
            });
        })
    };
    let cancel = {
        let state = state.clone();
        let view = view.clone();
        move |window: &mut Window, cx: &mut App| {
            if let Some(layer) = state.read(cx).layer {
                dialog::close(layer, window, cx);
            }
            view.update(cx, |view, cx| {
                view.dialog = None;
                cx.notify();
            })
            .ok();
        }
    };
    let on_default = {
        let view = view.clone();
        move |window: &mut Window, cx: &mut App| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| view.start_init(window, cx));
            }
        }
    };
    let on_initialize = {
        let view = view.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| view.start_init(window, cx));
            }
        }
    };

    let label = |text: &'static str| div().text_size(FS_SM).text_color(colors.muted).child(text);

    let body = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label("Folder"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().child(dialog.folder.clone()))
                        .child(
                            Button::new("init-browse")
                                .label("Browse…")
                                .icon(IconName::Folder)
                                .disabled(busy)
                                .focus_handle(dialog.browse_focus.clone())
                                .on_click(browse),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label("Default branch"))
                .child(dialog.branch.clone()),
        )
        .child(
            Checkbox::new("init-initial-commit")
                .label("Create an initial commit with a .gitignore")
                .checked(dialog.initial_commit)
                .disabled(busy)
                .focus_handle(dialog.checkbox_focus.clone())
                .on_toggle(toggle),
        )
        .when_some(dialog.error.clone(), |this, error| {
            this.child(
                Banner::new(BannerKind::Error, "Could not initialize the repository").body(error),
            )
        });

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("init-cancel")
                .label("Cancel")
                .min_width(px(72.))
                .disabled(busy)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(move |_, window, cx| cancel(window, cx)),
        )
        .child(
            Button::primary("init-initialize")
                .label("Initialize")
                .min_width(px(88.))
                .disabled(empty || busy)
                .focus_handle(dialog.init_focus.clone())
                .on_click(on_initialize),
        );

    let focus_order = vec![
        DialogFocus::input(dialog.folder_focus.clone()),
        DialogFocus::control(dialog.browse_focus.clone()),
        DialogFocus::input(dialog.branch_focus.clone()),
        DialogFocus::control(dialog.checkbox_focus.clone()),
        DialogFocus::control(dialog.cancel_focus.clone()),
        DialogFocus::control(dialog.init_focus.clone()),
    ];

    Dialog::new("Initialize repository")
        .body(body)
        .footer(footer)
        .width(px(520.))
        .busy(busy)
        .focus_order(focus_order)
        .autofocus(dialog.folder_focus.clone())
        .default_button(dialog.init_focus.clone())
        .on_default(on_default)
}

/// What one left-column action row shows.
struct ActionSpec {
    id: &'static str,
    icon: IconName,
    label: &'static str,
    kbd: &'static str,
    disabled: bool,
    tooltip: &'static str,
}

type RowAction = Rc<dyn Fn(&mut Window, &mut App)>;
type ClickAction = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ToggleAction = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A click handler for an action row, updating the welcome view.
fn row_action(
    cx: &Context<WelcomeView>,
    f: impl Fn(&mut WelcomeView, &mut Window, &mut Context<WelcomeView>) + 'static,
) -> RowAction {
    let view = cx.entity().downgrade();
    Rc::new(move |window, cx| {
        view.update(cx, |view, cx| f(view, window, cx)).ok();
    })
}

/// One action row: icon, label, key cap on the right. `Button` cannot render
/// this shape (it is 26px, centred, and has no trailing key cap), so the row
/// is a styled clickable div with the kit's hover and focus ring.
fn action_row(
    spec: ActionSpec,
    focus: FocusHandle,
    ring: bool,
    cx: &App,
    action: Option<RowAction>,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    let foreground = if spec.disabled {
        colors.disabled
    } else {
        colors.text
    };
    let mut row = div()
        .id(spec.id)
        .relative()
        .w_full()
        .h(px(ACTION_H))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .rounded(R_MD)
        .border_1()
        .border_color(if spec.disabled {
            colors.border
        } else {
            colors.border_strong
        })
        .bg(colors.bg)
        .text_color(foreground)
        .font_family(FONT_UI)
        .text_size(FS_BASE)
        .tooltip(Tooltip::text(spec.tooltip));

    if spec.disabled {
        row = row.cursor_not_allowed();
    } else {
        let hover = colors.hover;
        row = row
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .track_focus(&focus);
        if let Some(action) = action {
            let click = action.clone();
            row = row
                .on_click(move |_, window, cx| click(window, cx))
                .on_action(move |_: &ui::actions::Confirm, window, cx| action(window, cx));
        }
    }

    row = row
        .child(Icon::new(spec.icon).size(px(16.)).color(foreground))
        .child(div().flex_1().min_w_0().truncate().child(spec.label))
        .child(
            div()
                .flex_none()
                .when(spec.disabled, |this| this.opacity(0.6))
                .child(Kbd::new(spec.kbd)),
        );

    if ring {
        row = row.child(focus::focus_ring(focus::RingOffset::Inset2, R_MD, cx));
    }
    row
}

/// One 44px recent repository row with the design's four columns.
fn render_recent_row(
    index: usize,
    row: &RecentRow,
    selected: bool,
    last: bool,
    weak: WeakEntity<WelcomeView>,
    focus: FocusHandle,
    colors: ThemeColors,
) -> impl IntoElement {
    let mut element = div()
        .id(ElementId::named_usize("recent-row", index))
        .relative()
        .w_full()
        .h(px(ROW_H))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .bg(if selected {
            colors.selection
        } else {
            transparent_black()
        })
        .cursor_pointer();

    if !last {
        element = element.border_b_1().border_color(colors.border);
    }
    if !selected {
        let hover = colors.hover;
        element = element.hover(move |style| style.bg(hover));
    }

    element = element.on_click(move |event: &ClickEvent, window, cx| {
        window.focus(&focus, cx);
        let open = event.click_count() >= 2;
        weak.update(cx, |view, cx| {
            view.select_row(index, cx);
            if open {
                view.open_row(index, window, cx);
            }
        })
        .ok();
    });

    element
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .child(Icon::new(IconName::Folder).size(px(16.)).color(colors.text)),
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
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(FS_BASE)
                        .text_color(colors.text)
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(MONO)
                        .text_color(colors.muted)
                        .child(row.path_label.clone()),
                ),
        )
        .child(
            div()
                .w(px(BRANCH_W))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .when_some(row.branch.clone(), |this, branch| {
                    this.child(
                        Icon::new(IconName::Branch)
                            .size(px(12.))
                            .color(colors.muted),
                    )
                    .child(div().min_w_0().truncate().child(branch))
                })
                .when(row.branch.is_none(), |this| this.child("—")),
        )
        .child(
            div()
                .w(px(TIME_W))
                .flex_none()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .text_right()
                .child(row.time.clone()),
        )
}

fn read_recent(cx: &App) -> Vec<RecentRepo> {
    cx.try_global::<crate::AppSettings>()
        .map(|state| state.settings.recent.clone())
        .unwrap_or_default()
}

/// The last path component, without touching the file system.
fn recent_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Case-insensitive substring match on the name or the full displayed path.
fn matches_filter(name: &str, path: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    name.to_lowercase().contains(&filter) || path.to_lowercase().contains(&filter)
}

/// Keeps a selection valid when rows disappear; an empty list selects nothing
/// (index 0 is harmless because opening and removing check the length).
fn clamp_selection(selected: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        selected.min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_is_case_insensitive_on_the_name_or_the_path() {
        assert!(matches_filter("GitWarp", "~/dev/gitwarp", ""));
        assert!(matches_filter("GitWarp", "~/dev/gitwarp", "git"));
        assert!(matches_filter("GitWarp", "~/dev/gitwarp", "GITWARP"));
        assert!(matches_filter("notes", "/mnt/data/notes", "mnt/Data"));
        assert!(!matches_filter("notes", "/mnt/data/notes", "gitwarp"));
    }

    #[test]
    fn the_selection_is_clamped_after_removal_and_filtering() {
        // A list of three rows after Del shortens to two: index 2 clamps to 1.
        assert_eq!(clamp_selection(2, 2), 1);
        assert_eq!(clamp_selection(1, 2), 1);
        // Typing a filter that matches nothing leaves a harmless zero.
        assert_eq!(clamp_selection(3, 0), 0);
        assert_eq!(clamp_selection(0, 0), 0);
        assert_eq!(clamp_selection(4, 5), 4);
    }

    #[test]
    fn the_row_name_is_the_last_component() {
        assert_eq!(recent_name(Path::new("/home/user/gitwarp")), "gitwarp");
        assert_eq!(recent_name(Path::new("/")), "/");
    }

    /// Opens the real welcome window and drives the Initialize dialog through
    /// its action, so the builder, the focus trap and Esc are exercised.
    #[gpui::test]
    fn ctrl_n_opens_and_escape_closes_the_initialize_dialog(cx: &mut gpui::TestAppContext) {
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

        cx.update(|window, cx| window.dispatch_action(Box::new(NewRepository), cx));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            1,
            "Ctrl N did not open the Initialize dialog"
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
