//! Component actions shared by the kit.
//!
//! The bindings are global for now; per-component key contexts come with the
//! components themselves.

use gpui::{actions, App, Div, InteractiveElement, KeyBinding, Window};

actions!(
    ui,
    [
        Cancel,
        Confirm,
        SelectNext,
        SelectPrev,
        SelectFirst,
        SelectLast,
        FocusNext,
        FocusPrev,
    ]
);

/// Binds the shared actions to their default keystrokes.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("enter", Confirm, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("home", SelectFirst, None),
        KeyBinding::new("end", SelectLast, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
    ]);

    // Root handlers cannot run while nothing is focused: the dispatch path is
    // just the root node then. Fall back to the window itself.
    cx.on_action(|_: &FocusNext, cx| move_focus(cx, true));
    cx.on_action(|_: &FocusPrev, cx| move_focus(cx, false));
}

fn move_focus(cx: &mut App, next: bool) {
    // The window is taken out of the app while its input is dispatched, so
    // move focus once the current update has finished.
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        let Some(window) = window else {
            return;
        };
        let _ = window.update(cx, |_, window, _| {
            if next {
                window.focus_next();
            } else {
                window.focus_prev();
            }
        });
    });
}

/// Adds Tab/Shift-Tab focus traversal to a window root.
pub fn tab_navigation(root: Div) -> Div {
    root.on_action(|_: &FocusNext, window: &mut Window, _: &mut App| {
        window.focus_next();
    })
    .on_action(|_: &FocusPrev, window: &mut Window, _: &mut App| {
        window.focus_prev();
    })
}
