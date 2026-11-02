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
    pub fn verb(&self) -> &'static str {
        match self {
            Job::Fetch(_) => "Fetching",
            Job::Pull => "Pulling",
            Job::Push => "Pushing",
            Job::PushTag { .. } | Job::PushTags { .. } => "Pushing tags",
            Job::DeleteRemoteBranch { .. } => "Deleting remote branch",
            Job::UpdateSubmodule { .. } => "Updating submodule",
        }
    }

    /// The success line for the toast, once the job is done.
    pub fn done_message(&self, outcome: &NetworkOutcome) -> String {
        match self {
            Job::Fetch(_) if !outcome.changed => "Already up to date".to_owned(),
            Job::Fetch(_) => "Fetch complete".to_owned(),
            Job::Pull
                if outcome
                    .lines
                    .iter()
                    .any(|line| line.contains("Already up to date")) =>
            {
                "Already up to date".to_owned()
            }
            Job::Pull => "Pull complete".to_owned(),
            Job::Push => "Pushed the current branch".to_owned(),
            Job::PushTag { tag, .. } => format!("Pushed tag {tag}"),
            Job::PushTags { .. } => "Pushed all tags".to_owned(),
            Job::DeleteRemoteBranch { branch, .. } => format!("Deleted remote branch {branch}"),
            Job::UpdateSubmodule { path } => format!("Updated submodule {path}"),
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
                        stderr: format!(
                            "Branch {branch} has no upstream. Push it once or set an upstream."
                        ),
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
                                stderr: "No remote is configured for this repository.".to_owned(),
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
            stderr: "HEAD is detached; check out a branch first.".to_owned(),
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
                        stderr: format!("the host key of {} was not trusted", target.host),
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
                        stderr: format!("the host key of {} was not replaced", target.host),
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
        return "Operation cancelled.".to_owned();
    }
    if lower.contains("permission denied (publickey") {
        return "The SSH key was rejected. Check that your key is loaded in ssh-agent \
                and added to the account on the Git host."
            .to_owned();
    }
    if lower.contains("host key verification failed") {
        return "The SSH host key was not trusted, so the connection was stopped.".to_owned();
    }
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("terminal prompts disabled")
        || lower.contains("invalid username or password")
    {
        return "Authentication failed. Check your credentials or configure a git credential \
                helper."
            .to_owned();
    }
    if lower.contains("could not resolve host") {
        return format!(
            "Could not resolve the host. Check the address and your connection.\n{text}"
        );
    }
    if lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("operation timed out")
    {
        return format!("The Git host did not answer. Check your connection.\n{text}");
    }
    if lower.contains("not possible to fast-forward") || lower.contains("diverging branches") {
        return "The branch has diverged from its upstream. Pull is fast-forward only for now; \
                merge or rebase the branch in a terminal first."
            .to_owned();
    }
    if lower.contains("no upstream") || lower.contains("no tracking information") {
        return "The branch has no upstream. Push it once to create one, or set an upstream."
            .to_owned();
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
