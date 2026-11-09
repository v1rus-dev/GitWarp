//! The pure model of the cascading ref menus of the History: which ref rows a
//! commit gets, how many of them show before "More refs (N)", and what each
//! ref's submenu section holds. No GPUI here, so the shape (order, labels,
//! enabled state, hidden items) is unit-testable; `history_view` converts the
//! entries to the UI kit's menu entries (`ref_menu_entries`).
//!
//! Design: `docs/design/prototype/menus-refs.html`. Items the design marks
//! "Planned" (Rebase onto, Fast-forward, Compare with HEAD, Hide from graph,
//! Interactive rebase) are not part of the model.

use forge::{ServiceKind, WebTarget};
use git_core::{Head, Oid, Operation, Ref, RefKind};
use ui::IconName;

use crate::menu_model::{self, SidebarAction, SidebarMenuItem};
use crate::network_ops;
use crate::remote_accounts::RemoteTargets;
use crate::web_links::{self, HostLink};

/// With more refs than this a commit menu shows [`SHOWN_WHEN_OVERFLOWING`] rows
/// and a "More refs (N)" row for the rest.
pub const MAX_REF_ROWS: usize = 6;
/// How many ref rows stay at the top level when the refs overflow.
pub const SHOWN_WHEN_OVERFLOWING: usize = 5;

/// What a ref row stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefRowKind {
    /// A local branch, together with the remote branches of the same name on
    /// the same commit (`remotes` are remote names, e.g. `origin`).
    Local {
        head: bool,
        remotes: Vec<String>,
        ahead: usize,
        behind: usize,
    },
    /// A remote branch with no local branch of that name on the commit.
    Remote,
    Tag,
}

/// One ref row of a commit menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefRow {
    pub kind: RefRowKind,
    /// The short name: `main`, `origin/main` (remote-only) or `v1.0`.
    pub name: String,
}

impl RefRow {
    pub fn icon(&self) -> IconName {
        match self.kind {
            RefRowKind::Local { .. } => IconName::Branch,
            RefRowKind::Remote => IconName::Cloud,
            RefRowKind::Tag => IconName::Tag,
        }
    }

    /// Whether this is the checked-out branch (its icon turns accent).
    pub fn is_head(&self) -> bool {
        matches!(self.kind, RefRowKind::Local { head: true, .. })
    }
}

/// The muted text at the right of a ref row: `HEAD +origin ↑2 ↓1`, or `None`
/// when there is nothing to say (a remote-only branch, a tag, a plain branch).
pub fn row_hint(row: &RefRow) -> Option<String> {
    let RefRowKind::Local {
        head,
        remotes,
        ahead,
        behind,
    } = &row.kind
    else {
        return None;
    };
    let mut parts: Vec<String> = Vec::new();
    if *head {
        // The git term, not translated (the badge on the row says the same).
        parts.push("HEAD".to_owned());
    }
    parts.extend(remotes.iter().map(|remote| format!("+{remote}")));
    if *ahead > 0 {
        parts.push(format!("↑{ahead}"));
    }
    if *behind > 0 {
        parts.push(format!("↓{behind}"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Splits `origin/feat/x` into the remote and the branch over the configured
/// remotes; for a name no remote claims, at the first `/`.
fn split_remote<'a>(name: &'a str, targets: &[RemoteTargets]) -> Option<(&'a str, &'a str)> {
    web_links::split_remote_ref(name, targets).or_else(|| name.split_once('/'))
}

/// The ref rows of the commit `commit`: the HEAD branch, other local branches
/// A–Z, remote-only branches A–Z, then tags A–Z.
///
/// A remote branch whose name (without the remote) equals a local branch on
/// the same commit is folded into that local row (`+origin`); the symbolic
/// `<remote>/HEAD` is never a row.
pub fn commit_ref_rows(
    commit: Oid,
    head: Option<&Head>,
    refs: &[Ref],
    targets: &[RemoteTargets],
) -> Vec<RefRow> {
    let head_name = match head {
        Some(Head::Branch { name, .. }) => Some(name.as_str()),
        _ => None,
    };
    let on_commit = |kind: RefKind| {
        let mut list: Vec<&Ref> = refs
            .iter()
            .filter(|r| r.kind == kind && r.target == commit)
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    };

    let mut locals: Vec<RefRow> = on_commit(RefKind::LocalBranch)
        .into_iter()
        .map(|r| RefRow {
            name: r.name.clone(),
            kind: RefRowKind::Local {
                head: Some(r.name.as_str()) == head_name,
                remotes: Vec::new(),
                ahead: r.ahead,
                behind: r.behind,
            },
        })
        .collect();
    // HEAD first; the sort is stable, so the rest stays A–Z.
    locals.sort_by_key(|row| !row.is_head());

    let mut remote_only: Vec<RefRow> = Vec::new();
    for remote_ref in on_commit(RefKind::RemoteBranch) {
        if remote_ref.name.ends_with("/HEAD") {
            continue;
        }
        let split = split_remote(&remote_ref.name, targets);
        let twin = split.and_then(|(remote, branch)| {
            locals
                .iter_mut()
                .find(|row| row.name == branch)
                .map(|row| (remote, row))
        });
        match twin {
            Some((remote, row)) => {
                if let RefRowKind::Local { remotes, .. } = &mut row.kind {
                    remotes.push(remote.to_owned());
                }
            }
            None => remote_only.push(RefRow {
                kind: RefRowKind::Remote,
                name: remote_ref.name.clone(),
            }),
        }
    }

    let tags = on_commit(RefKind::Tag).into_iter().map(|r| RefRow {
        kind: RefRowKind::Tag,
        name: r.name.clone(),
    });
    locals.into_iter().chain(remote_only).chain(tags).collect()
}

/// `(top-level rows, rows for "More refs (N)")`: more than [`MAX_REF_ROWS`]
/// rows become five and the rest; N is the length of the second slice.
pub fn split_overflow(rows: &[RefRow]) -> (&[RefRow], &[RefRow]) {
    if rows.len() > MAX_REF_ROWS {
        rows.split_at(SHOWN_WHEN_OVERFLOWING)
    } else {
        (rows, &[])
    }
}

/// An icon button at the right of a section header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefMenuTool {
    pub icon: IconName,
    pub tooltip: String,
    /// The "on" state: the History is already narrowed to this ref.
    pub selected: bool,
    pub action: SidebarAction,
}

/// One row of a ref menu: a section header, an item or a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefMenuEntry {
    /// The ref's name in its original case, with optional icon buttons.
    Header {
        label: String,
        tools: Vec<RefMenuTool>,
    },
    Item(SidebarMenuItem),
    Separator,
}

/// What the menus are built from; owned, so a menu can keep it in its
/// callbacks.
#[derive(Clone, Debug)]
pub struct RefMenuCtx {
    pub head: Option<Head>,
    /// Shared, so a submenu opening clones a pointer, not every ref.
    pub refs: std::rc::Rc<[Ref]>,
    /// The hosts the remotes lead to (see [`web_links::host_links`]); empty
    /// hides "Open on <Service>" and "Open release".
    pub links: Vec<HostLink>,
    pub targets: Vec<RemoteTargets>,
    /// The operation the repository is stopped in; Merge is off while there is
    /// one (git refuses to start another).
    pub operation: Operation,
    /// The branch the History is narrowed to.
    pub filter: Option<String>,
}

impl RefMenuCtx {
    /// The configured remote names.
    fn remote_names(&self) -> Vec<String> {
        self.targets.iter().map(|t| t.name.clone()).collect()
    }

    /// What "Merge … into {head}" names: the branch, or `HEAD` when detached.
    fn head_label(&self) -> &str {
        menu_model::head_label(self.head.as_ref())
    }

    fn can_merge(&self) -> bool {
        menu_model::can_merge(self.head.as_ref(), self.operation)
    }

    fn find(&self, kind: RefKind, name: &str) -> Option<&Ref> {
        self.refs.iter().find(|r| r.kind == kind && r.name == name)
    }

    /// "Open on <Service>" for the branch `key` (`local:<name>` or
    /// `rbranch:<remote>/<name>`).
    fn open_branch(&self, key: &str) -> Option<SidebarMenuItem> {
        let (link, target) = web_links::branch_target(&self.links, &self.targets, &self.refs, key)?;
        menu_model::open_item(
            link,
            &target,
            ui::t!("history.menu.open_on_host", service = link.service_name()),
        )
    }

    /// The label and action of the branch filter for `name`: narrows the
    /// History to the branch, or, when that filter is already active, clears it
    /// (re-applying it would reset the rows, selection and scroll for nothing).
    /// The header tool and the "Show only commits…" item share it, so the
    /// keyboard has the same action as the mouse.
    fn filter_action(&self, name: &str) -> (bool, String, SidebarAction) {
        if self.filter.as_deref() == Some(name) {
            (
                true,
                ui::t!("history.refmenu.clear_filter").into(),
                SidebarAction::ShowAllCommits,
            )
        } else {
            (
                false,
                ui::t!("history.refmenu.show_only", name = name).into(),
                SidebarAction::ShowBranchCommits(name.to_owned()),
            )
        }
    }

    /// The filter tool of a branch header.
    fn filter_tool(&self, name: &str) -> RefMenuTool {
        let (selected, tooltip, action) = self.filter_action(name);
        RefMenuTool {
            icon: IconName::Filter,
            tooltip,
            selected,
            action,
        }
    }

    /// The regular item with the filter tool's action.
    fn filter_item(&self, name: &str) -> Option<SidebarMenuItem> {
        let (_, label, action) = self.filter_action(name);
        Some(SidebarMenuItem::new(label, IconName::Filter).action(action))
    }
}

/// "Merge `name` into {head}…", on when `allowed`.
fn merge_item(ctx: &RefMenuCtx, name: &str, allowed: bool) -> Option<SidebarMenuItem> {
    Some(
        SidebarMenuItem::new(
            ui::t!(
                "history.refmenu.merge_into",
                name = name,
                head = ctx.head_label()
            ),
            IconName::Merge,
        )
        .maybe_action(allowed.then(|| SidebarAction::Merge(name.to_owned()))),
    )
}

/// "New branch from `name`…" at the ref's commit; off while that is unknown.
fn new_branch_item(name: &str, oid: Option<Oid>) -> Option<SidebarMenuItem> {
    Some(
        SidebarMenuItem::new(
            ui::t!("history.refmenu.new_branch_from", name = name),
            IconName::Branch,
        )
        .maybe_action(oid.map(SidebarAction::CreateBranchAt)),
    )
}

/// "Copy … name": copies the ref's name.
fn copy_name_item(label: impl Into<String>, name: &str) -> Option<SidebarMenuItem> {
    Some(
        SidebarMenuItem::new(label, IconName::Copy)
            .action(SidebarAction::CopyText(name.to_owned())),
    )
}

fn items(list: Vec<Option<SidebarMenuItem>>) -> Vec<RefMenuEntry> {
    list.into_iter().flatten().map(RefMenuEntry::Item).collect()
}

/// `header`, then the non-empty groups with a rule between them.
fn sectioned(header: RefMenuEntry, groups: Vec<Vec<RefMenuEntry>>) -> Vec<RefMenuEntry> {
    let mut entries = vec![header];
    for (i, group) in groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .enumerate()
    {
        if i > 0 {
            entries.push(RefMenuEntry::Separator);
        }
        entries.extend(group);
    }
    entries
}

/// The section of the local branch `name`.
///
/// `pr_kind` is the service of the branch's host, `None` without one: it hides
/// "Create pull request…" and names it "merge request" for GitLab.
pub fn local_section(
    ctx: &RefMenuCtx,
    name: &str,
    pr_kind: Option<ServiceKind>,
) -> Vec<RefMenuEntry> {
    let current = menu_model::is_current(ctx.head.as_ref(), name);
    let branch = ctx.find(RefKind::LocalBranch, name);
    let oid = branch.map(|r| r.target);
    let remotes = ctx.remote_names();
    // `None` without a remote: Push and Set upstream are hidden then.
    let push_remote = network_ops::push_remote_for(&ctx.refs, name, &remotes);
    let remote_known = !remotes.is_empty();

    let switch = {
        let item = SidebarMenuItem::new(
            ui::t!("history.refmenu.switch_to", name = name),
            IconName::Check,
        )
        .maybe_action((!current).then(|| SidebarAction::CheckoutBranch(name.to_owned())));
        Some(if current {
            item.hint(ui::t!("history.refmenu.current"))
        } else {
            item
        })
    };
    let merge = merge_item(ctx, name, ctx.can_merge() && !current);
    // A pull without an upstream only fails with "no upstream".
    let pull = (current && branch.is_some_and(|r| r.upstream.is_some())).then(|| {
        SidebarMenuItem::new(ui::t!("history.refmenu.pull"), IconName::Down)
            .action(SidebarAction::Pull)
    });
    let push = push_remote.filter(|_| remote_known).map(|remote| {
        SidebarMenuItem::new(
            ui::t!("history.refmenu.push_to", name = name, remote = remote),
            IconName::Up,
        )
        .action(SidebarAction::PushBranch(name.to_owned()))
    });
    let set_upstream = remote_known.then(|| {
        SidebarMenuItem::new(ui::t!("history.refmenu.set_upstream"), IconName::Link)
            .action(SidebarAction::SetUpstream(name.to_owned()))
    });
    let new_branch = new_branch_item(name, oid);
    let new_tag = Some(
        SidebarMenuItem::new(
            ui::t!("history.refmenu.new_tag_on", name = name),
            IconName::Tag,
        )
        .maybe_action(oid.map(SidebarAction::CreateTagAt)),
    );
    let rename = Some(
        SidebarMenuItem::new(
            ui::t!("history.refmenu.rename_branch", name = name),
            IconName::Edit,
        )
        .shortcut("F2")
        .action(SidebarAction::RenameBranch(name.to_owned())),
    );
    let open = ctx.open_branch(&format!("local:{name}"));
    let create_pr = pr_kind.map(|kind| {
        let label = match kind {
            ServiceKind::GitLab => ui::t!("history.refmenu.create_mr"),
            _ => ui::t!("history.refmenu.create_pr"),
        };
        SidebarMenuItem::new(label, IconName::Pr)
            .action(SidebarAction::CreatePullRequest(name.to_owned()))
    });
    let copy = copy_name_item(ui::t!("history.refmenu.copy_branch_name"), name);
    let delete = Some(
        SidebarMenuItem::new(
            ui::t!("history.refmenu.delete_branch", name = name),
            IconName::Trash,
        )
        .danger()
        .shortcut("Del")
        .maybe_action((!current).then(|| SidebarAction::DeleteBranch(name.to_owned()))),
    );

    sectioned(
        RefMenuEntry::Header {
            label: name.to_owned(),
            tools: vec![ctx.filter_tool(name)],
        },
        vec![
            items(vec![switch, merge, ctx.filter_item(name)]),
            items(vec![pull, push, set_upstream]),
            items(vec![new_branch, new_tag]),
            items(vec![rename, open, create_pr]),
            items(vec![copy, delete]),
        ],
    )
}

/// The section of the remote branch `name` (`origin/main`).
pub fn remote_section(ctx: &RefMenuCtx, name: &str) -> Vec<RefMenuEntry> {
    let oid = ctx.find(RefKind::RemoteBranch, name).map(|r| r.target);
    // Only a configured remote can be fetched, so no first-`/` guess here.
    let remote = web_links::split_remote_ref(name, &ctx.targets).map(|(remote, _)| remote);

    let checkout = Some(
        SidebarMenuItem::new(ui::t!("history.refmenu.checkout_as_local"), IconName::Check)
            .action(SidebarAction::CheckoutRemoteBranch(name.to_owned())),
    );
    let merge = merge_item(ctx, name, ctx.can_merge());
    let fetch = remote.map(|remote| {
        SidebarMenuItem::new(
            ui::t!("history.refmenu.fetch_remote", remote = remote),
            IconName::Fetch,
        )
        .action(SidebarAction::FetchRemote(remote.to_owned()))
    });
    let new_branch = new_branch_item(name, oid);
    let open = ctx.open_branch(&format!("rbranch:{name}"));
    let copy = copy_name_item(ui::t!("history.refmenu.copy_branch_name"), name);
    let delete = Some(
        SidebarMenuItem::new(
            ui::t!("history.refmenu.delete_remote_branch"),
            IconName::Trash,
        )
        .danger()
        .action(SidebarAction::DeleteRemoteBranch(name.to_owned())),
    );

    sectioned(
        RefMenuEntry::Header {
            label: name.to_owned(),
            tools: vec![ctx.filter_tool(name)],
        },
        vec![
            items(vec![checkout, merge, ctx.filter_item(name)]),
            items(vec![fetch, new_branch]),
            items(vec![open, copy]),
            items(vec![delete]),
        ],
    )
}

/// The section of the tag `name`.
pub fn tag_section(ctx: &RefMenuCtx, name: &str) -> Vec<RefMenuEntry> {
    let oid = ctx.find(RefKind::Tag, name).map(|r| r.target);

    let checkout = Some(
        SidebarMenuItem::new(ui::t!("history.refmenu.checkout_tag"), IconName::Check)
            .action(SidebarAction::CheckoutTag(name.to_owned())),
    );
    let new_branch = new_branch_item(name, oid);
    let remotes = ctx.remote_names();
    let push = network_ops::default_remote_of(&remotes)
        .filter(|_| menu_model::can_push_tag(&remotes))
        .map(|remote| {
            SidebarMenuItem::new(
                ui::t!("history.refmenu.push_tag", remote = remote),
                IconName::Up,
            )
            .action(SidebarAction::PushTag(name.to_owned()))
        });
    // Tags are opened on the first host (the upstream's, else origin's), as
    // the sidebar does.
    let open = ctx.links.first().and_then(|link| {
        menu_model::open_item(
            link,
            &WebTarget::Tag(name.to_owned()),
            ui::t!(
                "history.refmenu.open_release",
                service = link.service_name()
            ),
        )
    });
    let rename = Some(
        SidebarMenuItem::new(ui::t!("history.refmenu.rename_tag"), IconName::Edit)
            .shortcut("F2")
            .action(SidebarAction::RenameTag(name.to_owned())),
    );
    let copy = copy_name_item(ui::t!("history.refmenu.copy_tag_name"), name);
    let delete = Some(
        SidebarMenuItem::new(ui::t!("history.refmenu.delete_tag"), IconName::Trash)
            .danger()
            .shortcut("Del")
            .action(SidebarAction::DeleteTag(name.to_owned())),
    );

    sectioned(
        RefMenuEntry::Header {
            label: name.to_owned(),
            tools: Vec::new(),
        },
        vec![
            items(vec![checkout, new_branch, push]),
            items(vec![open, rename, copy]),
            items(vec![delete]),
        ],
    )
}

/// The menu of a ref row or a ref badge: a local branch's section followed by
/// the section of each remote branch merged into it (`main +origin` shows two
/// headers), a remote branch's or a tag's own section. The ref row's submenu
/// in the commit menu has the same content.
///
/// `pr_kind` is the service of a local branch's host (see [`local_section`]).
pub fn badge_sections(
    ctx: &RefMenuCtx,
    row: &RefRow,
    pr_kind: Option<ServiceKind>,
) -> Vec<RefMenuEntry> {
    match &row.kind {
        RefRowKind::Local { remotes, .. } => {
            let mut entries = local_section(ctx, &row.name, pr_kind);
            for remote in remotes {
                entries.push(RefMenuEntry::Separator);
                entries.extend(remote_section(ctx, &format!("{remote}/{}", row.name)));
            }
            entries
        }
        RefRowKind::Remote => remote_section(ctx, &row.name),
        RefRowKind::Tag => tag_section(ctx, &row.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::{Instance, RepoCoords};

    /// The service of a GitHub-hosted branch.
    const PR: Option<ServiceKind> = Some(ServiceKind::GitHub);

    fn oid(n: u8) -> Oid {
        Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap()
    }

    fn make_ref(kind: RefKind, name: &str, target: Oid) -> Ref {
        let prefix = match kind {
            RefKind::LocalBranch => "refs/heads/",
            RefKind::RemoteBranch => "refs/remotes/",
            _ => "refs/tags/",
        };
        Ref {
            full_name: format!("{prefix}{name}"),
            name: name.to_owned(),
            kind,
            target,
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn local(name: &str, target: Oid) -> Ref {
        make_ref(RefKind::LocalBranch, name, target)
    }

    fn remote(name: &str, target: Oid) -> Ref {
        make_ref(RefKind::RemoteBranch, name, target)
    }

    fn tag(name: &str, target: Oid) -> Ref {
        make_ref(RefKind::Tag, name, target)
    }

    fn head_of(name: &str) -> Head {
        Head::Branch {
            name: name.to_owned(),
            target: None,
        }
    }

    fn targets(names: &[&str]) -> Vec<RemoteTargets> {
        names
            .iter()
            .map(|name| RemoteTargets {
                name: (*name).to_owned(),
                fetch_url: format!("https://github.com/o/{name}.git"),
                push_urls: vec![format!("https://github.com/o/{name}.git")],
            })
            .collect()
    }

    fn github_link() -> HostLink {
        HostLink {
            remotes: vec!["origin".to_owned()],
            fetch_remotes: vec!["origin".to_owned()],
            instance: Instance::new(ServiceKind::GitHub, "github.com").unwrap(),
            coords: RepoCoords {
                namespace: vec!["o".to_owned()],
                repo: "r".to_owned(),
            },
        }
    }

    /// A context on `main` with a remote `origin`, a GitHub host and the refs.
    fn ctx(refs: Vec<Ref>) -> RefMenuCtx {
        RefMenuCtx {
            head: Some(head_of("main")),
            refs: refs.into(),
            links: vec![github_link()],
            targets: targets(&["origin"]),
            operation: Operation::None,
            filter: None,
        }
    }

    fn row_names(rows: &[RefRow]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    fn labels(entries: &[RefMenuEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                RefMenuEntry::Item(item) => Some(item.label.clone()),
                _ => None,
            })
            .collect()
    }

    fn item<'a>(entries: &'a [RefMenuEntry], label: &str) -> &'a SidebarMenuItem {
        entries
            .iter()
            .find_map(|entry| match entry {
                RefMenuEntry::Item(item) if item.label == label => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {label:?} in {:?}", labels(entries)))
    }

    fn has(entries: &[RefMenuEntry], label: &str) -> bool {
        labels(entries).iter().any(|l| l == label)
    }

    #[test]
    fn rows_are_head_then_locals_then_remote_only_then_tags() {
        let c = oid(1);
        let refs = vec![
            tag("v2", c),
            tag("v1", c),
            remote("upstream/zed", c),
            remote("origin/alpha", c),
            local("zeta", c),
            local("main", c),
            local("beta", c),
            local("elsewhere", oid(2)),
        ];
        let rows = commit_ref_rows(
            c,
            Some(&head_of("main")),
            &refs,
            &targets(&["origin", "upstream"]),
        );
        assert_eq!(
            row_names(&rows),
            [
                "main",
                "beta",
                "zeta",
                "origin/alpha",
                "upstream/zed",
                "v1",
                "v2"
            ]
        );
        assert!(rows[0].is_head() && !rows[1].is_head());
        assert_eq!(rows[0].icon(), IconName::Branch);
        assert_eq!(rows[3].icon(), IconName::Cloud);
        assert_eq!(rows[5].icon(), IconName::Tag);
    }

    #[test]
    fn a_remote_branch_merges_into_its_local_twin_and_origin_head_is_skipped() {
        let c = oid(1);
        let refs = vec![
            local("main", c),
            remote("origin/main", c),
            remote("upstream/main", c),
            remote("origin/HEAD", c),
            remote("origin/feat/x", c),
        ];
        let rows = commit_ref_rows(
            c,
            Some(&head_of("main")),
            &refs,
            &targets(&["origin", "upstream"]),
        );
        assert_eq!(row_names(&rows), ["main", "origin/feat/x"]);
        let RefRowKind::Local { remotes, .. } = &rows[0].kind else {
            panic!("main is a local row");
        };
        assert_eq!(remotes, &["origin", "upstream"]);
    }

    #[test]
    fn a_twin_on_another_commit_does_not_merge() {
        let c = oid(1);
        let refs = vec![local("main", c), remote("origin/main", oid(2))];
        let rows = commit_ref_rows(c, Some(&head_of("main")), &refs, &targets(&["origin"]));
        assert_eq!(row_names(&rows), ["main"]);
        let rows = commit_ref_rows(oid(2), Some(&head_of("main")), &refs, &targets(&["origin"]));
        assert_eq!(row_names(&rows), ["origin/main"]);
    }

    #[test]
    fn a_remote_with_a_slash_in_its_name_is_split_by_the_configured_remotes() {
        let c = oid(1);
        let refs = vec![local("x", c), remote("my/fork/x", c)];
        let rows = commit_ref_rows(c, None, &refs, &targets(&["my/fork"]));
        assert_eq!(row_names(&rows), ["x"]);
    }

    #[test]
    fn overflow_splits_at_more_than_six() {
        let rows = |n: usize| -> Vec<RefRow> {
            (0..n)
                .map(|i| RefRow {
                    kind: RefRowKind::Tag,
                    name: format!("t{i}"),
                })
                .collect()
        };
        let six = rows(6);
        let (shown, rest) = split_overflow(&six);
        assert_eq!((shown.len(), rest.len()), (6, 0));
        let seven = rows(7);
        let (shown, rest) = split_overflow(&seven);
        assert_eq!((shown.len(), rest.len()), (5, 2));
        assert_eq!(row_names(rest), ["t5", "t6"]);
        let nine = rows(9);
        let (shown, rest) = split_overflow(&nine);
        assert_eq!((shown.len(), rest.len()), (5, 4));
        let (shown, rest) = split_overflow(&[]);
        assert!(shown.is_empty() && rest.is_empty());
    }

    #[test]
    fn hints_name_head_remotes_and_ahead_behind() {
        let row = |head, remotes: &[&str], ahead, behind| RefRow {
            name: "main".to_owned(),
            kind: RefRowKind::Local {
                head,
                remotes: remotes.iter().map(|r| (*r).to_owned()).collect(),
                ahead,
                behind,
            },
        };
        assert_eq!(
            row_hint(&row(true, &["origin"], 2, 1)).as_deref(),
            Some("HEAD +origin ↑2 ↓1")
        );
        assert_eq!(
            row_hint(&row(false, &["origin"], 0, 0)).as_deref(),
            Some("+origin")
        );
        assert_eq!(row_hint(&row(true, &[], 0, 0)).as_deref(), Some("HEAD"));
        assert_eq!(row_hint(&row(false, &[], 0, 3)).as_deref(), Some("↓3"));
        assert_eq!(row_hint(&row(false, &[], 0, 0)), None);
        let remote_only = RefRow {
            name: "origin/x".to_owned(),
            kind: RefRowKind::Remote,
        };
        assert_eq!(row_hint(&remote_only), None);
        let tag_row = RefRow {
            name: "v1".to_owned(),
            kind: RefRowKind::Tag,
        };
        assert_eq!(row_hint(&tag_row), None);
    }

    #[test]
    fn ahead_and_behind_come_from_the_local_ref() {
        let c = oid(1);
        let mut main = local("main", c);
        main.upstream = Some("origin/main".to_owned());
        main.ahead = 2;
        main.behind = 1;
        let refs = vec![main, remote("origin/main", c)];
        let rows = commit_ref_rows(c, Some(&head_of("main")), &refs, &targets(&["origin"]));
        assert_eq!(row_hint(&rows[0]).as_deref(), Some("HEAD +origin ↑2 ↓1"));
    }

    #[test]
    fn a_non_head_local_section_is_complete_and_in_the_design_order() {
        let c = oid(1);
        let ctx = ctx(vec![local("main", oid(9)), local("feat", c)]);
        let section = local_section(&ctx, "feat", PR);
        assert_eq!(
            section[0],
            RefMenuEntry::Header {
                label: "feat".to_owned(),
                tools: vec![RefMenuTool {
                    icon: IconName::Filter,
                    tooltip: "Show only commits of feat".to_owned(),
                    selected: false,
                    action: SidebarAction::ShowBranchCommits("feat".to_owned()),
                }],
            }
        );
        assert_eq!(
            labels(&section),
            [
                "Switch to feat",
                "Merge feat into main…",
                "Show only commits of feat",
                "Push feat to origin",
                "Set upstream…",
                "New branch from feat…",
                "New tag on feat…",
                "Rename feat…",
                "Create pull request…",
                "Copy branch name",
                "Delete feat…",
            ]
        );
        assert!(!has(&section, "Pull"));
        assert!(item(&section, "Switch to feat").enabled());
        assert!(item(&section, "Merge feat into main…").enabled());
        assert_eq!(
            item(&section, "Push feat to origin").action,
            SidebarAction::PushBranch("feat".to_owned())
        );
        assert_eq!(
            item(&section, "New branch from feat…").action,
            SidebarAction::CreateBranchAt(c)
        );
        assert_eq!(
            item(&section, "New tag on feat…").action,
            SidebarAction::CreateTagAt(c)
        );
        let delete = item(&section, "Delete feat…");
        assert!(delete.danger && delete.enabled());
        assert_eq!(
            delete.action,
            SidebarAction::DeleteBranch("feat".to_owned())
        );
        assert_eq!(
            item(&section, "Rename feat…").shortcut.as_deref(),
            Some("F2")
        );
        // No consecutive, leading or trailing rules.
        assert!(!matches!(section[1], RefMenuEntry::Separator));
        assert!(!matches!(section.last(), Some(RefMenuEntry::Separator)));
        assert!(section
            .windows(2)
            .all(|w| !(w[0] == RefMenuEntry::Separator && w[1] == RefMenuEntry::Separator)));
    }

    #[test]
    fn the_head_branch_section_switches_off_and_pulls() {
        let mut main = local("main", oid(1));
        main.upstream = Some("origin/main".to_owned());
        let ctx = ctx(vec![main.clone()]);
        let section = local_section(&ctx, "main", PR);
        let switch = item(&section, "Switch to main");
        assert!(!switch.enabled());
        assert_eq!(switch.action, SidebarAction::None);
        assert_eq!(switch.hint.as_deref(), Some("current"));
        assert!(!item(&section, "Merge main into main…").enabled());
        let delete = item(&section, "Delete main…");
        assert!(!delete.enabled() && delete.danger);
        let pull = item(&section, "Pull");
        assert_eq!(pull.action, SidebarAction::Pull);
        assert!(has(&section, "Push main to origin"));
        // Without an upstream there is nothing to pull from.
        main.upstream = None;
        let ctx = RefMenuCtx {
            refs: vec![main].into(),
            ..ctx
        };
        assert!(!has(&local_section(&ctx, "main", PR), "Pull"));
    }

    #[test]
    fn a_branch_is_pushed_to_its_upstream_remote() {
        let mut feat = local("feat", oid(1));
        feat.upstream = Some("fork/feat".to_owned());
        let mut ctx = ctx(vec![feat]);
        ctx.targets = targets(&["origin", "fork"]);
        let section = local_section(&ctx, "feat", PR);
        assert!(has(&section, "Push feat to fork"));
    }

    #[test]
    fn merge_is_off_on_a_detached_head() {
        let mut ctx = ctx(vec![local("feat", oid(1)), remote("origin/feat", oid(1))]);
        ctx.head = Some(Head::Detached(oid(5)));
        let section = local_section(&ctx, "feat", PR);
        let merge = item(&section, "Merge feat into HEAD…");
        assert!(!merge.enabled());
        assert!(item(&section, "Switch to feat").enabled());
        assert!(!has(&section, "Pull"));
        assert!(!item(
            &remote_section(&ctx, "origin/feat"),
            "Merge origin/feat into HEAD…"
        )
        .enabled());
    }

    #[test]
    fn merge_is_off_while_an_operation_is_stopped() {
        for operation in [
            Operation::Merge,
            Operation::Rebase { progress: None },
            Operation::CherryPick,
        ] {
            let mut ctx = ctx(vec![local("feat", oid(1)), remote("origin/feat", oid(1))]);
            ctx.operation = operation;
            assert!(!item(&local_section(&ctx, "feat", PR), "Merge feat into main…").enabled());
            assert!(!item(
                &remote_section(&ctx, "origin/feat"),
                "Merge origin/feat into main…"
            )
            .enabled());
        }
    }

    #[test]
    fn delete_shows_the_del_shortcut_like_the_sidebar() {
        let c = oid(1);
        let ctx = ctx(vec![local("feat", c), tag("v1", c)]);
        let local_menu = local_section(&ctx, "feat", PR);
        assert_eq!(
            item(&local_menu, "Delete feat…").shortcut.as_deref(),
            Some("Del")
        );
        let tag_menu = tag_section(&ctx, "v1");
        assert_eq!(
            item(&tag_menu, "Delete tag…").shortcut.as_deref(),
            Some("Del")
        );
    }

    #[test]
    fn a_branch_named_head_is_not_current_when_detached() {
        let c = oid(1);
        let mut ctx = ctx(vec![local("HEAD", c)]);
        ctx.head = Some(Head::Detached(c));
        let menu = local_section(&ctx, "HEAD", PR);
        assert!(item(&menu, "Switch to HEAD").enabled());
    }

    #[test]
    fn without_a_remote_push_upstream_fetch_and_tag_push_are_hidden() {
        let c = oid(1);
        let mut ctx = ctx(vec![local("feat", c), tag("v1", c)]);
        ctx.targets.clear();
        let local_labels = labels(&local_section(&ctx, "feat", PR));
        assert!(local_labels
            .iter()
            .all(|l| !l.starts_with("Push") && l != "Set upstream…"));
        assert!(local_labels.contains(&"Switch to feat".to_owned()));
        let tag_labels = labels(&tag_section(&ctx, "v1"));
        assert!(tag_labels.iter().all(|l| !l.starts_with("Push")));
        // A remote-only ref whose remote is not configured has nothing to fetch.
        let ctx = RefMenuCtx {
            refs: vec![remote("gone/x", c)].into(),
            ..ctx
        };
        assert!(labels(&remote_section(&ctx, "gone/x"))
            .iter()
            .all(|l| !l.starts_with("Fetch")));
    }

    #[test]
    fn without_a_host_open_and_create_pr_are_hidden() {
        let c = oid(1);
        let mut ctx = ctx(vec![
            local("feat", c),
            remote("origin/feat", c),
            tag("v1", c),
        ]);
        ctx.links.clear();
        for section in [
            local_section(&ctx, "feat", None),
            remote_section(&ctx, "origin/feat"),
            tag_section(&ctx, "v1"),
        ] {
            assert!(labels(&section).iter().all(|l| !l.starts_with("Open")
                && !l.contains("pull request")
                && !l.contains("merge request")));
        }
    }

    #[test]
    fn open_items_name_the_service_and_the_pr_label_follows_the_kind() {
        let c = oid(1);
        let mut feat = local("feat", c);
        feat.upstream = Some("origin/feat".to_owned());
        let ctx = ctx(vec![feat, remote("origin/feat", c), tag("v1", c)]);
        let local = local_section(&ctx, "feat", PR);
        assert!(matches!(
            item(&local, "Open on GitHub").action,
            SidebarAction::OpenUrl { .. }
        ));
        assert!(has(&remote_section(&ctx, "origin/feat"), "Open on GitHub"));
        assert!(has(&tag_section(&ctx, "v1"), "Open release on GitHub"));
        assert_eq!(
            item(&local, "Create pull request…").action,
            SidebarAction::CreatePullRequest("feat".to_owned())
        );
        assert!(has(
            &local_section(&ctx, "feat", Some(ServiceKind::GitLab)),
            "Create merge request…"
        ));
    }

    #[test]
    fn the_filter_tool_is_selected_for_the_filtered_branch() {
        let mut ctx = ctx(vec![local("feat", oid(1))]);
        ctx.filter = Some("feat".to_owned());
        let RefMenuEntry::Header { tools, .. } = &local_section(&ctx, "feat", PR)[0] else {
            panic!("a section starts with its header");
        };
        assert!(tools[0].selected);
        // Already filtered: the tool clears the filter instead of re-applying it.
        assert_eq!(tools[0].action, SidebarAction::ShowAllCommits);
        assert_eq!(tools[0].tooltip, "Clear branch filter");
        // The keyboard-reachable item does the same.
        let section = local_section(&ctx, "feat", PR);
        assert_eq!(
            item(&section, "Clear branch filter").action,
            SidebarAction::ShowAllCommits
        );
        assert_eq!(
            item(
                &local_section(&ctx, "main", PR),
                "Show only commits of main"
            )
            .action,
            SidebarAction::ShowBranchCommits("main".to_owned())
        );
        let RefMenuEntry::Header { tools, .. } = &local_section(&ctx, "main", PR)[0] else {
            panic!("a section starts with its header");
        };
        assert!(!tools[0].selected);
    }

    #[test]
    fn remote_section_has_the_design_items() {
        let c = oid(1);
        let ctx = ctx(vec![remote("origin/feat/x", c)]);
        let section = remote_section(&ctx, "origin/feat/x");
        let RefMenuEntry::Header { label, .. } = &section[0] else {
            panic!("a section starts with its header");
        };
        assert_eq!(label, "origin/feat/x");
        assert_eq!(
            labels(&section),
            [
                "Checkout as local branch",
                "Merge origin/feat/x into main…",
                "Show only commits of origin/feat/x",
                "Fetch origin",
                "New branch from origin/feat/x…",
                "Open on GitHub",
                "Copy branch name",
                "Delete remote branch…",
            ]
        );
        assert_eq!(
            item(&section, "Fetch origin").action,
            SidebarAction::FetchRemote("origin".to_owned())
        );
        let delete = item(&section, "Delete remote branch…");
        assert!(delete.danger);
        assert_eq!(
            delete.action,
            SidebarAction::DeleteRemoteBranch("origin/feat/x".to_owned())
        );
    }

    #[test]
    fn tag_section_has_the_design_items() {
        let c = oid(1);
        let ctx = ctx(vec![tag("v0.3.0", c)]);
        let section = tag_section(&ctx, "v0.3.0");
        assert_eq!(
            labels(&section),
            [
                "Checkout tag",
                "New branch from v0.3.0…",
                "Push tag to origin",
                "Open release on GitHub",
                "Rename tag…",
                "Copy tag name",
                "Delete tag…",
            ]
        );
        assert_eq!(
            item(&section, "New branch from v0.3.0…").action,
            SidebarAction::CreateBranchAt(c)
        );
        assert_eq!(
            item(&section, "Push tag to origin").action,
            SidebarAction::PushTag("v0.3.0".to_owned())
        );
        assert!(item(&section, "Delete tag…").danger);
    }

    #[test]
    fn a_merged_badge_shows_both_sections() {
        let c = oid(1);
        let refs = vec![local("main", c), remote("origin/main", c)];
        let ctx = ctx(refs.clone());
        let rows = commit_ref_rows(c, ctx.head.as_ref(), &refs, &ctx.targets);
        let entries = badge_sections(&ctx, &rows[0], PR);
        let headers: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                RefMenuEntry::Header { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["main", "origin/main"]);
        assert!(has(&entries, "Fetch origin"));
        // A tag or a remote-only badge is a single section.
        let tag_row = RefRow {
            kind: RefRowKind::Tag,
            name: "v1".to_owned(),
        };
        assert_eq!(badge_sections(&ctx, &tag_row, PR), tag_section(&ctx, "v1"));
    }

    #[test]
    fn a_planned_item_never_appears() {
        let c = oid(1);
        let mut feat = local("feat", c);
        feat.upstream = Some("origin/feat".to_owned());
        feat.behind = 2;
        let ctx = ctx(vec![
            local("main", c),
            feat,
            remote("origin/feat", c),
            tag("v1", c),
        ]);
        let mut all = Vec::new();
        all.extend(local_section(&ctx, "main", PR));
        all.extend(local_section(&ctx, "feat", PR));
        all.extend(remote_section(&ctx, "origin/feat"));
        all.extend(tag_section(&ctx, "v1"));
        for label in labels(&all) {
            for planned in [
                "Rebase",
                "Fast-forward",
                "Compare",
                "Hide from graph",
                "Interactive",
            ] {
                assert!(!label.contains(planned), "{label} is a Planned item");
            }
        }
        for entry in &all {
            if let RefMenuEntry::Header { tools, .. } = entry {
                assert!(tools.iter().all(|tool| tool.icon == IconName::Filter));
            }
        }
    }

    #[test]
    fn a_disabled_item_has_no_action() {
        let ctx = ctx(vec![local("main", oid(1))]);
        for entry in local_section(&ctx, "main", PR) {
            if let RefMenuEntry::Item(item) = entry {
                assert_eq!(
                    item.enabled(),
                    item.action != SidebarAction::None,
                    "{}",
                    item.label
                );
            }
        }
    }
}
