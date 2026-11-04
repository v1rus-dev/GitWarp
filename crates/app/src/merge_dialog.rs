//! The Merge dialog: merges a local branch or a remote-tracking ref into the
//! current branch.
//!
//! When the dialog opens, `Repo::merge_preview` runs off the main thread; the
//! status area, the primary button and the footer hint are derived from that
//! preview and the selected merge type by [`derive_status`]. The merge itself
//! also runs off the main thread, and its [`MergeOutcome`] decides the toast
//! and whether the shell switches to Changes ([`outcome_notice`]).

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};

use git_core::{Error, Git, MergeOutcome, MergePreview, MergeType, Repo};
use ui::metrics::{FONT_MONO, FS_BASE, FS_SM};
use ui::theme::ActiveTheme;
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Chip, ChipKind, Dialog, DialogFocus,
    DialogHost, Icon, IconName, Select, Spinner, ToastKind,
};

/// Width of the dialog, from the design.
const WIDTH: f32 = 520.;
/// Conflicted paths listed before the "Show N more" toggle.
const VISIBLE_PATHS: usize = 5;

/// What the shell does after the dialog's merge finished (not after Cancel).
/// The `bool` is `true` when the shell should switch to the Changes tab.
type Done = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What the shell hands the dialog when it opens.
pub(crate) struct MergeDialogContext {
    pub root: PathBuf,
    pub git: Result<Arc<Git>, Arc<Error>>,
    /// The branch (`feat/x`) or remote-tracking ref (`origin/feat/x`) to merge.
    pub branch: String,
    /// The checked-out branch the merge goes into.
    pub head: String,
    /// Staged plus unstaged files, for the local-changes note.
    pub dirty: usize,
}

/// The merge types in menu order; the index is the select's value.
const TYPES: [MergeType; 4] = [
    MergeType::FastForwardIfPossible,
    MergeType::NoFastForward,
    MergeType::FastForwardOnly,
    MergeType::Squash,
];

/// Where the conflict preview stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewState {
    Pending,
    /// The preview call itself failed; the reason is git's message.
    Failed(String),
    Ready(MergePreview),
}

/// What the status area shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StatusKind {
    /// Spinner row: "Checking for conflicts…".
    Pending,
    /// Muted error row: the preview could not be computed.
    Unavailable(String),
    /// Ok banner. `commits` is `None` when the count is not known (a squash of
    /// a branch HEAD could fast-forward to).
    Clean { commits: Option<usize> },
    /// Ok banner: HEAD will fast-forward.
    FastForward,
    /// Info banner: a merge commit will be created though HEAD could fast-forward.
    MergeCommitDespiteFastForward,
    /// Info banner: nothing to merge.
    UpToDate,
    /// Warn banner: diverged branches and Fast-forward only.
    CannotFastForward,
    /// Warn banner with the conflicted paths.
    Conflicts { paths: Vec<String> },
}

/// The label of the primary button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrimaryLabel {
    Merge,
    MergeWithConflicts,
    Squash,
}

/// The footer hint shown next to a disabled primary button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HintKind {
    NothingToMerge,
    PickAnotherType,
}

/// The state of the form derived from the preview and the merge type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub kind: StatusKind,
    pub primary: PrimaryLabel,
    pub enabled: bool,
    pub hint: Option<HintKind>,
}

/// Preview x merge type -> what the dialog shows and whether it can run.
pub(crate) fn derive_status(preview: &PreviewState, ty: MergeType) -> Status {
    let squash = ty == MergeType::Squash;
    let ff_only = ty == MergeType::FastForwardOnly;
    let ok = |kind, primary| Status {
        kind,
        primary,
        enabled: true,
        hint: None,
    };
    let plain = if squash {
        PrimaryLabel::Squash
    } else {
        PrimaryLabel::Merge
    };
    let blocked = |kind, hint| Status {
        kind,
        primary: plain,
        enabled: false,
        hint: Some(hint),
    };
    let ready = match preview {
        PreviewState::Pending => return ok(StatusKind::Pending, plain),
        PreviewState::Failed(reason) => {
            return ok(StatusKind::Unavailable(reason.clone()), plain);
        }
        PreviewState::Ready(preview) => preview,
    };
    match ready {
        MergePreview::Unavailable(reason) => ok(StatusKind::Unavailable(reason.clone()), plain),
        MergePreview::UpToDate => blocked(StatusKind::UpToDate, HintKind::NothingToMerge),
        MergePreview::FastForward => match ty {
            MergeType::NoFastForward => ok(StatusKind::MergeCommitDespiteFastForward, plain),
            MergeType::Squash => ok(StatusKind::Clean { commits: None }, plain),
            MergeType::FastForwardIfPossible | MergeType::FastForwardOnly => {
                ok(StatusKind::FastForward, plain)
            }
        },
        MergePreview::Clean { .. } | MergePreview::Conflicts { .. } if ff_only => {
            blocked(StatusKind::CannotFastForward, HintKind::PickAnotherType)
        }
        MergePreview::Clean { commits } => ok(
            StatusKind::Clean {
                commits: Some(*commits),
            },
            plain,
        ),
        MergePreview::Conflicts { paths, .. } => ok(
            StatusKind::Conflicts {
                paths: paths.clone(),
            },
            if squash {
                PrimaryLabel::Squash
            } else {
                PrimaryLabel::MergeWithConflicts
            },
        ),
    }
}

/// The toast a finished merge shows and whether the shell opens Changes.
pub(crate) struct Notice {
    pub kind: ToastKind,
    pub message: String,
    pub show_changes: bool,
}

/// The toast for `outcome` of merging `branch` into `head`.
pub(crate) fn outcome_notice(outcome: &MergeOutcome, branch: &str, head: &str) -> Notice {
    match outcome {
        MergeOutcome::Merged => Notice {
            kind: ToastKind::Ok,
            message: ui::t!("dialogs.merge.toast.merged", branch = branch, head = head).to_string(),
            show_changes: false,
        },
        MergeOutcome::FastForwarded => Notice {
            kind: ToastKind::Ok,
            message: ui::t!(
                "dialogs.merge.toast.fast_forwarded",
                branch = branch,
                head = head
            )
            .to_string(),
            show_changes: false,
        },
        MergeOutcome::Squashed => Notice {
            kind: ToastKind::Ok,
            message: ui::t!("dialogs.merge.toast.squashed", branch = branch).to_string(),
            show_changes: true,
        },
        MergeOutcome::UpToDate => Notice {
            kind: ToastKind::Info,
            message: ui::t!(
                "dialogs.merge.toast.up_to_date",
                branch = branch,
                head = head
            )
            .to_string(),
            show_changes: false,
        },
        MergeOutcome::Conflicts(paths) => Notice {
            kind: ToastKind::Warn,
            message: ui::i18n::t_count("dialogs.merge.toast.conflicts", count(paths.len()), &[])
                .to_string(),
            show_changes: true,
        },
    }
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

pub(crate) struct MergeDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    root: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    branch: String,
    head: String,
    dirty: usize,
    ty: MergeType,
    preview: PreviewState,
    /// The conflicted-path list is expanded past [`VISIBLE_PATHS`].
    show_all_paths: bool,
    running: bool,
    /// git's own message after a failed merge.
    error: Option<String>,
    select_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    on_done: Done,
}

impl MergeDialog {
    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn status(&self) -> Status {
        derive_status(&self.preview, self.ty)
    }

    /// Computes the conflict preview off the main thread.
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.preview = PreviewState::Failed(error.to_string());
                return;
            }
        };
        let root = self.root.clone();
        let branch = self.branch.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::open(git, &root)?.merge_preview(&branch) })
                .await;
            this.update(cx, |dialog, cx| {
                dialog.preview = match result {
                    Ok(preview) => PreviewState::Ready(preview),
                    Err(error) => PreviewState::Failed(error.to_string()),
                };
                dialog.notify_host(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs the merge off the main thread.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running || !self.status().enabled {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.error = Some(error.to_string());
                self.notify_host(cx);
                cx.notify();
                return;
            }
        };
        let root = self.root.clone();
        let branch = self.branch.clone();
        let ty = self.ty;
        self.running = true;
        self.error = None;
        self.notify_host(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::open(git, &root)?.merge(&branch, ty) })
                .await;
            this.update_in(cx, |dialog, window, cx| dialog.finish(result, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        result: Result<MergeOutcome, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        match result {
            Ok(outcome) => {
                let notice = outcome_notice(&outcome, &self.branch, &self.head);
                self.close(window, cx);
                toast::show(notice.kind, notice.message, window, cx);
                let on_done = self.on_done.clone();
                on_done(notice.show_changes, window, cx);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.notify_host(cx);
                cx.notify();
            }
        }
    }
}

/// Opens the dialog and keeps its state alive in the returned entity.
pub(crate) fn open(
    context: MergeDialogContext,
    on_done: impl Fn(bool, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Entity<MergeDialog> {
    let dialog = cx.new(|cx| {
        let mut dialog = MergeDialog {
            layer: None,
            host: None,
            root: context.root,
            git: context.git,
            branch: context.branch,
            head: context.head,
            dirty: context.dirty,
            ty: MergeType::default(),
            preview: PreviewState::Pending,
            show_all_paths: false,
            running: false,
            error: None,
            select_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            on_done: Rc::new(on_done),
        };
        dialog.load_preview(cx);
        dialog
    });
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new(ui::t!("dialogs.merge.title")).icon(IconName::Merge),
    });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
        cx.notify();
    });
    dialog
}

fn type_label(ty: MergeType) -> SharedString {
    match ty {
        MergeType::FastForwardIfPossible => ui::t!("dialogs.merge.type.ff"),
        MergeType::NoFastForward => ui::t!("dialogs.merge.type.no_ff"),
        MergeType::FastForwardOnly => ui::t!("dialogs.merge.type.ff_only"),
        MergeType::Squash => ui::t!("dialogs.merge.type.squash"),
    }
}

fn type_help(ty: MergeType, head: &str) -> SharedString {
    match ty {
        MergeType::FastForwardIfPossible => ui::t!("dialogs.merge.help.ff", head = head),
        MergeType::NoFastForward => ui::t!("dialogs.merge.help.no_ff"),
        MergeType::FastForwardOnly => ui::t!("dialogs.merge.help.ff_only", head = head),
        MergeType::Squash => ui::t!("dialogs.merge.help.squash"),
    }
}

fn primary_text(label: PrimaryLabel) -> SharedString {
    match label {
        PrimaryLabel::Merge => ui::t!("dialogs.merge.primary.merge"),
        PrimaryLabel::MergeWithConflicts => ui::t!("dialogs.merge.primary.with_conflicts"),
        PrimaryLabel::Squash => ui::t!("dialogs.merge.primary.squash"),
    }
}

fn hint_text(hint: HintKind) -> SharedString {
    match hint {
        HintKind::NothingToMerge => ui::t!("dialogs.merge.hint.nothing"),
        HintKind::PickAnotherType => ui::t!("dialogs.merge.hint.pick_type"),
    }
}

fn build(state: &Entity<MergeDialog>, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let status = dialog.status();
    let branch = dialog.branch.as_str();
    let head = dialog.head.as_str();

    let row = |label: SharedString, value: gpui::AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(86.))
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(label),
            )
            .child(value)
    };
    let branch_value = |name: &str| {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .child(
                Icon::new(IconName::Branch)
                    .size(px(13.))
                    .color(colors.muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(FS_BASE)
                    .child(name.to_owned()),
            )
    };

    let select = {
        let state = state.clone();
        Select::new("merge-type")
            .focus_handle(dialog.select_focus.clone())
            .options(TYPES.map(type_label))
            .selected(TYPES.iter().position(|ty| *ty == dialog.ty).unwrap_or(0))
            .disabled(dialog.running)
            .on_change(move |index, _, cx| {
                state.update(cx, |dialog, cx| {
                    dialog.ty = TYPES.get(index).copied().unwrap_or_default();
                    dialog.notify_host(cx);
                    cx.notify();
                });
            })
    };

    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(row(
                    ui::t!("dialogs.merge.field.merge"),
                    branch_value(branch).into_any_element(),
                ))
                .child(row(
                    ui::t!("dialogs.merge.field.into"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(branch_value(head))
                        .child(
                            Chip::new("merge-head-chip", ui::t!("dialogs.merge.head_chip"))
                                .kind(ChipKind::Acc),
                        )
                        .into_any_element(),
                ))
                .child(row(
                    ui::t!("dialogs.merge.field.type"),
                    div().flex_1().min_w_0().child(select).into_any_element(),
                )),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(type_help(dialog.ty, head)),
        );

    body = body.child(status_area(dialog, &status, state, colors));

    if dialog.dirty > 0 {
        body = body.child(
            Banner::new(
                BannerKind::Info,
                ui::i18n::t_count("dialogs.merge.dirty", count(dialog.dirty), &[]),
            )
            .body(ui::t!("dialogs.merge.dirty_note")),
        );
    }
    if let Some(error) = &dialog.error {
        body = body.child(
            Banner::new(BannerKind::Error, ui::t!("dialogs.ref.operation_failed"))
                .body(error.clone()),
        );
    }

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }
    };
    let primary = {
        let state = state.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(cx));
        }
    };
    let on_default = {
        let state = state.clone();
        move |_: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(cx));
        }
    };

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("merge-cancel")
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .disabled(dialog.running)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(
            Button::primary("merge-primary")
                .label(primary_text(status.primary))
                .min_width(px(72.))
                .disabled(!status.enabled || dialog.running)
                .focus_handle(dialog.primary_focus.clone())
                .on_click(primary),
        );

    let hint = if dialog.running {
        Some(ui::t!("dialogs.merge.working"))
    } else {
        status.hint.map(hint_text)
    };

    let mut panel = Dialog::new(ui::t!("dialogs.merge.title"))
        .icon(IconName::Merge)
        .width(px(WIDTH))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(dialog.running)
        .focus_order(vec![
            DialogFocus::control(dialog.select_focus.clone()),
            DialogFocus::control(dialog.cancel_focus.clone()),
            DialogFocus::control(dialog.primary_focus.clone()),
        ])
        .autofocus(dialog.select_focus.clone())
        .default_button(dialog.primary_focus.clone())
        .on_default(on_default);
    if let Some(hint) = hint {
        panel = panel.hint(hint);
    }
    panel
}

/// The conflict-preview area: a muted row, or one banner.
fn status_area(
    dialog: &MergeDialog,
    status: &Status,
    state: &Entity<MergeDialog>,
    colors: ui::ThemeColors,
) -> gpui::AnyElement {
    let branch = dialog.branch.as_str();
    let head = dialog.head.as_str();
    let muted_row = |icon: gpui::AnyElement, text: SharedString| {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(icon)
            .child(div().min_w_0().child(text))
            .into_any_element()
    };
    match &status.kind {
        StatusKind::Pending => muted_row(
            Spinner::new("merge-preview-spinner", px(13.)).into_any_element(),
            ui::t!("dialogs.merge.checking"),
        ),
        StatusKind::Unavailable(reason) => muted_row(
            Icon::new(IconName::Err)
                .size(px(13.))
                .color(colors.muted)
                .into_any_element(),
            ui::t!("dialogs.merge.unavailable", reason = reason),
        ),
        StatusKind::Clean { commits } => {
            let banner = Banner::new(BannerKind::Ok, ui::t!("dialogs.merge.clean.title"));
            match commits {
                Some(n) => banner.body(ui::i18n::t_count(
                    "dialogs.merge.clean.body",
                    count(*n),
                    &[("branch", &branch)],
                )),
                None => banner,
            }
            .into_any_element()
        }
        StatusKind::FastForward => Banner::new(
            BannerKind::Ok,
            ui::t!("dialogs.merge.ff.title", head = head, branch = branch),
        )
        .body(ui::t!("dialogs.merge.ff.body"))
        .into_any_element(),
        StatusKind::MergeCommitDespiteFastForward => {
            Banner::new(BannerKind::Info, ui::t!("dialogs.merge.commit.title"))
                .body(ui::t!(
                    "dialogs.merge.commit.body",
                    head = head,
                    branch = branch
                ))
                .into_any_element()
        }
        StatusKind::UpToDate => {
            Banner::new(BannerKind::Info, ui::t!("dialogs.merge.uptodate.title"))
                .body(ui::t!(
                    "dialogs.merge.uptodate.body",
                    head = head,
                    branch = branch
                ))
                .into_any_element()
        }
        StatusKind::CannotFastForward => Banner::new(
            BannerKind::Warn,
            ui::t!(
                "dialogs.merge.cannot_ff.title",
                head = head,
                branch = branch
            ),
        )
        .body(ui::t!("dialogs.merge.cannot_ff.body"))
        .into_any_element(),
        StatusKind::Conflicts { paths } => {
            let shown = if dialog.show_all_paths {
                paths.len()
            } else {
                paths.len().min(VISIBLE_PATHS)
            };
            let mut list = div().flex().flex_col().gap(px(2.));
            for path in &paths[..shown] {
                list = list.child(
                    div()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .child(path.clone()),
                );
            }
            if paths.len() > VISIBLE_PATHS {
                let label = if dialog.show_all_paths {
                    ui::t!("dialogs.merge.show_less")
                } else {
                    ui::t!("dialogs.merge.show_more", n = paths.len() - VISIBLE_PATHS)
                };
                let state = state.clone();
                list = list.child(
                    div()
                        .id("merge-paths-toggle")
                        .cursor_pointer()
                        .text_size(FS_SM)
                        .text_color(colors.accent)
                        .child(label)
                        .on_click(move |_, _, cx| {
                            state.update(cx, |dialog, cx| {
                                dialog.show_all_paths = !dialog.show_all_paths;
                                dialog.notify_host(cx);
                                cx.notify();
                            });
                        }),
                );
            }
            // The list lives inside the banner's text column, as in the design.
            Banner::new(
                BannerKind::Warn,
                ui::i18n::t_count("dialogs.merge.conflicts.title", count(paths.len()), &[]),
            )
            .body(ui::t!("dialogs.merge.conflicts.body"))
            .action(list)
            .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(preview: MergePreview) -> PreviewState {
        PreviewState::Ready(preview)
    }

    fn conflicts(n: usize) -> MergePreview {
        MergePreview::Conflicts {
            commits: 2,
            paths: (0..n).map(|i| format!("src/f{i}.rs")).collect(),
        }
    }

    const FF: MergeType = MergeType::FastForwardIfPossible;
    const NO_FF: MergeType = MergeType::NoFastForward;
    const FF_ONLY: MergeType = MergeType::FastForwardOnly;
    const SQUASH: MergeType = MergeType::Squash;

    #[test]
    fn pending_is_a_spinner_with_the_primary_enabled() {
        for ty in TYPES {
            let status = derive_status(&PreviewState::Pending, ty);
            assert_eq!(status.kind, StatusKind::Pending);
            assert!(status.enabled);
            assert_eq!(status.hint, None);
        }
        assert_eq!(
            derive_status(&PreviewState::Pending, SQUASH).primary,
            PrimaryLabel::Squash
        );
        assert_eq!(
            derive_status(&PreviewState::Pending, FF).primary,
            PrimaryLabel::Merge
        );
    }

    #[test]
    fn a_failed_or_unavailable_preview_keeps_the_primary_enabled() {
        let failed = derive_status(&PreviewState::Failed("boom".into()), FF);
        assert_eq!(failed.kind, StatusKind::Unavailable("boom".into()));
        assert!(failed.enabled);
        let unavailable =
            derive_status(&ready(MergePreview::Unavailable("old git".into())), FF_ONLY);
        assert_eq!(unavailable.kind, StatusKind::Unavailable("old git".into()));
        assert!(unavailable.enabled);
    }

    #[test]
    fn a_clean_merge_reports_the_commit_count() {
        for ty in [FF, NO_FF, SQUASH] {
            let status = derive_status(&ready(MergePreview::Clean { commits: 3 }), ty);
            assert_eq!(status.kind, StatusKind::Clean { commits: Some(3) });
            assert!(status.enabled);
            assert_eq!(status.hint, None);
        }
        assert_eq!(
            derive_status(&ready(MergePreview::Clean { commits: 3 }), SQUASH).primary,
            PrimaryLabel::Squash
        );
    }

    #[test]
    fn a_fast_forward_depends_on_the_type() {
        for ty in [FF, FF_ONLY] {
            let status = derive_status(&ready(MergePreview::FastForward), ty);
            assert_eq!(status.kind, StatusKind::FastForward);
            assert!(status.enabled);
        }
        let no_ff = derive_status(&ready(MergePreview::FastForward), NO_FF);
        assert_eq!(no_ff.kind, StatusKind::MergeCommitDespiteFastForward);
        assert!(no_ff.enabled);
        let squash = derive_status(&ready(MergePreview::FastForward), SQUASH);
        assert_eq!(squash.kind, StatusKind::Clean { commits: None });
        assert!(squash.enabled);
        assert_eq!(squash.primary, PrimaryLabel::Squash);
    }

    #[test]
    fn up_to_date_blocks_every_type() {
        for ty in TYPES {
            let status = derive_status(&ready(MergePreview::UpToDate), ty);
            assert_eq!(status.kind, StatusKind::UpToDate);
            assert!(!status.enabled);
            assert_eq!(status.hint, Some(HintKind::NothingToMerge));
        }
    }

    #[test]
    fn diverged_branches_block_fast_forward_only() {
        for preview in [MergePreview::Clean { commits: 1 }, conflicts(2)] {
            let status = derive_status(&ready(preview), FF_ONLY);
            assert_eq!(status.kind, StatusKind::CannotFastForward);
            assert!(!status.enabled);
            assert_eq!(status.hint, Some(HintKind::PickAnotherType));
        }
    }

    #[test]
    fn conflicts_relabel_the_primary_except_for_a_squash() {
        for ty in [FF, NO_FF] {
            let status = derive_status(&ready(conflicts(2)), ty);
            assert_eq!(
                status.kind,
                StatusKind::Conflicts {
                    paths: vec!["src/f0.rs".into(), "src/f1.rs".into()]
                }
            );
            assert!(status.enabled);
            assert_eq!(status.primary, PrimaryLabel::MergeWithConflicts);
        }
        let squash = derive_status(&ready(conflicts(2)), SQUASH);
        assert!(squash.enabled);
        assert_eq!(squash.primary, PrimaryLabel::Squash);
    }

    #[test]
    fn outcomes_pick_the_toast_and_the_changes_switch() {
        let merged = outcome_notice(&MergeOutcome::Merged, "feat/x", "main");
        assert_eq!(merged.kind, ToastKind::Ok);
        assert_eq!(merged.message, "Merged feat/x into main");
        assert!(!merged.show_changes);

        let ff = outcome_notice(&MergeOutcome::FastForwarded, "feat/x", "main");
        assert_eq!(ff.message, "Fast-forwarded main to feat/x");
        assert!(!ff.show_changes);

        let squashed = outcome_notice(&MergeOutcome::Squashed, "feat/x", "main");
        assert_eq!(
            squashed.message,
            "Squashed feat/x into the index — commit it from Changes"
        );
        assert!(squashed.show_changes);

        let up_to_date = outcome_notice(&MergeOutcome::UpToDate, "feat/x", "main");
        assert_eq!(up_to_date.kind, ToastKind::Info);
        assert_eq!(up_to_date.message, "main is already up to date with feat/x");

        let one = outcome_notice(&MergeOutcome::Conflicts(vec!["a".into()]), "feat/x", "main");
        assert_eq!(one.kind, ToastKind::Warn);
        assert_eq!(one.message, "Merge stopped: 1 file has conflicts");
        assert!(one.show_changes);
        let two = outcome_notice(
            &MergeOutcome::Conflicts(vec!["a".into(), "b".into()]),
            "feat/x",
            "main",
        );
        assert_eq!(two.message, "Merge stopped: 2 files have conflicts");
    }
}
