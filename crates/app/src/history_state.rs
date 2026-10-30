//! The history pane's load state, and the pure helpers behind its rendering.
//!
//! Everything here is free of entities: the mapping from [`git_core::Error`]
//! to a [`Failure`], the skeleton widths, the progress arithmetic and the
//! "Copy details" text are all unit-tested without GPUI.

use std::path::PathBuf;

use git_core::{Error, ErrorClass, GitSource, GitVersion};
use gpui::SharedString;

/// What the centre pane is showing.
#[derive(Clone, Debug)]
pub enum LoadState {
    /// The log is streaming in; `total` comes from `Repo::commit_count` and is
    /// `None` until (or unless) it arrives.
    Loading {
        total: Option<u64>,
    },
    /// The log finished; `items` may be empty.
    Loaded,
    /// The user stopped the load; rows that had arrived stay.
    Cancelled,
    Failed(Failure),
}

/// The failure states the history pane can show.
#[derive(Clone, Debug)]
pub enum Failure {
    NotARepository(PathBuf),
    PathNotFound(PathBuf),
    GitNotFound,
    GitTooOld {
        found: GitVersion,
        required: GitVersion,
        source: GitSource,
        path: PathBuf,
    },
    /// Any other git failure, with git's own message.
    Git {
        command: SharedString,
        code: Option<i32>,
        stderr: SharedString,
    },
    /// A failure with no git command behind it (I/O, a parse error): only the
    /// message is known.
    Other {
        message: SharedString,
    },
}

impl Failure {
    /// Classify a `git-core` error for the UI.
    pub fn from_error(error: &Error) -> Self {
        match error.class() {
            ErrorClass::NotARepository(path) => Failure::NotARepository(path.to_path_buf()),
            ErrorClass::PathNotFound(path) => Failure::PathNotFound(path.to_path_buf()),
            ErrorClass::GitNotFound => Failure::GitNotFound,
            ErrorClass::GitTooOld {
                found,
                required,
                source,
                path,
            } => Failure::GitTooOld {
                found,
                required,
                source,
                path: path.to_path_buf(),
            },
            ErrorClass::Other => match error {
                Error::Git {
                    command,
                    code,
                    stderr,
                } => Failure::Git {
                    command: (*command).into(),
                    code: *code,
                    stderr: stderr.trim().into(),
                },
                // I/O and parse failures have no command output: show their
                // message in the same box instead of inventing the command.
                other => Failure::Other {
                    message: other.to_string().into(),
                },
            },
        }
    }

    /// The one-line description the status bar and the empty state share.
    pub fn title(&self) -> SharedString {
        match self {
            Failure::NotARepository(_) => "Not a git repository".into(),
            Failure::PathNotFound(_) => "Folder not found".into(),
            Failure::GitNotFound => "git was not found".into(),
            Failure::GitTooOld { found, .. } => format!("git {found} is too old").into(),
            Failure::Git { .. } => "git could not read the history".into(),
            Failure::Other { .. } => "GitWarp could not read the history".into(),
        }
    }

    /// The "Copy details" clipboard text, for the failures that have one.
    pub fn copy_details(&self) -> Option<String> {
        match self {
            Failure::Git {
                command,
                code,
                stderr,
            } => Some(match code {
                Some(code) => format!("git {command}\nexit code {code}\n\n{stderr}"),
                None => format!("git {command}\nstopped\n\n{stderr}"),
            }),
            Failure::Other { message } => Some(message.to_string()),
            _ => None,
        }
    }
}

/// The body sentence for a git that cannot be used at all, or `None` when the
/// error is about the folder instead. The history states and the Open dialog
/// share this copy.
pub fn git_unavailable_message(error: &Error) -> Option<String> {
    match error.class() {
        ErrorClass::GitNotFound => Some(
            "GitWarp could not find a usable git. The bundled copy is missing and there is no \
             `git` in PATH."
                .to_owned(),
        ),
        ErrorClass::GitTooOld {
            found,
            required,
            source,
            path,
        } => Some(format!(
            "GitWarp needs git {required} or newer. {} git at {} reports {found}.",
            source_label(source),
            path.display()
        )),
        _ => None,
    }
}

/// The source name as a sentence starts it: `System`, `Bundled`, `Custom`.
pub fn source_label(source: GitSource) -> &'static str {
    match source {
        GitSource::Bundled => "Bundled",
        GitSource::System => "System",
        GitSource::Custom => "Custom",
    }
}

/// The columns a skeleton row draws, in screen order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkeletonColumn {
    Summary,
    Author,
    Date,
    Hash,
}

/// Width of a skeleton bar as a percentage of its cell, per row `index`.
///
/// Summary and author vary per row so the placeholder looks like text; date
/// and hash are fixed.
pub fn skeleton_percent(column: SkeletonColumn, index: usize) -> f32 {
    match column {
        SkeletonColumn::Summary => (40 + (37 * index) % 50) as f32,
        SkeletonColumn::Author => (50 + (17 * index) % 35) as f32,
        SkeletonColumn::Date => 60.,
        SkeletonColumn::Hash => 80.,
    }
}

/// Where the load is, as 0..=1, or `None` when the total is unknown.
pub fn progress_fraction(loaded: u64, total: Option<u64>) -> Option<f32> {
    let total = total?;
    Some(if total == 0 {
        0.
    } else {
        (loaded as f32 / total as f32).clamp(0., 1.)
    })
}

/// The `"{pct}%"` label next to the progress bar.
pub fn percent_text(loaded: u64, total: Option<u64>) -> Option<String> {
    progress_fraction(loaded, total)
        .map(|fraction| format!("{}%", (fraction * 100.).round() as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> GitVersion {
        GitVersion {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn maps_each_error_class_to_a_failure() {
        let path = PathBuf::from("/tmp/notes");
        let not_a_repo = Error::NotARepository { path: path.clone() };
        assert!(matches!(
            Failure::from_error(&not_a_repo),
            Failure::NotARepository(p) if p == path
        ));

        let missing = Error::PathNotFound { path: path.clone() };
        assert!(matches!(
            Failure::from_error(&missing),
            Failure::PathNotFound(p) if p == path
        ));

        let not_found = Error::GitNotFound {
            source: GitSource::System,
            path: PathBuf::new(),
        };
        assert!(matches!(
            Failure::from_error(&not_found),
            Failure::GitNotFound
        ));
        // A missing executable surfaces as `Spawn` before `Git::probe` can
        // report `GitNotFound`.
        let spawn = Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(matches!(Failure::from_error(&spawn), Failure::GitNotFound));

        let too_old = Error::UnsupportedGit {
            found: version(2, 28, 1),
            required: version(2, 31, 0),
            source: GitSource::System,
            path: path.clone(),
        };
        match Failure::from_error(&too_old) {
            Failure::GitTooOld {
                found,
                required,
                source,
                path: p,
            } => {
                assert_eq!(found, version(2, 28, 1));
                assert_eq!(required, version(2, 31, 0));
                assert_eq!(source, GitSource::System);
                assert_eq!(p, path);
            }
            other => panic!("unexpected {other:?}"),
        }

        let git = Error::Git {
            command: "log --all --topo-order",
            code: Some(128),
            stderr: "fatal: bad object refs/heads/broken\n".into(),
        };
        match Failure::from_error(&git) {
            Failure::Git {
                command,
                code,
                stderr,
            } => {
                assert_eq!(command.as_ref(), "log --all --topo-order");
                assert_eq!(code, Some(128));
                // The trailing newline is trimmed for the error box.
                assert_eq!(stderr.as_ref(), "fatal: bad object refs/heads/broken");
            }
            other => panic!("unexpected {other:?}"),
        }

        let io = Error::Io(std::io::Error::other("pipe closed"));
        match Failure::from_error(&io) {
            Failure::Other { message } => {
                assert!(message.contains("pipe closed"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn an_io_failure_does_not_invent_a_git_command() {
        let io = Error::Io(std::io::Error::other("pipe closed"));
        let failure = Failure::from_error(&io);
        assert_eq!(failure.title(), "GitWarp could not read the history");
        assert_eq!(
            failure.copy_details().as_deref(),
            Some("I/O error: pipe closed")
        );
        assert!(!failure.copy_details().unwrap().contains("git log"));
    }

    #[test]
    fn auto_with_a_too_old_system_reports_that_path() {
        let err = Error::NoUsableGit {
            bundled: Box::new(Error::GitNotFound {
                source: GitSource::Bundled,
                path: PathBuf::new(),
            }),
            system: Box::new(Error::UnsupportedGit {
                found: version(2, 28, 1),
                required: version(2, 31, 0),
                source: GitSource::System,
                path: PathBuf::from("/usr/bin/git"),
            }),
        };
        match Failure::from_error(&err) {
            Failure::GitTooOld { source, path, .. } => {
                assert_eq!(source, GitSource::System);
                assert_eq!(path, PathBuf::from("/usr/bin/git"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn auto_with_nothing_usable_is_git_not_found() {
        let err = Error::NoUsableGit {
            bundled: Box::new(Error::GitNotFound {
                source: GitSource::Bundled,
                path: PathBuf::new(),
            }),
            system: Box::new(Error::GitNotFound {
                source: GitSource::System,
                path: PathBuf::new(),
            }),
        };
        assert!(matches!(Failure::from_error(&err), Failure::GitNotFound));
    }

    #[test]
    fn titles_match_the_status_bar() {
        assert_eq!(
            Failure::NotARepository("/x".into()).title(),
            "Not a git repository"
        );
        assert_eq!(
            Failure::PathNotFound("/x".into()).title(),
            "Folder not found"
        );
        assert_eq!(Failure::GitNotFound.title(), "git was not found");
        assert_eq!(
            Failure::GitTooOld {
                found: version(2, 28, 1),
                required: version(2, 31, 0),
                source: GitSource::System,
                path: "/usr/bin/git".into(),
            }
            .title(),
            "git 2.28.1 is too old"
        );
        assert_eq!(
            Failure::Git {
                command: "log".into(),
                code: Some(128),
                stderr: "fatal".into(),
            }
            .title(),
            "git could not read the history"
        );
        assert_eq!(
            Failure::Other {
                message: "pipe closed".into()
            }
            .title(),
            "GitWarp could not read the history"
        );
    }

    #[test]
    fn skeleton_percentages_follow_the_design_table() {
        let summary: Vec<u32> = (0..10)
            .map(|i| skeleton_percent(SkeletonColumn::Summary, i) as u32)
            .collect();
        assert_eq!(summary, [40, 77, 64, 51, 88, 75, 62, 49, 86, 73]);
        let author: Vec<u32> = (0..10)
            .map(|i| skeleton_percent(SkeletonColumn::Author, i) as u32)
            .collect();
        assert_eq!(author, [50, 67, 84, 66, 83, 65, 82, 64, 81, 63]);
        assert_eq!(skeleton_percent(SkeletonColumn::Date, 0), 60.);
        assert_eq!(skeleton_percent(SkeletonColumn::Hash, 9), 80.);
    }

    #[test]
    fn progress_fraction_clamps_and_hides_without_a_total() {
        assert_eq!(progress_fraction(0, None), None);
        assert_eq!(progress_fraction(120, Some(1000)), Some(0.12));
        assert_eq!(progress_fraction(1500, Some(1000)), Some(1.));
        assert_eq!(progress_fraction(0, Some(0)), Some(0.));
    }

    #[test]
    fn percent_text_rounds() {
        assert_eq!(percent_text(0, None), None);
        assert_eq!(percent_text(340, Some(1000)), Some("34%".to_owned()));
        assert_eq!(percent_text(999, Some(1000)), Some("100%".to_owned()));
        assert_eq!(percent_text(0, Some(0)), Some("0%".to_owned()));
    }

    #[test]
    fn copy_details_has_the_command_the_exit_code_and_the_stderr() {
        let failure = Failure::Git {
            command: "log --all --topo-order".into(),
            code: Some(128),
            stderr: "fatal: bad object".into(),
        };
        assert_eq!(
            failure.copy_details().as_deref(),
            Some("git log --all --topo-order\nexit code 128\n\nfatal: bad object")
        );

        let stopped = Failure::Git {
            command: "log --all --topo-order".into(),
            code: None,
            stderr: "killed".into(),
        };
        assert_eq!(
            stopped.copy_details().as_deref(),
            Some("git log --all --topo-order\nstopped\n\nkilled")
        );

        assert_eq!(Failure::GitNotFound.copy_details(), None);
    }
}
