//! Browser sign-in for GitHub: the OAuth device flow and the refresh of the
//! expiring tokens it yields, plus the format in which those tokens are kept in
//! the keychain.
//!
//! Every function blocks; callers run them off the UI thread. Tokens and the
//! device code never appear in `Debug` / `Display` output of anything here;
//! the user code is meant to be shown to the user.

use crate::api::{ApiError, Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Client ID of the GitWarp OAuth App on github.com. It is a public value: the
/// device flow and the token refresh need no client secret, and the app has
/// "Expire user access tokens" switched on (8 h access token, rotating refresh
/// token).
pub const GITHUB_CLIENT_ID: &str = "Ov23linyzeq1tt9vdnJY";

/// Where the GitHub OAuth endpoints live.
pub const GITHUB_OAUTH_URL: &str = "https://github.com";

/// Scopes requested by the device flow. `workflow` lets pushes that touch
/// `.github/workflows` work; GitHub documents it as a regular OAuth scope.
pub const GITHUB_SCOPE: &str = "repo read:user workflow";

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Why a sign-in or refresh request failed. The texts carry no secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// DNS, connection, TLS or timeout failure: nothing was decided.
    Network(String),
    /// The service refused the request with an OAuth error code (for a
    /// refresh: the refresh token is expired, revoked or already used).
    Rejected(String),
    /// Any other unexpected answer (HTTP status, malformed body).
    Unexpected(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Network(why) => write!(f, "Cannot reach the service: {why}"),
            AuthError::Rejected(code) => write!(f, "The service refused the sign-in ({code})"),
            AuthError::Unexpected(why) => write!(f, "Unexpected answer from the service: {why}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<ApiError> for AuthError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::Unreachable(why) | ApiError::TlsUntrusted(why) => AuthError::Network(why),
            other => AuthError::Unexpected(other.to_string()),
        }
    }
}

/// An OAuth access token with its refresh token. Times are Unix seconds.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthTokens {
    pub access: String,
    pub refresh: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish_non_exhaustive()
    }
}

/// The JSON kept in the keychain for a browser sign-in.
#[derive(Serialize, Deserialize)]
struct SecretJson {
    v: u32,
    access: String,
    refresh: String,
    access_exp: u64,
    refresh_exp: u64,
}

/// Version of [`SecretJson`] this build reads and writes.
const SECRET_VERSION: u32 = 1;

impl OAuthTokens {
    /// The keychain secret: `{"v":1,"access":…,"refresh":…,"access_exp":…,"refresh_exp":…}`.
    pub fn to_secret(&self) -> String {
        let json = SecretJson {
            v: SECRET_VERSION,
            access: self.access.clone(),
            refresh: self.refresh.clone(),
            access_exp: self.access_expires_at,
            refresh_exp: self.refresh_expires_at,
        };
        // Plain strings and integers: serialization cannot fail.
        serde_json::to_string(&json).unwrap_or_default()
    }
}

/// What a keychain secret holds.
#[derive(Clone, PartialEq, Eq)]
pub enum StoredSecret {
    /// A personal access token, stored as the raw string.
    Pat(String),
    /// Tokens of a browser sign-in.
    OAuth(OAuthTokens),
    /// Looks like the JSON secret but cannot be read (damaged, or written by a
    /// newer version): the account has to be reconnected.
    Unusable,
}

impl fmt::Debug for StoredSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoredSecret::Pat(_) => f.write_str("StoredSecret::Pat(..)"),
            StoredSecret::OAuth(tokens) => write!(f, "StoredSecret::OAuth({tokens:?})"),
            StoredSecret::Unusable => f.write_str("StoredSecret::Unusable"),
        }
    }
}

/// Reads a keychain secret. Anything that is not a JSON object is a raw
/// personal access token (every secret written before browser sign-in existed
/// is one); no token format starts with `{`.
pub fn parse_secret(secret: &str) -> StoredSecret {
    let trimmed = secret.trim();
    if !trimmed.starts_with('{') {
        return StoredSecret::Pat(secret.to_owned());
    }
    match serde_json::from_str::<SecretJson>(trimmed) {
        Ok(json) if json.v == SECRET_VERSION && !json.access.is_empty() => {
            StoredSecret::OAuth(OAuthTokens {
                access: json.access,
                refresh: json.refresh,
                access_expires_at: json.access_exp,
                refresh_expires_at: json.refresh_exp,
            })
        }
        _ => StoredSecret::Unusable,
    }
}

/// The answer to the device-code request.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// Identifies the request in the polling calls; never shown or logged.
    device_code: String,
    /// The short code the user types on the verification page.
    pub user_code: String,
    /// The page where the user enters the code.
    pub verification_uri: String,
    /// Seconds until the codes expire.
    pub expires_in: u64,
    /// Minimum seconds between two polls.
    pub interval: u64,
}

impl DeviceCode {
    /// The secret half of the code, for [`github_device_poll`].
    pub fn device_code(&self) -> &str {
        &self.device_code
    }
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// Result of one poll of the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollStep {
    /// The user has not approved yet.
    Pending,
    /// Polling too fast: wait 5 seconds longer between polls.
    SlowDown,
    /// The user declined in the browser.
    Denied,
    /// The device code expired.
    Expired,
    Done(OAuthTokens),
}

fn string_of(body: &Value, key: &str) -> Option<String> {
    body.get(key)?.as_str().map(str::to_owned)
}

/// A non-success answer that is not a documented error code.
fn unexpected(status: u16, body: &Value) -> AuthError {
    match body.get("error").and_then(Value::as_str) {
        Some(code) => AuthError::Rejected(code.to_owned()),
        None => AuthError::Unexpected(format!("HTTP {status}")),
    }
}

/// Asks GitHub for a device code. `base` is [`GITHUB_OAUTH_URL`] (a stub in tests).
pub fn github_device_start(client: &Client, base: &str) -> Result<DeviceCode, AuthError> {
    let base = base.trim_end_matches('/');
    let (status, body) = client.post_form(
        &format!("{base}/login/device/code"),
        &[("client_id", GITHUB_CLIENT_ID), ("scope", GITHUB_SCOPE)],
    )?;
    if body.get("error").is_some() || !(200..300).contains(&status) {
        return Err(unexpected(status, &body));
    }
    let malformed = || AuthError::Unexpected("no device code in the answer".into());
    let device_code = string_of(&body, "device_code").ok_or_else(malformed)?;
    let user_code = string_of(&body, "user_code").ok_or_else(malformed)?;
    // The page is opened in the browser: only ever an https address, else the
    // documented one.
    let verification_uri = string_of(&body, "verification_uri")
        .filter(|uri| uri.starts_with("https://"))
        .unwrap_or_else(|| format!("{base}/login/device"));
    Ok(DeviceCode {
        device_code,
        user_code,
        verification_uri,
        expires_in: body
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(900),
        interval: body
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .max(1),
    })
}

/// One poll of the device flow: the answer of the token endpoint, mapped.
pub fn github_device_poll(
    client: &Client,
    base: &str,
    device_code: &str,
) -> Result<PollStep, AuthError> {
    github_device_poll_at(client, base, device_code, now_unix())
}

fn github_device_poll_at(
    client: &Client,
    base: &str,
    device_code: &str,
    now: u64,
) -> Result<PollStep, AuthError> {
    let base = base.trim_end_matches('/');
    let (status, body) = client.post_form(
        &format!("{base}/login/oauth/access_token"),
        &[
            ("client_id", GITHUB_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", DEVICE_GRANT),
        ],
    )?;
    if let Some(code) = body.get("error").and_then(Value::as_str) {
        return match code {
            "authorization_pending" => Ok(PollStep::Pending),
            "slow_down" => Ok(PollStep::SlowDown),
            "access_denied" => Ok(PollStep::Denied),
            "expired_token" => Ok(PollStep::Expired),
            other => Err(AuthError::Rejected(other.to_owned())),
        };
    }
    if !(200..300).contains(&status) {
        return Err(unexpected(status, &body));
    }
    tokens_from(&body, now).map(PollStep::Done)
}

/// Exchanges a refresh token for a new access token. GitHub rotates the
/// refresh token: the answer carries a new one and the old one is dead.
pub fn github_refresh(
    client: &Client,
    base: &str,
    refresh_token: &str,
) -> Result<OAuthTokens, AuthError> {
    github_refresh_at(client, base, refresh_token, now_unix())
}

fn github_refresh_at(
    client: &Client,
    base: &str,
    refresh_token: &str,
    now: u64,
) -> Result<OAuthTokens, AuthError> {
    let base = base.trim_end_matches('/');
    let (status, body) = client.post_form(
        &format!("{base}/login/oauth/access_token"),
        &[
            ("client_id", GITHUB_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )?;
    // GitHub answers a bad refresh token with HTTP 200 and `error` in the body.
    if let Some(code) = body.get("error").and_then(Value::as_str) {
        return Err(AuthError::Rejected(code.to_owned()));
    }
    if !(200..300).contains(&status) {
        return Err(unexpected(status, &body));
    }
    tokens_from(&body, now)
}

/// Reads a successful token answer. Both lifetimes and the refresh token are
/// required: the app is configured for expiring tokens, so an answer without
/// them is not one we can keep alive.
fn tokens_from(body: &Value, now: u64) -> Result<OAuthTokens, AuthError> {
    let malformed = || AuthError::Unexpected("no expiring token in the answer".into());
    let access = string_of(body, "access_token").ok_or_else(malformed)?;
    let refresh = string_of(body, "refresh_token").ok_or_else(malformed)?;
    let access_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .ok_or_else(malformed)?;
    let refresh_in = body
        .get("refresh_token_expires_in")
        .and_then(Value::as_u64)
        .ok_or_else(malformed)?;
    Ok(OAuthTokens {
        access,
        refresh,
        access_expires_at: now.saturating_add(access_in),
        refresh_expires_at: now.saturating_add(refresh_in),
    })
}

#[cfg(test)]
mod tests;
