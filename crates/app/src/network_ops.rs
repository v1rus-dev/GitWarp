//! The background half of the stage 8 network actions: SSH host-key preflight,
//! then the git-core operation, plus friendlier wording for the errors git
//! prints.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use git_core::network::{
    self, CancelToken, CloneOptions, FetchOptions, NetworkContext, NetworkOutcome, Progress,
    PushOptions,
};
use git_core::ssh::{self, HostKeyStatus};
use git_core::{Error, Git, Repo, Result};
use gpui::SharedString;

use crate::network_service::HostKeyAsker;
use crate::remote_accounts::{Direction, RemoteUse};

/// One network action the toolbar and the sidebar can start.
#[derive(Debug, Clone)]
pub enum Job {
    /// Fetch every remote with tags and pruning.
    Fetch(FetchOptions),
    /// Pull the current branch, fast-forward only.
    Pull,
    /// Push the current branch; the first push creates its upstream.
    Push,
    PushTag {
        remote: String,
        tag: String,
    },
    PushTags {
        remote: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    /// Initialise and update one submodule, by path.
    UpdateSubmodule {
        path: String,
    },
    /// Fetch a pull request's head ref into the local branch `branch` (no
    /// force: a diverged branch fails instead of losing commits); the caller
    /// checks the branch out afterwards.
    FetchPullRequest {
        remote: String,
        /// `refs/pull/<n>/head:refs/heads/<branch>`.
        refspec: String,
        branch: String,
        number: u64,
    },
}

impl Job {
    /// The word shown in the status bar and in toasts.
    pub fn verb(&self) -> SharedString {
        match self {
            Job::Fetch(_) | Job::FetchPullRequest { .. } => ui::t!("network.verb.fetching"),
            Job::Pull => ui::t!("network.verb.pulling"),
            Job::Push => ui::t!("network.verb.pushing"),
            Job::PushTag { .. } | Job::PushTags { .. } => ui::t!("network.verb.pushing_tags"),
            Job::DeleteRemoteBranch { .. } => ui::t!("network.verb.deleting_remote_branch"),
            Job::UpdateSubmodule { .. } => ui::t!("network.verb.updating_submodule"),
        }
    }

    /// The success line for the toast, once the job is done.
    pub fn done_message(&self, outcome: &NetworkOutcome) -> String {
        match self {
            Job::Fetch(_) if !outcome.changed => ui::t!("network.done.up_to_date").to_string(),
            Job::Fetch(_) => ui::t!("network.done.fetch").to_string(),
            Job::Pull
                if outcome
                    .lines
                    .iter()
                    .any(|line| line.contains("Already up to date")) =>
            {
                ui::t!("network.done.up_to_date").to_string()
            }
            Job::Pull => ui::t!("network.done.pull").to_string(),
            Job::Push => ui::t!("network.done.push").to_string(),
            Job::PushTag { tag, .. } => ui::t!("network.done.push_tag", tag = tag).to_string(),
            Job::PushTags { .. } => ui::t!("network.done.push_tags").to_string(),
            Job::DeleteRemoteBranch { branch, .. } => {
                ui::t!("network.done.delete_remote_branch", branch = branch).to_string()
            }
            Job::UpdateSubmodule { path } => {
                ui::t!("network.done.update_submodule", path = path).to_string()
            }
            Job::FetchPullRequest { .. } => ui::t!("network.done.fetch").to_string(),
        }
    }

    /// Runs the job, resolving the current branch's upstream where needed.
    pub fn run(
        &self,
        repo: &Repo,
        context: &NetworkContext,
        asker: &HostKeyAsker,
        cancel: &CancelToken,
        on_progress: impl FnMut(Progress),
    ) -> Result<NetworkOutcome> {
        match self {
            Job::Fetch(options) => fetch(repo, options, context, asker, cancel, on_progress),
            Job::Pull => {
                let branch = current_branch(repo)?;
                let Some((remote, upstream_branch)) = upstream(repo, &branch)? else {
                    return Err(Error::Git {
                        command: "pull",
                        code: None,
                        stderr: ui::t!("network.error.pull_no_upstream", branch = branch)
                            .to_string(),
                    });
                };
                pull(
                    repo,
                    Some(&remote),
                    Some(&upstream_branch),
                    context,
                    asker,
                    cancel,
                    on_progress,
                )
            }
            Job::Push => {
                let branch = current_branch(repo)?;
                let upstream = upstream(repo, &branch)?;
                let (remote, refspec, set_upstream) = match upstream {
                    Some((remote, upstream_branch)) => (remote, upstream_branch, false),
                    None => {
                        let Some(remote) = network::default_remote(repo)? else {
                            return Err(Error::Git {
                                command: "push",
                                code: None,
                                stderr: ui::t!("network.error.no_remote").to_string(),
                            });
                        };
                        (remote, branch, true)
                    }
                };
                push(
                    repo,
                    &PushOptions {
                        remote: Some(remote),
                        refspec: Some(refspec),
                        set_upstream,
                    },
                    context,
                    asker,
                    cancel,
                    on_progress,
                )
            }
            Job::PushTag { remote, tag } => {
                push_tag(repo, remote, tag, context, asker, cancel, on_progress)
            }
            Job::PushTags { remote } => {
                push_tags(repo, remote, context, asker, cancel, on_progress)
            }
            Job::DeleteRemoteBranch { remote, branch } => {
                delete_remote_branch(repo, remote, branch, context, asker, cancel, on_progress)
            }
            Job::UpdateSubmodule { path } => {
                update_submodule(repo, path, context, asker, cancel, on_progress)
            }
            Job::FetchPullRequest {
                remote, refspec, ..
            } => fetch_refspec(repo, remote, refspec, context, asker, cancel, on_progress),
        }
    }
}

/// The checked-out branch, or an error for a detached HEAD.
pub fn current_branch(repo: &Repo) -> Result<String> {
    match repo.head()? {
        git_core::Head::Branch { name, .. } => Ok(name),
        git_core::Head::Detached(_) => Err(Error::Git {
            command: "branch",
            code: None,
            stderr: ui::t!("network.error.detached_head").to_string(),
        }),
    }
}

/// The remote and remote branch a local branch tracks.
pub fn upstream(repo: &Repo, branch: &str) -> Result<Option<(String, String)>> {
    let refs = repo.refs()?;
    let Some(local) = refs.iter().find(|reference| {
        reference.kind == git_core::RefKind::LocalBranch && reference.name == branch
    }) else {
        return Ok(None);
    };
    let Some(upstream) = &local.upstream else {
        return Ok(None);
    };
    Ok(upstream
        .split_once('/')
        .map(|(remote, branch)| (remote.to_owned(), branch.to_owned())))
}

/// The remotes `job` will contact and which of their URLs, from what the
/// toolbar already knows (no git command): the job's own account lookup needs
/// this before the job starts. `head` and `refs` are the checked-out branch and
/// the refs as of the last load.
pub fn job_remote_uses(
    job: &Job,
    head: Option<&git_core::Head>,
    refs: Option<&[git_core::Ref]>,
    remotes: &[String],
) -> Vec<RemoteUse> {
    let one = |remote: &str, direction| {
        vec![RemoteUse {
            remote: remote.to_owned(),
            direction,
        }]
    };
    let upstream_remote = || {
        let Some(git_core::Head::Branch { name, .. }) = head else {
            return None;
        };
        refs?
            .iter()
            .find(|r| r.kind == git_core::RefKind::LocalBranch && r.name == *name)?
            .upstream
            .as_deref()?
            .split_once('/')
            .map(|(remote, _)| remote.to_owned())
    };
    match job {
        Job::Fetch(options) => match &options.remote {
            Some(remote) => one(remote, Direction::Fetch),
            None => remotes
                .iter()
                .map(|remote| RemoteUse {
                    remote: remote.clone(),
                    direction: Direction::Fetch,
                })
                .collect(),
        },
        Job::Pull => upstream_remote()
            .map(|remote| one(&remote, Direction::Fetch))
            .unwrap_or_default(),
        Job::Push => {
            // Without an upstream the push goes to `origin`, else the first.
            let remote = upstream_remote().or_else(|| {
                remotes
                    .iter()
                    .find(|remote| remote.as_str() == "origin")
                    .or_else(|| remotes.first())
                    .cloned()
            });
            remote
                .map(|remote| one(&remote, Direction::Push))
                .unwrap_or_default()
        }
        Job::PushTag { remote, .. }
        | Job::PushTags { remote }
        | Job::DeleteRemoteBranch { remote, .. } => one(remote, Direction::Push),
        Job::FetchPullRequest { remote, .. } => one(remote, Direction::Fetch),
        Job::UpdateSubmodule { .. } => Vec::new(),
    }
}

/// Checks every SSH remote involved in `urls` before git connects. Unknown
/// hosts are shown to the user; a changed key must be replaced explicitly.
pub fn preflight(urls: &[String], asker: &HostKeyAsker, cancel: &CancelToken) -> Result<()> {
    for url in urls {
        let Some(target) = ssh::ssh_target(url) else {
            continue;
        };
        // `ssh-keyscan` takes a moment even for a healthy host. Once a host is
        // known in this session there is nothing left to ask; a key that
        // changes later still makes git fail safely.
        if trusted()
            .lock()
            .is_ok_and(|trusted| trusted.contains(&target.host))
        {
            continue;
        }
        let status = ssh::check_host_key(&target)?;
        match status {
            HostKeyStatus::Known | HostKeyStatus::Unavailable => {
                if let Ok(mut trusted) = trusted().lock() {
                    trusted.insert(ssh::host_argument(&target));
                }
            }
            HostKeyStatus::Unreachable { .. } => {
                // The real operation will produce the authoritative error.
            }
            HostKeyStatus::Unknown { .. } => {
                if !asker.ask(target.clone(), status, cancel) {
                    if cancel.is_cancelled() {
                        return Err(Error::Cancelled { command: "ssh" });
                    }
                    return Err(Error::Git {
                        command: "ssh",
                        code: None,
                        stderr: ui::t!("network.error.host_key_not_trusted", host = target.host)
                            .to_string(),
                    });
                }
                ssh::trust_host_key(&target)?;
                if let Ok(mut trusted) = trusted().lock() {
                    trusted.insert(ssh::host_argument(&target));
                }
            }
            HostKeyStatus::Changed { .. } => {
                if !asker.ask(target.clone(), status, cancel) {
                    if cancel.is_cancelled() {
                        return Err(Error::Cancelled { command: "ssh" });
                    }
                    return Err(Error::Git {
                        command: "ssh",
                        code: None,
                        stderr: ui::t!("network.error.host_key_not_replaced", host = target.host)
                            .to_string(),
                    });
                }
                ssh::replace_host_key(&target)?;
                if let Ok(mut trusted) = trusted().lock() {
                    trusted.insert(ssh::host_argument(&target));
                }
            }
        }
    }
    Ok(())
}

/// Fetches with the SSH preflight in front of it.
pub fn fetch(
    repo: &Repo,
    options: &FetchOptions,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let urls = remote_urls(repo, options.remote.as_deref())?;
    preflight(&urls, asker, cancel)?;
    network::fetch(repo, options, context, cancel, on_progress)
}

/// Fetches one refspec with the SSH preflight in front of it.
pub fn fetch_refspec(
    repo: &Repo,
    remote: &str,
    refspec: &str,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let urls = remote_urls(repo, Some(remote))?;
    preflight(&urls, asker, cancel)?;
    network::fetch_refspec(repo, remote, refspec, context, cancel, on_progress)
}

/// Pulls with the SSH preflight of the upstream remote.
pub fn pull(
    repo: &Repo,
    remote: Option<&str>,
    branch: Option<&str>,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let urls = match remote {
        Some(remote) => remote_urls(repo, Some(remote))?,
        None => remote_urls(repo, None)?,
    };
    preflight(&urls, asker, cancel)?;
    network::pull(repo, remote, branch, context, cancel, on_progress)
}

/// Updates a submodule, preflighting its `.gitmodules` URL when it has one.
pub fn update_submodule(
    repo: &Repo,
    path: &str,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    if let Some(url) = network::submodule_url(repo, path)? {
        preflight(&[url], asker, cancel)?;
    }
    network::update_submodule(repo, path, context, cancel, on_progress)
}

/// Pushes with the SSH preflight of the destination remote.
pub fn push(
    repo: &Repo,
    options: &PushOptions,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    let remote = options
        .remote
        .clone()
        .or_else(|| network::default_remote(repo).ok().flatten());
    // Several push URLs: one git process per URL, so a failing target does not
    // keep the later ones from being pushed. One URL keeps the plain path.
    if let Some(remote) = &remote {
        let push_urls = repo.remote_push_urls(remote)?;
        if push_urls.len() > 1 {
            preflight(&push_urls, asker, cancel)?;
            let options = PushOptions {
                remote: Some(remote.clone()),
                ..options.clone()
            };
            return network::push_each(repo, &push_urls, &options, context, cancel, on_progress);
        }
    }
    let urls = remote_urls(repo, remote.as_deref())?;
    preflight(&urls, asker, cancel)?;
    network::push(repo, options, context, cancel, on_progress)
}

/// The push URLs of `remote` when it has several, which are pushed one git
/// process each so a failing target does not stop the later ones.
fn multi_push_urls(repo: &Repo, remote: &str) -> Result<Option<Vec<String>>> {
    let urls = repo.remote_push_urls(remote)?;
    Ok((urls.len() > 1).then_some(urls))
}

/// Clones with the SSH preflight of the source.
pub fn clone(
    git: &Git,
    options: &CloneOptions,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    preflight(std::slice::from_ref(&options.url), asker, cancel)?;
    network::clone(git, options, context, cancel, on_progress)
}

/// Pushes one tag with the preflight.
pub fn push_tag(
    repo: &Repo,
    remote: &str,
    tag: &str,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    if let Some(urls) = multi_push_urls(repo, remote)? {
        preflight(&urls, asker, cancel)?;
        let options = PushOptions {
            remote: Some(remote.to_owned()),
            refspec: Some(format!("refs/tags/{tag}")),
            set_upstream: false,
        };
        return network::push_each(repo, &urls, &options, context, cancel, on_progress);
    }
    preflight(&remote_urls(repo, Some(remote))?, asker, cancel)?;
    network::push_tag(repo, remote, tag, context, cancel, on_progress)
}

/// Pushes all tags with the preflight.
pub fn push_tags(
    repo: &Repo,
    remote: &str,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    if let Some(urls) = multi_push_urls(repo, remote)? {
        preflight(&urls, asker, cancel)?;
        // Same refs as `git push --tags`.
        let options = PushOptions {
            remote: Some(remote.to_owned()),
            refspec: Some("refs/tags/*:refs/tags/*".to_owned()),
            set_upstream: false,
        };
        return network::push_each(repo, &urls, &options, context, cancel, on_progress);
    }
    preflight(&remote_urls(repo, Some(remote))?, asker, cancel)?;
    network::push_tags(repo, remote, context, cancel, on_progress)
}

/// Deletes a remote branch with the preflight.
pub fn delete_remote_branch(
    repo: &Repo,
    remote: &str,
    branch: &str,
    context: &NetworkContext,
    asker: &HostKeyAsker,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<NetworkOutcome> {
    if let Some(urls) = multi_push_urls(repo, remote)? {
        preflight(&urls, asker, cancel)?;
        return network::delete_each(repo, &urls, remote, branch, context, cancel, on_progress);
    }
    preflight(&remote_urls(repo, Some(remote))?, asker, cancel)?;
    network::delete_remote_branch(repo, remote, branch, context, cancel, on_progress)
}

/// Hosts already checked in this app session, so repeated fetches do not run
/// `ssh-keyscan` again.
fn trusted() -> &'static Mutex<HashSet<String>> {
    static TRUSTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    TRUSTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Fetch URLs for `remote` (one) or every remote (all).
fn remote_urls(repo: &Repo, remote: Option<&str>) -> Result<Vec<String>> {
    let remotes = match remote {
        Some(remote) => vec![remote.to_owned()],
        None => repo.remotes()?,
    };
    let mut urls = Vec::with_capacity(remotes.len());
    for remote in remotes {
        if let Some(url) = repo.remote_url(&remote)? {
            urls.push(url);
        }
    }
    Ok(urls)
}

/// Whether lowercased git output says the credentials were refused or could
/// not be asked for.
fn is_auth_failure(lower: &str) -> bool {
    lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("terminal prompts disabled")
        || lower.contains("invalid username or password")
}

/// Whether `error` is an HTTPS authentication failure (not an SSH one).
pub fn is_auth_error(error: &Error) -> bool {
    !matches!(error, Error::Cancelled { .. }) && is_auth_failure(&error.to_string().to_lowercase())
}

/// Why one push target failed, as one line for the push result toast, and
/// whether it was an authentication failure.
pub fn target_failure(text: &str) -> (String, bool) {
    let error = Error::Git {
        command: "push",
        code: None,
        stderr: text.to_owned(),
    };
    if is_auth_error(&error) {
        return (ui::t!("network.push.token_invalid").to_string(), true);
    }
    let friendly = friendly_error(&error);
    let line = if friendly == error.to_string() {
        // Not worded by us: the line git would call the cause.
        let lines = || text.lines().map(str::trim).filter(|l| !l.is_empty());
        lines()
            .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
            .or_else(|| lines().find(|l| l.contains("rejected")))
            .or_else(|| lines().next())
            .map(|l| {
                l.trim_start_matches("fatal:")
                    .trim_start_matches("error:")
                    .trim()
            })
            .unwrap_or_default()
            .to_owned()
    } else {
        friendly.lines().next().unwrap_or_default().to_owned()
    };
    (line, false)
}

/// Rewrites the errors a user actually runs into.
pub fn friendly_error(error: &Error) -> String {
    let text = error.to_string();
    let lower = text.to_lowercase();
    if matches!(error, Error::Cancelled { .. }) {
        return ui::t!("network.friendly.cancelled").to_string();
    }
    if lower.contains("permission denied (publickey") {
        return ui::t!("network.friendly.ssh_key_rejected").to_string();
    }
    if lower.contains("host key verification failed") {
        return ui::t!("network.friendly.host_key_untrusted").to_string();
    }
    if is_auth_failure(&lower) {
        return ui::t!("network.friendly.auth_failed").to_string();
    }
    if lower.contains("could not resolve host") {
        return ui::t!("network.friendly.resolve_host", detail = text).to_string();
    }
    if lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("operation timed out")
    {
        return ui::t!("network.friendly.host_unreachable", detail = text).to_string();
    }
    if lower.contains("not possible to fast-forward") || lower.contains("diverging branches") {
        return ui::t!("network.friendly.diverged").to_string();
    }
    if lower.contains("no upstream") || lower.contains("no tracking information") {
        return ui::t!("network.friendly.no_upstream").to_string();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_common_network_errors() {
        let error = Error::Git {
            command: "fetch",
            code: Some(128),
            stderr: "git@github.com: Permission denied (publickey).".to_owned(),
        };
        assert!(friendly_error(&error).contains("SSH key"));
        let error = Error::Git {
            command: "pull",
            code: Some(1),
            stderr: "fatal: Not possible to fast-forward, aborting.".to_owned(),
        };
        assert!(friendly_error(&error).contains("fast-forward"));
    }

    fn local(name: &str, upstream: Option<&str>) -> git_core::Ref {
        git_core::Ref {
            full_name: format!("refs/heads/{name}"),
            name: name.to_owned(),
            kind: git_core::RefKind::LocalBranch,
            target: git_core::Oid::from_hex(&"1".repeat(40)).unwrap(),
            upstream: upstream.map(str::to_owned),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    fn uses_of(job: &Job, upstream: Option<&str>, remotes: &[&str]) -> Vec<(String, Direction)> {
        let head = git_core::Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let refs = [local("main", upstream)];
        let remotes: Vec<String> = remotes.iter().map(|r| (*r).to_owned()).collect();
        job_remote_uses(job, Some(&head), Some(&refs), &remotes)
            .into_iter()
            .map(|u| (u.remote, u.direction))
            .collect()
    }

    #[test]
    fn jobs_name_the_remotes_and_urls_they_contact() {
        let fetch_one = Job::Fetch(FetchOptions {
            remote: Some("up".to_owned()),
            tags: true,
            prune: false,
        });
        assert_eq!(
            uses_of(&fetch_one, None, &["origin", "up"]),
            [("up".to_owned(), Direction::Fetch)]
        );
        let fetch_all = Job::Fetch(FetchOptions::default());
        assert_eq!(
            uses_of(&fetch_all, None, &["origin", "up"]),
            [
                ("origin".to_owned(), Direction::Fetch),
                ("up".to_owned(), Direction::Fetch)
            ]
        );
        // Pull follows the upstream; without one there is nothing to look up.
        assert_eq!(
            uses_of(&Job::Pull, Some("up/main"), &["origin", "up"]),
            [("up".to_owned(), Direction::Fetch)]
        );
        assert!(uses_of(&Job::Pull, None, &["origin"]).is_empty());
        // Push: the upstream's remote, else origin, else the first remote.
        assert_eq!(
            uses_of(&Job::Push, Some("up/main"), &["origin", "up"]),
            [("up".to_owned(), Direction::Push)]
        );
        assert_eq!(
            uses_of(&Job::Push, None, &["up", "origin"]),
            [("origin".to_owned(), Direction::Push)]
        );
        assert_eq!(
            uses_of(&Job::Push, None, &["up", "other"]),
            [("up".to_owned(), Direction::Push)]
        );
        let tags = Job::PushTags {
            remote: "up".to_owned(),
        };
        assert_eq!(
            uses_of(&tags, None, &["origin", "up"]),
            [("up".to_owned(), Direction::Push)]
        );
        let sub = Job::UpdateSubmodule { path: "x".into() };
        assert!(uses_of(&sub, None, &["origin"]).is_empty());
    }

    #[test]
    fn authentication_failures_are_recognised() {
        let auth = |text: &str| {
            is_auth_error(&Error::Git {
                command: "push",
                code: Some(128),
                stderr: text.to_owned(),
            })
        };
        assert!(auth(
            "fatal: Authentication failed for 'https://codeberg.org/a/b.git/'"
        ));
        assert!(auth(
            "fatal: could not read Username for 'https://x': terminal prompts disabled"
        ));
        assert!(!auth("git@github.com: Permission denied (publickey)."));
        assert!(!auth("fatal: unable to access: Could not resolve host: x"));
        assert!(!is_auth_error(&Error::Cancelled { command: "push" }));
    }

    #[test]
    fn a_failed_push_target_gets_a_one_line_reason() {
        let (reason, auth) = target_failure("fatal: Authentication failed for 'https://h/x.git/'");
        assert_eq!(reason, "Token invalid or revoked");
        assert!(auth);
        let (reason, auth) = target_failure(
            "To /srv/a.git\n ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs",
        );
        assert!(!auth);
        assert_eq!(reason, "failed to push some refs");
        let (reason, _) = target_failure("ssh: connect to host x port 22: Connection refused");
        assert!(reason.starts_with("The Git host did not answer"));
        assert!(!reason.contains('\n'));
        let (reason, _) = target_failure("remote: nope");
        assert_eq!(reason, "remote: nope");
    }

    /// Runs git in `dir` with no user or system configuration.
    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git is installed");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn tags_and_branch_deletes_go_to_every_push_url_of_a_multi_push_remote() {
        let base = std::env::temp_dir().join(format!(
            "gitwarp-multi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let (work, a, b) = (base.join("work"), base.join("a.git"), base.join("b.git"));
        for dir in [&work, &a, &b] {
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&a, &["init", "-q", "--bare"]);
        git(&b, &["init", "-q", "--bare"]);
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("f"), "x").unwrap();
        git(&work, &["add", "f"]);
        git(&work, &["commit", "-q", "-m", "x"]);
        git(&work, &["remote", "add", "origin", a.to_str().unwrap()]);
        git(
            &work,
            &[
                "remote",
                "set-url",
                "--add",
                "--push",
                "origin",
                a.to_str().unwrap(),
            ],
        );
        git(
            &work,
            &[
                "remote",
                "set-url",
                "--add",
                "--push",
                "origin",
                b.to_str().unwrap(),
            ],
        );
        git(&work, &["tag", "v1"]);
        git(&work, &["branch", "topic"]);

        let git_bin = std::sync::Arc::new(
            git_core::Git::resolve(git_core::GitChoice::System)
                .unwrap()
                .git,
        );
        let repo = Repo::open(git_bin, &work).unwrap();
        let context = NetworkContext::with_env([
            ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        ]);
        let asker = HostKeyAsker::unavailable();
        let cancel = CancelToken::new();

        let one = push_tag(&repo, "origin", "v1", &context, &asker, &cancel, |_| {}).unwrap();
        assert_eq!(one.targets.len(), 2);
        assert!(one.targets.iter().all(|t| t.result.is_ok()));
        git(&work, &["tag", "v2"]);
        let all = push_tags(&repo, "origin", &context, &asker, &cancel, |_| {}).unwrap();
        assert_eq!(all.targets.len(), 2);
        for dir in [&a, &b] {
            assert_eq!(git(dir, &["tag", "--list"]).trim(), "v1\nv2");
        }

        git(&work, &["push", "-q", "origin", "topic"]);
        let gone =
            delete_remote_branch(&repo, "origin", "topic", &context, &asker, &cancel, |_| {})
                .unwrap();
        assert_eq!(gone.targets.len(), 2);
        for dir in [&a, &b] {
            assert!(!git(dir, &["branch", "--list"]).contains("topic"));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fetching_a_pull_request_contacts_its_remote_over_the_fetch_url() {
        let job = Job::FetchPullRequest {
            remote: "origin".into(),
            refspec: "refs/pull/5/head:refs/heads/feat".into(),
            branch: "feat".into(),
            number: 5,
        };
        let uses = job_remote_uses(&job, None, None, &["origin".into(), "fork".into()]);
        assert_eq!(
            uses,
            vec![RemoteUse {
                remote: "origin".into(),
                direction: Direction::Fetch
            }]
        );
    }
}
