//! The repository shell: toolbar, sidebar, history, details dock and status bar.
//!
//! One [`RepositoryView`] lives per open repository; the [`Workspace`] owns it
//! and swaps it for the welcome screen. Everything that belongs to a later
//! stage is a placeholder or a disabled control, never a dead click.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, size, App, AppContext, Bounds, ClickEvent, Context,
    Decorations, Div, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window,
};

use git_core::{Error, Git, Head, Ref, RefKind};
use ui::metrics::{DOCK_HEADER, FONT_MONO, FONT_UI, FS_BASE, FS_SM, R_SM, STATUS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    menu, Button, Icon, IconName, MenuEntry, MenuItem, SearchField, SegmentItem, Segmented,
    Spinner, Splitter, TextInput, TextInputEvent, Tooltip, Tree, TreeNodeData,
};

use crate::format::format_count;
use crate::history_state::LoadState;
use crate::history_view::HistoryView;
use crate::settings::{Dock, SIDEBAR_WIDTH_DEFAULT, SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN};
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

/// The room the history pane always keeps for itself.
pub const HISTORY_MIN_WIDTH: f32 = 360.;
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

/// The width the sidebar renders at: its stored width, but never so wide that
/// the history pane drops below [`HISTORY_MIN_WIDTH`], and never below its own
/// 160 px minimum.
///
/// `details_width` is the width the details panel actually renders at, which
/// only counts while it is docked right and open. The stored width is kept.
pub fn effective_sidebar_width(stored: f32, window_width: f32, details_width: f32) -> f32 {
    let stored = stored.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
    let room = window_width - HISTORY_MIN_WIDTH - details_width;
    stored.min(room).max(SIDEBAR_WIDTH_MIN)
}

/// The six sidebar sections, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Local,
    Remotes,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Local,
        Section::Remotes,
        Section::Tags,
        Section::Stashes,
        Section::Worktrees,
        Section::Submodules,
    ];

    fn index(self) -> usize {
        match self {
            Section::Local => 0,
            Section::Remotes => 1,
            Section::Tags => 2,
            Section::Stashes => 3,
            Section::Worktrees => 4,
            Section::Submodules => 5,
        }
    }

    /// The key prefix of a section's tree node.
    fn key(self) -> &'static str {
        match self {
            Section::Local => "section:local",
            Section::Remotes => "section:remotes",
            Section::Tags => "section:tags",
            Section::Stashes => "section:stashes",
            Section::Worktrees => "section:worktrees",
            Section::Submodules => "section:submodules",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.key() == key)
    }
}

/// Which sidebar sections are expanded. The three filled in stage 4 start
/// collapsed; the rest start open.
#[derive(Clone, Debug)]
pub struct Sections([bool; 6]);

impl Default for Sections {
    fn default() -> Self {
        Self([true, true, true, false, false, false])
    }
}

impl Sections {
    fn expanded(&self, section: Section) -> bool {
        self.0[section.index()]
    }

    fn toggle(&mut self, section: Section) {
        let slot = &mut self.0[section.index()];
        *slot = !*slot;
    }
}

/// Builds the visible sidebar rows from the repository's refs.
///
/// `refs` is `None` while they load or after a failure: then only the section
/// headers are shown, without counts. Sections without refs keep their header;
/// the three later stages fill (stashes, worktrees, submodules) never get a
/// count.
pub fn sidebar_nodes(
    refs: Option<&[Ref]>,
    head: Option<&Head>,
    sections: &Sections,
    selected: Option<&str>,
) -> Vec<TreeNodeData> {
    let counts = refs.is_some();
    let refs = refs.unwrap_or(&[]);
    let locals: Vec<&Ref> = refs
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .collect();
    let remotes: Vec<&Ref> = refs
        .iter()
        .filter(|r| r.kind == RefKind::RemoteBranch && !r.name.ends_with("/HEAD"))
        .collect();
    let tags: Vec<&Ref> = refs.iter().filter(|r| r.kind == RefKind::Tag).collect();
    let head_branch = match head {
        Some(Head::Branch { name, .. }) => Some(name.as_str()),
        _ => None,
    };

    let mut nodes = Vec::new();
    let section =
        |nodes: &mut Vec<TreeNodeData>, section: Section, label: &str, count: Option<usize>| {
            let mut node = TreeNodeData::section(label.to_owned())
                .key(section.key())
                .has_children(true)
                .expanded(sections.expanded(section));
            if let Some(count) = count {
                node = node.count(count.to_string());
            }
            nodes.push(node);
        };

    section(
        &mut nodes,
        Section::Local,
        "Local branches",
        counts.then_some(locals.len()),
    );
    if sections.expanded(Section::Local) {
        for r in &locals {
            nodes.push(branch_node(
                &r.name,
                head_branch == Some(r.name.as_str()),
                selected,
            ));
        }
    }

    section(
        &mut nodes,
        Section::Remotes,
        "Remotes",
        counts.then_some(remotes.len()),
    );
    if sections.expanded(Section::Remotes) {
        for r in &remotes {
            nodes.push(leaf_node(
                &format!("remote:{}", r.name),
                &r.name,
                IconName::Branch,
                selected,
            ));
        }
    }

    section(
        &mut nodes,
        Section::Tags,
        "Tags",
        counts.then_some(tags.len()),
    );
    if sections.expanded(Section::Tags) {
        for r in &tags {
            nodes.push(leaf_node(
                &format!("tag:{}", r.name),
                &r.name,
                IconName::Tag,
                selected,
            ));
        }
    }

    // Filled by stage 4: headers only, no counts.
    for (section_id, label) in [
        (Section::Stashes, "Stashes"),
        (Section::Worktrees, "Worktrees"),
        (Section::Submodules, "Submodules"),
    ] {
        section(&mut nodes, section_id, label, None);
    }

    nodes
}

fn branch_node(name: &str, current: bool, selected: Option<&str>) -> TreeNodeData {
    let key = format!("local:{name}");
    let mut node = leaf_node(&key, name, IconName::Branch, selected);
    if current {
        node = node.current();
    }
    node
}

fn leaf_node(key: &str, label: &str, icon: IconName, selected: Option<&str>) -> TreeNodeData {
    TreeNodeData::new(label.to_owned())
        .key(key.to_owned())
        .level(1)
        .icon(icon)
        .selected(selected == Some(key))
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
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    search: Entity<TextInput>,
    search_focus: FocusHandle,
    /// The search field's text and submit events, forwarded to the history.
    _search_events: Subscription,
    sidebar_filter: Entity<TextInput>,
    sidebar_open: bool,
    sidebar_width: f32,
    sections: Sections,
    selected_node: Option<SharedString>,
    dock: Dock,
    details_open: bool,
    details_size: f32,
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
            if let Some(root) = history.read(cx).root() {
                if this.root != root {
                    this.root = root.to_path_buf();
                    this.repo_name = repo_name(root);
                }
            }
            cx.notify();
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
            sections: Sections::default(),
            selected_node: None,
            dock,
            details_open: true,
            details_size,
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

    fn toggle_section(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(section) = Section::from_key(key) {
            self.sections.toggle(section);
            cx.notify();
        }
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

        // History is the only view in this stage; Changes is stage 6.
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
        bar = bar.child(no_shrink(
            Segmented::new("view-switch")
                .item(history_item)
                .item(
                    changes_item
                        .disabled(true)
                        .tooltip("Changes — arrives in a later version"),
                )
                .selected(0),
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
        let keys: Vec<SharedString> = nodes.iter().map(|node| node.key.clone()).collect();

        let view = cx.entity();
        let activate = {
            let keys = keys.clone();
            let view = view.clone();
            move |index: usize, _: &mut Window, cx: &mut App| {
                let Some(key) = keys.get(index).cloned() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    view.selected_node = Some(key);
                    cx.notify();
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
                div().w_full().flex_1().min_h_0().child(
                    Tree::new("sidebar-tree", nodes)
                        .on_toggle(section_toggler(keys.clone(), view.clone()))
                        .on_expand(section_toggler(keys.clone(), view.clone()))
                        .on_collapse(section_toggler(keys, view))
                        .on_activate(activate),
                ),
            )
    }

    fn tree_nodes(&self, cx: &Context<Self>) -> Vec<TreeNodeData> {
        let history = self.history.read(cx);
        sidebar_nodes(
            history.refs(),
            history.head(),
            &self.sections,
            self.selected_node.as_ref().map(|key| key.as_str()),
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

    fn render_details(&mut self, width: f32, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut panel = div().flex_none().flex().flex_col().bg(colors.bg);
        panel = match self.dock {
            Dock::Bottom => panel.w_full().h(px(width)).border_t_1(),
            Dock::Right => panel.h_full().w(px(width)).border_l_1(),
        };
        panel
            .border_color(colors.border)
            .child(self.render_details_header(cx))
            .child(self.render_details_body(colors))
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

    fn render_details_body(&self, colors: ThemeColors) -> Div {
        match self.dock {
            Dock::Bottom => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .child(
                    details_pane("Commit details", colors)
                        .w(px(310.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(
                    details_pane("Changed files", colors)
                        .w(px(250.))
                        .flex_none()
                        .h_full()
                        .border_r_1(),
                )
                .child(details_pane("Diff", colors).flex_1().min_w_0().h_full()),
            Dock::Right => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    // Stage 5 fills this pane; until then it keeps the
                    // design's maximum height so the three panes are visible.
                    details_pane("Commit details", colors)
                        .h(px(230.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(
                    details_pane("Changed files", colors)
                        .h(px(160.))
                        .flex_none()
                        .w_full()
                        .border_b_1(),
                )
                .child(details_pane("Diff", colors).flex_1().w_full()),
        }
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let history = self.history.read(cx);
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
                // The failure takes the left side and the stretch, so it
                // cannot push the counters and the git button off the bar.
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

        bar = bar
            .child(format!(
                "{} commits loaded",
                format_count(history.commit_count())
            ))
            .child(
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
            );
        bar
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

        // A stored sidebar (up to 420) plus a right-docked details panel (up to
        // 900) could leave the history pane with nothing on a ~1270 px window.
        // Only the rendered widths are capped; the stored sizes stay as saved.
        let sidebar_space = if self.sidebar_open {
            self.sidebar_width
        } else {
            0.
        };
        let details_limit = match self.dock {
            Dock::Right => details_width_limit(width, sidebar_space),
            Dock::Bottom => dock_bounds(Dock::Bottom).1,
        };
        let details_width = if self.details_open {
            match self.dock {
                Dock::Right => effective_details_width(self.details_size, width, sidebar_space),
                Dock::Bottom => clamp_details(self.details_size, Dock::Bottom),
            }
        } else {
            0.
        };
        let sidebar_width = if self.sidebar_open {
            effective_sidebar_width(self.sidebar_width, width, details_width)
        } else {
            0.
        };
        // The splitter may not drag the sidebar past what renders either.
        let sidebar_limit = SIDEBAR_WIDTH_MAX
            .min(width - HISTORY_MIN_WIDTH - details_width)
            .max(SIDEBAR_WIDTH_MIN);

        let sidebar = self.sidebar_open.then(|| {
            (
                self.render_sidebar(sidebar_width, cx),
                self.render_sidebar_splitter(sidebar_width, sidebar_limit, cx),
            )
        });
        let details_open = self.details_open;
        let dock = self.dock;
        let splitter =
            details_open.then(|| self.render_details_splitter(details_width, details_limit, cx));
        let panel = details_open.then(|| self.render_details(details_width, cx));
        let status = self.render_status_bar(cx);

        let history_pane = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.history.clone());

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

/// One empty details pane: a centred, muted label between its borders.
fn details_pane(label: &'static str, colors: ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .border_color(colors.border)
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(label)
}

/// A click, Enter/Space or ←/→ on a section header flips its expansion.
fn section_toggler(
    keys: Vec<SharedString>,
    view: Entity<RepositoryView>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    move |index, _, cx| {
        let Some(key) = keys.get(index).cloned() else {
            return;
        };
        view.update(cx, |view, cx| view.toggle_section(&key, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_core::Oid;
    use gpui::px;

    fn reference(kind: RefKind, name: &str) -> Ref {
        Ref {
            full_name: format!("refs/{name}"),
            name: name.to_owned(),
            kind,
            target: Oid::from_hex(&"1".repeat(40)).unwrap(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
        }
    }

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
    fn the_sidebar_never_squeezes_the_history_pane_below_360() {
        // 1000 px, a 320 px right-docked panel and a stored 420 px sidebar:
        // only 320 px of sidebar fit beside the 360 px the history needs.
        assert_eq!(effective_sidebar_width(420., 1000., 320.), 320.);
        // Nothing to squeeze: the stored width stands.
        assert_eq!(effective_sidebar_width(240., 1000., 320.), 240.);
        assert_eq!(effective_sidebar_width(240., 1270., 425.), 240.);
        // The 160 px sidebar minimum beats the 360 px history rule.
        assert_eq!(effective_sidebar_width(420., 800., 400.), 160.);
        // No right-docked panel (bottom dock or closed): only the 360 px rule.
        assert_eq!(effective_sidebar_width(420., 1000., 0.), 420.);
        assert_eq!(effective_sidebar_width(420., 500., 0.), 160.);
    }

    #[test]
    fn sidebar_lists_sections_with_counts_and_the_current_branch() {
        let refs = vec![
            reference(RefKind::LocalBranch, "main"),
            reference(RefKind::LocalBranch, "feat/lanes"),
            reference(RefKind::RemoteBranch, "origin/main"),
            reference(RefKind::RemoteBranch, "origin/HEAD"),
            reference(RefKind::Tag, "v0.3.0"),
        ];
        let head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        let nodes = sidebar_nodes(Some(&refs), Some(&head), &Sections::default(), None);

        let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_ref()).collect();
        assert_eq!(
            labels,
            vec![
                "Local branches",
                "main",
                "feat/lanes",
                "Remotes",
                "origin/main",
                "Tags",
                "v0.3.0",
                "Stashes",
                "Worktrees",
                "Submodules",
            ]
        );
        assert!(nodes[0].section);
        assert!(nodes[0].expanded);
        assert_eq!(
            nodes[0].trailing,
            Some(ui::TreeNodeTrailing::Count("2".into()))
        );
        assert!(nodes[1].current);
        assert!(!nodes[2].current);
        // `origin/HEAD` is a symbolic ref, not a branch row.
        assert!(nodes.iter().all(|n| n.label != "origin/HEAD"));
        // The stage-4 sections have headers but no counts.
        assert!(nodes[7].trailing.is_none());
        assert!(!nodes[7].expanded);
        assert_eq!(
            nodes[5].trailing,
            Some(ui::TreeNodeTrailing::Count("1".into()))
        );
    }

    #[test]
    fn loading_refs_show_headers_without_counts() {
        let nodes = sidebar_nodes(None, None, &Sections::default(), None);
        assert_eq!(nodes.len(), 6);
        assert!(nodes.iter().all(|node| node.section));
        assert!(nodes.iter().all(|node| node.trailing.is_none()));
    }

    #[test]
    fn collapsed_sections_hide_their_leaves() {
        let refs = vec![reference(RefKind::LocalBranch, "main")];
        let mut sections = Sections::default();
        sections.toggle(Section::Local);
        let nodes = sidebar_nodes(Some(&refs), None, &sections, None);
        assert_eq!(nodes.len(), 6);
        assert!(!nodes[0].expanded);
        assert_eq!(nodes[0].label.as_ref(), "Local branches");
    }

    #[test]
    fn selection_marks_the_matching_leaf() {
        let refs = vec![reference(RefKind::Tag, "v1")];
        let nodes = sidebar_nodes(Some(&refs), None, &Sections::default(), Some("tag:v1"));
        let tag = nodes.iter().find(|n| n.label == "v1").unwrap();
        assert!(tag.selected);
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
