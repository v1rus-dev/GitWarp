//! Single-line text input, ported from GPUI's `examples/input.rs`.

mod model;
mod textarea;

use std::ops::Range;

use gpui::{
    actions, div, fill, point, px, relative, size, App, AppContext, Bounds, BoxShadow,
    ClipboardItem, Context, CursorStyle, DragMoveEvent, Element, ElementId, ElementInputHandler,
    EmptyView, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    Hsla, InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels, Point, Render, ShapedLine,
    SharedString, StatefulInteractiveElement, Style, Styled, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};

use crate::metrics::{CTL, FONT_UI, FS_BASE, R_SM};
use crate::theme::ActiveTheme;
use model::EditModel;

pub use textarea::{TextArea, TextAreaEvent};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        SelectHome,
        SelectEnd,
        SelectAll,
        Home,
        End,
        WordBackspace,
        WordDelete,
        Paste,
        Cut,
        Copy,
        Submit,
    ]
);

/// Emitted by [`TextInput`] as the text changes or the user submits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    Changed(String),
    Submit,
}

/// The id of the input that owns the active selection drag.
#[derive(Clone)]
struct TextDrag(ElementId);

/// A single-line text field with selection, IME and horizontal scrolling.
pub struct TextInput {
    focus_handle: FocusHandle,
    model: EditModel,
    placeholder: SharedString,
    font_family: SharedString,
    disabled: bool,
    error: bool,
    /// Draw the value as bullets (passwords). Editing still sees the text.
    masked: bool,
    borderless: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_offset: f32,
    is_selecting: bool,
    /// Left/Right at the edge of the text go on to the next binding (see
    /// [`Self::propagate_edge_arrows`]).
    propagate_edge_arrows: bool,
}

impl TextInput {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            model: EditModel::new(),
            placeholder: SharedString::default(),
            font_family: FONT_UI.into(),
            disabled: false,
            error: false,
            masked: false,
            borderless: false,
            last_layout: None,
            last_bounds: None,
            scroll_offset: 0.,
            is_selecting: false,
            propagate_edge_arrows: false,
        }
    }

    /// Opt in to passing Left at the start and Right at the end of the text on
    /// to the next binding instead of swallowing them. Only a field wrapped by
    /// something that uses those arrows should set it: the menu's filter field
    /// opens and closes submenus with them.
    pub fn propagate_edge_arrows(&mut self, on: bool) {
        self.propagate_edge_arrows = on;
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn text(&self) -> &str {
        &self.model.text
    }

    /// The horizontal offset used to keep the cursor visible.
    pub fn scroll_offset(&self) -> Pixels {
        px(self.scroll_offset)
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.set_text(text);
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

    /// The font of the field's text; the UI font by default. Paths and other
    /// machine-readable values read better in the mono font.
    pub fn set_font_family(&mut self, family: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.font_family = family.into();
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        self.focus_handle = self.focus_handle.clone().tab_stop(!disabled);
        cx.notify();
    }

    pub fn set_error(&mut self, error: bool, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    /// Draw the value as bullets, for passwords and other secrets.
    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    pub fn set_max_len(&mut self, max_len: Option<usize>, cx: &mut Context<Self>) {
        self.model.max_len = max_len;
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.model.select_all();
        cx.notify();
    }

    /// Drops the field's own border, padding and background, for an input that
    /// sits in a container which draws them (the command palette's top row).
    pub fn set_borderless(&mut self, borderless: bool) {
        self.borderless = borderless;
    }

    fn after_edit(&mut self, before: String, cx: &mut Context<Self>) {
        if self.model.text != before {
            cx.emit(TextInputEvent::Changed(self.model.text.clone()));
        }
        cx.notify();
    }

    fn delete_backward(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.backspace();
        self.after_edit(before, cx);
    }

    fn delete_forward(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.delete();
        self.after_edit(before, cx);
    }

    // With the caret already at an edge (and no selection to collapse) ← / →
    // have nowhere to go, so the key is passed on to the next binding: a menu
    // around a filter field uses that to open and close submenus.
    fn move_left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.propagate_edge_arrows && self.model.selection.is_empty() && self.model.cursor() == 0
        {
            cx.propagate();
            return;
        }
        self.model.move_left();
        cx.notify();
    }

    fn move_right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.propagate_edge_arrows
            && self.model.selection.is_empty()
            && self.model.cursor() == self.model.text.len()
        {
            cx.propagate();
            return;
        }
        self.model.move_right();
        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_left();
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_right();
        cx.notify();
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.model.move_word_left();
        cx.notify();
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.model.move_word_right();
        cx.notify();
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_word_left();
        cx.notify();
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_word_right();
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.model.move_home();
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.model.move_end();
        cx.notify();
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_home();
        cx.notify();
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_end();
        cx.notify();
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.model.select_all();
        cx.notify();
    }

    fn delete_word_left(&mut self, _: &WordBackspace, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.delete_word_left();
        self.after_edit(before, cx);
    }

    fn delete_word_right(&mut self, _: &WordDelete, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.model.text.clone();
        self.model.delete_word_right();
        self.after_edit(before, cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submit);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let single_line = text.replace(['\n', '\r'], " ");
            let before = self.model.text.clone();
            self.model.insert(&single_line);
            self.after_edit(before, cx);
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
            let before = self.model.text.clone();
            self.model.replace_range(self.model.selection.clone(), "");
            self.after_edit(before, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);

        if event.click_count >= 2 {
            let range = self.model.word_range(index);
            self.model.selection = range;
            self.model.reversed = false;
            cx.notify();
        } else if event.modifiers.shift {
            self.model.select_to(index);
            cx.notify();
        } else {
            self.model.move_to(index);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection(event.position, cx);
    }

    /// Extends the selection while the pointer is down. Mouse-move listeners
    /// on the element only fire while it is hovered, so the drag path below
    /// also routes moves that happen outside the field.
    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.is_selecting {
            let index = self.index_for_mouse_position(position);
            self.model.select_to(index);
            cx.notify();
        }
    }

    fn on_drag_move(
        &mut self,
        event: &DragMoveEvent<TextDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_selection(event.event.position, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.model.text.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.model.text.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + px(self.scroll_offset))
    }
}

impl EntityInputHandler for TextInput {
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
        let before = self.model.text.clone();
        self.model.replace_range(range, new_text);
        self.after_edit(before, cx);
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
        let last_layout = self.last_layout.as_ref()?;
        let range = self.model.range_from_utf16(&range_utf16);
        let scroll = px(self.scroll_offset);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start) - scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end) - scroll,
                bounds.bottom(),
            ),
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
        let last_layout = self.last_layout.as_ref()?;
        let utf8 = last_layout.index_for_x(point.x - bounds.left() + px(self.scroll_offset))?;
        Some(self.model.offset_to_utf16(utf8))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
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
        style.size.height = window.line_height().into();
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
        let content = input.model.text.clone();
        let selected_range = input.model.selection.clone();
        let cursor = input.model.cursor();
        let marked = input.model.marked.clone();
        let disabled = input.disabled;
        let scroll = input.scroll_offset;
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            let color = if disabled {
                colors.disabled
            } else {
                colors.muted
            };
            (input.placeholder.clone(), color)
        } else {
            // One bullet per byte keeps every index (cursor, selection, IME
            // marked range) valid; a multi-byte character shows as two or
            // three bullets, which is what a password field may do.
            let text: SharedString = if input.masked {
                "*".repeat(content.len()).into()
            } else {
                content.clone().into()
            };
            (
                text,
                if disabled {
                    colors.disabled
                } else {
                    style.color
                },
            )
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if content.is_empty() {
            vec![run]
        } else if let Some(marked_range) = marked.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let text_len = display_text.len();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let view_width: f32 = bounds.size.width.into();
        let text_width: f32 = line.x_for_index(text_len).into();
        let cursor_x: f32 = line.x_for_index(cursor).into();
        let mut scroll_offset = scroll;
        if cursor_x - scroll_offset > view_width - 2. {
            scroll_offset = cursor_x - view_width + 2.;
        }
        if cursor_x - scroll_offset < 0. {
            scroll_offset = cursor_x;
        }
        scroll_offset = scroll_offset.clamp(0., (text_width - view_width).max(0.));
        let scroll_px = px(scroll_offset);

        let (selection, cursor) = if selected_range.is_empty() {
            let cursor = if input.focus_handle.is_focused(window) && !disabled {
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.left() - scroll_px + line.x_for_index(cursor),
                            bounds.top(),
                        ),
                        size(px(1.), bounds.bottom() - bounds.top()),
                    ),
                    colors.text,
                ))
            } else {
                None
            };
            (None, cursor)
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() - scroll_px + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() - scroll_px + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    colors.selection,
                )),
                None,
            )
        };

        self.input.update(cx, |input, _| {
            input.scroll_offset = scroll_offset;
            input.last_bounds = Some(bounds);
        });

        PrepaintState {
            line: Some(line),
            cursor,
            selection,
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
        let (focus_handle, disabled) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), input.disabled)
        };

        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint.line.take().expect("text line must be shaped");
        let scroll = self.input.read(cx).scroll_offset;
        line.paint(
            point(bounds.left() - px(scroll), bounds.top()),
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .expect("text line must paint");

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focused = !self.disabled && self.focus_handle.is_focused(window);
        let border = if self.disabled {
            colors.border
        } else if self.error {
            colors.error
        } else if focused {
            colors.focus
        } else {
            colors.border_strong
        };
        let ring_color: Option<Hsla> = if self.disabled {
            None
        } else if self.error {
            Some(colors.error)
        } else if focused {
            Some(colors.focus)
        } else {
            None
        };

        let input_id = ElementId::Name(format!("text-input-{}", cx.entity_id()).into());
        let mut root = div()
            .id(input_id.clone())
            .relative()
            .h(CTL)
            .flex()
            .items_center()
            .rounded(R_SM)
            .border_1()
            .border_color(border)
            .text_size(FS_BASE)
            .font_family(self.font_family.clone())
            .text_color(if self.disabled {
                colors.disabled
            } else {
                colors.text
            })
            .overflow_hidden();

        if self.borderless {
            root = root.px(px(0.)).border_0().bg(gpui::transparent_black());
        } else {
            root = root.px(px(8.)).bg(colors.bg);
            if let Some(color) = ring_color {
                root = root.shadow(vec![BoxShadow {
                    color,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                }]);
            }
            if !self.disabled && !focused && !self.error {
                let hover_border = colors.muted;
                root = root.hover(move |style| style.border_color(hover_border));
            }
        }

        if self.disabled {
            root = root.cursor_not_allowed();
        } else {
            let drag_id = input_id.clone();
            root = root
                .cursor(CursorStyle::IBeam)
                .track_focus(&self.focus_handle)
                .key_context("TextInput")
                .on_action(cx.listener(Self::delete_backward))
                .on_action(cx.listener(Self::delete_forward))
                .on_action(cx.listener(Self::move_left))
                .on_action(cx.listener(Self::move_right))
                .on_action(cx.listener(Self::select_left))
                .on_action(cx.listener(Self::select_right))
                .on_action(cx.listener(Self::word_left))
                .on_action(cx.listener(Self::word_right))
                .on_action(cx.listener(Self::select_word_left))
                .on_action(cx.listener(Self::select_word_right))
                .on_action(cx.listener(Self::home))
                .on_action(cx.listener(Self::end))
                .on_action(cx.listener(Self::select_home))
                .on_action(cx.listener(Self::select_end))
                .on_action(cx.listener(Self::select_all_action))
                .on_action(cx.listener(Self::submit))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_action(cx.listener(Self::delete_word_left))
                .on_action(cx.listener(Self::delete_word_right))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_drag(TextDrag(drag_id.clone()), |_, _, _, cx| {
                    cx.new(|_| EmptyView)
                })
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<TextDrag>, window, cx| {
                        if event.drag(cx).0 != drag_id {
                            return;
                        }
                        this.on_drag_move(event, window, cx);
                    },
                ));
        }

        root.child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .flex()
                .items_center()
                .child(TextElement { input: cx.entity() }),
        )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TextInputEvent> for TextInput {}

/// Keeps the input's keys ahead of the global context-less `ui` bindings.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-left", WordLeft, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-right", WordRight, Some("TextInput")),
        crate::actions::key_binding(
            cx,
            "secondary-shift-left",
            SelectWordLeft,
            Some("TextInput"),
        ),
        crate::actions::key_binding(
            cx,
            "secondary-shift-right",
            SelectWordRight,
            Some("TextInput"),
        ),
        crate::actions::key_binding(cx, "secondary-backspace", WordBackspace, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-delete", WordDelete, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("shift-home", SelectHome, Some("TextInput")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-a", SelectAll, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-c", Copy, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-x", Cut, Some("TextInput")),
        crate::actions::key_binding(cx, "secondary-v", Paste, Some("TextInput")),
        KeyBinding::new("enter", Submit, Some("TextInput")),
    ]);
    textarea::bind_keys(cx);
}
