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
    ElementId, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    Subscription, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::{Error, Git, Head, Repo};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_SM, FS_XL, MONO, R_MD, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, focus, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus,
    DialogHost, EmptyState, Icon, IconName, Kbd, Scrollbar, SearchField, TextInput, TextInputEvent,
    TextInputSubmit, ToastKind, Tooltip,
};

use crate::clone_dialog::CloneDialog;
use crate::format::{display_path, expand_home, home_dir, relative_time};
use crate::history_view::unix_now;
use crate::init_flow::{banner_body, details, DialogKind, InitFlow, StatusKind};
use crate::settings::RecentRepo;
use crate::welcome_activity::WelcomeActivity;
use crate::workspace::{
    git_label, git_tooltip, inline_window_controls, repo_name, title_strip, titlebar, titlebar_end,
    titlebar_fill, uses_title_strip, Workspace,
};

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
    settings_focus: FocusHandle,
    /// The open Initialize dialog, if any.
    dialog: Option<Entity<InitDialog>>,
    /// The open Clone dialog, if any.
    clone_dialog: Option<Entity<CloneDialog>>,
    /// The "Your activity" card above the recent list.
    activity: Entity<WelcomeActivity>,
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
            input.set_placeholder(ui::t!("welcome.recent.filter"), cx);
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

        let activity = {
            let (git, workspace) = (git.clone(), workspace.clone());
            cx.new(|cx| WelcomeActivity::new(git, workspace, cx))
        };
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
            settings_focus: cx.focus_handle().tab_stop(true),
            dialog: None,
            clone_dialog: None,
            activity,
            _filter_events: filter_events,
        }
    }

    /// The focus handle of the recent list.
    pub fn list_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Adopts a newly resolved git (after a Retry) and repaints the label.
    pub fn set_git(&mut self, git: Result<Arc<Git>, Arc<Error>>, cx: &mut Context<Self>) {
        self.git = git.clone();
        self.activity
            .update(cx, |activity, cx| activity.set_git(git, cx));
        cx.notify();
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

    /// Tells the activity card the recent list and the selected row.
    fn sync_activity(&self, cx: &mut Context<Self>) {
        // Most recently opened first: worktrees of one repository are listed
        // under the newest entry.
        let mut by_recency: Vec<&RecentRepo> = self.recent.iter().collect();
        by_recency.sort_by_key(|repo| std::cmp::Reverse(repo.last_opened));
        let paths = by_recency.iter().map(|repo| repo.path.clone()).collect();
        let selected = self
            .recent
            .iter()
            .filter(|repo| self.matches(repo))
            .nth(self.selected)
            .map(|repo| repo.path.clone());
        self.activity
            .update(cx, |activity, cx| activity.sync(paths, selected, cx));
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
        let logo = crate::app_icon::logo(px(26.));

        let open = row_action(cx, |view, window, cx| {
            view.open_repository_dialog(window, cx)
        });
        let init = row_action(cx, |view, window, cx| view.open_init_dialog(window, cx));

        let open_ring = focus::focus_visible(&self.open_focus, window, cx);
        let init_ring = focus::focus_visible(&self.init_focus, window, cx);
        let settings_ring = focus::focus_visible(&self.settings_focus, window, cx);

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
                    .child(logo)
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
                    .child(ui::t!("welcome.tagline")),
            )
            .child(action_row(
                ActionSpec {
                    id: "welcome-open",
                    icon: IconName::Folder,
                    label: ui::t!("welcome.action.open"),
                    kbd: Some("Ctrl+O"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.open_tooltip"),
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
                    label: ui::t!("welcome.action.clone"),
                    kbd: Some("Ctrl+Shift+O"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.clone_tooltip"),
                },
                self.clone_focus.clone(),
                false,
                cx,
                {
                    let view = cx.entity().downgrade();
                    Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                        view.update(cx, |view, cx| view.open_clone_dialog(window, cx))
                            .ok();
                    }))
                },
            ))
            .child(action_row(
                ActionSpec {
                    id: "welcome-init",
                    icon: IconName::Init,
                    label: ui::t!("welcome.action.init"),
                    kbd: Some("Ctrl+N"),
                    disabled: false,
                    tooltip: ui::t!("welcome.action.init_tooltip"),
                },
                self.init_focus.clone(),
                init_ring,
                cx,
                Some(init),
            ))
            // Spacer: pushes Settings to the bottom of the panel.
            .child(div().flex_1())
            .child(action_row(
                ActionSpec {
                    id: "welcome-settings",
                    icon: IconName::Gear,
                    label: ui::t!("repo.toolbar.settings"),
                    kbd: Some("Ctrl+,"),
                    disabled: false,
                    tooltip: ui::t!("palette.command.open_settings"),
                },
                self.settings_focus.clone(),
                settings_ring,
                cx,
                {
                    let workspace = self.workspace.clone();
                    Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                        let _ = workspace
                            .update(cx, |workspace, cx| workspace.open_settings(window, cx));
                    }))
                },
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
                    .child(ui::t!("welcome.recent.title")),
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
                EmptyState::new(ui::t!("welcome.recent.empty_title"))
                    .icon(IconName::Folder)
                    .text(ui::t!("welcome.recent.empty_text"))
                    .into_any_element(),
            )
        } else if rows.is_empty() {
            Some(
                EmptyState::new(ui::t!("welcome.recent.no_matches_title"))
                    .icon(IconName::Folder)
                    .text(ui::t!(
                        "welcome.recent.no_matches_text",
                        filter = self.filter_text
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
            // The scrollbar overlays the rows: as an in-flow sibling its 10px
            // track stays even without overflow and cuts the row highlight short.
            let scrollbar = div().absolute().top_0().bottom_0().right_0();
            panel
                .relative()
                .child(list)
                .child(scrollbar.child(Scrollbar::uniform(
                    "recent-scrollbar",
                    self.list_scroll.clone(),
                )))
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
            .child(ui::t!("welcome.recent.hint.select"))
            .child(Kbd::new("Enter"))
            .child(ui::t!("welcome.recent.hint.open"))
            .child(Kbd::new("Del"))
            .child(ui::t!("welcome.recent.hint.remove"));

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(24.))
            .py(px(20.))
            .children(crate::activity_strip::enabled(cx).then(|| self.activity.clone()))
            .child(header)
            .child(panel)
            .child(hint)
    }

    /// Clears a dialog state whose layer was closed with Esc or the scrim.
    fn prune_dialog(&mut self, window: &Window, cx: &mut App) {
        if let Some(dialog) = &self.dialog {
            let open = dialog
                .read(cx)
                .layer
                .is_some_and(|layer| overlay::layer_view(layer, window, cx).is_some());
            if !open {
                self.dialog = None;
            }
        }
        if let Some(dialog) = &self.clone_dialog {
            let open = dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx));
            if open {
                self.clone_dialog = None;
            }
        }
    }

    /// Opens the Clone repository dialog; brings an existing one back if its
    /// layer was closed with Esc.
    pub(crate) fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.clone_dialog {
            if !dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx)) {
                return;
            }
        }
        let workspace = self.workspace.clone();
        let dialog = crate::clone_dialog::open(self.git.clone(), workspace, window, cx);
        self.clone_dialog = Some(dialog);
        cx.notify();
    }

    /// Opens the Initialize dialog; reopens it when the old layer is gone.
    ///
    /// The folder flow is the Open dialog's ([`crate::init_flow`]): the field
    /// is probed as it is typed, an existing repository is opened instead of
    /// initialized, and a missing folder is fine because `git init` creates it.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.dialog {
            if let Some(layer) = dialog.read(cx).layer {
                if overlay::layer_view(layer, window, cx).is_some() {
                    return;
                }
            }
        }

        let git = self.git.clone();
        let flow = cx.new(|_| InitFlow::new(git));
        let view = cx.entity().downgrade();
        flow.update(cx, |flow, _| {
            flow.set_on_open(Rc::new(move |root: PathBuf, created: bool, window, cx| {
                let Some(view) = view.upgrade() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    let label = display_path(&root, view.home.as_deref());
                    let name = repo_name(&root);
                    if let Some(dialog) = view.dialog.take() {
                        dialog.update(cx, |dialog, cx| dialog.close(window, cx));
                    }
                    if let Some(workspace) = view.workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_repository(root, window, cx);
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

        let dialog = cx.new(|cx| InitDialog::new(flow, self.home.clone(), window, cx));

        let state = dialog.downgrade();
        let view = cx.entity().downgrade();
        let (layer, host) =
            dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
                Some(state) => build_init_dialog(&state, &view, cx),
                None => Dialog::new(ui::t!("welcome.init.title")),
            });

        dialog.update(cx, |dialog, _| {
            dialog.layer = Some(layer);
            dialog.host = Some(host);
        });
        self.dialog = Some(dialog);
        cx.notify();
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_dialog(window, cx);
        self.sync_recent(cx);
        let show_activity = crate::activity_strip::enabled(cx);
        self.activity
            .update(cx, |activity, cx| activity.set_enabled(show_activity, cx));
        self.sync_activity(cx);
        self.ensure_branches(cx);

        let client = inline_window_controls(window);
        let colors = cx.theme().colors;
        let git = git_label(&self.git);
        let workspace = self.workspace.clone();

        let bar = titlebar(cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::app_icon::logo(px(16.)))
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
            .when(uses_title_strip(), |column| {
                column.child(title_strip("GitWarp".into(), cx))
            })
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
                    .gap(px(6.))
                    .bg(colors.sidebar)
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .id("welcome-status-version")
                            .tooltip(Tooltip::text(ui::t!("welcome.status.version_tooltip")))
                            .child(ui::t!(
                                "welcome.status.version",
                                version = env!("CARGO_PKG_VERSION")
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("welcome-status-git")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .hover(|this| this.bg(colors.hover))
                            .tooltip(Tooltip::text(git_tooltip(&self.git)))
                            .on_click(
                                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                                    let _ = workspace.update(cx, |workspace, cx| {
                                        workspace.open_settings_at(
                                            crate::settings_ui::Page::Git,
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            )
                            .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                            .child(git),
                    ),
            )
    }
}

/// The Initialize dialog's state. The dialog builder reads it on every frame,
/// so the flow's busy, error and probe state take effect at once.
struct InitDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    /// The shared probe and initialize flow (also used by the Open dialog).
    flow: Entity<InitFlow>,
    folder: Entity<TextInput>,
    branch: Entity<TextInput>,
    home: Option<PathBuf>,
    /// Set while the dialog fills the field itself: the change event that
    /// queues behind it must not restart the probe with the debounce.
    ignore_change: bool,
    folder_focus: FocusHandle,
    branch_focus: FocusHandle,
    browse_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    cancel_focus: FocusHandle,
    init_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _flow_observer: Subscription,
}

/// What the folder picker answered; the portal may be missing entirely.
enum PickerOutcome {
    Picked(Option<PathBuf>),
    Cancelled,
    Failed(String),
}

impl InitDialog {
    fn new(
        flow: Entity<InitFlow>,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("~/develop/new-project", cx);
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
                if matches!(event, TextInputEvent::Changed(_)) {
                    this.probe_folder(cx);
                }
            }),
            cx.subscribe(&branch, |this, _, _: &TextInputEvent, cx| {
                this.sync_error_border(cx);
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
            flow,
            folder,
            branch,
            home,
            ignore_change: false,
            folder_focus,
            branch_focus,
            browse_focus: cx.focus_handle().tab_stop(true),
            checkbox_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            init_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
            _flow_observer: flow_observer,
        }
    }

    fn spec(&self, cx: &App) -> crate::init_flow::StatusSpec {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let branch_blank = self.branch.read(cx).text().trim().is_empty();
        self.flow
            .read(cx)
            .spec(DialogKind::Initialize, empty, branch_blank)
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    /// Probes the typed folder, one debounce after the last keystroke.
    fn probe_folder(&mut self, cx: &mut Context<Self>) {
        if self.ignore_change {
            // The dialog filled the field and probed at once already.
            self.ignore_change = false;
            return;
        }
        let text = self.folder.read(cx).text().trim().to_owned();
        if text.is_empty() {
            self.flow.update(cx, |flow, cx| flow.reset(cx));
        } else {
            let path = expand_home(&text, self.home.as_deref());
            self.flow.update(cx, |flow, cx| flow.probe(path, true, cx));
        }
        self.sync_error_border(cx);
        cx.notify();
    }

    /// A blank default branch keeps the primary disabled and marks the field.
    fn sync_error_border(&mut self, cx: &mut Context<Self>) {
        let empty = self.folder.read(cx).text().trim().is_empty();
        let blank = self.branch.read(cx).text().trim().is_empty();
        let error = self
            .flow
            .read(cx)
            .spec(DialogKind::Initialize, empty, blank)
            .kind
            == StatusKind::Plain
            && blank;
        self.branch
            .update(cx, |input, cx| input.set_error(error, cx));
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

    /// The primary button and Enter: open the repository the folder already
    /// belongs to, or initialize a plain folder.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.flow.read(cx).busy() {
            return;
        }
        let spec = self.spec(cx);
        if !spec.enabled {
            return;
        }
        match spec.kind {
            StatusKind::Repo | StatusKind::Nested => {
                let root = match self.flow.read(cx).result() {
                    Some(Ok(git_core::FolderProbe::Repo { root, .. }))
                    | Some(Ok(git_core::FolderProbe::Nested { root })) => root.clone(),
                    _ => return,
                };
                self.flow
                    .update(cx, |flow, cx| flow.open_root(root, window, cx));
            }
            StatusKind::Plain => {
                let text = self.folder.read(cx).text().trim().to_owned();
                if text.is_empty() {
                    return;
                }
                let path = expand_home(&text, self.home.as_deref());
                let branch_text = self.branch.read(cx).text().trim().to_owned();
                let branch = (!branch_text.is_empty()).then_some(branch_text);
                self.flow
                    .update(cx, |flow, cx| flow.start_init(path, branch, window, cx));
            }
            _ => {}
        }
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

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(ui::t!("welcome.init.picker_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = match receiver.await {
                Ok(Ok(Some(paths))) => PickerOutcome::Picked(paths.into_iter().next()),
                Ok(Ok(None)) => PickerOutcome::Cancelled,
                Ok(Err(error)) => PickerOutcome::Failed(error.to_string()),
                Err(error) => PickerOutcome::Failed(error.to_string()),
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
        outcome: PickerOutcome,
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
            PickerOutcome::Picked(Some(path)) => {
                let label = display_path(&path, home.as_deref());
                self.set_folder_text(label, cx);
                // The picker is an explicit choice: probe at once.
                self.flow.update(cx, |flow, cx| flow.probe(path, false, cx));
                self.notify_host(cx);
                cx.notify();
            }
            PickerOutcome::Picked(None) | PickerOutcome::Cancelled => {}
            PickerOutcome::Failed(message) => {
                // No xdg portal, no picker: the same toast as the Open dialog.
                toast::show(
                    ToastKind::Error,
                    ui::t!("welcome.open.picker_failed", message = message),
                    window,
                    cx,
                );
            }
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }
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
    let flow = dialog.flow.read(cx);
    let spec = dialog.spec(cx);
    let busy = flow.busy();
    let initial_commit = flow.initial_commit();
    let error = flow.error().cloned();

    let browse: ClickAction = {
        let state = state.clone();
        Box::new(move |_, window, cx| {
            state.update(cx, |dialog, cx| dialog.browse(window, cx));
        })
    };
    let toggle: ToggleAction = {
        let state = state.clone();
        Box::new(move |next, _, cx| {
            state.update(cx, |dialog, cx| {
                dialog
                    .flow
                    .update(cx, |flow, cx| flow.set_initial_commit(next, cx));
                dialog.notify_host(cx);
            });
        })
    };
    let activate = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.activate(window, cx));
        }
    };
    let cancel = {
        let state = state.clone();
        let view = view.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
            view.update(cx, |view, cx| {
                view.dialog = None;
                cx.notify();
            })
            .ok();
        }
    };
    let on_initialize = {
        let activate = activate.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| activate(window, cx)
    };

    let label = |text: SharedString| div().text_size(FS_SM).text_color(colors.muted).child(text);

    let mut status = div().flex().flex_col().gap(px(8.));
    let mut has_status = false;
    if let Some((kind, title)) = spec.banner {
        let mut banner = Banner::new(kind, title);
        if let Some(body) = banner_body(flow, DialogKind::Initialize) {
            banner = banner.body(body);
        }
        status = status.child(banner);
        has_status = true;
    }
    if let Some(extra) = details(flow, dialog.home.as_deref(), colors) {
        status = status.child(extra);
        has_status = true;
    }
    if let Some(error) = error {
        status = status
            .child(Banner::new(BannerKind::Error, ui::t!("welcome.open.init_failed")).body(error));
        has_status = true;
    }

    let mut body = div().flex().flex_col().gap(px(16.)).child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label(ui::t!("welcome.open.folder")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().child(dialog.folder.clone()))
                    .child(
                        Button::new("init-browse")
                            .label(ui::t!("welcome.open.browse"))
                            .icon(IconName::Folder)
                            .disabled(busy)
                            .focus_handle(dialog.browse_focus.clone())
                            .on_click(browse),
                    ),
            ),
    );
    if has_status {
        body = body.child(status);
    }
    body = body
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label(ui::t!("welcome.init.default_branch")))
                .child(dialog.branch.clone()),
        )
        .child(
            Checkbox::new("init-initial-commit")
                .label(ui::t!("welcome.init.initial_commit"))
                .checked(initial_commit)
                .disabled(busy)
                .focus_handle(dialog.checkbox_focus.clone())
                .on_toggle(toggle),
        );

    let primary_enabled = spec.enabled && !busy;
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("init-cancel")
                .label(ui::t!("welcome.common.cancel"))
                .min_width(px(72.))
                .disabled(busy)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(move |_, window, cx| cancel(window, cx)),
        )
        .child(
            Button::primary("init-initialize")
                .label(spec.primary)
                .min_width(px(88.))
                .disabled(!primary_enabled)
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

    let mut panel = Dialog::new(ui::t!("welcome.init.title"))
        .body(body)
        .footer(footer)
        .width(px(520.))
        .busy(busy)
        .focus_order(focus_order)
        .autofocus(dialog.folder_focus.clone())
        .default_button(dialog.init_focus.clone())
        .on_default(activate);
    if let Some(hint) = spec.hint {
        panel = panel.hint(hint);
    }
    panel
}

/// What one left-column action row shows.
struct ActionSpec {
    id: &'static str,
    icon: IconName,
    label: SharedString,
    /// Shown only for shortcuts that are actually bound.
    kbd: Option<&'static str>,
    disabled: bool,
    tooltip: SharedString,
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
        .child(div().flex_1().min_w_0().truncate().child(spec.label));
    if let Some(kbd) = spec.kbd {
        row = row.child(
            div()
                .flex_none()
                .when(spec.disabled, |this| this.opacity(0.6))
                .child(Kbd::new(kbd)),
        );
    }

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
pub(crate) fn recent_name(path: &Path) -> String {
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

    /// Opening an existing repository from the Initialize dialog runs from
    /// the dialog's own update; the flow re-enters it only after that
    /// returned, so no double entity lease.
    #[gpui::test]
    fn opening_an_existing_repository_closes_the_initialize_dialog(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("init-dialog");
        let git = crate::test_support::git();
        let (mut cx, workspace) = crate::test_support::open_workspace(git.clone(), cx);

        let view = cx.update(|window, cx| {
            cx.new(|cx| WelcomeView::new(Ok(git), workspace.downgrade(), window, cx))
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.open_init_dialog(window, cx));
        });
        let dialog = view
            .read_with(&cx, |view, _| view.dialog.clone())
            .expect("the Initialize dialog is open");

        dialog.update(&mut cx, |dialog, cx| {
            dialog.ignore_change = true;
            dialog.folder.update(cx, |input, cx| {
                input.set_text(repo.path().display().to_string(), cx)
            });
            dialog.flow.update(cx, |flow, cx| {
                flow.probe(repo.path().to_path_buf(), false, cx)
            });
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
            view.read_with(&cx, |view, _| view.dialog.is_none()),
            "the welcome view kept the closed dialog"
        );
        assert!(
            cx.debug_bounds("sidebar-filter").is_some(),
            "the workspace did not open the repository"
        );
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

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(crate::workspace::OpenCloneDialog), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| overlay::layer_count(window, cx)),
            1,
            "Ctrl Shift O did not open the Clone dialog"
        );
    }
}
