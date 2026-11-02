//! The app side of git's interactive prompts.
//!
//! Git is a child process without a terminal here, so HTTPS credentials go
//! through `GIT_ASKPASS` and SSH host keys are checked before the operation
//! starts. Both arrive as requests for the UI:
//!
//! * [`CredentialPrompt`] — git asked for a user name or password. The
//!   workspace opens a small modal and sends the answer back to the waiting
//!   helper process.
//! * [`HostKeyRequest`] — an SSH host is not in `known_hosts` (or its key
//!   changed). The workspace asks the user, then the worker records the key.
//!
//! The askpass helper is this same executable, started by git with the prompt
//! as its only argument. [`is_helper`] tells `main` to answer it instead of
//! opening a window.
//!
//! A job may also carry the accounts of the hosts it talks to ([`JobAuth`]).
//! The helper sends the job id with each prompt; for a host of the job the
//! server answers from the account and its secret-store token, so the token
//! lives only in the app and in git's pipe, never in an environment variable,
//! an argument or a log line.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge::secrets::SecretStore;
use git_core::network::{CancelToken, NetworkContext};
use git_core::ssh::{HostKeyStatus, SshTarget};

const ADDR_ENV: &str = "GITWARP_ASKPASS_ADDR";
const TOKEN_ENV: &str = "GITWARP_ASKPASS_TOKEN";
const JOB_ENV: &str = "GITWARP_ASKPASS_JOB";
/// How long a helper waits for the user before it gives up and git fails.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(600);
/// A helper that connects must send its request within this long.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// The token and job lines are short; anything longer is not our helper.
const MAX_LINE: u64 = 256;
/// A git prompt is a sentence; more than this is not one.
const MAX_PROMPT: usize = 64 * 1024;
/// Helpers served at once (each waits for the user at most `PROMPT_TIMEOUT`).
const MAX_CONNECTIONS: usize = 16;

/// 128 unpredictable bits without a new dependency: `RandomState` keys come
/// from the OS random generator, and every half is a keyed hash of different
/// input, so the value is not a counter or a clock reading.
fn random_u128() -> u128 {
    let half = |salt: u8| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write(&[salt]);
        hasher.write_u32(std::process::id());
        hasher.finish()
    };
    (u128::from(half(1)) << 64) | u128::from(half(2))
}

/// Equality that takes the same time wherever the strings differ.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |diff, (x, y)| diff | (x ^ y))
        == 0
}

/// A job id as it travels in the environment and the helper's request.
fn job_text(id: u128) -> String {
    format!("{id:032x}")
}

/// One credential request from a running git child.
pub struct CredentialPrompt {
    /// Git's own text, e.g. `Password for 'https://user@github.com': `.
    pub prompt: String,
    /// `Some(text)` to answer, `None` to cancel.
    pub reply: Sender<Option<String>>,
}

impl CredentialPrompt {
    /// Whether the answer should be hidden in the dialog.
    pub fn secret(&self) -> bool {
        let prompt = self.prompt.to_lowercase();
        prompt.contains("password") || prompt.contains("passphrase") || prompt.contains("token")
    }

    /// A short label for the dialog field.
    pub fn label(&self) -> gpui::SharedString {
        if self.secret() {
            ui::t!("network.credential.password")
        } else {
            ui::t!("network.credential.user_name")
        }
    }
}

/// An SSH host the user has to decide about before git connects.
pub struct HostKeyRequest {
    pub target: SshTarget,
    pub status: HostKeyStatus,
    /// `true` trusts (or replaces) the key; `false` aborts the operation.
    pub reply: Sender<bool>,
}

/// A clone of the request channel, usable from a background worker.
#[derive(Clone)]
pub struct HostKeyAsker {
    tx: Sender<HostKeyRequest>,
}

impl HostKeyAsker {
    /// Asks the UI and waits. A missing or closed UI, or a cancelled
    /// operation, means "do not connect".
    pub fn ask(&self, target: SshTarget, status: HostKeyStatus, cancel: &CancelToken) -> bool {
        let (reply, answer) = mpsc::channel();
        if self
            .tx
            .send(HostKeyRequest {
                target,
                status,
                reply,
            })
            .is_err()
        {
            return false;
        }
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() || started.elapsed() > PROMPT_TIMEOUT {
                return false;
            }
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    /// An asker for callers without a network service (tests, or a build with
    /// no UI): every question is answered "no".
    pub fn unavailable() -> Self {
        let (tx, _rx) = mpsc::channel();
        Self { tx }
    }
}

/* --------------------------- accounts of a job --------------------------- */

/// The web host git talks to: what a credential prompt names, normalised so
/// the same server compares equal however its URL was written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostKey {
    /// `https` (or `http`), lowercase.
    pub scheme: String,
    /// Lowercase, without the brackets of an IPv6 literal.
    pub host: String,
    /// `None` for the scheme's default port.
    pub port: Option<u16>,
}

impl HostKey {
    pub fn new(scheme: &str, host: &str, port: Option<u16>) -> Self {
        let scheme = scheme.to_ascii_lowercase();
        let default = match scheme.as_str() {
            "https" => Some(443),
            "http" => Some(80),
            _ => None,
        };
        Self {
            port: port.filter(|port| Some(*port) != default),
            host: host.trim_matches(['[', ']']).to_ascii_lowercase(),
            scheme,
        }
    }

    /// `scheme://host[:port]`, the form of git's `credential.<url>.*` keys.
    pub fn url(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{}://{host}:{port}", self.scheme),
            None => format!("{}://{host}", self.scheme),
        }
    }
}

/// Which account answers for a host. Holds no secret: the token is read from
/// the secret store when a prompt arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCredential {
    pub account_id: String,
    /// What git sends as the user name (`ServiceKind::git_username`).
    pub username: String,
}

/// What a running job may answer on its own.
#[derive(Clone)]
pub struct JobAuth {
    /// Whether a prompt the job cannot answer may go to the user.
    pub interactive: bool,
    pub accounts: HashMap<HostKey, AccountCredential>,
    /// The secret store; `None` opens the system keychain on first use.
    pub store: Option<Arc<dyn SecretStore>>,
}

/// Keeps a job's accounts registered; dropping it forgets them.
pub struct JobGuard {
    id: u128,
    jobs: Jobs,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        match self.jobs.lock() {
            Ok(mut jobs) => jobs.remove(&self.id),
            Err(poisoned) => poisoned.into_inner().remove(&self.id),
        };
    }
}

type Jobs = Arc<Mutex<HashMap<u128, Arc<JobAuth>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Username,
    Password,
}

/// What git's prompt text asks and for which host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPrompt {
    pub kind: PromptKind,
    pub host: HostKey,
    /// The user name in the URL, present in password prompts.
    pub user: Option<String>,
}

/// Reads `Username for '<url>': ` and `Password for '<url>': `. Anything else
/// (an SSH passphrase, a host-key question, a changed git, a URL with a path
/// or a delimiter that could hide another host) is `None`: the prompt is then
/// treated as one of an unknown host and never gets a token.
///
/// The whole authority, user information included, must be free of `/`, `?`,
/// `#`, whitespace and control characters, so `https://github.com/@evil.test`
/// or `https://u@evil.test/p@github.com` cannot pass for github.com. The host
/// is what follows the last `@`.
pub fn parse_prompt(text: &str) -> Option<ParsedPrompt> {
    let text = text.trim();
    let (kind, rest) = match text.strip_prefix("Username for ") {
        Some(rest) => (PromptKind::Username, rest),
        None => (PromptKind::Password, text.strip_prefix("Password for ")?),
    };
    let rest = rest.strip_prefix('\'')?;
    let url = &rest[..rest.rfind('\'')?];
    let (scheme, authority) = url.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if authority
        .chars()
        .any(|c| matches!(c, '/' | '?' | '#') || c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((user, hostport)) => (Some(user.to_owned()), hostport),
        None => (None, authority),
    };
    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        let (host, tail) = after.split_once(']')?;
        // An IPv6 literal: hex digits, colons and an embedded IPv4 tail only.
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
        {
            return None;
        }
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(port.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        (host, port)
    } else {
        let (host, port) = match hostport.rsplit_once(':') {
            Some((host, port)) => (host, Some(port.parse().ok()?)),
            None => (hostport, None),
        };
        // Same rule as `forge::url::valid_host`, which is crate-private there.
        let valid = !host.is_empty()
            && !host.starts_with('.')
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
        if !valid {
            return None;
        }
        (host, port)
    };
    Some(ParsedPrompt {
        kind,
        host: HostKey::new(scheme, host, port),
        user,
    })
}

/// The answer the job's accounts give to `prompt`, or `None` when the host is
/// not one of the job's, the prompt names another user than the one we
/// answered, or the secret cannot be had.
fn account_answer(auth: &JobAuth, prompt: &ParsedPrompt) -> Option<String> {
    let credential = auth.accounts.get(&prompt.host)?;
    match prompt.kind {
        PromptKind::Username => Some(credential.username.clone()),
        PromptKind::Password => {
            // git repeats the user name we gave in the password prompt; any
            // other (a redirect, a crafted URL) gets no token.
            let user = prompt.user.as_deref()?;
            if user != credential.username && user.replace("%40", "@") != credential.username {
                return None;
            }
            let read = |store: &dyn SecretStore| store.get(&credential.account_id);
            let token = match &auth.store {
                Some(store) => read(store.as_ref()),
                None => match crate::accounts::resolve_store() {
                    Ok(store) => read(store.as_ref()),
                    Err(error) => Err(error),
                },
            };
            // Neither the token nor the reason it failed is logged.
            token.ok().flatten().filter(|token| !token.is_empty())
        }
    }
}

/// The process-wide prompt service: one askpass listener and one queue of host
/// key questions. Created once in `main` and stored as a GPUI global.
pub struct NetworkService {
    askpass: Option<AskpassServer>,
    host_keys: Mutex<Receiver<HostKeyRequest>>,
    host_key_tx: Sender<HostKeyRequest>,
}

impl gpui::Global for NetworkService {}

impl NetworkService {
    pub fn start() -> Self {
        let (host_key_tx, host_keys) = mpsc::channel();
        Self {
            askpass: AskpassServer::start().ok(),
            host_keys: Mutex::new(host_keys),
            host_key_tx,
        }
    }

    /// The environment for a git network command.
    ///
    /// Non-interactive commands (the background fetch) never prompt. An
    /// interactive command gets the askpass variables when the listener could
    /// be started; without it git fails with its own authentication error
    /// instead of hanging.
    pub fn context(&self, interactive: bool) -> NetworkContext {
        // Only let git prompt when there is a helper to answer: without the
        // listener an interactive prompt would just hang on a missing tty.
        let prompting = interactive && self.askpass.is_some();
        let mut env = vec![(
            "GIT_TERMINAL_PROMPT".to_owned(),
            if prompting { "1" } else { "0" }.to_owned(),
        )];
        if prompting {
            if let Some(askpass) = &self.askpass {
                env.extend(askpass.variables(true, None));
            }
        }
        NetworkContext::with_env(env)
    }

    /// The context of a job that carries accounts.
    ///
    /// Git is told to ignore credential helpers for the account hosts only
    /// (`credential.<url>.helper=` resets the list for that URL), so it asks
    /// askpass and nothing is stored back into a helper; other hosts behave as
    /// without accounts. The returned guard keeps the accounts registered, so
    /// hold it until the job ended. Without accounts, or without the askpass
    /// listener, this is [`NetworkService::context`] and there is no guard.
    pub fn job_context(&self, auth: JobAuth) -> (NetworkContext, Option<JobGuard>) {
        let Some(askpass) = self.askpass.as_ref().filter(|_| !auth.accounts.is_empty()) else {
            return (self.context(auth.interactive), None);
        };
        let mut config: Vec<String> = auth
            .accounts
            .keys()
            .flat_map(|host| {
                [
                    format!("credential.{}.helper=", host.url()),
                    // Prompts then name the host only, which is what the
                    // parser accepts. A redirect still re-prompts for the new
                    // host (planner spike, git 2.56), so it cannot carry the
                    // token elsewhere.
                    format!("credential.{}.useHttpPath=false", host.url()),
                ]
            })
            .collect();
        config.sort();
        let id = random_u128();
        // A background job asks askpass for account hosts only; git still must
        // not prompt on a terminal, and SSH is left alone.
        let mut env = vec![(
            "GIT_TERMINAL_PROMPT".to_owned(),
            if auth.interactive { "1" } else { "0" }.to_owned(),
        )];
        env.extend(askpass.variables(auth.interactive, Some(job_text(id))));
        let guard = JobGuard {
            id,
            jobs: askpass.jobs.clone(),
        };
        match askpass.jobs.lock() {
            Ok(mut jobs) => jobs.insert(id, Arc::new(auth)),
            Err(poisoned) => poisoned.into_inner().insert(id, Arc::new(auth)),
        };
        (
            NetworkContext::with_env(env).with_git_config(config),
            Some(guard),
        )
    }

    pub fn host_keys(&self) -> HostKeyAsker {
        HostKeyAsker {
            tx: self.host_key_tx.clone(),
        }
    }

    /// The next credential question, if any. The workspace polls this once
    /// per frame while a network operation runs.
    pub fn try_recv_credential(&self) -> Option<CredentialPrompt> {
        self.askpass.as_ref()?.try_recv()
    }

    /// The next host-key question, if any.
    pub fn try_recv_host_key(&self) -> Option<HostKeyRequest> {
        let receiver = match self.host_keys.lock() {
            Ok(receiver) => receiver,
            Err(poisoned) => poisoned.into_inner(),
        };
        receiver.try_recv().ok()
    }
}

/// The TCP listener git's askpass helpers connect to.
struct AskpassServer {
    addr: SocketAddr,
    token: String,
    prompts: Mutex<Receiver<CredentialPrompt>>,
    jobs: Jobs,
}

impl AskpassServer {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        // A 128-bit random token keeps unrelated local processes from
        // answering prompts; it is never printed or logged.
        let token = job_text(random_u128());
        let (tx, rx) = mpsc::channel();
        let thread_token = token.clone();
        let jobs: Jobs = Jobs::default();
        let thread_jobs = jobs.clone();
        let open = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                // Over the limit: drop the connection, git fails its prompt.
                if open.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    open.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let tx = tx.clone();
                let token = thread_token.clone();
                let jobs = thread_jobs.clone();
                let open = open.clone();
                std::thread::spawn(move || {
                    serve(stream, &token, &jobs, tx);
                    open.fetch_sub(1, Ordering::AcqRel);
                });
            }
        });
        Ok(Self {
            addr,
            token,
            prompts: Mutex::new(rx),
            jobs,
        })
    }

    /// The askpass variables; `ssh` also routes SSH prompts to the helper.
    /// `job` names the job whose accounts may answer. The token is the
    /// listener's own, not an account secret.
    ///
    /// The token and job id sit in the environment of the git process, so the
    /// hooks and credential helpers it starts can read them. Hooks already run
    /// arbitrary code as the user; what the job id can fetch is limited to the
    /// job's own hosts, and only while the job runs. There is no cap on how
    /// often a host is answered: git-lfs and other legitimate requests of the
    /// same job ask again, and any same-user process can read the token from
    /// the Secret Service / Keychain directly, so a cap would add nothing.
    fn variables(&self, ssh: bool, job: Option<String>) -> Vec<(String, String)> {
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "gitwarp".to_owned());
        let mut variables = vec![("GIT_ASKPASS".to_owned(), exe.clone())];
        if ssh {
            variables.push(("SSH_ASKPASS".to_owned(), exe));
            // OpenSSH 8.4+ honours SSH_ASKPASS_REQUIRE=force without DISPLAY.
            variables.push(("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned()));
        }
        variables.push((ADDR_ENV.to_owned(), self.addr.to_string()));
        variables.push((TOKEN_ENV.to_owned(), self.token.clone()));
        if let Some(job) = job {
            variables.push((JOB_ENV.to_owned(), job));
        }
        variables
    }

    fn try_recv(&self) -> Option<CredentialPrompt> {
        let receiver = match self.prompts.lock() {
            Ok(receiver) => receiver,
            Err(poisoned) => poisoned.into_inner(),
        };
        receiver.try_recv().ok()
    }
}

/// The protocol between the helper process and the app:
/// `TOKEN\n<job id or empty>\n<length>\n<prompt bytes>` then
/// `OK <length>\n<answer>` or `CANCEL`.
fn serve(mut stream: TcpStream, token: &str, jobs: &Jobs, tx: Sender<CredentialPrompt>) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    // One short line; `None` when it is missing, too long or not terminated.
    fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
        let mut line = String::new();
        reader.by_ref().take(MAX_LINE).read_line(&mut line).ok()?;
        line.ends_with('\n').then(|| line.trim_end().to_owned())
    }
    // A wrong token closes the connection at once.
    match read_line(&mut reader) {
        Some(sent) if constant_time_eq(&sent, token) => {}
        _ => {
            let _ = writeln!(stream, "CANCEL");
            return;
        }
    }
    let Some(job_line) = read_line(&mut reader) else {
        return;
    };
    // Empty: a command without accounts (clone, submodule, ...).
    let job = match job_line.as_str() {
        "" => None,
        id => match u128::from_str_radix(id, 16) {
            Ok(id) => Some(id),
            Err(_) => return,
        },
    };
    let Some(length) = read_line(&mut reader).and_then(|line| line.parse::<usize>().ok()) else {
        return;
    };
    if length > MAX_PROMPT {
        return;
    }
    let mut prompt = vec![0u8; length];
    if reader.read_exact(&mut prompt).is_err() {
        return;
    }
    let prompt = String::from_utf8_lossy(&prompt).into_owned();

    if let Some(id) = job {
        let registered = match jobs.lock() {
            Ok(jobs) => jobs.get(&id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(&id).cloned(),
        };
        // A job that already ended has nothing left to ask.
        let Some(registered) = registered else {
            let _ = writeln!(stream, "CANCEL");
            return;
        };
        let auth = &registered;
        let answer = parse_prompt(&prompt).and_then(|parsed| account_answer(auth, &parsed));
        if let Some(answer) = answer {
            let _ = writeln!(stream, "OK {}", answer.len());
            let _ = stream.write_all(answer.as_bytes());
            let _ = stream.flush();
            return;
        }
        // Not one of the job's hosts (or no usable secret): the user decides,
        // unless nobody is watching.
        if !auth.interactive {
            let _ = writeln!(stream, "CANCEL");
            return;
        }
    }

    let (reply, answer) = mpsc::channel();
    if tx.send(CredentialPrompt { prompt, reply }).is_err() {
        return;
    }
    match answer.recv_timeout(PROMPT_TIMEOUT) {
        Ok(Some(value)) => {
            let _ = writeln!(stream, "OK {}", value.len());
            let _ = stream.write_all(value.as_bytes());
            let _ = stream.flush();
        }
        _ => {
            let _ = writeln!(stream, "CANCEL");
        }
    }
}

/// Whether this process was started by git as its askpass helper.
pub fn is_helper() -> bool {
    std::env::var_os(ADDR_ENV).is_some()
}

/// Answers one askpass invocation and exits. Called at the top of `main`
/// before any window or logging is set up.
pub fn run_helper() {
    let prompt = std::env::args_os()
        .nth(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (Some(addr), Some(token)) = (std::env::var(ADDR_ENV).ok(), std::env::var(TOKEN_ENV).ok())
    else {
        std::process::exit(1);
    };
    let Ok(mut stream) = TcpStream::connect(&addr) else {
        std::process::exit(1);
    };
    let _ = writeln!(stream, "{token}");
    let _ = writeln!(stream, "{}", std::env::var(JOB_ENV).unwrap_or_default());
    let _ = writeln!(stream, "{}", prompt.len());
    let _ = stream.write_all(prompt.as_bytes());
    let _ = stream.flush();

    let Ok(reader_stream) = stream.try_clone() else {
        std::process::exit(1);
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        std::process::exit(1);
    }
    let line = line.trim_end();
    if line == "CANCEL" {
        std::process::exit(1);
    }
    let Some(length) = line
        .strip_prefix("OK ")
        .and_then(|length| length.parse::<usize>().ok())
    else {
        std::process::exit(1);
    };
    let mut answer = vec![0u8; length];
    if reader.read_exact(&mut answer).is_err() {
        std::process::exit(1);
    }
    let _ = std::io::stdout().write_all(&answer);
    let _ = std::io::stdout().write_all(b"\n");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::secrets::MemoryStore;

    /// What a helper process does: one prompt, one answer (`None` = CANCEL).
    fn ask(
        addr: SocketAddr,
        token: &str,
        job: Option<String>,
        prompt: &str,
    ) -> std::thread::JoinHandle<Option<String>> {
        let token = token.to_owned();
        let prompt = prompt.to_owned();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            writeln!(stream, "{token}").unwrap();
            writeln!(stream, "{}", job.unwrap_or_default()).unwrap();
            writeln!(stream, "{}", prompt.len()).unwrap();
            stream.write_all(prompt.as_bytes()).unwrap();
            stream.flush().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let length = line
                .trim_end()
                .strip_prefix("OK ")?
                .parse::<usize>()
                .unwrap();
            let mut answer = vec![0u8; length];
            reader.read_exact(&mut answer).unwrap();
            Some(String::from_utf8(answer).unwrap())
        })
    }

    fn endpoint(service: &NetworkService) -> (SocketAddr, String) {
        let askpass = service.askpass.as_ref().expect("the listener starts");
        (askpass.addr, askpass.token.clone())
    }

    fn github() -> HostKey {
        HostKey::new("https", "github.com", None)
    }

    /// A job with an account on github.com whose token is in `store`.
    fn github_job(interactive: bool, token: Option<&str>) -> JobAuth {
        let store = MemoryStore::new();
        if let Some(token) = token {
            store.set("acc-1", token).unwrap();
        }
        JobAuth {
            interactive,
            accounts: HashMap::from([(
                github(),
                AccountCredential {
                    account_id: "acc-1".to_owned(),
                    username: "mira".to_owned(),
                },
            )]),
            store: Some(Arc::new(store)),
        }
    }

    fn job_id(context: &NetworkContext) -> String {
        context
            .env
            .iter()
            .find(|(key, _)| key == JOB_ENV)
            .expect("the job id is in the environment")
            .1
            .clone()
    }

    #[test]
    fn askpass_round_trip_reaches_the_ui_and_returns_the_answer() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let helper = ask(addr, &token, None, "Password for 'https://example.test': ");

        let prompt = loop {
            if let Some(prompt) = service.try_recv_credential() {
                break prompt;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(prompt.secret());
        prompt
            .reply
            .send(Some("hunter2".to_owned()))
            .expect("the helper is still connected");
        assert_eq!(helper.join().unwrap().as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let service = NetworkService::start();
        let (addr, _) = endpoint(&service);
        let helper = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            writeln!(stream, "not-the-token").unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            line.trim_end().to_owned()
        });
        assert_eq!(helper.join().unwrap(), "CANCEL");
        assert!(service.try_recv_credential().is_none());
    }

    #[test]
    fn labels_passwords_and_user_names() {
        let prompt = |text: &str| CredentialPrompt {
            prompt: text.to_owned(),
            reply: mpsc::channel().0,
        };
        assert!(prompt("Password for 'https://a@b': ").secret());
        assert_eq!(prompt("Password for 'https://a@b': ").label(), "Password");
        assert!(!prompt("Username for 'https://b': ").secret());
        assert_eq!(prompt("Username for 'https://b': ").label(), "User name");
    }

    #[test]
    fn a_job_with_an_account_answers_without_the_dialog() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let (context, _guard) = service.job_context(github_job(true, Some("s3cret-token")));
        let id = job_id(&context);

        let user = ask(
            addr,
            &token,
            Some(id.clone()),
            "Username for 'https://github.com': ",
        );
        assert_eq!(user.join().unwrap().as_deref(), Some("mira"));
        let password = ask(
            addr,
            &token,
            Some(id.clone()),
            "Password for 'https://mira@github.com': ",
        );
        assert_eq!(password.join().unwrap().as_deref(), Some("s3cret-token"));
        // Neither question reached the user.
        assert!(service.try_recv_credential().is_none());
    }

    #[test]
    fn an_unknown_host_goes_to_the_dialog_in_an_interactive_job() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let (context, _guard) = service.job_context(github_job(true, Some("s3cret-token")));
        let helper = ask(
            addr,
            &token,
            Some(job_id(&context)),
            // Another host must never get the github.com token.
            "Password for 'https://mira@evil.test': ",
        );
        let prompt = loop {
            if let Some(prompt) = service.try_recv_credential() {
                break prompt;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(prompt.prompt, "Password for 'https://mira@evil.test': ");
        prompt.reply.send(Some("typed".to_owned())).unwrap();
        assert_eq!(helper.join().unwrap().as_deref(), Some("typed"));
    }

    #[test]
    fn a_background_job_cancels_what_it_cannot_answer() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let (context, _guard) = service.job_context(github_job(false, Some("s3cret-token")));
        let id = job_id(&context);
        // A host without an account, and an SSH prompt: both are cancelled.
        for prompt in [
            "Username for 'https://other.test': ",
            "Enter passphrase for key '/home/u/.ssh/id_ed25519': ",
        ] {
            assert_eq!(
                ask(addr, &token, Some(id.clone()), prompt).join().unwrap(),
                None
            );
        }
        // The account's own host is still answered.
        let user = ask(
            addr,
            &token,
            Some(id.clone()),
            "Username for 'https://github.com': ",
        );
        assert_eq!(user.join().unwrap().as_deref(), Some("mira"));
        assert!(service.try_recv_credential().is_none());
    }

    #[test]
    fn a_missing_secret_falls_back_to_the_dialog_or_cancels() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let prompt = "Password for 'https://mira@github.com': ";

        let (context, _guard) = service.job_context(github_job(false, None));
        let background = ask(addr, &token, Some(job_id(&context)), prompt);
        assert_eq!(background.join().unwrap(), None);
        assert!(service.try_recv_credential().is_none());

        let (context, _guard) = service.job_context(github_job(true, None));
        let helper = ask(addr, &token, Some(job_id(&context)), prompt);
        let asked = loop {
            if let Some(asked) = service.try_recv_credential() {
                break asked;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        asked.reply.send(None).unwrap();
        assert_eq!(helper.join().unwrap(), None);
    }

    #[test]
    fn the_accounts_of_a_job_are_dropped_when_it_ends() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let (context, guard) = service.job_context(github_job(true, Some("s3cret-token")));
        let id = job_id(&context);
        drop(guard);
        assert!(service
            .askpass
            .as_ref()
            .unwrap()
            .jobs
            .lock()
            .unwrap()
            .is_empty());
        // A late prompt of the finished job is neither answered nor shown.
        let late = ask(
            addr,
            &token,
            Some(id.clone()),
            "Password for 'https://mira@github.com': ",
        );
        assert_eq!(late.join().unwrap(), None);
        assert!(service.try_recv_credential().is_none());
    }

    #[test]
    fn a_job_context_keeps_secrets_and_helpers_out_of_the_command() {
        let service = NetworkService::start();
        let mut auth = github_job(true, Some("s3cret-token"));
        auth.accounts.insert(
            HostKey::new("https", "git.example.com", Some(8443)),
            AccountCredential {
                account_id: "acc-2".to_owned(),
                username: "oauth2".to_owned(),
            },
        );
        let (context, guard) = service.job_context(auth);
        assert!(guard.is_some());
        assert_eq!(
            context.git_config,
            [
                "credential.https://git.example.com:8443.helper=",
                "credential.https://git.example.com:8443.useHttpPath=false",
                "credential.https://github.com.helper=",
                "credential.https://github.com.useHttpPath=false",
            ]
        );
        assert_eq!(
            context.config_args()[..2],
            ["-c", "credential.https://git.example.com:8443.helper="]
        );
        let text = format!("{:?}", context);
        assert!(!text.contains("s3cret-token"));
        assert!(context
            .env
            .iter()
            .any(|(k, v)| k == "GIT_ASKPASS" && !v.is_empty()));
        assert!(context
            .env
            .iter()
            .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == "1"));
    }

    #[test]
    fn a_background_job_context_does_not_touch_ssh_or_the_terminal() {
        let service = NetworkService::start();
        let (context, _guard) = service.job_context(github_job(false, Some("t")));
        let keys: Vec<&str> = context.env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"GIT_ASKPASS"));
        assert!(!keys.contains(&"SSH_ASKPASS"));
        assert!(context
            .env
            .iter()
            .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == "0"));
    }

    #[test]
    fn a_job_without_accounts_is_the_plain_context() {
        let service = NetworkService::start();
        let (context, guard) = service.job_context(JobAuth {
            interactive: false,
            accounts: HashMap::new(),
            store: None,
        });
        assert!(guard.is_none());
        assert!(context.git_config.is_empty());
        assert!(!context.env.iter().any(|(k, _)| k == "GIT_ASKPASS"));
    }

    fn parsed(text: &str) -> ParsedPrompt {
        parse_prompt(text).unwrap_or_else(|| panic!("should parse: {text}"))
    }

    #[test]
    fn parses_username_and_password_prompts() {
        let p = parsed("Username for 'https://github.com': ");
        assert_eq!(p.kind, PromptKind::Username);
        assert_eq!(p.host, github());
        assert_eq!(p.user, None);

        let p = parsed("Password for 'https://mira@github.com': ");
        assert_eq!(p.kind, PromptKind::Password);
        assert_eq!(p.host, github());
        assert_eq!(p.user.as_deref(), Some("mira"));
    }

    #[test]
    fn parses_ports_ipv6_and_case() {
        let p = parsed("Username for 'https://Git.Example.com:8443': ");
        assert_eq!(p.host, HostKey::new("https", "git.example.com", Some(8443)));
        // The default port is the same server as no port.
        assert_eq!(
            parsed("Username for 'https://github.com:443': ").host,
            github()
        );
        assert_eq!(
            parsed("Username for 'http://h.test:80': ").host,
            HostKey::new("http", "h.test", None)
        );

        let p = parsed("Password for 'https://u@[::1]:8443': ");
        assert_eq!(p.host, HostKey::new("https", "::1", Some(8443)));
        assert_eq!(p.host.url(), "https://[::1]:8443");
        let p = parsed("Username for 'https://[2001:db8::2]': ");
        assert_eq!(p.host, HostKey::new("https", "2001:db8::2", None));
        assert_eq!(p.host.url(), "https://[2001:db8::2]");
        assert_eq!(
            HostKey::new("https", "git.example.com", Some(8443)).url(),
            "https://git.example.com:8443"
        );
    }

    #[test]
    fn prompts_that_could_hide_another_host_are_not_parsed() {
        for text in [
            // Sub-paths (credential.useHttpPath is forced off for accounts).
            "Username for 'https://github.com/acme/app.git': ",
            "Password for 'https://mira@git.example.com:8443/g/p.git': ",
            "Username for 'https://github.com/@evil.test': ",
            "Username for 'https://github.com?@evil.test': ",
            "Username for 'https://github.com#@evil.test': ",
            "Password for 'https://u@evil.test/p@github.com': ",
            "Username for 'https://github.com\t@evil.test': ",
            "Username for 'https://git hub.com': ",
            "Username for 'https://[::1]x': ",
        ] {
            assert_eq!(parse_prompt(text), None, "{text:?}");
        }
        // An escaped slash stays in the user part: the host is evil.test.
        let p = parsed("Username for 'https://github.com%2F@evil.test': ");
        assert_eq!(p.host, HostKey::new("https", "evil.test", None));
        // The host is what follows the last `@`.
        let p = parsed("Password for 'https://a@b@github.com': ");
        assert_eq!(p.host, github());
        assert_eq!(p.user.as_deref(), Some("a@b"));
    }

    #[test]
    fn unknown_prompt_text_is_not_parsed() {
        for text in [
            "",
            "Enter passphrase for key '/home/u/.ssh/id_ed25519': ",
            "Password: ",
            "Username for https://github.com: ",
            "Username for 'github.com': ",
            "Username for 'https://': ",
            "Username for 'https://host:notaport': ",
            "Username for 'https://[::1': ",
            "Are you sure you want to continue connecting (yes/no)? ",
        ] {
            assert_eq!(parse_prompt(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_password_prompt_for_another_user_gets_no_token() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let (context, _guard) = service.job_context(github_job(false, Some("s3cret-token")));
        let id = job_id(&context);
        let other = ask(
            addr,
            &token,
            Some(id.clone()),
            "Password for 'https://someone-else@github.com': ",
        );
        assert_eq!(other.join().unwrap(), None);
        // No user at all is not the user we answered either.
        let none = ask(
            addr,
            &token,
            Some(id),
            "Password for 'https://github.com': ",
        );
        assert_eq!(none.join().unwrap(), None);
    }

    #[test]
    fn tokens_and_job_ids_are_random_128_bit_values() {
        let service = NetworkService::start();
        let (_, token) = endpoint(&service);
        assert_eq!(token.len(), 32);
        let (a, _ga) = service.job_context(github_job(true, Some("t")));
        let (b, _gb) = service.job_context(github_job(true, Some("t")));
        let (a, b) = (job_id(&a), job_id(&b));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }

    #[test]
    fn oversized_requests_are_dropped() {
        let service = NetworkService::start();
        let (addr, token) = endpoint(&service);
        let mut stream = TcpStream::connect(addr).unwrap();
        writeln!(stream, "{token}").unwrap();
        writeln!(stream).unwrap();
        writeln!(stream, "{}", MAX_PROMPT + 1).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // The server closes without answering and without asking the user.
        assert_eq!(reader.read_line(&mut line).unwrap(), 0);
        assert!(service.try_recv_credential().is_none());
    }
}
