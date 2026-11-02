use crate::encode::{encode_path, encode_segment};
use crate::remote::RepoCoords;
use crate::{Instance, ServiceKind};

/// What to open on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebTarget {
    Repo,
    Commit(String),
    Branch(String),
    Tag(String),
    File { rev: String, path: String },
    PullRequest(u64),
}

/// Browser URL of `target` in the repository `coords` on `instance`.
/// Branch, tag and file names are percent-encoded per segment.
/// Bitbucket Server uses the Bitbucket Cloud layout (spec covers cloud only).
pub fn web_url(instance: &Instance, coords: &RepoCoords, target: &WebTarget) -> String {
    let project = project_url(instance, coords);
    match instance.kind {
        ServiceKind::GitHub => format!("{project}{}", github(target)),
        ServiceKind::GitLab => format!("{project}{}", gitlab(target)),
        ServiceKind::Gitea => format!("{project}{}", gitea(target)),
        ServiceKind::Bitbucket => format!("{project}{}", bitbucket(target)),
        ServiceKind::AzureDevOps => format!("{project}{}", azure(target)),
    }
}

fn project_url(instance: &Instance, coords: &RepoCoords) -> String {
    let mut url = instance.base_url.clone();
    for segment in &coords.namespace {
        url.push('/');
        url.push_str(&encode_segment(segment));
    }
    if instance.kind == ServiceKind::AzureDevOps {
        url.push_str("/_git");
    }
    url.push('/');
    url.push_str(&encode_segment(&coords.repo));
    url
}

fn github(t: &WebTarget) -> String {
    match t {
        WebTarget::Repo => String::new(),
        WebTarget::Commit(sha) => format!("/commit/{}", encode_segment(sha)),
        WebTarget::Branch(b) => format!("/tree/{}", encode_path(b)),
        WebTarget::Tag(tag) => format!("/releases/tag/{}", encode_path(tag)),
        WebTarget::File { rev, path } => {
            format!("/blob/{}/{}", encode_path(rev), encode_path(path))
        }
        WebTarget::PullRequest(n) => format!("/pull/{n}"),
    }
}

fn gitlab(t: &WebTarget) -> String {
    match t {
        WebTarget::Repo => String::new(),
        WebTarget::Commit(sha) => format!("/-/commit/{}", encode_segment(sha)),
        WebTarget::Branch(b) => format!("/-/tree/{}", encode_path(b)),
        WebTarget::Tag(tag) => format!("/-/tags/{}", encode_path(tag)),
        WebTarget::File { rev, path } => {
            format!("/-/blob/{}/{}", encode_path(rev), encode_path(path))
        }
        WebTarget::PullRequest(n) => format!("/-/merge_requests/{n}"),
    }
}

fn gitea(t: &WebTarget) -> String {
    match t {
        WebTarget::Repo => String::new(),
        WebTarget::Commit(sha) => format!("/commit/{}", encode_segment(sha)),
        WebTarget::Branch(b) => format!("/src/branch/{}", encode_path(b)),
        WebTarget::Tag(tag) => format!("/src/tag/{}", encode_path(tag)),
        WebTarget::File { rev, path } => {
            format!("/src/commit/{}/{}", encode_path(rev), encode_path(path))
        }
        WebTarget::PullRequest(n) => format!("/pulls/{n}"),
    }
}

fn bitbucket(t: &WebTarget) -> String {
    match t {
        WebTarget::Repo => String::new(),
        WebTarget::Commit(sha) => format!("/commits/{}", encode_segment(sha)),
        WebTarget::Branch(b) => format!("/branch/{}", encode_path(b)),
        WebTarget::Tag(tag) => format!("/src/{}", encode_path(tag)),
        WebTarget::File { rev, path } => format!("/src/{}/{}", encode_path(rev), encode_path(path)),
        WebTarget::PullRequest(n) => format!("/pull-requests/{n}"),
    }
}

fn azure(t: &WebTarget) -> String {
    match t {
        WebTarget::Repo => String::new(),
        WebTarget::Commit(sha) => format!("/commit/{}", encode_segment(sha)),
        WebTarget::Branch(b) => format!("?version=GB{}", encode_path(b)),
        WebTarget::Tag(tag) => format!("?version=GT{}", encode_path(tag)),
        WebTarget::File { rev, path } => {
            format!(
                "?path=/{}&version=GC{}",
                encode_path(path),
                encode_path(rev)
            )
        }
        WebTarget::PullRequest(n) => format!("/pullrequest/{n}"),
    }
}
