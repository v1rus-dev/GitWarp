//! Multi-line text area, ported from the single-line input.
//!
//! The commit editor needs a subject line, a body and the usual editing keys.
//! The model is the same [`EditModel`] as [`super::TextInput`]; only the layout
//! and the vertical movement differ.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, px, relative, size, App, AppContext, Bounds, ClipboardItem, Context,
    CursorStyle, DragMoveEvent, Element, ElementId, ElementInputHandler, EmptyView, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InteractiveElement,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, ParentElement, Pixels, Point, Render, ShapedLine, SharedString,
    StatefulInteractiveElement, Style, Styled, TextAlign, TextRun, UTF16Selection, Window,
};

use crate::metrics::{FONT_MONO, MONO, R_SM};
use crate::theme::ActiveTheme;

use super::model::EditModel;
use super::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectEnd, SelectHome,
    SelectLeft, SelectRight, SelectWordLeft, SelectWordRight, WordBackspace, WordDelete, WordLeft,
    WordRight,
};

actions!(
    text_area,
    [
        Newline,
        Up,
        Down,
        SelectUp,
        SelectDown,
        DocHome,
        DocEnd,
        SelectDocHome,
        SelectDocEnd,
    ]
);

/// Emitted by [`TextArea`] when its text changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextAreaEvent {
    Changed(String),
}

const PADDING_X: f32 = 10.;
const PADDING_Y: f32 = 6.;
const LINE_HEIGHT: f32 = 20.;

/// The id of the text area that owns the active selection drag.
#[derive(Clone)]
struct TextAreaDrag(ElementId);

/// A multi-line text field with selection, IME and vertical scrolling.
pub struct TextArea {
    focus_handle: FocusHandle,
    model: EditModel,
    placeholder: SharedString,
    font_family: SharedString,
    disabled: bool,
    height: Pixels,
    /// The column a run of Up/Down keeps when the next line is shorter.
    preferred_column: Option<usize>,
    scroll_offset: f32,
    last_lines: Vec<ShapedLine>,
    last_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextArea {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            model: EditModel::new(),
            placeholder: SharedString::default(),
            font_family: FONT_MONO.into(),
            disabled: false,
            height: px(104.),
            preferred_column: None,
            scroll_offset: 0.,
            last_lines: Vec::new(),
            last_starts: vec![0],
            last_bounds: None,
            is_selecting: false,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn text(&self) -> &str {
        &self.model.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.set_text(text);
        self.preferred_column = None;
        self.after_edit(before, cx);
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_font_family(&mut self, family: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.font_family = family.into();
        cx.notify();
    }

    pub fn set_height(&mut self, height: Pixels, cx: &mut Context<Self>) {
        self.height = height;
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        self.focus_handle = self.focus_handle.clone().tab_stop(!disabled);
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.preferred_column = None;
        self.model.select_all();
        cx.notify();
    }

    fn after_edit(&mut self, before: String, cx: &mut Context<Self>) {
        if self.model.text != before {
            cx.emit(TextAreaEvent::Changed(self.model.text.clone()));
        }
        cx.notify();
    }

    fn edit(&mut self, edit: impl FnOnce(&mut EditModel), cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.preferred_column = None;
        edit(&mut self.model);
        self.after_edit(before, cx);
    }

    fn move_once(&mut self, movement: impl FnOnce(&mut EditModel), cx: &mut Context<Self>) {
        self.preferred_column = None;
        movement(&mut self.model);
        cx.notify();
    }

    fn move_vertical(&mut self, delta: isize, select: bool, cx: &mut Context<Self>) {
        let text = &self.model.text;
        let cursor = self.model.cursor();
        let start = line_start(text, cursor);
        let column = text[start..cursor].chars().count();
        let column = *self.preferred_column.get_or_insert(column);
        let target_start = if delta < 0 {
            if start == 0 {
                0
            } else {
                line_start(text, start - 1)
            }
        } else {
            let end = line_end(text, start);
            if end >= text.len() {
                text.len()
            } else {
                line_start(text, end + 1)
            }
        };
        let target = char_offset(text, target_start, column);
        if select {
            self.model.select_to(target);
        } else {
            self.model.move_to(target);
        }
        cx.notify();
    }

    fn delete_backward(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|model| model.backspace(), cx);
    }

    fn delete_forward(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|model| model.delete(), cx);
    }

    fn delete_word_left(&mut self, _: &WordBackspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|model| model.delete_word_left(), cx);
    }

    fn delete_word_right(&mut self, _: &WordDelete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|model| model.delete_word_right(), cx);
    }

    fn move_left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.move_left(), cx);
    }

    fn move_right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.move_right(), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.select_left(), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.select_right(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.move_word_left(), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.move_word_right(), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.select_word_left(), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.select_word_right(), cx);
    }

    fn move_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let start = line_start(&self.model.text, self.model.cursor());
        self.move_once(|model| model.move_to(start), cx);
    }

    fn move_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let end = line_end(&self.model.text, self.model.cursor());
        self.move_once(|model| model.move_to(end), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let start = line_start(&self.model.text, self.model.cursor());
        self.move_once(|model| model.select_to(start), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        let end = line_end(&self.model.text, self.model.cursor());
        self.move_once(|model| model.select_to(end), cx);
    }

    fn doc_home(&mut self, _: &DocHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.move_to(0), cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.model.text.len();
        self.move_once(|model| model.move_to(end), cx);
    }

    fn select_doc_home(&mut self, _: &SelectDocHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_once(|model| model.select_to(0), cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.model.text.len();
        self.move_once(|model| model.select_to(end), cx);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(
            |model| {
                model.insert("\n");
            },
            cx,
        );
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            self.edit(
                |model| {
                    model.insert(&text);
                },
                cx,
            );
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.model.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.model.text[self.model.selection.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.model.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.model.text[self.model.selection.clone()].to_string(),
            ));
            self.edit(
                |model| {
                    model.replace_range(model.selection.clone(), "");
                },
                cx,
            );
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);

        if event.click_count >= 2 {
            let range = self.model.word_range(index);
            self.model.selection = range;
            self.model.reversed = false;
            self.preferred_column = None;
            cx.notify();
        } else if event.modifiers.shift {
            self.model.select_to(index);
            cx.notify();
        } else {
            self.model.move_to(index);
            self.preferred_column = None;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection(event.position, cx);
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.is_selecting {
            let index = self.index_for_mouse_position(position);
            self.model.select_to(index);
            cx.notify();
        }
    }

    fn on_drag_move(
        &mut self,
        event: &DragMoveEvent<TextAreaDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(event.event.position, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.model.text.len();
        }
        let top = f32::from(bounds.top()) + PADDING_Y - self.scroll_offset;
        let line = ((f32::from(position.y) - top) / LINE_HEIGHT)
            .floor()
            .max(0.) as usize;
        let line = line.min(self.last_starts.len().saturating_sub(1));
        let start = self.last_starts[line];
        let end = line_end(&self.model.text, start);
        let Some(shaped) = self.last_lines.get(line) else {
            return start;
        };
        let x = position.x - bounds.left() - px(PADDING_X);
        (start + shap_ed_closest(shaped, x))
            .min(end)
            .min(self.model.text.len())
    }
}

/// `line.closest_index_for_x`, named once so the call site stays readable.
fn shap_ed_closest(line: &ShapedLine, x: Pixels) -> usize {
    line.closest_index_for_x(x)
}

/// The byte offset where the line containing `offset` starts.
fn line_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[..offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// The byte offset of the end of the line containing `offset`, before its
/// newline.
fn line_end(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len())
}

/// The offset `column` characters into the line that starts at `start`,
/// clamped to the line's end.
fn char_offset(text: &str, start: usize, column: usize) -> usize {
    let end = line_end(text, start);
    text[start..end]
        .char_indices()
        .nth(column)
        .map(|(index, _)| start + index)
        .unwrap_or(end)
}

/// The starts of every line in `text`.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.model.range_from_utf16(&range_utf16);
        actual_range.replace(self.model.range_to_utf16(&range));
        Some(self.model.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.disabled {
            return None;
        }
        Some(UTF16Selection {
            range: self.model.range_to_utf16(&self.model.selection),
            reversed: self.model.reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.model
            .marked
            .as_ref()
            .map(|range| self.model.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.model.set_marked(None);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.model.range_from_utf16(range))
            .or_else(|| self.model.marked.clone())
            .unwrap_or_else(|| self.model.selection.clone());
        self.edit(
            |model| {
                model.replace_range(range, new_text);
            },
            cx,
        );
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.model.range_from_utf16(range))
            .or_else(|| self.model.marked.clone())
            .unwrap_or_else(|| self.model.selection.clone());
        let before = self.model.text.clone();
        self.preferred_column = None;
        self.model
            .replace_and_mark(range, new_text, new_selected_range_utf16);
        self.after_edit(before, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.model.range_from_utf16(&range_utf16);
        let start = line_start(&self.model.text, range.start);
        let line = self
            .last_starts
            .iter()
            .position(|line_start| *line_start == start)
            .unwrap_or(0);
        let shaped = self.last_lines.get(line)?;
        let index = range.start.saturating_sub(start);
        let x = shaped.x_for_index(index);
        let y =
            bounds.top() + px(PADDING_Y) + px(line as f32 * LINE_HEIGHT) - px(self.scroll_offset);
        Some(Bounds::new(
            point(bounds.left() + px(PADDING_X) + x, y),
            size(px(1.), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        if !bounds.contains(&point) {
            return None;
        }
        let offset = self.index_for_mouse_position(point);
        Some(self.model.offset_to_utf16(offset))
    }
}

struct TextElement {
    input: Entity<TextArea>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let colors = cx.theme().colors;
        let input = self.input.read(cx);
        let text = input.model.text.clone();
        let selection = input.model.selection.clone();
        let cursor = input.model.cursor();
        let disabled = input.disabled;
        let scroll = input.scroll_offset;
        let placeholder = input.placeholder.clone();
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text_color = if disabled {
            colors.disabled
        } else {
            style.color
        };

        let starts = line_starts(&text);
        let cursor_line = text[..cursor.min(text.len())]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let inner_height = (f32::from(bounds.size.height) - 2. * PADDING_Y).max(0.);
        let content_height = starts.len() as f32 * LINE_HEIGHT;
        let mut scroll_offset = scroll;
        let cursor_top = cursor_line as f32 * LINE_HEIGHT;
        if cursor_top - scroll_offset < 0. {
            scroll_offset = cursor_top;
        }
        if cursor_top + LINE_HEIGHT - scroll_offset > inner_height {
            scroll_offset = cursor_top + LINE_HEIGHT - inner_height;
        }
        scroll_offset = scroll_offset.clamp(0., (content_height - inner_height).max(0.));

        let mut lines = Vec::with_capacity(starts.len());
        for (index, start) in starts.iter().copied().enumerate() {
            let end = line_end(&text, start);
            let (display, color) = if text.is_empty() && index == 0 {
                (placeholder.clone(), colors.muted)
            } else {
                (SharedString::from(text[start..end].to_string()), text_color)
            };
            let run = TextRun {
                len: display.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            lines.push(
                window
                    .text_system()
                    .shape_line(display, font_size, &[run], None),
            );
        }

        let mut selection_quads = Vec::new();
        if !selection.is_empty() {
            let width = (f32::from(bounds.size.width) - 2. * PADDING_X).max(0.);
            for (index, start) in starts.iter().copied().enumerate() {
                let end = line_end(&text, start);
                if selection.end <= start || selection.start > end + 1 {
                    continue;
                }
                let from = selection.start.clamp(start, end) - start;
                let to = if selection.end > end {
                    width.max(f32::from(lines[index].width))
                } else {
                    f32::from(lines[index].x_for_index(selection.end - start))
                };
                let from = f32::from(lines[index].x_for_index(from));
                if to > from {
                    let y = bounds.top() + px(PADDING_Y) + px(index as f32 * LINE_HEIGHT)
                        - px(scroll_offset);
                    selection_quads.push(fill(
                        Bounds::new(
                            point(bounds.left() + px(PADDING_X) + px(from), y),
                            size(px(to - from), px(LINE_HEIGHT)),
                        ),
                        colors.selection,
                    ));
                }
            }
        }

        let cursor = if !disabled && input.focus_handle.is_focused(window) && selection.is_empty() {
            let start = starts.get(cursor_line).copied().unwrap_or(text.len());
            let shaped = &lines[cursor_line];
            let x = shaped.x_for_index(
                cursor
                    .saturating_sub(start)
                    .min(line_end(&text, start).saturating_sub(start)),
            );
            let y = bounds.top() + px(PADDING_Y) + px(cursor_line as f32 * LINE_HEIGHT)
                - px(scroll_offset);
            Some(fill(
                Bounds::new(
                    point(bounds.left() + px(PADDING_X) + x, y),
                    size(px(1.), px(LINE_HEIGHT)),
                ),
                colors.text,
            ))
        } else {
            None
        };

        self.input.update(cx, |input, _| {
            input.scroll_offset = scroll_offset;
            input.last_bounds = Some(bounds);
            input.last_starts = starts;
        });

        PrepaintState {
            lines,
            cursor,
            selection: selection_quads,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, disabled, scroll, focused) = {
            let input = self.input.read(cx);
            (
                input.focus_handle.clone(),
                input.disabled,
                input.scroll_offset,
                input.focus_handle.is_focused(window),
            )
        };

        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }

        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }

        for (index, line) in prepaint.lines.iter().enumerate() {
            let origin = point(
                bounds.left() + px(PADDING_X),
                bounds.top() + px(PADDING_Y) + px(index as f32 * LINE_HEIGHT) - px(scroll),
            );
            line.paint(origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx)
                .expect("text line must paint");
        }

        if focused {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focused = !self.disabled && self.focus_handle.is_focused(window);
        let border = if self.disabled {
            colors.border
        } else if focused {
            colors.focus
        } else {
            colors.border_strong
        };
        let input_id = ElementId::Name(format!("text-area-{}", cx.entity_id()).into());
        let mut root = div()
            .id(input_id.clone())
            .relative()
            .h(self.height)
            .w_full()
            .rounded(R_SM)
            .border_1()
            .border_color(border)
            .bg(colors.bg)
            .text_size(MONO)
            .line_height(px(LINE_HEIGHT))
            .font_family(self.font_family.clone())
            .text_color(if self.disabled {
                colors.disabled
            } else {
                colors.text
            })
            .overflow_hidden();

        if self.disabled {
            root = root.cursor_not_allowed();
        } else {
            let drag_id = input_id.clone();
            root = root
                .cursor(CursorStyle::IBeam)
                .track_focus(&self.focus_handle)
                .key_context("TextArea")
                .on_action(cx.listener(Self::delete_backward))
                .on_action(cx.listener(Self::delete_forward))
                .on_action(cx.listener(Self::delete_word_left))
                .on_action(cx.listener(Self::delete_word_right))
                .on_action(cx.listener(Self::move_left))
                .on_action(cx.listener(Self::move_right))
                .on_action(cx.listener(Self::select_left))
                .on_action(cx.listener(Self::select_right))
                .on_action(cx.listener(Self::word_left))
                .on_action(cx.listener(Self::word_right))
                .on_action(cx.listener(Self::select_word_left))
                .on_action(cx.listener(Self::select_word_right))
                .on_action(cx.listener(Self::move_home))
                .on_action(cx.listener(Self::move_end))
                .on_action(cx.listener(Self::select_home))
                .on_action(cx.listener(Self::select_end))
                .on_action(cx.listener(Self::doc_home))
                .on_action(cx.listener(Self::doc_end))
                .on_action(cx.listener(Self::select_doc_home))
                .on_action(cx.listener(Self::select_doc_end))
                .on_action(cx.listener(Self::select_all_action))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_action(cx.listener(Self::newline))
                .on_action(cx.listener(Self::up))
                .on_action(cx.listener(Self::down))
                .on_action(cx.listener(Self::select_up))
                .on_action(cx.listener(Self::select_down))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_drag(TextAreaDrag(drag_id.clone()), |_, _, _, cx| {
                    cx.new(|_| EmptyView)
                })
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<TextAreaDrag>, window, cx| {
                        if event.drag(cx).0 != drag_id {
                            return;
                        }
                        this.on_drag_move(event, window, cx);
                    },
                ));
        }

        root.child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TextAreaEvent> for TextArea {}

/// Binds the text area's own keys in its `TextArea` context.
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextArea")),
        KeyBinding::new("delete", Delete, Some("TextArea")),
        KeyBinding::new("left", Left, Some("TextArea")),
        KeyBinding::new("right", Right, Some("TextArea")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextArea")),
        KeyBinding::new("shift-right", SelectRight, Some("TextArea")),
        KeyBinding::new("up", Up, Some("TextArea")),
        KeyBinding::new("down", Down, Some("TextArea")),
        KeyBinding::new("shift-up", SelectUp, Some("TextArea")),
        KeyBinding::new("shift-down", SelectDown, Some("TextArea")),
        KeyBinding::new("home", Home, Some("TextArea")),
        KeyBinding::new("end", End, Some("TextArea")),
        KeyBinding::new("shift-home", SelectHome, Some("TextArea")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextArea")),
        KeyBinding::new("enter", Newline, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-left", WordLeft, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-right", WordRight, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-shift-left", SelectWordLeft, Some("TextArea")),
        crate::actions::key_binding(
            cx,
            "secondary-shift-right",
            SelectWordRight,
            Some("TextArea"),
        ),
        crate::actions::key_binding(cx, "secondary-backspace", WordBackspace, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-delete", WordDelete, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-home", DocHome, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-end", DocEnd, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-shift-home", SelectDocHome, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-shift-end", SelectDocEnd, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-a", SelectAll, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-c", Copy, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-x", Cut, Some("TextArea")),
        crate::actions::key_binding(cx, "secondary-v", Paste, Some("TextArea")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_bounds_find_the_line_around_an_offset() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_start(text, 5), 4);
        assert_eq!(line_end(text, 5), 7);
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_end(text, 0), 3);
        assert_eq!(line_start(text, 9), 8);
        assert_eq!(line_end(text, 9), 13);
    }

    #[test]
    fn char_offset_clamps_to_the_line() {
        let text = "abc\nde";
        assert_eq!(char_offset(text, 0, 2), 2);
        assert_eq!(char_offset(text, 0, 9), 3);
        assert_eq!(char_offset(text, 4, 1), 5);
    }

    #[test]
    fn line_starts_split_on_newlines() {
        assert_eq!(line_starts(""), vec![0]);
        assert_eq!(line_starts("a"), vec![0]);
        assert_eq!(line_starts("a\nb\n"), vec![0, 2, 4]);
        assert_eq!(line_starts("a\n\nb"), vec![0, 2, 3]);
    }
}
