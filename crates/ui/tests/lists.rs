//! Interaction tests for the tree, list rows and splitter.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, Context, Entity, FocusHandle, IntoElement, KeyUpEvent,
    Keystroke, Modifiers, MouseButton, Render, ScrollHandle, TestAppContext,
    UniformListScrollHandle, VisualTestContext, Window,
};

use ui::{actions, focus, ListRow, ScrollArea, Scrollbar, Splitter, Tree, TreeNodeData};

fn init(cx: &mut TestAppContext) {
    cx.update(ui::init).expect("ui::init");
}

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

struct TreeRoot {
    focus: FocusHandle,
    nodes: Vec<TreeNodeData>,
    activated: Rc<Cell<Option<usize>>>,
    expanded: Rc<Cell<Option<usize>>>,
    collapsed: Rc<Cell<Option<usize>>>,
    context: Rc<Cell<Option<usize>>>,
}

impl TreeRoot {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            nodes: vec![
                TreeNodeData::new("one"),
                TreeNodeData::new("folder")
                    .has_children(true)
                    .expanded(false),
                TreeNodeData::new("open-folder")
                    .has_children(true)
                    .expanded(true),
                TreeNodeData::new("two"),
            ],
            activated: Rc::new(Cell::new(None)),
            expanded: Rc::new(Cell::new(None)),
            collapsed: Rc::new(Cell::new(None)),
            context: Rc::new(Cell::new(None)),
        }
    }
}

impl Render for TreeRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let activated = self.activated.clone();
        let expanded = self.expanded.clone();
        let collapsed = self.collapsed.clone();
        let context = self.context.clone();
        let tree = Tree::new("tree", self.nodes.clone())
            .focus_handle(self.focus.clone())
            .on_activate(move |index, _, _| activated.set(Some(index)))
            .on_expand(move |index, _, _| expanded.set(Some(index)))
            .on_collapse(move |index, _, _| collapsed.set(Some(index)))
            .on_context_menu(move |index, _, _, _| context.set(Some(index)));

        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(240.))
                .h(px(320.))
                .child(tree),
        );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn tree_down_down_enter_activates_the_third_visible_node(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, TreeRoot::new);
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("down down");
    press(cx, "enter");

    assert_eq!(
        cx.read(|app| view.read(app).activated.get()),
        Some(2),
        "enter must activate the node under the keyboard marker"
    );
}

#[gpui::test]
fn tree_right_expands_and_left_collapses_via_callbacks(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, TreeRoot::new);
    cx.simulate_keystrokes("tab");

    cx.simulate_keystrokes("down");
    press(cx, "right");
    assert_eq!(
        cx.read(|app| view.read(app).expanded.get()),
        Some(1),
        "right must expand the collapsed folder"
    );

    cx.simulate_keystrokes("down");
    press(cx, "left");
    assert_eq!(
        cx.read(|app| view.read(app).collapsed.get()),
        Some(2),
        "left must collapse the expanded folder"
    );
}

#[gpui::test]
fn tree_right_click_reports_the_row_index(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, TreeRoot::new);
    let bounds = cx
        .debug_bounds("tree-node-2")
        .expect("third tree row was not drawn");

    cx.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
    assert_eq!(
        cx.read(|app| view.read(app).context.get()),
        Some(2),
        "right-click must report the row under the pointer"
    );
}

struct SplitterRoot {
    resized: Rc<Cell<Option<f32>>>,
}

impl Render for SplitterRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let resized = self.resized.clone();
        let splitter = Splitter::vertical("splitter")
            .size(px(240.))
            .bounds(px(160.), px(420.))
            .on_resize(move |size, _, _| resized.set(Some(size.into())));

        div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .h(px(200.))
                .child(splitter),
        )
    }
}

#[gpui::test]
fn splitter_drag_reports_a_clamped_size(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| SplitterRoot {
        resized: Rc::new(Cell::new(None)),
    });
    let bounds = cx
        .debug_bounds("splitter-handle")
        .expect("splitter handle was not drawn");

    let start = bounds.center();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        start + gpui::point(px(400.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        start + gpui::point(px(420.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        start + gpui::point(px(420.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(
        cx.read(|app| view.read(app).resized.get()),
        Some(420.),
        "the drag must report the size clamped to max"
    );
}

struct TwoSplitters {
    first_resizes: Rc<Cell<u32>>,
    second_size: Rc<Cell<Option<f32>>>,
}

impl Render for TwoSplitters {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let first_resizes = self.first_resizes.clone();
        let second_size = self.second_size.clone();
        let first = Splitter::vertical("splitter-a")
            .size(px(240.))
            .bounds(px(160.), px(420.))
            .on_resize(move |_, _, _| first_resizes.set(first_resizes.get() + 1));
        let second = Splitter::vertical("splitter-b")
            .size(px(240.))
            .bounds(px(160.), px(420.))
            .on_resize(move |size, _, _| second_size.set(Some(size.into())));

        div()
            .size_full()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .h(px(200.))
                    .child(first),
            )
            .child(
                div()
                    .absolute()
                    .left(px(240.))
                    .top(px(40.))
                    .h(px(200.))
                    .child(second),
            )
    }
}

#[gpui::test]
fn dragging_one_splitter_never_resizes_another(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| TwoSplitters {
        first_resizes: Rc::new(Cell::new(0)),
        second_size: Rc::new(Cell::new(None)),
    });

    let first = cx
        .debug_bounds("splitter-a-handle")
        .expect("no first handle");
    let second = cx
        .debug_bounds("splitter-b-handle")
        .expect("no second handle");

    // Press and release the first splitter without dragging it.
    cx.simulate_mouse_down(first.center(), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(first.center(), MouseButton::Left, Modifiers::default());

    // Now drag the second one: only its own on_resize may run.
    cx.simulate_mouse_down(second.center(), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        second.center() + gpui::point(px(40.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        second.center() + gpui::point(px(60.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        second.center() + gpui::point(px(60.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(
        cx.read(|app| view.read(app).first_resizes.get()),
        0,
        "the untouched splitter resized during another splitter's drag"
    );
    assert_eq!(
        cx.read(|app| view.read(app).second_size.get()),
        Some(300.),
        "the dragged splitter did not resize"
    );
}

struct ScrollTreeRoot {
    focus: FocusHandle,
    scroll: ScrollHandle,
    nodes: Vec<TreeNodeData>,
}

impl ScrollTreeRoot {
    fn new(cx: &mut Context<Self>, count: usize) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            nodes: (0..count)
                .map(|index| TreeNodeData::new(format!("node-{index}")))
                .collect(),
        }
    }
}

impl Render for ScrollTreeRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let tree = Tree::new("scroll-tree", self.nodes.clone())
            .focus_handle(self.focus.clone())
            .scroll_handle(self.scroll.clone());

        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(240.))
                .h(px(200.))
                .child(tree),
        );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn moving_the_marker_past_the_bottom_scrolls_it_into_view(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |cx| ScrollTreeRoot::new(cx, 40));
    let scroll = cx.read(|app| view.read(app).scroll.clone());

    cx.simulate_keystrokes("tab");
    for _ in 0..35 {
        cx.simulate_keystrokes("down");
    }
    cx.run_until_parked();

    let offset: f32 = scroll.offset().y.into();
    assert!(offset < 0., "the tree did not scroll (offset {offset})");

    let row = cx
        .debug_bounds("scroll-tree-node-35")
        .expect("the marker row was not drawn");
    let top: f32 = row.origin.y.into();
    let bottom: f32 = (row.origin.y + row.size.height).into();
    assert!(top >= 40., "the marker row is above the viewport: {top}");
    assert!(
        bottom <= 240.,
        "the marker row is below the viewport: {bottom}"
    );
}

struct SelectedTreeRoot {
    focus: FocusHandle,
    nodes: Vec<TreeNodeData>,
    activated: Rc<Cell<Option<usize>>>,
}

impl Render for SelectedTreeRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let activated = self.activated.clone();
        let tree = Tree::new("selected-tree", self.nodes.clone())
            .focus_handle(self.focus.clone())
            .on_activate(move |index, _, _| activated.set(Some(index)));

        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(240.))
                .h(px(320.))
                .child(tree),
        );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn the_marker_starts_on_the_selected_node(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |cx| SelectedTreeRoot {
        focus: cx.focus_handle().tab_stop(true),
        nodes: vec![
            TreeNodeData::new("one"),
            TreeNodeData::new("two"),
            TreeNodeData::new("three").selected(true),
            TreeNodeData::new("four"),
        ],
        activated: Rc::new(Cell::new(None)),
    });

    cx.simulate_keystrokes("tab");
    press(cx, "enter");

    assert_eq!(
        cx.read(|app| view.read(app).activated.get()),
        Some(2),
        "the marker must start on the selected row, not the first row"
    );
}

struct KeyTreeRoot {
    focus: FocusHandle,
    nodes: Vec<TreeNodeData>,
    activated: Rc<Cell<Option<usize>>>,
}

impl Render for KeyTreeRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let activated = self.activated.clone();
        let tree = Tree::new("key-tree", self.nodes.clone())
            .focus_handle(self.focus.clone())
            .on_activate(move |index, _, _| activated.set(Some(index)));

        let content = div().size_full().relative().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(40.))
                .w(px(240.))
                .h(px(320.))
                .child(tree),
        );

        focus::track_pointer(actions::tab_navigation(content))
    }
}

#[gpui::test]
fn the_marker_follows_the_node_key_when_rows_above_it_disappear(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |cx| KeyTreeRoot {
        focus: cx.focus_handle().tab_stop(true),
        nodes: (0..8)
            .map(|index| TreeNodeData::new(format!("node-{index}")).key(format!("key-{index}")))
            .collect(),
        activated: Rc::new(Cell::new(None)),
    });

    cx.simulate_keystrokes("tab");
    for _ in 0..5 {
        cx.simulate_keystrokes("down");
    }

    // Collapse/remove the two rows above the marker: node-5 moves from
    // index 5 to index 3, but keeps its key.
    cx.update(|_, app| {
        view.update(app, |root, cx| {
            root.nodes.drain(0..2);
            cx.notify();
        })
    });
    cx.run_until_parked();

    press(cx, "enter");
    assert_eq!(
        cx.read(|app| view.read(app).activated.get()),
        Some(3),
        "enter activated a different node than the keyboard marker"
    );
}

struct ListRowRoot {
    scroll: ScrollHandle,
    uniform_scroll: UniformListScrollHandle,
}

impl Render for ListRowRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let flex_rows = div().flex().flex_col().child(
            ListRow::new("flex-row")
                .selected(true)
                .list_focused(true)
                .child("Selected row"),
        );

        let uniform = uniform_list("list-uniform", 5, |range, _, _| {
            range
                .map(|index| {
                    ListRow::new(gpui::ElementId::named_usize("uniform-row", index))
                        .selected(index == 0)
                        .list_focused(true)
                        .child("Selected row")
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(self.uniform_scroll.clone())
        .flex_1()
        .h_full();

        div()
            .size_full()
            .relative()
            // A plain flex column.
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .w(px(300.))
                    .h(px(200.))
                    .child(ScrollArea::new("list-scroll", self.scroll.clone()).child(flex_rows)),
            )
            // A uniform_list with its own scrollbar, like the gallery.
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(280.))
                    .w(px(300.))
                    .h(px(200.))
                    .flex()
                    .child(uniform)
                    .child(Scrollbar::uniform(
                        "list-uniform-bar",
                        self.uniform_scroll.clone(),
                    )),
            )
    }
}

#[gpui::test]
fn a_list_row_fills_the_list_width_without_overflowing(cx: &mut TestAppContext) {
    let (_, cx) = start(cx, |_| ListRowRoot {
        scroll: ScrollHandle::new(),
        uniform_scroll: UniformListScrollHandle::new(),
    });
    cx.run_until_parked();

    // 300px pane, a 10px scrollbar on the right, and 6px gutters each side.
    for selector in ["list-row-flex-row", "list-row-uniform-row-0"] {
        let row = cx.debug_bounds(selector).expect("no list row");
        let left: f32 = row.origin.x.into();
        let right: f32 = (row.origin.x + row.size.width).into();
        assert_eq!(left, 46., "{selector} does not start after its 6px gutter");
        assert_eq!(
            right, 324.,
            "{selector} must end 6px before the scrollbar track"
        );
    }
}
