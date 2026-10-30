//! The repository shell: toolbar, sidebar, history, details dock and status bar.
//!
//! One [`RepositoryView`] lives per open repository; the [`Workspace`] owns it
//! and swaps it for the welcome screen. Everything that belongs to a later
//! stage is a placeholder or a disabled control, never a dead click.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, size, uniform_list, AnyElement, App, AppContext, Bounds,
    ClickEvent, ClipboardItem, Context, Decorations, Div, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, ListHorizontalSizingBehavior, MouseButton, ParentElement,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    StyledText, Subscription, TextRun, UniformListScrollHandle, WeakEntity, Window,
};

use git_core::{ChangedFile, CommitDetails, CommitInfo, Error, Git, Head, Oid, RefKind};
use ui::actions::{SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::metrics::{DOCK_HEADER, FONT_MONO, FONT_UI, FS_BASE, FS_MD, FS_SM, MONO, R_SM, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    menu, Avatar, Button, CountPill, FileRow, Icon, IconName, MenuEntry, MenuItem, Scrollbar,
    SearchField, SegmentItem, Segmented, Skeleton, Spinner, Splitter, TextInput, TextInputEvent,
    Tooltip, Tree, TreeNodeData,
};

use crate::changes_view::{ChangesEvent, ChangesView};
use crate::details::{read_details, read_diff, DetailsState, DiffState};
use crate::diff_model::{DiffRow, RowKind, SplitCell, SplitCellKind, SplitRow};
use crate::format::{absolute_time, format_count, relative_time};
use crate::history_state::LoadState;
use crate::history_view::HistoryView;
use crate::settings::{
    sidebar_width_limit, DiffLayout, Dock, SIDEBAR_WIDTH_DEFAULT, SIDEBAR_WIDTH_MAX,
    SIDEBAR_WIDTH_MIN,
};
use crate::sidebar::{
    commit_for, read_sidebar_extras, sidebar_menu, sidebar_nodes, Expansion, SidebarAction,
    SidebarExtras, SidebarMenuEntry,
};
use crate::syntax::{self, HighlightSpan, Highlighted, Token};
use crate::workspace::{
    git_label, git_tooltip, repo_name, titlebar, titlebar_end, titlebar_fill, Workspace,
};

actions!(
    gitwarp,
    [
        ToggleSidebar,
        ToggleDetails,
        FocusSearch,
        SearchNext,
        SearchPrev,
        SearchClear,
        SearchFocusList,
        OpenRepoSwitcher,
        OpenBranchSwitcher
    ]
);

/// The two centre views the toolbar switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    History,
    Changes,
}

/// Binds the search field's own keys in its `history-search` context, so
/// Enter/F3/Esc/↓ never fire while another input has the keyboard. Enter is
/// handled through the input's own `Submit` event instead. Called once at
/// startup.
pub(crate) fn bind_search_keys(cx: &mut App) {
    cx.bind_keys([
        ui::actions::key_binding(cx, "shift-enter", SearchPrev, Some("history-search")),
        ui::actions::key_binding(cx, "f3", SearchNext, Some("history-search")),
        ui::actions::key_binding(cx, "shift-f3", SearchPrev, Some("history-search")),
        ui::actions::key_binding(cx, "escape", SearchClear, Some("history-search")),
        ui::actions::key_binding(cx, "down", SearchFocusList, Some("history-search")),
    ]);
}

/// Height of a toolbar tool button.
const TOOL_H: f32 = 28.;

/// What the toolbar shows at a given window width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarLayout {
    /// Labels on the tool buttons and the segmented view switch.
    pub labels: bool,
    /// The Ctrl K hint inside the search field.
    pub search_kbd: bool,
    /// Widest the repository name may get; `None` hides it.
    pub repo_name: Option<f32>,
    /// Widest the branch name may get; `None` hides it.
    pub branch_name: Option<f32>,
    /// The search field's flex basis.
    pub search_basis: f32,
    /// The search field's minimum width.
    pub search_min: f32,
    /// Gap between the toolbar items.
    pub gap: f32,
    /// Horizontal padding of the bar.
    pub padding: f32,
    /// Horizontal padding of the two switchers and the tool buttons.
    pub item_pad: f32,
}

impl ToolbarLayout {
    fn full() -> Self {
        Self {
            labels: true,
            search_kbd: true,
            repo_name: Some(96.),
            branch_name: Some(96.),
            search_basis: 270.,
            search_min: 150.,
            gap: 6.,
            padding: 12.,
            item_pad: 8.,
        }
    }

    fn compact(self) -> Self {
        Self {
            gap: 4.,
            item_pad: 7.,
            ..self
        }
    }
}

/// The toolbar steps, measured against the real text widths of every item.
///
/// The window's client-side frame keeps a 10px shadow band and a 1px border on
/// each side, so a window of `width` has `width - 22` of content. Every item
/// except the search field is `flex-none`, so the search is the only item that
/// can shrink; the steps below keep the worst case (names at their maximum,
/// `Fetch`/`Pull`/`Push`/`Stash`/`History`/`Changes` labelled) inside the bar.
///
/// The labels are dropped from 1360 down: at 1200 a long repository name plus
/// the segmented labels do not fit even with the search at its minimum, which
/// clipped "Changes".
pub fn toolbar_layout(width: f32) -> ToolbarLayout {
    let full = ToolbarLayout::full();
    if width >= 1360. {
        // The design's own 140px cap on both names.
        ToolbarLayout {
            repo_name: Some(140.),
            branch_name: Some(140.),
            ..full
        }
    } else if width >= 1000. {
        // Icon-only tools and view switch, so the bar fits at 1200.
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            ..full
        }
    } else if width >= 870. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            search_basis: 180.,
            search_min: 120.,
            ..full
        }
    } else if width >= 810. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: Some(64.),
            branch_name: Some(64.),
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
    } else if width >= 760. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: Some(64.),
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
        .compact()
    } else if width >= 700. {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: None,
            search_basis: 96.,
            search_min: 96.,
            ..full
        }
        .compact()
    } else {
        ToolbarLayout {
            labels: false,
            search_kbd: false,
            repo_name: None,
            branch_name: None,
            search_basis: 72.,
            search_min: 72.,
            padding: 8.,
            ..full
        }
        .compact()
    }
}

/// Where the details dock sits: the stored choice, else by window width.
pub fn resolve_dock(setting: Option<Dock>, width: f32) -> Dock {
    setting.unwrap_or(if width >= 1400. {
        Dock::Right
    } else {
        Dock::Bottom
    })
}

/// The default size of the details panel for a dock.
pub fn dock_default(dock: Dock) -> f32 {
    match dock {
        Dock::Bottom => 340.,
        Dock::Right => 640.,
    }
}

/// The smallest and largest size of the details panel for a dock.
pub fn dock_bounds(dock: Dock) -> (f32, f32) {
    match dock {
        Dock::Bottom => (220., 560.),
        Dock::Right => (220., 900.),
    }
}

/// Clamps a stored size to the current dock's range.
pub fn clamp_details(size: f32, dock: Dock) -> f32 {
    let (min, max) = dock_bounds(dock);
    if size.is_finite() {
        size.clamp(min, max)
    } else {
        dock_default(dock)
    }
}

/// A right-docked details panel never takes more than this share of the space
/// right of the sidebar.
const DETAILS_MAX_SHARE: f32 = 0.5;

/// The widest a right-docked details panel may render: half of the space right
/// of the sidebar, within the panel's own 220–900 range.
///
/// `sidebar_width` is the width the sidebar actually renders at (0 while it is
/// collapsed).
pub fn details_width_limit(window_width: f32, sidebar_width: f32) -> f32 {
    let (min, max) = dock_bounds(Dock::Right);
    (DETAILS_MAX_SHARE * (window_width - sidebar_width)).clamp(min, max)
}

/// The width a right-docked details panel renders at: its stored size, capped
/// by [`details_width_limit`] and never below its 220 px minimum.
///
/// The stored size is not rewritten; only the rendered width shrinks.
pub fn effective_details_width(stored: f32, window_width: f32, sidebar_width: f32) -> f32 {
    clamp_details(stored, Dock::Right).min(details_width_limit(window_width, sidebar_width))
}

/// The width the sidebar renders at: its stored width, capped to half the
/// visible area and never below its own 160 px minimum.
///
/// The stored width is kept as saved; only the rendered width shrinks.
pub fn effective_sidebar_width(stored: f32, window_width: f32) -> f32 {
    stored.clamp(SIDEBAR_WIDTH_MIN, sidebar_width_limit(window_width))
}

pub struct RepositoryView {
    /// The path the view was opened for, so the workspace can reopen it.
    path: PathBuf,
    /// Canonical root, for the "current" marker in the repository menu.
    root: PathBuf,
    repo_name: SharedString,
    /// `git 2.56.0 · bundled`, or `git not found`.
    git_label: SharedString,
    /// The informational tooltip of the status-bar git label.
    git_tooltip: SharedString,
    history: Entity<HistoryView>,
    _history_observer: Subscription,
    changes: Entity<ChangesView>,
    _changes_observer: Subscription,
    view: ViewMode,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    search: Entity<TextInput>,
    search_focus: FocusHandle,
    /// The search field's text and submit events, forwarded to the history.
    _search_events: Subscription,
    sidebar_filter: Entity<TextInput>,
    sidebar_open: bool,
    sidebar_width: f32,
    /// The sidebar tree's scroll state, shared with its horizontal scrollbar.
    sidebar_scroll: ScrollHandle,
    /// The sidebar's expansion overrides over the defaults.
    sidebar: Expansion,
    /// The sidebar filter field's text.
    filter: String,
    /// The sidebar's own lists; `None` while they load.
    sidebar_extras: Option<SidebarExtras>,
    /// The root the lists belong to, so they load once per root.
    sidebar_root: Option<PathBuf>,
    /// Bumped per load; an older result that finishes later is dropped.
    sidebar_generation: u64,
    /// The sidebar filter's text events.
    _sidebar_filter_events: Subscription,
    /// The resolved git, for the sidebar's own background reads.
    git: Result<Arc<Git>, Arc<Error>>,
    selected_node: Option<SharedString>,
    dock: Dock,
    details_open: bool,
    details_size: f32,
    /// What the selected commit's details dock shows.
    details: DetailsState,
    /// The commit `details` belongs to; `None` while nothing is selected.
    details_oid: Option<Oid>,
    /// Bumped per details load; an older result that finishes later is dropped.
    details_generation: u64,
    /// The row of `details.files()` the diff pane shows.
    selected_file: Option<usize>,
    /// The selected file's diff.
    diff: DiffState,
    /// Bumped per diff load; an older result that finishes later is dropped.
    diff_generation: u64,
    diff_scroll: UniformListScrollHandle,
    /// Advance of one mono character at [`MONO`], for the diff's scroll width.
    mono_cell: Pixels,
    /// Unified or side-by-side, persisted in the settings.
    diff_layout: DiffLayout,
    /// Syntax spans per (commit, path), so revisiting a file is free.
    highlight_cache: HashMap<(Oid, String), Arc<Highlighted>>,
    info_scroll: ScrollHandle,
    files_scroll: ScrollHandle,
    /// The file list's keyboard focus; arrow keys move the file selection only
    /// while it is focused.
    files_focus: FocusHandle,
    /// Window bounds of the two dropdown anchors, captured while painting.
    repo_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    branch_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl RepositoryView {
    pub fn new(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Canonicalising is filesystem work; the history view reports the
        // real root as soon as `Repo::open` finished and the observer below
        // adopts it. Until then the given path is the best answer.
        let root = path.clone();
        let name = repo_name(&path);
        let git_label = git_label(&git);
        let git_tooltip = git_tooltip(&git);
        let history =
            cx.new(|cx| HistoryView::open(path.clone(), git.clone(), workspace.clone(), cx));
        let observer = cx.observe(&history, |this, history, cx| {
            // A repository opened from a subdirectory is remembered by its
            // root, so the switcher's "current" marker must follow it.
            let (root, selected) = {
                let history = history.read(cx);
                (
                    history.root().map(std::path::Path::to_path_buf),
                    history.selected_commit(),
                )
            };
            if let Some(root) = root {
                if this.root != root {
                    this.root = root.clone();
                    this.repo_name = repo_name(&root);
                }
                // The sidebar's own lists need the root: load them once, when
                // the history first reports it.
                this.ensure_sidebar(cx);
            }
            this.sync_details(selected, cx);
            cx.notify();
        });
        let changes = cx.new(|cx| ChangesView::new(path.clone(), git.clone(), window, cx));
        let changes_observer = cx.subscribe(&changes, |this, _, event: &ChangesEvent, cx| {
            if *event == ChangesEvent::Committed {
                this.history.update(cx, |history, cx| history.reload(cx));
            }
        });
        let search = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Search commits, authors, hashes", cx);
            input
        });
        let search_focus = search.read(cx).focus_handle().clone();
        let search_events = cx.subscribe(&search, |this, _, event: &TextInputEvent, cx| {
            match event {
                TextInputEvent::Changed(text) => {
                    this.history
                        .update(cx, |history, cx| history.set_search_query(text.clone(), cx));
                }
                // Enter in the field: the next match, wrapping.
                TextInputEvent::Submit => {
                    this.history
                        .update(cx, |history, cx| history.search_step(true, cx));
                }
            }
        });
        let sidebar_filter = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Filter branches, tags…", cx);
            input
        });
        let sidebar_filter_events =
            cx.subscribe(&sidebar_filter, |this, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(text) = event {
                    this.filter = text.clone();
                    cx.notify();
                }
            });

        let settings = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        let dock = resolve_dock(settings.dock, f32::from(window.viewport_size().width));
        let details_size = settings
            .details_size
            .map(|size| clamp_details(size, dock))
            .unwrap_or_else(|| dock_default(dock));
        let focus = cx.focus_handle().tab_stop(false);

        Self {
            path,
            root,
            repo_name: name,
            git_label,
            git_tooltip,
            history,
            _history_observer: observer,
            changes,
            _changes_observer: changes_observer,
            view: ViewMode::History,
            workspace,
            focus: focus.clone(),
            search,
            search_focus,
            _search_events: search_events,
            sidebar_filter,
            sidebar_open: true,
            sidebar_width: settings
                .sidebar_width
                .clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX),
            sidebar_scroll: ScrollHandle::new(),
            sidebar: Expansion::default(),
            filter: String::new(),
            sidebar_extras: None,
            sidebar_root: None,
            sidebar_generation: 0,
            _sidebar_filter_events: sidebar_filter_events,
            git,
            selected_node: None,
            dock,
            details_open: true,
            details_size,
            details: DetailsState::Empty,
            details_oid: None,
            details_generation: 0,
            selected_file: None,
            diff: DiffState::Empty,
            diff_generation: 0,
            diff_scroll: UniformListScrollHandle::new(),
            mono_cell: window
                .text_system()
                .ch_advance(
                    window.text_system().resolve_font(&gpui::font(FONT_MONO)),
                    MONO,
                )
                .unwrap_or(px(7.2)),
            diff_layout: settings.diff_layout,
            highlight_cache: HashMap::new(),
            info_scroll: ScrollHandle::new(),
            files_scroll: ScrollHandle::new(),
            files_focus: cx.focus_handle().tab_stop(true),
            repo_bounds: Rc::new(Cell::new(None)),
            branch_bounds: Rc::new(Cell::new(None)),
        }
    }

    /// The focus handle of the repository root; the workspace focuses it after
    /// opening, so the keyboard lands in the new content.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The focus handle of the toolbar search field.
    pub fn search_focus(&self) -> FocusHandle {
        self.search_focus.clone()
    }

    /// The path this view was opened for; reopened after a git retry.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    pub(crate) fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    /// Switches the centre pane and moves the keyboard to the new content.
    fn set_view(&mut self, mode: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == mode {
            return;
        }
        self.view = mode;
        match mode {
            ViewMode::Changes => {
                self.changes.update(cx, |changes, cx| changes.refresh(cx));
                let focus = self.changes.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            ViewMode::History => {
                let focus = self.history.read(cx).list_focus();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    /// The history view, for the search's focus hand-off and the tests.
    #[cfg(test)]
    pub(crate) fn history(&self) -> Entity<HistoryView> {
        self.history.clone()
    }

    /// Enter / F3 in the search field: the next match.
    fn search_next(&mut self, cx: &mut Context<Self>) {
        self.history
            .update(cx, |history, cx| history.search_step(true, cx));
    }

    /// Shift+Enter / Shift+F3: the previous match.
    fn search_prev(&mut self, cx: &mut Context<Self>) {
        self.history
            .update(cx, |history, cx| history.search_step(false, cx));
    }

    /// Esc: clear the field's text (which clears the search) and give the list
    /// the keyboard, keeping its selection.
    fn search_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.history
            .update(cx, |history, cx| history.clear_search(cx));
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
    }

    /// ↓ in the field: hand the keyboard to the list without moving the
    /// selection.
    fn search_focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.history.read(cx).list_focus();
        window.focus(&focus, cx);
    }

    fn set_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        if self.dock != dock {
            self.dock = dock;
            // The size the user dragged stays: it is clamped to the new
            // dock's range when rendered, never overwritten.
        }
        self.details_open = true;
        crate::update_settings(cx, |settings| settings.dock = Some(dock));
        cx.notify();
    }

    fn set_sidebar_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        self.sidebar_width = f32::from(width).clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
        cx.notify();
    }

    fn set_details_size(&mut self, size: Pixels, cx: &mut Context<Self>) {
        self.details_size = clamp_details(f32::from(size), self.dock);
        cx.notify();
    }

    /// Saves both panel sizes once a splitter drag ends.
    fn persist_layout(&mut self, cx: &mut Context<Self>) {
        let sidebar_width = self.sidebar_width;
        let details_size = self.details_size;
        crate::update_settings(cx, |settings| {
            settings.sidebar_width = sidebar_width;
            settings.details_size = Some(details_size);
        });
    }

    fn reset_details_size(&mut self, cx: &mut Context<Self>) {
        self.details_size = dock_default(self.dock);
        let size = self.details_size;
        crate::update_settings(cx, |settings| settings.details_size = Some(size));
        cx.notify();
    }

    /// Flips the expansion of any expandable sidebar row, by key. `current`
    /// is the row's effective state as it was built.
    fn toggle_node(&mut self, key: &str, current: bool, cx: &mut Context<Self>) {
        // While filtering every expandable row is forced open. Recording a
        // flip here would silently change the state restored when the filter
        // is cleared, so expansion toggles are ignored until then.
        if !self.filter.trim().is_empty() {
            return;
        }
        self.sidebar.toggle(key, current);
        cx.notify();
    }

    /// Click / Enter / Space on a leaf: selects it and reveals its commit in
    /// the history. Folders, remotes and sections only toggle, so they never
    /// reach this.
    fn activate_node(&mut self, key: &str, cx: &mut Context<Self>) {
        self.selected_node = Some(key.into());
        self.reveal_node(key, cx);
        cx.notify();
    }

    /// Jumps the history to the commit a leaf points at, when it has one.
    fn reveal_node(&mut self, key: &str, cx: &mut Context<Self>) {
        let oid = {
            let history = self.history.read(cx);
            commit_for(
                key,
                history.refs().unwrap_or(&[]),
                self.sidebar_extras.as_ref(),
            )
        };
        if let Some(oid) = oid {
            self.history
                .update(cx, |history, cx| history.reveal_commit(oid, cx));
        }
    }

    /// Right-click on a sidebar row: selects it (without the jump) and opens
    /// its kind's menu at the pointer. A kind without a menu does nothing.
    fn open_sidebar_menu(
        &mut self,
        key: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = sidebar_menu(
            key,
            self.history.read(cx).head(),
            self.sidebar_extras.as_ref(),
        );
        if model.is_empty() {
            return;
        }
        self.selected_node = Some(key.into());
        let view = cx.entity();
        let entries: Vec<MenuEntry> = model
            .into_iter()
            .map(|entry| match entry {
                SidebarMenuEntry::Separator => MenuEntry::Separator,
                SidebarMenuEntry::Item(item) => {
                    let mut menu_item = MenuItem::new(item.label)
                        .icon(item.icon)
                        .disabled(!item.enabled);
                    if let Some(shortcut) = item.shortcut {
                        menu_item = menu_item.shortcut(shortcut);
                    }
                    if item.danger {
                        menu_item = menu_item.danger();
                    }
                    menu_item = match item.action {
                        SidebarAction::CopyText(text) => menu_item.action(move |_, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        }),
                        SidebarAction::Refresh => {
                            let view = view.clone();
                            menu_item.action(move |_, cx| {
                                view.update(cx, |view, cx| view.refresh(cx));
                            })
                        }
                        SidebarAction::None => menu_item,
                    };
                    MenuEntry::Item(menu_item)
                }
            })
            .collect();
        menu::open_context_menu(position, entries, window, cx);
        cx.notify();
    }

    /// Starts the sidebar's background load when the history first reports a
    /// root. Loading is once per root; `refresh` forces it again.
    fn ensure_sidebar(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self
            .history
            .read(cx)
            .root()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        if self.sidebar_root.as_ref() == Some(&root) {
            return;
        }
        self.start_sidebar(root, cx);
    }

    /// Re-runs the sidebar's background load (the "Refresh list" action).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.sidebar_root.clone().or_else(|| {
            self.history
                .read(cx)
                .root()
                .map(std::path::Path::to_path_buf)
        });
        if let Some(root) = root {
            self.start_sidebar(root, cx);
        }
    }

    /// Reads the four sidebar lists in the background. Each failure is logged
    /// and becomes an empty list for that section, so one failure never hides
    /// the others.
    fn start_sidebar(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.sidebar_root = Some(root.clone());
        self.sidebar_generation = self.sidebar_generation.wrapping_add(1);
        let generation = self.sidebar_generation;
        self.sidebar_extras = None;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::warn!("could not read the sidebar: {error}");
                self.sidebar_extras = Some(SidebarExtras::default());
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let extras = cx
                .background_executor()
                .spawn(async move { read_sidebar_extras(git, &root) })
                .await;
            this.update(cx, |view, cx| {
                view.set_sidebar_extras(generation, extras, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_sidebar_extras(
        &mut self,
        generation: u64,
        extras: SidebarExtras,
        cx: &mut Context<Self>,
    ) {
        // A newer load owns the lists now; this older answer is stale.
        if generation != self.sidebar_generation {
            return;
        }
        self.sidebar_extras = Some(extras);
        cx.notify();
    }

    /// Loads the details when the history selection moved to another commit,
    /// and clears them when nothing is selected.
    fn sync_details(&mut self, selected: Option<Arc<CommitInfo>>, cx: &mut Context<Self>) {
        let Some(commit) = selected else {
            if self.details_oid.take().is_some() {
                self.details_generation = self.details_generation.wrapping_add(1);
                self.details = DetailsState::Empty;
                self.selected_file = None;
                self.diff_generation = self.diff_generation.wrapping_add(1);
                self.diff = DiffState::Empty;
                cx.notify();
            }
            return;
        };
        if self.details_oid == Some(commit.id) {
            return;
        }
        self.start_details(commit.id, cx);
    }

    /// Reads one commit's details and changed files in the background.
    fn start_details(&mut self, oid: Oid, cx: &mut Context<Self>) {
        self.details_oid = Some(oid);
        self.selected_file = None;
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.diff = DiffState::Empty;
        self.details_generation = self.details_generation.wrapping_add(1);
        let generation = self.details_generation;
        self.details = DetailsState::Loading;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.details = DetailsState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_details(git, &root, oid) })
                .await;
            this.update(cx, |view, cx| view.set_details(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_details(
        &mut self,
        generation: u64,
        result: Result<crate::details::CommitDetailsData, String>,
        cx: &mut Context<Self>,
    ) {
        // A newer selection owns the pane now; this older answer is stale.
        if generation != self.details_generation {
            return;
        }
        match result {
            Ok(data) => {
                self.selected_file = (!data.files.is_empty()).then_some(0);
                self.details = DetailsState::Loaded(Box::new(data));
                self.start_diff(cx);
            }
            Err(error) => {
                log::warn!("could not read the commit details: {error}");
                self.details = DetailsState::Failed(error);
            }
        }
        cx.notify();
    }

    /// Selects one changed file and scrolls it into view.
    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let count = self.details.files().map(<[ChangedFile]>::len).unwrap_or(0);
        if index >= count || self.selected_file == Some(index) {
            return;
        }
        self.selected_file = Some(index);
        self.files_scroll.scroll_to_item(index);
        self.start_diff(cx);
        cx.notify();
    }

    /// Moves the file selection by `step`, clamped at the ends.
    fn step_file(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.details.files().map(<[ChangedFile]>::len).unwrap_or(0);
        if count == 0 {
            return;
        }
        let current = self.selected_file.unwrap_or(0) as isize;
        let next = current.saturating_add(step).clamp(0, count as isize - 1) as usize;
        self.select_file(next, cx);
    }

    /// Focuses the file list, so its arrow keys work.
    fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.files_focus, cx);
    }

    /// Reads the selected file's diff in the background.
    fn start_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        self.diff_scroll = UniformListScrollHandle::new();
        self.diff = DiffState::Loading;
        let (Some(oid), Some(file)) = (
            self.details_oid,
            self.selected_file
                .and_then(|index| self.details.files().and_then(|files| files.get(index)))
                .cloned(),
        ) else {
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
        let key = (oid, file.path.clone());
        let cached = self.highlight_cache.get(&key).cloned();
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_diff(git, &root, oid, &file, cached) })
                .await;
            this.update(cx, |view, cx| view.set_diff(generation, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps the syntax spans of a file, dropping the oldest entry past the
    /// cache's small cap.
    fn cache_highlight(&mut self, key: (Oid, String), spans: Arc<Highlighted>) {
        const LIMIT: usize = 64;
        if self.highlight_cache.len() >= LIMIT && !self.highlight_cache.contains_key(&key) {
            if let Some(oldest) = self.highlight_cache.keys().next().cloned() {
                self.highlight_cache.remove(&oldest);
            }
        }
        self.highlight_cache.insert(key, spans);
    }

    /// Switches the diff pane between unified and side-by-side and remembers
    /// the choice.
    fn set_diff_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.diff_layout == layout {
            return;
        }
        self.diff_layout = layout;
        self.diff_scroll = UniformListScrollHandle::new();
        crate::update_settings(cx, |settings| settings.diff_layout = layout);
        cx.notify();
    }

    fn set_diff(
        &mut self,
        generation: u64,
        result: Result<crate::details::DiffData, String>,
        cx: &mut Context<Self>,
    ) {
        // A newer file owns the pane now; this older answer is stale.
        if generation != self.diff_generation {
            return;
        }
        match result {
            Ok(data) => {
                if let (Some(oid), Some(file)) = (
                    self.details_oid,
                    self.selected_file
                        .and_then(|index| self.details.files().and_then(|files| files.get(index)))
                        .cloned(),
                ) {
                    self.cache_highlight((oid, file.path.clone()), data.highlight.clone());
                }
                self.diff = DiffState::Loaded(Arc::new(data));
            }
            Err(error) => {
                log::warn!("could not read the file diff: {error}");
                self.diff = DiffState::Failed(error);
            }
        }
        cx.notify();
    }

    /// Opens the recent-repositories dropdown under the repository switcher.
    pub fn open_repo_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.repo_menu_entries(cx);
        let anchor = self
            .repo_bounds
            .get()
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(200.), px(28.))));
        menu::open_dropdown(anchor, entries, window, cx);
    }

    /// Opens the local-branches dropdown under the branch switcher.
    pub(crate) fn open_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.branch_menu_entries(cx);
        let anchor = self
            .branch_bounds
            .get()
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(200.), px(28.))));
        menu::open_dropdown(anchor, entries, window, cx);
    }

    fn repo_menu_entries(&self, cx: &Context<Self>) -> Vec<MenuEntry> {
        let recent = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.recent.clone())
            .unwrap_or_default();
        let mut entries = Vec::new();
        for repo in recent {
            let name = repo
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| repo.path.display().to_string());
            let current = repo.path == self.root;
            let workspace = self.workspace.clone();
            let path = repo.path.clone();
            entries.push(MenuEntry::Item(
                MenuItem::new(name)
                    .icon(if current {
                        IconName::Check
                    } else {
                        IconName::Folder
                    })
                    .action(move |window, cx| {
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace.open_repository(path.clone(), window, cx);
                            })
                            .ok();
                    }),
            ));
        }
        if !entries.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        let workspace = self.workspace.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new("Open repository…")
                .icon(IconName::Folder)
                .shortcut("Ctrl O")
                .action(move |window, cx| {
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.open_repository_dialog(window, cx);
                        })
                        .ok();
                }),
        ));
        let workspace = self.workspace.clone();
        entries.push(MenuEntry::Item(
            MenuItem::new("Close repository")
                .icon(IconName::X)
                .action(move |window, cx| {
                    workspace
                        .update(cx, |workspace, cx| workspace.show_welcome(window, cx))
                        .ok();
                }),
        ));
        entries
    }

    fn branch_menu_entries(&self, cx: &Context<Self>) -> Vec<MenuEntry> {
        let history = self.history.read(cx);
        let head_branch = match history.head() {
            Some(Head::Branch { name, .. }) => Some(name.as_str()),
            _ => None,
        };
        let Some(refs) = history.refs() else {
            return vec![
                MenuEntry::Item(
                    MenuItem::new("Checkout arrives in a later version")
                        .icon(IconName::Branch)
                        .disabled(true),
                ),
                MenuEntry::Separator,
                MenuEntry::Item(
                    MenuItem::new("Loading branches…")
                        .icon(IconName::Branch)
                        .disabled(true),
                ),
            ];
        };
        // The rows are branch targets for a checkout that does not exist yet;
        // say so before the (disabled) rows instead of leaving them looking
        // broken.
        let mut entries: Vec<MenuEntry> = vec![
            MenuEntry::Item(
                MenuItem::new("Checkout arrives in a later version")
                    .icon(IconName::Branch)
                    .disabled(true),
            ),
            MenuEntry::Separator,
        ];
        entries.extend(
            refs.iter()
                .filter(|r| r.kind == RefKind::LocalBranch)
                .map(|r| {
                    let current = head_branch == Some(r.name.as_str());
                    MenuEntry::Item(
                        MenuItem::new(r.name.clone())
                            .icon(if current {
                                IconName::Check
                            } else {
                                IconName::Branch
                            })
                            // Checkout is stage 7.
                            .disabled(true),
                    )
                }),
        );
        if entries.len() == 2 {
            entries.push(MenuEntry::Item(
                MenuItem::new("No local branches")
                    .icon(IconName::Branch)
                    .disabled(true),
            ));
        }
        entries
    }

    fn branch_label(&self, cx: &Context<Self>) -> SharedString {
        match self.history.read(cx).head() {
            Some(Head::Branch { name, .. }) => name.clone().into(),
            Some(Head::Detached(oid)) => oid.short().into(),
            None => "…".into(),
        }
    }

    fn render_toolbar(
        &mut self,
        layout: ToolbarLayout,
        client: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let branch = self.branch_label(cx);

        let sidebar_toggle = Button::ghost("sidebar-toggle")
            .icon_only(IconName::Sidebar)
            .tooltip_kbd("Toggle sidebar", "Ctrl B")
            .on_click(cx.listener(|view, _, _, cx| view.toggle_sidebar(cx)));

        let repo_click = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_repo_menu(window, cx));
            }
        };
        let repo_switcher = switcher(
            "repo-switcher",
            IconName::Folder,
            Some(self.repo_name.clone()),
            layout.repo_name,
            layout.item_pad,
            ("Switch repository", None),
            self.repo_bounds.clone(),
            repo_click,
            colors,
        );

        let branch_click = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.open_branch_menu(window, cx));
            }
        };
        let branch_switcher = switcher(
            "branch-switcher",
            IconName::Branch,
            Some(branch),
            layout.branch_name,
            layout.item_pad,
            ("Switch branch", Some("Ctrl Shift B")),
            self.branch_bounds.clone(),
            branch_click,
            colors,
        );

        let mut bar = titlebar(cx)
            .gap(px(layout.gap))
            .px(px(layout.padding))
            .child(no_shrink(sidebar_toggle))
            .child(repo_switcher)
            .child(branch_switcher)
            .child(no_shrink(
                div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(colors.border),
            ))
            .child(no_shrink(tool_button(
                "toolbar-fetch",
                IconName::Fetch,
                "Fetch",
                "Fetch — arrives in a later version",
                layout.labels,
            )))
            .child(no_shrink(tool_button(
                "toolbar-pull",
                IconName::Down,
                "Pull",
                "Pull — arrives in a later version",
                layout.labels,
            )))
            .child(no_shrink(tool_button(
                "toolbar-push",
                IconName::Up,
                "Push",
                "Push — arrives in a later version",
                layout.labels,
            )))
            .child(no_shrink(tool_button(
                "toolbar-stash",
                IconName::Stash,
                "Stash",
                "Stash — arrives in a later version",
                layout.labels,
            )))
            .child(titlebar_fill());

        let history_item = if layout.labels {
            SegmentItem::new("History").with_icon(IconName::Lanes)
        } else {
            SegmentItem::icon(IconName::Lanes)
        };
        let changes_item = if layout.labels {
            SegmentItem::new("Changes").with_icon(IconName::Edit)
        } else {
            SegmentItem::icon(IconName::Edit)
        };
        let switch_view = cx.entity();
        bar = bar.child(no_shrink(
            Segmented::new("view-switch")
                .item(history_item)
                .item(changes_item.tooltip("Local changes"))
                .selected(match self.view {
                    ViewMode::History => 0,
                    ViewMode::Changes => 1,
                })
                .on_select(move |index, window, cx| {
                    let mode = if index == 1 {
                        ViewMode::Changes
                    } else {
                        ViewMode::History
                    };
                    switch_view.update(cx, |view, cx| view.set_view(mode, window, cx));
                }),
        ));

        let mut field = SearchField::new("toolbar-search", self.search.clone());
        match self.history.read(cx).search_status() {
            // A query replaces the key hint with its count (or "No matches").
            Some(status) => field = field.trailing_text(status),
            None if layout.search_kbd => field = field.hint("Ctrl K"),
            None => {}
        }
        bar = bar.child(
            div()
                .id("toolbar-search-wrap")
                .flex_basis(px(layout.search_basis))
                .flex_shrink(1.)
                .min_w(px(layout.search_min))
                // The field's own keys live here, so they never fire while
                // the sidebar filter or another input has the keyboard.
                .key_context("history-search")
                .on_action(cx.listener(|view, _: &SearchNext, _, cx| view.search_next(cx)))
                .on_action(cx.listener(|view, _: &SearchPrev, _, cx| view.search_prev(cx)))
                .on_action(cx.listener(|view, _: &SearchClear, window, cx| {
                    view.search_clear(window, cx);
                }))
                .on_action(cx.listener(|view, _: &SearchFocusList, window, cx| {
                    view.search_focus_list(window, cx);
                }))
                .tooltip(Tooltip::with_kbd(
                    "Search commits, authors, hashes",
                    "Ctrl K",
                ))
                .child(field),
        );

        bar = bar.child(no_shrink(
            Button::ghost("settings-button")
                .icon_only(IconName::Gear)
                .disabled(true)
                .tooltip("Settings — arrives in a later version"),
        ));

        titlebar_end(bar, client)
    }

    fn render_sidebar(&mut self, width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let nodes = self.tree_nodes(cx);
        let filtering = !self.filter.trim().is_empty();
        let no_matches = filtering && nodes.is_empty();
        // Every row's identity and expansion, so a click, Enter/Space or
        // ←/→ can toggle any expandable node, not only the sections.
        let rows: Vec<(SharedString, bool, bool)> = nodes
            .iter()
            .map(|node| (node.key.clone(), node.has_children, node.expanded))
            .collect();

        let view = cx.entity();
        let activate = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, _: &mut Window, cx: &mut App| {
                let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    if has_children {
                        view.toggle_node(&key, expanded, cx);
                    } else {
                        view.activate_node(&key, cx);
                    }
                });
            }
        };
        // A right-click selects the row and opens its kind's menu at the
        // pointer; the sidebar keeps the keyboard focus.
        let context_menu = {
            let rows = rows.clone();
            let view = view.clone();
            move |index: usize, position: Point<Pixels>, window: &mut Window, cx: &mut App| {
                let Some((key, _, _)) = rows.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    view.open_sidebar_menu(&key, position, window, cx);
                });
            }
        };

        div()
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            // GPUI flex columns lay their children out from the start instead
            // of stretching them: every row spans the sidebar explicitly, or
            // an auto-width filter field collapses to its icon.
            .items_start()
            .bg(colors.sidebar)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .px(px(10.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .child(SearchField::new(
                        "sidebar-filter",
                        self.sidebar_filter.clone(),
                    )),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(no_matches, |this| {
                        this.child(
                            div()
                                .mx(px(6.))
                                .pl(px(6.))
                                .pr(px(8.))
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("No matches"),
                        )
                    })
                    .when(!no_matches, |this| {
                        this.child(
                            div().w_full().flex_1().min_w_0().min_h_0().child(
                                Tree::new("sidebar-tree", nodes)
                                    .horizontal_scroll()
                                    .scroll_handle(self.sidebar_scroll.clone())
                                    .on_toggle(node_toggler(rows.clone(), view.clone()))
                                    .on_expand(node_toggler(rows.clone(), view.clone()))
                                    .on_collapse(node_toggler(rows, view))
                                    .on_activate(activate)
                                    .on_context_menu(context_menu),
                            ),
                        )
                        .child(Scrollbar::horizontal(
                            "sidebar-scrollbar",
                            self.sidebar_scroll.clone(),
                        ))
                    }),
            )
    }

    fn tree_nodes(&self, cx: &Context<Self>) -> Vec<TreeNodeData> {
        let history = self.history.read(cx);
        sidebar_nodes(
            history.refs(),
            history.head(),
            self.sidebar_extras.as_ref(),
            &self.sidebar,
            &self.filter,
            self.selected_node.as_deref(),
        )
    }

    fn render_sidebar_splitter(
        &mut self,
        width: f32,
        max: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let reset_view = view.clone();
        let persist_view = view.clone();
        Splitter::vertical("sidebar-splitter")
            .size(px(width))
            .bounds(px(SIDEBAR_WIDTH_MIN), px(max))
            .on_resize(move |size, _, cx| {
                view.update(cx, |view, cx| view.set_sidebar_width(size, cx));
            })
            .on_resize_end(move |_, cx| {
                persist_view.update(cx, |view, cx| view.persist_layout(cx));
            })
            .on_reset(move |_, cx| {
                reset_view.update(cx, |view, cx| {
                    view.set_sidebar_width(px(SIDEBAR_WIDTH_DEFAULT), cx);
                    view.persist_layout(cx);
                });
            })
    }

    fn render_details_splitter(
        &mut self,
        width: f32,
        limit: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (min, _) = dock_bounds(self.dock);
        let size = px(width);
        let splitter = match self.dock {
            Dock::Bottom => Splitter::horizontal("details-splitter"),
            Dock::Right => Splitter::vertical("details-splitter"),
        };
        let view = cx.entity();
        let reset_view = view.clone();
        let persist_view = view.clone();
        splitter
            .size(size)
            .bounds(px(min), px(limit))
            // A panel docked bottom or right grows as the pointer moves up or
            // left.
            .inverted(true)
            .on_resize(move |size, _, cx| {
                view.update(cx, |view, cx| view.set_details_size(size, cx));
            })
            .on_resize_end(move |_, cx| {
                persist_view.update(cx, |view, cx| view.persist_layout(cx));
            })
            .on_reset(move |_, cx| {
                reset_view.update(cx, |view, cx| view.reset_details_size(cx));
            })
    }

    fn render_details(&mut self, size: f32, main_width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut panel = div().flex_none().flex().flex_col().bg(colors.bg);
        panel = match self.dock {
            Dock::Bottom => panel.w_full().h(px(size)).border_t_1(),
            Dock::Right => panel.h_full().w(px(size)).border_l_1(),
        };
        panel
            .border_color(colors.border)
            .child(self.render_details_header(cx))
            .child(self.render_details_body(size, main_width, colors, cx))
    }

    fn render_details_header(&mut self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let short = self.history.read(cx).selected_short();
        let dock = self.dock;
        let view = cx.entity();
        let switch = Segmented::new("dock-switch")
            .small()
            .item(
                SegmentItem::icon(IconName::PanelR)
                    .id("dock-right")
                    .tooltip("Dock details to the right"),
            )
            .item(
                SegmentItem::icon(IconName::PanelB)
                    .id("dock-bottom")
                    .tooltip("Dock details to the bottom"),
            )
            .selected(if dock == Dock::Right { 0 } else { 1 })
            .on_select(move |index, _, cx| {
                let dock = if index == 0 {
                    Dock::Right
                } else {
                    Dock::Bottom
                };
                view.update(cx, |view, cx| view.set_dock(dock, cx));
            });

        div()
            .h(DOCK_HEADER)
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .font_weight(FontWeight::SEMIBOLD)
            .child("Commit")
            .when_some(short, |this, hash| {
                this.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.muted)
                        .child(hash),
                )
            })
            .child(div().flex_1())
            .child(no_shrink(switch))
    }

    fn render_details_body(
        &mut self,
        size: f32,
        main_width: f32,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let diff_width = match self.dock {
            Dock::Bottom => (main_width - 560.).max(0.),
            Dock::Right => size,
        };
        match self.dock {
            Dock::Bottom => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .child(
                    self.render_info_pane(colors, cx)
                        .w(px(310.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(
                    self.render_files_pane(colors, cx)
                        .w(px(250.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(
                    self.render_diff_pane(diff_width, colors, cx)
                        .flex_1()
                        .min_w_0()
                        .h_full(),
                ),
            Dock::Right => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    self.render_info_pane(colors, cx)
                        .h(px(230.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(
                    self.render_files_pane(colors, cx)
                        .h(px(160.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(
                    self.render_diff_pane(diff_width, colors, cx)
                        .flex_1()
                        .w_full(),
                ),
        }
    }

    /// The commit's message and metadata, scrolling when it is long.
    fn render_info_pane(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let content = self.render_info_content(colors, cx);
        let viewport = div()
            .id("details-info-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.info_scroll)
            .child(content);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .border_color(colors.border)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(viewport)
                    .child(Scrollbar::vertical(
                        "details-info-bar",
                        self.info_scroll.clone(),
                    )),
            )
    }

    fn render_info_content(&self, colors: ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        match &self.details {
            DetailsState::Empty => muted_note("Select a commit", colors).into_any_element(),
            DetailsState::Loading => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .px(px(18.))
                .py(px(16.))
                .child(Skeleton::new("details-skeleton-title", px(190.)))
                .child(Skeleton::new("details-skeleton-body", px(240.)))
                .child(Skeleton::new("details-skeleton-meta", px(150.)))
                .into_any_element(),
            DetailsState::Failed(message) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(18.))
                .py(px(16.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Could not load the commit"),
                )
                .child(div().text_color(colors.error).child(message.clone()))
                .into_any_element(),
            DetailsState::Loaded(data) => self.render_loaded_info(&data.details, cx),
        }
    }

    fn render_loaded_info(&self, details: &CommitDetails, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let now = crate::history_view::unix_now();
        let mut column = div().flex().flex_col().px(px(18.)).py(px(16.)).child(
            div()
                .mb(px(8.))
                .text_size(FS_MD)
                .line_height(px(19.))
                .font_weight(FontWeight::BOLD)
                .child(details.summary.clone()),
        );
        if !details.body.is_empty() {
            column = column.child(
                div()
                    .mb(px(8.))
                    .text_size(FS_BASE)
                    .child(details.body.clone()),
            );
        }

        let author = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                Avatar::new(details.author_email.clone(), px(20.))
                    .name(details.author_name.clone()),
            )
            .child(div().flex_1().min_w_0().truncate().child(format!(
                "{} <{}>",
                details.author_name, details.author_email
            )));
        let committer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                Avatar::new(details.committer_email.clone(), px(20.))
                    .name(details.committer_name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(details.committer_name.clone()),
            );
        let authored = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(absolute_time(details.author_time))
            .child(div().text_color(colors.muted).child(format!(
                "({})",
                relative_time(now, details.author_time.max(0) as u64)
            )));
        let full_hash = details.id.to_string();
        let mut hash = full_hash.clone();
        hash.truncate(20);
        hash.push('…');
        let hash = div()
            .id("details-hash")
            .font_family(FONT_MONO)
            .text_size(FS_SM)
            .truncate()
            .tooltip(Tooltip::text(full_hash))
            .child(hash);

        let parents = if details.parents.is_empty() {
            div().text_color(colors.muted).child("none (root commit)")
        } else {
            let view = cx.entity();
            let mut row = div().flex().items_center().gap(px(6.));
            for (index, parent) in details.parents.iter().copied().enumerate() {
                let view = view.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("details-parent-{index}")))
                        .debug_selector(move || format!("details-parent-{index}"))
                        .font_family(FONT_MONO)
                        .text_size(FS_SM)
                        .text_color(colors.accent)
                        .cursor_pointer()
                        .hover(move |style| style.text_color(colors.accent_hover))
                        .child({
                            let mut hash = parent.to_string();
                            hash.truncate(7);
                            hash
                        })
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.history.update(cx, |history, cx| {
                                    history.reveal_commit(parent, cx);
                                });
                            });
                        }),
                );
            }
            row
        };

        column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(FS_SM)
                    .child(meta_row("Author", author, colors))
                    .child(meta_row("Authored", authored, colors))
                    .child(meta_row("Committer", committer, colors))
                    .child(meta_row("Hash", hash, colors))
                    .child(meta_row("Parents", parents, colors)),
            )
            .into_any_element()
    }

    /// The changed-file header and rows.
    fn render_files_pane(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let count = self.details.files().map(<[ChangedFile]>::len);
        let is_merge = self
            .details
            .details()
            .is_some_and(|details| details.parents.len() > 1);
        let mut header = div()
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text)
                    .child("Changed files"),
            );
        if let Some(count) = count {
            header = header.child(CountPill::new(count.to_string()));
        }
        header = header.child(div().flex_1());
        if is_merge {
            header = header.child(
                div()
                    .id("details-vs-parent")
                    .tooltip(Tooltip::text("Combined diff against first parent"))
                    .child("vs parent 1"),
            );
        }

        let body = self.render_files_body(colors, cx);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .border_color(colors.border)
            .child(header)
            .child(body)
    }

    fn render_files_body(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let Some(files) = self.details.files() else {
            return match &self.details {
                DetailsState::Loading => {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(8.))
                        .children((0..4).map(|index| {
                            Skeleton::new(
                                SharedString::from(format!("details-files-skeleton-{index}")),
                                px(180.),
                            )
                        }))
                }
                DetailsState::Failed(message) => div()
                    .p(px(12.))
                    .text_color(colors.error)
                    .child(message.clone()),
                _ => muted_note("No files changed", colors),
            };
        };
        if files.is_empty() {
            return muted_note("No files changed", colors);
        }

        let view = cx.entity();
        let selected = self.selected_file;
        let mut list = div()
            .id("details-files-list")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.files_scroll)
            .track_focus(&self.files_focus)
            .key_context("details-files")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| view.focus_files(window, cx)),
            )
            .on_action(cx.listener(|view, _: &SelectNext, _, cx| view.step_file(1, cx)))
            .on_action(cx.listener(|view, _: &SelectPrev, _, cx| view.step_file(-1, cx)))
            .on_action(cx.listener(|view, _: &SelectFirst, _, cx| view.step_file(isize::MIN, cx)))
            .on_action(cx.listener(|view, _: &SelectLast, _, cx| view.step_file(isize::MAX, cx)));
        for (index, file) in files.iter().enumerate() {
            let view = view.clone();
            list = list.child(
                div()
                    .debug_selector(move || format!("details-file-{index}"))
                    .child(
                        FileRow::new(
                            SharedString::from(format!("details-file-{index}")),
                            file.kind.letter(),
                            file.path.clone(),
                        )
                        .added(file.added)
                        .deleted(file.deleted)
                        .selected(selected == Some(index))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| view.select_file(index, cx));
                        }),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(list)
            .child(Scrollbar::vertical(
                "details-files-bar",
                self.files_scroll.clone(),
            ))
    }

    /// The diff toolbar and the selected file's unified diff.
    fn render_diff_pane(&mut self, width: f32, colors: ThemeColors, cx: &mut Context<Self>) -> Div {
        let path = self
            .selected_file
            .and_then(|index| self.details.files().and_then(|files| files.get(index)))
            .map(|file| match &file.old_path {
                Some(old) => format!("{} ← {old}", file.path),
                None => file.path.clone(),
            });
        let mut toolbar = div()
            .id("details-diff-toolbar")
            .h(px(34.))
            .flex_none()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(colors.border);
        if let Some(path) = path {
            toolbar = toolbar.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(FS_SM)
                    .text_color(colors.text)
                    .child(path),
            );
        } else {
            toolbar = toolbar.child(div().flex_1());
        }

        let layout = self.diff_layout;
        let view = cx.entity();
        let switch = Segmented::new("diff-layout")
            .small()
            .item(
                SegmentItem::new("Unified")
                    .with_icon(IconName::Unified)
                    .id("diff-unified"),
            )
            .item(
                SegmentItem::new("Side-by-side")
                    .with_icon(IconName::Split)
                    .id("diff-split"),
            )
            .selected(if layout == DiffLayout::Split { 1 } else { 0 })
            .on_select(move |index, _, cx| {
                let layout = if index == 1 {
                    DiffLayout::Split
                } else {
                    DiffLayout::Unified
                };
                view.update(cx, |view, cx| view.set_diff_layout(layout, cx));
            });
        toolbar = toolbar.child(no_shrink(switch));

        let body = match &self.diff {
            DiffState::Empty => diff_note("Select a file", colors),
            DiffState::Loading => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .child(Skeleton::new("diff-skeleton-1", px(320.)))
                .child(Skeleton::new("diff-skeleton-2", px(260.)))
                .child(Skeleton::new("diff-skeleton-3", px(290.))),
            DiffState::Failed(message) if message.contains("too large") => {
                diff_note("Diff too large to display", colors)
            }
            DiffState::Failed(message) => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .text_color(colors.error)
                .child(message.clone()),
            DiffState::Loaded(data) => {
                if data.diff.binary {
                    diff_note("Binary file", colors)
                } else if data.rows.is_empty() {
                    diff_note("No changes", colors)
                } else {
                    let data = data.clone();
                    let list = if layout == DiffLayout::Split {
                        let row_width = split_width(self.mono_cell, data.columns, width);
                        uniform_list(
                            "details-diff",
                            data.split_rows.len(),
                            cx.processor(move |view, range, window, cx| {
                                view.render_split_rows(
                                    &data.split_rows,
                                    &data.highlight,
                                    range,
                                    row_width,
                                    window,
                                    cx,
                                )
                            }),
                        )
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .track_scroll(&self.diff_scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .into_any_element()
                    } else {
                        let row_width = diff_width(self.mono_cell, data.columns, width);
                        uniform_list(
                            "details-diff",
                            data.rows.len(),
                            cx.processor(move |view, range, window, cx| {
                                view.render_diff_rows(
                                    &data.rows,
                                    &data.highlight,
                                    range,
                                    row_width,
                                    window,
                                    cx,
                                )
                            }),
                        )
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .track_scroll(&self.diff_scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .into_any_element()
                    };
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(list)
                        .child(Scrollbar::uniform(
                            "details-diff-bar",
                            self.diff_scroll.clone(),
                        ))
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .text_size(MONO)
            .child(toolbar)
            .child(body)
    }

    fn render_diff_rows(
        &mut self,
        rows: &[DiffRow],
        highlight: &Highlighted,
        range: std::ops::Range<usize>,
        width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        range
            .filter_map(|index| {
                rows.get(index).map(|row| {
                    let (old_spans, new_spans) = match row.kind {
                        RowKind::Removed => (highlight.spans(true, row.old_line), &[][..]),
                        RowKind::Added => (&[][..], highlight.spans(false, row.new_line)),
                        _ => {
                            let spans = if row.old_line.is_some() {
                                highlight.spans(true, row.old_line)
                            } else {
                                highlight.spans(false, row.new_line)
                            };
                            (spans, &[][..])
                        }
                    };
                    diff_row(row, index, width, colors, old_spans, new_spans)
                })
            })
            .collect()
    }

    fn render_split_rows(
        &mut self,
        rows: &[SplitRow],
        highlight: &Highlighted,
        range: std::ops::Range<usize>,
        width: Pixels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.theme().colors;
        let half = px(((f32::from(width) - 2. * DIFF_GUTTER) / 2.).max(0.));
        range
            .filter_map(|index| {
                rows.get(index)
                    .map(|row| split_row(row, index, width, half, colors, highlight))
            })
            .collect()
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut bar = div()
            .h(STATUS)
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(12.))
            .bg(colors.bg)
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted);

        if self.view == ViewMode::Changes {
            let changes = self.changes.read(cx);
            if changes.is_loading() {
                bar = bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(Spinner::new("status-spinner", px(12.)))
                        .child("Loading changes…"),
                );
            } else if let Some(error) = changes.failure() {
                bar = bar.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.error)
                        .child(error.to_string()),
                );
            }
            bar = bar.child(div().flex_1());
            bar = bar.child(format!(
                "{} unstaged · {} staged",
                changes.unstaged_count(),
                changes.staged_count()
            ));
        } else {
            let history = self.history.read(cx);
            let mut fill = true;
            match history.state() {
                LoadState::Loading { .. } => {
                    bar = bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(Spinner::new("status-spinner", px(12.)))
                            .child("Loading history…"),
                    );
                }
                LoadState::Cancelled => {
                    bar = bar.child(div().text_color(colors.muted).child("Loading cancelled"));
                }
                LoadState::Failed(failure) => {
                    bar = bar.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.error)
                            .child(failure.title()),
                    );
                    fill = false;
                }
                LoadState::Loaded => {}
            }
            if fill {
                bar = bar.child(div().flex_1());
            }
            bar = bar.child(format!(
                "{} commits loaded",
                format_count(history.commit_count())
            ));
        }

        bar.child(
            div()
                .id("status-git")
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(R_SM)
                .text_color(colors.muted)
                .tooltip(Tooltip::text(self.git_tooltip.clone()))
                .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                .child(self.git_label.clone()),
        )
    }
}

impl Render for RepositoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let colors = cx.theme().colors;
        let width = f32::from(window.viewport_size().width);
        let layout = toolbar_layout(width);

        // Until the user picks a side, the dock follows the window width, so
        // a window resized past the breakpoint docks the details right. The
        // dragged size is kept across the flip and only clamped for the dock
        // that renders it.
        let auto_dock = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.dock.is_none())
            .unwrap_or(false);
        if auto_dock {
            let dock = resolve_dock(None, width);
            if dock != self.dock {
                self.dock = dock;
            }
        }

        let toolbar = self.render_toolbar(layout, client, cx);

        // The sidebar may take at most half of the visible area. Only the
        // rendered width is capped; the stored width stays as saved.
        let sidebar_width = if self.sidebar_open {
            effective_sidebar_width(self.sidebar_width, width)
        } else {
            0.
        };
        let details_limit = match self.dock {
            Dock::Right => details_width_limit(width, sidebar_width),
            Dock::Bottom => dock_bounds(Dock::Bottom).1,
        };
        let details_width = if self.details_open {
            match self.dock {
                Dock::Right => effective_details_width(self.details_size, width, sidebar_width),
                Dock::Bottom => clamp_details(self.details_size, Dock::Bottom),
            }
        } else {
            0.
        };
        // The splitter may not drag the sidebar past half the window either.
        let sidebar_limit = sidebar_width_limit(width);

        let sidebar = self.sidebar_open.then(|| {
            (
                self.render_sidebar(sidebar_width, cx),
                self.render_sidebar_splitter(sidebar_width, sidebar_limit, cx),
            )
        });
        // The local-changes view has its own diff pane; the commit details
        // dock belongs to history only.
        let details_open = self.details_open && self.view == ViewMode::History;
        let dock = self.dock;
        let splitter =
            details_open.then(|| self.render_details_splitter(details_width, details_limit, cx));
        let panel = details_open
            .then(|| self.render_details(details_width, width - sidebar_width - 5., cx));
        let status = self.render_status_bar(cx);

        let content = match self.view {
            ViewMode::History => self.history.clone().into_any_element(),
            ViewMode::Changes => self.changes.clone().into_any_element(),
        };
        let history_pane = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(content);

        let main = match (details_open, dock) {
            (false, _) => div().flex_1().min_w_0().flex().child(history_pane),
            (true, Dock::Bottom) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(history_pane)
                .when_some(splitter, |this, splitter| this.child(splitter))
                .when_some(panel, |this, panel| this.child(panel)),
            (true, Dock::Right) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .child(history_pane)
                .when_some(splitter, |this, splitter| this.child(splitter))
                .when_some(panel, |this, panel| this.child(panel)),
        };

        let mut body = div().flex_1().min_h_0().w_full().flex();
        if let Some((sidebar, splitter)) = sidebar {
            body = body.child(sidebar).child(splitter);
        }
        let body = body.child(main);

        div()
            .id("repository-view")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .key_context("repository")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &ToggleSidebar, _, cx| view.toggle_sidebar(cx)))
            .on_action(cx.listener(|view, _: &ToggleDetails, _, cx| view.toggle_details(cx)))
            .on_action(cx.listener(|view, _: &FocusSearch, window, cx| {
                let handle = view.search_focus();
                window.focus(&handle, cx);
                // Ctrl+K selects the field's text, so typing replaces it.
                view.search.update(cx, |input, cx| input.select_all(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &OpenRepoSwitcher, window, cx| {
                view.open_repo_menu(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenBranchSwitcher, window, cx| {
                view.open_branch_menu(window, cx);
            }))
            .child(toolbar)
            .child(body)
            .child(status)
    }
}

/// A toolbar item with rich content: icon · bold name · chevron.
///
/// `Button` has no rich-content variant, and the design's two switchers are
/// custom `btn-tool` rows, so this builds just the row (hover, tooltip and
/// click) rather than re-implementing the button component.
#[allow(clippy::too_many_arguments)]
fn switcher(
    id: &'static str,
    icon: IconName,
    name: Option<SharedString>,
    name_max: Option<f32>,
    padding: f32,
    tooltip: (&'static str, Option<&'static str>),
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    colors: ThemeColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .relative()
        .h(px(TOOL_H))
        .px(px(padding))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(R_SM)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(Icon::new(icon).size(px(14.)).color(colors.muted));
    row = match tooltip.1 {
        Some(kbd) => row.tooltip(Tooltip::with_kbd(tooltip.0, kbd)),
        None => row.tooltip(Tooltip::text(tooltip.0)),
    };
    if let (Some(name), Some(max)) = (name, name_max) {
        row = row.child(
            div()
                .max_w(px(max))
                .truncate()
                .font_weight(FontWeight::BOLD)
                .child(name),
        );
    }
    row.child(Icon::new(IconName::ChevD).size(px(12.)).color(colors.muted))
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |_, painted, _, _| bounds.set(Some(painted)),
            )
            .absolute()
            .inset_0(),
        )
}

/// A toolbar item that must keep its width: only the search field may shrink.
fn no_shrink(child: impl IntoElement) -> Div {
    div().flex_none().child(child)
}

/// One disabled tool button; the label is dropped on narrow windows and the
/// tooltip keeps the name and says the action is not available yet.
fn tool_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    tooltip: &'static str,
    labels: bool,
) -> Button {
    let button = Button::tool(id).icon(icon).disabled(true).tooltip(tooltip);
    if labels {
        button.label(label)
    } else {
        button
    }
}

/// One diff row and its cells, in pixels.
const DIFF_ROW: f32 = 22.;
const DIFF_GUTTER: f32 = 44.;
const DIFF_MARKER: f32 = 16.;

/// A muted, centred message for a diff that has nothing to paint.
fn diff_note(text: &'static str, colors: ThemeColors) -> Div {
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

/// The width of one diff row: its gutters plus the widest line, or the pane
/// width when the lines are shorter, so a line's background fills the pane.
fn diff_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let text = columns as f32 * f32::from(cell) + 24.;
    px((DIFF_GUTTER * 2. + DIFF_MARKER + text).max(pane))
}

/// One unified-view row: a hunk header, a numbered line or a no-newline note.
fn diff_row(
    row: &DiffRow,
    index: usize,
    width: Pixels,
    colors: ThemeColors,
    old_spans: &[HighlightSpan],
    new_spans: &[HighlightSpan],
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("diff-row-{index}")))
        .debug_selector(move || format!("diff-row-{index}"))
        .h(px(DIFF_ROW))
        .w(width)
        .flex_none()
        .flex()
        .items_center()
        .whitespace_nowrap();
    match row.kind {
        RowKind::Hunk => base
            .px(px(8.))
            .bg(colors.diff_hunk_bg)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(row.text.clone())
            .into_any_element(),
        RowKind::Note => base
            .px(px(8.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(row.text.clone())
            .into_any_element(),
        RowKind::Context | RowKind::Added | RowKind::Removed => {
            let (old_background, new_background, line_background, strong) = match row.kind {
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
            let spans = match row.kind {
                RowKind::Removed => old_spans,
                RowKind::Added => new_spans,
                _ => old_spans,
            };
            base.child(diff_gutter(row.old_line, old_background, colors))
                .child(diff_gutter(row.new_line, new_background, colors))
                .child(diff_marker(row.kind, line_background, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl(px(6.))
                        .pr(px(6.))
                        .when_some(line_background, |this, background| this.bg(background))
                        .text_color(colors.text)
                        .child(styled_diff_text(
                            &row.text,
                            &row.emphasis,
                            spans,
                            colors,
                            strong,
                        )),
                )
                .into_any_element()
        }
    }
}

/// One 44 px line-number gutter; an empty cell when the line is not on that
/// side of the diff.
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
        .text_size(MONO)
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

/// The width of one side-by-side row: two gutters and two equal text halves.
fn split_width(cell: Pixels, columns: usize, pane: f32) -> Pixels {
    let half = columns as f32 * f32::from(cell) + 12.;
    px((2. * DIFF_GUTTER + 2. * half).max(pane))
}

/// One side-by-side row: a hunk header, a note, or an old/new cell pair.
fn split_row(
    row: &SplitRow,
    index: usize,
    width: Pixels,
    half: Pixels,
    colors: ThemeColors,
    highlight: &Highlighted,
) -> AnyElement {
    let base = div()
        .id(SharedString::from(format!("split-row-{index}")))
        .debug_selector(move || format!("split-row-{index}"))
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
        SplitRow::Pair { old, new } => base
            .child(split_gutter(old.as_ref(), colors))
            .child(split_content(
                old.as_ref(),
                half,
                colors,
                highlight.spans(true, old.as_ref().map(|cell| cell.line)),
            ))
            .child(
                split_gutter(new.as_ref(), colors)
                    .border_l_1()
                    .border_color(colors.border),
            )
            .child(split_content(
                new.as_ref(),
                half,
                colors,
                highlight.spans(false, new.as_ref().map(|cell| cell.line)),
            ))
            .into_any_element(),
    }
}

/// A split row's line-number gutter; a missing side stays transparent.
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
        .text_size(MONO)
        .text_color(colors.muted)
        .child(cell.map(|cell| cell.line.to_string()).unwrap_or_default())
}

/// One half of a split row. A missing side is a hatched filler.
fn split_content(
    cell: Option<&SplitCell>,
    width: Pixels,
    colors: ThemeColors,
    spans: &[HighlightSpan],
) -> Div {
    let content = div()
        .w(width)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .pl(px(6.))
        .pr(px(6.))
        .overflow_hidden();
    let Some(cell) = cell else {
        return content.bg(gpui::pattern_slash(colors.active, 1., 5.));
    };
    let (background, strong) = match cell.kind {
        SplitCellKind::Added => (Some(colors.diff_add_bg), colors.diff_add_strong),
        SplitCellKind::Removed => (Some(colors.diff_del_bg), colors.diff_del_strong),
        SplitCellKind::Context => (None, colors.diff_add_strong),
    };
    content
        .when_some(background, |this, background| this.bg(background))
        .text_color(colors.text)
        .child(styled_diff_text(
            &cell.text,
            &cell.emphasis,
            spans,
            colors,
            strong,
        ))
}

/// A line's text with the word-diff ranges painted over the line background.
///
/// GPUI text runs carry the background but no corner radius, so the design's
/// 2 px radius is dropped, like the history search's highlight.
fn styled_diff_text(
    text: &str,
    emphasis: &[std::ops::Range<usize>],
    spans: &[HighlightSpan],
    colors: ThemeColors,
    background: Hsla,
) -> StyledText {
    if emphasis.is_empty() && spans.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    let font = gpui::font(FONT_MONO);
    let runs = syntax::line_segments(text, spans, emphasis)
        .into_iter()
        .map(|segment| {
            let color = segment
                .token
                .map(|token| token_color(token, colors))
                .unwrap_or(colors.text);
            diff_text_run(
                segment.range.len(),
                &font,
                color,
                segment.changed.then_some(background),
            )
        })
        .collect::<Vec<_>>();
    if runs.is_empty() {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
}

/// The theme colour of a syntax token.
fn token_color(token: Token, colors: ThemeColors) -> Hsla {
    match token {
        Token::Keyword => colors.code_kw,
        Token::String => colors.code_str,
        Token::Number => colors.code_num,
        Token::Comment => colors.code_com,
        Token::Type => colors.code_ty,
        Token::Function => colors.code_fn,
    }
}

fn diff_text_run(len: usize, font: &gpui::Font, color: Hsla, background: Option<Hsla>) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: background,
        underline: None,
        strikethrough: None,
    }
}

/// A muted one-line note, for an empty pane or a cleared selection.
fn muted_note(label: &'static str, colors: ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .p(px(16.))
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(label)
}

/// One label/value row of the details metadata grid.
fn meta_row(label: &'static str, value: impl IntoElement, colors: ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(74.))
                .flex_none()
                .text_color(colors.muted)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// A click, Enter/Space or ←/→ on an expandable row flips its expansion.
fn node_toggler(
    rows: Vec<(SharedString, bool, bool)>,
    view: Entity<RepositoryView>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    move |index, _, cx| {
        let Some((key, has_children, expanded)) = rows.get(index).cloned() else {
            return;
        };
        if !has_children {
            return;
        }
        view.update(cx, |view, cx| view.toggle_node(&key, expanded, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    /// Builds the real window (the workspace frame around the repository
    /// view) in a test window and returns the bounds of the sidebar filter.
    fn sidebar_filter_bounds(cx: &mut gpui::TestAppContext) -> gpui::Bounds<Pixels> {
        use gpui::{
            point, size, Bounds, VisualTestContext, WindowBackgroundAppearance, WindowBounds,
            WindowDecorations, WindowOptions,
        };

        cx.update(ui::init).expect("ui::init");
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1600.), px(1000.)),
                    ))),
                    // The app always asks for client-side decorations.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        crate::workspace::Workspace::new(
                            Some(PathBuf::from("/nonexistent")),
                            Err(Arc::new(Error::PathNotFound {
                                path: PathBuf::from("/nonexistent"),
                            })),
                            git_core::GitChoice::Auto,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        cx.debug_bounds("sidebar-filter")
            .expect("the sidebar filter was not drawn")
    }

    #[gpui::test]
    fn the_sidebar_filter_spans_the_sidebar(cx: &mut gpui::TestAppContext) {
        let bounds = sidebar_filter_bounds(cx);
        // The sidebar is 240 wide with a 1px right border and 10px padding
        // on each side of the filter area.
        assert_eq!(
            f32::from(bounds.size.width),
            SIDEBAR_WIDTH_DEFAULT - 1. - 20.,
            "the filter field collapsed instead of spanning the sidebar"
        );
        assert_eq!(f32::from(bounds.size.height), 26.);
    }

    #[test]
    fn toolbar_keeps_the_window_controls_visible_at_every_width() {
        // Inter 13px, generously rounded up per character: the prototype's own
        // measurement is 51px for "gitwarp" (7.3px/char).
        fn text(chars: usize) -> f32 {
            chars as f32 * 8.
        }
        // Every item at its worst case, in the same terms as the toolbar.
        fn needed(layout: ToolbarLayout) -> f32 {
            let name_item = |name: Option<f32>| {
                let name = name.unwrap_or(0.);
                2. * layout.item_pad + 14. + 6. + name + 6. + 12.
            };
            // Button: 8px padding each side, 14px icon, 6px gap.
            let tools = if layout.labels {
                4. * (16. + 14. + 6.)
                    + text("Fetch".len())
                    + text("Pull".len())
                    + text("Push".len())
                    + text("Stash".len())
            } else {
                // Icon-only tool buttons are 28 wide (`Button` squares them).
                112.
            };
            // Segment: 10px padding each side, 14px icon, 6px gap, 1px border.
            let segmented = if layout.labels {
                2. * (20. + 14. + 6.) + text("History".len()) + text("Changes".len()) + 2.
            } else {
                70.
            };
            let fixed = 26. // sidebar toggle
                + name_item(layout.repo_name)
                + name_item(layout.branch_name)
                + 9. // separator
                + tools
                + segmented
                + 26. // settings
                + 104.; // window controls
            fixed + layout.search_min + 2. * layout.padding + 12. * layout.gap
        }

        for width in [
            620., 700., 760., 810., 870., 1000., 1200., 1360., 1400., 1600.,
        ] {
            let layout = toolbar_layout(width);
            // The client-side frame keeps a 10px shadow band and a 1px border
            // on each side.
            let content = width - 22.;
            assert!(
                needed(layout) <= content,
                "toolbar overflows at {width}: needs {} of {content}",
                needed(layout)
            );
        }
    }

    #[test]
    fn toolbar_steps_hide_labels_and_names_in_order() {
        assert!(toolbar_layout(1600.).labels);
        assert!(toolbar_layout(1360.).labels);
        assert!(!toolbar_layout(1359.).labels);
        assert!(!toolbar_layout(1200.).labels);
        assert_eq!(toolbar_layout(1600.).repo_name, Some(140.));
        assert_eq!(toolbar_layout(1200.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(1000.).repo_name, Some(96.));
        assert_eq!(toolbar_layout(900.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(820.).repo_name, Some(64.));
        assert_eq!(toolbar_layout(800.).repo_name, None);
        assert_eq!(toolbar_layout(800.).branch_name, Some(64.));
        assert_eq!(toolbar_layout(720.).branch_name, None);
        assert_eq!(toolbar_layout(650.).search_min, 72.);
        assert_eq!(toolbar_layout(650.).padding, 8.);
        assert_eq!(toolbar_layout(720.).search_min, 96.);
        assert_eq!(toolbar_layout(1200.).search_min, 150.);
        assert!(!toolbar_layout(900.).search_kbd);
        assert!(!toolbar_layout(1200.).search_kbd);
        assert!(toolbar_layout(1600.).search_kbd);
        assert_eq!(toolbar_layout(1200.).gap, 6.);
        assert_eq!(toolbar_layout(800.).gap, 4.);
        assert_eq!(toolbar_layout(1200.).item_pad, 8.);
        assert_eq!(toolbar_layout(800.).item_pad, 7.);
    }

    #[test]
    fn dock_defaults_to_right_only_on_wide_windows() {
        assert_eq!(resolve_dock(None, 1400.), Dock::Right);
        assert_eq!(resolve_dock(None, 1399.), Dock::Bottom);
        assert_eq!(resolve_dock(Some(Dock::Bottom), 1600.), Dock::Bottom);
        assert_eq!(resolve_dock(Some(Dock::Right), 800.), Dock::Right);
    }

    #[test]
    fn details_size_is_clamped_to_the_dock() {
        assert_eq!(dock_default(Dock::Bottom), 340.);
        assert_eq!(dock_default(Dock::Right), 640.);
        assert_eq!(clamp_details(100., Dock::Bottom), 220.);
        assert_eq!(clamp_details(999., Dock::Bottom), 560.);
        assert_eq!(clamp_details(999., Dock::Right), 900.);
        assert_eq!(clamp_details(f32::NAN, Dock::Right), 640.);
    }

    #[test]
    fn right_docked_details_never_take_over_half_the_pane() {
        // The live bug: 1270 px with a 420 px sidebar and a 900 px panel left
        // the history pane ~200 px. Half the space right of the sidebar is
        // 425 px.
        assert_eq!(effective_details_width(900., 1270., 420.), 425.);
        assert_eq!(details_width_limit(1270., 420.), 425.);
        // A stored 640 px panel still fits on a wide window.
        assert_eq!(effective_details_width(640., 1600., 240.), 640.);
        assert_eq!(details_width_limit(1600., 240.), 680.);
        // On a tiny window the 220 px minimum beats the half rule.
        assert_eq!(effective_details_width(900., 500., 300.), 220.);
        assert_eq!(details_width_limit(500., 300.), 220.);
    }

    #[test]
    fn the_sidebar_never_takes_more_than_half_the_window() {
        assert_eq!(sidebar_width_limit(1600.), 800.);
        assert_eq!(sidebar_width_limit(1000.), 500.);
        // The 160 px minimum beats the half rule on a tiny window.
        assert_eq!(sidebar_width_limit(300.), 160.);

        // The stored width stands while it fits in the allowed half.
        assert_eq!(effective_sidebar_width(240., 1600.), 240.);
        assert_eq!(effective_sidebar_width(420., 1000.), 420.);
        // Wider stored values render at half the visible area.
        assert_eq!(effective_sidebar_width(900., 1600.), 800.);
        assert_eq!(effective_sidebar_width(420., 500.), 250.);
        assert_eq!(effective_sidebar_width(420., 300.), 160.);
    }

    #[test]
    fn details_header_keeps_its_height() {
        assert_eq!(DOCK_HEADER, px(34.));
        assert_eq!(ui::metrics::BAR, px(40.));
        assert_eq!(STATUS, px(24.));
    }

    /// Makes one commit on the repository.
    fn commit(repo: &Path, file: &str, message: &str) {
        std::fs::write(repo.join(file), message).expect("write the committed file");
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test User")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["add", file]);
        run(&["commit", "-q", "-m", message]);
    }

    /// Opens the repository view as the window root on a real repository and
    /// waits for its history to load.
    fn open_repository_view(
        repo: &crate::test_support::TempRepo,
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::VisualTestContext, Entity<RepositoryView>) {
        use gpui::{
            point, size, AppContext, Bounds, VisualTestContext, WindowBackgroundAppearance,
            WindowBounds, WindowDecorations, WindowOptions,
        };

        let git = crate::test_support::git();
        cx.update(ui::init).expect("ui::init");
        cx.update(bind_search_keys);
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
                    let workspace = cx.new(|cx| {
                        Workspace::new(None, Ok(git.clone()), git_core::GitChoice::Auto, window, cx)
                    });
                    let weak = workspace.downgrade();
                    cx.new(|cx| {
                        RepositoryView::new(
                            repo.path().to_path_buf(),
                            Ok(git.clone()),
                            weak,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let view = cx
            .update(|window, _| window.root::<RepositoryView>().flatten())
            .expect("the repository view is the window root");
        (cx, view)
    }

    /// The toolbar switch replaces the history pane with the local-changes
    /// view and back; the changes view loads the work-tree status.
    #[gpui::test]
    fn the_view_switch_opens_local_changes(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("view-switch");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::Changes, window, cx));
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.view, ViewMode::Changes);
            let changes = view.changes.read(cx);
            assert_eq!(changes.unstaged_count(), 1, "{:?}", changes.status());
        });
        assert!(cx.debug_bounds("changes-view").is_some());
        assert!(
            cx.debug_bounds("changes-unstaged-0").is_some(),
            "the changed file was not drawn"
        );

        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::History, window, cx));
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert_eq!(view.view, ViewMode::History));
        assert!(cx.debug_bounds("changes-view").is_none());
    }

    /// A commit made in the changes view reloads the history behind it.
    #[gpui::test]
    fn committing_from_changes_reloads_the_history(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("changes-commit-reload");
        commit(repo.path(), "a.txt", "one\n");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_view(ViewMode::Changes, window, cx));
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.changes.update(cx, |changes, cx| {
                    changes.refresh(cx);
                    changes.set_message_for_test("feat: two", cx);
                });
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.changes.update(cx, |changes, cx| {
                    changes.stage_all(crate::changes_view::Side::Unstaged, cx)
                });
            });
            let _ = window;
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.changes
                    .update(cx, |changes, cx| changes.commit(window, cx));
            });
        });
        cx.run_until_parked();

        view.read_with(&cx, |view, cx| {
            let history = view.history.read(cx);
            assert_eq!(history.commit_count(), 2, "the new commit is missing");
            assert_eq!(
                history.selected_commit().expect("HEAD is selected").summary,
                "feat: two"
            );
        });
    }

    /// Selecting a commit loads its details and changed files into the dock,
    /// and clicking another row replaces them.
    #[gpui::test]
    fn selecting_a_commit_fills_the_details_dock(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-panel");
        commit(repo.path(), "a.txt", "one\ntwo\n");
        commit(repo.path(), "b.txt", "three\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();

        view.read_with(&cx, |view, _| {
            let details = view
                .details
                .details()
                .expect("the selected commit's details did not load");
            assert_eq!(details.summary, "three");
            let files = view
                .details
                .files()
                .expect("the changed files did not load");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "b.txt");
            assert_eq!(
                view.selected_file,
                Some(0),
                "the first file is not selected"
            );
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the selected file's diff did not load");
            };
            assert!(diff.rows.len() > 1, "the diff has no body rows");
            assert!(diff.rows.iter().any(|row| row.text.contains("three")));
        });
        assert!(
            cx.debug_bounds("details-file-0").is_some(),
            "the changed file row was not drawn"
        );
        assert!(
            cx.debug_bounds("diff-row-0").is_some(),
            "the diff rows were not drawn"
        );

        // Clicking the older row loads that commit instead.
        let row = cx
            .debug_bounds("history-row-1")
            .expect("the second history row was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            let details = view
                .details
                .details()
                .expect("the clicked commit's details did not load");
            assert_eq!(details.summary, "one two");
            let files = view
                .details
                .files()
                .expect("the changed files did not load");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "a.txt");
            assert_eq!(view.selected_file, Some(0));
        });
    }

    /// Clicking a changed file selects it, loads its diff and takes the
    /// keyboard: ↑/↓ then move the file selection.
    #[gpui::test]
    fn the_file_list_selects_files_and_answers_arrow_keys(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-files");
        std::fs::write(repo.path().join("a.txt"), "alpha\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "beta\n").unwrap();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["add", "-A"])
            .output()
            .expect("git runs");
        assert!(out.status.success());
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["commit", "-q", "-m", "two files"])
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git runs");
        assert!(out.status.success());

        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.details.files().map(<[git_core::ChangedFile]>::len),
                Some(2)
            );
            assert_eq!(view.selected_file, Some(0));
        });

        let row = cx
            .debug_bounds("details-file-1")
            .expect("the second changed file was not drawn");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.selected_file, Some(1));
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the second file's diff did not load");
            };
            assert!(diff.rows.iter().any(|row| row.text.contains("beta")));
        });

        cx.simulate_keystrokes("up");
        view.read_with(&cx, |view, _| assert_eq!(view.selected_file, Some(0)));
        cx.simulate_keystrokes("down");
        view.read_with(&cx, |view, _| assert_eq!(view.selected_file, Some(1)));
    }

    /// Clicking a parent's short hash selects that commit in the history.
    #[gpui::test]
    fn clicking_a_parent_hash_selects_that_commit(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("details-parent");
        commit(repo.path(), "a.txt", "one\n");
        commit(repo.path(), "b.txt", "two\n");
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(
                view.details
                    .details()
                    .map(|details| details.summary.as_str()),
                Some("two")
            );
        });

        let parent = cx
            .debug_bounds("details-parent-0")
            .expect("the parent link was not drawn");
        cx.simulate_click(parent.center(), Modifiers::default());
        cx.run_until_parked();
        view.read_with(&cx, |view, cx| {
            assert_eq!(
                view.details
                    .details()
                    .map(|details| details.summary.as_str()),
                Some("one"),
                "the parent commit's details did not load"
            );
            assert_eq!(
                view.history
                    .read(cx)
                    .selected_commit()
                    .map(|c| c.summary.clone()),
                Some("one".to_string()),
                "the parent was not selected in the history"
            );
        });
    }

    /// The layout toggle switches the pane to side-by-side rows and stores the
    /// choice in the settings.
    #[gpui::test]
    fn the_diff_toggle_switches_to_side_by_side_and_persists(cx: &mut gpui::TestAppContext) {
        let repo = crate::test_support::TempRepo::new("details-split");
        commit(repo.path(), "main.rs", "fn main() {}\n");
        cx.update(|cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings::default(),
                save_in_flight: false,
                dirty: false,
            });
        });
        let (mut cx, view) = open_repository_view(&repo, cx);
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.diff_layout, DiffLayout::Unified);
        });

        view.update(&mut cx, |view, cx| {
            view.set_diff_layout(DiffLayout::Split, cx)
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| {
            assert_eq!(view.diff_layout, DiffLayout::Split);
            let DiffState::Loaded(diff) = &view.diff else {
                panic!("the diff did not load");
            };
            assert!(!diff.split_rows.is_empty());
            assert_eq!(diff.highlight.language, Some("rust"));
            assert!(
                !diff.highlight.new.is_empty(),
                "the new side was not highlighted"
            );
        });
        assert!(
            cx.debug_bounds("split-row-0").is_some(),
            "the side-by-side rows were not drawn"
        );
        let saved = cx.update(|_, cx| cx.global::<crate::AppSettings>().settings.diff_layout);
        assert_eq!(saved, DiffLayout::Split, "the layout was not persisted");
    }

    /// Lets the 150 ms search debounce and the background scan finish. The
    /// test scheduler does not advance its clock on its own while parked.
    fn settle_search(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
    }

    /// Typing a query into the toolbar field jumps the list to the first
    /// match; Enter / F3 step forward and Shift+Enter back, both wrapping; Esc
    /// clears the field and hands the list the keyboard.
    #[gpui::test]
    fn the_search_field_jumps_to_matches_and_steps_through_them(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("history-search");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta one");
        commit(repo.path(), "c.txt", "Gamma");
        commit(repo.path(), "d.txt", "Beta two");

        let (mut cx, view) = open_repository_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        // Newest first: 0 "Beta two", 1 "Gamma", 2 "Beta one", 3 "Alpha".
        assert_eq!(history.read_with(&cx, |view, _| view.commit_count()), 4);

        let field = cx
            .debug_bounds("toolbar-search")
            .expect("the toolbar search was not drawn");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();

        // A match below the current selection is selected after the debounce.
        cx.simulate_input("gamma");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(1),
            "typing did not jump to the matching row"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 1".to_owned())
        );

        // Esc clears the query, the count disappears and the list gets the
        // keyboard without moving the selection.
        cx.simulate_keystrokes("escape");
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status()),
            None,
            "Esc did not clear the search"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(1)
        );
        let text = view.read_with(&cx, |view, app| view.search.read(app).text().to_owned());
        assert!(text.is_empty(), "Esc did not clear the field's text");
        cx.update(|window, app| {
            assert!(
                history.read(app).list_focus().is_focused(window),
                "Esc did not focus the history list"
            );
        });

        // Two matches, and the selection is on row 1 ("Gamma"), so the first
        // match at or after it is row 2.
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input("beta");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("2 / 2".to_owned())
        );

        cx.simulate_keystrokes("enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0),
            "Enter did not wrap to the first match"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 2".to_owned())
        );
        cx.simulate_keystrokes("enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0),
            "Shift+Enter did not wrap back to the first match"
        );
        cx.simulate_keystrokes("f3");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2),
            "F3 did not step to the next match"
        );

        // ↓ keeps the selection and gives the list the keyboard.
        cx.simulate_keystrokes("down");
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(2)
        );
        cx.update(|window, app| {
            assert!(
                history.read(app).list_focus().is_focused(window),
                "↓ did not focus the history list"
            );
        });
    }

    /// Enter inside the debounce window applies the field's text first, so the
    /// step (and the scan's jump) uses the new query, not the previous one's
    /// matches.
    #[gpui::test]
    fn enter_during_the_debounce_applies_the_new_query(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let repo = crate::test_support::TempRepo::new("history-search-debounce");
        commit(repo.path(), "a.txt", "Alpha");
        commit(repo.path(), "b.txt", "Beta one");
        commit(repo.path(), "c.txt", "Gamma");
        commit(repo.path(), "d.txt", "Beta two");

        let (mut cx, view) = open_repository_view(&repo, cx);
        let history = view.read_with(&cx, |view, _| view.history());
        let field = cx
            .debug_bounds("toolbar-search")
            .expect("the toolbar search was not drawn");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();

        // Apply "beta": it matches rows 0 and 2, and the jump selects row 0,
        // the first match at or after the initial selection.
        cx.simulate_input("beta");
        settle_search(&mut cx);
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(0)
        );

        // A new query followed by Enter before the debounce fires must search
        // "alpha" (row 3), not step over the old "beta" matches.
        cx.simulate_keystrokes("escape");
        cx.simulate_click(field.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input("alpha");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            history.read_with(&cx, |view, _| view.selected_row()),
            Some(3),
            "Enter inside the debounce did not apply the new query"
        );
        assert_eq!(
            history.read_with(&cx, |view, _| view.search_status().map(|s| s.to_string())),
            Some("1 / 1".to_owned())
        );
    }
}
