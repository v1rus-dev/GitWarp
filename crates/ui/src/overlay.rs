//! A minimal per-window overlay stack for modals and popovers.
//!
//! The root view renders [`render_overlays`] as its last child. Openers are
//! responsible for focusing the view they pass in; closing restores the focus
//! that was active before the layer opened.

use std::collections::HashMap;

use gpui::{
    anchored, deferred, div, px, AnyView, App, AppContext, BorrowAppContext, Div, Entity,
    FocusHandle, Global, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    Point, Styled, Window, WindowId,
};

use crate::actions::Cancel;
use crate::theme::{ActiveTheme, ThemeColors};

/// How a layer is placed in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerKind {
    /// Centred over a full-window scrim.
    Modal,
    /// Anchored at a window position.
    Popover { position: Point<Pixels> },
}

/// Identifies one open layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(u64);

#[cfg(test)]
impl LayerId {
    pub(crate) fn for_test(value: u64) -> Self {
        Self(value)
    }
}

/// One open overlay.
struct Layer {
    kind: LayerKind,
    view: AnyView,
    dismiss_on_outside_click: bool,
    restore_focus: Option<FocusHandle>,
}

/// Per-window ordered overlay stack.
pub struct Overlays {
    layers: LayerStack<Layer>,
    /// Holds focus while a keyboard-triggered close waits for its key-up.
    focus: FocusHandle,
    /// Set by a menu or dialog that handles Enter; the next close may then
    /// defer its focus restore until the matching key-up.
    defer_restore_once: bool,
    /// The focus to restore on the next key-up; the outer `Option` records
    /// that a restore is pending, the inner one may be `None` (blur).
    pending_restore: Option<Option<FocusHandle>>,
}

impl Overlays {
    fn new(focus: FocusHandle) -> Self {
        Self {
            layers: LayerStack::default(),
            focus,
            defer_restore_once: false,
            pending_restore: None,
        }
    }

    fn open(&mut self, layer: Layer) -> LayerId {
        // A new layer takes focus itself; never let an older deferred restore
        // steal it back.
        self.pending_restore = None;
        self.defer_restore_once = false;
        self.layers.open(layer)
    }

    /// Removes `id`; also reports whether it was the topmost layer.
    fn close(&mut self, id: LayerId) -> Option<(Layer, bool)> {
        let was_top = self.layers.is_top(id);
        self.layers.close(id).map(|layer| (layer, was_top))
    }

    fn close_top(&mut self) -> Option<Layer> {
        self.clear_deferred_restore();
        self.pending_restore = None;
        self.layers.close_top()
    }

    fn defer_restore(&mut self) {
        self.defer_restore_once = true;
    }

    fn clear_deferred_restore(&mut self) {
        self.defer_restore_once = false;
    }

    fn take_pending_restore(&mut self) -> Option<Option<FocusHandle>> {
        self.pending_restore.take()
    }
}

/// An ordered layer stack, generic so its logic is testable without a window.
struct LayerStack<T> {
    entries: Vec<(LayerId, T)>,
    next_id: u64,
}

impl<T> Default for LayerStack<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 0,
        }
    }
}

impl<T> LayerStack<T> {
    fn open(&mut self, entry: T) -> LayerId {
        self.next_id += 1;
        let id = LayerId(self.next_id);
        self.entries.push((id, entry));
        id
    }

    fn close(&mut self, id: LayerId) -> Option<T> {
        let index = self
            .entries
            .iter()
            .position(|(layer_id, _)| *layer_id == id)?;
        Some(self.entries.remove(index).1)
    }

    fn is_top(&self, id: LayerId) -> bool {
        self.entries
            .last()
            .is_some_and(|(layer_id, _)| *layer_id == id)
    }

    fn close_top(&mut self) -> Option<T> {
        self.entries.pop().map(|(_, entry)| entry)
    }

    #[cfg(test)]
    fn ids(&self) -> Vec<LayerId> {
        self.entries.iter().map(|(id, _)| *id).collect()
    }
}

/// The overlay stack of every window.
#[derive(Default)]
struct OverlayRegistry {
    by_window: HashMap<WindowId, Entity<Overlays>>,
}

impl Global for OverlayRegistry {}

/// Drops the overlay state of a window that closed.
pub(crate) fn init(cx: &mut App) {
    cx.on_window_closed(|cx, window_id| {
        if cx.try_global::<OverlayRegistry>().is_some() {
            cx.update_global::<OverlayRegistry, _>(|registry, _| {
                registry.by_window.remove(&window_id);
            });
        }
    })
    .detach();
}

/// The focus a new layer should return to when it closes.
///
/// If focus is currently on the overlay sentinel while a keyboard-triggered
/// restore is pending, the real opener is used instead.
fn restore_target(window: &Window, cx: &mut App) -> Option<FocusHandle> {
    let focused = window.focused(cx);
    let Some(overlays) = try_entity(window, cx) else {
        return focused;
    };
    let overlays = overlays.read(cx);
    if focused.as_ref() == Some(&overlays.focus) {
        overlays.pending_restore.clone().flatten().or(focused)
    } else {
        focused
    }
}

/// Opens a centred modal with a full-window scrim.
pub fn open_modal(view: AnyView, window: &mut Window, cx: &mut App) -> LayerId {
    let restore_focus = restore_target(window, cx);
    let id = entity(window, cx).update(cx, |overlays, _| {
        overlays.open(Layer {
            kind: LayerKind::Modal,
            view,
            dismiss_on_outside_click: true,
            restore_focus,
        })
    });
    window.refresh();
    id
}

/// Opens a popover anchored at a window position.
pub fn open_popover(
    position: Point<Pixels>,
    view: AnyView,
    window: &mut Window,
    cx: &mut App,
) -> LayerId {
    let restore_focus = restore_target(window, cx);
    let id = entity(window, cx).update(cx, |overlays, _| {
        overlays.open(Layer {
            kind: LayerKind::Popover { position },
            view,
            dismiss_on_outside_click: true,
            restore_focus,
        })
    });
    window.refresh();
    id
}

/// Defers the focus restore of the next close until the key-up of the key
/// press that triggered it. Call this from an Enter/Space handler before
/// closing: GPUI activates a focused button on key-up, so restoring focus
/// immediately would activate the opener a second time.
pub fn defer_focus_restore(window: &Window, cx: &mut App) {
    if let Some(overlays) = try_entity(window, cx) {
        overlays.update(cx, |overlays, _| overlays.defer_restore());
    }
}

/// Cancels a deferral registered by [`defer_focus_restore`] when the key
/// press did not end up closing a layer.
pub fn clear_deferred_focus_restore(window: &Window, cx: &mut App) {
    if let Some(overlays) = try_entity(window, cx) {
        overlays.update(cx, |overlays, _| overlays.clear_deferred_restore());
    }
}

/// Closes the layer with `id`, restoring the focus saved when it opened.
///
/// Closing a layer that is not on top leaves the top layer's focus alone.
pub fn close(id: LayerId, window: &mut Window, cx: &mut App) {
    let Some(overlays) = try_entity(window, cx) else {
        return;
    };
    let mut deferred = false;
    let closed = overlays.update(cx, |overlays, _| {
        let Some((layer, was_top)) = overlays.close(id) else {
            overlays.clear_deferred_restore();
            return None;
        };
        if was_top && overlays.defer_restore_once {
            overlays.clear_deferred_restore();
            overlays.pending_restore = Some(layer.restore_focus.clone());
            deferred = true;
        } else {
            overlays.clear_deferred_restore();
        }
        Some((layer, was_top))
    });
    if let Some((layer, was_top)) = closed {
        if deferred {
            let focus = overlays.read(cx).focus.clone();
            window.focus(&focus, cx);
        } else if was_top {
            restore(layer, window, cx);
        }
        window.refresh();
    }
}

/// Closes the topmost layer; returns whether one was open.
pub fn close_top(window: &mut Window, cx: &mut App) -> bool {
    let Some(overlays) = try_entity(window, cx) else {
        return false;
    };
    let Some(layer) = overlays.update(cx, |overlays, _| overlays.close_top()) else {
        return false;
    };
    restore(layer, window, cx);
    window.refresh();
    true
}

fn restore(layer: Layer, window: &mut Window, cx: &mut App) {
    if let Some(handle) = layer.restore_focus {
        window.focus(&handle, cx);
    }
}

/// Renders every layer for `window`, bottom-to-top. Add it as the root's last child.
pub fn render_overlays(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let overlays = entity(window, cx);
    let colors = cx.theme().colors;
    let focus = overlays.read(cx).focus.clone();
    let layers: Vec<(LayerId, LayerKind, AnyView, bool)> = overlays
        .read(cx)
        .layers
        .entries
        .iter()
        .map(|(id, layer)| {
            (
                *id,
                layer.kind,
                layer.view.clone(),
                layer.dismiss_on_outside_click,
            )
        })
        .collect();

    // A zero-sized focus sentinel receives the key-up of a keyboard-triggered
    // close and only then returns focus to the opener, so that key-up cannot
    // activate it. It must not cover the window: that would steal mouse-down
    // focus from everything underneath the overlay root.
    let key_up_overlays = overlays.clone();
    let sentinel_focus = focus.clone();
    let sentinel = div()
        .size(px(0.))
        .track_focus(&focus)
        .capture_key_up(move |_, window, cx| {
            let pending = key_up_overlays.update(cx, |overlays, _| overlays.take_pending_restore());
            // Something else took focus deliberately (for example a menu
            // action); leave it alone.
            if window.focused(cx).as_ref() != Some(&sentinel_focus) {
                return;
            }
            if let Some(handle) = pending {
                match handle {
                    Some(handle) => window.focus(&handle, cx),
                    None => window.blur(cx),
                }
                window.refresh();
            }
        });
    let mut root = div().absolute().inset_0().child(sentinel);
    for (index, (id, kind, view, dismiss)) in layers.into_iter().enumerate() {
        let layer = layer_element(id, kind, view, dismiss, colors);
        root = root.child(deferred(layer).with_priority(index));
    }
    if let Some(toasts) = crate::components::toast::render_stack(window, cx) {
        root = root.child(deferred(toasts).with_priority(usize::MAX));
    }
    root
}

fn layer_element(
    id: LayerId,
    kind: LayerKind,
    view: AnyView,
    dismiss_on_outside_click: bool,
    colors: ThemeColors,
) -> Div {
    match kind {
        LayerKind::Modal => modal_layer(id, view, dismiss_on_outside_click, colors),
        LayerKind::Popover { position } => {
            popover_layer(id, position, view, dismiss_on_outside_click)
        }
    }
}

fn modal_layer(
    id: LayerId,
    view: AnyView,
    dismiss_on_outside_click: bool,
    colors: ThemeColors,
) -> Div {
    let mut scrim = div().absolute().inset_0().bg(colors.overlay).occlude();
    if dismiss_on_outside_click {
        scrim = scrim.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            close(id, window, cx);
        });
    }

    div()
        .absolute()
        .inset_0()
        .on_action(move |_: &Cancel, window, cx| close(id, window, cx))
        .child(scrim)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(view),
                ),
        )
}

fn popover_layer(
    id: LayerId,
    position: Point<Pixels>,
    view: AnyView,
    dismiss_on_outside_click: bool,
) -> Div {
    let mut catcher = div().absolute().inset_0().occlude();
    if dismiss_on_outside_click {
        catcher = catcher
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                close(id, window, cx);
            })
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                close(id, window, cx);
            });
    }

    div()
        .absolute()
        .inset_0()
        .on_action(move |_: &Cancel, window, cx| close(id, window, cx))
        .child(catcher)
        .child(
            anchored()
                .position(position)
                .snap_to_window_with_margin(px(4.))
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(view),
                ),
        )
}

/// The registry entry for `window`, created on first use.
fn entity(window: &Window, cx: &mut App) -> Entity<Overlays> {
    let window_id = window.window_handle().window_id();
    if let Some(entity) = cx
        .try_global::<OverlayRegistry>()
        .and_then(|registry| registry.by_window.get(&window_id))
    {
        return entity.clone();
    }

    if cx.try_global::<OverlayRegistry>().is_none() {
        cx.set_global(OverlayRegistry::default());
    }
    let entity = cx.new(|cx| Overlays::new(cx.focus_handle().tab_stop(false)));
    cx.update_global::<OverlayRegistry, _>(|registry, _| {
        registry.by_window.insert(window_id, entity.clone());
    });
    entity
}

fn try_entity(window: &Window, cx: &App) -> Option<Entity<Overlays>> {
    let window_id = window.window_handle().window_id();
    cx.try_global::<OverlayRegistry>()?
        .by_window
        .get(&window_id)
        .cloned()
}

/// Number of open layers in `window`; zero if nothing has been opened.
pub fn layer_count(window: &Window, cx: &App) -> usize {
    try_entity(window, cx).map_or(0, |overlays| overlays.read(cx).layers.entries.len())
}

/// The view of layer `id`, if it is open in `window`.
pub fn layer_view(id: LayerId, window: &Window, cx: &App) -> Option<AnyView> {
    let overlays = try_entity(window, cx)?;
    overlays
        .read(cx)
        .layers
        .entries
        .iter()
        .find(|(layer_id, _)| *layer_id == id)
        .map(|(_, layer)| layer.view.clone())
}

/// Enables or disables closing a layer by clicking outside it.
pub fn set_dismiss_on_outside_click(id: LayerId, dismiss: bool, window: &mut Window, cx: &mut App) {
    let Some(overlays) = try_entity(window, cx) else {
        return;
    };
    let changed = overlays.update(cx, |overlays, _| {
        if let Some((_, layer)) = overlays
            .layers
            .entries
            .iter_mut()
            .find(|(layer_id, _)| *layer_id == id)
        {
            layer.dismiss_on_outside_click = dismiss;
            true
        } else {
            false
        }
    });
    if changed {
        window.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestLayer {
        restore_focus: Option<&'static str>,
    }

    impl TestLayer {
        fn new(restore_focus: Option<&'static str>) -> Self {
            Self { restore_focus }
        }
    }

    #[test]
    fn close_top_closes_the_second_layer_and_returns_its_restore_focus() {
        let mut layers = LayerStack::default();
        let first = layers.open(TestLayer::new(None));
        let second = layers.open(TestLayer::new(Some("B")));

        let closed = layers.close_top().expect("a layer was open");
        assert_eq!(closed.restore_focus, Some("B"));
        assert_eq!(layers.ids(), vec![first]);
        assert_ne!(first, second);
    }

    #[test]
    fn close_by_id_in_the_middle_keeps_the_order() {
        let mut layers = LayerStack::default();
        let first = layers.open(TestLayer::new(None));
        let middle = layers.open(TestLayer::new(None));
        let last = layers.open(TestLayer::new(None));

        assert!(layers.close(middle).is_some());
        assert_eq!(layers.ids(), vec![first, last]);
        assert!(layers.close(middle).is_none());
    }
}
