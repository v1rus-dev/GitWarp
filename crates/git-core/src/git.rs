//! Which `git` executable to run: one shipped with the app, the one on PATH,
//! or a user-chosen path. Every repository command goes through [`Git`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::{base_command, Error, Result};

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
            "" => GitChoice::Auto,
            "auto" => GitChoice::Auto,
            "bundled" => GitChoice::Bundled,
            "system" => GitChoice::System,
            path => GitChoice::Custom(PathBuf::from(path)),
        }
    }
}

impl GitChoice {
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
    pub const MIN: GitVersion = GitVersion::new(2, 31, 0);

    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse the output of `git --version`, e.g. `git version 2.39.3 (Apple
    /// Git-146)` or `git version 2.45.1.windows.1`. Only the leading numeric
    /// components count; a missing patch is 0.
    pub fn parse(output: &str) -> Option<Self> {
        let token = output
            .trim()
            .strip_prefix("git version ")?
            .split_whitespace()
            .next()?;
        let mut parts = token.split('.').map_while(|p| p.parse::<u32>().ok());
        Some(Self::new(
            parts.next()?,
            parts.next()?,
            parts.next().unwrap_or(0),
        ))
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
    /// Set when `Auto` found a bundled git that did not work and fell back.
    fallback_reason: Option<String>,
}

impl Git {
    /// Find the executable for `choice`, run `--version` and check the floor.
    pub fn resolve(choice: GitChoice) -> Result<Self> {
        Self::resolve_with(choice, bundled_path())
    }

    /// [`Git::resolve`] with the bundled location injected (for tests).
    pub(crate) fn resolve_with(choice: GitChoice, bundled: Option<PathBuf>) -> Result<Self> {
        match choice {
            GitChoice::Auto => match bundled {
                // A broken bundle must not leave the app without git: fall
                // back to the system one and let the caller report why.
                Some(exe) => match Self::probe(exe, GitSource::Bundled) {
                    Ok(git) => Ok(git),
                    Err(e) => {
                        let mut git = Self::probe("git".into(), GitSource::System)?;
                        git.fallback_reason = Some(format!("bundled git unusable: {e}"));
                        Ok(git)
                    }
                },
                None => Self::probe("git".into(), GitSource::System),
            },
            GitChoice::Bundled => {
                let exe = bundled.ok_or_else(|| Error::GitNotFound("bundled git".into()))?;
                Self::probe(exe, GitSource::Bundled)
            }
            // Resolved through PATH by the OS (`git.exe` on Windows).
            GitChoice::System => Self::probe("git".into(), GitSource::System),
            GitChoice::Custom(exe) => Self::probe(exe, GitSource::Custom),
        }
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
        if !out.status.success() {
            return Err(Error::Git {
                command: "--version",
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let version = GitVersion::parse(&text).ok_or(Error::Parse("git --version output"))?;
        Ok(Self {
            exe,
            version: check_floor(version)?,
            source,
            fallback_reason: None,
        })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn version(&self) -> GitVersion {
        self.version
    }

    pub fn source(&self) -> GitSource {
        self.source
    }

    /// Why `Auto` did not use the bundled git, if it had to fall back.
    pub fn fallback_reason(&self) -> Option<&str> {
        self.fallback_reason.as_deref()
    }
}

fn check_floor(found: GitVersion) -> Result<GitVersion> {
    if found < GitVersion::MIN {
        Err(Error::UnsupportedGit {
            found,
            required: GitVersion::MIN,
        })
    } else {
        Ok(found)
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
        assert_eq!(v("git version 2.56.0\n"), Some(GitVersion::new(2, 56, 0)));
        assert_eq!(
            v("git version 2.39.3 (Apple Git-146)"),
            Some(GitVersion::new(2, 39, 3))
        );
        assert_eq!(
            v("git version 2.45.1.windows.1"),
            Some(GitVersion::new(2, 45, 1))
        );
        assert_eq!(v("git version 2.31"), Some(GitVersion::new(2, 31, 0)));
        // A pre-release suffix on the last component is ignored.
        assert_eq!(v("git version 2.50.0-rc1"), Some(GitVersion::new(2, 50, 0)));
        assert_eq!(v("something else"), None);
        assert_eq!(v(""), None);
    }

    #[test]
    fn rejects_versions_below_the_floor() {
        assert!(matches!(
            check_floor(GitVersion::new(2, 30, 9)),
            Err(Error::UnsupportedGit { .. })
        ));
        assert_eq!(
            check_floor(GitVersion::new(2, 31, 0)).unwrap(),
            GitVersion::MIN
        );
        assert!(check_floor(GitVersion::new(3, 0, 0)).is_ok());
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

        let git = Git::resolve_with(GitChoice::Auto, Some(fake.clone())).unwrap();
        assert_eq!(git.source(), GitSource::System);
        assert!(git.fallback_reason().unwrap().contains("bundled git"));

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
