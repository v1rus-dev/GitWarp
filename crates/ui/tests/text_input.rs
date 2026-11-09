//! GPUI interaction tests for the text input.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, point, prelude::*, px, Context, Entity, EntityInputHandler, IntoElement, Modifiers,
    MouseButton, Render, TestAppContext, VisualTestContext, Window,
};

use ui::actions::{Cancel, MenuOpenSubmenu, SelectFirst};
use ui::{TextInput, TextInputEvent};

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

fn start(
    cx: &mut TestAppContext,
    configure: impl FnOnce(&mut TextInput, &mut Context<TextInput>),
) -> (Entity<InputRoot>, &mut VisualTestContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            configure(&mut input, cx);
            input
        });
        InputRoot::new(input)
    });
    cx.update(|window, _| window.activate_window());
    let handle = cx.read(|app| view.read(app).input.read(app).focus_handle().clone());
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.run_until_parked();
    (view, cx)
}

struct InputRoot {
    input: Entity<TextInput>,
    cancels: Rc<Cell<u32>>,
    firsts: Rc<Cell<u32>>,
}

impl InputRoot {
    fn new(input: Entity<TextInput>) -> Self {
        Self {
            input,
            cancels: Rc::new(Cell::new(0)),
            firsts: Rc::new(Cell::new(0)),
        }
    }
}

impl Render for InputRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .on_action(
                cx.listener(|this, _: &Cancel, _, _| this.cancels.set(this.cancels.get() + 1)),
            )
            .on_action(
                cx.listener(|this, _: &SelectFirst, _, _| this.firsts.set(this.firsts.get() + 1)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .w(px(300.))
                    .child(self.input.clone()),
            )
    }
}

fn text(cx: &TestAppContext, view: &Entity<InputRoot>) -> String {
    cx.read(|app| view.read(app).input.read(app).text().to_owned())
}

#[gpui::test]
fn typing_and_editing(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});

    cx.simulate_input("hello");
    assert_eq!(text(cx, &view), "hello");

    cx.simulate_keystrokes("left left backspace");
    assert_eq!(text(cx, &view), "helo");
}

#[gpui::test]
fn select_all_then_type_replaces(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});
    cx.simulate_input("hello");

    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("x");

    assert_eq!(text(cx, &view), "x");
}

#[gpui::test]
fn home_and_end_stay_inside_the_input(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});
    cx.simulate_input("abc");

    cx.simulate_keystrokes("home");
    cx.simulate_input("X");
    assert_eq!(text(cx, &view), "Xabc");

    cx.simulate_keystrokes("end");
    cx.simulate_input("Y");
    assert_eq!(text(cx, &view), "XabcY");

    assert_eq!(
        cx.read(|app| view.read(app).firsts.get()),
        0,
        "home/end leaked to the ancestor SelectFirst handler"
    );
}

#[gpui::test]
fn enter_submits_and_escape_reaches_the_ancestor(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});
    let submitted = Rc::new(Cell::new(false));
    let input = cx.read(|app| view.read(app).input.clone());
    let _subscription = {
        let submitted = submitted.clone();
        cx.update(|_, app| {
            app.subscribe(&input, move |_, event: &TextInputEvent, _| {
                if matches!(event, TextInputEvent::Submit) {
                    submitted.set(true);
                }
            })
        })
    };

    cx.simulate_keystrokes("enter");
    assert!(submitted.get(), "enter did not emit Submit");

    cx.simulate_keystrokes("escape");
    assert_eq!(
        cx.read(|app| view.read(app).cancels.get()),
        1,
        "escape did not bubble to Cancel"
    );
}

#[gpui::test]
fn disabled_input_ignores_typing(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |input, cx| {
        input.set_disabled(true, cx);
        input.set_placeholder("disabled", cx);
    });

    cx.simulate_input("hello");
    assert_eq!(text(cx, &view), "");
}

#[gpui::test]
fn max_len_is_enforced(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |input, cx| input.set_max_len(Some(3), cx));

    cx.simulate_input("hello");
    assert_eq!(text(cx, &view), "hel");
}

#[gpui::test]
fn copy_and_paste_use_the_clipboard(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});
    cx.simulate_input("hello");

    cx.simulate_keystrokes("secondary-a secondary-c");
    let clipboard = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("nothing was copied");
    assert_eq!(clipboard, "hello");

    cx.simulate_keystrokes("end secondary-v");
    assert_eq!(text(cx, &view), "hellohello");
}

#[gpui::test]
fn ime_max_len_truncation_keeps_the_selection_on_a_char_boundary(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |input, cx| {
        input.set_text("X😀😀Z", cx);
        input.set_max_len(Some(4), cx);
    });
    let input = cx.read(|app| view.read(app).input.clone());

    // Replace the emoji with "aé" and select all of it; only "a" fits. The
    // selection must not end inside the following emoji.
    cx.update(|window, app| {
        input.update(app, |input, cx| {
            input.replace_and_mark_text_in_range(Some(1..3), "aé", Some(0..3), window, cx);
        })
    });

    cx.simulate_keystrokes("secondary-c");
    let clipboard = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("nothing was copied; the selection is not on a char boundary");
    assert_eq!(
        clipboard, "a",
        "the selection does not cover the inserted text"
    );
}

#[gpui::test]
fn dragging_the_selection_past_the_right_edge_keeps_extending(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_, _| {});
    let long = "abcdefghij".repeat(20);
    let input = cx.read(|app| view.read(app).input.clone());
    cx.update(|_, app| input.update(app, |input, cx| input.set_text(long.clone(), cx)));
    cx.simulate_keystrokes("home");

    cx.simulate_mouse_down(
        point(px(45.), px(53.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(45.), px(53.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(1200.), px(53.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(2000.), px(53.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        point(px(2000.), px(53.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    cx.simulate_keystrokes("secondary-c");
    let clipboard = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("nothing was selected: the drag stopped at the field edge");
    assert_eq!(
        clipboard, long,
        "dragging past the right edge must select to the end of the text"
    );
    let scroll = cx.read(|app| view.read(app).input.read(app).scroll_offset());
    assert!(
        scroll > px(0.),
        "the text did not scroll to keep the dragged cursor visible"
    );
}

#[gpui::test]
fn tab_focuses_the_input(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| TextInput::new(window, cx));
        InputRoot::new(input)
    });
    cx.update(|window, _| window.activate_window());

    cx.simulate_keystrokes("tab");
    cx.simulate_input("hi");
    assert_eq!(text(cx, &view), "hi");
}

/// Counts the `MenuOpenSubmenu` actions that reach a `probe` ancestor bound to
/// Right, i.e. the Right presses the input passed on.
struct EdgeProbe {
    input: Entity<TextInput>,
    passed_on: Rc<Cell<u32>>,
}

impl Render for EdgeProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context("probe")
            .on_action(cx.listener(|this, _: &MenuOpenSubmenu, _, _| {
                this.passed_on.set(this.passed_on.get() + 1)
            }))
            .child(div().w(px(300.)).child(self.input.clone()))
    }
}

fn right_at_the_end_passed_on(cx: &mut TestAppContext, propagate: bool) -> u32 {
    init(cx);
    cx.update(|cx| {
        cx.bind_keys([gpui::KeyBinding::new(
            "right",
            MenuOpenSubmenu,
            Some("probe"),
        )])
    });
    let passed_on = Rc::new(Cell::new(0));
    let probe = passed_on.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.propagate_edge_arrows(propagate);
            input
        });
        EdgeProbe {
            input,
            passed_on: probe,
        }
    });
    cx.update(|window, _| window.activate_window());
    let handle = cx.read(|app| view.read(app).input.read(app).focus_handle().clone());
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.run_until_parked();
    cx.simulate_input("ab");
    // The caret is at the end of the text: Right has nowhere to go.
    cx.simulate_keystrokes("right");
    passed_on.get()
}

#[gpui::test]
fn right_at_the_end_stays_in_the_input_by_default(cx: &mut TestAppContext) {
    assert_eq!(right_at_the_end_passed_on(cx, false), 0);
}

#[gpui::test]
fn right_at_the_end_passes_on_when_the_input_opted_in(cx: &mut TestAppContext) {
    assert_eq!(right_at_the_end_passed_on(cx, true), 1);
}
