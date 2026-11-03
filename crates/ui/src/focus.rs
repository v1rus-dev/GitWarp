//! Keyboard-versus-mouse modality and the shared focus ring.

use gpui::{
    div, px, App, Div, FocusHandle, Global, InteractiveElement, IntoElement, Pixels, Styled, Window,
};

use crate::theme::ActiveTheme;

/// Whether the last input came from the keyboard.
struct FocusState {
    keyboard_modality: bool,
}

impl Global for FocusState {}

/// Installs the modality global and the keystroke observer.
pub(crate) fn init(cx: &mut App) {
    cx.set_global(FocusState {
        keyboard_modality: false,
    });
    cx.observe_keystrokes(|_, _, cx| set_keyboard(true, cx))
        .detach();
}

/// True after the last input was a keystroke, false after the last mouse-down.
pub fn is_keyboard(cx: &App) -> bool {
    cx.global::<FocusState>().keyboard_modality
}

/// Runs `activate` for an Enter press on a focused control.
///
/// Enter is the global `Confirm` action, which swallows the key-down GPUI would
/// pair with the key-up to click a focused element; so a control handles
/// `Confirm` itself (the design's native buttons and checkboxes act on Enter).
/// The focus restore of a layer the activation closes waits for the key-up, so
/// the opener that regains focus is not activated by it.
pub(crate) fn activate_on_enter(
    window: &mut Window,
    cx: &mut App,
    activate: impl FnOnce(&mut Window, &mut App),
) {
    crate::overlay::defer_focus_restore(window, cx);
    activate(window, cx);
    crate::overlay::clear_deferred_focus_restore(window, cx);
}

/// True when `handle` is focused and focus came from the keyboard.
pub fn focus_visible(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    handle.is_focused(window) && is_keyboard(cx)
}

/// Marks the modality as mouse-driven when a mouse-down reaches the root.
pub fn track_pointer(root: Div) -> Div {
    root.capture_any_mouse_down(|_, _, cx| set_keyboard(false, cx))
}

fn set_keyboard(keyboard: bool, cx: &mut App) {
    let changed = {
        let state = cx.global_mut::<FocusState>();
        let changed = state.keyboard_modality != keyboard;
        state.keyboard_modality = keyboard;
        changed
    };
    if changed {
        cx.refresh_windows();
    }
}

/// Where a focus ring sits relative to its parent's box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingOffset {
    /// 1px outside the parent.
    Outset1,
    /// 2px outside the parent.
    Outset2,
    /// 2px inside the parent.
    Inset2,
}

/// A 2px `focus` ring drawn over a relative parent without affecting layout.
pub fn focus_ring(offset: RingOffset, radius: Pixels, cx: &App) -> impl IntoElement {
    let (inset, grow) = match offset {
        RingOffset::Outset1 => (px(-1.), px(1.)),
        RingOffset::Outset2 => (px(-2.), px(2.)),
        RingOffset::Inset2 => (px(2.), px(-2.)),
    };
    let radius = radius + grow;
    let radius = if radius.signum() < 0. { px(0.) } else { radius };

    div()
        .absolute()
        .inset(inset)
        .rounded(radius)
        .border_2()
        .border_color(cx.theme().colors.focus)
}
