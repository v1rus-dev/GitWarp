//! Context menus and dropdown menus.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    anchored, canvas, deferred, div, point, px, App, AppContext, BorrowAppContext, Bounds, Context,
    Div, ElementId, Entity, FocusHandle, FontWeight, Global, IntoElement, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, SharedString, Size, Stateful, Styled, Subscription, Task,
    Window, WindowId,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::actions::{
    Cancel, Confirm, FocusNext, FocusPrev, MenuCloseSubmenu, MenuOpenSubmenu, SelectNext,
    SelectPrev,
};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::components::{SearchField, Tooltip};
use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_UI, FS_BASE, FS_SM, FS_XS, R_MD, R_SM};
use crate::overlay::{self, LayerId};
use crate::theme::ActiveTheme;

/// A menu callback.
pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Builds the rows of a submenu; called each time the item opens it.
pub type MenuSubmenu = Rc<dyn Fn() -> Vec<MenuEntry>>;

/// One actionable row of a menu.
#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub shortcut: Option<SharedString>,
    pub danger: bool,
    pub disabled: bool,
    pub action: MenuAction,
    /// When set, the item opens a cascading child panel (hover for 200 ms,
    /// click, Enter or →) instead of running `action`.
    pub submenu: Option<MenuSubmenu>,
    /// When set, activating the item swaps the open menu's rows for these
    /// in place instead of closing it and running `action`.
    pub replace_rows: Option<MenuSubmenu>,
    /// Muted text right-aligned before the shortcut ("HEAD +origin", "↑2 ↓1").
    pub hint: Option<SharedString>,
    /// Draws the icon in the accent colour (the checked-out branch).
    pub accent_icon: bool,
    /// What a [`MenuEntry::Filter`] above matches against; the label by default.
    pub filter_text: Option<SharedString>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            danger: false,
            disabled: false,
            action: Rc::new(|_, _| {}),
            submenu: None,
            replace_rows: None,
            hint: None,
            accent_icon: false,
            filter_text: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn action(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Rc::new(action);
        self
    }

    /// Makes the item open a cascading child panel beside its row.
    pub fn submenu(mut self, entries: impl Fn() -> Vec<MenuEntry> + 'static) -> Self {
        self.submenu = Some(Rc::new(entries));
        self
    }

    /// Makes the item replace the open menu's rows in place (same position,
    /// same focus) instead of closing it. A "Back" row is just another such
    /// item that returns the parent's rows.
    pub fn replace_rows(mut self, entries: impl Fn() -> Vec<MenuEntry> + 'static) -> Self {
        self.replace_rows = Some(Rc::new(entries));
        self
    }

    /// Muted text right-aligned before the shortcut.
    pub fn hint(mut self, text: impl Into<SharedString>) -> Self {
        self.hint = Some(text.into());
        self
    }

    /// Draws the icon in the accent colour.
    pub fn accent_icon(mut self) -> Self {
        self.accent_icon = true;
        self
    }

    /// The text a [`MenuEntry::Filter`] matches; the label when unset.
    pub fn filter_text(mut self, text: impl Into<SharedString>) -> Self {
        self.filter_text = Some(text.into());
        self
    }

    fn matches(&self, query: &str) -> bool {
        let text = self.filter_text.as_ref().unwrap_or(&self.label);
        text.to_lowercase().contains(&query.to_lowercase())
    }
}

/// A small icon button at the right end of a [`MenuHeader`].
#[derive(Clone)]
pub struct MenuTool {
    pub icon: IconName,
    pub tooltip: SharedString,
    /// The "on" state: accent icon over the active fill.
    pub selected: bool,
    pub action: MenuAction,
}

impl MenuTool {
    pub fn new(icon: IconName, tooltip: impl Into<SharedString>) -> Self {
        Self {
            icon,
            tooltip: tooltip.into(),
            selected: false,
            action: Rc::new(|_, _| {}),
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn action(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Rc::new(action);
        self
    }
}

/// A section title with optional icon buttons on the right.
#[derive(Clone)]
pub struct MenuHeader {
    pub label: SharedString,
    pub tools: Vec<MenuTool>,
}

impl MenuHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tools: Vec::new(),
        }
    }

    pub fn tool(mut self, tool: MenuTool) -> Self {
        self.tools.push(tool);
        self
    }
}

/// A row of a menu: an item, a rule, a section header or the filter field.
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
    /// A section title: 11px, semibold, muted, in the ref's original case.
    Header(MenuHeader),
    /// A text field; the items after it whose filter text does not contain
    /// the query (case-insensitive) are hidden, and focus starts in the field.
    Filter {
        placeholder: SharedString,
    },
}

/// Per item (in item order, separators and headers not counted), whether the
/// filter query leaves it visible. Only items after a [`MenuEntry::Filter`]
/// can be hidden; an empty query hides nothing.
pub fn visible_items(entries: &[MenuEntry], query: &str) -> Vec<bool> {
    let mut filtering = false;
    let mut visible = Vec::new();
    for entry in entries {
        match entry {
            MenuEntry::Filter { .. } => filtering = true,
            MenuEntry::Item(item) => visible.push(!filtering || item.matches(query)),
            MenuEntry::Separator | MenuEntry::Header(_) => {}
        }
    }
    visible
}

/// Highlight movement over the enabled flags of a menu's items.
///
/// The highlight is an item index (separators are not part of the flags), and
/// navigation only stops on enabled items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuNav {
    highlight: Option<usize>,
}

impl MenuNav {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    pub fn clear(&mut self) {
        self.highlight = None;
    }

    pub fn set(&mut self, index: usize) {
        self.highlight = Some(index);
    }

    /// The next enabled item, wrapping; nothing when no item is enabled.
    pub fn next(&mut self, enabled: &[bool]) {
        let Some(len) = non_zero_len(enabled) else {
            self.highlight = None;
            return;
        };
        let start = match self.highlight {
            Some(index) => (index + 1) % len,
            None => 0,
        };
        self.highlight = (0..len)
            .map(|step| (start + step) % len)
            .find(|index| enabled[*index]);
    }

    /// The previous enabled item, wrapping; from nothing this is the last one.
    pub fn prev(&mut self, enabled: &[bool]) {
        let Some(len) = non_zero_len(enabled) else {
            self.highlight = None;
            return;
        };
        let start = match self.highlight {
            Some(index) => (index + len - 1) % len,
            None => len - 1,
        };
        self.highlight = (0..len)
            .map(|step| (start + len - step) % len)
            .find(|index| enabled[*index]);
    }
}

fn non_zero_len(enabled: &[bool]) -> Option<usize> {
    (!enabled.is_empty()).then_some(enabled.len())
}

/// Per-window id of the open menu, so a new one can close it.
#[derive(Default)]
struct MenuRegistry {
    by_window: HashMap<WindowId, LayerId>,
}

impl Global for MenuRegistry {}

/// Drops the remembered menu of a window that closed.
pub(crate) fn init(cx: &mut App) {
    cx.on_window_closed(|cx, window_id| {
        if cx.try_global::<MenuRegistry>().is_some() {
            cx.update_global::<MenuRegistry, _>(|registry, _| forget(registry, window_id));
        }
    })
    .detach();
}

fn forget(registry: &mut MenuRegistry, closed: WindowId) {
    registry.by_window.remove(&closed);
}

/// Opens a menu at the pointer, clamped 4px inside the window.
pub fn open_context_menu(
    position: Point<Pixels>,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    open_at(position, entries, None, window, cx)
}

/// Opens a menu 2px below the bottom-left corner of `anchor`.
pub fn open_dropdown(
    anchor: Bounds<Pixels>,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    open_dropdown_at(anchor, entries, None, window, cx)
}

/// Like [`open_dropdown`], with item `highlight` (an item index, separators
/// not counted) highlighted from the start: a select opens on its current
/// value, so the arrows move from there.
pub fn open_dropdown_at(
    anchor: Bounds<Pixels>,
    entries: Vec<MenuEntry>,
    highlight: Option<usize>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    let position = point(anchor.left(), anchor.bottom() + px(2.));
    open_at(position, entries, highlight, window, cx)
}

fn open_at(
    position: Point<Pixels>,
    entries: Vec<MenuEntry>,
    highlight: Option<usize>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    close_previous(window, cx);
    let view = cx.new(|cx| {
        let mut menu = MenuView::new(entries, window, cx);
        let enabled = menu.levels[0].enabled();
        if let Some(index) = highlight.filter(|index| enabled.get(*index) == Some(&true)) {
            menu.levels[0].nav.set(index);
        }
        menu
    });
    let focus = view.read(cx).entry_focus(cx);
    let id = overlay::open_popover(position, view.clone().into(), window, cx);
    view.update(cx, |menu, _| menu.layer = Some(id));
    remember(window, cx, id);
    window.focus(&focus, cx);
    id
}

fn close_previous(window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    let previous = cx
        .try_global::<MenuRegistry>()
        .and_then(|registry| registry.by_window.get(&window_id).copied());
    if let Some(previous) = previous {
        overlay::close(previous, window, cx);
    }
}

fn remember(window: &Window, cx: &mut App, id: LayerId) {
    let window_id = window.window_handle().window_id();
    if cx.try_global::<MenuRegistry>().is_none() {
        cx.set_global(MenuRegistry::default());
    }
    cx.update_global::<MenuRegistry, _>(|registry, _| {
        registry.by_window.insert(window_id, id);
    });
}

/// Gap kept between a window edge and a menu panel.
const WINDOW_MARGIN: f32 = 4.;
/// How far a child panel overlaps its parent's edge.
const SUBMENU_OVERLAP: f32 = 4.;
/// A child panel's top sits this far above the row that opened it, so its
/// first item lines up with that row (the panel's padding).
const SUBMENU_RISE: f32 = 6.;
/// Hover time on a row before its submenu opens or a sibling's closes.
const HOVER_DELAY: Duration = Duration::from_millis(200);
/// How long a pointer inside the safe triangle is trusted before the row it
/// stopped on takes over.
const SAFE_GRACE: Duration = Duration::from_millis(300);

/// Where a child panel's top-left corner goes: against the parent's right
/// edge, or flipped to the left when it would not fit, and level with the row
/// that opened it, kept inside the window.
pub fn submenu_origin(
    parent: Bounds<Pixels>,
    row: Bounds<Pixels>,
    size: Size<Pixels>,
    viewport: Size<Pixels>,
) -> Point<Pixels> {
    let (width, height) = (f32::from(size.width), f32::from(size.height));
    let mut x = f32::from(parent.right()) - SUBMENU_OVERLAP;
    if x + width > f32::from(viewport.width) - WINDOW_MARGIN {
        x = f32::from(parent.left()) + SUBMENU_OVERLAP - width;
    }
    let lowest = f32::from(viewport.height) - WINDOW_MARGIN - height;
    // Taller than the window: stay at the top edge rather than the bottom.
    let y = (f32::from(row.top()) - SUBMENU_RISE)
        .min(lowest)
        .max(WINDOW_MARGIN);
    point(px(x), px(y))
}

/// Whether `now` lies in the triangle from `apex` (the pointer when it left
/// the parent row) to the near edge of the child panel `target`, so a diagonal
/// move toward the child does not pass over, and switch to, other parent rows.
pub fn in_safe_triangle(
    apex: Point<Pixels>,
    now: Point<Pixels>,
    target: Bounds<Pixels>,
    flipped: bool,
) -> bool {
    let edge = f32::from(if flipped {
        target.right()
    } else {
        target.left()
    });
    let apex = (f32::from(apex.x), f32::from(apex.y));
    // An apex level with or past the near edge has no triangle (and would
    // make a degenerate one that contains everything).
    if (flipped && apex.0 <= edge) || (!flipped && apex.0 >= edge) {
        return false;
    }
    let top = (edge, f32::from(target.top()));
    let bottom = (edge, f32::from(target.bottom()));
    let p = (f32::from(now.x), f32::from(now.y));
    let side = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let d1 = side(p, apex, top);
    let d2 = side(p, top, bottom);
    let d3 = side(p, bottom, apex);
    let negative = d1 < 0. || d2 < 0. || d3 < 0.;
    let positive = d1 > 0. || d2 > 0. || d3 > 0.;
    !(negative && positive)
}

/// The filter field of a menu and what it currently holds.
struct FilterField {
    input: Entity<TextInput>,
    _subscription: Subscription,
}

/// One panel of a menu: the root, or a submenu opened from a row of the level
/// before it.
struct Level {
    entries: Vec<MenuEntry>,
    nav: MenuNav,
    filter: Option<FilterField>,
    query: String,
    /// The item of this level whose child level is open (`levels[i + 1]`).
    open_row: Option<usize>,
    /// Window bounds of the rows that open a submenu, by item index; written
    /// by a canvas while painting, read by the next render.
    row_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// Window bounds of the panel, written by a canvas while painting.
    panel_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Level {
    /// A level showing `entries`, with a filter field if they have one.
    fn new(
        entries: Vec<MenuEntry>,
        level: usize,
        window: &mut Window,
        cx: &mut Context<MenuView>,
    ) -> Self {
        let filter = entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Filter { placeholder } => Some(placeholder.clone()),
                _ => None,
            })
            .map(|placeholder| {
                let input = cx.new(|cx| TextInput::new(window, cx));
                input.update(cx, |input, cx| {
                    input.set_placeholder(placeholder.clone(), cx)
                });
                let _subscription = cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &TextInputEvent, window, cx| {
                        match event {
                            TextInputEvent::Changed(text) => {
                                this.set_query(level, text.clone(), cx)
                            }
                            // The input's own Enter binding shadows `Confirm`, so
                            // Enter in the field activates the highlighted row here.
                            TextInputEvent::Submit => this.confirm(window, cx),
                        }
                    },
                );
                FilterField {
                    input,
                    _subscription,
                }
            });
        Self {
            entries,
            nav: MenuNav::new(),
            filter,
            query: String::new(),
            open_row: None,
            row_bounds: Rc::default(),
            panel_bounds: Rc::default(),
        }
    }

    /// Enabled flags of the items only, in item order; a row hidden by the
    /// filter counts as disabled so the arrows skip it.
    fn enabled(&self) -> Vec<bool> {
        let visible = visible_items(&self.entries, &self.query);
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(!item.disabled),
                MenuEntry::Separator | MenuEntry::Header(_) | MenuEntry::Filter { .. } => None,
            })
            .zip(visible)
            .map(|(enabled, visible)| enabled && visible)
            .collect()
    }

    fn item(&self, index: usize) -> Option<&MenuItem> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item),
                MenuEntry::Separator | MenuEntry::Header(_) | MenuEntry::Filter { .. } => None,
            })
            .nth(index)
    }

    /// Where focus goes when this level takes it: the filter field, else the menu.
    fn entry_focus(&self, menu: &FocusHandle, cx: &App) -> FocusHandle {
        match &self.filter {
            Some(filter) => filter.input.read(cx).focus_handle().clone(),
            None => menu.clone(),
        }
    }
}

/// A hover decision waiting for [`HOVER_DELAY`]: at `level`, open the submenu
/// of an item, or (`None`) close the child level.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PendingHover {
    level: usize,
    open: Option<usize>,
}

struct MenuView {
    levels: Vec<Level>,
    /// The deepest level that owns the keyboard (arrows, Enter, the filter).
    active_level: usize,
    focus: FocusHandle,
    layer: Option<LayerId>,
    /// The last input was a key: the highlighted row shows the focus ring.
    keyboard: bool,
    /// The pointer position last seen over a row with an open submenu: the
    /// apex of the safe triangle.
    apex: Option<Point<Pixels>>,
    pending: Option<PendingHover>,
    hover_task: Option<Task<()>>,
    /// A row passed over inside the safe triangle, taken after [`SAFE_GRACE`]
    /// if the pointer has not reached the child by then.
    grace_row: Option<(usize, usize)>,
    grace_task: Option<Task<()>>,
}

impl MenuView {
    fn new(entries: Vec<MenuEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = Level::new(entries, 0, window, cx);
        Self {
            levels: vec![root],
            active_level: 0,
            focus: cx.focus_handle().tab_stop(false),
            layer: None,
            keyboard: false,
            apex: None,
            pending: None,
            hover_task: None,
            grace_row: None,
            grace_task: None,
        }
    }

    /// Where focus goes when the menu opens: the filter field, else the menu.
    fn entry_focus(&self, cx: &App) -> FocusHandle {
        self.levels[self.active_level].entry_focus(&self.focus, cx)
    }

    fn set_query(&mut self, level: usize, query: String, cx: &mut Context<Self>) {
        let Some(target) = self.levels.get_mut(level) else {
            return;
        };
        target.query = query;
        // A narrowed list highlights its first row so Enter picks it; the old
        // highlight may be hidden now.
        target.nav.clear();
        if !target.query.is_empty() {
            let enabled = target.enabled();
            target.nav.next(&enabled);
        }
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.keyboard = true;
        let level = &mut self.levels[self.active_level];
        let enabled = level.enabled();
        if forward {
            level.nav.next(&enabled);
        } else {
            level.nav.prev(&enabled);
        }
        cx.notify();
    }

    /// Activates the highlighted row (Enter).
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let level = self.active_level;
        if let Some(index) = self.levels[level].nav.highlight() {
            // Enter closes the menu on key-down; the paired key-up
            // must not activate the opener that regains focus.
            overlay::defer_focus_restore(window, cx);
            self.activate(level, index, window, cx);
            overlay::clear_deferred_focus_restore(window, cx);
        }
    }

    fn activate(
        &mut self,
        level: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.levels.get(level).and_then(|l| l.item(index)) else {
            return;
        };
        if let Some(rows) = item.replace_rows.clone() {
            self.cancel_hover();
            self.levels.truncate(level + 1);
            self.active_level = level;
            self.levels[level] = Level::new(rows(), level, window, cx);
            // The old field may have held focus; its replacement is not focused.
            window.focus(&self.entry_focus(cx), cx);
            cx.notify();
            return;
        }
        if item.submenu.is_some() {
            self.open_child(level, index, true, window, cx);
            return;
        }
        let action = item.action.clone();
        self.close_and_run(action, window, cx);
    }

    /// Opens the child level of item `index` of `level`. With `take_focus`
    /// (key or click) the child becomes the active level with its first
    /// enabled item highlighted; a hover open leaves the keyboard where it is.
    fn open_child(
        &mut self,
        level: usize,
        index: usize,
        take_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(submenu) = self
            .levels
            .get(level)
            .and_then(|l| l.item(index))
            .filter(|item| !item.disabled)
            .and_then(|item| item.submenu.clone())
        else {
            return;
        };
        self.cancel_hover();
        if !(self.levels[level].open_row == Some(index) && self.levels.len() > level + 1) {
            self.levels.truncate(level + 1);
            self.active_level = self.active_level.min(level);
            let child = Level::new(submenu(), level + 1, window, cx);
            self.levels.push(child);
            self.levels[level].open_row = Some(index);
        }
        if take_focus {
            self.active_level = level + 1;
            let child = &mut self.levels[level + 1];
            child.nav.clear();
            let enabled = child.enabled();
            child.nav.next(&enabled);
            window.focus(&self.entry_focus(cx), cx);
        }
        cx.notify();
    }

    /// Closes level `from` and everything below it.
    fn close_levels(&mut self, from: usize, window: &mut Window, cx: &mut Context<Self>) {
        if from == 0 || from >= self.levels.len() {
            return;
        }
        self.cancel_hover();
        self.levels.truncate(from);
        self.levels[from - 1].open_row = None;
        self.apex = None;
        if self.active_level >= from {
            self.active_level = from - 1;
        }
        window.focus(&self.entry_focus(cx), cx);
        cx.notify();
    }

    /// → : opens the highlighted row's submenu.
    fn open_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keyboard = true;
        let level = self.active_level;
        if let Some(index) = self.levels[level].nav.highlight() {
            self.open_child(level, index, true, window, cx);
        }
    }

    /// ← : closes the active level, unless it is the root.
    fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keyboard = true;
        self.close_levels(self.active_level, window, cx);
    }

    /// Esc: closes the deepest submenu, or at the root the whole menu.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.levels.len() > 1 {
            self.close_levels(self.levels.len() - 1, window, cx);
        } else {
            cx.propagate();
        }
    }

    fn cancel_hover(&mut self) {
        self.pending = None;
        self.hover_task = None;
        self.grace_row = None;
        self.grace_task = None;
    }

    fn set_active(&mut self, level: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_level != level {
            self.active_level = level;
            window.focus(&self.entry_focus(cx), cx);
            cx.notify();
        }
    }

    /// The pointer moved over enabled item `index` of `level`.
    fn hover(
        &mut self,
        level: usize,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.keyboard {
            self.keyboard = false;
            cx.notify();
        }
        let Some(target) = self.levels.get(level) else {
            return;
        };
        if target.open_row == Some(index) {
            // Back on the row that owns the open child: nothing pending
            // applies any more, and the safe triangle starts from here.
            self.apex = Some(position);
            self.cancel_hover();
            self.highlight(level, index, window, cx);
            return;
        }
        if target.open_row.is_some() && self.heading_for_child(level, position) {
            // Passing over this row on the way to the child: keep the child
            // and the highlight; if the pointer stops here, take the row later.
            if self.grace_row != Some((level, index)) {
                self.grace_row = Some((level, index));
                self.grace_task = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(SAFE_GRACE).await;
                    this.update_in(cx, |this, window, cx| {
                        if let Some((level, index)) = this.grace_row.take() {
                            this.switch_row(level, index, window, cx);
                        }
                    })
                    .ok();
                }));
            }
            return;
        }
        self.switch_row(level, index, window, cx);
    }

    /// The pointer settled on item `index` of `level`: highlight it and
    /// schedule what its row implies for the child level.
    fn switch_row(
        &mut self,
        level: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_hover();
        self.highlight(level, index, window, cx);
        let Some(target) = self.levels.get(level) else {
            return;
        };
        let opens = target
            .item(index)
            .is_some_and(|item| item.submenu.is_some());
        let wanted = if opens {
            Some(PendingHover {
                level,
                open: Some(index),
            })
        } else if target.open_row.is_some() {
            Some(PendingHover { level, open: None })
        } else {
            None
        };
        let Some(wanted) = wanted else {
            return;
        };
        self.pending = Some(wanted);
        self.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                let Some(pending) = this.pending.take() else {
                    return;
                };
                match pending.open {
                    Some(index) => this.open_child(pending.level, index, false, window, cx),
                    None => this.close_levels(pending.level + 1, window, cx),
                }
            })
            .ok();
        }));
    }

    fn highlight(
        &mut self,
        level: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active(level, window, cx);
        let nav = &mut self.levels[level].nav;
        if nav.highlight() != Some(index) {
            nav.set(index);
            cx.notify();
        }
    }

    /// Whether the pointer, over a row of `level`, is inside the safe triangle
    /// toward that level's open child.
    fn heading_for_child(&self, level: usize, position: Point<Pixels>) -> bool {
        let (Some(apex), Some(child), Some(parent)) = (
            self.apex,
            self.levels
                .get(level + 1)
                .and_then(|l| l.panel_bounds.get()),
            self.levels[level].panel_bounds.get(),
        ) else {
            return false;
        };
        let flipped = child.left() < parent.left();
        in_safe_triangle(apex, position, child, flipped)
    }

    /// The pointer is over panel `level`: pending decisions made on a
    /// shallower level no longer apply.
    fn enter_panel(&mut self, level: usize) {
        if self.pending.is_some_and(|pending| pending.level < level) {
            self.pending = None;
            self.hover_task = None;
        }
        if self
            .grace_row
            .is_some_and(|(row_level, _)| row_level < level)
        {
            self.grace_row = None;
            self.grace_task = None;
        }
    }

    /// Closes the menu, then runs `action`.
    fn close_and_run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer {
            overlay::close(layer, window, cx);
        }
        action(window, cx);
    }

    /// The panel of `level`: its rows, and a canvas recording its bounds.
    fn render_level(&self, level: usize, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let state = &self.levels[level];
        let panel_bounds = state.panel_bounds.clone();
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(if level == 0 { 220. } else { 240. }))
            .p(px(6.))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_MD)
            .shadow(cx.theme().pop_shadow())
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, _, _| panel_bounds.set(Some(bounds)),
                )
                .absolute()
                .inset_0(),
            );
        if level > 0 {
            // Child panels sit above the layer's full-window mouse catcher: they
            // must block it, or a click inside would count as outside.
            panel = panel.occlude().on_mouse_move(cx.listener(
                move |this, _: &MouseMoveEvent, _, _| {
                    this.enter_panel(level);
                },
            ));
        }

        let visible = visible_items(&state.entries, &state.query);
        let mut item_index = 0;
        for (entry_index, entry) in state.entries.iter().enumerate() {
            match entry {
                MenuEntry::Header(header) => {
                    panel = panel.child(menu_header_row(header, entry_index, colors, cx));
                }
                MenuEntry::Filter { .. } => {
                    if let Some(filter) = &state.filter {
                        panel =
                            panel.child(div().flex_none().px(px(4.)).pt(px(2.)).pb(px(6.)).child(
                                SearchField::new(
                                    SharedString::from(format!("menu-filter-{level}")),
                                    filter.input.clone(),
                                ),
                            ));
                    }
                }
                MenuEntry::Separator => {
                    panel = panel.child(
                        div()
                            .h(px(1.))
                            .flex_none()
                            .my(px(4.))
                            .mx(px(2.))
                            .bg(colors.border),
                    );
                }
                MenuEntry::Item(item) => {
                    let index = item_index;
                    item_index += 1;
                    if !visible[index] {
                        continue;
                    }
                    let highlighted = !item.disabled && state.nav.highlight() == Some(index);
                    let row = menu_item_row(
                        item,
                        RowState {
                            level,
                            index,
                            highlighted,
                            open: state.open_row == Some(index),
                            ring: highlighted && self.keyboard,
                            bounds: state.row_bounds.clone(),
                        },
                        colors,
                        cx,
                    );
                    panel = panel.child(if item.disabled {
                        row
                    } else {
                        row.on_mouse_move(cx.listener(
                            move |this, event: &MouseMoveEvent, window, cx| {
                                this.hover(level, index, event.position, window, cx);
                            },
                        ))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.activate(level, index, window, cx);
                            },
                        ))
                    });
                }
            }
        }
        panel
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let mut measuring = false;
        let mut children = Vec::new();
        for level in 1..self.levels.len() {
            let parent = &self.levels[level - 1];
            let row = parent
                .open_row
                .and_then(|index| parent.row_bounds.borrow().get(&index).copied());
            let (Some(parent_bounds), Some(row)) = (parent.panel_bounds.get(), row) else {
                // The parent has not painted yet; try again next frame.
                measuring = true;
                continue;
            };
            // The panel's own size is only known after it has painted once, and
            // the flip depends on it: draw it invisibly at the unflipped spot
            // for that frame, then place it for real. Notifying from the paint
            // callback is not allowed, so the next frame is requested here.
            let size = self.levels[level].panel_bounds.get().map(|b| b.size);
            let origin = match size {
                Some(size) => submenu_origin(parent_bounds, row, size, viewport),
                None => point(
                    parent_bounds.right() - px(SUBMENU_OVERLAP),
                    row.top() - px(SUBMENU_RISE),
                ),
            };
            measuring |= size.is_none();
            children.push(
                deferred(
                    anchored()
                        .position(origin)
                        .snap_to_window_with_margin(px(WINDOW_MARGIN))
                        .child(self.render_level(level, cx).opacity(if size.is_some() {
                            1.
                        } else {
                            0.
                        })),
                )
                .with_priority(level),
            );
        }
        if measuring {
            window.request_animation_frame();
        }

        div()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .key_context("ui-menu")
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(false, cx)))
            .on_action(cx.listener(|this, _: &MenuOpenSubmenu, window, cx| {
                this.open_highlighted(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MenuCloseSubmenu, window, cx| this.close_active(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.cancel(window, cx)))
            // An open menu absorbs Tab so focus cannot escape behind it: a
            // menu that is no longer focused cannot handle Esc any more.
            .on_action(cx.listener(|_, _: &FocusNext, _, _| {}))
            .on_action(cx.listener(|_, _: &FocusPrev, _, _| {}))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .child(self.render_level(0, cx))
            .children(children)
    }
}

/// A section title with its tool buttons.
fn menu_header_row(
    header: &MenuHeader,
    entry_index: usize,
    colors: crate::theme::ThemeColors,
    cx: &mut Context<MenuView>,
) -> Div {
    // GPUI has no letter-spacing, so the design's .06em tracking is not drawn.
    let mut row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(10.))
        .pt(px(6.))
        .pb(px(4.))
        .font_family(FONT_UI)
        .text_size(FS_XS)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.muted)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(header.label.clone()),
        );
    for (tool_index, tool) in header.tools.iter().enumerate() {
        let selector = format!("menu-tool-{entry_index}-{tool_index}");
        let action = tool.action.clone();
        let (background, icon_color) = if tool.selected {
            (colors.active, colors.accent)
        } else {
            (gpui::transparent_black(), colors.text)
        };
        row = row.child(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.))
                // The design pulls the last button 4px into the header's padding.
                .mr(px(-4.))
                .rounded(R_SM)
                .bg(background)
                .cursor_pointer()
                .hover(move |style| style.bg(colors.hover))
                .active(move |style| style.bg(colors.active))
                .tooltip(Tooltip::text(tool.tooltip.clone()))
                .child(Icon::new(tool.icon).size(px(12.)).color(icon_color))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_and_run(action.clone(), window, cx);
                })),
        );
    }
    row
}

/// What a row needs to know besides its item.
struct RowState {
    level: usize,
    index: usize,
    highlighted: bool,
    /// The row's submenu is open: it keeps the `active` fill and a bright chevron.
    open: bool,
    /// Draw the keyboard focus ring (highlighted by a key press).
    ring: bool,
    /// Where a submenu row records its window bounds for placing the child.
    bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
}

fn menu_item_row(
    item: &MenuItem,
    state: RowState,
    colors: crate::theme::ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    let RowState {
        level,
        index,
        highlighted,
        open,
        ring,
        bounds,
    } = state;
    let cascades = item.submenu.is_some();
    // The open fill wins over the highlight (design: `.menu-item.hi.open`).
    let highlighted = highlighted && !open;
    let foreground = if item.disabled {
        colors.disabled
    } else if open {
        colors.text
    } else if highlighted {
        colors.selection_fg
    } else if item.danger {
        colors.error
    } else {
        colors.text
    };
    let icon_color = if item.disabled {
        colors.disabled
    } else if item.danger {
        colors.error
    } else if highlighted {
        colors.selection_sub
    } else if item.accent_icon {
        colors.accent
    } else {
        colors.muted
    };
    let shortcut_color = if item.disabled {
        colors.disabled
    } else if highlighted {
        colors.selection_sub
    } else {
        colors.muted
    };
    let chevron_color = if item.disabled {
        colors.disabled
    } else if open {
        colors.text
    } else if highlighted {
        colors.selection_sub
    } else {
        colors.muted
    };

    let icon_slot: Div = match item.icon {
        Some(icon) => div()
            .w(px(14.))
            .h(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(icon).size(px(14.)).color(icon_color)),
        None => div().w(px(14.)).h(px(14.)),
    };

    let mut row = div()
        .id(ElementId::named_usize("menu-item", level * 1000 + index))
        .debug_selector(move || format!("menu-{level}-{index}"))
        .relative()
        .h(px(28.))
        // A fixed-height item in the menu's flex column keeps its height.
        .flex_none()
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(R_SM)
        .font_family(FONT_UI)
        .text_size(FS_BASE)
        .text_color(foreground)
        .child(icon_slot)
        .child(div().flex_1().child(item.label.clone()))
        .when_some(item.hint.clone(), |this, hint| {
            this.child(
                div()
                    .flex_none()
                    .ml(px(12.))
                    .text_size(FS_SM)
                    .text_color(shortcut_color)
                    .child(hint),
            )
        })
        .when_some(item.shortcut.clone(), |this, shortcut| {
            this.child(
                div()
                    .text_size(FS_SM)
                    .text_color(shortcut_color)
                    .child(crate::kbd::platform_label(&shortcut)),
            )
        })
        .when(cascades, |this| {
            this.child(
                div().flex_none().ml(px(4.)).child(
                    Icon::new(IconName::ChevR)
                        .size(px(12.))
                        .color(chevron_color),
                ),
            )
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, row_bounds, _, _| {
                        bounds.borrow_mut().insert(index, row_bounds);
                    },
                )
                .absolute()
                .inset_0(),
            )
        });

    if open {
        row = row.bg(colors.active);
    } else if highlighted {
        row = row.bg(colors.selection);
    }
    if ring {
        row = row.child(focus::focus_ring(RingOffset::Inset2, R_SM, cx));
    }
    if item.disabled {
        row = row.cursor_not_allowed();
    } else {
        row = row.cursor_pointer();
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn down_moves_over_enabled_items_and_wraps() {
        let mut nav = MenuNav::new();
        let enabled = [true, false, true, true];
        assert_eq!(nav.highlight(), None);
        nav.next(&enabled);
        assert_eq!(nav.highlight(), Some(0));
        nav.next(&enabled);
        assert_eq!(nav.highlight(), Some(2));
        nav.next(&enabled);
        assert_eq!(nav.highlight(), Some(3));
        nav.next(&enabled);
        assert_eq!(nav.highlight(), Some(0));
    }

    #[test]
    fn up_from_nothing_goes_to_the_last_enabled_item() {
        let mut nav = MenuNav::new();
        nav.prev(&[true, true, false]);
        assert_eq!(nav.highlight(), Some(1));
        nav.prev(&[true, true, false]);
        assert_eq!(nav.highlight(), Some(0));
        nav.prev(&[true, true, false]);
        assert_eq!(nav.highlight(), Some(1));
    }

    #[test]
    fn disabled_items_are_never_highlighted() {
        let mut nav = MenuNav::new();
        let enabled = [false, false];
        nav.next(&enabled);
        assert_eq!(nav.highlight(), None);
        nav.prev(&enabled);
        assert_eq!(nav.highlight(), None);
    }

    fn item(label: &'static str) -> MenuEntry {
        MenuEntry::Item(MenuItem::new(label))
    }

    fn filter() -> MenuEntry {
        MenuEntry::Filter {
            placeholder: "Filter".into(),
        }
    }

    #[test]
    fn items_before_the_filter_and_an_empty_query_stay_visible() {
        let entries = [item("Pinned"), filter(), item("main"), item("dev")];
        assert_eq!(visible_items(&entries, ""), [true, true, true]);
        assert_eq!(visible_items(&entries, "zzz"), [true, false, false]);
    }

    #[test]
    fn filter_matches_case_insensitively_on_the_label_or_filter_text() {
        let entries = [
            filter(),
            item("Feature/Login"),
            MenuEntry::Item(MenuItem::new("origin/x").filter_text("remote hotfix")),
            MenuEntry::Separator,
            item("main"),
        ];
        assert_eq!(visible_items(&entries, "FEATURE"), [true, false, false]);
        assert_eq!(visible_items(&entries, "hotfix"), [false, true, false]);
        // A rule between items does not shift the per-item flags.
        assert_eq!(visible_items(&entries, "ma"), [false, false, true]);
    }

    #[test]
    fn menus_without_a_filter_show_everything() {
        let entries = [
            item("a"),
            MenuEntry::Header(MenuHeader::new("H")),
            item("b"),
        ];
        assert_eq!(visible_items(&entries, "zzz"), [true, true]);
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: gpui::size(px(w), px(h)),
        }
    }

    const VIEWPORT: (f32, f32) = (1000., 800.);

    fn origin(parent: Bounds<Pixels>, row: Bounds<Pixels>, size: (f32, f32)) -> (f32, f32) {
        let at = submenu_origin(
            parent,
            row,
            gpui::size(px(size.0), px(size.1)),
            gpui::size(px(VIEWPORT.0), px(VIEWPORT.1)),
        );
        (f32::from(at.x), f32::from(at.y))
    }

    #[test]
    fn a_child_that_fits_sits_on_the_parents_right_edge_level_with_its_row() {
        let parent = bounds(100., 100., 220., 300.);
        let row = bounds(106., 150., 208., 28.);
        // x = parent.right - 4; y = row.top - 6.
        assert_eq!(origin(parent, row, (240., 200.)), (316., 144.));
    }

    #[test]
    fn a_child_that_does_not_fit_on_the_right_flips_to_the_left() {
        let parent = bounds(700., 100., 220., 300.);
        let row = bounds(706., 150., 208., 28.);
        // 916 + 240 > 1000 - 4, so x = parent.left + 4 - width = 464.
        assert_eq!(origin(parent, row, (240., 200.)), (464., 144.));
        // Exactly fitting stays on the right: 916 + 80 = 996 = 1000 - 4.
        assert_eq!(origin(parent, row, (80., 200.)), (916., 144.));
    }

    #[test]
    fn a_child_is_kept_inside_the_window_vertically() {
        let parent = bounds(100., 100., 220., 300.);
        // Near the bottom: pushed up so its bottom edge keeps the 4px margin.
        let low = bounds(106., 700., 208., 28.);
        assert_eq!(origin(parent, low, (240., 200.)).1, 596.);
        // Near the top: row.top - 6 would be negative, so the margin applies.
        let high = bounds(106., 6., 208., 28.);
        assert_eq!(origin(parent, high, (240., 200.)).1, 4.);
        // Taller than the window: the top edge wins.
        assert_eq!(origin(parent, low, (240., 900.)).1, 4.);
    }

    #[test]
    fn the_safe_triangle_covers_the_diagonal_toward_the_child() {
        let child = bounds(300., 100., 240., 200.);
        let apex = point(px(250.), px(130.));
        // Between the apex and the child's near edge.
        assert!(in_safe_triangle(
            apex,
            point(px(280.), px(140.)),
            child,
            false
        ));
        assert!(in_safe_triangle(
            apex,
            point(px(299.), px(250.)),
            child,
            false
        ));
        // Straight down from the apex, or above the child's top: outside.
        assert!(!in_safe_triangle(
            apex,
            point(px(250.), px(180.)),
            child,
            false
        ));
        assert!(!in_safe_triangle(
            apex,
            point(px(280.), px(90.)),
            child,
            false
        ));
        // Past the child's bottom edge: outside.
        assert!(!in_safe_triangle(
            apex,
            point(px(290.), px(320.)),
            child,
            false
        ));
    }

    #[test]
    fn the_safe_triangle_mirrors_for_a_flipped_child() {
        let child = bounds(0., 100., 240., 200.);
        let apex = point(px(290.), px(130.));
        assert!(in_safe_triangle(
            apex,
            point(px(260.), px(140.)),
            child,
            true
        ));
        assert!(!in_safe_triangle(
            apex,
            point(px(290.), px(180.)),
            child,
            true
        ));
        // Not mirrored: the same points are outside for an unflipped child.
        assert!(!in_safe_triangle(
            apex,
            point(px(260.), px(140.)),
            child,
            false
        ));
    }

    #[test]
    fn an_apex_at_or_past_the_near_edge_has_no_triangle() {
        let child = bounds(300., 100., 240., 200.);
        assert!(!in_safe_triangle(
            point(px(300.), px(130.)),
            point(px(310.), px(150.)),
            child,
            false
        ));
    }

    #[test]
    fn each_level_navigates_over_its_own_enabled_flags() {
        // Two levels keep independent highlights: moving in the child leaves
        // the parent's opener row highlighted.
        let mut parent = MenuNav::new();
        let mut child = MenuNav::new();
        parent.set(1);
        child.next(&[false, true, true]);
        child.next(&[false, true, true]);
        child.next(&[false, true, true]);
        assert_eq!(child.highlight(), Some(1));
        assert_eq!(parent.highlight(), Some(1));
    }

    #[test]
    fn pruning_drops_the_menu_of_a_closed_window() {
        let mut registry = MenuRegistry::default();
        registry
            .by_window
            .insert(WindowId::from(1), LayerId::for_test(1));
        registry
            .by_window
            .insert(WindowId::from(2), LayerId::for_test(2));

        forget(&mut registry, WindowId::from(1));

        assert!(!registry.by_window.contains_key(&WindowId::from(1)));
        assert!(registry.by_window.contains_key(&WindowId::from(2)));
    }
}
