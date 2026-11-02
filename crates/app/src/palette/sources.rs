//! What the palette searches and how a query turns into grouped hits.
//!
//! Everything here is plain data and pure functions: the view builds a
//! [`Sources`] snapshot when it opens and calls [`search`] on every keystroke.

use std::cmp::Ordering;
use std::io;
use std::path::Path;
use std::process::Command as Process;
use std::sync::Arc;

use git_core::{CommitInfo, Git, Head, Oid, Ref, RefKind};
use gpui::{App, Global};
use ui::IconName;

use super::matcher::{self, Match};
use crate::settings_ui::Page;

/// The most hits one group shows.
pub const GROUP_CAP: usize = 50;
/// Commands shown for an empty query.
const EMPTY_COMMANDS: usize = 8;
/// Branches shown for an empty query.
const EMPTY_BRANCHES: usize = 5;
/// How many recent commands and branches are remembered.
const RECENT_CAP: usize = 16;
/// Characters of the abbreviated hash a Commits row shows.
const SHORT_HASH: usize = 7;
/// The gap between the abbreviated hash and the summary in a Commits row.
const HASH_GAP: &str = "  ";

/// The palette's groups, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Commands,
    Branches,
    Commits,
    Files,
}

impl Group {
    /// The header text, already upper case: GPUI has no text-transform.
    pub fn header(self) -> &'static str {
        match self {
            Group::Commands => "COMMANDS",
            Group::Branches => "BRANCHES",
            Group::Commits => "COMMITS",
            Group::Files => "FILES",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Group::Commands => IconName::Command,
            Group::Branches => IconName::Branch,
            Group::Commits => IconName::Hash,
            Group::Files => IconName::File,
        }
    }
}

/// An app action the palette can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    FetchAll,
    Pull,
    Push,
    StashChanges,
    SwitchBranch,
    CreateBranchHere,
    CreateTagHere,
    ShowBranchCommits,
    AddRemote,
    AddWorktree,
    Refresh,
    ToggleSidebar,
    ToggleDetails,
    ToggleActivity,
    FocusSearch,
    ShowHistory,
    ShowChanges,
    ShowWorktrees,
    CollapseIdleLanes,
    FitGraph,
    OpenSettings(Page),
    OpenRepository,
    Clone,
    CloseRepository,
}

/// The view state two commands' titles depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewState {
    pub idle_lanes_collapsed: bool,
    pub activity_collapsed: bool,
}

impl Command {
    /// Every command, in no particular order.
    pub fn all() -> Vec<Command> {
        let mut all = vec![
            Command::FetchAll,
            Command::Pull,
            Command::Push,
            Command::StashChanges,
            Command::SwitchBranch,
            Command::CreateBranchHere,
            Command::CreateTagHere,
            Command::AddRemote,
            Command::AddWorktree,
            Command::Refresh,
            Command::ToggleSidebar,
            Command::ToggleDetails,
            Command::ToggleActivity,
            Command::FocusSearch,
            Command::ShowHistory,
            Command::ShowChanges,
            Command::ShowWorktrees,
            Command::CollapseIdleLanes,
            Command::FitGraph,
            Command::ShowBranchCommits,
            Command::OpenRepository,
            Command::Clone,
            Command::CloseRepository,
        ];
        all.extend(Page::ALL.into_iter().map(Command::OpenSettings));
        all
    }

    pub fn title(self, state: ViewState) -> String {
        match self {
            Command::FetchAll => "Fetch all remotes".into(),
            Command::Pull => "Pull current branch".into(),
            Command::Push => "Push current branch".into(),
            Command::StashChanges => "Stash local changes".into(),
            Command::SwitchBranch => "Switch branch…".into(),
            Command::CreateBranchHere => "Create branch here…".into(),
            Command::CreateTagHere => "Create tag here…".into(),
            Command::ShowBranchCommits => "Show all commits of current branch".into(),
            Command::AddRemote => "Add remote…".into(),
            Command::AddWorktree => "Add worktree…".into(),
            Command::Refresh => "Refresh repository".into(),
            Command::ToggleSidebar => "Toggle sidebar".into(),
            Command::ToggleDetails => "Toggle details dock".into(),
            Command::ToggleActivity if state.activity_collapsed => "Show Activity strip".into(),
            Command::ToggleActivity => "Hide Activity strip".into(),
            Command::FocusSearch => "Search commits".into(),
            Command::ShowHistory => "Switch to History".into(),
            Command::ShowChanges => "Switch to Local changes".into(),
            Command::ShowWorktrees => "Switch to Worktrees".into(),
            Command::CollapseIdleLanes if state.idle_lanes_collapsed => {
                "Expand idle lanes in graph".into()
            }
            Command::CollapseIdleLanes => "Collapse idle lanes in graph".into(),
            Command::FitGraph => "Fit graph to its lanes".into(),
            Command::OpenSettings(Page::General) => "Open Settings".into(),
            Command::OpenSettings(page) => format!("Open Settings → {}", page_name(page)),
            Command::OpenRepository => "Open repository…".into(),
            Command::Clone => "Clone repository…".into(),
            Command::CloseRepository => "Close repository".into(),
        }
    }

    /// The commands the workspace runs (they replace or cover the repository
    /// view); the repository view runs the rest.
    pub fn is_workspace_level(self) -> bool {
        matches!(
            self,
            Command::OpenSettings(_)
                | Command::OpenRepository
                | Command::Clone
                | Command::CloseRepository
        )
    }

    /// The key cap of the command's binding: the user's override from the
    /// Keybindings page when the command has an entry there, else the fixed
    /// shortcut the app binds. `None` for a command without a shortcut.
    pub fn key_cap(self, cx: &App) -> Option<String> {
        let id = match self {
            Command::FetchAll => "fetch",
            Command::Pull => "pull",
            Command::Push => "push",
            Command::SwitchBranch => "branch",
            Command::ToggleSidebar => "sidebar",
            Command::ToggleDetails => "dock",
            Command::FocusSearch => "search",
            Command::OpenSettings(Page::General) => "settings",
            // Bound in `main.rs` without an entry on the Keybindings page.
            Command::StashChanges => return Some("Ctrl Shift S".into()),
            Command::OpenRepository => return Some("Ctrl O".into()),
            Command::Clone => return Some("Ctrl Shift O".into()),
            _ => return None,
        };
        crate::settings_ui::keybindings::key_cap(cx, id)
    }
}

/// The page's name in the settings navigation.
fn page_name(page: Page) -> &'static str {
    match page {
        Page::General => "General",
        Page::Appearance => "Appearance",
        Page::Git => "Git",
        Page::Worktrees => "Worktrees",
        Page::Ssh => "SSH keys",
        Page::Keybindings => "Keybindings",
        Page::Diff => "Diff & merge",
        Page::Identity => "Identity",
    }
}

/// What picking a hit does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Command(Command),
    Branch { name: String, remote: bool },
    Commit(Oid),
    File(String),
}

/// One row of the results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub group: Group,
    pub target: Target,
    /// The text shown; `positions` index its characters.
    pub title: String,
    pub positions: Vec<usize>,
    /// The checked-out branch: it gets the `current` badge.
    pub current: bool,
}

/// The commands and branches used most recently this session, newest first.
/// Kept in a global so a new palette starts from the last one's habits.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    commands: Vec<Command>,
    branches: Vec<String>,
}

impl Global for Recent {}

impl Recent {
    pub fn touch_command(&mut self, command: Command) {
        self.commands.retain(|c| *c != command);
        self.commands.insert(0, command);
        self.commands.truncate(RECENT_CAP);
    }

    pub fn touch_branch(&mut self, name: &str) {
        self.branches.retain(|b| b != name);
        self.branches.insert(0, name.to_owned());
        self.branches.truncate(RECENT_CAP);
    }
}

struct BranchSource {
    name: String,
    remote: bool,
}

struct CommitSource {
    oid: Oid,
    hex: String,
    info: Arc<CommitInfo>,
}

/// The snapshot a palette searches.
pub struct Sources {
    state: ViewState,
    current: Option<String>,
    branches: Vec<BranchSource>,
    commits: Vec<CommitSource>,
    files: Vec<String>,
}

impl Sources {
    /// `commits` are the loaded history rows, newest first.
    pub fn new(
        state: ViewState,
        head: Option<&Head>,
        refs: &[Ref],
        commits: &[Arc<CommitInfo>],
    ) -> Self {
        let current = match head {
            Some(Head::Branch { name, .. }) => Some(name.clone()),
            _ => None,
        };
        let branches_of = |kind: RefKind| {
            refs.iter()
                .filter(move |r| r.kind == kind)
                // `origin/HEAD` is a pointer, not a branch to switch to.
                .filter(move |r| kind != RefKind::RemoteBranch || !r.name.ends_with("/HEAD"))
                .map(move |r| BranchSource {
                    name: r.name.clone(),
                    remote: kind == RefKind::RemoteBranch,
                })
        };
        let branches = branches_of(RefKind::LocalBranch)
            .chain(branches_of(RefKind::RemoteBranch))
            .collect();
        let commits = commits
            .iter()
            .map(|info| CommitSource {
                oid: info.id,
                hex: info.id.to_string(),
                info: info.clone(),
            })
            .collect();
        Self {
            state,
            current,
            branches,
            commits,
            files: Vec::new(),
        }
    }

    /// The tracked files, once they have loaded.
    pub fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
    }
}

/// The hits for `query`, grouped in display order and capped per group.
/// An empty query lists the habitual commands and branches instead.
pub fn search(query: &str, sources: &Sources, recent: &Recent) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return default_hits(sources, recent);
    }
    let mut hits = Vec::new();

    let commands = Command::all();
    let titles: Vec<String> = commands
        .iter()
        .map(|command| command.title(sources.state))
        .collect();
    for scored in top(titles
        .iter()
        .enumerate()
        .filter_map(|(order, title)| scored(matcher::fuzzy(query, title), title, order)))
    {
        hits.push(Hit {
            group: Group::Commands,
            target: Target::Command(commands[scored.order]),
            title: titles[scored.order].clone(),
            positions: scored.positions,
            current: false,
        });
    }

    for scored in top(sources
        .branches
        .iter()
        .enumerate()
        .filter_map(|(order, b)| scored(matcher::fuzzy(query, &b.name), &b.name, order)))
    {
        let branch = &sources.branches[scored.order];
        hits.push(Hit {
            group: Group::Branches,
            target: Target::Branch {
                name: branch.name.clone(),
                remote: branch.remote,
            },
            title: branch.name.clone(),
            positions: scored.positions,
            current: is_current(sources, branch),
        });
    }

    for scored in top(sources.commits.iter().enumerate().filter_map(|(order, c)| {
        let by_hash = matcher::hash_prefix(query, &c.hex);
        let by_summary = matcher::fuzzy(query, &c.info.summary);
        // Positions index the displayed `hash  summary`, not the summary.
        let shift = SHORT_HASH + HASH_GAP.len();
        let by_summary = by_summary.map(|m| Match {
            score: m.score,
            positions: m.positions.into_iter().map(|p| p + shift).collect(),
        });
        let by_hash = by_hash.map(|m| Match {
            score: m.score,
            positions: m
                .positions
                .into_iter()
                .filter(|p| *p < SHORT_HASH)
                .collect(),
        });
        // Newest first among equals: no length tie-break for commits.
        let best = match (by_hash, by_summary) {
            (Some(hash), _) => hash,
            (None, Some(summary)) => summary,
            (None, None) => return None,
        };
        Some(Scored {
            score: best.score,
            len: 0,
            order,
            positions: best.positions,
        })
    })) {
        let commit = &sources.commits[scored.order];
        hits.push(Hit {
            group: Group::Commits,
            target: Target::Commit(commit.oid),
            title: commit_title(commit),
            positions: scored.positions,
            current: false,
        });
    }

    for scored in top(sources
        .files
        .iter()
        .enumerate()
        .filter_map(|(order, path)| scored(matcher::fuzzy(query, path), path, order)))
    {
        hits.push(Hit {
            group: Group::Files,
            target: Target::File(sources.files[scored.order].clone()),
            title: sources.files[scored.order].clone(),
            positions: scored.positions,
            current: false,
        });
    }
    hits
}

fn commit_title(commit: &CommitSource) -> String {
    format!(
        "{}{HASH_GAP}{}",
        &commit.hex[..SHORT_HASH.min(commit.hex.len())],
        commit.info.summary
    )
}

fn is_current(sources: &Sources, branch: &BranchSource) -> bool {
    !branch.remote && sources.current.as_deref() == Some(branch.name.as_str())
}

/// Commands (the recent ones first, then A–Z) and branches (the current one,
/// the recent ones, then the rest) for an empty query.
fn default_hits(sources: &Sources, recent: &Recent) -> Vec<Hit> {
    let state = sources.state;
    let all = Command::all();
    let mut commands: Vec<Command> = recent
        .commands
        .iter()
        .copied()
        .filter(|c| all.contains(c))
        .collect();
    let mut rest: Vec<Command> = all.into_iter().filter(|c| !commands.contains(c)).collect();
    rest.sort_by_cached_key(|c| c.title(state).to_lowercase());
    commands.extend(rest);
    commands.truncate(EMPTY_COMMANDS);
    let mut hits: Vec<Hit> = commands
        .into_iter()
        .map(|command| Hit {
            group: Group::Commands,
            target: Target::Command(command),
            title: command.title(state),
            positions: Vec::new(),
            current: false,
        })
        .collect();

    let mut order: Vec<&BranchSource> = Vec::new();
    let by_name = |name: &str| sources.branches.iter().find(|b| b.name == name);
    if let Some(current) = sources.current.as_deref().and_then(by_name) {
        order.push(current);
    }
    for name in &recent.branches {
        if let Some(branch) = by_name(name) {
            if !order.iter().any(|b| std::ptr::eq(*b, branch)) {
                order.push(branch);
            }
        }
    }
    for branch in sources.branches.iter().filter(|b| !b.remote) {
        if !order.iter().any(|b| std::ptr::eq(*b, branch)) {
            order.push(branch);
        }
    }
    order.truncate(EMPTY_BRANCHES);
    hits.extend(order.into_iter().map(|branch| Hit {
        group: Group::Branches,
        target: Target::Branch {
            name: branch.name.clone(),
            remote: branch.remote,
        },
        title: branch.name.clone(),
        positions: Vec::new(),
        current: is_current(sources, branch),
    }));
    hits
}

/// The tracked files of the work tree at `root` (`git ls-files`), as
/// repository-relative paths in git's order. Blocks while git runs: call it
/// on the background executor.
pub fn tracked_files(git: &Git, root: &Path) -> io::Result<Vec<String>> {
    let mut command = Process::new(git.path());
    // -z: paths come NUL-separated and unquoted, whatever they contain.
    command
        .arg("-C")
        .arg(root)
        .args(["--no-pager", "ls-files", "-z"])
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        // Never pop up a console window on Windows.
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(parse_ls_files(&output.stdout))
}

/// Splits `git ls-files -z` output into paths.
fn parse_ls_files(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// One match before it becomes a [`Hit`].
struct Scored {
    score: i32,
    /// Shorter texts win ties: `Push` over `Push tags to the default remote`.
    len: usize,
    /// Index into the source list; the source's own order breaks last ties.
    order: usize,
    positions: Vec<usize>,
}

fn scored(found: Option<Match>, text: &str, order: usize) -> Option<Scored> {
    found.map(|m| Scored {
        score: m.score,
        len: text.chars().count(),
        order,
        positions: m.positions,
    })
}

/// The best [`GROUP_CAP`] matches, best first.
fn top(matches: impl Iterator<Item = Scored>) -> Vec<Scored> {
    let mut all: Vec<Scored> = matches.collect();
    let rank = |a: &Scored, b: &Scored| -> Ordering {
        b.score
            .cmp(&a.score)
            .then(a.len.cmp(&b.len))
            .then(a.order.cmp(&b.order))
    };
    if all.len() > GROUP_CAP {
        all.select_nth_unstable_by(GROUP_CAP, rank);
        all.truncate(GROUP_CAP);
    }
    all.sort_by(rank);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(byte: u8) -> Oid {
        Oid::from_hex(&format!("{byte:02x}").repeat(20)).expect("valid hex")
    }

    fn commit(byte: u8, summary: &str) -> Arc<CommitInfo> {
        Arc::new(CommitInfo {
            id: oid(byte),
            parents: Vec::new(),
            author_name: "A".into(),
            author_email: "a@example.com".into(),
            author_time: 0,
            summary: summary.into(),
        })
    }

    fn reference(kind: RefKind, name: &str) -> Ref {
        Ref {
            full_name: format!("refs/{name}"),
            name: name.into(),
            kind,
            target: oid(1),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn sources(commits: &[Arc<CommitInfo>]) -> Sources {
        let refs = [
            reference(RefKind::LocalBranch, "main"),
            reference(RefKind::LocalBranch, "feat/palette"),
            reference(RefKind::RemoteBranch, "origin/main"),
            reference(RefKind::RemoteBranch, "origin/HEAD"),
            reference(RefKind::Tag, "v1.0"),
        ];
        let head = Head::Branch {
            name: "main".into(),
            target: Some(oid(1)),
        };
        Sources::new(ViewState::default(), Some(&head), &refs, commits)
    }

    fn titles(hits: &[Hit], group: Group) -> Vec<&str> {
        hits.iter()
            .filter(|h| h.group == group)
            .map(|h| h.title.as_str())
            .collect()
    }

    #[test]
    fn results_come_grouped_in_display_order() {
        let mut s = sources(&[commit(0xa1, "Fix the feature picker")]);
        s.set_files(vec!["src/feature.rs".into()]);
        let hits = search("fe", &s, &Recent::default());
        let groups: Vec<Group> = hits.iter().map(|h| h.group).collect();
        let mut sorted = groups.clone();
        sorted.sort_by_key(|g| *g as u8);
        assert_eq!(groups, sorted);
        for group in [
            Group::Commands,
            Group::Branches,
            Group::Commits,
            Group::Files,
        ] {
            assert!(!titles(&hits, group).is_empty(), "{group:?} has hits");
        }
    }

    #[test]
    fn remote_head_pointers_are_not_branches() {
        let hits = search("origin", &sources(&[]), &Recent::default());
        assert_eq!(titles(&hits, Group::Branches), vec!["origin/main"]);
    }

    #[test]
    fn the_current_branch_is_flagged() {
        let hits = search("main", &sources(&[]), &Recent::default());
        let current: Vec<&str> = hits
            .iter()
            .filter(|h| h.current)
            .map(|h| h.title.as_str())
            .collect();
        assert_eq!(current, vec!["main"]);
    }

    #[test]
    fn a_hash_prefix_leads_the_commits_over_better_summary_matches() {
        let commits = [
            commit(0xb2, "a1a1 is mentioned here"),
            commit(0xa1, "Something else"),
        ];
        let hits = search("a1a1", &sources(&commits), &Recent::default());
        let found: Vec<&Hit> = hits.iter().filter(|h| h.group == Group::Commits).collect();
        assert_eq!(found[0].target, Target::Commit(oid(0xa1)));
        assert_eq!(found[0].title, "a1a1a1a  Something else");
        // The highlight sits on the abbreviated hash.
        assert_eq!(found[0].positions, vec![0, 1, 2, 3]);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_summary_match_highlights_the_summary_part_of_the_row() {
        let hits = search(
            "parser",
            &sources(&[commit(0xa1, "Fix parser")]),
            &Recent::default(),
        );
        let hit = hits
            .iter()
            .find(|h| h.group == Group::Commits)
            .expect("a commit hit");
        let text: String = hit
            .positions
            .iter()
            .filter_map(|p| hit.title.chars().nth(*p))
            .collect();
        assert_eq!(text, "parser");
    }

    #[test]
    fn no_match_returns_no_hits() {
        let mut s = sources(&[commit(0xa1, "Fix")]);
        s.set_files(vec!["a.rs".into()]);
        assert!(search("zzzzqqq", &s, &Recent::default()).is_empty());
    }

    #[test]
    fn each_group_is_capped() {
        let commits: Vec<_> = (0..200u8).map(|i| commit(i, "same summary")).collect();
        let mut s = sources(&commits);
        s.set_files((0..300).map(|i| format!("src/file{i}.rs")).collect());
        let hits = search("s", &s, &Recent::default());
        assert_eq!(titles(&hits, Group::Commits).len(), GROUP_CAP);
        assert_eq!(titles(&hits, Group::Files).len(), GROUP_CAP);
    }

    #[test]
    fn the_push_command_leads_for_push() {
        let hits = search("push", &sources(&[]), &Recent::default());
        assert_eq!(titles(&hits, Group::Commands)[0], "Push current branch");
    }

    #[test]
    fn the_idle_lane_title_follows_the_state() {
        let mut s = sources(&[]);
        let before = search("idle lanes", &s, &Recent::default());
        assert_eq!(
            titles(&before, Group::Commands),
            vec!["Collapse idle lanes in graph"]
        );
        s.state.idle_lanes_collapsed = true;
        let after = search("idle lanes", &s, &Recent::default());
        assert_eq!(
            titles(&after, Group::Commands),
            vec!["Expand idle lanes in graph"]
        );
    }

    #[test]
    fn an_empty_query_lists_recent_commands_then_a_to_z_and_the_current_branch_first() {
        let mut recent = Recent::default();
        recent.touch_command(Command::Clone);
        recent.touch_command(Command::Push);
        recent.touch_branch("feat/palette");
        let hits = search("  ", &sources(&[commit(0xa1, "x")]), &recent);
        let commands = titles(&hits, Group::Commands);
        assert_eq!(commands.len(), EMPTY_COMMANDS);
        assert_eq!(&commands[..2], ["Push current branch", "Clone repository…"]);
        // The rest is alphabetical.
        let rest: Vec<String> = commands[2..].iter().map(|t| t.to_lowercase()).collect();
        let mut sorted = rest.clone();
        sorted.sort();
        assert_eq!(rest, sorted);
        // Branches: the current one, the recent one, then the other locals.
        assert_eq!(titles(&hits, Group::Branches), vec!["main", "feat/palette"]);
        assert!(titles(&hits, Group::Commits).is_empty());
        assert!(titles(&hits, Group::Files).is_empty());
    }

    #[test]
    fn ls_files_output_splits_on_nul_and_keeps_odd_names() {
        let output = b"src/main.rs\0docs/a b.md\0caf\xc3\xa9\nline.txt\0";
        assert_eq!(
            parse_ls_files(output),
            vec!["src/main.rs", "docs/a b.md", "café\nline.txt"]
        );
        assert!(parse_ls_files(b"").is_empty());
    }

    #[test]
    fn touching_a_recent_item_moves_it_to_the_front_once() {
        let mut recent = Recent::default();
        recent.touch_command(Command::Pull);
        recent.touch_command(Command::Push);
        recent.touch_command(Command::Pull);
        assert_eq!(recent.commands, vec![Command::Pull, Command::Push]);
    }

    #[test]
    fn every_settings_page_has_a_command() {
        let titles: Vec<String> = Command::all()
            .into_iter()
            .map(|c| c.title(ViewState::default()))
            .collect();
        for expected in [
            "Open Settings",
            "Open Settings → Git",
            "Open Settings → SSH keys",
            "Open Settings → Keybindings",
        ] {
            assert!(titles.iter().any(|t| t == expected), "{expected}");
        }
    }
}
