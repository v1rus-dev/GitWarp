use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, Stdio};
use std::thread::JoinHandle;

use crate::{Error, Oid, Repo, Result};

const FIELD: u8 = 0x1f;
const RECORD: u8 = 0x1e;
//                 id   parents  author   email    time     subject
const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1e";

/// One commit as shown in the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: Oid,
    /// In git order: the first parent is the branch the commit was made on.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    /// Unix timestamp, seconds.
    pub author_time: i64,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct LogOptions {
    /// Show all branches, remotes and tags (stash excluded). When false, only
    /// `revs` (or HEAD if empty).
    pub all: bool,
    /// Extra revisions or ranges, e.g. `main`, `v1.0..HEAD`.
    pub revs: Vec<String>,
    /// Only commits touching these paths (file history).
    pub paths: Vec<String>,
    pub max_count: Option<usize>,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            all: true,
            revs: Vec::new(),
            paths: Vec::new(),
            max_count: None,
        }
    }
}

/// Commits streamed from a running `git log`. Dropping the stream stops git.
pub struct LogStream {
    /// `None` once the log is over (or was empty from the start).
    running: Option<Running>,
    buf: Vec<u8>,
}

struct Running {
    child: Child,
    reader: BufReader<ChildStdout>,
    /// Drained on its own thread: a full stderr pipe would block git while we
    /// are still reading stdout.
    stderr: Option<JoinHandle<String>>,
}

impl LogStream {
    pub(crate) fn spawn(repo: &Repo, options: &LogOptions) -> Result<Self> {
        // `git log` on a repository without commits fails when it has to
        // default to HEAD (`--all` just prints nothing): that is an empty log.
        if !options.all && options.revs.is_empty() && !has_head(repo)? {
            return Ok(Self {
                running: None,
                buf: Vec::new(),
            });
        }
        let mut cmd = repo.command();
        cmd.args(["log", "--topo-order", "--no-decorate", FORMAT]);
        if options.all {
            // The stash is shown separately, like Fork does.
            cmd.args(["--exclude=refs/stash", "--all"]);
        }
        if let Some(n) = options.max_count {
            cmd.arg(format!("--max-count={n}"));
        }
        // A rev starting with `-` must not be taken for an option.
        cmd.arg("--end-of-options");
        cmd.args(&options.revs);
        cmd.arg("--");
        cmd.args(&options.paths);

        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Spawn)?;
        let stdout = child.stdout.take().ok_or(Error::Parse("git log stdout"))?;
        let stderr = child.stderr.take().map(|mut e| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = e.read_to_end(&mut bytes);
                String::from_utf8_lossy(&bytes).into_owned()
            })
        });
        Ok(Self {
            running: Some(Running {
                child,
                reader: BufReader::with_capacity(256 * 1024, stdout),
                stderr,
            }),
            buf: Vec::with_capacity(256),
        })
    }

    /// Read up to `n` commits; an empty vec means the log is over.
    /// Handy for feeding the UI in pages.
    pub fn next_chunk(&mut self, n: usize) -> Result<Vec<CommitInfo>> {
        let mut out = Vec::with_capacity(n);
        for item in self.by_ref().take(n) {
            out.push(item?);
        }
        Ok(out)
    }

    fn finish(&mut self) -> Option<Error> {
        let mut run = self.running.take()?;
        let status = run.child.wait();
        // The child has exited, so its stderr hits EOF and the thread ends.
        let stderr = run
            .stderr
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        match status {
            Ok(status) if status.success() => None,
            Ok(_) => Some(Error::Git {
                command: "log",
                stderr,
            }),
            Err(e) => Some(Error::Io(e)),
        }
    }
}

fn has_head(repo: &Repo) -> Result<bool> {
    let out = repo
        .command()
        .args(["rev-parse", "-q", "--verify", "HEAD"])
        .output()
        .map_err(Error::Spawn)?;
    Ok(out.status.success())
}

impl Iterator for LogStream {
    type Item = Result<CommitInfo>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let run = self.running.as_mut()?;
            self.buf.clear();
            match run.reader.read_until(RECORD, &mut self.buf) {
                Ok(0) => return self.finish().map(Err),
                Ok(_) => {
                    // git puts a newline between records; skip blank leftovers.
                    let record = trim_record(&self.buf);
                    if record.is_empty() {
                        continue;
                    }
                    return Some(parse_record(record));
                }
                Err(e) => {
                    // The stream is unusable; stop git instead of leaking it.
                    self.running = None;
                    return Some(Err(Error::Io(e)));
                }
            }
        }
    }
}

impl Drop for Running {
    /// Kill and reap the child. The stderr thread is not joined: it only ends
    /// once git closes the pipe, and it finishes on its own right after the
    /// kill. Harmless after `finish`: std ignores a kill of a reaped child.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn trim_record(raw: &[u8]) -> &[u8] {
    let raw = raw.strip_suffix(&[RECORD]).unwrap_or(raw);
    let start = raw
        .iter()
        .position(|&b| b != b'\n' && b != b'\r')
        .unwrap_or(raw.len());
    &raw[start..]
}

fn parse_record(record: &[u8]) -> Result<CommitInfo> {
    let mut fields = record.splitn(6, |&b| b == FIELD);
    let mut next = |what| fields.next().ok_or(Error::Parse(what));

    let id = parse_oid(next("commit id")?)?;
    let parents = next("parents")?
        .split(|&b| b == b' ')
        .filter(|p| !p.is_empty())
        .map(parse_oid)
        .collect::<Result<Vec<_>>>()?;
    let author_name = lossy(next("author name")?);
    let author_email = lossy(next("author email")?);
    let author_time = std::str::from_utf8(next("author time")?)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .ok_or(Error::Parse("author time"))?;
    let summary = lossy(next("subject")?);

    Ok(CommitInfo {
        id,
        parents,
        author_name,
        author_email,
        author_time,
        summary,
    })
}

fn parse_oid(bytes: &[u8]) -> Result<Oid> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(Oid::from_hex)
        .ok_or(Error::Parse("object id"))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_record() {
        let id = "a".repeat(40);
        let p1 = "b".repeat(40);
        let p2 = "c".repeat(40);
        let raw = format!(
            "\n{id}\x1f{p1} {p2}\x1fЕгор\x1fe@x.dev\x1f1700000000\x1fMerge: fix\x1fthings\x1e"
        );
        let c = parse_record(trim_record(raw.as_bytes())).unwrap();
        assert_eq!(c.id.to_string(), id);
        assert_eq!(c.parents.len(), 2);
        assert_eq!(c.author_name, "Егор");
        assert_eq!(c.author_time, 1_700_000_000);
        // The subject is the last field, so separators inside it survive.
        assert_eq!(c.summary, "Merge: fix\x1fthings");
    }

    #[test]
    fn root_commit_has_no_parents() {
        let raw = format!("{}\x1f\x1fa\x1fb\x1f1\x1finit", "d".repeat(40));
        assert!(parse_record(raw.as_bytes()).unwrap().parents.is_empty());
    }
}
