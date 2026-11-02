//! Where a new worktree goes: the folder name from the user's template and a
//! free path under the base folder. Pure, so the settings page can preview it.

use std::path::{Path, PathBuf};

/// A branch name as a folder-name part: lowercase, every run of characters
/// outside `[a-z0-9]` becomes one `-`, no leading or trailing `-`
/// (`feat/Idle lanes` becomes `feat-idle-lanes`).
pub fn slug(branch: &str) -> String {
    let mut out = String::with_capacity(branch.len());
    let mut pending_dash = false;
    for c in branch.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// `base/<template>` with `{repo}` and `{branch}` substituted (the branch as a
/// [`slug`]). While `taken` says the path exists, `-2`, `-3`… is appended.
// Used by quick create and the switcher (later steps) and the settings preview.
pub fn worktree_path_for(
    base: &Path,
    template: &str,
    repo: &str,
    branch: &str,
    taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    let name = template
        .replace("{repo}", repo)
        .replace("{branch}", &slug(branch));
    let first = base.join(&name);
    if !taken(&first) {
        return first;
    }
    (2u32..)
        .map(|n| base.join(format!("{name}-{n}")))
        .find(|path| !taken(path))
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_collapses_separators() {
        assert_eq!(slug("feat/Idle lanes"), "feat-idle-lanes");
        assert_eq!(slug("fix/git-detect"), "fix-git-detect");
        assert_eq!(slug("--Release//1.2__x--"), "release-1-2-x");
        assert_eq!(slug("main"), "main");
        assert_eq!(slug("///"), "");
    }

    #[test]
    fn both_templates_expand() {
        let base = Path::new("/w");
        let free = |_: &Path| false;
        assert_eq!(
            worktree_path_for(base, "{repo}-{branch}", "gitwarp", "fix/Git detect", free),
            PathBuf::from("/w/gitwarp-fix-git-detect")
        );
        assert_eq!(
            worktree_path_for(base, "{branch}", "gitwarp", "fix/Git detect", free),
            PathBuf::from("/w/fix-git-detect")
        );
    }

    #[test]
    fn taken_paths_get_a_numeric_suffix() {
        let base = Path::new("/w");
        let taken = |p: &Path| {
            [
                PathBuf::from("/w/gitwarp-x"),
                PathBuf::from("/w/gitwarp-x-2"),
            ]
            .contains(&p.to_path_buf())
        };
        assert_eq!(
            worktree_path_for(base, "{repo}-{branch}", "gitwarp", "x", taken),
            PathBuf::from("/w/gitwarp-x-3")
        );
    }
}
