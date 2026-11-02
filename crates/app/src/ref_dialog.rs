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
    ParentElement, Styled, Subscription, Window,
};

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
use crate::ref_ops::{
    ref_name_error, remote_name_error, remote_url_kind, CardRefKind, CommitCard, RemoteUrlKind,
};

/// The width every remote/destructive dialog uses when the design gives none.
const CONFIRM_WIDTH: f32 = 480.;

/// Shared context the shell passes to a dialog when it opens.
pub(crate) struct RefDialogContext {
    pub root: PathBuf,
    pub git: Result<Arc<Git>, Arc<Error>>,
    pub refs: Vec<Ref>,
    pub remotes: Vec<String>,
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
                area.set_placeholder("Release notes or a short description", cx);
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
                            "Enter a folder name"
                        } else {
                            "Enter a branch name"
                        },
                    )
                } else if name_error.is_some() {
                    Some("Fix the name to continue")
                } else if rename_unchanged {
                    Some("Enter a new name")
                } else {
                    None
                },
            ),
            RefDialogKind::CreateTag { .. } | RefDialogKind::RenameTag { .. } => (
                !name.is_empty()
                    && name_error.is_none()
                    && !rename_unchanged
                    && !(self.annotated && message.is_empty()),
                if name.is_empty() {
                    Some("Enter a tag name")
                } else if name_error.is_some() {
                    Some("Fix the name to continue")
                } else if rename_unchanged {
                    Some("Enter a new name")
                } else if self.annotated && message.is_empty() {
                    Some("Add a message to continue")
                } else {
                    None
                },
            ),
            RefDialogKind::Remote { .. } => (
                self.remote_valid(&name, name_error.as_ref(), url_kind),
                if name.is_empty() {
                    Some("Enter a remote name")
                } else if name_error.is_some() {
                    Some("Choose another name")
                } else if url.is_empty() {
                    Some("Enter the remote address")
                } else if url_kind == RemoteUrlKind::Bad {
                    Some("Fix the address")
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
                    format!("Created {name} and switched to it")
                } else {
                    format!("Created {name}")
                })
            }),
            RefDialogKind::CreateTag { card } => Box::new(move |repo: &Repo| {
                let message = annotated.then_some(message.as_str());
                repo.create_tag(&name, card.oid, message, sign)?;
                Ok(format!("Created tag {name}"))
            }),
            RefDialogKind::RenameBranch { old } => Box::new(move |repo: &Repo| {
                repo.rename_branch(&old, &name)?;
                Ok(format!("Renamed {old} to {name}"))
            }),
            RefDialogKind::RenameWorktree { from } => Box::new(move |repo: &Repo| {
                let to = worktree_destination(&from, &name);
                repo.move_worktree(&from, &to)?;
                Ok(format!("Moved worktree to {}", to.display()))
            }),
            RefDialogKind::RenameTag { old } => Box::new(move |repo: &Repo| {
                repo.rename_tag(&old, &name)?;
                Ok(format!("Renamed tag {old} to {name}"))
            }),
            RefDialogKind::Remote { editing } => Box::new(move |repo: &Repo| match editing {
                None => {
                    repo.add_remote(&name, &url)?;
                    Ok(format!("Added remote {name}"))
                }
                Some(edit) => {
                    if edit.name != name {
                        repo.rename_remote(&edit.name, &name)?;
                    }
                    if edit.url != url {
                        repo.set_remote_url(&name, &url)?;
                    }
                    Ok(format!("Updated remote {name}"))
                }
            }),
            RefDialogKind::Reset { card } => Box::new(move |repo: &Repo| {
                repo.reset(card.oid, mode)?;
                Ok(format!("Reset {branch} to {}", card.short))
            }),
            RefDialogKind::DeleteBranch { name, force } => Box::new(move |repo: &Repo| {
                repo.delete_branch(&name, force)?;
                Ok(format!("Deleted branch {name}"))
            }),
            RefDialogKind::DeleteTag { name } => Box::new(move |repo: &Repo| {
                repo.delete_tag(&name)?;
                Ok(format!("Deleted tag {name}"))
            }),
            RefDialogKind::RemoveRemote { name } => Box::new(move |repo: &Repo| {
                repo.remove_remote(&name)?;
                Ok(format!("Removed remote {name}"))
            }),
            RefDialogKind::DropStash { index, .. } => Box::new(move |repo: &Repo| {
                repo.stash_drop(index)?;
                Ok(format!("Dropped stash@{{{index}}}"))
            }),
            RefDialogKind::CherryPick { card } => Box::new(move |repo: &Repo| {
                repo.cherry_pick(card.oid)?;
                Ok(format!("Cherry-picked {}", card.short))
            }),
            RefDialogKind::Revert { card } => Box::new(move |repo: &Repo| {
                repo.revert(card.oid)?;
                Ok(format!("Reverted {}", card.short))
            }),
        })
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
                self.close(window, cx);
                toast::show(ToastKind::Ok, notice, window, cx);
                let on_done = self.on_done.clone();
                on_done(window, cx);
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
    hint: Option<&'static str>,
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
        None => Dialog::new("Repository operation").icon(IconName::Git),
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
        RefDialogKind::CreateBranch { .. } => ("Create branch here", IconName::Branch, 480.),
        RefDialogKind::CreateTag { .. } => ("Create tag here", IconName::Tag, 500.),
        RefDialogKind::RenameBranch { .. } => ("Rename branch", IconName::Edit, 480.),
        RefDialogKind::RenameWorktree { .. } => ("Rename worktree", IconName::Edit, 480.),
        RefDialogKind::RenameTag { .. } => ("Rename tag", IconName::Edit, 480.),
        RefDialogKind::Remote { editing: None } => ("Add remote", IconName::Cloud, 500.),
        RefDialogKind::Remote { editing: Some(_) } => ("Edit remote", IconName::Cloud, 500.),
        RefDialogKind::Reset { .. } => ("Reset branch to here", IconName::Undo, 500.),
        RefDialogKind::DeleteBranch { .. } => ("Delete branch", IconName::Trash, CONFIRM_WIDTH),
        RefDialogKind::DeleteTag { .. } => ("Delete tag", IconName::Trash, CONFIRM_WIDTH),
        RefDialogKind::RemoveRemote { .. } => ("Remove remote", IconName::Trash, CONFIRM_WIDTH),
        RefDialogKind::DropStash { .. } => ("Drop stash", IconName::Trash, CONFIRM_WIDTH),
        RefDialogKind::CherryPick { .. } => ("Cherry-pick commit", IconName::Undo, CONFIRM_WIDTH),
        RefDialogKind::Revert { .. } => ("Revert commit", IconName::Undo, CONFIRM_WIDTH),
    };

    let mut body = div().flex().flex_col().gap(px(14.));
    body = match &dialog.kind {
        RefDialogKind::CreateBranch { card } => {
            let mut body = body.child(commit_card(card, colors)).child(name_field(
                "Branch name",
                dialog.name.clone(),
                valid.name_error.as_deref().filter(|_| dialog.touched),
                Some(&format!(
                    "Starts at {}. Use / to group branches, e.g. feat/…",
                    card.short
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
                    .label("Switch to the new branch after creating it")
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
                    "Tag name",
                    dialog.name.clone(),
                    valid.name_error.as_deref().filter(|_| dialog.touched),
                    dialog
                        .latest_tag
                        .as_ref()
                        .map(|tag| format!("Latest tag: {tag}"))
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
                        .map(|_| "An annotated tag needs a message."),
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
                            .label(format!("Sign with GPG key {key}"))
                            .checked(dialog.sign)
                            .focus_handle(dialog.sign_focus.clone())
                            .on_toggle(toggle),
                    );
                }
            }
            body = body.child(
                Checkbox::new("ref-tag-push")
                    .label("Push the tag to origin after creating — arrives with push")
                    .checked(false)
                    .disabled(true),
            );
            body
        }
        RefDialogKind::RenameWorktree { from } => body.child(name_field(
            "New folder name",
            dialog.name.clone(),
            valid.name_error.as_deref().filter(|_| dialog.touched),
            Some(&format!(
                "Moves {} next to its current folder. Git refuses a locked or main worktree.",
                from.display()
            )),
            colors,
        )),
        RefDialogKind::RenameBranch { old } | RefDialogKind::RenameTag { old } => body
            .child(name_field(
                if matches!(dialog.kind, RefDialogKind::RenameBranch { .. }) {
                    "New branch name"
                } else {
                    "New tag name"
                },
                dialog.name.clone(),
                valid.name_error.as_deref().filter(|_| dialog.touched),
                Some(&format!("Renames {old} in this repository.")),
                colors,
            )),
        RefDialogKind::Remote { editing } => {
            let existing = if dialog.remote_names.is_empty() {
                "No remotes yet.".to_owned()
            } else {
                format!("Existing remotes: {}", dialog.remote_names.join(", "))
            };
            let mut body = body
                .child(name_field(
                    "Name",
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
                        .child("Renaming the remote also renames its remote-tracking branches."),
                );
            } else {
                body = body.child(
                    Checkbox::new("ref-remote-fetch")
                        .label("Fetch branches and tags right after adding — arrives with fetch")
                        .checked(false)
                        .disabled(true),
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
                format!("The branch “{name}” has commits that are not merged anywhere else. Deleting it loses them.")
            } else {
                format!("Deletes the local branch “{name}”. This cannot be undone.")
            },
            if *force {
                "Deleting an unmerged branch is permanent."
            } else {
                "The commits stay in the repository until they are unreachable."
            },
        )),
        RefDialogKind::DeleteTag { name } => body.child(confirm_note(
            format!("Deletes the tag “{name}”. This cannot be undone."),
            "Other clones keep their copy until it is deleted there too.",
        )),
        RefDialogKind::RemoveRemote { name } => body.child(confirm_note(
            format!("Removes the remote “{name}” and its remote-tracking branches from this repository."),
            "The repository on the server is not touched.",
        )),
        RefDialogKind::DropStash { index, label } => body.child(confirm_note(
            format!("Drops stash@{{{index}}}: {label}"),
            "The changes are gone; this cannot be undone.",
        )),
        RefDialogKind::CherryPick { card } => body
            .child(commit_card(card, colors))
            .child(Banner::new(
                BannerKind::Info,
                format!("Applies {} on top of {}.", card.short, dialog.branch),
            )
            .body("A conflict aborts the cherry-pick and leaves the repository as it was.")),
        RefDialogKind::Revert { card } => body
            .child(commit_card(card, colors))
            .child(Banner::new(
                BannerKind::Info,
                format!("Creates a new commit that undoes {}.", card.short),
            )
            .body("A conflict aborts the revert and leaves the repository as it was.")),
    };
    if let Some(error) = &dialog.error {
        body =
            body.child(Banner::new(BannerKind::Error, "The operation failed").body(error.clone()));
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
                "Create and switch"
            } else {
                "Create branch"
            }
        }
        RefDialogKind::CreateTag { .. } => "Create tag",
        RefDialogKind::RenameBranch { .. }
        | RefDialogKind::RenameTag { .. }
        | RefDialogKind::RenameWorktree { .. } => "Rename",
        RefDialogKind::Remote { editing: None, .. } => "Add remote",
        RefDialogKind::Remote {
            editing: Some(_), ..
        } => "Save",
        RefDialogKind::Reset { .. } => "Reset",
        RefDialogKind::DeleteBranch { force: true, .. } => "Delete anyway",
        RefDialogKind::DeleteBranch { .. } => "Delete branch",
        RefDialogKind::DeleteTag { .. } => "Delete tag",
        RefDialogKind::RemoveRemote { .. } => "Remove remote",
        RefDialogKind::DropStash { .. } => "Drop stash",
        RefDialogKind::CherryPick { .. } => "Cherry-pick",
        RefDialogKind::Revert { .. } => "Revert",
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
                .label("Cancel")
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
    if dialog.annotated && dialog.signing_key.is_some() {
        focus_order.push(DialogFocus::control(dialog.sign_focus.clone()));
    }
    focus_order.push(DialogFocus::control(dialog.cancel_focus.clone()));
    focus_order.push(DialogFocus::control(dialog.primary_focus.clone()));

    let hint = if dialog.running {
        Some("Working…".to_owned())
    } else if matches!(dialog.kind, RefDialogKind::DeleteBranch { force: true, .. }) {
        Some("This cannot be undone".to_owned())
    } else {
        valid.hint.map(str::to_owned)
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
                            "Commit".to_owned()
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
    label: &'static str,
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
        "Message",
        area.map(IntoElement::into_any_element),
        error,
        None,
        colors,
    )
}

fn url_field(dialog: &RefDialog, valid: &Validity, colors: ThemeColors) -> impl IntoElement {
    let url_error =
        (dialog.touched && !valid.url.is_empty() && valid.url_kind == RemoteUrlKind::Bad)
            .then_some(valid.url_kind.label());
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
        "Address",
        dialog.url.clone().map(IntoElement::into_any_element),
        url_error,
        None,
        colors,
    );
    if let Some(note) = note {
        field = field.child(note);
    }
    field
}

fn field_shell(
    label: &'static str,
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
        Some("Use a single folder name, without / or \\".to_owned())
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
                .child("Type"),
        )
        .child(
            Segmented::new("ref-tag-type")
                .item(SegmentItem::new("Annotated"))
                .item(SegmentItem::new("Lightweight"))
                .selected(if dialog.annotated { 0 } else { 1 })
                .on_select(listener),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(if dialog.annotated {
                    "Stores author, date and a message. Recommended for releases."
                } else {
                    "Just a name pointing at the commit."
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
                .child("Mode"),
        )
        .child(
            Segmented::new("ref-reset-mode")
                .item(SegmentItem::new("Soft"))
                .item(SegmentItem::new("Mixed"))
                .item(SegmentItem::new("Hard"))
                .selected(selected)
                .on_select(listener),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(match dialog.reset_mode {
                    ResetMode::Soft => {
                        "Moves HEAD only. The index and work tree keep their changes."
                    }
                    ResetMode::Mixed => {
                        "Moves HEAD and clears the index. The work tree keeps its files."
                    }
                    ResetMode::Hard => "Moves HEAD and discards index and work tree changes.",
                }),
        )
}

fn reset_note(dialog: &RefDialog, card: &CommitCard, colors: ThemeColors) -> impl IntoElement {
    let _ = colors;
    if dialog.reset_mode == ResetMode::Hard {
        Banner::new(
            BannerKind::Warn,
            format!("Hard reset to {} discards every local change", card.short),
        )
        .body("Staged and unstaged changes in the work tree are lost. This cannot be undone.")
    } else {
        Banner::new(
            BannerKind::Info,
            format!("Moves {} to {}", dialog.branch, card.short),
        )
        .body("Local commits after this point become unreachable from the branch.")
    }
}

fn confirm_note(title: String, body: &'static str) -> impl IntoElement {
    Banner::new(BannerKind::Warn, title).body(body)
}

/// The branch dialog's local-changes note. The shell knows the dirty count and
/// whether the commit is HEAD; both arrive through the dialog's context, so
/// this is built from the card and the current branch only. Kept separate so
/// the note can grow when the shell passes more context.
fn branch_changes_note(
    context_dirty: usize,
    is_head: bool,
) -> Option<(BannerKind, String, &'static str)> {
    if context_dirty == 0 {
        return None;
    }
    if is_head {
        Some((
            BannerKind::Info,
            format!("{context_dirty} local changes will come with you"),
            "The new branch starts at the current commit, so nothing conflicts.",
        ))
    } else {
        Some((
            BannerKind::Warn,
            format!("{context_dirty} local changes may conflict"),
            "Switching to another commit rewrites files that differ from your working tree. GitWarp stops and keeps your changes if a file conflicts.",
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
