//! Pull / merge request reads: listing, the newest request per branch and the
//! review state. GitHub, GitLab and Gitea / Forgejo are implemented; Bitbucket
//! and Azure DevOps answer [`ApiError::Unsupported`].
//!
//! Endpoints (REST only):
//! - GitHub `GET /repos/{owner}/{repo}/pulls`
//!   <https://docs.github.com/en/rest/pulls/pulls#list-pull-requests>,
//!   `GET /repos/{owner}/{repo}/pulls/{n}/reviews`
//!   <https://docs.github.com/en/rest/pulls/reviews#list-reviews-for-a-pull-request>
//! - GitLab `GET /projects/:id/merge_requests`
//!   <https://docs.gitlab.com/api/merge_requests/#list-project-merge-requests>,
//!   `GET /projects/:id/merge_requests/:iid/approvals`
//!   <https://docs.gitlab.com/api/merge_request_approvals/>
//! - Gitea / Forgejo `GET /repos/{owner}/{repo}/pulls`, `…/pulls/{index}/reviews`
//!   (swagger: `https://<host>/swagger.v1.json`, e.g. <https://codeberg.org/api/swagger>)

use super::net::{full_name, parse_rfc3339, q, repo_url, Auth};
use super::{ApiError, Client};
use crate::{Instance, RepoCoords, ServiceKind};
use serde_json::Value;
use std::collections::HashMap;

/// Lifecycle of a pull / merge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// Review outcome. Listing endpoints do not carry it: lists return
/// [`ReviewState::None`]; ask [`pull_request_review`] for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// GitHub / Gitea number, GitLab `iid`.
    pub number: u64,
    pub title: String,
    pub state: PrState,
    pub review: ReviewState,
    pub author: String,
    pub head_branch: String,
    /// `owner/repo` the head branch lives in (fork support). `None` when the
    /// service does not say: a GitLab merge request from another project, or
    /// a deleted fork.
    pub head_repo_full_name: Option<String>,
    pub base_branch: String,
    /// Unix seconds.
    pub updated_at: u64,
    pub web_url: String,
}

/// Whose requests to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrFilter {
    All,
    Mine { login: String },
    ReviewRequested { login: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrStateFilter {
    Open,
    All,
}

/// One page of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullPage {
    pub items: Vec<PullRequest>,
    pub has_next: bool,
}

const PAGE_SIZE: u32 = 50;

/// Lists pull / merge requests of a repository, newest update first.
/// `page` is 1-based. GitLab filters by author / reviewer on the server;
/// GitHub and Gitea have no such parameter on their list endpoint, so `Mine`
/// and `ReviewRequested` filter the fetched page on the client (a page may
/// therefore hold fewer than 50 items while `has_next` is true).
pub fn list_pull_requests(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    auth: &Auth<'_>,
    filter: &PrFilter,
    state: PrStateFilter,
    page: u32,
) -> Result<PullPage, ApiError> {
    let url = list_url(instance, coords, state, filter, page, PAGE_SIZE)?;
    let (mut raw, has_next) =
        client.get_json_cached::<Vec<Value>>(&url, &auth.headers(instance.kind))?;
    if instance.kind != ServiceKind::GitLab {
        retain_matching(&mut raw, filter);
    }
    Ok(PullPage {
        items: parse_list(instance.kind, coords, &raw),
        has_next,
    })
}

pub(crate) fn list_url(
    instance: &Instance,
    coords: &RepoCoords,
    state: PrStateFilter,
    filter: &PrFilter,
    page: u32,
    per_page: u32,
) -> Result<String, ApiError> {
    let repo = repo_url(instance, coords)?;
    let all = state == PrStateFilter::All;
    Ok(match instance.kind {
        ServiceKind::GitHub => format!(
            "{repo}/pulls?state={}&sort=updated&direction=desc&per_page={per_page}&page={page}",
            if all { "all" } else { "open" }
        ),
        ServiceKind::GitLab => {
            let mut url = format!(
                "{repo}/merge_requests?state={}&order_by=updated_at&sort=desc&per_page={per_page}&page={page}",
                if all { "all" } else { "opened" }
            );
            match filter {
                PrFilter::All => {}
                PrFilter::Mine { login } => {
                    url.push_str(&format!("&author_username={}", q(login)));
                }
                PrFilter::ReviewRequested { login } => {
                    url.push_str(&format!("&reviewer_username={}", q(login)));
                }
            }
            url
        }
        ServiceKind::Gitea => format!(
            "{repo}/pulls?state={}&sort=recentupdate&limit={per_page}&page={page}",
            if all { "all" } else { "open" }
        ),
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => return Err(ApiError::Unsupported),
    })
}

/// Client-side author / requested-reviewer filter (GitHub, Gitea JSON shape).
fn retain_matching(items: &mut Vec<Value>, filter: &PrFilter) {
    match filter {
        PrFilter::All => {}
        PrFilter::Mine { login } => {
            items.retain(|v| text(v, "/user/login").is_some_and(|a| a.eq_ignore_ascii_case(login)))
        }
        PrFilter::ReviewRequested { login } => items.retain(|v| {
            v.get("requested_reviewers")
                .and_then(Value::as_array)
                .is_some_and(|rs| {
                    rs.iter()
                        .any(|r| text(r, "/login").is_some_and(|l| l.eq_ignore_ascii_case(login)))
                })
        }),
    }
}

/// For each of `branches`, the newest pull request whose head is that branch
/// of this repository (forks excluded): an open / draft one first, otherwise
/// the most recently updated merged or closed one. Branches without a request
/// are absent from the map.
///
/// Requests: GitHub and Gitea one list call (the 100 / 50 most recently
/// updated requests, all states, filtered on the client; older ones are not
/// seen), GitLab one `merge_requests?source_branch=` call per branch.
pub fn pull_requests_for_branches(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    auth: &Auth<'_>,
    branches: &[String],
) -> Result<HashMap<String, PullRequest>, ApiError> {
    let repo = repo_url(instance, coords)?;
    let headers = auth.headers(instance.kind);
    let mut found: Vec<PullRequest> = Vec::new();
    match instance.kind {
        ServiceKind::GitLab => {
            for branch in branches {
                let url = branch_url(&repo, branch);
                let (raw, _) = client.get_json_cached::<Vec<Value>>(&url, &headers)?;
                found.extend(parse_list(instance.kind, coords, &raw));
            }
        }
        _ => {
            if branches.is_empty() {
                return Ok(HashMap::new());
            }
            let per_page = if instance.kind == ServiceKind::GitHub {
                100
            } else {
                PAGE_SIZE
            };
            let url = list_url(
                instance,
                coords,
                PrStateFilter::All,
                &PrFilter::All,
                1,
                per_page,
            )?;
            let (raw, _) = client.get_json_cached::<Vec<Value>>(&url, &headers)?;
            found = parse_list(instance.kind, coords, &raw);
        }
    }
    let own = full_name(coords);
    let mut best: HashMap<String, PullRequest> = HashMap::new();
    for pr in found {
        let in_this_repo = pr
            .head_repo_full_name
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case(&own));
        if !in_this_repo || !branches.contains(&pr.head_branch) {
            continue;
        }
        match best.get(&pr.head_branch) {
            Some(current) if !is_better(&pr, current) => {}
            _ => {
                best.insert(pr.head_branch.clone(), pr);
            }
        }
    }
    Ok(best)
}

pub(crate) fn branch_url(repo: &str, branch: &str) -> String {
    format!(
        "{repo}/merge_requests?source_branch={}&state=all&order_by=updated_at&sort=desc&per_page=20",
        q(branch)
    )
}

fn is_open(state: PrState) -> bool {
    matches!(state, PrState::Open | PrState::Draft)
}

/// Open beats finished; among equals the later update wins.
fn is_better(candidate: &PullRequest, current: &PullRequest) -> bool {
    (is_open(candidate.state), candidate.updated_at) > (is_open(current.state), current.updated_at)
}

/// Review state of one request: GitHub and Gitea take every reviewer's latest
/// decision (changes requested beats approved); GitLab only knows whether the
/// approval rules are met (`approved`), so it never reports ChangesRequested.
/// GitHub states `APPROVED` / `CHANGES_REQUESTED` / `COMMENTED` / `DISMISSED`
/// are not enumerated on the docs page; they follow the review API payloads.
pub fn pull_request_review(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    auth: &Auth<'_>,
    number: u64,
) -> Result<ReviewState, ApiError> {
    let repo = repo_url(instance, coords)?;
    let headers = auth.headers(instance.kind);
    match instance.kind {
        ServiceKind::GitLab => {
            let url = format!("{repo}/merge_requests/{number}/approvals");
            let (v, _) = client.get_json_cached::<Value>(&url, &headers)?;
            Ok(
                if v.get("approved").and_then(Value::as_bool) == Some(true) {
                    ReviewState::Approved
                } else {
                    ReviewState::None
                },
            )
        }
        ServiceKind::GitHub | ServiceKind::Gitea => {
            let url = if instance.kind == ServiceKind::GitHub {
                format!("{repo}/pulls/{number}/reviews?per_page=100")
            } else {
                format!("{repo}/pulls/{number}/reviews?limit=50")
            };
            let (reviews, _) = client.get_json_cached::<Vec<Value>>(&url, &headers)?;
            Ok(review_from_list(&reviews))
        }
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => Err(ApiError::Unsupported),
    }
}

/// Reviews arrive oldest first; the last decisive review of each user counts.
/// Comment-only, pending and dismissed reviews do not change a decision.
pub(crate) fn review_from_list(reviews: &[Value]) -> ReviewState {
    let mut latest: HashMap<String, ReviewState> = HashMap::new();
    for r in reviews {
        if r.get("dismissed").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let state = match text(r, "/state").as_deref() {
            Some("APPROVED") => ReviewState::Approved,
            Some("CHANGES_REQUESTED" | "REQUEST_CHANGES") => ReviewState::ChangesRequested,
            _ => continue,
        };
        let user = text(r, "/user/login").unwrap_or_default();
        latest.insert(user, state);
    }
    if latest.values().any(|s| *s == ReviewState::ChangesRequested) {
        ReviewState::ChangesRequested
    } else if latest.values().any(|s| *s == ReviewState::Approved) {
        ReviewState::Approved
    } else {
        ReviewState::None
    }
}

// ------------------------------------------------------------------ parsing

pub(crate) fn text(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)?.as_str().map(str::to_string)
}

fn is_true(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool) == Some(true)
}

pub(crate) fn parse_list(
    kind: ServiceKind,
    coords: &RepoCoords,
    raw: &[Value],
) -> Vec<PullRequest> {
    raw.iter()
        .filter_map(|v| match kind {
            ServiceKind::GitHub => parse_github(v),
            ServiceKind::GitLab => parse_gitlab(v, coords),
            ServiceKind::Gitea => parse_gitea(v),
            ServiceKind::Bitbucket | ServiceKind::AzureDevOps => None,
        })
        .collect()
}

/// Parses one request of the service's JSON shape (also the create response).
pub(crate) fn parse_one(
    kind: ServiceKind,
    coords: &RepoCoords,
    v: &Value,
) -> Result<PullRequest, ApiError> {
    parse_list(kind, coords, std::slice::from_ref(v))
        .pop()
        .ok_or_else(|| ApiError::Decode("not a pull request".into()))
}

/// GitHub: `merged_at` set → Merged; `state: closed` → Closed; `draft` → Draft.
pub(crate) fn parse_github(v: &Value) -> Option<PullRequest> {
    let state = if v.get("merged_at").is_some_and(|m| !m.is_null()) {
        PrState::Merged
    } else if text(v, "/state")? == "closed" {
        PrState::Closed
    } else if is_true(v, "/draft") {
        PrState::Draft
    } else {
        PrState::Open
    };
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        title: text(v, "/title")?,
        state,
        review: ReviewState::None,
        author: text(v, "/user/login").unwrap_or_default(),
        head_branch: text(v, "/head/ref")?,
        head_repo_full_name: text(v, "/head/repo/full_name"),
        base_branch: text(v, "/base/ref")?,
        updated_at: text(v, "/updated_at")
            .and_then(|t| parse_rfc3339(&t))
            .unwrap_or(0),
        web_url: text(v, "/html_url")?,
    })
}

/// GitLab: `merged` → Merged; `closed` / `locked` → Closed; `opened` + `draft`
/// → Draft. The head project's path is not in the answer; it is this repository
/// when `source_project_id == target_project_id`, unknown otherwise.
pub(crate) fn parse_gitlab(v: &Value, coords: &RepoCoords) -> Option<PullRequest> {
    let state = match text(v, "/state")?.as_str() {
        "merged" => PrState::Merged,
        "closed" | "locked" => PrState::Closed,
        _ if is_true(v, "/draft") => PrState::Draft,
        _ => PrState::Open,
    };
    let same_project = v.get("source_project_id")? == v.get("target_project_id")?;
    Some(PullRequest {
        number: v.get("iid")?.as_u64()?,
        title: text(v, "/title")?,
        state,
        review: ReviewState::None,
        author: text(v, "/author/username").unwrap_or_default(),
        head_branch: text(v, "/source_branch")?,
        head_repo_full_name: same_project.then(|| full_name(coords)),
        base_branch: text(v, "/target_branch")?,
        updated_at: text(v, "/updated_at")
            .and_then(|t| parse_rfc3339(&t))
            .unwrap_or(0),
        web_url: text(v, "/web_url")?,
    })
}

/// Gitea / Forgejo: `merged` → Merged; `state: closed` → Closed; `draft` (or a
/// `WIP:` / `[WIP]` title prefix, how older versions mark drafts) → Draft.
pub(crate) fn parse_gitea(v: &Value) -> Option<PullRequest> {
    let title = text(v, "/title")?;
    let wip = {
        let t = title.trim_start().to_ascii_uppercase();
        t.starts_with("WIP:") || t.starts_with("[WIP]")
    };
    let state = if is_true(v, "/merged") {
        PrState::Merged
    } else if text(v, "/state")? == "closed" {
        PrState::Closed
    } else if is_true(v, "/draft") || wip {
        PrState::Draft
    } else {
        PrState::Open
    };
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        title,
        state,
        review: ReviewState::None,
        author: text(v, "/user/login").unwrap_or_default(),
        head_branch: text(v, "/head/ref")?,
        head_repo_full_name: text(v, "/head/repo/full_name"),
        base_branch: text(v, "/base/ref")?,
        updated_at: text(v, "/updated_at")
            .and_then(|t| parse_rfc3339(&t))
            .unwrap_or(0),
        web_url: text(v, "/html_url")?,
    })
}
