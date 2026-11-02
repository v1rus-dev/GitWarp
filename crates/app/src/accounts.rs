//! Hosting-service accounts, independent of the UI: the secret store global,
//! the account status check, and the rules of the Add / Reconnect dialog.
//!
//! Everything here is plain data and blocking functions, so it is unit tested
//! without a window. The Settings → Accounts page and the dialog call these on
//! the background executor.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use forge::api::{ApiError, Client, Probe, TokenCheck, TokenKind};
use forge::secrets::{KeychainStore, SecretError, SecretStore};
use forge::{Instance, ServiceKind};
use gpui::{App, Global};

use crate::settings::{AccountRecord, LEGACY_ACCOUNT_ID_PREFIX};

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
pub(crate) fn self_hosted_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::GitHub => "GitHub Enterprise Server",
        ServiceKind::GitLab => "GitLab self-managed",
        ServiceKind::Gitea => "Gitea / Forgejo",
        ServiceKind::Bitbucket => "Bitbucket",
        ServiceKind::AzureDevOps => "Azure DevOps Server",
    }
}

/// The card text under a service on the first step.
pub(crate) fn service_blurb(kind: ServiceKind) -> String {
    match kind {
        ServiceKind::Gitea => "Codeberg, Gitea and Forgejo servers".to_owned(),
        ServiceKind::AzureDevOps => "Azure DevOps Services and Server".to_owned(),
        other => format!("{} or your own server", public_host(other)),
    }
}

/// Whether the service has a browser sign-in flow for desktop apps. Only
/// shown as an unavailable option today: OAuth lands in a later step.
pub(crate) fn has_oauth(kind: ServiceKind) -> bool {
    matches!(
        kind,
        ServiceKind::GitHub | ServiceKind::GitLab | ServiceKind::Gitea
    )
}

/// The permissions GitWarp asks for in a token.
pub(crate) fn required_permissions(kind: ServiceKind) -> &'static [&'static str] {
    match kind {
        ServiceKind::GitHub => &["repo", "read:user", "workflow"],
        ServiceKind::GitLab => &["api", "read_repository", "write_repository"],
        ServiceKind::Gitea => &["repository: read and write", "user: read and write"],
        ServiceKind::Bitbucket => &[
            "Repositories: read and write",
            "Pull requests: read and write",
        ],
        ServiceKind::AzureDevOps => &["Code: read and write"],
    }
}

/// Where in the service's settings tokens are created.
pub(crate) fn token_where(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::GitHub => "Settings → Developer settings → Personal access tokens",
        ServiceKind::GitLab => "Preferences → Access tokens",
        ServiceKind::Gitea => "Settings → Applications → Generate new token",
        ServiceKind::Bitbucket => "Personal settings → API tokens (Bitbucket API token)",
        ServiceKind::AzureDevOps => "User settings → Personal access tokens",
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
    let kind = kind_of(account).ok_or_else(|| format!("unknown service `{}`", account.kind))?;
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

/* ------------------------------ account status --------------------------- */

/// What the Accounts page shows for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountStatus {
    Checking,
    Connected,
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
    store.set(id, &password)?;
    if !is_current() {
        store.delete(id)?;
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
    let store = match store {
        Ok(store) => store,
        Err(error) => return CheckOutcome::status(AccountStatus::Unchecked(error.to_string())),
    };
    let instance = match instance_of(account) {
        Ok(instance) => instance,
        Err(error) => return CheckOutcome::status(AccountStatus::Unreachable(Some(error))),
    };
    let mut secret = match store.get(&account.id) {
        Ok(secret) => secret,
        Err(error) => return CheckOutcome::status(AccountStatus::Unchecked(error.to_string())),
    };
    if secret.is_none() && account.id.starts_with(LEGACY_ACCOUNT_ID_PREFIX) {
        if let Some(git) = git {
            if let Ok(Some(credential)) = git_core::credential::fill(git, instance.host()) {
                match import_legacy_secret(&**store, &account.id, credential.password, is_current) {
                    Ok(Some(password)) => secret = Some(password),
                    // Removed while the check ran: nothing was written.
                    Ok(None) => return CheckOutcome::status(AccountStatus::Checking),
                    Err(error) => {
                        return CheckOutcome::status(AccountStatus::Unchecked(error.to_string()))
                    }
                }
            }
        }
    }
    let Some(token) = secret else {
        return CheckOutcome::status(AccountStatus::Invalid(Some(
            "No token is stored in the system keychain for this account.".to_owned(),
        )));
    };
    let result = forge::api::current_user(&Client::new(), &instance, &token, TokenKind::Pat);
    outcome_of(&account.login, result)
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
            method: crate::settings::AccountMethod::Token,
            ssh_host: self.ssh_host.clone(),
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
pub(crate) fn continue_label(public: bool, probe: &ProbeState) -> &'static str {
    match probe {
        ProbeState::Checking => "Checking…",
        ProbeState::Detected { .. } => "Continue",
        _ if public => "Continue",
        _ => "Check address",
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
    /// Any other failure; carries the error text.
    Other(String),
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
        Err(ApiError::Forbidden) => Err(SignInError::Permissions(
            required_permissions(kind)
                .iter()
                .map(|p| (*p).to_owned())
                .collect(),
        )),
        Err(error @ (ApiError::Unreachable(_) | ApiError::TlsUntrusted(_))) => {
            Err(SignInError::Network(error.to_string()))
        }
        Err(other) => Err(SignInError::Other(other.to_string())),
    }
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
        }
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
        assert_eq!(continue_label(true, &ProbeState::Idle), "Continue");
        assert_eq!(continue_label(false, &ProbeState::Idle), "Check address");
        assert_eq!(continue_label(false, &ProbeState::Checking), "Checking…");
        assert_eq!(continue_label(false, &detected), "Continue");

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
            token: "secret".to_owned(),
        };
        let record = verified.record("id1".to_owned(), "  ");
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

    #[test]
    fn account_ids_are_unique() {
        let a = new_account_id();
        let b = new_account_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
