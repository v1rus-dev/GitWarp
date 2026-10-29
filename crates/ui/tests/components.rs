//! GPUI interaction tests for the static controls.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, point, prelude::*, px, Context, Entity, IntoElement, KeyUpEvent, Keystroke, Modifiers,
    Render, TestAppContext, VisualTestContext, Window,
};

use ui::{actions, focus, Button, Checkbox, SegmentItem, Segmented};

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

/// `simulate_keystrokes` only sends key-down; GPUI's keyboard activation runs on key-up.
fn press(cx: &mut VisualTestContext, keystroke: &str) {
    cx.simulate_keystrokes(keystroke);
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(keystroke).expect("valid keystroke"),
    });
}

fn start<V: Render + 'static>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Context<V>) -> V,
) -> (Entity<V>, &mut VisualTestContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|_, cx| build(cx));
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

struct ButtonRoot {
    clicks: Rc<Cell<u32>>,
    disabled_clicks: Rc<Cell<u32>>,
}

impl Render for ButtonRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let disabled_clicks = self.disabled_clicks.clone();
        let content = div()
            .size_full()
            .relative()
            .child(
                div().absolute().left(px(40.)).top(px(40.)).child(
                    Button::new("btn")
                        .label("Click")
                        .min_width(px(80.))
                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                ),
            )
            .child(
                div().absolute().left(px(140.)).top(px(40.)).child(
                    Button::new("btn-disabled")
                        .label("Nope")
                        .min_width(px(80.))
                        .disabled(true)
                        .on_click(move |_, _, _| disabled_clicks.set(disabled_clicks.get() + 1)),
                ),
            );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

impl ButtonRoot {
    fn new() -> Self {
        Self {
            clicks: Rc::new(Cell::new(0)),
            disabled_clicks: Rc::new(Cell::new(0)),
        }
    }
}

#[gpui::test]
fn button_click_fires(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| ButtonRoot::new());
    cx.simulate_click(point(px(80.), px(53.)), Modifiers::default());
    assert_eq!(cx.read(|app| view.read(app).clicks.get()), 1);
}

#[gpui::test]
fn focused_button_fires_on_enter_and_space(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| ButtonRoot::new());

    cx.simulate_keystrokes("tab");
    assert_eq!(
        cx.read(|app| view.read(app).clicks.get()),
        0,
        "tab must not click"
    );

    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).clicks.get()),
        1,
        "enter did not click"
    );

    press(cx, "space");
    assert_eq!(
        cx.read(|app| view.read(app).clicks.get()),
        2,
        "space did not click"
    );
}

#[gpui::test]
fn disabled_button_ignores_click_and_tab(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| ButtonRoot::new());

    cx.simulate_click(point(px(180.), px(53.)), Modifiers::default());
    assert_eq!(
        cx.read(|app| view.read(app).disabled_clicks.get()),
        0,
        "disabled clicked"
    );

    cx.simulate_keystrokes("tab");
    press(cx, "enter");

    assert_eq!(
        cx.read(|app| view.read(app).clicks.get()),
        1,
        "tab did not reach the button"
    );
    assert_eq!(
        cx.read(|app| view.read(app).disabled_clicks.get()),
        0,
        "tab reached the disabled button"
    );
}

struct CheckboxRoot {
    toggles: Rc<Cell<Option<bool>>>,
    disabled_toggles: Rc<Cell<Option<bool>>>,
}

impl CheckboxRoot {
    fn new() -> Self {
        Self {
            toggles: Rc::new(Cell::new(None)),
            disabled_toggles: Rc::new(Cell::new(None)),
        }
    }
}

impl Render for CheckboxRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let toggles = self.toggles.clone();
        let disabled_toggles = self.disabled_toggles.clone();
        let content = div()
            .size_full()
            .relative()
            .child(
                div().absolute().left(px(40.)).top(px(40.)).child(
                    Checkbox::new("check")
                        .label("Toggle")
                        .checked(false)
                        .on_toggle(move |next, _, _| toggles.set(Some(next))),
                ),
            )
            .child(
                div().absolute().left(px(40.)).top(px(90.)).child(
                    Checkbox::new("check-disabled")
                        .label("Disabled")
                        .checked(false)
                        .disabled(true)
                        .on_toggle(move |next, _, _| disabled_toggles.set(Some(next))),
                ),
            );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn space_toggles_a_checkbox_and_disabled_ignores(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| CheckboxRoot::new());

    cx.simulate_keystrokes("tab");
    press(cx, "space");
    assert_eq!(cx.read(|app| view.read(app).toggles.get()), Some(true));

    cx.simulate_click(point(px(47.), px(103.)), Modifiers::default());
    assert_eq!(cx.read(|app| view.read(app).disabled_toggles.get()), None);
}

struct SegmentedRoot {
    selected: Rc<Cell<Option<usize>>>,
}

impl Render for SegmentedRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let content = div().size_full().relative().child(
            div().absolute().left(px(40.)).top(px(40.)).child(
                Segmented::new("seg")
                    .items([
                        SegmentItem::new("One"),
                        SegmentItem::new("Two"),
                        SegmentItem::new("Three"),
                    ])
                    .selected(0)
                    .on_select(move |index, _, _| selected.set(Some(index))),
            ),
        );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn clicking_a_segment_selects_it(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| SegmentedRoot {
        selected: Rc::new(Cell::new(None)),
    });

    let bounds = cx
        .debug_bounds("seg-segment-2")
        .expect("segment bounds were not drawn");
    cx.simulate_click(bounds.center(), Modifiers::default());

    assert_eq!(cx.read(|app| view.read(app).selected.get()), Some(2));
}
