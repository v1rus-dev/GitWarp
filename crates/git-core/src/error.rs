use crate::GitVersion;
use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// `git` could not be started (not installed, not on PATH).
    Spawn(std::io::Error),
    Io(std::io::Error),
    /// No git executable at the requested place (the string says where).
    GitNotFound(String),
    /// The git executable is older than the minimum the app relies on.
    UnsupportedGit {
        found: GitVersion,
        required: GitVersion,
    },
    /// `git` exited with an error; `stderr` is its message.
    Git {
        command: &'static str,
        stderr: String,
    },
    Parse(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not run git: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::GitNotFound(what) => write!(f, "git not found: {what}"),
            Error::UnsupportedGit { found, required } => {
                write!(f, "git {found} is too old, {required} or newer is required")
            }
            Error::Git { command, stderr } => write!(f, "git {command} failed: {}", stderr.trim()),
            Error::Parse(what) => write!(f, "could not parse {what}"),
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
