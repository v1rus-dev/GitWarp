//! Read-only text with mouse selection and copy.
//!
//! A [`SelectableText`] wraps to the width it is given, paints the theme's
//! selection colour behind the selected bytes, and binds the usual editing
//! keys in its `SelectableText` key context. It is built on GPUI's
//! [`StyledText`], whose [`TextLayout`] already maps byte indices to pixels and
//! back for wrapped multi-line text.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    actions, div, fill, point, px, size, App, AppContext, ClipboardItem, CursorStyle,
    DragMoveEvent, Element, ElementId, EmptyView, Entity, FocusHandle, GlobalElementId,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, ParentElement, Pixels, Point, RenderOnce, StatefulInteractiveElement,
    Styled, StyledText, TextLayout, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::ActiveTheme;

actions!(selectable_text, [Copy, SelectAll]);

/// The id of the selectable text that owns the active drag.
#[derive(Clone)]
struct SelectableDrag(ElementId);

struct SelectableTextState {
    focus: FocusHandle,
    /// The text the selection was made for. Keyed state outlives a text
    /// change under the same element id, so the selection must be dropped
    /// when the text is no longer this one.
    text: gpui::SharedString,
    /// Where the selection started; `cursor` is its moving end.
    anchor: usize,
    cursor: usize,
    selecting: bool,
    /// The latest laid-out text. Shared with the element that fills it during
    /// measurement, so hit tests in event handlers see the same layout.
    layout: Rc<RefCell<Option<TextLayout>>>,
    /// Whether `layout` has been through prepaint (and so has bounds).
    ready: Rc<Cell<bool>>,
}

impl SelectableTextState {
    fn new(focus: FocusHandle) -> Self {
        Self {
            focus,
            text: gpui::SharedString::default(),
            anchor: 0,
            cursor: 0,
            selecting: false,
            layout: Rc::new(RefCell::new(None)),
            ready: Rc::new(Cell::new(false)),
        }
    }

    fn selection(&self) -> Range<usize> {
        normalize_selection(self.anchor, self.cursor)
    }
}

type SelectionHandler = Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// Read-only, selectable text. Create it with an [`ElementId`] and the text;
/// it inherits the surrounding theme text style (size, colour, font).
#[derive(IntoElement)]
pub struct SelectableText {
    id: ElementId,
    text: gpui::SharedString,
    on_selection_change: Option<SelectionHandler>,
}

impl SelectableText {
    pub fn new(id: impl Into<ElementId>, text: impl Into<gpui::SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            on_selection_change: None,
        }
    }

    /// Called with whether the text currently has a non-empty selection.
    pub fn on_selection_change(
        mut self,
        listener: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_selection_change = Some(Rc::new(listener));
        self
    }
}

/// The smaller and larger end of a selection, in byte offsets.
fn normalize_selection(anchor: usize, cursor: usize) -> Range<usize> {
    if anchor <= cursor {
        anchor..cursor
    } else {
        cursor..anchor
    }
}

/// `selection` cut to `text`'s length and to whole characters. A selection can
/// outlive its text (keyed state, same element id) and a byte index inside a
/// character would panic both `&text[range]` and the text layout.
fn clamp_selection(text: &str, selection: &Range<usize>) -> Range<usize> {
    let len = text.len();
    let mut start = selection.start.min(len);
    let mut end = selection.end.min(len);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < start {
        end = start;
    }
    start..end
}

/// The selection that is still valid for `text`: none unless `stored` is still
/// the text the state was made for.
fn live_selection(stored: &str, text: &str, selection: &Range<usize>) -> Range<usize> {
    if stored != text {
        return 0..0;
    }
    clamp_selection(text, selection)
}

/// The selected part of `text`, or `None` when the range is empty or stale.
fn selected_text<'a>(text: &'a str, selection: &Range<usize>) -> Option<&'a str> {
    text.get(clamp_selection(text, selection))
        .filter(|selected| !selected.is_empty())
}

/// The word (or whitespace/punctuation run) around `index`.
fn word_range(text: &str, index: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let index = index.min(text.len());
    let mut last = 0..0;
    for (start, word) in text.split_word_bound_indices() {
        let end = start + word.len();
        if index < end || (index == end && end == text.len()) {
            return start..end;
        }
        last = start..end;
    }
    last
}

fn index_for_point(
    state: &Entity<SelectableTextState>,
    cx: &App,
    position: Point<Pixels>,
) -> usize {
    let state = state.read(cx);
    if !state.ready.get() {
        return 0;
    }
    let layout = state.layout.borrow();
    match layout.as_ref() {
        Some(layout) => layout
            .index_for_position(position)
            .unwrap_or_else(|index| index),
        None => 0,
    }
}

fn notify_selection(
    state: &Entity<SelectableTextState>,
    listener: &Option<SelectionHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(listener) = listener {
        let selected = !state.read(cx).selection().is_empty();
        listener(selected, window, cx);
    }
}

fn select_all(state: &Entity<SelectableTextState>, len: usize, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, _| {
        state.anchor = 0;
        state.cursor = len;
        state.selecting = false;
    });
    window.refresh();
}

fn end_selection(state: &Entity<SelectableTextState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, _| state.selecting = false);
    window.refresh();
}

/// Drops the selection (focus or the pointer left): a listener, e.g. a toast's
/// timer, must know it is no longer paused by a selection.
fn clear_selection(
    state: &Entity<SelectableTextState>,
    listener: &Option<SelectionHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).selection().is_empty() {
        return;
    }
    state.update(cx, |state, _| {
        state.anchor = 0;
        state.cursor = 0;
        state.selecting = false;
    });
    notify_selection(state, listener, window, cx);
    window.refresh();
}

fn copy_selection(state: &Entity<SelectableTextState>, text: &str, cx: &mut App) {
    let state = state.read(cx);
    let selection = live_selection(state.text.as_ref(), text, &state.selection());
    if let Some(selected) = selected_text(text, &selection) {
        cx.write_to_clipboard(ClipboardItem::new_string(selected.to_string()));
    }
}

impl RenderOnce for SelectableText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| {
            SelectableTextState::new(cx.focus_handle())
        });
        let focus = state.read(cx).focus.clone();
        // The keyed state outlives the text: a selection made in a longer text
        // must not survive a shorter one, and a text that lost focus must not
        // keep a toast timer paused.
        let changed = state.read(cx).text != self.text;
        let had_selection = !state.read(cx).selection().is_empty();
        let unfocused = had_selection && !focus.is_focused(window);
        if changed || unfocused {
            let text = self.text.clone();
            state.update(cx, |state, _| {
                state.text = text;
                state.anchor = 0;
                state.cursor = 0;
                state.selecting = false;
            });
            if had_selection {
                if let Some(listener) = self.on_selection_change.as_ref() {
                    listener(false, window, cx);
                }
            }
        }
        let layout_slot = state.read(cx).layout.clone();
        let ready = state.read(cx).ready.clone();
        ready.set(false);

        let text = self.text;
        let selection = state.read(cx).selection();
        let inner = SelectableTextElement {
            text: StyledText::new(text.clone()),
            source: text.clone(),
            selection,
            layout_slot: layout_slot.clone(),
            ready: ready.clone(),
        };
        *layout_slot.borrow_mut() = Some(inner.text.layout().clone());

        let len = text.len();

        let on_copy = {
            let state = state.clone();
            let text = text.clone();
            move |_: &Copy, _: &mut Window, cx: &mut App| copy_selection(&state, &text, cx)
        };
        let on_select_all = {
            let state = state.clone();
            move |_: &SelectAll, window: &mut Window, cx: &mut App| {
                select_all(&state, len, window, cx)
            }
        };
        let listener = self.on_selection_change.clone();
        let on_mouse_down = {
            let state = state.clone();
            let text = text.clone();
            let listener = listener.clone();
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                let focus = state.read(cx).focus.clone();
                focus.focus(window, cx);
                let index = index_for_point(&state, cx, event.position);
                state.update(cx, |state, _| match event.click_count {
                    3.. => {
                        state.anchor = 0;
                        state.cursor = text.len();
                        state.selecting = false;
                    }
                    2 => {
                        let range = word_range(&text, index);
                        state.anchor = range.start;
                        state.cursor = range.end;
                        state.selecting = false;
                    }
                    _ => {
                        state.anchor = index;
                        state.cursor = index;
                        state.selecting = true;
                    }
                });
                notify_selection(&state, &listener, window, cx);
                window.refresh();
            }
        };
        let on_mouse_move = {
            let state = state.clone();
            let listener = listener.clone();
            move |event: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if !state.read(cx).selecting {
                    return;
                }
                let index = index_for_point(&state, cx, event.position);
                state.update(cx, |state, _| state.cursor = index);
                notify_selection(&state, &listener, window, cx);
                window.refresh();
            }
        };
        let on_mouse_up = {
            let state = state.clone();
            move |_: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                end_selection(&state, window, cx);
            }
        };
        let on_mouse_up_out = {
            let state = state.clone();
            move |_: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                end_selection(&state, window, cx);
            }
        };
        let on_mouse_down_out = {
            let state = state.clone();
            let listener = listener.clone();
            move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                clear_selection(&state, &listener, window, cx);
            }
        };
        let on_drag_move = {
            let state = state.clone();
            let drag_id = self.id.clone();
            move |event: &DragMoveEvent<SelectableDrag>, window: &mut Window, cx: &mut App| {
                if event.drag(cx).0 != drag_id {
                    return;
                }
                if !state.read(cx).selecting {
                    return;
                }
                let index = index_for_point(&state, cx, event.event.position);
                state.update(cx, |state, _| state.cursor = index);
                notify_selection(&state, &listener, window, cx);
                window.refresh();
            }
        };

        div()
            .id(self.id.clone())
            .min_w_0()
            .cursor(CursorStyle::IBeam)
            .track_focus(&focus)
            .key_context("SelectableText")
            .on_action(on_copy)
            .on_action(on_select_all)
            .on_mouse_down(MouseButton::Left, on_mouse_down)
            .on_mouse_down_out(on_mouse_down_out)
            .on_mouse_up(MouseButton::Left, on_mouse_up)
            .on_mouse_up_out(MouseButton::Left, on_mouse_up_out)
            .on_mouse_move(on_mouse_move)
            .on_drag(SelectableDrag(self.id.clone()), |_, _, _, cx| {
                cx.new(|_| EmptyView)
            })
            .on_drag_move(on_drag_move)
            .child(inner)
    }
}

/// Binds Copy and Select All in the `SelectableText` key context.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        crate::actions::key_binding(cx, "secondary-c", Copy, Some("SelectableText")),
        crate::actions::key_binding(cx, "secondary-a", SelectAll, Some("SelectableText")),
    ]);
}

struct SelectableTextElement {
    text: StyledText,
    /// The bytes the layout was made from, for clamping the selection.
    source: gpui::SharedString,
    selection: Range<usize>,
    layout_slot: Rc<RefCell<Option<TextLayout>>>,
    ready: Rc<Cell<bool>>,
}

impl IntoElement for SelectableTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        Element::request_layout(&mut self.text, id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        Element::prepaint(&mut self.text, id, inspector_id, bounds, state, window, cx);
        *self.layout_slot.borrow_mut() = Some(self.text.layout().clone());
        self.ready.set(true);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_selection(
            &self.source,
            self.text.layout(),
            &self.selection,
            window,
            cx,
        );
        Element::paint(
            &mut self.text,
            id,
            inspector_id,
            bounds,
            state,
            prepaint,
            window,
            cx,
        );
    }
}

/// Paints the selection highlight for `selection` behind the text.
fn paint_selection(
    text: &str,
    layout: &TextLayout,
    selection: &Range<usize>,
    window: &mut Window,
    cx: &mut App,
) {
    let selection = clamp_selection(text, selection);
    if selection.is_empty() {
        return;
    }
    let colors = cx.theme().colors;
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut line_start = 0usize;
    let mut row = 0usize;

    for line in layout.line_layouts() {
        let len = line.len();
        let mut row_start = 0usize;
        let mut boundaries: Vec<usize> = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
            })
            .collect();
        boundaries.push(len);

        for end in boundaries {
            let global_start = line_start + row_start;
            let global_end = line_start + end;
            if selection.start <= global_end && selection.end >= global_start {
                let from = selection.start.max(global_start);
                let to = selection.end.min(global_end);
                let base_x = line.unwrapped_layout.x_for_index(row_start);
                let x0 = line.unwrapped_layout.x_for_index(from - line_start) - base_x;
                let continues = selection.end > global_end;
                let x1 = if continues {
                    if end == len {
                        (line.unwrapped_layout.width - base_x) + px(4.)
                    } else {
                        line.unwrapped_layout.x_for_index(end) - base_x
                    }
                } else {
                    line.unwrapped_layout.x_for_index(to - line_start) - base_x
                };
                if x1 > x0 {
                    let origin = point(
                        bounds.left() + x0,
                        bounds.top() + px(row as f32 * f32::from(line_height)),
                    );
                    let quad: PaintQuad = fill(
                        gpui::Bounds::new(origin, size(x1 - x0, line_height)),
                        colors.selection,
                    );
                    window.paint_quad(quad);
                }
            }
            row += 1;
            row_start = end;
        }
        line_start += len + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_normalizes_both_directions() {
        assert_eq!(normalize_selection(2, 7), 2..7);
        assert_eq!(normalize_selection(7, 2), 2..7);
        assert_eq!(normalize_selection(4, 4), 4..4);
    }

    #[test]
    fn a_word_range_covers_the_word_under_the_pointer() {
        assert_eq!(word_range("hello world", 2), 0..5);
        assert_eq!(word_range("hello world", 7), 6..11);
        // Punctuation and whitespace are their own runs.
        assert_eq!(word_range("one, two", 1), 0..3);
        assert_eq!(word_range("one, two", 3), 3..4);
        assert_eq!(word_range("one, two", 4), 4..5);
    }

    #[test]
    fn a_word_range_clamps_at_the_edges() {
        assert_eq!(word_range("", 0), 0..0);
        assert_eq!(word_range("hello", 99), 0..5);
        assert_eq!(word_range("hello", 5), 0..5);
    }

    #[test]
    fn word_ranges_stay_on_character_boundaries() {
        let text = "héllo wörld";
        let range = word_range(text, 1);
        assert_eq!(&text[range], "héllo");
        let range = word_range(text, 7);
        assert_eq!(&text[range], "wörld");
    }

    #[test]
    fn a_selection_from_a_longer_text_is_dropped_when_the_text_shrinks() {
        let long = "a much longer line of text";
        let short = "tiny";
        assert_eq!(live_selection(long, short, &(4..20)), 0..0);
        assert_eq!(
            selected_text(short, &live_selection(long, short, &(4..20))),
            None
        );
        // The same text keeps its selection.
        assert_eq!(live_selection(short, short, &(0..4)), 0..4);
        assert_eq!(selected_text(short, &(0..4)), Some("tiny"));
    }

    #[test]
    fn selections_are_clamped_to_the_text_and_to_whole_characters() {
        let text = "héllo";
        // The end falls inside `é` (bytes 1..3): only `h` is selected.
        assert_eq!(clamp_selection(text, &(0..2)), 0..1);
        assert_eq!(selected_text(text, &(0..2)), Some("h"));
        assert_eq!(selected_text(text, &(0..99)), Some("héllo"));
        assert_eq!(selected_text(text, &(99..120)), None);
        assert_eq!(selected_text("", &(5..9)), None);
        // A reverse range (stale anchor/cursor) is empty, never inverted.
        let (anchor, cursor) = (4usize, 2usize);
        assert_eq!(clamp_selection(text, &(anchor..cursor)), 4..4);
    }
}
