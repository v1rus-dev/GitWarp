//! A scrollbar drawn from a GPUI scroll handle.
//!
//! GPUI paints no scrollbars, so this module draws the track and thumb from
//! `ScrollHandle` state and maps pointer drags back to scroll offsets.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, point, px, App, AppContext, DragMoveEvent, ElementId, EmptyView, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Point, RenderOnce, ScrollHandle,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window,
};
use gpui::{AnyElement, MouseDownEvent};

use crate::theme::ActiveTheme;

/// Either a plain scroll handle or a uniform list's handle.
#[derive(Clone, Debug)]
pub enum ScrollHandleSource {
    Scroll(ScrollHandle),
    Uniform(UniformListScrollHandle),
}

impl ScrollHandleSource {
    fn handle(&self) -> ScrollHandle {
        match self {
            Self::Scroll(handle) => handle.clone(),
            Self::Uniform(handle) => handle.0.borrow().base_handle.clone(),
        }
    }
}

impl From<ScrollHandle> for ScrollHandleSource {
    fn from(handle: ScrollHandle) -> Self {
        Self::Scroll(handle)
    }
}

impl From<&ScrollHandle> for ScrollHandleSource {
    fn from(handle: &ScrollHandle) -> Self {
        Self::Scroll(handle.clone())
    }
}

impl From<UniformListScrollHandle> for ScrollHandleSource {
    fn from(handle: UniformListScrollHandle) -> Self {
        Self::Uniform(handle)
    }
}

impl From<&UniformListScrollHandle> for ScrollHandleSource {
    fn from(handle: &UniformListScrollHandle) -> Self {
        Self::Uniform(handle.clone())
    }
}

/// The thumb's position and length along its track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbMetrics {
    pub offset: Pixels,
    pub size: Pixels,
}

/// Thumb geometry for a scroll range, or `None` when nothing overflows.
///
/// `track` and `viewport` are the track and visible lengths along the scroll
/// axis; `content` is the full content length, and `scroll` is the distance
/// scrolled from the start (never negative).
pub fn thumb_metrics(
    track: Pixels,
    viewport: Pixels,
    content: Pixels,
    scroll: Pixels,
) -> Option<ThumbMetrics> {
    let track_f: f32 = track.into();
    let viewport_f: f32 = viewport.into();
    let content_f: f32 = content.into();
    let scroll_f: f32 = scroll.into();
    if track_f <= 0. || viewport_f <= 0. || content_f <= viewport_f {
        return None;
    }

    let size = (track_f * viewport_f / content_f).clamp(0., track_f);
    let max_scroll = content_f - viewport_f;
    let travel = track_f - size;
    let scroll = scroll_f.clamp(0., max_scroll);
    let offset = if max_scroll <= 0. {
        0.
    } else {
        scroll / max_scroll * travel
    };

    Some(ThumbMetrics {
        offset: px(offset),
        size: px(size),
    })
}

#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Default)]
struct DragState {
    start: Option<Point<Pixels>>,
    start_scroll: f32,
    dragging: bool,
    /// The scroll handle is only populated after the first layout, so the
    /// first frame asks for exactly one extra frame to draw the thumb.
    retried: bool,
}

/// The id of the scrollbar whose drag is active, so one scrollbar's drag never
/// moves another one's thumb.
#[derive(Clone)]
struct ScrollbarDrag(ElementId);

/// A vertical or horizontal scrollbar for a [`ScrollHandle`].
#[derive(IntoElement)]
pub struct Scrollbar {
    id: ElementId,
    source: ScrollHandleSource,
    axis: Axis,
}

impl Scrollbar {
    pub fn vertical(id: impl Into<ElementId>, source: impl Into<ScrollHandleSource>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            axis: Axis::Vertical,
        }
    }

    pub fn horizontal(id: impl Into<ElementId>, source: impl Into<ScrollHandleSource>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            axis: Axis::Horizontal,
        }
    }

    /// A vertical scrollbar for a `uniform_list`.
    pub fn uniform(id: impl Into<ElementId>, handle: UniformListScrollHandle) -> Self {
        Self::vertical(id, handle)
    }
}

impl RenderOnce for Scrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let handle = self.source.handle();
        let vertical = matches!(self.axis, Axis::Vertical);
        let viewport = handle.bounds().size;
        let max = handle.max_offset();
        let offset = handle.offset();

        let (viewport_len, content_len, scroll) = if vertical {
            (
                viewport.height,
                viewport.height + max.y,
                px(-f32::from(offset.y)),
            )
        } else {
            (
                viewport.width,
                viewport.width + max.x,
                px(-f32::from(offset.x)),
            )
        };
        let track_len = viewport_len;
        let metrics = thumb_metrics(track_len, viewport_len, content_len, scroll);

        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| DragState::default());
        if f32::from(viewport_len) <= 0. && !state.read(cx).retried {
            state.update(cx, |state, _| state.retried = true);
            window.on_next_frame(|window, _| window.refresh());
        }
        let mut track = div()
            .id(self.id.clone())
            .relative()
            .flex_none()
            .when(vertical, |this| this.w(px(10.)).h_full())
            .when(!vertical, |this| this.h(px(10.)).w_full());

        if let Some(metrics) = metrics {
            let thumb_id = ElementId::Name(format!("{}-thumb", self.id).into());
            let mut thumb = div()
                .id(thumb_id)
                .absolute()
                .rounded(px(3.))
                .bg(colors.border_strong)
                .cursor_pointer()
                .when(vertical, |this| {
                    this.left(px(2.))
                        .w(px(6.))
                        .top(metrics.offset)
                        .h(metrics.size)
                })
                .when(!vertical, |this| {
                    this.top(px(2.))
                        .h(px(6.))
                        .left(metrics.offset)
                        .w(metrics.size)
                });

            {
                let state = state.clone();
                let handle = handle.clone();
                thumb = thumb.on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        let current = scroll_position(&handle, vertical);
                        state.update(cx, |state, _| {
                            state.start = Some(event.position);
                            state.start_scroll = f32::from(current);
                            state.dragging = true;
                        });
                        window.refresh();
                        cx.stop_propagation();
                    },
                );
            }
            thumb = thumb.on_drag(ScrollbarDrag(self.id.clone()), |_, _, _, cx| {
                cx.new(|_| EmptyView)
            });
            {
                let state = state.clone();
                let handle = handle.clone();
                let drag_id = self.id.clone();
                thumb = thumb.on_drag_move(
                    move |event: &DragMoveEvent<ScrollbarDrag>,
                          window: &mut Window,
                          cx: &mut App| {
                        if event.drag(cx).0 != drag_id {
                            return;
                        }
                        let (Some(start), start_scroll, dragging) = ({
                            let state = state.read(cx);
                            (state.start, state.start_scroll, state.dragging)
                        }) else {
                            return;
                        };
                        if !dragging {
                            return;
                        }
                        let delta = if vertical {
                            f32::from(event.event.position.y - start.y)
                        } else {
                            f32::from(event.event.position.x - start.x)
                        };
                        let Some(metrics) =
                            thumb_metrics(track_len, viewport_len, content_len, px(0.))
                        else {
                            return;
                        };
                        let travel = f32::from(track_len - metrics.size);
                        if travel <= 0. {
                            return;
                        }
                        let max_scroll = f32::from(max_component(max, vertical));
                        let scroll =
                            (start_scroll + delta * max_scroll / travel).clamp(0., max_scroll);
                        set_scroll(&handle, vertical, px(scroll));
                        window.refresh();
                    },
                );
            }
            {
                let up_state = state.clone();
                thumb = thumb.on_mouse_up(
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
                thumb = thumb.on_mouse_up_out(
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

            track = track.child(thumb);
        }

        {
            let handle = handle.clone();
            track = track.on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window: &mut Window, _: &mut App| {
                    let Some(metrics) = metrics else {
                        return;
                    };
                    let bounds = handle.bounds();
                    let pointer = if vertical {
                        f32::from(event.position.y - bounds.top())
                    } else {
                        f32::from(event.position.x - bounds.left())
                    };
                    let thumb_start = f32::from(metrics.offset);
                    let thumb_end = thumb_start + f32::from(metrics.size);
                    if pointer >= thumb_start && pointer <= thumb_end {
                        return;
                    }
                    let page = f32::from(viewport_len);
                    let current = f32::from(scroll_position(&handle, vertical));
                    let max_scroll = f32::from(max_component(max, vertical));
                    let next = if pointer < thumb_start {
                        (current - page).clamp(0., max_scroll)
                    } else {
                        (current + page).clamp(0., max_scroll)
                    };
                    set_scroll(&handle, vertical, px(next));
                    window.refresh();
                },
            );
        }

        track
    }
}

fn scroll_position(handle: &ScrollHandle, vertical: bool) -> Pixels {
    let offset = handle.offset();
    if vertical {
        px(-f32::from(offset.y))
    } else {
        px(-f32::from(offset.x))
    }
}

fn max_component(max: Point<Pixels>, vertical: bool) -> Pixels {
    if vertical {
        max.y
    } else {
        max.x
    }
}

fn set_scroll(handle: &ScrollHandle, vertical: bool, scroll: Pixels) {
    let value = f32::from(scroll);
    if vertical {
        handle.set_offset(point(px(0.), px(-value)));
    } else {
        handle.set_offset(point(px(-value), px(0.)));
    }
}

/// A scrollable viewport with its scrollbar beside it.
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    scroll: ScrollHandle,
    horizontal: bool,
    content: Option<AnyElement>,
}

impl ScrollArea {
    pub fn new(id: impl Into<ElementId>, scroll: ScrollHandle) -> Self {
        Self {
            id: id.into(),
            scroll,
            horizontal: false,
            content: None,
        }
    }

    /// Scroll along the x axis, with the scrollbar below the viewport.
    pub fn horizontal(mut self) -> Self {
        self.horizontal = true;
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.content = Some(child.into_any_element());
        self
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let id_name = self.id.to_string();
        let content_id = ElementId::Name(format!("{id_name}-content").into());
        let mut viewport = div()
            .id(content_id)
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .track_scroll(&self.scroll)
            .when(self.horizontal, |this| this.overflow_x_scroll())
            .when(!self.horizontal, |this| this.overflow_y_scroll());
        if let Some(content) = self.content {
            viewport = viewport.child(content);
        }

        let mut root = div().w_full().h_full();
        root = root
            .when(self.horizontal, |this| this.flex().flex_col())
            .when(!self.horizontal, |this| this.flex().flex_row());
        root.child(viewport).child(if self.horizontal {
            Scrollbar::horizontal(
                ElementId::Name(format!("{id_name}-bar").into()),
                self.scroll,
            )
            .into_any_element()
        } else {
            Scrollbar::vertical(
                ElementId::Name(format!("{id_name}-bar").into()),
                self.scroll,
            )
            .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_thumb_sits_at_the_start() {
        let thumb = thumb_metrics(px(300.), px(300.), px(900.), px(0.)).expect("overflow");
        assert_eq!(thumb.size, px(100.));
        assert_eq!(thumb.offset, px(0.));
    }

    #[test]
    fn the_thumb_reaches_the_end_at_the_max_scroll() {
        let thumb = thumb_metrics(px(300.), px(300.), px(900.), px(600.)).expect("overflow");
        assert_eq!(thumb.size, px(100.));
        assert_eq!(thumb.offset, px(200.));
    }

    #[test]
    fn the_thumb_moves_halfway_at_half_scroll() {
        let thumb = thumb_metrics(px(300.), px(300.), px(900.), px(300.)).expect("overflow");
        assert_eq!(thumb.offset, px(100.));
    }

    #[test]
    fn scroll_is_clamped_to_the_content() {
        let thumb = thumb_metrics(px(300.), px(300.), px(900.), px(9999.)).expect("overflow");
        assert_eq!(thumb.offset, px(200.));
    }

    #[test]
    fn content_that_fits_has_no_thumb() {
        assert_eq!(thumb_metrics(px(300.), px(300.), px(300.), px(0.)), None);
        assert_eq!(thumb_metrics(px(300.), px(300.), px(299.), px(0.)), None);
    }
}
