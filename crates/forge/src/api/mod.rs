//! Blocking HTTP access to hosting services: instance probe and token check.
//!
//! Every function blocks; callers run them off the UI thread. No message or
//! `Debug` output of this module ever contains a token.

use crate::{known_kind_for_host, Instance, ServiceKind};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use std::time::Duration;
use ureq::config::Config;
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

/// Profile endpoint of Azure DevOps (a single global service, not per organization).
const AZURE_PROFILE_URL: &str =
    "https://app.vssps.visualstudio.com/_apis/profile/profiles/me?api-version=7.1";

/// Scopes GitWarp needs per service, reported as `missing` by [`current_user`].
const GITHUB_SCOPES: [&str; 3] = ["repo", "read:user", "workflow"];
const GITLAB_SCOPES: [&str; 3] = ["api", "read_repository", "write_repository"];

/// Why a request failed, with text fit to show in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401: the token is wrong, expired or revoked.
    Unauthorized,
    /// 403 without a rate-limit signal: the token lacks permission.
    Forbidden,
    NotFound,
    /// Rate limit hit; `reset_at` is a Unix timestamp when the service sent one.
    RateLimited {
        reset_at: Option<u64>,
    },
    /// DNS, connection or timeout failure.
    Unreachable(String),
    /// The server certificate was not accepted (self-signed, unknown CA, ...).
    TlsUntrusted(String),
    /// Any other unexpected HTTP status.
    Status(u16),
    /// The response body was not what the service documents.
    Decode(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => f.write_str("The service rejected the token (unauthorized)"),
            ApiError::Forbidden => f.write_str("The token is not allowed to do this (forbidden)"),
            ApiError::NotFound => f.write_str("The service does not know this address (not found)"),
            ApiError::RateLimited { reset_at: None } => f.write_str("Rate limit reached"),
            ApiError::RateLimited { reset_at: Some(t) } => {
                write!(f, "Rate limit reached (resets at Unix time {t})")
            }
            ApiError::Unreachable(why) => write!(f, "Cannot reach the service: {why}"),
            ApiError::TlsUntrusted(why) => {
                write!(f, "The server certificate is not trusted: {why}")
            }
            ApiError::Status(code) => {
                write!(f, "Unexpected response from the service (HTTP {code})")
            }
            ApiError::Decode(why) => write!(f, "Unexpected answer from the service: {why}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// Result of [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// `None` when nothing at the address looks like a supported service.
    pub kind: Option<ServiceKind>,
    pub version: Option<String>,
}

/// How a GitLab token is sent: personal access tokens use `PRIVATE-TOKEN`,
/// OAuth tokens use `Authorization: Bearer`. Ignored for other services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Pat,
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Outcome of [`current_user`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCheck {
    pub user: User,
    /// `None` when the service does not expose scopes (fine-grained GitHub
    /// tokens, Gitea, Bitbucket, Azure, GitLab OAuth tokens).
    pub scopes: Option<Vec<String>>,
    /// Required scopes the token lacks; empty when `scopes` is `None`.
    pub missing: Vec<String>,
}

/// One configured HTTP agent. Cheap to clone; clones share connections.
#[derive(Clone)]
pub struct Client {
    agent: Agent,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Client")
    }
}

impl Client {
    /// Uses the OS certificate store; plain `http://` instances work too.
    pub fn new() -> Client {
        let config = Config::builder()
            // 4xx / 5xx come back as responses so they can be mapped to ApiError.
            .http_status_as_error(false)
            // Never follow redirects: ureq would resend the Authorization
            // header to wherever the server points, and a 3xx is not an answer
            // we can use anyway.
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .timeout_connect(Some(Duration::from_secs(5)))
            .user_agent(concat!("GitWarp/", env!("CARGO_PKG_VERSION")))
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Client {
            agent: config.into(),
        }
    }

    /// Test constructor: same agent (plain http to `127.0.0.1` needs no TLS).
    #[cfg(test)]
    pub(crate) fn for_tests() -> Client {
        Client::new()
    }

    fn get(&self, url: &str, headers: &[(&str, String)]) -> Result<Response<Body>, ApiError> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        request.call().map_err(map_transport)
    }

    /// GET that maps non-2xx statuses to `ApiError` and decodes the JSON body.
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<(T, Response<Body>), ApiError> {
        let mut response = check_status(self.get(url, headers)?)?;
        let value = response
            .body_mut()
            .read_json::<T>()
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok((value, response))
    }
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

fn map_transport(error: ureq::Error) -> ApiError {
    let text = error.to_string();
    match &error {
        ureq::Error::Rustls(_) | ureq::Error::Tls(_) | ureq::Error::Pem(_) => {
            ApiError::TlsUntrusted(text)
        }
        // A rejected certificate can also surface as an io error wrapping the rustls error.
        ureq::Error::Io(e) if e.to_string().to_ascii_lowercase().contains("certificate") => {
            ApiError::TlsUntrusted(text)
        }
        _ => ApiError::Unreachable(text),
    }
}

fn header_str<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

fn check_status(response: Response<Body>) -> Result<Response<Body>, ApiError> {
    let status = response.status().as_u16();
    let reset_at =
        || header_str(&response, "x-ratelimit-reset").and_then(|v| v.trim().parse().ok());
    match status {
        200..=299 => Ok(response),
        401 => Err(ApiError::Unauthorized),
        403 if header_str(&response, "x-ratelimit-remaining").map(str::trim) == Some("0") => {
            Err(ApiError::RateLimited {
                reset_at: reset_at(),
            })
        }
        403 => Err(ApiError::Forbidden),
        404 => Err(ApiError::NotFound),
        429 => Err(ApiError::RateLimited {
            reset_at: reset_at(),
        }),
        other => Err(ApiError::Status(other)),
    }
}

/// Status plus JSON body (if it parses) of an unauthenticated probe request.
fn probe_get(client: &Client, url: &str) -> Result<(u16, Option<Value>), ApiError> {
    let mut response = client.get(url, &[("Accept", "application/json".to_string())])?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_json::<Value>().ok();
    Ok((status, body))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// Works out what runs at `base_url`: known public hosts are answered without
/// a request, anything else is asked in turn for the version endpoint of
/// GitLab, Gitea / Forgejo, GitHub Enterprise and Bitbucket Server.
pub fn probe(client: &Client, base_url: &str) -> Result<Probe, ApiError> {
    // The kind passed to `new` only drives normalization here.
    let instance = Instance::new(ServiceKind::GitLab, base_url)
        .map_err(|e| ApiError::Unreachable(format!("invalid address: {e}")))?;
    if let Some(kind) = known_kind_for_host(instance.host()) {
        return Ok(Probe {
            kind: Some(kind),
            version: None,
        });
    }
    let base = &instance.base_url;
    let found = |kind, version| {
        Ok(Probe {
            kind: Some(kind),
            version,
        })
    };

    // GitLab answers 401 with a JSON body on this endpoint when it needs a token.
    let (status, body) = probe_get(client, &format!("{base}/api/v4/version"))?;
    match (status, &body) {
        (200, Some(b)) => return found(ServiceKind::GitLab, string_field(b, "version")),
        (401, Some(b)) if b.is_object() => return found(ServiceKind::GitLab, None),
        _ => {}
    }
    let (status, body) = probe_get(client, &format!("{base}/api/v1/version"))?;
    if let (200, Some(b)) = (status, &body) {
        if let Some(version) = string_field(b, "version") {
            return found(ServiceKind::Gitea, Some(version));
        }
    }
    let (status, body) = probe_get(client, &format!("{base}/api/v3/meta"))?;
    if let (200, Some(b)) = (status, &body) {
        if b.is_object() {
            return found(ServiceKind::GitHub, string_field(b, "installed_version"));
        }
    }
    let (status, body) = probe_get(
        client,
        &format!("{base}/rest/api/1.0/application-properties"),
    )?;
    if let (200, Some(b)) = (status, &body) {
        if let Some(version) = string_field(b, "version") {
            return found(ServiceKind::Bitbucket, Some(version));
        }
    }
    Ok(Probe {
        kind: None,
        version: None,
    })
}

/// Asks the service who owns `token` and which of the scopes GitWarp needs it
/// lacks. `token_kind` only matters for GitLab.
pub fn current_user(
    client: &Client,
    instance: &Instance,
    token: &str,
    token_kind: TokenKind,
) -> Result<TokenCheck, ApiError> {
    let api = instance.api_base();
    match instance.kind {
        ServiceKind::GitHub => github_user(client, &api, token),
        ServiceKind::GitLab => gitlab_user(client, &api, token, token_kind),
        ServiceKind::Gitea => gitea_user(client, &api, token),
        ServiceKind::Bitbucket => bitbucket_user(client, &api, token),
        ServiceKind::AzureDevOps => azure_user(client, AZURE_PROFILE_URL, token),
    }
}

fn no_scopes(user: User) -> TokenCheck {
    TokenCheck {
        user,
        scopes: None,
        missing: Vec::new(),
    }
}

fn missing_scopes(
    required: &[&str],
    granted: &[String],
    implied_by: &[(&str, &[&str])],
) -> Vec<String> {
    let has = |scope: &str| {
        granted.iter().any(|g| g == scope)
            || implied_by
                .iter()
                .any(|(big, covers)| covers.contains(&scope) && granted.iter().any(|g| g == big))
    };
    required
        .iter()
        .filter(|s| !has(s))
        .map(|s| s.to_string())
        .collect()
}

fn github_user(client: &Client, api: &str, token: &str) -> Result<TokenCheck, ApiError> {
    #[derive(Deserialize)]
    struct Me {
        login: String,
        name: Option<String>,
        avatar_url: Option<String>,
    }
    let headers = [
        ("Authorization", format!("Bearer {token}")),
        ("Accept", "application/vnd.github+json".to_string()),
    ];
    let (me, response) = client.get_json::<Me>(&format!("{api}/user"), &headers)?;
    let user = User {
        login: me.login,
        name: me.name,
        avatar_url: me.avatar_url,
    };
    // No header at all means a fine-grained token: scopes are not applicable.
    let Some(header) = header_str(&response, "x-oauth-scopes") else {
        return Ok(no_scopes(user));
    };
    let scopes: Vec<String> = header
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // The `user` scope includes `read:user`.
    let missing = missing_scopes(&GITHUB_SCOPES, &scopes, &[("user", &["read:user"])]);
    Ok(TokenCheck {
        user,
        scopes: Some(scopes),
        missing,
    })
}

fn gitlab_user(
    client: &Client,
    api: &str,
    token: &str,
    token_kind: TokenKind,
) -> Result<TokenCheck, ApiError> {
    #[derive(Deserialize)]
    struct Me {
        username: String,
        name: Option<String>,
        avatar_url: Option<String>,
    }
    #[derive(Deserialize)]
    struct Pat {
        scopes: Vec<String>,
    }
    let auth = match token_kind {
        TokenKind::Pat => ("PRIVATE-TOKEN", token.to_string()),
        TokenKind::OAuth => ("Authorization", format!("Bearer {token}")),
    };
    let headers = [auth];
    let (me, _) = client.get_json::<Me>(&format!("{api}/user"), &headers)?;
    let user = User {
        login: me.username,
        name: me.name,
        avatar_url: me.avatar_url,
    };
    // Best effort: OAuth tokens and older servers have no such endpoint.
    let scopes = client
        .get_json::<Pat>(&format!("{api}/personal_access_tokens/self"), &headers)
        .ok()
        .map(|(pat, _)| pat.scopes);
    let Some(scopes) = scopes else {
        return Ok(no_scopes(user));
    };
    // `api` grants full read / write access, repository included.
    let missing = missing_scopes(
        &GITLAB_SCOPES,
        &scopes,
        &[("api", &["read_repository", "write_repository"])],
    );
    Ok(TokenCheck {
        user,
        scopes: Some(scopes),
        missing,
    })
}

fn gitea_user(client: &Client, api: &str, token: &str) -> Result<TokenCheck, ApiError> {
    #[derive(Deserialize)]
    struct Me {
        login: String,
        full_name: Option<String>,
        avatar_url: Option<String>,
    }
    let headers = [("Authorization", format!("token {token}"))];
    let (me, _) = client.get_json::<Me>(&format!("{api}/user"), &headers)?;
    Ok(no_scopes(User {
        login: me.login,
        name: me.full_name.filter(|n| !n.is_empty()),
        avatar_url: me.avatar_url,
    }))
}

fn bitbucket_user(client: &Client, api: &str, token: &str) -> Result<TokenCheck, ApiError> {
    let headers = [("Authorization", format!("Bearer {token}"))];
    let (me, _) = client.get_json::<Value>(&format!("{api}/user"), &headers)?;
    // `username` was removed from the Cloud API; `nickname` replaces it.
    let login = string_field(&me, "nickname")
        .or_else(|| string_field(&me, "username"))
        .or_else(|| string_field(&me, "account_id"))
        .ok_or_else(|| ApiError::Decode("no user name in the response".into()))?;
    let avatar_url = me
        .pointer("/links/avatar/href")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(no_scopes(User {
        login,
        name: string_field(&me, "display_name"),
        avatar_url,
    }))
}

fn azure_user(client: &Client, url: &str, token: &str) -> Result<TokenCheck, ApiError> {
    let headers = [(
        "Authorization",
        format!("Basic {}", base64(format!(":{token}").as_bytes())),
    )];
    let (me, _) = client.get_json::<Value>(url, &headers)?;
    let name = string_field(&me, "displayName");
    let login = string_field(&me, "emailAddress")
        .or_else(|| name.clone())
        .ok_or_else(|| ApiError::Decode("no user name in the response".into()))?;
    Ok(no_scopes(User {
        login,
        name,
        avatar_url: None,
    }))
}

/// Standard base64 with padding (RFC 4648), for the Azure Basic header.
pub(crate) fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
