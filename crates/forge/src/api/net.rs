//! Shared plumbing of the pull request / CI / create-PR calls: credentials,
//! the conditional-request cache, authenticated GET / POST and small helpers.

use super::{check_status, header_str, ApiError, Client, TokenKind};
use crate::encode::{encode_path, encode_segment};
use crate::{Instance, RepoCoords, ServiceKind};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use ureq::http::Response;
use ureq::Body;

/// Optional credentials of one request. Public repositories can be read
/// without a token; `Debug` never shows it.
#[derive(Clone, Copy)]
pub struct Auth<'a> {
    token: Option<&'a str>,
    kind: TokenKind,
}

impl<'a> Auth<'a> {
    /// No token: only public data is visible.
    pub fn anonymous() -> Auth<'static> {
        Auth {
            token: None,
            kind: TokenKind::Pat,
        }
    }

    /// `kind` only matters for GitLab (PAT vs OAuth header). An empty token is
    /// treated as no token.
    pub fn new(token: Option<&'a str>, kind: TokenKind) -> Auth<'a> {
        Auth {
            token: token.filter(|t| !t.is_empty()),
            kind,
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Request headers for `service`. Per service:
    /// GitHub `Authorization: Bearer`, GitLab `PRIVATE-TOKEN` (PAT) or
    /// `Authorization: Bearer` (OAuth), Gitea / Forgejo `Authorization: token`.
    pub(crate) fn headers(&self, service: ServiceKind) -> Vec<(&'static str, String)> {
        let mut headers = vec![(
            "Accept",
            match service {
                ServiceKind::GitHub => "application/vnd.github+json".to_string(),
                _ => "application/json".to_string(),
            },
        )];
        if service == ServiceKind::GitHub {
            headers.push(("X-GitHub-Api-Version", "2022-11-28".to_string()));
        }
        if let Some(token) = self.token {
            match service {
                ServiceKind::GitLab => match self.kind {
                    TokenKind::Pat => headers.push(("PRIVATE-TOKEN", token.to_string())),
                    TokenKind::OAuth => {
                        headers.push(("Authorization", format!("Bearer {token}")));
                    }
                },
                ServiceKind::Gitea => headers.push(("Authorization", format!("token {token}"))),
                _ => headers.push(("Authorization", format!("Bearer {token}"))),
            }
        }
        headers
    }
}

impl fmt::Debug for Auth<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.token.is_some() {
            "Auth(token)"
        } else {
            "Auth(anonymous)"
        })
    }
}

/// `{api}/repos/{owner}/{repo}` (GitHub, Gitea) or `{api}/projects/{path}` (GitLab).
pub(crate) fn repo_url(instance: &Instance, coords: &RepoCoords) -> Result<String, ApiError> {
    let api = instance.api_base();
    let owner = coords.namespace.join("/");
    match instance.kind {
        ServiceKind::GitHub | ServiceKind::Gitea => Ok(format!(
            "{api}/repos/{}/{}",
            encode_path(&owner),
            encode_segment(&coords.repo)
        )),
        ServiceKind::GitLab => Ok(format!(
            "{api}/projects/{}",
            encode_segment(&format!("{owner}/{}", coords.repo))
        )),
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => Err(ApiError::Unsupported),
    }
}

/// `owner/…/repo`.
pub(crate) fn full_name(coords: &RepoCoords) -> String {
    format!("{}/{}", coords.namespace.join("/"), coords.repo)
}

/// Percent-encodes one query value (same rules as a path segment).
pub(crate) fn q(value: &str) -> String {
    encode_segment(value)
}

// ---------------------------------------------------------------- ETag cache

/// Largest response body kept in the cache.
const MAX_CACHED_BODY: usize = 2 * 1024 * 1024;
/// Entries kept by default.
const DEFAULT_CAPACITY: usize = 256;

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) etag: String,
    pub(crate) body: String,
    pub(crate) has_next: bool,
}

/// In-memory cache of GET responses keyed by URL and credentials, for
/// conditional requests: `If-None-Match` is sent and a 304 reuses the cached
/// body (a 304 does not count against the GitHub rate limit). First in, first
/// out once `capacity` entries are stored. Holds response bodies only; the key
/// carries a hash of the auth headers, never a token.
pub struct ETagCache {
    capacity: usize,
    inner: Mutex<(HashMap<String, Entry>, VecDeque<String>)>,
}

impl ETagCache {
    pub fn new(capacity: usize) -> ETagCache {
        ETagCache {
            capacity: capacity.max(1),
            inner: Mutex::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.lock().0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (HashMap<String, Entry>, VecDeque<String>)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn get(&self, key: &str) -> Option<Entry> {
        self.lock().0.get(key).cloned()
    }

    pub(crate) fn put(&self, key: String, entry: Entry) {
        if entry.body.len() > MAX_CACHED_BODY {
            return;
        }
        let mut guard = self.lock();
        let (map, order) = &mut *guard;
        if map.insert(key.clone(), entry).is_none() {
            order.push_back(key);
            while map.len() > self.capacity {
                match order.pop_front() {
                    Some(old) => map.remove(&old),
                    None => break,
                };
            }
        }
    }
}

impl Default for ETagCache {
    fn default() -> Self {
        ETagCache::new(DEFAULT_CAPACITY)
    }
}

impl fmt::Debug for ETagCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ETagCache({} entries)", self.len())
    }
}

/// Cache key: URL plus a hash of the headers, so two accounts never share a
/// cached answer for a private repository.
pub(crate) fn cache_key(url: &str, headers: &[(&str, String)]) -> String {
    let mut hasher = DefaultHasher::new();
    for (name, value) in headers {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    format!("{url}\n{:016x}", hasher.finish())
}

fn has_next_page(response: &Response<Body>) -> bool {
    // GitHub and Gitea send `Link: <…>; rel="next"`, GitLab `X-Next-Page`.
    header_str(response, "link").is_some_and(|l| l.contains("rel=\"next\""))
        || header_str(response, "x-next-page").is_some_and(|v| !v.trim().is_empty())
}

impl Client {
    /// GET with conditional-request caching. Returns the decoded body and
    /// whether the service announced a next page.
    pub(crate) fn get_json_cached<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<(T, bool), ApiError> {
        let key = cache_key(url, headers);
        let cached = self.etags.get(&key);
        let mut sent = headers.to_vec();
        if let Some(entry) = &cached {
            sent.push(("If-None-Match", entry.etag.clone()));
        }
        let mut response = self.get(url, &sent)?;
        if response.status().as_u16() == 304 {
            let entry = cached.ok_or(ApiError::Status(304))?;
            return decode(&entry.body).map(|v| (v, entry.has_next));
        }
        response = check_status(response)?;
        let has_next = has_next_page(&response);
        let etag = header_str(&response, "etag").map(str::to_string);
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        let value = decode(&body)?;
        if let Some(etag) = etag {
            self.etags.put(
                key,
                Entry {
                    etag,
                    body,
                    has_next,
                },
            );
        }
        Ok((value, has_next))
    }

    /// POST of a JSON body; the response is returned as is (status, text) so
    /// the caller can map 409 / 422 itself.
    pub(crate) fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, Response<Body>), ApiError> {
        let mut request = self.agent.post(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let response = request.send_json(body).map_err(super::map_transport)?;
        Ok((response.status().as_u16(), response))
    }
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|e| ApiError::Decode(e.to_string()))
}

/// Decodes a successful POST response.
pub(crate) fn read_json<T: DeserializeOwned>(mut response: Response<Body>) -> Result<T, ApiError> {
    response
        .body_mut()
        .read_json::<T>()
        .map_err(|e| ApiError::Decode(e.to_string()))
}

/// The service's own explanation of a rejected request (`message`, GitHub's
/// `errors[].message`), shortened. Never contains request data.
pub(crate) fn error_message(mut response: Response<Body>) -> String {
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let mut parts = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        for key in ["message", "error"] {
            match value.get(key) {
                Some(Value::String(s)) => parts.push(s.clone()),
                Some(other) if !other.is_null() => parts.push(other.to_string()),
                _ => {}
            }
        }
        if let Some(errors) = value.get("errors").and_then(Value::as_array) {
            for e in errors {
                if let Some(m) = e.get("message").and_then(Value::as_str) {
                    parts.push(m.to_string());
                }
            }
        }
    }
    let mut message = parts.join(": ");
    if message.is_empty() {
        message = text;
    }
    message.chars().take(300).collect()
}

// ------------------------------------------------------------------ time

/// Parses an RFC 3339 timestamp (`2024-05-01T12:34:56Z`, with optional
/// fraction and `±hh:mm` offset) to Unix seconds.
pub(crate) fn parse_rfc3339(text: &str) -> Option<u64> {
    let (date, time) = text.trim().split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset_secs) = match time.find(['Z', 'z', '+', '-']) {
        Some(i) => {
            let (clock, zone) = time.split_at(i);
            let secs = match zone.as_bytes()[0] {
                b'Z' | b'z' => 0,
                sign => {
                    let (h, m) = zone[1..].split_once(':').unwrap_or((&zone[1..], "0"));
                    let total = h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60;
                    if sign == b'-' {
                        -total
                    } else {
                        total
                    }
                }
            };
            (clock, secs)
        }
        None => (time, 0),
    };
    let mut c = clock.split(':');
    let hour: i64 = c.next()?.parse().ok()?;
    let minute: i64 = c.next()?.parse().ok()?;
    let second: i64 = c.next()?.split('.').next()?.parse().ok()?;
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    u64::try_from(secs).ok()
}

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
