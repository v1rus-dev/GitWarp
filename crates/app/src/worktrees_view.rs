//! The Worktrees page: quick create, the selected worktree's details and the
//! actions on it (switch, open in a new window, reveal, lock, remove), over the
//! shared [`WorktreesModel`].
//!
//! The page is opened from the toolbar tab, the Worktrees section's context
//! menu, a worktree leaf's double-click, or the repository menu.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window,
};

use ui::actions::Cancel;
use ui::metrics::{FONT_MONO, FS_BASE, FS_LG, FS_MD, FS_SM, FS_XS, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    toast, Banner, BannerKind, Button, ButtonSize, Chip, ChipKind, Icon, IconName, Kbd, Skeleton,
    Spinner, StatusGlyph, TextInput, TextInputEvent, ToastKind,
};

use crate::format::display_path;
use crate::worktree_dialog::{
    self, Done, RemoveTarget, SelectedCommit, WorktreeDialog, WorktreeDialogContext,
};
use crate::worktrees_model::{BranchCheck, WorktreeEntry, WorktreesModel, WorktreesModelEvent};

/// How many changed files the detail pane lists before "+N more".
const CHANGES_SHOWN: usize = 5;

/// Emitted by the page for its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreesEvent {
    /// Esc with nothing open and no input focused: back to History.
    Leave,
}

pub struct WorktreesView {
    model: Entity<WorktreesModel>,
    selected_commit: Option<SelectedCommit>,
    focus: FocusHandle,
    quick: Entity<TextInput>,
    /// The quick-create input changed at least once, so errors may show.
    quick_touched: bool,
    dialog: Option<Entity<WorktreeDialog>>,
    _subscriptions: Vec<Subscription>,
}

impl WorktreesView {
    pub(crate) fn new(
        model: Entity<WorktreesModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let quick = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_placeholder(ui::i18n::t("worktrees.quick.placeholder"), cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &quick,
                window,
                |this, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed(_) => {
                        this.quick_touched = true;
                        this.sync_input(cx);
                        cx.notify();
                    }
                    TextInputEvent::Submit => this.submit_quick(window, cx),
                },
            ),
            cx.observe(&model, |this, _, cx| {
                this.sync_input(cx);
                cx.notify();
            }),
            cx.subscribe(&model, |this, _, event: &WorktreesModelEvent, cx| {
                if *event == WorktreesModelEvent::Created {
                    // The name was used: start the next one from scratch.
                    this.quick_touched = false;
                    this.quick.update(cx, |quick, cx| quick.set_text("", cx));
                }
            }),
        ];
        Self {
            model,
            selected_commit: None,
            focus: cx.focus_handle().tab_stop(false),
            quick,
            quick_touched: false,
            dialog: None,
            _subscriptions: subscriptions,
        }
    }

    /// Whether the add or remove dialog is open (tests).
    #[cfg(test)]
    pub(crate) fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Puts the cursor in the quick-create input (the sidebar's "New
    /// worktree…").
    pub(crate) fn focus_quick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.quick.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        self.quick.update(cx, |quick, cx| quick.select_all(cx));
    }

    /// The selected history commit, offered as a "Start from" option.
    pub(crate) fn set_selected(
        &mut self,
        selected: Option<SelectedCommit>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_commit == selected {
            return;
        }
        self.selected_commit = selected;
        cx.notify();
    }

    /// Adopts the repository root the history resolved.
    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.set_root(root, cx));
    }

    /// Reads the worktrees again.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.reload(cx));
    }

    /// Input state follows the model: locked while creating, red on an error.
    fn sync_input(&mut self, cx: &mut Context<Self>) {
        let creating = self.model.read(cx).is_creating();
        let error = self.quick_error(cx);
        self.quick.update(cx, |quick, cx| {
            quick.set_disabled(creating, cx);
            quick.set_error(error, cx);
        });
    }

    fn quick_text(&self, cx: &App) -> String {
        self.quick.read(cx).text().trim().to_owned()
    }

    fn quick_check(&self, cx: &App) -> BranchCheck {
        self.model.read(cx).check_branch(&self.quick_text(cx))
    }

    fn quick_error(&self, cx: &App) -> bool {
        self.quick_touched
            && matches!(
                self.quick_check(cx),
                BranchCheck::Invalid | BranchCheck::InUse { .. }
            )
    }

    /// Enter in the input: Create, when the name allows it.
    fn submit_quick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).is_creating() {
            return;
        }
        if matches!(
            self.quick_check(cx),
            BranchCheck::Existing | BranchCheck::Tracking(_) | BranchCheck::New
        ) {
            let name = self.quick_text(cx);
            self.create(&name, window, cx);
        }
    }

    fn create(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.create(name, window, cx));
    }

    fn dialog_context(&self, cx: &App) -> WorktreeDialogContext {
        let model = self.model.read(cx);
        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.worktrees.clone())
            .unwrap_or_default();
        WorktreeDialogContext {
            root: model.root().to_path_buf(),
            git: model.git().clone(),
            home: model.home().map(Path::to_path_buf),
            branches: model.branches().to_vec(),
            worktrees: model
                .entries()
                .iter()
                .map(|entry| entry.worktree.clone())
                .collect(),
            current_branch: model.current_branch().map(str::to_owned),
            selected: self.selected_commit.clone(),
            workspace: model.workspace().clone(),
            main_line: model.main_line().map(str::to_owned),
            confirm_delete_branch: settings.confirm_delete_branch,
            open_after: settings.after_create == crate::settings::AfterCreate::Switch,
        }
    }

    fn changed_callback(&self) -> Done {
        let model = self.model.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let _ = window;
            model.update(cx, |model, cx| model.operation_done(cx));
        })
    }

    fn close_previous_dialog(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(previous) = self.dialog.take() {
            previous.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    }

    pub(crate) fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (loaded, loading, empty) = {
            let model = self.model.read(cx);
            (
                model.is_loaded(),
                model.is_loading(),
                model.entries().is_empty(),
            )
        };
        if !loaded || loading && empty {
            toast::show(
                ToastKind::Info,
                ui::i18n::t("worktrees.toast.list_loading"),
                window,
                cx,
            );
            self.refresh(cx);
            return;
        }
        self.close_previous_dialog(window, cx);
        let context = self.dialog_context(cx);
        let on_done = self.changed_callback();
        self.dialog = Some(worktree_dialog::open_add(context, on_done, window, cx));
    }

    /// Opens the remove dialog, or removes at once when the worktree is clean
    /// and Settings → Worktrees no longer asks about clean ones.
    pub(crate) fn open_remove(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.model.read(cx).entry(path).cloned() else {
            toast::show(
                ToastKind::Warn,
                ui::i18n::t("worktrees.toast.gone"),
                window,
                cx,
            );
            self.refresh(cx);
            return;
        };
        let confirm_clean = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.worktrees.confirm_remove_clean)
            .unwrap_or(true);
        if !confirm_clean
            && !entry.is_main()
            && entry.locked().is_none()
            && entry.changes.is_empty()
        {
            self.model
                .update(cx, |model, cx| model.remove_clean(path, window, cx));
            return;
        }
        self.close_previous_dialog(window, cx);
        let target = RemoveTarget {
            path: entry.path().to_path_buf(),
            branch: entry.worktree.branch.clone(),
            locked: entry.worktree.locked.clone(),
            missing: entry.missing,
            main: entry.is_main(),
            changes: entry.changed_paths(),
        };
        let context = self.dialog_context(cx);
        let on_done = self.changed_callback();
        self.dialog = Some(worktree_dialog::open_remove(
            target, context, on_done, window, cx,
        ));
    }
}

impl EventEmitter<WorktreesEvent> for WorktreesView {}

impl Render for WorktreesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let model = self.model.read(cx);
        let count = model.entries().len();
        let stale = model.entries().iter().filter(|e| e.missing).count();
        let loaded = model.is_loaded();
        let error = model.error().map(str::to_owned);

        let mut header = div().flex().items_center().gap(px(8.)).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(FS_LG)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(ui::i18n::t("worktrees.view.title")),
                )
                .when(loaded && error.is_none(), |this| {
                    this.child(
                        div()
                            .text_size(FS_BASE)
                            .text_color(colors.muted)
                            .child(count.to_string()),
                    )
                }),
        );
        if stale > 0 {
            header = header.child(
                Button::new("worktrees-prune")
                    .label(ui::t!("worktrees.view.prune", n = stale))
                    .icon(IconName::Trash)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.model.update(cx, |model, cx| model.prune(window, cx));
                    })),
            );
        }

        let mut content = div()
            .id("worktrees-view")
            .debug_selector(|| "worktrees-view".to_string())
            .size_full()
            .flex()
            .flex_col()
            .gap(px(16.))
            .px(px(20.))
            .py(px(16.))
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .key_context("worktrees")
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                // Esc in the input belongs to the input; elsewhere it leaves
                // the page. (A dialog has its own focus and handles its Esc.)
                if this.quick.read(cx).focus_handle().is_focused(window) {
                    return;
                }
                cx.emit(WorktreesEvent::Leave);
            }))
            .child(header);

        if let Some(error) = error {
            return content
                .child(
                    Banner::new(BannerKind::Error, ui::i18n::t("worktrees.view.read_error"))
                        .body(error)
                        .mono_body(),
                )
                .child(
                    // Content-width button, not stretched to the column.
                    div().flex().child(
                        Button::new("worktrees-retry")
                            .label(ui::i18n::t("worktrees.view.retry"))
                            .icon(IconName::Fetch)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx))),
                    ),
                )
                .into_any_element();
        }
        if !loaded {
            return content.child(skeleton(colors)).into_any_element();
        }

        content = content.child(self.quick_card(colors, cx));
        if count == 1 {
            content = content.child(
                Banner::new(BannerKind::Info, ui::i18n::t("worktrees.view.only_title"))
                    .body(ui::i18n::t("worktrees.view.only_body")),
            );
        }
        let selected = self.model.read(cx).selected().cloned();
        if let Some(entry) = selected {
            content = content.child(self.detail(&entry, colors, cx));
        }
        content.child(key_hints(colors)).into_any_element()
    }
}

impl WorktreesView {
    /// The quick-create card: input, buttons, hint line and branch chips.
    fn quick_card(&self, colors: ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let text = self.quick_text(cx);
        let check = model.check_branch(&text);
        let creating = model.is_creating();
        let show_error =
            self.quick_touched && matches!(check, BranchCheck::Invalid | BranchCheck::InUse { .. });
        let can_create = matches!(
            check,
            BranchCheck::Existing | BranchCheck::Tracking(_) | BranchCheck::New
        ) && !creating;

        let hint: AnyElement = match &check {
            BranchCheck::Invalid if show_error => {
                error_hint(ui::i18n::t("worktrees.toast.invalid_branch"), colors)
            }
            BranchCheck::InUse { path, name } if show_error => {
                let path = path.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(error_hint(
                        ui::t!("worktrees.toast.branch_in_use", name = text, used = name),
                        colors,
                    ))
                    .child(
                        Button::ghost("worktrees-goto")
                            .size(ButtonSize::Small)
                            .label(ui::t!("worktrees.quick.goto", name = name))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let path = path.clone();
                                this.model.update(cx, |model, cx| model.select(&path, cx));
                            })),
                    )
                    .into_any_element()
            }
            BranchCheck::Existing | BranchCheck::Tracking(_) | BranchCheck::New => {
                let folder = display_path(&model.preview_path(&text, cx), model.home());
                let kind = match &check {
                    BranchCheck::Existing => {
                        ui::i18n::t("worktrees.quick.kind_existing").to_string()
                    }
                    BranchCheck::Tracking(remote) => {
                        ui::t!("worktrees.quick.kind_tracks", remote = remote).to_string()
                    }
                    _ => match model.main_line() {
                        Some(base) => {
                            ui::t!("worktrees.quick.kind_new_from", base = base).to_string()
                        }
                        None => ui::i18n::t("worktrees.quick.kind_new_from_current").to_string(),
                    },
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(
                        Icon::new(IconName::Folder)
                            .size(px(12.))
                            .color(colors.muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_color(colors.text)
                            .child(folder),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .child(format!("· {kind}")),
                    )
                    .into_any_element()
            }
            _ => {
                let settings = cx
                    .try_global::<crate::AppSettings>()
                    .map(|state| state.settings.worktrees.clone())
                    .unwrap_or_default();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(ui::i18n::t("worktrees.quick.template_hint"))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .whitespace_nowrap()
                            .child(format!("{}/{}", settings.base, settings.template)),
                    )
                    .into_any_element()
            }
        };

        let create_label = if creating {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(Spinner::new("worktrees-creating", px(14.)))
                .child(ui::i18n::t("worktrees.quick.creating"))
                .into_any_element()
        } else {
            Button::primary("worktrees-create")
                .icon(IconName::Plus)
                .label(ui::i18n::t("worktrees.quick.create"))
                .disabled(!can_create)
                .on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.submit_quick(window, cx)),
                )
                .into_any_element()
        };

        let free: Vec<String> = model
            .free_branches()
            .into_iter()
            .map(str::to_owned)
            .collect();

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::Branch)
                            .size(px(14.))
                            .color(colors.muted),
                    )
                    .child(div().flex_1().min_w_0().child(self.quick.clone()))
                    .child(create_label)
                    .child(
                        Button::new("worktrees-add")
                            .label(ui::i18n::t("worktrees.quick.add"))
                            .disabled(creating)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_add(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(22.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(hint),
            );
        if !free.is_empty() {
            let mut chips = div().flex().flex_wrap().items_center().gap(px(6.)).child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::i18n::t("worktrees.quick.free_branches")),
            );
            for (index, branch) in free.into_iter().enumerate() {
                let name = branch.clone();
                chips = chips.child(
                    Button::new(format!("worktrees-chip-{index}"))
                        .size(ButtonSize::Small)
                        .icon(IconName::Branch)
                        .label(branch)
                        // Design `.bchip` is set in the mono font.
                        .mono(true)
                        .disabled(creating)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.create(&name, window, cx)
                        })),
                );
            }
            card = card.child(chips);
        }
        card.into_any_element()
    }

    /// The selected worktree: title, actions, facts, local changes, footer.
    fn detail(
        &self,
        entry: &WorktreeEntry,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = self.model.read(cx).home().map(Path::to_path_buf);
        let path = entry.path().to_path_buf();
        let name = entry.name();
        let missing = entry.missing;
        let current = entry.is_current();
        let main = entry.is_main();
        let locked = entry.locked().is_some();

        let mut chips = div().flex().flex_wrap().items_center().gap(px(6.));
        if current {
            chips = chips.child(
                Chip::new("wt-chip-current", ui::i18n::t("worktrees.chip.current"))
                    .kind(ChipKind::Acc)
                    .icon(IconName::Check),
            );
        }
        if main {
            chips = chips.child(
                Chip::new("wt-chip-main", ui::i18n::t("worktrees.chip.default"))
                    .icon(IconName::Git),
            );
        }
        if let Some(reason) = entry.locked() {
            chips = chips.child(
                Chip::new(
                    "wt-chip-locked",
                    if reason.is_empty() {
                        ui::i18n::t("worktrees.chip.locked")
                    } else {
                        ui::t!("worktrees.chip.locked_reason", reason = reason)
                    },
                )
                .kind(ChipKind::Warn)
                .icon(IconName::Lock),
            );
        }
        if missing {
            chips = chips.child(
                Chip::new("wt-chip-missing", ui::i18n::t("worktrees.chip.missing"))
                    .kind(ChipKind::Err)
                    .icon(IconName::Warn),
            );
        } else if entry.changes.is_empty() {
            chips = chips.child(
                Chip::new("wt-chip-clean", ui::i18n::t("worktrees.chip.clean"))
                    .kind(ChipKind::Ok)
                    .icon(IconName::Check),
            );
        } else {
            chips = chips.child(
                Chip::new(
                    "wt-chip-dirty",
                    ui::t!("worktrees.chip.changed", n = entry.changes.len()),
                )
                .kind(ChipKind::Warn)
                .icon(IconName::Edit),
            );
        }

        let switch_path = path.clone();
        let window_path = path.clone();
        let copy_path = path.clone();
        let top = div()
            .flex()
            .items_start()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div().mt(px(2.)).child(
                    Icon::new(if main {
                        IconName::Git
                    } else {
                        IconName::Worktree
                    })
                    .size(px(20.))
                    .color(colors.accent),
                ),
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
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(FS_MD)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name.clone()),
                            )
                            .child(chips),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(FONT_MONO)
                                    .text_size(FS_SM)
                                    .text_color(colors.muted)
                                    .when(missing, |this| this.line_through())
                                    .child(display_path(&path, home.as_deref())),
                            )
                            .child(
                                Button::ghost("worktree-copy-path")
                                    .icon_only(IconName::Copy)
                                    .size(ButtonSize::Small)
                                    .tooltip(ui::i18n::t("worktrees.detail.copy_path"))
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.model.update(cx, |model, cx| {
                                                model.copy_path(&copy_path, window, cx)
                                            });
                                        },
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::primary("worktree-switch")
                            .label(if current {
                                ui::i18n::t("worktrees.detail.current")
                            } else {
                                ui::i18n::t("worktrees.detail.switch")
                            })
                            .disabled(current || missing)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.model.update(cx, |model, cx| {
                                    model.switch_to(&switch_path, window, cx)
                                });
                            })),
                    )
                    .child(
                        Button::new("worktree-window")
                            .label(ui::i18n::t("worktrees.detail.new_window"))
                            .disabled(missing)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.model.update(cx, |model, cx| {
                                    model.open_in_window(&window_path, window, cx)
                                });
                            })),
                    ),
            );

        let mut detail = div()
            .id("worktree-detail")
            .debug_selector(|| "worktree-detail".to_string())
            .flex()
            .flex_col()
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .bg(colors.surface)
            .child(top);

        if missing {
            detail = detail.child(
                div().px(px(16.)).py(px(12.)).child(
                    Banner::new(
                        BannerKind::Warn,
                        ui::i18n::t("worktrees.detail.missing_title"),
                    )
                    .body(ui::i18n::t("worktrees.detail.missing_body")),
                ),
            );
        }

        // BRANCH / HEAD / REMOTE
        let branch_label = entry.branch().map_or_else(
            || ui::i18n::t("worktrees.detached_head").to_string(),
            str::to_owned,
        );
        let head_short = entry
            .worktree
            .head
            .map(|oid| {
                let mut short = oid.to_string();
                short.truncate(7);
                short
            })
            .unwrap_or_default();
        let remote = if entry.ahead > 0 || entry.behind > 0 {
            let mut parts = Vec::new();
            if entry.ahead > 0 {
                parts.push(ui::t!("worktrees.detail.ahead", n = entry.ahead).to_string());
            }
            if entry.behind > 0 {
                parts.push(ui::t!("worktrees.detail.behind", n = entry.behind).to_string());
            }
            div().text_color(colors.text).child(parts.join(" · "))
        } else if entry.branch().is_none() {
            div()
                .text_color(colors.muted)
                .child(ui::i18n::t("worktrees.detail.no_upstream_detached"))
        } else if entry.has_upstream {
            div()
                .text_color(colors.muted)
                .child(ui::i18n::t("worktrees.detail.up_to_date"))
        } else {
            div()
                .text_color(colors.muted)
                .child(ui::i18n::t("worktrees.detail.no_upstream"))
        };
        let cell = |label: SharedString, value: AnyElement| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(FS_XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.muted)
                        .child(label.to_uppercase()),
                )
                .child(value)
        };
        detail = detail.child(
            div()
                .flex()
                .gap(px(16.))
                .px(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(colors.border)
                .text_size(FS_BASE)
                .child(cell(
                    ui::i18n::t("worktrees.detail.branch"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            Icon::new(IconName::Branch)
                                .size(px(14.))
                                .color(colors.muted),
                        )
                        .child(div().truncate().font_family(FONT_MONO).child(branch_label))
                        .into_any_element(),
                ))
                .child(cell(
                    ui::i18n::t("worktrees.detail.head"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .child(div().flex_none().font_family(FONT_MONO).child(head_short))
                        .child(
                            div()
                                .truncate()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(entry.head_summary.clone().unwrap_or_default()),
                        )
                        .into_any_element(),
                ))
                .child(cell(
                    ui::i18n::t("worktrees.detail.remote"),
                    remote.into_any_element(),
                )),
        );

        if !missing {
            let mut section = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .text_size(FS_XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.muted)
                        .child(ui::i18n::t("worktrees.detail.local_changes").to_uppercase()),
                );
            if entry.changes.is_empty() {
                section = section.child(
                    div()
                        .text_size(FS_BASE)
                        .text_color(colors.muted)
                        .child(ui::i18n::t("worktrees.detail.clean_note")),
                );
            } else {
                for change in entry.changes.iter().take(CHANGES_SHOWN) {
                    section = section.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(FS_BASE)
                            .child(StatusGlyph::new(change.status))
                            .child(div().min_w_0().truncate().child(change.path.clone())),
                    );
                }
                if entry.changes.len() > CHANGES_SHOWN {
                    section = section.child(div().text_size(FS_SM).text_color(colors.muted).child(
                        ui::t!(
                            "worktrees.detail.more",
                            n = entry.changes.len() - CHANGES_SHOWN
                        ),
                    ));
                }
            }
            detail = detail.child(section);
        }

        let reveal_path = path.clone();
        let lock_path = path.clone();
        let remove_path = path;
        let reveal_label = display_path(&reveal_path, home.as_deref());
        detail = detail.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(8.))
                .child(
                    Button::ghost("worktree-reveal")
                        .size(ButtonSize::Small)
                        .icon(IconName::Folder)
                        .label(ui::i18n::t("worktrees.detail.reveal"))
                        .disabled(missing)
                        .on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
                            cx.reveal_path(&reveal_path);
                            toast::show(
                                ToastKind::Info,
                                ui::t!("worktrees.toast.revealed", label = reveal_label),
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::ghost("worktree-lock")
                        .size(ButtonSize::Small)
                        .icon(IconName::Lock)
                        .label(if locked {
                            ui::i18n::t("worktrees.detail.unlock")
                        } else {
                            ui::i18n::t("worktrees.detail.lock")
                        })
                        .disabled(main || missing)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.model
                                .update(cx, |model, cx| model.toggle_lock(&lock_path, window, cx));
                        })),
                )
                .child(div().flex_1())
                .when(main, |this| {
                    this.child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(ui::i18n::t("worktrees.detail.main_note")),
                    )
                })
                .child(
                    Button::danger("worktree-remove")
                        .size(ButtonSize::Small)
                        .icon(IconName::Trash)
                        .label(ui::i18n::t("worktrees.detail.remove"))
                        .disabled(main)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.open_remove(&remove_path, window, cx)
                        })),
                ),
        );
        detail.into_any_element()
    }
}

fn error_hint(text: impl Into<SharedString>, colors: ThemeColors) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_color(colors.error)
        .child(Icon::new(IconName::Warn).size(px(12.)).color(colors.error))
        .child(text.into())
        .into_any_element()
}

fn skeleton(_colors: ThemeColors) -> AnyElement {
    let mut bars = div().flex().flex_col().gap(px(14.));
    for (index, width) in [360., 520., 280., 440.].into_iter().enumerate() {
        bars = bars.child(Skeleton::new(
            format!("worktrees-skeleton-{index}"),
            px(width),
        ));
    }
    bars.into_any_element()
}

fn key_hints(colors: ThemeColors) -> AnyElement {
    let group = |keys: &[&'static str], label: SharedString| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .children(keys.iter().map(|key| Kbd::new(*key)))
            .child(div().ml(px(2.)).child(label))
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(16.))
        .text_size(FS_SM)
        .text_color(colors.muted)
        // One cap with "+" between the keys, as everywhere else in the app.
        .child(group(
            &["Ctrl+Alt+W"],
            ui::i18n::t("worktrees.hints.switcher"),
        ))
        .child(group(&["Alt+1–9"], ui::i18n::t("worktrees.hints.jump")))
        .child(group(&["Enter"], ui::i18n::t("worktrees.hints.switch")))
        .child(group(&["Del"], ui::i18n::t("worktrees.hints.remove")))
        .into_any_element()
}
