//! Component actions shared by the kit.
//!
//! The bindings are global for now; per-component key contexts come with the
//! components themselves.

use std::rc::Rc;

use gpui::{
    actions, Action, App, Div, InteractiveElement, KeyBinding, KeyBindingContextPredicate, Window,
};

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
        TreeExpand,
        TreeCollapse,
        TreeActivate,
    ]
);

/// Builds a keybinding, resolving `secondary-` for the host OS.
///
/// `KeyBinding::new` parses without an OS, so `secondary-` there is an error;
/// bindings that name it must go through `KeyBinding::load`.
pub fn key_binding(
    cx: &App,
    keystrokes: &str,
    action: impl Action,
    context: Option<&str>,
) -> KeyBinding {
    let context = context.map(|context| {
        Rc::new(KeyBindingContextPredicate::parse(context).expect("valid key binding context"))
    });
    KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("valid keystroke")
}

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
        KeyBinding::new("right", TreeExpand, Some("ui-tree")),
        KeyBinding::new("left", TreeCollapse, Some("ui-tree")),
        KeyBinding::new("space", TreeActivate, Some("ui-tree")),
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
        let _ = window.update(cx, |_, window, cx| {
            if next {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
        });
    });
}

/// Adds Tab/Shift-Tab focus traversal to a window root.
pub fn tab_navigation(root: Div) -> Div {
    root.on_action(|_: &FocusNext, window: &mut Window, cx: &mut App| {
        window.focus_next(cx);
    })
    .on_action(|_: &FocusPrev, window: &mut Window, cx: &mut App| {
        window.focus_prev(cx);
    })
}
