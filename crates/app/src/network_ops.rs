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
}

impl Job {
    /// The word shown in the status bar and in toasts.
    pub fn verb(&self) -> SharedString {
        match self {
            Job::Fetch(_) => ui::t!("network.verb.fetching"),
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
    let urls = remote_urls(repo, remote.as_deref())?;
    preflight(&urls, asker, cancel)?;
    network::push(repo, options, context, cancel, on_progress)
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
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("terminal prompts disabled")
        || lower.contains("invalid username or password")
    {
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
}
