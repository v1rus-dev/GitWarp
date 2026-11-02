//! GPUI interaction tests for the floating components.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, Context, Entity, FocusHandle, IntoElement, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window,
};

use ui::{
    actions, dialog, focus, menu, overlay, toast, Button, Dialog, DialogFocus, MenuEntry, MenuItem,
    Select, TextInput, TextInputEvent, ToastKind, Tree, TreeNodeData,
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
fn press(cx: &mut VisualTestContext, keystroke: &'static str) {
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

/// The root rows of the submenu test: "Open" swaps in a page whose "Back" swaps the root back.
fn submenu_root(root_hits: &Rc<Cell<u32>>, inner_hits: &Rc<Cell<u32>>) -> Vec<MenuEntry> {
    let root = root_hits.clone();
    let inner = inner_hits.clone();
    let page_root = root_hits.clone();
    let page_inner = inner_hits.clone();
    vec![
        MenuEntry::Item(MenuItem::new("Open").submenu(move || {
            let root = page_root.clone();
            let inner = page_inner.clone();
            vec![
                MenuEntry::Item(MenuItem::new("Back").submenu(move || submenu_root(&root, &inner))),
                MenuEntry::Separator,
                menu_item("Inner", true, &page_inner),
            ]
        })),
        menu_item("Root item", true, &root),
        menu_item("Unused", true, &inner),
    ]
}

#[gpui::test]
fn menu_submenu_replaces_the_rows_in_place_and_back_restores_them(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let root_hits = Rc::new(Cell::new(0));
    let inner_hits = Rc::new(Cell::new(0));
    let entries = submenu_root(&root_hits, &inner_hits);

    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();

    // "Open" keeps the menu open and shows the page: Back, then Inner.
    cx.simulate_keystrokes("down enter");
    assert_eq!(layer_count(cx), 1, "the submenu closed the menu");

    // Back returns to the root rows, where the second item is "Root item".
    cx.simulate_keystrokes("down enter");
    assert_eq!(layer_count(cx), 1, "Back closed the menu");
    cx.simulate_keystrokes("down down enter");
    assert_eq!(root_hits.get(), 1, "Back did not restore the root rows");
    assert_eq!(inner_hits.get(), 0);
    assert_eq!(layer_count(cx), 0);

    // The page's own item runs and closes the menu.
    let entries = submenu_root(&root_hits, &inner_hits);
    cx.update(|window, app| {
        menu::open_context_menu(point(px(40.), px(40.)), entries, window, app);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("down enter down down enter");
    assert_eq!(inner_hits.get(), 1, "the page item did not run");
    assert_eq!(root_hits.get(), 1);
    assert_eq!(layer_count(cx), 0);
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

    cx.update(|window, cx| window.focus(&outside, cx));
    let _layer = open_test_dialog(cx, &view, false, false, &hits);
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 0, "escape did not close the dialog");
    assert_focused(cx, &outside);

    cx.update(|window, cx| window.focus(&outside, cx));
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

    // The menu opens on the current value (item 0), so two Downs reach item 2.
    cx.simulate_keystrokes("down down enter");
    assert_eq!(
        chosen.get(),
        Some(2),
        "choosing an option did not report it"
    );
    assert_eq!(layer_count(cx), 0, "the select menu stayed open");
}

#[gpui::test]
fn enter_that_closes_a_select_menu_does_not_reopen_it_on_key_up(cx: &mut TestAppContext) {
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
    assert_eq!(layer_count(cx), 1, "enter did not open the select");

    cx.simulate_keystrokes("down");
    press(cx, "enter");

    assert_eq!(chosen.get(), Some(1), "the option was not chosen");
    assert_eq!(
        layer_count(cx),
        0,
        "the key-up after closing activated the Select again"
    );
}

struct TreeMenuRoot {
    focus: FocusHandle,
    nodes: Vec<TreeNodeData>,
    first: Rc<Cell<u32>>,
    second: Rc<Cell<u32>>,
}

impl Render for TreeMenuRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let first = self.first.clone();
        let second = self.second.clone();
        let tree = Tree::new("menu-tree", self.nodes.clone())
            .focus_handle(self.focus.clone())
            .on_context_menu(move |_, position, window, cx| {
                menu::open_context_menu(
                    position,
                    vec![
                        menu_item("First", true, &first),
                        menu_item("Second", true, &second),
                    ],
                    window,
                    cx,
                );
            });

        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(240.))
                .h(px(320.))
                .child(tree),
        );
        let content = focus::track_pointer(actions::tab_navigation(content));
        content.child(overlay::render_overlays(window, cx))
    }
}

#[gpui::test]
fn a_tree_context_menu_takes_focus_from_the_tree(cx: &mut TestAppContext) {
    init(cx);
    let first = Rc::new(Cell::new(0));
    let second = Rc::new(Cell::new(0));
    let (_, cx) = cx.add_window_view(|_, cx| TreeMenuRoot {
        focus: cx.focus_handle().tab_stop(true),
        nodes: vec![
            TreeNodeData::new("one"),
            TreeNodeData::new("two"),
            TreeNodeData::new("three"),
        ],
        first: first.clone(),
        second: second.clone(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    let row = cx.debug_bounds("menu-tree-node-1").expect("no tree row");
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "the context menu did not open");

    // Escape must reach the menu, not the tree.
    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 0, "escape did not close the context menu");

    // ↓ must highlight a menu item, not move the tree marker; Enter runs it.
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    cx.simulate_keystrokes("down");
    press(cx, "enter");

    assert_eq!(first.get(), 1, "enter did not activate the menu item");
    assert_eq!(layer_count(cx), 0, "the context menu stayed open");
}

struct DialogOpenerRoot {
    opener: FocusHandle,
    input: Entity<TextInput>,
    layer: Rc<Cell<Option<overlay::LayerId>>>,
    opens: Rc<Cell<u32>>,
}

impl Render for DialogOpenerRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layer = self.layer.clone();
        let input = self.input.clone();
        let input_focus = input.read(cx).focus_handle().clone();
        let opens = self.opens.clone();
        let opener = Button::new("dialog-opener")
            .label("Open")
            .focus_handle(self.opener.clone())
            .on_click(move |_, window, cx| {
                opens.set(opens.get() + 1);
                let layer_in_dialog = layer.clone();
                let body = input.clone();
                let autofocus = input_focus.clone();
                let id = dialog::open(window, cx, move |_, _| {
                    Dialog::new("New branch")
                        .body(body.clone())
                        .focus_order([DialogFocus::input(autofocus.clone())])
                        .autofocus(autofocus.clone())
                        .on_default({
                            let layer_in_dialog = layer_in_dialog.clone();
                            move |window, cx| {
                                if let Some(layer) = layer_in_dialog.get() {
                                    dialog::close(layer, window, cx);
                                }
                            }
                        })
                });
                layer.set(Some(id));
            });

        let content = div()
            .size_full()
            .relative()
            .child(div().absolute().left(px(40.)).top(px(40.)).child(opener));
        let content = focus::track_pointer(actions::tab_navigation(content));
        content.child(overlay::render_overlays(window, cx))
    }
}

#[gpui::test]
fn enter_that_closes_a_dialog_does_not_reopen_it_on_key_up(cx: &mut TestAppContext) {
    init(cx);
    let layer = Rc::new(Cell::new(None));
    let opens = Rc::new(Cell::new(0));
    let (_, cx) = cx.add_window_view(|window, cx| DialogOpenerRoot {
        opener: cx.focus_handle().tab_stop(true),
        input: cx.new(|cx| TextInput::new(window, cx)),
        layer: layer.clone(),
        opens: opens.clone(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    cx.simulate_keystrokes("tab");
    press(cx, "enter");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "the dialog did not open");
    assert_eq!(opens.get(), 1);

    press(cx, "enter");

    assert_eq!(layer_count(cx), 0, "enter did not close the dialog");
    assert_eq!(
        opens.get(),
        1,
        "the key-up after closing activated the opener again"
    );
}

fn open_dynamic_busy_dialog(
    cx: &mut VisualTestContext,
    root: &Entity<TestRoot>,
    busy: Rc<Cell<bool>>,
) -> overlay::LayerId {
    let a = cx.read(|app| root.read(app).a.clone());
    cx.update(|window, app| {
        dialog::open(window, app, move |_, _| {
            Dialog::new("Busy dialog")
                .body(focus_box("busy-body", &a))
                .focus_order([DialogFocus::control(a.clone())])
                .autofocus(a.clone())
                .busy(busy.get())
        })
    })
}

#[gpui::test]
fn a_dialog_rebuilt_as_busy_ignores_escape_and_the_scrim(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let busy = Rc::new(Cell::new(false));
    open_dynamic_busy_dialog(cx, &view, busy.clone());
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    busy.set(true);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();

    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 1, "escape closed a busy dialog");
    cx.simulate_click(point(px(5.), px(5.)), Modifiers::default());
    assert_eq!(layer_count(cx), 1, "the scrim closed a busy dialog");

    busy.set(false);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    assert_eq!(
        layer_count(cx),
        0,
        "escape did not close after clearing busy"
    );
}

#[gpui::test]
fn enter_without_on_default_does_nothing_and_with_it_runs_once(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));
    let input = cx.update(|window, app| app.new(|cx| TextInput::new(window, cx)));
    let input_focus = cx.read(|app| input.read(app).focus_handle().clone());
    let default_focus = cx.read(|app| view.read(app).b.clone());

    // `default_button` alone (no `on_default`): Enter does nothing.
    let layer = cx.update(|window, app| {
        let body = input.clone();
        let autofocus = input_focus.clone();
        let default = default_focus.clone();
        let hits = hits.clone();
        dialog::open(window, app, move |_, _| {
            Dialog::new("Rename branch")
                .body(body.clone())
                .footer(
                    Button::new("dialog-default")
                        .label("OK")
                        .focus_handle(default.clone())
                        .on_click({
                            let hits = hits.clone();
                            move |_, _, _| hits.set(hits.get() + 1)
                        }),
                )
                .focus_order([DialogFocus::input(autofocus.clone())])
                .autofocus(autofocus.clone())
                .default_button(default.clone())
        })
    });
    cx.run_until_parked();

    press(cx, "enter");
    assert_eq!(
        hits.get(),
        0,
        "enter activated default_button without on_default"
    );
    assert_eq!(layer_count(cx), 1, "enter closed the dialog on its own");

    cx.update(|window, app| dialog::close(layer, window, app));
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 0);

    // With `on_default`, the same Enter runs it exactly once.
    cx.update(|window, app| {
        let body = input.clone();
        let autofocus = input_focus.clone();
        let hits = hits.clone();
        dialog::open(window, app, move |_, _| {
            Dialog::new("Rename branch")
                .body(body.clone())
                .focus_order([DialogFocus::input(autofocus.clone())])
                .autofocus(autofocus.clone())
                .on_default({
                    let hits = hits.clone();
                    move |_, _| hits.set(hits.get() + 1)
                })
        });
    });
    cx.run_until_parked();

    press(cx, "enter");
    assert_eq!(hits.get(), 1, "enter did not run on_default exactly once");
}

#[gpui::test]
fn enter_in_a_dialog_input_fires_the_default_once_and_swallows_submit(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));
    let submits = Rc::new(Cell::new(0));
    let input = cx.update(|window, app| app.new(|cx| TextInput::new(window, cx)));
    let focus = cx.read(|app| input.read(app).focus_handle().clone());
    let _subscription = {
        let submits = submits.clone();
        cx.update(|_, app| {
            app.subscribe(&input, move |_, event: &TextInputEvent, _| {
                if matches!(event, TextInputEvent::Submit) {
                    submits.set(submits.get() + 1);
                }
            })
        })
    };

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

    press(cx, "enter");
    assert_eq!(hits.get(), 1, "the default action did not run exactly once");
    assert_eq!(
        submits.get(),
        0,
        "the dialog must consume Enter instead of emitting Submit"
    );
}

#[gpui::test]
fn tab_inside_a_menu_does_not_move_focus_behind_it(cx: &mut TestAppContext) {
    let (_, cx) = start(cx);
    let hits = Rc::new(Cell::new(0));

    cx.update(|window, app| {
        menu::open_context_menu(
            point(px(40.), px(40.)),
            vec![menu_item("Item", true, &hits)],
            window,
            app,
        );
    });
    cx.run_until_parked();
    let before = focused(cx);

    cx.simulate_keystrokes("tab");

    assert_eq!(focused(cx), before, "tab moved focus behind the menu");
    assert_eq!(layer_count(cx), 1, "tab closed the menu");
}

#[gpui::test]
fn escape_closes_a_dialog_whose_only_default_is_disabled(cx: &mut TestAppContext) {
    let (view, cx) = start(cx);
    let outside = cx.read(|app| view.read(app).outside.clone());
    cx.update(|window, cx| window.focus(&outside, cx));

    // Like the Remove-worktree dialog for a dirty worktree: the initial focus
    // target is a disabled button, which never tracks its focus handle.
    let primary = cx.update(|_, app| app.focus_handle());
    let cancel = cx.update(|_, app| app.focus_handle());
    let layer = {
        let (primary, cancel) = (primary.clone(), cancel.clone());
        cx.update(|window, app| {
            dialog::open(window, app, move |_, _| {
                Dialog::new("Remove")
                    .footer(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("cancel")
                                    .label("Cancel")
                                    .focus_handle(cancel.clone()),
                            )
                            .child(
                                Button::danger("primary")
                                    .label("Remove")
                                    .disabled(true)
                                    .focus_handle(primary.clone()),
                            ),
                    )
                    .focus_order([
                        DialogFocus::control(cancel.clone()),
                        DialogFocus::control(primary.clone()),
                    ])
                    .default_button(primary.clone())
                    .default_danger(true)
            })
        })
    };
    let _ = layer;
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1);

    // Tab enters the dialog's own focus order, not the page behind it.
    cx.simulate_keystrokes("tab");
    assert_focused(cx, &cancel);

    // Tabbing onto the disabled button loses focus; the dialog takes it back.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    assert_eq!(layer_count(cx), 0, "escape did not close the dialog");
    assert_focused(cx, &outside);
}

#[gpui::test]
fn select_opens_with_space_and_down_and_picks_with_the_keyboard(cx: &mut TestAppContext) {
    init(cx);
    let chosen = Rc::new(Cell::new(None));
    let (_, cx) = cx.add_window_view(|_, _| SelectRoot {
        chosen: chosen.clone(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    cx.simulate_keystrokes("tab");
    press(cx, "space");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "space did not open the select menu");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 0, "escape did not close the select menu");

    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "down did not open the select menu");
    cx.simulate_keystrokes("down down enter");
    assert_eq!(chosen.get(), Some(2));
    assert_eq!(layer_count(cx), 0);
}

#[gpui::test]
fn a_select_menu_opens_on_the_current_value_and_returns_focus_after_a_pick(
    cx: &mut TestAppContext,
) {
    init(cx);
    let chosen = Rc::new(Cell::new(None));
    let (_, cx) = cx.add_window_view(|_, _| SelectRoot {
        chosen: chosen.clone(),
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    // Opened with Down, the current item (0) is highlighted: Enter picks it
    // without moving, and one Up would wrap from it to the last item.
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "down did not open the select menu");
    cx.simulate_keystrokes("up");
    press(cx, "enter");
    assert_eq!(
        chosen.get(),
        Some(3),
        "Up did not move from the current item"
    );
    assert_eq!(layer_count(cx), 0);

    // Focus is back on the select: Enter opens the menu again.
    cx.run_until_parked();
    press(cx, "enter");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 1, "focus did not return to the select");
}

/// Opens a dialog holding a button, a checkbox and a select (focus handles
/// `a`, `b`, `c` of the root), the way the app's forms use them.
fn open_controls_dialog(
    cx: &mut VisualTestContext,
    root: &Entity<TestRoot>,
    clicks: &Rc<Cell<u32>>,
    toggles: &Rc<Cell<u32>>,
    chosen: &Rc<Cell<Option<usize>>>,
) -> (FocusHandle, FocusHandle, FocusHandle) {
    let (a, b, c) = cx.read(|app| {
        let root = root.read(app);
        (root.a.clone(), root.b.clone(), root.c.clone())
    });
    let (clicks, toggles, chosen) = (clicks.clone(), toggles.clone(), chosen.clone());
    let handles = (a.clone(), b.clone(), c.clone());
    cx.update(|window, app| {
        dialog::open(window, app, move |_, _| {
            let (clicks, toggles, chosen) = (clicks.clone(), toggles.clone(), chosen.clone());
            Dialog::new("Controls")
                .body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            Button::new("activate-button")
                                .label("Go")
                                .focus_handle(a.clone())
                                .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                        )
                        .child(
                            ui::Checkbox::new("activate-checkbox")
                                .label("Tick")
                                .focus_handle(b.clone())
                                .on_toggle(move |_, _, _| toggles.set(toggles.get() + 1)),
                        )
                        .child(
                            Select::new("dialog-select")
                                .options(["main", "develop", "feat/lanes"])
                                .selected(0)
                                .focus_handle(c.clone())
                                .on_change(move |index, _, _| chosen.set(Some(index))),
                        ),
                )
                .focus_order([
                    DialogFocus::control(a.clone()),
                    DialogFocus::control(b.clone()),
                    DialogFocus::control(c.clone()),
                ])
                .autofocus(a.clone())
        })
    });
    cx.run_until_parked();
    handles
}

/// Enter on a focused button or checkbox inside a dialog activates it, like
/// the design's native controls (the dialog's `Confirm` must not swallow it).
#[gpui::test]
fn enter_activates_a_focused_button_and_toggles_a_focused_checkbox(cx: &mut TestAppContext) {
    let (root, cx) = start(cx);
    let (clicks, toggles) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let chosen = Rc::new(Cell::new(None));
    let (button, checkbox, _) = open_controls_dialog(cx, &root, &clicks, &toggles, &chosen);

    cx.update(|window, app| window.focus(&button, app));
    press(cx, "enter");
    assert_eq!(clicks.get(), 1, "Enter did not activate the button");

    cx.update(|window, app| window.focus(&checkbox, app));
    press(cx, "enter");
    assert_eq!(toggles.get(), 1, "Enter did not toggle the checkbox");
    assert_eq!(layer_count(cx), 1, "the dialog closed");
}

/// In a dialog, a select picked with Enter hands focus back to itself.
#[gpui::test]
fn a_select_in_a_dialog_gets_focus_back_after_a_pick(cx: &mut TestAppContext) {
    let (root, cx) = start(cx);
    let (clicks, toggles) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let chosen = Rc::new(Cell::new(None));
    let (_, _, select) = open_controls_dialog(cx, &root, &clicks, &toggles, &chosen);

    cx.update(|window, app| window.focus(&select, app));
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(layer_count(cx), 2, "down did not open the select menu");
    cx.simulate_keystrokes("down");
    press(cx, "enter");
    cx.run_until_parked();
    assert_eq!(chosen.get(), Some(1));
    assert_eq!(layer_count(cx), 1, "only the dialog stays open");
    assert_focused(cx, &select);
}
