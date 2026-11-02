//! The repositories an account can see, for the Clone dialog's "From account" tab.
//!
//! Endpoints (checked against the vendors' documentation; the page names the
//! parameter or field unless noted):
//!
//! * GitHub `GET /user/repos` with `affiliation`, `sort`, `per_page` (max 100)
//!   and `Link: <…>; rel="next"` paging —
//!   <https://docs.github.com/en/rest/repos/repos#list-repositories-for-the-authenticated-user>,
//!   <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api>
//! * GitLab `GET /projects` with `membership`, `order_by=last_activity_at`,
//!   `per_page` (max 100) and the `X-Next-Page` header —
//!   <https://docs.gitlab.com/api/projects/#list-all-projects>,
//!   <https://docs.gitlab.com/api/rest/#pagination>. The projects list does
//!   not document `forked_from_project`; it is read when present.
//! * Gitea / Forgejo `GET /user/repos` with `page` / `limit` and a `Link`
//!   header — <https://docs.gitea.com/api/> (`userCurrentListRepos`; the
//!   rendered page was not readable here, parameters follow the Gitea
//!   Swagger convention shared by all list endpoints).
//! * Bitbucket Cloud `GET /2.0/repositories?role=member` with `pagelen`
//!   (max 100) and the `next` URL of the page object —
//!   <https://developer.atlassian.com/cloud/bitbucket/rest/api-group-repositories/>,
//!   <https://developer.atlassian.com/cloud/bitbucket/rest/intro/#pagination>.
//!   Bitbucket Cloud has no archived flag. Bitbucket Server (self-hosted) has
//!   a different API and is [`ApiError::Unsupported`].
//! * Azure DevOps lists repositories per project, not per account:
//!   [`ApiError::Unsupported`].
//!
//! A server-supplied "next page" URL is only followed when it is on the same
//! origin as the API base, so the token is never sent to another host.

use super::net::{parse_rfc3339, Auth};
use super::{header_str, string_field, ApiError, Client, TokenKind};
use crate::{Instance, ServiceKind};
use serde_json::Value;

/// At most this many repositories are returned; the rest is dropped (the
/// services sort by recent activity where they can, so the newest stay).
pub const MAX_REPOS: usize = 500;

/// A guard against a server that keeps answering with a next page.
const MAX_PAGES: usize = 30;

/// One repository of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    /// User, organization or group path (`acme`, `group/sub`).
    pub owner: String,
    pub name: String,
    pub description: Option<String>,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    /// Unix seconds of the last update or activity; 0 when unknown.
    pub updated_at: u64,
    pub clone_https: String,
}

/// The repositories of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoListing {
    /// Most recently updated first, at most [`MAX_REPOS`].
    pub repos: Vec<RepoSummary>,
    /// More repositories exist than were returned.
    pub truncated: bool,
}

/// Lists the repositories `token` can see on `instance`, all pages up to
/// [`MAX_REPOS`], most recently updated first. The token is sent like the
/// other account calls send it (`Auth`, personal-access-token style).
pub fn list_user_repos(
    client: &Client,
    instance: &Instance,
    token: &str,
) -> Result<RepoListing, ApiError> {
    match instance.kind {
        ServiceKind::AzureDevOps => Err(ApiError::Unsupported),
        ServiceKind::Bitbucket if !instance.is_public() => Err(ApiError::Unsupported),
        kind => list_repos_at(client, kind, &instance.api_base(), token),
    }
}

pub(super) fn list_repos_at(
    client: &Client,
    kind: ServiceKind,
    api: &str,
    token: &str,
) -> Result<RepoListing, ApiError> {
    let headers = Auth::new(Some(token), TokenKind::Pat).headers(kind);
    let first = match kind {
        ServiceKind::GitHub => format!(
            "{api}/user/repos?per_page=100&sort=updated&affiliation=owner,collaborator,organization_member"
        ),
        ServiceKind::GitLab => {
            format!("{api}/projects?membership=true&order_by=last_activity_at&per_page=100")
        }
        ServiceKind::Gitea => format!("{api}/user/repos?limit=50&page=1"),
        ServiceKind::Bitbucket => {
            format!("{api}/repositories?role=member&pagelen=100&sort=-updated_on")
        }
        ServiceKind::AzureDevOps => return Err(ApiError::Unsupported),
    };
    let origin = origin_of(api);

    let mut repos = Vec::new();
    let mut truncated = false;
    let mut url = first;
    for _ in 0..MAX_PAGES {
        let (body, response) = client.get_json::<Value>(&url, &headers)?;
        let items = match kind {
            ServiceKind::Bitbucket => body.get("values"),
            _ => Some(&body),
        }
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::Decode("expected a list of repositories".into()))?;
        let before = repos.len();
        repos.extend(items.iter().filter_map(|item| parse_item(kind, item)));
        let next = match kind {
            ServiceKind::GitHub | ServiceKind::Gitea => {
                header_str(&response, "link").and_then(next_link)
            }
            ServiceKind::GitLab => header_str(&response, "x-next-page")
                .map(str::trim)
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .map(|page| format!("{}&page={page}", first_without_page(&url))),
            ServiceKind::Bitbucket => string_field(&body, "next"),
            ServiceKind::AzureDevOps => None,
        };
        let Some(next) = next else { break };
        // Never send the token to another origin, and stop on an empty page.
        if !same_origin(&origin, &next) || items.is_empty() && repos.len() == before {
            break;
        }
        if repos.len() >= MAX_REPOS {
            truncated = true;
            break;
        }
        url = next;
    }
    truncated |= repos.len() > MAX_REPOS;
    repos.truncate(MAX_REPOS);
    // Stable: equal times keep the service's order.
    repos.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    Ok(RepoListing { repos, truncated })
}

/// `scheme://host[:port]` of an API base.
fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            format!("{scheme}://{authority}")
        }
        None => url.to_string(),
    }
}

fn same_origin(origin: &str, url: &str) -> bool {
    url.strip_prefix(origin)
        .is_some_and(|rest| rest.starts_with(['/', '?']))
}

/// A GitLab page URL without its `page` parameter, so the next one can be appended.
fn first_without_page(url: &str) -> String {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("page="))
        .collect();
    format!("{path}?{}", kept.join("&"))
}

/// The `rel="next"` target of an RFC 8288 `Link` header.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        let is_next = params
            .split(';')
            .any(|p| p.trim().replace(' ', "") == "rel=\"next\"");
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        is_next.then(|| target.to_string())
    })
}

fn str_of<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer)?.as_str()
}

fn bool_of(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn description(value: &Value) -> Option<String> {
    value
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// One repository document; `None` when it lacks a name, owner or clone URL.
fn parse_item(kind: ServiceKind, item: &Value) -> Option<RepoSummary> {
    match kind {
        ServiceKind::GitHub | ServiceKind::Gitea => Some(RepoSummary {
            owner: str_of(item, "/owner/login")?.to_string(),
            name: str_of(item, "/name")?.to_string(),
            description: description(item),
            private: bool_of(item, "private"),
            fork: bool_of(item, "fork"),
            archived: bool_of(item, "archived"),
            updated_at: str_of(item, "/updated_at")
                .and_then(parse_rfc3339)
                .unwrap_or(0),
            clone_https: str_of(item, "/clone_url")?.to_string(),
        }),
        ServiceKind::GitLab => Some(RepoSummary {
            owner: str_of(item, "/namespace/full_path")?.to_string(),
            name: str_of(item, "/path")?.to_string(),
            description: description(item),
            // "internal" is visible to every signed-in user: not private.
            private: str_of(item, "/visibility") == Some("private"),
            fork: item
                .get("forked_from_project")
                .is_some_and(|v| !v.is_null()),
            archived: bool_of(item, "archived"),
            updated_at: str_of(item, "/last_activity_at")
                .and_then(parse_rfc3339)
                .unwrap_or(0),
            clone_https: str_of(item, "/http_url_to_repo")?.to_string(),
        }),
        ServiceKind::Bitbucket => {
            let (owner, name) = str_of(item, "/full_name")?.split_once('/')?;
            let clone = item
                .pointer("/links/clone")?
                .as_array()?
                .iter()
                .find(|c| c.get("name").and_then(Value::as_str) == Some("https"))
                .and_then(|c| c.get("href")?.as_str())?;
            Some(RepoSummary {
                owner: owner.to_string(),
                name: name.to_string(),
                description: description(item),
                private: bool_of(item, "is_private"),
                fork: item.get("parent").is_some_and(|v| !v.is_null()),
                archived: false,
                updated_at: str_of(item, "/updated_on")
                    .and_then(parse_rfc3339)
                    .unwrap_or(0),
                clone_https: strip_userinfo(clone),
            })
        }
        ServiceKind::AzureDevOps => None,
    }
}

/// Bitbucket puts the user name into the clone URL (`https://user@host/…`);
/// the account supplies it, so the address is stored without it.
fn strip_userinfo(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            format!("{scheme}://{host}{tail}")
        }
        None => url.to_string(),
    }
}
