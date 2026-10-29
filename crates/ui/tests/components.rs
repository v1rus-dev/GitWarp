//! GPUI interaction tests for the static controls.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, point, prelude::*, px, Context, Entity, IntoElement, KeyUpEvent, Keystroke, Modifiers,
    Render, TestAppContext, VisualTestContext, Window,
};

use ui::{
    actions, focus, Button, Checkbox, SearchField, SegmentItem, Segmented, Tab, Tabs, TextInput,
};

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

struct TabsRoot {
    labels: Vec<&'static str>,
    selected: Rc<Cell<Option<usize>>>,
}

impl Render for TabsRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let tabs = Tabs::new("tabs")
            .tabs(
                self.labels
                    .iter()
                    .map(|label| Tab::new(*label).id(*label).closable()),
            )
            .selected(0)
            .on_select(move |index, _, _| selected.set(Some(index)));
        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(320.))
                .child(tabs),
        );
        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn closing_a_tab_keeps_focus_on_the_same_tab(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| TabsRoot {
        labels: vec!["One", "Two", "Three"],
        selected: Rc::new(Cell::new(None)),
    });

    // Focus the second tab and select it.
    cx.simulate_keystrokes("tab tab");
    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(1),
        "enter did not select the focused tab"
    );

    // Closing the first tab shifts "Two" to index 0. Focus must stay on it,
    // so Enter selects index 0 rather than the tab that took index 1.
    cx.update(|_, app| {
        view.update(app, |root, cx| {
            root.labels.remove(0);
            cx.notify();
        })
    });
    cx.run_until_parked();

    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(0),
        "focus moved to a different tab when tab 1 closed"
    );
}

struct DuplicateTabsRoot {
    selected: Rc<Cell<Option<usize>>>,
}

impl Render for DuplicateTabsRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let tabs = Tabs::new("duplicate-tabs")
            .tabs([Tab::new("README.md"), Tab::new("README.md")])
            .selected(0)
            .on_select(move |index, _, _| selected.set(Some(index)));
        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(320.))
                .child(tabs),
        );
        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn duplicate_tab_labels_get_distinct_focus_handles(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| DuplicateTabsRoot {
        selected: Rc::new(Cell::new(None)),
    });

    // Tab once: the first "README.md". Enter selects it.
    cx.simulate_keystrokes("tab");
    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(0),
        "the first duplicate tab was not focused"
    );

    // Tab again: the second "README.md" must be a separate focus stop.
    cx.simulate_keystrokes("tab");
    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(1),
        "both duplicate tabs share one focus handle"
    );
}

struct SegmentedRemoveRoot {
    labels: Vec<&'static str>,
    selected: Rc<Cell<Option<usize>>>,
}

impl Render for SegmentedRemoveRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let segmented = Segmented::new("seg-remove")
            .items(self.labels.iter().copied().map(SegmentItem::new))
            .selected(0)
            .on_select(move |index, _, _| selected.set(Some(index)));
        let content = div()
            .size_full()
            .relative()
            .child(div().absolute().left(px(40.)).top(px(40.)).child(segmented));
        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn segmented_focus_survives_removing_an_earlier_item(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| SegmentedRemoveRoot {
        labels: vec!["One", "Two", "Three"],
        selected: Rc::new(Cell::new(None)),
    });

    // Focus the second segment and select it.
    cx.simulate_keystrokes("tab tab");
    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(1),
        "enter did not select the focused segment"
    );

    // Removing the first item shifts "Two" to index 0; focus must stay on it.
    cx.update(|_, app| {
        view.update(app, |root, cx| {
            root.labels.remove(0);
            cx.notify();
        })
    });
    cx.run_until_parked();

    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).selected.get()),
        Some(0),
        "focus moved to a different segment when an earlier item was removed"
    );
}

#[gpui::test]
fn switch_first_paint_uses_the_final_knob_position(cx: &mut TestAppContext) {
    use ui::Switch;

    struct Root;
    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(40.))
                        .top(px(40.))
                        .child(Switch::new("on").checked(true)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(40.))
                        .top(px(80.))
                        .child(Switch::new("off").checked(false)),
                )
        }
    }

    init(cx);
    let (_, cx) = cx.add_window_view(|_, _| Root);
    cx.run_until_parked();

    let on = cx.debug_bounds("on-knob").expect("no checked knob");
    let off = cx.debug_bounds("off-knob").expect("no unchecked knob");
    let offset: f32 = (on.origin.x - off.origin.x).into();
    assert_eq!(
        offset, 12.,
        "first paint must show the final knob positions"
    );
}

#[gpui::test]
fn indeterminate_progress_starts_inside_the_track(cx: &mut TestAppContext) {
    use ui::Progress;

    struct Root;
    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().relative().child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .child(Progress::new("indet", 0.).indeterminate()),
            )
        }
    }

    init(cx);
    let (_, cx) = cx.add_window_view(|_, _| Root);
    cx.run_until_parked();

    let track = cx.debug_bounds("indet-track").expect("no track");
    let fill = cx.debug_bounds("indet-fill").expect("no fill");
    let track_left: f32 = track.origin.x.into();
    let track_right: f32 = (track.origin.x + track.size.width).into();
    let fill_left: f32 = fill.origin.x.into();
    let fill_right: f32 = (fill.origin.x + fill.size.width).into();
    assert!(fill_left >= track_left, "segment starts off the track");
    assert!(fill_right <= track_right, "segment starts past the track");
}

#[gpui::test]
fn a_search_field_fills_the_sidebar(cx: &mut TestAppContext) {
    struct Root;
    impl Render for Root {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let input = window.use_keyed_state("sidebar-search-input", cx, |window, cx| {
                TextInput::new(window, cx)
            });
            // The repository sidebar: a 240px flex column with the filter
            // area inside 10px padding.
            div().size_full().flex().flex_col().child(
                div()
                    .w(px(240.))
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .child(
                        div()
                            .w_full()
                            .px(px(10.))
                            .pt(px(12.))
                            .pb(px(8.))
                            .child(SearchField::new("sidebar-search", input)),
                    )
                    .child(div().w_full().flex_1().min_h_0()),
            )
        }
    }

    init(cx);
    let (_, cx) = cx.add_window_view(|_, _| Root);
    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("sidebar-search")
        .expect("the search field was not drawn");
    // 240 - 1px border - 2 * 10px padding: the field spans the filter area
    // instead of collapsing to its icon.
    assert_eq!(bounds.size.width, px(219.));
    assert_eq!(bounds.size.height, px(26.));
}
