//! Pull request badges and CI status of one open repository.
//!
//! The core is plain data and pure functions, so it is unit tested without a
//! window: which remote speaks for the repository, which branches and commits
//! are worth a request, when a cached answer is stale, and what an error does
//! to the badges. [`Request`] runs the blocking `forge::api` calls (always on
//! the background executor); [`PrPopover`] at the end is the thin GPUI view of
//! a badge's popover.
//!
//! Request budget per repository open (N = local branches tracking the primary
//! remote, T = commits asked for, at most [`MAX_CI_TIPS`] + 2):
//! - pull requests: 1 call on GitHub and Gitea, min(N, [`MAX_PR_BRANCHES`])
//!   calls on GitLab (one `source_branch` query each);
//! - checks: 2 calls per commit on GitHub and GitLab, 1 on Gitea, only for
//!   commits whose answer is missing or not final (see [`CiCache::needs_fetch`]);
//! - review state: 1 call each time a popover opens.
//!
//! Requests share one `forge::api::Client`, whose conditional-request cache
//! answers unchanged resources with a free 304.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use forge::api::{
    commit_checks, pull_request_head_ref, pull_request_review, pull_requests_for_branches,
    ApiError, Auth, CiState, Client, CommitChecks, PrState, PullRequest, ReviewState, TokenKind,
};
use forge::secrets::SecretStore;
use forge::{AccountRef, Instance, RepoCoords, ServiceKind};
use git_core::{Head, Oid, Ref, RefKind};
use gpui::prelude::*;
use gpui::{div, px, Context, FontWeight, Render, SharedString, WeakEntity, Window};
use ui::metrics::{FONT_UI, FS_BASE, FS_SM, R_MD};
use ui::overlay::{self, LayerId};
use ui::{
    ActiveTheme, Button, ButtonSize, CiGlyph, CiGlyphState, IconName, PrBadge, PrBadgeState,
    ReviewMark,
};

use crate::format::relative_time;
use crate::remote_accounts::{self, RemoteTargets, UrlAccount};
use crate::repo_view::RepositoryView;
use crate::web_links::{self, HostLink};

/* ------------------------------- constants ------------------------------- */

/// The pull request map is refreshed on window focus only when older than this.
pub const PR_STALE: Duration = Duration::from_secs(60);
/// A pending / running answer is asked again after this long (while visible).
pub const CI_PENDING_REFETCH: Duration = Duration::from_secs(30);
/// "No checks configured" is rechecked after this long: a workflow may appear.
pub const CI_EMPTY_RECHECK: Duration = Duration::from_secs(300);
/// GitLab asks per branch; branches past this many get no badge.
pub const MAX_PR_BRANCHES: usize = 30;
/// Checks are requested for at most this many pull request branch tips.
pub const MAX_CI_TIPS: usize = 10;
/// A rate limit without a reset time pauses requests for this long.
const RATE_LIMIT_FALLBACK: u64 = 60;
/// The popover shows the head branch cut to this many characters.
const BRANCH_MAX: usize = 34;

/* ------------------------------ the target ------------------------------- */

/// The repository the badges are about: the primary remote's host, project and
/// account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The remote that speaks for the repository.
    pub remote: String,
    pub link: HostLink,
    /// The account whose token is sent; `None` reads public data anonymously.
    pub account_id: Option<String>,
}

/// Whether `forge::api` has pull request / CI calls for the service.
fn api_supported(kind: ServiceKind) -> bool {
    matches!(
        kind,
        ServiceKind::GitHub | ServiceKind::GitLab | ServiceKind::Gitea
    )
}

/// The remote that speaks for the repository: the current branch's upstream
/// remote, else `origin`, else the first by name, among the remotes whose
/// fetch URL is on a host with an API (the order of [`web_links::host_links`]).
/// `remembered` is the repository's remembered account per remote; a remote
/// with several matching accounts and no remembered one is read anonymously.
pub fn primary_target(
    accounts: &[AccountRef],
    targets: &[RemoteTargets],
    head: Option<&Head>,
    refs: &[Ref],
    remembered: &std::collections::BTreeMap<String, String>,
) -> Option<Target> {
    let preferred = web_links::upstream_remote(head, refs, targets);
    let links = web_links::host_links(accounts, targets, preferred.as_deref());
    let link = links
        .into_iter()
        .find(|l| !l.fetch_remotes.is_empty() && api_supported(l.instance.kind))?;
    let remote = link.fetch_remotes[0].clone();
    let fetch_url = &targets.iter().find(|t| t.name == remote)?.fetch_url;
    let account_id = match remote_accounts::resolve_url(
        accounts,
        fetch_url,
        remembered.get(&remote).map(String::as_str),
    )
    .account
    {
        UrlAccount::Linked(id) => Some(id),
        UrlAccount::Choose(_) | UrlAccount::NoAccount(_) | UrlAccount::Unknown => None,
    };
    Some(Target {
        remote,
        link,
        account_id,
    })
}

/// A local branch that tracks a branch of the primary remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedBranch {
    pub local: String,
    /// The branch on the service: what a pull request's head is called.
    pub remote_branch: String,
    /// The local branch's tip.
    pub tip: Oid,
    /// The remote-tracking ref's tip: the commit the service knows as the
    /// branch head, hence the one worth asking checks for.
    pub remote_tip: Option<Oid>,
}

/// The tips of the remote-tracking branches of `remote` (symbolic
/// `<remote>/HEAD` left out): every commit reachable from them is on the host.
pub fn primary_remote_tips(refs: &[Ref], remote: &str) -> Vec<Oid> {
    let prefix = format!("{remote}/");
    let mut tips: Vec<Oid> = Vec::new();
    for r in refs {
        if r.kind == RefKind::RemoteBranch
            && r.name.starts_with(&prefix)
            && !r.name.ends_with("/HEAD")
            && !tips.contains(&r.target)
        {
            tips.push(r.target);
        }
    }
    tips
}

/// Every commit reachable from `tips` through `parents` (which answers `None`
/// for a commit that is not loaded, ending that path). The tips themselves
/// are always in the set.
pub fn reachable(tips: &[Oid], parents: impl Fn(Oid) -> Option<Vec<Oid>>) -> HashSet<Oid> {
    let mut seen: HashSet<Oid> = tips.iter().copied().collect();
    let mut stack: Vec<Oid> = tips.to_vec();
    while let Some(oid) = stack.pop() {
        for parent in parents(oid).unwrap_or_default() {
            if seen.insert(parent) {
                stack.push(parent);
            }
        }
    }
    seen
}

/// The local branches whose upstream lives on a remote that leads to the
/// target's repository.
pub fn tracked_branches(
    refs: &[Ref],
    targets: &[RemoteTargets],
    target: &Target,
) -> Vec<TrackedBranch> {
    refs.iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .filter_map(|r| {
            let upstream = r.upstream.as_deref()?;
            let (remote, branch) = web_links::split_remote_ref(upstream, targets)?;
            target.link.serves(remote).then(|| TrackedBranch {
                local: r.name.clone(),
                remote_branch: branch.to_owned(),
                tip: r.target,
                remote_tip: refs
                    .iter()
                    .find(|u| u.kind == RefKind::RemoteBranch && u.name == upstream)
                    .map(|u| u.target),
            })
        })
        .collect()
}

/* ---------------------------- refresh policy ----------------------------- */

/// What asks for a fresh pull request map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrTrigger {
    /// The repository was opened.
    Open,
    /// The window got focus again.
    Focus,
    /// A fetch or push finished.
    AfterNetwork,
    /// The user pressed the Refresh action. The only trigger that is explicit.
    Manual,
}

impl PrTrigger {
    /// Only the user's own refresh forgets what went wrong before.
    pub fn resets_health(self) -> bool {
        self == PrTrigger::Manual
    }
}

/// What the repository's own reloads (a ref change, a worktree change) mean
/// for the pull request map: not the user asking, so no health reset, and the
/// map is only refreshed when the last attempt is stale.
pub const INTERNAL_RELOAD: PrTrigger = PrTrigger::Focus;

/// Whether `trigger` refreshes the map now. Opening and the user's refresh go
/// at once; a window focus and a finished fetch or push wait until the last
/// *attempt* (successful or not) is [`PR_STALE`] old, so a failing service is
/// not asked again on every trigger.
pub fn pr_refresh_due(trigger: PrTrigger, attempted_at: Option<Instant>, now: Instant) -> bool {
    match trigger {
        PrTrigger::Focus | PrTrigger::AfterNetwork => {
            attempted_at.is_none_or(|at| now.saturating_duration_since(at) >= PR_STALE)
        }
        PrTrigger::Open | PrTrigger::Manual => true,
    }
}

/// The commits worth a checks request, most wanted first, without repeats: the
/// selected one, HEAD, then the tips of branches with a pull request.
pub fn ci_wanted(selected: Option<Oid>, head: Option<Oid>, pr_tips: &[Oid]) -> Vec<Oid> {
    let mut wanted: Vec<Oid> = Vec::new();
    let candidates = selected
        .into_iter()
        .chain(head)
        .chain(pr_tips.iter().copied().take(MAX_CI_TIPS));
    for oid in candidates {
        if !wanted.contains(&oid) {
            wanted.push(oid);
        }
    }
    wanted
}

/* -------------------------------- health --------------------------------- */

/// What a failed request means for the badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorEffect {
    /// 401: stop, hide the badges and say so once.
    TokenRejected,
    /// Stop until the limit resets.
    RateLimited { reset_at: Option<u64> },
    /// The service cannot be reached: say nothing, try again on the next trigger.
    Offline,
    /// The service has no such calls: never ask again.
    Unsupported,
    /// An anonymous read was refused (404 / 403): no access to this
    /// repository without an account. Stop until the account or remote changes.
    NoAccess,
    /// Anything else: this answer is missing, the rest still works.
    Other,
}

pub fn classify(error: &ApiError) -> ErrorEffect {
    match error {
        ApiError::Unauthorized => ErrorEffect::TokenRejected,
        ApiError::RateLimited { reset_at } => ErrorEffect::RateLimited {
            reset_at: *reset_at,
        },
        ApiError::Unreachable(_) | ApiError::TlsUntrusted(_) => ErrorEffect::Offline,
        ApiError::Unsupported => ErrorEffect::Unsupported,
        ApiError::Forbidden
        | ApiError::NotFound
        | ApiError::Status(_)
        | ApiError::Decode(_)
        | ApiError::AlreadyExists { .. }
        | ApiError::Rejected(_) => ErrorEffect::Other,
    }
}

/// Whether requests may go out and badges may show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    pub token_rejected: bool,
    pub offline: bool,
    /// Unix time the rate limit lifts.
    pub rate_limited_until: Option<u64>,
    pub unsupported: bool,
    /// An anonymous request was refused: the repository needs an account.
    pub no_access: bool,
}

impl Health {
    /// Records a failure and says what it was. `anonymous` is whether the
    /// request carried no token.
    pub fn apply(&mut self, error: &ApiError, now_unix: u64, anonymous: bool) -> ErrorEffect {
        let mut effect = classify(error);
        if anonymous && matches!(error, ApiError::NotFound | ApiError::Forbidden) {
            effect = ErrorEffect::NoAccess;
        }
        match effect {
            ErrorEffect::TokenRejected => self.token_rejected = true,
            ErrorEffect::RateLimited { reset_at } => {
                self.rate_limited_until = Some(
                    reset_at
                        .unwrap_or(now_unix + RATE_LIMIT_FALLBACK)
                        .max(now_unix),
                );
            }
            ErrorEffect::Offline => self.offline = true,
            ErrorEffect::Unsupported => self.unsupported = true,
            ErrorEffect::NoAccess => self.no_access = true,
            ErrorEffect::Other => {}
        }
        effect
    }

    /// A request got through. A rate-limit pause is not lifted by one:
    /// requests already in flight may succeed while the limit stands, and the
    /// pause ends by time only.
    pub fn succeeded(&mut self) {
        self.offline = false;
    }

    /// The user asked again: a rejected token may have been replaced, or access
    /// granted, since. The rate-limit pause still runs to its reset time.
    pub fn manual_retry(&mut self) {
        self.token_rejected = false;
        self.offline = false;
        self.no_access = false;
    }

    pub fn can_request(&self, now_unix: u64) -> bool {
        !self.unsupported
            && !self.token_rejected
            && !self.no_access
            && self
                .rate_limited_until
                .is_none_or(|until| now_unix >= until)
    }

    /// A rejected token or an unsupported service hides what was fetched.
    pub fn badges_visible(&self) -> bool {
        !self.token_rejected && !self.unsupported && !self.no_access
    }
}

/// The accounts whose rejected token was already announced this session.
#[derive(Default)]
pub struct RejectionLog {
    seen: Mutex<HashSet<String>>,
}

impl RejectionLog {
    /// True the first time `account` is reported.
    pub fn first(&self, account: &str) -> bool {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_owned())
    }
}

/// The process-wide log: one toast per account per session, however many
/// repositories ask.
pub fn session_rejections() -> &'static RejectionLog {
    static LOG: OnceLock<RejectionLog> = OnceLock::new();
    LOG.get_or_init(RejectionLog::default)
}

/* --------------------------------- CI cache ------------------------------- */

#[derive(Debug, Clone)]
pub enum CiEntry {
    Loaded {
        checks: CommitChecks,
        at: Instant,
    },
    Failed {
        message: String,
        at: Instant,
    },
    /// The service does not know the commit (404 / 422): it is not pushed.
    /// Shown as nothing at all.
    Absent {
        at: Instant,
    },
    /// The service could not be reached and nothing was known before: shown
    /// as nothing, asked again later.
    Offline {
        at: Instant,
    },
}

impl CiEntry {
    fn at(&self) -> Instant {
        match self {
            CiEntry::Loaded { at, .. }
            | CiEntry::Failed { at, .. }
            | CiEntry::Absent { at }
            | CiEntry::Offline { at } => *at,
        }
    }
}

/// Passed, failed and cancelled never change for a commit.
pub fn is_final(state: CiState) -> bool {
    matches!(
        state,
        CiState::Passed | CiState::Failed | CiState::Cancelled
    )
}

/// Checks by commit, with the refetch policy.
#[derive(Debug, Default)]
pub struct CiCache {
    entries: HashMap<String, CiEntry>,
    inflight: HashSet<String>,
}

impl CiCache {
    pub fn get(&self, sha: &str) -> Option<&CiEntry> {
        self.entries.get(sha)
    }

    pub fn checks(&self, sha: &str) -> Option<&CommitChecks> {
        match self.entries.get(sha)? {
            CiEntry::Loaded { checks, .. } => Some(checks),
            CiEntry::Failed { .. } | CiEntry::Absent { .. } | CiEntry::Offline { .. } => None,
        }
    }

    /// Whether a request for `sha` should go out now: nothing known yet; a
    /// pending / running answer older than [`CI_PENDING_REFETCH`]; "no checks"
    /// older than [`CI_EMPTY_RECHECK`]; a failed request older than
    /// [`CI_PENDING_REFETCH`]. A final answer never. One request per commit
    /// at a time.
    pub fn needs_fetch(&self, sha: &str, now: Instant) -> bool {
        if self.inflight.contains(sha) {
            return false;
        }
        let Some(entry) = self.entries.get(sha) else {
            return true;
        };
        let age = now.saturating_duration_since(entry.at());
        match entry {
            CiEntry::Loaded { checks, .. } => match checks.overall {
                state if is_final(state) => false,
                CiState::None => age >= CI_EMPTY_RECHECK,
                _ => age >= CI_PENDING_REFETCH,
            },
            CiEntry::Failed { .. } | CiEntry::Offline { .. } => age >= CI_PENDING_REFETCH,
            CiEntry::Absent { .. } => age >= CI_EMPTY_RECHECK,
        }
    }

    pub fn mark_inflight(&mut self, sha: &str) {
        self.inflight.insert(sha.to_owned());
    }

    /// Stores one answer. A failed refetch keeps the last good answer.
    pub fn store(&mut self, sha: &str, result: Result<CommitChecks, ApiError>, now: Instant) {
        self.inflight.remove(sha);
        match result {
            Ok(checks) => {
                self.entries
                    .insert(sha.to_owned(), CiEntry::Loaded { checks, at: now });
            }
            // Not pushed: the service has no checks to show, and a "No checks
            // configured" view would be wrong for it.
            Err(ApiError::NotFound | ApiError::Status(422)) => {
                self.entries
                    .insert(sha.to_owned(), CiEntry::Absent { at: now });
            }
            Err(error) => {
                // A failed refetch keeps the last good answer.
                if matches!(self.entries.get(sha), Some(CiEntry::Loaded { .. })) {
                    return;
                }
                let entry = match error {
                    // Offline is neutral: no banner for an answer never seen.
                    ApiError::Unreachable(_) | ApiError::TlsUntrusted(_) => {
                        CiEntry::Offline { at: now }
                    }
                    other => CiEntry::Failed {
                        message: other.to_string(),
                        at: now,
                    },
                };
                self.entries.insert(sha.to_owned(), entry);
            }
        }
    }

    /// A request that never ran (the service was paused).
    pub fn release(&mut self, sha: &str) {
        self.inflight.remove(sha);
    }

    /// Drops one answer so the next request asks again (the Try again button).
    pub fn forget(&mut self, sha: &str) {
        self.entries.remove(sha);
    }

    pub fn is_inflight(&self, sha: &str) -> bool {
        self.inflight.contains(sha)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.inflight.clear();
    }
}

/* ------------------------------ view models ------------------------------ */

pub fn badge_state(state: PrState) -> PrBadgeState {
    match state {
        PrState::Open => PrBadgeState::Open,
        PrState::Draft => PrBadgeState::Draft,
        PrState::Merged => PrBadgeState::Merged,
        PrState::Closed => PrBadgeState::Closed,
    }
}

fn state_label(state: PrState) -> SharedString {
    match state {
        PrState::Open => ui::t!("repo.hosting.pr.state.open"),
        PrState::Draft => ui::t!("repo.hosting.pr.state.draft"),
        PrState::Merged => ui::t!("repo.hosting.pr.state.merged"),
        PrState::Closed => ui::t!("repo.hosting.pr.state.closed"),
    }
}

fn review_mark(review: ReviewState) -> Option<ReviewMark> {
    match review {
        ReviewState::Approved => Some(ReviewMark::Approved),
        ReviewState::ChangesRequested => Some(ReviewMark::ChangesRequested),
        ReviewState::None => None,
    }
}

fn review_label(review: ReviewState) -> Option<SharedString> {
    match review {
        ReviewState::Approved => Some(ui::t!("repo.hosting.pr.review.approved")),
        ReviewState::ChangesRequested => Some(ui::t!("repo.hosting.pr.review.changes")),
        ReviewState::None => None,
    }
}

/// What a pull request badge shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrBadgeView {
    pub number: u64,
    pub state: PrBadgeState,
    pub review: Option<ReviewMark>,
    /// `#142`.
    pub text: SharedString,
    /// `#142 · Open · Approved`.
    pub tooltip: SharedString,
}

/// The badge of `pr`; `review` is known only once its popover was opened.
pub fn badge_view(pr: &PullRequest, review: Option<ReviewState>) -> PrBadgeView {
    let review = review.unwrap_or(ReviewState::None);
    let state = state_label(pr.state);
    let tooltip = match review_label(review) {
        Some(label) => ui::t!(
            "repo.hosting.pr.tooltip_review",
            number = pr.number,
            state = state,
            review = label
        ),
        None => ui::t!("repo.hosting.pr.tooltip", number = pr.number, state = state),
    };
    PrBadgeView {
        number: pr.number,
        state: badge_state(pr.state),
        review: review_mark(review),
        text: format!("#{}", pr.number).into(),
        tooltip,
    }
}

/// The glyph of a checks state; `None` for no checks at all (no dot).
pub fn ci_glyph(state: CiState) -> Option<CiGlyphState> {
    match state {
        CiState::Passed => Some(CiGlyphState::Passed),
        CiState::Failed => Some(CiGlyphState::Failed),
        CiState::Running => Some(CiGlyphState::Running),
        CiState::Pending => Some(CiGlyphState::Pending),
        CiState::Cancelled => Some(CiGlyphState::Cancelled),
        CiState::None => None,
    }
}

pub fn ci_label(state: CiGlyphState) -> SharedString {
    match state {
        CiGlyphState::Passed => ui::t!("repo.hosting.ci.passed"),
        CiGlyphState::Failed => ui::t!("repo.hosting.ci.failed"),
        CiGlyphState::Running => ui::t!("repo.hosting.ci.running"),
        CiGlyphState::Pending => ui::t!("repo.hosting.ci.pending"),
        CiGlyphState::Cancelled => ui::t!("repo.hosting.ci.cancelled"),
    }
}

/// `3m 12s`, `24s`, `1h 02m`; `—` when the service did not say.
pub fn format_duration(seconds: Option<u64>) -> String {
    match seconds {
        None => "—".to_owned(),
        Some(s) if s < 60 => format!("{s}s"),
        Some(s) if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        Some(s) => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
    }
}

/// `3 of 4 passed`.
pub fn checks_summary(checks: &CommitChecks) -> SharedString {
    let passed = checks
        .checks
        .iter()
        .filter(|c| c.state == CiState::Passed)
        .count();
    ui::t!(
        "repo.hosting.checks.summary",
        passed = passed,
        total = checks.checks.len()
    )
}

/// What the history rows and the sidebar draw, rebuilt whenever the state
/// changes. Cheap to clone behind an `Arc`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HostingView {
    /// Pull request badges by local branch name.
    pub prs: HashMap<String, PrBadgeView>,
    /// The checks glyph of the commits asked about.
    pub ci: HashMap<Oid, CiGlyphState>,
}

impl HostingView {
    /// The badge of a local branch, if it has a pull request.
    pub fn badge(&self, branch: &str) -> Option<&PrBadgeView> {
        self.prs.get(branch)
    }
}

/// The popover of one pull request, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopoverModel {
    pub number: u64,
    pub state: PrBadgeState,
    pub state_label: SharedString,
    /// `#142 on GitHub`.
    pub on_service: SharedString,
    pub title: String,
    pub author: String,
    /// `feat/x → main`, the head cut at [`BRANCH_MAX`].
    pub branches: String,
    /// `updated 2 h ago`.
    pub updated: SharedString,
    /// `None` while the review is being fetched.
    pub review: Option<ReviewState>,
    /// The checks line, once their answer is known.
    pub checks: Option<(Option<CiGlyphState>, SharedString)>,
    pub web_url: String,
    pub service: String,
    pub can_checkout: bool,
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let head: String = text.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

/// The checks line of the popover: `3 of 4 passed`, plus `so far` while they
/// run.
fn popover_checks(checks: &CommitChecks) -> (Option<CiGlyphState>, SharedString) {
    let glyph = ci_glyph(checks.overall);
    let line = if checks.checks.is_empty() {
        ui::t!("repo.hosting.pop.no_checks")
    } else if matches!(checks.overall, CiState::Running | CiState::Pending) {
        ui::t!(
            "repo.hosting.pop.checks_so_far",
            summary = checks_summary(checks)
        )
    } else {
        ui::t!("repo.hosting.pop.checks", summary = checks_summary(checks))
    };
    (glyph, line)
}

pub fn popover_model(
    pr: &PullRequest,
    review: Option<ReviewState>,
    checks: Option<&CommitChecks>,
    service: &str,
    now_unix: u64,
) -> PopoverModel {
    PopoverModel {
        number: pr.number,
        state: badge_state(pr.state),
        state_label: state_label(pr.state),
        on_service: ui::t!(
            "repo.hosting.pop.on_service",
            number = pr.number,
            service = service
        ),
        title: pr.title.clone(),
        author: pr.author.clone(),
        branches: format!("{} → {}", cut(&pr.head_branch, BRANCH_MAX), pr.base_branch),
        updated: ui::t!(
            "repo.hosting.pop.updated",
            when = relative_time(now_unix, pr.updated_at)
        ),
        review,
        checks: checks.map(popover_checks),
        web_url: pr.web_url.clone(),
        service: service.to_owned(),
        // A merged request has no branch to check out.
        can_checkout: pr.state != PrState::Merged,
    }
}

/// The local branch a pull request is checked out into: its head branch's name,
/// or `pr/<n>` when a local branch of that name exists already.
pub fn checkout_branch_name(pr: &PullRequest, locals: &[String]) -> String {
    if locals.contains(&pr.head_branch) {
        format!("pr/{}", pr.number)
    } else {
        pr.head_branch.clone()
    }
}

/// `+refs/pull/5/head:refs/heads/<branch>`-style refspec (no force: an existing
/// `pr/<n>` that diverged fails instead of losing commits).
pub fn checkout_refspec(kind: ServiceKind, number: u64, branch: &str) -> Result<String, ApiError> {
    Ok(format!(
        "{}:refs/heads/{branch}",
        pull_request_head_ref(kind, number)?
    ))
}

/* --------------------------------- state ---------------------------------- */

/// Everything the repository view knows about hosting status.
pub struct HostingState {
    pub client: Client,
    pub target: Option<Target>,
    pub tracked: Vec<TrackedBranch>,
    prs: HashMap<String, PullRequest>,
    /// When the last pull request request was started, whatever came of it.
    pr_attempted_at: Option<Instant>,
    pr_inflight: bool,
    reviews: HashMap<u64, ReviewState>,
    pub ci: CiCache,
    pub health: Health,
    /// The tips of the primary remote's remote-tracking branches.
    remote_tips: Vec<Oid>,
    /// Commits known to be on the host: reachable from `remote_tips` through
    /// the commits the history has loaded.
    reach: HashSet<Oid>,
    /// How many loaded commits `reach` was computed over; `None` = outdated.
    reach_loaded: Option<usize>,
}

impl Default for HostingState {
    fn default() -> Self {
        Self {
            client: Client::new(),
            target: None,
            tracked: Vec::new(),
            prs: HashMap::new(),
            pr_attempted_at: None,
            pr_inflight: false,
            reviews: HashMap::new(),
            ci: CiCache::default(),
            health: Health::default(),
            remote_tips: Vec::new(),
            reach: HashSet::new(),
            reach_loaded: None,
        }
    }
}

impl HostingState {
    /// Adopts a (possibly different) target and tracked branches. A new target
    /// drops everything fetched for the old one.
    pub fn retarget(
        &mut self,
        target: Option<Target>,
        tracked: Vec<TrackedBranch>,
        remote_tips: Vec<Oid>,
    ) {
        if self.remote_tips != remote_tips {
            self.reach_loaded = None;
        }
        self.remote_tips = remote_tips;
        if self.target != target {
            self.prs.clear();
            self.pr_attempted_at = None;
            self.pr_inflight = false;
            self.reviews.clear();
            self.ci.clear();
            self.health = Health::default();
        }
        self.target = target;
        self.tracked = tracked;
    }

    /// Whether a pull request refresh should start now.
    pub fn wants_prs(&self, trigger: PrTrigger, now: Instant, now_unix: u64) -> bool {
        self.target.is_some()
            && !self.pr_inflight
            && !self.tracked.is_empty()
            && self.health.can_request(now_unix)
            && pr_refresh_due(trigger, self.pr_attempted_at, now)
    }

    pub fn begin_prs(&mut self, now: Instant) {
        self.pr_inflight = true;
        self.pr_attempted_at = Some(now);
    }

    /// Records a failed request: `anonymous` requests refused with 404 / 403
    /// mean there is no access without an account.
    pub fn record(&mut self, error: &ApiError, now_unix: u64) -> ErrorEffect {
        let anonymous = self.target.as_ref().is_none_or(|t| t.account_id.is_none());
        self.health.apply(error, now_unix, anonymous)
    }

    /// The tips of the primary remote's remote-tracking branches.
    pub fn remote_tips(&self) -> &[Oid] {
        &self.remote_tips
    }

    /// Whether the reachable set must be computed again for `loaded` commits.
    pub fn reach_outdated(&self, loaded: usize) -> bool {
        self.reach_loaded != Some(loaded)
    }

    pub fn set_reach(&mut self, reach: HashSet<Oid>, loaded: usize) {
        self.reach = reach;
        self.reach_loaded = Some(loaded);
    }

    /// Whether the host is known to have `oid`.
    pub fn on_remote(&self, oid: Oid) -> bool {
        self.reach.contains(&oid)
    }

    /// The remote branches to ask about, bounded for services that ask per
    /// branch.
    pub fn pr_branches(&self) -> Vec<String> {
        let kind = self.target.as_ref().map(|t| t.link.instance.kind);
        let mut names: Vec<String> = Vec::new();
        for branch in &self.tracked {
            if !names.contains(&branch.remote_branch) {
                names.push(branch.remote_branch.clone());
            }
        }
        if kind == Some(ServiceKind::GitLab) {
            names.truncate(MAX_PR_BRANCHES);
        }
        names
    }

    /// Stores the pull request map (or the failure) and says what a failure
    /// means.
    pub fn finish_prs(
        &mut self,
        result: Result<HashMap<String, PullRequest>, ApiError>,
        now_unix: u64,
    ) -> Option<ErrorEffect> {
        self.pr_inflight = false;
        match result {
            Ok(map) => {
                self.prs = map;
                self.health.succeeded();
                None
            }
            Err(error) => Some(self.record(&error, now_unix)),
        }
    }

    pub fn pr_for_local(&self, local: &str) -> Option<&PullRequest> {
        let tracked = self.tracked.iter().find(|b| b.local == local)?;
        self.prs.get(&tracked.remote_branch)
    }

    pub fn pr_by_number(&self, number: u64) -> Option<&PullRequest> {
        self.prs.values().find(|pr| pr.number == number)
    }

    /// The head commit of pull request `number` as the host has it: the
    /// remote-tracking tip of its branch.
    pub fn tip_of_pr(&self, number: u64) -> Option<Oid> {
        let pr = self.pr_by_number(number)?;
        self.tracked
            .iter()
            .find(|b| b.remote_branch == pr.head_branch)
            .and_then(|b| b.remote_tip)
    }

    /// The local branch with a pull request whose tip is `oid`.
    pub fn pr_branch_at(&self, oid: Oid) -> Option<&str> {
        self.tracked
            .iter()
            .find(|b| b.tip == oid && self.prs.contains_key(&b.remote_branch))
            .map(|b| b.local.as_str())
    }

    pub fn set_review(&mut self, number: u64, review: ReviewState) {
        self.reviews.insert(number, review);
    }

    pub fn review(&self, number: u64) -> Option<ReviewState> {
        self.reviews.get(&number).copied()
    }

    /// The head commits of branches that have a pull request: their
    /// remote-tracking tips, which the host certainly has (a local tip may be
    /// unpushed).
    pub fn pr_tips(&self) -> Vec<Oid> {
        self.tracked
            .iter()
            .filter(|b| self.prs.contains_key(&b.remote_branch))
            .filter_map(|b| b.remote_tip)
            .collect()
    }

    /// The commits worth asking checks for: PR heads, plus the selected commit
    /// and HEAD when the host is known to have them.
    pub fn ci_candidates(&self, selected: Option<Oid>, head: Option<Oid>) -> Vec<Oid> {
        ci_wanted(
            selected.filter(|oid| self.on_remote(*oid)),
            head.filter(|oid| self.on_remote(*oid)),
            &self.pr_tips(),
        )
    }

    /// Whether a commit the rows show still has pending or running checks.
    pub fn has_pending_ci(&self, selected: Option<Oid>, head: Option<Oid>) -> bool {
        self.ci_candidates(selected, head).iter().any(|oid| {
            self.ci
                .checks(&oid.to_string())
                .is_some_and(|c| matches!(c.overall, CiState::Pending | CiState::Running))
        })
    }

    /// The commits whose checks should be asked for now.
    pub fn ci_due(
        &self,
        selected: Option<Oid>,
        head: Option<Oid>,
        now: Instant,
        now_unix: u64,
    ) -> Vec<Oid> {
        if self.target.is_none() || !self.health.can_request(now_unix) {
            return Vec::new();
        }
        self.ci_candidates(selected, head)
            .into_iter()
            .filter(|oid| self.ci.needs_fetch(&oid.to_string(), now))
            .collect()
    }

    /// What the rows draw.
    pub fn view(&self) -> HostingView {
        if !self.health.badges_visible() {
            return HostingView::default();
        }
        let prs = self
            .tracked
            .iter()
            .filter_map(|b| {
                let pr = self.prs.get(&b.remote_branch)?;
                Some((b.local.clone(), badge_view(pr, self.review(pr.number))))
            })
            .collect();
        let ci = self
            .ci_oids()
            .into_iter()
            .filter_map(|oid| {
                let checks = self.ci.checks(&oid.to_string())?;
                Some((oid, ci_glyph(checks.overall)?))
            })
            .collect();
        HostingView { prs, ci }
    }

    fn ci_oids(&self) -> Vec<Oid> {
        // Only commits that were asked about can have an entry.
        self.ci
            .entries
            .keys()
            .filter_map(|sha| Oid::from_hex(sha))
            .collect()
    }

    /// The service's name as menus write it, "GitHub".
    pub fn service_name(&self) -> String {
        self.target
            .as_ref()
            .map(|t| t.link.service_name().to_owned())
            .unwrap_or_default()
    }
}

/* -------------------------------- requests -------------------------------- */

/// One batch of blocking calls against the target; build it on the UI thread,
/// run it on the background executor.
#[derive(Clone)]
pub struct Request {
    client: Client,
    instance: Instance,
    coords: RepoCoords,
    account_id: Option<String>,
    store: Option<Arc<dyn SecretStore>>,
}

impl Request {
    pub fn new(client: Client, target: &Target, store: Option<Arc<dyn SecretStore>>) -> Request {
        Request {
            client,
            instance: target.link.instance.clone(),
            coords: target.link.coords.clone(),
            account_id: target.account_id.clone(),
            store,
        }
    }

    /// The account's token: a failure to read it falls back to anonymous
    /// reads, which still work for public repositories.
    fn token(&self) -> Option<String> {
        let id = self.account_id.as_deref()?;
        let store = match &self.store {
            Some(store) => store.clone(),
            None => crate::accounts::resolve_store().ok()?,
        };
        store.get(id).ok().flatten()
    }

    pub fn pull_requests(
        &self,
        branches: &[String],
    ) -> Result<HashMap<String, PullRequest>, ApiError> {
        let token = self.token();
        let auth = Auth::new(token.as_deref(), TokenKind::Pat);
        pull_requests_for_branches(&self.client, &self.instance, &self.coords, &auth, branches)
    }

    pub fn review(&self, number: u64) -> Result<ReviewState, ApiError> {
        let token = self.token();
        let auth = Auth::new(token.as_deref(), TokenKind::Pat);
        pull_request_review(&self.client, &self.instance, &self.coords, &auth, number)
    }

    /// Asks for each sha in turn; stops at the first failure that would repeat
    /// (rejected token, rate limit, unsupported service) and leaves the rest
    /// unanswered.
    pub fn checks(&self, shas: &[String]) -> Vec<(String, Result<CommitChecks, ApiError>)> {
        let token = self.token();
        let auth = Auth::new(token.as_deref(), TokenKind::Pat);
        let mut answers = Vec::new();
        for sha in shas {
            let result = commit_checks(&self.client, &self.instance, &self.coords, &auth, sha);
            let stop = result.as_ref().err().is_some_and(|e| {
                matches!(
                    classify(e),
                    ErrorEffect::TokenRejected
                        | ErrorEffect::RateLimited { .. }
                        | ErrorEffect::Unsupported
                        | ErrorEffect::Offline
                )
            });
            answers.push((sha.clone(), result));
            if stop {
                break;
            }
        }
        answers
    }
}

pub fn unix_now() -> u64 {
    crate::history_view::unix_now()
}

/* --------------------------------- popover -------------------------------- */

/// The popover under a pull request badge.
pub struct PrPopover {
    model: PopoverModel,
    view: WeakEntity<RepositoryView>,
    layer: Option<LayerId>,
}

impl PrPopover {
    pub fn new(model: PopoverModel, view: WeakEntity<RepositoryView>) -> Self {
        Self {
            model,
            view,
            layer: None,
        }
    }

    pub fn set_layer(&mut self, layer: LayerId) {
        self.layer = Some(layer);
    }

    pub fn number(&self) -> u64 {
        self.model.number
    }

    /// Replaces what is shown, keeping the popover open.
    pub fn set_model(&mut self, model: PopoverModel, cx: &mut Context<Self>) {
        self.model = model;
        cx.notify();
    }

    fn close(&self, window: &mut Window, cx: &mut gpui::App) {
        if let Some(layer) = self.layer {
            overlay::close(layer, window, cx);
        }
    }
}

impl Render for PrPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let model = self.model.clone();
        let review = match model.review {
            None => div()
                .text_color(colors.muted)
                .child(ui::t!("repo.hosting.pop.review_loading")),
            Some(review) => match (review_mark(review), review_label(review)) {
                (Some(mark), Some(label)) => div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(ui::Icon::new(mark.icon()).size(px(12.)).color(colors.text))
                    .child(label),
                _ => div()
                    .text_color(colors.muted)
                    .child(ui::t!("repo.hosting.pop.no_reviews")),
            },
        };
        let dot = || div().text_color(colors.muted).child("·");
        let mut status = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .text_size(FS_SM)
            .child(review);
        if let Some((glyph, line)) = model.checks.clone() {
            status = status.child(dot()).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .when_some(glyph, |this, glyph| {
                        this.child(CiGlyph::new("pr-popover-ci", glyph).tooltip(ci_label(glyph)))
                    })
                    .child(line),
            );
        }

        let open_view = self.view.clone();
        let open_url = model.web_url.clone();
        let service = model.service.clone();
        let service_label = service.clone();
        let checkout_view = self.view.clone();
        let number = model.number;
        let this_open = cx.entity().downgrade();
        let this_checkout = cx.entity().downgrade();
        div()
            .w(px(340.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .rounded(R_MD)
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(PrBadge::new(
                        "pr-popover-state",
                        model.state,
                        model.state_label.clone(),
                    ))
                    .child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(model.on_service.clone()),
                    ),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(19.))
                    .child(model.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(model.author.clone())
                    .child("·")
                    .child(model.branches.clone())
                    .child("·")
                    .child(model.updated.clone()),
            )
            .child(status)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("pr-popover-open")
                            .size(ButtonSize::Small)
                            .icon(IconName::External)
                            .label(ui::t!("repo.details.open_on_host", service = service_label))
                            .on_click(move |_, window, cx| {
                                if let Some(popover) = this_open.upgrade() {
                                    popover.update(cx, |p, cx| p.close(window, cx));
                                }
                                let url = open_url.clone();
                                open_view
                                    .update(cx, |view, cx| view.open_hosting_url(&url, window, cx))
                                    .ok();
                            }),
                    )
                    .child({
                        let button = Button::primary("pr-popover-checkout")
                            .size(ButtonSize::Small)
                            .label(ui::t!("repo.hosting.pop.checkout"));
                        if model.can_checkout {
                            button
                                .on_click(move |_, window, cx| {
                                    if let Some(popover) = this_checkout.upgrade() {
                                        popover.update(cx, |p, cx| p.close(window, cx));
                                    }
                                    checkout_view
                                        .update(cx, |view, cx| {
                                            view.checkout_pull_request(number, window, cx)
                                        })
                                        .ok();
                                })
                                .into_any_element()
                        } else {
                            button
                                .disabled(true)
                                .tooltip(ui::t!("repo.hosting.pop.checkout_merged_tip"))
                                .into_any_element()
                        }
                    }),
            )
    }
}

/* ---------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use forge::api::Check;
    use std::collections::BTreeMap;

    fn oid(n: u8) -> Oid {
        Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap()
    }

    fn targets(list: &[(&str, &str)]) -> Vec<RemoteTargets> {
        list.iter()
            .map(|(name, url)| RemoteTargets {
                name: (*name).into(),
                fetch_url: (*url).into(),
                push_urls: vec![(*url).into()],
            })
            .collect()
    }

    fn branch(name: &str, upstream: Option<&str>, n: u8) -> Ref {
        Ref {
            full_name: format!("refs/heads/{name}"),
            name: name.into(),
            kind: RefKind::LocalBranch,
            target: oid(n),
            upstream: upstream.map(str::to_owned),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn remote_branch(name: &str, n: u8) -> Ref {
        Ref {
            full_name: format!("refs/remotes/{name}"),
            name: name.into(),
            kind: RefKind::RemoteBranch,
            target: oid(n),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    /// A state targeting `origin` on GitHub with the given refs tracked.
    fn state_with(refs: &[Ref], account: Option<&str>) -> HostingState {
        let mut state = HostingState::default();
        let t = targets(&[("origin", "https://github.com/o/r.git")]);
        let accounts: Vec<AccountRef> = account
            .map(|id| AccountRef {
                id: id.into(),
                instance: Instance::new(ServiceKind::GitHub, "https://github.com").unwrap(),
                login: "me".into(),
            })
            .into_iter()
            .collect();
        let target = primary_target(&accounts, &t, None, refs, &BTreeMap::new()).unwrap();
        let tracked = tracked_branches(refs, &t, &target);
        state.retarget(Some(target), tracked, primary_remote_tips(refs, "origin"));
        state
    }

    fn pr(number: u64, head: &str, state: PrState) -> PullRequest {
        PullRequest {
            number,
            title: format!("title {number}"),
            state,
            review: ReviewState::None,
            author: "mira".into(),
            head_branch: head.into(),
            head_repo_full_name: Some("o/r".into()),
            base_branch: "main".into(),
            updated_at: 1_000,
            web_url: format!("https://github.com/o/r/pull/{number}"),
        }
    }

    fn checks(overall: CiState, states: &[CiState]) -> CommitChecks {
        CommitChecks {
            overall,
            checks: states
                .iter()
                .map(|s| Check {
                    name: "build".into(),
                    state: *s,
                    duration_secs: Some(192),
                    web_url: None,
                })
                .collect(),
        }
    }

    fn head_on(name: &str) -> Head {
        Head::Branch {
            name: name.into(),
            target: Some(oid(1)),
        }
    }

    // ----- primary remote -----

    #[test]
    fn the_upstream_remote_of_the_current_branch_speaks_for_the_repository() {
        let t = targets(&[
            ("origin", "git@github.com:o/r.git"),
            ("fork", "git@github.com:me/r.git"),
        ]);
        let refs = [branch("feat", Some("fork/feat"), 1)];
        let target =
            primary_target(&[], &t, Some(&head_on("feat")), &refs, &BTreeMap::new()).unwrap();
        assert_eq!(target.remote, "fork");
        assert_eq!(target.link.coords.repo, "r");
        assert_eq!(target.link.coords.namespace, ["me"]);
        assert_eq!(target.account_id, None);
    }

    #[test]
    fn without_an_upstream_origin_wins_over_other_remotes() {
        let t = targets(&[
            ("aaa", "git@github.com:x/r.git"),
            ("origin", "git@github.com:o/r.git"),
        ]);
        let target = primary_target(&[], &t, None, &[], &BTreeMap::new()).unwrap();
        assert_eq!(target.remote, "origin");
    }

    #[test]
    fn remotes_on_unknown_or_api_less_hosts_are_not_a_target() {
        let t = targets(&[
            ("origin", "git@bitbucket.org:o/r.git"),
            ("other", "git@example.invalid:o/r.git"),
        ]);
        assert!(primary_target(&[], &t, None, &[], &BTreeMap::new()).is_none());
        // A usable remote behind an unusable one is still found.
        let t = targets(&[
            ("origin", "git@bitbucket.org:o/r.git"),
            ("mirror", "git@gitlab.com:o/r.git"),
        ]);
        assert_eq!(
            primary_target(&[], &t, None, &[], &BTreeMap::new())
                .unwrap()
                .remote,
            "mirror"
        );
    }

    #[test]
    fn a_configured_account_supplies_the_token_owner() {
        let account = AccountRef {
            id: "acc1".into(),
            instance: Instance::new(ServiceKind::GitHub, "https://github.com").unwrap(),
            login: "me".into(),
        };
        let t = targets(&[("origin", "https://github.com/o/r.git")]);
        let target = primary_target(&[account], &t, None, &[], &BTreeMap::new()).unwrap();
        assert_eq!(target.account_id.as_deref(), Some("acc1"));
    }

    #[test]
    fn only_branches_tracking_the_primary_remote_are_tracked() {
        let t = targets(&[
            ("origin", "git@github.com:o/r.git"),
            ("fork", "git@github.com:me/r.git"),
        ]);
        let target = primary_target(&[], &t, None, &[], &BTreeMap::new()).unwrap();
        let refs = [
            branch("a", Some("origin/feature/a"), 1),
            branch("b", Some("fork/b"), 2),
            branch("c", None, 3),
        ];
        let tracked = tracked_branches(&refs, &t, &target);
        assert_eq!(
            tracked,
            vec![TrackedBranch {
                local: "a".into(),
                remote_branch: "feature/a".into(),
                tip: oid(1),
                remote_tip: None,
            }]
        );
    }

    // ----- refresh policy -----

    #[test]
    fn a_focus_refreshes_the_map_only_when_it_is_older_than_a_minute() {
        let now = Instant::now();
        let fresh = now - Duration::from_secs(59);
        let stale = now - Duration::from_secs(60);
        assert!(pr_refresh_due(PrTrigger::Focus, None, now));
        assert!(!pr_refresh_due(PrTrigger::Focus, Some(fresh), now));
        assert!(pr_refresh_due(PrTrigger::Focus, Some(stale), now));
    }

    #[test]
    fn open_and_manual_refresh_always_refresh() {
        let now = Instant::now();
        for trigger in [PrTrigger::Open, PrTrigger::Manual] {
            assert!(pr_refresh_due(trigger, Some(now), now));
        }
    }

    #[test]
    fn nothing_is_asked_for_without_a_target_or_tracked_branches() {
        let state = HostingState::default();
        assert!(!state.wants_prs(PrTrigger::Open, Instant::now(), 0));
    }

    // ----- which commits get a CI request -----

    #[test]
    fn ci_wants_selected_head_and_pr_tips_without_repeats_and_bounded() {
        let tips: Vec<Oid> = (10..40).map(oid).collect();
        let wanted = ci_wanted(Some(oid(10)), Some(oid(1)), &tips);
        assert_eq!(wanted[0], oid(10));
        assert_eq!(wanted[1], oid(1));
        // selected + head + at most MAX_CI_TIPS tips, the selected one deduped.
        assert_eq!(wanted.len(), 2 + MAX_CI_TIPS - 1);
        assert_eq!(ci_wanted(None, None, &[]), Vec::<Oid>::new());
    }

    #[test]
    fn a_final_answer_is_never_asked_again() {
        let mut cache = CiCache::default();
        let now = Instant::now();
        for (sha, state) in [
            ("p", CiState::Passed),
            ("f", CiState::Failed),
            ("c", CiState::Cancelled),
        ] {
            assert!(cache.needs_fetch(sha, now));
            cache.store(sha, Ok(checks(state, &[state])), now);
            assert!(!cache.needs_fetch(sha, now + Duration::from_secs(86_400)));
        }
    }

    #[test]
    fn a_pending_or_running_answer_is_asked_again_every_thirty_seconds() {
        let mut cache = CiCache::default();
        let now = Instant::now();
        for (sha, state) in [("r", CiState::Running), ("p", CiState::Pending)] {
            cache.store(sha, Ok(checks(state, &[state])), now);
            assert!(!cache.needs_fetch(sha, now + Duration::from_secs(29)));
            assert!(cache.needs_fetch(sha, now + Duration::from_secs(30)));
        }
    }

    #[test]
    fn no_checks_are_rechecked_after_five_minutes_and_failures_after_thirty_seconds() {
        let mut cache = CiCache::default();
        let now = Instant::now();
        cache.store("n", Ok(checks(CiState::None, &[])), now);
        assert!(!cache.needs_fetch("n", now + Duration::from_secs(299)));
        assert!(cache.needs_fetch("n", now + Duration::from_secs(300)));
        cache.store("e", Err(ApiError::Status(503)), now);
        assert!(!cache.needs_fetch("e", now + Duration::from_secs(29)));
        assert!(cache.needs_fetch("e", now + Duration::from_secs(30)));
    }

    #[test]
    fn one_request_per_commit_at_a_time_and_a_failed_refetch_keeps_the_last_answer() {
        let mut cache = CiCache::default();
        let now = Instant::now();
        cache.mark_inflight("s");
        assert!(!cache.needs_fetch("s", now));
        cache.store("s", Ok(checks(CiState::Running, &[CiState::Running])), now);
        cache.mark_inflight("s");
        cache.store("s", Err(ApiError::Unreachable("offline".into())), now);
        assert_eq!(cache.checks("s").unwrap().overall, CiState::Running);
    }

    #[test]
    fn ci_due_skips_final_commits_and_everything_while_paused() {
        let refs = [
            branch("a", Some("origin/a"), 5),
            remote_branch("origin/a", 2),
        ];
        let mut state = state_with(&refs, None);
        state.finish_prs(
            Ok(HashMap::from([("a".to_owned(), pr(7, "a", PrState::Open))])),
            0,
        );
        // HEAD (1) is on the host; the PR head is the remote-tracking tip (2).
        state.set_reach(HashSet::from([oid(1), oid(2)]), 0);
        let now = Instant::now();
        let due = state.ci_due(None, Some(oid(1)), now, 0);
        assert_eq!(due, vec![oid(1), oid(2)]);
        state.ci.store(
            &oid(1).to_string(),
            Ok(checks(CiState::Passed, &[CiState::Passed])),
            now,
        );
        assert_eq!(state.ci_due(None, Some(oid(1)), now, 0), vec![oid(2)]);
        state.health.apply(
            &ApiError::RateLimited {
                reset_at: Some(100),
            },
            0,
            false,
        );
        assert!(state.ci_due(None, Some(oid(1)), now, 50).is_empty());
        assert_eq!(state.ci_due(None, Some(oid(1)), now, 100), vec![oid(2)]);
    }

    // ----- errors -----

    #[test]
    fn errors_map_to_their_effect() {
        assert_eq!(
            classify(&ApiError::Unauthorized),
            ErrorEffect::TokenRejected
        );
        assert_eq!(
            classify(&ApiError::RateLimited { reset_at: Some(5) }),
            ErrorEffect::RateLimited { reset_at: Some(5) }
        );
        assert_eq!(
            classify(&ApiError::Unreachable("x".into())),
            ErrorEffect::Offline
        );
        assert_eq!(
            classify(&ApiError::TlsUntrusted("x".into())),
            ErrorEffect::Offline
        );
        assert_eq!(classify(&ApiError::Unsupported), ErrorEffect::Unsupported);
        assert_eq!(classify(&ApiError::NotFound), ErrorEffect::Other);
        assert_eq!(classify(&ApiError::Status(500)), ErrorEffect::Other);
    }

    #[test]
    fn a_rejected_token_stops_requests_and_hides_badges_until_a_manual_retry() {
        let mut health = Health::default();
        assert!(health.can_request(0) && health.badges_visible());
        health.apply(&ApiError::Unauthorized, 0, false);
        assert!(!health.can_request(0) && !health.badges_visible());
        health.manual_retry();
        assert!(health.can_request(0) && health.badges_visible());
    }

    #[test]
    fn a_rate_limit_pauses_requests_until_the_reset_and_falls_back_to_a_minute() {
        let mut health = Health::default();
        health.apply(
            &ApiError::RateLimited {
                reset_at: Some(500),
            },
            100,
            false,
        );
        assert!(!health.can_request(499) && health.can_request(500));
        assert!(health.badges_visible());
        health.apply(&ApiError::RateLimited { reset_at: None }, 100, false);
        assert!(!health.can_request(159) && health.can_request(160));
    }

    #[test]
    fn offline_is_silent_and_retried_on_the_next_trigger() {
        let mut health = Health::default();
        health.apply(&ApiError::Unreachable("dns".into()), 0, false);
        assert!(health.offline && health.can_request(0) && health.badges_visible());
        health.succeeded();
        assert!(!health.offline);
    }

    #[test]
    fn an_unsupported_service_is_never_asked_again() {
        let mut health = Health::default();
        health.apply(&ApiError::Unsupported, 0, false);
        assert!(!health.can_request(u64::MAX) && !health.badges_visible());
    }

    #[test]
    fn a_rejected_token_is_announced_once_per_account() {
        let log = RejectionLog::default();
        assert!(log.first("a"));
        assert!(!log.first("a"));
        assert!(log.first("b"));
    }

    #[test]
    fn a_failed_pull_request_refresh_keeps_the_last_map_but_a_rejected_token_hides_it() {
        let refs = [branch("a", Some("origin/a"), 2)];
        let mut state = state_with(&refs, None);
        let now = Instant::now();
        state.begin_prs(now);
        state.finish_prs(
            Ok(HashMap::from([("a".to_owned(), pr(7, "a", PrState::Open))])),
            0,
        );
        assert!(state.view().badge("a").is_some());
        state.begin_prs(now);
        assert_eq!(
            state.finish_prs(Err(ApiError::Unreachable("x".into())), 0),
            Some(ErrorEffect::Offline)
        );
        assert!(state.view().badge("a").is_some());
        state.begin_prs(now);
        assert_eq!(
            state.finish_prs(Err(ApiError::Unauthorized), 0),
            Some(ErrorEffect::TokenRejected)
        );
        assert!(state.view().badge("a").is_none());
    }

    // ----- badge view model -----

    #[test]
    fn the_badge_follows_the_state_and_the_known_review() {
        let open = badge_view(&pr(142, "a", PrState::Open), None);
        assert_eq!(open.text, "#142");
        assert_eq!(open.state, PrBadgeState::Open);
        assert_eq!(open.review, None);
        assert_eq!(open.tooltip, "#142 · Open");
        let approved = badge_view(&pr(142, "a", PrState::Open), Some(ReviewState::Approved));
        assert_eq!(approved.review, Some(ReviewMark::Approved));
        assert_eq!(approved.tooltip, "#142 · Open · Approved");
        let changes = badge_view(
            &pr(128, "a", PrState::Closed),
            Some(ReviewState::ChangesRequested),
        );
        assert_eq!(changes.state, PrBadgeState::Closed);
        assert_eq!(changes.review, Some(ReviewMark::ChangesRequested));
        assert_eq!(changes.tooltip, "#128 · Closed · Changes requested");
        assert_eq!(
            badge_view(&pr(1, "a", PrState::Draft), None).state,
            PrBadgeState::Draft
        );
        assert_eq!(
            badge_view(&pr(1, "a", PrState::Merged), None).state,
            PrBadgeState::Merged
        );
    }

    // ----- popover -----

    #[test]
    fn checkout_uses_the_head_branch_name_unless_it_exists_locally() {
        let p = pr(142, "feat/idle-lanes", PrState::Open);
        assert_eq!(
            checkout_branch_name(&p, &["main".into()]),
            "feat/idle-lanes"
        );
        assert_eq!(
            checkout_branch_name(&p, &["main".into(), "feat/idle-lanes".into()]),
            "pr/142"
        );
    }

    #[test]
    fn the_checkout_refspec_fetches_the_service_head_ref_into_a_local_branch() {
        assert_eq!(
            checkout_refspec(ServiceKind::GitHub, 5, "pr/5").unwrap(),
            "refs/pull/5/head:refs/heads/pr/5"
        );
        assert_eq!(
            checkout_refspec(ServiceKind::GitLab, 9, "x").unwrap(),
            "refs/merge-requests/9/head:refs/heads/x"
        );
        assert!(checkout_refspec(ServiceKind::Bitbucket, 1, "x").is_err());
    }

    #[test]
    fn checkout_is_disabled_for_a_merged_request_only() {
        for (state, can) in [
            (PrState::Open, true),
            (PrState::Draft, true),
            (PrState::Closed, true),
            (PrState::Merged, false),
        ] {
            let model = popover_model(&pr(1, "a", state), None, None, "GitHub", 10_000);
            assert_eq!(model.can_checkout, can, "{state:?}");
        }
    }

    #[test]
    fn the_popover_summarises_the_request_and_its_checks() {
        let mut p = pr(139, "feat/palette", PrState::Draft);
        p.updated_at = 10_000 - 7_200;
        let running = checks(
            CiState::Running,
            &[
                CiState::Passed,
                CiState::Running,
                CiState::Pending,
                CiState::Passed,
            ],
        );
        let model = popover_model(&p, None, Some(&running), "GitHub", 10_000);
        assert_eq!(model.on_service, "#139 on GitHub");
        assert_eq!(model.branches, "feat/palette → main");
        assert_eq!(model.updated, "updated 2 h ago");
        assert_eq!(model.state, PrBadgeState::Draft);
        let (glyph, line) = model.checks.unwrap();
        assert_eq!(glyph, Some(CiGlyphState::Running));
        assert_eq!(line, "Checks 2 of 4 passed so far");
        let done = checks(CiState::Passed, &[CiState::Passed, CiState::Passed]);
        let (_, line) = popover_model(
            &p,
            Some(ReviewState::Approved),
            Some(&done),
            "GitHub",
            10_000,
        )
        .checks
        .unwrap();
        assert_eq!(line, "Checks 2 of 2 passed");
    }

    #[test]
    fn a_long_head_branch_is_cut_with_an_ellipsis() {
        let long = "feature/".to_owned() + &"x".repeat(60);
        let model = popover_model(&pr(1, &long, PrState::Open), None, None, "GitHub", 10_000);
        let head = model.branches.split(" → ").next().unwrap();
        assert_eq!(head.chars().count(), BRANCH_MAX);
        assert!(head.ends_with('…'));
    }

    // ----- history CI dot and checks list -----

    #[test]
    fn the_history_dot_maps_each_state_and_skips_no_checks() {
        assert_eq!(ci_glyph(CiState::Passed), Some(CiGlyphState::Passed));
        assert_eq!(ci_glyph(CiState::Failed), Some(CiGlyphState::Failed));
        assert_eq!(ci_glyph(CiState::Running), Some(CiGlyphState::Running));
        assert_eq!(ci_glyph(CiState::Pending), Some(CiGlyphState::Pending));
        assert_eq!(ci_glyph(CiState::Cancelled), Some(CiGlyphState::Cancelled));
        assert_eq!(ci_glyph(CiState::None), None);
    }

    #[test]
    fn the_view_carries_a_dot_only_for_commits_with_checks() {
        let mut state = HostingState::default();
        let now = Instant::now();
        state.ci.store(
            &oid(1).to_string(),
            Ok(checks(CiState::Failed, &[CiState::Failed])),
            now,
        );
        state
            .ci
            .store(&oid(2).to_string(), Ok(checks(CiState::None, &[])), now);
        state
            .ci
            .store(&oid(3).to_string(), Err(ApiError::Status(500)), now);
        let view = state.view();
        assert_eq!(view.ci.get(&oid(1)), Some(&CiGlyphState::Failed));
        assert_eq!(view.ci.len(), 1);
    }

    // ----- fix round 1 -----

    #[test]
    fn only_the_explicit_refresh_resets_health_and_internal_reloads_respect_staleness() {
        assert!(PrTrigger::Manual.resets_health());
        for trigger in [PrTrigger::Open, PrTrigger::Focus, PrTrigger::AfterNetwork] {
            assert!(!trigger.resets_health());
        }
        // The repository's own reloads (ref change, worktree change).
        assert!(!INTERNAL_RELOAD.resets_health());
        let now = Instant::now();
        assert!(!pr_refresh_due(INTERNAL_RELOAD, Some(now), now));
    }

    #[test]
    fn a_failed_attempt_backs_off_for_a_minute_for_focus_and_network_triggers() {
        let refs = [branch("a", Some("origin/a"), 2)];
        let mut state = state_with(&refs, Some("acc"));
        let now = Instant::now();
        assert!(state.wants_prs(PrTrigger::Focus, now, 0));
        state.begin_prs(now);
        state.finish_prs(Err(ApiError::Status(500)), 0);
        let soon = now + Duration::from_secs(59);
        let later = now + Duration::from_secs(60);
        for trigger in [PrTrigger::Focus, PrTrigger::AfterNetwork] {
            assert!(!state.wants_prs(trigger, soon, 0), "{trigger:?}");
            assert!(state.wants_prs(trigger, later, 0), "{trigger:?}");
        }
        // Opening and the user's refresh are not held back.
        assert!(state.wants_prs(PrTrigger::Open, soon, 0));
        assert!(state.wants_prs(PrTrigger::Manual, soon, 0));
    }

    #[test]
    fn an_anonymous_refusal_means_no_access_until_the_account_or_remote_changes() {
        let refs = [branch("a", Some("origin/a"), 2)];
        for error in [ApiError::NotFound, ApiError::Forbidden] {
            let mut state = state_with(&refs, None);
            assert_eq!(state.record(&error, 0), ErrorEffect::NoAccess);
            assert!(!state.health.can_request(0) && !state.health.badges_visible());
            assert!(!state.wants_prs(PrTrigger::Open, Instant::now(), 0));
            // Same target again: still no access.
            let t = targets(&[("origin", "https://github.com/o/r.git")]);
            let target = state.target.clone();
            state.retarget(target, state.tracked.clone(), Vec::new());
            assert!(!state.health.can_request(0));
            // The user's refresh asks again.
            state.health.manual_retry();
            assert!(state.health.can_request(0));
            state.record(&error, 0);
            // An account appearing changes the target and clears it.
            let account = AccountRef {
                id: "acc".into(),
                instance: Instance::new(ServiceKind::GitHub, "https://github.com").unwrap(),
                login: "me".into(),
            };
            let with_account =
                primary_target(&[account], &t, None, &refs, &BTreeMap::new()).unwrap();
            state.retarget(Some(with_account), state.tracked.clone(), Vec::new());
            assert!(state.health.can_request(0));
        }
    }

    #[test]
    fn a_refusal_with_an_account_is_only_a_missing_answer() {
        let refs = [branch("a", Some("origin/a"), 2)];
        let mut state = state_with(&refs, Some("acc"));
        assert_eq!(state.record(&ApiError::NotFound, 0), ErrorEffect::Other);
        assert!(state.health.can_request(0));
    }

    #[test]
    fn a_success_does_not_lift_a_rate_limit_pause() {
        let mut health = Health::default();
        health.apply(
            &ApiError::RateLimited {
                reset_at: Some(500),
            },
            100,
            false,
        );
        health.succeeded();
        assert!(!health.can_request(499) && health.can_request(500));
        health.manual_retry();
        assert!(!health.can_request(499), "a refresh does not either");
    }

    #[test]
    fn a_commit_the_host_does_not_know_is_absent_not_failed() {
        let now = Instant::now();
        for error in [ApiError::NotFound, ApiError::Status(422)] {
            let mut cache = CiCache::default();
            cache.mark_inflight("s");
            cache.store("s", Err(error), now);
            assert!(matches!(cache.get("s"), Some(CiEntry::Absent { .. })));
            assert!(cache.checks("s").is_none());
            assert!(!cache.is_inflight("s"));
            assert!(!cache.needs_fetch("s", now + Duration::from_secs(299)));
            assert!(cache.needs_fetch("s", now + Duration::from_secs(300)));
        }
        // It never reaches the history as a dot.
        let mut state = HostingState::default();
        state
            .ci
            .store(&oid(1).to_string(), Err(ApiError::NotFound), now);
        assert!(state.view().ci.is_empty());
    }

    #[test]
    fn offline_is_neutral_without_a_previous_answer_and_keeps_the_previous_one() {
        let now = Instant::now();
        let mut cache = CiCache::default();
        cache.store("n", Err(ApiError::Unreachable("dns".into())), now);
        assert!(matches!(cache.get("n"), Some(CiEntry::Offline { .. })));
        assert!(cache.checks("n").is_none());
        assert!(!cache.needs_fetch("n", now + Duration::from_secs(29)));
        assert!(cache.needs_fetch("n", now + Duration::from_secs(30)));
        cache.store("n", Err(ApiError::TlsUntrusted("x".into())), now);
        assert!(matches!(cache.get("n"), Some(CiEntry::Offline { .. })));
        cache.store("k", Ok(checks(CiState::Running, &[CiState::Running])), now);
        cache.store("k", Err(ApiError::Unreachable("dns".into())), now);
        assert_eq!(cache.checks("k").unwrap().overall, CiState::Running);
        cache.store("f", Err(ApiError::Status(500)), now);
        assert!(matches!(cache.get("f"), Some(CiEntry::Failed { .. })));
    }

    #[test]
    fn pr_heads_are_remote_tracking_tips_not_local_tips() {
        let refs = [
            branch("a", Some("origin/a"), 9),
            remote_branch("origin/a", 3),
            branch("unpushed", Some("origin/gone"), 8),
        ];
        let mut state = state_with(&refs, None);
        state.finish_prs(
            Ok(HashMap::from([
                ("a".to_owned(), pr(7, "a", PrState::Open)),
                ("gone".to_owned(), pr(8, "gone", PrState::Open)),
            ])),
            0,
        );
        // The local tip (9) is ahead of the host; only the remote tip counts.
        assert_eq!(state.pr_tips(), vec![oid(3)]);
        assert_eq!(state.tip_of_pr(7), Some(oid(3)));
        assert_eq!(state.tip_of_pr(8), None);
    }

    #[test]
    fn head_and_selection_get_checks_only_when_the_host_has_them() {
        let refs = [remote_branch("origin/main", 5)];
        let mut state = state_with(&refs, None);
        assert_eq!(state.remote_tips(), [oid(5)]);
        // Nothing known to be on the host yet except the tips.
        assert!(state.ci_candidates(Some(oid(1)), Some(oid(2))).is_empty());
        state.set_reach(HashSet::from([oid(5), oid(2)]), 3);
        assert!(!state.reach_outdated(3) && state.reach_outdated(4));
        // HEAD (2) is pushed, the selected commit (1) is not.
        assert_eq!(
            state.ci_candidates(Some(oid(1)), Some(oid(2))),
            vec![oid(2)]
        );
    }

    #[test]
    fn the_primary_remotes_tips_and_what_they_reach() {
        let refs = [
            remote_branch("origin/main", 5),
            remote_branch("origin/dev", 5),
            remote_branch("origin/HEAD", 5),
            remote_branch("fork/x", 6),
            branch("local", None, 7),
        ];
        assert_eq!(primary_remote_tips(&refs, "origin"), vec![oid(5)]);
        // 5 -> 4 -> 3, and 7 -> 4; commit 2 is not loaded.
        let parents = |o: Oid| {
            if o == oid(5) {
                Some(vec![oid(4)])
            } else if o == oid(4) {
                Some(vec![oid(3)])
            } else if o == oid(3) {
                Some(vec![oid(2)])
            } else {
                None
            }
        };
        let reach = reachable(&[oid(5)], parents);
        assert!(reach.contains(&oid(5)) && reach.contains(&oid(3)) && reach.contains(&oid(2)));
        assert!(!reach.contains(&oid(7)));
    }

    #[test]
    fn pending_checks_on_screen_keep_the_recheck_timer_alive() {
        let refs = [
            branch("a", Some("origin/a"), 2),
            remote_branch("origin/a", 2),
        ];
        let mut state = state_with(&refs, None);
        state.finish_prs(
            Ok(HashMap::from([("a".to_owned(), pr(7, "a", PrState::Open))])),
            0,
        );
        let now = Instant::now();
        assert!(!state.has_pending_ci(None, None));
        state.ci.store(
            &oid(2).to_string(),
            Ok(checks(CiState::Running, &[CiState::Running])),
            now,
        );
        assert!(state.has_pending_ci(None, None));
        state.ci.store(
            &oid(2).to_string(),
            Ok(checks(CiState::Passed, &[CiState::Passed])),
            now,
        );
        assert!(!state.has_pending_ci(None, None));
    }

    #[test]
    fn durations_read_like_the_design() {
        assert_eq!(format_duration(None), "—");
        assert_eq!(format_duration(Some(24)), "24s");
        assert_eq!(format_duration(Some(192)), "3m 12s");
        assert_eq!(format_duration(Some(3_720)), "1h 02m");
    }
}
