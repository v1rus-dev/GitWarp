use crate::{check_output, Error, Oid, Repo, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
    Stash,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Full name, e.g. `refs/heads/main`.
    pub full_name: String,
    /// Short name for the sidebar, e.g. `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub kind: RefKind,
    /// The commit the ref points at (annotated tags are peeled).
    pub target: Oid,
    /// For local branches: their upstream, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// For local branches with an upstream: commits ahead / behind it.
    pub ahead: usize,
    pub behind: usize,
    /// The configured upstream no longer exists (`[gone]`).
    pub upstream_gone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch { name: String, target: Option<Oid> },
    Detached(Oid),
}

pub(crate) fn list(repo: &Repo) -> Result<Vec<Ref>> {
    let out = repo
        .command()
        .args([
            "for-each-ref",
            "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)",
        ])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "for-each-ref")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter(|l| !l.is_empty())
        .map(parse_ref_line)
        .collect()
}

fn parse_ref_line(line: &str) -> Result<Ref> {
    let mut f = line.split('\x1f');
    let full_name = f.next().ok_or(Error::Parse("ref name"))?.to_owned();
    let object = f.next().ok_or(Error::Parse("ref target"))?;
    let peeled = f.next().unwrap_or("");
    let upstream = f.next().filter(|u| !u.is_empty()).map(str::to_owned);
    let (ahead, behind, upstream_gone) = parse_track(f.next().unwrap_or(""));

    let target = Oid::from_hex(if peeled.is_empty() { object } else { peeled })
        .ok_or(Error::Parse("ref target"))?;
    let (kind, name) = classify(&full_name);
    Ok(Ref {
        name: name.to_owned(),
        full_name,
        kind,
        target,
        upstream,
        ahead,
        behind,
        upstream_gone,
    })
}

/// `%(upstream:track,nobracket)`: empty, `ahead 3`, `behind 2`,
/// `ahead 3, behind 2` or `gone`. Unknown values count as no divergence, so a
/// newer git adding one does not break the list.
fn parse_track(track: &str) -> (usize, usize, bool) {
    let mut ahead = 0;
    let mut behind = 0;
    let mut gone = false;
    for part in track.split(',') {
        let part = part.trim();
        if part == "gone" {
            gone = true;
        } else if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind, gone)
}

/// The configured remotes, one name per line, in git's order.
pub(crate) fn remotes(repo: &Repo) -> Result<Vec<String>> {
    let out = repo
        .command()
        .arg("remote")
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "remote")?;
    Ok(parse_remotes(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_remotes(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn classify(full: &str) -> (RefKind, &str) {
    if let Some(n) = full.strip_prefix("refs/heads/") {
        (RefKind::LocalBranch, n)
    } else if let Some(n) = full.strip_prefix("refs/remotes/") {
        (RefKind::RemoteBranch, n)
    } else if let Some(n) = full.strip_prefix("refs/tags/") {
        (RefKind::Tag, n)
    } else if full == "refs/stash" {
        (RefKind::Stash, "stash")
    } else {
        (RefKind::Other, full)
    }
}

pub(crate) fn head(repo: &Repo) -> Result<Head> {
    let sym = repo
        .command()
        .args(["symbolic-ref", "-q", "--short", "HEAD"])
        .output()?;
    let target = repo
        .command()
        .args(["rev-parse", "-q", "--verify", "HEAD"])
        .output()?;
    let target = Oid::from_hex(String::from_utf8_lossy(&target.stdout).trim());

    if sym.status.success() {
        let name = String::from_utf8_lossy(&sym.stdout).trim().to_owned();
        Ok(Head::Branch { name, target })
    } else {
        target.map(Head::Detached).ok_or(Error::Parse("HEAD"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_refs() {
        let oid = "1".repeat(40);
        let tag = "2".repeat(40);
        let r = parse_ref_line(&format!(
            "refs/heads/feature/x\x1f{oid}\x1f\x1forigin/feature/x\x1f"
        ))
        .unwrap();
        assert_eq!(
            (r.kind, r.name.as_str()),
            (RefKind::LocalBranch, "feature/x")
        );
        assert_eq!(r.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((r.ahead, r.behind, r.upstream_gone), (0, 0, false));

        // Annotated tag: the peeled commit wins over the tag object.
        let r = parse_ref_line(&format!("refs/tags/v1\x1f{oid}\x1f{tag}\x1f\x1f")).unwrap();
        assert_eq!(r.kind, RefKind::Tag);
        assert_eq!(r.target.to_string(), tag);
    }

    #[test]
    fn parses_every_track_value() {
        assert_eq!(parse_track(""), (0, 0, false));
        assert_eq!(parse_track("ahead 3"), (3, 0, false));
        assert_eq!(parse_track("behind 2"), (0, 2, false));
        assert_eq!(parse_track("ahead 3, behind 2"), (3, 2, false));
        assert_eq!(parse_track("gone"), (0, 0, true));
        // git has no other values today; unknown ones are ignored.
        assert_eq!(parse_track("sideways 4"), (0, 0, false));
    }

    #[test]
    fn parses_remote_names_in_order() {
        assert!(parse_remotes("").is_empty());
        assert!(parse_remotes("\n").is_empty());
        assert_eq!(parse_remotes("zeta\nalpha\n"), ["zeta", "alpha"]);
    }
}
