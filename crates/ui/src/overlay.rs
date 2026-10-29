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

/// One open overlay.
struct Layer {
    kind: LayerKind,
    view: AnyView,
    dismiss_on_outside_click: bool,
    restore_focus: Option<FocusHandle>,
}

/// Per-window ordered overlay stack.
#[derive(Default)]
pub struct Overlays {
    layers: LayerStack<Layer>,
}

impl Overlays {
    fn open(&mut self, layer: Layer) -> LayerId {
        self.layers.open(layer)
    }

    fn close(&mut self, id: LayerId) -> Option<Layer> {
        self.layers.close(id)
    }

    fn close_top(&mut self) -> Option<Layer> {
        self.layers.close_top()
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

/// Opens a centred modal with a full-window scrim.
pub fn open_modal(view: AnyView, window: &mut Window, cx: &mut App) -> LayerId {
    let restore_focus = window.focused(cx);
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
    let restore_focus = window.focused(cx);
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

/// Closes the layer with `id`, restoring the focus saved when it opened.
pub fn close(id: LayerId, window: &mut Window, cx: &mut App) {
    let Some(overlays) = try_entity(window, cx) else {
        return;
    };
    let closed = overlays.update(cx, |overlays, _| overlays.close(id));
    if let Some(layer) = closed {
        restore(layer, window);
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
    restore(layer, window);
    window.refresh();
    true
}

fn restore(layer: Layer, window: &mut Window) {
    if let Some(handle) = layer.restore_focus {
        window.focus(&handle);
    }
}

/// Renders every layer for `window`, bottom-to-top. Add it as the root's last child.
pub fn render_overlays(_window: &mut Window, cx: &mut App) -> impl IntoElement {
    let overlays = entity(_window, cx);
    let colors = cx.theme().colors;
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

    let mut root = div().absolute().inset_0();
    for (index, (id, kind, view, dismiss)) in layers.into_iter().enumerate() {
        let layer = layer_element(id, kind, view, dismiss, colors);
        root = root.child(deferred(layer).with_priority(index));
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
        catcher = catcher.on_mouse_down(MouseButton::Left, move |_, window, cx| {
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
    let entity = cx.new(|_| Overlays::default());
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
