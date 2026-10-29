//! Which `git` executable to run: one shipped with the app, the one on PATH,
//! or a user-chosen path. Every repository command goes through [`Git`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::{base_command, check_output, Error, Result};

/// Where the resolved executable came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSource {
    Bundled,
    System,
    Custom,
}

impl fmt::Display for GitSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GitSource::Bundled => "bundled",
            GitSource::System => "system",
            GitSource::Custom => "custom",
        })
    }
}

/// What the user asked for; turned into a [`Git`] by [`Git::resolve`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GitChoice {
    /// Bundled if the app ships one, otherwise the system git.
    #[default]
    Auto,
    Bundled,
    System,
    Custom(PathBuf),
}

impl GitChoice {
    /// Parse a CLI/env value: `auto`, `bundled`, `system`, or a path.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "" | "auto" => GitChoice::Auto,
            "bundled" => GitChoice::Bundled,
            "system" => GitChoice::System,
            path => GitChoice::Custom(PathBuf::from(path)),
        }
    }

    /// Like [`GitChoice::parse`] for values that may not be UTF-8: those can
    /// only be paths.
    pub fn parse_os(value: &std::ffi::OsStr) -> Self {
        match value.to_str() {
            Some(v) => Self::parse(v),
            None => GitChoice::Custom(PathBuf::from(value)),
        }
    }
}

/// `major.minor.patch` of a git executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// `rev-parse --path-format=absolute` needs 2.31; `--end-of-options` 2.24.
    pub const MIN: GitVersion = GitVersion {
        major: 2,
        minor: 31,
        patch: 0,
    };

    /// Parse the output of `git --version`, e.g. `git version 2.39.3 (Apple
    /// Git-146)` or `git version 2.45.1.windows.1`. Only the leading numeric
    /// components count; a missing patch is 0.
    pub(crate) fn parse(output: &str) -> Option<Self> {
        let token = output
            .trim()
            .strip_prefix("git version ")?
            .split_whitespace()
            .next()?;
        let mut parts = token.split('.').map_while(|p| p.parse::<u32>().ok());
        Some(Self {
            major: parts.next()?,
            minor: parts.next()?,
            patch: parts.next().unwrap_or(0),
        })
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A resolved, version-checked `git` executable.
#[derive(Debug, Clone)]
pub struct Git {
    exe: PathBuf,
    version: GitVersion,
    source: GitSource,
}

/// The outcome of [`Git::resolve`].
#[derive(Debug)]
pub struct Resolution {
    pub git: Git,
    /// Why `Auto` did not use the bundled git, if it had to fall back.
    pub fallback: Option<Error>,
}

impl Git {
    /// Find the executable for `choice`, run `--version` and check the floor.
    pub fn resolve(choice: GitChoice) -> Result<Resolution> {
        Self::resolve_with(choice, bundled_path())
    }

    /// [`Git::resolve`] with the bundled location injected (for tests).
    pub(crate) fn resolve_with(choice: GitChoice, bundled: Option<PathBuf>) -> Result<Resolution> {
        let found = |git| Resolution {
            git,
            fallback: None,
        };
        match choice {
            GitChoice::Auto => match bundled {
                // A broken bundle must not leave the app without git: fall
                // back to the system one and let the caller report why.
                Some(exe) => match Self::probe(exe, GitSource::Bundled) {
                    Ok(git) => Ok(found(git)),
                    Err(e) => Ok(Resolution {
                        git: Self::system()?,
                        fallback: Some(e),
                    }),
                },
                None => Self::system().map(found),
            },
            GitChoice::Bundled => {
                let exe = bundled.ok_or_else(|| Error::GitNotFound("bundled git".into()))?;
                Self::probe(exe, GitSource::Bundled).map(found)
            }
            GitChoice::System => Self::system().map(found),
            GitChoice::Custom(exe) => Self::probe(exe, GitSource::Custom).map(found),
        }
    }

    /// Resolved through PATH by the OS (`git.exe` on Windows).
    fn system() -> Result<Self> {
        Self::probe("git".into(), GitSource::System)
    }

    fn probe(exe: PathBuf, source: GitSource) -> Result<Self> {
        let out = base_command(&exe)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::GitNotFound(exe.display().to_string()),
                _ => Error::Spawn(e),
            })?;
        let out = check_output(out, "--version")?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = GitVersion::parse(&text).ok_or(Error::Parse("git --version output"))?;
        check_floor(version)?;
        Ok(Self {
            exe,
            version,
            source,
        })
    }

    pub(crate) fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn version(&self) -> GitVersion {
        self.version
    }

    pub fn source(&self) -> GitSource {
        self.source
    }
}

fn check_floor(found: GitVersion) -> Result<()> {
    if found < GitVersion::MIN {
        Err(Error::UnsupportedGit {
            found,
            required: GitVersion::MIN,
        })
    } else {
        Ok(())
    }
}

/// The git shipped next to the app, if any. Layout per OS:
/// - Windows: `<app dir>/git/cmd/git.exe` (MinGit)
/// - macOS: `<app dir>/../Resources/git/bin/git` (inside the .app bundle)
/// - Linux: `<app dir>/git/bin/git`, then `<app dir>/../lib/gitlane/git/bin/git`
fn bundled_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // A symlinked launcher must still find files next to the real binary.
    let exe = exe.canonicalize().unwrap_or(exe);
    bundled_candidates(exe.parent()?)
        .into_iter()
        .find(|p| p.is_file())
}

fn bundled_candidates(app_dir: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![app_dir.join("git/cmd/git.exe")]
    } else if cfg!(target_os = "macos") {
        vec![app_dir.join("../Resources/git/bin/git")]
    } else {
        vec![
            app_dir.join("git/bin/git"),
            app_dir.join("../lib/gitlane/git/bin/git"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_output() {
        let v = |s| GitVersion::parse(s);
        assert_eq!(
            v("git version 2.56.0\n"),
            Some(GitVersion {
                major: 2,
                minor: 56,
                patch: 0
            })
        );
        assert_eq!(
            v("git version 2.39.3 (Apple Git-146)"),
            Some(GitVersion {
                major: 2,
                minor: 39,
                patch: 3
            })
        );
        assert_eq!(
            v("git version 2.45.1.windows.1"),
            Some(GitVersion {
                major: 2,
                minor: 45,
                patch: 1
            })
        );
        assert_eq!(
            v("git version 2.31"),
            Some(GitVersion {
                major: 2,
                minor: 31,
                patch: 0
            })
        );
        // A pre-release suffix on the last component is ignored.
        assert_eq!(
            v("git version 2.50.0-rc1"),
            Some(GitVersion {
                major: 2,
                minor: 50,
                patch: 0
            })
        );
        assert_eq!(v("something else"), None);
        assert_eq!(v(""), None);
    }

    #[test]
    fn rejects_versions_below_the_floor() {
        assert!(matches!(
            check_floor(GitVersion {
                major: 2,
                minor: 30,
                patch: 9
            }),
            Err(Error::UnsupportedGit { .. })
        ));
        assert!(check_floor(GitVersion::MIN).is_ok());
        assert!(check_floor(GitVersion {
            major: 3,
            minor: 0,
            patch: 0
        })
        .is_ok());
    }

    #[test]
    fn missing_custom_executable_is_an_error() {
        let missing = PathBuf::from("/nonexistent/definitely/not/git");
        assert!(matches!(
            Git::resolve(GitChoice::Custom(missing)),
            Err(Error::GitNotFound(_))
        ));
    }

    #[test]
    fn empty_choice_is_auto() {
        assert_eq!(GitChoice::parse(""), GitChoice::Auto);
        assert_eq!(GitChoice::parse("  "), GitChoice::Auto);
    }

    #[cfg(unix)]
    #[test]
    fn failing_version_command_is_a_git_error() {
        let false_exe = ["/bin/false", "/usr/bin/false"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .expect("`false` is installed");
        assert!(matches!(
            Git::resolve(GitChoice::Custom(false_exe)),
            Err(Error::Git {
                command: "--version",
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn broken_bundle_falls_back_in_auto_but_fails_when_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("git");
        std::fs::write(&fake, "not executable").unwrap();

        let r = Git::resolve_with(GitChoice::Auto, Some(fake.clone())).unwrap();
        assert_eq!(r.git.source(), GitSource::System);
        assert!(r.fallback.is_some());

        assert!(Git::resolve_with(GitChoice::Bundled, Some(fake)).is_err());
        assert!(Git::resolve_with(GitChoice::Bundled, None).is_err());
    }

    #[test]
    fn parses_choices() {
        assert_eq!(GitChoice::parse("system"), GitChoice::System);
        assert_eq!(GitChoice::parse("bundled"), GitChoice::Bundled);
        assert_eq!(GitChoice::parse("auto"), GitChoice::Auto);
        assert_eq!(
            GitChoice::parse("/opt/git/bin/git"),
            GitChoice::Custom("/opt/git/bin/git".into())
        );
    }
}
