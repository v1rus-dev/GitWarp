//! Import of the accounts that the `gh` and `glab` command-line tools are
//! signed in to (Settings → Accounts, AC-5).
//!
//! GPUI-free: the JSON parsing, the host → instance mapping and the dismissed /
//! already-present filtering are plain functions, unit tested on the shapes the
//! tools print. Talking to the tools is blocking and runs on the background
//! executor; every spawn has a timeout, a null stdin and no prompts, because
//! reading from the keyring may block on an unlock dialog.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use forge::{Instance, ServiceKind};
use serde_json::Value;

use crate::settings::AccountRecord;

/// How long `status` may take: it makes network calls for every account.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// How long fetching one token may take: the keyring may prompt.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `--version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Output beyond this is dropped; the tools print far less.
const OUTPUT_LIMIT: u64 = 1024 * 1024;

/// A command-line tool accounts can be imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tool {
    Gh,
    Glab,
}

impl Tool {
    /// The executable name; also what is stored in `imported_from`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tool::Gh => "gh",
            Tool::Glab => "glab",
        }
    }
}

/// A signed-in login found in a tool and not yet in GitWarp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub tool: Tool,
    /// The host as the tool names it (`github.com`, `ghe.acme.io`).
    pub host: String,
    pub login: String,
    pub kind: ServiceKind,
    /// Normalized base URL of the instance.
    pub base_url: String,
}

impl Candidate {
    /// Identifies the candidate in the dismissed list and in row state.
    pub(crate) fn key(&self) -> String {
        dismiss_key(self.tool, &self.host, &self.login)
    }
}

/// The `tool|host|login` key stored in `Settings.dismissed_imports`.
pub(crate) fn dismiss_key(tool: Tool, host: &str, login: &str) -> String {
    format!("{}|{}|{login}", tool.name(), host.to_ascii_lowercase())
}

/// Why a tool could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliError {
    /// The executable does not exist.
    Missing,
    /// It did not answer in time.
    Timeout,
    /// It exited with an error, or its output was not what was expected.
    Failed,
    /// It answered with nothing.
    Empty,
}

/* --------------------------------- parsing -------------------------------- */

/// A login as the tool reports it, before mapping to an instance and
/// filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub tool: Tool,
    pub host: String,
    pub login: String,
    /// `https` / `http` as the tool reports it (glab); empty for gh.
    pub protocol: String,
}

/// Whether a token source names an environment variable (`GH_TOKEN`,
/// `GITLAB_TOKEN`): such a login is not a stored one and is not imported.
fn is_env_source(source: &str) -> bool {
    source.ends_with("_TOKEN") && source == source.to_ascii_uppercase()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

/// Parses `gh auth status --json hosts`. An error means the output is not the
/// documented shape (an old `gh` without `--json` prints help or an error), so
/// import is unavailable rather than guessed from text.
pub(crate) fn parse_gh_status(json: &str) -> Result<Vec<Found>, CliError> {
    let value: Value = serde_json::from_str(json).map_err(|_| CliError::Failed)?;
    let hosts = value
        .get("hosts")
        .and_then(Value::as_object)
        .ok_or(CliError::Failed)?;
    let mut found = Vec::new();
    for (host_key, entries) in hosts {
        for entry in entries.as_array().into_iter().flatten() {
            if text(entry, "state") != "success" || is_env_source(text(entry, "tokenSource")) {
                continue;
            }
            let host = match text(entry, "host") {
                "" => host_key.trim(),
                host => host,
            };
            let login = text(entry, "login");
            if host.is_empty() || login.is_empty() {
                continue;
            }
            found.push(Found {
                tool: Tool::Gh,
                host: host.to_owned(),
                login: login.to_owned(),
                protocol: String::new(),
            });
        }
    }
    Ok(found)
}

/// Parses `glab auth status --all --output json`.
pub(crate) fn parse_glab_status(json: &str) -> Result<Vec<Found>, CliError> {
    let value: Value = serde_json::from_str(json).map_err(|_| CliError::Failed)?;
    let hosts = value
        .get("hosts")
        .and_then(Value::as_array)
        .ok_or(CliError::Failed)?;
    let mut found = Vec::new();
    for entry in hosts {
        if text(entry, "state") != "success" || is_env_source(text(entry, "token_source")) {
            continue;
        }
        let (host, login) = (text(entry, "host"), text(entry, "user"));
        if host.is_empty() || login.is_empty() {
            continue;
        }
        found.push(Found {
            tool: Tool::Glab,
            host: host.to_owned(),
            login: login.to_owned(),
            protocol: text(entry, "api_protocol").to_ascii_lowercase(),
        });
    }
    Ok(found)
}

/// Maps a found login to its instance: github.com is GitHub public, any other
/// `gh` host is GitHub self-hosted; `glab` hosts are GitLab on
/// `api_protocol://host` (https when the tool did not say).
pub(crate) fn candidate_of(found: Found) -> Option<Candidate> {
    let (kind, url) = match found.tool {
        Tool::Gh if found.host.eq_ignore_ascii_case("github.com") => {
            (ServiceKind::GitHub, "https://github.com".to_owned())
        }
        Tool::Gh => (ServiceKind::GitHub, format!("https://{}", found.host)),
        Tool::Glab => {
            let scheme = if found.protocol == "http" {
                "http"
            } else {
                "https"
            };
            (ServiceKind::GitLab, format!("{scheme}://{}", found.host))
        }
    };
    let instance = Instance::new(kind, &url).ok()?;
    Some(Candidate {
        tool: found.tool,
        host: found.host,
        login: found.login,
        kind,
        base_url: instance.base_url,
    })
}

/// Whether GitWarp already has an account for this instance and login.
fn has_account(accounts: &[AccountRecord], candidate: &Candidate) -> bool {
    accounts.iter().any(|a| {
        a.kind == candidate.kind.id()
            && a.base_url
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&candidate.base_url)
            && a.login.eq_ignore_ascii_case(&candidate.login)
    })
}

/// The candidates worth showing: not already an account, not dismissed, each
/// (tool, host, login) once.
pub(crate) fn visible(
    candidates: &[Candidate],
    accounts: &[AccountRecord],
    dismissed: &[String],
) -> Vec<Candidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .iter()
        .filter(|c| !has_account(accounts, c))
        .filter(|c| !dismissed.contains(&c.key()))
        .filter(|c| seen.insert(c.key()))
        .cloned()
        .collect()
}

/* --------------------------------- running -------------------------------- */

/// Runs `program` and returns its stdout. Stdin is null, stderr is dropped,
/// prompts are disabled, and the process is killed after `timeout`.
fn run(program: &str, args: &[&str], timeout: Duration) -> Result<String, CliError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_PROMPT", "1")
        .env("NO_COLOR", "1");
    // Never pop up a console window on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CliError::Missing,
        _ => CliError::Failed,
    })?;
    let Some(stdout) = child.stdout.take() else {
        kill(&mut child);
        return Err(CliError::Failed);
    };
    // A reader thread, so a child that never closes its pipe cannot block us:
    // the answer is awaited on a channel with the same deadline.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let result = stdout.take(OUTPUT_LIMIT).read_to_end(&mut out);
        let _ = sender.send(result.map(|_| out));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                kill(&mut child);
                return Err(CliError::Timeout);
            }
            Err(_) => {
                kill(&mut child);
                return Err(CliError::Failed);
            }
        }
    };
    if !status.success() {
        return Err(CliError::Failed);
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining.max(Duration::from_millis(200))) {
        Ok(Ok(bytes)) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Ok(Err(_)) => Err(CliError::Failed),
        Err(_) => Err(CliError::Timeout),
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether `program` is installed and runs.
fn available(program: &str) -> bool {
    run(program, &["--version"], VERSION_TIMEOUT).is_ok()
}

/// Asks the installed tools who is signed in. Blocking. A missing tool, a tool
/// too old for the JSON output and any failure contribute nothing: import is
/// then simply not offered.
pub(crate) fn detect() -> Vec<Candidate> {
    let mut found = Vec::new();
    if available("gh") {
        if let Some(list) = run("gh", &["auth", "status", "--json", "hosts"], STATUS_TIMEOUT)
            .ok()
            .and_then(|out| parse_gh_status(&out).ok())
        {
            found.extend(list);
        }
    }
    if available("glab") {
        if let Some(list) = run(
            "glab",
            &["auth", "status", "--all", "--output", "json"],
            STATUS_TIMEOUT,
        )
        .ok()
        .and_then(|out| parse_glab_status(&out).ok())
        {
            found.extend(list);
        }
    }
    found.into_iter().filter_map(candidate_of).collect()
}

/// Reads the token of `candidate` from its tool. Blocking; called only when
/// the user clicks Import. The token is never logged.
pub(crate) fn fetch_token(candidate: &Candidate) -> Result<String, CliError> {
    let out = match candidate.tool {
        Tool::Gh => run(
            "gh",
            &[
                "auth",
                "token",
                "--hostname",
                &candidate.host,
                "--user",
                &candidate.login,
            ],
            TOKEN_TIMEOUT,
        )?,
        Tool::Glab => run(
            "glab",
            &["config", "get", "token", "--host", &candidate.host],
            TOKEN_TIMEOUT,
        )?,
    };
    let token = out.trim();
    if token.is_empty() {
        return Err(CliError::Empty);
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AccountMethod;

    const GH_OK: &str = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"mira-k","tokenSource":"keyring","scopes":"gist, read:org, repo","gitProtocol":"https"}]}}"#;

    fn account(kind: &str, url: &str, login: &str) -> AccountRecord {
        AccountRecord {
            id: "a1".into(),
            kind: kind.into(),
            base_url: url.into(),
            login: login.into(),
            label: login.into(),
            method: AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    fn candidates(found: Vec<Found>) -> Vec<Candidate> {
        found.into_iter().filter_map(candidate_of).collect()
    }

    #[test]
    fn gh_success_is_found_and_maps_to_public_github() {
        let found = parse_gh_status(GH_OK).unwrap();
        assert_eq!(found.len(), 1);
        let list = candidates(found);
        assert_eq!(list[0].kind, ServiceKind::GitHub);
        assert_eq!(list[0].base_url, "https://github.com");
        assert_eq!(list[0].login, "mira-k");
        assert_eq!(list[0].tool, Tool::Gh);
    }

    #[test]
    fn gh_error_state_and_env_sources_are_skipped() {
        let json = r#"{"hosts":{"github.com":[
            {"state":"error","active":false,"host":"github.com","login":"old","tokenSource":"keyring","error":"bad credentials"},
            {"state":"success","active":true,"host":"github.com","login":"ci","tokenSource":"GH_TOKEN"},
            {"state":"success","active":false,"host":"github.com","login":"ci2","tokenSource":"GITHUB_TOKEN"},
            {"state":"success","active":false,"host":"github.com","login":"file","tokenSource":"oauth_token"}]}}"#;
        let found = parse_gh_status(json).unwrap();
        let logins: Vec<&str> = found.iter().map(|f| f.login.as_str()).collect();
        assert_eq!(logins, ["file"]);
    }

    #[test]
    fn gh_several_accounts_and_hosts_map_to_instances() {
        let json = r#"{"hosts":{
            "github.com":[
              {"state":"success","active":true,"host":"github.com","login":"a","tokenSource":"keyring"},
              {"state":"success","active":false,"host":"github.com","login":"b","tokenSource":"keyring"}],
            "ghe.acme.io":[
              {"state":"success","active":true,"host":"ghe.acme.io","login":"c","tokenSource":"keyring"}]}}"#;
        let list = candidates(parse_gh_status(json).unwrap());
        assert_eq!(list.len(), 3);
        let ghe = list.iter().find(|c| c.login == "c").unwrap();
        assert_eq!(ghe.kind, ServiceKind::GitHub);
        assert_eq!(ghe.base_url, "https://ghe.acme.io");
        assert_eq!(ghe.host, "ghe.acme.io");
    }

    #[test]
    fn an_old_gh_without_json_is_unavailable() {
        assert_eq!(
            parse_gh_status("unknown flag: --json"),
            Err(CliError::Failed)
        );
        assert_eq!(parse_gh_status(""), Err(CliError::Failed));
        assert_eq!(parse_gh_status(r#"{"other":1}"#), Err(CliError::Failed));
    }

    #[test]
    fn glab_hosts_map_to_gitlab_with_the_api_protocol() {
        let json = r#"{"hosts":[
            {"host":"gitlab.com","state":"success","user":"mira","token_source":"keyring","git_protocol":"ssh","api_protocol":"https","api_endpoint":"https://gitlab.com/api/v4","graphql_endpoint":"https://gitlab.com/api/graphql"},
            {"host":"git.lan:8080","state":"success","user":"mk","token_source":"config","api_protocol":"http"},
            {"host":"gl.acme.io","state":"success","user":"nop","token_source":"config"},
            {"host":"gone.example","state":"error","user":"x","token_source":"config","error":"401"},
            {"host":"ci.example","state":"success","user":"bot","token_source":"GITLAB_TOKEN"}]}"#;
        let list = candidates(parse_glab_status(json).unwrap());
        let urls: Vec<(&str, &str)> = list
            .iter()
            .map(|c| (c.login.as_str(), c.base_url.as_str()))
            .collect();
        assert_eq!(
            urls,
            [
                ("mira", "https://gitlab.com"),
                ("mk", "http://git.lan:8080"),
                ("nop", "https://gl.acme.io"),
            ]
        );
        assert!(list.iter().all(|c| c.kind == ServiceKind::GitLab));
    }

    #[test]
    fn glab_garbage_is_unavailable() {
        assert_eq!(parse_glab_status("not json"), Err(CliError::Failed));
        assert_eq!(parse_glab_status(r#"{"hosts":{}}"#), Err(CliError::Failed));
    }

    #[test]
    fn accounts_already_in_gitwarp_are_not_offered() {
        let list = candidates(parse_gh_status(GH_OK).unwrap());
        let same = [account("github", "https://github.com/", "Mira-K")];
        assert!(visible(&list, &same, &[]).is_empty());
        // Another login, another instance or another service is a different account.
        for other in [
            account("github", "https://github.com", "someone"),
            account("github", "https://ghe.acme.io", "mira-k"),
            account("gitlab", "https://github.com", "mira-k"),
        ] {
            assert_eq!(visible(&list, &[other], &[]).len(), 1);
        }
    }

    #[test]
    fn dismissed_entries_are_never_shown_again() {
        let list = candidates(parse_gh_status(GH_OK).unwrap());
        let key = dismiss_key(Tool::Gh, "GitHub.com", "mira-k");
        assert_eq!(key, "gh|github.com|mira-k");
        assert!(visible(&list, &[], &[key]).is_empty());
        // The same login through the other tool is a separate entry.
        let other = dismiss_key(Tool::Glab, "github.com", "mira-k");
        assert_eq!(visible(&list, &[], &[other]).len(), 1);
    }

    #[test]
    fn a_login_listed_twice_is_shown_once() {
        let mut list = candidates(parse_gh_status(GH_OK).unwrap());
        list.push(list[0].clone());
        assert_eq!(visible(&list, &[], &[]).len(), 1);
    }

    #[test]
    fn a_missing_executable_is_reported_as_missing() {
        assert_eq!(
            run(
                "gitwarp-no-such-tool",
                &["--version"],
                Duration::from_secs(1)
            ),
            Err(CliError::Missing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_process_is_killed_at_the_timeout() {
        let started = Instant::now();
        let result = run("sleep", &["5"], Duration::from_millis(200));
        assert_eq!(result, Err(CliError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn stdout_is_captured_and_failures_are_reported() {
        assert_eq!(
            run("echo", &["hi"], Duration::from_secs(2)).as_deref(),
            Ok("hi\n")
        );
        assert_eq!(
            run("false", &[], Duration::from_secs(2)),
            Err(CliError::Failed)
        );
    }
}
