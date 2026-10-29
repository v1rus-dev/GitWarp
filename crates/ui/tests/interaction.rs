//! Interaction tests for focus tracking, tab navigation and the overlay stack.

use gpui::{
    div, point, prelude::*, px, AnyView, Entity, FocusHandle, IntoElement, Modifiers, Pixels,
    Render, TestAppContext, VisualTestContext, Window,
};

use ui::theme::ActiveTheme;
use ui::{actions, focus, overlay};

struct TestRoot {
    focus_a: FocusHandle,
    focus_b: FocusHandle,
    focus_c: FocusHandle,
}

impl TestRoot {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_a: cx.focus_handle().tab_index(1).tab_stop(true),
            focus_b: cx.focus_handle().tab_index(2).tab_stop(true),
            focus_c: cx.focus_handle().tab_index(3).tab_stop(true),
        }
    }
}

impl Render for TestRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let content = div()
            .size_full()
            .relative()
            .bg(colors.bg)
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .child(focus_box("A", &self.focus_a)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(140.))
                    .top(px(40.))
                    .child(focus_box("B", &self.focus_b)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(240.))
                    .top(px(40.))
                    .child(focus_box("C", &self.focus_c)),
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
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .track_focus(handle)
        .child(label)
}

struct TestPanel {
    focus: FocusHandle,
    width: Pixels,
    height: Pixels,
}

impl TestPanel {
    fn new(cx: &mut Context<Self>, width: Pixels, height: Pixels) -> Self {
        Self {
            focus: cx.focus_handle(),
            width,
            height,
        }
    }
}

impl Render for TestPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .w(self.width)
            .h(self.height)
            .rounded(px(12.))
            .border_1()
            .border_color(colors.border_strong)
            .bg(colors.surface)
            .track_focus(&self.focus)
            .child("panel")
    }
}

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

fn start(cx: &mut TestAppContext) -> (Entity<TestRoot>, &mut VisualTestContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|_, cx| TestRoot::new(cx));
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

fn keyboard(cx: &mut VisualTestContext) -> bool {
    cx.read(focus::is_keyboard)
}

fn visible(cx: &mut VisualTestContext, handle: &FocusHandle) -> bool {
    cx.update(|window, app| focus::focus_visible(handle, window, app))
}

fn focused(cx: &mut VisualTestContext) -> Option<FocusHandle> {
    cx.update(|window, app| window.focused(app))
}

fn assert_focused(cx: &mut VisualTestContext, handle: &FocusHandle) {
    assert_eq!(focused(cx).as_ref(), Some(handle), "wrong element focused");
}

fn stack_len(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, app| overlay::layer_count(window, app))
}

fn new_panel(cx: &mut TestAppContext, width: f32, height: f32) -> (Entity<TestPanel>, FocusHandle) {
    let panel = cx.update(|app| app.new(|cx| TestPanel::new(cx, px(width), px(height))));
    let focus = cx.read(|app| panel.read(app).focus.clone());
    (panel, focus)
}

#[gpui::test]
fn tab_moves_focus_and_sets_keyboard_modality(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let (a, b) = cx.read(|app| {
        let root = view.read(app);
        (root.focus_a.clone(), root.focus_b.clone())
    });

    assert!(!keyboard(cx), "modality starts on mouse");
    assert!(focused(cx).is_none(), "nothing is focused yet");

    cx.simulate_keystrokes("tab");
    assert_focused(cx, &a);
    assert!(keyboard(cx));
    assert!(visible(cx, &a));

    cx.simulate_keystrokes("tab");
    assert_focused(cx, &b);

    cx.simulate_keystrokes("shift-tab");
    assert_focused(cx, &a);
}

#[gpui::test]
fn mouse_click_focuses_a_box_without_a_ring(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let b = cx.read(|app| view.read(app).focus_b.clone());

    cx.simulate_keystrokes("tab");
    assert!(keyboard(cx));

    cx.simulate_click(point(px(180.), px(53.)), Modifiers::default());

    assert_focused(cx, &b);
    assert!(!keyboard(cx));
    assert!(!visible(cx, &b));
}

#[gpui::test]
fn escape_closes_the_modal_and_restores_focus(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let a = cx.read(|app| view.read(app).focus_a.clone());

    cx.simulate_keystrokes("tab");
    assert_focused(cx, &a);

    let (panel, panel_focus) = new_panel(cx, 360., 160.);
    cx.update(|window, cx| {
        overlay::open_modal(AnyView::from(panel), window, cx);
        window.focus(&panel_focus);
    });
    cx.run_until_parked();
    assert_eq!(stack_len(cx), 1);
    assert_focused(cx, &panel_focus);

    cx.simulate_keystrokes("escape");

    assert_eq!(stack_len(cx), 0);
    assert_focused(cx, &a);
}

#[gpui::test]
fn escape_closes_only_the_top_layer(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let a = cx.read(|app| view.read(app).focus_a.clone());
    cx.simulate_keystrokes("tab");

    let (modal, modal_focus) = new_panel(cx, 360., 160.);
    cx.update(|window, cx| {
        overlay::open_modal(AnyView::from(modal), window, cx);
        window.focus(&modal_focus);
    });

    let (popover, popover_focus) = new_panel(cx, 220., 80.);
    cx.update(|window, cx| {
        overlay::open_popover(
            point(px(100.), px(100.)),
            AnyView::from(popover),
            window,
            cx,
        );
        window.focus(&popover_focus);
    });
    cx.run_until_parked();
    assert_eq!(stack_len(cx), 2);

    cx.simulate_keystrokes("escape");
    assert_eq!(stack_len(cx), 1);
    assert_focused(cx, &modal_focus);

    cx.simulate_keystrokes("escape");
    assert_eq!(stack_len(cx), 0);
    assert_focused(cx, &a);
}

#[gpui::test]
fn closing_a_lower_layer_keeps_focus_on_the_top_layer(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    // Focus something behind the layers, so a restore of the lower layer's
    // saved focus would be observable.
    cx.simulate_keystrokes("tab");

    let (lower, lower_focus) = new_panel(cx, 360., 160.);
    let lower_layer = cx.update(|window, cx| overlay::open_modal(AnyView::from(lower), window, cx));
    cx.update(|window, _| window.focus(&lower_focus));

    let (top, top_focus) = new_panel(cx, 220., 80.);
    cx.update(|window, cx| overlay::open_modal(AnyView::from(top), window, cx));
    cx.update(|window, _| window.focus(&top_focus));
    cx.run_until_parked();
    assert_eq!(stack_len(cx), 2);

    cx.update(|window, cx| overlay::close(lower_layer, window, cx));

    assert_eq!(stack_len(cx), 1);
    assert_focused(cx, &top_focus);
}

#[gpui::test]
fn popover_closes_on_an_outside_click_only(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);

    let (popover, popover_focus) = new_panel(cx, 220., 80.);
    cx.update(|window, cx| {
        overlay::open_popover(
            point(px(100.), px(100.)),
            AnyView::from(popover),
            window,
            cx,
        );
        window.focus(&popover_focus);
    });
    cx.run_until_parked();
    assert_eq!(stack_len(cx), 1);

    cx.simulate_click(point(px(110.), px(110.)), Modifiers::default());
    assert_eq!(stack_len(cx), 1, "click inside the popover closed it");

    cx.simulate_click(point(px(800.), px(800.)), Modifiers::default());
    assert_eq!(stack_len(cx), 0, "click outside did not close the popover");
}

#[gpui::test]
fn modal_closes_on_a_scrim_click_only(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);

    let (modal, modal_focus) = new_panel(cx, 360., 160.);
    cx.update(|window, cx| {
        overlay::open_modal(AnyView::from(modal), window, cx);
        window.focus(&modal_focus);
    });
    cx.run_until_parked();
    assert_eq!(stack_len(cx), 1);

    let viewport = cx.update(|window, _| window.viewport_size());
    let inside = point(viewport.width / 2., viewport.height / 2.);
    cx.simulate_click(inside, Modifiers::default());
    assert_eq!(stack_len(cx), 1, "click inside the modal closed it");

    cx.simulate_click(point(px(5.), px(5.)), Modifiers::default());
    assert_eq!(stack_len(cx), 0, "scrim click did not close the modal");
}
