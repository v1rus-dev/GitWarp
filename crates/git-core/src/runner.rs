//! How a repository is found.
//!
//! [`GitRunner`] is the seam between the `git` executable and in-process
//! [`gix`]. [`Repo::open`](crate::Repo::open) is the first caller; commands
//! that must keep hooks, credentials and signing still spawn `git`.

use std::path::{Path, PathBuf};

use crate::{check_output, Error, Git, GitEngine, Result};

/// Paths [`crate::Repo`] needs after a repository has been found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRepo {
    pub git_dir: PathBuf,
    /// `objects/info`, where the commit-graph lives. For a linked worktree
    /// this is the common directory, not the worktree's own git dir.
    pub objects_info: PathBuf,
    /// Top of the work tree, or the git dir for a bare repository.
    pub root: PathBuf,
    pub bare: bool,
}

/// Finds a repository. Implemented for the resolved [`Git`] handle, which
/// dispatches on [`GitEngine`].
pub trait GitRunner: Send + Sync {
    fn discover(&self, path: &Path) -> Result<DiscoveredRepo>;
}

impl GitRunner for Git {
    fn discover(&self, path: &Path) -> Result<DiscoveredRepo> {
        match self.engine() {
            GitEngine::Native => discover_native(path),
            GitEngine::Cli => discover_cli(self, path),
        }
    }
}

fn discover_native(path: &Path) -> Result<DiscoveredRepo> {
    let repo = match gix::discover(path) {
        Ok(repo) => repo,
        Err(err) => return Err(map_discover_error(path, err)),
    };
    // `gix` opens a repository git itself would refuse. Format version 1
    // means every `extensions.*` key must be one git understands; a higher
    // version is refused outright. The wording matches git so the UI keeps
    // showing the extension name.
    reject_unsupported_format(&repo)?;
    let bare = repo.is_bare();
    let git_dir = abs_existing(repo.git_dir());
    // `common_dir` is the main git dir for a linked worktree, so the
    // commit-graph written on the main repo is visible from the link.
    // `objects/info` itself may not exist yet; canonicalize the objects
    // directory, which does.
    let objects_info = abs_existing(&repo.common_dir().join("objects")).join("info");
    let root = if bare {
        git_dir.clone()
    } else {
        match repo.workdir() {
            Some(dir) => abs_existing(dir),
            None => git_dir.clone(),
        }
    };
    Ok(DiscoveredRepo {
        git_dir,
        objects_info,
        root,
        bare,
    })
}

fn discover_cli(git: &Git, path: &Path) -> Result<DiscoveredRepo> {
    let out = git
        .command()
        .arg("-C")
        .arg(path)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-path",
            "objects/info",
            "--is-bare-repository",
            "--show-toplevel",
        ])
        .output()
        .map_err(Error::Spawn)?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    let bare = lines.get(2) == Some(&"true");
    // In a bare repository `--show-toplevel` fails (exit 128) after the
    // other answers were already printed; anywhere else a failure is real
    // and git's own message is what the user needs to see.
    if !bare {
        match check_output(out, "rev-parse") {
            Ok(_) => {}
            Err(Error::Git {
                command,
                code,
                stderr,
            }) => {
                if stderr.contains("not a git repository") {
                    return Err(Error::NotARepository {
                        path: path.to_path_buf(),
                    });
                }
                return Err(Error::Git {
                    command,
                    code,
                    stderr,
                });
            }
            Err(e) => return Err(e),
        }
    }
    let line = |i: usize| {
        lines
            .get(i)
            .map(PathBuf::from)
            .ok_or(Error::Parse("rev-parse output"))
    };
    let git_dir = line(0)?;
    let objects_info = line(1)?;
    let root = if bare { git_dir.clone() } else { line(3)? };
    Ok(DiscoveredRepo {
        git_dir,
        objects_info,
        root,
        bare,
    })
}

/// Extensions git 2.31+ (the app's floor, and well past it on 2.56) accepts
/// when `core.repositoryformatversion` is 1. Compared case-insensitively,
/// the way git does.
const KNOWN_EXTENSIONS: &[&str] = &[
    "noop",
    "preciousobjects",
    "partialclone",
    "worktreeconfig",
    "objectformat",
    "compatobjectformat",
    "refstorage",
    "relativeworktrees",
];

fn reject_unsupported_format(repo: &gix::Repository) -> Result<()> {
    let config = repo.config_snapshot();
    let version = match config.try_integer("core.repositoryformatversion") {
        Ok(Some(version)) => version,
        Ok(None) => 0,
        Err(err) => {
            return Err(Error::Git {
                command: "discover",
                code: None,
                stderr: err.to_string(),
            });
        }
    };
    if version > 1 {
        return Err(Error::Git {
            command: "discover",
            code: None,
            stderr: format!("fatal: Expected git repo version <= 1, found {version}\n"),
        });
    }
    if version < 1 {
        return Ok(());
    }
    let Some(sections) = config.sections_by_name("extensions") else {
        return Ok(());
    };
    let mut unknown = Vec::new();
    for section in sections {
        for name in section.value_names() {
            let known = KNOWN_EXTENSIONS
                .iter()
                .any(|ext| ext.eq_ignore_ascii_case(&name));
            let seen = unknown
                .iter()
                .any(|seen: &String| seen.eq_ignore_ascii_case(&name));
            if !known && !seen {
                unknown.push(name);
            }
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    let mut stderr = String::from("fatal: unknown repository extension found:\n");
    for name in unknown {
        stderr.push('\t');
        stderr.push_str(&name);
        stderr.push('\n');
    }
    Err(Error::Git {
        command: "discover",
        code: None,
        stderr,
    })
}

fn map_discover_error(path: &Path, err: gix::Error) -> Error {
    if is_not_a_repository(&err) {
        Error::NotARepository {
            path: path.to_path_buf(),
        }
    } else {
        // Same shape as a failing `git rev-parse`: the UI prints `stderr`.
        // There is no process, so the exit code stays empty.
        Error::Git {
            command: "discover",
            code: None,
            stderr: err.to_string(),
        }
    }
}

fn is_not_a_repository(err: &gix::Error) -> bool {
    let mut current: &dyn std::error::Error = err;
    loop {
        let text = current.to_string().to_ascii_lowercase();
        if text.contains("not a git repository")
            || text.contains("could not find a git repository")
            || text.contains("no git repository")
        {
            return true;
        }
        match current.source() {
            Some(next) => current = next,
            None => return false,
        }
    }
}

fn abs_existing(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitChoice, GitEngine};
    use std::sync::Arc;

    fn git(engine: GitEngine) -> Git {
        Git::resolve(GitChoice::System)
            .expect("system git")
            .git
            .with_engine(engine)
    }

    fn init(dir: &Path) {
        let status = git(GitEngine::Cli)
            .command()
            .arg("-C")
            .arg(dir)
            .args(["init", "-q", "-b", "main"])
            .status()
            .expect("spawn git init");
        assert!(status.success(), "git init failed");
    }

    #[test]
    fn native_and_cli_agree_on_a_work_tree() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let native = git(GitEngine::Native).discover(tmp.path()).expect("native");
        let cli = git(GitEngine::Cli).discover(tmp.path()).expect("cli");
        assert!(!native.bare);
        assert_eq!(native.root, cli.root);
        assert_eq!(native.git_dir, cli.git_dir);
        assert_eq!(native.objects_info, cli.objects_info);
        assert_eq!(native.root, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn native_reports_a_plain_folder_as_not_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir(&dir).unwrap();
        let err = git(GitEngine::Native).discover(&dir).unwrap_err();
        assert!(
            matches!(err, Error::NotARepository { .. }),
            "unexpected {err}"
        );
    }

    #[test]
    fn native_opens_a_bare_repository_at_its_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        let status = git(GitEngine::Cli)
            .command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&bare)
            .status()
            .expect("spawn");
        assert!(status.success());
        let found = git(GitEngine::Native).discover(&bare).expect("bare");
        assert!(found.bare);
        assert_eq!(found.root, found.git_dir);
        assert_eq!(found.root, bare.canonicalize().unwrap());
    }

    #[test]
    fn native_finds_the_root_from_a_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let sub = tmp.path().join("crates").join("app");
        std::fs::create_dir_all(&sub).unwrap();
        let found = git(GitEngine::Native).discover(&sub).expect("nested");
        assert_eq!(found.root, tmp.path().canonicalize().unwrap());
        assert!(!found.bare);
    }

    #[test]
    fn native_refuses_an_unknown_repository_extension() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let config = tmp.path().join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(
            "\n[core]\n\trepositoryformatversion = 1\n[extensions]\n\tfrobnicate = yes\n",
        );
        std::fs::write(&config, text).unwrap();
        match git(GitEngine::Native).discover(tmp.path()) {
            Err(Error::Git { stderr, .. }) => {
                assert!(stderr.contains("frobnicate"), "{stderr}");
            }
            other => panic!("unexpected result {other:?}"),
        }
    }

    #[test]
    fn native_keeps_a_repository_that_only_uses_known_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let config = tmp.path().join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(
            "\n[core]\n\trepositoryformatversion = 1\n[extensions]\n\tworktreeConfig = true\n",
        );
        std::fs::write(&config, text).unwrap();
        assert!(git(GitEngine::Native).discover(tmp.path()).is_ok());
    }

    #[test]
    fn a_resolved_handle_discovers_natively() {
        let resolved = Git::resolve(GitChoice::System).unwrap().git;
        assert_eq!(resolved.engine(), GitEngine::Native);
        // The trait is what `Repo::open` calls; keep the object-safe path
        // compiling even though the concrete type is what the app holds.
        let runner: Arc<dyn GitRunner> = Arc::new(resolved);
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir(&dir).unwrap();
        assert!(matches!(
            runner.discover(&dir),
            Err(Error::NotARepository { .. })
        ));
    }
}
