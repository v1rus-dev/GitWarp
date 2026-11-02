//! Hosting-service accounts, independent of the UI: the secret store global,
//! the account status check, and the rules of the Add / Reconnect dialog.
//!
//! Everything here is plain data and blocking functions, so it is unit tested
//! without a window. The Settings → Accounts page and the dialog call these on
//! the background executor.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use forge::api::{ApiError, Client, Probe, RepoListing, RepoSummary, TokenCheck, TokenKind};
use forge::auth::{AuthError, DeviceCode, OAuthTokens, PollStep, StoredSecret};
use forge::secrets::{KeychainStore, SecretError, SecretStore};
use forge::{Instance, ServiceKind};
use gpui::{App, Global, SharedString};

use crate::network_service::{AccountCredential, HostKey};
use crate::settings::{AccountMethod, AccountRecord, LEGACY_ACCOUNT_ID_PREFIX};

/* ------------------------------ secret store ----------------------------- */

/// The shared handle to the secret store, or the reason there is none.
pub(crate) type StoreResult = Result<Arc<dyn SecretStore>, SecretError>;

/// The outcome of opening the keychain, once known. `None` while the startup
/// task is still running.
#[derive(Default)]
struct SecretStoreGlobal {
    state: Option<StoreResult>,
}

impl Global for SecretStoreGlobal {}

/// Opens the system keychain. Blocking (it talks to the Secret Service on
/// Linux) and idempotent: the platform store is created once per process, so
/// calling it again just returns the cached outcome.
pub(crate) fn resolve_store() -> StoreResult {
    KeychainStore::init().map(|store| Arc::new(store) as Arc<dyn SecretStore>)
}

/// Opens the keychain once at startup, off the UI thread, and keeps the
/// result in a global. There is no fallback to a plaintext store: when the
/// keychain is unavailable the Accounts page shows the reason.
pub(crate) fn init_secret_store(cx: &mut App) {
    cx.set_global(SecretStoreGlobal::default());
    cx.spawn(async move |cx| {
        let state = cx
            .background_executor()
            .spawn(async { resolve_store() })
            .await;
        cx.update(|cx| cx.global_mut::<SecretStoreGlobal>().state = Some(state));
    })
    .detach();
}

/// Installs an empty in-memory store as the opened keychain, so a test never
/// talks to the system one.
#[cfg(test)]
pub(crate) fn set_test_store(cx: &mut App) {
    cx.set_global(SecretStoreGlobal {
        state: Some(Ok(Arc::new(forge::secrets::MemoryStore::new()))),
    });
}

/// The store opened at startup, `None` while that is still in progress. A
/// caller that gets `None` runs [`resolve_store`] on the background executor
/// instead of waiting.
pub(crate) fn cached_store(cx: &App) -> Option<StoreResult> {
    cx.try_global::<SecretStoreGlobal>()
        .and_then(|global| global.state.clone())
}

/* --------------------------------- ids ----------------------------------- */

/// A fresh account id: time plus a per-process counter, in hex. It is also the
/// keychain entry name, so it never changes for the life of the account.
pub(crate) fn new_account_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}{:04x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/* ------------------------------ service facts ---------------------------- */

/// The two-letter badge shown instead of an icon; GitHub has a real icon.
pub(crate) fn monogram(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::GitHub => "",
        ServiceKind::GitLab => "GL",
        ServiceKind::Gitea => "GT",
        ServiceKind::Bitbucket => "BB",
        ServiceKind::AzureDevOps => "AZ",
    }
}

/// The well-known public host of `kind`.
pub(crate) fn public_host(kind: ServiceKind) -> &'static str {
    forge::PUBLIC_INSTANCES
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, host)| *host)
        .unwrap_or_default()
}

/// What the self-hosted flavour of `kind` is called.
pub(crate) fn self_hosted_name(kind: ServiceKind) -> SharedString {
    match kind {
        ServiceKind::GitHub => ui::t!("settings.accounts.service.self_hosted.github"),
        ServiceKind::GitLab => ui::t!("settings.accounts.service.self_hosted.gitlab"),
        ServiceKind::Gitea => ui::t!("settings.accounts.service.self_hosted.gitea"),
        ServiceKind::Bitbucket => ui::t!("settings.accounts.service.self_hosted.bitbucket"),
        ServiceKind::AzureDevOps => ui::t!("settings.accounts.service.self_hosted.azure"),
    }
}

/// The card text under a service on the first step.
pub(crate) fn service_blurb(kind: ServiceKind) -> SharedString {
    match kind {
        ServiceKind::Gitea => ui::t!("settings.accounts.service.blurb.gitea"),
        ServiceKind::AzureDevOps => ui::t!("settings.accounts.service.blurb.azure"),
        other => ui::t!(
            "settings.accounts.service.blurb.public",
            host = public_host(other)
        ),
    }
}

/// Whether the service has a browser sign-in flow for desktop apps. Whether
/// GitWarp can run it today is [`browser_available`].
pub(crate) fn has_oauth(kind: ServiceKind) -> bool {
    matches!(
        kind,
        ServiceKind::GitHub | ServiceKind::GitLab | ServiceKind::Gitea
    )
}

/// Whether "Sign in with browser" can be used: only GitHub's public instance
/// has a GitWarp OAuth app so far (device flow); the client IDs for
/// self-hosted instances, GitLab and Gitea come later.
pub(crate) fn browser_available(kind: ServiceKind, self_hosted: bool) -> bool {
    kind == ServiceKind::GitHub && !self_hosted
}

/// The method the sign-in step starts on: the browser where it is available
/// (and the account, when reconnecting, did not use a token), else a token.
pub(crate) fn default_method(
    kind: ServiceKind,
    self_hosted: bool,
    reconnecting: Option<AccountMethod>,
) -> AccountMethod {
    if browser_available(kind, self_hosted) && reconnecting != Some(AccountMethod::Token) {
        AccountMethod::Browser
    } else {
        AccountMethod::Token
    }
}

/// The permissions GitWarp asks for in a token.
pub(crate) fn required_permissions(kind: ServiceKind) -> Vec<String> {
    // GitHub and GitLab list token scope names, which are identifiers and stay
    // as they are; the others name permissions as the service's UI words them.
    let literal = |scopes: &[&str]| scopes.iter().map(|s| (*s).to_owned()).collect();
    match kind {
        ServiceKind::GitHub => literal(&["repo", "read:user", "workflow"]),
        ServiceKind::GitLab => literal(&["api", "read_repository", "write_repository"]),
        ServiceKind::Gitea => vec![
            ui::t!("settings.accounts.permission.gitea.repository").to_string(),
            ui::t!("settings.accounts.permission.gitea.user").to_string(),
        ],
        ServiceKind::Bitbucket => vec![
            ui::t!("settings.accounts.permission.bitbucket.repositories").to_string(),
            ui::t!("settings.accounts.permission.bitbucket.pull_requests").to_string(),
        ],
        ServiceKind::AzureDevOps => {
            vec![ui::t!("settings.accounts.permission.azure.code").to_string()]
        }
    }
}

/// Where in the service's settings tokens are created.
pub(crate) fn token_where(kind: ServiceKind) -> SharedString {
    match kind {
        ServiceKind::GitHub => ui::t!("settings.accounts.token_where.github"),
        ServiceKind::GitLab => ui::t!("settings.accounts.token_where.gitlab"),
        ServiceKind::Gitea => ui::t!("settings.accounts.token_where.gitea"),
        ServiceKind::Bitbucket => ui::t!("settings.accounts.token_where.bitbucket"),
        ServiceKind::AzureDevOps => ui::t!("settings.accounts.token_where.azure"),
    }
}

/// The page where a token for GitWarp is created, prefilled where the
/// service supports it.
pub(crate) fn token_page_url(kind: ServiceKind, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match kind {
        ServiceKind::GitHub => {
            format!("{base}/settings/tokens/new?scopes=repo,read:user,workflow&description=GitWarp")
        }
        ServiceKind::GitLab => format!(
            "{base}/-/user_settings/personal_access_tokens?name=GitWarp&scopes=api,read_repository,write_repository"
        ),
        ServiceKind::Gitea => format!("{base}/user/settings/applications"),
        ServiceKind::Bitbucket => {
            "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned()
        }
        ServiceKind::AzureDevOps => format!("{base}/_usersSettings/tokens"),
    }
}

/// The account's profile page in the browser.
pub(crate) fn profile_url(account: &AccountRecord) -> String {
    format!(
        "{}/{}",
        account.base_url.trim_end_matches('/'),
        account.login
    )
}

/// `host[:port]` of a base URL, for display.
pub(crate) fn host_of(base_url: &str) -> String {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.trim_end_matches('/').to_owned()
}

/// The record's service, `None` for a kind this build does not know.
pub(crate) fn kind_of(account: &AccountRecord) -> Option<ServiceKind> {
    ServiceKind::from_id(&account.kind)
}

/// The record as a `forge` instance.
pub(crate) fn instance_of(account: &AccountRecord) -> Result<Instance, String> {
    let kind = kind_of(account).ok_or_else(|| {
        ui::t!(
            "settings.accounts.error.unknown_service",
            kind = account.kind
        )
        .to_string()
    })?;
    let mut instance = Instance::new(kind, &account.base_url).map_err(|e| e.to_string())?;
    instance.ssh_host = account.ssh_host.clone();
    Ok(instance)
}

/// Whether finishing a new account on `base_url` needs a label: another
/// account already uses the same instance, and a label tells them apart in
/// the clone, push and pull-request menus.
pub(crate) fn needs_label(existing: &[AccountRecord], base_url: &str) -> bool {
    existing
        .iter()
        .any(|a| a.base_url.eq_ignore_ascii_case(base_url))
}

/* ------------------------------ access tokens ---------------------------- */

/// A browser sign-in's access token is renewed when it has less than this
/// left, so a long git operation that starts now does not outlive it.
const REFRESH_WINDOW_SECS: u64 = 5 * 60;

/// The Accounts page warns when a browser sign-in's refresh token ends within
/// this many days: after that the user has to sign in again.
pub(crate) const EXPIRY_WARNING_DAYS: u64 = 14;

const SECS_PER_DAY: u64 = 24 * 60 * 60;

/// Why no usable token could be had for an account. Carries no token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenError {
    /// Nothing is stored for the account.
    Missing,
    /// The stored sign-in is dead (refresh token expired or refused, secret
    /// unreadable): the account has to be reconnected.
    NeedsReconnect,
    /// The service could not be asked; the text says why.
    Unreachable(String),
    /// The keychain could not be read.
    Keychain(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Missing => f.write_str("No token is stored for this account"),
            TokenError::NeedsReconnect => f.write_str("The account has to be reconnected"),
            TokenError::Unreachable(why) => write!(f, "Cannot renew the sign-in: {why}"),
            TokenError::Keychain(why) => write!(f, "Keychain error: {why}"),
        }
    }
}

impl std::error::Error for TokenError {}

/// One in-process lock per account id.
fn refresh_lock(id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(id.to_owned())
        .or_default()
        .clone()
}

/// Where the per-account lock files live. Tests use a scratch directory so
/// they never touch the user's configuration.
fn lock_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        Some(std::env::temp_dir().join("gitwarp-test-locks"))
    }
    #[cfg(not(test))]
    {
        crate::settings::config_dir().map(|dir| dir.join("locks"))
    }
}

/// Lock file name of an account: the id with everything but `[A-Za-z0-9_-]`
/// replaced, so a tampered id cannot escape the directory.
fn lock_file_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.lock")
}

/// Runs `f` while holding the account's locks: the in-process mutex and an
/// exclusive lock on a per-account file, so that other GitWarp processes
/// (a second window, a CLI helper) do not refresh or rewrite the same secret
/// at the same time: a refresh token is single-use. `File::lock` blocks and is
/// advisory; it is released when the file is dropped. If the lock file cannot
/// be created the in-process lock alone is used (better than failing git).
fn with_account_lock<T>(id: &str, f: impl FnOnce() -> T) -> T {
    let lock = refresh_lock(id);
    let _in_process = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let _file = lock_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(lock_file_name(id)))
            .ok()?;
        file.lock().ok()?;
        Some(file)
    });
    f()
}

/// Rotated tokens whose write to the keychain failed twice, by account id.
/// The old refresh token is dead, so these are the only live ones: consulted
/// first, dropped by the next successful write or removal.
fn rotated_cache() -> std::sync::MutexGuard<'static, HashMap<String, OAuthTokens>> {
    static ROTATED: OnceLock<Mutex<HashMap<String, OAuthTokens>>> = OnceLock::new();
    ROTATED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Writes the secret of an account (Add, Reconnect, import) under the
/// account's locks, so it cannot interleave with a token refresh.
pub(crate) fn store_secret(
    store: &dyn SecretStore,
    id: &str,
    secret: &str,
) -> Result<(), SecretError> {
    with_account_lock(id, || {
        let result = store.set(id, secret);
        if result.is_ok() {
            rotated_cache().remove(id);
        }
        result
    })
}

/// Deletes the secret of an account (removal) under the account's locks.
pub(crate) fn forget_secret(store: &dyn SecretStore, id: &str) -> Result<(), SecretError> {
    with_account_lock(id, || {
        rotated_cache().remove(id);
        store.delete(id)
    })
}

/// Whether an OAuth-format secret may be used for this instance. Only the
/// public github.com over https has a GitWarp OAuth app: a tampered
/// `base_url` in settings must never receive the token.
pub(crate) fn oauth_allowed(instance: &Instance) -> bool {
    instance.kind == ServiceKind::GitHub
        && instance.is_public()
        && !is_plain_http(&instance.base_url)
}

/// [`oauth_allowed`] for the host git is asking a password for.
pub(crate) fn oauth_allowed_for_host(host: &HostKey) -> bool {
    host.scheme == "https" && host.host == "github.com" && host.port.is_none()
}

/// The token to use for `account` right now: a personal access token as
/// stored, or a browser sign-in's access token, renewed first when it ends
/// within five minutes. The single place a stored secret becomes a token.
/// Blocking (keychain, and the service when renewing): background threads only.
pub(crate) fn access_token(
    store: &dyn SecretStore,
    account: &AccountRecord,
) -> Result<String, TokenError> {
    let allowed = instance_of(account).is_ok_and(|instance| oauth_allowed(&instance));
    access_credential_for_id(store, &account.id, allowed).map(|(token, _)| token)
}

/// [`access_token`] for a caller that only has the account id (git's askpass
/// answer). `oauth_allowed` says whether the instance may receive a browser
/// sign-in's token ([`oauth_allowed`]); browser sign-in exists for GitHub
/// only, so a secret in the OAuth format is renewed against GitHub.
pub(crate) fn access_token_for_id(
    store: &dyn SecretStore,
    id: &str,
    oauth_allowed: bool,
) -> Result<String, TokenError> {
    access_credential_for_id(store, id, oauth_allowed).map(|(token, _)| token)
}

/// [`access_token_for_id`] that also returns how to send the token, for
/// callers that build a `forge::api::Auth`.
pub(crate) fn access_credential_for_id(
    store: &dyn SecretStore,
    id: &str,
    oauth_allowed: bool,
) -> Result<(String, TokenKind), TokenError> {
    let refresh = |refresh_token: &str| {
        forge::auth::github_refresh(&Client::new(), forge::auth::GITHUB_OAUTH_URL, refresh_token)
    };
    credential_with(store, id, oauth_allowed, forge::auth::now_unix(), &refresh)
}

fn needs_refresh(tokens: &OAuthTokens, now: u64) -> bool {
    tokens.access_expires_at.saturating_sub(now) < REFRESH_WINDOW_SECS
}

/// What the keychain (or the rotated-token cache) holds for an account.
enum Current {
    Pat(String),
    OAuth(OAuthTokens),
}

/// The account's current credentials, and the raw secret the keychain held
/// when read (`None` when it holds nothing or could not be read).
fn read_current(
    store: &dyn SecretStore,
    id: &str,
    oauth_allowed: bool,
) -> Result<(Current, Option<String>), TokenError> {
    let stored = match store.get(id) {
        Ok(Some(secret)) if !secret.is_empty() => Some(secret),
        Ok(_) => None,
        Err(error) => {
            // A failed read is no reason to drop tokens we know are live.
            return match rotated_cache().get(id) {
                Some(tokens) if oauth_allowed => Ok((Current::OAuth(tokens.clone()), None)),
                _ => Err(TokenError::Keychain(error.to_string())),
            };
        }
    };
    let rotated = rotated_cache().get(id).cloned();
    if let Some(tokens) = rotated {
        // Written after the keychain copy was made and never persisted.
        return if oauth_allowed {
            Ok((Current::OAuth(tokens), stored))
        } else {
            Err(TokenError::NeedsReconnect)
        };
    }
    let Some(secret) = stored.clone() else {
        return Err(TokenError::Missing);
    };
    match forge::auth::parse_secret(&secret) {
        StoredSecret::Pat(token) => Ok((Current::Pat(token), stored)),
        StoredSecret::OAuth(_) if !oauth_allowed => Err(TokenError::NeedsReconnect),
        StoredSecret::OAuth(tokens) => Ok((Current::OAuth(tokens), stored)),
        StoredSecret::Unusable => Err(TokenError::NeedsReconnect),
    }
}

/// [`credential_with`] without the token kind.
#[cfg(test)]
fn access_token_with(
    store: &dyn SecretStore,
    id: &str,
    now: u64,
    refresh: &dyn Fn(&str) -> Result<OAuthTokens, AuthError>,
) -> Result<String, TokenError> {
    credential_with(store, id, true, now, refresh).map(|(token, _)| token)
}

/// The decision logic of [`access_token`], with the clock and the refresh call
/// injected. Also says how the token is sent: a browser sign-in's token is
/// `TokenKind::OAuth` (Bearer), a stored personal access token `TokenKind::Pat`.
fn credential_with(
    store: &dyn SecretStore,
    id: &str,
    oauth_allowed: bool,
    now: u64,
    refresh: &dyn Fn(&str) -> Result<OAuthTokens, AuthError>,
) -> Result<(String, TokenKind), TokenError> {
    match read_current(store, id, oauth_allowed)?.0 {
        Current::Pat(token) => return Ok((token, TokenKind::Pat)),
        Current::OAuth(tokens) if !needs_refresh(&tokens, now) => {
            return Ok((tokens.access, TokenKind::OAuth))
        }
        Current::OAuth(_) => {}
    }
    // Single flight, across threads and processes: whoever gets the locks first
    // refreshes; the others wait here and then find the rotated secret instead
    // of using the dead refresh token a second time.
    with_account_lock(id, || {
        let (current, stored) = read_current(store, id, oauth_allowed)?;
        let tokens = match current {
            Current::Pat(token) => return Ok((token, TokenKind::Pat)),
            Current::OAuth(tokens) => tokens,
        };
        if !needs_refresh(&tokens, now) {
            return Ok((tokens.access, TokenKind::OAuth));
        }
        if tokens.refresh.is_empty() || tokens.refresh_expires_at <= now {
            return Err(TokenError::NeedsReconnect);
        }
        match refresh(&tokens.refresh) {
            Ok(renewed) => Ok(persist(
                store,
                id,
                stored.as_deref(),
                renewed,
                oauth_allowed,
            )),
            Err(AuthError::Rejected(_)) => {
                // The refresh token may have been used by a process that does
                // not take our lock: if the keychain now holds a different one,
                // that process already renewed the sign-in.
                let (current, now_stored) = read_current(store, id, oauth_allowed)
                    .map_err(|_| TokenError::NeedsReconnect)?;
                match current {
                    Current::Pat(token) => Ok((token, TokenKind::Pat)),
                    Current::OAuth(other) if other.refresh == tokens.refresh => {
                        Err(TokenError::NeedsReconnect)
                    }
                    Current::OAuth(other) if !needs_refresh(&other, now) => {
                        Ok((other.access, TokenKind::OAuth))
                    }
                    Current::OAuth(other) => match refresh(&other.refresh) {
                        Ok(renewed) => Ok(persist(
                            store,
                            id,
                            now_stored.as_deref(),
                            renewed,
                            oauth_allowed,
                        )),
                        Err(error) => Err(refresh_failure(error)),
                    },
                }
            }
            Err(error) => Err(refresh_failure(error)),
        }
    })
}

fn refresh_failure(error: AuthError) -> TokenError {
    match error {
        AuthError::Rejected(_) => TokenError::NeedsReconnect,
        AuthError::Network(why) | AuthError::Unexpected(why) => TokenError::Unreachable(why),
    }
}

/// Stores freshly rotated tokens, called with the account's locks held. The
/// old refresh token is dead, so they must not be lost:
/// - the account was removed meanwhile (`get` says nothing is stored): nothing
///   is written;
/// - the keychain secret no longer is what this refresh started from (Reconnect
///   wrote new tokens): it is not overwritten, its token is used;
/// - otherwise the secret is written, twice if needed (a failed `get` does not
///   skip it); if both writes fail the tokens are kept in memory and used until
///   a later write succeeds.
fn persist(
    store: &dyn SecretStore,
    id: &str,
    read_from: Option<&str>,
    renewed: OAuthTokens,
    oauth_allowed: bool,
) -> (String, TokenKind) {
    match store.get(id) {
        Ok(None) => {
            rotated_cache().remove(id);
            return (renewed.access, TokenKind::OAuth);
        }
        Ok(Some(current)) if read_from.is_some_and(|read| read != current) => {
            // Somebody wrote meanwhile: theirs is newer than ours.
            match forge::auth::parse_secret(&current) {
                StoredSecret::Pat(token) => {
                    rotated_cache().remove(id);
                    return (token, TokenKind::Pat);
                }
                StoredSecret::OAuth(tokens) if oauth_allowed => {
                    rotated_cache().remove(id);
                    return (tokens.access, TokenKind::OAuth);
                }
                _ => {}
            }
        }
        _ => {}
    }
    let secret = renewed.to_secret();
    if store.set(id, &secret).is_ok() || store.set(id, &secret).is_ok() {
        rotated_cache().remove(id);
    } else {
        rotated_cache().insert(id.to_owned(), renewed.clone());
    }
    (renewed.access, TokenKind::OAuth)
}

/// Whole days (rounded up) a browser sign-in's refresh token has left when
/// that is under [`EXPIRY_WARNING_DAYS`], `None` for everything else.
fn expiry_warning_days(secret: &StoredSecret, now: u64) -> Option<u64> {
    let StoredSecret::OAuth(tokens) = secret else {
        return None;
    };
    let left = tokens.refresh_expires_at.saturating_sub(now);
    (left < EXPIRY_WARNING_DAYS * SECS_PER_DAY).then(|| left.div_ceil(SECS_PER_DAY).max(1))
}

/* ------------------------------ account status --------------------------- */

/// What the Accounts page shows for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountStatus {
    Checking,
    Connected,
    /// Connected, but a browser sign-in's refresh token ends within
    /// [`EXPIRY_WARNING_DAYS`]: after that the account has to be reconnected.
    Expiring {
        days: u64,
    },
    /// The service rejected the token, or none is stored; the text explains
    /// the second case.
    Invalid(Option<String>),
    /// The instance did not answer; other failures carry their text.
    Unreachable(Option<String>),
    /// The check could not run, for example because the keychain is locked.
    Unchecked(String),
}

/// The result of checking one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckOutcome {
    pub status: AccountStatus,
    /// The login the service reports, when it differs from the stored one.
    pub new_login: Option<String>,
}

impl CheckOutcome {
    fn status(status: AccountStatus) -> Self {
        Self {
            status,
            new_login: None,
        }
    }
}

/// Maps a token check to the page status.
pub(crate) fn outcome_of(stored_login: &str, result: Result<TokenCheck, ApiError>) -> CheckOutcome {
    match result {
        Ok(check) => CheckOutcome {
            new_login: (check.user.login != stored_login).then_some(check.user.login),
            status: AccountStatus::Connected,
        },
        Err(ApiError::Unauthorized) => CheckOutcome::status(AccountStatus::Invalid(None)),
        Err(ApiError::Unreachable(_) | ApiError::TlsUntrusted(_)) => {
            CheckOutcome::status(AccountStatus::Unreachable(None))
        }
        Err(other) => CheckOutcome::status(AccountStatus::Unreachable(Some(other.to_string()))),
    }
}

/// Moves a token found in the git credential helper into the keychain, unless
/// the account was removed meanwhile (`is_current` is false): then nothing is
/// written, and a write that raced with the removal is undone. `Ok(None)` means
/// the account is gone.
fn import_legacy_secret(
    store: &dyn SecretStore,
    id: &str,
    password: String,
    is_current: &dyn Fn() -> bool,
) -> Result<Option<String>, SecretError> {
    if !is_current() {
        return Ok(None);
    }
    store_secret(store, id, &password)?;
    if !is_current() {
        forget_secret(store, id)?;
        return Ok(None);
    }
    Ok(Some(password))
}

/// Checks one account. Blocking: reads the keychain, asks the service who
/// owns the token. An account migrated from the old settings has no secret
/// yet; its first check looks in the git credential helper for the token the
/// old Identity page stored there and moves it into the keychain.
pub(crate) fn check_account(
    store: &StoreResult,
    git: Option<&git_core::Git>,
    account: &AccountRecord,
    is_current: &dyn Fn() -> bool,
) -> CheckOutcome {
    let oauth_ok = instance_of(account).is_ok_and(|instance| oauth_allowed(&instance));
    check_account_with(store, git, account, is_current, &Client::new(), oauth_ok)
}

/// [`check_account`] with the HTTP client and the OAuth host rule injected.
fn check_account_with(
    store: &StoreResult,
    git: Option<&git_core::Git>,
    account: &AccountRecord,
    is_current: &dyn Fn() -> bool,
    client: &Client,
    oauth_ok: bool,
) -> CheckOutcome {
    let store = match store {
        Ok(store) => store,
        Err(error) => return CheckOutcome::status(AccountStatus::Unchecked(error.to_string())),
    };
    let instance = match instance_of(account) {
        Ok(instance) => instance,
        Err(error) => return CheckOutcome::status(AccountStatus::Unreachable(Some(error))),
    };
    let mut token = match access_token_for_id(&**store, &account.id, oauth_ok) {
        Ok(token) => Some(token),
        Err(TokenError::Missing) => None,
        Err(TokenError::NeedsReconnect) => {
            return CheckOutcome::status(AccountStatus::Invalid(None))
        }
        Err(TokenError::Unreachable(why)) => {
            return CheckOutcome::status(AccountStatus::Unreachable(Some(why)))
        }
        Err(TokenError::Keychain(why)) => {
            return CheckOutcome::status(AccountStatus::Unchecked(why))
        }
    };
    if token.is_none() && account.id.starts_with(LEGACY_ACCOUNT_ID_PREFIX) {
        if let Some(git) = git {
            if let Ok(Some(credential)) = git_core::credential::fill(git, instance.host()) {
                match import_legacy_secret(&**store, &account.id, credential.password, is_current) {
                    Ok(Some(password)) => token = Some(password),
                    // Removed while the check ran: nothing was written.
                    Ok(None) => return CheckOutcome::status(AccountStatus::Checking),
                    Err(error) => {
                        return CheckOutcome::status(AccountStatus::Unchecked(error.to_string()))
                    }
                }
            }
        }
    }
    let Some(token) = token else {
        return CheckOutcome::status(AccountStatus::Invalid(Some(
            ui::t!("settings.accounts.status.no_token").to_string(),
        )));
    };
    let token_kind = match account.method {
        AccountMethod::Token => TokenKind::Pat,
        AccountMethod::Browser => TokenKind::OAuth,
    };
    let result = forge::api::current_user(client, &instance, &token, token_kind);
    let mut outcome = outcome_of(&account.login, result);
    // The refresh token, not the 8 h access token, decides when the account
    // stops working.
    if outcome.status == AccountStatus::Connected && account.method == AccountMethod::Browser {
        let secret = store.get(&account.id).ok().flatten();
        let days = secret.and_then(|secret| {
            expiry_warning_days(&forge::auth::parse_secret(&secret), forge::auth::now_unix())
        });
        if let Some(days) = days {
            outcome.status = AccountStatus::Expiring { days };
        }
    }
    outcome
}

/* ------------------------------ CLI import ------------------------------- */

/// Why importing a `gh` / `glab` login failed. Carries no token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportFailure {
    Cli(crate::cli_import::CliError),
    /// The service answered 401 for the token.
    Rejected,
    /// The token works but lacks these permissions.
    Scopes(Vec<String>),
    /// Any other failure of the check.
    Api(String),
    Keychain(String),
}

/// Imports one login: reads its token from the tool, verifies it with the
/// service, stores it in the keychain like the Add dialog's token path and
/// returns the record to add. Blocking; the token never leaves this function.
pub(crate) fn import_cli_account(
    store: &StoreResult,
    candidate: &crate::cli_import::Candidate,
) -> Result<AccountRecord, ImportFailure> {
    let token = crate::cli_import::fetch_token(candidate).map_err(ImportFailure::Cli)?;
    let instance = Instance::new(candidate.kind, &candidate.base_url)
        .map_err(|e| ImportFailure::Api(e.to_string()))?;
    let check = forge::api::current_user(&Client::new(), &instance, &token, TokenKind::Pat)
        .map_err(|e| match e {
            ApiError::Unauthorized => ImportFailure::Rejected,
            other => ImportFailure::Api(other.to_string()),
        })?;
    if !check.missing.is_empty() {
        return Err(ImportFailure::Scopes(check.missing));
    }
    let verified = Verified {
        kind: candidate.kind,
        base_url: instance.base_url,
        ssh_host: instance.ssh_host,
        login: check.user.login,
        method: AccountMethod::Token,
        token,
    };
    let store = store
        .as_ref()
        .map_err(|e| ImportFailure::Keychain(e.to_string()))?;
    let id = new_account_id();
    store_secret(&**store, &id, &verified.token)
        .map_err(|e| ImportFailure::Keychain(e.to_string()))?;
    let mut record = verified.record(id, "");
    record.imported_from = Some(candidate.tool.name().to_owned());
    Ok(record)
}

/* ----------------------------- device flow ------------------------------- */

/// Extra seconds between polls after GitHub says `slow_down`.
const SLOW_DOWN_SECS: u64 = 5;

/// Poll failures in a row (network trouble, a 5xx) the sign-in survives: a
/// code is valid for 15 minutes and a blip should not throw it away.
const MAX_POLL_FAILURES: u32 = 3;

/// `m:ss`, the countdown shown while waiting for approval.
pub(crate) fn format_countdown(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// What the dialog's timer does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tick {
    Wait,
    /// Ask GitHub whether the user has approved.
    Poll,
    /// The code ran out locally.
    Expired,
}

/// What a poll answer means for the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PollOutcome {
    Continue,
    Denied,
    Expired,
    Done(OAuthTokens),
    Failed(AuthError),
}

/// The schedule of one device flow: when to poll, how the interval grows and
/// when to give up. Time is seconds since the code was issued, so the logic
/// has no clock of its own. Holds the device code, so it has no `Debug`.
pub(crate) struct DevicePoll {
    device_code: String,
    expires_in: u64,
    interval: u64,
    next_poll: u64,
    failures: u32,
}

impl DevicePoll {
    pub(crate) fn new(code: &DeviceCode) -> Self {
        Self {
            device_code: code.device_code().to_owned(),
            expires_in: code.expires_in,
            interval: code.interval,
            next_poll: code.interval,
            failures: 0,
        }
    }

    pub(crate) fn device_code(&self) -> &str {
        &self.device_code
    }

    #[cfg(test)]
    pub(crate) fn interval(&self) -> u64 {
        self.interval
    }

    /// Seconds until the code expires.
    pub(crate) fn remaining(&self, elapsed: u64) -> u64 {
        self.expires_in.saturating_sub(elapsed)
    }

    pub(crate) fn tick(&self, elapsed: u64) -> Tick {
        if elapsed >= self.expires_in {
            Tick::Expired
        } else if elapsed >= self.next_poll {
            Tick::Poll
        } else {
            Tick::Wait
        }
    }

    /// Applies the answer of a poll made `elapsed` seconds in.
    pub(crate) fn apply(
        &mut self,
        elapsed: u64,
        result: Result<PollStep, AuthError>,
    ) -> PollOutcome {
        match result {
            Ok(PollStep::Pending) => {
                self.failures = 0;
                self.next_poll = elapsed.saturating_add(self.interval);
                PollOutcome::Continue
            }
            Ok(PollStep::SlowDown) => {
                self.failures = 0;
                self.interval = self.interval.saturating_add(SLOW_DOWN_SECS);
                self.next_poll = elapsed.saturating_add(self.interval);
                PollOutcome::Continue
            }
            Ok(PollStep::Denied) => PollOutcome::Denied,
            Ok(PollStep::Expired) => PollOutcome::Expired,
            Ok(PollStep::Done(tokens)) => PollOutcome::Done(tokens),
            Err(error @ (AuthError::Network(_) | AuthError::Unexpected(_))) => {
                self.failures += 1;
                if self.failures >= MAX_POLL_FAILURES {
                    PollOutcome::Failed(error)
                } else {
                    self.next_poll = elapsed.saturating_add(self.interval);
                    PollOutcome::Continue
                }
            }
            // The service refused the device code itself: retrying cannot help.
            Err(error @ AuthError::Rejected(_)) => PollOutcome::Failed(error),
        }
    }
}

/* ---------------------------- Add / Reconnect ---------------------------- */

/// The steps of the Add account dialog. Reconnect starts at [`Step::SignIn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Service,
    Instance,
    SignIn,
    Done,
}

impl Step {
    /// The step the Back button returns to.
    pub(crate) fn back(self) -> Step {
        match self {
            Step::Service | Step::Instance => Step::Service,
            Step::SignIn => Step::Instance,
            Step::Done => Step::SignIn,
        }
    }

    /// 1-based number in the stepper; the result counts as past the last step.
    pub(crate) fn number(self) -> usize {
        match self {
            Step::Service => 1,
            Step::Instance => 2,
            Step::SignIn => 3,
            Step::Done => 4,
        }
    }
}

/// What the instance step knows about the address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeState {
    Idle,
    Checking,
    /// The address is the chosen service; `version` when it reported one.
    Detected {
        version: Option<String>,
    },
    /// The address answered like another service.
    Other(ServiceKind),
    Unreachable(String),
    Tls(String),
    NotRecognized,
}

/// Maps a [`forge::api::probe`] answer for the service the user picked.
pub(crate) fn probe_state(selected: ServiceKind, result: Result<Probe, ApiError>) -> ProbeState {
    match result {
        Ok(Probe {
            kind: Some(kind),
            version,
        }) if kind == selected => ProbeState::Detected { version },
        Ok(Probe {
            kind: Some(other), ..
        }) => ProbeState::Other(other),
        Ok(Probe { kind: None, .. }) => ProbeState::NotRecognized,
        Err(ApiError::TlsUntrusted(why)) => {
            ProbeState::Tls(ApiError::TlsUntrusted(why).to_string())
        }
        Err(other) => ProbeState::Unreachable(other.to_string()),
    }
}

/// Parses what the user typed in the Instance URL field.
pub(crate) fn instance_from_input(kind: ServiceKind, text: &str) -> Option<Instance> {
    let text = text.trim();
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    Instance::new(kind, text)
        .ok()
        .filter(|instance| !instance.host().is_empty())
}

/// A token the service accepted, with the instance it was checked against.
/// The dialog saves exactly this, never values recomputed from the form,
/// which the user may have changed since.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Verified {
    pub kind: ServiceKind,
    pub base_url: String,
    pub ssh_host: Option<String>,
    pub login: String,
    pub method: AccountMethod,
    /// The keychain secret: the raw token, or for a browser sign-in the JSON
    /// secret of `forge::auth` holding both tokens.
    pub token: String,
}

impl std::fmt::Debug for Verified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verified")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("login", &self.login)
            .finish_non_exhaustive()
    }
}

impl Verified {
    /// The record to store. An empty `label` defaults to the login.
    pub(crate) fn record(&self, id: String, label: &str) -> AccountRecord {
        let label = label.trim();
        AccountRecord {
            id,
            kind: self.kind.id().to_owned(),
            base_url: self.base_url.clone(),
            login: self.login.clone(),
            label: if label.is_empty() {
                self.login.clone()
            } else {
                label.to_owned()
            },
            method: self.method,
            ssh_host: self.ssh_host.clone(),
            imported_from: None,
        }
    }
}

/// Whether a token-check answer still belongs to what the dialog shows: it is
/// the latest attempt and the dialog is still open.
pub(crate) fn attempt_is_current(attempt: u64, latest: u64, open: bool) -> bool {
    open && attempt == latest
}

/// Whether the address sends the token unencrypted.
pub(crate) fn is_plain_http(base_url: &str) -> bool {
    base_url
        .trim_start()
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

/// Label of the primary button on the instance step.
pub(crate) fn continue_label(public: bool, probe: &ProbeState) -> SharedString {
    match probe {
        ProbeState::Checking => ui::t!("settings.accounts.dialog.checking"),
        ProbeState::Detected { .. } => ui::t!("settings.accounts.dialog.continue"),
        _ if public => ui::t!("settings.accounts.dialog.continue"),
        _ => ui::t!("settings.accounts.dialog.check_address"),
    }
}

/// Whether the instance step's primary button can be pressed. A certificate
/// that is not trusted and a service of another type block the step.
pub(crate) fn continue_enabled(public: bool, address_valid: bool, probe: &ProbeState) -> bool {
    match probe {
        ProbeState::Checking | ProbeState::Tls(_) | ProbeState::Other(_) => false,
        _ => public || address_valid,
    }
}

/// Why a token was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignInError {
    /// The service answered 401.
    Wrong,
    /// The token works but lacks these permissions.
    Permissions(Vec<String>),
    /// The service could not be reached; carries the error text.
    Network(String),
    /// Browser sign-in: the user declined in the browser.
    Denied,
    /// Browser sign-in: the device code ran out before it was approved.
    Expired,
    /// Any other failure; carries the error text.
    Other(String),
}

/// Maps a failed device-flow request (or poll) of the browser sign-in.
pub(crate) fn browser_error(error: AuthError) -> SignInError {
    match error {
        AuthError::Network(why) => SignInError::Network(AuthError::Network(why).to_string()),
        other => SignInError::Other(other.to_string()),
    }
}

/// Maps a token check made by the dialog: the login on success.
pub(crate) fn sign_in_outcome(
    kind: ServiceKind,
    result: Result<TokenCheck, ApiError>,
) -> Result<String, SignInError> {
    match result {
        Ok(check) if check.missing.is_empty() => Ok(check.user.login),
        Ok(check) => Err(SignInError::Permissions(check.missing)),
        Err(ApiError::Unauthorized) => Err(SignInError::Wrong),
        // 403 means the token was understood but is not allowed to read the
        // profile: it lacks permissions, but the service did not say which.
        Err(ApiError::Forbidden) => Err(SignInError::Permissions(required_permissions(kind))),
        Err(error @ (ApiError::Unreachable(_) | ApiError::TlsUntrusted(_))) => {
            Err(SignInError::Network(error.to_string()))
        }
        Err(other) => Err(SignInError::Other(other.to_string())),
    }
}

/* -------------------------- clone from an account ------------------------- */

/// The sources of the Clone dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneTab {
    Url,
    Account,
}

/// "From account" opens first when there is an account to list; without one
/// the tab is hidden and the dialog is the URL form.
pub(crate) fn default_clone_tab(account_count: usize) -> CloneTab {
    if account_count > 0 {
        CloneTab::Account
    } else {
        CloneTab::Url
    }
}

/// An account's entry in the account select: `<label> · <host>`.
pub(crate) fn account_option_label(account: &AccountRecord) -> String {
    format!(
        "{} · {}",
        account.display_label(),
        host_of(&account.base_url)
    )
}

/// Why the repository list of an account could not be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepoListError {
    /// The service rejected the token, or none is stored: Reconnect helps.
    Revoked,
    /// The instance did not answer: Retry helps.
    Unreachable,
    /// The service has no account-wide repository list (Azure DevOps).
    Unsupported,
    /// Anything else, with the text to show (Retry helps).
    Other(String),
}

pub(crate) fn repo_list_error(error: ApiError) -> RepoListError {
    match error {
        ApiError::Unauthorized => RepoListError::Revoked,
        ApiError::Unreachable(_) | ApiError::TlsUntrusted(_) => RepoListError::Unreachable,
        ApiError::Unsupported => RepoListError::Unsupported,
        other => RepoListError::Other(other.to_string()),
    }
}

/// The repositories `account` can see. Blocking: reads the keychain, then
/// asks the service. The token never leaves this function.
pub(crate) fn list_account_repos(
    store: &StoreResult,
    account: &AccountRecord,
) -> Result<RepoListing, RepoListError> {
    let store = store
        .as_ref()
        .map_err(|error| RepoListError::Other(error.to_string()))?;
    let instance = instance_of(account).map_err(RepoListError::Other)?;
    // No token is needed to know there is no list to ask for.
    if instance.kind == ServiceKind::AzureDevOps {
        return Err(RepoListError::Unsupported);
    }
    // Blocking: may renew a browser sign-in's access token first.
    let token = access_token(&**store, account).map_err(|error| match error {
        TokenError::Missing | TokenError::NeedsReconnect => RepoListError::Revoked,
        TokenError::Unreachable(_) => RepoListError::Unreachable,
        TokenError::Keychain(why) => RepoListError::Other(why),
    })?;
    forge::api::list_user_repos(&Client::new(), &instance, &token).map_err(repo_list_error)
}

/// Indexes into `repos` of those matching `query`: a case-insensitive
/// substring of `owner/name description`; an empty query matches all.
pub(crate) fn filter_repos(repos: &[RepoSummary], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    repos
        .iter()
        .enumerate()
        .filter(|(_, repo)| {
            query.is_empty()
                || format!(
                    "{}/{} {}",
                    repo.owner,
                    repo.name,
                    repo.description.as_deref().unwrap_or("")
                )
                .to_lowercase()
                .contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

/// The host entry that lets a clone of `url` use `account`'s token through
/// the askpass helper. `None` for a non-HTTPS address, an unknown service, or
/// an address that is not on the account's own instance: a service can report
/// a clone URL on another host, and the token must never go there.
pub(crate) fn clone_credential(
    account: &AccountRecord,
    url: &str,
) -> Option<(HostKey, AccountCredential)> {
    let remote = forge::RemoteRef::parse(url)?;
    let instance = instance_of(account).ok()?;
    // `matches` also accepts the SSH host and host aliases; an HTTPS token is
    // only for the web host itself, on the instance's port and sub-path.
    instance.matches(&remote)?;
    if !remote.host.eq_ignore_ascii_case(instance.host()) {
        return None;
    }
    let scheme = match remote.transport {
        forge::Transport::Https => "https",
        // Same rule as the push / fetch jobs: plain http only for an account
        // that was itself set up on an http:// instance.
        forge::Transport::Http
            if account
                .base_url
                .trim()
                .to_ascii_lowercase()
                .starts_with("http://") =>
        {
            "http"
        }
        forge::Transport::Http | forge::Transport::Ssh => return None,
    };
    let kind = kind_of(account)?;
    Some((
        HostKey::new(scheme, &remote.host, remote.port),
        AccountCredential {
            account_id: account.id.clone(),
            username: kind.git_username(&account.login),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::api::User;
    use forge::secrets::MemoryStore;

    fn record(id: &str, url: &str) -> AccountRecord {
        AccountRecord {
            id: id.to_owned(),
            kind: "github".to_owned(),
            base_url: url.to_owned(),
            login: "mira-k".to_owned(),
            label: "mira-k".to_owned(),
            method: crate::settings::AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    fn repo(owner: &str, name: &str, description: Option<&str>) -> RepoSummary {
        RepoSummary {
            owner: owner.to_owned(),
            name: name.to_owned(),
            description: description.map(str::to_owned),
            private: false,
            fork: false,
            archived: false,
            updated_at: 0,
            clone_https: format!("https://github.com/{owner}/{name}.git"),
        }
    }

    #[test]
    fn the_default_tab_follows_the_accounts() {
        assert_eq!(default_clone_tab(0), CloneTab::Url);
        assert_eq!(default_clone_tab(1), CloneTab::Account);
        assert_eq!(default_clone_tab(4), CloneTab::Account);
    }

    #[test]
    fn the_account_option_shows_label_and_host() {
        let mut account = record("a1", "https://github.com/");
        assert_eq!(account_option_label(&account), "mira-k · github.com");
        account.label = "Work".to_owned();
        assert_eq!(account_option_label(&account), "Work · github.com");
    }

    #[test]
    fn the_search_matches_owner_name_and_description_ignoring_case() {
        let repos = [
            repo("mira-k", "gitwarp", Some("A fast git client")),
            repo("nexu-io", "open-design", None),
            repo("mira-k", "dotfiles", Some("Shell configs")),
        ];
        assert_eq!(filter_repos(&repos, ""), [0, 1, 2]);
        assert_eq!(filter_repos(&repos, "  "), [0, 1, 2]);
        assert_eq!(filter_repos(&repos, "MIRA"), [0, 2]);
        assert_eq!(filter_repos(&repos, "client"), [0]);
        assert_eq!(filter_repos(&repos, "nexu-io/open"), [1]);
        assert!(filter_repos(&repos, "zzz").is_empty());
    }

    #[test]
    fn list_errors_map_to_what_the_user_can_do() {
        assert_eq!(
            repo_list_error(ApiError::Unauthorized),
            RepoListError::Revoked
        );
        assert_eq!(
            repo_list_error(ApiError::Unreachable("dns".into())),
            RepoListError::Unreachable
        );
        assert_eq!(
            repo_list_error(ApiError::Unsupported),
            RepoListError::Unsupported
        );
        assert!(matches!(
            repo_list_error(ApiError::Status(500)),
            RepoListError::Other(_)
        ));
    }

    #[test]
    fn a_missing_token_means_reconnect_without_a_request() {
        let store: StoreResult = Ok(Arc::new(MemoryStore::new()));
        let account = record("a1", "https://github.com");
        assert_eq!(
            list_account_repos(&store, &account),
            Err(RepoListError::Revoked)
        );
    }

    #[test]
    fn a_clone_uses_the_account_for_https_only() {
        let account = record("a1", "https://github.com");
        let (host, credential) =
            clone_credential(&account, "https://github.com/acme/tool.git").unwrap();
        assert_eq!(host, HostKey::new("https", "github.com", None));
        assert_eq!(credential.account_id, "a1");
        assert_eq!(credential.username, "mira-k");
        assert!(clone_credential(&account, "git@github.com:acme/tool.git").is_none());
        assert!(clone_credential(&account, "http://github.com/acme/tool.git").is_none());
    }

    /// A self-hosted service may report a clone URL on another host; the
    /// token is not offered there.
    #[test]
    fn a_clone_url_on_another_host_gets_no_token() {
        let mut account = record("a2", "https://git.corp.example");
        account.kind = "gitlab".to_owned();
        assert!(clone_credential(&account, "https://git.corp.example/grp/tool.git").is_some());
        for other in [
            "https://evil.example/grp/tool.git",
            "https://git.corp.example.evil.example/grp/tool.git",
            "https://git.corp.example:8443/grp/tool.git",
            "https://user@evil.example/grp/tool.git",
        ] {
            assert!(clone_credential(&account, other).is_none(), "{other}");
        }
        // The instance's sub-path is part of the match.
        let mut sub = record("a3", "https://corp.example/git");
        sub.kind = "gitlab".to_owned();
        assert!(clone_credential(&sub, "https://corp.example/git/grp/tool.git").is_some());
        assert!(clone_credential(&sub, "https://corp.example/other/grp/tool.git").is_none());
    }

    fn check(login: &str, missing: &[&str]) -> TokenCheck {
        TokenCheck {
            user: User {
                login: login.to_owned(),
                name: None,
                avatar_url: None,
            },
            scopes: None,
            missing: missing.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn status_mapping_follows_the_api_error() {
        let ok = outcome_of("mira-k", Ok(check("mira-k", &[])));
        assert_eq!(ok.status, AccountStatus::Connected);
        assert_eq!(ok.new_login, None);

        let renamed = outcome_of("old", Ok(check("new", &[])));
        assert_eq!(renamed.status, AccountStatus::Connected);
        assert_eq!(renamed.new_login.as_deref(), Some("new"));

        assert_eq!(
            outcome_of("x", Err(ApiError::Unauthorized)).status,
            AccountStatus::Invalid(None)
        );
        for error in [
            ApiError::Unreachable("dns".into()),
            ApiError::TlsUntrusted("self-signed".into()),
        ] {
            assert_eq!(
                outcome_of("x", Err(error)).status,
                AccountStatus::Unreachable(None)
            );
        }
        for error in [
            ApiError::Forbidden,
            ApiError::NotFound,
            ApiError::RateLimited { reset_at: None },
            ApiError::Status(502),
            ApiError::Decode("eof".into()),
        ] {
            let text = error.to_string();
            assert_eq!(
                outcome_of("x", Err(error)).status,
                AccountStatus::Unreachable(Some(text))
            );
        }
    }

    #[test]
    fn a_missing_secret_is_an_invalid_account_without_a_request() {
        let store: StoreResult = Ok(Arc::new(MemoryStore::new()));
        let outcome = check_account(&store, None, &record("a1", "https://github.com"), &|| true);
        assert!(matches!(outcome.status, AccountStatus::Invalid(Some(_))));
    }

    #[test]
    fn an_unavailable_store_leaves_accounts_unchecked() {
        let store: StoreResult = Err(SecretError::Unavailable("no D-Bus".into()));
        let outcome = check_account(&store, None, &record("a1", "https://github.com"), &|| true);
        match outcome.status {
            AccountStatus::Unchecked(text) => assert!(text.contains("no D-Bus"), "{text}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_label_is_required_for_a_second_account_on_the_same_instance() {
        let existing = vec![record("a1", "https://github.com")];
        assert!(needs_label(&existing, "https://github.com"));
        assert!(needs_label(&existing, "https://GitHub.com"));
        assert!(!needs_label(&existing, "https://git.acme.corp"));
        assert!(!needs_label(&[], "https://github.com"));
    }

    #[test]
    fn steps_walk_back_to_the_service_card() {
        assert_eq!(Step::Done.back(), Step::SignIn);
        assert_eq!(Step::SignIn.back(), Step::Instance);
        assert_eq!(Step::Instance.back(), Step::Service);
        assert_eq!(Step::Service.back(), Step::Service);
        assert_eq!(Step::Done.number(), 4);
    }

    #[test]
    fn the_instance_step_gates_its_button() {
        let detected = ProbeState::Detected { version: None };
        assert_eq!(continue_label(true, &ProbeState::Idle).as_ref(), "Continue");
        assert_eq!(
            continue_label(false, &ProbeState::Idle).as_ref(),
            "Check address"
        );
        assert_eq!(
            continue_label(false, &ProbeState::Checking).as_ref(),
            "Checking…"
        );
        assert_eq!(continue_label(false, &detected).as_ref(), "Continue");

        assert!(continue_enabled(true, false, &ProbeState::Idle));
        assert!(!continue_enabled(false, false, &ProbeState::Idle));
        assert!(continue_enabled(false, true, &ProbeState::Idle));
        assert!(continue_enabled(false, true, &detected));
        assert!(continue_enabled(false, true, &ProbeState::NotRecognized));
        assert!(!continue_enabled(false, true, &ProbeState::Checking));
        assert!(!continue_enabled(false, true, &ProbeState::Tls("x".into())));
        assert!(!continue_enabled(
            false,
            true,
            &ProbeState::Other(ServiceKind::GitLab)
        ));
    }

    #[test]
    fn probe_answers_map_to_states() {
        let found = |kind, version: Option<&str>| {
            Ok(Probe {
                kind,
                version: version.map(str::to_owned),
            })
        };
        assert_eq!(
            probe_state(
                ServiceKind::GitLab,
                found(Some(ServiceKind::GitLab), Some("18.4"))
            ),
            ProbeState::Detected {
                version: Some("18.4".into())
            }
        );
        assert_eq!(
            probe_state(ServiceKind::GitHub, found(Some(ServiceKind::GitLab), None)),
            ProbeState::Other(ServiceKind::GitLab)
        );
        assert_eq!(
            probe_state(ServiceKind::GitHub, found(None, None)),
            ProbeState::NotRecognized
        );
        assert!(matches!(
            probe_state(
                ServiceKind::GitHub,
                Err(ApiError::Unreachable("dns".into()))
            ),
            ProbeState::Unreachable(_)
        ));
        assert!(matches!(
            probe_state(ServiceKind::GitHub, Err(ApiError::TlsUntrusted("ca".into()))),
            ProbeState::Tls(text) if text.contains("ca")
        ));
    }

    #[test]
    fn instance_input_is_validated() {
        assert!(instance_from_input(ServiceKind::GitLab, "").is_none());
        assert!(instance_from_input(ServiceKind::GitLab, "gitlab modsen").is_none());
        assert!(instance_from_input(ServiceKind::GitLab, "ftp://git.example.com").is_none());
        let instance = instance_from_input(ServiceKind::GitLab, " git.example.com/ ").unwrap();
        assert_eq!(instance.base_url, "https://git.example.com");
    }

    #[test]
    fn sign_in_results_map_to_dialog_errors() {
        let kind = ServiceKind::GitHub;
        assert_eq!(
            sign_in_outcome(kind, Ok(check("mira-k", &[]))),
            Ok("mira-k".to_owned())
        );
        assert_eq!(
            sign_in_outcome(kind, Ok(check("mira-k", &["workflow"]))),
            Err(SignInError::Permissions(vec!["workflow".to_owned()]))
        );
        assert_eq!(
            sign_in_outcome(kind, Err(ApiError::Unauthorized)),
            Err(SignInError::Wrong)
        );
        assert_eq!(
            sign_in_outcome(kind, Err(ApiError::Forbidden)),
            Err(SignInError::Permissions(vec![
                "repo".to_owned(),
                "read:user".to_owned(),
                "workflow".to_owned()
            ]))
        );
        assert!(matches!(
            sign_in_outcome(kind, Err(ApiError::Unreachable("t".into()))),
            Err(SignInError::Network(_))
        ));
        assert!(matches!(
            sign_in_outcome(kind, Err(ApiError::Status(500))),
            Err(SignInError::Other(_))
        ));
    }

    #[test]
    fn token_and_profile_urls() {
        assert_eq!(
            token_page_url(ServiceKind::GitHub, "https://github.com"),
            "https://github.com/settings/tokens/new?scopes=repo,read:user,workflow&description=GitWarp"
        );
        assert_eq!(
            token_page_url(ServiceKind::Gitea, "https://codeberg.org/"),
            "https://codeberg.org/user/settings/applications"
        );
        assert_eq!(
            token_page_url(ServiceKind::Bitbucket, "https://bitbucket.org"),
            "https://id.atlassian.com/manage-profile/security/api-tokens"
        );
        assert_eq!(
            profile_url(&record("a", "https://git.acme.corp/")),
            "https://git.acme.corp/mira-k"
        );
        assert_eq!(host_of("https://git.acme.corp:8443/"), "git.acme.corp:8443");
    }

    #[test]
    fn an_answer_applies_only_to_the_latest_attempt_of_an_open_dialog() {
        assert!(attempt_is_current(3, 3, true));
        assert!(!attempt_is_current(2, 3, true));
        assert!(!attempt_is_current(3, 3, false));
    }

    #[test]
    fn the_saved_record_comes_from_the_verified_instance() {
        let verified = Verified {
            kind: ServiceKind::GitLab,
            base_url: "https://gitlab.acme.corp".to_owned(),
            ssh_host: Some("ssh.acme.corp".to_owned()),
            login: "mira".to_owned(),
            method: AccountMethod::Token,
            token: "secret".to_owned(),
        };
        let record = verified.record("id1".to_owned(), "  ");
        assert_eq!(record.method, AccountMethod::Token);
        assert_eq!(record.kind, "gitlab");
        assert_eq!(record.base_url, "https://gitlab.acme.corp");
        assert_eq!(record.ssh_host.as_deref(), Some("ssh.acme.corp"));
        assert_eq!(record.label, "mira");
        assert_eq!(verified.record("id2".to_owned(), " Work ").label, "Work");
        assert!(!format!("{verified:?}").contains("secret"));
    }

    #[test]
    fn plain_http_is_detected() {
        assert!(is_plain_http("http://git.local"));
        assert!(is_plain_http("HTTP://git.local:3000"));
        assert!(!is_plain_http("https://git.local"));
        assert!(!is_plain_http(""));
    }

    #[test]
    fn legacy_import_skips_a_removed_account() {
        let store = MemoryStore::new();
        assert_eq!(
            import_legacy_secret(&store, "legacy-github", "pw".into(), &|| false),
            Ok(None)
        );
        assert_eq!(store.get("legacy-github").unwrap(), None);
        assert_eq!(
            import_legacy_secret(&store, "legacy-github", "pw".into(), &|| true),
            Ok(Some("pw".to_owned()))
        );
        assert_eq!(store.get("legacy-github").unwrap().as_deref(), Some("pw"));

        // Removed between the check and the write: the write is undone.
        let calls = std::cell::Cell::new(0);
        let flips = || {
            calls.set(calls.get() + 1);
            calls.get() == 1
        };
        assert_eq!(
            import_legacy_secret(&store, "legacy-x", "pw".into(), &flips),
            Ok(None)
        );
        assert_eq!(store.get("legacy-x").unwrap(), None);
    }

    // ---- browser sign-in: tokens ----

    const NOW: u64 = 1_000_000;

    fn oauth(access: &str, refresh: &str, access_in: u64, refresh_in: u64) -> OAuthTokens {
        OAuthTokens {
            access: access.to_owned(),
            refresh: refresh.to_owned(),
            access_expires_at: NOW + access_in,
            refresh_expires_at: NOW + refresh_in,
        }
    }

    fn never_refresh(_: &str) -> Result<OAuthTokens, AuthError> {
        panic!("must not refresh");
    }

    fn token_of(store: &dyn SecretStore, id: &str) -> Result<String, TokenError> {
        access_token_with(store, id, NOW, &never_refresh)
    }

    #[test]
    fn a_raw_secret_is_a_personal_access_token() {
        let store = MemoryStore::new();
        store.set("pat", "ghp_raw").unwrap();
        assert_eq!(token_of(&store, "pat"), Ok("ghp_raw".to_owned()));
    }

    #[test]
    fn a_missing_or_empty_secret_is_missing() {
        let store = MemoryStore::new();
        assert_eq!(token_of(&store, "none"), Err(TokenError::Missing));
        store.set("empty", "").unwrap();
        assert_eq!(token_of(&store, "empty"), Err(TokenError::Missing));
    }

    #[test]
    fn an_unreadable_json_secret_needs_a_reconnect() {
        let store = MemoryStore::new();
        store.set("bad", r#"{"v":9}"#).unwrap();
        assert_eq!(token_of(&store, "bad"), Err(TokenError::NeedsReconnect));
    }

    #[test]
    fn a_fresh_access_token_is_returned_without_refreshing() {
        let store = MemoryStore::new();
        // Six minutes left: outside the five-minute window.
        store
            .set("a", &oauth("acc", "ref", 6 * 60, 1_000_000).to_secret())
            .unwrap();
        assert_eq!(token_of(&store, "a"), Ok("acc".to_owned()));
    }

    #[test]
    fn refresh_happens_inside_the_five_minute_window_and_the_rotation_is_stored() {
        let store = MemoryStore::new();
        store
            .set(
                "a",
                &oauth("old-acc", "old-ref", 4 * 60 + 59, 1_000_000).to_secret(),
            )
            .unwrap();
        let seen = Mutex::new(Vec::new());
        let refresh = |token: &str| {
            seen.lock().unwrap().push(token.to_owned());
            Ok(oauth("new-acc", "new-ref", 28_800, 15_638_400))
        };
        assert_eq!(
            access_token_with(&store, "a", NOW, &refresh),
            Ok("new-acc".to_owned())
        );
        assert_eq!(*seen.lock().unwrap(), vec!["old-ref".to_owned()]);
        // The new refresh token replaced the old one in the keychain.
        let stored = forge::auth::parse_secret(&store.get("a").unwrap().unwrap());
        assert_eq!(
            stored,
            StoredSecret::OAuth(oauth("new-acc", "new-ref", 28_800, 15_638_400))
        );
        // And the next call needs no refresh.
        assert_eq!(token_of(&store, "a"), Ok("new-acc".to_owned()));
    }

    #[test]
    fn an_expired_refresh_token_needs_a_reconnect_without_asking_the_service() {
        let store = MemoryStore::new();
        let mut tokens = oauth("acc", "ref", 0, 0);
        tokens.refresh_expires_at = NOW - 1;
        store.set("a", &tokens.to_secret()).unwrap();
        assert_eq!(token_of(&store, "a"), Err(TokenError::NeedsReconnect));
    }

    #[test]
    fn a_refused_refresh_needs_a_reconnect_and_keeps_the_secret() {
        let store = MemoryStore::new();
        let secret = oauth("acc", "ref", 10, 1_000_000).to_secret();
        store.set("a", &secret).unwrap();
        let refused = |_: &str| Err(AuthError::Rejected("bad_refresh_token".into()));
        assert_eq!(
            access_token_with(&store, "a", NOW, &refused),
            Err(TokenError::NeedsReconnect)
        );
        assert_eq!(store.get("a").unwrap().as_deref(), Some(secret.as_str()));
    }

    #[test]
    fn a_network_failure_while_refreshing_is_unreachable_not_a_reconnect() {
        let store = MemoryStore::new();
        store
            .set("a", &oauth("acc", "ref", 10, 1_000_000).to_secret())
            .unwrap();
        let down = |_: &str| Err(AuthError::Network("timed out".into()));
        assert_eq!(
            access_token_with(&store, "a", NOW, &down),
            Err(TokenError::Unreachable("timed out".into()))
        );
    }

    #[test]
    fn concurrent_callers_refresh_once_and_share_the_rotated_token() {
        use std::sync::atomic::AtomicUsize;
        let store = Arc::new(MemoryStore::new());
        store
            .set("flight", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (store, calls, barrier) = (store.clone(), calls.clone(), barrier.clone());
                std::thread::spawn(move || {
                    // A counting fake refresher; the sleep keeps the others waiting
                    // on the lock while it runs. A second use of `ref-1` would fail,
                    // like GitHub's single-use refresh token.
                    let refresh = |token: &str| {
                        let n = calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(token, "ref-1", "the old refresh token was used twice");
                        assert_eq!(n, 0);
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        Ok(oauth("new", "ref-2", 28_800, 15_638_400))
                    };
                    barrier.wait();
                    access_token_with(&*store, "flight", NOW, &refresh)
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), Ok("new".to_owned()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refresh_for_a_removed_account_writes_nothing_back() {
        let store = MemoryStore::new();
        store
            .set("gone", &oauth("acc", "ref", 10, 1_000_000).to_secret())
            .unwrap();
        // The account is removed while the service call is in flight.
        let refresh = |_: &str| {
            store.delete("gone").unwrap();
            Ok(oauth("new", "ref-2", 28_800, 15_638_400))
        };
        assert_eq!(
            access_token_with(&store, "gone", NOW, &refresh),
            Ok("new".to_owned())
        );
        assert_eq!(store.get("gone").unwrap(), None);
    }

    #[test]
    fn expiry_warning_uses_the_refresh_token_not_the_access_token() {
        let day = SECS_PER_DAY;
        let secret =
            |access_in, refresh_in| StoredSecret::OAuth(oauth("a", "r", access_in, refresh_in));
        // Access token ends in an hour, refresh token in 100 days: no warning.
        assert_eq!(expiry_warning_days(&secret(3600, 100 * day), NOW), None);
        // Exactly 14 days left is not yet a warning; just under is.
        assert_eq!(expiry_warning_days(&secret(0, 14 * day), NOW), None);
        assert_eq!(expiry_warning_days(&secret(0, 14 * day - 1), NOW), Some(14));
        assert_eq!(expiry_warning_days(&secret(0, 5 * day), NOW), Some(5));
        assert_eq!(expiry_warning_days(&secret(0, 5 * day - 1), NOW), Some(5));
        assert_eq!(expiry_warning_days(&secret(0, 60), NOW), Some(1));
        // A personal access token has no refresh expiry.
        assert_eq!(
            expiry_warning_days(&StoredSecret::Pat("x".into()), NOW),
            None
        );
    }

    #[test]
    fn browser_sign_in_is_offered_for_public_github_only() {
        assert!(browser_available(ServiceKind::GitHub, false));
        assert!(!browser_available(ServiceKind::GitHub, true));
        for kind in [
            ServiceKind::GitLab,
            ServiceKind::Gitea,
            ServiceKind::Bitbucket,
            ServiceKind::AzureDevOps,
        ] {
            assert!(!browser_available(kind, false), "{kind:?}");
        }
        assert_eq!(
            default_method(ServiceKind::GitHub, false, None),
            AccountMethod::Browser
        );
        assert_eq!(
            default_method(ServiceKind::GitHub, false, Some(AccountMethod::Browser)),
            AccountMethod::Browser
        );
        assert_eq!(
            default_method(ServiceKind::GitHub, false, Some(AccountMethod::Token)),
            AccountMethod::Token
        );
        assert_eq!(
            default_method(ServiceKind::GitHub, true, None),
            AccountMethod::Token
        );
        assert_eq!(
            default_method(ServiceKind::GitLab, false, None),
            AccountMethod::Token
        );
    }

    #[test]
    fn countdown_is_minutes_and_two_digit_seconds() {
        assert_eq!(format_countdown(899), "14:59");
        assert_eq!(format_countdown(900), "15:00");
        assert_eq!(format_countdown(65), "1:05");
        assert_eq!(format_countdown(0), "0:00");
    }

    // ---- browser sign-in: device flow schedule ----

    fn poll_after(code: &str, interval: u64, expires_in: u64) -> DevicePoll {
        DevicePoll {
            device_code: code.to_owned(),
            expires_in,
            interval,
            next_poll: interval,
            failures: 0,
        }
    }

    #[test]
    fn device_poll_waits_for_the_interval_then_polls() {
        let mut poll = poll_after("dc", 5, 900);
        assert_eq!(poll.tick(0), Tick::Wait);
        assert_eq!(poll.tick(4), Tick::Wait);
        assert_eq!(poll.tick(5), Tick::Poll);
        assert_eq!(poll.apply(5, Ok(PollStep::Pending)), PollOutcome::Continue);
        assert_eq!(poll.tick(9), Tick::Wait);
        assert_eq!(poll.tick(10), Tick::Poll);
        assert_eq!(poll.remaining(100), 800);
        assert_eq!(poll.remaining(1000), 0);
    }

    #[test]
    fn slow_down_adds_five_seconds_to_the_interval() {
        let mut poll = poll_after("dc", 5, 900);
        assert_eq!(poll.apply(5, Ok(PollStep::SlowDown)), PollOutcome::Continue);
        assert_eq!(poll.interval(), 10);
        assert_eq!(poll.tick(14), Tick::Wait);
        assert_eq!(poll.tick(15), Tick::Poll);
        assert_eq!(
            poll.apply(15, Ok(PollStep::SlowDown)),
            PollOutcome::Continue
        );
        assert_eq!(poll.interval(), 15);
    }

    #[test]
    fn the_code_expires_locally_at_expires_in() {
        let poll = poll_after("dc", 5, 900);
        assert_eq!(poll.tick(899), Tick::Poll);
        assert_eq!(poll.tick(900), Tick::Expired);
    }

    #[test]
    fn terminal_poll_answers_end_the_flow() {
        let mut poll = poll_after("dc", 5, 900);
        assert_eq!(poll.apply(5, Ok(PollStep::Denied)), PollOutcome::Denied);
        assert_eq!(poll.apply(5, Ok(PollStep::Expired)), PollOutcome::Expired);
        let tokens = oauth("a", "r", 28_800, 15_638_400);
        assert_eq!(
            poll.apply(5, Ok(PollStep::Done(tokens.clone()))),
            PollOutcome::Done(tokens)
        );
        assert!(matches!(
            poll.apply(5, Err(AuthError::Rejected("incorrect_device_code".into()))),
            PollOutcome::Failed(AuthError::Rejected(_))
        ));
    }

    #[test]
    fn a_few_poll_failures_in_a_row_are_survived_but_not_many() {
        let mut poll = poll_after("dc", 5, 900);
        let blip = || Err(AuthError::Network("timed out".into()));
        assert_eq!(poll.apply(5, blip()), PollOutcome::Continue);
        assert_eq!(poll.apply(10, blip()), PollOutcome::Continue);
        // A good answer resets the count.
        assert_eq!(poll.apply(15, Ok(PollStep::Pending)), PollOutcome::Continue);
        assert_eq!(poll.apply(20, blip()), PollOutcome::Continue);
        assert_eq!(poll.apply(25, blip()), PollOutcome::Continue);
        assert!(matches!(
            poll.apply(30, blip()),
            PollOutcome::Failed(AuthError::Network(_))
        ));
    }

    #[test]
    fn browser_errors_map_to_dialog_errors() {
        let error = browser_error(AuthError::Rejected("access_denied".into()));
        assert!(matches!(error, SignInError::Other(_)));
        assert!(matches!(
            browser_error(AuthError::Network("down".into())),
            SignInError::Network(_)
        ));
    }

    // ---- browser sign-in: locks, write-back, host guard ----

    /// A keychain double that can fail reads and writes on demand.
    struct FlakyStore {
        inner: MemoryStore,
        fail_gets: std::sync::atomic::AtomicUsize,
        fail_sets: std::sync::atomic::AtomicUsize,
    }

    impl FlakyStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_gets: 0.into(),
                fail_sets: 0.into(),
            }
        }
    }

    fn take(counter: &std::sync::atomic::AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }

    impl SecretStore for FlakyStore {
        fn get(&self, id: &str) -> Result<Option<String>, SecretError> {
            if take(&self.fail_gets) {
                return Err(SecretError::Failed("locked".into()));
            }
            self.inner.get(id)
        }
        fn set(&self, id: &str, secret: &str) -> Result<(), SecretError> {
            if take(&self.fail_sets) {
                return Err(SecretError::Failed("locked".into()));
            }
            self.inner.set(id, secret)
        }
        fn delete(&self, id: &str) -> Result<(), SecretError> {
            self.inner.delete(id)
        }
    }

    fn secret_of(store: &dyn SecretStore, id: &str) -> StoredSecret {
        forge::auth::parse_secret(&store.get(id).unwrap().unwrap())
    }

    #[test]
    fn a_rejected_refresh_after_another_process_renewed_uses_its_tokens() {
        let store = MemoryStore::new();
        store
            .set("xp1", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        // Another process (not taking our lock) uses ref-1 between our read
        // and our refresh: GitHub then refuses ref-1 for us.
        let refresh = |token: &str| {
            assert_eq!(token, "ref-1");
            store
                .set(
                    "xp1",
                    &oauth("other", "ref-2", 28_800, 15_638_400).to_secret(),
                )
                .unwrap();
            Err(AuthError::Rejected("bad_refresh_token".into()))
        };
        assert_eq!(
            access_token_with(&store, "xp1", NOW, &refresh),
            Ok("other".to_owned())
        );
    }

    #[test]
    fn a_rejected_refresh_after_another_process_renewed_refreshes_once_more_if_needed() {
        let store = MemoryStore::new();
        store
            .set("xp2", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        let calls = Mutex::new(Vec::new());
        let refresh = |token: &str| {
            calls.lock().unwrap().push(token.to_owned());
            if token == "ref-1" {
                // The other process' result is itself about to end.
                store
                    .set("xp2", &oauth("other", "ref-2", 20, 1_000_000).to_secret())
                    .unwrap();
                Err(AuthError::Rejected("bad_refresh_token".into()))
            } else {
                Ok(oauth("third", "ref-3", 28_800, 15_638_400))
            }
        };
        assert_eq!(
            access_token_with(&store, "xp2", NOW, &refresh),
            Ok("third".to_owned())
        );
        assert_eq!(*calls.lock().unwrap(), vec!["ref-1", "ref-2"]);
        assert_eq!(
            secret_of(&store, "xp2"),
            StoredSecret::OAuth(oauth("third", "ref-3", 28_800, 15_638_400))
        );
    }

    #[test]
    fn a_rejection_with_an_unchanged_refresh_token_still_needs_a_reconnect() {
        let store = MemoryStore::new();
        store
            .set("xp3", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        let refused = |_: &str| Err(AuthError::Rejected("bad_refresh_token".into()));
        assert_eq!(
            access_token_with(&store, "xp3", NOW, &refused),
            Err(TokenError::NeedsReconnect)
        );
    }

    #[test]
    fn a_failed_get_before_the_write_back_does_not_skip_it() {
        let store = FlakyStore::new();
        store
            .set("wb1", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        let refresh = |_: &str| {
            // The write-back's own `get` fails once.
            store.fail_gets.store(1, Ordering::SeqCst);
            Ok(oauth("new", "ref-2", 28_800, 15_638_400))
        };
        assert_eq!(
            access_token_with(&store, "wb1", NOW, &refresh),
            Ok("new".to_owned())
        );
        assert_eq!(
            secret_of(&store, "wb1"),
            StoredSecret::OAuth(oauth("new", "ref-2", 28_800, 15_638_400))
        );
    }

    #[test]
    fn rotated_tokens_survive_two_failed_writes_until_a_write_succeeds() {
        let store = FlakyStore::new();
        let stale = oauth("old", "ref-1", 10, 1_000_000).to_secret();
        store.set("wb2", &stale).unwrap();
        let refresh = |_: &str| {
            store.fail_sets.store(2, Ordering::SeqCst);
            Ok(oauth("new", "ref-2", 28_800, 15_638_400))
        };
        assert_eq!(
            access_token_with(&store, "wb2", NOW, &refresh),
            Ok("new".to_owned())
        );
        // The keychain still has the dead secret...
        assert_eq!(store.get("wb2").unwrap().as_deref(), Some(stale.as_str()));
        // ...but the next call uses the rotated tokens without refreshing.
        assert_eq!(token_of(&store.inner, "wb2"), Ok("new".to_owned()));
        // A later successful write (Reconnect) clears the in-memory copy.
        store_secret(
            &store,
            "wb2",
            &oauth("fresh", "ref-9", 28_800, 15_638_400).to_secret(),
        )
        .unwrap();
        assert_eq!(token_of(&store, "wb2"), Ok("fresh".to_owned()));
        // Removing the account clears it too.
        store.fail_sets.store(2, Ordering::SeqCst);
        let refresh = |_: &str| Ok(oauth("again", "ref-3", 28_800, 15_638_400));
        store
            .inner
            .set("wb2", &oauth("x", "r", 10, 1_000_000).to_secret())
            .unwrap();
        assert_eq!(
            access_token_with(&store, "wb2", NOW, &refresh),
            Ok("again".to_owned())
        );
        forget_secret(&store, "wb2").unwrap();
        assert_eq!(token_of(&store, "wb2"), Err(TokenError::Missing));
    }

    #[test]
    fn a_refresh_does_not_overwrite_a_secret_written_meanwhile() {
        let store = MemoryStore::new();
        store
            .set("cw1", &oauth("old", "ref-1", 10, 1_000_000).to_secret())
            .unwrap();
        let reconnected = oauth("reconnected", "ref-r", 28_800, 15_638_400);
        // Reconnect stores fresh tokens while our refresh call is in flight.
        let refresh = |_: &str| {
            store.set("cw1", &reconnected.to_secret()).unwrap();
            Ok(oauth("renewed", "ref-2", 28_800, 15_638_400))
        };
        assert_eq!(
            access_token_with(&store, "cw1", NOW, &refresh),
            Ok("reconnected".to_owned())
        );
        assert_eq!(secret_of(&store, "cw1"), StoredSecret::OAuth(reconnected));
    }

    #[test]
    fn writes_and_removals_wait_for_a_refresh_in_progress_in_another_process() {
        // A second process holds the account's lock file.
        let dir = lock_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(lock_file_name("fl1"));
        let other_process = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .unwrap();
        other_process.lock().unwrap();
        let store = Arc::new(MemoryStore::new());
        store.set("fl1", "old").unwrap();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (store, done) = (store.clone(), done.clone());
            std::thread::spawn(move || {
                store_secret(&*store, "fl1", "new").unwrap();
                done.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !done.load(Ordering::SeqCst),
            "the write must wait for the lock"
        );
        assert_eq!(store.get("fl1").unwrap().as_deref(), Some("old"));
        other_process.unlock().unwrap();
        writer.join().unwrap();
        assert_eq!(store.get("fl1").unwrap().as_deref(), Some("new"));

        other_process.lock().unwrap();
        let remover = {
            let store = store.clone();
            std::thread::spawn(move || forget_secret(&*store, "fl1").unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(store.get("fl1").unwrap().as_deref(), Some("new"));
        other_process.unlock().unwrap();
        remover.join().unwrap();
        assert_eq!(store.get("fl1").unwrap(), None);
    }

    #[test]
    fn lock_file_names_cannot_escape_the_directory() {
        assert_eq!(lock_file_name("1a2b-c_d"), "1a2b-c_d.lock");
        assert_eq!(lock_file_name("../../etc/x"), "______etc_x.lock");
    }

    #[test]
    fn an_oauth_secret_is_never_used_for_another_host() {
        let store = MemoryStore::new();
        store
            .set("hg1", &oauth("acc", "ref", 28_800, 15_638_400).to_secret())
            .unwrap();
        let ok = |allowed| credential_with(&store, "hg1", allowed, NOW, &never_refresh);
        assert_eq!(ok(true), Ok(("acc".to_owned(), TokenKind::OAuth)));
        assert_eq!(ok(false), Err(TokenError::NeedsReconnect));
        // A personal access token is not tied to the OAuth app.
        store.set("hg2", "ghp_raw").unwrap();
        assert_eq!(
            credential_with(&store, "hg2", false, NOW, &never_refresh),
            Ok(("ghp_raw".to_owned(), TokenKind::Pat))
        );
    }

    #[test]
    fn a_tampered_base_url_gets_no_oauth_token() {
        let store = MemoryStore::new();
        // `access_token` uses the real clock.
        let real_now = forge::auth::now_unix();
        let live = OAuthTokens {
            access: "acc".into(),
            refresh: "ref".into(),
            access_expires_at: real_now + 28_800,
            refresh_expires_at: real_now + 15_638_400,
        };
        store.set("hg3", &live.to_secret()).unwrap();
        let mut account = record("hg3", "https://github.com");
        account.kind = "github".to_owned();
        assert_eq!(access_token(&store, &account), Ok("acc".to_owned()));
        for evil in [
            "https://evil.example",
            "http://github.com",
            "https://github.com:8443",
            "https://github.com/sub",
            "https://github.com.evil.example",
        ] {
            account.base_url = evil.to_owned();
            assert_eq!(
                access_token(&store, &account),
                Err(TokenError::NeedsReconnect),
                "{evil}"
            );
        }
        // Another service kind on github.com's address is not GitHub's app either.
        account.base_url = "https://github.com".to_owned();
        account.kind = "gitlab".to_owned();
        assert_eq!(
            access_token(&store, &account),
            Err(TokenError::NeedsReconnect)
        );
    }

    #[test]
    fn askpass_host_rule_is_github_com_over_https() {
        let key = |scheme, host, port| HostKey::new(scheme, host, port);
        assert!(oauth_allowed_for_host(&key("https", "github.com", None)));
        assert!(!oauth_allowed_for_host(&key("http", "github.com", None)));
        assert!(!oauth_allowed_for_host(&key("https", "evil.example", None)));
        assert!(!oauth_allowed_for_host(&key(
            "https",
            "github.com",
            Some(8443)
        )));
    }

    #[test]
    fn expiry_arithmetic_saturates() {
        let tokens = forge::auth::OAuthTokens {
            access: "a".into(),
            refresh: "r".into(),
            access_expires_at: u64::MAX,
            refresh_expires_at: u64::MAX,
        };
        assert!(!needs_refresh(&tokens, NOW));
        let mut poll = poll_after("dc", u64::MAX, u64::MAX);
        assert_eq!(
            poll.apply(u64::MAX, Ok(PollStep::Pending)),
            PollOutcome::Continue
        );
        assert_eq!(poll.apply(1, Ok(PollStep::SlowDown)), PollOutcome::Continue);
    }

    /// Serves `{"login":"mira-k"}` with the GitHub scopes header to every request.
    fn stub_github() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = r#"{"login":"mira-k","name":null,"avatar_url":null}"#;
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-OAuth-Scopes: repo, read:user, workflow\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(out.as_bytes());
            }
        });
        base
    }

    #[test]
    fn check_account_reports_expiring_from_the_refresh_token() {
        let base = stub_github();
        let store: StoreResult = Ok(Arc::new(MemoryStore::new()));
        let mut account = record("ck1", &base);
        account.kind = "github".to_owned();
        account.method = AccountMethod::Browser;
        let secret = |refresh_days: u64| {
            OAuthTokens {
                access: "acc".into(),
                refresh: "ref".into(),
                access_expires_at: forge::auth::now_unix() + 28_800,
                refresh_expires_at: forge::auth::now_unix() + refresh_days * SECS_PER_DAY,
            }
            .to_secret()
        };
        let check = |store: &StoreResult| {
            check_account_with(store, None, &account, &|| true, &Client::new(), true)
        };
        store.as_ref().unwrap().set("ck1", &secret(5)).unwrap();
        assert_eq!(check(&store).status, AccountStatus::Expiring { days: 5 });
        // Plenty of refresh lifetime left: plain Connected.
        store.as_ref().unwrap().set("ck1", &secret(100)).unwrap();
        assert_eq!(check(&store).status, AccountStatus::Connected);
        // The same secret on an instance that may not receive it is Invalid.
        assert_eq!(
            check_account_with(&store, None, &account, &|| true, &Client::new(), false).status,
            AccountStatus::Invalid(None)
        );
    }

    #[test]
    fn account_ids_are_unique() {
        let a = new_account_id();
        let b = new_account_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
