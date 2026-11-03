//! GPUI interaction tests for the multi-line text area.

use gpui::{
    div, prelude::*, px, App, Context, Entity, IntoElement, Render, TestAppContext,
    VisualTestContext, Window,
};

use ui::TextArea;

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

struct Root {
    area: Entity<TextArea>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(12.)).child(self.area.clone())
    }
}

/// Opens a text area with the keyboard focus and returns its text.
fn start(cx: &mut TestAppContext) -> (Entity<Root>, &mut VisualTestContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let area = cx.new(|cx| TextArea::new(window, cx));
        Root { area }
    });
    cx.update(|window, _| window.activate_window());
    let handle = cx.read(|app| view.read(app).area.read(app).focus_handle().clone());
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.run_until_parked();
    (view, cx)
}

fn text(view: &Entity<Root>, cx: &App) -> String {
    view.read(cx).area.read(cx).text().to_string()
}

#[test]
fn enter_inserts_a_newline_and_arrows_move_between_lines() {
    let mut cx = TestAppContext::single();
    let (view, cx) = start(&mut cx);

    cx.simulate_input("subject");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("body");
    assert_eq!(cx.read(|app| text(&view, app)), "subject\nbody");

    // Up keeps the column; Home/End work inside the line.
    cx.simulate_keystrokes("up");
    cx.simulate_keystrokes("home");
    cx.simulate_input("X");
    assert_eq!(cx.read(|app| text(&view, app)), "Xsubject\nbody");

    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    assert_eq!(cx.read(|app| text(&view, app)), "Xsubject!\nbody");

    // Down from the end of the first line lands inside the shorter second.
    cx.simulate_keystrokes("down");
    cx.simulate_input("Y");
    assert_eq!(cx.read(|app| text(&view, app)), "Xsubject!\nbodyY");
}

#[test]
fn paste_keeps_newlines_and_backspace_joins_lines() {
    let mut cx = TestAppContext::single();
    let (view, cx) = start(&mut cx);

    cx.simulate_input("one\ntwo\nthree");
    assert_eq!(cx.read(|app| text(&view, app)), "one\ntwo\nthree");

    // Backspace at the start of a line joins it with the previous one.
    cx.simulate_keystrokes("home");
    cx.simulate_keystrokes("backspace");
    assert_eq!(cx.read(|app| text(&view, app)), "one\ntwothree");
}

#[test]
fn shift_arrows_select_and_typing_replaces_the_selection() {
    let mut cx = TestAppContext::single();
    let (view, cx) = start(&mut cx);

    cx.simulate_input("hello");
    cx.simulate_keystrokes("home shift-right shift-right");
    cx.simulate_input("HE");
    assert_eq!(cx.read(|app| text(&view, app)), "HEllo");
}
