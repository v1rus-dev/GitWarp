//! Interaction tests for the tree, list rows and splitter.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, IntoElement, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window,
};

use ui::{actions, focus, Splitter, Tree, TreeNodeData};

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
