//! Stage 7 shared pieces: the commit card the branch/tag/reset dialogs show,
//! the action a history row's menu asks the shell to run, and the pure
//! validation the dialogs use.
//!
//! Everything here is free of windows and views, so the names, errors and menu decisions
//! are unit-testable without a window.

use git_core::{CommitInfo, Head, Oid, Ref, RefKind};
use gpui::SharedString;

/// One ref chip on a [`CommitCard`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRef {
    pub name: String,
    pub kind: CardRefKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardRefKind {
    Local,
    Remote,
    Tag,
    Head,
}

/// The commit summary card at the top of the branch, tag and reset dialogs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitCard {
    pub oid: Oid,
    pub short: String,
    pub summary: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub refs: Vec<CardRef>,
}

impl CommitCard {
    /// A card with only a hash, for a commit the history has not loaded (a tag
    /// pointing outside the loaded page, for example).
    pub fn bare(oid: Oid) -> Self {
        let mut short = oid.to_string();
        short.truncate(7);
        Self {
            oid,
            short,
            summary: String::new(),
            author: String::new(),
            time: 0,
            refs: Vec::new(),
        }
    }

    /// Builds the card from a loaded commit, its repository's refs and HEAD.
    pub fn from_commit(commit: &CommitInfo, refs: &[Ref], head: Option<&Head>) -> Self {
        let mut short = commit.id.to_string();
        short.truncate(7);
        let mut cards: Vec<CardRef> = refs
            .iter()
            .filter(|r| r.target == commit.id)
            .filter_map(|r| {
                let kind = match r.kind {
                    RefKind::LocalBranch => CardRefKind::Local,
                    RefKind::RemoteBranch => CardRefKind::Remote,
                    RefKind::Tag => CardRefKind::Tag,
                    _ => return None,
                };
                Some(CardRef {
                    name: r.name.clone(),
                    kind,
                })
            })
            .collect();
        // A detached HEAD is not a ref but still tells the user where they are.
        if matches!(head, Some(Head::Detached(oid)) if *oid == commit.id) {
            cards.insert(
                0,
                CardRef {
                    name: "HEAD".to_owned(),
                    kind: CardRefKind::Head,
                },
            );
        }
        cards.truncate(3);
        Self {
            oid: commit.id,
            short,
            summary: commit.summary.clone(),
            author: commit.author_name.clone(),
            time: commit.author_time,
            refs: cards,
        }
    }
}

/// What a history row's menu asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefAction {
    /// Checks out a local branch, asking what to do with uncommitted changes.
    CheckoutBranch(String),
    /// Checks out the commit with a detached HEAD.
    CheckoutCommit(CommitCard),
    CreateBranch(CommitCard),
    CreateTag(CommitCard),
    CherryPick(CommitCard),
    Revert(CommitCard),
    Reset(CommitCard),
}

/// Why a proposed ref name cannot be used, or `None` when it can.
///
/// The messages are the design's: a name with whitespace, one with characters
/// git forbids, one with a forbidden start or end, and one already taken.
pub fn ref_name_error(name: &str, taken: &[String]) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.chars().any(char::is_whitespace) {
        return Some(ui::t!("dialogs.ref_name.spaces").to_string());
    }
    let forbidden =
        name.contains("..") || name.contains("@{") || name.chars().any(|c| "~^:?*[\\".contains(c));
    if forbidden {
        return Some(ui::t!("dialogs.ref_name.forbidden_chars").to_string());
    }
    if name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
    {
        return Some(ui::t!("dialogs.ref_name.bad_edges").to_string());
    }
    if taken.iter().any(|existing| existing == name) {
        return Some(ui::t!("dialogs.ref_name.exists", name = name).to_string());
    }
    None
}

/// Why a proposed remote name cannot be used, or `None`.
pub fn remote_name_error(name: &str, taken: &[String]) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.chars().any(char::is_whitespace) {
        return Some(ui::t!("dialogs.remote_name.spaces").to_string());
    }
    if taken.iter().any(|existing| existing == name) {
        return Some(ui::t!("dialogs.remote_name.exists", name = name).to_string());
    }
    None
}

/// What kind of address the remote dialog sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteUrlKind {
    Https,
    Ssh,
    Bad,
}

impl RemoteUrlKind {
    /// The phrase the protocol note uses; the dialog also colours it.
    pub fn label(self) -> SharedString {
        match self {
            RemoteUrlKind::Https => ui::t!("dialogs.remote_url.https"),
            RemoteUrlKind::Ssh => ui::t!("dialogs.remote_url.ssh"),
            RemoteUrlKind::Bad => ui::t!("dialogs.remote_url.bad"),
        }
    }
}

/// Classifies a remote address the way the design's dialog does.
pub fn remote_url_kind(url: &str) -> RemoteUrlKind {
    let url = url.trim();
    if url.is_empty() {
        return RemoteUrlKind::Bad;
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return RemoteUrlKind::Https;
    }
    if url.starts_with("ssh://") || url.starts_with("git://") {
        return RemoteUrlKind::Ssh;
    }
    // The scp-like form `user@host:path`; both halves must be non-empty and the
    // whole thing free of whitespace.
    let Some((user, rest)) = url.split_once('@') else {
        return RemoteUrlKind::Bad;
    };
    let Some((host, path)) = rest.split_once(':') else {
        return RemoteUrlKind::Bad;
    };
    if user.is_empty() || host.is_empty() || path.is_empty() || url.chars().any(char::is_whitespace)
    {
        RemoteUrlKind::Bad
    } else {
        RemoteUrlKind::Ssh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn accepts_ordinary_branch_names() {
        assert_eq!(ref_name_error("feat/my-change", &[]), None);
        assert_eq!(ref_name_error("v0.3.1", &[]), None);
        assert_eq!(ref_name_error("", &[]), None);
    }

    #[test]
    fn rejects_the_names_the_design_rejects() {
        assert!(ref_name_error("my branch", &[]).unwrap().contains("spaces"));
        for bad in [
            "a..b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b", "a@{b",
        ] {
            assert!(ref_name_error(bad, &[]).is_some(), "{bad}");
        }
        for bad in ["-x", "/x", "x/", "x.", "x.lock"] {
            assert!(ref_name_error(bad, &[]).is_some(), "{bad}");
        }
    }

    #[test]
    fn rejects_a_taken_name() {
        let error = ref_name_error("main", &taken(&["main"])).unwrap();
        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn remote_names_cannot_have_spaces_or_repeat() {
        assert_eq!(remote_name_error("fork", &[]), None);
        assert!(remote_name_error("my fork", &[]).is_some());
        assert!(remote_name_error("origin", &taken(&["origin"]))
            .unwrap()
            .contains("already exists"));
    }

    #[test]
    fn classifies_remote_addresses() {
        assert_eq!(
            remote_url_kind("https://github.com/acme/repo.git"),
            RemoteUrlKind::Https
        );
        assert_eq!(
            remote_url_kind("git@github.com:acme/repo.git"),
            RemoteUrlKind::Ssh
        );
        assert_eq!(
            remote_url_kind("ssh://git@example.com/acme/repo.git"),
            RemoteUrlKind::Ssh
        );
        assert_eq!(remote_url_kind("github.com/mira"), RemoteUrlKind::Bad);
        assert_eq!(remote_url_kind("git@github.com:"), RemoteUrlKind::Bad);
        assert_eq!(remote_url_kind(""), RemoteUrlKind::Bad);
    }
}
