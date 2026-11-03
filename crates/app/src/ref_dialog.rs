//! The stage 7 dialogs: create branch/tag here, rename branch/tag, add or edit
//! a remote, reset, and the destructive confirmations (delete branch, delete
//! tag, remove remote, drop stash, cherry-pick, revert).
//!
//! One [`RefDialog`] entity drives every kind: the kind carries the data, the
//! entity carries the inputs and the run state, and [`build`] renders the
//! panel from them. Operations run the real git in the background; on success
//! the dialog closes, a toast reports it and `on_done` reloads the shell.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, FocusHandle, IntoElement,
    ParentElement, SharedString, Styled, Subscription, Window,
};

use git_core::network::FetchOptions;
use git_core::{Error, Git, Oid, Ref, RefKind, Repo, ResetMode};
use ui::metrics::{FONT_MONO, FS_BASE, FS_SM, R_MD};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    dialog, overlay, toast, Banner, BannerKind, Button, Checkbox, Dialog, DialogFocus, DialogHost,
    Icon, IconName, RefBadge, SegmentItem, Segmented, TextArea, TextAreaEvent, TextInput,
    TextInputEvent, ToastKind,
};

use crate::format::relative_time;
use crate::history_view::unix_now;
use crate::network_ops::Job;
use crate::ref_ops::{
    ref_name_error, remote_name_error, remote_url_kind, CardRefKind, CommitCard, RemoteUrlKind,
};

/// The width every remote/destructive dialog uses when the design gives none.
const CONFIRM_WIDTH: f32 = 480.;

/// The network action a dialog asks the shell to start after its local git call
/// succeeded, so it gets the toolbar's progress, prompts and toasts.
pub(crate) type NetworkAction = Rc<dyn Fn(Job, &mut Window, &mut App)>;

/// Shared context the shell passes to a dialog when it opens.
pub(crate) struct RefDialogContext {
    pub root: PathBuf,
    pub git: Result<Arc<Git>, Arc<Error>>,
    pub refs: Vec<Ref>,
    pub remotes: Vec<String>,
    /// `git_core::network::default_remote`'s answer, for the tag push.
    pub default_remote: Option<String>,
    /// Starts a network job in the repository view behind the dialog.
    pub network: Option<NetworkAction>,
    /// `user.signingkey`, when configured; the tag dialog offers signing then.
    pub signing_key: Option<String>,
    /// HEAD's commit, for the "starts at the current commit" note.
    pub head_oid: Option<Oid>,
    /// The checked-out branch, or `HEAD` when detached.
    pub branch: String,
    /// Staged plus unstaged files, for the branch dialog's local-changes note.
    pub dirty: usize,
}

/// The remote being edited, so the dialog knows what actually changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEdit {
    pub name: String,
    pub url: String,
}

/// What a dialog is for. Everything it needs to draw and run is here; the
/// inputs and the run state live on [`RefDialog`].
#[derive(Clone, Debug)]
pub(crate) enum RefDialogKind {
    CreateBranch {
        card: CommitCard,
    },
    CreateTag {
        card: CommitCard,
    },
    RenameBranch {
        old: String,
    },
    RenameTag {
        old: String,
    },
    Remote {
        editing: Option<RemoteEdit>,
    },
    Reset {
        card: CommitCard,
    },
    DeleteBranch {
        name: String,
        /// `true` after the safe delete refused because the branch is unmerged.
        force: bool,
    },
    DeleteTag {
        name: String,
    },
    RemoveRemote {
        name: String,
    },
    DropStash {
        index: usize,
        label: String,
    },
    CherryPick {
        card: CommitCard,
    },
    Revert {
        card: CommitCard,
    },
    /// Moves a worktree's folder. `from` is its current path.
    RenameWorktree {
        from: PathBuf,
    },
}

type DialogAction = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// What the shell does after a dialog's operation succeeded.
type Done = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct RefDialog {
    layer: Option<overlay::LayerId>,
    host: Option<Entity<DialogHost>>,
    root: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    kind: RefDialogKind,
    name: Option<Entity<TextInput>>,
    message: Option<Entity<TextArea>>,
    url: Option<Entity<TextInput>>,
    /// Local branch names / tag names, so validation can say "already exists".
    taken: Vec<String>,
    remote_names: Vec<String>,
    latest_tag: Option<String>,
    checkout: bool,
    annotated: bool,
    sign: bool,
    /// Tag dialog: push the new tag once it exists.
    push_tag_after_create: bool,
    /// The default remote the tag is pushed to, and `None` when there is none.
    push_remote: Option<String>,
    /// Add-remote dialog: fetch the new remote once it exists.
    fetch_after_add: bool,
    network: Option<NetworkAction>,
    signing_key: Option<String>,
    reset_mode: ResetMode,
    branch: String,
    head_oid: Option<Oid>,
    dirty: usize,
    running: bool,
    /// git's own message after a failed operation, shown in the dialog.
    error: Option<String>,
    /// Set once an input changed, so errors do not colour an untouched field.
    touched: bool,
    name_focus: FocusHandle,
    message_focus: FocusHandle,
    url_focus: FocusHandle,
    checkout_focus: FocusHandle,
    push_focus: FocusHandle,
    fetch_focus: FocusHandle,
    sign_focus: FocusHandle,
    cancel_focus: FocusHandle,
    primary_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    on_done: Done,
}

impl RefDialog {
    fn new(
        kind: RefDialogKind,
        context: RefDialogContext,
        on_done: Done,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_name = match &kind {
            RefDialogKind::RenameWorktree { from } => from
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            RefDialogKind::RenameBranch { old } | RefDialogKind::RenameTag { old } => old.clone(),
            RefDialogKind::Remote {
                editing: Some(edit),
            } => edit.name.clone(),
            _ => String::new(),
        };
        let initial_url = match &kind {
            RefDialogKind::Remote {
                editing: Some(edit),
            } => edit.url.clone(),
            _ => String::new(),
        };
        let taken = match &kind {
            RefDialogKind::CreateBranch { .. } | RefDialogKind::RenameBranch { .. } => context
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::LocalBranch && r.name != initial_name)
                .map(|r| r.name.clone())
                .collect(),
            RefDialogKind::CreateTag { .. } | RefDialogKind::RenameTag { .. } => context
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::Tag && r.name != initial_name)
                .map(|r| r.name.clone())
                .collect(),
            RefDialogKind::Remote { .. } => context
                .remotes
                .iter()
                .filter(|name| **name != initial_name)
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let latest_tag = context
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::Tag)
            .map(|r| r.name.clone())
            .max();

        let mut subscriptions = Vec::new();
        let name = needs_name(&kind).then(|| {
            let placeholder = match kind {
                RefDialogKind::CreateBranch { .. } => "feat/my-change",
                RefDialogKind::CreateTag { .. } => "v0.3.1",
                RefDialogKind::Remote { .. } => "upstream",
                RefDialogKind::RenameWorktree { .. } => "new-folder-name",
                _ => "",
            };
            let entity = cx.new(|cx| {
                let mut input = TextInput::new(window, cx);
                input.set_font_family(FONT_MONO, cx);
                input.set_placeholder(placeholder, cx);
                if !initial_name.is_empty() {
                    input.set_text(initial_name.clone(), cx);
                }
                input
            });
            subscriptions.push(cx.subscribe(&entity, |this, _, _: &TextInputEvent, cx| {
                this.touched = true;
                this.notify_host(cx);
                cx.notify();
            }));
            entity
        });
        let message = matches!(kind, RefDialogKind::CreateTag { .. }).then(|| {
            let entity = cx.new(|cx| {
                let mut area = TextArea::new(window, cx);
                area.set_placeholder(ui::t!("dialogs.ref.tag_message_placeholder"), cx);
                area
            });
            subscriptions.push(cx.subscribe(&entity, |this, _, _: &TextAreaEvent, cx| {
                this.touched = true;
                this.notify_host(cx);
                cx.notify();
            }));
            entity
        });
        let url = matches!(kind, RefDialogKind::Remote { .. }).then(|| {
            let entity = cx.new(|cx| {
                let mut input = TextInput::new(window, cx);
                input.set_font_family(FONT_MONO, cx);
                input.set_placeholder("git@github.com:user/repo.git", cx);
                if !initial_url.is_empty() {
                    input.set_text(initial_url.clone(), cx);
                }
                input
            });
            subscriptions.push(cx.subscribe(&entity, |this, _, _: &TextInputEvent, cx| {
                this.touched = true;
                this.notify_host(cx);
                cx.notify();
            }));
            entity
        });

        let name_focus = name
            .as_ref()
            .map(|input| input.read(cx).focus_handle().clone())
            .unwrap_or_else(|| cx.focus_handle());
        let message_focus = message
            .as_ref()
            .map(|area| area.read(cx).focus_handle().clone())
            .unwrap_or_else(|| cx.focus_handle());
        let url_focus = url
            .as_ref()
            .map(|input| input.read(cx).focus_handle().clone())
            .unwrap_or_else(|| cx.focus_handle());

        let annotated = matches!(kind, RefDialogKind::CreateTag { .. });
        let checkout = matches!(kind, RefDialogKind::CreateBranch { .. });
        let push_remote = if annotated {
            context.default_remote
        } else {
            None
        };
        let network = context.network;
        let signing_key = context.signing_key;
        let sign = signing_key.is_some();
        Self {
            layer: None,
            host: None,
            root: context.root,
            git: context.git,
            kind,
            name,
            message,
            url,
            taken,
            remote_names: context.remotes,
            latest_tag,
            checkout,
            annotated,
            sign,
            push_tag_after_create: false,
            push_remote,
            fetch_after_add: true,
            network,
            signing_key,
            reset_mode: ResetMode::Mixed,
            branch: context.branch,
            head_oid: context.head_oid,
            dirty: context.dirty,
            running: false,
            error: None,
            touched: false,
            name_focus,
            message_focus,
            url_focus,
            checkout_focus: cx.focus_handle().tab_stop(true),
            push_focus: cx.focus_handle().tab_stop(true),
            fetch_focus: cx.focus_handle().tab_stop(true),
            sign_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            primary_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
            on_done,
        }
    }

    fn name_text(&self, cx: &App) -> String {
        self.name
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    fn message_text(&self, cx: &App) -> String {
        self.message
            .as_ref()
            .map(|area| area.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    fn url_text(&self, cx: &App) -> String {
        self.url
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    /// The name problem for the current input, if any.
    fn name_error(&self, cx: &App) -> Option<String> {
        let name = self.name_text(cx);
        match &self.kind {
            RefDialogKind::Remote { .. } => remote_name_error(&name, &self.taken),
            RefDialogKind::RenameWorktree { .. } => worktree_folder_error(&name),
            _ => ref_name_error(&name, &self.taken),
        }
    }

    /// Whether "renaming" to the current name: git would refuse that.
    fn rename_is_unchanged(&self, cx: &App) -> bool {
        let name = self.name_text(cx);
        match &self.kind {
            RefDialogKind::RenameBranch { old } | RefDialogKind::RenameTag { old } => name == *old,
            RefDialogKind::RenameWorktree { from } => from
                .file_name()
                .is_some_and(|current| current == name.as_str()),
            _ => false,
        }
    }

    /// Everything that gates the primary button and the footer hint.
    fn validity(&self, cx: &App) -> Validity {
        let name = self.name_text(cx);
        let name_error = self.name_error(cx);
        let url = self.url_text(cx);
        let url_kind = remote_url_kind(&url);
        let message = self.message_text(cx);
        let rename_unchanged = self.rename_is_unchanged(cx);

        let (enabled, hint) = match &self.kind {
            RefDialogKind::CreateBranch { .. }
            | RefDialogKind::RenameBranch { .. }
            | RefDialogKind::RenameWorktree { .. } => (
                !name.is_empty() && name_error.is_none() && !rename_unchanged,
                if name.is_empty() {
                    Some(
                        if matches!(self.kind, RefDialogKind::RenameWorktree { .. }) {
                            ui::t!("dialogs.ref.hint.folder_name")
                        } else {
                            ui::t!("dialogs.ref.hint.branch_name")
                        },
                    )
                } else if name_error.is_some() {
                    Some(ui::t!("dialogs.ref.hint.fix_name"))
                } else if rename_unchanged {
                    Some(ui::t!("dialogs.ref.hint.new_name"))
                } else {
                    None
                },
            ),
            RefDialogKind::CreateTag { .. } | RefDialogKind::RenameTag { .. } => (
                !(name.is_empty()
                    || name_error.is_some()
                    || rename_unchanged
                    || self.annotated && message.is_empty()),
                if name.is_empty() {
                    Some(ui::t!("dialogs.ref.hint.tag_name"))
                } else if name_error.is_some() {
                    Some(ui::t!("dialogs.ref.hint.fix_name"))
                } else if rename_unchanged {
                    Some(ui::t!("dialogs.ref.hint.new_name"))
                } else if self.annotated && message.is_empty() {
                    Some(ui::t!("dialogs.ref.hint.add_message"))
                } else {
                    None
                },
            ),
            RefDialogKind::Remote { .. } => (
                self.remote_valid(&name, name_error.as_ref(), url_kind),
                if name.is_empty() {
                    Some(ui::t!("dialogs.ref.hint.remote_name"))
                } else if name_error.is_some() {
                    Some(ui::t!("dialogs.ref.hint.other_name"))
                } else if url.is_empty() {
                    Some(ui::t!("dialogs.ref.hint.remote_address"))
                } else if url_kind == RemoteUrlKind::Bad {
                    Some(ui::t!("dialogs.ref.hint.fix_address"))
                } else {
                    None
                },
            ),
            _ => (true, None),
        };
        Validity {
            name_error,
            url,
            url_kind,
            message,
            enabled: enabled && !self.running,
            hint,
        }
    }

    fn remote_valid(
        &self,
        name: &str,
        name_error: Option<&String>,
        url_kind: RemoteUrlKind,
    ) -> bool {
        !name.is_empty() && name_error.is_none() && url_kind != RemoteUrlKind::Bad
    }

    fn notify_host(&self, cx: &mut App) {
        if let Some(host) = &self.host {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    /// The tests type into the name field without reaching a window.
    #[cfg(test)]
    pub(crate) fn set_name_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(name) = &self.name {
            name.update(cx, |input, cx| input.set_text(text, cx));
        }
        self.touched = true;
    }

    /// The tests press the primary button.
    #[cfg(test)]
    pub(crate) fn submit_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start(window, cx);
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    /// Runs the operation the dialog is for, off the UI thread.
    fn start(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running || !self.validity(cx).enabled {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let Some(operation) = self.operation(cx) else {
            return;
        };
        let root = self.root.clone();
        self.running = true;
        self.error = None;
        self.notify_host(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &root)?;
                    operation(&repo)
                })
                .await;
            this.update_in(cx, |dialog, window, cx| dialog.finish(result, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// The git call and its success message, built from the current inputs.
    fn operation(&self, cx: &App) -> Option<Operation> {
        let name = self.name_text(cx);
        let message = self.message_text(cx);
        let url = self.url_text(cx);
        let checkout = self.checkout;
        let annotated = self.annotated;
        let sign = self.sign;
        let mode = self.reset_mode;
        let branch = self.branch.clone();
        let kind = self.kind.clone();
        Some(match kind {
            RefDialogKind::CreateBranch { card } => Box::new(move |repo: &Repo| {
                repo.create_branch(&name, card.oid, checkout)?;
                Ok(if checkout {
                    ui::t!("dialogs.ref.done.created_and_switched", name = name).to_string()
                } else {
                    ui::t!("dialogs.ref.done.created", name = name).to_string()
                })
            }),
            RefDialogKind::CreateTag { card } => Box::new(move |repo: &Repo| {
                let message = annotated.then_some(message.as_str());
                repo.create_tag(&name, card.oid, message, sign)?;
                Ok(ui::t!("dialogs.ref.done.created_tag", name = name).to_string())
            }),
            RefDialogKind::RenameBranch { old } => Box::new(move |repo: &Repo| {
                repo.rename_branch(&old, &name)?;
                Ok(ui::t!("dialogs.ref.done.renamed", old = old, name = name).to_string())
            }),
            RefDialogKind::RenameWorktree { from } => Box::new(move |repo: &Repo| {
                let to = worktree_destination(&from, &name);
                repo.move_worktree(&from, &to)?;
                Ok(ui::t!("dialogs.ref.done.moved_worktree", path = to.display()).to_string())
            }),
            RefDialogKind::RenameTag { old } => Box::new(move |repo: &Repo| {
                repo.rename_tag(&old, &name)?;
                Ok(ui::t!("dialogs.ref.done.renamed_tag", old = old, name = name).to_string())
            }),
            RefDialogKind::Remote { editing } => Box::new(move |repo: &Repo| match editing {
                None => {
                    repo.add_remote(&name, &url)?;
                    Ok(ui::t!("dialogs.ref.done.added_remote", name = name).to_string())
                }
                Some(edit) => {
                    if edit.name != name {
                        repo.rename_remote(&edit.name, &name)?;
                    }
                    if edit.url != url {
                        repo.set_remote_url(&name, &url)?;
                    }
                    Ok(ui::t!("dialogs.ref.done.updated_remote", name = name).to_string())
                }
            }),
            RefDialogKind::Reset { card } => Box::new(move |repo: &Repo| {
                repo.reset(card.oid, mode)?;
                Ok(ui::t!(
                    "dialogs.ref.done.reset",
                    branch = branch,
                    commit = card.short
                )
                .to_string())
            }),
            RefDialogKind::DeleteBranch { name, force } => Box::new(move |repo: &Repo| {
                repo.delete_branch(&name, force)?;
                Ok(ui::t!("dialogs.ref.done.deleted_branch", name = name).to_string())
            }),
            RefDialogKind::DeleteTag { name } => Box::new(move |repo: &Repo| {
                repo.delete_tag(&name)?;
                Ok(ui::t!("dialogs.ref.done.deleted_tag", name = name).to_string())
            }),
            RefDialogKind::RemoveRemote { name } => Box::new(move |repo: &Repo| {
                repo.remove_remote(&name)?;
                Ok(ui::t!("dialogs.ref.done.removed_remote", name = name).to_string())
            }),
            RefDialogKind::DropStash { index, .. } => Box::new(move |repo: &Repo| {
                repo.stash_drop(index)?;
                Ok(ui::t!("dialogs.ref.done.dropped_stash", index = index).to_string())
            }),
            RefDialogKind::CherryPick { card } => Box::new(move |repo: &Repo| {
                repo.cherry_pick(card.oid)?;
                Ok(ui::t!("dialogs.ref.done.cherry_picked", commit = card.short).to_string())
            }),
            RefDialogKind::Revert { card } => Box::new(move |repo: &Repo| {
                repo.revert(card.oid)?;
                Ok(ui::t!("dialogs.ref.done.reverted", commit = card.short).to_string())
            }),
        })
    }

    /// The network job the dialog starts once its local git call has landed.
    fn follow_up(&self, cx: &App) -> Option<Job> {
        match &self.kind {
            RefDialogKind::CreateTag { .. } if self.push_tag_after_create => {
                let remote = self.push_remote.clone()?;
                let tag = self.name_text(cx);
                (!tag.is_empty()).then_some(Job::PushTag { remote, tag })
            }
            RefDialogKind::Remote { editing: None } if self.fetch_after_add => {
                let remote = self.name_text(cx);
                (!remote.is_empty()).then_some(Job::Fetch(FetchOptions {
                    remote: Some(remote),
                    tags: true,
                    prune: false,
                }))
            }
            _ => None,
        }
    }

    fn finish(
        &mut self,
        result: std::result::Result<String, Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        match result {
            Ok(notice) => {
                let follow_up = self.follow_up(cx);
                self.close(window, cx);
                toast::show(ToastKind::Ok, notice, window, cx);
                let on_done = self.on_done.clone();
                on_done(window, cx);
                if let (Some(job), Some(network)) = (follow_up, self.network.clone()) {
                    network(job, window, cx);
                }
            }
            Err(error) => {
                let message = error.to_string();
                // The safe delete explains itself: offer the forced variant in
                // place of an error the user cannot act on.
                if let RefDialogKind::DeleteBranch { name, force: false } = &self.kind {
                    if message.contains("not fully merged") {
                        let name = name.clone();
                        self.kind = RefDialogKind::DeleteBranch { name, force: true };
                        self.error = None;
                        self.notify_host(cx);
                        cx.notify();
                        return;
                    }
                }
                self.error = Some(message);
                self.notify_host(cx);
                cx.notify();
            }
        }
    }
}

/// A git operation and the toast it reports on success.
type Operation = Box<dyn FnOnce(&Repo) -> git_core::Result<String> + Send>;

fn needs_name(kind: &RefDialogKind) -> bool {
    matches!(
        kind,
        RefDialogKind::CreateBranch { .. }
            | RefDialogKind::CreateTag { .. }
            | RefDialogKind::RenameBranch { .. }
            | RefDialogKind::RenameTag { .. }
            | RefDialogKind::RenameWorktree { .. }
            | RefDialogKind::Remote { .. }
    )
}

/// The computed state of the form, shared by the builder and `start`.
struct Validity {
    name_error: Option<String>,
    url: String,
    url_kind: RemoteUrlKind,
    message: String,
    enabled: bool,
    hint: Option<SharedString>,
}

/// Opens a dialog and keeps its state in the shell.
pub(crate) fn open(
    kind: RefDialogKind,
    context: RefDialogContext,
    on_done: Done,
    window: &mut Window,
    cx: &mut Context<crate::repo_view::RepositoryView>,
) -> Entity<RefDialog> {
    let dialog = cx.new(|cx| RefDialog::new(kind, context, on_done, window, cx));
    let state = dialog.downgrade();
    let (layer, host) = dialog::open_with_view(window, cx, move |_, cx| match state.upgrade() {
        Some(state) => build(&state, cx),
        None => Dialog::new(ui::t!("dialogs.ref.title_fallback")).icon(IconName::Git),
    });
    dialog.update(cx, |dialog, cx| {
        dialog.layer = Some(layer);
        dialog.host = Some(host);
        cx.notify();
    });
    dialog
}

fn build(state: &Entity<RefDialog>, cx: &App) -> Dialog {
    let dialog = state.read(cx);
    let colors = cx.theme().colors;
    let valid = dialog.validity(cx);
    let (title, icon, width) = match &dialog.kind {
        RefDialogKind::CreateBranch { .. } => (
            ui::t!("dialogs.ref.title.create_branch"),
            IconName::Branch,
            480.,
        ),
        RefDialogKind::CreateTag { .. } => {
            (ui::t!("dialogs.ref.title.create_tag"), IconName::Tag, 500.)
        }
        RefDialogKind::RenameBranch { .. } => (
            ui::t!("dialogs.ref.title.rename_branch"),
            IconName::Edit,
            480.,
        ),
        RefDialogKind::RenameWorktree { .. } => (
            ui::t!("dialogs.ref.title.rename_worktree"),
            IconName::Edit,
            480.,
        ),
        RefDialogKind::RenameTag { .. } => {
            (ui::t!("dialogs.ref.title.rename_tag"), IconName::Edit, 480.)
        }
        RefDialogKind::Remote { editing: None } => (
            ui::t!("dialogs.ref.title.add_remote"),
            IconName::Cloud,
            500.,
        ),
        RefDialogKind::Remote { editing: Some(_) } => (
            ui::t!("dialogs.ref.title.edit_remote"),
            IconName::Cloud,
            500.,
        ),
        RefDialogKind::Reset { .. } => (ui::t!("dialogs.ref.title.reset"), IconName::Undo, 500.),
        RefDialogKind::DeleteBranch { .. } => (
            ui::t!("dialogs.ref.title.delete_branch"),
            IconName::Trash,
            CONFIRM_WIDTH,
        ),
        RefDialogKind::DeleteTag { .. } => (
            ui::t!("dialogs.ref.title.delete_tag"),
            IconName::Trash,
            CONFIRM_WIDTH,
        ),
        RefDialogKind::RemoveRemote { .. } => (
            ui::t!("dialogs.ref.title.remove_remote"),
            IconName::Trash,
            CONFIRM_WIDTH,
        ),
        RefDialogKind::DropStash { .. } => (
            ui::t!("dialogs.ref.title.drop_stash"),
            IconName::Trash,
            CONFIRM_WIDTH,
        ),
        RefDialogKind::CherryPick { .. } => (
            ui::t!("dialogs.ref.title.cherry_pick"),
            IconName::Undo,
            CONFIRM_WIDTH,
        ),
        RefDialogKind::Revert { .. } => (
            ui::t!("dialogs.ref.title.revert"),
            IconName::Undo,
            CONFIRM_WIDTH,
        ),
    };

    let mut body = div().flex().flex_col().gap(px(14.));
    body = match &dialog.kind {
        RefDialogKind::CreateBranch { card } => {
            let mut body = body.child(commit_card(card, colors)).child(name_field(
                ui::t!("dialogs.ref.field.branch_name"),
                dialog.name.clone(),
                valid.name_error.as_deref().filter(|_| dialog.touched),
                Some(&*ui::t!(
                    "dialogs.ref.help.branch_start",
                    commit = card.short
                )),
                colors,
            ));
            let toggle = {
                let state = state.clone();
                move |next: bool, _: &mut Window, cx: &mut App| {
                    state.update(cx, |dialog, cx| {
                        dialog.checkout = next;
                        dialog.notify_host(cx);
                        cx.notify();
                    });
                }
            };
            body = body.child(
                Checkbox::new("ref-branch-checkout")
                    .label(ui::t!("dialogs.ref.switch_after_create"))
                    .checked(dialog.checkout)
                    .focus_handle(dialog.checkout_focus.clone())
                    .on_toggle(toggle),
            );
            if dialog.checkout {
                body = body.child(branch_note(card, dialog));
            }
            body
        }
        RefDialogKind::CreateTag { card } => {
            let mut body = body
                .child(commit_card(card, colors))
                .child(name_field(
                    ui::t!("dialogs.ref.field.tag_name"),
                    dialog.name.clone(),
                    valid.name_error.as_deref().filter(|_| dialog.touched),
                    dialog
                        .latest_tag
                        .as_ref()
                        .map(|tag| ui::t!("dialogs.ref.help.latest_tag", tag = tag).to_string())
                        .as_deref(),
                    colors,
                ))
                .child(tag_type(dialog, state, colors));
            if dialog.annotated {
                body = body.child(message_field(
                    dialog.message.clone(),
                    dialog
                        .touched
                        .then_some(valid.message.is_empty())
                        .filter(|empty| *empty)
                        .map(|_| ui::t!("dialogs.ref.error.tag_message"))
                        .as_deref(),
                    colors,
                ));
                if let Some(key) = &dialog.signing_key {
                    let toggle = {
                        let state = state.clone();
                        move |next: bool, _: &mut Window, cx: &mut App| {
                            state.update(cx, |dialog, cx| {
                                dialog.sign = next;
                                dialog.notify_host(cx);
                                cx.notify();
                            });
                        }
                    };
                    body = body.child(
                        Checkbox::new("ref-tag-sign")
                            .label(ui::t!("dialogs.ref.sign_tag", key = key))
                            .checked(dialog.sign)
                            .focus_handle(dialog.sign_focus.clone())
                            .on_toggle(toggle),
                    );
                }
            }
            if let Some(remote) = &dialog.push_remote {
                let toggle = {
                    let state = state.clone();
                    move |next: bool, _: &mut Window, cx: &mut App| {
                        state.update(cx, |dialog, cx| {
                            dialog.push_tag_after_create = next;
                            dialog.notify_host(cx);
                            cx.notify();
                        });
                    }
                };
                body = body.child(
                    Checkbox::new("ref-tag-push")
                        .label(ui::t!("dialogs.ref.push_tag", remote = remote))
                        .checked(dialog.push_tag_after_create)
                        .focus_handle(dialog.push_focus.clone())
                        .on_toggle(toggle),
                );
                if dialog.push_tag_after_create {
                    body = body.child(
                        Banner::new(BannerKind::Warn, ui::t!("dialogs.ref.pushed_tags_public"))
                            .body(ui::t!("dialogs.ref.pushed_tags_public_body")),
                    );
                }
            }
            body
        }
        RefDialogKind::RenameWorktree { from } => body.child(name_field(
            ui::t!("dialogs.ref.field.new_folder_name"),
            dialog.name.clone(),
            valid.name_error.as_deref().filter(|_| dialog.touched),
            Some(&*ui::t!(
                "dialogs.ref.help.move_worktree",
                path = from.display()
            )),
            colors,
        )),
        RefDialogKind::RenameBranch { old } | RefDialogKind::RenameTag { old } => {
            body.child(name_field(
                if matches!(dialog.kind, RefDialogKind::RenameBranch { .. }) {
                    ui::t!("dialogs.ref.field.new_branch_name")
                } else {
                    ui::t!("dialogs.ref.field.new_tag_name")
                },
                dialog.name.clone(),
                valid.name_error.as_deref().filter(|_| dialog.touched),
                Some(&*ui::t!("dialogs.ref.help.renames", old = old)),
                colors,
            ))
        }
        RefDialogKind::Remote { editing } => {
            let existing = if dialog.remote_names.is_empty() {
                ui::t!("dialogs.ref.help.no_remotes").to_string()
            } else {
                ui::t!(
                    "dialogs.ref.help.existing_remotes",
                    remotes = dialog.remote_names.join(", ")
                )
                .to_string()
            };
            let mut body = body
                .child(name_field(
                    ui::t!("dialogs.ref.field.name"),
                    dialog.name.clone(),
                    valid.name_error.as_deref().filter(|_| dialog.touched),
                    Some(&existing),
                    colors,
                ))
                .child(url_field(dialog, &valid, colors));
            if editing.is_some() {
                body = body.child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child(ui::t!("dialogs.ref.help.rename_remote")),
                );
            } else {
                let toggle = {
                    let state = state.clone();
                    move |next: bool, _: &mut Window, cx: &mut App| {
                        state.update(cx, |dialog, cx| {
                            dialog.fetch_after_add = next;
                            dialog.notify_host(cx);
                            cx.notify();
                        });
                    }
                };
                body = body.child(
                    Checkbox::new("ref-remote-fetch")
                        .label(ui::t!("dialogs.ref.fetch_after_add"))
                        .checked(dialog.fetch_after_add)
                        .focus_handle(dialog.fetch_focus.clone())
                        .on_toggle(toggle),
                );
            }
            body
        }
        RefDialogKind::Reset { card } => body
            .child(commit_card(card, colors))
            .child(reset_mode_control(dialog, state, colors))
            .child(reset_note(dialog, card, colors)),
        RefDialogKind::DeleteBranch { name, force } => body.child(confirm_note(
            if *force {
                ui::t!("dialogs.ref.confirm.branch_unmerged", name = name)
            } else {
                ui::t!("dialogs.ref.confirm.branch", name = name)
            },
            if *force {
                ui::t!("dialogs.ref.confirm.branch_unmerged_note")
            } else {
                ui::t!("dialogs.ref.confirm.branch_note")
            },
        )),
        RefDialogKind::DeleteTag { name } => body.child(confirm_note(
            ui::t!("dialogs.ref.confirm.tag", name = name),
            ui::t!("dialogs.ref.confirm.tag_note"),
        )),
        RefDialogKind::RemoveRemote { name } => body.child(confirm_note(
            ui::t!("dialogs.ref.confirm.remote", name = name),
            ui::t!("dialogs.ref.confirm.remote_note"),
        )),
        RefDialogKind::DropStash { index, label } => body.child(confirm_note(
            ui::t!("dialogs.ref.confirm.stash", index = index, label = label),
            ui::t!("dialogs.ref.confirm.stash_note"),
        )),
        RefDialogKind::CherryPick { card } => body.child(commit_card(card, colors)).child(
            Banner::new(
                BannerKind::Info,
                ui::t!(
                    "dialogs.ref.confirm.cherry_pick",
                    commit = card.short,
                    branch = dialog.branch
                ),
            )
            .body(ui::t!("dialogs.ref.confirm.cherry_pick_note")),
        ),
        RefDialogKind::Revert { card } => body.child(commit_card(card, colors)).child(
            Banner::new(
                BannerKind::Info,
                ui::t!("dialogs.ref.confirm.revert", commit = card.short),
            )
            .body(ui::t!("dialogs.ref.confirm.revert_note")),
        ),
    };
    if let Some(error) = &dialog.error {
        body = body.child(
            Banner::new(BannerKind::Error, ui::t!("dialogs.ref.operation_failed"))
                .body(error.clone()),
        );
    }

    let cancel = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.close(window, cx));
        }) as DialogAction
    };
    let primary = {
        let state = state.clone();
        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(window, cx));
        }) as DialogAction
    };
    let on_default = {
        let state = state.clone();
        move |window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.start(window, cx));
        }
    };

    let primary_label = match &dialog.kind {
        RefDialogKind::CreateBranch { .. } => {
            if dialog.checkout {
                ui::t!("dialogs.ref.primary.create_and_switch")
            } else {
                ui::t!("dialogs.ref.primary.create_branch")
            }
        }
        RefDialogKind::CreateTag { .. } if dialog.push_tag_after_create => {
            ui::t!("dialogs.ref.primary.create_and_push")
        }
        RefDialogKind::CreateTag { .. } => ui::t!("dialogs.ref.primary.create_tag"),
        RefDialogKind::RenameBranch { .. }
        | RefDialogKind::RenameTag { .. }
        | RefDialogKind::RenameWorktree { .. } => ui::t!("dialogs.ref.primary.rename"),
        RefDialogKind::Remote { editing: None, .. } => ui::t!("dialogs.ref.primary.add_remote"),
        RefDialogKind::Remote {
            editing: Some(_), ..
        } => ui::t!("dialogs.ref.primary.save"),
        RefDialogKind::Reset { .. } => ui::t!("dialogs.ref.primary.reset"),
        RefDialogKind::DeleteBranch { force: true, .. } => {
            ui::t!("dialogs.ref.primary.delete_anyway")
        }
        RefDialogKind::DeleteBranch { .. } => ui::t!("dialogs.ref.primary.delete_branch"),
        RefDialogKind::DeleteTag { .. } => ui::t!("dialogs.ref.primary.delete_tag"),
        RefDialogKind::RemoveRemote { .. } => ui::t!("dialogs.ref.primary.remove_remote"),
        RefDialogKind::DropStash { .. } => ui::t!("dialogs.ref.primary.drop_stash"),
        RefDialogKind::CherryPick { .. } => ui::t!("dialogs.ref.primary.cherry_pick"),
        RefDialogKind::Revert { .. } => ui::t!("dialogs.ref.primary.revert"),
    };
    let dangerous = matches!(
        dialog.kind,
        RefDialogKind::DeleteBranch { .. }
            | RefDialogKind::DeleteTag { .. }
            | RefDialogKind::RemoveRemote { .. }
            | RefDialogKind::DropStash { .. }
            | RefDialogKind::Reset { .. }
    );
    let mut primary_button = if dangerous {
        Button::danger("ref-primary")
    } else {
        Button::primary("ref-primary")
    };
    primary_button = primary_button
        .label(primary_label)
        .min_width(px(72.))
        .disabled(!valid.enabled)
        .focus_handle(dialog.primary_focus.clone())
        .on_click(primary);
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("ref-cancel")
                .label(ui::t!("dialogs.common.cancel"))
                .min_width(px(72.))
                .disabled(dialog.running)
                .focus_handle(dialog.cancel_focus.clone())
                .on_click(cancel),
        )
        .child(primary_button);

    let mut focus_order = Vec::new();
    if needs_name(&dialog.kind) {
        focus_order.push(DialogFocus::input(dialog.name_focus.clone()));
    }
    if dialog.message.is_some() {
        focus_order.push(DialogFocus::input(dialog.message_focus.clone()));
    }
    if dialog.url.is_some() {
        focus_order.push(DialogFocus::input(dialog.url_focus.clone()));
    }
    if matches!(dialog.kind, RefDialogKind::CreateBranch { .. }) {
        focus_order.push(DialogFocus::control(dialog.checkout_focus.clone()));
    }
    if matches!(dialog.kind, RefDialogKind::CreateTag { .. }) && dialog.push_remote.is_some() {
        focus_order.push(DialogFocus::control(dialog.push_focus.clone()));
    }
    if matches!(dialog.kind, RefDialogKind::Remote { editing: None }) {
        focus_order.push(DialogFocus::control(dialog.fetch_focus.clone()));
    }
    if dialog.annotated && dialog.signing_key.is_some() {
        focus_order.push(DialogFocus::control(dialog.sign_focus.clone()));
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let hint = if dialog.running {
        Some(ui::t!("dialogs.ref.working"))
    } else if matches!(dialog.kind, RefDialogKind::DeleteBranch { force: true, .. }) {
        Some(ui::t!("dialogs.ref.cannot_undo"))
    } else {
        valid.hint
    };

    let mut panel = Dialog::new(title)
        .icon(icon)
        .width(px(width))
        .compact_body()
        .body(body)
        .footer(footer)
        .busy(dialog.running)
        .focus_order(focus_order)
        .default_button(dialog.primary_focus.clone())
        .default_danger(dangerous)
        .on_default(on_default);
    if needs_name(&dialog.kind) {
        panel = panel.autofocus(dialog.name_focus.clone());
    }
    if let Some(hint) = hint {
        panel = panel.hint(hint);
    }
    panel
}

/// The tinted commit summary at the top of the branch, tag and reset dialogs.
fn commit_card(card: &CommitCard, colors: ThemeColors) -> impl IntoElement {
    let mut root = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(3.))
        .p(px(10.))
        .rounded(R_MD)
        .border_1()
        .border_color(colors.border)
        .bg(colors.bg)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.accent)
                        .child(card.short.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(FS_BASE)
                        .child(if card.summary.is_empty() {
                            ui::t!("dialogs.ref.card.commit").to_string()
                        } else {
                            card.summary.clone()
                        }),
                ),
        );
    if !card.author.is_empty() {
        let now = unix_now();
        let when = relative_time(now, card.time.max(0) as u64);
        let mut meta = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(format!("{} · {when}", card.author));
        for card_ref in &card.refs {
            let kind = match card_ref.kind {
                CardRefKind::Local => ui::RefKind::Local,
                CardRefKind::Remote => ui::RefKind::Remote,
                CardRefKind::Tag => ui::RefKind::Tag,
                CardRefKind::Head => ui::RefKind::Head,
            };
            meta = meta.child(
                RefBadge::new(format!("card-ref-{}", card_ref.name), card_ref.name.clone())
                    .kind(kind)
                    .max_label_width(px(150.)),
            );
        }
        root = root.child(meta);
    }
    root
}

fn name_field(
    label: SharedString,
    input: Option<Entity<TextInput>>,
    error: Option<&str>,
    help: Option<&str>,
    colors: ThemeColors,
) -> impl IntoElement {
    field_shell(
        label,
        input.map(IntoElement::into_any_element),
        error,
        help,
        colors,
    )
}

fn message_field(
    area: Option<Entity<TextArea>>,
    error: Option<&str>,
    colors: ThemeColors,
) -> impl IntoElement {
    field_shell(
        ui::t!("dialogs.ref.field.message"),
        area.map(IntoElement::into_any_element),
        error,
        None,
        colors,
    )
}

fn url_field(dialog: &RefDialog, valid: &Validity, colors: ThemeColors) -> impl IntoElement {
    let url_error =
        (dialog.touched && !valid.url.is_empty() && valid.url_kind == RemoteUrlKind::Bad)
            .then(|| valid.url_kind.label());
    let note = match valid.url_kind {
        RemoteUrlKind::Https | RemoteUrlKind::Ssh => {
            let (icon, color) = if valid.url_kind == RemoteUrlKind::Ssh {
                (IconName::Key, colors.info)
            } else {
                (IconName::Check, colors.success)
            };
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(FS_SM)
                    .text_color(color)
                    .child(Icon::new(icon).size(px(12.)).color(color))
                    .child(valid.url_kind.label()),
            )
        }
        RemoteUrlKind::Bad => None,
    };
    let mut field = field_shell(
        ui::t!("dialogs.ref.field.address"),
        dialog.url.clone().map(IntoElement::into_any_element),
        url_error.as_deref(),
        None,
        colors,
    );
    if let Some(note) = note {
        field = field.child(note);
    }
    field
}

fn field_shell(
    label: SharedString,
    control: Option<AnyElement>,
    error: Option<&str>,
    help: Option<&str>,
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
            .child(div().text_size(FS_SM).text_color(colors.error).child("*")),
    );
    if let Some(control) = control {
        field = field.child(control);
    }
    if let Some(error) = error {
        field = field.child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                .child(error.to_owned()),
        );
    } else if let Some(help) = help {
        field = field.child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(help.to_owned()),
        );
    }
    field
}

/// Why a worktree folder name cannot be used, or `None` when it can.
///
/// The name is one path segment: no separators, and not `.` or `..`.
fn worktree_folder_error(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name == "." || name == ".." || name.contains(['/', '\\']) || name.contains('\0') {
        Some(ui::t!("dialogs.ref.error.folder_name").to_string())
    } else {
        None
    }
}

/// The destination of a renamed worktree: the same parent, a new folder name.
fn worktree_destination(from: &Path, name: &str) -> PathBuf {
    match from.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn branch_note(card: &CommitCard, dialog: &RefDialog) -> AnyElement {
    let is_head = dialog.head_oid == Some(card.oid);
    match branch_changes_note(dialog.dirty, is_head) {
        Some((kind, title, body)) => Banner::new(kind, title).body(body).into_any_element(),
        None => div().into_any_element(),
    }
}

fn tag_type(
    dialog: &RefDialog,
    state: &Entity<RefDialog>,
    colors: ThemeColors,
) -> impl IntoElement {
    let listener = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.annotated = index == 0;
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!("dialogs.ref.tag_type")),
        )
        .child(
            Segmented::new("ref-tag-type")
                .item(SegmentItem::new(ui::t!("dialogs.ref.tag_type.annotated")))
                .item(SegmentItem::new(ui::t!("dialogs.ref.tag_type.lightweight")))
                .selected(if dialog.annotated { 0 } else { 1 })
                .on_select(listener),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(if dialog.annotated {
                    ui::t!("dialogs.ref.tag_type.annotated_note")
                } else {
                    ui::t!("dialogs.ref.tag_type.lightweight_note")
                }),
        )
}

fn reset_mode_control(
    dialog: &RefDialog,
    state: &Entity<RefDialog>,
    colors: ThemeColors,
) -> impl IntoElement {
    let listener = {
        let state = state.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| {
                dialog.reset_mode = match index {
                    0 => ResetMode::Soft,
                    2 => ResetMode::Hard,
                    _ => ResetMode::Mixed,
                };
                dialog.notify_host(cx);
                cx.notify();
            });
        }
    };
    let selected = match dialog.reset_mode {
        ResetMode::Soft => 0,
        ResetMode::Mixed => 1,
        ResetMode::Hard => 2,
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!("dialogs.ref.reset_mode")),
        )
        .child(
            Segmented::new("ref-reset-mode")
                .item(SegmentItem::new(ui::t!("dialogs.ref.reset_mode.soft")))
                .item(SegmentItem::new(ui::t!("dialogs.ref.reset_mode.mixed")))
                .item(SegmentItem::new(ui::t!("dialogs.ref.reset_mode.hard")))
                .selected(selected)
                .on_select(listener),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(match dialog.reset_mode {
                    ResetMode::Soft => ui::t!("dialogs.ref.reset_mode.soft_note"),
                    ResetMode::Mixed => ui::t!("dialogs.ref.reset_mode.mixed_note"),
                    ResetMode::Hard => ui::t!("dialogs.ref.reset_mode.hard_note"),
                }),
        )
}

fn reset_note(dialog: &RefDialog, card: &CommitCard, colors: ThemeColors) -> impl IntoElement {
    let _ = colors;
    if dialog.reset_mode == ResetMode::Hard {
        Banner::new(
            BannerKind::Warn,
            ui::t!("dialogs.ref.reset.hard_title", commit = card.short),
        )
        .body(ui::t!("dialogs.ref.reset.hard_body"))
    } else {
        Banner::new(
            BannerKind::Info,
            ui::t!(
                "dialogs.ref.reset.title",
                branch = dialog.branch,
                commit = card.short
            ),
        )
        .body(ui::t!("dialogs.ref.reset.body"))
    }
}

fn confirm_note(title: SharedString, body: SharedString) -> impl IntoElement {
    Banner::new(BannerKind::Warn, title).body(body)
}

/// The branch dialog's local-changes note. The shell knows the dirty count and
/// whether the commit is HEAD; both arrive through the dialog's context, so
/// this is built from the card and the current branch only. Kept separate so
/// the note can grow when the shell passes more context.
fn branch_changes_note(
    context_dirty: usize,
    is_head: bool,
) -> Option<(BannerKind, String, SharedString)> {
    if context_dirty == 0 {
        return None;
    }
    if is_head {
        Some((
            BannerKind::Info,
            ui::i18n::t_count(
                "dialogs.ref.dirty.carry",
                u64::try_from(context_dirty).unwrap_or(u64::MAX),
                &[],
            )
            .to_string(),
            ui::t!("dialogs.ref.dirty.carry_note"),
        ))
    } else {
        Some((
            BannerKind::Warn,
            ui::i18n::t_count(
                "dialogs.ref.dirty.conflict",
                u64::try_from(context_dirty).unwrap_or(u64::MAX),
                &[],
            )
            .to_string(),
            ui::t!("dialogs.ref.dirty.conflict_note"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_notes_follow_the_dirty_count_and_target() {
        assert!(branch_changes_note(0, true).is_none());
        let (kind, title, _) = branch_changes_note(6, true).unwrap();
        assert_eq!(kind, BannerKind::Info);
        assert_eq!(title, "6 local changes will come with you");
        let (kind, _, _) = branch_changes_note(1, false).unwrap();
        assert_eq!(kind, BannerKind::Warn);
    }
}
