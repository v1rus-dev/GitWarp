//! Pure model of the recent repositories list: what state a row is in
//! (available / checking / unavailable and why), which rows are selected, and
//! the wording shared by the welcome list and the Activity card.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use git_core::{Head, PathProbe};
use gpui::SharedString;
use ui::ChipKind;

use crate::format::{display_path, same_path};

/// Why a recent entry can't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    FolderMissing,
    NotARepository,
    /// A linked worktree whose git directory is gone.
    WorktreeRemoved {
        main_repo: Option<PathBuf>,
    },
    /// The OS refused (permissions, an unmounted share); carries the error text.
    Unreadable(String),
}

/// The state of one row of the recent list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// The background check has not finished.
    Checking,
    Available {
        branch: Option<SharedString>,
    },
    Unavailable(Unavailable),
}

/// The result of `Repo::open(path)` plus its HEAD: the repository root git found and its HEAD.
pub type OpenResult = Result<(PathBuf, Option<Head>), git_core::Error>;

/// Classifies a recent entry from the file-system probe and, for a
/// [`PathProbe::Candidate`], the result of opening it.
///
/// `open` is `None` when git could not be started (probing needs only the
/// file system); a candidate is then shown as available without a branch.
pub fn classify(path: &Path, probe: PathProbe, open: Option<OpenResult>) -> RowState {
    match probe {
        PathProbe::Missing | PathProbe::NotADirectory => {
            RowState::Unavailable(Unavailable::FolderMissing)
        }
        PathProbe::Unreadable(error) => RowState::Unavailable(Unavailable::Unreadable(error)),
        PathProbe::BrokenWorktree { main_repo, .. } => {
            RowState::Unavailable(Unavailable::WorktreeRemoved { main_repo })
        }
        PathProbe::Candidate { .. } => match open {
            None => RowState::Available { branch: None },
            Some(Ok((root, head))) => {
                // `Repo::open` searches upward: a folder that is no longer a
                // repository but sits inside another one opens that parent.
                // Only the stored folder itself counts as the repository.
                if same_path(&root, path) {
                    let branch = match head {
                        Some(Head::Branch { name, .. }) => Some(SharedString::from(name)),
                        // Same as before the row states: a detached HEAD shows no branch.
                        Some(Head::Detached(_)) | None => None,
                    };
                    RowState::Available { branch }
                } else {
                    RowState::Unavailable(Unavailable::NotARepository)
                }
            }
            Some(Err(git_core::Error::NotARepository { .. })) => {
                RowState::Unavailable(Unavailable::NotARepository)
            }
            Some(Err(git_core::Error::PathNotFound { .. })) => {
                RowState::Unavailable(Unavailable::FolderMissing)
            }
            Some(Err(error)) => RowState::Unavailable(Unavailable::Unreadable(error.to_string())),
        },
    }
}

impl Unavailable {
    /// The short label of the row chip.
    pub fn chip_label(&self) -> SharedString {
        match self {
            Self::FolderMissing => ui::t!("welcome.recent.state.missing"),
            Self::NotARepository => ui::t!("welcome.recent.state.not_repo"),
            Self::WorktreeRemoved { .. } => ui::t!("welcome.recent.state.worktree_removed"),
            Self::Unreadable(_) => ui::t!("welcome.recent.state.unreadable"),
        }
    }

    /// A missing folder is an error; the other states are warnings (the entry may come back).
    pub fn chip_kind(&self) -> ChipKind {
        match self {
            Self::FolderMissing => ChipKind::Err,
            _ => ChipKind::Warn,
        }
    }

    /// The row tooltip. `home` shortens paths to `~`.
    pub fn tooltip(&self, path: &Path, home: Option<&Path>) -> SharedString {
        let shown = display_path(path, home);
        match self {
            Self::FolderMissing => ui::t!("welcome.recent.tip.missing", path = shown),
            Self::NotARepository => ui::t!("welcome.recent.tip.not_repo", path = shown),
            Self::WorktreeRemoved {
                main_repo: Some(main),
            } => ui::t!(
                "welcome.recent.tip.worktree_removed",
                main = display_path(main, home)
            ),
            Self::WorktreeRemoved { main_repo: None } => {
                ui::t!("welcome.recent.tip.worktree_removed_unknown")
            }
            Self::Unreadable(error) => ui::t!("welcome.recent.tip.unreadable", error = error),
        }
    }
}

impl RowState {
    pub fn unavailable(&self) -> Option<&Unavailable> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// What a [`RecentSelection`] selects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Selected {
    /// Just the cursor row.
    #[default]
    Cursor,
    /// An explicit multi-selection. Empty means nothing is selected:
    /// Ctrl-toggling off the last selected row must not fall back to the
    /// cursor row. Any other selection change (click, Shift range, select all,
    /// `clear`) replaces it.
    Explicit(BTreeSet<PathBuf>),
}

/// Which recent rows are selected. Keyed by path so filtering the list does
/// not lose the selection; the index arguments are positions in the currently
/// visible `rows`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentSelection {
    /// The keyboard cursor, an index into the visible rows.
    pub cursor: usize,
    /// Where a Shift range starts.
    anchor: Option<usize>,
    selected: Selected,
}

impl RecentSelection {
    /// Plain click or arrow movement: the row alone, no multi-selection.
    pub fn click(&mut self, index: usize) {
        self.cursor = index;
        self.anchor = Some(index);
        self.selected = Selected::Cursor;
    }

    /// Ctrl/Cmd+click: adds or removes the row. The cursor row counts as
    /// selected before the first toggle, so the click extends it.
    pub fn toggle(&mut self, index: usize, rows: &[PathBuf]) {
        let Some(path) = rows.get(index) else { return };
        let mut set = match std::mem::take(&mut self.selected) {
            Selected::Cursor => rows.get(self.cursor).cloned().into_iter().collect(),
            Selected::Explicit(set) => set,
        };
        if !set.remove(path) {
            set.insert(path.clone());
        }
        self.selected = Selected::Explicit(set);
        self.cursor = index;
        self.anchor = Some(index);
    }

    /// Shift+click: selects the range from the anchor (the cursor, if none) to `index`.
    pub fn extend_to(&mut self, index: usize, rows: &[PathBuf]) {
        if rows.is_empty() {
            return;
        }
        let index = index.min(rows.len() - 1);
        let anchor = self.anchor.unwrap_or(self.cursor).min(rows.len() - 1);
        let (from, to) = (anchor.min(index), anchor.max(index));
        self.selected = Selected::Explicit(rows[from..=to].iter().cloned().collect());
        self.cursor = index;
        self.anchor = Some(anchor);
    }

    /// Shift+Up/Down: [`Self::extend_to`] one row away from the cursor.
    pub fn extend_by(&mut self, delta: isize, rows: &[PathBuf]) {
        let target = self.cursor.saturating_add_signed(delta);
        self.extend_to(target, rows);
    }

    /// Ctrl/Cmd+A: every visible row.
    pub fn select_all(&mut self, rows: &[PathBuf]) {
        self.selected = if rows.is_empty() {
            Selected::Cursor
        } else {
            Selected::Explicit(rows.iter().cloned().collect())
        };
        self.anchor.get_or_insert(self.cursor);
    }

    /// Drops the multi-selection; the cursor row stays selected.
    pub fn clear(&mut self) {
        self.selected = Selected::Cursor;
        self.anchor = None;
    }

    /// The paths a command (Del, the context menu) acts on: the selected
    /// visible rows in list order, or the cursor row when nothing is selected
    /// explicitly. Rows hidden by the filter are never targeted.
    pub fn targets(&self, rows: &[PathBuf]) -> Vec<PathBuf> {
        match &self.selected {
            Selected::Cursor => rows.get(self.cursor).cloned().into_iter().collect(),
            Selected::Explicit(set) => rows
                .iter()
                .filter(|path| set.contains(*path))
                .cloned()
                .collect(),
        }
    }

    /// How many visible rows are selected.
    pub fn count(&self, rows: &[PathBuf]) -> usize {
        self.targets(rows).len()
    }

    /// Whether the row at `index` is highlighted as selected.
    pub fn is_selected(&self, index: usize, rows: &[PathBuf]) -> bool {
        match &self.selected {
            Selected::Cursor => index == self.cursor,
            Selected::Explicit(set) => rows.get(index).is_some_and(|path| set.contains(path)),
        }
    }

    /// Forgets selected paths that are no longer in the list (after a removal)
    /// and keeps the cursor inside `len` visible rows.
    pub fn retain(&mut self, existing: &[PathBuf], len: usize) {
        if let Selected::Explicit(set) = &mut self.selected {
            let had = !set.is_empty();
            set.retain(|path| existing.contains(path));
            // Every explicitly selected row was removed: fall back to the cursor row.
            if had && set.is_empty() {
                self.selected = Selected::Cursor;
            }
        }
        self.anchor = None;
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(format!("/r/{n}")))
            .collect()
    }

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/r/{name}"))
    }

    fn not_a_repo(path: &str) -> git_core::Error {
        git_core::Error::NotARepository { path: p(path) }
    }

    #[test]
    fn classify_probe_results() {
        let path = p("x");
        let state = |probe| classify(&path, probe, None);
        assert_eq!(
            state(PathProbe::Missing),
            RowState::Unavailable(Unavailable::FolderMissing)
        );
        assert_eq!(
            state(PathProbe::NotADirectory),
            RowState::Unavailable(Unavailable::FolderMissing)
        );
        assert_eq!(
            state(PathProbe::Unreadable("denied".into())),
            RowState::Unavailable(Unavailable::Unreadable("denied".into()))
        );
        assert_eq!(
            state(PathProbe::BrokenWorktree {
                gitdir: p("m/.git/worktrees/x"),
                main_repo: Some(p("m")),
            }),
            RowState::Unavailable(Unavailable::WorktreeRemoved {
                main_repo: Some(p("m"))
            })
        );
        // Git could not start: the file system found nothing wrong.
        assert_eq!(
            state(PathProbe::Candidate { linked: false }),
            RowState::Available { branch: None }
        );
    }

    #[test]
    fn classify_open_results() {
        let path = p("x");
        let branch = Head::Branch {
            name: "main".into(),
            target: None,
        };
        assert_eq!(
            classify(
                &path,
                PathProbe::Candidate { linked: false },
                Some(Ok((path.clone(), Some(branch))))
            ),
            RowState::Available {
                branch: Some("main".into())
            }
        );
        assert_eq!(
            classify(
                &path,
                PathProbe::Candidate { linked: false },
                Some(Ok((path.clone(), None)))
            ),
            RowState::Available { branch: None }
        );
        // Parent-repository trap: the folder opened a different (enclosing) root.
        assert_eq!(
            classify(
                &path,
                PathProbe::Candidate { linked: false },
                Some(Ok((p("parent"), None)))
            ),
            RowState::Unavailable(Unavailable::NotARepository)
        );
        assert_eq!(
            classify(
                &path,
                PathProbe::Candidate { linked: false },
                Some(Err(not_a_repo("x")))
            ),
            RowState::Unavailable(Unavailable::NotARepository)
        );
        assert_eq!(
            classify(
                &path,
                PathProbe::Candidate { linked: false },
                Some(Err(git_core::Error::PathNotFound { path: p("x") }))
            ),
            RowState::Unavailable(Unavailable::FolderMissing)
        );
        let other = git_core::Error::Spawn(std::io::Error::other("boom"));
        let RowState::Unavailable(Unavailable::Unreadable(text)) = classify(
            &path,
            PathProbe::Candidate { linked: false },
            Some(Err(other)),
        ) else {
            panic!("expected Unreadable");
        };
        assert!(text.contains("boom"), "{text}");
    }

    #[test]
    fn classify_accepts_the_same_folder_through_a_symlink() {
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("gitwarp-recent-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let real = dir.join("real");
            let link = dir.join("link");
            std::fs::create_dir_all(&real).unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let state = classify(
                &link,
                PathProbe::Candidate { linked: false },
                Some(Ok((real, None))),
            );
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(state, RowState::Available { branch: None });
        }
    }

    #[test]
    fn chip_wording_and_tooltips() {
        let path = PathBuf::from("/home/u/proj/wt");
        let home = Some(Path::new("/home/u"));
        assert_eq!(Unavailable::FolderMissing.chip_kind(), ChipKind::Err);
        assert_eq!(Unavailable::NotARepository.chip_kind(), ChipKind::Warn);
        assert_eq!(
            Unavailable::FolderMissing.chip_label().as_ref(),
            "Folder not found"
        );
        assert_eq!(
            Unavailable::FolderMissing.tooltip(&path, home).as_ref(),
            "~/proj/wt no longer exists"
        );
        assert_eq!(
            Unavailable::WorktreeRemoved {
                main_repo: Some(PathBuf::from("/home/u/main"))
            }
            .tooltip(&path, home)
            .as_ref(),
            "The worktree was removed from its repository (~/main)"
        );
        assert_eq!(
            Unavailable::Unreadable("Permission denied".into())
                .tooltip(&path, home)
                .as_ref(),
            "Permission denied"
        );
    }

    #[test]
    fn selection_click_resets_the_set() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = RecentSelection::default();
        sel.toggle(2, &rows);
        sel.click(1);
        assert_eq!(sel.targets(&rows), vec![p("b")]);
        assert!(sel.is_selected(1, &rows));
        assert!(!sel.is_selected(0, &rows));
    }

    #[test]
    fn selection_toggle_extends_the_cursor_row() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = RecentSelection::default();
        sel.click(0);
        sel.toggle(2, &rows);
        assert_eq!(sel.targets(&rows), vec![p("a"), p("c")]);
        assert_eq!(sel.count(&rows), 2);
        sel.toggle(0, &rows);
        assert_eq!(sel.targets(&rows), vec![p("c")]);
        assert!(sel.is_selected(2, &rows) && !sel.is_selected(0, &rows));
    }

    #[test]
    fn toggling_off_the_last_selected_row_selects_nothing() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = RecentSelection::default();
        sel.click(1);
        sel.toggle(1, &rows);
        assert!(sel.targets(&rows).is_empty());
        assert_eq!(sel.count(&rows), 0);
        assert!(!sel.is_selected(1, &rows));
        // The next Ctrl+click selects only the clicked row, not the stale cursor row.
        sel.toggle(2, &rows);
        assert_eq!(sel.targets(&rows), vec![p("c")]);
        // A plain click (or arrow) brings the cursor selection back.
        sel.toggle(2, &rows);
        assert!(sel.targets(&rows).is_empty());
        sel.click(0);
        assert_eq!(sel.targets(&rows), vec![p("a")]);
        assert!(sel.is_selected(0, &rows));
        // `clear` also restores the cursor row.
        sel.toggle(0, &rows);
        sel.clear();
        assert_eq!(sel.targets(&rows), vec![p("a")]);
    }

    #[test]
    fn selection_shift_range_both_directions() {
        let rows = rows(&["a", "b", "c", "d", "e"]);
        let mut sel = RecentSelection::default();
        sel.click(1);
        sel.extend_to(3, &rows);
        assert_eq!(sel.targets(&rows), vec![p("b"), p("c"), p("d")]);
        // The anchor stays at 1, so shrinking and flipping works.
        sel.extend_to(0, &rows);
        assert_eq!(sel.targets(&rows), vec![p("a"), p("b")]);
        sel.extend_by(1, &rows);
        assert_eq!(sel.cursor, 1);
        assert_eq!(sel.targets(&rows), vec![p("b")]);
        sel.extend_by(100, &rows);
        assert_eq!(sel.cursor, 4);
        assert_eq!(sel.count(&rows), 4);
    }

    #[test]
    fn selection_select_all_covers_only_visible_rows() {
        let all = rows(&["a", "b", "c", "d"]);
        let mut sel = RecentSelection::default();
        let visible = vec![p("b"), p("d")];
        sel.select_all(&visible);
        assert_eq!(sel.targets(&visible), visible);
        // Back in the full list, hidden rows are not selected.
        assert_eq!(sel.targets(&all), vec![p("b"), p("d")]);
        sel.select_all(&all);
        assert_eq!(sel.count(&all), 4);
        // A filter hides rows: they are not targeted.
        assert_eq!(sel.targets(&visible), visible);
        sel.clear();
        sel.cursor = 1;
        assert_eq!(sel.targets(&all), vec![p("b")]);
    }

    #[test]
    fn selection_retain_after_removal() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = RecentSelection::default();
        sel.select_all(&rows);
        sel.cursor = 3;
        let left = vec![p("a"), p("c")];
        sel.retain(&left, left.len());
        assert_eq!(sel.targets(&left), left);
        assert_eq!(sel.cursor, 1);
        sel.retain(&[], 0);
        assert_eq!(sel.cursor, 0);
        assert!(sel.targets(&[]).is_empty());
    }

    #[test]
    fn selection_ignores_out_of_range_indices() {
        let rows = rows(&["a"]);
        let mut sel = RecentSelection::default();
        sel.toggle(5, &rows);
        sel.extend_to(5, &rows);
        assert_eq!(sel.targets(&rows), vec![p("a")]);
        sel.extend_to(0, &[]);
    }
}
