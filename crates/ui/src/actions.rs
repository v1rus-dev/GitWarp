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
