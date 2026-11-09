//! The working tree: which files changed on each side of the index, and the
//! operations that move changes between the work tree, the index and a commit.
//!
//! Everything runs through the `git` CLI. The parsers are free functions so
//! the machine-readable output can be tested without a repository.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::conflict::IndexStage;
use crate::diff::{parse_name_status, parse_numstat};
use crate::{check_output, ChangeKind, Error, FileDiff, Oid, Repo, Result};

/// Largest untracked file whose lines are counted for the `+N` column.
pub(crate) const UNTRACKED_COUNT_CAP: u64 = 2 * 1024 * 1024;

/// One file with local changes, on one side of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingFile {
    pub kind: ChangeKind,
    /// The path in the index / HEAD, for a rename or a copy.
    pub old_path: Option<String>,
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
    /// The file is not tracked by git at all (`?` in `git status`).
    pub untracked: bool,
}

/// HEAD as the commit box needs it: the short id and subject for the amend
/// hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadCommit {
    pub id: Oid,
    pub short: String,
    pub summary: String,
    /// The full message, for the amend editor.
    pub message: String,
}

impl HeadCommit {
    /// The short id, when the summary alone is not enough.
    pub fn short_oid(&self) -> &str {
        &self.short
    }
}

/// Everything the local-changes view shows about the work tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingStatus {
    /// The checked-out branch; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// HEAD's commit; `None` in an empty repository.
    pub head: Option<HeadCommit>,
    /// Changes between HEAD and the index, in path order.
    pub staged: Vec<WorkingFile>,
    /// Changes between the index and the work tree, in path order. Untracked
    /// files are here too; conflicts are not (see [`Self::conflicted`]).
    pub unstaged: Vec<WorkingFile>,
    /// Unmerged paths of a merge/rebase/cherry-pick in progress, in path
    /// order. Each path is listed here only: `git diff` and `git diff
    /// --cached` also report an unmerged path (as `U` and as `M`), so those
    /// entries are dropped from `staged` and `unstaged`.
    pub conflicted: Vec<WorkingFile>,
}

impl WorkingStatus {
    pub fn staged_count(&self) -> usize {
        self.staged.len()
    }

    pub fn unstaged_count(&self) -> usize {
        self.unstaged.len()
    }

    pub fn conflicted_count(&self) -> usize {
        self.conflicted.len()
    }

    /// The amend hint: `e4f2a91 · feat: …`, or the short id alone.
    pub fn head_label(&self) -> Option<String> {
        self.head.as_ref().map(|head| {
            if head.summary.is_empty() {
                head.short.clone()
            } else {
                format!("{} · {}", head.summary, head.short)
            }
        })
    }
}

/// One selected hunk, or a set of selected lines inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkPick {
    /// Index into [`FileDiff::hunks`].
    pub hunk: usize,
    /// Line indices into `hunks[hunk].lines`; `None` picks the whole hunk.
    pub lines: Option<Vec<usize>>,
}

impl HunkPick {
    pub fn whole(hunk: usize) -> Self {
        Self { hunk, lines: None }
    }

    pub fn lines(hunk: usize, mut lines: Vec<usize>) -> Self {
        lines.sort_unstable();
        lines.dedup();
        Self {
            hunk,
            lines: Some(lines),
        }
    }

    fn picks_line(&self, line: usize) -> bool {
        match &self.lines {
            None => true,
            Some(lines) => lines.contains(&line),
        }
    }
}

/// One path with local changes and a one-letter status for the worktree page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedEntry {
    pub path: String,
    /// `M` modified, `A` added, `D` deleted, `R` renamed (`path` is the new
    /// name), `U` conflicted, `?` untracked.
    pub status: char,
}

/// The paths with local changes, staged or not, including untracked and
/// conflicted files. One entry per path, sorted; line counts are not read.
///
/// This is the cheap list the worktree page shows; [`status`] is the full
/// working-tree view with added/deleted counts.
pub(crate) fn changed_paths(repo: &Repo) -> Result<Vec<String>> {
    Ok(changed_entries(repo)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

/// Like [`changed_paths`], with a status letter per path.
pub(crate) fn changed_entries(repo: &Repo) -> Result<Vec<ChangedEntry>> {
    let out = repo
        .read_command()
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "status")?;
    let mut entries = parse_changed_entries(&out.stdout);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    Ok(entries)
}

/// `git status --porcelain=v1 -z`: `XY SP PATH NUL`; a rename or copy is
/// followed by the original path as its own NUL field.
fn parse_changed_entries(raw: &[u8]) -> Vec<ChangedEntry> {
    let mut entries = Vec::new();
    let mut fields = raw.split(|&byte| byte == 0);
    while let Some(record) = fields.next() {
        if record.len() < 4 {
            continue;
        }
        entries.push(ChangedEntry {
            path: String::from_utf8_lossy(&record[3..]).into_owned(),
            status: status_letter(record[0], record[1]),
        });
        if matches!(record[0], b'R' | b'C') {
            // The original path of a rename/copy.
            fields.next();
        }
    }
    entries
}

/// Collapses the two-column `XY` code into one letter. Conflicts win, then
/// renames, additions (a copy counts as an addition), deletions; everything
/// else (modified, type change) is `M`.
fn status_letter(x: u8, y: u8) -> char {
    match (x, y) {
        (b'?', b'?') => '?',
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => 'U',
        _ if x == b'R' || y == b'R' => 'R',
        _ if x == b'A' || x == b'C' || y == b'C' => 'A',
        _ if x == b'D' || y == b'D' => 'D',
        _ => 'M',
    }
}

#[cfg(test)]
fn parse_changed_paths(raw: &[u8]) -> Vec<String> {
    parse_changed_entries(raw)
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

/// Reads the whole working-tree status.
pub(crate) fn status(repo: &Repo) -> Result<WorkingStatus> {
    let porcelain = porcelain(repo)?;
    let mut staged = side(repo, true)?;
    let mut unstaged = side(repo, false)?;

    // An unmerged path shows up in both diffs too (`U` unstaged, `U`/`M`
    // staged); the porcelain's `u` records are the only source for conflicts.
    let conflicted_set: std::collections::HashSet<&str> =
        porcelain.conflicted.iter().map(String::as_str).collect();
    staged.retain(|file| !conflicted_set.contains(file.path.as_str()));
    unstaged.retain(|file| !conflicted_set.contains(file.path.as_str()));
    for path in porcelain.untracked {
        if unstaged.iter().any(|file| file.path == path) {
            continue;
        }
        unstaged.push(untracked_file(repo, path));
    }
    let mut conflicted: Vec<WorkingFile> = porcelain
        .conflicted
        .into_iter()
        .map(|path| WorkingFile {
            kind: ChangeKind::Other('U'),
            old_path: None,
            path,
            added: 0,
            deleted: 0,
            binary: false,
            untracked: false,
        })
        .collect();
    sort_files(&mut staged);
    sort_files(&mut unstaged);
    sort_files(&mut conflicted);

    Ok(WorkingStatus {
        branch: porcelain.branch,
        head: head_commit(repo)?,
        staged,
        unstaged,
        conflicted,
    })
}

/// HEAD's commit, or `None` while the branch is unborn.
pub(crate) fn head_commit(repo: &Repo) -> Result<Option<HeadCommit>> {
    let out = repo
        .command()
        .args(["log", "-1", "--format=%H%x1f%h%x1f%s%x1f%B"])
        .output()
        .map_err(Error::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.trim_end_matches('\n').splitn(4, '\x1f');
    let id = fields
        .next()
        .and_then(Oid::from_hex)
        .ok_or(Error::Parse("HEAD object id"))?;
    let short = fields.next().unwrap_or_default().to_string();
    let summary = fields.next().unwrap_or_default().to_string();
    let message = fields.next().unwrap_or_default().to_string();
    Ok(Some(HeadCommit {
        id,
        short,
        summary,
        message,
    }))
}

/// A git command whose pathspecs are all literal: user paths such as
/// `a[1].txt` must not glob onto other files (`a1.txt`).
pub(crate) fn literal_command(repo: &Repo) -> std::process::Command {
    let mut command = repo.command();
    command.env("GIT_LITERAL_PATHSPECS", "1");
    command
}

/// A private copy of the index for work-tree `git diff` runs.
///
/// `git diff` against the work tree silently refreshes the index's stat cache
/// and writes it back under `index.lock`, and it ignores `GIT_OPTIONAL_LOCKS=0`
/// (checked on git 2.56; `git status` and `git diff --cached` do honour it or
/// never write). A background refresh must not touch the real index, so the
/// diff gets a copy to refresh instead. The copy lives in the git dir because
/// a split index finds its `sharedindex.*` files there. Dropped after the run.
struct ScratchIndex(PathBuf);

/// File-name prefix of the scratch copies; the name is
/// `index.gitwarp-<pid>-<n>-<unix seconds>`, so a stale one can be recognised
/// by age (process liveness is not portable).
const SCRATCH_PREFIX: &str = "index.gitwarp-";
/// A scratch copy older than this is a leftover of a crashed run.
const SCRATCH_STALE_SECS: u64 = 3600;

type WarningSink = fn(&str);
static WARNING_SINK: std::sync::OnceLock<WarningSink> = std::sync::OnceLock::new();

/// Where the crate reports problems it recovers from (git-core has no logger
/// of its own and takes no logging dependency). The first call wins.
pub fn set_warning_sink(sink: WarningSink) {
    let _ = WARNING_SINK.set(sink);
}

/// Reports `message` once per git dir.
fn warn_once(git_dir: &Path, message: &str) {
    static SEEN: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut seen) = SEEN.lock() else { return };
    if seen.iter().any(|dir| dir == git_dir) {
        return;
    }
    seen.push(git_dir.to_path_buf());
    if let Some(sink) = WARNING_SINK.get() {
        sink(message);
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl ScratchIndex {
    /// `None` when there is no index to copy (a fresh repository): nothing
    /// to refresh then, so the plain command is already read-only.
    ///
    /// Also `None` when making the copy fails (warned once per repository).
    /// The diff then runs on the real index, still with `GIT_OPTIONAL_LOCKS=0`
    /// (which `git diff` ignores): residual risk is that it briefly holds
    /// `index.lock` and rewrites the stat cache, so a `git commit` started in
    /// the same instant can fail with "index.lock: File exists". Rare (needs a
    /// full disk, a permission problem or a filesystem without `set_modified`),
    /// and preferable to showing no diff.
    fn copy_of(repo: &Repo) -> Option<Self> {
        let original = repo.git_dir().join("index");
        match fs::metadata(&original) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                Self::warn_failed(repo, &error);
                return None;
            }
        }
        match Self::try_copy(repo, &original) {
            Ok(scratch) => Some(scratch),
            Err(error) => {
                Self::warn_failed(repo, &error);
                None
            }
        }
    }

    fn warn_failed(repo: &Repo, error: &std::io::Error) {
        warn_once(
            repo.git_dir(),
            &format!(
                "cannot copy the index for a read-only diff ({error}); diffing the real index, \
                 which may briefly take index.lock"
            ),
        );
    }

    fn try_copy(repo: &Repo, original: &Path) -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let copy = repo.git_dir().join(format!(
            "{SCRATCH_PREFIX}{}-{n}-{}",
            std::process::id(),
            unix_now()
        ));
        // The guard exists before the copy, so Drop removes a partly written one.
        let scratch = Self(copy);
        fs::copy(original, &scratch.0)?;
        // Keep the original's mtime: git decides which entries are "racily clean" (same
        // second as the index was written, so their stat data proves nothing) by comparing
        // against the index file's mtime. A fresh mtime on the copy makes such entries look
        // clean and a same-size edit made right after a `git add`/`commit` would be missed.
        let mtime = fs::metadata(original)?.modified()?;
        fs::OpenOptions::new()
            .write(true)
            .open(&scratch.0)?
            .set_modified(mtime)?;
        Ok(scratch)
    }

    fn apply(&self, command: &mut std::process::Command) {
        command.env("GIT_INDEX_FILE", &self.0);
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Removes scratch index copies (and their git lock files) that a crashed run
/// left in the git dir: those whose embedded timestamp is over an hour old.
/// Age rather than process liveness because the latter is not portable.
/// Returns how many files were removed. Call off the UI thread.
pub(crate) fn remove_stale_scratch_indexes(git_dir: &Path, now_secs: u64) -> usize {
    let Ok(entries) = fs::read_dir(git_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.filter_map(std::result::Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(SCRATCH_PREFIX) else {
            continue;
        };
        let stamp = rest
            .trim_end_matches(".lock")
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<u64>().ok());
        if stamp.is_some_and(|t| now_secs.saturating_sub(t) > SCRATCH_STALE_SECS)
            && fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// Adds `paths` to the index, including deletions and new files.
pub(crate) fn stage(repo: &Repo, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut command = literal_command(repo);
    command.args(["add", "-A", "--"]);
    command.args(paths);
    let out = command.output().map_err(Error::Spawn)?;
    check_output(out, "add").map(drop)
}

/// Which side of a merge wins when a conflicted file is resolved wholesale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    /// The branch being merged into (HEAD): index stage 2.
    Ours,
    /// The branch being merged in: index stage 3.
    Theirs,
}

/// The unmerged index entries (stage 1 base, 2 ours, 3 theirs) of `path`,
/// from `git ls-files -u -z`; empty when the path is not conflicted.
pub(crate) fn unmerged(repo: &Repo, path: &str) -> Result<Vec<IndexStage>> {
    let out = literal_command(repo)
        .args(["ls-files", "-u", "-z", "--"])
        .arg(path)
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "ls-files -u")?;
    let mut entries = Vec::new();
    // `<mode> SP <oid> SP <stage> TAB <path> NUL`
    for record in out.stdout.split(|&byte| byte == 0) {
        let Some(tab) = record.iter().position(|&b| b == b'\t') else {
            continue;
        };
        if &record[tab + 1..] != path.as_bytes() {
            continue;
        }
        let head = String::from_utf8_lossy(&record[..tab]);
        let mut fields = head.split(' ');
        let (Some(mode), Some(oid), Some(stage)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(Error::Parse("ls-files -u entry"));
        };
        let stage = match stage {
            "1" => 1,
            "2" => 2,
            "3" => 3,
            _ => return Err(Error::Parse("ls-files -u stage")),
        };
        entries.push(IndexStage {
            stage,
            mode: mode.to_string(),
            oid: oid.to_string(),
        });
    }
    Ok(entries)
}

/// Resolves a conflicted `path` by taking one side as is.
///
/// If the chosen side does not have the file (it deleted it: a modify/delete
/// conflict), the resolution is the deletion, `git rm`; otherwise
/// `checkout --ours|--theirs` followed by `git add`.
pub(crate) fn resolve_take(repo: &Repo, path: &str, side: ConflictSide) -> Result<()> {
    let (side_stage, flag) = match side {
        ConflictSide::Ours => (2, "--ours"),
        ConflictSide::Theirs => (3, "--theirs"),
    };
    let has_side = unmerged(repo, path)?
        .iter()
        .any(|entry| entry.stage == side_stage);
    if has_side {
        let out = literal_command(repo)
            .args(["checkout", flag, "--", path])
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "checkout")?;
        stage(repo, &[path.to_string()])
    } else {
        let out = literal_command(repo)
            .args(["rm", "-q", "--", path])
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "rm").map(drop)
    }
}

/// Removes `paths` from the index (the work tree is left alone).
pub(crate) fn unstage(repo: &Repo, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let has_head = repo
        .command()
        .args(["rev-parse", "-q", "--verify", "HEAD"])
        .output()
        .map_err(Error::Spawn)?
        .status
        .success();
    let mut command = literal_command(repo);
    if has_head {
        command.args(["restore", "--staged", "--"]);
    } else {
        // `git restore --staged` needs a HEAD to restore from; in an unborn
        // branch the index entry is simply removed.
        command.args(["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"]);
    }
    command.args(paths);
    let out = command.output().map_err(Error::Spawn)?;
    let name = if has_head {
        "restore --staged"
    } else {
        "rm --cached"
    };
    check_output(out, name).map(drop)
}

/// Throws away every local change: the index, tracked edits and untracked
/// files. Used when a checkout is told to start from a clean work tree.
pub(crate) fn discard_all(repo: &Repo) -> Result<()> {
    let has_head = repo
        .command()
        .args(["rev-parse", "-q", "--verify", "HEAD"])
        .output()
        .map_err(Error::Spawn)?
        .status
        .success();
    if has_head {
        let out = repo
            .command()
            .args(["reset", "--hard", "HEAD"])
            .output()
            .map_err(Error::Spawn)?;
        check_output(out, "reset --hard")?;
    }
    let out = repo
        .command()
        .args(["clean", "-fd", "-q"])
        .output()
        .map_err(Error::Spawn)?;
    check_output(out, "clean").map(drop)
}

/// Restores tracked files from the index and removes untracked ones.
pub(crate) fn discard(repo: &Repo, tracked: &[String], untracked: &[String]) -> Result<()> {
    if !tracked.is_empty() {
        let mut command = literal_command(repo);
        command.args(["restore", "--worktree", "--"]);
        command.args(tracked);
        let out = command.output().map_err(Error::Spawn)?;
        check_output(out, "restore --worktree")?;
    }
    if !untracked.is_empty() {
        let mut command = literal_command(repo);
        command.args(["clean", "-f", "-q", "--"]);
        command.args(untracked);
        let out = command.output().map_err(Error::Spawn)?;
        check_output(out, "clean")?;
    }
    Ok(())
}

/// Stages the picked hunks/lines of one unstaged file.
pub(crate) fn stage_selection(
    repo: &Repo,
    file: &WorkingFile,
    diff: &FileDiff,
    picks: &[HunkPick],
) -> Result<()> {
    let patch = build_patch(file, diff, picks).ok_or(Error::Parse("patch selection"))?;
    apply_patch(repo, &patch, true, false)
}

/// Unstages the picked hunks/lines of one staged file.
pub(crate) fn unstage_selection(
    repo: &Repo,
    file: &WorkingFile,
    diff: &FileDiff,
    picks: &[HunkPick],
) -> Result<()> {
    let patch = build_patch(file, diff, picks).ok_or(Error::Parse("patch selection"))?;
    apply_patch(repo, &patch, true, true)
}

/// Creates a commit from the index. Returns the new commit's id.
pub(crate) fn commit(repo: &Repo, message: &str, amend: bool) -> Result<Oid> {
    let mut command = repo.command();
    command.args(["commit", "-q", "-F", "-"]);
    if amend {
        command.arg("--amend");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(Error::Spawn)?;
    child
        .stdin
        .take()
        .ok_or(Error::Parse("commit stdin"))?
        .write_all(message.as_bytes())?;
    let out = child.wait_with_output().map_err(Error::Spawn)?;
    check_output(out, "commit")?;
    let out = repo
        .command()
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "rev-parse HEAD")?;
    Oid::from_hex(String::from_utf8_lossy(&out.stdout).trim())
        .ok_or(Error::Parse("commit object id"))
}

/// The work-tree side of one file. `old_path` is the index path of a rename
/// or copy; both sides of the rename must match the pathspec or git reports
/// the new name as a whole new file.
pub(crate) fn unstaged_diff(repo: &Repo, path: &str, old_path: Option<&str>) -> Result<FileDiff> {
    file_patch(repo, path, old_path, false, crate::DiffOptions::default())
}

/// A work-tree file read for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeFile {
    /// Contains a NUL byte.
    Binary,
    Text(WorktreeText),
}

/// The lines of a text work-tree file: `\n` and `\r\n` endings are stripped,
/// invalid UTF-8 is replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeText {
    pub lines: Vec<String>,
    /// The last line ended with a newline (true for an empty file).
    pub ends_with_newline: bool,
}

/// Reads the work-tree file `path` (repo-relative). A missing file is an
/// [`Error::Io`] of kind `NotFound`; one over the display cap is
/// [`Error::TooLarge`].
pub(crate) fn worktree_file(repo: &Repo, path: &str) -> Result<WorktreeFile> {
    let full = repo.root().join(path);
    if fs::metadata(&full).map_err(Error::Io)?.len() > UNTRACKED_COUNT_CAP {
        return Err(Error::TooLarge { what: "diff" });
    }
    let bytes = fs::read(&full).map_err(Error::Io)?;
    if bytes.contains(&0) {
        return Ok(WorktreeFile::Binary);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = Vec::new();
    let mut ends_with_newline = true;
    for line in text.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(content) => (content.strip_suffix('\r').unwrap_or(content), true),
            None => (line, false),
        };
        ends_with_newline = newline;
        lines.push(content.to_string());
    }
    Ok(WorktreeFile::Text(WorktreeText {
        lines,
        ends_with_newline,
    }))
}

/// A patch-shaped view of an untracked file: every line is an addition, so
/// the same diff model and the same hunk controls work on it.
pub(crate) fn untracked_diff(repo: &Repo, path: &str) -> Result<FileDiff> {
    let text = match worktree_file(repo, path)? {
        WorktreeFile::Binary => {
            return Ok(FileDiff {
                path: path.to_string(),
                binary: true,
                ..FileDiff::default()
            })
        }
        WorktreeFile::Text(text) => text,
    };
    let mut lines: Vec<crate::DiffLine> = text
        .lines
        .into_iter()
        .enumerate()
        .map(|(index, content)| crate::DiffLine {
            kind: crate::LineKind::Added,
            old_line: None,
            new_line: Some(index as u32 + 1),
            text: content,
            no_newline: false,
        })
        .collect();
    if let Some(last) = lines.last_mut() {
        last.no_newline = !text.ends_with_newline;
    }
    let count = lines.len() as u32;
    Ok(FileDiff {
        path: path.to_string(),
        old_path: None,
        binary: false,
        hunks: if count == 0 {
            Vec::new()
        } else {
            vec![crate::Hunk {
                header: format!("@@ -0,0 +1,{count} @@"),
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: count,
                lines,
            }]
        },
    })
}

/// The index side of one file.
pub(crate) fn staged_diff(repo: &Repo, path: &str, old_path: Option<&str>) -> Result<FileDiff> {
    staged_diff_with(repo, path, old_path, crate::DiffOptions::default())
}

pub(crate) fn staged_diff_with(
    repo: &Repo,
    path: &str,
    old_path: Option<&str>,
    options: crate::DiffOptions,
) -> Result<FileDiff> {
    file_patch(repo, path, old_path, true, options)
}

pub(crate) fn unstaged_diff_with(
    repo: &Repo,
    path: &str,
    old_path: Option<&str>,
    options: crate::DiffOptions,
) -> Result<FileDiff> {
    file_patch(repo, path, old_path, false, options)
}

fn file_patch(
    repo: &Repo,
    path: &str,
    old_path: Option<&str>,
    cached: bool,
    options: crate::DiffOptions,
) -> Result<FileDiff> {
    let scratch = if cached {
        None
    } else {
        ScratchIndex::copy_of(repo)
    };
    let mut command = literal_command(repo);
    // Read-only: see `Repo::read_command` and `ScratchIndex`.
    command.env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(scratch) = &scratch {
        scratch.apply(&mut command);
    }
    command.arg("diff");
    if cached {
        command.arg("--cached");
    }
    command.args(["-p", "--find-renames", "--no-color", "--no-ext-diff"]);
    command.args(options.args());
    command.arg("--");
    command.arg(path);
    if let Some(old_path) = old_path {
        command.arg(old_path);
    }
    let out = command.output().map_err(Error::Spawn)?;
    let out = check_output(out, "diff -p")?;
    crate::diff::parse_patch(&out.stdout)
}

/// One side's changed files, from `git diff`'s name-status and numstat.
fn side(repo: &Repo, cached: bool) -> Result<Vec<WorkingFile>> {
    let scratch = if cached {
        None
    } else {
        ScratchIndex::copy_of(repo)
    };
    let numstat = run_diff(repo, scratch.as_ref(), cached, "--numstat")?;
    let name_status = run_diff(repo, scratch.as_ref(), cached, "--name-status")?;
    let stats = parse_numstat(&numstat)?;
    let files = parse_name_status(&name_status)?;
    Ok(files
        .into_iter()
        .map(|file| {
            let stat = stats.get(&file.path).copied().unwrap_or_default();
            WorkingFile {
                kind: file.kind,
                old_path: file.old_path,
                path: file.path,
                added: stat.added,
                deleted: stat.deleted,
                binary: stat.binary,
                untracked: false,
            }
        })
        .collect())
}

fn run_diff(
    repo: &Repo,
    scratch: Option<&ScratchIndex>,
    cached: bool,
    mode: &'static str,
) -> Result<Vec<u8>> {
    let mut command = repo.read_command();
    if let Some(scratch) = scratch {
        scratch.apply(&mut command);
    }
    command.arg("diff");
    if cached {
        command.arg("--cached");
    }
    command.args([mode, "-z", "--find-renames", "--no-ext-diff"]);
    let out = command.output().map_err(Error::Spawn)?;
    Ok(check_output(out, "diff")?.stdout)
}

fn sort_files(files: &mut [WorkingFile]) {
    files.sort_by(|a, b| a.path.cmp(&b.path));
}

fn untracked_file(repo: &Repo, path: String) -> WorkingFile {
    let full = repo.root().join(&path);
    let (added, binary) = match fs::metadata(&full) {
        Ok(meta) if meta.len() > UNTRACKED_COUNT_CAP => (0, true),
        Ok(_) => match fs::read(&full) {
            Ok(bytes) if bytes.contains(&0) => (0, true),
            Ok(bytes) => (count_lines(&bytes), false),
            Err(_) => (0, false),
        },
        Err(_) => (0, false),
    };
    WorkingFile {
        kind: ChangeKind::Added,
        old_path: None,
        path,
        added,
        deleted: 0,
        binary,
        untracked: true,
    }
}

/// `git`'s line count: every `\n` ends a line, plus a last line without one.
fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&byte| byte == b'\n').count();
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

#[derive(Debug, Default)]
struct Porcelain {
    branch: Option<String>,
    untracked: Vec<String>,
    conflicted: Vec<String>,
}

/// Parses `git status --porcelain=v2 -z --branch` for the facts the two
/// diffs do not carry: the branch, untracked paths and conflicts.
fn parse_porcelain(raw: &[u8]) -> Result<Porcelain> {
    let mut out = Porcelain::default();
    let mut fields = raw.split(|&byte| byte == 0);
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        if let Some(head) = text.strip_prefix("# branch.head ") {
            let head = head.trim();
            if head != "(detached)" {
                out.branch = Some(head.to_string());
            }
        } else if let Some(path) = text.strip_prefix("? ") {
            out.untracked.push(path.to_string());
        } else if let Some(rest) = text.strip_prefix("u ") {
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path`: the path is last and may
            // contain spaces.
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                out.conflicted.push(path.to_string());
            }
        } else if text.starts_with("2 ") {
            // A rename/copy record is followed by the original path.
            fields.next();
        }
    }
    Ok(out)
}

/// Repo-relative paths with unresolved conflicts, in git's order.
pub(crate) fn conflicted_paths(repo: &Repo) -> Result<Vec<String>> {
    Ok(porcelain(repo)?.conflicted)
}

/// Reads the status porcelain and returns only its parsed facts.
fn porcelain(repo: &Repo) -> Result<Porcelain> {
    let out = repo
        .read_command()
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--branch",
        ])
        .output()
        .map_err(Error::Spawn)?;
    let out = check_output(out, "status")?;
    parse_porcelain(&out.stdout)
}

/// Builds the unified patch for the picked hunks and lines.
///
/// Unselected changed lines are dropped: an unselected addition disappears, an
/// unselected removal turns into a context line (the old content stays). A
/// hunk whose changes all disappeared is left out entirely.
fn build_patch(file: &WorkingFile, diff: &FileDiff, picks: &[HunkPick]) -> Option<String> {
    let mut body = String::new();
    for pick in picks {
        let hunk = diff.hunks.get(pick.hunk)?;
        let mut lines = String::new();
        let mut old_count = 0u32;
        let mut new_count = 0u32;
        let mut changed = false;
        for (index, line) in hunk.lines.iter().enumerate() {
            let picked = pick.picks_line(index);
            match line.kind {
                crate::LineKind::Context => {
                    lines.push(' ');
                    lines.push_str(&line.text);
                    lines.push('\n');
                    old_count += 1;
                    new_count += 1;
                    if line.no_newline {
                        lines.push_str("\\ No newline at end of file\n");
                    }
                }
                crate::LineKind::Added if picked => {
                    lines.push('+');
                    lines.push_str(&line.text);
                    lines.push('\n');
                    new_count += 1;
                    changed = true;
                    if line.no_newline {
                        lines.push_str("\\ No newline at end of file\n");
                    }
                }
                crate::LineKind::Added => {}
                crate::LineKind::Removed if picked => {
                    lines.push('-');
                    lines.push_str(&line.text);
                    lines.push('\n');
                    old_count += 1;
                    changed = true;
                    if line.no_newline {
                        lines.push_str("\\ No newline at end of file\n");
                    }
                }
                crate::LineKind::Removed => {
                    lines.push(' ');
                    lines.push_str(&line.text);
                    lines.push('\n');
                    old_count += 1;
                    new_count += 1;
                    if line.no_newline {
                        lines.push_str("\\ No newline at end of file\n");
                    }
                }
            }
        }
        if !changed {
            continue;
        }
        body.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, old_count, hunk.new_start, new_count
        ));
        body.push_str(&lines);
    }
    if body.is_empty() {
        return None;
    }

    let new_path = &file.path;
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let mut patch = format!(
        "diff --git {} {}\n",
        quote_path("a/", old_path),
        quote_path("b/", new_path)
    );
    match file.kind {
        ChangeKind::Added => patch.push_str("new file mode 100644\n"),
        ChangeKind::Deleted => patch.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    let old_name = match file.kind {
        ChangeKind::Added => "/dev/null".to_string(),
        _ => quote_path("a/", old_path),
    };
    let new_name = match file.kind {
        ChangeKind::Deleted => "/dev/null".to_string(),
        _ => quote_path("b/", new_path),
    };
    patch.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    patch.push_str(&body);
    Some(patch)
}

/// A patch-header path: `prefix` plus `path`, quoted the way git quotes a
/// path only when it contains a quote, a backslash or a control character.
/// Spaces are literal in git's patch headers, so they stay unquoted.
fn quote_path(prefix: &str, path: &str) -> String {
    let full = format!("{prefix}{path}");
    let needs_quotes = full
        .chars()
        .any(|ch| ch == '"' || ch == '\\' || ch.is_control());
    if !needs_quotes {
        return full;
    }
    let mut quoted = String::with_capacity(full.len() + 2);
    quoted.push('"');
    for ch in full.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Runs `git apply --cached` with the patch on stdin.
fn apply_patch(repo: &Repo, patch: &str, cached: bool, reverse: bool) -> Result<()> {
    let mut command = repo.command();
    command.args(["apply", "--recount", "--whitespace=nowarn"]);
    if cached {
        command.arg("--cached");
    }
    if reverse {
        command.arg("--reverse");
    }
    command.arg("-");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(Error::Spawn)?;
    child
        .stdin
        .take()
        .ok_or(Error::Parse("apply stdin"))?
        .write_all(patch.as_bytes())?;
    let out = child.wait_with_output().map_err(Error::Spawn)?;
    check_output(out, "apply").map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffLine, Hunk, LineKind};

    #[test]
    fn stale_scratch_indexes_are_removed_and_fresh_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_000_000;
        let names = [
            ("index.gitwarp-1-0-100", true),
            ("index.gitwarp-1-0-100.lock", true),
            (&*format!("index.gitwarp-2-0-{}", now - 10), false),
            (&*format!("index.gitwarp-2-1-{}.lock", now - 10), false),
            // Not ours, or unparsable: never touched.
            ("index", false),
            ("index.lock", false),
            ("index.gitwarp-x", false),
        ];
        for (name, _) in names {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        assert_eq!(remove_stale_scratch_indexes(dir.path(), now), 2);
        for (name, removed) in names {
            assert_eq!(dir.path().join(name).exists(), !removed, "{name}");
        }
    }

    #[test]
    fn dropping_the_guard_removes_a_partly_written_copy() {
        let dir = tempfile::tempdir().unwrap();
        let guard = ScratchIndex(dir.path().join("probe"));
        fs::write(&guard.0, "partial").unwrap();
        drop(guard);
        assert!(!dir.path().join("probe").exists());
    }

    fn file(kind: ChangeKind, path: &str) -> WorkingFile {
        WorkingFile {
            kind,
            old_path: None,
            path: path.to_string(),
            added: 0,
            deleted: 0,
            binary: false,
            untracked: false,
        }
    }

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_line: None,
            new_line: None,
            text: text.to_string(),
            no_newline: false,
        }
    }

    fn diff(lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            old_path: None,
            path: "src/a.rs".into(),
            binary: false,
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,4 @@".into(),
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 4,
                lines,
            }],
        }
    }

    #[test]
    fn a_partial_hunk_keeps_unselected_removals_as_context() {
        let diff = diff(vec![
            line(LineKind::Context, "one"),
            line(LineKind::Removed, "two"),
            line(LineKind::Added, "TWO"),
            line(LineKind::Added, "extra"),
        ]);
        // Pick only the second added line: the removed line becomes context
        // and the first (unselected) addition disappears.
        let patch = build_patch(
            &file(ChangeKind::Modified, "src/a.rs"),
            &diff,
            &[HunkPick::lines(0, vec![3])],
        )
        .unwrap();
        assert!(patch.contains("@@ -1,2 +1,3 @@\n"));
        assert!(patch.contains(" one\n two\n+extra\n"));
        assert!(!patch.contains("-two"));
        assert!(!patch.contains("+TWO"));
    }

    #[test]
    fn a_hunk_with_no_picked_change_is_left_out() {
        let diff = diff(vec![
            line(LineKind::Context, "one"),
            line(LineKind::Removed, "two"),
            line(LineKind::Added, "TWO"),
        ]);
        assert!(build_patch(
            &file(ChangeKind::Modified, "src/a.rs"),
            &diff,
            &[HunkPick::lines(0, vec![])]
        )
        .is_none());
    }

    #[test]
    fn an_added_file_patch_starts_from_dev_null() {
        let diff = diff(vec![line(LineKind::Added, "hello")]);
        let patch = build_patch(
            &file(ChangeKind::Added, "new.txt"),
            &diff,
            &[HunkPick::whole(0)],
        )
        .unwrap();
        assert!(patch.starts_with(
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n"
        ));
        assert!(patch.contains("@@ -1,0 +1,1 @@\n+hello\n"));
    }

    #[test]
    fn a_deleted_file_patch_ends_at_dev_null() {
        let diff = diff(vec![line(LineKind::Removed, "gone")]);
        let patch = build_patch(
            &file(ChangeKind::Deleted, "old.txt"),
            &diff,
            &[HunkPick::whole(0)],
        )
        .unwrap();
        assert!(patch.contains("deleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n"));
    }

    #[test]
    fn counts_lines_without_a_trailing_newline() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
    }

    #[test]
    fn parses_untracked_conflicted_and_renamed_status_records() {
        let raw = b"# branch.head main\0# branch.oid abc\0? new file.txt\0u UU N... 100644 100644 100644 100644 aaa bbb ccc src/conflict.rs\x002 R. N... 100644 100644 100644 aaa bbb R100 new.txt\0old.txt\0";
        let parsed = parse_porcelain(raw).unwrap();
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert_eq!(parsed.untracked, ["new file.txt"]);
        assert_eq!(parsed.conflicted, ["src/conflict.rs"]);
    }

    #[test]
    fn the_cheap_status_lists_each_changed_path_once() {
        let raw = b" M src/a.rs\0?? new file.txt\0R  renamed.rs\0old.rs\0";
        let paths = parse_changed_paths(raw);
        assert_eq!(paths, ["src/a.rs", "new file.txt", "renamed.rs"]);
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let parsed = parse_porcelain(b"# branch.head (detached)\0").unwrap();
        assert_eq!(parsed.branch, None);
    }

    #[test]
    fn head_label_joins_summary_and_short_id() {
        let mut status = WorkingStatus::default();
        assert_eq!(status.head_label(), None);
        status.head = Some(HeadCommit {
            id: Oid::from_hex(&"a".repeat(40)).unwrap(),
            short: "aaaaaaa".into(),
            summary: "feat: one".into(),
            message: "feat: one".into(),
        });
        assert_eq!(status.head_label().as_deref(), Some("feat: one · aaaaaaa"));
    }

    #[test]
    fn a_head_commit_shortens_its_id() {
        let head = HeadCommit {
            id: Oid::from_hex(&"b".repeat(40)).unwrap(),
            short: "bbbbbbb".into(),
            summary: String::new(),
            message: String::new(),
        };
        assert_eq!(head.short_oid(), "bbbbbbb");
    }

    #[test]
    fn a_used_path_is_quoted_in_the_patch_header() {
        assert_eq!(quote_path("a/", "plain.rs"), "a/plain.rs");
        assert_eq!(quote_path("a/", "a b.rs"), "a/a b.rs");
        assert_eq!(quote_path("a/", "a\"b.rs"), "\"a/a\\\"b.rs\"");
    }

    #[test]
    fn files_are_sorted_by_path() {
        let mut files = [
            file(ChangeKind::Modified, "z.rs"),
            file(ChangeKind::Modified, "a.rs"),
        ];
        sort_files(&mut files);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "z.rs");
    }
}
