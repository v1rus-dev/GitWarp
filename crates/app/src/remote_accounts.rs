//! Remotes ↔ accounts, independent of the UI: which account serves each remote
//! URL, what the sidebar shows for it (chip, "Add account" row, push URL rows),
//! what Account… in the remote's menu does, and the in-memory answer cache of
//! the anonymous "is this repository public" check.
//!
//! Everything here is plain data and pure functions (the cache is the one
//! piece of process state), so it is unit tested without a window. The
//! resolution itself is `forge::resolve_with`; this module feeds it the
//! configured accounts and the remembered choice and shapes the answer.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use forge::{resolve_with, AccountRef, Instance, RemoteRef, RepoCoords, Resolution, ServiceKind};

use crate::accounts::{instance_of, kind_of, public_host};
use crate::network_service::{AccountCredential, HostKey};
use crate::settings::AccountRecord;

/// The URLs of one configured remote, as read from git.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteTargets {
    pub name: String,
    pub fetch_url: String,
    /// `git remote get-url --push --all`: the fetch URL when no `pushurl` is set.
    pub push_urls: Vec<String>,
}

/// The accounts as `forge` sees them. A record whose service or URL does not
/// parse is skipped: it cannot match any remote anyway.
pub fn account_refs(accounts: &[AccountRecord]) -> Vec<AccountRef> {
    accounts
        .iter()
        .filter_map(|account| {
            Some(AccountRef {
                id: account.id.clone(),
                instance: instance_of(account).ok()?,
                login: account.login.clone(),
            })
        })
        .collect()
}

/* ------------------------------ resolution ------------------------------- */

/// Which account serves one URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlAccount {
    /// One account applies: the only match, or the remembered one.
    Linked(String),
    /// Several accounts match and none is remembered.
    Choose(Vec<String>),
    /// No account, on a host of a known public service.
    NoAccount(ServiceKind),
    /// Not a known service and no account instance matches (or not a URL).
    Unknown,
}

/// One URL of a remote, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `host[:port]` for display (the URL itself when it does not parse).
    pub host: String,
    pub account: UrlAccount,
    /// Every account that matches the URL, remembered choice or not.
    pub matching: Vec<String>,
    /// The service of the account or of the known host, for the icon.
    pub service: Option<ServiceKind>,
    /// What to ask the service when there is no account on a known host.
    pub public_probe: Option<PublicProbe>,
}

/// An anonymous "is this repository public" question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicProbe {
    pub instance: Instance,
    pub coords: RepoCoords,
}

impl PublicProbe {
    /// The cache key: instance and `owner/repo`.
    pub fn key(&self) -> String {
        format!(
            "{}|{}/{}",
            self.instance.base_url,
            self.coords.namespace.join("/"),
            self.coords.repo
        )
    }
}

/// Services whose repositories can be checked without a token (see
/// `forge::api::repo_is_public`); the others have no anonymous answer.
fn can_check_public(kind: ServiceKind) -> bool {
    matches!(
        kind,
        ServiceKind::GitHub | ServiceKind::GitLab | ServiceKind::Gitea
    )
}

fn display_host(remote: &RemoteRef) -> String {
    match (remote.port, remote.transport) {
        // The SSH port belongs to sshd; it says nothing about the web host.
        (Some(port), forge::Transport::Https | forge::Transport::Http) => {
            format!("{}:{port}", remote.host)
        }
        _ => remote.host.clone(),
    }
}

/// Resolves one URL against the accounts. `remembered` is the account id the
/// user picked for this remote; an id that no longer matches is ignored.
pub fn resolve_url(refs: &[AccountRef], url: &str, remembered: Option<&str>) -> Target {
    let Some(remote) = RemoteRef::parse(url) else {
        return Target {
            host: url.trim().to_owned(),
            account: UrlAccount::Unknown,
            matching: Vec::new(),
            service: None,
            public_probe: None,
        };
    };
    let host = display_host(&remote);
    let matching: Vec<String> = match resolve_with(refs, &remote, None) {
        Resolution::One(account) => vec![account.id.clone()],
        Resolution::Many(accounts) => accounts.iter().map(|a| a.id.clone()).collect(),
        Resolution::None { .. } => Vec::new(),
    };
    let (account, service, public_probe) = match resolve_with(refs, &remote, remembered) {
        Resolution::One(account) => (
            UrlAccount::Linked(account.id.clone()),
            Some(account.instance.kind),
            None,
        ),
        Resolution::Many(accounts) => {
            // Several accounts on a public instance: the repository may still
            // be public, in which case no choice is needed to read it.
            let probe = accounts.first().and_then(|first| {
                (first.instance.is_public() && can_check_public(first.instance.kind))
                    .then(|| {
                        let coords = first.instance.matches(&remote)?;
                        Some(PublicProbe {
                            instance: first.instance.clone(),
                            coords,
                        })
                    })
                    .flatten()
            });
            (
                UrlAccount::Choose(accounts.iter().map(|a| a.id.clone()).collect()),
                accounts.first().map(|a| a.instance.kind),
                probe,
            )
        }
        Resolution::None {
            known_kind: Some(kind),
        } => {
            let probe = can_check_public(kind)
                .then(|| {
                    let instance =
                        Instance::new(kind, &format!("https://{}", public_host(kind))).ok()?;
                    let coords = instance.matches(&remote)?;
                    Some(PublicProbe { instance, coords })
                })
                .flatten();
            (UrlAccount::NoAccount(kind), Some(kind), probe)
        }
        Resolution::None { known_kind: None } => (UrlAccount::Unknown, None, None),
    };
    Target {
        host,
        account,
        matching,
        service,
        public_probe,
    }
}

/// Whether the remote pushes somewhere else than it fetches from: several
/// push URLs, or one whose host differs from the fetch URL's. A push URL on
/// the same host (another path, or ssh instead of https) is still the same
/// account.
pub fn is_multi_push(targets: &RemoteTargets) -> bool {
    match targets.push_urls.as_slice() {
        [] => false,
        [one] => host_key(one) != host_key(&targets.fetch_url),
        _ => true,
    }
}

fn host_key(url: &str) -> String {
    RemoteRef::parse(url)
        .map(|remote| remote.host)
        .unwrap_or_else(|| url.trim().to_owned())
}

/// Whether two URLs of one remote are the same address, ignoring a trailing
/// slash.
fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// A remote with every URL resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteResolution {
    pub name: String,
    pub multi_push: bool,
    /// The fetch URL is one of the push URLs, so its row would repeat that
    /// push row.
    pub fetch_is_a_push_url: bool,
    pub fetch: Target,
    /// One per push URL; empty unless `multi_push`.
    pub pushes: Vec<Target>,
}

/// Resolves a remote's fetch URL, and each push URL when it has several.
/// `remembered` applies to every URL: it only counts where that account
/// matches the URL's host.
pub fn resolve_remote(
    refs: &[AccountRef],
    targets: &RemoteTargets,
    remembered: Option<&str>,
) -> RemoteResolution {
    let multi_push = is_multi_push(targets);
    RemoteResolution {
        name: targets.name.clone(),
        multi_push,
        fetch_is_a_push_url: multi_push
            && targets
                .push_urls
                .iter()
                .any(|url| same_url(url, &targets.fetch_url)),
        fetch: resolve_url(refs, &targets.fetch_url, remembered),
        pushes: if multi_push {
            targets
                .push_urls
                .iter()
                .map(|url| resolve_url(refs, url, remembered))
                .collect()
        } else {
            Vec::new()
        },
    }
}

/// The remote-name → resolution list for a repository, given the settings'
/// remembered choices for it.
pub fn resolve_all(
    accounts: &[AccountRecord],
    targets: &[RemoteTargets],
    remembered: Option<&std::collections::BTreeMap<String, String>>,
) -> Vec<RemoteResolution> {
    let refs = account_refs(accounts);
    targets
        .iter()
        .map(|t| {
            let id = remembered.and_then(|m| m.get(&t.name)).map(String::as_str);
            resolve_remote(&refs, t, id)
        })
        .collect()
}

/* ------------------------------ network jobs ----------------------------- */

/// Which URL of a remote a job contacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The fetch URL: fetch, pull.
    Fetch,
    /// Every push URL: push, push tags, delete a remote branch.
    Push,
}

/// A remote a job talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUse {
    pub remote: String,
    pub direction: Direction,
}

/// The accounts a network job can answer git's prompts with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobCredentials {
    /// One entry per host git will contact where an account applies.
    pub accounts: HashMap<HostKey, AccountCredential>,
    /// Remotes with several matching accounts and no remembered choice, in
    /// the job's order: an interactive job asks before it starts.
    pub choose: Vec<String>,
}

/// The accounts for `uses`, from the URLs git will contact.
///
/// Only `https://` URLs count, and `http://` only when the account's own
/// instance is `http://` too: a token is never offered over a scheme its
/// instance does not use. A host is keyed by scheme, host and port, so a token
/// can reach no other server than the account's instance. When two remotes of
/// one job map the same host to different accounts the first remote wins: git
/// asks about the host, not the remote.
pub fn job_credentials(
    accounts: &[AccountRecord],
    targets: &[RemoteTargets],
    uses: &[RemoteUse],
    remembered: &std::collections::BTreeMap<String, String>,
) -> JobCredentials {
    let refs = account_refs(accounts);
    let mut result = JobCredentials::default();
    for used in uses {
        let Some(target) = targets.iter().find(|t| t.name == used.remote) else {
            continue;
        };
        let urls: &[String] = match used.direction {
            Direction::Fetch => std::slice::from_ref(&target.fetch_url),
            Direction::Push => &target.push_urls,
        };
        for url in urls {
            let resolved =
                resolve_url(&refs, url, remembered.get(&used.remote).map(String::as_str));
            match resolved.account {
                UrlAccount::Linked(id) => {
                    if let Some((key, credential)) = credential_for(accounts, &id, url) {
                        result.accounts.entry(key).or_insert(credential);
                    }
                }
                UrlAccount::Choose(_) => {
                    if !result.choose.contains(&used.remote) {
                        result.choose.push(used.remote.clone());
                    }
                }
                UrlAccount::NoAccount(_) | UrlAccount::Unknown => {}
            }
        }
    }
    result
}

/// An account a job answered prompts with, for the "rejected the token" toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsedAccount {
    pub account_id: String,
    /// `host[:port]`.
    pub host: String,
    pub label: String,
}

/// The accounts of `credentials`, ordered by host.
pub fn used_accounts(credentials: &JobCredentials, accounts: &[AccountRecord]) -> Vec<UsedAccount> {
    let mut used: Vec<UsedAccount> = credentials
        .accounts
        .iter()
        .filter_map(|(key, credential)| {
            let label = label_of(accounts, &credential.account_id)?;
            let url = key.url();
            Some(UsedAccount {
                account_id: credential.account_id.clone(),
                host: url
                    .split_once("://")
                    .map_or(url.clone(), |(_, h)| h.to_owned()),
                label: label.to_owned(),
            })
        })
        .collect();
    used.sort_by(|a, b| a.host.cmp(&b.host));
    used
}

/// The account whose token a failed job most likely used: the one whose host
/// git's error names, else the first.
pub fn failed_account<'a>(used: &'a [UsedAccount], error_text: &str) -> Option<&'a UsedAccount> {
    let lower = error_text.to_lowercase();
    used.iter()
        .find(|account| lower.contains(&account.host.to_lowercase()))
        .or_else(|| used.first())
}

/// The host and account of one push URL, for the push result toast.
pub fn push_target_info(
    accounts: &[AccountRecord],
    url: &str,
    remembered: Option<&str>,
) -> (String, Option<(String, String)>) {
    let target = resolve_url(&account_refs(accounts), url, remembered);
    let account = match &target.account {
        UrlAccount::Linked(id) => label_of(accounts, id).map(|l| (id.clone(), l.to_owned())),
        _ => None,
    };
    (target.host, account)
}

fn credential_for(
    accounts: &[AccountRecord],
    id: &str,
    url: &str,
) -> Option<(HostKey, AccountCredential)> {
    let account = accounts.iter().find(|a| a.id == id)?;
    let remote = RemoteRef::parse(url)?;
    let scheme = match remote.transport {
        forge::Transport::Https => "https",
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

/* -------------------------------- display -------------------------------- */

/// The answer of the public check for one repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicState {
    /// Not asked yet or still running for the first time: the sidebar shows
    /// nothing, so a chip never flickers from one state to another.
    Pending,
    Public,
    /// Private, unknown to the service, or the check failed or is unknown.
    NotPublic,
}

/// The small label at the trailing edge of a remote row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub text: String,
    /// Drawn in the warning colour with a warning icon.
    pub warn: bool,
    pub tooltip: String,
}

/// The "Add account for <host>" row under a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddAccount {
    pub kind: ServiceKind,
    pub host: String,
    /// "Add account for <host>".
    pub label: String,
    pub tooltip: String,
}

/// One `push → <host>` or `fetch → <host>` row under a multi-push remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRow {
    pub host: String,
    pub label: String,
    pub service: Option<ServiceKind>,
    pub chip: Option<Chip>,
}

/// What the sidebar shows for one remote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteDisplay {
    /// The service of the row's icon; `None` keeps the generic cloud.
    pub service: Option<ServiceKind>,
    pub chip: Option<Chip>,
    /// Tooltip of the remote row.
    pub tooltip: Option<String>,
    pub add_account: Option<AddAccount>,
    /// The `fetch → <host>` row of a multi-push remote, first.
    pub fetch: Option<PushRow>,
    pub pushes: Vec<PushRow>,
}

fn label_of<'a>(accounts: &'a [AccountRecord], id: &str) -> Option<&'a str> {
    accounts
        .iter()
        .find(|a| a.id == id)
        .map(AccountRecord::display_label)
}

/// The neutral chip of a repository that needs no account to be read.
fn public_chip(host: &str) -> Chip {
    Chip {
        text: ui::t!("sidebar.remote.chip.public").to_string(),
        warn: false,
        tooltip: ui::t!("sidebar.remote.tip.public", host = host).to_string(),
    }
}

/// The chip of one URL, and whether it also earns the "Add account" row.
fn target_chip(
    target: &Target,
    accounts: &[AccountRecord],
    public: &dyn Fn(&PublicProbe) -> PublicState,
) -> (Option<Chip>, bool) {
    let host = &target.host;
    match &target.account {
        UrlAccount::Linked(id) => {
            let chip = label_of(accounts, id).map(|label| Chip {
                text: label.to_owned(),
                warn: false,
                tooltip: ui::t!("sidebar.remote.tip.account", host = host, label = label)
                    .to_string(),
            });
            (chip, false)
        }
        UrlAccount::Choose(ids) => {
            // A public repository needs no choice: show the neutral chip, like
            // for a remote without accounts. Account… still opens the picker.
            // Not public, or nothing to ask: the warning stays.
            let state = target
                .public_probe
                .as_ref()
                .map_or(PublicState::NotPublic, public);
            match state {
                PublicState::Pending => (None, false),
                PublicState::Public => (Some(public_chip(host)), false),
                PublicState::NotPublic => (
                    Some(Chip {
                        text: ui::t!("sidebar.remote.chip.choose").to_string(),
                        warn: true,
                        tooltip: ui::i18n::t_count(
                            "sidebar.remote.tip.choose",
                            ids.len() as u64,
                            &[("host", host)],
                        )
                        .to_string(),
                    }),
                    false,
                ),
            }
        }
        UrlAccount::NoAccount(_) => {
            // No account for a known service. Signing in is optional — a public
            // repository needs none, and the user may only want to browse or
            // fetch over SSH — so this is never flagged. A public repository
            // still gets the neutral chip; everything else shows nothing.
            // Account… in the remote's menu is still the way to add one.
            let state = target
                .public_probe
                .as_ref()
                .map_or(PublicState::NotPublic, public);
            match state {
                PublicState::Public => (Some(public_chip(host)), false),
                PublicState::Pending | PublicState::NotPublic => (None, false),
            }
        }
        UrlAccount::Unknown => (None, false),
    }
}

/// `a`, `a and b`, `a, b and c`, in the active language.
fn join_hosts(hosts: &[&str]) -> String {
    match hosts {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => ui::t!(
            "sidebar.remote.join_and",
            init = init.join(&ui::t!("sidebar.remote.list_sep")),
            last = last
        )
        .to_string(),
    }
}

/// What the sidebar shows for `resolution`. `public` answers the anonymous
/// check from the cache.
pub fn display(
    resolution: &RemoteResolution,
    accounts: &[AccountRecord],
    public: &dyn Fn(&PublicProbe) -> PublicState,
) -> RemoteDisplay {
    let name = &resolution.name;
    if resolution.multi_push {
        let hosts: Vec<&str> = resolution.pushes.iter().map(|t| t.host.as_str()).collect();
        let row = |target: &Target, key: &'static str| PushRow {
            host: target.host.clone(),
            label: match key {
                "fetch" => ui::t!("sidebar.remote.row.fetch", host = target.host).to_string(),
                _ => ui::t!("sidebar.remote.row.push", host = target.host).to_string(),
            },
            service: target.service,
            chip: target_chip(target, accounts, public).0,
        };
        return RemoteDisplay {
            service: None,
            chip: Some(Chip {
                text: ui::i18n::t_count(
                    "sidebar.remote.chip.push_urls",
                    resolution.pushes.len() as u64,
                    &[],
                )
                .to_string(),
                warn: false,
                tooltip: ui::t!("sidebar.remote.tip.pushes_to", hosts = join_hosts(&hosts))
                    .to_string(),
            }),
            tooltip: Some(
                ui::i18n::t_count(
                    "sidebar.remote.tip.remote_pushes",
                    resolution.pushes.len() as u64,
                    &[("name", name)],
                )
                .to_string(),
            ),
            add_account: None,
            // The fetch row only when it says something the push rows do not.
            fetch: (!resolution.fetch_is_a_push_url).then(|| row(&resolution.fetch, "fetch")),
            pushes: resolution.pushes.iter().map(|t| row(t, "push")).collect(),
        };
    }
    let target = &resolution.fetch;
    let (chip, add) = target_chip(target, accounts, public);
    RemoteDisplay {
        service: target.service,
        chip,
        tooltip: (!matches!(target.account, UrlAccount::Unknown)).then(|| {
            ui::t!("sidebar.remote.tip.remote", name = name, host = target.host).to_string()
        }),
        add_account: match (&target.account, add) {
            (UrlAccount::NoAccount(kind), true) => Some(AddAccount {
                kind: *kind,
                host: target.host.clone(),
                label: ui::t!("sidebar.remote.row.add_account", host = target.host).to_string(),
                tooltip: ui::t!(
                    "sidebar.remote.tip.add_account",
                    host = target.host,
                    remote = name
                )
                .to_string(),
            }),
            _ => None,
        },
        fetch: None,
        pushes: Vec::new(),
    }
}

/// The public checks the display is waiting for, each question once.
pub fn public_probes(resolutions: &[RemoteResolution]) -> Vec<PublicProbe> {
    let mut seen = HashSet::new();
    let mut probes = Vec::new();
    for resolution in resolutions {
        let mut targets = vec![&resolution.fetch];
        targets.extend(&resolution.pushes);
        for target in targets {
            if let Some(probe) = &target.public_probe {
                if seen.insert(probe.key()) {
                    probes.push(probe.clone());
                }
            }
        }
    }
    probes
}

/* ------------------------- Account… in the menu -------------------------- */

/// What Account… does for a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountAction {
    /// Several accounts match: the picker, with the current choice preselected.
    Pick {
        host: String,
        ids: Vec<String>,
        current: Option<String>,
    },
    /// No account on a known service: the Add account dialog.
    Add { kind: ServiceKind, host: String },
    /// Exactly one account matches.
    AlreadyOne { host: String, label: String },
    /// Not a known service and no account for it: Settings is the only way.
    NoService { host: String },
}

/// The behaviour of Account… for a single-push remote.
pub fn account_action(resolution: &RemoteResolution, accounts: &[AccountRecord]) -> AccountAction {
    let target = &resolution.fetch;
    let host = target.host.clone();
    match target.matching.as_slice() {
        [] => match target.account {
            UrlAccount::NoAccount(kind) => AccountAction::Add { kind, host },
            _ => AccountAction::NoService { host },
        },
        [one] => AccountAction::AlreadyOne {
            label: label_of(accounts, one).unwrap_or_default().to_owned(),
            host,
        },
        many => AccountAction::Pick {
            host,
            ids: many.to_vec(),
            current: match &target.account {
                UrlAccount::Linked(id) => Some(id.clone()),
                _ => None,
            },
        },
    }
}

/// The choice to remember after `new_id` was added for the remote at `url`:
/// the new account when it joined others on the same host, nothing when it is
/// the only match (it is picked up on its own).
pub fn choice_after_add(accounts: &[AccountRecord], url: &str, new_id: &str) -> Option<String> {
    let refs = account_refs(accounts);
    let remote = RemoteRef::parse(url)?;
    match resolve_with(&refs, &remote, None) {
        Resolution::Many(found) if found.iter().any(|a| a.id == new_id) => Some(new_id.to_owned()),
        _ => None,
    }
}

/* ------------------------------ public cache ----------------------------- */

/// How long a failed or unknown check waits before it may be asked again.
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
enum Slot {
    /// Being asked; `retry` when an earlier attempt failed (the display
    /// already shows "not public", so nothing flickers while it runs).
    InFlight {
        retry: bool,
    },
    Done(bool),
    /// The last attempt failed or had no answer, at this time.
    Failed(Instant),
}

fn cache() -> &'static Mutex<HashMap<String, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// The cached answer for `key`; `Pending` while it is unknown or running for
/// the first time.
pub fn cached_public(key: &str) -> PublicState {
    match cache().lock().ok().and_then(|c| c.get(key).copied()) {
        Some(Slot::Done(true)) => PublicState::Public,
        Some(Slot::Done(false) | Slot::Failed(_) | Slot::InFlight { retry: true }) => {
            PublicState::NotPublic
        }
        Some(Slot::InFlight { retry: false }) | None => PublicState::Pending,
    }
}

/// Marks `key` as being asked. `true` when the caller must run the request:
/// an answer is asked once per process, a failed attempt again after a minute.
pub fn claim_public_check(key: &str) -> bool {
    claim_at(key, Instant::now())
}

fn claim_at(key: &str, now: Instant) -> bool {
    let Ok(mut cache) = cache().lock() else {
        return false;
    };
    match cache.get(key).copied() {
        None => {
            cache.insert(key.to_owned(), Slot::InFlight { retry: false });
            true
        }
        Some(Slot::Failed(at)) if now.saturating_duration_since(at) >= RETRY_AFTER => {
            cache.insert(key.to_owned(), Slot::InFlight { retry: true });
            true
        }
        Some(_) => false,
    }
}

/// Records the answer; a late one replaces what a timeout showed.
pub fn finish_public_check(key: &str, public: bool) {
    if let Ok(mut cache) = cache().lock() {
        cache.insert(key.to_owned(), Slot::Done(public));
    }
}

/// The request failed or the service gave no usable answer: the slot is
/// released (nothing is remembered as "private"), and the question may be
/// asked again after a minute.
pub fn release_public_check(key: &str) {
    release_at(key, Instant::now());
}

fn release_at(key: &str, now: Instant) {
    if let Ok(mut cache) = cache().lock() {
        // An answer that arrived meanwhile stays.
        if !matches!(cache.get(key), Some(Slot::Done(_))) {
            cache.insert(key.to_owned(), Slot::Failed(now));
        }
    }
}

/// The request took too long: show "not public" for now and treat it as a
/// failed attempt. Does nothing when the answer already arrived.
pub fn expire_public_check(key: &str) {
    if let Ok(mut cache) = cache().lock() {
        if matches!(cache.get(key), Some(Slot::InFlight { .. })) {
            cache.insert(key.to_owned(), Slot::Failed(Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AccountMethod;

    fn account(id: &str, kind: &str, url: &str, login: &str) -> AccountRecord {
        AccountRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            base_url: url.to_owned(),
            login: login.to_owned(),
            label: login.to_owned(),
            method: AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    fn github(id: &str, login: &str) -> AccountRecord {
        account(id, "github", "https://github.com", login)
    }

    fn targets(name: &str, fetch: &str, push: &[&str]) -> RemoteTargets {
        RemoteTargets {
            name: name.to_owned(),
            fetch_url: fetch.to_owned(),
            push_urls: if push.is_empty() {
                vec![fetch.to_owned()]
            } else {
                push.iter().map(|s| (*s).to_owned()).collect()
            },
        }
    }

    fn resolved(
        accounts: &[AccountRecord],
        t: &RemoteTargets,
        remembered: Option<&str>,
    ) -> RemoteResolution {
        resolve_remote(&account_refs(accounts), t, remembered)
    }

    fn shown(
        accounts: &[AccountRecord],
        t: &RemoteTargets,
        remembered: Option<&str>,
        public: PublicState,
    ) -> RemoteDisplay {
        display(&resolved(accounts, t, remembered), accounts, &|_| public)
    }

    const REPO: &str = "https://github.com/acme/app.git";

    #[test]
    fn one_matching_account_is_used() {
        let accounts = [github("a1", "mira")];
        let t = targets("origin", REPO, &[]);
        let r = resolved(&accounts, &t, None);
        assert_eq!(r.fetch.account, UrlAccount::Linked("a1".into()));
        assert_eq!(r.fetch.service, Some(ServiceKind::GitHub));
        let d = shown(&accounts, &t, None, PublicState::Pending);
        let chip = d.chip.unwrap();
        assert_eq!(chip.text, "mira");
        assert!(!chip.warn);
        assert_eq!(chip.tooltip, "github.com · account mira");
        assert_eq!(d.add_account, None);
    }

    #[test]
    fn the_chip_shows_the_label_not_the_login() {
        let mut work = github("a1", "mira-k");
        work.label = "Acme work".into();
        let d = shown(
            &[work],
            &targets("origin", REPO, &[]),
            None,
            PublicState::Pending,
        );
        assert_eq!(d.chip.unwrap().text, "Acme work");
    }

    #[test]
    fn several_accounts_ask_to_choose_until_one_is_remembered() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = targets("origin", REPO, &[]);
        let r = resolved(&accounts, &t, None);
        assert_eq!(
            r.fetch.account,
            UrlAccount::Choose(vec!["a1".into(), "a2".into()])
        );
        // While the public check runs nothing is shown; a repository that is
        // not public (or unknown) keeps the warning.
        assert_eq!(shown(&accounts, &t, None, PublicState::Pending).chip, None);
        let chip = shown(&accounts, &t, None, PublicState::NotPublic)
            .chip
            .unwrap();
        assert_eq!(chip.text, "choose account");
        assert!(chip.warn);
        assert_eq!(
            chip.tooltip,
            "2 accounts on github.com — pick one from the remote’s menu: Account…"
        );

        let r = resolved(&accounts, &t, Some("a2"));
        assert_eq!(r.fetch.account, UrlAccount::Linked("a2".into()));
        // Both still match: the picker has something to pick from.
        assert_eq!(r.fetch.matching, ["a1", "a2"]);
        assert_eq!(
            shown(&accounts, &t, Some("a2"), PublicState::Pending)
                .chip
                .unwrap()
                .text,
            "mira-at-acme"
        );
    }

    #[test]
    fn several_accounts_on_a_public_repository_show_the_neutral_public_chip() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = targets("upstream", "https://github.com/torvalds/linux.git", &[]);
        let r = resolved(&accounts, &t, None);
        // The public check runs for remotes that need a choice too.
        assert_eq!(
            r.fetch
                .public_probe
                .as_ref()
                .map(PublicProbe::key)
                .as_deref(),
            Some("https://github.com|torvalds/linux")
        );
        let d = shown(&accounts, &t, None, PublicState::Public);
        let chip = d.chip.unwrap();
        assert_eq!(chip.text, "public");
        assert!(!chip.warn);
        assert_eq!(
            chip.tooltip,
            "github.com · public repository, no account needed"
        );
        assert_eq!(d.add_account, None);
        // Account… still offers the choice.
        assert!(matches!(
            account_action(&r, &accounts),
            AccountAction::Pick { .. }
        ));
        // A remembered choice wins over "public": the account chip shows.
        let chip = shown(&accounts, &t, Some("a2"), PublicState::Public)
            .chip
            .unwrap();
        assert_eq!(chip.text, "mira-at-acme");
        // Accounts on a self-hosted instance are not asked anonymously.
        let selfhosted = [
            account("g1", "gitlab", "https://git.example.com", "a"),
            account("g2", "gitlab", "https://git.example.com", "b"),
        ];
        let t = targets("work", "https://git.example.com/g/p.git", &[]);
        assert_eq!(resolved(&selfhosted, &t, None).fetch.public_probe, None);
    }

    #[test]
    fn a_remembered_account_that_was_removed_is_ignored() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = targets("origin", REPO, &[]);
        // `zz` is not an account any more.
        let r = resolved(&accounts, &t, Some("zz"));
        assert!(matches!(r.fetch.account, UrlAccount::Choose(_)));
        // One left: it is used whatever the stale id says.
        let r = resolved(&accounts[..1], &t, Some("a2"));
        assert_eq!(r.fetch.account, UrlAccount::Linked("a1".into()));
    }

    #[test]
    fn a_remembered_account_of_another_host_is_ignored() {
        let accounts = [
            github("a1", "mira"),
            account("g1", "gitlab", "https://gitlab.com", "mira"),
        ];
        let t = targets("origin", REPO, &[]);
        let r = resolved(&accounts, &t, Some("g1"));
        assert_eq!(r.fetch.account, UrlAccount::Linked("a1".into()));
    }

    #[test]
    fn no_account_on_a_known_service_waits_for_the_public_check() {
        let t = targets("upstream", "https://github.com/rust-lang/rust.git", &[]);
        let r = resolved(&[], &t, None);
        assert_eq!(r.fetch.account, UrlAccount::NoAccount(ServiceKind::GitHub));
        let probe = r.fetch.public_probe.clone().unwrap();
        assert_eq!(probe.key(), "https://github.com|rust-lang/rust");

        // Pending: no chip and no add row, so nothing flickers.
        let d = shown(&[], &t, None, PublicState::Pending);
        assert_eq!((d.chip, d.add_account), (None, None));

        let d = shown(&[], &t, None, PublicState::Public);
        let chip = d.chip.unwrap();
        assert_eq!(chip.text, "public");
        assert!(!chip.warn);
        assert_eq!(
            chip.tooltip,
            "github.com · public repository, no account needed"
        );
        assert_eq!(d.add_account, None);

        let d = shown(&[], &t, None, PublicState::NotPublic);
        assert!(d.chip.is_none());
        assert!(d.add_account.is_none());
        assert_eq!(d.service, Some(ServiceKind::GitHub));
        assert_eq!(d.tooltip.as_deref(), Some("upstream · github.com"));
    }

    #[test]
    fn a_service_without_an_anonymous_check_shows_no_chip() {
        // Bitbucket (and other services without a public probe) have no
        // anonymous check, so public_probe is None and the account is
        // NoAccount. No chip is shown regardless of the public state.
        let t = targets("origin", "https://bitbucket.org/acme/app.git", &[]);
        let r = resolved(&[], &t, None);
        assert_eq!(r.fetch.public_probe, None);
        let d = shown(&[], &t, None, PublicState::Pending);
        assert!(d.chip.is_none());
        assert!(d.add_account.is_none());
    }

    #[test]
    fn an_unknown_host_has_no_chip_and_no_service() {
        let t = targets("origin", "https://git.example.com/acme/app.git", &[]);
        let r = resolved(&[], &t, None);
        assert_eq!(r.fetch.account, UrlAccount::Unknown);
        let d = shown(&[], &t, None, PublicState::NotPublic);
        assert_eq!(d, RemoteDisplay::default());
        // A local path is not a remote URL at all.
        let t = targets("local", "/srv/git/app.git", &[]);
        assert_eq!(resolved(&[], &t, None).fetch.account, UrlAccount::Unknown);
    }

    #[test]
    fn a_self_hosted_host_is_known_through_its_account() {
        let accounts = [account("g1", "gitlab", "https://git.example.com", "mira")];
        let t = targets("work", "git@git.example.com:acme/app.git", &[]);
        let r = resolved(&accounts, &t, None);
        assert_eq!(r.fetch.account, UrlAccount::Linked("g1".into()));
        assert_eq!(r.fetch.service, Some(ServiceKind::GitLab));
    }

    #[test]
    fn a_self_hosted_port_is_part_of_the_displayed_host() {
        let accounts = [account(
            "g1",
            "gitlab",
            "https://git.example.com:8443",
            "mira",
        )];
        let t = targets("work", "https://git.example.com:8443/acme/app.git", &[]);
        let r = resolved(&accounts, &t, None);
        assert_eq!(r.fetch.host, "git.example.com:8443");
        // The ssh port is sshd's: not shown.
        let t = targets("work", "ssh://git@git.example.com:2222/acme/app.git", &[]);
        let r = resolved(&accounts, &t, None);
        assert_eq!(r.fetch.host, "git.example.com");
    }

    #[test]
    fn multi_push_is_several_push_urls_or_a_push_url_on_another_host() {
        let same = targets("origin", REPO, &[]);
        assert!(!is_multi_push(&same));
        // One pushurl on the same host (even another path or transport).
        let fork = targets("origin", REPO, &["git@github.com:me/app.git"]);
        assert!(!is_multi_push(&fork));
        // One pushurl on another host.
        let other = targets("origin", REPO, &["https://codeberg.org/me/app.git"]);
        assert!(is_multi_push(&other));
        // Two pushurls, even on one host.
        let two = targets(
            "origin",
            REPO,
            &["git@github.com:me/app.git", "git@github.com:you/app.git"],
        );
        assert!(is_multi_push(&two));
        // No push URLs at all (should not happen): not multi.
        let none = RemoteTargets {
            name: "x".into(),
            fetch_url: REPO.into(),
            push_urls: vec![],
        };
        assert!(!is_multi_push(&none));
    }

    #[test]
    fn a_multi_push_remote_shows_each_target_with_its_own_account() {
        let accounts = [
            github("a1", "mira-k"),
            account("c1", "gitea", "https://codeberg.org", "mira-k"),
        ];
        let t = targets(
            "publish",
            REPO,
            &[
                "https://github.com/acme/app.git",
                "https://codeberg.org/acme/app.git",
            ],
        );
        let r = resolved(&accounts, &t, None);
        assert!(r.multi_push);
        let d = display(&r, &accounts, &|_| PublicState::Pending);
        assert_eq!(d.service, None);
        assert_eq!(d.add_account, None);
        let chip = d.chip.unwrap();
        assert_eq!(chip.text, "2 push URLs");
        assert_eq!(chip.tooltip, "Pushes to github.com and codeberg.org");
        assert_eq!(d.tooltip.as_deref(), Some("publish · pushes to 2 targets"));
        // The fetch URL is the first push URL: no separate fetch row.
        assert!(r.fetch_is_a_push_url);
        assert!(d.fetch.is_none());
        assert_eq!(d.pushes.len(), 2);
        assert_eq!(d.pushes[0].host, "github.com");
        assert_eq!(d.pushes[0].label, "push → github.com");
        assert_eq!(d.pushes[0].service, Some(ServiceKind::GitHub));
        assert_eq!(d.pushes[0].chip.as_ref().unwrap().text, "mira-k");
        assert_eq!(d.pushes[1].host, "codeberg.org");
        assert_eq!(d.pushes[1].service, Some(ServiceKind::Gitea));
        assert_eq!(
            d.pushes[1].chip.as_ref().unwrap().tooltip,
            "codeberg.org · account mira-k"
        );
    }

    #[test]
    fn a_single_push_url_on_another_host_is_a_singular_chip_with_a_fetch_row() {
        let accounts = [github("a1", "mira-k")];
        let t = targets("origin", REPO, &["https://codeberg.org/me/app.git"]);
        let r = resolved(&accounts, &t, None);
        assert!(r.multi_push);
        let d = display(&r, &accounts, &|_| PublicState::NotPublic);
        let chip = d.chip.unwrap();
        assert_eq!(chip.text, "1 push URL");
        assert_eq!(chip.tooltip, "Pushes to codeberg.org");
        assert_eq!(d.tooltip.as_deref(), Some("origin · pushes to 1 target"));
        let fetch = d.fetch.unwrap();
        assert_eq!(fetch.label, "fetch → github.com");
        assert_eq!(fetch.chip.unwrap().text, "mira-k");
        assert_eq!(d.pushes.len(), 1);
        assert_eq!(d.pushes[0].chip, None);
    }

    #[test]
    fn a_push_url_without_an_account_shows_no_chip_and_no_add_row() {
        // A push URL with no linked account no longer shows a warning chip or
        // an add-account row — the user can add one via the remote's menu.
        let accounts = [github("a1", "mira-k")];
        let t = targets(
            "publish",
            REPO,
            &[
                "https://github.com/acme/app.git",
                "https://gitlab.com/acme/app.git",
            ],
        );
        let r = resolved(&accounts, &t, None);
        let d = display(&r, &accounts, &|_| PublicState::NotPublic);
        assert_eq!(d.add_account, None);
        assert_eq!(d.pushes[1].chip, None);
        // Pending also shows no chip.
        let d = display(&r, &accounts, &|_| PublicState::Pending);
        assert_eq!(d.pushes[1].chip, None);
    }

    #[test]
    fn joined_hosts_read_naturally() {
        assert_eq!(join_hosts(&["a"]), "a");
        assert_eq!(join_hosts(&["a", "b"]), "a and b");
        assert_eq!(join_hosts(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn public_probes_are_deduplicated_and_skip_linked_remotes() {
        let accounts = [github("a1", "mira")];
        let rs = resolve_all(
            &accounts,
            &[
                targets("origin", REPO, &[]),
                targets("up1", "https://gitlab.com/g/p.git", &[]),
                targets("up2", "git@gitlab.com:g/p.git", &[]),
                targets("up3", "https://gitlab.com/g/other.git", &[]),
            ],
            None,
        );
        let keys: Vec<String> = public_probes(&rs).iter().map(PublicProbe::key).collect();
        assert_eq!(
            keys,
            ["https://gitlab.com|g/p", "https://gitlab.com|g/other"]
        );
    }

    #[test]
    fn resolve_all_applies_the_remembered_choice_per_remote_name() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let remembered = std::collections::BTreeMap::from([("origin".to_owned(), "a2".to_owned())]);
        let rs = resolve_all(
            &accounts,
            &[
                targets("origin", REPO, &[]),
                targets("upstream", "https://github.com/up/app.git", &[]),
            ],
            Some(&remembered),
        );
        assert_eq!(rs[0].fetch.account, UrlAccount::Linked("a2".into()));
        assert!(matches!(rs[1].fetch.account, UrlAccount::Choose(_)));
    }

    #[test]
    fn account_action_per_situation() {
        let two = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = targets("origin", REPO, &[]);
        assert_eq!(
            account_action(&resolved(&two, &t, Some("a2")), &two),
            AccountAction::Pick {
                host: "github.com".into(),
                ids: vec!["a1".into(), "a2".into()],
                current: Some("a2".into())
            }
        );
        assert_eq!(
            account_action(&resolved(&two, &t, None), &two),
            AccountAction::Pick {
                host: "github.com".into(),
                ids: vec!["a1".into(), "a2".into()],
                current: None
            }
        );
        let one = &two[..1];
        assert_eq!(
            account_action(&resolved(one, &t, None), one),
            AccountAction::AlreadyOne {
                host: "github.com".into(),
                label: "mira".into()
            }
        );
        assert_eq!(
            account_action(&resolved(&[], &t, None), &[]),
            AccountAction::Add {
                kind: ServiceKind::GitHub,
                host: "github.com".into()
            }
        );
        let unknown = targets("x", "https://git.example.com/a/b.git", &[]);
        assert_eq!(
            account_action(&resolved(&[], &unknown, None), &[]),
            AccountAction::NoService {
                host: "git.example.com".into()
            }
        );
    }

    #[test]
    fn a_new_account_is_remembered_only_when_it_joins_others() {
        let mut accounts = vec![github("a1", "mira")];
        // Alone on the host: found on its own, nothing to store.
        assert_eq!(choice_after_add(&accounts, REPO, "a1"), None);
        accounts.push(github("a2", "mira-at-acme"));
        assert_eq!(choice_after_add(&accounts, REPO, "a2"), Some("a2".into()));
        // An account of another host does not matter.
        assert_eq!(
            choice_after_add(&accounts, "https://gitlab.com/g/p.git", "a2"),
            None
        );
    }

    #[test]
    fn records_that_do_not_parse_are_skipped() {
        let accounts = [
            github("a1", "mira"),
            account("bad1", "nope", "https://x.io", "m"),
            account("bad2", "github", "ftp://x.io", "m"),
        ];
        let ids: Vec<String> = account_refs(&accounts).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["a1"]);
    }

    fn uses(list: &[(&str, Direction)]) -> Vec<RemoteUse> {
        list.iter()
            .map(|(remote, direction)| RemoteUse {
                remote: (*remote).to_owned(),
                direction: *direction,
            })
            .collect()
    }

    fn creds(
        accounts: &[AccountRecord],
        t: &[RemoteTargets],
        u: &[(&str, Direction)],
        remembered: &[(&str, &str)],
    ) -> JobCredentials {
        let remembered = remembered
            .iter()
            .map(|(r, id)| ((*r).to_owned(), (*id).to_owned()))
            .collect();
        job_credentials(accounts, t, &uses(u), &remembered)
    }

    fn key(scheme: &str, host: &str, port: Option<u16>) -> HostKey {
        HostKey::new(scheme, host, port)
    }

    #[test]
    fn a_linked_https_remote_gives_its_host_an_account() {
        let accounts = [github("a1", "mira")];
        let t = [targets("origin", REPO, &[])];
        let c = creds(&accounts, &t, &[("origin", Direction::Fetch)], &[]);
        assert_eq!(c.choose, Vec::<String>::new());
        assert_eq!(
            c.accounts,
            HashMap::from([(
                key("https", "github.com", None),
                AccountCredential {
                    account_id: "a1".into(),
                    username: "mira".into()
                }
            )])
        );
    }

    #[test]
    fn only_https_counts_and_http_only_for_an_http_account() {
        let accounts = [
            github("a1", "mira"),
            account("g1", "gitlab", "http://git.lan:8080", "mira"),
        ];
        let t = [
            targets("ssh", "git@github.com:acme/app.git", &[]),
            targets("plain", "http://github.com/acme/app.git", &[]),
            targets("lan", "http://git.lan:8080/g/p.git", &[]),
            targets("lans", "https://git.lan:8080/g/p.git", &[]),
        ];
        let c = creds(
            &accounts,
            &t,
            &[
                ("ssh", Direction::Fetch),
                ("plain", Direction::Fetch),
                ("lan", Direction::Fetch),
            ],
            &[],
        );
        // ssh: no token over ssh; plain http to an https account: refused;
        // http to an http instance: allowed, with GitLab's fixed user name.
        assert_eq!(
            c.accounts.keys().collect::<Vec<_>>(),
            vec![&key("http", "git.lan", Some(8080))]
        );
        assert_eq!(c.accounts.values().next().unwrap().username, "oauth2");
        // An https URL of the same http-only instance is fine too.
        let c = creds(&accounts, &t, &[("lans", Direction::Fetch)], &[]);
        assert!(c
            .accounts
            .contains_key(&key("https", "git.lan", Some(8080))));
    }

    #[test]
    fn several_accounts_without_a_choice_are_listed_not_used() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = [
            targets("origin", REPO, &[]),
            targets("up", "https://gitlab.com/g/p.git", &[]),
        ];
        let c = creds(
            &accounts,
            &t,
            &[("origin", Direction::Fetch), ("origin", Direction::Push)],
            &[],
        );
        assert!(c.accounts.is_empty());
        assert_eq!(c.choose, ["origin"]);
        // A remembered choice settles it, and the chosen login is used.
        let c = creds(
            &accounts,
            &t,
            &[("origin", Direction::Fetch)],
            &[("origin", "a2")],
        );
        assert!(c.choose.is_empty());
        assert_eq!(c.accounts.values().next().unwrap().account_id, "a2");
        assert_eq!(c.accounts.values().next().unwrap().username, "mira-at-acme");
    }

    #[test]
    fn a_push_job_covers_every_push_url_host() {
        let accounts = [
            github("a1", "mira"),
            account("c1", "gitea", "https://codeberg.org", "mira-k"),
        ];
        let t = [targets(
            "publish",
            REPO,
            &[
                "https://github.com/acme/app.git",
                "https://codeberg.org/acme/app.git",
                "https://nowhere.example/acme/app.git",
            ],
        )];
        let push = creds(&accounts, &t, &[("publish", Direction::Push)], &[]);
        let mut hosts: Vec<_> = push.accounts.keys().map(|k| k.host.as_str()).collect();
        hosts.sort_unstable();
        // The host without an account stays out: it keeps today's behaviour.
        assert_eq!(hosts, ["codeberg.org", "github.com"]);
        // Fetching contacts the fetch URL only.
        let fetch = creds(&accounts, &t, &[("publish", Direction::Fetch)], &[]);
        assert_eq!(fetch.accounts.len(), 1);
    }

    #[test]
    fn a_host_shared_by_two_remotes_keeps_the_first_account() {
        let accounts = [github("a1", "mira"), github("a2", "mira-at-acme")];
        let t = [
            targets("origin", REPO, &[]),
            targets("fork", "https://github.com/me/app.git", &[]),
        ];
        let c = creds(
            &accounts,
            &t,
            &[("origin", Direction::Fetch), ("fork", Direction::Fetch)],
            &[("origin", "a1"), ("fork", "a2")],
        );
        assert_eq!(c.accounts.len(), 1);
        assert_eq!(c.accounts.values().next().unwrap().account_id, "a1");
    }

    #[test]
    fn an_unknown_remote_or_a_local_path_adds_nothing() {
        let accounts = [github("a1", "mira")];
        let t = [targets("local", "/srv/git/app.git", &[])];
        let c = creds(
            &accounts,
            &t,
            &[("local", Direction::Fetch), ("missing", Direction::Fetch)],
            &[],
        );
        assert_eq!(c, JobCredentials::default());
    }

    #[test]
    fn the_failed_account_is_the_one_whose_host_the_error_names() {
        let accounts = [
            github("a1", "mira"),
            account("c1", "gitea", "https://codeberg.org", "mira-k"),
        ];
        let t = [targets(
            "publish",
            REPO,
            &[
                "https://github.com/acme/app.git",
                "https://codeberg.org/acme/app.git",
            ],
        )];
        let c = creds(&accounts, &t, &[("publish", Direction::Push)], &[]);
        let used = used_accounts(&c, &accounts);
        assert_eq!(
            used.iter().map(|u| u.host.as_str()).collect::<Vec<_>>(),
            ["codeberg.org", "github.com"]
        );
        let failed = failed_account(
            &used,
            "fatal: Authentication failed for 'https://github.com/acme/app.git/'",
        );
        assert_eq!(failed.unwrap().account_id, "a1");
        // No host in the text: the first one.
        assert_eq!(failed_account(&used, "boom").unwrap().account_id, "c1");
        assert_eq!(failed_account(&[], "boom"), None);
    }

    #[test]
    fn push_targets_are_described_by_host_and_account() {
        let accounts = [github("a1", "mira")];
        let (host, account) = push_target_info(&accounts, "https://github.com/a/b.git", None);
        assert_eq!(host, "github.com");
        assert_eq!(account, Some(("a1".to_owned(), "mira".to_owned())));
        let (host, account) = push_target_info(&accounts, "https://codeberg.org/a/b.git", None);
        assert_eq!((host.as_str(), account), ("codeberg.org", None));
    }

    #[test]
    fn the_public_cache_asks_once_and_a_late_answer_wins() {
        let key = "https://test.invalid|cache/once";
        assert_eq!(cached_public(key), PublicState::Pending);
        assert!(claim_public_check(key));
        assert!(!claim_public_check(key));
        assert_eq!(cached_public(key), PublicState::Pending);
        expire_public_check(key);
        assert_eq!(cached_public(key), PublicState::NotPublic);
        finish_public_check(key, true);
        assert_eq!(cached_public(key), PublicState::Public);
        // Expiry and a later failure never override an answer.
        expire_public_check(key);
        release_public_check(key);
        assert_eq!(cached_public(key), PublicState::Public);
        assert!(!claim_public_check(key));
    }

    #[test]
    fn a_failed_check_is_released_and_retried_at_most_once_a_minute() {
        let key = "https://test.invalid|cache/retry";
        let t0 = Instant::now();
        assert!(claim_at(key, t0));
        release_at(key, t0);
        // Shown as "not public", but nothing was cached as the answer.
        assert_eq!(cached_public(key), PublicState::NotPublic);
        // Too soon: not asked again.
        assert!(!claim_at(key, t0 + Duration::from_secs(59)));
        // After a minute the next sidebar pass asks again; the display keeps
        // "not public" meanwhile instead of flickering back to nothing.
        assert!(claim_at(key, t0 + Duration::from_secs(60)));
        assert_eq!(cached_public(key), PublicState::NotPublic);
        assert!(!claim_at(key, t0 + Duration::from_secs(61)));
        // A second failure restarts the minute.
        let t1 = t0 + Duration::from_secs(70);
        release_at(key, t1);
        assert!(!claim_at(key, t1 + Duration::from_secs(30)));
        assert!(claim_at(key, t1 + Duration::from_secs(60)));
        // The answer finally arrives and sticks.
        finish_public_check(key, false);
        assert!(!claim_at(key, t1 + Duration::from_secs(600)));
    }
}
