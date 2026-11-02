//! Context menus and dropdown menus.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, App, AppContext, BorrowAppContext, Bounds, Context, Div, ElementId,
    FocusHandle, Global, IntoElement, MouseMoveEvent, ParentElement, Pixels, Point, Render,
    SharedString, Stateful, Styled, Window, WindowId,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::actions::{Confirm, FocusNext, FocusPrev, SelectNext, SelectPrev};
use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_UI, FS_BASE, FS_SM, R_MD, R_SM};
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
}

/// A row of a menu: either an item or a rule.
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
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
    open_at(position, entries, window, cx)
}

/// Opens a menu 2px below the bottom-left corner of `anchor`.
pub fn open_dropdown(
    anchor: Bounds<Pixels>,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    let position = point(anchor.left(), anchor.bottom() + px(2.));
    open_at(position, entries, window, cx)
}

fn open_at(
    position: Point<Pixels>,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    close_previous(window, cx);
    let view = cx.new(|cx| MenuView::new(entries, cx));
    let focus = view.read(cx).focus.clone();
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

struct MenuView {
    entries: Vec<MenuEntry>,
    nav: MenuNav,
    focus: FocusHandle,
    layer: Option<LayerId>,
}

impl MenuView {
    fn new(entries: Vec<MenuEntry>, cx: &mut Context<Self>) -> Self {
        Self {
            entries,
            nav: MenuNav::new(),
            focus: cx.focus_handle().tab_stop(false),
            layer: None,
        }
    }

    /// Enabled flags of the items only, in item order.
    fn enabled(&self) -> Vec<bool> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(!item.disabled),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    fn item(&self, index: usize) -> Option<&MenuItem> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item),
                MenuEntry::Separator => None,
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

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.item(index) else {
            return;
        };
        if let Some(submenu) = item.submenu.clone() {
            self.entries = submenu();
            self.nav.clear();
            cx.notify();
            return;
        }
        let action = item.action.clone();
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
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                if let Some(index) = this.nav.highlight() {
                    // Enter closes the menu on key-down; the paired key-up
                    // must not activate the opener that regains focus.
                    overlay::defer_focus_restore(window, cx);
                    this.activate(index, window, cx);
                    overlay::clear_deferred_focus_restore(window, cx);
                }
            }));

        let mut item_index = 0;
        for entry in &self.entries {
            match entry {
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
        .when_some(item.shortcut.clone(), |this, shortcut| {
            this.child(
                div()
                    .text_size(FS_SM)
                    .text_color(shortcut_color)
                    .child(shortcut),
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
