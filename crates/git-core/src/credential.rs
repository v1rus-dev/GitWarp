//! The user's stored HTTPS credentials, read through `git credential`.
//!
//! The Identity page shows which GitHub/GitLab accounts a credential helper
//! already knows and can add or remove one. The token itself is never written
//! to `settings.json`; `git credential approve` hands it to whichever helper
//! the user configured (a keychain, `store`, a manager, …).

use std::io::Write;
use std::process::Stdio;

use crate::{Error, Git, Result};

/// One stored HTTPS credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

fn command(git: &Git, action: &str) -> std::process::Command {
    let mut cmd = git.command();
    cmd.args(["credential", action])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn request(host: &str, extra: &[(&str, &str)]) -> String {
    let mut text = format!("protocol=https\nhost={host}\n");
    for (key, value) in extra {
        text.push_str(&format!("{key}={value}\n"));
    }
    text.push('\n');
    text
}

fn run(mut cmd: std::process::Command, input: String) -> Result<std::process::Output> {
    let mut child = cmd.spawn().map_err(Error::Spawn)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    child.wait_with_output().map_err(Error::Io)
}

/// Asks the configured helper for a stored credential. `None` means no helper
/// had one (or the helper needs an interaction this process cannot provide).
pub fn fill(git: &Git, host: &str) -> Result<Option<Credential>> {
    let out = run(command(git, "fill"), request(host, &[]))?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut username = String::new();
    let mut password = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("username=") {
            username = value.to_owned();
        } else if let Some(value) = line.strip_prefix("password=") {
            password = value.to_owned();
        }
    }
    if username.is_empty() && password.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Credential { username, password }))
    }
}

/// Stores a credential in the configured helper.
pub fn approve(git: &Git, host: &str, username: &str, password: &str) -> Result<()> {
    let out = run(
        command(git, "approve"),
        request(host, &[("username", username), ("password", password)]),
    )?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            command: "credential approve",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Forgets the credential for `host`.
pub fn reject(git: &Git, host: &str, username: &str) -> Result<()> {
    let out = run(
        command(git, "reject"),
        request(host, &[("username", username)]),
    )?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            command: "credential reject",
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}
