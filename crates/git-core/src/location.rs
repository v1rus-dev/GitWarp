//! Cheap, file-system-only classification of a stored repository path.
//!
//! [`crate::Repo::open`] searches *upward* for a repository, so a removed
//! worktree that sits inside another repository would silently open that
//! parent repository. [`probe_repo_path`] looks at the folder itself (and a
//! linked worktree's `.git` file) first, without starting git.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// What the file system says about a folder that is supposed to be a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathProbe {
    /// The path does not exist.
    Missing,
    /// The path exists but is not a directory.
    NotADirectory,
    /// The path (or something needed to inspect it) can't be read; carries the OS error text.
    Unreadable(String),
    /// `.git` is a file whose `gitdir:` target no longer exists: a worktree
    /// that was removed from its repository.
    BrokenWorktree {
        /// The missing git directory, resolved against the worktree folder.
        gitdir: PathBuf,
        /// The main repository, when `gitdir` has the form `<main>/.git/worktrees/<name>`.
        main_repo: Option<PathBuf>,
    },
    /// Nothing wrong was found; whether it is a repository is up to `Repo::open`.
    Candidate,
}

/// Probes `path` using only `std::fs`.
pub fn probe_repo_path(path: &Path) -> PathProbe {
    match fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => return PathProbe::NotADirectory,
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return PathProbe::Missing,
        Err(e) => return PathProbe::Unreadable(e.to_string()),
    }

    let dot_git = path.join(".git");
    let meta = match fs::metadata(&dot_git) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return PathProbe::Candidate,
        Err(e) => return PathProbe::Unreadable(e.to_string()),
    };
    if !meta.is_file() {
        return PathProbe::Candidate;
    }

    let text = match fs::read_to_string(&dot_git) {
        Ok(text) => text,
        Err(e) => return PathProbe::Unreadable(e.to_string()),
    };
    let Some(gitdir) = parse_gitdir_pointer(path, &text) else {
        // Not a gitdir pointer; let git decide.
        return PathProbe::Candidate;
    };

    match fs::metadata(&gitdir) {
        Ok(_) => PathProbe::Candidate,
        // A missing gitdir is only a "removed worktree" when the pointer has the
        // linked-worktree shape; a submodule pointer falls through to `Repo::open`.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if is_worktree_gitdir_shape(&gitdir) {
                let main_repo = main_repo_of(&gitdir);
                PathProbe::BrokenWorktree { gitdir, main_repo }
            } else {
                PathProbe::Candidate
            }
        }
        Err(e) => PathProbe::Unreadable(e.to_string()),
    }
}

/// Whether `path` is a linked worktree: its `.git` is a file whose `gitdir:`
/// points at a linked-worktree git directory, whether or not it still exists.
/// A submodule's pointer (`.git/modules/<name>`, even `.git/modules/worktrees/<x>`)
/// and a main worktree (`.git` is a directory) are not linked worktrees. File system only.
pub fn is_linked_worktree(path: &Path) -> bool {
    let Some(gitdir) = gitdir_pointer(path) else {
        return false;
    };
    if gitdir.exists() {
        // A linked worktree's git directory has a `commondir` file; a submodule's does not.
        gitdir.join("commondir").is_file()
    } else {
        is_worktree_gitdir_shape(&gitdir)
    }
}

/// `<something>/worktrees/<name>` that is not inside a `modules` directory
/// (`<main>/.git/modules/worktrees/<x>` belongs to a submodule called `worktrees/<x>`).
/// Matches `<main>/.git/worktrees/<name>` and `<bare>/worktrees/<name>`.
fn is_worktree_gitdir_shape(gitdir: &Path) -> bool {
    let Some(parent) = gitdir.parent() else {
        return false;
    };
    parent.file_name().is_some_and(|name| name == "worktrees")
        && !parent
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "modules")
}

/// Reads `<path>/.git` as a `gitdir:` pointer file and returns its target resolved
/// against `path` (lexically normalized). `None` when `.git` is not a readable file
/// holding a non-empty `gitdir:` line.
pub fn gitdir_pointer(path: &Path) -> Option<PathBuf> {
    let dot_git = path.join(".git");
    if !fs::metadata(&dot_git).ok()?.is_file() {
        return None;
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    parse_gitdir_pointer(path, &text)
}

fn parse_gitdir_pointer(path: &Path, text: &str) -> Option<PathBuf> {
    let target = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|target| !target.is_empty())?;
    // A relative gitdir is relative to the folder that holds the `.git` file.
    Some(normalize(&path.join(target)))
}

/// `<main>/.git/worktrees/<name>` -> `<main>`.
fn main_repo_of(gitdir: &Path) -> Option<PathBuf> {
    let worktrees = gitdir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let dot_git = worktrees.parent()?;
    if dot_git.file_name()? != ".git" {
        return None;
    }
    dot_git.parent().map(Path::to_path_buf)
}

/// Resolves `.` and `..` lexically, so `<wt>/../main/.git/worktrees/x` reads as a plain path.
/// Symlinks are not followed: the path is only used for display and to derive `main_repo`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            probe_repo_path(&dir.path().join("gone")),
            PathProbe::Missing
        );
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, "x").unwrap();
        assert_eq!(probe_repo_path(&file), PathProbe::NotADirectory);
    }

    #[test]
    fn plain_directory_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe_repo_path(dir.path()), PathProbe::Candidate);
    }

    #[test]
    fn git_directory_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(probe_repo_path(dir.path()), PathProbe::Candidate);
    }

    #[test]
    fn git_file_with_existing_gitdir_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let gitdir = dir.path().join("main/.git/worktrees/wt");
        fs::create_dir_all(&gitdir).unwrap();
        let wt = dir.path().join("wt");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(probe_repo_path(&wt), PathProbe::Candidate);
    }

    #[test]
    fn git_file_pointing_nowhere_is_a_broken_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let gitdir = main.join(".git/worktrees/wt");
        let wt = dir.path().join("wt");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(
            probe_repo_path(&wt),
            PathProbe::BrokenWorktree {
                gitdir,
                main_repo: Some(main)
            }
        );
    }

    #[test]
    fn relative_gitdir_is_resolved_against_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let wt = dir.path().join("wt");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        assert_eq!(
            probe_repo_path(&wt),
            PathProbe::BrokenWorktree {
                gitdir: main.join(".git/worktrees/wt"),
                main_repo: Some(main.clone())
            }
        );
        // Once the gitdir exists the same pointer is fine.
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        assert_eq!(probe_repo_path(&wt), PathProbe::Candidate);
    }

    #[test]
    fn bare_repo_worktree_has_no_main_repo() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("sub");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../bare.git/worktrees/sub\n").unwrap();
        assert_eq!(
            probe_repo_path(&wt),
            PathProbe::BrokenWorktree {
                gitdir: dir.path().join("bare.git/worktrees/sub"),
                main_repo: None
            }
        );
    }

    #[test]
    fn missing_submodule_gitdir_is_a_candidate_not_a_broken_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        for pointer in [
            "gitdir: ../.git/modules/sub\n",
            "gitdir: ../.git/modules/worktrees/sub\n",
            "gitdir: ../elsewhere\n",
        ] {
            fs::write(sub.join(".git"), pointer).unwrap();
            assert_eq!(probe_repo_path(&sub), PathProbe::Candidate, "{pointer}");
        }
    }

    #[test]
    fn git_file_without_gitdir_line_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "garbage\n").unwrap();
        assert_eq!(probe_repo_path(dir.path()), PathProbe::Candidate);
    }

    #[test]
    fn linked_worktree_detection() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        fs::create_dir(&wt).unwrap();
        let pointer = |text: &str| fs::write(wt.join(".git"), text).unwrap();

        // Removed worktree: the gitdir does not exist.
        pointer("gitdir: ../main/.git/worktrees/wt\n");
        assert!(is_linked_worktree(&wt));
        // Alive worktree: recognised by its `commondir` file.
        let alive = dir.path().join("main/.git/worktrees/wt");
        fs::create_dir_all(&alive).unwrap();
        assert!(!is_linked_worktree(&wt));
        fs::write(alive.join("commondir"), "../..\n").unwrap();
        assert!(is_linked_worktree(&wt));
        // Submodule.
        pointer("gitdir: ../.git/modules/x\n");
        assert!(!is_linked_worktree(&wt));
        // Submodule named `worktrees/x`: shape matches, but it lives under `modules`
        // (alive without commondir, and gone).
        pointer("gitdir: ../.git/modules/worktrees/x\n");
        assert!(!is_linked_worktree(&wt));
        let sub_gitdir = dir.path().join(".git/modules/worktrees/x");
        fs::create_dir_all(&sub_gitdir).unwrap();
        assert!(!is_linked_worktree(&wt));
        // Not a pointer.
        pointer("garbage\n");
        assert!(!is_linked_worktree(&wt));
        // Normal repository and plain folder.
        fs::remove_file(wt.join(".git")).unwrap();
        assert!(!is_linked_worktree(&wt));
        fs::create_dir(wt.join(".git")).unwrap();
        assert!(!is_linked_worktree(&wt));
        assert!(!is_linked_worktree(&dir.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // root ignores permission bits; nothing to check there.
        let readable_anyway = fs::read_dir(&locked).is_ok();
        let probe = probe_repo_path(&locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if readable_anyway {
            return;
        }
        assert!(matches!(probe, PathProbe::Unreadable(_)), "{probe:?}");
    }
}
