//! Which `git` executable to run: one shipped with the app, the one on PATH,
//! or a user-chosen path. Every repository command goes through [`Git`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{check_output, Error, Result};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
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

/// How long `git --version` may take before the executable is declared broken.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

impl Git {
    /// Find the executable for `choice`, run `--version` and check the floor.
    pub fn resolve(choice: GitChoice) -> Result<Resolution> {
        Self::resolve_with(choice, bundled_path(), "git".as_ref(), PROBE_TIMEOUT)
    }

    /// [`Git::resolve`] with the bundled location, the system executable and
    /// the probe timeout injected (for tests).
    pub(crate) fn resolve_with(
        choice: GitChoice,
        bundled: Option<PathBuf>,
        system: &Path,
        timeout: Duration,
    ) -> Result<Resolution> {
        let system = || Self::probe(system.into(), GitSource::System, timeout);
        let found = |git| Resolution {
            git,
            fallback: None,
        };
        match choice {
            GitChoice::Auto => match bundled {
                // A broken bundle must not leave the app without git: fall
                // back to the system one and let the caller report why.
                Some(exe) => match Self::probe(exe, GitSource::Bundled, timeout) {
                    Ok(git) => Ok(found(git)),
                    Err(bundled) => match system() {
                        Ok(git) => Ok(Resolution {
                            git,
                            fallback: Some(bundled),
                        }),
                        Err(system) => Err(Error::NoUsableGit {
                            bundled: Box::new(bundled),
                            system: Box::new(system),
                        }),
                    },
                },
                None => system().map(found),
            },
            GitChoice::Bundled => {
                let exe = bundled.ok_or_else(|| Error::GitNotFound {
                    source: GitSource::Bundled,
                    path: PathBuf::new(),
                })?;
                Self::probe(exe, GitSource::Bundled, timeout).map(found)
            }
            // Resolved through PATH by the OS (`git.exe` on Windows).
            GitChoice::System => system().map(found),
            GitChoice::Custom(exe) => Self::probe(exe, GitSource::Custom, timeout).map(found),
        }
    }

    fn probe(exe: PathBuf, source: GitSource, timeout: Duration) -> Result<Self> {
        let mut git = Self {
            exe,
            version: GitVersion::default(),
            source,
        };
        let mut child = git
            .command()
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::GitNotFound {
                    source,
                    path: git.exe.clone(),
                },
                _ => Error::Spawn(e),
            })?;
        // A hung executable (e.g. a stalled network mount) must not hang the
        // app: poll, and kill it when the time is up.
        let deadline = Instant::now() + timeout;
        while child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Git {
                    command: "--version",
                    code: None,
                    stderr: format!("timed out after {timeout:?}"),
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = check_output(child.wait_with_output()?, "--version")?;
        let text = String::from_utf8_lossy(&out.stdout);
        git.version = GitVersion::parse(&text).ok_or(Error::Parse("git --version output"))?;
        check_floor(git.version, source, &git.exe)?;
        Ok(git)
    }

    /// A `git` process with settings that make output stable to parse,
    /// whatever the user's config says.
    pub(crate) fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args([
            "-c",
            "core.quotepath=off",
            "-c",
            "color.ui=false",
            "-c",
            "log.showSignature=false",
            "--no-pager",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
        // Never pop up a console window on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    pub fn version(&self) -> GitVersion {
        self.version
    }

    pub fn source(&self) -> GitSource {
        self.source
    }
}

fn check_floor(found: GitVersion, source: GitSource, path: &Path) -> Result<()> {
    if found < GitVersion::MIN {
        Err(Error::UnsupportedGit {
            found,
            required: GitVersion::MIN,
            source,
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

/// The git shipped next to the app, if any. Layout per OS:
/// - Windows: `<app dir>/git/cmd/git.exe` (MinGit)
/// - macOS: `<app dir>/../Resources/git/bin/git` (inside the .app bundle)
/// - Linux: `<app dir>/git/bin/git`, then `<app dir>/../lib/gitwarp/git/bin/git`
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
            app_dir.join("../lib/gitwarp/git/bin/git"),
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
        let path = Path::new("/usr/bin/git");
        assert!(matches!(
            check_floor(
                GitVersion {
                    major: 2,
                    minor: 30,
                    patch: 9
                },
                GitSource::System,
                path
            ),
            Err(Error::UnsupportedGit { .. })
        ));
        assert!(check_floor(GitVersion::MIN, GitSource::System, path).is_ok());
        assert!(check_floor(
            GitVersion {
                major: 3,
                minor: 0,
                patch: 0
            },
            GitSource::System,
            path
        )
        .is_ok());
    }

    #[test]
    fn missing_custom_executable_is_an_error() {
        let missing = PathBuf::from("/nonexistent/definitely/not/git");
        assert!(matches!(
            Git::resolve(GitChoice::Custom(missing)),
            Err(Error::GitNotFound { .. })
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

        let r = Git::resolve_with(
            GitChoice::Auto,
            Some(fake.clone()),
            "git".as_ref(),
            PROBE_TIMEOUT,
        )
        .unwrap();
        assert_eq!(r.git.source(), GitSource::System);
        assert!(r.fallback.is_some());

        assert!(Git::resolve_with(
            GitChoice::Bundled,
            Some(fake),
            "git".as_ref(),
            PROBE_TIMEOUT
        )
        .is_err());
        assert!(
            Git::resolve_with(GitChoice::Bundled, None, "git".as_ref(), PROBE_TIMEOUT).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn auto_reports_both_causes_when_nothing_works() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("git");
        std::fs::write(&fake, "not executable").unwrap();
        let missing = tmp.path().join("no-such-git");
        let err =
            Git::resolve_with(GitChoice::Auto, Some(fake), &missing, PROBE_TIMEOUT).unwrap_err();
        assert!(matches!(err, Error::NoUsableGit { .. }));
        let text = err.to_string();
        assert!(
            text.contains("bundled:") && text.contains("system:"),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hung_executable_times_out() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("git");
        std::fs::write(&script, "#!/bin/sh\nexec sleep 5\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let err = Git::probe(script, GitSource::Custom, Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
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
