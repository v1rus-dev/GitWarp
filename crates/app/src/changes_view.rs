//! Local changes: the two file lists, the selected file's diff with stage
//! controls, and the commit box.
//!
//! All repository work runs on the background executor. The view only holds
//! the answer: the status snapshot, the selected file's diff and the picks the
//! user made for line-level staging.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, px, uniform_list, AnyElement, App, AppContext, Context, Div, Entity,
    EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    ListHorizontalSizingBehavior, ParentElement, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Subscription, TextRun, UniformListScrollHandle,
    Window,
};

use git_core::{ChangeKind, Error, FileDiff, Git, HunkPick, Repo, WorkingFile, WorkingStatus};
use ui::actions::{key_binding, SelectNext, SelectPrev};
use ui::components::toast::{self, ToastKind};
use ui::metrics::{FONT_MONO, FS_SM, FS_XS, MONO};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    Button, ButtonSize, CountPill, FileRow, IconName, ScrollArea, Scrollbar, SegmentItem,
    Segmented, SelectableText, Skeleton, Switch, TextArea, TextAreaEvent,
};

use crate::diff_model::{
    build_rows_with, build_split_rows, max_columns, DiffRow, RowKind, SplitCell, SplitCellKind,
    SplitRow,
};
use crate::settings::DiffLayout;

/// The width of the two file lists.
pub const FILES_WIDTH: f32 = 310.;
const DIFF_ROW: f32 = 22.;
const DIFF_GUTTER: f32 = 44.;
const DIFF_MARKER: f32 = 16.;
/// The subject line's soft limit, for the character guides and counter.
pub const SUBJECT_LIMIT: usize = 50;
/// The body's soft line-length limit.
pub const BODY_LIMIT: usize = 72;

/// Emitted when a commit succeeded, so the shell can reload the history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesEvent {
    Committed,
    /// A commit the user asked to push succeeded; the shell starts the push.
    PushAfterCommit,
}

/// Which side of the index a file row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Unstaged,
    Staged,
}

impl Side {
    fn staged(self) -> bool {
        self == Self::Staged
    }
}

/// One selected row: its side and index in that side's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub side: Side,
    pub index: usize,
}

/// What the view knows about the work tree.
#[derive(Clone, Debug, Default)]
pub enum StatusState {
    #[default]
    Loading,
    Loaded(Arc<WorkingStatus>),
    Failed(String),
}

/// One file's diff, flattened into the rows the two layouts paint.
#[derive(Clone, Debug)]
pub struct ChangesDiff {
    pub diff: FileDiff,
    pub rows: Vec<DiffRow>,
    pub split_rows: Vec<SplitRow>,
    pub columns: usize,
}

/// What the diff pane knows about the selected file.
#[derive(Clone, Debug, Default)]
pub enum DiffState {
    #[default]
    Empty,
    Loading,
    Loaded(Arc<ChangesDiff>),
    Failed(String),
}

pub struct ChangesView {
    path: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    state: StatusState,
    generation: u64,
    selection: Option<Selection>,
    diff: DiffState,
    diff_generation: u64,
    /// Picked `(hunk, line)` pairs for line-level staging.
    picked: HashSet<(usize, usize)>,
    pick_anchor: Option<(usize, usize)>,
    /// The row whose discard button is waiting for confirmation.
    confirming_discard: Option<Selection>,
    message: Entity<TextArea>,
    _message_events: Subscription,
    amend: bool,
    /// The message the user typed before amend replaced it.
    draft: Option<String>,
    busy: bool,
    focus: FocusHandle,
    unstaged_scroll: ScrollHandle,
    staged_scroll: ScrollHandle,
    diff_scroll: UniformListScrollHandle,
    mono_cell: Pixels,
    layout: DiffLayout,
}

impl ChangesView {
    pub fn new(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let message = cx.new(|cx| {
            let mut input = TextArea::new(window, cx);
            input.set_placeholder("feat(graph): …", cx);
            input.set_font_family(FONT_MONO, cx);
            input.set_height(px(104.), cx);
            input
        });
        let message_events = cx.subscribe(&message, |_, _, event: &TextAreaEvent, cx| {
            let TextAreaEvent::Changed(_) = event;
            cx.notify();
        });
        let mono_size = cx
            .try_global::<crate::AppSettings>()
            .map(|state| px(f32::from(state.settings.mono_font_size)))
            .unwrap_or(MONO);
        let mono_cell = window
            .text_system()
            .ch_advance(
                window.text_system().resolve_font(&gpui::font(FONT_MONO)),
                mono_size,
            )
            .unwrap_or(px(7.2));
        let layout = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_layout)
            .unwrap_or_default();
        let mut view = Self {
            path,
            git,
            state: StatusState::Loading,
            generation: 0,
            selection: None,
            diff: DiffState::Empty,
            diff_generation: 0,
            picked: HashSet::new(),
            pick_anchor: None,
            confirming_discard: None,
            message,
            _message_events: message_events,
            amend: false,
            draft: None,
            busy: false,
            focus: cx.focus_handle().tab_stop(false),
            unstaged_scroll: ScrollHandle::new(),
            staged_scroll: ScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            mono_cell,
            layout,
        };
        view.start_status(cx);
        view
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn status(&self) -> Option<&WorkingStatus> {
        match &self.state {
            StatusState::Loaded(status) => Some(status),
            _ => None,
        }
    }

    pub fn unstaged_count(&self) -> usize {
        self.status().map_or(0, WorkingStatus::unstaged_count)
    }

    pub fn staged_count(&self) -> usize {
        self.status().map_or(0, WorkingStatus::staged_count)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, StatusState::Loading)
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            StatusState::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Reloads the status, e.g. when the view becomes visible or the work tree
    /// changed outside the app.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start_status(cx);
    }

    fn file(&self, selection: Selection) -> Option<&WorkingFile> {
        let status = self.status()?;
        match selection.side {
            Side::Unstaged => status.unstaged.get(selection.index),
            Side::Staged => status.staged.get(selection.index),
        }
    }

    fn selection_path(&self) -> Option<(Side, String)> {
        let selection = self.selection?;
        self.file(selection)
            .map(|file| (selection.side, file.path.clone()))
    }

    fn start_status(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let previous = self.selection_path();
        self.state = StatusState::Loading;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.state = StatusState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    repo.working_status().map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |view, cx| {
                view.set_status(generation, previous, result, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_status(
        &mut self,
        generation: u64,
        previous: Option<(Side, String)>,
        result: Result<WorkingStatus, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let status = match result {
            Ok(status) => Arc::new(status),
            Err(error) => {
                self.state = StatusState::Failed(error);
                self.diff = DiffState::Empty;
                cx.notify();
                return;
            }
        };
        let selection = previous
            .and_then(|(side, path)| {
                let files = match side {
                    Side::Unstaged => &status.unstaged,
                    Side::Staged => &status.staged,
                };
                files
                    .iter()
                    .position(|file| file.path == path)
                    .map(|index| Selection { side, index })
                    .or_else(|| {
                        // The file may have moved to the other side.
                        let other = if side == Side::Unstaged {
                            Side::Staged
                        } else {
                            Side::Unstaged
                        };
                        let files = match other {
                            Side::Unstaged => &status.unstaged,
                            Side::Staged => &status.staged,
                        };
                        files
                            .iter()
                            .position(|file| file.path == path)
                            .map(|index| Selection { side: other, index })
                    })
            })
            .or_else(|| {
                (!status.unstaged.is_empty()).then_some(Selection {
                    side: Side::Unstaged,
                    index: 0,
                })
            })
            .or_else(|| {
                (!status.staged.is_empty()).then_some(Selection {
                    side: Side::Staged,
                    index: 0,
                })
            });
        self.selection = selection;
        self.picked.clear();
        self.pick_anchor = None;
        self.confirming_discard = None;
        self.state = StatusState::Loaded(status);
        self.start_diff(cx);
        cx.notify();
    }

    /// Moves the file selection through both lists with the arrow keys.
    fn step_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(status) = self.status() else {
            return;
        };
        let mut rows = Vec::with_capacity(status.unstaged.len() + status.staged.len());
        rows.extend((0..status.unstaged.len()).map(|index| Selection {
            side: Side::Unstaged,
            index,
        }));
        rows.extend((0..status.staged.len()).map(|index| Selection {
            side: Side::Staged,
            index,
        }));
        if rows.is_empty() {
            return;
        }
        let current = self
            .selection
            .and_then(|selection| rows.iter().position(|row| *row == selection))
            .unwrap_or(0) as isize;
        let next = current
            .saturating_add(step)
            .clamp(0, rows.len() as isize - 1) as usize;
        let selection = rows[next];
        self.select(selection.side, selection.index, cx);
    }

    /// Selects a file row and loads its diff.
    fn select(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        let selection = Selection { side, index };
        if self.selection == Some(selection) {
            return;
        }
        self.selection = Some(selection);
        self.picked.clear();
        self.pick_anchor = None;
        self.confirming_discard = None;
        self.start_diff(cx);
        cx.notify();
    }

    fn start_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        self.diff_scroll = UniformListScrollHandle::new();
        let Some(selection) = self.selection else {
            self.diff = DiffState::Empty;
            return;
        };
        let Some(file) = self.file(selection).cloned() else {
            self.diff = DiffState::Empty;
            return;
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.diff = DiffState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        self.diff = DiffState::Loading;
        let path = self.path.clone();
        let staged = selection.side.staged();
        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let options = settings.diff_options();
        let word_highlight = settings.diff_word_highlight;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_diff(git, &path, &file, staged, options, word_highlight) })
                .await;
            this.update(cx, |view, cx| view.set_diff(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_diff(
        &mut self,
        generation: u64,
        result: Result<ChangesDiff, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.diff_generation {
            return;
        }
        self.diff = match result {
            Ok(data) => DiffState::Loaded(Arc::new(data)),
            Err(error) => DiffState::Failed(error),
        };
        cx.notify();
    }

    fn diff_data(&self) -> Option<Arc<ChangesDiff>> {
        match &self.diff {
            DiffState::Loaded(data) => Some(data.clone()),
            _ => None,
        }
    }

    fn set_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.diff_scroll = UniformListScrollHandle::new();
        crate::update_settings(cx, |settings| settings.diff_layout = layout);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_message_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.message
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    fn set_amend(&mut self, amend: bool, cx: &mut Context<Self>) {
        if self.amend == amend {
            return;
        }
        self.amend = amend;
        let current = self.message.read(cx).text().to_string();
        if amend {
            self.draft = Some(current);
            if let Some(head) = self.status().and_then(|status| status.head.as_ref()) {
                let message = head.message.trim_end().to_string();
                self.message
                    .update(cx, |input, cx| input.set_text(message, cx));
            }
        } else if let Some(draft) = self.draft.take() {
            self.message
                .update(cx, |input, cx| input.set_text(draft, cx));
        }
        cx.notify();
    }

    fn toggle_pick(&mut self, hunk: usize, line: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        if !self.can_pick(selection) {
            return;
        }
        if shift {
            if let Some((anchor_hunk, anchor_line)) = self.pick_anchor {
                if anchor_hunk == hunk {
                    let (from, to) = if anchor_line <= line {
                        (anchor_line, line)
                    } else {
                        (line, anchor_line)
                    };
                    for index in from..=to {
                        self.picked.insert((hunk, index));
                    }
                    cx.notify();
                    return;
                }
            }
        }
        let key = (hunk, line);
        if !self.picked.remove(&key) {
            self.picked.insert(key);
        }
        self.pick_anchor = Some(key);
        cx.notify();
    }

    /// Line picking works on the unstaged side for additions, modifications
    /// and deletions, and on the staged side only for modifications: those
    /// are the patches that can be reversed line by line.
    fn can_pick(&self, selection: Selection) -> bool {
        let Some(file) = self.file(selection) else {
            return false;
        };
        if selection.side == Side::Unstaged {
            matches!(
                file.kind,
                ChangeKind::Added | ChangeKind::Modified | ChangeKind::Deleted
            )
        } else {
            file.kind == ChangeKind::Modified
        }
    }

    fn picked_in(&self, hunk: usize) -> usize {
        self.picked
            .iter()
            .filter(|(picked_hunk, _)| *picked_hunk == hunk)
            .count()
    }

    fn picks(&self, hunk: usize) -> Vec<HunkPick> {
        let mut lines: Vec<usize> = self
            .picked
            .iter()
            .filter(|(picked_hunk, _)| *picked_hunk == hunk)
            .map(|(_, line)| *line)
            .collect();
        lines.sort_unstable();
        if lines.is_empty() {
            vec![HunkPick::whole(hunk)]
        } else {
            vec![HunkPick::lines(hunk, lines)]
        }
    }

    fn stage_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(file) = self.file(selection).cloned() else {
            return;
        };
        let Some(data) = self.diff_data() else {
            return;
        };
        let picks = self.picks(hunk);
        let picked = picks[0].lines.as_ref().map_or(0, Vec::len);
        let staged = selection.side.staged();
        let notice = match (picked > 0, staged) {
            (true, true) => ui::i18n::t_count("changes.notice.unstaged_lines", picked as u64, &[]),
            (true, false) => ui::i18n::t_count("changes.notice.staged_lines", picked as u64, &[]),
            (false, true) => ui::t!(
                "changes.notice.unstaged_hunk",
                n = hunk + 1,
                path = file.path
            ),
            (false, false) => ui::t!("changes.notice.staged_hunk", n = hunk + 1, path = file.path),
        }
        .to_string();
        self.picked.clear();
        self.pick_anchor = None;
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage_selection(&file, &data.diff, &picks)?;
            } else {
                repo.stage_selection(&file, &data.diff, &picks)?;
            }
            Ok(notice)
        });
    }

    /// Space on a file row: stages the selected unstaged file, or unstages the
    /// selected staged one.
    fn stage_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(selection) = self.selection {
            self.stage_file(selection.side, selection.index, cx);
        }
    }

    /// Ctrl+Shift+Z on a file row: asks to discard the selected unstaged file
    /// through the inline confirm bar, whatever the "confirm discard" setting
    /// says — a key press never discards on its own.
    fn ask_discard_selected(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(selection) = self.selection.filter(|s| s.side == Side::Unstaged) {
            if self.file(selection).is_some() {
                self.confirming_discard = Some(selection);
                cx.notify();
            }
        }
    }

    fn stage_file(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .status()
            .and_then(|status| match side {
                Side::Unstaged => status.unstaged.get(index),
                Side::Staged => status.staged.get(index),
            })
            .cloned()
        else {
            return;
        };
        let path = file.path.clone();
        let staged = side.staged();
        let notice = if staged {
            ui::t!("changes.notice.unstaged_file", path = path)
        } else {
            ui::t!("changes.notice.staged_file", path = path)
        }
        .to_string();
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage(std::slice::from_ref(&path))?;
            } else {
                repo.stage(std::slice::from_ref(&path))?;
            }
            Ok(notice)
        });
    }

    pub(crate) fn stage_all(&mut self, side: Side, cx: &mut Context<Self>) {
        let Some(status) = self.status() else {
            return;
        };
        let paths: Vec<String> = match side {
            Side::Unstaged => status
                .unstaged
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            Side::Staged => status.staged.iter().map(|file| file.path.clone()).collect(),
        };
        if paths.is_empty() {
            return;
        }
        let staged = side.staged();
        let notice = if staged {
            ui::i18n::t_count("changes.notice.unstaged_files", paths.len() as u64, &[])
        } else {
            ui::i18n::t_count("changes.notice.staged_files", paths.len() as u64, &[])
        }
        .to_string();
        self.start_action(cx, move |repo| {
            if staged {
                repo.unstage(&paths)?;
            } else {
                repo.stage(&paths)?;
            }
            Ok(notice)
        });
    }

    fn discard(&mut self, side: Side, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .status()
            .and_then(|status| match side {
                Side::Unstaged => status.unstaged.get(index),
                Side::Staged => status.staged.get(index),
            })
            .cloned()
        else {
            return;
        };
        let path = file.path.clone();
        let untracked = file.untracked;
        let notice = ui::t!("changes.notice.discarded", path = path).to_string();
        self.confirming_discard = None;
        self.start_action(cx, move |repo| {
            if untracked {
                repo.discard(&[], std::slice::from_ref(&path))?;
            } else {
                repo.discard(std::slice::from_ref(&path), &[])?;
            }
            Ok(notice)
        });
    }

    /// Runs one repository mutation, then reloads the status.
    fn start_action<F>(&mut self, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(&Repo) -> git_core::Result<String> + Send + 'static,
    {
        if self.busy {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("cannot change the work tree: {error}");
                return;
            }
        };
        self.busy = true;
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    let notice = action(&repo).map_err(|error| error.to_string())?;
                    let status = repo.working_status().map_err(|error| error.to_string())?;
                    Ok::<_, String>((notice, status))
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_action(result, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_action(
        &mut self,
        result: Result<(String, WorkingStatus), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        match result {
            Ok((notice, status)) => {
                toast::show(ToastKind::Ok, notice, window, cx);
                let previous = self.selection_path();
                self.apply_status(status, previous, cx);
            }
            Err(error) => {
                toast::show(ToastKind::Error, error, window, cx);
                cx.notify();
            }
        }
    }

    fn apply_status(
        &mut self,
        status: WorkingStatus,
        previous: Option<(Side, String)>,
        cx: &mut Context<Self>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.set_status(generation, previous, Ok(status), cx);
    }

    /// Commits the index, or amends HEAD, from the commit box.
    pub(crate) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_maybe_push(false, window, cx);
    }

    /// Commits and asks the shell to push the branch once the commit lands.
    fn commit_and_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_maybe_push(true, window, cx);
    }

    fn commit_maybe_push(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let message = self.message.read(cx).text().trim_end().to_string();
        if message.trim().is_empty() {
            toast::show(
                ToastKind::Warn,
                ui::t!("changes.commit.message_required"),
                window,
                cx,
            );
            return;
        }
        let staged = self.staged_count();
        let amend = self.amend;
        if !amend && staged == 0 {
            return;
        }
        let branch = self
            .status()
            .and_then(|status| status.branch.clone())
            .unwrap_or_else(|| "HEAD".to_string());
        let notice = if amend {
            ui::t!("changes.notice.amended").to_string()
        } else {
            ui::i18n::t_count(
                "changes.notice.committed",
                staged as u64,
                &[("branch", &branch)],
            )
            .to_string()
        };
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("cannot commit: {error}");
                return;
            }
        };
        self.busy = true;
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|error| error.to_string())?;
                    repo.commit(&message, amend)
                        .map_err(|error| error.to_string())?;
                    let status = repo.working_status().map_err(|error| error.to_string())?;
                    Ok::<_, String>((notice, status))
                })
                .await;
            this.update_in(cx, |view, window, cx| {
                view.finish_commit(result, push, window, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_commit(
        &mut self,
        result: Result<(String, WorkingStatus), String>,
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        match result {
            Ok((notice, status)) => {
                toast::show(ToastKind::Ok, notice, window, cx);
                self.message.update(cx, |input, cx| input.set_text("", cx));
                self.amend = false;
                self.draft = None;
                let previous = self.selection_path();
                self.apply_status(status, previous, cx);
                cx.emit(ChangesEvent::Committed);
                if push {
                    cx.emit(ChangesEvent::PushAfterCommit);
                }
            }
            Err(error) => {
                toast::show(ToastKind::Error, error, window, cx);
                cx.notify();
            }
        }
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let pane_width = (f32::from(window.viewport_size().width) - FILES_WIDTH - 560.).max(320.);
        div()
            .size_full()
            .flex()
            .flex_col()
            .debug_selector(|| "changes-view".to_string())
            .key_context("changes")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &CommitChanges, window, cx| {
                view.commit(window, cx);
            }))
            .on_action(cx.listener(|view, _: &StageSelected, _, cx| {
                view.stage_selected(cx);
            }))
            .on_action(cx.listener(|view, _: &DiscardSelected, _, cx| {
                view.ask_discard_selected(cx);
            }))
            .on_action(cx.listener(|view, _: &SelectNext, _, cx| {
                view.step_selection(1, cx);
            }))
            .on_action(cx.listener(|view, _: &SelectPrev, _, cx| {
                view.step_selection(-1, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(self.render_files(cx))
                    .child(self.render_diff(pane_width, colors, cx)),
            )
            .child(self.render_commit_box(cx))
    }

    fn render_files(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let confirm = self.confirming_discard.and_then(|selection| {
            self.file(selection)
                .map(|file| (selection, file.path.clone()))
        });
        let mut panel = div()
            .w(px(FILES_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_start()
            .border_r_1()
            .border_color(colors.border);
        if let Some((selection, path)) = confirm {
            let discard_view = view.clone();
            let cancel_view = view.clone();
            panel = panel.child(
                div()
                    .w_full()
                    .flex_none()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(ui::t!("changes.discard.confirm", path = path)),
                    )
                    .child(
                        Button::danger("confirm-discard")
                            .size(ButtonSize::Small)
                            .label(ui::t!("changes.discard.button"))
                            .on_click(move |_, _, cx| {
                                discard_view.update(cx, |view, cx| {
                                    view.discard(selection.side, selection.index, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("cancel-discard")
                            .size(ButtonSize::Small)
                            .label(ui::t!("changes.discard.cancel"))
                            .on_click(move |_, _, cx| {
                                cancel_view.update(cx, |view, cx| {
                                    view.confirming_discard = None;
                                    cx.notify();
                                });
                            }),
                    ),
            );
        }

        let (unstaged, staged) = match self.status() {
            Some(status) => (status.unstaged.clone(), status.staged.clone()),
            None => (Vec::new(), Vec::new()),
        };
        panel
            .child(self.render_group(
                Side::Unstaged,
                ui::t!("changes.group.unstaged"),
                &unstaged,
                ui::t!("changes.group.unstaged_empty"),
                self.unstaged_scroll.clone(),
                view.clone(),
                cx,
            ))
            .child(self.render_group(
                Side::Staged,
                ui::t!("changes.group.staged"),
                &staged,
                ui::t!("changes.group.staged_empty"),
                self.staged_scroll.clone(),
                view,
                cx,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_group(
        &mut self,
        side: Side,
        title: SharedString,
        files: &[WorkingFile],
        empty: SharedString,
        scroll: ScrollHandle,
        view: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let busy = self.busy;
        let all_view = view.clone();
        let all_label = match side {
            Side::Unstaged => ui::t!("changes.group.stage_all"),
            Side::Staged => ui::t!("changes.group.unstage_all"),
        };
        let mut rows = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let selected = self.selection == Some(Selection { side, index });
            let status = status_letter(file);
            let path = file.path.clone();
            let mut row = FileRow::new(
                format!(
                    "changes-{}-{index}",
                    if side == Side::Unstaged {
                        "unstaged"
                    } else {
                        "staged"
                    }
                ),
                status,
                path,
            )
            .added(file.added)
            .deleted(file.deleted)
            .selected(selected);
            let select_view = view.clone();
            let stage_view = view.clone();
            let discard_view = view.clone();
            let stage_icon = if side == Side::Unstaged {
                IconName::Plus
            } else {
                IconName::Minus
            };
            let stage_tip = match (
                side == Side::Unstaged,
                crate::settings_ui::keybindings::key_cap(cx, "stage"),
            ) {
                (true, Some(cap)) => ui::t!("changes.row.stage_with_key", cap = cap),
                (false, Some(cap)) => ui::t!("changes.row.unstage_with_key", cap = cap),
                (true, None) => ui::t!("changes.row.stage"),
                (false, None) => ui::t!("changes.row.unstage"),
            };
            row = row.action(
                Button::ghost(format!("stage-{side:?}-{index}"))
                    .size(ButtonSize::Xs)
                    .icon_only(stage_icon)
                    .tooltip(stage_tip)
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        stage_view.update(cx, |view, cx| view.stage_file(side, index, cx));
                    }),
            );
            if side == Side::Unstaged {
                row = row.action(
                    Button::ghost(format!("discard-{side:?}-{index}"))
                        .size(ButtonSize::Xs)
                        .icon_only(IconName::Undo)
                        .tooltip(
                            match crate::settings_ui::keybindings::key_cap(cx, "discard") {
                                Some(cap) => ui::t!(
                                    "changes.row.discard_with_key",
                                    cap = cap.replace(' ', "+")
                                ),
                                None => ui::t!("changes.row.discard"),
                            },
                        )
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let confirm = cx
                                .try_global::<crate::AppSettings>()
                                .map(|state| state.settings.confirm_discard)
                                .unwrap_or(true);
                            discard_view.update(cx, |view, cx| {
                                if confirm {
                                    view.confirming_discard = Some(Selection { side, index });
                                    cx.notify();
                                } else {
                                    view.discard(side, index, cx);
                                }
                            });
                        }),
                );
            }
            let debug_id = format!(
                "changes-{}-{index}",
                if side == Side::Unstaged {
                    "unstaged"
                } else {
                    "staged"
                }
            );
            rows.push(
                div()
                    .debug_selector(move || debug_id.clone())
                    .key_context("changes-file")
                    .child(row.on_click(move |_, _, cx| {
                        select_view.update(cx, |view, cx| view.select(side, index, cx));
                    })),
            );
        }

        let list = if files.is_empty() {
            div()
                .px(px(16.))
                .py(px(10.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(empty)
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        };
        let count = files.len();
        let stage_all = Button::new(format!("stage-all-{side:?}"))
            .size(ButtonSize::Small)
            .label(all_label)
            .disabled(busy || count == 0)
            .on_click(move |_, _, cx| {
                all_view.update(cx, |view, cx| view.stage_all(side, cx));
            });
        div()
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(side == Side::Staged, |this| {
                this.border_t_1().border_color(colors.border)
            })
            .child(
                div()
                    .w_full()
                    .h(px(34.))
                    .flex_none()
                    .pl(px(16.))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(CountPill::new(count.to_string()))
                    .child(div().flex_1())
                    .child(stage_all),
            )
            .child(ScrollArea::new(format!("changes-scroll-{side:?}"), scroll).child(list))
    }

    fn render_diff(&mut self, pane_width: f32, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let selection = self.selection;
        let path = selection
            .and_then(|selection| self.file(selection))
            .map(|file| match &file.old_path {
                Some(old) => format!("{} ← {old}", file.path),
                None => file.path.clone(),
            });
        let hunks = self
            .diff_data()
            .map(|data| data.diff.hunks.len())
            .unwrap_or(0);
        let layout = self.layout;
        let view = cx.entity();
        let mut toolbar = div()
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border);
        toolbar = toolbar.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(FS_SM)
                .text_color(colors.text)
                .child(path.unwrap_or_default()),
        );
        if let Some(selection) = selection {
            let side = match selection.side {
                Side::Unstaged => ui::t!("changes.side.unstaged"),
                Side::Staged => ui::t!("changes.side.staged"),
            };
            toolbar = toolbar.child(
                div()
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(ui::i18n::t_count(
                        "changes.diff.side_hunks",
                        hunks as u64,
                        &[("side", &side)],
                    )),
            );
        }
        toolbar = toolbar.child(
            Segmented::new("changes-layout")
                .small()
                .item(
                    SegmentItem::new(ui::t!("changes.layout.unified"))
                        .with_icon(IconName::Unified)
                        .id("changes-unified"),
                )
                .item(
                    SegmentItem::new(ui::t!("changes.layout.split"))
                        .with_icon(IconName::Split)
                        .id("changes-split"),
                )
                .selected(if layout == DiffLayout::Split { 1 } else { 0 })
                .on_select(move |index, _, cx| {
                    let layout = if index == 1 {
                        DiffLayout::Split
                    } else {
                        DiffLayout::Unified
                    };
                    view.update(cx, |view, cx| view.set_layout(layout, cx));
                }),
        );

        let body = match &self.diff {
            DiffState::Empty => diff_note(ui::t!("changes.diff.select_file"), colors),
            DiffState::Loading => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .child(Skeleton::new("changes-diff-skeleton-1", px(320.)))
                .child(Skeleton::new("changes-diff-skeleton-2", px(260.)))
                .child(Skeleton::new("changes-diff-skeleton-3", px(290.))),
            DiffState::Failed(message) => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .text_color(colors.error)
                .child(SelectableText::new(
                    gpui::ElementId::Name("changes-diff-error".into()),
                    message.clone(),
                )),
            DiffState::Loaded(data) => {
                if data.diff.binary {
                    diff_note(ui::t!("changes.diff.binary"), colors)
                } else if data.rows.is_empty() {
                    diff_note(ui::t!("changes.diff.no_changes"), colors)
                } else if layout == DiffLayout::Split {
                    let row_width = split_width(pane_width);
                    let data = data.clone();
                    let list = uniform_list(
                        "changes-diff",
                        data.split_rows.len(),
                        cx.processor(move |view, range, window, cx| {
                            view.render_split_rows(data.clone(), range, row_width, window, cx)
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "changes-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                } else {
                    let row_width = diff_width(self.mono_cell, data.columns, pane_width);
                    let data = data.clone();
                    let list = uniform_list(
                        "changes-diff",
                        data.rows.len(),
                        cx.processor(move |view, range, window, cx| {
                            view.render_unified_rows(data.clone(), range, row_width, window, cx)
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "changes-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                }
            }
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(cx.theme().mono_font_size)
            .child(toolbar)
            .child(body)
    }

    fn render_unified_rows(
        &mut self,
        data: Arc<ChangesDiff>,
        range: std::ops::Range<usize>,
        row_width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let pane_width = px(f32::from(row_width));
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        let selection = self.selection;
        let staged = selection.is_some_and(|selection| selection.side.staged());
        let can_pick = selection.is_some_and(|selection| self.can_pick(selection));
        let hunk_button_allowed = selection.is_some_and(|selection| {
            self.file(selection).is_some_and(|file| {
                matches!(
                    file.kind,
                    ChangeKind::Added | ChangeKind::Modified | ChangeKind::Deleted
                ) && !(selection.side == Side::Staged && file.kind == ChangeKind::Added)
            })
        });
        range
            .filter_map(|index| {
                let row = data.rows.get(index)?;
                let row = row.clone();
                match row.kind {
                    RowKind::Hunk => {
                        let hunk = row.hunk.unwrap_or(0);
                        let picked = self.picked_in(hunk);
                        let label = match (picked, staged) {
                            (0, false) => ui::t!("changes.hunk.stage"),
                            (0, true) => ui::t!("changes.hunk.unstage"),
                            (n, false) => {
                                ui::i18n::t_count("changes.hunk.stage_lines", n as u64, &[])
                            }
                            (n, true) => {
                                ui::i18n::t_count("changes.hunk.unstage_lines", n as u64, &[])
                            }
                        };
                        let stage_view = cx.entity();
                        let button = Button::new(format!("changes-hunk-{hunk}"))
                            .size(ButtonSize::Small)
                            .label(label)
                            .disabled(self.busy || !hunk_button_allowed)
                            .on_click(move |_, _, cx| {
                                stage_view.update(cx, |view, cx| view.stage_hunk(hunk, cx));
                            });
                        Some(
                            div()
                                .id(SharedString::from(format!("changes-hunk-row-{index}")))
                                .h(px(DIFF_ROW))
                                .w(pane_width)
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .pl(px(8.))
                                .pr(px(6.))
                                .bg(colors.diff_hunk_bg)
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child(div().flex_1().min_w_0().truncate().child(row.text.clone()))
                                .child(button)
                                .into_any_element(),
                        )
                    }
                    RowKind::Note => Some(
                        div()
                            .h(px(DIFF_ROW))
                            .w(row_width)
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(8.))
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(row.text.clone())
                            .into_any_element(),
                    ),
                    RowKind::Context | RowKind::Added | RowKind::Removed => {
                        let (old_background, new_background, line_background, strong) =
                            match row.kind {
                                RowKind::Added => (
                                    None,
                                    Some(colors.diff_add_bg),
                                    Some(colors.diff_add_bg),
                                    colors.diff_add_strong,
                                ),
                                RowKind::Removed => (
                                    Some(colors.diff_del_bg),
                                    None,
                                    Some(colors.diff_del_bg),
                                    colors.diff_del_strong,
                                ),
                                _ => (None, None, None, colors.diff_add_strong),
                            };
                        let picked = row
                            .line
                            .zip(row.hunk)
                            .is_some_and(|(line, hunk)| self.picked.contains(&(hunk, line)));
                        let target = row
                            .line
                            .zip(row.hunk)
                            .filter(|_| matches!(row.kind, RowKind::Added | RowKind::Removed));
                        let pick_view = cx.entity();
                        let gutter = div()
                            .id(SharedString::from(format!("changes-gutter-{index}")))
                            .flex()
                            .flex_none()
                            .h_full()
                            .when(can_pick && target.is_some(), |this| this.cursor_pointer())
                            .on_click(move |event, _, cx| {
                                let Some((line, hunk)) = target else {
                                    return;
                                };
                                let shift = event.modifiers().shift;
                                pick_view.update(cx, |view, cx| {
                                    view.toggle_pick(hunk, line, shift, cx);
                                });
                            })
                            .when(show_numbers, |this| {
                                this.child(diff_gutter(row.old_line, old_background, colors))
                                    .child(diff_gutter(row.new_line, new_background, colors))
                            });
                        let mut element = div()
                            .id(SharedString::from(format!("changes-row-{index}")))
                            .relative()
                            .h(px(DIFF_ROW))
                            .w(row_width)
                            .flex_none()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .child(gutter)
                            .child(diff_marker(row.kind, line_background, colors));
                        if picked {
                            element = element.child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(0.))
                                    .w(px(3.))
                                    .h_full()
                                    .bg(colors.accent),
                            );
                        }
                        Some(
                            element
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .pl(px(6.))
                                        .pr(px(6.))
                                        .when_some(line_background, |this, background| {
                                            this.bg(background)
                                        })
                                        .text_color(colors.text)
                                        .child(emphasized_text(
                                            &row.text,
                                            &row.emphasis,
                                            strong,
                                            colors,
                                        )),
                                )
                                .into_any_element(),
                        )
                    }
                }
            })
            .collect()
    }

    fn render_split_rows(
        &mut self,
        data: Arc<ChangesDiff>,
        range: std::ops::Range<usize>,
        row_width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let show_numbers = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.diff_line_numbers)
            .unwrap_or(true);
        let gutters = if show_numbers { 2. * DIFF_GUTTER } else { 0. };
        let half = px(((f32::from(row_width) - gutters) / 2.).max(0.));
        range
            .filter_map(|index| {
                let row = data.split_rows.get(index)?;
                Some(split_row(row, index, row_width, half, colors, show_numbers))
            })
            .collect()
    }

    fn render_commit_box(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let text = self.message.read(cx).text().to_string();
        let (subject, longest_body) = message_counters(&text);
        let head = self.status().and_then(|status| status.head.clone());
        let branch = self
            .status()
            .and_then(|status| status.branch.clone())
            .unwrap_or_else(|| "HEAD".to_string());
        let staged = self.staged_count();
        let busy = self.busy;
        let commit_label = if self.amend {
            ui::t!("changes.commit.amend")
        } else {
            ui::i18n::t_count(
                "changes.commit.button",
                staged as u64,
                &[("branch", &branch)],
            )
        };
        let can_commit = !busy && (!commit_label.is_empty()) && (self.amend || staged > 0);
        let commit_view = cx.entity();
        let amend_view = cx.entity();
        let switch = Switch::new("amend-switch")
            .label(ui::t!("changes.commit.amend_switch"))
            .checked(self.amend)
            .disabled(busy || head.is_none())
            .on_toggle(move |checked, _, cx| {
                amend_view.update(cx, |view, cx| view.set_amend(checked, cx));
            });
        let help = match &head {
            Some(head) => ui::t!("changes.commit.amend_help", short = head.short),
            None => ui::t!("changes.commit.amend_none"),
        };
        let guide_50 = 10. + SUBJECT_LIMIT as f32 * f32::from(self.mono_cell);
        let guide_72 = 10. + BODY_LIMIT as f32 * f32::from(self.mono_cell);
        let counter = |value: usize, limit: usize, label: SharedString| {
            div()
                .text_size(FS_XS)
                .text_color(if value > limit {
                    colors.warning
                } else {
                    colors.muted
                })
                .child(format!("{label} {value} / {limit}"))
        };
        div()
            .w_full()
            .flex_none()
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .p(px(12.))
            .flex()
            .gap(px(16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .mb(px(4.))
                            .child(ui::t!("changes.commit.message_help")),
                    )
                    .child(
                        div()
                            .relative()
                            .child(self.message.clone())
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .left(px(guide_50))
                                    .w(px(1.))
                                    .bg(colors.border_strong)
                                    .opacity(0.7),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .left(px(guide_72))
                                    .w(px(1.))
                                    .bg(colors.border_strong),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .child(counter(
                                subject,
                                SUBJECT_LIMIT,
                                ui::t!("changes.commit.subject"),
                            ))
                            .child(counter(
                                longest_body,
                                BODY_LIMIT,
                                ui::t!("changes.commit.longest_body"),
                            ))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(FS_XS)
                                    .text_color(colors.muted)
                                    .child(ui::t!(
                                        "changes.commit.hint",
                                        key = ui::kbd::platform_label("Ctrl+Enter")
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(260.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(switch)
                            .child(div().text_size(FS_XS).text_color(colors.muted).child(help)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child({
                                let commit_view = commit_view.clone();
                                Button::primary("commit-button")
                                    .label(commit_label)
                                    .disabled(!can_commit)
                                    .on_click(move |_, window, cx| {
                                        commit_view.update(cx, |view, cx| view.commit(window, cx));
                                    })
                            })
                            .child({
                                let commit_view = commit_view.clone();
                                Button::new("commit-push-button")
                                    .label(ui::t!("changes.commit.commit_and_push"))
                                    .disabled(!can_commit)
                                    .on_click(move |_, window, cx| {
                                        commit_view.update(cx, |view, cx| {
                                            view.commit_and_push(window, cx)
                                        });
                                    })
                            }),
                    ),
            )
    }
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render(window, cx)
    }
}

impl EventEmitter<ChangesEvent> for ChangesView {}

/// Binds Ctrl+Enter for the commit box. Called once at startup.
pub fn bind_keys(cx: &mut App, settings: &crate::settings::Settings) {
    let key = settings
        .keybindings
        .get("commit")
        .filter(|keys| !keys.is_empty())
        .cloned()
        .unwrap_or_else(|| "secondary-enter".to_owned());
    let keys = |id: &str, default: &str| {
        settings
            .keybindings
            .get(id)
            .filter(|keys| !keys.is_empty())
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    // The file-row keys live in their own context: it is active on a row and
    // its buttons only, so Space still types in the message editor and a
    // focused button keeps its own Space.
    let file = Some("changes-file && !ui-button");
    cx.bind_keys([
        key_binding(cx, &key, CommitChanges, None),
        key_binding(cx, &keys("stage", "space"), StageSelected, file),
        key_binding(
            cx,
            &keys("discard", "secondary-shift-z"),
            DiscardSelected,
            file,
        ),
    ]);
}

actions!(gitwarp, [CommitChanges, StageSelected, DiscardSelected]);

/// Reads one file's diff and flattens it into rows off the UI thread.
fn read_diff(
    git: Arc<Git>,
    root: &Path,
    file: &WorkingFile,
    staged: bool,
    options: git_core::DiffOptions,
    word_highlight: bool,
) -> Result<ChangesDiff, String> {
    let repo = Repo::open(git, root).map_err(|error| error.to_string())?;
    let diff = if staged {
        repo.staged_diff_with(&file.path, file.old_path.as_deref(), options)
            .map_err(|error| error.to_string())?
    } else if file.untracked {
        repo.untracked_diff(&file.path)
            .map_err(|error| error.to_string())?
    } else {
        repo.unstaged_diff_with(&file.path, file.old_path.as_deref(), options)
            .map_err(|error| error.to_string())?
    };
    let rows = build_rows_with(&diff.hunks, word_highlight);
    let split_rows = build_split_rows(&rows);
    let columns = max_columns(&rows);
    Ok(ChangesDiff {
        diff,
        rows,
        split_rows,
        columns,
    })
}

/// The glyph a working-tree file shows: untracked and conflicted files keep
/// git's own letters instead of the change kind.
fn status_letter(file: &WorkingFile) -> char {
    if file.untracked {
        '?'
    } else {
        file.kind.letter()
    }
}

/// Changed paths of the work tree, grouped the way the sidebar's view nav
/// shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    /// Added and untracked paths.
    pub added: usize,
    /// Modified, renamed, copied, type-changed and conflicted paths.
    pub modified: usize,
    pub deleted: usize,
}

impl ChangeCounts {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.modified == 0 && self.deleted == 0
    }
}

/// Counts the unique changed paths in `status`: a path on both sides of the
/// index counts once, with the unstaged kind when the two differ. `None`
/// (status loading or failed) counts nothing.
pub fn change_counts(status: Option<&WorkingStatus>) -> ChangeCounts {
    let Some(status) = status else {
        return ChangeCounts::default();
    };
    // The index is the older state of a path; the work tree overrides it.
    let mut files: HashMap<&str, &WorkingFile> = HashMap::new();
    for file in &status.staged {
        files.insert(file.path.as_str(), file);
    }
    for file in &status.unstaged {
        files.insert(file.path.as_str(), file);
    }
    let mut counts = ChangeCounts::default();
    for file in files.values() {
        if file.untracked || file.kind == ChangeKind::Added {
            counts.added += 1;
        } else if file.kind == ChangeKind::Deleted {
            counts.deleted += 1;
        } else {
            counts.modified += 1;
        }
    }
    counts
}

/// The subject length and the longest body line, for the counters.
fn message_counters(text: &str) -> (usize, usize) {
    let mut lines = text.split('\n');
    let subject = lines.next().unwrap_or("").chars().count();
    let body = lines.map(|line| line.chars().count()).max().unwrap_or(0);
    (subject, body)
}

/// A muted, centred note for a pane with nothing to paint.
fn diff_note(text: SharedString, colors: ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(text)
}

fn diff_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let text = columns as f32 * f32::from(cell) + 24.;
    px((DIFF_GUTTER * 2. + DIFF_MARKER + text).max(pane))
}

fn split_width(pane: f32) -> Pixels {
    // Two equal halves that fill the pane whatever the content width; each
    // half clips its own lines (design: `.sbs { overflow: hidden }`).
    px(pane)
}

/// One 44 px line-number gutter.
fn diff_gutter(line: Option<u32>, background: Option<Hsla>, colors: ThemeColors) -> Div {
    div()
        .w(px(DIFF_GUTTER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(px(6.))
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.muted)
        .child(line.map(|line| line.to_string()).unwrap_or_default())
}

/// The 16 px `+` / `−` marker column.
fn diff_marker(kind: RowKind, background: Option<Hsla>, colors: ThemeColors) -> Div {
    let (marker, color) = match kind {
        RowKind::Added => ('+', colors.success),
        RowKind::Removed => ('−', colors.error),
        _ => (' ', colors.muted),
    };
    div()
        .w(px(DIFF_MARKER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .when_some(background, |this, background| this.bg(background))
        .text_color(color)
        .child(marker.to_string())
}

/// Text with the changed words drawn on their stronger background.
fn emphasized_text(
    text: &str,
    emphasis: &[std::ops::Range<usize>],
    strong: Hsla,
    colors: ThemeColors,
) -> StyledText {
    if emphasis.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    let font = gpui::font(FONT_MONO);
    let mut runs = Vec::new();
    let mut position = 0;
    let push = |runs: &mut Vec<TextRun>, len: usize, background: Option<Hsla>| {
        if len == 0 {
            return;
        }
        runs.push(TextRun {
            len,
            font: font.clone(),
            color: colors.text,
            background_color: background,
            underline: None,
            strikethrough: None,
        });
    };
    for range in emphasis {
        let start = range.start.min(text.len());
        let end = range.end.min(text.len());
        if start > position {
            push(&mut runs, start - position, None);
        }
        push(&mut runs, end.saturating_sub(start), Some(strong));
        position = end;
    }
    if position < text.len() {
        push(&mut runs, text.len() - position, None);
    }
    if runs.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
}

/// One side-by-side row.
fn split_row(
    row: &SplitRow,
    index: usize,
    width: Pixels,
    half: Pixels,
    colors: ThemeColors,
    show_numbers: bool,
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("changes-split-row-{index}")))
        .h(px(DIFF_ROW))
        .w(width)
        .flex_none()
        .flex()
        .items_center()
        .whitespace_nowrap();
    match row {
        SplitRow::Header(text) => base
            .px(px(8.))
            .bg(colors.diff_hunk_bg)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(text.clone())
            .into_any_element(),
        SplitRow::Note(text) => base
            .px(px(8.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(text.clone())
            .into_any_element(),
        SplitRow::Pair { old, new } => {
            let mut base = base;
            if show_numbers {
                base = base.child(split_gutter(old.as_ref(), colors));
            }
            base = base.child(split_content(old.as_ref(), half, colors));
            if show_numbers {
                base = base.child(
                    split_gutter(new.as_ref(), colors)
                        .border_l_1()
                        .border_color(colors.border),
                );
            }
            base.child(split_content(new.as_ref(), half, colors))
                .into_any_element()
        }
    }
}

fn split_gutter(cell: Option<&SplitCell>, colors: ThemeColors) -> Div {
    let background = cell.and_then(|cell| match cell.kind {
        SplitCellKind::Added => Some(colors.diff_add_bg),
        SplitCellKind::Removed => Some(colors.diff_del_bg),
        SplitCellKind::Context => None,
    });
    div()
        .w(px(DIFF_GUTTER))
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(px(6.))
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.muted)
        .child(cell.map(|cell| cell.line.to_string()).unwrap_or_default())
}

fn split_content(cell: Option<&SplitCell>, width: Pixels, colors: ThemeColors) -> Div {
    let (background, strong) = match cell.map(|cell| cell.kind) {
        Some(SplitCellKind::Added) => (Some(colors.diff_add_bg), colors.diff_add_strong),
        Some(SplitCellKind::Removed) => (Some(colors.diff_del_bg), colors.diff_del_strong),
        _ => (None, colors.diff_add_strong),
    };
    let content = div()
        .w(width)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .pl(px(6.))
        .pr(px(6.))
        .overflow_hidden()
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.text);
    match cell {
        Some(cell) => content.child(emphasized_text(&cell.text, &cell.emphasis, strong, colors)),
        None => content.bg(colors.hover),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_split_the_subject_from_the_body() {
        assert_eq!(message_counters(""), (0, 0));
        assert_eq!(message_counters("subject"), (7, 0));
        assert_eq!(message_counters("subject\n\nbody line\nx"), (7, 9));
        assert_eq!(message_counters("привет"), (6, 0));
    }

    #[test]
    fn untracked_files_keep_gits_question_mark() {
        let file = WorkingFile {
            kind: ChangeKind::Added,
            old_path: None,
            path: "new.txt".into(),
            added: 1,
            deleted: 0,
            binary: false,
            untracked: true,
        };
        assert_eq!(status_letter(&file), '?');
    }

    #[test]
    fn change_counts_are_empty_without_a_status() {
        assert_eq!(change_counts(None), ChangeCounts::default());
        assert!(change_counts(Some(&WorkingStatus::default())).is_empty());
    }

    #[test]
    fn change_counts_group_paths_by_kind() {
        let status = WorkingStatus {
            staged: vec![
                working_file("a.txt", ChangeKind::Added, false),
                working_file("b.txt", ChangeKind::Deleted, false),
            ],
            unstaged: vec![
                working_file("c.txt", ChangeKind::Added, true),
                working_file("d.txt", ChangeKind::Modified, false),
                working_file("e.txt", ChangeKind::Renamed, false),
                working_file("f.txt", ChangeKind::Other('U'), false),
            ],
            ..WorkingStatus::default()
        };
        assert_eq!(
            change_counts(Some(&status)),
            ChangeCounts {
                added: 2,
                modified: 3,
                deleted: 1,
            }
        );
    }

    #[test]
    fn change_counts_count_a_path_on_both_sides_once() {
        let status = WorkingStatus {
            staged: vec![
                working_file("a.txt", ChangeKind::Added, false),
                working_file("b.txt", ChangeKind::Deleted, false),
            ],
            unstaged: vec![
                working_file("a.txt", ChangeKind::Modified, false),
                working_file("b.txt", ChangeKind::Modified, false),
            ],
            ..WorkingStatus::default()
        };
        // The work tree's kind wins: both paths count as modified, once each.
        assert_eq!(
            change_counts(Some(&status)),
            ChangeCounts {
                added: 0,
                modified: 2,
                deleted: 0,
            }
        );
    }

    #[test]
    fn a_single_line_diff_is_never_narrower_than_the_pane() {
        assert!(diff_width(px(7.), 4, 400.) >= px(400.));
        assert!(split_width(400.) >= px(400.));
    }

    /// A changed file of `path`, for the counter tests.
    fn working_file(path: &str, kind: ChangeKind, untracked: bool) -> WorkingFile {
        WorkingFile {
            kind,
            old_path: None,
            path: path.into(),
            added: 1,
            deleted: 0,
            binary: false,
            untracked,
        }
    }

    /// Writes `content` to `file` under `dir`, creating parent directories.
    fn write(dir: &Path, file: &str, content: impl AsRef<[u8]>) {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// Runs git in `dir` with a throw-away identity.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git is installed");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Opens the changes view as the window root and waits for its status.
    fn open_changes(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, Entity<ChangesView>) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        let git = crate::test_support::git();
        cx.update(ui::init).expect("ui::init");
        cx.update(|cx| bind_keys(cx, &crate::settings::Settings::default()));
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1400.), px(900.)),
                    ))),
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        ChangesView::new(repo.path().to_path_buf(), Ok(git.clone()), window, cx)
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let view = cx
            .update(|window, _| window.root::<ChangesView>().flatten())
            .expect("the changes view is the window root");
        (cx, view)
    }

    /// Space on a focused file row stages the selected file; Ctrl+Shift+Z asks
    /// to discard it through the inline confirm bar and discards nothing.
    #[gpui::test]
    fn space_stages_and_ctrl_shift_z_asks_to_discard(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("changes-keys");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "two\n");

        let (mut cx, view) = open_changes(&repo, cx);
        let row = cx
            .debug_bounds("changes-unstaged-0")
            .expect("the unstaged row was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();

        cx.simulate_keystrokes("secondary-shift-z");
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.confirming_discard,
                Some(Selection {
                    side: Side::Unstaged,
                    index: 0
                })
            );
            assert_eq!(view.unstaged_count(), 1, "the key discarded without asking");
        });
        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "two\n"
        );

        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 1, "Space did not stage the file");
            assert_eq!(view.unstaged_count(), 0);
        });
    }

    #[gpui::test]
    fn loads_the_status_selects_the_first_file_and_stages(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-stage");
        write(repo.path(), "a.txt", "one\ntwo\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        write(repo.path(), "a.txt", "one\nTWO\nthree\n");
        write(repo.path(), "new.txt", "new\n");

        let (mut cx, view) = open_changes(&repo, cx);
        view.read_with(&cx, |view, _| {
            assert_eq!(view.unstaged_count(), 2, "{:?}", view.state);
            assert_eq!(view.staged_count(), 0);
            assert_eq!(
                view.selection.map(|selection| selection.side),
                Some(Side::Unstaged)
            );
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the first file's diff did not load");
            };
            assert!(diff.rows.iter().any(|row| row.text == "TWO"));
        });
        assert!(
            cx.debug_bounds("changes-view").is_some(),
            "the view was not drawn"
        );
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_some(),
            "the first unstaged row was not drawn"
        );
        assert!(
            cx.debug_bounds("changes-unstaged-1").is_some(),
            "the untracked row was not drawn"
        );

        view.update(&mut cx, |view, cx| view.stage_file(Side::Unstaged, 0, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged.len(), 1);
            assert_eq!(status.staged[0].path, "a.txt");
            assert_eq!(status.unstaged.len(), 1);
            assert_eq!(status.unstaged[0].path, "new.txt");
        });

        view.update(&mut cx, |view, cx| view.stage_all(Side::Unstaged, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 2);
            assert_eq!(view.unstaged_count(), 0);
        });

        view.update(&mut cx, |view, cx| view.stage_all(Side::Staged, cx));
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.staged_count(), 0);
            assert_eq!(view.unstaged_count(), 2);
        });
    }

    /// Picking a line in the diff stages only that line of the hunk.
    #[gpui::test]
    fn stages_a_picked_line(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-lines");
        let mut original = String::new();
        for line in 1..=20 {
            original.push_str(&format!("line {line}\n"));
        }
        write(repo.path(), "a.txt", &original);
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        let changed = original
            .replace("line 2\n", "line two\n")
            .replace("line 18\n", "line eighteen\n");
        write(repo.path(), "a.txt", &changed);

        let (mut cx, view) = open_changes(&repo, cx);
        let target = view.read_with(&cx, |view, _| {
            let data = view.diff_data().expect("the diff loaded");
            let row = data
                .rows
                .iter()
                .find(|row| row.text == "line eighteen")
                .expect("the second hunk's added line");
            (row.hunk.unwrap(), row.line.unwrap())
        });
        let picked_count = view.update(&mut cx, |view, cx| {
            view.toggle_pick(target.0, target.1, false, cx);
            assert_eq!(view.picked.len(), 1);
            assert_eq!(view.picked_in(target.0), 1);
            view.picks(target.0)
        });
        assert_eq!(picked_count.len(), 1);
        assert!(picked_count[0].lines.is_some());
        view.update(&mut cx, |view, cx| view.stage_hunk(target.0, cx));
        cx.run_until_parked();

        view.read_with(&cx, |view, _| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged.len(), 1);
            assert_eq!(status.unstaged.len(), 1);
        });
        let cached = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["diff", "--cached", "-U0"])
            .output()
            .unwrap();
        let cached = String::from_utf8_lossy(&cached.stdout);
        assert!(cached.contains("+line eighteen"), "{cached}");
        assert!(!cached.contains("+line two"), "{cached}");
    }

    /// Ctrl+Enter in the message editor reaches the view root and commits.
    #[gpui::test]
    fn ctrl_enter_commits_from_the_message_editor(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-ctrl-enter");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);

        let (mut cx, view) = open_changes(&repo, cx);
        write(repo.path(), "a.txt", "two\n");
        view.update(&mut cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| view.stage_all(Side::Unstaged, cx));
        cx.run_until_parked();

        let handle = cx.update(|window, cx| {
            let handle = view.read(cx).message.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
            handle
        });
        let _ = handle;
        cx.simulate_input("first commit");
        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.head.as_ref().unwrap().summary, "first commit");
            assert_eq!(status.unstaged_count(), 0);
            assert_eq!(view.message.read(cx).text(), "");
        });
    }

    #[gpui::test]
    fn commits_the_index_and_prefills_the_amend_message(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-commit");
        write(repo.path(), "a.txt", "one\n");
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);

        let (mut cx, view) = open_changes(&repo, cx);
        write(repo.path(), "a.txt", "two\n");
        view.update(&mut cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.message.update(cx, |message, cx| {
                message.set_text("feat: two\n\nWhy it exists.\n", cx)
            });
            view.stage_all(Side::Unstaged, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.commit(window, cx));
        });
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let status = view.status().expect("the status reloaded");
            assert_eq!(status.staged_count(), 0);
            assert_eq!(status.unstaged_count(), 0);
            assert_eq!(status.head.as_ref().unwrap().summary, "feat: two");
            assert_eq!(view.message.read(cx).text(), "");
        });
        // The amend hint uses HEAD's full message.
        view.update(&mut cx, |view, cx| view.set_amend(true, cx));
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.message.read(cx).text(), "feat: two\n\nWhy it exists.");
        });
    }
}
