//! Resizable panel splitters.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::MouseDownEvent;
use gpui::{
    div, px, App, AppContext, DragMoveEvent, ElementId, EmptyView, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, RenderOnce, StatefulInteractiveElement, Styled,
    Window,
};

use crate::theme::ActiveTheme;

/// Which edge a splitter resizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// A 5px-wide handle that resizes the panel's width.
    Vertical,
    /// A 5px-tall handle that resizes the panel's height.
    Horizontal,
}

#[derive(Default)]
struct SplitterState {
    start: Option<Point<Pixels>>,
    start_size: f32,
    dragging: bool,
}

/// The id of the splitter whose drag is active.
///
/// `on_drag_move` fires on every splitter as soon as any drag is active, so
/// the payload must say which one owns it.
#[derive(Clone)]
struct SplitterDrag(ElementId);

/// Clamp a drag delta to the panel's allowed range.
///
/// An inverted splitter (a panel docked right or bottom) grows when the
/// pointer moves left or up.
pub fn clamp_size(
    start: Pixels,
    delta: Pixels,
    min: Pixels,
    max: Pixels,
    inverted: bool,
) -> Pixels {
    let delta: f32 = delta.into();
    let delta = if inverted { -delta } else { delta };
    let start: f32 = start.into();
    let min: f32 = min.into();
    let max: f32 = max.into();
    px((start + delta).clamp(min, max))
}

type ResizeHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App) + 'static>;
type ResetHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// A draggable splitter handle.
#[derive(IntoElement)]
pub struct Splitter {
    id: ElementId,
    axis: SplitAxis,
    size: Pixels,
    min: Pixels,
    max: Pixels,
    inverted: bool,
    on_resize: Option<ResizeHandler>,
    on_reset: Option<ResetHandler>,
}

impl Splitter {
    pub fn vertical(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            axis: SplitAxis::Vertical,
            size: px(240.),
            min: px(160.),
            max: px(420.),
            inverted: false,
            on_resize: None,
            on_reset: None,
        }
    }

    pub fn horizontal(id: impl Into<ElementId>) -> Self {
        Self {
            axis: SplitAxis::Horizontal,
            ..Self::vertical(id)
        }
    }

    /// The panel's current size along the split axis.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// The caller's allowed size range.
    pub fn bounds(mut self, min: Pixels, max: Pixels) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// For panels docked right or bottom: pointer movement is mirrored.
    pub fn inverted(mut self, inverted: bool) -> Self {
        self.inverted = inverted;
        self
    }

    pub fn on_resize(mut self, listener: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(listener));
        self
    }

    /// Double-click resets the panel to its default size.
    pub fn on_reset(mut self, listener: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for Splitter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let vertical = self.axis == SplitAxis::Vertical;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SplitterState::default());
        let dragging = state.read(cx).dragging;
        let debug = format!("{}-handle", self.id);

        let line = div()
            .absolute()
            .rounded(px(1.))
            .bg(colors.accent)
            .when(vertical, |this| {
                this.left(px(1.5)).top(px(0.)).bottom(px(0.)).w(px(2.))
            })
            .when(!vertical, |this| {
                this.top(px(1.5)).left(px(0.)).right(px(0.)).h(px(2.))
            });
        let line = if dragging {
            line.opacity(1.)
        } else {
            line.opacity(0.)
                .group_hover("ui-splitter", |style| style.opacity(1.))
        };

        let size = self.size;
        let min = self.min;
        let max = self.max;
        let inverted = self.inverted;

        let mut handle = div()
            .id(self.id.clone())
            .group("ui-splitter")
            .relative()
            .flex_none()
            .debug_selector(move || debug)
            .when(vertical, |this| {
                this.w(px(5.))
                    .h_full()
                    .ml(px(-2.))
                    .mr(px(-3.))
                    .cursor_col_resize()
            })
            .when(!vertical, |this| {
                this.h(px(5.))
                    .w_full()
                    .mt(px(-2.))
                    .mb(px(-3.))
                    .cursor_row_resize()
            })
            .child(line);

        {
            let state = state.clone();
            let on_reset = self.on_reset.clone();
            handle = handle.on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    if event.click_count >= 2 {
                        if let Some(listener) = &on_reset {
                            listener(window, cx);
                        }
                        return;
                    }
                    state.update(cx, |state, _| {
                        state.start = Some(event.position);
                        state.start_size = f32::from(size);
                        state.dragging = false;
                    });
                },
            );
        }

        handle = handle.on_drag(SplitterDrag(self.id.clone()), |_, _, _, cx| {
            cx.new(|_| EmptyView)
        });
        {
            let state = state.clone();
            let on_resize = self.on_resize.clone();
            let drag_id = self.id.clone();
            handle = handle.on_drag_move(
                move |event: &DragMoveEvent<SplitterDrag>, window: &mut Window, cx: &mut App| {
                    if event.drag(cx).0 != drag_id {
                        return;
                    }
                    let Some(start) = state.read(cx).start else {
                        return;
                    };
                    let start_size = px(state.read(cx).start_size);
                    let delta = if vertical {
                        event.event.position.x - start.x
                    } else {
                        event.event.position.y - start.y
                    };
                    let next = clamp_size(start_size, delta, min, max, inverted);
                    state.update(cx, |state, _| state.dragging = true);
                    if let Some(listener) = &on_resize {
                        listener(next, window, cx);
                    }
                    window.refresh();
                },
            );
        }
        {
            let up_state = state.clone();
            handle = handle.on_mouse_up(
                MouseButton::Left,
                move |_, window: &mut Window, cx: &mut App| {
                    up_state.update(cx, |state, _| {
                        state.dragging = false;
                        state.start = None;
                    });
                    window.refresh();
                },
            );
            let out_state = state.clone();
            handle = handle.on_mouse_up_out(
                MouseButton::Left,
                move |_, window: &mut Window, cx: &mut App| {
                    out_state.update(cx, |state, _| {
                        state.dragging = false;
                        state.start = None;
                    });
                    window.refresh();
                },
            );
        }

        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_adds_its_delta_and_clamps_to_the_bounds() {
        assert_eq!(
            clamp_size(px(240.), px(40.), px(160.), px(420.), false),
            px(280.)
        );
        assert_eq!(
            clamp_size(px(240.), px(400.), px(160.), px(420.), false),
            px(420.),
        );
        assert_eq!(
            clamp_size(px(240.), px(-400.), px(160.), px(420.), false),
            px(160.),
        );
    }

    #[test]
    fn an_inverted_splitter_mirrors_the_delta() {
        assert_eq!(
            clamp_size(px(640.), px(80.), px(220.), px(900.), true),
            px(560.)
        );
        assert_eq!(
            clamp_size(px(640.), px(-400.), px(220.), px(900.), true),
            px(900.),
        );
    }
}
