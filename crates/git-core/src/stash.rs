use crate::{check_output, Error, Oid, Repo, Result};

/// One entry of the stash reflog, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// 0 for `stash@{0}`.
    pub index: usize,
    /// The stash commit.
    pub oid: Oid,
    /// The reflog subject, e.g. `WIP on main: 1a2b3c4 subject` or `On main: message`.
    pub message: String,
}

pub(crate) fn list(repo: &Repo) -> Result<Vec<Stash>> {
    // `git stash list` refuses to run without a work tree; a bare repository
    // simply has no stash to show.
    if repo.is_bare() {
        return Ok(Vec::new());
    }
    let out = repo
        .command()
        .args(["stash", "list", "--format=%gd%x1f%H%x1f%gs"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "stash list")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter(|l| !l.is_empty())
        .map(parse_stash_line)
        .collect()
}

/// One `stash@{N}\x1f<oid>\x1f<subject>` line.
fn parse_stash_line(line: &str) -> Result<Stash> {
    // The subject is last, so separators inside it survive.
    let mut f = line.splitn(3, '\x1f');
    let selector = f.next().ok_or(Error::Parse("stash selector"))?;
    let oid = f.next().ok_or(Error::Parse("stash commit"))?;
    let message = f.next().unwrap_or("");
    Ok(Stash {
        index: parse_index(selector).ok_or(Error::Parse("stash selector"))?,
        oid: Oid::from_hex(oid).ok_or(Error::Parse("stash commit"))?,
        message: message.to_owned(),
    })
}

/// The `N` of `stash@{N}`.
fn parse_index(selector: &str) -> Option<usize> {
    selector
        .strip_prefix("stash@{")?
        .strip_suffix('}')?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn parses_a_wip_entry() {
        let s =
            parse_stash_line(&format!("stash@{{2}}\x1f{OID}\x1fWIP on main: 1a2b3c4 fix")).unwrap();
        assert_eq!(s.index, 2);
        assert_eq!(s.oid.to_string(), OID);
        assert_eq!(s.message, "WIP on main: 1a2b3c4 fix");
    }

    #[test]
    fn parses_an_entry_without_a_separator_in_the_subject() {
        let s = parse_stash_line(&format!("stash@{{0}}\x1f{OID}\x1fOn main: message")).unwrap();
        assert_eq!((s.index, s.message.as_str()), (0, "On main: message"));
    }

    #[test]
    fn keeps_a_separator_inside_the_subject() {
        let s = parse_stash_line(&format!("stash@{{0}}\x1f{OID}\x1fa\x1fb")).unwrap();
        assert_eq!(s.message, "a\x1fb");
    }

    #[test]
    fn rejects_a_malformed_selector_or_commit() {
        assert!(parse_stash_line(&format!("HEAD\x1f{OID}\x1fm")).is_err());
        assert!(parse_stash_line("stash@{0}\x1fnot-an-oid\x1fm").is_err());
    }

    #[test]
    fn parse_index_reads_the_number() {
        assert_eq!(parse_index("stash@{12}"), Some(12));
        assert_eq!(parse_index("stash@{x}"), None);
        assert_eq!(parse_index("refs/stash"), None);
    }
}
