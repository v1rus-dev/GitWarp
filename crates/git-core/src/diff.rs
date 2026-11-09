//! Commit details, the files a commit changed and per-file unified diffs.
//!
//! Everything is read from the `git` CLI, like the rest of this crate. The
//! parsers are free functions so the `-z` output formats can be tested
//! without a repository.

use crate::{check_output, Error, Oid, Repo, Result};

/// How a diff treats whitespace-only changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffWhitespace {
    /// Show every change.
    #[default]
    None,
    /// Ignore whitespace at line end (`--ignore-space-at-eol`).
    Eol,
    /// Ignore all whitespace (`-w`).
    All,
}

/// The context and whitespace options the Diff & merge page persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    /// Unchanged lines around a hunk, 0–20.
    pub context: u8,
    pub whitespace: DiffWhitespace,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context: 3,
            whitespace: DiffWhitespace::None,
        }
    }
}

impl DiffOptions {
    /// The arguments shared by every `git diff` invocation.
    pub(crate) fn args(&self) -> Vec<String> {
        let mut args = vec![format!("-U{}", self.context.min(20))];
        match self.whitespace {
            DiffWhitespace::None => {}
            DiffWhitespace::Eol => args.push("--ignore-space-at-eol".to_owned()),
            DiffWhitespace::All => args.push("-w".to_owned()),
        }
        args
    }
}

/// Largest patch (or blob) held in memory; a bigger one is reported instead.
const SIZE_CAP: usize = 2 * 1024 * 1024;
/// Largest number of patch lines the diff view will accept.
const LINE_CAP: usize = 20_000;

const FIELD: u8 = 0x1f;

/// A commit's full metadata, as the details pane shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetails {
    pub id: Oid,
    /// In git order: the first parent is the branch the commit was made on.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    /// Unix timestamp, seconds.
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    /// Unix timestamp, seconds.
    pub committer_time: i64,
    /// The first line of the message.
    pub summary: String,
    /// The message after the subject, without its leading or trailing blank
    /// lines.
    pub body: String,
    /// The raw full message (`%B`) without trailing newlines. Unlike `summary`
    /// (`%s`, which joins a wrapped first paragraph into one line) it is the text
    /// exactly as written, so copying it reproduces the commit message.
    pub message: String,
}

/// How a commit changed one file, as `git diff --name-status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    /// A letter this crate does not know (unmerged states, for example).
    Other(char),
}

impl ChangeKind {
    fn from_letter(letter: char) -> Self {
        match letter {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            other => Self::Other(other),
        }
    }

    /// The letter git uses for this kind, for the status glyph.
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Other(letter) => letter,
        }
    }
}

/// One file changed by a commit, counted against its first parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub kind: ChangeKind,
    /// The path in the parent, for a rename or a copy.
    pub old_path: Option<String>,
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

/// One line of a unified diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

impl LineKind {
    /// The marker drawn in the unified view.
    pub fn marker(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Added => '+',
            Self::Removed => '−',
        }
    }
}

/// One line inside a hunk, with the line numbers it occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The line without its trailing newline or its `+`/`-`/` ` marker.
    pub text: String,
    /// Whether git printed `\ No newline at end of file` after this line.
    pub no_newline: bool,
}

/// One `@@` hunk of a unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The raw header, e.g. `@@ -38,9 +38,14 @@ impl GraphLayout`.
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

/// A commit's diff for one file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub path: String,
    /// Git considers the content binary: there are no hunks to show.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// The old and new text of one file, for syntax highlighting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileTexts {
    /// The parent's text; `None` for an added file, a binary file or a file
    /// over the size cap.
    pub old: Option<String>,
    pub new: Option<String>,
    /// Either side is binary.
    pub binary: bool,
}

const DETAILS_FORMAT: &str =
    "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s%x1f%B";

/// Reads one commit's full metadata.
pub(crate) fn details(repo: &Repo, oid: Oid) -> Result<CommitDetails> {
    let out = repo
        .command()
        .args(["show", "-s", "--no-patch", DETAILS_FORMAT])
        .arg(oid.to_string())
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "show -s")?;
    parse_details(&out.stdout)
}

/// Lists the files a commit changed, diffed against its first parent (or the
/// empty tree for a root commit).
pub(crate) fn changes(repo: &Repo, oid: Oid) -> Result<Vec<ChangedFile>> {
    let parents = parents(repo, oid)?;
    let parent = parents.first().map(Oid::to_string);

    let numstat = diff_tree(repo, parent.as_deref(), oid, "--numstat")?;
    let name_status = diff_tree(repo, parent.as_deref(), oid, "--name-status")?;
    let stats = parse_numstat(&numstat)?;
    let files = parse_name_status(&name_status)?;
    Ok(files
        .into_iter()
        .map(|file| {
            let stat = stats.get(&file.path).copied().unwrap_or_default();
            ChangedFile {
                kind: file.kind,
                old_path: file.old_path,
                path: file.path,
                added: stat.added,
                deleted: stat.deleted,
                binary: stat.binary,
            }
        })
        .collect())
}

/// Reads the unified diff of one file against the commit's first parent.
pub(crate) fn file_diff(
    repo: &Repo,
    oid: Oid,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileDiff> {
    file_diff_with(repo, oid, path, old_path, DiffOptions::default())
}

/// [`file_diff`] with the user's context and whitespace options.
pub(crate) fn file_diff_with(
    repo: &Repo,
    oid: Oid,
    path: &str,
    old_path: Option<&str>,
    options: DiffOptions,
) -> Result<FileDiff> {
    let parents = parents(repo, oid)?;
    let parent = parents.first().map(Oid::to_string);

    let mut command = repo.command();
    command.args([
        "diff-tree",
        "-r",
        "-p",
        "--find-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-commit-id",
    ]);
    command.args(options.args());
    match &parent {
        Some(parent) => {
            command.arg(parent);
        }
        None => {
            command.arg("--root");
        }
    }
    command.arg(oid.to_string()).arg("--").arg(path);
    // Both sides of a rename must match the pathspec, or git reports the new
    // name as an addition instead of following the rename.
    if let Some(old_path) = old_path {
        command.arg(old_path);
    }
    let out = command.output().map_err(Error::Spawn)?;
    let out = check_output(out, "diff-tree -p")?;
    if out.stdout.len() > SIZE_CAP {
        return Err(Error::TooLarge { what: "diff" });
    }
    parse_patch(&out.stdout)
}

/// Reads the old and new text of one file for highlighting.
///
/// A missing side (an added or deleted file), a binary file and a file over
/// the size cap all come back as `None`; the diff view still renders from git.
pub(crate) fn file_texts(
    repo: &Repo,
    oid: Oid,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileTexts> {
    let parents = parents(repo, oid)?;
    let old_path = old_path.unwrap_or(path);
    let old = match parents.first() {
        Some(parent) => blob(repo, &parent.to_string(), old_path)?,
        None => None,
    };
    let new = blob(repo, &oid.to_string(), path)?;
    Ok(FileTexts {
        binary: old.as_deref().is_some_and(is_binary) || new.as_deref().is_some_and(is_binary),
        old: old
            .filter(|bytes| !is_binary(bytes))
            .and_then(bytes_to_text),
        new: new
            .filter(|bytes| !is_binary(bytes))
            .and_then(bytes_to_text),
    })
}

/// The parents of `oid`, in git order.
fn parents(repo: &Repo, oid: Oid) -> Result<Vec<Oid>> {
    let out = repo
        .command()
        .args(["rev-list", "--parents", "-n", "1"])
        .arg(oid.to_string())
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-list --parents")?;
    let line = String::from_utf8_lossy(&out.stdout);
    line.split_whitespace()
        .skip(1)
        .map(|hex| Oid::from_hex(hex).ok_or(Error::Parse("parent object id")))
        .collect()
}

/// Runs `diff-tree` with one output mode for the two sides.
fn diff_tree(repo: &Repo, parent: Option<&str>, oid: Oid, mode: &'static str) -> Result<Vec<u8>> {
    let mut command = repo.command();
    command.args(["diff-tree", "-r", "-z", "--find-renames", "--no-commit-id"]);
    match parent {
        Some(parent) => {
            command.arg(parent);
        }
        None => {
            command.arg("--root");
        }
    }
    let out = command
        .arg(mode)
        .arg(oid.to_string())
        .output()
        .map_err(Error::Spawn)?;
    Ok(check_output(out, "diff-tree")?.stdout)
}

/// One file's line counts, from `--numstat -z`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stat {
    pub(crate) added: usize,
    pub(crate) deleted: usize,
    pub(crate) binary: bool,
}

/// One file's status, from `--name-status -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedFile {
    pub(crate) kind: ChangeKind,
    pub(crate) old_path: Option<String>,
    pub(crate) path: String,
}

fn parse_details(raw: &[u8]) -> Result<CommitDetails> {
    let record = raw.split(|&b| b == FIELD).collect::<Vec<_>>();
    if record.len() < 10 {
        return Err(Error::Parse("commit details"));
    }
    let text = |i: usize| String::from_utf8_lossy(record[i]).into_owned();
    let id = Oid::from_hex(&text(0)).ok_or(Error::Parse("commit id"))?;
    let parents = text(1)
        .split_whitespace()
        .map(|hex| Oid::from_hex(hex).ok_or(Error::Parse("parent object id")))
        .collect::<Result<Vec<_>>>()?;
    let time = |i: usize| {
        text(i)
            .trim()
            .parse::<i64>()
            .map_err(|_| Error::Parse("commit time"))
    };
    let summary = text(8);
    // `%B` is the last field, so a separator inside the message survives.
    let message = record[9..]
        .iter()
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join("\u{1f}");
    let body = message_body(&message);
    let message = message.trim_end_matches(['\n', '\r']).to_string();
    Ok(CommitDetails {
        id,
        parents,
        author_name: text(2),
        author_email: text(3),
        author_time: time(4)?,
        committer_name: text(5),
        committer_email: text(6),
        committer_time: time(7)?,
        summary,
        body,
        message,
    })
}

/// The message without its subject and its surrounding blank lines.
fn message_body(message: &str) -> String {
    let message = message.trim_end_matches(['\n', '\r']);
    let body = match message.split_once('\n') {
        Some((_, body)) => body,
        None => "",
    };
    body.trim_matches(['\n', '\r']).to_string()
}

/// Parses `--numstat -z` output into per-path counts.
///
/// A rename prints an empty path field before the NUL and then the old and
/// new paths, so its counts are filed under the new path.
pub(crate) fn parse_numstat(raw: &[u8]) -> Result<std::collections::HashMap<String, Stat>> {
    let mut stats = std::collections::HashMap::new();
    let mut fields = raw.split(|&b| b == 0);
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, |&b| b == b'\t');
        let added = parts.next().ok_or(Error::Parse("numstat additions"))?;
        let deleted = parts.next().ok_or(Error::Parse("numstat deletions"))?;
        let path = parts.next().ok_or(Error::Parse("numstat path"))?;
        let path = if path.is_empty() {
            // Rename or copy: old path, then new path.
            fields.next();
            fields.next().ok_or(Error::Parse("numstat new path"))?
        } else {
            path
        };
        let count = |bytes: &[u8]| -> Result<usize> {
            if bytes == b"-" {
                return Ok(0);
            }
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.parse().ok())
                .ok_or(Error::Parse("numstat count"))
        };
        let binary = added == b"-" || deleted == b"-";
        stats.insert(
            String::from_utf8_lossy(path).into_owned(),
            Stat {
                added: count(added)?,
                deleted: count(deleted)?,
                binary,
            },
        );
    }
    Ok(stats)
}

/// Parses `--name-status -z` output, keeping git's order.
pub(crate) fn parse_name_status(raw: &[u8]) -> Result<Vec<NamedFile>> {
    let mut files = Vec::new();
    let mut fields = raw.split(|&b| b == 0);
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        let letter = text.chars().next().ok_or(Error::Parse("name-status"))?;
        let kind = ChangeKind::from_letter(letter);
        let (old_path, path) = if matches!(kind, ChangeKind::Renamed | ChangeKind::Copied) {
            let old = fields.next().ok_or(Error::Parse("name-status old path"))?;
            let new = fields.next().ok_or(Error::Parse("name-status new path"))?;
            (
                Some(String::from_utf8_lossy(old).into_owned()),
                String::from_utf8_lossy(new).into_owned(),
            )
        } else {
            (
                None,
                String::from_utf8_lossy(fields.next().ok_or(Error::Parse("name-status path"))?)
                    .into_owned(),
            )
        };
        files.push(NamedFile {
            kind,
            old_path,
            path,
        });
    }
    Ok(files)
}

/// Parses one file's unified patch.
pub(crate) fn parse_patch(raw: &[u8]) -> Result<FileDiff> {
    let text = String::from_utf8_lossy(raw);
    if text.lines().count() > LINE_CAP {
        return Err(Error::TooLarge { what: "diff" });
    }
    let mut diff = FileDiff::default();
    // The paths in the `diff --git` header are only a fallback for a patch
    // whose `+++`/`---` lines are absent (a binary file).
    let mut old_path: Option<String> = None;
    let mut path: Option<String> = None;
    // Inside a hunk an added line can itself start with `+++ ` (or a removed
    // line with `--- `), which must not be mistaken for a file header.
    let mut in_hunk = false;

    for line in text.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("diff --git ") {
            in_hunk = false;
            if let Some((a, b)) = parse_diff_git_header(line) {
                old_path = Some(a);
                path = Some(b);
            }
        } else if !in_hunk && line.starts_with("--- ") {
            let rest = &line[4..];
            if rest != "/dev/null" {
                old_path = Some(strip_ab_prefix(rest));
            }
        } else if !in_hunk && line.starts_with("+++ ") {
            let rest = &line[4..];
            if rest != "/dev/null" {
                path = Some(strip_ab_prefix(rest));
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            diff.binary = true;
        } else if let Some(header) = line.strip_prefix("@@ ") {
            in_hunk = true;
            let header = format!("@@ {header}");
            let (old_start, old_count, new_start, new_count) =
                parse_hunk_header(&header).ok_or(Error::Parse("hunk header"))?;
            diff.hunks.push(Hunk {
                header,
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
        } else if line.starts_with('\\') {
            if let Some(last) = diff.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
                last.no_newline = true;
            }
        } else if let Some(hunk) = diff.hunks.last_mut() {
            let (kind, text) = match line.chars().next() {
                Some(' ') => (LineKind::Context, &line[1..]),
                Some('+') => (LineKind::Added, &line[1..]),
                Some('-') => (LineKind::Removed, &line[1..]),
                _ => continue,
            };
            let old_line = match kind {
                LineKind::Added => None,
                _ => Some(
                    hunk.old_start
                        + hunk
                            .lines
                            .iter()
                            .filter(|line| line.kind != LineKind::Added)
                            .count() as u32,
                ),
            };
            let new_line = match kind {
                LineKind::Removed => None,
                _ => Some(
                    hunk.new_start
                        + hunk
                            .lines
                            .iter()
                            .filter(|line| line.kind != LineKind::Removed)
                            .count() as u32,
                ),
            };
            hunk.lines.push(DiffLine {
                kind,
                old_line,
                new_line,
                text: text.to_string(),
                no_newline: false,
            });
        }
    }

    if let Some(name) = path {
        diff.path = name;
    } else if let Some(name) = old_path.clone() {
        diff.path = name;
    }
    diff.old_path = old_path.filter(|old| Some(old) != Some(&diff.path));
    Ok(diff)
}

/// Number of lines of `kinds` already in the hunk.
/// The `a/…` and `b/…` paths of a `diff --git` line.
fn parse_diff_git_header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    // Quoted paths can contain spaces; the unquoted form is `a/x b/y`.
    let (a, b) = rest.rsplit_once(" b/")?;
    Some((strip_ab_prefix(a), b.to_string()))
}

fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// `@@ -a,b +c,d @@ section` → the four numbers.
pub(crate) fn parse_hunk_header(header: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = header.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let mut parts = ranges.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// Reads one blob as bytes, capped; `None` when the path does not exist there.
fn blob(repo: &Repo, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let object = format!("{rev}:{path}");
    let out = repo
        .command()
        .args(["cat-file", "blob", "--"])
        .arg(object)
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    if out.stdout.len() > SIZE_CAP {
        return Ok(None);
    }
    Ok(Some(out.stdout))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn bytes_to_text(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_options_build_the_context_and_whitespace_arguments() {
        assert_eq!(DiffOptions::default().args(), vec!["-U3".to_owned()]);
        assert_eq!(
            DiffOptions {
                context: 0,
                whitespace: DiffWhitespace::Eol,
            }
            .args(),
            vec!["-U0".to_owned(), "--ignore-space-at-eol".to_owned()]
        );
        assert_eq!(
            DiffOptions {
                context: 99,
                whitespace: DiffWhitespace::All,
            }
            .args(),
            vec!["-U20".to_owned(), "-w".to_owned()]
        );
    }

    #[test]
    fn parses_hunk_headers_with_and_without_counts() {
        assert_eq!(
            parse_hunk_header("@@ -38,9 +38,14 @@ impl GraphLayout"),
            Some((38, 9, 38, 14))
        );
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1, 1, 1)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,11 @@"), Some((0, 0, 1, 11)));
        assert_eq!(parse_hunk_header("nonsense"), None);
    }

    #[test]
    fn parses_details_with_a_multiline_body() {
        let parents = format!("{} {}", "b".repeat(40), "c".repeat(40));
        let raw = format!(
            "{}\x1f{parents}\x1fAnn\x1fann@x.dev\x1f1700000000\x1fBob\x1fbob@x.dev\x1f1700000100\x1fAdd a thing\x1fAdd a thing\n\nWhy it exists.\n\nCloses #1\n",
            "a".repeat(40),
        );
        let details = parse_details(raw.as_bytes()).unwrap();
        assert_eq!(details.id.to_string(), "a".repeat(40));
        assert_eq!(details.parents.len(), 2);
        assert_eq!(details.author_name, "Ann");
        assert_eq!(details.committer_name, "Bob");
        assert_eq!(details.committer_time, 1_700_000_100);
        assert_eq!(details.summary, "Add a thing");
        assert_eq!(details.body, "Why it exists.\n\nCloses #1");
        assert_eq!(
            details.message,
            "Add a thing\n\nWhy it exists.\n\nCloses #1"
        );
    }

    #[test]
    fn full_message_keeps_a_wrapped_subject_verbatim() {
        // `%s` joins the wrapped first paragraph into one line; `%B` does not.
        let raw = format!(
            "{}\x1f\x1fAnn\x1fann@x.dev\x1f1700000000\x1fBob\x1fbob@x.dev\x1f1700000100\x1fA long subject wrapped over two lines\x1fA long subject wrapped\nover two lines\n\nBody\n",
            "a".repeat(40),
        );
        let details = parse_details(raw.as_bytes()).unwrap();
        assert_eq!(
            details.message,
            "A long subject wrapped\nover two lines\n\nBody"
        );
    }

    #[test]
    fn parses_numstat_records_including_renames_and_binary() {
        let raw = b"9\t3\tsrc/a.rs\0-\t-\timg.png\x002\t1\t\0old.txt\0new.txt\0";
        let stats = parse_numstat(raw).unwrap();
        assert_eq!(
            stats.get("src/a.rs"),
            Some(&Stat {
                added: 9,
                deleted: 3,
                binary: false
            })
        );
        assert_eq!(
            stats.get("img.png"),
            Some(&Stat {
                added: 0,
                deleted: 0,
                binary: true
            })
        );
        assert_eq!(
            stats.get("new.txt"),
            Some(&Stat {
                added: 2,
                deleted: 1,
                binary: false
            })
        );
        assert!(!stats.contains_key("old.txt"));
    }

    #[test]
    fn parses_name_status_renames_and_plain_files() {
        let raw = b"M\0src/a.rs\0R050\0old.txt\0new.txt\0A\0fresh.rs\0";
        let files = parse_name_status(raw).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].kind, ChangeKind::Modified);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[1].kind, ChangeKind::Renamed);
        assert_eq!(files[1].old_path.as_deref(), Some("old.txt"));
        assert_eq!(files[1].path, "new.txt");
        assert_eq!(files[2].kind, ChangeKind::Added);
    }

    #[test]
    fn parses_a_patch_into_hunks_with_line_numbers() {
        let patch = concat!(
            "diff --git a/src/a.rs b/src/a.rs\n",
            "index 1..2 100644\n",
            "--- a/src/a.rs\n",
            "+++ b/src/a.rs\n",
            "@@ -1,3 +1,4 @@ fn main\n",
            " one\n",
            "-two\n",
            "+TWO\n",
            " three\n",
            "+four\n",
            "\\ No newline at end of file\n",
        );
        let diff = parse_patch(patch.as_bytes()).unwrap();
        assert_eq!(diff.path, "src/a.rs");
        assert_eq!(diff.old_path, None);
        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];
        assert_eq!(hunk.header, "@@ -1,3 +1,4 @@ fn main");
        let lines: Vec<_> = hunk
            .lines
            .iter()
            .map(|l| (l.kind, l.old_line, l.new_line, l.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            vec![
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Removed, Some(2), None, "two"),
                (LineKind::Added, None, Some(2), "TWO"),
                (LineKind::Context, Some(3), Some(3), "three"),
                (LineKind::Added, None, Some(4), "four"),
            ]
        );
        assert!(hunk.lines[4].no_newline);
    }

    #[test]
    fn diff_markers_inside_a_hunk_are_line_text_not_headers() {
        let patch = concat!(
            "diff --git a/a.md b/a.md\n",
            "--- a/a.md\n",
            "+++ b/a.md\n",
            "@@ -1,2 +1,2 @@\n",
            "--- a removed line that starts with dashes\n",
            "+++ an added line that starts with pluses\n",
        );
        let diff = parse_patch(patch.as_bytes()).unwrap();
        assert_eq!(diff.path, "a.md");
        assert_eq!(diff.hunks.len(), 1);
        let kinds: Vec<_> = diff.hunks[0].lines.iter().map(|line| line.kind).collect();
        assert_eq!(kinds, [LineKind::Removed, LineKind::Added]);
        assert_eq!(
            diff.hunks[0].lines[1].text,
            "++ an added line that starts with pluses"
        );
    }

    #[test]
    fn parses_binary_patches_and_rename_headers() {
        let patch = b"diff --git a/old.bin b/new.bin\n\
similarity index 100%\n\
rename from old.bin\n\
rename to new.bin\n\
Binary files a/old.bin and b/new.bin differ\n";
        let diff = parse_patch(patch).unwrap();
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
        assert_eq!(diff.path, "new.bin");
        assert_eq!(diff.old_path.as_deref(), Some("old.bin"));
    }

    #[test]
    fn an_empty_patch_is_an_empty_diff() {
        let diff = parse_patch(b"").unwrap();
        assert!(diff.hunks.is_empty());
        assert!(!diff.binary);
    }

    #[test]
    fn strips_the_subject_from_the_body() {
        assert_eq!(message_body("subject\n\nbody\nmore\n"), "body\nmore");
        assert_eq!(message_body("subject"), "");
        assert_eq!(message_body("subject\n\n"), "");
    }
}
