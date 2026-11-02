//! Writes against the hosting service: creating a pull / merge request and the
//! ref name to fetch one for checkout.
//!
//! Endpoints:
//! - GitHub create `POST /repos/{owner}/{repo}/pulls` (`title`, `head`, `base`,
//!   `body`, `draft`)
//!   <https://docs.github.com/en/rest/pulls/pulls#create-a-pull-request>;
//!   reviewers `POST /repos/{owner}/{repo}/pulls/{n}/requested_reviewers`
//!   (`reviewers`, 201) <https://docs.github.com/en/rest/pulls/review-requests>.
//!   An existing request answers 422 ("A pull request already exists for …";
//!   the docs only say "Validation failed"), then the open request for the head
//!   is looked up with `GET …/pulls?state=open&head=owner:branch`.
//! - GitLab create `POST /projects/:id/merge_requests` (`source_branch`,
//!   `target_branch`, `title`, `description`, `reviewer_ids`; no draft
//!   parameter: the title gets the `Draft: ` prefix)
//!   <https://docs.gitlab.com/api/merge_requests/#create-mr>; reviewer ids from
//!   `GET /users?username=` <https://docs.gitlab.com/api/users/>. An existing
//!   request answers 409 (status confirmed by third-party clients, not by the
//!   docs), then it is looked up with `GET …/merge_requests?source_branch=`.
//! - Gitea / Forgejo create `POST /repos/{owner}/{repo}/pulls`
//!   (`CreatePullRequestOption`: `head`, `base`, `title`, `body`; no draft
//!   field: title prefix `WIP: `), reviewers
//!   `POST …/pulls/{index}/requested_reviewers` (`reviewers`), 409 when a
//!   request exists (swagger: <https://codeberg.org/swagger.v1.json>).
//!
//! Checkout refs: GitHub / Gitea `refs/pull/N/head`, GitLab
//! `refs/merge-requests/N/head`.

use super::net::{error_message, q, read_json, repo_url, Auth};
use super::pulls::{parse_list, parse_one, text};
use super::{check_status, ApiError, Client};
use crate::{Instance, RepoCoords, ServiceKind};
use serde_json::{json, Value};
use ureq::http::Response;
use ureq::Body;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPullRequest {
    /// Source branch; `user:branch` for a request from a fork (GitHub).
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
    /// User names (logins) to request a review from.
    pub reviewers: Vec<String>,
}

/// The request that was created. Reviewers are best effort: the request
/// exists even when asking for them failed, `reviewers_error` says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPullRequest {
    pub pull_request: super::PullRequest,
    pub reviewers_error: Option<ApiError>,
}

/// The ref that points at the head of request `number`, to fetch for checkout.
/// Bitbucket / Azure: [`ApiError::Unsupported`].
pub fn pull_request_head_ref(kind: ServiceKind, number: u64) -> Result<String, ApiError> {
    match kind {
        ServiceKind::GitHub | ServiceKind::Gitea => Ok(format!("refs/pull/{number}/head")),
        ServiceKind::GitLab => Ok(format!("refs/merge-requests/{number}/head")),
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => Err(ApiError::Unsupported),
    }
}

/// Creates a pull / merge request in `coords`. A token is required. "Already
/// exists" becomes [`ApiError::AlreadyExists`]; any other 400 / 409 / 422
/// becomes [`ApiError::Rejected`] with the service's explanation.
pub fn create_pull_request(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    auth: &Auth<'_>,
    new: &NewPullRequest,
) -> Result<CreatedPullRequest, ApiError> {
    let repo = repo_url(instance, coords)?;
    if !auth.has_token() {
        return Err(ApiError::Unauthorized);
    }
    let kind = instance.kind;
    let headers = auth.headers(kind);
    let mut reviewers_error = None;

    let (url, body) = match kind {
        ServiceKind::GitHub => (
            format!("{repo}/pulls"),
            json!({"title": new.title, "head": new.head, "base": new.base,
                   "body": new.body, "draft": new.draft}),
        ),
        ServiceKind::Gitea => (
            format!("{repo}/pulls"),
            json!({"title": draft_title(new, "WIP: "), "head": new.head, "base": new.base,
                   "body": new.body}),
        ),
        ServiceKind::GitLab => {
            let (ids, error) = gitlab_reviewer_ids(client, instance, auth, &new.reviewers);
            reviewers_error = error;
            (
                format!("{repo}/merge_requests"),
                json!({"title": draft_title(new, "Draft: "), "source_branch": new.head,
                       "target_branch": new.base, "description": new.body,
                       "reviewer_ids": ids}),
            )
        }
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => return Err(ApiError::Unsupported),
    };

    let (status, response) = client.post_json(&url, &headers, &body)?;
    let response = match classify(status, response)? {
        Reply::Created(response) => response,
        Reply::Refused(message) => {
            return Err(match is_duplicate(kind, status, &message) {
                true => find_existing(client, instance, coords, &headers, new)
                    .map(|pr| ApiError::AlreadyExists {
                        number: pr.number,
                        web_url: pr.web_url,
                    })
                    .unwrap_or(ApiError::Rejected(message)),
                false => ApiError::Rejected(message),
            })
        }
    };
    let value: Value = read_json(response)?;
    let pull_request = parse_one(kind, coords, &value)?;

    if kind != ServiceKind::GitLab && !new.reviewers.is_empty() {
        let url = format!("{repo}/pulls/{}/requested_reviewers", pull_request.number);
        let body = json!({ "reviewers": new.reviewers });
        reviewers_error = client
            .post_json(&url, &headers, &body)
            .and_then(|(status, response)| match classify(status, response)? {
                Reply::Created(_) => Ok(()),
                Reply::Refused(message) => Err(ApiError::Rejected(message)),
            })
            .err();
    }
    Ok(CreatedPullRequest {
        pull_request,
        reviewers_error,
    })
}

fn draft_title(new: &NewPullRequest, prefix: &str) -> String {
    let lower = new.title.trim_start().to_ascii_lowercase();
    let marked = ["draft:", "[draft]", "wip:", "[wip]"]
        .iter()
        .any(|p| lower.starts_with(p));
    if new.draft && !marked {
        format!("{prefix}{}", new.title)
    } else {
        new.title.clone()
    }
}

enum Reply {
    Created(Response<Body>),
    /// 400 / 409 / 422 with the service's message.
    Refused(String),
}

fn classify(status: u16, response: Response<Body>) -> Result<Reply, ApiError> {
    match status {
        400 | 409 | 422 => Ok(Reply::Refused(error_message(response))),
        _ => check_status(response).map(Reply::Created),
    }
}

/// GitHub answers an existing request with 422 and a message saying so; GitLab
/// and Gitea with 409.
fn is_duplicate(kind: ServiceKind, status: u16, message: &str) -> bool {
    match kind {
        ServiceKind::GitHub => {
            status == 422 && message.to_ascii_lowercase().contains("already exists")
        }
        _ => status == 409,
    }
}

/// The open request with the same head and base, after a duplicate answer.
fn find_existing(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    headers: &[(&str, String)],
    new: &NewPullRequest,
) -> Option<super::PullRequest> {
    let repo = repo_url(instance, coords).ok()?;
    let branch = new.head.rsplit(':').next().unwrap_or(&new.head);
    let url = match instance.kind {
        ServiceKind::GitHub => {
            let head = if new.head.contains(':') {
                new.head.clone()
            } else {
                format!("{}:{}", coords.namespace.join("/"), new.head)
            };
            format!("{repo}/pulls?state=open&head={}&per_page=5", q(&head))
        }
        ServiceKind::GitLab => format!(
            "{repo}/merge_requests?state=opened&source_branch={}&target_branch={}&per_page=5",
            q(&new.head),
            q(&new.base)
        ),
        _ => format!("{repo}/pulls?state=open&sort=recentupdate&limit=50"),
    };
    let (raw, _) = client.get_json::<Vec<Value>>(&url, headers).ok()?;
    parse_list(instance.kind, coords, &raw)
        .into_iter()
        .find(|pr| pr.head_branch == branch && pr.base_branch == new.base)
}

/// Resolves reviewer user names to GitLab ids; the first failure (unknown
/// user, network) is returned next to the ids that did resolve.
fn gitlab_reviewer_ids(
    client: &Client,
    instance: &Instance,
    auth: &Auth<'_>,
    names: &[String],
) -> (Vec<u64>, Option<ApiError>) {
    let headers = auth.headers(instance.kind);
    let api = instance.api_base();
    let mut ids = Vec::new();
    let mut error = None;
    for name in names {
        let url = format!("{api}/users?username={}", q(name));
        match client.get_json::<Vec<Value>>(&url, &headers) {
            Ok((users, _)) => {
                let id = users
                    .iter()
                    .find(|u| text(u, "/username").is_some_and(|n| n.eq_ignore_ascii_case(name)))
                    .and_then(|u| u.get("id")?.as_u64());
                match id {
                    Some(id) => ids.push(id),
                    None => error = error.or(Some(ApiError::NotFound)),
                }
            }
            Err(e) => error = error.or(Some(e)),
        }
    }
    (ids, error)
}
