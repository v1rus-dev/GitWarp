//! The menu model shared by the sidebar and the History's ref menus: the
//! action a menu item runs, the GPUI-free item, and the predicates both menus
//! call so that they can never disagree (is this the current branch, can it be
//! merged, can a tag be pushed). Nothing here knows which menu is built.

use std::rc::Rc;

use git_core::{Head, Oid, Operation};
use gpui::{App, Window};
use ui::IconName;

use crate::web_links::HostLink;

/// What a sidebar context-menu item does when it is enabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    /// Nothing yet: the item arrives with a later stage.
    None,
    /// Writes this text to the clipboard.
    CopyText(String),
    /// Opens `url` in the browser and toasts that `host` was opened.
    OpenUrl {
        url: String,
        host: String,
    },
    /// Reloads the sidebar's lists ([`RepositoryView::refresh`]).
    ///
    /// [`RepositoryView::refresh`]: crate::repo_view::RepositoryView::refresh
    Refresh,
    /// Checks out a local branch.
    CheckoutBranch(String),
    /// Checks out a remote branch, creating a local tracking branch if needed.
    CheckoutRemoteBranch(String),
    /// Checks out a tag with a detached HEAD.
    CheckoutTag(String),
    /// Opens the Merge dialog for a local branch or a remote-tracking ref
    /// (`origin/feat/x`), merging it into the current branch.
    Merge(String),
    /// Opens "Create branch here" at `oid`.
    CreateBranchAt(Oid),
    /// Opens "Create pull request" for a local branch.
    CreatePullRequest(String),
    RenameBranch(String),
    DeleteBranch(String),
    RenameTag(String),
    DeleteTag(String),
    /// Applies a stash by index without dropping it.
    ApplyStash(usize),
    PopStash(usize),
    DropStash(usize),
    /// Opens "Create tag" at HEAD.
    CreateTag,
    /// Opens "Create tag" at this commit.
    CreateTagAt(Oid),
    /// Pushes a local branch (checked out or not) to its upstream remote.
    PushBranch(String),
    /// Pulls the current branch.
    Pull,
    /// Copies the full message (subject and body) of a commit.
    CopyCommitMessage(Oid),
    /// Opens the add-remote dialog.
    AddRemote,
    /// Account… of a remote: picker, Add account dialog or an info toast.
    RemoteAccount(String),
    /// "Accounts for push URLs…" of a multi-push remote: an info toast.
    RemotePushAccounts(String),
    /// "Add account for <host>" row under a remote.
    AddAccountFor(String),
    EditRemote(String),
    RemoveRemote(String),
    /// Fetches one remote (`origin`, `upstream`, …).
    FetchRemote(String),
    /// Fetches every remote with tags and pruning.
    FetchAll,
    /// Prunes stale remote-tracking branches while fetching.
    FetchPrune,
    /// Makes `origin/<name>` (or the default remote's branch) the upstream.
    SetUpstream(String),
    /// Pushes one tag to the default remote.
    PushTag(String),
    /// Pushes all tags to the default remote.
    PushTags,
    /// Deletes a remote branch, e.g. `origin/feat/x`.
    DeleteRemoteBranch(String),
    /// Shows a stash's files and diff in the details dock.
    ViewStash(usize),
    /// Initialises and updates a submodule, by path.
    UpdateSubmodule(String),
    /// Opens an initialised submodule's folder in a second window.
    OpenSubmodule(String),
    /// Narrows the History list to one branch's own commits (local name or
    /// `<remote>/<name>`).
    ShowBranchCommits(String),
    /// Drops the History's branch filter (the active filter tool of a ref menu header).
    ShowAllCommits,
    /// Shows the Worktrees page.
    ShowWorktrees,
    /// Opens the add-worktree dialog.
    AddWorktree,
    /// Opens a worktree in a second window.
    OpenWorktree(String),
    /// Moves a worktree's folder. The path is the current location.
    RenameWorktree(String),
    /// Locks a worktree by path.
    LockWorktree(String),
    /// Unlocks a worktree by path.
    UnlockWorktree(String),
    /// Opens the remove-worktree dialog for a path.
    RemoveWorktree(String),
    /// Drops the records of worktrees whose folders are gone.
    PruneWorktrees,
}

/// One item of a sidebar context menu, free of GPUI so the labels, order and
/// enabled state are unit-testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarMenuItem {
    pub label: String,
    pub icon: IconName,
    /// Muted text at the right (`current`).
    pub hint: Option<String>,
    pub shortcut: Option<String>,
    pub danger: bool,
    /// [`SidebarAction::None`] exactly when the item is disabled.
    pub action: SidebarAction,
}

impl SidebarMenuItem {
    /// A shown-but-disabled item; [`Self::action`] enables it.
    pub(crate) fn new(label: impl Into<String>, icon: IconName) -> Self {
        Self {
            label: label.into(),
            icon,
            hint: None,
            shortcut: None,
            danger: false,
            action: SidebarAction::None,
        }
    }

    /// An item is enabled exactly when it has an action.
    pub fn enabled(&self) -> bool {
        !matches!(self.action, SidebarAction::None)
    }

    pub(crate) fn action(mut self, action: SidebarAction) -> Self {
        self.action = action;
        self
    }

    /// Enabled with the action when there is one, else shown but off.
    pub(crate) fn maybe_action(self, action: Option<SidebarAction>) -> Self {
        match action {
            Some(action) => self.action(action),
            None => self,
        }
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: &str) -> Self {
        self.shortcut = Some(shortcut.to_owned());
        self
    }

    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// The UI kit's menu item: a click calls `handler` with the action; an
    /// item without one stays shown but off.
    pub(crate) fn into_menu_item(self, handler: SidebarActionHandler) -> ui::MenuItem {
        let disabled = !self.enabled();
        let mut menu_item = ui::MenuItem::new(self.label)
            .icon(self.icon)
            .disabled(disabled);
        if let Some(shortcut) = self.shortcut {
            menu_item = menu_item.shortcut(shortcut);
        }
        if let Some(hint) = self.hint {
            menu_item = menu_item.hint(hint);
        }
        if self.danger {
            menu_item = menu_item.danger();
        }
        let action = self.action;
        menu_item.action(move |window, cx| handler(action.clone(), window, cx))
    }
}

/// Runs a [`SidebarAction`] of a menu item.
pub(crate) type SidebarActionHandler = Rc<dyn Fn(SidebarAction, &mut Window, &mut App)>;

/// "Open on <Service>" (or any other `label`) for `target` on the host of
/// `link`; `None` when the service has no such page.
pub(crate) fn open_item(
    link: &HostLink,
    target: &forge::WebTarget,
    label: impl Into<String>,
) -> Option<SidebarMenuItem> {
    let url = link.url(target)?;
    Some(
        SidebarMenuItem::new(label, IconName::External).action(SidebarAction::OpenUrl {
            url,
            host: link.host().to_owned(),
        }),
    )
}

/// Merging needs a branch to merge into (not a detached or unread HEAD) and
/// no stopped operation (git refuses to start another).
pub(crate) fn can_merge(head: Option<&Head>, operation: Operation) -> bool {
    matches!(head, Some(Head::Branch { .. })) && operation == Operation::None
}

/// What "Merge … into {head}" names: the branch, or `HEAD` when detached or
/// still unread.
pub(crate) fn head_label(head: Option<&Head>) -> &str {
    match head {
        Some(Head::Branch { name, .. }) => name.as_str(),
        Some(Head::Detached(_)) | None => "HEAD",
    }
}

/// Whether `name` is the checked-out branch. By the branch name of HEAD, so a
/// detached or unread HEAD matches nothing (not even a branch called `HEAD`).
pub(crate) fn is_current(head: Option<&Head>, name: &str) -> bool {
    matches!(head, Some(Head::Branch { name: current, .. }) if current == name)
}

/// Pushing a tag needs a remote to push it to; the menus hide the item without
/// one. `remotes` are the configured remote names.
pub(crate) fn can_push_tag(remotes: &[String]) -> bool {
    !remotes.is_empty()
}
