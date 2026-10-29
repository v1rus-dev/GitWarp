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
            "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)",
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

    let target = Oid::from_hex(if peeled.is_empty() { object } else { peeled })
        .ok_or(Error::Parse("ref target"))?;
    let (kind, name) = classify(&full_name);
    Ok(Ref {
        name: name.to_owned(),
        full_name,
        kind,
        target,
        upstream,
    })
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
            "refs/heads/feature/x\x1f{oid}\x1f\x1forigin/feature/x"
        ))
        .unwrap();
        assert_eq!(
            (r.kind, r.name.as_str()),
            (RefKind::LocalBranch, "feature/x")
        );
        assert_eq!(r.upstream.as_deref(), Some("origin/feature/x"));

        // Annotated tag: the peeled commit wins over the tag object.
        let r = parse_ref_line(&format!("refs/tags/v1\x1f{oid}\x1f{tag}\x1f")).unwrap();
        assert_eq!(r.kind, RefKind::Tag);
        assert_eq!(r.target.to_string(), tag);
    }
}
