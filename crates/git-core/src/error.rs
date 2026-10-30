use std::path::{Path, PathBuf};

use crate::{GitSource, GitVersion};
use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// `git` could not be started (not installed, not on PATH).
    Spawn(std::io::Error),
    Io(std::io::Error),
    /// No git executable at `path` (what was tried). For the bundled git an
    /// empty `path` means no bundle exists next to the app.
    GitNotFound {
        source: GitSource,
        path: PathBuf,
    },
    /// `Auto` could use neither the bundled nor the system git.
    NoUsableGit {
        bundled: Box<Error>,
        system: Box<Error>,
    },
    /// The git executable is older than the minimum the app relies on.
    UnsupportedGit {
        found: GitVersion,
        required: GitVersion,
        source: GitSource,
        path: PathBuf,
    },
    /// The opened path is inside no git working tree.
    NotARepository {
        path: PathBuf,
    },
    /// `git init` was asked for a path that is already inside a working tree.
    AlreadyARepository {
        root: PathBuf,
    },
    /// The opened path does not exist (or is not a directory).
    PathNotFound {
        path: PathBuf,
    },
    /// `git` exited with an error; `stderr` is its message, `code` its exit
    /// status (`None` when a signal killed it).
    Git {
        command: &'static str,
        code: Option<i32>,
        stderr: String,
    },
    Parse(&'static str),
    /// A diff or blob is too large to load into the view.
    TooLarge {
        what: &'static str,
    },
}

/// What kind of failure an [`Error`] is, for the UI to match on instead of
/// sniffing the message strings itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass<'a> {
    NotARepository(&'a Path),
    PathNotFound(&'a Path),
    /// Not installed, not on PATH, or nothing usable came out of `Auto`.
    GitNotFound,
    GitTooOld {
        found: GitVersion,
        required: GitVersion,
        source: GitSource,
        path: &'a Path,
    },
    Other,
}

impl Error {
    pub fn class(&self) -> ErrorClass<'_> {
        match self {
            Error::NotARepository { path } => ErrorClass::NotARepository(path),
            Error::PathNotFound { path } => ErrorClass::PathNotFound(path),
            Error::GitNotFound { .. } => ErrorClass::GitNotFound,
            Error::Spawn(e) if e.kind() == std::io::ErrorKind::NotFound => ErrorClass::GitNotFound,
            Error::UnsupportedGit {
                found,
                required,
                source,
                path,
            } => ErrorClass::GitTooOld {
                found: *found,
                required: *required,
                source: *source,
                path,
            },
            // `Auto` failed on both sides: the "too old" side explains more
            // than a missing bundle alone, and the system git is the one the
            // user can fix (the bundled one comes with the app).
            Error::NoUsableGit { bundled, system } => match system.class() {
                ErrorClass::GitTooOld {
                    found,
                    required,
                    source,
                    path,
                } => ErrorClass::GitTooOld {
                    found,
                    required,
                    source,
                    path,
                },
                _ => match bundled.class() {
                    ErrorClass::GitTooOld {
                        found,
                        required,
                        source,
                        path,
                    } => ErrorClass::GitTooOld {
                        found,
                        required,
                        source,
                        path,
                    },
                    _ => ErrorClass::GitNotFound,
                },
            },
            _ => ErrorClass::Other,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not run git: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::GitNotFound { source, path } if path.as_os_str().is_empty() => {
                write!(f, "{source} git not found")
            }
            Error::GitNotFound { source, path } => {
                write!(f, "{source} git not found: {}", path.display())
            }
            Error::NoUsableGit { bundled, system } => {
                write!(f, "no usable git (bundled: {bundled}; system: {system})")
            }
            Error::UnsupportedGit {
                found,
                required,
                source,
                path,
            } => write!(
                f,
                "{source} git {found} is too old, {required} or newer is required: {}",
                path.display()
            ),
            Error::NotARepository { path } => {
                write!(f, "{} is not a git repository", path.display())
            }
            Error::AlreadyARepository { root } => {
                write!(
                    f,
                    "the folder is already inside a git repository at {}",
                    root.display()
                )
            }
            Error::PathNotFound { path } => write!(f, "{} does not exist", path.display()),
            Error::Git {
                command,
                code: Some(code),
                stderr,
            } => write!(f, "git {command} failed (exit {code}): {}", stderr.trim()),
            Error::Git {
                command,
                code: None,
                stderr,
            } => write!(f, "git {command} failed: {}", stderr.trim()),
            Error::Parse(what) => write!(f, "could not parse {what}"),
            Error::TooLarge { what } => write!(f, "the {what} is too large to show"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Spawn(e) | Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn too_old(source: GitSource) -> Box<Error> {
        Box::new(Error::UnsupportedGit {
            found: GitVersion {
                major: 2,
                minor: 28,
                patch: 1,
            },
            required: GitVersion::MIN,
            source,
            path: PathBuf::from("/usr/bin/git"),
        })
    }

    fn not_found(source: GitSource) -> Box<Error> {
        Box::new(Error::GitNotFound {
            source,
            path: PathBuf::new(),
        })
    }

    #[test]
    fn classifies_a_git_failure_as_other() {
        let err = Error::Git {
            command: "rev-parse",
            code: Some(128),
            stderr: "fatal: ambiguous argument".into(),
        };
        assert_eq!(err.class(), ErrorClass::Other);
    }

    #[test]
    fn classifies_a_missing_executable_as_git_not_found() {
        let err = Error::Spawn(std::io::Error::new(std::io::ErrorKind::NotFound, "nope"));
        assert_eq!(err.class(), ErrorClass::GitNotFound);
        let err = Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "nope",
        ));
        assert_eq!(err.class(), ErrorClass::Other);
    }

    #[test]
    fn auto_with_no_usable_git_is_git_not_found() {
        let err = Error::NoUsableGit {
            bundled: not_found(GitSource::Bundled),
            system: not_found(GitSource::System),
        };
        assert_eq!(err.class(), ErrorClass::GitNotFound);
    }

    #[test]
    fn auto_prefers_the_system_side_when_it_is_too_old() {
        let err = Error::NoUsableGit {
            bundled: not_found(GitSource::Bundled),
            system: too_old(GitSource::System),
        };
        match err.class() {
            ErrorClass::GitTooOld { source, path, .. } => {
                assert_eq!(source, GitSource::System);
                assert_eq!(path, Path::new("/usr/bin/git"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn auto_falls_back_to_the_bundled_side_when_only_it_is_too_old() {
        let err = Error::NoUsableGit {
            bundled: too_old(GitSource::Bundled),
            system: not_found(GitSource::System),
        };
        match err.class() {
            ErrorClass::GitTooOld { source, .. } => assert_eq!(source, GitSource::Bundled),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classifies_a_repository_failure_with_its_path() {
        let err = Error::NotARepository {
            path: PathBuf::from("/tmp/notes"),
        };
        assert_eq!(
            err.class(),
            ErrorClass::NotARepository(Path::new("/tmp/notes"))
        );
        let err = Error::PathNotFound {
            path: PathBuf::from("/tmp/gone"),
        };
        assert_eq!(
            err.class(),
            ErrorClass::PathNotFound(Path::new("/tmp/gone"))
        );
    }

    #[test]
    fn an_already_initialized_folder_names_the_root() {
        let err = Error::AlreadyARepository {
            root: PathBuf::from("/tmp/repo"),
        };
        assert_eq!(err.class(), ErrorClass::Other);
        assert!(err.to_string().contains("/tmp/repo"), "{err}");
    }

    #[test]
    fn display_names_the_exit_code() {
        let err = Error::Git {
            command: "log --all --topo-order",
            code: Some(128),
            stderr: "fatal: bad object\n".into(),
        };
        assert_eq!(
            err.to_string(),
            "git log --all --topo-order failed (exit 128): fatal: bad object"
        );
    }
}
