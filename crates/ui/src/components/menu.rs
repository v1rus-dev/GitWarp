//! Context menus and dropdown menus.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, App, AppContext, BorrowAppContext, Bounds, Context, Div, ElementId, Entity,
    FocusHandle, FontWeight, Global, IntoElement, MouseMoveEvent, ParentElement, Pixels, Point,
    Render, SharedString, Stateful, Styled, Subscription, Window, WindowId,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::actions::{Confirm, FocusNext, FocusPrev, SelectNext, SelectPrev};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::components::{SearchField, Tooltip};
use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_UI, FS_BASE, FS_SM, FS_XS, R_MD, R_SM};
use crate::overlay::{self, LayerId};
use crate::theme::ActiveTheme;

/// A menu callback.
pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Builds the rows an item replaces its menu with; called on every activation.
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
    /// When set, activating the item swaps the open menu's rows for these
    /// instead of closing it and running `action`.
    pub submenu: Option<MenuSubmenu>,
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

    /// Makes the item replace the open menu's rows in place (same position,
    /// same focus) instead of closing it. A "Back" row is just another such
    /// item that returns the parent's rows.
    pub fn submenu(mut self, entries: impl Fn() -> Vec<MenuEntry> + 'static) -> Self {
        self.submenu = Some(Rc::new(entries));
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
        if let Some(index) = highlight.filter(|index| menu.enabled().get(*index) == Some(&true)) {
            menu.nav.set(index);
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

/// The filter field of a menu and what it currently holds.
struct FilterField {
    input: Entity<TextInput>,
    _subscription: Subscription,
}

struct MenuView {
    entries: Vec<MenuEntry>,
    nav: MenuNav,
    focus: FocusHandle,
    layer: Option<LayerId>,
    filter: Option<FilterField>,
    query: String,
}

impl MenuView {
    fn new(entries: Vec<MenuEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut menu = Self {
            entries: Vec::new(),
            nav: MenuNav::new(),
            focus: cx.focus_handle().tab_stop(false),
            layer: None,
            filter: None,
            query: String::new(),
        };
        menu.set_entries(entries, window, cx);
        menu
    }

    /// Swaps the rows and rebuilds the filter field for them, if they have one.
    fn set_entries(
        &mut self,
        entries: Vec<MenuEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter = entries
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
                    |this, _, event: &TextInputEvent, window, cx| {
                        match event {
                            TextInputEvent::Changed(text) => this.set_query(text.clone(), cx),
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
        self.entries = entries;
        self.query.clear();
        self.nav.clear();
    }

    /// Where focus goes when the menu opens: the filter field, else the menu.
    fn entry_focus(&self, cx: &App) -> FocusHandle {
        match &self.filter {
            Some(filter) => filter.input.read(cx).focus_handle().clone(),
            None => self.focus.clone(),
        }
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        // A narrowed list highlights its first row so Enter picks it; the old
        // highlight may be hidden now.
        self.nav.clear();
        if !self.query.is_empty() {
            self.nav.next(&self.enabled());
        }
        cx.notify();
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

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let enabled = self.enabled();
        if forward {
            self.nav.next(&enabled);
        } else {
            self.nav.prev(&enabled);
        }
        cx.notify();
    }

    /// Activates the highlighted row (Enter).
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.nav.highlight() {
            // Enter closes the menu on key-down; the paired key-up
            // must not activate the opener that regains focus.
            overlay::defer_focus_restore(window, cx);
            self.activate(index, window, cx);
            overlay::clear_deferred_focus_restore(window, cx);
        }
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.item(index) else {
            return;
        };
        if let Some(submenu) = item.submenu.clone() {
            self.set_entries(submenu(), window, cx);
            // The old field may have held focus; its replacement is not focused.
            window.focus(&self.entry_focus(cx), cx);
            cx.notify();
            return;
        }
        let action = item.action.clone();
        self.close_and_run(action, window, cx);
    }

    /// Closes the menu, then runs `action`.
    fn close_and_run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer {
            overlay::close(layer, window, cx);
        }
        action(window, cx);
    }
}

impl Render for MenuView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mut panel = div()
            .flex()
            .flex_col()
            .min_w(px(220.))
            .p(px(6.))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_MD)
            .shadow(cx.theme().pop_shadow())
            .track_focus(&self.focus)
            .key_context("ui-menu")
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(false, cx)))
            // An open menu absorbs Tab so focus cannot escape behind it: a
            // menu that is no longer focused cannot handle Esc any more.
            .on_action(cx.listener(|_, _: &FocusNext, _, _| {}))
            .on_action(cx.listener(|_, _: &FocusPrev, _, _| {}))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)));

        let visible = visible_items(&self.entries, &self.query);
        let mut item_index = 0;
        for (entry_index, entry) in self.entries.iter().enumerate() {
            match entry {
                MenuEntry::Header(header) => {
                    panel = panel.child(menu_header_row(header, entry_index, colors, cx));
                }
                MenuEntry::Filter { .. } => {
                    if let Some(filter) = &self.filter {
                        panel = panel.child(
                            div()
                                .flex_none()
                                .px(px(4.))
                                .pt(px(2.))
                                .pb(px(6.))
                                .child(SearchField::new("menu-filter", filter.input.clone())),
                        );
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
                    let highlighted = !item.disabled && self.nav.highlight() == Some(index);
                    let row = menu_item_row(item, index, highlighted, colors);
                    panel = panel.child(if item.disabled {
                        row
                    } else {
                        row.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            if this.nav.highlight() != Some(index) {
                                this.nav.set(index);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.activate(index, window, cx);
                            },
                        ))
                    });
                }
            }
        }
        panel
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

fn menu_item_row(
    item: &MenuItem,
    index: usize,
    highlighted: bool,
    colors: crate::theme::ThemeColors,
) -> Stateful<Div> {
    let foreground = if item.disabled {
        colors.disabled
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
        .id(ElementId::named_usize("menu-item", index))
        .debug_selector(move || format!("menu-0-{index}"))
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
        });

    if highlighted {
        row = row.bg(colors.selection);
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
