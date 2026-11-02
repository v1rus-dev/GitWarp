//! "Open on <host>": which hosting service a repository's remotes point at and
//! the browser URL of a commit, branch, tag or file there. Pure data and
//! functions, no network, keychain or git: it works without an account for a
//! public service and uses the configured accounts only to know self-hosted
//! instances.

use std::collections::HashSet;

use forge::{
    known_kind_for_host, try_web_url, AccountRef, Instance, RemoteRef, RepoCoords, ServiceKind,
    WebTarget, PUBLIC_INSTANCES,
};
use git_core::{Head, Ref, RefKind};

use crate::remote_accounts::{is_multi_push, RemoteTargets};

/// One repository on one host that a remote (or several) points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLink {
    /// The remotes that lead here, the preferred one first.
    pub remotes: Vec<String>,
    /// The remotes whose FETCH URL is on this host; a subset of `remotes`
    /// (a multi-push remote's push URLs can lead elsewhere).
    pub fetch_remotes: Vec<String>,
    pub instance: Instance,
    pub coords: RepoCoords,
}

impl HostLink {
    /// The remote shown in "Open on <Service> (<remote>)".
    pub fn remote(&self) -> &str {
        &self.remotes[0]
    }

    /// Whether `remote` leads to this repository.
    pub fn serves(&self, remote: &str) -> bool {
        self.remotes.iter().any(|r| r == remote)
    }

    /// Whether `remote`'s fetch URL is on this host: the remote's own page.
    pub fn fetches(&self, remote: &str) -> bool {
        self.fetch_remotes.iter().any(|r| r == remote)
    }

    /// The service as the menus name it: "Codeberg" for codeberg.org, "Gitea"
    /// for the other Gitea / Forgejo hosts, else the service's own name.
    pub fn service_name(&self) -> &'static str {
        match self.instance.kind {
            ServiceKind::Gitea if self.instance.host() == "codeberg.org" => "Codeberg",
            ServiceKind::Gitea => "Gitea",
            kind => kind.display_name(),
        }
    }

    /// Host for the "Opened <host> in your browser" toast.
    pub fn host(&self) -> &str {
        self.instance.host()
    }

    /// The page of `target`, `None` when the service has none (blame on
    /// Azure DevOps).
    pub fn url(&self, target: &WebTarget) -> Option<String> {
        try_web_url(&self.instance, &self.coords, target)
    }
}

/// The instance a remote URL belongs to: a configured account's (self-hosted
/// or not) first, then the public service of its host.
fn instance_for(refs: &[AccountRef], remote: &RemoteRef) -> Option<(Instance, RepoCoords)> {
    for account in refs {
        if let Some(coords) = account.instance.matches(remote) {
            return Some((account.instance.clone(), coords));
        }
    }
    let kind = known_kind_for_host(&remote.host)?;
    let host = PUBLIC_INSTANCES
        .iter()
        .find(|&&(k, _)| k == kind)
        .map(|&(_, h)| h)?;
    let instance = Instance::new(kind, &format!("https://{host}")).ok()?;
    let coords = instance.matches(remote)?;
    Some((instance, coords))
}

/// The remote the current branch's upstream lives on, if any.
pub fn upstream_remote(
    head: Option<&Head>,
    refs: &[Ref],
    targets: &[RemoteTargets],
) -> Option<String> {
    let Some(Head::Branch { name, .. }) = head else {
        return None;
    };
    let upstream = refs
        .iter()
        .find(|r| r.kind == RefKind::LocalBranch && r.name == *name)?
        .upstream
        .as_deref()?;
    split_remote_ref(upstream, targets).map(|(remote, _)| remote.to_owned())
}

/// Splits `origin/feat/x` into the remote and the branch, over the longest
/// configured remote name that prefixes it.
pub fn split_remote_ref<'a>(
    name: &'a str,
    targets: &[RemoteTargets],
) -> Option<(&'a str, &'a str)> {
    targets
        .iter()
        .filter_map(|t| {
            let rest = name.strip_prefix(t.name.as_str())?.strip_prefix('/')?;
            (!rest.is_empty()).then_some((&name[..t.name.len()], rest))
        })
        .max_by_key(|(remote, _)| remote.len())
}

/// Every repository the remotes point at on a known or configured host, one
/// entry per (instance, repository), in the order the commit menu lists them:
/// the `preferred` remote (the current branch's upstream) first, then
/// `origin`, then the rest by name. A multi-push remote also contributes its
/// push URLs.
pub fn host_links(
    refs: &[AccountRef],
    targets: &[RemoteTargets],
    preferred: Option<&str>,
) -> Vec<HostLink> {
    let mut ordered: Vec<&RemoteTargets> = targets.iter().collect();
    ordered.sort_by_key(|t| {
        let rank = if Some(t.name.as_str()) == preferred {
            0
        } else if t.name == "origin" {
            1
        } else {
            2
        };
        (rank, t.name.clone())
    });
    let mut links: Vec<HostLink> = Vec::new();
    let mut seen: HashSet<(String, Vec<String>, String)> = HashSet::new();
    for target in ordered {
        let mut urls = vec![target.fetch_url.as_str()];
        if is_multi_push(target) {
            urls.extend(target.push_urls.iter().map(String::as_str));
        }
        for (index, url) in urls.into_iter().enumerate() {
            let is_fetch = index == 0;
            let Some(remote) = RemoteRef::parse(url) else {
                continue;
            };
            let Some((instance, coords)) = instance_for(refs, &remote) else {
                continue;
            };
            let key = (
                instance.base_url.clone(),
                coords.namespace.clone(),
                coords.repo.clone(),
            );
            if seen.insert(key) {
                links.push(HostLink {
                    remotes: vec![target.name.clone()],
                    fetch_remotes: if is_fetch {
                        vec![target.name.clone()]
                    } else {
                        Vec::new()
                    },
                    instance,
                    coords,
                });
            } else if let Some(link) = links
                .iter_mut()
                .find(|l| l.instance == instance && l.coords == coords)
            {
                if !link.serves(&target.name) {
                    link.remotes.push(target.name.clone());
                }
                if is_fetch && !link.fetches(&target.name) {
                    link.fetch_remotes.push(target.name.clone());
                }
            }
        }
    }
    links
}

/// The link and target of "Open on <host>" for a sidebar branch key:
/// `local:<name>` follows its upstream (none: no item), `rbranch:<remote>/<name>`
/// its own remote.
pub fn branch_target<'a>(
    links: &'a [HostLink],
    targets: &[RemoteTargets],
    refs: &[Ref],
    key: &str,
) -> Option<(&'a HostLink, WebTarget)> {
    let (remote, branch) = if let Some(name) = key.strip_prefix("local:") {
        let upstream = refs
            .iter()
            .find(|r| r.kind == RefKind::LocalBranch && r.name == name)?
            .upstream
            .as_deref()?;
        split_remote_ref(upstream, targets)?
    } else {
        split_remote_ref(key.strip_prefix("rbranch:")?, targets)?
    };
    let link = links.iter().find(|l| l.fetches(remote))?;
    Some((link, WebTarget::Branch(branch.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::Instance;
    use git_core::Oid;

    fn targets(list: &[(&str, &str)]) -> Vec<RemoteTargets> {
        list.iter()
            .map(|(name, url)| RemoteTargets {
                name: (*name).into(),
                fetch_url: (*url).into(),
                push_urls: vec![(*url).into()],
            })
            .collect()
    }

    fn account(id: &str, kind: ServiceKind, url: &str) -> AccountRef {
        AccountRef {
            id: id.into(),
            instance: Instance::new(kind, url).unwrap(),
            login: "me".into(),
        }
    }

    fn oid() -> Oid {
        Oid::from_hex(&"a".repeat(40)).unwrap()
    }

    fn branch(name: &str, upstream: Option<&str>) -> Ref {
        Ref {
            full_name: format!("refs/heads/{name}"),
            name: name.into(),
            kind: RefKind::LocalBranch,
            target: oid(),
            upstream: upstream.map(str::to_owned),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

    #[test]
    fn public_hosts_work_without_an_account() {
        let t = targets(&[("origin", "git@github.com:o/r.git")]);
        let links = host_links(&[], &t, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].service_name(), "GitHub");
        assert_eq!(
            links[0].url(&WebTarget::Commit("abc".into())).unwrap(),
            "https://github.com/o/r/commit/abc"
        );
    }

    #[test]
    fn unknown_hosts_are_excluded_until_an_account_names_them() {
        let t = targets(&[("origin", "https://git.corp.example/g/r.git")]);
        assert!(host_links(&[], &t, None).is_empty());
        let accounts = [account(
            "a",
            ServiceKind::GitLab,
            "https://git.corp.example",
        )];
        let links = host_links(&accounts, &t, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].service_name(), "GitLab");
        assert_eq!(
            links[0].url(&WebTarget::Repo).unwrap(),
            "https://git.corp.example/g/r"
        );
    }

    #[test]
    fn a_local_path_is_not_a_host() {
        let t = targets(&[("origin", "/srv/git/r.git"), ("up", "../other")]);
        assert!(host_links(&[], &t, None).is_empty());
    }

    #[test]
    fn codeberg_and_other_gitea_hosts_are_named_apart() {
        let accounts = [account("g", ServiceKind::Gitea, "https://git.example.org")];
        let t = targets(&[
            ("origin", "https://codeberg.org/o/r.git"),
            ("home", "https://git.example.org/o/r.git"),
        ]);
        let links = host_links(&accounts, &t, None);
        let names: Vec<_> = links.iter().map(HostLink::service_name).collect();
        assert_eq!(names, ["Codeberg", "Gitea"]);
    }

    #[test]
    fn the_same_repository_through_two_remotes_is_one_host() {
        let t = targets(&[
            ("origin", "https://github.com/o/r.git"),
            ("mirror", "git@github.com:o/r.git"),
        ]);
        let links = host_links(&[], &t, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].remotes, ["origin", "mirror"]);
        assert!(links[0].serves("mirror"));
    }

    #[test]
    fn two_repositories_make_two_hosts_ordered_upstream_origin_name() {
        let t = targets(&[
            ("zed", "https://gitlab.com/g/r.git"),
            ("publish", "https://github.com/me/r.git"),
            ("origin", "https://github.com/o/r.git"),
        ]);
        let order = |preferred| {
            host_links(&[], &t, preferred)
                .iter()
                .map(|l| l.remote().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(None), ["origin", "publish", "zed"]);
        assert_eq!(order(Some("zed")), ["zed", "origin", "publish"]);
    }

    #[test]
    fn a_multi_push_remote_adds_its_push_hosts() {
        let mut t = targets(&[("origin", "https://github.com/o/r.git")]);
        t[0].push_urls = vec![
            "https://github.com/o/r.git".into(),
            "https://gitlab.com/g/r.git".into(),
        ];
        let links = host_links(&[], &t, None);
        let names: Vec<_> = links.iter().map(HostLink::service_name).collect();
        assert_eq!(names, ["GitHub", "GitLab"]);
    }

    #[test]
    fn credentials_in_the_remote_url_never_reach_the_link() {
        let t = targets(&[("origin", "https://user:secret@github.com/o/r.git")]);
        let links = host_links(&[], &t, None);
        let url = links[0].url(&WebTarget::Branch("main".into())).unwrap();
        assert_eq!(url, "https://github.com/o/r/tree/main");
        assert!(!url.contains("user") && !url.contains("secret"));
    }

    #[test]
    fn azure_blame_is_hidden() {
        let t = targets(&[("origin", "https://dev.azure.com/org/proj/_git/repo")]);
        let links = host_links(&[], &t, None);
        let blame = WebTarget::Blame {
            rev: "abc".into(),
            path: "a.rs".into(),
        };
        assert_eq!(links[0].url(&blame), None);
    }

    #[test]
    fn upstream_remote_of_the_current_branch() {
        let t = targets(&[("origin", "x"), ("my/fork", "y")]);
        let refs = [branch("main", Some("my/fork/main")), branch("loose", None)];
        let head = |n: &str| Head::Branch {
            name: n.into(),
            target: None,
        };
        assert_eq!(
            upstream_remote(Some(&head("main")), &refs, &t).as_deref(),
            Some("my/fork")
        );
        assert_eq!(upstream_remote(Some(&head("loose")), &refs, &t), None);
        assert_eq!(
            upstream_remote(Some(&Head::Detached(oid())), &refs, &t),
            None
        );
        assert_eq!(upstream_remote(None, &refs, &t), None);
    }

    #[test]
    fn a_remote_opens_on_its_fetch_host_not_a_push_host() {
        let targets = vec![
            RemoteTargets {
                name: "a".into(),
                fetch_url: "https://gitlab.com/g/r.git".into(),
                push_urls: vec!["https://gitlab.com/g/r.git".into()],
            },
            RemoteTargets {
                name: "origin".into(),
                fetch_url: "https://github.com/o/r.git".into(),
                push_urls: vec![
                    "https://gitlab.com/g/r.git".into(),
                    "https://github.com/o/r.git".into(),
                ],
            },
        ];
        let links = host_links(&[], &targets, Some("a"));
        let host_of = |remote: &str| {
            links
                .iter()
                .find(|l| l.fetches(remote))
                .map(|l| l.host().to_owned())
        };
        assert_eq!(host_of("a").as_deref(), Some("gitlab.com"));
        assert_eq!(host_of("origin").as_deref(), Some("github.com"));
        let (link, _) = branch_target(&links, &targets, &[], "rbranch:origin/dev").unwrap();
        assert_eq!(link.host(), "github.com");
    }

    #[test]
    fn branch_targets_follow_upstream_or_remote() {
        let t = targets(&[
            ("origin", "https://github.com/o/r.git"),
            ("pub", "https://gitlab.com/g/r.git"),
        ]);
        let links = host_links(&[], &t, None);
        let refs = [
            branch("feat", Some("pub/feat/x")),
            branch("main", Some("origin/main")),
            branch("loose", None),
            branch("tracks-local", Some("main")),
        ];
        let target = |key: &str| branch_target(&links, &t, &refs, key);

        let (link, what) = target("local:feat").unwrap();
        assert_eq!(link.remote(), "pub");
        assert_eq!(what, WebTarget::Branch("feat/x".into()));

        let (link, what) = target("local:main").unwrap();
        assert_eq!(link.remote(), "origin");
        assert_eq!(what, WebTarget::Branch("main".into()));

        assert!(target("local:loose").is_none());
        assert!(target("local:tracks-local").is_none());
        assert!(target("local:missing").is_none());

        let (link, what) = target("rbranch:origin/release/1.0").unwrap();
        assert_eq!(link.remote(), "origin");
        assert_eq!(what, WebTarget::Branch("release/1.0".into()));
        assert!(target("rbranch:unknown/x").is_none());
    }

    #[test]
    fn a_branch_on_a_merged_duplicate_remote_still_finds_its_host() {
        let t = targets(&[
            ("origin", "https://github.com/o/r.git"),
            ("mirror", "git@github.com:o/r.git"),
        ]);
        let links = host_links(&[], &t, None);
        assert!(branch_target(&links, &t, &[], "rbranch:mirror/dev").is_some());
    }
}
