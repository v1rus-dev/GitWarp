//! Submodules as recorded in `.gitmodules` and the index.
//!
//! The state comes from `git submodule status` alone, without a second
//! `git status` run. That means a submodule whose work tree is dirty but
//! still on the recorded commit shows as [`SubmoduleState::Clean`]; a full
//! status belongs to stage 6.

use crate::{check_output, Error, Oid, Repo, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleState {
    /// Checked out at the recorded commit (the work tree may still be dirty).
    Clean,
    /// The checked-out commit differs from the one the superproject records.
    Modified,
    /// Not checked out yet.
    Uninitialized,
    /// A merge left conflicting submodule entries.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// Path relative to the work-tree root, `/`-separated.
    pub path: String,
    /// The commit `git submodule status` reported (the checked-out one).
    pub oid: Option<Oid>,
    pub state: SubmoduleState,
}

pub(crate) fn list(repo: &Repo) -> Result<Vec<Submodule>> {
    // `git submodule status` refuses to run without a work tree; a bare
    // repository has no submodules to show.
    if repo.is_bare() {
        return Ok(Vec::new());
    }
    let out = repo
        .command()
        .args(["submodule", "status"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "submodule status")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter(|l| !l.is_empty())
        .map(parse_status_line)
        .collect()
}

/// One `git submodule status` line: a prefix character, the commit, a space,
/// the path and optionally ` (describe)`.
fn parse_status_line(line: &str) -> Result<Submodule> {
    let mut chars = line.chars();
    let state = match chars.next() {
        Some(' ') => SubmoduleState::Clean,
        Some('+') => SubmoduleState::Modified,
        Some('-') => SubmoduleState::Uninitialized,
        Some('U') => SubmoduleState::Conflict,
        _ => return Err(Error::Parse("submodule status")),
    };
    let (oid, rest) = line[1..]
        .split_once(' ')
        .ok_or(Error::Parse("submodule status"))?;
    // git prints no ` (describe)` for an uninitialized submodule: whatever
    // follows the oid is the path, parentheses included.
    let path = if state == SubmoduleState::Uninitialized {
        rest
    } else {
        strip_describe(rest)
    }
    .to_owned();
    if path.is_empty() {
        return Err(Error::Parse("submodule path"));
    }
    Ok(Submodule {
        path,
        oid: Oid::from_hex(oid),
        state,
    })
}

/// Drops git's trailing ` (describe)` or ` (heads/main)` decoration. Only a
/// suffix in that exact shape is removed, so paths may contain spaces and
/// parentheses. Only for states where git prints that suffix; an
/// uninitialized submodule has no decoration.
fn strip_describe(path: &str) -> &str {
    if !path.ends_with(')') {
        return path;
    }
    match path.rfind(" (") {
        Some(i) => &path[..i],
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn parses_every_state() {
        let cases = [
            (' ', SubmoduleState::Clean),
            ('+', SubmoduleState::Modified),
            ('-', SubmoduleState::Uninitialized),
            ('U', SubmoduleState::Conflict),
        ];
        for (prefix, state) in cases {
            let s = parse_status_line(&format!("{prefix}{OID} vendor/lib")).unwrap();
            assert_eq!(s.state, state);
            assert_eq!(s.path, "vendor/lib");
            assert_eq!(s.oid.unwrap().to_string(), OID);
        }
    }

    #[test]
    fn keeps_paths_with_spaces_and_parentheses() {
        let s = parse_status_line(&format!(" {OID} vendor/my lib (v1)")).unwrap();
        assert_eq!(s.path, "vendor/my lib");
        let s = parse_status_line(&format!(" {OID} dir with spaces")).unwrap();
        assert_eq!(s.path, "dir with spaces");
        let s = parse_status_line(&format!(" {OID} odd (name)")).unwrap();
        assert_eq!(s.path, "odd");
    }

    #[test]
    fn an_uninitialized_submodule_keeps_a_parenthesized_path() {
        // git prints no describe suffix for `-`: the whole tail is the path.
        let s = parse_status_line(&format!("-{OID} odd (name)")).unwrap();
        assert_eq!(s.state, SubmoduleState::Uninitialized);
        assert_eq!(s.path, "odd (name)");
    }

    #[test]
    fn rejects_a_malformed_line() {
        assert!(parse_status_line("").is_err());
        assert!(parse_status_line(&format!("x{OID} vendor/lib")).is_err());
        assert!(parse_status_line(&format!(" {OID}")).is_err());
        // A broken commit still yields the entry, with no oid.
        let s = parse_status_line(" deadbeef vendor/lib").unwrap();
        assert_eq!(s.oid, None);
    }

    #[test]
    fn strip_describe_only_touches_a_trailing_decoration() {
        assert_eq!(strip_describe("vendor/lib (v1.0)"), "vendor/lib");
        assert_eq!(strip_describe("vendor/lib"), "vendor/lib");
        assert_eq!(strip_describe("vendor/(lib)"), "vendor/(lib)");
        assert_eq!(strip_describe("vendor/lib (v1"), "vendor/lib (v1");
    }
}
