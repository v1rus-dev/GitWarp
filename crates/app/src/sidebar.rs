//! The sidebar model: the six sections, their expansion state, the filter and
//! the tree rows built from the repository's data.
//!
//! [`sidebar_nodes`] is a pure function of the loaded data, the expansion
//! overrides and the filter. [`RepositoryView`](crate::repo_view::RepositoryView)
//! only calls it while rendering; git never runs here.

use std::cmp::{Ordering, Reverse};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use forge::ServiceKind;
use git_core::{
    Git, Head, Oid, Operation, Ref, RefKind, Repo, Stash, Submodule, SubmoduleState, Worktree,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, ElementId, FocusHandle, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, RenderOnce, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use ui::actions::{Cancel, Confirm, SelectNext, SelectPrev, TreeDelete};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_SM, FS_XS, R_SM};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{Button, Chip, CountPill, Icon, IconName, Skeleton, Tooltip, TreeNodeData};

use crate::remote_accounts::{self, RemoteDisplay, RemoteTargets};
use crate::web_links::{self, HostLink};
use crate::worktree_glyphs::{main_worktree_tip, worktree_glyphs};
use crate::worktrees_model::WorktreeEntry;

/// The six sidebar sections, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Local,
    Remotes,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
}

impl Section {
    /// The key prefix of a section's tree node.
    fn key(self) -> &'static str {
        match self {
            Section::Local => "section:local",
            Section::Remotes => "section:remotes",
            Section::Tags => "section:tags",
            Section::Stashes => "section:stashes",
            Section::Worktrees => "section:worktrees",
            Section::Submodules => "section:submodules",
        }
    }
}

/// A section or remote with more direct children than this starts collapsed:
/// the tree is not virtualized, so a huge repository would otherwise build
/// every row on the first paint.
const MAX_DIRECT_CHILDREN: usize = 200;

/// The expansion overrides over the defaults. Sections, folders and remotes
/// start expanded (a section or remote with more than
/// [`MAX_DIRECT_CHILDREN`] direct children starts collapsed); a toggle stores
/// only the keys the user changed, so clearing the filter never loses them.
#[derive(Clone, Debug, Default)]
pub struct Expansion {
    overrides: HashMap<SharedString, bool>,
}

impl Expansion {
    /// Whether `key` is expanded: the recorded override, else `default`.
    fn effective(&self, key: &str, default: bool) -> bool {
        self.overrides.get(key).copied().unwrap_or(default)
    }

    /// Flips the effective state of `key` and records it.
    pub fn toggle(&mut self, key: &str, current: bool) {
        self.overrides.insert(key.into(), !current);
    }
}

/// The lists the sidebar loads itself: everything the history view does not
/// already read.
#[derive(Clone, Debug, Default)]
pub struct SidebarExtras {
    pub remotes: Vec<String>,
    /// The fetch URL of every remote, for the Edit remote dialog.
    pub remote_urls: Vec<(String, String)>,
    /// Fetch and push URLs of every remote, for the account chips and menus.
    pub remote_targets: Vec<RemoteTargets>,
    /// The canonical common git dir: the key of the remembered remote accounts,
    /// shared by every worktree of the repository.
    pub common_dir: Option<PathBuf>,
    /// `user.signingkey`, for the tag dialog's signing option.
    pub signing_key: Option<String>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
}

/// Reads the four sidebar lists off the UI thread. One failing list is logged
/// and stays empty, so it never hides the others.
pub(crate) fn read_sidebar_extras(git: Arc<Git>, root: &Path) -> SidebarExtras {
    let repo = match Repo::open(git, root) {
        Ok(repo) => repo,
        Err(error) => {
            log::warn!("could not open the repository for the sidebar: {error}");
            return SidebarExtras::default();
        }
    };
    let remotes = repo.remotes().unwrap_or_else(|error| {
        log::warn!("could not list the remotes: {error}");
        Vec::new()
    });
    let remote_urls: Vec<(String, String)> = remotes
        .iter()
        .filter_map(|name| match repo.remote_url(name) {
            Ok(Some(url)) => Some((name.clone(), url)),
            Ok(None) => None,
            Err(error) => {
                log::warn!("could not read the URL of {name}: {error}");
                None
            }
        })
        .collect();
    let remote_targets = remote_urls
        .iter()
        .map(|(name, fetch_url)| RemoteTargets {
            name: name.clone(),
            fetch_url: fetch_url.clone(),
            push_urls: repo.remote_push_urls(name).unwrap_or_else(|error| {
                log::warn!("could not read the push URLs of {name}: {error}");
                Vec::new()
            }),
        })
        .collect();
    let common_dir = git_core::common_dir(&repo)
        .map_err(|error| log::warn!("could not find the common git dir: {error}"))
        .ok();
    let signing_key = repo.signing_key().unwrap_or_else(|error| {
        log::warn!("could not read the signing key: {error}");
        None
    });
    let stashes = repo.stashes().unwrap_or_else(|error| {
        log::warn!("could not list the stashes: {error}");
        Vec::new()
    });
    let worktrees = repo.worktrees().unwrap_or_else(|error| {
        log::warn!("could not list the worktrees: {error}");
        Vec::new()
    });
    let submodules = repo.submodules().unwrap_or_else(|error| {
        log::warn!("could not list the submodules: {error}");
        Vec::new()
    });
    SidebarExtras {
        remotes,
        remote_urls,
        remote_targets,
        common_dir,
        signing_key,
        stashes,
        worktrees,
        submodules,
    }
}

/// The commit a sidebar leaf reveals in the history, if it has one.
///
/// `local:`, `rbranch:` and `tag:` resolve through the refs, `stash:` through
/// the stashes and `wt:` through the worktrees; everything else (folders,
/// remotes, sections, submodules) and a stale key whose entry is gone has no
/// commit.
pub fn commit_for(key: &str, refs: &[Ref], extras: Option<&SidebarExtras>) -> Option<Oid> {
    if let Some(name) = key.strip_prefix("local:") {
        return refs
            .iter()
            .find(|reference| reference.kind == RefKind::LocalBranch && reference.name == name)
            .map(|reference| reference.target);
    }
    if let Some(name) = key.strip_prefix("rbranch:") {
        return refs
            .iter()
            .find(|reference| reference.kind == RefKind::RemoteBranch && reference.name == name)
            .map(|reference| reference.target);
    }
    if let Some(name) = key.strip_prefix("tag:") {
        return refs
            .iter()
            .find(|reference| reference.kind == RefKind::Tag && reference.name == name)
            .map(|reference| reference.target);
    }
    if let Some(index) = key.strip_prefix("stash:") {
        let index: usize = index.parse().ok()?;
        return extras?
            .stashes
            .iter()
            .find(|stash| stash.index == index)
            .map(|stash| stash.oid);
    }
    if let Some(path) = key.strip_prefix("wt:") {
        return extras?
            .worktrees
            .iter()
            .find(|worktree| worktree.path.display().to_string() == path)
            .and_then(|worktree| worktree.head);
    }
    None
}

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
    pub shortcut: Option<String>,
    pub danger: bool,
    pub enabled: bool,
    pub action: SidebarAction,
}

impl SidebarMenuItem {
    /// A shown-but-disabled item: no later stage has wired it yet.
    fn new(label: impl Into<String>, icon: IconName) -> Self {
        Self {
            label: label.into(),
            icon,
            shortcut: None,
            danger: false,
            enabled: false,
            action: SidebarAction::None,
        }
    }
}

/// One row of a sidebar context menu: an item or a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarMenuEntry {
    Item(SidebarMenuItem),
    Separator,
}

impl SidebarMenuEntry {
    fn shortcut(self, hint: &str) -> Self {
        match self {
            SidebarMenuEntry::Item(mut item) => {
                item.shortcut = Some(hint.to_owned());
                SidebarMenuEntry::Item(item)
            }
            other => other,
        }
    }

    fn danger(self) -> Self {
        match self {
            SidebarMenuEntry::Item(mut item) => {
                item.danger = true;
                SidebarMenuEntry::Item(item)
            }
            other => other,
        }
    }

    /// An item is enabled exactly when it has an action.
    fn action(self, action: SidebarAction) -> Self {
        match self {
            SidebarMenuEntry::Item(mut item) => {
                item.enabled = !matches!(action, SidebarAction::None);
                item.action = action;
                SidebarMenuEntry::Item(item)
            }
            other => other,
        }
    }

    /// [`Self::action`] for an action that is only available in some states.
    fn maybe_action(self, action: Option<SidebarAction>) -> Self {
        match action {
            Some(action) => self.action(action),
            None => self,
        }
    }
}

/// The context menu of a sidebar node, in the design's order, or an empty list
/// for a node that has none (the other sections, folders and unknown keys).
///
/// `head` is the current branch name (`Head::Branch`), or `HEAD` when detached
/// or still loading; `extras` supplies the worktree's lock state; `refs`
/// resolves a tag's commit for "Create branch here…"; `operation` is the one
/// the repository is stopped in (Merge is disabled while there is one).
pub fn sidebar_menu(
    key: &str,
    head: Option<&Head>,
    extras: Option<&SidebarExtras>,
    refs: Option<&[Ref]>,
    operation: Operation,
) -> Vec<SidebarMenuEntry> {
    sidebar_menu_with_links(key, head, extras, refs, &[], operation)
}

/// "Open on <Service>" for `target` on the host of `link`; `None` when the
/// service has no such page.
fn open_item(link: &HostLink, target: &forge::WebTarget) -> Option<SidebarMenuEntry> {
    let url = link.url(target)?;
    Some(
        item(
            &ui::t!("sidebar.menu.open_on_host", service = link.service_name()),
            IconName::External,
        )
        .action(SidebarAction::OpenUrl {
            url,
            host: link.host().to_owned(),
        }),
    )
}

/// [`sidebar_menu`] plus the "Open on <Service>" items of the repository's
/// hosts (`links`, see [`web_links::host_links`]); an empty list hides them.
pub fn sidebar_menu_with_links(
    key: &str,
    head: Option<&Head>,
    extras: Option<&SidebarExtras>,
    refs: Option<&[Ref]>,
    links: &[HostLink],
    operation: Operation,
) -> Vec<SidebarMenuEntry> {
    let targets: &[RemoteTargets] = extras.map_or(&[], |e| &e.remote_targets);
    // The branch's own page on its host; hidden without a host, and for a
    // local branch without an upstream.
    let branch_open = |key: &str| {
        let (link, target) = web_links::branch_target(links, targets, refs.unwrap_or(&[]), key)?;
        open_item(link, &target)
    };
    // Merging needs a branch to merge into: not on a detached or unread HEAD.
    // ... and not while another operation is stopped (git refuses to start one).
    let can_merge = matches!(head, Some(Head::Branch { .. })) && operation == Operation::None;
    let head = match head {
        Some(Head::Branch { name, .. }) => name.as_str(),
        Some(Head::Detached(_)) | None => "HEAD",
    };
    // Stage 7 wires checkout, rename/delete and the stash/remote dialogs; the
    // network (stage 8), worktree (stage 9) and merge/rebase (stage 12) items
    // stay shown-but-disabled.
    if let Some(name) = key.strip_prefix("local:") {
        let current = name == head;
        let mut entries = vec![
            item(&ui::t!("sidebar.menu.checkout"), IconName::Check)
                .shortcut("Enter")
                .maybe_action((!current).then(|| SidebarAction::CheckoutBranch(name.to_owned()))),
            item(
                &ui::t!("sidebar.menu.show_commits_of", name = name),
                IconName::Branch,
            )
            .action(SidebarAction::ShowBranchCommits(name.to_owned())),
            item(
                &ui::t!("sidebar.menu.merge_into", name = name, head = head),
                IconName::Merge,
            )
            .maybe_action((can_merge && !current).then(|| SidebarAction::Merge(name.to_owned()))),
            item(
                &ui::t!("sidebar.menu.rebase_onto", name = name, head = head),
                IconName::Branch,
            ),
            SidebarMenuEntry::Separator,
            item(&ui::t!("sidebar.menu.rename"), IconName::Edit)
                .shortcut("F2")
                .action(SidebarAction::RenameBranch(name.to_owned())),
            item(&ui::t!("sidebar.menu.set_upstream"), IconName::Link)
                .action(SidebarAction::SetUpstream(name.to_owned())),
            item(&ui::t!("sidebar.menu.delete_branch"), IconName::Trash)
                .danger()
                .shortcut("Del")
                .maybe_action((!current).then(|| SidebarAction::DeleteBranch(name.to_owned()))),
            SidebarMenuEntry::Separator,
        ];
        if let Some(open) = branch_open(key) {
            entries.push(open);
            entries.push(SidebarMenuEntry::Separator);
        }
        entries.push(
            item(&ui::t!("sidebar.menu.copy_branch_name"), IconName::Copy)
                .action(SidebarAction::CopyText(name.to_owned())),
        );
        return entries;
    }
    if let Some(name) = key.strip_prefix("rbranch:") {
        let mut entries = vec![
            item(
                &ui::t!("sidebar.menu.checkout_as_local_branch"),
                IconName::Check,
            )
            .shortcut("Enter")
            .action(SidebarAction::CheckoutRemoteBranch(name.to_owned())),
            item(
                &ui::t!("sidebar.menu.show_commits_of", name = name),
                IconName::Branch,
            )
            .action(SidebarAction::ShowBranchCommits(name.to_owned())),
            item(
                &ui::t!("sidebar.menu.merge_into", name = name, head = head),
                IconName::Merge,
            )
            .maybe_action(can_merge.then(|| SidebarAction::Merge(name.to_owned()))),
            SidebarMenuEntry::Separator,
            item(
                &ui::t!("sidebar.menu.delete_remote_branch"),
                IconName::Trash,
            )
            .danger()
            .action(SidebarAction::DeleteRemoteBranch(name.to_owned())),
        ];
        // Set apart from the group above only when there is something to set apart.
        if let Some(open) = branch_open(key) {
            entries.push(SidebarMenuEntry::Separator);
            entries.push(open);
            entries.push(SidebarMenuEntry::Separator);
        }
        entries.push(
            item(&ui::t!("sidebar.menu.copy_branch_name"), IconName::Copy)
                .action(SidebarAction::CopyText(name.to_owned())),
        );
        return entries;
    }
    if let Some(name) = key.strip_prefix("tag:") {
        let target = refs.and_then(|refs| {
            refs.iter()
                .find(|r| r.kind == RefKind::Tag && r.name == name)
                .map(|r| r.target)
        });
        let mut entries = vec![
            item(&ui::t!("sidebar.menu.checkout"), IconName::Check)
                .shortcut("Enter")
                .action(SidebarAction::CheckoutTag(name.to_owned())),
            item(&ui::t!("sidebar.menu.create_branch_here"), IconName::Branch)
                .maybe_action(target.map(SidebarAction::CreateBranchAt)),
            item(&ui::t!("sidebar.menu.push_tag_to_origin"), IconName::Up)
                .action(SidebarAction::PushTag(name.to_owned())),
            SidebarMenuEntry::Separator,
        ];
        // Tags are opened on the first host (the upstream's, else origin's).
        if let Some(open) = links
            .first()
            .and_then(|link| open_item(link, &forge::WebTarget::Tag(name.to_owned())))
        {
            entries.push(open);
            entries.push(SidebarMenuEntry::Separator);
        }
        entries.extend([
            item(&ui::t!("sidebar.menu.rename_tag"), IconName::Edit)
                .shortcut("F2")
                .action(SidebarAction::RenameTag(name.to_owned())),
            item(&ui::t!("sidebar.menu.delete_tag"), IconName::Trash)
                .danger()
                .shortcut("Del")
                .action(SidebarAction::DeleteTag(name.to_owned())),
        ]);
        return entries;
    }
    if let Some(index) = key.strip_prefix("stash:") {
        let index = index.parse::<usize>().ok();
        return vec![
            item(&ui::t!("sidebar.menu.apply_stash"), IconName::Check)
                .maybe_action(index.map(SidebarAction::ApplyStash)),
            item(&ui::t!("sidebar.menu.pop_stash"), IconName::Undo)
                .maybe_action(index.map(SidebarAction::PopStash)),
            item(&ui::t!("sidebar.menu.view_changes"), IconName::Eye)
                .maybe_action(index.map(SidebarAction::ViewStash)),
            SidebarMenuEntry::Separator,
            item(&ui::t!("sidebar.menu.drop_stash"), IconName::Trash)
                .danger()
                .shortcut("Del")
                .maybe_action(index.map(SidebarAction::DropStash)),
        ];
    }
    if let Some(name) = key.strip_prefix("remote:") {
        let multi_push = extras
            .and_then(|extras| extras.remote_targets.iter().find(|t| t.name == name))
            .is_some_and(remote_accounts::is_multi_push);
        let mut entries = vec![
            item(&ui::t!("sidebar.menu.fetch"), IconName::Fetch)
                .action(SidebarAction::FetchRemote(name.to_owned())),
            item(&ui::t!("sidebar.menu.edit_remote"), IconName::Edit)
                .action(SidebarAction::EditRemote(name.to_owned())),
            SidebarMenuEntry::Separator,
            if multi_push {
                item(&ui::t!("sidebar.menu.push_accounts"), IconName::User)
                    .action(SidebarAction::RemotePushAccounts(name.to_owned()))
            } else {
                item(&ui::t!("sidebar.menu.account"), IconName::User)
                    .action(SidebarAction::RemoteAccount(name.to_owned()))
            },
        ];
        // The repository page of the remote's own host.
        let remote_link = links.iter().find(|link| link.fetches(name));
        entries.extend(remote_link.and_then(|link| open_item(link, &forge::WebTarget::Repo)));
        entries.push(SidebarMenuEntry::Separator);
        entries.extend(
            [item(&ui::t!("sidebar.menu.remove_remote"), IconName::Trash)
                .danger()
                .shortcut("Del")
                .action(SidebarAction::RemoveRemote(name.to_owned()))],
        );
        return entries;
    }
    if let Some(path) = key.strip_prefix("wt:") {
        let locked = extras
            .and_then(|extras| {
                extras
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.path.display().to_string() == path)
            })
            .is_some_and(|worktree| worktree.locked.is_some());
        return vec![
            item(&ui::t!("sidebar.menu.open_in_new_window"), IconName::Folder)
                .action(SidebarAction::OpenWorktree(path.to_owned())),
            item(&ui::t!("sidebar.menu.rename_worktree"), IconName::Edit)
                .shortcut("F2")
                .action(SidebarAction::RenameWorktree(path.to_owned())),
            item(&ui::t!("sidebar.menu.manage_worktrees"), IconName::Worktree)
                .action(SidebarAction::ShowWorktrees),
            item(&ui::t!("sidebar.menu.add_worktree"), IconName::Plus)
                .action(SidebarAction::AddWorktree),
            SidebarMenuEntry::Separator,
            item(
                &if locked {
                    ui::t!("sidebar.menu.unlock")
                } else {
                    ui::t!("sidebar.menu.lock")
                },
                IconName::Lock,
            )
            .action(if locked {
                SidebarAction::UnlockWorktree(path.to_owned())
            } else {
                SidebarAction::LockWorktree(path.to_owned())
            }),
            item(&ui::t!("sidebar.menu.remove_worktree"), IconName::Trash)
                .danger()
                .shortcut("Del")
                .action(SidebarAction::RemoveWorktree(path.to_owned())),
        ];
    }
    if let Some(path) = key.strip_prefix("sub:") {
        // A submodule that is not checked out has no folder with a `.git` to open.
        let initialised = extras
            .and_then(|extras| extras.submodules.iter().find(|sub| sub.path == path))
            .is_some_and(|sub| sub.state != SubmoduleState::Uninitialized);
        return vec![
            item(&ui::t!("sidebar.menu.update_submodule"), IconName::Fetch)
                .action(SidebarAction::UpdateSubmodule(path.to_owned())),
            item(&ui::t!("sidebar.menu.open_in_new_window"), IconName::Folder)
                .maybe_action(initialised.then(|| SidebarAction::OpenSubmodule(path.to_owned()))),
        ];
    }
    match key {
        "section:remotes" => vec![
            item(&ui::t!("sidebar.menu.add_remote"), IconName::Plus)
                .action(SidebarAction::AddRemote),
            SidebarMenuEntry::Separator,
            item(&ui::t!("sidebar.menu.fetch_all_remotes"), IconName::Fetch)
                .shortcut("Ctrl+Shift+F")
                .action(SidebarAction::FetchAll),
            item(
                &ui::t!("sidebar.menu.prune_stale_remote_branches"),
                IconName::Trash,
            )
            .action(SidebarAction::FetchPrune),
        ],
        "section:tags" => vec![
            item(&ui::t!("sidebar.menu.create_tag"), IconName::Plus)
                .action(SidebarAction::CreateTag),
            SidebarMenuEntry::Separator,
            item(
                &ui::t!("sidebar.menu.fetch_tags_from_origin"),
                IconName::Fetch,
            )
            .action(SidebarAction::FetchAll),
            item(
                &ui::t!("sidebar.menu.push_all_tags_to_origin"),
                IconName::Up,
            )
            .action(SidebarAction::PushTags),
        ],
        "section:worktrees" => vec![
            item(&ui::t!("sidebar.menu.manage_worktrees"), IconName::Worktree)
                .action(SidebarAction::ShowWorktrees),
            item(&ui::t!("sidebar.menu.add_worktree"), IconName::Plus)
                .action(SidebarAction::AddWorktree),
            SidebarMenuEntry::Separator,
            item(
                &ui::t!("sidebar.menu.prune_missing_worktrees"),
                IconName::Trash,
            )
            .action(SidebarAction::PruneWorktrees),
            item(&ui::t!("sidebar.menu.refresh_list"), IconName::Fetch)
                .action(SidebarAction::Refresh),
        ],
        _ => Vec::new(),
    }
}

/// The action a keyboard shortcut runs on a sidebar row: the enabled menu item
/// whose shortcut hint is `hint` (`Enter`, `F2` or `Del`), so the key and the
/// menu can never disagree. `None` when the row's menu has no such item or it
/// is disabled (e.g. Checkout / Delete on the current branch).
pub fn shortcut_action(
    key: &str,
    hint: &str,
    head: Option<&Head>,
    extras: Option<&SidebarExtras>,
    refs: Option<&[Ref]>,
    operation: Operation,
) -> Option<SidebarAction> {
    sidebar_menu(key, head, extras, refs, operation)
        .into_iter()
        .find_map(|entry| match entry {
            SidebarMenuEntry::Item(item)
                if item.enabled && item.shortcut.as_deref() == Some(hint) =>
            {
                Some(item.action)
            }
            _ => None,
        })
}

/// A disabled menu item, for the compact builders above.
fn item(label: &str, icon: IconName) -> SidebarMenuEntry {
    SidebarMenuEntry::Item(SidebarMenuItem::new(label, icon))
}

/// Adds "Create pull request…" (GitLab: "Create merge request…") to the menu
/// of the local branch `branch`: right after "Open on <Service>" when the menu
/// has one, else in the group above "Copy branch name".
pub fn with_create_pull_request(
    entries: &mut Vec<SidebarMenuEntry>,
    branch: &str,
    kind: forge::ServiceKind,
) {
    let label = match kind {
        forge::ServiceKind::GitLab => ui::t!("sidebar.menu.create_merge_request"),
        _ => ui::t!("sidebar.menu.create_pull_request"),
    };
    let create =
        item(&label, IconName::Pr).action(SidebarAction::CreatePullRequest(branch.to_owned()));
    let open = entries.iter().position(|entry| {
        matches!(
            entry,
            SidebarMenuEntry::Item(SidebarMenuItem {
                action: SidebarAction::OpenUrl { .. },
                ..
            })
        )
    });
    match open {
        Some(index) => entries.insert(index + 1, create),
        // `…, Separator, Copy branch name`: the item joins a group of its own.
        None => {
            let at = entries.len().saturating_sub(1);
            entries.insert(at, SidebarMenuEntry::Separator);
            entries.insert(at, create);
        }
    }
}

/// Builds the visible sidebar rows.
///
/// `refs` is `None` while they load or after a failure: then the ref-based
/// sections show only their headers, without counts. `extras` is the same for
/// the lists of its own: while it is `None` the remotes section counts the
/// distinct prefixes of the remote refs, and the three later sections have no
/// count.
///
/// `filter` is matched trimmed and case-insensitively. With a filter only the
/// leaves that match (and the folders and remotes above them) are built, each
/// section's count becomes its number of matches, and a section without one
/// disappears; an empty result means "no matches at all".
#[cfg(test)]
pub fn sidebar_nodes(
    refs: Option<&[Ref]>,
    head: Option<&Head>,
    extras: Option<&SidebarExtras>,
    expansion: &Expansion,
    filter: &str,
    selected: Option<&str>,
) -> Vec<TreeNodeData> {
    sidebar_nodes_with(
        refs,
        head,
        extras,
        &RemoteDisplays::new(),
        expansion,
        filter,
        selected,
    )
}

/// The icon of a remote row: the GitHub mark, a bordered monogram for the
/// other services, the generic cloud when the service is unknown.
fn service_icon(node: TreeNodeData, service: Option<ServiceKind>) -> TreeNodeData {
    match service {
        Some(ServiceKind::GitHub) => node.icon(IconName::Github),
        Some(kind) => node
            .icon(IconName::Cloud)
            .badge(crate::accounts::monogram(kind)),
        None => node.icon(IconName::Cloud),
    }
}

/// What the sidebar shows per remote, by remote name; see
/// [`remote_accounts::display`].
pub type RemoteDisplays = HashMap<String, RemoteDisplay>;

/// [`sidebar_nodes`] with the account chips, "Add account" rows and push URL
/// rows of the remotes in `remotes`.
pub fn sidebar_nodes_with(
    refs: Option<&[Ref]>,
    head: Option<&Head>,
    extras: Option<&SidebarExtras>,
    remotes: &RemoteDisplays,
    expansion: &Expansion,
    filter: &str,
    selected: Option<&str>,
) -> Vec<TreeNodeData> {
    let filter = filter.trim().to_lowercase();
    let filter = (!filter.is_empty()).then_some(filter.as_str());
    Builder {
        refs,
        head,
        extras,
        remotes,
        expansion,
        filter,
        selected,
    }
    .build()
}

struct Builder<'a> {
    refs: Option<&'a [Ref]>,
    head: Option<&'a Head>,
    extras: Option<&'a SidebarExtras>,
    remotes: &'a RemoteDisplays,
    expansion: &'a Expansion,
    /// Lowercased, trimmed and non-empty; `None` when not filtering.
    filter: Option<&'a str>,
    selected: Option<&'a str>,
}

/// One branch name split on `/`, the shape the local tree is grouped by.
struct BranchEntry<'a> {
    segments: Vec<&'a str>,
    reference: &'a Ref,
}

impl<'a> BranchEntry<'a> {
    fn new(reference: &'a Ref) -> Self {
        Self {
            segments: reference.name.split('/').collect(),
            reference,
        }
    }

    /// The same branch one folder level down.
    fn nested(&self) -> Self {
        Self {
            segments: self.segments[1..].to_vec(),
            reference: self.reference,
        }
    }
}

/// What one level of the local tree contributed.
struct Level {
    /// The rows of this level and their descendants.
    nodes: Vec<TreeNodeData>,
    /// Leaves built at this level and below (the filter's match count).
    leaves: usize,
}

impl Builder<'_> {
    fn build(&self) -> Vec<TreeNodeData> {
        let mut nodes = Vec::new();
        self.local_section(&mut nodes);
        self.remotes_section(&mut nodes);
        self.tags_section(&mut nodes);
        self.stashes_section(&mut nodes);
        self.worktrees_section(&mut nodes);
        self.submodules_section(&mut nodes);
        nodes
    }

    /// Whether `key` renders expanded: the filter forces everything open.
    fn expanded(&self, key: &str, default: bool) -> bool {
        self.filter.is_some() || self.expansion.effective(key, default)
    }

    fn filtering(&self) -> bool {
        self.filter.is_some()
    }

    /// A section header; the caller decided its expansion and count.
    fn section_node(
        &self,
        section: Section,
        label: &str,
        count: Option<usize>,
        expanded: bool,
    ) -> TreeNodeData {
        let mut node = TreeNodeData::section(label.to_owned())
            .key(section.key())
            .has_children(true)
            .expanded(expanded);
        if let Some(count) = count {
            node = node.count(count.to_string());
        }
        node
    }

    fn is_head(&self, name: &str) -> bool {
        matches!(self.head, Some(Head::Branch { name: head, .. }) if head == name)
    }

    // ---- Local branches ------------------------------------------------

    fn local_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let refs = self.refs.unwrap_or(&[]);
        let locals: Vec<&Ref> = refs
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch)
            .collect();
        let entries: Vec<BranchEntry> = locals.iter().map(|r| BranchEntry::new(r)).collect();
        let direct = local_direct_children(&entries);

        if self.filtering() {
            let level = self.local_level(1, &entries, "");
            if level.leaves > 0 {
                nodes.push(self.section_node(
                    Section::Local,
                    &ui::t!("sidebar.section.local"),
                    Some(level.leaves),
                    true,
                ));
                nodes.extend(level.nodes);
            }
            return;
        }

        let expanded = self.expanded(Section::Local.key(), direct <= MAX_DIRECT_CHILDREN);
        let count = self.refs.is_some().then_some(locals.len());
        nodes.push(self.section_node(
            Section::Local,
            &ui::t!("sidebar.section.local"),
            count,
            expanded,
        ));
        // Children of a collapsed section are not built at all.
        if expanded {
            nodes.extend(self.local_level(1, &entries, "").nodes);
        }
    }

    fn local_level(&self, level: usize, entries: &[BranchEntry<'_>], parent: &str) -> Level {
        let mut leaf_entries: Vec<&BranchEntry<'_>> = Vec::new();
        let mut folders: Vec<(String, Vec<BranchEntry<'_>>)> = Vec::new();
        for entry in entries {
            if entry.segments.len() <= 1 {
                leaf_entries.push(entry);
            } else {
                let name = entry.segments[0].to_owned();
                match folders.iter_mut().find(|(candidate, _)| candidate == &name) {
                    Some((_, children)) => children.push(entry.nested()),
                    None => folders.push((name, vec![entry.nested()])),
                }
            }
        }
        // Leaves first, then folders; each group case-insensitively sorted.
        leaf_entries.sort_by(|a, b| {
            leaf_segment(a)
                .to_lowercase()
                .cmp(&leaf_segment(b).to_lowercase())
        });
        folders.sort_by_key(|(name, _)| name.to_lowercase());

        let mut out = Vec::new();
        let mut leaves = 0;

        for entry in leaf_entries {
            if !self.matches(&entry.reference.name) {
                continue;
            }
            out.push(self.local_leaf(level, entry.reference));
            leaves += 1;
        }

        for (name, children) in folders {
            let prefix = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            let key = format!("folder:{prefix}");
            let selected = self.selected == Some(key.as_str());
            if self.filtering() {
                // A folder exists only as the ancestor of a shown leaf.
                let child = self.local_level(level + 1, &children, &prefix);
                if child.leaves == 0 {
                    continue;
                }
                out.push(
                    TreeNodeData::new(name)
                        .key(key)
                        .level(level)
                        .icon(IconName::Folder)
                        .has_children(true)
                        .expanded(true)
                        .selected(selected),
                );
                out.extend(child.nodes);
                leaves += child.leaves;
                continue;
            }
            let expanded = self.expanded(&key, true);
            out.push(
                TreeNodeData::new(name)
                    .key(key)
                    .level(level)
                    .icon(IconName::Folder)
                    .has_children(true)
                    .expanded(expanded)
                    .selected(selected),
            );
            if expanded {
                out.extend(self.local_level(level + 1, &children, &prefix).nodes);
            }
        }

        Level { nodes: out, leaves }
    }

    fn local_leaf(&self, level: usize, reference: &Ref) -> TreeNodeData {
        let key = format!("local:{}", reference.name);
        let selected = self.selected == Some(key.as_str());
        let label = reference.name.rsplit('/').next().unwrap_or("");
        let mut node = TreeNodeData::new(label.to_owned())
            .key(key)
            .level(level)
            .icon(IconName::Branch)
            .selected(selected);
        if self.is_head(&reference.name) {
            node = node.current();
        }
        tracking(node, reference)
    }

    // ---- Remotes -------------------------------------------------------

    fn remotes_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let refs = self.refs.unwrap_or(&[]);
        // The configured remotes once they load; before that, the distinct
        // prefixes of the remote refs.
        let names: Vec<String> = match self.extras {
            Some(extras) => extras.remotes.clone(),
            None => distinct_prefixes(refs),
        };
        let count = self.refs.is_some().then_some(names.len());

        let mut built = Vec::new();
        let mut matches = 0;
        for name in &names {
            let mut branches: Vec<(&str, &Ref)> = refs
                .iter()
                .filter(|r| r.kind == RefKind::RemoteBranch)
                .filter_map(|r| {
                    let label = r.name.strip_prefix(name.as_str())?.strip_prefix('/')?;
                    // `<remote>/HEAD` is a symbolic duplicate, not a branch.
                    (!label.is_empty() && label != "HEAD").then_some((label, r))
                })
                .collect();
            branches.sort_by_key(|(label, _)| label.to_lowercase());
            let key = format!("remote:{name}");

            if self.filtering() {
                let mut children = Vec::new();
                for (label, reference) in branches {
                    if self.matches_remote(name, label) {
                        children.push(self.remote_branch(reference, label));
                    }
                }
                if children.is_empty() {
                    continue;
                }
                matches += children.len();
                built.push(self.remote_node(name, &key, true));
                built.extend(children);
                continue;
            }

            let expanded = self.expanded(&key, branches.len() <= MAX_DIRECT_CHILDREN);
            // A remote with no branches is still shown, just empty.
            built.push(self.remote_node(name, &key, expanded));
            if expanded {
                built.extend(self.remote_account_rows(name));
                for (label, reference) in branches {
                    built.push(self.remote_branch(reference, label));
                }
            }
        }

        if self.filtering() {
            if matches > 0 {
                nodes.push(self.section_node(
                    Section::Remotes,
                    &ui::t!("sidebar.section.remotes"),
                    Some(matches),
                    true,
                ));
                nodes.extend(built);
            }
            return;
        }
        let expanded = self.expanded(Section::Remotes.key(), names.len() <= MAX_DIRECT_CHILDREN);
        nodes.push(self.section_node(
            Section::Remotes,
            &ui::t!("sidebar.section.remotes"),
            count,
            expanded,
        ));
        if expanded {
            nodes.extend(built);
        }
    }

    fn remote_node(&self, name: &str, key: &str, expanded: bool) -> TreeNodeData {
        let display = self.remotes.get(name);
        let node = TreeNodeData::new(name.to_owned())
            .key(key.to_owned())
            .level(1)
            .has_children(true)
            .expanded(expanded)
            .selected(self.selected == Some(key));
        let mut node = service_icon(node, display.and_then(|d| d.service));
        if let Some(chip) = display.and_then(|d| d.chip.as_ref()) {
            node = node.chip(
                chip.text.clone(),
                chip.warn,
                Some(chip.tooltip.clone().into()),
            );
        }
        if let Some(tooltip) = display.and_then(|d| d.tooltip.as_ref()) {
            node = node.tooltip(tooltip.clone());
        }
        node
    }

    /// The rows between a remote and its branches: "Add account for <host>",
    /// or the `fetch →` and `push →` rows of a multi-push remote.
    fn remote_account_rows(&self, name: &str) -> Vec<TreeNodeData> {
        let Some(display) = self.remotes.get(name) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if let Some(add) = &display.add_account {
            let key = format!("addacc:{name}");
            rows.push(
                TreeNodeData::new(add.label.clone())
                    .key(key.clone())
                    .level(2)
                    .icon(IconName::Plus)
                    .accent()
                    .tooltip(add.tooltip.clone())
                    .selected(self.selected == Some(key.as_str())),
            );
        }
        let account_row = |key: String, row: &remote_accounts::PushRow| {
            let node = TreeNodeData::new(row.label.clone())
                .key(key.clone())
                .level(2)
                .selected(self.selected == Some(key.as_str()));
            let mut node = service_icon(node, row.service);
            if let Some(chip) = &row.chip {
                node = node.chip(
                    chip.text.clone(),
                    chip.warn,
                    Some(chip.tooltip.clone().into()),
                );
            }
            node
        };
        if let Some(fetch) = &display.fetch {
            rows.push(account_row(format!("fetchurl:{name}"), fetch));
        }
        for (index, push) in display.pushes.iter().enumerate() {
            rows.push(account_row(format!("pushurl:{name}:{index}"), push));
        }
        rows
    }

    fn remote_branch(&self, reference: &Ref, label: &str) -> TreeNodeData {
        let key = format!("rbranch:{}", reference.name);
        TreeNodeData::new(label.to_owned())
            .key(key.clone())
            .level(2)
            .icon(IconName::Branch)
            .selected(self.selected == Some(key.as_str()))
    }

    // ---- Tags ----------------------------------------------------------

    fn tags_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let refs = self.refs.unwrap_or(&[]);
        let tags: Vec<&Ref> = refs.iter().filter(|r| r.kind == RefKind::Tag).collect();

        if self.filtering() {
            let mut matched: Vec<&Ref> = tags
                .into_iter()
                .filter(|reference| self.matches(&reference.name))
                .collect();
            matched.sort_by_cached_key(|reference| Reverse(natural_key(&reference.name)));
            if !matched.is_empty() {
                let count = matched.len();
                nodes.push(self.section_node(
                    Section::Tags,
                    &ui::t!("sidebar.section.tags"),
                    Some(count),
                    true,
                ));
                nodes.extend(
                    matched
                        .into_iter()
                        .map(|reference| self.tag_node(reference)),
                );
            }
            return;
        }

        let expanded = self.expanded(Section::Tags.key(), tags.len() <= MAX_DIRECT_CHILDREN);
        let count = self.refs.is_some().then_some(tags.len());
        nodes.push(self.section_node(
            Section::Tags,
            &ui::t!("sidebar.section.tags"),
            count,
            expanded,
        ));
        // Children of a collapsed section are not built, and the sort only
        // runs when they are needed.
        if expanded {
            let mut tags = tags;
            // Newest version first; the version runs compare numerically.
            tags.sort_by_cached_key(|reference| Reverse(natural_key(&reference.name)));
            nodes.extend(tags.into_iter().map(|reference| self.tag_node(reference)));
        }
    }

    fn tag_node(&self, reference: &Ref) -> TreeNodeData {
        let key = format!("tag:{}", reference.name);
        TreeNodeData::new(reference.name.clone())
            .key(key.clone())
            .level(1)
            .icon(IconName::Tag)
            .selected(self.selected == Some(key.as_str()))
    }

    // ---- Stashes -------------------------------------------------------

    fn stashes_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let stashes: &[Stash] = self.extras.map(|e| e.stashes.as_slice()).unwrap_or(&[]);

        if self.filtering() {
            let mut matched: Vec<(&Stash, String)> = stashes
                .iter()
                .map(|stash| (stash, stash_label(&stash.message)))
                .filter(|(stash, label)| {
                    self.matches(label) || self.matches(&format!("stash@{{{}}}", stash.index))
                })
                .collect();
            matched.sort_by_key(|(stash, _)| stash.index);
            if !matched.is_empty() {
                let count = matched.len();
                nodes.push(self.section_node(
                    Section::Stashes,
                    &ui::t!("sidebar.section.stashes"),
                    Some(count),
                    true,
                ));
                nodes.extend(
                    matched
                        .into_iter()
                        .map(|(stash, label)| self.stash_node(stash, label)),
                );
            }
            return;
        }

        let expanded = self.expanded(Section::Stashes.key(), stashes.len() <= MAX_DIRECT_CHILDREN);
        let count = self.extras.map(|_| stashes.len());
        nodes.push(self.section_node(
            Section::Stashes,
            &ui::t!("sidebar.section.stashes"),
            count,
            expanded,
        ));
        if expanded {
            let mut sorted: Vec<&Stash> = stashes.iter().collect();
            sorted.sort_by_key(|stash| stash.index);
            nodes.extend(
                sorted
                    .into_iter()
                    .map(|stash| self.stash_node(stash, stash_label(&stash.message))),
            );
        }
    }

    fn stash_node(&self, stash: &Stash, label: String) -> TreeNodeData {
        let key = format!("stash:{}", stash.index);
        TreeNodeData::new(label)
            .key(key.clone())
            .level(1)
            .icon(IconName::Stash)
            .selected(self.selected == Some(key.as_str()))
    }

    // ---- Worktrees -----------------------------------------------------

    fn worktrees_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let worktrees: &[Worktree] = self.extras.map(|e| e.worktrees.as_slice()).unwrap_or(&[]);

        if self.filtering() {
            let matched: Vec<&Worktree> = worktrees
                .iter()
                .filter(|worktree| {
                    self.matches(&path_label(&worktree.path))
                        || self.matches(&worktree.path.display().to_string())
                })
                .collect();
            if !matched.is_empty() {
                let count = matched.len();
                nodes.push(self.section_node(
                    Section::Worktrees,
                    &ui::t!("sidebar.section.worktrees"),
                    Some(count),
                    true,
                ));
                nodes.extend(
                    matched
                        .into_iter()
                        .map(|worktree| self.worktree_node(worktree)),
                );
            }
            return;
        }

        let expanded = self.expanded(
            Section::Worktrees.key(),
            worktrees.len() <= MAX_DIRECT_CHILDREN,
        );
        let count = self.extras.map(|_| worktrees.len());
        nodes.push(self.section_node(
            Section::Worktrees,
            &ui::t!("sidebar.section.worktrees"),
            count,
            expanded,
        ));
        if expanded {
            nodes.extend(
                worktrees
                    .iter()
                    .map(|worktree| self.worktree_node(worktree)),
            );
        }
    }

    fn worktree_node(&self, worktree: &Worktree) -> TreeNodeData {
        let label = path_label(&worktree.path);
        let path = worktree.path.display().to_string();
        let key = format!("wt:{path}");
        let mut node = TreeNodeData::new(label)
            .key(key.clone())
            .level(1)
            .icon(IconName::Worktree)
            .selected(self.selected == Some(key.as_str()));
        if worktree.current {
            node = node.current();
        }
        if let Some(state) = worktree_state(worktree) {
            node = node.state(state);
        }
        node
    }

    // ---- Submodules ----------------------------------------------------

    fn submodules_section(&self, nodes: &mut Vec<TreeNodeData>) {
        let submodules: &[Submodule] = self.extras.map(|e| e.submodules.as_slice()).unwrap_or(&[]);

        if self.filtering() {
            let matched: Vec<&Submodule> = submodules
                .iter()
                .filter(|submodule| self.matches(&submodule.path))
                .collect();
            if !matched.is_empty() {
                let count = matched.len();
                nodes.push(self.section_node(
                    Section::Submodules,
                    &ui::t!("sidebar.section.submodules"),
                    Some(count),
                    true,
                ));
                nodes.extend(
                    matched
                        .into_iter()
                        .map(|submodule| self.submodule_node(submodule)),
                );
            }
            return;
        }

        let expanded = self.expanded(
            Section::Submodules.key(),
            submodules.len() <= MAX_DIRECT_CHILDREN,
        );
        let count = self.extras.map(|_| submodules.len());
        nodes.push(self.section_node(
            Section::Submodules,
            &ui::t!("sidebar.section.submodules"),
            count,
            expanded,
        ));
        if expanded {
            nodes.extend(
                submodules
                    .iter()
                    .map(|submodule| self.submodule_node(submodule)),
            );
        }
    }

    fn submodule_node(&self, submodule: &Submodule) -> TreeNodeData {
        let key = format!("sub:{}", submodule.path);
        let mut node = TreeNodeData::new(submodule.path.clone())
            .key(key.clone())
            .level(1)
            .icon(IconName::Box)
            .selected(self.selected == Some(key.as_str()));
        if let Some(state) = submodule_state(submodule.state) {
            node = node.state(state);
        }
        node
    }

    // ---- Matching ------------------------------------------------------

    /// Case-insensitive substring match; everything matches without a filter.
    fn matches(&self, text: &str) -> bool {
        match self.filter {
            None => true,
            Some(filter) => text.to_lowercase().contains(filter),
        }
    }

    fn matches_remote(&self, remote: &str, branch: &str) -> bool {
        match self.filter {
            None => true,
            Some(filter) => {
                branch.to_lowercase().contains(filter)
                    || format!("{remote}/{branch}").to_lowercase().contains(filter)
            }
        }
    }
}

/// The label of the branch entry at its own level.
fn leaf_segment<'a>(entry: &BranchEntry<'a>) -> &'a str {
    entry.segments.last().copied().unwrap_or("")
}

/// The number of rows directly under the local-branches header: root leaves
/// plus the root folders.
fn local_direct_children(entries: &[BranchEntry<'_>]) -> usize {
    let mut leaves = 0;
    let mut folders: Vec<&str> = Vec::new();
    for entry in entries {
        if entry.segments.len() <= 1 {
            leaves += 1;
        } else if !folders.contains(&entry.segments[0]) {
            folders.push(entry.segments[0]);
        }
    }
    leaves + folders.len()
}

/// The distinct `<remote>` prefixes of the remote refs, in their order.
fn distinct_prefixes(refs: &[Ref]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for reference in refs.iter().filter(|r| r.kind == RefKind::RemoteBranch) {
        let Some((prefix, _)) = reference.name.split_once('/') else {
            continue;
        };
        if !names.iter().any(|name| name == prefix) {
            names.push(prefix.to_owned());
        }
    }
    names
}

/// The ahead/behind arrows, or `gone` when the upstream disappeared.
fn tracking(node: TreeNodeData, reference: &Ref) -> TreeNodeData {
    if reference.upstream_gone {
        return node.state(ui::t!("sidebar.state.gone"));
    }
    let mut node = node;
    if reference.ahead > 0 {
        node = node.ahead(reference.ahead);
    }
    if reference.behind > 0 {
        node = node.behind(reference.behind);
    }
    node
}

/// The message a stash row shows:
/// `WIP on <branch>: <sha> <subject>` → `WIP on <branch>`,
/// `On <branch>: <message>` → `<message>`, anything else unchanged.
fn stash_label(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("WIP on ") {
        return match rest.split_once(':') {
            Some((branch, _)) => ui::t!("sidebar.stash.wip_on", branch = branch).to_string(),
            None => message.to_owned(),
        };
    }
    if let Some(rest) = message.strip_prefix("On ") {
        if let Some((_, text)) = rest.split_once(':') {
            return text.trim_start().to_owned();
        }
    }
    message.to_owned()
}

fn worktree_state(worktree: &Worktree) -> Option<String> {
    if worktree.prunable {
        Some(ui::t!("sidebar.state.missing").to_string())
    } else if worktree.locked.is_some() {
        Some(ui::t!("sidebar.state.locked").to_string())
    } else if worktree.bare {
        Some(ui::t!("sidebar.state.bare").to_string())
    } else if worktree.detached {
        Some(ui::t!("sidebar.state.detached").to_string())
    } else {
        worktree.branch.clone()
    }
}

fn submodule_state(state: SubmoduleState) -> Option<SharedString> {
    match state {
        SubmoduleState::Modified => Some(ui::t!("sidebar.state.modified")),
        SubmoduleState::Uninitialized => Some(ui::t!("sidebar.state.not_initialized")),
        SubmoduleState::Conflict => Some(ui::t!("sidebar.state.conflict")),
        SubmoduleState::Clean => None,
    }
}

/// The last path component, or the whole path when there is none.
fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One natural-sort run of a tag name, precomputed once per tag so
/// `sort_by_cached_key` never allocates while comparing.
#[derive(Debug, PartialEq, Eq)]
enum TagRun {
    /// A digit run: the value without leading zeros, and the lowercased text
    /// for the mixed digit/text comparison.
    Num { value: String, text: String },
    /// A non-digit run, lowercased.
    Text(String),
}

impl TagRun {
    fn text(&self) -> &str {
        match self {
            TagRun::Num { text, .. } | TagRun::Text(text) => text,
        }
    }
}

impl Ord for TagRun {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (TagRun::Num { value: a, .. }, TagRun::Num { value: b, .. }) => {
                // More digits is a larger number; equal lengths compare
                // lexicographically.
                a.len().cmp(&b.len()).then_with(|| a.cmp(b))
            }
            // Every other pair compares as lowercased text, exactly like the
            // old per-comparison comparison.
            _ => self.text().cmp(other.text()),
        }
    }
}

impl PartialOrd for TagRun {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The cached sort key of a tag name: alternating digit and non-digit runs.
fn natural_key(name: &str) -> Vec<TagRun> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut previous: Option<bool> = None;
    for (index, ch) in name.char_indices() {
        let digit = ch.is_ascii_digit();
        if let Some(previous) = previous {
            if previous != digit {
                out.push(TagRun::new(&name[start..index]));
                start = index;
            }
        }
        previous = Some(digit);
    }
    if start < name.len() {
        out.push(TagRun::new(&name[start..]));
    }
    out
}

impl TagRun {
    fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        if text.bytes().all(|byte| byte.is_ascii_digit()) {
            TagRun::Num {
                value: text.trim_start_matches('0').to_owned(),
                text: lower,
            }
        } else {
            TagRun::Text(lower)
        }
    }
}

// ----- the Worktrees view's panel -----

/// The rows of the worktree panel: those whose folder name or branch contain
/// `query` (case-insensitive), the main worktree pinned first and the rest by
/// recency (the model's order breaks ties).
pub fn worktree_panel_rows<'a>(
    entries: &'a [WorktreeEntry],
    query: &str,
) -> Vec<&'a WorktreeEntry> {
    let query = query.trim().to_lowercase();
    let mut rows: Vec<&WorktreeEntry> = entries
        .iter()
        .filter(|entry| {
            query.is_empty()
                || format!("{} {}", entry.name(), entry.branch().unwrap_or(""))
                    .to_lowercase()
                    .contains(&query)
        })
        .collect();
    // `sort_by_key` is stable: equal recency keeps the model's order.
    rows.sort_by_key(|entry| (!entry.is_main(), Reverse(entry.last_used.unwrap_or(0))));
    rows
}

/// What a worktree's context menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeAction {
    Switch,
    OpenInWindow,
    Reveal,
    CopyPath,
    ToggleLock,
    Remove,
}

/// One item of a worktree's context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeMenuItem {
    pub action: WorktreeAction,
    pub label: SharedString,
    pub icon: IconName,
    pub disabled: bool,
    pub danger: bool,
}

/// The context menu of `entry`; `None` is the separator. The disabled rules
/// are the design's: switching needs another, existing folder; locking and
/// removing never apply to the main worktree, locking not to a missing one.
pub fn worktree_menu(entry: &WorktreeEntry) -> Vec<Option<WorktreeMenuItem>> {
    let item = |action, label, icon, disabled, danger| {
        Some(WorktreeMenuItem {
            action,
            label,
            icon,
            disabled,
            danger,
        })
    };
    vec![
        item(
            WorktreeAction::Switch,
            ui::t!("sidebar.worktree_menu.switch"),
            IconName::Worktree,
            entry.is_current() || entry.missing,
            false,
        ),
        item(
            WorktreeAction::OpenInWindow,
            ui::t!("sidebar.worktree_menu.open_in_window"),
            IconName::Sidebar,
            entry.missing,
            false,
        ),
        item(
            WorktreeAction::Reveal,
            ui::t!("sidebar.worktree_menu.reveal"),
            IconName::Folder,
            entry.missing,
            false,
        ),
        item(
            WorktreeAction::CopyPath,
            ui::t!("sidebar.worktree_menu.copy_path"),
            IconName::Copy,
            false,
            false,
        ),
        None,
        item(
            WorktreeAction::ToggleLock,
            if entry.locked().is_some() {
                ui::t!("sidebar.worktree_menu.unlock")
            } else {
                ui::t!("sidebar.worktree_menu.lock")
            },
            IconName::Lock,
            entry.is_main() || entry.missing,
            false,
        ),
        item(
            WorktreeAction::Remove,
            ui::t!("sidebar.worktree_menu.remove"),
            IconName::Trash,
            entry.is_main(),
            true,
        ),
    ]
}

/// What the panel reports to its owner.
#[derive(Clone, Debug)]
pub enum PanelEvent {
    /// A click: show this worktree in the detail pane.
    Select(PathBuf),
    /// A double-click: switch to this worktree.
    Switch(PathBuf),
    /// A right-click, with the pointer position.
    Menu(PathBuf, Point<Pixels>),
    /// ↓ / ↑: move the selection.
    Move(isize),
    /// Enter: switch to the selected worktree.
    Confirm,
    /// Del: remove the selected worktree.
    Delete,
    /// Esc: leave the Worktrees view.
    Leave,
    /// "New worktree…".
    NewWorktree,
}

/// What the panel shows instead of rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelState {
    Loading,
    Error,
    Ready,
}

type PanelHandler = Rc<dyn Fn(PanelEvent, &mut Window, &mut App) + 'static>;

/// The worktree list that replaces the sidebar tree in the Worktrees view.
#[derive(IntoElement)]
pub struct WorktreePanel {
    state: PanelState,
    /// The visible rows, already filtered and ordered.
    rows: Vec<WorktreeEntry>,
    /// Every worktree, for the header count.
    total: usize,
    query: String,
    selected: Option<PathBuf>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    on_event: PanelHandler,
}

impl WorktreePanel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: PanelState,
        rows: Vec<WorktreeEntry>,
        total: usize,
        query: String,
        selected: Option<PathBuf>,
        focus: FocusHandle,
        scroll: ScrollHandle,
        on_event: impl Fn(PanelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            state,
            rows,
            total,
            query,
            selected,
            focus,
            scroll,
            on_event: Rc::new(on_event),
        }
    }
}

impl RenderOnce for WorktreePanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let ring_visible = ui::focus::focus_visible(&self.focus, window, cx);

        let mut root = div()
            .id("worktree-panel")
            .debug_selector(|| "worktree-panel".to_string())
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // `ui-tree` carries the Delete binding; ←/→/Space/F2 are not
            // handled here and do nothing.
            .key_context("ui-tree")
            .track_focus(&self.focus);
        {
            let on_event = self.on_event.clone();
            root = root.on_action(move |_: &SelectNext, window: &mut Window, cx: &mut App| {
                on_event(PanelEvent::Move(1), window, cx)
            });
        }
        {
            let on_event = self.on_event.clone();
            root = root.on_action(move |_: &SelectPrev, window: &mut Window, cx: &mut App| {
                on_event(PanelEvent::Move(-1), window, cx)
            });
        }
        {
            let on_event = self.on_event.clone();
            root = root.on_action(move |_: &Confirm, window: &mut Window, cx: &mut App| {
                on_event(PanelEvent::Confirm, window, cx)
            });
        }
        {
            let on_event = self.on_event.clone();
            root = root.on_action(move |_: &TreeDelete, window: &mut Window, cx: &mut App| {
                on_event(PanelEvent::Delete, window, cx)
            });
        }
        {
            let on_event = self.on_event.clone();
            root = root.on_action(move |_: &Cancel, window: &mut Window, cx: &mut App| {
                on_event(PanelEvent::Leave, window, cx)
            });
        }

        // Header: WORKTREES + count.
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(10.))
            .pb(px(4.))
            .text_size(FS_XS)
            .font_weight(FontWeight::BOLD)
            .text_color(colors.muted)
            .child(ui::t!("sidebar.panel.header"));
        if self.state == PanelState::Ready {
            header = header.child(CountPill::new(self.total.to_string()));
        }
        // Design: while loading or failed only the skeleton / message shows,
        // no WORKTREES header and no "New worktree…" button.
        if self.state == PanelState::Ready {
            root = root.child(header);
        }

        let message = |text: SharedString| {
            div()
                .px(px(12.))
                .py(px(16.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(text)
        };
        let body = match self.state {
            PanelState::Loading => div()
                .id("worktree-panel-loading")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .px(px(12.))
                .py(px(16.))
                .children(
                    [180., 140., 170., 150.]
                        .into_iter()
                        .enumerate()
                        .map(|(i, w)| Skeleton::new(("worktree-panel-skeleton", i), px(w))),
                )
                .into_any_element(),
            PanelState::Error => div()
                .flex_1()
                .min_h_0()
                .child(message(ui::t!("sidebar.panel.error")))
                .into_any_element(),
            PanelState::Ready if self.rows.is_empty() => div()
                .flex_1()
                .min_h_0()
                .child(message(ui::t!(
                    "sidebar.panel.no_match",
                    query = self.query.trim()
                )))
                .into_any_element(),
            PanelState::Ready => {
                let mut list = div()
                    .id("worktree-panel-list")
                    // Shrinks to scroll but does not grow: "New worktree…"
                    // sits right under the rows (design `.add-row`).
                    .flex_initial()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll);
                let pinned = self.rows.first().is_some_and(WorktreeEntry::is_main);
                for (index, entry) in self.rows.iter().enumerate() {
                    let selected = self.selected.as_deref() == Some(entry.path());
                    let row = worktree_row(
                        index,
                        entry,
                        selected,
                        selected && ring_visible,
                        &self.focus,
                        &self.on_event,
                        colors,
                        cx,
                    );
                    // The divider below the pinned main worktree lives in the
                    // same list child, so scroll-to-item indexes stay rows.
                    let divider = pinned && index == 0 && self.rows.len() > 1;
                    list =
                        list.child(div().flex_none().flex().flex_col().child(row).when(
                            divider,
                            |this| {
                                this.child(div().h(px(1.)).mx(px(14.)).my(px(4.)).bg(colors.border))
                            },
                        ));
                }
                list.into_any_element()
            }
        };
        root = root.child(body);

        // "New worktree…" focuses the quick-create input of the centre pane.
        if self.state != PanelState::Ready {
            return root;
        }
        let on_event = self.on_event.clone();
        root.child(
            div().flex_none().px(px(8.)).pt(px(6.)).pb(px(8.)).child(
                div().w_full().child(
                    Button::new("worktree-panel-new")
                        .icon(IconName::Plus)
                        .label(ui::t!("sidebar.panel.new"))
                        .on_click(move |_, window, cx| {
                            on_event(PanelEvent::NewWorktree, window, cx)
                        }),
                ),
            ),
        )
    }
}

#[allow(clippy::too_many_arguments)]
/// One two-line row: name (and the "default" chip) over the branch, status
/// glyphs at the end. The current worktree has the accent bar and a bold
/// name; the selected one the selection background; the two are independent.
fn worktree_row(
    index: usize,
    entry: &WorktreeEntry,
    selected: bool,
    ring: bool,
    focus: &FocusHandle,
    on_event: &PanelHandler,
    colors: ThemeColors,
    cx: &App,
) -> impl IntoElement {
    let main = entry.is_main();
    let current = entry.is_current();
    let missing = entry.missing;
    let path = entry.path().to_path_buf();
    let foreground = if selected {
        colors.selection_fg
    } else if missing {
        colors.muted
    } else {
        colors.text
    };
    let secondary = if selected {
        colors.selection_sub
    } else {
        colors.muted
    };
    let icon_color = if selected {
        if main {
            colors.selection_sub
        } else {
            colors.selection_fg
        }
    } else if main || current {
        colors.accent
    } else {
        colors.muted
    };

    let mut name = div()
        .min_w_0()
        .truncate()
        .font_weight(if current {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        })
        .child(entry.name());
    if missing {
        name = name.line_through();
    }
    let branch = div()
        .min_w_0()
        .truncate()
        .font_family(FONT_MONO)
        .text_size(FS_SM)
        .text_color(secondary)
        .child(match entry.branch() {
            Some(branch) => SharedString::from(branch.to_owned()),
            None => ui::t!("sidebar.panel.detached_head"),
        });

    let id = format!("worktree-row-{index}");
    let mut row = div()
        .id(ElementId::named_usize("worktree-row", index))
        .debug_selector({
            let id = id.clone();
            move || id.clone()
        })
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .min_h(px(40.))
        .mx(px(6.))
        .px(px(8.))
        .py(px(4.))
        .rounded(R_SM)
        .font_family(FONT_UI)
        .text_size(FS_BASE)
        .text_color(foreground)
        .child(
            div().flex_none().child(
                Icon::new(if main {
                    IconName::Git
                } else {
                    IconName::Worktree
                })
                .size(px(14.))
                .color(icon_color),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .min_w_0()
                        .child(name)
                        .when(main, |this| {
                            this.child(div().flex_none().child(Chip::new(
                                ElementId::named_usize("worktree-row-default", index),
                                ui::t!("sidebar.panel.default"),
                            )))
                        }),
                )
                .child(branch),
        )
        .child(worktree_glyphs(&id, entry, selected, colors));

    if main {
        row = row.tooltip(Tooltip::text(main_worktree_tip()));
    }
    if current {
        row = row.child(
            div()
                .absolute()
                .left(px(-4.))
                .top(px(4.))
                .bottom(px(4.))
                .w(px(2.))
                .rounded(px(1.))
                .bg(colors.accent),
        );
    }
    if selected {
        row = row.bg(colors.selection);
    } else {
        let hover = colors.hover;
        row = row.hover(move |style| style.bg(hover));
    }
    if ring {
        row = row.child(ui::focus::focus_ring(
            ui::focus::RingOffset::Inset2,
            R_SM,
            cx,
        ));
    }
    {
        let on_event = on_event.clone();
        let focus = focus.clone();
        let path = path.clone();
        row = row.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                // The panel keeps the keyboard even when a menu or dialog
                // opens from the handler.
                window.focus(&focus, cx);
                if event.click_count >= 2 {
                    on_event(PanelEvent::Switch(path.clone()), window, cx);
                } else {
                    on_event(PanelEvent::Select(path.clone()), window, cx);
                }
                window.prevent_default();
            },
        );
    }
    {
        let on_event = on_event.clone();
        let focus = focus.clone();
        row = row.on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                window.focus(&focus, cx);
                on_event(PanelEvent::Menu(path.clone(), event.position), window, cx);
                window.prevent_default();
            },
        );
    }
    row
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use git_core::Oid;
    use ui::TreeNodeTrailing;

    use super::*;

    fn reference(kind: RefKind, name: &str) -> Ref {
        Ref {
            full_name: format!("refs/{name}"),
            name: name.to_owned(),
            kind,
            target: Oid::from_hex(&"1".repeat(40)).unwrap(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn oid() -> Oid {
        Oid::from_hex(&"2".repeat(40)).unwrap()
    }

    fn build_nodes(
        refs: &[Ref],
        extras: Option<&SidebarExtras>,
        head: Option<&Head>,
        expansion: &Expansion,
        filter: &str,
    ) -> Vec<TreeNodeData> {
        sidebar_nodes(Some(refs), head, extras, expansion, filter, None)
    }

    fn keys(nodes: &[TreeNodeData]) -> Vec<&str> {
        nodes.iter().map(|node| node.key.as_ref()).collect()
    }

    fn labels(nodes: &[TreeNodeData]) -> Vec<&str> {
        nodes.iter().map(|node| node.label.as_ref()).collect()
    }

    fn find<'a>(nodes: &'a [TreeNodeData], key: &str) -> Option<&'a TreeNodeData> {
        nodes.iter().find(|node| node.key == key)
    }

    fn local(name: &str) -> Ref {
        reference(RefKind::LocalBranch, name)
    }

    #[test]
    fn folders_group_branches_in_order() {
        let refs = [
            local("main"),
            local("feat/a"),
            local("feat/b"),
            local("fix/c"),
            local("feat/ui/d"),
        ];
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "");
        assert_eq!(
            labels(&nodes),
            vec![
                "Local branches",
                "main",
                "feat",
                "a",
                "b",
                "ui",
                "d",
                "fix",
                "c",
                "Remotes",
                "Tags",
                "Stashes",
                "Worktrees",
                "Submodules",
            ]
        );
        let levels: Vec<usize> = nodes.iter().map(|node| node.level).collect();
        assert_eq!(levels, vec![0, 1, 1, 2, 2, 2, 3, 1, 2, 0, 0, 0, 0, 0]);
        assert_eq!(
            find(&nodes, "folder:feat").unwrap().icon,
            Some(IconName::Folder)
        );
        assert_eq!(find(&nodes, "folder:feat/ui").unwrap().label, "ui");
        assert_eq!(find(&nodes, "local:feat/ui/d").unwrap().label, "d");
        // Every key is unique.
        let unique: HashSet<&str> = keys(&nodes).into_iter().collect();
        assert_eq!(unique.len(), nodes.len());
    }

    #[test]
    fn remotes_are_flat_and_skip_head() {
        let refs = [
            reference(RefKind::RemoteBranch, "origin/main"),
            reference(RefKind::RemoteBranch, "origin/feat/idle-lanes"),
            reference(RefKind::RemoteBranch, "origin/HEAD"),
            reference(RefKind::LocalBranch, "main"),
        ];
        let extras = SidebarExtras {
            remotes: vec!["origin".to_owned(), "empty".to_owned()],
            ..Default::default()
        };
        let nodes = build_nodes(&refs, Some(&extras), None, &Expansion::default(), "");
        let section = find(&nodes, "section:remotes").unwrap();
        assert_eq!(section.trailing, Some(TreeNodeTrailing::Count("2".into())));
        // Remote order is the configured order; a remote with no branches is
        // still shown.
        assert_eq!(
            find(&nodes, "remote:origin").unwrap().icon,
            Some(IconName::Cloud)
        );
        assert!(find(&nodes, "remote:empty").unwrap().has_children);
        assert!(find(&nodes, "remote:empty").unwrap().expanded);
        // Branches are flat under their remote, sorted by label, and the
        // symbolic HEAD is gone.
        let branch_labels: Vec<&str> = nodes
            .iter()
            .filter(|node| node.key.starts_with("rbranch:"))
            .map(|node| node.label.as_ref())
            .collect();
        assert_eq!(branch_labels, vec!["feat/idle-lanes", "main"]);
        assert!(find(&nodes, "rbranch:origin/feat/idle-lanes").is_some());
        assert!(nodes.iter().all(|node| node.label != "HEAD"));
        // Without the extras, the count comes from the ref prefixes.
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "");
        assert_eq!(
            find(&nodes, "section:remotes").unwrap().trailing,
            Some(TreeNodeTrailing::Count("1".into()))
        );
    }

    #[test]
    fn tags_sort_newest_version_first() {
        let refs = [
            reference(RefKind::Tag, "v0.2.0"),
            reference(RefKind::Tag, "v0.10.0"),
            reference(RefKind::Tag, "v0.9.1"),
            reference(RefKind::Tag, "release"),
        ];
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "");
        let tag_labels: Vec<&str> = nodes
            .iter()
            .filter(|node| node.key.starts_with("tag:"))
            .map(|node| node.label.as_ref())
            .collect();
        assert_eq!(tag_labels, vec!["v0.10.0", "v0.9.1", "v0.2.0", "release"]);
    }

    #[test]
    fn stash_labels_follow_the_three_message_shapes() {
        let extras = SidebarExtras {
            stashes: vec![
                Stash {
                    index: 0,
                    oid: oid(),
                    message: "WIP on main: 1a2b3c4 fix the thing".to_owned(),
                },
                Stash {
                    index: 1,
                    oid: oid(),
                    message: "On feat/x: a deliberate message".to_owned(),
                },
                Stash {
                    index: 2,
                    oid: oid(),
                    message: "something else".to_owned(),
                },
            ],
            ..Default::default()
        };
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "");
        let stash_labels: Vec<&str> = nodes
            .iter()
            .filter(|node| node.key.starts_with("stash:"))
            .map(|node| node.label.as_ref())
            .collect();
        assert_eq!(
            stash_labels,
            vec!["WIP on main", "a deliberate message", "something else"]
        );
        // `stash@{0}` comes first.
        let stash_keys: Vec<&str> = nodes
            .iter()
            .filter(|node| node.key.starts_with("stash:"))
            .map(|node| node.key.as_ref())
            .collect();
        assert_eq!(stash_keys, vec!["stash:0", "stash:1", "stash:2"]);
        assert_eq!(
            find(&nodes, "section:stashes").unwrap().trailing,
            Some(TreeNodeTrailing::Count("3".into()))
        );
    }

    #[test]
    fn worktree_state_prefers_prunable_then_locked_then_bare_then_detached() {
        let worktree = |index: usize,
                        branch: Option<&str>,
                        bare: bool,
                        detached: bool,
                        locked: Option<&str>,
                        prunable: bool| Worktree {
            path: PathBuf::from(format!("/tmp/wt{index}")),
            branch: branch.map(str::to_owned),
            head: Some(oid()),
            bare,
            detached,
            locked: locked.map(str::to_owned),
            prunable,
            current: index == 0,
            main: index == 0,
        };
        let extras = SidebarExtras {
            worktrees: vec![
                worktree(0, Some("main"), true, true, Some("busy"), true),
                worktree(1, None, true, true, Some("busy"), false),
                worktree(2, None, true, true, None, false),
                worktree(3, None, false, true, None, false),
                worktree(4, Some("feat/x"), false, false, None, false),
                worktree(5, None, false, false, None, false),
            ],
            ..Default::default()
        };
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "");
        let states: Vec<Option<&TreeNodeTrailing>> = nodes
            .iter()
            .filter(|node| node.key.starts_with("wt:"))
            .map(|node| node.trailing.as_ref())
            .collect();
        assert_eq!(
            states,
            vec![
                Some(&TreeNodeTrailing::State("missing".into())),
                Some(&TreeNodeTrailing::State("locked".into())),
                Some(&TreeNodeTrailing::State("bare".into())),
                Some(&TreeNodeTrailing::State("detached".into())),
                Some(&TreeNodeTrailing::State("feat/x".into())),
                None,
            ]
        );
        // The worktree this repository was opened in is marked current; the
        // others are not.
        assert!(find(&nodes, "wt:/tmp/wt0").unwrap().current);
        assert!(!find(&nodes, "wt:/tmp/wt1").unwrap().current);
    }

    #[test]
    fn submodule_states_map_to_their_text() {
        let extras = SidebarExtras {
            submodules: vec![
                Submodule {
                    path: "vendor/a".to_owned(),
                    oid: Some(oid()),
                    state: SubmoduleState::Modified,
                },
                Submodule {
                    path: "vendor/b".to_owned(),
                    oid: None,
                    state: SubmoduleState::Uninitialized,
                },
                Submodule {
                    path: "vendor/c".to_owned(),
                    oid: Some(oid()),
                    state: SubmoduleState::Conflict,
                },
                Submodule {
                    path: "vendor/d".to_owned(),
                    oid: Some(oid()),
                    state: SubmoduleState::Clean,
                },
            ],
            ..Default::default()
        };
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "");
        let states: Vec<Option<&TreeNodeTrailing>> = nodes
            .iter()
            .filter(|node| node.key.starts_with("sub:"))
            .map(|node| node.trailing.as_ref())
            .collect();
        assert_eq!(
            states,
            vec![
                Some(&TreeNodeTrailing::State("modified".into())),
                Some(&TreeNodeTrailing::State("not initialized".into())),
                Some(&TreeNodeTrailing::State("conflict".into())),
                None,
            ]
        );
        assert_eq!(find(&nodes, "sub:vendor/a").unwrap().label, "vendor/a");
    }

    #[test]
    fn tracking_shows_ahead_behind_and_gone_and_the_head_branch() {
        let mut ahead = local("feat/ahead");
        ahead.ahead = 4;
        ahead.behind = 2;
        let mut gone = local("feat/gone");
        gone.upstream_gone = true;
        let refs = [local("main"), ahead, gone];
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let nodes = build_nodes(&refs, None, Some(&head), &Expansion::default(), "");
        assert_eq!(
            find(&nodes, "local:feat/ahead").unwrap().trailing,
            Some(TreeNodeTrailing::AheadBehind {
                ahead: 4,
                behind: 2
            })
        );
        assert_eq!(
            find(&nodes, "local:feat/gone").unwrap().trailing,
            Some(TreeNodeTrailing::State("gone".into()))
        );
        assert!(find(&nodes, "local:main").unwrap().current);
        assert!(!find(&nodes, "local:feat/gone").unwrap().current);
        assert_eq!(find(&nodes, "local:main").unwrap().trailing, None);
    }

    #[test]
    fn a_collapsed_section_has_no_children() {
        let refs = [local("main"), local("feat/a")];
        let mut expansion = Expansion::default();
        expansion.toggle("section:local", true);
        let nodes = build_nodes(&refs, None, None, &expansion, "");
        assert!(!find(&nodes, "section:local").unwrap().expanded);
        assert!(nodes.iter().all(|node| !node.key.starts_with("local:")));
        assert!(nodes.iter().all(|node| !node.key.starts_with("folder:")));
    }

    #[test]
    fn a_section_with_more_than_200_children_starts_collapsed() {
        let refs: Vec<Ref> = (0..201).map(|i| local(&format!("b{i}"))).collect();
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "");
        assert!(!find(&nodes, "section:local").unwrap().expanded);
        assert!(nodes.iter().all(|node| !node.key.starts_with("local:")));
    }

    #[test]
    fn toggling_a_folder_expands_and_collapses_it() {
        let refs = [local("feat/a"), local("fix/b")];
        let mut expansion = Expansion::default();
        let nodes = build_nodes(&refs, None, None, &expansion, "");
        assert!(find(&nodes, "folder:feat").unwrap().expanded);
        assert!(find(&nodes, "local:feat/a").is_some());

        expansion.toggle("folder:feat", true);
        let nodes = build_nodes(&refs, None, None, &expansion, "");
        assert!(!find(&nodes, "folder:feat").unwrap().expanded);
        assert!(find(&nodes, "local:feat/a").is_none());
        // The other folder keeps its own state.
        assert!(find(&nodes, "local:fix/b").is_some());

        expansion.toggle("folder:feat", false);
        let nodes = build_nodes(&refs, None, None, &expansion, "");
        assert!(find(&nodes, "local:feat/a").is_some());
    }

    #[test]
    fn a_remote_with_more_than_200_branches_starts_collapsed() {
        let refs: Vec<Ref> = (0..201)
            .map(|i| reference(RefKind::RemoteBranch, &format!("origin/b{i}")))
            .collect();
        let extras = SidebarExtras {
            remotes: vec!["origin".to_owned()],
            ..Default::default()
        };
        let nodes = build_nodes(&refs, Some(&extras), None, &Expansion::default(), "");
        assert!(!find(&nodes, "remote:origin").unwrap().expanded);
        assert!(nodes.iter().all(|node| !node.key.starts_with("rbranch:")));
    }

    #[test]
    fn filtering_keeps_ancestors_and_hides_other_sections() {
        let refs = [
            local("main"),
            local("feat/ui/d"),
            local("fix/c"),
            reference(RefKind::Tag, "v1.0.0"),
        ];
        let mut expansion = Expansion::default();
        // A collapsed folder is forced open while filtering.
        expansion.toggle("folder:feat", true);
        let nodes = build_nodes(&refs, None, None, &expansion, "  UI  ");
        assert_eq!(
            keys(&nodes),
            vec![
                "section:local",
                "folder:feat",
                "folder:feat/ui",
                "local:feat/ui/d",
            ]
        );
        assert!(find(&nodes, "section:tags").is_none());
        assert!(find(&nodes, "local:main").is_none());
    }

    #[test]
    fn filtering_counts_matches() {
        let refs = [
            local("feat/a"),
            local("feat/b"),
            local("fix/c"),
            reference(RefKind::Tag, "release-a"),
            reference(RefKind::Tag, "release-b"),
        ];
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "a");
        // "feat/a" and "feat/b" both match through the folder name.
        assert_eq!(
            find(&nodes, "section:local").unwrap().trailing,
            Some(TreeNodeTrailing::Count("2".into()))
        );
        assert_eq!(
            find(&nodes, "section:tags").unwrap().trailing,
            Some(TreeNodeTrailing::Count("2".into()))
        );
        assert!(find(&nodes, "section:tags").unwrap().expanded);
    }

    #[test]
    fn filtering_matches_remote_branches_by_both_names() {
        let refs = [reference(RefKind::RemoteBranch, "origin/feat/idle-lanes")];
        let extras = SidebarExtras {
            remotes: vec!["origin".to_owned()],
            ..Default::default()
        };
        let nodes = build_nodes(
            &refs,
            Some(&extras),
            None,
            &Expansion::default(),
            "feat/idle",
        );
        assert_eq!(
            keys(&nodes),
            vec![
                "section:remotes",
                "remote:origin",
                "rbranch:origin/feat/idle-lanes",
            ]
        );
        // The `<remote>/<branch>` form matches too.
        let nodes = build_nodes(
            &refs,
            Some(&extras),
            None,
            &Expansion::default(),
            "origin/feat",
        );
        assert!(find(&nodes, "rbranch:origin/feat/idle-lanes").is_some());
    }

    #[test]
    fn filtering_matches_stashes_worktrees_and_submodules() {
        let extras = SidebarExtras {
            stashes: vec![Stash {
                index: 0,
                oid: oid(),
                message: "On main: keep me".to_owned(),
            }],
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/feature-wt"),
                branch: Some("feature".to_owned()),
                head: Some(oid()),
                bare: false,
                detached: false,
                locked: None,
                prunable: false,
                current: true,
                main: true,
            }],
            submodules: vec![Submodule {
                path: "vendor/lib".to_owned(),
                oid: Some(oid()),
                state: SubmoduleState::Clean,
            }],
            ..Default::default()
        };
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "keep me");
        assert_eq!(keys(&nodes), vec!["section:stashes", "stash:0"]);
        let nodes = build_nodes(
            &[],
            Some(&extras),
            None,
            &Expansion::default(),
            "feature-wt",
        );
        assert!(find(&nodes, "wt:/tmp/feature-wt").is_some());
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "stash@{0}");
        assert!(find(&nodes, "stash:0").is_some());
        let nodes = build_nodes(&[], Some(&extras), None, &Expansion::default(), "vendor");
        assert!(find(&nodes, "sub:vendor/lib").is_some());
    }

    #[test]
    fn a_filter_with_no_match_builds_nothing() {
        let refs = [local("main")];
        let nodes = build_nodes(&refs, None, None, &Expansion::default(), "zzz");
        assert!(nodes.is_empty());
    }

    #[test]
    fn clearing_the_filter_restores_the_expansion_overrides() {
        let refs = [local("feat/a")];
        let mut expansion = Expansion::default();
        expansion.toggle("folder:feat", true);
        let collapsed = build_nodes(&refs, None, None, &expansion, "");
        assert!(find(&collapsed, "local:feat/a").is_none());

        // The filter forces the folder open and matches its leaf.
        let filtered = build_nodes(&refs, None, None, &expansion, "a");
        assert!(find(&filtered, "local:feat/a").is_some());
        assert!(find(&filtered, "folder:feat").unwrap().expanded);

        // Clearing it brings the user's collapse back untouched.
        let cleared = build_nodes(&refs, None, None, &expansion, "");
        assert!(find(&cleared, "local:feat/a").is_none());
        assert!(!find(&cleared, "folder:feat").unwrap().expanded);
    }

    #[test]
    fn sections_show_counts_and_the_head_branch() {
        let refs = [
            local("main"),
            local("feat/lanes"),
            reference(RefKind::RemoteBranch, "origin/main"),
            reference(RefKind::RemoteBranch, "origin/HEAD"),
            reference(RefKind::Tag, "v0.3.0"),
        ];
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let nodes = build_nodes(&refs, None, Some(&head), &Expansion::default(), "");
        assert_eq!(
            labels(&nodes),
            vec![
                "Local branches",
                "main",
                "feat",
                "lanes",
                "Remotes",
                "origin",
                "main",
                "Tags",
                "v0.3.0",
                "Stashes",
                "Worktrees",
                "Submodules",
            ]
        );
        assert!(nodes[0].section);
        assert!(nodes[0].expanded);
        assert_eq!(nodes[0].trailing, Some(TreeNodeTrailing::Count("2".into())));
        assert!(find(&nodes, "local:main").unwrap().current);
        assert!(!find(&nodes, "local:feat/lanes").unwrap().current);
        // `origin/HEAD` is a symbolic ref, not a branch row.
        assert!(nodes.iter().all(|node| node.label != "HEAD"));
        // The later sections have headers but, without extras, no counts.
        assert!(find(&nodes, "section:stashes").unwrap().trailing.is_none());
        assert_eq!(
            find(&nodes, "section:tags").unwrap().trailing,
            Some(TreeNodeTrailing::Count("1".into()))
        );
    }

    #[test]
    fn loading_refs_show_headers_without_counts() {
        let nodes = sidebar_nodes(None, None, None, &Expansion::default(), "", None);
        assert_eq!(nodes.len(), 6);
        assert!(nodes.iter().all(|node| node.section));
        assert!(nodes.iter().all(|node| node.trailing.is_none()));
    }

    #[test]
    fn selection_marks_the_matching_leaf() {
        let refs = [reference(RefKind::Tag, "v1")];
        let nodes = sidebar_nodes(
            Some(&refs),
            None,
            None,
            &Expansion::default(),
            "",
            Some("tag:v1"),
        );
        let tag = nodes.iter().find(|node| node.label == "v1").unwrap();
        assert!(tag.selected);
    }

    fn oid_of(byte: u8) -> Oid {
        Oid::from_hex(&format!("{byte:02x}").repeat(20)).unwrap()
    }

    fn worktree(path: &str, locked: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: None,
            head: Some(oid()),
            bare: false,
            detached: false,
            locked: locked.map(str::to_owned),
            prunable: false,
            current: false,
            main: false,
        }
    }

    #[test]
    fn commit_for_resolves_every_leaf_kind() {
        let mut main = local("main");
        main.target = oid_of(0x11);
        let mut remote = reference(RefKind::RemoteBranch, "origin/feat");
        remote.target = oid_of(0x22);
        let mut tag = reference(RefKind::Tag, "v1");
        tag.target = oid_of(0x33);
        let refs = [main, remote, tag];

        let mut with_head = worktree("/tmp/with-head", None);
        with_head.head = Some(oid_of(0x55));
        let mut no_head = worktree("/tmp/no-head", None);
        no_head.head = None;
        let extras = SidebarExtras {
            stashes: vec![Stash {
                index: 0,
                oid: oid_of(0x44),
                message: "WIP on main: x".to_owned(),
            }],
            worktrees: vec![with_head, no_head],
            submodules: vec![Submodule {
                path: "vendor/a".to_owned(),
                oid: Some(oid()),
                state: SubmoduleState::Clean,
            }],
            ..Default::default()
        };

        assert_eq!(
            commit_for("local:main", &refs, Some(&extras)),
            Some(oid_of(0x11))
        );
        assert_eq!(
            commit_for("rbranch:origin/feat", &refs, Some(&extras)),
            Some(oid_of(0x22))
        );
        assert_eq!(
            commit_for("tag:v1", &refs, Some(&extras)),
            Some(oid_of(0x33))
        );
        assert_eq!(
            commit_for("stash:0", &refs, Some(&extras)),
            Some(oid_of(0x44))
        );
        assert_eq!(
            commit_for("wt:/tmp/with-head", &refs, Some(&extras)),
            Some(oid_of(0x55))
        );
        // A worktree with no HEAD has no commit to reveal.
        assert_eq!(commit_for("wt:/tmp/no-head", &refs, Some(&extras)), None);
        // Stashes and worktrees need their lists.
        assert_eq!(commit_for("stash:0", &refs, None), None);
        assert_eq!(commit_for("wt:/tmp/with-head", &refs, None), None);
        // A stash index that is gone.
        assert_eq!(commit_for("stash:7", &refs, Some(&extras)), None);

        // No jump for submodules, folders, sections or unknown keys.
        assert_eq!(commit_for("sub:vendor/a", &refs, Some(&extras)), None);
        assert_eq!(commit_for("folder:feat", &refs, Some(&extras)), None);
        assert_eq!(commit_for("section:local", &refs, Some(&extras)), None);
        assert_eq!(commit_for("unknown", &refs, Some(&extras)), None);
        // A stale key whose ref is gone.
        assert_eq!(commit_for("local:gone", &refs, Some(&extras)), None);
        assert_eq!(
            commit_for("rbranch:origin/gone", &refs, Some(&extras)),
            None
        );
        assert_eq!(commit_for("tag:gone", &refs, Some(&extras)), None);
    }

    /// A separator is written `—` so a menu is one comparable label list.
    fn menu_labels(entries: &[SidebarMenuEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| match entry {
                SidebarMenuEntry::Item(item) => item.label.as_str(),
                SidebarMenuEntry::Separator => "—",
            })
            .collect()
    }

    fn hosted_ref(name: &str, kind: RefKind, upstream: Option<&str>) -> Ref {
        Ref {
            full_name: format!("refs/{name}"),
            name: name.to_owned(),
            kind,
            target: oid_of(0x01),
            upstream: upstream.map(str::to_owned),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn hosted_extras(urls: &[(&str, &str)]) -> SidebarExtras {
        SidebarExtras {
            remote_targets: urls
                .iter()
                .map(|(name, url)| RemoteTargets {
                    name: (*name).to_owned(),
                    fetch_url: (*url).to_owned(),
                    push_urls: vec![(*url).to_owned()],
                })
                .collect(),
            ..SidebarExtras::default()
        }
    }

    #[test]
    fn create_pull_request_follows_open_on_host_or_joins_the_copy_group() {
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let extras = hosted_extras(&[("origin", "git@github.com:o/r.git")]);
        let refs = [
            hosted_ref("main", RefKind::LocalBranch, Some("origin/main")),
            hosted_ref("loose", RefKind::LocalBranch, None),
        ];
        let links = web_links::host_links(&[], &extras.remote_targets, None);

        // A branch with an upstream has "Open on GitHub": the new item follows it.
        let mut with_open = sidebar_menu_with_links(
            "local:main",
            Some(&head),
            Some(&extras),
            Some(&refs),
            &links,
            Operation::None,
        );
        with_create_pull_request(&mut with_open, "main", forge::ServiceKind::GitHub);
        let labels = menu_labels(&with_open);
        let open = labels.iter().position(|l| *l == "Open on GitHub").unwrap();
        assert_eq!(labels[open + 1], "Create pull request…");
        assert_eq!(labels.last(), Some(&"Copy branch name"));

        // Without an upstream there is no "Open on": a group of its own above Copy.
        let mut without = sidebar_menu_with_links(
            "local:loose",
            Some(&head),
            Some(&extras),
            Some(&refs),
            &links,
            Operation::None,
        );
        with_create_pull_request(&mut without, "loose", forge::ServiceKind::GitLab);
        let labels = menu_labels(&without);
        let n = labels.len();
        assert_eq!(
            &labels[n - 4..],
            ["—", "Create merge request…", "—", "Copy branch name"]
        );
        let SidebarMenuEntry::Item(item) = &without[n - 3] else {
            panic!("an item")
        };
        assert!(item.enabled);
        assert_eq!(
            item.action,
            SidebarAction::CreatePullRequest("loose".to_owned())
        );
    }

    #[test]
    fn open_on_host_items_follow_the_design_order() {
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let extras = hosted_extras(&[("origin", "git@github.com:o/r.git")]);
        let refs = [
            hosted_ref("main", RefKind::LocalBranch, Some("origin/main")),
            hosted_ref("loose", RefKind::LocalBranch, None),
            hosted_ref("v1", RefKind::Tag, None),
        ];
        let links = web_links::host_links(&[], &extras.remote_targets, None);
        let menu = |key: &str| {
            sidebar_menu_with_links(
                key,
                Some(&head),
                Some(&extras),
                Some(&refs),
                &links,
                Operation::None,
            )
        };
        let labels = |key: &str| {
            menu(key)
                .iter()
                .map(|entry| match entry {
                    SidebarMenuEntry::Item(item) => item.label.clone(),
                    SidebarMenuEntry::Separator => "—".to_owned(),
                })
                .collect::<Vec<_>>()
        };

        let local = labels("local:main");
        assert_eq!(
            local[local.len() - 4..],
            ["—", "Open on GitHub", "—", "Copy branch name"]
        );
        // No upstream, no page on the host.
        assert!(!labels("local:loose")
            .iter()
            .any(|l| l.starts_with("Open on")));
        assert_eq!(
            labels("rbranch:origin/dev")[3..],
            [
                "—",
                "Delete remote branch…",
                "—",
                "Open on GitHub",
                "—",
                "Copy branch name"
            ]
        );
        let tag = labels("tag:v1");
        assert_eq!(
            tag[2..],
            [
                "Push tag to origin",
                "—",
                "Open on GitHub",
                "—",
                "Rename tag…",
                "Delete tag"
            ]
        );
        assert_eq!(
            labels("remote:origin"),
            [
                "Fetch",
                "Edit remote…",
                "—",
                "Account…",
                "Open on GitHub",
                "—",
                "Remove remote"
            ]
        );

        let open = |key: &str| {
            menu(key).into_iter().find_map(|entry| match entry {
                SidebarMenuEntry::Item(item) => match item.action {
                    SidebarAction::OpenUrl { url, host } => Some((url, host)),
                    _ => None,
                },
                SidebarMenuEntry::Separator => None,
            })
        };
        let github = |url: &str| Some((url.to_owned(), "github.com".to_owned()));
        assert_eq!(
            open("local:main"),
            github("https://github.com/o/r/tree/main")
        );
        assert_eq!(
            open("rbranch:origin/dev"),
            github("https://github.com/o/r/tree/dev")
        );
        assert_eq!(
            open("tag:v1"),
            github("https://github.com/o/r/releases/tag/v1")
        );
        assert_eq!(open("remote:origin"), github("https://github.com/o/r"));
    }

    #[test]
    fn without_a_known_host_the_menus_are_unchanged() {
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let extras = hosted_extras(&[("origin", "https://git.corp.example/o/r.git")]);
        let refs = [hosted_ref(
            "main",
            RefKind::LocalBranch,
            Some("origin/main"),
        )];
        let links = web_links::host_links(&[], &extras.remote_targets, None);
        assert!(links.is_empty());
        for key in [
            "local:main",
            "rbranch:origin/main",
            "tag:v1",
            "remote:origin",
        ] {
            assert_eq!(
                menu_labels(&sidebar_menu_with_links(
                    key,
                    Some(&head),
                    Some(&extras),
                    Some(&refs),
                    &links,
                    Operation::None
                )),
                menu_labels(&sidebar_menu(
                    key,
                    Some(&head),
                    Some(&extras),
                    Some(&refs),
                    Operation::None
                )),
                "{key}"
            );
        }
    }

    #[test]
    fn submodule_menu_enables_open_only_when_initialised() {
        let extras = SidebarExtras {
            submodules: vec![
                Submodule {
                    path: "vendor/a".to_owned(),
                    oid: Some(oid()),
                    state: SubmoduleState::Clean,
                },
                Submodule {
                    path: "vendor/b".to_owned(),
                    oid: None,
                    state: SubmoduleState::Uninitialized,
                },
            ],
            ..Default::default()
        };
        let ready = sidebar_menu("sub:vendor/a", None, Some(&extras), None, Operation::None);
        assert_eq!(
            menu_item(&ready, "Update submodule").action,
            SidebarAction::UpdateSubmodule("vendor/a".to_owned())
        );
        assert_eq!(
            menu_item(&ready, "Open in new window").action,
            SidebarAction::OpenSubmodule("vendor/a".to_owned())
        );
        let missing = sidebar_menu("sub:vendor/b", None, Some(&extras), None, Operation::None);
        assert!(menu_item(&missing, "Update submodule").enabled);
        assert!(!menu_item(&missing, "Open in new window").enabled);
    }

    fn menu_item<'a>(entries: &'a [SidebarMenuEntry], label: &str) -> &'a SidebarMenuItem {
        entries
            .iter()
            .find_map(|entry| match entry {
                SidebarMenuEntry::Item(item) if item.label == label => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no `{label}` item in the menu"))
    }

    #[test]
    fn every_menu_kind_lists_the_designs_items_in_order() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let extras = SidebarExtras {
            worktrees: vec![
                worktree("/tmp/plain", None),
                worktree("/tmp/locked", Some("busy")),
            ],
            ..Default::default()
        };

        assert_eq!(
            menu_labels(&sidebar_menu(
                "local:feat/idle-lanes",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Checkout",
                "Show all commits of feat/idle-lanes",
                "Merge feat/idle-lanes into main…",
                "Rebase main onto feat/idle-lanes",
                "—",
                "Rename…",
                "Set upstream",
                "Delete branch",
                "—",
                "Copy branch name",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "rbranch:origin/feat/idle-lanes",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Checkout as local branch",
                "Show all commits of origin/feat/idle-lanes",
                "Merge origin/feat/idle-lanes into main…",
                "—",
                "Delete remote branch…",
                "Copy branch name",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "tag:v1",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Checkout",
                "Create branch here…",
                "Push tag to origin",
                "—",
                "Rename tag…",
                "Delete tag",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "stash:0",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Apply stash",
                "Pop stash",
                "View changes",
                "—",
                "Drop stash",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "remote:origin",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Fetch",
                "Edit remote…",
                "—",
                "Account…",
                "—",
                "Remove remote"
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "wt:/tmp/plain",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Open in new window",
                "Rename worktree…",
                "Manage worktrees…",
                "Add worktree…",
                "—",
                "Lock",
                "Remove worktree…",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "wt:/tmp/locked",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Open in new window",
                "Rename worktree…",
                "Manage worktrees…",
                "Add worktree…",
                "—",
                "Unlock",
                "Remove worktree…",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "sub:vendor/a",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec!["Update submodule", "Open in new window"]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "section:remotes",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Add remote…",
                "—",
                "Fetch all remotes",
                "Prune stale remote branches",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "section:tags",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Create tag…",
                "—",
                "Fetch tags from origin",
                "Push all tags to origin",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "section:worktrees",
                Some(&head),
                Some(&extras),
                None,
                Operation::None
            )),
            vec![
                "Manage worktrees…",
                "Add worktree…",
                "—",
                "Prune missing worktrees",
                "Refresh list",
            ]
        );
        // The other sections, folders and unknown keys have no menu.
        assert!(sidebar_menu(
            "section:local",
            Some(&head),
            Some(&extras),
            None,
            Operation::None
        )
        .is_empty());
        assert!(sidebar_menu(
            "section:stashes",
            Some(&head),
            Some(&extras),
            None,
            Operation::None
        )
        .is_empty());
        assert!(sidebar_menu(
            "section:submodules",
            Some(&head),
            Some(&extras),
            None,
            Operation::None
        )
        .is_empty());
        assert!(sidebar_menu(
            "folder:feat",
            Some(&head),
            Some(&extras),
            None,
            Operation::None
        )
        .is_empty());
        assert!(
            sidebar_menu("unknown", Some(&head), Some(&extras), None, Operation::None).is_empty()
        );
    }

    #[test]
    fn merge_is_enabled_for_another_local_branch() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let menu = sidebar_menu("local:feat/x", Some(&head), None, None, Operation::None);
        let merge = menu_item(&menu, "Merge feat/x into main…");
        assert!(merge.enabled);
        assert_eq!(merge.action, SidebarAction::Merge("feat/x".to_owned()));
        // Rebase stays disabled until its own stage.
        assert!(!menu_item(&menu, "Rebase main onto feat/x").enabled);
    }

    #[test]
    fn merge_is_disabled_for_the_current_branch() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let menu = sidebar_menu("local:main", Some(&head), None, None, Operation::None);
        let merge = menu_item(&menu, "Merge main into main…");
        assert!(!merge.enabled);
        assert_eq!(merge.action, SidebarAction::None);
    }

    #[test]
    fn merge_is_disabled_while_an_operation_is_stopped() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        for operation in [Operation::Merge, Operation::Squash, Operation::CherryPick] {
            let local = sidebar_menu("local:feat/x", Some(&head), None, None, operation);
            assert!(!menu_item(&local, "Merge feat/x into main…").enabled);
            let remote = sidebar_menu("rbranch:origin/feat/x", Some(&head), None, None, operation);
            assert!(!menu_item(&remote, "Merge origin/feat/x into main…").enabled);
        }
        let local = sidebar_menu("local:feat/x", Some(&head), None, None, Operation::None);
        assert!(menu_item(&local, "Merge feat/x into main…").enabled);
    }

    #[test]
    fn merge_is_disabled_on_a_detached_head() {
        let detached = Head::Detached(oid());
        let local = sidebar_menu("local:feat/x", Some(&detached), None, None, Operation::None);
        assert!(!menu_item(&local, "Merge feat/x into HEAD…").enabled);
        let remote = sidebar_menu(
            "rbranch:origin/feat/x",
            Some(&detached),
            None,
            None,
            Operation::None,
        );
        assert!(!menu_item(&remote, "Merge origin/feat/x into HEAD…").enabled);
        let unread = sidebar_menu("local:feat/x", None, None, None, Operation::None);
        assert!(!menu_item(&unread, "Merge feat/x into HEAD…").enabled);
    }

    #[test]
    fn merge_is_enabled_for_a_remote_branch() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let menu = sidebar_menu(
            "rbranch:origin/feat/x",
            Some(&head),
            None,
            None,
            Operation::None,
        );
        let merge = menu_item(&menu, "Merge origin/feat/x into main…");
        assert!(merge.enabled);
        assert_eq!(
            merge.action,
            SidebarAction::Merge("origin/feat/x".to_owned())
        );
    }

    #[test]
    fn a_detached_head_says_head_in_the_branch_menus() {
        let detached = Head::Detached(oid());
        assert_eq!(
            menu_labels(&sidebar_menu(
                "local:feat/x",
                Some(&detached),
                None,
                None,
                Operation::None
            )),
            vec![
                "Checkout",
                "Show all commits of feat/x",
                "Merge feat/x into HEAD…",
                "Rebase HEAD onto feat/x",
                "—",
                "Rename…",
                "Set upstream",
                "Delete branch",
                "—",
                "Copy branch name",
            ]
        );
        assert_eq!(
            menu_labels(&sidebar_menu(
                "rbranch:origin/feat/x",
                Some(&detached),
                None,
                None,
                Operation::None
            )),
            vec![
                "Checkout as local branch",
                "Show all commits of origin/feat/x",
                "Merge origin/feat/x into HEAD…",
                "—",
                "Delete remote branch…",
                "Copy branch name",
            ]
        );
        // HEAD not read yet is named the same way.
        assert_eq!(
            menu_labels(&sidebar_menu(
                "local:feat/x",
                None,
                None,
                None,
                Operation::None
            ))[2],
            "Merge feat/x into HEAD…"
        );
    }

    #[test]
    fn stage_seven_and_eight_actions_are_enabled() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let extras = SidebarExtras {
            remotes: vec!["origin".into()],
            worktrees: vec![worktree("/tmp/wt", None)],
            ..Default::default()
        };
        let keys = [
            "local:feat/idle-lanes",
            "rbranch:origin/feat/idle-lanes",
            "tag:v1",
            "stash:0",
            "remote:origin",
            "wt:/tmp/wt",
            "sub:vendor/a",
            "section:remotes",
            "section:tags",
            "section:worktrees",
        ];
        for key in keys {
            let entries = sidebar_menu(key, Some(&head), Some(&extras), None, Operation::None);
            let enabled: Vec<&str> = entries
                .iter()
                .filter_map(|entry| match entry {
                    SidebarMenuEntry::Item(item) if item.enabled => Some(item.label.as_str()),
                    _ => None,
                })
                .collect();
            let expected: &[&str] = match key {
                "local:feat/idle-lanes" => &[
                    "Checkout",
                    "Show all commits of feat/idle-lanes",
                    "Merge feat/idle-lanes into main…",
                    "Rename…",
                    "Set upstream",
                    "Delete branch",
                    "Copy branch name",
                ],
                "rbranch:origin/feat/idle-lanes" => &[
                    "Checkout as local branch",
                    "Show all commits of origin/feat/idle-lanes",
                    "Merge origin/feat/idle-lanes into main…",
                    "Delete remote branch…",
                    "Copy branch name",
                ],
                "tag:v1" => &[
                    "Checkout",
                    "Push tag to origin",
                    "Rename tag…",
                    "Delete tag",
                ],
                "stash:0" => &["Apply stash", "Pop stash", "View changes", "Drop stash"],
                // Open in new window stays disabled: this fixture has no
                // initialised `vendor/a`.
                "sub:vendor/a" => &["Update submodule"],
                "remote:origin" => &["Fetch", "Edit remote…", "Account…", "Remove remote"],
                "wt:/tmp/wt" => &[
                    "Open in new window",
                    "Rename worktree…",
                    "Manage worktrees…",
                    "Add worktree…",
                    "Lock",
                    "Remove worktree…",
                ],
                "section:remotes" => &[
                    "Add remote…",
                    "Fetch all remotes",
                    "Prune stale remote branches",
                ],
                "section:tags" => &[
                    "Create tag…",
                    "Fetch tags from origin",
                    "Push all tags to origin",
                ],
                "section:worktrees" => &[
                    "Manage worktrees…",
                    "Add worktree…",
                    "Prune missing worktrees",
                    "Refresh list",
                ],
                _ => &[],
            };
            assert_eq!(enabled, expected, "wrong enabled items for `{key}`");
        }

        // The local branch copies its full name.
        let local_menu = sidebar_menu(
            "local:feat/idle-lanes",
            Some(&head),
            Some(&extras),
            None,
            Operation::None,
        );
        assert_eq!(
            menu_item(&local_menu, "Copy branch name").action,
            SidebarAction::CopyText("feat/idle-lanes".to_owned())
        );
        // A remote branch copies `<remote>/<branch>`.
        let remote_menu = sidebar_menu(
            "rbranch:origin/feat/idle-lanes",
            Some(&head),
            Some(&extras),
            None,
            Operation::None,
        );
        assert_eq!(
            menu_item(&remote_menu, "Copy branch name").action,
            SidebarAction::CopyText("origin/feat/idle-lanes".to_owned())
        );
        // Refresh list reloads the sidebar's lists.
        let worktrees_menu = sidebar_menu(
            "section:worktrees",
            Some(&head),
            Some(&extras),
            None,
            Operation::None,
        );
        assert_eq!(
            menu_item(&worktrees_menu, "Refresh list").action,
            SidebarAction::Refresh
        );
        // A tag resolves its commit so "Create branch here…" can open.
        let refs = vec![Ref {
            full_name: "refs/tags/v1".into(),
            name: "v1".into(),
            kind: RefKind::Tag,
            target: oid(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }];
        let tag_menu = sidebar_menu(
            "tag:v1",
            Some(&head),
            Some(&extras),
            Some(&refs),
            Operation::None,
        );
        assert_eq!(
            menu_item(&tag_menu, "Create branch here…").action,
            SidebarAction::CreateBranchAt(oid())
        );
        // The current branch is neither checked out nor deleted.
        let current = sidebar_menu(
            "local:main",
            Some(&head),
            Some(&extras),
            None,
            Operation::None,
        );
        assert_eq!(menu_item(&current, "Checkout").action, SidebarAction::None);
        assert_eq!(
            menu_item(&current, "Delete branch").action,
            SidebarAction::None
        );
        // The stage 8 network actions are wired.
        assert_eq!(
            menu_item(&remote_menu, "Delete remote branch…").action,
            SidebarAction::DeleteRemoteBranch("origin/feat/idle-lanes".to_owned())
        );
        assert_eq!(
            menu_item(&tag_menu, "Push tag to origin").action,
            SidebarAction::PushTag("v1".to_owned())
        );
        let stash_menu = sidebar_menu("stash:0", Some(&head), Some(&extras), None, Operation::None);
        assert_eq!(
            menu_item(&stash_menu, "View changes").action,
            SidebarAction::ViewStash(0)
        );
        // The shortcut hints of the design are kept.
        assert_eq!(
            menu_item(&local_menu, "Checkout").shortcut.as_deref(),
            Some("Enter")
        );
        assert_eq!(
            menu_item(&local_menu, "Rename…").shortcut.as_deref(),
            Some("F2")
        );
        assert_eq!(
            menu_item(&local_menu, "Delete branch").shortcut.as_deref(),
            Some("Del")
        );
        assert!(menu_item(&local_menu, "Delete branch").danger);
        let remotes_menu = sidebar_menu(
            "section:remotes",
            Some(&head),
            Some(&extras),
            None,
            Operation::None,
        );
        assert_eq!(
            menu_item(&remotes_menu, "Fetch all remotes")
                .shortcut
                .as_deref(),
            Some("Ctrl+Shift+F")
        );
    }

    #[test]
    fn shortcut_actions_follow_the_menu_hints() {
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let run = |key: &str, hint: &str| {
            shortcut_action(key, hint, Some(&head), None, None, Operation::None)
        };
        assert_eq!(
            run("local:feat", "Enter"),
            Some(SidebarAction::CheckoutBranch("feat".into()))
        );
        assert_eq!(run("local:main", "Enter"), None, "the current branch");
        assert_eq!(
            run("rbranch:origin/x", "Enter"),
            Some(SidebarAction::CheckoutRemoteBranch("origin/x".into()))
        );
        assert_eq!(
            run("tag:v1", "Enter"),
            Some(SidebarAction::CheckoutTag("v1".into()))
        );
        assert_eq!(run("stash:0", "Enter"), None);
        assert_eq!(
            run("local:feat", "F2"),
            Some(SidebarAction::RenameBranch("feat".into()))
        );
        assert_eq!(
            run("tag:v1", "F2"),
            Some(SidebarAction::RenameTag("v1".into()))
        );
        assert_eq!(
            run("wt:/tmp/w", "F2"),
            Some(SidebarAction::RenameWorktree("/tmp/w".into()))
        );
        assert_eq!(
            run("local:feat", "Del"),
            Some(SidebarAction::DeleteBranch("feat".into()))
        );
        assert_eq!(run("local:main", "Del"), None, "the current branch");
        assert_eq!(
            run("tag:v1", "Del"),
            Some(SidebarAction::DeleteTag("v1".into()))
        );
        assert_eq!(run("stash:2", "Del"), Some(SidebarAction::DropStash(2)));
        assert_eq!(
            run("remote:origin", "Del"),
            Some(SidebarAction::RemoveRemote("origin".into()))
        );
        assert_eq!(
            run("wt:/tmp/w", "Del"),
            Some(SidebarAction::RemoveWorktree("/tmp/w".into()))
        );
        assert_eq!(run("section:tags", "Del"), None);
    }

    fn worktree_entry(
        name: &str,
        branch: Option<&str>,
        main: bool,
        last_used: Option<u64>,
    ) -> WorktreeEntry {
        WorktreeEntry {
            worktree: Worktree {
                path: PathBuf::from(format!("/tmp/{name}")),
                branch: branch.map(str::to_owned),
                head: None,
                bare: false,
                detached: branch.is_none(),
                locked: None,
                prunable: false,
                current: false,
                main,
            },
            changes: Vec::new(),
            missing: false,
            head_summary: None,
            ahead: 0,
            behind: 0,
            has_upstream: false,
            last_used,
        }
    }

    fn panel_names(entries: &[WorktreeEntry], query: &str) -> Vec<String> {
        worktree_panel_rows(entries, query)
            .iter()
            .map(|entry| entry.name())
            .collect()
    }

    #[test]
    fn the_panel_pins_main_first_then_orders_by_recency() {
        // The model hands them over current first; main is not the current one.
        let entries = [
            worktree_entry("review", Some("feat/palette"), false, Some(300)),
            worktree_entry("app", Some("main"), true, Some(10)),
            worktree_entry("hotfix", Some("fix/git"), false, Some(500)),
            worktree_entry("bisect", None, false, None),
        ];
        assert_eq!(
            panel_names(&entries, ""),
            ["app", "hotfix", "review", "bisect"]
        );
    }

    #[test]
    fn the_panel_filter_matches_name_and_branch_ignoring_case() {
        let entries = [
            worktree_entry("app", Some("main"), true, None),
            worktree_entry("review", Some("feat/Palette"), false, None),
            worktree_entry("bisect", None, false, None),
        ];
        assert_eq!(panel_names(&entries, "PAL"), ["review"]);
        assert_eq!(panel_names(&entries, " bis "), ["bisect"]);
        assert_eq!(panel_names(&entries, "main"), ["app"]);
        // The panel's "detached HEAD" label is not searchable text.
        assert!(panel_names(&entries, "detached").is_empty());
        assert!(panel_names(&entries, "nothing").is_empty());
    }

    #[test]
    fn the_worktree_menu_follows_the_designs_disabled_rules() {
        let disabled = |entry: &WorktreeEntry| -> Vec<(&'static str, bool)> {
            worktree_menu(entry)
                .into_iter()
                .flatten()
                // Leaked so the expectations below stay `&str` literals; test-only.
                .map(|item| {
                    let label: &'static str = Box::leak(item.label.to_string().into_boxed_str());
                    (label, item.disabled)
                })
                .collect()
        };
        // The main, current worktree: no switch, no lock, no remove.
        let mut main = worktree_entry("app", Some("main"), true, None);
        main.worktree.current = true;
        assert_eq!(
            disabled(&main),
            [
                ("Switch to this worktree", true),
                ("Open in new window", false),
                ("Reveal in file manager", false),
                ("Copy path", false),
                ("Lock", true),
                ("Remove worktree…", true),
            ]
        );
        // A plain linked worktree: everything is available; a lock flips the
        // label.
        let mut linked = worktree_entry("hotfix", Some("fix"), false, None);
        assert!(disabled(&linked).iter().all(|(_, off)| !off));
        linked.worktree.locked = Some("why".to_owned());
        assert!(disabled(&linked).contains(&("Unlock", false)));
        // A missing folder cannot be opened, revealed, switched to or locked,
        // but its record can be copied and removed.
        linked.missing = true;
        assert_eq!(
            disabled(&linked),
            [
                ("Switch to this worktree", true),
                ("Open in new window", true),
                ("Reveal in file manager", true),
                ("Copy path", false),
                ("Unlock", true),
                ("Remove worktree…", false),
            ]
        );
        // The separator sits between Copy path and Lock/Unlock.
        assert!(worktree_menu(&linked)[4].is_none());
    }

    fn targets(name: &str, fetch: &str, push: &[&str]) -> RemoteTargets {
        RemoteTargets {
            name: name.to_owned(),
            fetch_url: fetch.to_owned(),
            push_urls: push.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn the_remote_menu_names_account_for_a_plain_and_a_multi_push_remote() {
        let extras = SidebarExtras {
            remotes: vec!["origin".into(), "publish".into()],
            remote_targets: vec![
                targets(
                    "origin",
                    "https://github.com/a/b.git",
                    &["https://github.com/a/b.git"],
                ),
                targets(
                    "publish",
                    "https://github.com/a/b.git",
                    &["https://github.com/a/b.git", "https://codeberg.org/a/b.git"],
                ),
            ],
            ..Default::default()
        };
        let plain = sidebar_menu("remote:origin", None, Some(&extras), None, Operation::None);
        assert_eq!(
            menu_labels(&plain),
            vec![
                "Fetch",
                "Edit remote…",
                "—",
                "Account…",
                "—",
                "Remove remote"
            ]
        );
        assert_eq!(
            menu_item(&plain, "Account…").action,
            SidebarAction::RemoteAccount("origin".into())
        );
        let multi = sidebar_menu("remote:publish", None, Some(&extras), None, Operation::None);
        assert_eq!(
            menu_labels(&multi),
            vec![
                "Fetch",
                "Edit remote…",
                "—",
                "Accounts for push URLs…",
                "—",
                "Remove remote"
            ]
        );
        assert_eq!(
            menu_item(&multi, "Accounts for push URLs…").action,
            SidebarAction::RemotePushAccounts("publish".into())
        );
    }

    fn chip_of(text: &str, warn: bool) -> remote_accounts::Chip {
        remote_accounts::Chip {
            text: text.to_owned(),
            warn,
            tooltip: format!("tip {text}"),
        }
    }

    fn display_with(chip: Option<&str>, add: bool) -> RemoteDisplay {
        RemoteDisplay {
            service: Some(ServiceKind::GitHub),
            chip: chip.map(|text| chip_of(text, true)),
            tooltip: Some("origin · github.com".into()),
            add_account: add.then(|| remote_accounts::AddAccount {
                kind: ServiceKind::GitHub,
                host: "github.com".into(),
                label: "Add account for github.com".into(),
                tooltip: "Add an account for github.com and use it for origin".into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_remote_row_carries_its_chip_and_an_add_account_row_comes_first() {
        let refs = [reference(RefKind::RemoteBranch, "origin/main")];
        let extras = SidebarExtras {
            remotes: vec!["origin".to_owned()],
            ..Default::default()
        };
        let mut remotes = RemoteDisplays::new();
        remotes.insert("origin".into(), display_with(Some("no account"), true));
        let nodes = sidebar_nodes_with(
            Some(&refs),
            None,
            Some(&extras),
            &remotes,
            &Expansion::default(),
            "",
            None,
        );
        let origin = find(&nodes, "remote:origin").unwrap();
        assert_eq!(origin.icon, Some(IconName::Github));
        assert_eq!(
            origin.trailing,
            Some(TreeNodeTrailing::Chip {
                text: "no account".into(),
                warn: true,
                tooltip: Some("tip no account".into())
            })
        );
        assert_eq!(origin.tooltip.as_deref(), Some("origin · github.com"));
        let add = find(&nodes, "addacc:origin").unwrap();
        assert_eq!(add.label, "Add account for github.com");
        assert_eq!((add.level, add.icon), (2, Some(IconName::Plus)));
        assert!(add.accent);
        assert_eq!(
            add.tooltip.as_deref(),
            Some("Add an account for github.com and use it for origin")
        );
        let at = |key: &str| nodes.iter().position(|n| n.key == key).unwrap();
        assert!(at("remote:origin") < at("addacc:origin"));
        assert!(at("addacc:origin") < at("rbranch:origin/main"));
        // Without displays the row is the plain cloud, no chip.
        let plain = sidebar_nodes(
            Some(&refs),
            None,
            Some(&extras),
            &Expansion::default(),
            "",
            None,
        );
        let origin = find(&plain, "remote:origin").unwrap();
        assert_eq!(
            (origin.icon, origin.trailing.clone()),
            (Some(IconName::Cloud), None)
        );
        assert!(find(&plain, "addacc:origin").is_none());
    }

    #[test]
    fn a_service_without_an_icon_gets_its_monogram_badge() {
        let extras = SidebarExtras {
            remotes: vec!["work".to_owned()],
            ..Default::default()
        };
        let mut remotes = RemoteDisplays::new();
        remotes.insert(
            "work".into(),
            RemoteDisplay {
                service: Some(ServiceKind::GitLab),
                ..Default::default()
            },
        );
        let nodes = sidebar_nodes_with(
            Some(&[]),
            None,
            Some(&extras),
            &remotes,
            &Expansion::default(),
            "",
            None,
        );
        let work = find(&nodes, "remote:work").unwrap();
        assert_eq!(work.badge.as_deref(), Some("GL"));
        assert_eq!(work.icon, Some(IconName::Cloud));
    }

    #[test]
    fn a_multi_push_remote_lists_fetch_and_push_rows_before_the_branches() {
        let refs = [reference(RefKind::RemoteBranch, "publish/main")];
        let extras = SidebarExtras {
            remotes: vec!["publish".to_owned()],
            ..Default::default()
        };
        let row = |label: &str, service, chip: Option<&str>| remote_accounts::PushRow {
            host: String::new(),
            label: label.to_owned(),
            service: Some(service),
            chip: chip.map(|text| chip_of(text, false)),
        };
        let mut remotes = RemoteDisplays::new();
        remotes.insert(
            "publish".into(),
            RemoteDisplay {
                service: None,
                chip: Some(chip_of("2 push URLs", false)),
                tooltip: Some("publish · pushes to 2 targets".into()),
                add_account: None,
                fetch: Some(row(
                    "fetch → github.com",
                    ServiceKind::GitHub,
                    Some("mira-k"),
                )),
                pushes: vec![
                    row("push → github.com", ServiceKind::GitHub, Some("mira-k")),
                    row("push → codeberg.org", ServiceKind::Gitea, None),
                ],
            },
        );
        let nodes = sidebar_nodes_with(
            Some(&refs),
            None,
            Some(&extras),
            &remotes,
            &Expansion::default(),
            "",
            None,
        );
        assert_eq!(
            nodes
                .iter()
                .skip(2)
                .take(5)
                .map(|n| (n.label.as_ref(), n.level))
                .collect::<Vec<_>>(),
            vec![
                ("publish", 1),
                ("fetch → github.com", 2),
                ("push → github.com", 2),
                ("push → codeberg.org", 2),
                ("main", 2)
            ]
        );
        let publish = find(&nodes, "remote:publish").unwrap();
        assert_eq!(publish.icon, Some(IconName::Cloud));
        assert_eq!(
            publish.tooltip.as_deref(),
            Some("publish · pushes to 2 targets")
        );
        let chip = |key: &str| match &find(&nodes, key).unwrap().trailing {
            Some(TreeNodeTrailing::Chip { text, .. }) => Some(text.to_string()),
            _ => None,
        };
        assert_eq!(chip("remote:publish").as_deref(), Some("2 push URLs"));
        assert_eq!(chip("fetchurl:publish").as_deref(), Some("mira-k"));
        assert_eq!(chip("pushurl:publish:0").as_deref(), Some("mira-k"));
        assert_eq!(chip("pushurl:publish:1"), None);
        // Gitea has no icon of its own: the badge stands in.
        assert_eq!(
            find(&nodes, "pushurl:publish:1").unwrap().badge.as_deref(),
            Some("GT")
        );
    }
}
