//! GPUI interaction tests for the floating components.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, Context, Entity, FocusHandle, IntoElement, KeyUpEvent, Keystroke,
    Modifiers, Render, TestAppContext, VisualTestContext, Window,
};

use ui::{
    actions, dialog, focus, menu, overlay, toast, Dialog, DialogFocus, MenuEntry, MenuItem, Select,
    TextInput, ToastKind,
};

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

struct TestRoot {
    a: FocusHandle,
    b: FocusHandle,
    c: FocusHandle,
    outside: FocusHandle,
}

impl TestRoot {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            a: cx.focus_handle().tab_stop(true),
            b: cx.focus_handle().tab_stop(true),
            c: cx.focus_handle().tab_stop(true),
            outside: cx.focus_handle().tab_stop(true),
        }
    }
}

impl Render for TestRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .size_full()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .child(focus_box("outside", &self.outside)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(140.))
                    .top(px(40.))
                    .child(focus_box("a", &self.a)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(240.))
                    .top(px(40.))
                    .child(focus_box("b", &self.b)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(340.))
                    .top(px(40.))
                    .child(focus_box("c", &self.c)),
            );
        let content = focus::track_pointer(actions::tab_navigation(content));
        content.child(overlay::render_overlays(window, cx))
    }
}

fn focus_box(label: &'static str, handle: &FocusHandle) -> impl IntoElement {
    div()
        .id(label)
        .w(px(80.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .track_focus(handle)
        .child(label)
}

fn start(cx: &mut TestAppContext) -> (Entity<TestRoot>, &mut VisualTestContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|_, cx| TestRoot::new(cx));
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

/// `simulate_keystrokes` only sends key-down; keyboard clicks run on key-up.
fn press(cx: &mut VisualTestContext, keystroke: &str) {
    cx.simulate_keystrokes(keystroke);
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(keystroke).expect("valid keystroke"),
    });
}

fn focused(cx: &mut VisualTestContext) -> Option<FocusHandle> {
    cx.update(|window, app| window.focused(app))
}

fn assert_focused(cx: &mut VisualTestContext, handle: &FocusHandle) {
    assert_eq!(focused(cx).as_ref(), Some(handle), "wrong element focused");
}

fn layer_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, app| overlay::layer_count(window, app))
}

fn menu_item(label: &'static str, enabled: bool, hits: &Rc<Cell<u32>>) -> MenuEntry {
    let hits = hits.clone();
    MenuEntry::Item(
        MenuItem::new(label)
            .disabled(!enabled)
            .action(move |_, _| hits.set(hits.get() + 1)),
    )
}

#[gpui::test]
fn menu_down_down_enter_runs_the_second_enabled_item_and_closes(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let first = Rc::new(Cell::new(0));
    let second = Rc::new(Cell::new(0));
    let entries = vec![
        menu_item("First", true, &first),
        menu_item("Second", true, &second),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem::new("Disabled").disabled(true)),
    ];

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    cx.simulate_keystrokes("down down enter");

    assert_eq!(first.get(), 0, "the first item ran");
    assert_eq!(second.get(), 1, "the second item did not run");
    assert_eq!(layer_count(cx), 0, "the menu stayed open");
}

#[gpui::test]
fn menu_up_from_nothing_highlights_the_last_enabled_item(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let first = Rc::new(Cell::new(0));
    let last = Rc::new(Cell::new(0));
    let entries = vec![
        menu_item("First", true, &first),
        menu_item("Last", true, &last),
        MenuEntry::Item(MenuItem::new("Disabled").disabled(true)),
    ];

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("up enter");

    assert_eq!(first.get(), 0, "up did not start from the last item");
    assert_eq!(last.get(), 1, "the last enabled item did not run");
    assert_eq!(layer_count(cx), 0);
}

#[gpui::test]
fn menu_skips_disabled_items(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let first = Rc::new(Cell::new(0));
    let hidden = Rc::new(Cell::new(0));
    let last = Rc::new(Cell::new(0));
    let entries = vec![
        menu_item("First", true, &first),
        menu_item("Disabled", false, &hidden),
        menu_item("Last", true, &last),
    ];

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("down down enter");

    assert_eq!(first.get(), 0);
    assert_eq!(hidden.get(), 0, "a disabled item ran");
    assert_eq!(last.get(), 1, "down did not skip the disabled item");
}

#[gpui::test]
fn menu_escape_closes_without_running_anything(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));
    let entries = vec![menu_item("Item", true, &hits)];

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("down escape");

    assert_eq!(hits.get(), 0, "escape ran the highlighted item");
    assert_eq!(layer_count(cx), 0, "escape did not close the menu");
}

#[gpui::test]
fn menu_closes_on_a_click_outside(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));
    let entries = vec![menu_item("Item", true, &hits)];

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    cx.simulate_click(point(px(420.), px(300.)), Modifiers::default());

    assert_eq!(layer_count(cx), 0, "a click outside did not close the menu");
    assert_eq!(hits.get(), 0);
}

fn open_test_dialog(
    cx: &mut VisualTestContext,
    root: &Entity<TestRoot>,
    danger: bool,
    busy: bool,
    hits: &Rc<Cell<u32>>,
) -> ui::overlay::LayerId {
    let (a, b, c) = cx.read(|app| {
        let root = root.read(app);
        (root.a.clone(), root.b.clone(), root.c.clone())
    });
    let hits = hits.clone();
    cx.update(|window, app| {
        let (body_a, body_b, body_c) = (a.clone(), b.clone(), c.clone());
        dialog::open(window, app, move |_, _| {
            Dialog::new("Test dialog")
                .body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(focus_box("dialog-a", &body_a))
                        .child(focus_box("dialog-b", &body_b))
                        .child(focus_box("dialog-c", &body_c)),
                )
                .focus_order([
                    DialogFocus::input(body_a.clone()),
                    DialogFocus::control(body_b.clone()),
                    DialogFocus::control(body_c.clone()),
                ])
                .autofocus(body_a.clone())
                .default_button(body_b.clone())
                .default_danger(danger)
                .busy(busy)
                .on_default({
                    let hits = hits.clone();
                    move |_, _| hits.set(hits.get() + 1)
                })
        })
    })
}

#[gpui::test]
fn dialog_tab_wraps_inside(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let a = cx.read(|app| view.read(app).a.clone());
    let b = cx.read(|app| view.read(app).b.clone());
    let c = cx.read(|app| view.read(app).c.clone());
    let hits = Rc::new(Cell::new(0));

    open_test_dialog(cx, &view, false, false, &hits);
    cx.run_until_parked();
    assert_focused(cx, &a);

    cx.simulate_keystrokes("tab");
    assert_focused(cx, &b);
    cx.simulate_keystrokes("tab");
    assert_focused(cx, &c);
    cx.simulate_keystrokes("tab");
    assert_focused(cx, &a);
    cx.simulate_keystrokes("shift-tab");
    assert_focused(cx, &c);
}

#[gpui::test]
fn dialog_escape_closes_unless_busy_and_restores_focus(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let outside = cx.read(|app| view.read(app).outside.clone());
    let hits = Rc::new(Cell::new(0));

    cx.update(|window, _| window.focus(&outside));
    let _layer = open_test_dialog(cx, &view, false, false, &hits);
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 0, "escape did not close the dialog");
    assert_focused(cx, &outside);

    cx.update(|window, _| window.focus(&outside));
    let layer = open_test_dialog(cx, &view, false, true, &hits);
    cx.run_until_parked();

    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 1, "escape closed a busy dialog");

    cx.simulate_click(point(px(5.), px(5.)), Modifiers::default());
    assert_eq!(layer_count(cx), 1, "the scrim closed a busy dialog");

    cx.update(|window, app| dialog::set_busy(layer, false, window, app));
    cx.simulate_keystrokes("escape");
    assert_eq!(
        layer_count(cx),
        0,
        "escape did not close after clearing busy"
    );
}

#[gpui::test]
fn dialog_enter_runs_the_default_button(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));

    open_test_dialog(cx, &view, false, false, &hits);
    cx.run_until_parked();

    cx.simulate_keystrokes("enter");
    assert_eq!(hits.get(), 1, "enter did not run the default button");
}

#[gpui::test]
fn dialog_enter_ignores_a_danger_default(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));

    open_test_dialog(cx, &view, true, false, &hits);
    cx.run_until_parked();

    cx.simulate_keystrokes("enter");
    assert_eq!(hits.get(), 0, "a danger default was activated");
}

#[gpui::test]
fn dialog_enter_in_a_text_input_submits_the_default(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));
    let input = cx.update(|window, app| app.new(|cx| TextInput::new(window, cx)));
    let focus = cx.read(|app| input.read(app).focus_handle().clone());

    cx.update(|window, app| {
        let body = input.clone();
        let autofocus = focus.clone();
        let default = focus.clone();
        let hits = hits.clone();
        dialog::open(window, app, move |_, _| {
            Dialog::new("Rename branch")
                .body(body.clone())
                .focus_order([DialogFocus::input(autofocus.clone())])
                .autofocus(autofocus.clone())
                .default_button(default.clone())
                .on_default({
                    let hits = hits.clone();
                    move |_, _| hits.set(hits.get() + 1)
                })
        });
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("enter");
    assert_eq!(hits.get(), 1, "enter in a text input did not submit");
}

#[gpui::test]
fn error_toast_stays_and_info_expires(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);

    cx.update(|window, app| {
        toast::show(ToastKind::Error, "Push rejected", window, app);
        toast::show(ToastKind::Info, "Fetched origin", window, app);
    });
    assert_eq!(cx.update(|window, app| toast::count(window, app)), 2);

    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();

    let messages: Vec<String> = cx.update(|window, app| {
        toast::toasts(window, app)
            .into_iter()
            .map(|toast| toast.message.to_string())
            .collect()
    });
    assert_eq!(messages, vec!["Push rejected".to_string()]);
}

struct SelectRoot {
    chosen: Rc<Cell<Option<usize>>>,
}

impl Render for SelectRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.chosen.clone();
        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(220.))
                .child(
                    Select::new("select")
                        .options(["main", "develop", "feat/lanes", "release/0.3"])
                        .selected(0)
                        .on_change(move |index, _, _| chosen.set(Some(index))),
                ),
        );
        let content = focus::track_pointer(actions::tab_navigation(content));
        content.child(overlay::render_overlays(window, cx))
    }
}

#[gpui::test]
fn choosing_a_select_option_calls_on_change(cx: &mut TestAppContext) {
    init(cx);
    let chosen = Rc::new(Cell::new(None));
    let (_, cx) = cx.add_window_view(|_, _| SelectRoot {
        chosen: chosen.clone(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    cx.simulate_keystrokes("tab");
    press(cx, "enter");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "enter did not open the select menu");

    cx.simulate_keystrokes("down down enter");
    assert_eq!(
        chosen.get(),
        Some(1),
        "choosing an option did not report it"
    );
    assert_eq!(layer_count(cx), 0, "the select menu stayed open");
}
