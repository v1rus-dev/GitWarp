//! The repository file watcher, without GPUI: where a repository keeps its git data
//! ([`GitLayout`]), which kind of change a touched path stands for ([`classify`]), which folders
//! have to be watched ([`watch_plan`]) and the running watcher itself ([`RepoWatcher`]). The
//! repository view owns one and turns its [`WatchSignal`]s into reloads.

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::ops::{BitOr, BitOrAssign};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_debouncer_full::notify::{Config, ErrorKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, Debouncer, NoCache};

/// A set of change kinds, a hand-written bitmask (nine kinds, so `u16`).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Kinds(u16);

impl Kinds {
    pub const NONE: Kinds = Kinds(0);
    /// The checked-out `HEAD` of this work tree.
    pub const HEAD: Kinds = Kinds(1);
    /// Any branch, tag or remote ref, or `packed-refs`.
    pub const REFS: Kinds = Kinds(1 << 1);
    /// The index (staging area).
    pub const INDEX: Kinds = Kinds(1 << 2);
    /// A merge / rebase / cherry-pick / revert / bisect state file appeared, changed or went.
    pub const OPERATION: Kinds = Kinds(1 << 3);
    /// The stash (its ref or its reflog).
    pub const STASH: Kinds = Kinds(1 << 4);
    /// The repository config (remotes, upstreams).
    pub const CONFIG: Kinds = Kinds(1 << 5);
    /// A linked worktree was added, removed, locked or moved its HEAD.
    pub const WORKTREES: Kinds = Kinds(1 << 6);
    /// A file of the work tree (or of a submodule) changed.
    pub const WORKTREE_FILES: Kinds = Kinds(1 << 7);
    /// The backend lost events (queue overflow): everything may have changed.
    pub const RESCAN: Kinds = Kinds(1 << 8);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every bit of `other` is set.
    pub fn contains(self, other: Kinds) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Kinds) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for Kinds {
    type Output = Kinds;
    fn bitor(self, rhs: Kinds) -> Kinds {
        Kinds(self.0 | rhs.0)
    }
}

impl BitOrAssign for Kinds {
    fn bitor_assign(&mut self, rhs: Kinds) {
        self.0 |= rhs.0;
    }
}

/// Where one repository keeps its data, in raw and canonical form.
///
/// For a normal repository `git_dir == common_dir`. For a linked worktree `git_dir` is
/// `<main>/.git/worktrees/<name>` (HEAD, index) and `common_dir` is `<main>/.git` (refs,
/// config). A bare repository has no `root`.
#[derive(Clone, Debug)]
pub struct GitLayout {
    root: Option<Roots>,
    git_dir: Roots,
    common_dir: Roots,
    /// Names of top-level work-tree folders that git ignores (`target`, `node_modules`);
    /// paths below them classify as [`Kinds::NONE`].
    ignored_dirs: Vec<OsString>,
}

/// One folder as given and as canonicalized. Events carry either form depending on the
/// platform (macOS reports `/private/var/...` for `/var/...`), so both are matched.
#[derive(Clone, Debug)]
struct Roots {
    raw: PathBuf,
    canonical: PathBuf,
}

impl Roots {
    fn new(raw: &Path) -> Self {
        // Raw paths can carry `\\?\` too (the CLI engine reports them that way on Windows), and
        // events never do.
        let raw = strip_verbatim(raw);
        // A folder that cannot be canonicalized (gone, no access) still matches by its raw form.
        let canonical = std::fs::canonicalize(&raw)
            .map(|p| strip_verbatim(&p))
            .unwrap_or_else(|_| raw.clone());
        Roots { raw, canonical }
    }

    /// The part of `path` below this folder (raw form first, then canonical), if any.
    fn relative<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.raw)
            .ok()
            .or_else(|| path.strip_prefix(&self.canonical).ok())
    }
}

impl GitLayout {
    /// `root` is the work tree (`None` for a bare repository).
    pub fn new(root: Option<&Path>, git_dir: &Path, common_dir: &Path) -> Self {
        GitLayout {
            root: root.map(Roots::new),
            git_dir: Roots::new(git_dir),
            common_dir: Roots::new(common_dir),
            ignored_dirs: Vec::new(),
        }
    }

    /// Paths below these top-level work-tree folders are not changes anybody can see in
    /// `git status`, so [`classify`] drops them (see [`ignored_top_level_dirs`]).
    pub fn with_ignored_dirs(mut self, dirs: Vec<OsString>) -> Self {
        self.ignored_dirs = dirs;
        self
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_ref().map(|r| r.raw.as_path())
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir.raw
    }

    pub fn common_dir(&self) -> &Path {
        &self.common_dir.raw
    }

    /// Whether the git dir and the common dir differ. Compared canonically: the two come from
    /// different code paths (git-core's `git_dir()` and `common_dir()`) and can spell the same
    /// folder differently (`\\?\` prefix, symlinks, `..`), which must not look like a linked
    /// worktree (its refs events would then be dropped).
    fn is_linked(&self) -> bool {
        self.git_dir.canonical != self.common_dir.canonical
    }
}

/// `std::fs::canonicalize` returns `\\?\C:\x` (or `\\?\UNC\srv\x`) on Windows while events
/// carry the plain form, so the verbatim prefix is dropped before comparing. A no-op for
/// every other path.
fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Which kinds of change touching `path` stands for; [`Kinds::NONE`] for paths nobody
/// reads (objects, hooks, lock files, `logs/HEAD`, ...).
pub fn classify(layout: &GitLayout, path: &Path) -> Kinds {
    let path = strip_verbatim(path);
    let path = path.as_path();

    // The linked worktree's own git dir sits inside the common dir, so it is tried first.
    if let Some(rel) = layout.git_dir.relative(path).filter(|r| has_components(r)) {
        let mut kinds = classify_git_dir(rel);
        if !layout.is_linked() {
            kinds |= classify_common_dir(rel);
        }
        return kinds;
    }
    if layout.is_linked() {
        if let Some(rel) = layout
            .common_dir
            .relative(path)
            .filter(|r| has_components(r))
        {
            return classify_common_dir(rel);
        }
    }
    if layout.git_dir.relative(path).is_some() || layout.common_dir.relative(path).is_some() {
        // The git dir itself (e.g. its mode changed): nothing anybody reads.
        return Kinds::NONE;
    }
    if let Some(root) = &layout.root {
        // Everything else below the work tree, a nested `.git` of a submodule included.
        if let Some(rel) = root.relative(path) {
            let top = rel.components().next();
            if let Some(Component::Normal(top)) = top {
                if layout.ignored_dirs.iter().any(|d| d == top) {
                    return Kinds::NONE;
                }
            }
            return Kinds::WORKTREE_FILES;
        }
    }
    Kinds::NONE
}

fn has_components(rel: &Path) -> bool {
    rel.components().next().is_some()
}

fn names(rel: &Path) -> Vec<&OsStr> {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// A git lock file (`index.lock`, `HEAD.lock`, `refs/heads/x.lock`, `packed-refs.lock`).
/// It is only the first half of an atomic update: the final name shows up as its own event
/// or as the second path of a rename event, and that one is what counts.
fn is_lock(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| n.ends_with(".lock"))
}

/// Entries of the per-worktree git dir whose presence says an operation (merge, rebase,
/// cherry-pick, revert, bisect, squash) is in progress. The view's fingerprint uses the same
/// list, one bit per entry.
pub const OPERATION_MARKERS: [&str; 10] = [
    "MERGE_HEAD",
    "SQUASH_MSG",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "BISECT_LOG",
    "rebase-merge",
    "rebase-apply",
    "sequencer",
    "MERGE_MSG",
];
/// The markers that are folders: anything below them counts as well.
const OPERATION_DIRS: &[&str] = &["rebase-merge", "rebase-apply", "sequencer"];

/// `rel` is relative to the per-worktree git dir.
fn classify_git_dir(rel: &Path) -> Kinds {
    let c = names(rel);
    if c.iter().any(|n| is_lock(n)) {
        return Kinds::NONE;
    }
    let Some(first) = c.first().and_then(|n| n.to_str()) else {
        return Kinds::NONE;
    };
    match (first, c.len()) {
        ("HEAD", 1) => Kinds::HEAD,
        ("index", 1) => Kinds::INDEX,
        (name, 1) if OPERATION_MARKERS.contains(&name) => Kinds::OPERATION,
        (name, _) if OPERATION_DIRS.contains(&name) => Kinds::OPERATION,
        _ => Kinds::NONE,
    }
}

/// `rel` is relative to the common git dir.
fn classify_common_dir(rel: &Path) -> Kinds {
    let c = names(rel);
    if c.iter().any(|n| is_lock(n)) {
        return Kinds::NONE;
    }
    let Some(first) = c.first().and_then(|n| n.to_str()) else {
        return Kinds::NONE;
    };
    match first {
        "refs" if c.len() == 2 && c[1] == "stash" => Kinds::STASH | Kinds::REFS,
        "refs" | "packed-refs" => Kinds::REFS,
        // The reftable ref backend keeps every ref (and HEAD) in `reftable/*`; `tables.list`
        // is swapped on every ref update. (`.lock` names were dropped above.)
        "reftable" => Kinds::REFS,
        // `git stash list` reads the reflog, and dropping a stash entry other than the top
        // one changes only the reflog. Every other reflog is noise.
        "logs" if c.len() == 3 && c[1] == "refs" && c[2] == "stash" => Kinds::STASH,
        "config" if c.len() == 1 => Kinds::CONFIG,
        // `worktrees/<name>` added or removed, or its HEAD / lock marker changed.
        "worktrees" if c.len() <= 2 => Kinds::WORKTREES,
        "worktrees" if c[2] == "HEAD" || c[2] == "locked" => Kinds::WORKTREES,
        // A submodule's git dir: its commit shows up in the parent's status. Its object
        // store is noise.
        "modules" if c.contains(&OsStr::new("objects")) => Kinds::NONE,
        "modules" => Kinds::WORKTREE_FILES,
        _ => Kinds::NONE,
    }
}

/// How much of the repository is watched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Git folders and the work tree.
    Normal,
    /// Git folders only. Used after the OS watch limit was hit; the window-activation check
    /// then covers the work tree.
    Reduced,
}

/// The folders to watch for `layout`. `pruned_dirs` are names of top-level work-tree folders
/// that git ignores (`target`, `node_modules`, ...); they are skipped where a recursive watch
/// costs a watch per directory (Linux). Folders that do not exist are left out.
pub fn watch_plan(
    layout: &GitLayout,
    mode: Mode,
    pruned_dirs: &[OsString],
) -> Vec<(PathBuf, RecursiveMode)> {
    watch_plan_impl(layout, mode, pruned_dirs, cfg!(target_os = "linux"))
}

/// `per_dir` is the Linux strategy: inotify watches directories one by one, so the work tree
/// gets a non-recursive watch on its root plus a recursive one per top-level folder that is
/// not pruned. Elsewhere a single recursive watch on the root covers everything cheaply.
fn watch_plan_impl(
    layout: &GitLayout,
    mode: Mode,
    pruned_dirs: &[OsString],
    per_dir: bool,
) -> Vec<(PathBuf, RecursiveMode)> {
    let mut plan: Vec<(PathBuf, RecursiveMode)> = Vec::new();
    let mut add = |path: PathBuf, recursive: RecursiveMode| {
        // Identical folders can be spelled differently (raw vs `\\?\`, symlinks): one watch.
        if path.exists() && !plan.iter().any(|(p, _)| same_folder(p, &path)) {
            plan.push((path, recursive));
        }
    };

    let gd = layout.git_dir().to_path_buf();
    let cd = layout.common_dir().to_path_buf();
    add(gd, RecursiveMode::NonRecursive);
    if layout.is_linked() {
        add(cd.clone(), RecursiveMode::NonRecursive);
    }
    add(cd.join("refs"), RecursiveMode::Recursive);
    add(cd.join("logs").join("refs"), RecursiveMode::NonRecursive);
    add(cd.join("worktrees"), RecursiveMode::Recursive);
    // Submodule git dirs: a commit made inside a submodule moves its HEAD/refs here.
    add(cd.join("modules"), RecursiveMode::Recursive);
    // Only the reftable ref backend has this folder; it then holds every ref.
    add(cd.join("reftable"), RecursiveMode::NonRecursive);

    if mode == Mode::Normal {
        // A bare repository has no root and so no work-tree entries.
        if let Some(root) = layout.root() {
            if per_dir {
                add(root.to_path_buf(), RecursiveMode::NonRecursive);
                for dir in top_level_dirs(root, pruned_dirs) {
                    add(dir, RecursiveMode::Recursive);
                }
            } else {
                add(root.to_path_buf(), RecursiveMode::Recursive);
            }
        }
    }
    plan
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || Roots::new(a).canonical == Roots::new(b).canonical
}

/// Direct child folders of `root` other than `.git` and the pruned ones, sorted. Symlinks
/// are not followed.
fn top_level_dirs(root: &Path, pruned_dirs: &[OsString]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            name != ".git" && !pruned_dirs.contains(&name)
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// How long the debouncer holds an event back before reporting it.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// What the watcher reports: the merged kinds of one debounced batch, and whether the OS
/// watch limit was hit while the watcher ran (the owner should rebuild it in [`Mode::Reduced`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WatchSignal {
    pub kinds: Kinds,
    pub limit_hit: bool,
}

/// Why a watcher could not be started.
#[derive(Debug)]
pub enum StartError {
    /// The OS watch limit (inotify `ENOSPC`) was hit; start again in [`Mode::Reduced`].
    Limit,
    /// Anything else (no backend, no folder could be watched).
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Limit => f.write_str("OS watch limit reached"),
            StartError::Other(reason) => f.write_str(reason),
        }
    }
}

/// A running watcher of one repository. Dropping it stops the watcher without blocking.
pub struct RepoWatcher {
    // Held only for its Drop: it stops the debouncer thread and removes the OS watches. Shared
    // with the thread that adds watches for new top-level folders (Linux); that thread only
    // holds a `Weak`, so dropping the watcher still drops the debouncer.
    _debouncer: Arc<Mutex<Debouncer<RecommendedWatcher, NoCache>>>,
    mode: Mode,
}

type Sink = Arc<Mutex<Box<dyn FnMut(WatchSignal) + Send>>>;

impl RepoWatcher {
    /// Starts watching `layout` in [`Mode::Normal`], asking the `git` on `PATH` which top-level
    /// folders to skip. Blocks (the recursive watch walks the tree), so call it only from a
    /// background thread. The view uses [`RepoWatcher::start_in`] with the app's git.
    #[cfg(test)]
    pub fn start(
        layout: &GitLayout,
        sink: impl FnMut(WatchSignal) + Send + 'static,
    ) -> Result<RepoWatcher, StartError> {
        Self::start_in(layout, Mode::Normal, Path::new("git"), sink)
    }

    /// Like [`RepoWatcher::start`] with an explicit mode and the git executable used for the
    /// ignore check (the app's resolved git).
    pub fn start_in(
        layout: &GitLayout,
        mode: Mode,
        git_exe: &Path,
        sink: impl FnMut(WatchSignal) + Send + 'static,
    ) -> Result<RepoWatcher, StartError> {
        // On every platform: ignored top-level folders (`target/`, `node_modules/`) are dropped
        // from classification, so their churn does not run `git status` every second. On Linux
        // they are also not watched at all (a watch per directory). Ignored folders nested
        // deeper are left to the view's 1 s throttle and its equal-status drop.
        let pruned = match (mode, layout.root()) {
            (Mode::Normal, Some(root)) => ignored_top_level_dirs(git_exe, root),
            _ => Vec::new(),
        };
        Self::start_with(layout, mode, git_exe, &pruned, sink)
    }

    /// The core: watch the plan for (`layout`, `mode`, `pruned_dirs`).
    pub fn start_with(
        layout: &GitLayout,
        mode: Mode,
        git_exe: &Path,
        pruned_dirs: &[OsString],
        sink: impl FnMut(WatchSignal) + Send + 'static,
    ) -> Result<RepoWatcher, StartError> {
        let layout = layout.clone().with_ignored_dirs(pruned_dirs.to_vec());
        let sink: Sink = Arc::new(Mutex::new(Box::new(sink)));
        // New top-level folders (Linux only) go to a thread that adds their recursive watch:
        // `watch()` walks the tree, which must not happen on the debouncer's thread or the UI's.
        let (new_dirs_tx, new_dirs_rx) = std::sync::mpsc::channel::<PathBuf>();
        let watch_new_dirs = cfg!(target_os = "linux") && mode == Mode::Normal;
        let handler_layout = layout.clone();
        let handler_sink = Arc::clone(&sink);
        // NoCache: the default file-id cache walks the whole tree on macOS/Windows while holding
        // the debouncer's lock, and rename stitching is not needed here.
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            DEBOUNCE,
            None,
            move |result: DebounceEventResult| {
                if watch_new_dirs {
                    if let Ok(events) = &result {
                        for dir in new_top_level_dirs(&handler_layout, events) {
                            let _ = new_dirs_tx.send(dir);
                        }
                    }
                }
                if let Some(signal) = signal_of(&handler_layout, result) {
                    if let Ok(mut sink) = handler_sink.lock() {
                        sink(signal);
                    }
                }
            },
            NoCache,
            Config::default(),
        )
        .map_err(|e| match e.kind {
            ErrorKind::MaxFilesWatch => StartError::Limit,
            _ => StartError::Other(e.to_string()),
        })?;

        let plan = watch_plan(&layout, mode, pruned_dirs);
        let mut watched = 0usize;
        for (path, recursive) in &plan {
            match debouncer.watch(path, *recursive) {
                Ok(()) => watched += 1,
                // Watches added before the failure stay behind, so the whole debouncer is
                // dropped (on return) and the caller rebuilds with fewer paths.
                Err(e) if matches!(e.kind, ErrorKind::MaxFilesWatch) => {
                    return Err(StartError::Limit);
                }
                Err(e) => log::warn!("cannot watch {}: {e}", path.display()),
            }
        }
        if watched == 0 {
            return Err(StartError::Other("no folder could be watched".into()));
        }
        let debouncer = Arc::new(Mutex::new(debouncer));
        if watch_new_dirs {
            let weak = Arc::downgrade(&debouncer);
            let git_exe = git_exe.to_path_buf();
            let root = layout.root().map(Path::to_path_buf);
            std::thread::spawn(move || {
                // Ends when the debouncer (and with it the sending side) is dropped.
                while let Ok(dir) = new_dirs_rx.recv() {
                    let (Some(debouncer), Some(root)) = (weak.upgrade(), root.as_deref()) else {
                        break;
                    };
                    watch_new_dir(&debouncer, &sink, &git_exe, root, &dir);
                }
            });
        }
        Ok(RepoWatcher {
            _debouncer: debouncer,
            mode,
        })
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }
}

/// Direct child folders of the work tree that an event says were created or renamed in and
/// that exist now. Only these need a watch of their own; the root watch is not recursive on
/// Linux.
fn new_top_level_dirs(
    layout: &GitLayout,
    events: &[notify_debouncer_full::DebouncedEvent],
) -> Vec<PathBuf> {
    use notify_debouncer_full::notify::{event::ModifyKind, EventKind};
    let Some(root) = &layout.root else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for event in events {
        if !(event.kind.is_create() || matches!(event.kind, EventKind::Modify(ModifyKind::Name(_))))
        {
            continue;
        }
        for path in &event.paths {
            let path = strip_verbatim(path);
            let Some(rel) = root.relative(&path) else {
                continue;
            };
            let mut parts = rel.components();
            let (Some(Component::Normal(name)), None) = (parts.next(), parts.next()) else {
                continue;
            };
            if name == ".git" || layout.ignored_dirs.iter().any(|d| d == name) {
                continue;
            }
            // Not `is_dir()`: a symlinked folder is not followed by the initial plan either.
            let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
            if is_dir && !dirs.contains(&path) {
                dirs.push(path);
            }
        }
    }
    dirs
}

/// Adds the recursive watch for a top-level folder that appeared after the watcher started
/// (unless git ignores it), then reports the work tree as changed: files created in it
/// between its creation and the watch were never seen.
fn watch_new_dir(
    debouncer: &Mutex<Debouncer<RecommendedWatcher, NoCache>>,
    sink: &Sink,
    git_exe: &Path,
    root: &Path,
    dir: &Path,
) {
    let Some(name) = dir.file_name() else { return };
    if !dir.is_dir() || !ignored_among(git_exe, root, &[name.to_os_string()]).is_empty() {
        return;
    }
    let result = match debouncer.lock() {
        Ok(mut debouncer) => debouncer.watch(dir, RecursiveMode::Recursive),
        Err(_) => return,
    };
    match result {
        Ok(()) => {
            if let Ok(mut sink) = sink.lock() {
                sink(WatchSignal {
                    kinds: Kinds::WORKTREE_FILES,
                    limit_hit: false,
                });
            }
        }
        // Out of watches: the owner rebuilds in reduced mode (the focus check covers the rest).
        Err(e) if matches!(e.kind, ErrorKind::MaxFilesWatch) => {
            if let Ok(mut sink) = sink.lock() {
                sink(WatchSignal {
                    kinds: Kinds::NONE,
                    limit_hit: true,
                });
            }
        }
        Err(e) => log::warn!("cannot watch new folder {}: {e}", dir.display()),
    }
}

/// Turns one debouncer delivery into a signal; `None` when nothing relevant happened.
fn signal_of(layout: &GitLayout, result: DebounceEventResult) -> Option<WatchSignal> {
    let mut signal = WatchSignal::default();
    match result {
        Ok(events) => {
            for event in &events {
                if event.need_rescan() {
                    signal.kinds |= Kinds::RESCAN;
                }
                // Every path counts: a `x.lock -> x` rename carries both, and the final
                // name is the one that matters.
                for path in &event.paths {
                    signal.kinds |= classify(layout, path);
                }
            }
        }
        Err(errors) => {
            for error in errors {
                if matches!(error.kind, ErrorKind::MaxFilesWatch) {
                    signal.limit_hit = true;
                } else {
                    log::warn!("file watcher error: {error}");
                }
            }
        }
    }
    (!signal.kinds.is_empty() || signal.limit_hit).then_some(signal)
}

/// Names of the top-level folders of `root` that git ignores (`target`, `node_modules`, ...).
/// Empty when git cannot be run: nothing is skipped then, which is only slower.
fn ignored_top_level_dirs(git_exe: &Path, root: &Path) -> Vec<OsString> {
    let dirs: Vec<OsString> = top_level_dirs(root, &[])
        .iter()
        .filter_map(|d| d.file_name().map(OsStr::to_os_string))
        .collect();
    ignored_among(git_exe, root, &dirs)
}

/// The folders of `dirs` (names directly under `root`) that git ignores.
fn ignored_among(git_exe: &Path, root: &Path, dirs: &[OsString]) -> Vec<OsString> {
    if dirs.is_empty() {
        return Vec::new();
    }
    // NUL-separated both ways so odd folder names survive; the trailing slash makes
    // directory-only patterns (`target/`) match.
    let mut input = Vec::new();
    for dir in dirs {
        let Some(name) = dir.to_str() else { continue };
        input.extend_from_slice(name.as_bytes());
        input.extend_from_slice(b"/\0");
    }
    let mut cmd = Command::new(git_exe);
    cmd.arg("-C")
        .arg(root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else {
        log::warn!("git check-ignore could not be started; watching every top-level folder");
        return Vec::new();
    };
    // Write from a thread so a full stdout pipe cannot deadlock against our stdin write.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output();
    let _ = writer.join();
    // Exit status 1 only means "nothing ignored"; the output is what counts.
    let Ok(out) = out else { return Vec::new() };
    out.stdout
        .split(|b| *b == 0)
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .map(OsString::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> GitLayout {
        GitLayout::new(
            Some(Path::new("/r")),
            Path::new("/r/.git"),
            Path::new("/r/.git"),
        )
    }

    /// Main repo `/m`, linked worktree `/w`.
    fn linked() -> GitLayout {
        GitLayout::new(
            Some(Path::new("/w")),
            Path::new("/m/.git/worktrees/w"),
            Path::new("/m/.git"),
        )
    }

    fn bare() -> GitLayout {
        GitLayout::new(None, Path::new("/b.git"), Path::new("/b.git"))
    }

    fn c(l: &GitLayout, p: &str) -> Kinds {
        classify(l, Path::new(p))
    }

    #[test]
    fn normal_repo_table() {
        let l = layout();
        let cases: &[(&str, Kinds)] = &[
            ("/r/.git/HEAD", Kinds::HEAD),
            ("/r/.git/refs/heads/main", Kinds::REFS),
            ("/r/.git/refs/remotes/origin/x", Kinds::REFS),
            ("/r/.git/refs/tags/v1", Kinds::REFS),
            ("/r/.git/refs/stash", Kinds::STASH | Kinds::REFS),
            ("/r/.git/packed-refs", Kinds::REFS),
            ("/r/.git/index", Kinds::INDEX),
            ("/r/.git/MERGE_HEAD", Kinds::OPERATION),
            ("/r/.git/MERGE_MSG", Kinds::OPERATION),
            ("/r/.git/SQUASH_MSG", Kinds::OPERATION),
            ("/r/.git/CHERRY_PICK_HEAD", Kinds::OPERATION),
            ("/r/.git/REVERT_HEAD", Kinds::OPERATION),
            ("/r/.git/REBASE_HEAD", Kinds::OPERATION),
            ("/r/.git/BISECT_LOG", Kinds::OPERATION),
            ("/r/.git/rebase-merge", Kinds::OPERATION),
            ("/r/.git/rebase-merge/done", Kinds::OPERATION),
            ("/r/.git/rebase-apply/patch", Kinds::OPERATION),
            ("/r/.git/sequencer/todo", Kinds::OPERATION),
            ("/r/.git/logs/refs/stash", Kinds::STASH),
            ("/r/.git/logs/HEAD", Kinds::NONE),
            ("/r/.git/logs/refs/heads/main", Kinds::NONE),
            ("/r/.git/objects/ab/cdef", Kinds::NONE),
            ("/r/.git/FETCH_HEAD", Kinds::NONE),
            ("/r/.git/ORIG_HEAD", Kinds::NONE),
            ("/r/.git/AUTO_MERGE", Kinds::NONE),
            ("/r/.git/COMMIT_EDITMSG", Kinds::NONE),
            ("/r/.git/hooks/pre-commit", Kinds::NONE),
            ("/r/.git/info/exclude", Kinds::NONE),
            ("/r/.git/gc.pid", Kinds::NONE),
            ("/r/.git/gc.log", Kinds::NONE),
            ("/r/.git/shallow", Kinds::NONE),
            ("/r/.git/config", Kinds::CONFIG),
            ("/r/.git/worktrees", Kinds::WORKTREES),
            ("/r/.git/worktrees/w", Kinds::WORKTREES),
            ("/r/.git/worktrees/w/HEAD", Kinds::WORKTREES),
            ("/r/.git/worktrees/w/locked", Kinds::WORKTREES),
            ("/r/.git/worktrees/w/index", Kinds::NONE),
            ("/r/.git/modules/sub/HEAD", Kinds::WORKTREE_FILES),
            ("/r/.git/modules/sub/refs/heads/x", Kinds::WORKTREE_FILES),
            ("/r/.git/modules/sub/objects/ab/cd", Kinds::NONE),
            ("/r/.git/reftable/tables.list", Kinds::REFS),
            ("/r/.git/reftable/0x1-0x2-ab.ref", Kinds::REFS),
            ("/r/.git/reftable/tables.list.lock", Kinds::NONE),
            ("/r/.git/unknown", Kinds::NONE),
            // The git dir itself, and a work-tree path.
            ("/r/.git", Kinds::NONE),
            ("/r/src/main.rs", Kinds::WORKTREE_FILES),
            ("/r/Cargo.lock", Kinds::WORKTREE_FILES),
            ("/r/sub/.git", Kinds::WORKTREE_FILES),
            ("/r/sub/.git/HEAD", Kinds::WORKTREE_FILES),
            // Outside the repository.
            ("/elsewhere/file", Kinds::NONE),
            ("/rx/file", Kinds::NONE),
        ];
        for (path, want) in cases {
            assert_eq!(c(&l, path), *want, "{path}");
        }
    }

    #[test]
    fn lock_files_are_ignored() {
        let l = layout();
        for p in [
            "/r/.git/index.lock",
            "/r/.git/HEAD.lock",
            "/r/.git/refs/heads/x.lock",
            "/r/.git/refs/stash.lock",
            "/r/.git/packed-refs.lock",
            "/r/.git/config.lock",
            "/r/.git/MERGE_HEAD.lock",
            "/r/.git/worktrees/w/HEAD.lock",
        ] {
            assert_eq!(c(&l, p), Kinds::NONE, "{p}");
        }
    }

    #[test]
    fn rename_of_a_lock_file_counts_through_its_final_name() {
        // The debouncer can report `x.lock -> x` as one event with both paths; the caller
        // ORs `classify` over every path, so the final name must carry the kind.
        let l = layout();
        let both = c(&l, "/r/.git/refs/heads/x.lock") | c(&l, "/r/.git/refs/heads/x");
        assert_eq!(both, Kinds::REFS);
        let both = c(&l, "/r/.git/index.lock") | c(&l, "/r/.git/index");
        assert_eq!(both, Kinds::INDEX);
        let both = c(&l, "/r/.git/HEAD.lock") | c(&l, "/r/.git/HEAD");
        assert_eq!(both, Kinds::HEAD);
    }

    #[test]
    fn linked_worktree_table() {
        let l = linked();
        let cases: &[(&str, Kinds)] = &[
            // Per-worktree git dir: HEAD, index, operations.
            ("/m/.git/worktrees/w/HEAD", Kinds::HEAD),
            ("/m/.git/worktrees/w/index", Kinds::INDEX),
            ("/m/.git/worktrees/w/MERGE_HEAD", Kinds::OPERATION),
            ("/m/.git/worktrees/w/rebase-merge/done", Kinds::OPERATION),
            ("/m/.git/worktrees/w/logs/HEAD", Kinds::NONE),
            ("/m/.git/worktrees/w/index.lock", Kinds::NONE),
            // Refs and config are shared and live in the common dir.
            ("/m/.git/refs/heads/main", Kinds::REFS),
            ("/m/.git/refs/stash", Kinds::STASH | Kinds::REFS),
            ("/m/.git/packed-refs", Kinds::REFS),
            ("/m/.git/config", Kinds::CONFIG),
            ("/m/.git/logs/refs/stash", Kinds::STASH),
            // The main worktree's HEAD and index are not this window's business.
            ("/m/.git/HEAD", Kinds::NONE),
            ("/m/.git/index", Kinds::NONE),
            // Other linked worktrees and the folder of this one.
            ("/m/.git/worktrees", Kinds::WORKTREES),
            ("/m/.git/worktrees/w", Kinds::WORKTREES),
            ("/m/.git/worktrees/other", Kinds::WORKTREES),
            ("/m/.git/worktrees/other/HEAD", Kinds::WORKTREES),
            ("/m/.git/worktrees/other/index", Kinds::NONE),
            ("/m/.git/modules/sub/HEAD", Kinds::WORKTREE_FILES),
            // Work tree.
            ("/w/src/lib.rs", Kinds::WORKTREE_FILES),
            ("/w/.git", Kinds::WORKTREE_FILES),
            ("/m/src/lib.rs", Kinds::NONE),
        ];
        for (path, want) in cases {
            assert_eq!(c(&l, path), *want, "{path}");
        }
    }

    #[test]
    fn bare_repo_has_no_worktree_files() {
        let l = bare();
        assert_eq!(c(&l, "/b.git/HEAD"), Kinds::HEAD);
        assert_eq!(c(&l, "/b.git/refs/heads/main"), Kinds::REFS);
        assert_eq!(c(&l, "/b.git/config"), Kinds::CONFIG);
        assert_eq!(c(&l, "/b.git/objects/aa/bb"), Kinds::NONE);
        assert_eq!(c(&l, "/other/file"), Kinds::NONE);
    }

    /// Raw and canonical forms differ when the repository path goes through a symlink
    /// (`/var -> /private/var` on macOS); events may carry either.
    #[cfg(unix)]
    #[test]
    fn raw_and_canonical_prefixes_both_match() {
        let base = TempDir::new("classify-canon");
        let real = base.path().join("real");
        std::fs::create_dir_all(real.join(".git")).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let raw_root = link.clone();
        let l = GitLayout::new(
            Some(&raw_root),
            &raw_root.join(".git"),
            &raw_root.join(".git"),
        );
        let canon_root = std::fs::canonicalize(&real).unwrap();
        assert_ne!(raw_root, canon_root);
        for root in [&raw_root, &canon_root] {
            assert_eq!(classify(&l, &root.join(".git/HEAD")), Kinds::HEAD);
            assert_eq!(classify(&l, &root.join(".git/refs/heads/x")), Kinds::REFS);
            assert_eq!(classify(&l, &root.join("src/a.rs")), Kinds::WORKTREE_FILES);
        }
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\repo\.git")),
            PathBuf::from(r"C:\repo\.git")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share\r")),
            PathBuf::from(r"\\srv\share\r")
        );
        assert_eq!(
            strip_verbatim(Path::new("/r/.git")),
            PathBuf::from("/r/.git")
        );
        assert_eq!(strip_verbatim(Path::new(r"C:\r")), PathBuf::from(r"C:\r"));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_event_paths_match_plain_layout() {
        let l = GitLayout::new(
            Some(Path::new(r"C:\r")),
            Path::new(r"C:\r\.git"),
            Path::new(r"C:\r\.git"),
        );
        assert_eq!(classify(&l, Path::new(r"\\?\C:\r\.git\HEAD")), Kinds::HEAD);
    }

    #[test]
    fn the_same_folder_spelled_differently_is_not_a_linked_worktree() {
        let t = TempDir::new("same-folder");
        let root = t.path();
        mkdirs(root, &[".git/refs/heads", "sub"]);
        // `common_dir` as another code path might spell it.
        let other = root.join("sub").join("..").join(".git");
        let l = GitLayout::new(Some(root), &root.join(".git"), &other);
        assert!(!l.is_linked());
        assert_eq!(
            classify(&l, &root.join(".git/refs/heads/main")),
            Kinds::REFS
        );
        assert_eq!(classify(&l, &root.join(".git/HEAD")), Kinds::HEAD);
        // The watch plan holds the git dir once.
        let plan = watch_plan_impl(&l, Mode::Reduced, &[], true);
        assert_eq!(
            plan.iter()
                .filter(|(p, _)| same_folder(p, &root.join(".git")))
                .count(),
            1,
            "{plan:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_common_dir_is_not_a_linked_worktree() {
        let t = TempDir::new("symlink-folder");
        let root = t.path();
        mkdirs(root, &[".git/refs"]);
        std::os::unix::fs::symlink(root.join(".git"), root.join("gitlink")).unwrap();
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join("gitlink"));
        assert!(!l.is_linked());
        assert!(!layout().is_linked() && linked().is_linked());
    }

    #[test]
    fn ignored_top_level_dirs_classify_as_nothing() {
        let l = layout().with_ignored_dirs(vec![OsString::from("target")]);
        assert_eq!(c(&l, "/r/target/debug/app"), Kinds::NONE);
        assert_eq!(c(&l, "/r/target"), Kinds::NONE);
        assert_eq!(c(&l, "/r/src/target/x"), Kinds::WORKTREE_FILES);
        assert_eq!(c(&l, "/r/targets/x"), Kinds::WORKTREE_FILES);
        assert_eq!(c(&l, "/r/.git/HEAD"), Kinds::HEAD);
    }

    #[test]
    fn plan_watches_submodule_git_dirs_and_reftable_when_present() {
        let t = TempDir::new("plan-modules");
        let root = t.path();
        mkdirs(root, &[".git/refs", ".git/modules/sub", ".git/reftable"]);
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        let plan = watch_plan_impl(&l, Mode::Reduced, &[], true);
        assert!(plan.contains(&(root.join(".git/modules"), RecursiveMode::Recursive)));
        assert!(plan.contains(&(root.join(".git/reftable"), RecursiveMode::NonRecursive)));
        let t = TempDir::new("plan-no-modules");
        mkdirs(t.path(), &[".git/refs"]);
        let l = GitLayout::new(
            Some(t.path()),
            &t.path().join(".git"),
            &t.path().join(".git"),
        );
        let plan = watch_plan_impl(&l, Mode::Reduced, &[], true);
        assert!(!plan
            .iter()
            .any(|(p, _)| p.ends_with("modules") || p.ends_with("reftable")));
    }

    #[test]
    fn kinds_bit_operations() {
        let k = Kinds::HEAD | Kinds::REFS;
        assert!(k.contains(Kinds::HEAD));
        assert!(!k.contains(Kinds::INDEX));
        assert!(k.intersects(Kinds::REFS | Kinds::INDEX));
        assert!(Kinds::NONE.is_empty());
        let mut m = Kinds::NONE;
        m |= Kinds::STASH;
        assert_eq!(m, Kinds::STASH);
    }

    /// A unique temp folder removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("gitwarp-watch-{name}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mkdirs(base: &Path, rels: &[&str]) {
        for r in rels {
            std::fs::create_dir_all(base.join(r)).unwrap();
        }
    }

    use RecursiveMode::{NonRecursive as NR, Recursive as R};

    #[test]
    fn plan_normal_repo_linux_strategy() {
        let t = TempDir::new("plan-normal");
        let root = t.path();
        mkdirs(
            root,
            &[".git/refs", ".git/logs/refs", "src", "target", "docs"],
        );
        std::fs::write(root.join("file.txt"), "x").unwrap();
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        let plan = watch_plan_impl(&l, Mode::Normal, &[OsString::from("target")], true);
        assert_eq!(
            plan,
            vec![
                (root.join(".git"), NR),
                (root.join(".git/refs"), R),
                (root.join(".git/logs/refs"), NR),
                (root.to_path_buf(), NR),
                (root.join("docs"), R),
                (root.join("src"), R),
            ]
        );
    }

    #[test]
    fn plan_normal_repo_recursive_root_strategy() {
        let t = TempDir::new("plan-recursive");
        let root = t.path();
        mkdirs(root, &[".git/refs", "src", "target"]);
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        // The pruned list is a Linux-only optimisation: elsewhere the root is watched as a whole.
        let plan = watch_plan_impl(&l, Mode::Normal, &[OsString::from("target")], false);
        assert_eq!(
            plan,
            vec![
                (root.join(".git"), NR),
                (root.join(".git/refs"), R),
                (root.to_path_buf(), R),
            ]
        );
    }

    #[test]
    fn plan_missing_folders_are_skipped() {
        let t = TempDir::new("plan-missing");
        let root = t.path();
        // No refs, logs or worktrees folders at all.
        mkdirs(root, &[".git"]);
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        let plan = watch_plan_impl(&l, Mode::Reduced, &[], true);
        assert_eq!(plan, vec![(root.join(".git"), NR)]);

        // A root that is gone is skipped too.
        let gone = GitLayout::new(
            Some(&root.join("nope")),
            &root.join(".git"),
            &root.join(".git"),
        );
        let plan = watch_plan_impl(&gone, Mode::Normal, &[], true);
        assert_eq!(plan, vec![(root.join(".git"), NR)]);
    }

    #[test]
    fn plan_reduced_mode_has_git_dirs_only() {
        let t = TempDir::new("plan-reduced");
        let root = t.path();
        mkdirs(
            root,
            &[".git/refs", ".git/logs/refs", ".git/worktrees", "src"],
        );
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        for per_dir in [true, false] {
            let plan = watch_plan_impl(&l, Mode::Reduced, &[], per_dir);
            assert_eq!(
                plan,
                vec![
                    (root.join(".git"), NR),
                    (root.join(".git/refs"), R),
                    (root.join(".git/logs/refs"), NR),
                    (root.join(".git/worktrees"), R),
                ],
                "per_dir = {per_dir}"
            );
        }
    }

    #[test]
    fn plan_linked_worktree() {
        let t = TempDir::new("plan-linked");
        let base = t.path();
        let main = base.join("m");
        let work = base.join("w");
        mkdirs(
            &main,
            &[".git/refs", ".git/logs/refs", ".git/worktrees/w", "src"],
        );
        mkdirs(&work, &["src", "target"]);
        // `.git` of a linked worktree is a file, never a folder.
        std::fs::write(work.join(".git"), "gitdir: x").unwrap();
        let gd = main.join(".git/worktrees/w");
        let cd = main.join(".git");
        let l = GitLayout::new(Some(&work), &gd, &cd);
        let plan = watch_plan_impl(&l, Mode::Normal, &[OsString::from("target")], true);
        assert_eq!(
            plan,
            vec![
                (gd.clone(), NR),
                (cd.clone(), NR),
                (cd.join("refs"), R),
                (cd.join("logs/refs"), NR),
                (cd.join("worktrees"), R),
                (work.clone(), NR),
                (work.join("src"), R),
            ]
        );
    }

    #[test]
    fn plan_bare_repo_has_no_worktree_entries() {
        let t = TempDir::new("plan-bare");
        let b = t.path();
        mkdirs(b, &["refs", "objects"]);
        let l = GitLayout::new(None, b, b);
        for per_dir in [true, false] {
            let plan = watch_plan_impl(&l, Mode::Normal, &[], per_dir);
            assert_eq!(plan, vec![(b.to_path_buf(), NR), (b.join("refs"), R)]);
        }
    }

    #[test]
    fn plan_top_level_files_and_symlinks_are_not_watched_recursively() {
        let t = TempDir::new("plan-files");
        let root = t.path();
        mkdirs(root, &[".git", "real"]);
        std::fs::write(root.join("a.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        let l = GitLayout::new(Some(root), &root.join(".git"), &root.join(".git"));
        let plan = watch_plan_impl(&l, Mode::Normal, &[], true);
        assert_eq!(
            plan,
            vec![
                (root.join(".git"), NR),
                (root.to_path_buf(), NR),
                (root.join("real"), R),
            ]
        );
    }

    // ---- the real watcher ----

    use crate::test_support::TempRepo;
    use std::sync::mpsc;
    use std::time::Instant;

    fn git_cli(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// Waits up to 10 s for a signal containing `want`.
    fn wait_for(rx: &mpsc::Receiver<WatchSignal>, want: Kinds) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Kinds::NONE;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(sig) = rx.recv_timeout(left) {
                seen |= sig.kinds;
                if seen.intersects(want) {
                    return true;
                }
            }
        }
        false
    }

    /// Starts a watcher on a fresh repository; canonical paths because FSEvents reports them.
    fn started(name: &str) -> (TempRepo, PathBuf, RepoWatcher, mpsc::Receiver<WatchSignal>) {
        let repo = TempRepo::new(name);
        let root = std::fs::canonicalize(repo.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        git_cli(&root, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let layout = GitLayout::new(Some(&root), &root.join(".git"), &root.join(".git"));
        let (tx, rx) = mpsc::channel();
        let watcher = RepoWatcher::start(&layout, move |s| {
            let _ = tx.send(s);
        })
        .expect("the watcher starts");
        // FSEvents and Windows need a moment before the stream delivers.
        std::thread::sleep(Duration::from_millis(500));
        (repo, root, watcher, rx)
    }

    #[test]
    fn real_watcher_reports_a_commit() {
        let (_repo, root, watcher, rx) = started("watch-commit");
        assert_eq!(watcher.mode(), Mode::Normal);
        git_cli(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
        assert!(wait_for(&rx, Kinds::REFS | Kinds::HEAD));
    }

    #[test]
    fn real_watcher_reports_a_work_tree_file() {
        let (_repo, root, _watcher, rx) = started("watch-file");
        std::fs::write(root.join("src").join("x.txt"), "x").unwrap();
        assert!(wait_for(&rx, Kinds::WORKTREE_FILES));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_created_after_the_start_gets_watched() {
        let (_repo, root, _watcher, rx) = started("watch-newdir");
        std::fs::create_dir_all(root.join("newdir")).unwrap();
        // Let the creation be reported and the new watch be added, then forget those signals.
        std::thread::sleep(Duration::from_millis(2500));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("newdir").join("z.txt"), "z").unwrap();
        assert!(wait_for(&rx, Kinds::WORKTREE_FILES));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_ignored_folder_is_not_watched() {
        let (_repo, root, _watcher, rx) = started("watch-newignored");
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::thread::sleep(Duration::from_millis(1000));
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("build").join("o.bin"), "z").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(sig) = rx.recv_timeout(left) {
                assert!(
                    !sig.kinds.intersects(Kinds::WORKTREE_FILES),
                    "a file in an ignored new folder was reported"
                );
            }
        }
    }

    #[test]
    fn reduced_mode_ignores_the_work_tree() {
        let repo = TempRepo::new("watch-reduced");
        let root = std::fs::canonicalize(repo.path()).unwrap();
        git_cli(&root, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let layout = GitLayout::new(Some(&root), &root.join(".git"), &root.join(".git"));
        let (tx, rx) = mpsc::channel();
        let watcher = RepoWatcher::start_in(&layout, Mode::Reduced, Path::new("git"), move |s| {
            let _ = tx.send(s);
        })
        .expect("the watcher starts");
        assert_eq!(watcher.mode(), Mode::Reduced);
        std::thread::sleep(Duration::from_millis(500));
        git_cli(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
        assert!(wait_for(&rx, Kinds::REFS | Kinds::HEAD));
    }

    #[test]
    fn signal_merges_every_path_and_maps_errors() {
        use notify_debouncer_full::notify::{Error, Event, EventKind};
        use notify_debouncer_full::DebouncedEvent;
        let l = layout();
        let event = Event::new(EventKind::Any)
            .add_path("/r/.git/refs/heads/x.lock".into())
            .add_path("/r/.git/refs/heads/x".into());
        let sig = signal_of(&l, Ok(vec![DebouncedEvent::new(event, Instant::now())])).unwrap();
        assert_eq!(sig.kinds, Kinds::REFS);
        assert!(!sig.limit_hit);

        let sig = signal_of(&l, Err(vec![Error::new(ErrorKind::MaxFilesWatch)])).unwrap();
        assert!(sig.limit_hit);
        assert!(signal_of(&l, Err(vec![Error::generic("boom")])).is_none());
        let noise = Event::new(EventKind::Any).add_path("/r/.git/objects/ab/c".into());
        assert!(signal_of(&l, Ok(vec![DebouncedEvent::new(noise, Instant::now())])).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ignored_top_level_dirs_come_from_git() {
        let repo = TempRepo::new("watch-ignore");
        let root = repo.path();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let ignored = ignored_top_level_dirs(Path::new("git"), root);
        assert_eq!(ignored, vec![OsString::from("target")]);
    }
}
