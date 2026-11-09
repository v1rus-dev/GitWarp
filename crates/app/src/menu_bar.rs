//! The in-window menu bar (Windows and Linux): the application menu model of
//! [`crate::app_menu`] as `File  View  Repository  Window` in the title strip.
//!
//! A click opens the menu's dropdown under its name; while one is open the
//! pointer switches menus by hovering the other names. Rows show the live
//! shortcut of their action and are greyed out when the focused view does not
//! handle the action (the same check the macOS menu bar makes).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, px, Action, App, BorrowAppContext, Bounds, DispatchPhase, Global,
    InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, ParentElement, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, WindowId,
};
use ui::components::menu::{self, MenuEntry, MenuItem};
use ui::metrics::{FONT_UI, FS_BASE, R_SM};
use ui::overlay::{self, LayerId};
use ui::theme::ActiveTheme;

use crate::app_menu::{self, Entry};

/// One row of a dropdown, with everything the live window decided about it.
#[derive(Clone)]
pub(crate) enum Row {
    Separator,
    Item {
        label: SharedString,
        shortcut: Option<String>,
        disabled: bool,
        action: Rc<dyn Action>,
    },
    Submenu {
        label: SharedString,
        rows: Rc<Vec<Row>>,
    },
}

/// Turns model entries into rows. `shortcut` and `available` are the window
/// lookups, passed in so the conversion is testable without a window.
pub(crate) fn rows(
    entries: &[Entry],
    shortcut: &dyn Fn(&dyn Action) -> Option<String>,
    available: &dyn Fn(&dyn Action) -> bool,
) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator => Row::Separator,
            Entry::Item { label, action } => Row::Item {
                label: label.clone(),
                shortcut: shortcut(action.as_ref()),
                disabled: !available(action.as_ref()),
                action: Rc::from(action.boxed_clone()),
            },
            Entry::Submenu(spec) => Row::Submenu {
                label: spec.name.clone(),
                rows: Rc::new(rows(&spec.entries, shortcut, available)),
            },
        })
        .collect()
}

/// The UI-kit entries of `rows`; choosing an item dispatches its action once
/// the menu has closed, so the focus is back in the workspace.
fn to_entries(rows: &[Row]) -> Vec<MenuEntry> {
    rows.iter()
        .map(|row| match row {
            Row::Separator => MenuEntry::Separator,
            Row::Item {
                label,
                shortcut,
                disabled,
                action,
            } => {
                let action = action.clone();
                let mut item =
                    MenuItem::new(label.clone())
                        .disabled(*disabled)
                        .action(move |window, cx| {
                            let action = action.boxed_clone();
                            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
                        });
                if let Some(shortcut) = shortcut {
                    item = item.shortcut(shortcut.clone());
                }
                MenuEntry::Item(item)
            }
            Row::Submenu { label, rows } => {
                let rows = rows.clone();
                MenuEntry::Item(MenuItem::new(label.clone()).submenu(move || to_entries(&rows)))
            }
        })
        .collect()
}

/// Splits `ctrl-shift-,` into (`ctrl-shift`, `,`); the key may be `-` itself.
fn split_key(keystroke: &str) -> (&str, &str) {
    if let Some(modifiers) = keystroke.strip_suffix("--") {
        (modifiers, "-")
    } else {
        match keystroke.rfind('-') {
            Some(0) | None => ("", keystroke),
            Some(at) => (&keystroke[..at], &keystroke[at + 1..]),
        }
    }
}

fn is_punctuation(key: &str) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_alphanumeric())
}

/// Which spelling of a keystroke to show.
///
/// A binding has a display form (`display`) and the authored form (`inner`).
/// On Windows the display form follows the keyboard layout: on a layout where
/// `,` is typed with Shift on another key, `ctrl-,` reads `ctrl-shift-.`. For
/// a punctuation key whose display form gained a Shift the authored form is
/// the one the user expects; letters and everything else keep the display form
/// (the authored form drops Shift there).
fn pick_spelling(display: &str, inner: &str) -> String {
    let (display_mods, display_key) = split_key(display);
    let (inner_mods, inner_key) = split_key(inner);
    let has_shift = |mods: &str| mods.split('-').any(|m| m == "shift");
    if has_shift(display_mods)
        && !has_shift(inner_mods)
        && is_punctuation(display_key)
        && is_punctuation(inner_key)
    {
        inner.to_owned()
    } else {
        display.to_owned()
    }
}

/// The shortcut text of an action's highest-precedence binding (`Ctrl+Shift+F`).
fn shortcut_text(window: &Window, action: &dyn Action) -> Option<String> {
    let binding = window.bindings_for_action(action).into_iter().last()?;
    let os = if cfg!(target_os = "windows") {
        gpui::OperatingSystem::Windows
    } else {
        gpui::OperatingSystem::Linux
    };
    let text = binding
        .keystrokes()
        .iter()
        .map(|keystroke| {
            let shown = pick_spelling(&keystroke.unparse(os), &keystroke.inner().unparse(os));
            crate::settings_ui::keybindings::display(&shown)
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

/// The rows of every top-level menu, read while the focus is still where the
/// user left it (an open dropdown takes the focus, which would skew the
/// availability of the actions).
fn snapshot(window: &Window, cx: &mut App) -> Rc<Vec<Rc<Vec<Row>>>> {
    let menus = app_menu::menus(false);
    let shortcut = |action: &dyn Action| shortcut_text(window, action);
    // Inside a window update the app-level check cannot reach the window and
    // falls back to the global listeners (Quit), which the window check misses.
    let globals: Vec<bool> = menus
        .iter()
        .flat_map(|menu| &menu.entries)
        .filter_map(|entry| match entry {
            Entry::Item { action, .. } => Some(cx.is_action_available(action.as_ref())),
            _ => None,
        })
        .collect();
    let mut globals = globals.into_iter();
    let cx = &*cx;
    let available = |action: &dyn Action| window.is_action_available(action, cx);
    // The handler lives on the workspace root, so availability cannot tell
    // that there is nothing to close on the welcome screen.
    let repository_shown = window
        .root::<crate::workspace::Workspace>()
        .flatten()
        .is_some_and(|workspace| workspace.read(cx).shows_repository());
    Rc::new(
        menus
            .iter()
            .map(|menu| {
                let mut built = rows(&menu.entries, &shortcut, &available);
                // Items are visited in the same order as `globals` was built.
                for row in &mut built {
                    if let Row::Item {
                        disabled, action, ..
                    } = row
                    {
                        let global = globals.next().unwrap_or(false);
                        *disabled = *disabled && !global;
                        if action.as_any().is::<crate::workspace::CloseRepository>() {
                            *disabled = !repository_shown;
                        }
                    }
                }
                Rc::new(built)
            })
            .collect(),
    )
}

type Snapshot = Rc<Vec<Rc<Vec<Row>>>>;

/// The dropdown a window's bar has open.
struct OpenMenu {
    index: usize,
    layer: LayerId,
    snapshot: Snapshot,
}

#[derive(Default)]
struct OpenMenus {
    by_window: HashMap<WindowId, OpenMenu>,
}

impl Global for OpenMenus {}

/// The open dropdown of this window's bar, if its layer is still showing.
fn open_menu(window: &Window, cx: &App) -> Option<(usize, Snapshot)> {
    let open = cx
        .try_global::<OpenMenus>()?
        .by_window
        .get(&window.window_handle().window_id())?;
    overlay::layer_view(open.layer, window, cx)?;
    Some((open.index, open.snapshot.clone()))
}

fn show_menu(
    index: usize,
    snapshot: Snapshot,
    anchor: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(rows) = snapshot.get(index) else {
        return;
    };
    let layer = menu::open_dropdown(anchor, to_entries(rows), window, cx);
    let window_id = window.window_handle().window_id();
    if cx.try_global::<OpenMenus>().is_none() {
        cx.set_global(OpenMenus::default());
    }
    cx.update_global::<OpenMenus, _>(|open, _| {
        open.by_window.insert(
            window_id,
            OpenMenu {
                index,
                layer,
                snapshot,
            },
        );
    });
}

/// The index of the button whose bounds contain `position`.
fn button_at(buttons: &[Bounds<Pixels>], position: gpui::Point<Pixels>) -> Option<usize> {
    buttons.iter().position(|bounds| bounds.contains(&position))
}

/// The bar itself: one text button per top-level menu.
#[derive(IntoElement)]
pub(crate) struct MenuBar;

impl RenderOnce for MenuBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let names: Vec<SharedString> = app_menu::menus(false)
            .into_iter()
            .map(|menu| menu.name)
            .collect();
        let open_index = open_menu(window, cx).map(|(index, _)| index);
        let bounds = Rc::new(RefCell::new(vec![Bounds::default(); names.len()]));

        let mut bar = div()
            .relative()
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(4.))
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .text_color(colors.text);
        for (index, name) in names.into_iter().enumerate() {
            let open = open_index == Some(index);
            let click_bounds = bounds.clone();
            let paint_bounds = bounds.clone();
            bar = bar.child(
                div()
                    .id(("menu-bar", index))
                    .relative()
                    .h(px(24.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded(R_SM)
                    .cursor_pointer()
                    .when(open, |button| button.bg(colors.active))
                    .when(!open, |button| {
                        button.hover(move |style| style.bg(colors.hover))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        let anchor = click_bounds.borrow()[index];
                        let snapshot = snapshot(window, cx);
                        show_menu(index, snapshot, anchor, window, cx);
                    })
                    .child(name)
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |_, painted, _, _| paint_bounds.borrow_mut()[index] = painted,
                        )
                        .absolute()
                        .inset_0(),
                    ),
            );
        }

        // While a dropdown is open its overlay occludes the buttons, so they
        // get no hover of their own: watch the pointer at the window level and
        // switch menus when it crosses another name.
        let hover_bounds = bounds;
        bar.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let hover_bounds = hover_bounds.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let Some((current, snapshot)) = open_menu(window, cx) else {
                            return;
                        };
                        let hit = button_at(&hover_bounds.borrow(), event.position);
                        if let Some(index) = hit.filter(|index| *index != current) {
                            let anchor = hover_bounds.borrow()[index];
                            show_menu(index, snapshot, anchor, window, cx);
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(action: &dyn Action) -> String {
        action.name().rsplit("::").next().unwrap().to_owned()
    }

    fn built() -> Vec<Vec<Row>> {
        let shortcut = |action: &dyn Action| match name_of(action).as_str() {
            "OpenRepositoryDialog" => Some("Ctrl+O".to_owned()),
            _ => None,
        };
        let available = |action: &dyn Action| name_of(action) != "FetchAll";
        app_menu::menus(false)
            .iter()
            .map(|menu| rows(&menu.entries, &shortcut, &available))
            .collect()
    }

    fn items(rows: &[Row]) -> Vec<(&str, Option<&str>, bool)> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Item {
                    label,
                    shortcut,
                    disabled,
                    ..
                } => Some((label.as_ref(), shortcut.as_deref(), *disabled)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rows_mirror_the_model() {
        let menus = app_menu::menus(false);
        let built = built();
        assert_eq!(built.len(), menus.len());
        for (menu, built) in menus.iter().zip(&built) {
            assert_eq!(menu.entries.len(), built.len());
            for (entry, row) in menu.entries.iter().zip(built) {
                assert_eq!(
                    matches!(entry, Entry::Separator),
                    matches!(row, Row::Separator)
                );
            }
        }
    }

    #[test]
    fn shortcut_and_availability_come_from_the_lookups() {
        let built = built();
        let file = items(&built[0]);
        assert_eq!(file[0].1, Some("Ctrl+O"));
        assert!(file
            .iter()
            .skip(1)
            .all(|(_, shortcut, _)| shortcut.is_none()));
        assert!(file.iter().all(|(_, _, disabled)| !disabled));
        let repository = items(&built[2]);
        let disabled: Vec<_> = repository.iter().filter(|(_, _, d)| *d).collect();
        assert_eq!(disabled.len(), 1);
    }

    #[test]
    fn submenus_convert_recursively() {
        let nested = [Entry::Submenu(app_menu::MenuSpec {
            name: "More".into(),
            entries: vec![Entry::Item {
                label: "Quit".into(),
                action: Box::new(crate::Quit),
            }],
        })];
        let built = rows(&nested, &|_| None, &|_| false);
        let Row::Submenu { label, rows } = &built[0] else {
            panic!("expected a submenu");
        };
        assert_eq!(label.as_ref(), "More");
        assert_eq!(items(rows), vec![("Quit", None, true)]);
    }

    #[test]
    fn punctuation_keeps_the_authored_spelling() {
        let shown =
            |d: &str, i: &str| crate::settings_ui::keybindings::display(&pick_spelling(d, i));
        // Windows layout where `,` needs Shift: `ctrl-,` is displayed as ctrl-shift-.
        assert_eq!(shown("ctrl-shift-.", "ctrl-,"), "Ctrl+,");
        assert_eq!(shown("ctrl-,", "ctrl-,"), "Ctrl+,");
        // Letters keep Shift (the authored form dropped it).
        assert_eq!(shown("ctrl-shift-f", "ctrl-F"), "Ctrl+Shift+F");
        assert_eq!(shown("f5", "f5"), "F5");
        assert_eq!(shown("ctrl-alt-w", "ctrl-alt-w"), "Ctrl+Alt+W");
        // A literal minus key.
        assert_eq!(split_key("ctrl--"), ("ctrl", "-"));
        assert_eq!(split_key("-"), ("", "-"));
    }

    #[test]
    fn button_hit_test() {
        let at = |x: f32| gpui::point(px(x), px(5.));
        let button = |x: f32| Bounds {
            origin: gpui::point(px(x), px(0.)),
            size: gpui::size(px(40.), px(24.)),
        };
        let buttons = [button(0.), button(40.)];
        assert_eq!(button_at(&buttons, at(10.)), Some(0));
        assert_eq!(button_at(&buttons, at(50.)), Some(1));
        assert_eq!(button_at(&buttons, at(90.)), None);
    }
}
