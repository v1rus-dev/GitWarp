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
    let Some(target) = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|target| !target.is_empty())
    else {
        // Not a gitdir pointer; let git decide.
        return PathProbe::Candidate;
    };

    // A relative gitdir is relative to the folder that holds the `.git` file.
    let gitdir = normalize(&path.join(target));
    match fs::metadata(&gitdir) {
        Ok(_) => PathProbe::Candidate,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let main_repo = main_repo_of(&gitdir);
            PathProbe::BrokenWorktree { gitdir, main_repo }
        }
        Err(e) => PathProbe::Unreadable(e.to_string()),
    }
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
    fn unusual_gitdir_has_no_main_repo() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("sub");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../elsewhere\n").unwrap();
        assert_eq!(
            probe_repo_path(&wt),
            PathProbe::BrokenWorktree {
                gitdir: dir.path().join("elsewhere"),
                main_repo: None
            }
        );
    }

    #[test]
    fn git_file_without_gitdir_line_is_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "garbage\n").unwrap();
        assert_eq!(probe_repo_path(dir.path()), PathProbe::Candidate);
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
