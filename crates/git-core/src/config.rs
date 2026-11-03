//! Reading and writing the user's git configuration.
//!
//! The Identity settings page edits `user.name`, `user.email`,
//! `user.signingkey` and `commit.gpgsign` either globally or for the open
//! repository. Everything goes through `git config`, so includes, `includeIf`
//! and conditional configuration keep working exactly as in the terminal.

use std::process::Stdio;

use crate::{check_output, Error, Repo, Result};

/// Which configuration file an identity edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    /// `~/.gitconfig`.
    Global,
    /// `.git/config` of the repository.
    Local,
}

impl ConfigScope {
    fn flag(self) -> &'static str {
        match self {
            ConfigScope::Global => "--global",
            ConfigScope::Local => "--local",
        }
    }
}

/// The commit identity and signing settings shown on the Identity page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    /// `user.signingkey`, empty when none is configured.
    pub signing_key: String,
    /// `commit.gpgsign`.
    pub sign_commits: bool,
}

/// Reads a global value without a repository (the welcome screen's case).
pub fn global_get(git: &crate::Git, key: &str) -> Result<Option<String>> {
    let out = git
        .command()
        .args(["config", "--global", "--get", key])
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
        ));
    }
    match out.status.code() {
        Some(1) => Ok(None),
        _ => Err(Error::Git {
            command: "config",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
    }
}

/// The global identity, for pages opened without a repository.
pub fn global_identity(git: &crate::Git) -> Result<Identity> {
    Ok(Identity {
        name: global_get(git, "user.name")?.unwrap_or_default(),
        email: global_get(git, "user.email")?.unwrap_or_default(),
        signing_key: global_get(git, "user.signingkey")?.unwrap_or_default(),
        sign_commits: global_get(git, "commit.gpgsign")?.as_deref() == Some("true"),
    })
}

/// Writes the global author without a repository.
pub fn set_global_identity(git: &crate::Git, name: &str, email: &str) -> Result<()> {
    for (key, value) in [("user.name", name.trim()), ("user.email", email.trim())] {
        let mut cmd = git.command();
        cmd.args(["config", "--global"]);
        if value.is_empty() {
            cmd.args(["--unset-all", key]);
        } else {
            cmd.args([key, value]);
        }
        let out = cmd.stdin(Stdio::null()).output().map_err(Error::Spawn)?;
        if !(out.status.success() || value.is_empty() && out.status.code() == Some(5)) {
            return Err(Error::Git {
                command: "config",
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
    }
    Ok(())
}

/// Writes the global signing settings without a repository.
pub fn set_global_signing(git: &crate::Git, sign: bool, key: &str) -> Result<()> {
    for (name, value) in [
        ("commit.gpgsign", if sign { "true" } else { "" }),
        ("user.signingkey", key.trim()),
    ] {
        let mut cmd = git.command();
        cmd.args(["config", "--global"]);
        if value.is_empty() {
            cmd.args(["--unset-all", name]);
        } else {
            cmd.args([name, value]);
        }
        let out = cmd.stdin(Stdio::null()).output().map_err(Error::Spawn)?;
        if !(out.status.success() || value.is_empty() && out.status.code() == Some(5)) {
            return Err(Error::Git {
                command: "config",
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
    }
    Ok(())
}

impl Repo {
    /// One configuration value, or `None` when the key is not set.
    pub fn config_get(&self, key: &str, scope: ConfigScope) -> Result<Option<String>> {
        let out = self
            .command()
            .args(["config", scope.flag(), "--get", key])
            .stdin(Stdio::null())
            .output()
            .map_err(Error::Spawn)?;
        if out.status.success() {
            let value = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
            return Ok(Some(value));
        }
        // `git config --get` exits 1 when the key is absent; any other status
        // is a real failure (a broken config file, an unreadable include).
        match out.status.code() {
            Some(1) => Ok(None),
            _ => Err(Error::Git {
                command: "config",
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
        }
    }

    /// Sets one value in the chosen scope.
    pub fn config_set(&self, key: &str, value: &str, scope: ConfigScope) -> Result<()> {
        let out = self
            .command()
            .args(["config", scope.flag(), key, value])
            .stdin(Stdio::null())
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "config").map(drop)
    }

    /// Removes one value from the chosen scope; a missing key is not an error.
    pub fn config_unset(&self, key: &str, scope: ConfigScope) -> Result<()> {
        let out = self
            .command()
            .args(["config", scope.flag(), "--unset-all", key])
            .stdin(Stdio::null())
            .output()
            .map_err(Error::Spawn)?;
        match out.status.code() {
            Some(0 | 5) => Ok(()),
            _ => Err(Error::Git {
                command: "config",
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
        }
    }

    /// The identity fields for `scope`. The repository scope falls back to the
    /// global values for fields the repository does not override, so the page
    /// shows what a commit would actually use.
    pub fn identity(&self, scope: ConfigScope) -> Result<Identity> {
        let read = |key: &str| -> Result<Option<String>> {
            let local = self.config_get(key, scope)?;
            if local.is_some() || scope == ConfigScope::Global {
                return Ok(local);
            }
            self.config_get(key, ConfigScope::Global)
        };
        Ok(Identity {
            name: read("user.name")?.unwrap_or_default(),
            email: read("user.email")?.unwrap_or_default(),
            signing_key: read("user.signingkey")?.unwrap_or_default(),
            sign_commits: read("commit.gpgsign")?.as_deref() == Some("true"),
        })
    }

    /// Writes the author fields; an empty value removes the key.
    pub fn set_identity(&self, scope: ConfigScope, name: &str, email: &str) -> Result<()> {
        for (key, value) in [("user.name", name.trim()), ("user.email", email.trim())] {
            if value.is_empty() {
                self.config_unset(key, scope)?;
            } else {
                self.config_set(key, value, scope)?;
            }
        }
        Ok(())
    }

    /// Turns commit signing on or off and records the key.
    pub fn set_signing(&self, scope: ConfigScope, sign: bool, key: &str) -> Result<()> {
        if sign {
            self.config_set("commit.gpgsign", "true", scope)?;
        } else {
            self.config_unset("commit.gpgsign", scope)?;
        }
        let key = key.trim();
        if key.is_empty() {
            self.config_unset("user.signingkey", scope)?;
        } else {
            self.config_set("user.signingkey", key, scope)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Arc;

    fn repo() -> (tempfile::TempDir, Repo) {
        let temp = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .arg(temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git is installed");
        assert!(status.success());
        let git = Arc::new(crate::Git::resolve(crate::GitChoice::System).unwrap().git);
        let repo = Repo::open(git, temp.path()).unwrap();
        (temp, repo)
    }

    fn config_file(repo: &Repo, name: &str) -> String {
        std::fs::read_to_string(repo.git_dir().join(name)).unwrap_or_default()
    }

    #[test]
    fn identity_reads_writes_and_unsets_in_the_chosen_scope() {
        let (_temp, repo) = repo();
        repo.set_identity(ConfigScope::Local, "Mira", "mira@example.com")
            .unwrap();
        let identity = repo.identity(ConfigScope::Local).unwrap();
        assert_eq!(identity.name, "Mira");
        assert_eq!(identity.email, "mira@example.com");

        repo.set_identity(ConfigScope::Local, "", "").unwrap();
        assert_eq!(
            repo.config_get("user.name", ConfigScope::Local).unwrap(),
            None
        );
        assert_eq!(
            repo.config_get("user.email", ConfigScope::Local).unwrap(),
            None
        );
        let config = config_file(&repo, "config");
        assert!(!config.contains("Mira"));
    }

    #[test]
    fn signing_settings_round_trip() {
        let (_temp, repo) = repo();
        repo.set_signing(ConfigScope::Local, true, "SHA256:abc")
            .unwrap();
        let identity = repo.identity(ConfigScope::Local).unwrap();
        assert!(identity.sign_commits);
        assert_eq!(identity.signing_key, "SHA256:abc");

        repo.set_signing(ConfigScope::Local, false, "").unwrap();
        assert_eq!(
            repo.config_get("commit.gpgsign", ConfigScope::Local)
                .unwrap(),
            None
        );
        assert_eq!(
            repo.config_get("user.signingkey", ConfigScope::Local)
                .unwrap(),
            None
        );
    }
}
