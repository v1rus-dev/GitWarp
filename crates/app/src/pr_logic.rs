//! Pull / merge requests without the UI: which remotes can take one, what the
//! "Create pull request" dialog prefills and decides, how API errors map onto
//! dialog and panel states, and the panel's filters, cache and checkout naming.
//!
//! Everything here is plain data and pure functions (the list cache is the one
//! piece of process state), so it is unit tested without a window.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use forge::api::{ApiError, Client, PrFilter, PrStateFilter, PullRequest};
use forge::{Instance, RemoteRef, RepoCoords, ServiceKind};
use git_core::network::NetworkOutcome;
use git_core::Error;
use gpui::SharedString;

use crate::accounts::{instance_of, kind_of};
use crate::network_ops;
use crate::remote_accounts::{account_refs, resolve_url, RemoteTargets, UrlAccount};
use crate::settings::AccountRecord;

/* -------------------------------- targets -------------------------------- */

/// A remote that can take a pull request: its fetch URL resolves to an
/// account on a service whose request API is implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrTarget {
    pub remote: String,
    pub account_id: String,
    pub login: String,
    pub instance: Instance,
    pub coords: RepoCoords,
}

impl PrTarget {
    pub fn kind(&self) -> ServiceKind {
        self.instance.kind
    }

    pub fn host(&self) -> &str {
        self.instance.host()
    }

    /// The service as the menus name it (see [`crate::web_links::service_name`]).
    pub fn service_name(&self) -> &'static str {
        crate::web_links::service_name(&self.instance)
    }

    /// The cache key of a list: instance, `owner/repo` and the account, since
    /// "Mine" and "Review requested" depend on whose login asks.
    pub fn cache_key(&self) -> String {
        format!(
            "{}|{}/{}|{}",
            self.instance.base_url,
            self.coords.namespace.join("/"),
            self.coords.repo,
            self.account_id
        )
    }
}

/// Whether the request API of `kind` is implemented (Bitbucket and Azure
/// DevOps answer `ApiError::Unsupported`).
pub fn supports_pull_requests(kind: ServiceKind) -> bool {
    matches!(
        kind,
        ServiceKind::GitHub | ServiceKind::GitLab | ServiceKind::Gitea
    )
}

/// The remotes that can take a pull request, the `preferred` one (the current
/// branch's upstream) first, then `origin`, then the rest by name. A remote
/// without an account, with several unchosen accounts, or on a service
/// without the API is left out.
pub fn pr_targets(
    accounts: &[AccountRecord],
    remote_targets: &[RemoteTargets],
    remembered: &BTreeMap<String, String>,
    preferred: Option<&str>,
) -> Vec<PrTarget> {
    let refs = account_refs(accounts);
    let mut ordered: Vec<&RemoteTargets> = remote_targets.iter().collect();
    ordered.sort_by_key(|t| {
        let rank = if Some(t.name.as_str()) == preferred {
            0
        } else if t.name == "origin" {
            1
        } else {
            2
        };
        (rank, t.name.clone())
    });
    ordered
        .into_iter()
        .filter_map(|target| {
            let resolved = resolve_url(
                &refs,
                &target.fetch_url,
                remembered.get(&target.name).map(String::as_str),
            );
            let UrlAccount::Linked(id) = resolved.account else {
                return None;
            };
            let account = accounts.iter().find(|a| a.id == id)?;
            if !supports_pull_requests(kind_of(account)?) {
                return None;
            }
            let instance = instance_of(account).ok()?;
            let coords = instance.matches(&RemoteRef::parse(&target.fetch_url)?)?;
            Some(PrTarget {
                remote: target.name.clone(),
                account_id: account.id.clone(),
                login: account.login.clone(),
                instance,
                coords,
            })
        })
        .collect()
}

/// The target a pull request for a branch goes to by default: the remote the
/// branch tracks when it can take one, else the first target.
pub fn default_target<'a>(
    targets: &'a [PrTarget],
    upstream_remote: Option<&str>,
) -> Option<&'a PrTarget> {
    upstream_remote
        .and_then(|remote| targets.iter().find(|t| t.remote == remote))
        .or_else(|| targets.first())
}

/// "pull request" / "merge request" as the active language says it, for
/// running text; `capitalised` for the start of a sentence or a title.
pub fn noun(kind: ServiceKind, capitalised: bool) -> SharedString {
    match (kind, capitalised) {
        (ServiceKind::GitLab, false) => ui::t!("dialogs.pr.noun.merge_request"),
        (ServiceKind::GitLab, true) => ui::t!("dialogs.pr.noun_cap.merge_request"),
        (_, false) => ui::t!("dialogs.pr.noun.pull_request"),
        (_, true) => ui::t!("dialogs.pr.noun_cap.pull_request"),
    }
}

/// The plural of [`noun`], for "No pull requests".
pub fn nouns(kind: ServiceKind) -> SharedString {
    match kind {
        ServiceKind::GitLab => ui::t!("dialogs.pr.nouns.merge_request"),
        _ => ui::t!("dialogs.pr.nouns.pull_request"),
    }
}

/// The two-letter tab label: "PRs", "MRs" on GitLab.
pub fn noun_short(kind: ServiceKind) -> SharedString {
    match kind {
        ServiceKind::GitLab => ui::t!("dialogs.pr.short.merge_request"),
        _ => ui::t!("dialogs.pr.short.pull_request"),
    }
}

/// One HTTP client for every pull request call, so the conditional-request
/// cache behind `list_pull_requests` survives between refreshes.
pub fn shared_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

/* ------------------------------ create dialog ---------------------------- */

/// How many commit subjects the description lists; the rest is summarised.
const MAX_BULLETS: usize = 50;

/// The title and description a new request starts with: one commit gives its
/// subject as the title, none or several the branch name made readable; the
/// description lists the commit subjects as bullets, oldest first.
pub fn prefill(subjects: &[String], head: &str) -> (String, String) {
    let title = match subjects {
        [only] => only.trim().to_owned(),
        _ => humanise_branch(head),
    };
    let mut body: Vec<String> = subjects
        .iter()
        .take(MAX_BULLETS)
        .map(|subject| format!("- {}", subject.trim()))
        .collect();
    if subjects.len() > MAX_BULLETS {
        body.push(format!("- … +{}", subjects.len() - MAX_BULLETS));
    }
    (title, body.join("\n"))
}

/// `feat/idle-lanes` becomes `Feat: idle lanes`: the first `/` is a colon, the
/// other separators spaces, the first letter upper case.
pub fn humanise_branch(branch: &str) -> String {
    let mut text = String::with_capacity(branch.len() + 1);
    let mut colon_done = false;
    for ch in branch.chars() {
        match ch {
            '/' if !colon_done => {
                colon_done = true;
                text.push_str(": ");
            }
            '/' | '-' | '_' => text.push(' '),
            ch => text.push(ch),
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The base branch a request goes to by default: the remote's own default
/// (`refs/remotes/<remote>/HEAD`), else `main`, `master`, else the first other
/// branch. Never the head itself (a branch cannot be merged into itself): with
/// no other branch there is no base.
pub fn default_base(
    remote_default: Option<&str>,
    branches: &[String],
    head: &str,
) -> Option<String> {
    let usable = |name: &str| name != head && branches.iter().any(|b| b == name);
    remote_default
        .filter(|name| usable(name))
        .map(str::to_owned)
        .or_else(|| {
            ["main", "master"]
                .into_iter()
                .find(|n| usable(n))
                .map(str::to_owned)
        })
        .or_else(|| branches.iter().find(|b| b.as_str() != head).cloned())
}

/// The branches a request can go into: every branch of the remote except the
/// head itself (a branch cannot be merged into itself), sorted.
pub fn base_choices(branches: &[String], head: &str) -> Vec<String> {
    let mut choices: Vec<String> = branches.iter().filter(|b| *b != head).cloned().collect();
    choices.sort();
    choices.dedup();
    choices
}

/// The logins typed into the reviewers field: separated by commas or white
/// space, a leading `@` dropped, duplicates (ignoring case) removed.
pub fn parse_reviewers(text: &str) -> Vec<String> {
    let mut logins: Vec<String> = Vec::new();
    for word in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let login = word.trim().trim_start_matches('@');
        if !login.is_empty() && !logins.iter().any(|l| l.eq_ignore_ascii_case(login)) {
            logins.push(login.to_owned());
        }
    }
    logins
}

/// What the head branch needs before a request can be created for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushNeed {
    /// The remote has no branch of that name.
    NotOnRemote,
    /// The remote has it and lacks `n` local commits.
    Ahead(usize),
    UpToDate,
    /// The remote has commits the local branch lacks and nothing to push: the
    /// request is created from what the remote has.
    Behind(usize),
    /// Both sides have commits: the push would be refused.
    Diverged,
}

/// Decides from the remote-tracking branch of the head: whether it exists and
/// how the local branch compares to it.
pub fn push_need(on_remote: bool, ahead: usize, behind: usize) -> PushNeed {
    match (on_remote, ahead, behind) {
        (false, ..) => PushNeed::NotOnRemote,
        (true, 0, 0) => PushNeed::UpToDate,
        (true, 0, behind) => PushNeed::Behind(behind),
        (true, ahead, 0) => PushNeed::Ahead(ahead),
        (true, ..) => PushNeed::Diverged,
    }
}

impl PushNeed {
    /// Whether the dialog pushes the branch before creating the request.
    pub fn pushes(self) -> bool {
        matches!(self, PushNeed::NotOnRemote | PushNeed::Ahead(_))
    }

    /// Whether a request cannot be created yet: the push would be refused, so
    /// the branch has to be pulled or rebased first.
    pub fn blocks(self) -> bool {
        self == PushNeed::Diverged
    }
}

/// How a failed create call is shown in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateFailure {
    /// A request for this head and base exists: banner and "Open #N".
    Exists {
        number: u64,
        web_url: String,
    },
    /// 401: banner with Reconnect.
    TokenRejected,
    /// The service refused the request and said why.
    Rejected(String),
    RateLimited,
    /// No connection, or an untrusted certificate.
    Offline(String),
    Other(String),
}

pub fn create_failure(error: &ApiError) -> CreateFailure {
    match error {
        ApiError::AlreadyExists { number, web_url } => CreateFailure::Exists {
            number: *number,
            web_url: web_url.clone(),
        },
        ApiError::Unauthorized => CreateFailure::TokenRejected,
        ApiError::Rejected(why) => CreateFailure::Rejected(why.clone()),
        ApiError::RateLimited { .. } => CreateFailure::RateLimited,
        ApiError::Unreachable(_) | ApiError::TlsUntrusted(_) => {
            CreateFailure::Offline(error.to_string())
        }
        other => CreateFailure::Other(other.to_string()),
    }
}

/// Why the push in front of a create failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushFailure {
    /// What git said, as it said it.
    pub text: String,
    /// The account's token was refused: offer Reconnect.
    pub token_rejected: bool,
}

/// git's own words: the stderr of a failed command, else the error text.
fn git_text(error: &Error) -> String {
    match error {
        Error::Git { stderr, .. } => stderr.trim().to_owned(),
        other => other.to_string(),
    }
}

pub fn push_failure(error: &Error) -> PushFailure {
    PushFailure {
        text: git_text(error),
        token_rejected: network_ops::is_auth_error(error),
    }
}

/// The first push target that failed, when the push went to several URLs and
/// "succeeded" as a job: nothing may be created while a target refused it.
pub fn failed_target(outcome: &NetworkOutcome) -> Option<PushFailure> {
    outcome.targets.iter().find_map(|target| {
        let text = target.result.as_ref().err()?;
        Some(PushFailure {
            text: text.trim().to_owned(),
            token_rejected: network_ops::target_failure(text).1,
        })
    })
}

/* --------------------------------- panel --------------------------------- */

/// The panel's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrTab {
    Mine,
    ReviewRequested,
    All,
}

impl PrTab {
    pub const ALL: [PrTab; 3] = [PrTab::Mine, PrTab::ReviewRequested, PrTab::All];

    pub fn from_index(index: usize) -> PrTab {
        Self::ALL.get(index).copied().unwrap_or(PrTab::Mine)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// The API question a tab asks. Mine and All list every state (the design
/// shows merged and closed requests there); a request waiting for a review
/// is open by definition.
pub fn tab_query(tab: PrTab, login: &str) -> (PrFilter, PrStateFilter) {
    match tab {
        PrTab::Mine => (
            PrFilter::Mine {
                login: login.to_owned(),
            },
            PrStateFilter::All,
        ),
        PrTab::ReviewRequested => (
            PrFilter::ReviewRequested {
                login: login.to_owned(),
            },
            PrStateFilter::Open,
        ),
        PrTab::All => (PrFilter::All, PrStateFilter::All),
    }
}

/// How a failed list call is shown in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    /// 401: the token was revoked; Reconnect.
    TokenRevoked,
    RateLimited {
        reset_at: Option<u64>,
    },
    /// No connection, or an untrusted certificate.
    Offline(String),
    Other(String),
}

pub fn list_error(error: &ApiError) -> ListError {
    match error {
        ApiError::Unauthorized => ListError::TokenRevoked,
        ApiError::RateLimited { reset_at } => ListError::RateLimited {
            reset_at: *reset_at,
        },
        ApiError::Unreachable(_) | ApiError::TlsUntrusted(_) => {
            ListError::Offline(error.to_string())
        }
        other => ListError::Other(other.to_string()),
    }
}

/// How long a loaded list counts as current.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// Whether a list fetched at `fetched` is still current at `now`.
pub fn is_fresh(fetched: Instant, now: Instant) -> bool {
    now.saturating_duration_since(fetched) < STALE_AFTER
}

/// One loaded page of a tab.
#[derive(Debug, Clone)]
pub struct Listing {
    pub items: Vec<PullRequest>,
    pub has_next: bool,
    pub fetched: Instant,
}

/// The loaded lists per repository and tab, shared by every window so a
/// second window on the same repository does not ask again within a minute.
#[derive(Debug, Default)]
pub struct PrCache {
    lists: HashMap<(String, PrTab), Listing>,
}

impl PrCache {
    pub fn get(&self, repo_key: &str, tab: PrTab) -> Option<&Listing> {
        self.lists.get(&(repo_key.to_owned(), tab))
    }

    pub fn put(&mut self, repo_key: &str, tab: PrTab, listing: Listing) {
        self.lists.insert((repo_key.to_owned(), tab), listing);
    }

    /// Forgets a repository's lists, e.g. after a request was created.
    pub fn invalidate(&mut self, repo_key: &str) {
        self.lists.retain(|(key, _), _| key != repo_key);
    }
}

/// The process-wide cache.
pub fn cache() -> &'static Mutex<PrCache> {
    static CACHE: OnceLock<Mutex<PrCache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AccountMethod;
    use forge::api::PrState;

    fn account(id: &str, kind: &str, base_url: &str, login: &str) -> AccountRecord {
        AccountRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            base_url: base_url.to_owned(),
            login: login.to_owned(),
            label: login.to_owned(),
            method: AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    fn remote(name: &str, url: &str) -> RemoteTargets {
        RemoteTargets {
            name: name.to_owned(),
            fetch_url: url.to_owned(),
            push_urls: vec![url.to_owned()],
        }
    }

    pub(super) fn target_for(account: &str) -> PrTarget {
        PrTarget {
            remote: "origin".to_owned(),
            account_id: account.to_owned(),
            login: "mira-k".to_owned(),
            instance: Instance::new(ServiceKind::GitHub, "https://github.com").unwrap(),
            coords: RepoCoords {
                namespace: vec!["o".to_owned()],
                repo: "r".to_owned(),
            },
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn targets_are_remotes_with_an_account_on_a_supported_service() {
        let accounts = [
            account("a1", "github", "https://github.com", "mira-k"),
            account("a2", "gitlab", "https://gitlab.modsen.app", "yegor"),
            account("a3", "bitbucket", "https://bitbucket.org", "mira"),
        ];
        let remotes = [
            remote("upstream", "https://github.com/nexu-io/gitwarp.git"),
            remote("work", "git@gitlab.modsen.app:team/sub/gitwarp.git"),
            remote("bb", "https://bitbucket.org/mira/gitwarp.git"),
            remote("fork", "https://codeberg.org/mira/gitwarp.git"),
            remote("local", "/srv/git/gitwarp.git"),
        ];
        let targets = pr_targets(&accounts, &remotes, &BTreeMap::new(), None);
        let names: Vec<_> = targets.iter().map(|t| t.remote.as_str()).collect();
        // Bitbucket has no request API, codeberg.org no account, a path no host.
        assert_eq!(names, ["upstream", "work"]);
        assert_eq!(targets[0].login, "mira-k");
        assert_eq!(targets[0].coords.repo, "gitwarp");
        assert_eq!(targets[0].coords.namespace, ["nexu-io"]);
        assert_eq!(targets[1].coords.namespace, ["team", "sub"]);
        assert_eq!(targets[1].kind().pr_noun(), "merge request");
        assert_eq!(targets[0].kind().pr_noun(), "pull request");
        assert_eq!(targets[0].service_name(), "GitHub");
    }

    #[test]
    fn targets_put_the_preferred_remote_then_origin_first() {
        let accounts = [account("a1", "github", "https://github.com", "mira-k")];
        let remotes = [
            remote("zeta", "https://github.com/a/z.git"),
            remote("origin", "https://github.com/a/o.git"),
            remote("up", "https://github.com/a/u.git"),
        ];
        let names = |preferred| {
            pr_targets(&accounts, &remotes, &BTreeMap::new(), preferred)
                .into_iter()
                .map(|t| t.remote)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(None), ["origin", "up", "zeta"]);
        assert_eq!(names(Some("zeta")), ["zeta", "origin", "up"]);
    }

    #[test]
    fn several_accounts_need_a_remembered_choice() {
        let accounts = [
            account("a1", "github", "https://github.com", "mira-k"),
            account("a2", "github", "https://github.com", "mira-work"),
        ];
        let remotes = [remote("origin", "https://github.com/a/o.git")];
        assert!(pr_targets(&accounts, &remotes, &BTreeMap::new(), None).is_empty());
        let remembered = BTreeMap::from([("origin".to_owned(), "a2".to_owned())]);
        let targets = pr_targets(&accounts, &remotes, &remembered, None);
        assert_eq!(targets[0].login, "mira-work");
    }

    #[test]
    fn the_default_target_follows_the_upstream_remote() {
        let accounts = [account("a1", "github", "https://github.com", "mira-k")];
        let remotes = [
            remote("origin", "https://github.com/a/o.git"),
            remote("up", "https://github.com/a/u.git"),
        ];
        let targets = pr_targets(&accounts, &remotes, &BTreeMap::new(), None);
        assert_eq!(default_target(&targets, Some("up")).unwrap().remote, "up");
        assert_eq!(
            default_target(&targets, Some("gone")).unwrap().remote,
            "origin"
        );
        assert_eq!(default_target(&targets, None).unwrap().remote, "origin");
        assert!(default_target(&[], None).is_none());
    }

    #[test]
    fn prefill_uses_the_subject_of_a_single_commit() {
        let (title, body) = prefill(&strings(&["fix(tree): keep selection"]), "fix/tree-filter");
        assert_eq!(title, "fix(tree): keep selection");
        assert_eq!(body, "- fix(tree): keep selection");
    }

    #[test]
    fn prefill_names_several_commits_after_the_branch() {
        let subjects = strings(&["feat(graph): collapse idle lanes", "feat(graph): clip"]);
        let (title, body) = prefill(&subjects, "feat/idle-lanes");
        assert_eq!(title, "Feat: idle lanes");
        assert_eq!(
            body,
            "- feat(graph): collapse idle lanes\n- feat(graph): clip"
        );
    }

    #[test]
    fn prefill_without_commits_leaves_the_description_empty() {
        let (title, body) = prefill(&[], "fix/git-detect");
        assert_eq!(title, "Fix: git detect");
        assert_eq!(body, "");
    }

    #[test]
    fn prefill_summarises_a_very_long_range() {
        let subjects: Vec<String> = (0..60).map(|n| format!("commit {n}")).collect();
        let (_, body) = prefill(&subjects, "big");
        assert_eq!(body.lines().count(), MAX_BULLETS + 1);
        assert!(body.ends_with("- … +10"));
    }

    #[test]
    fn branch_names_are_made_readable() {
        assert_eq!(humanise_branch("feat/idle-lanes"), "Feat: idle lanes");
        assert_eq!(humanise_branch("main"), "Main");
        assert_eq!(humanise_branch("fix/a/b_c"), "Fix: a b c");
        assert_eq!(humanise_branch("release/0.3-hotfix"), "Release: 0.3 hotfix");
        assert_eq!(humanise_branch(""), "");
    }

    #[test]
    fn the_base_defaults_to_the_remote_head_then_main_then_master() {
        let branches = strings(&["develop", "master", "feat/x"]);
        assert_eq!(
            default_base(Some("develop"), &branches, "feat/x").as_deref(),
            Some("develop")
        );
        // A pointer at a branch that is not there is ignored.
        assert_eq!(
            default_base(Some("trunk"), &branches, "feat/x").as_deref(),
            Some("master")
        );
        let with_main = strings(&["develop", "main", "master"]);
        assert_eq!(
            default_base(None, &with_main, "feat/x").as_deref(),
            Some("main")
        );
        let odd = strings(&["feat/x", "trunk"]);
        assert_eq!(default_base(None, &odd, "feat/x").as_deref(), Some("trunk"));
        assert_eq!(default_base(None, &[], "feat/x"), None);
    }

    #[test]
    fn the_base_is_never_the_head() {
        // On main with a develop branch: neither the remote default nor
        // main / master may be picked when they are the head.
        let branches = strings(&["main", "develop"]);
        assert_eq!(
            default_base(Some("main"), &branches, "main").as_deref(),
            Some("develop")
        );
        assert_eq!(
            default_base(None, &strings(&["master", "x"]), "master").as_deref(),
            Some("x")
        );
        // Nothing else to merge into: no base at all.
        assert_eq!(
            default_base(Some("main"), &strings(&["main"]), "main"),
            None
        );
        assert_eq!(default_base(None, &strings(&["main"]), "main"), None);
    }

    #[test]
    fn a_list_is_cached_per_account() {
        let mut a = target_for("acc1");
        let b = target_for("acc2");
        assert_ne!(a.cache_key(), b.cache_key());
        a.account_id = "acc2".to_owned();
        assert_eq!(a.cache_key(), b.cache_key());
    }

    #[test]
    fn the_head_is_not_offered_as_a_base() {
        let branches = strings(&["main", "feat/x", "release/0.3", "main"]);
        assert_eq!(base_choices(&branches, "feat/x"), ["main", "release/0.3"]);
    }

    #[test]
    fn reviewers_split_on_commas_and_spaces() {
        assert_eq!(
            parse_reviewers("lin-zhao, @priya-n  tomas-r,,"),
            ["lin-zhao", "priya-n", "tomas-r"]
        );
        assert_eq!(parse_reviewers("Lin lin LIN"), ["Lin"]);
        assert!(parse_reviewers("  , ").is_empty());
        assert!(parse_reviewers("@").is_empty());
    }

    #[test]
    fn push_is_needed_when_the_remote_lacks_commits() {
        assert_eq!(push_need(false, 0, 0), PushNeed::NotOnRemote);
        assert_eq!(push_need(false, 3, 1), PushNeed::NotOnRemote);
        assert_eq!(push_need(true, 2, 0), PushNeed::Ahead(2));
        assert_eq!(push_need(true, 0, 0), PushNeed::UpToDate);
        assert_eq!(push_need(true, 0, 4), PushNeed::Behind(4));
        assert_eq!(push_need(true, 1, 1), PushNeed::Diverged);
        assert!(PushNeed::NotOnRemote.pushes());
        assert!(PushNeed::Ahead(1).pushes());
        assert!(!PushNeed::Diverged.pushes());
        assert!(PushNeed::Diverged.blocks());
        assert!(!PushNeed::Ahead(1).blocks());
        assert!(!PushNeed::UpToDate.pushes());
        assert!(!PushNeed::Behind(1).pushes());
    }

    #[test]
    fn create_errors_map_onto_dialog_states() {
        let exists = ApiError::AlreadyExists {
            number: 142,
            web_url: "https://github.com/a/b/pull/142".to_owned(),
        };
        assert_eq!(
            create_failure(&exists),
            CreateFailure::Exists {
                number: 142,
                web_url: "https://github.com/a/b/pull/142".to_owned()
            }
        );
        assert_eq!(
            create_failure(&ApiError::Unauthorized),
            CreateFailure::TokenRejected
        );
        assert_eq!(
            create_failure(&ApiError::Rejected("No commits between".to_owned())),
            CreateFailure::Rejected("No commits between".to_owned())
        );
        assert_eq!(
            create_failure(&ApiError::RateLimited { reset_at: None }),
            CreateFailure::RateLimited
        );
        assert!(matches!(
            create_failure(&ApiError::Unreachable("dns".to_owned())),
            CreateFailure::Offline(_)
        ));
        assert!(matches!(
            create_failure(&ApiError::Forbidden),
            CreateFailure::Other(_)
        ));
    }

    #[test]
    fn a_failed_push_shows_git_s_own_words() {
        let error = Error::Git {
            command: "push",
            code: Some(1),
            stderr: "! [rejected] feat -> feat (non-fast-forward)\n".to_owned(),
        };
        let failure = push_failure(&error);
        assert_eq!(failure.text, "! [rejected] feat -> feat (non-fast-forward)");
        assert!(!failure.token_rejected);
        let auth = Error::Git {
            command: "push",
            code: Some(128),
            stderr: "fatal: Authentication failed for 'https://github.com/a/b.git/'".to_owned(),
        };
        assert!(push_failure(&auth).token_rejected);
    }

    #[test]
    fn a_refused_push_target_stops_the_create() {
        use git_core::network::{PushStatus, PushTargetOutcome};
        let outcome = NetworkOutcome {
            targets: vec![
                PushTargetOutcome {
                    url: "https://a".to_owned(),
                    result: Ok(PushStatus::NewBranch),
                    partial: false,
                },
                PushTargetOutcome {
                    url: "https://b".to_owned(),
                    result: Err("remote: Permission denied".to_owned()),
                    partial: false,
                },
            ],
            ..NetworkOutcome::default()
        };
        assert_eq!(
            failed_target(&outcome).unwrap().text,
            "remote: Permission denied"
        );
        assert!(failed_target(&NetworkOutcome::default()).is_none());
    }

    #[test]
    fn list_errors_map_onto_panel_states() {
        assert_eq!(list_error(&ApiError::Unauthorized), ListError::TokenRevoked);
        assert_eq!(
            list_error(&ApiError::RateLimited { reset_at: Some(9) }),
            ListError::RateLimited { reset_at: Some(9) }
        );
        assert!(matches!(
            list_error(&ApiError::TlsUntrusted("self-signed".to_owned())),
            ListError::Offline(_)
        ));
        assert!(matches!(
            list_error(&ApiError::Status(503)),
            ListError::Other(_)
        ));
    }

    #[test]
    fn tabs_map_onto_api_filters() {
        assert_eq!(
            tab_query(PrTab::Mine, "mira-k"),
            (
                PrFilter::Mine {
                    login: "mira-k".to_owned()
                },
                PrStateFilter::All
            )
        );
        assert_eq!(
            tab_query(PrTab::ReviewRequested, "mira-k"),
            (
                PrFilter::ReviewRequested {
                    login: "mira-k".to_owned()
                },
                PrStateFilter::Open
            )
        );
        assert_eq!(
            tab_query(PrTab::All, "mira-k"),
            (PrFilter::All, PrStateFilter::All)
        );
        for tab in PrTab::ALL {
            assert_eq!(PrTab::from_index(tab.index()), tab);
        }
        assert_eq!(PrTab::from_index(99), PrTab::Mine);
    }

    #[test]
    fn a_list_is_fresh_for_sixty_seconds() {
        let start = Instant::now();
        assert!(is_fresh(start, start));
        assert!(is_fresh(start, start + Duration::from_secs(59)));
        assert!(!is_fresh(start, start + Duration::from_secs(60)));
        // A clock that reads earlier than the fetch is not stale.
        assert!(is_fresh(start + Duration::from_secs(5), start));
    }

    fn listing(numbers: &[u64]) -> Listing {
        Listing {
            items: numbers
                .iter()
                .map(|&number| PullRequest {
                    number,
                    title: format!("PR {number}"),
                    state: PrState::Open,
                    review: forge::api::ReviewState::None,
                    author: "mira-k".to_owned(),
                    head_branch: "feat".to_owned(),
                    head_repo_full_name: None,
                    base_branch: "main".to_owned(),
                    updated_at: 0,
                    web_url: String::new(),
                })
                .collect(),
            has_next: false,
            fetched: Instant::now(),
        }
    }

    #[test]
    fn the_cache_keeps_one_list_per_repository_and_tab() {
        let mut cache = PrCache::default();
        cache.put("gh|a/b", PrTab::Mine, listing(&[1]));
        cache.put("gh|a/b", PrTab::All, listing(&[1, 2]));
        cache.put("gh|a/c", PrTab::Mine, listing(&[3]));
        assert_eq!(cache.get("gh|a/b", PrTab::All).unwrap().items.len(), 2);
        assert!(cache.get("gh|a/b", PrTab::ReviewRequested).is_none());
        cache.invalidate("gh|a/b");
        assert!(cache.get("gh|a/b", PrTab::Mine).is_none());
        assert!(cache.get("gh|a/b", PrTab::All).is_none());
        assert!(cache.get("gh|a/c", PrTab::Mine).is_some());
    }
}
