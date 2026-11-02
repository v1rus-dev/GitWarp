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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use git_core::network::{CancelToken, NetworkContext};
use git_core::ssh::{HostKeyStatus, SshTarget};

const ADDR_ENV: &str = "GITWARP_ASKPASS_ADDR";
const TOKEN_ENV: &str = "GITWARP_ASKPASS_TOKEN";
/// How long a helper waits for the user before it gives up and git fails.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(600);

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
                env.extend(askpass.variables());
            }
        }
        NetworkContext::with_env(env)
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
}

impl AskpassServer {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        // A local token is enough to keep unrelated processes from answering
        // prompts; it is never printed or logged.
        let token = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let (tx, rx) = mpsc::channel();
        let thread_token = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                let token = thread_token.clone();
                std::thread::spawn(move || serve(stream, &token, tx));
            }
        });
        Ok(Self {
            addr,
            token,
            prompts: Mutex::new(rx),
        })
    }

    fn variables(&self) -> Vec<(String, String)> {
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "gitwarp".to_owned());
        vec![
            ("GIT_ASKPASS".to_owned(), exe.clone()),
            ("SSH_ASKPASS".to_owned(), exe),
            // OpenSSH 8.4+ honours SSH_ASKPASS_REQUIRE=force without DISPLAY.
            ("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned()),
            ("GITWARP_ASKPASS_ADDR".to_owned(), self.addr.to_string()),
            ("GITWARP_ASKPASS_TOKEN".to_owned(), self.token.clone()),
        ]
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
/// `TOKEN\n<length>\n<prompt bytes>` then `OK <length>\n<answer>` or
/// `CANCEL`.
fn serve(mut stream: TcpStream, token: &str, tx: Sender<CredentialPrompt>) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim_end() != token {
        let _ = writeln!(stream, "CANCEL");
        return;
    }
    line.clear();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let Ok(length) = line.trim().parse::<usize>() else {
        return;
    };
    let mut prompt = vec![0u8; length];
    if reader.read_exact(&mut prompt).is_err() {
        return;
    }
    let prompt = String::from_utf8_lossy(&prompt).into_owned();
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

    #[test]
    fn askpass_round_trip_reaches_the_ui_and_returns_the_answer() {
        let service = NetworkService::start();
        let askpass = service.askpass.as_ref().expect("the listener starts");
        let addr = askpass.addr;
        let token = askpass.token.clone();
        let helper = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            writeln!(stream, "{token}").unwrap();
            let prompt = "Password for 'https://example.test': ";
            writeln!(stream, "{}", prompt.len()).unwrap();
            stream.write_all(prompt.as_bytes()).unwrap();
            stream.flush().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let length = line
                .trim_end()
                .strip_prefix("OK ")
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let mut answer = vec![0u8; length];
            reader.read_exact(&mut answer).unwrap();
            String::from_utf8(answer).unwrap()
        });

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
        assert_eq!(helper.join().unwrap(), "hunter2");
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let service = NetworkService::start();
        let askpass = service.askpass.as_ref().expect("the listener starts");
        let addr = askpass.addr;
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
}
