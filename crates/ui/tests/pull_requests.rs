//! Pull request badges and CI glyphs: drawing, and where a click on a badge
//! goes (to the badge, not to the row that carries it).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, Render, TestAppContext, VisualTestContext, Window,
};

use ui::{CiGlyph, CiGlyphState, PrBadge, PrBadgeState, ReviewMark, Tree, TreeNodeData, TreePr};

fn start<V: Render + 'static>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Context<V>) -> V,
) -> (Entity<V>, &mut VisualTestContext) {
    cx.update(ui::init).expect("ui::init");
    let (view, cx) = cx.add_window_view(|_, cx| build(cx));
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

fn click_at(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_event(MouseDownEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
    });
}

struct Gallery {
    clicks: Rc<Cell<u32>>,
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let badges = [
            PrBadgeState::Open,
            PrBadgeState::Draft,
            PrBadgeState::Merged,
            PrBadgeState::Closed,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, state)| {
            let clicks = clicks.clone();
            PrBadge::new(format!("badge-{i}"), state, format!("#{i}"))
                .review(Some(ReviewMark::Approved))
                .tooltip("tip")
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
        });
        let glyphs = [
            CiGlyphState::Passed,
            CiGlyphState::Failed,
            CiGlyphState::Running,
            CiGlyphState::Pending,
            CiGlyphState::Cancelled,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, state)| CiGlyph::new(format!("glyph-{i}"), state));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(20.))
            .child(div().flex().gap(px(8.)).children(badges))
            .child(div().flex().gap(px(8.)).children(glyphs))
    }
}

#[gpui::test]
fn every_badge_state_and_glyph_state_is_drawn(cx: &mut TestAppContext) {
    let (_view, cx) = start(cx, |_| Gallery {
        clicks: Rc::new(Cell::new(0)),
    });
    for name in ["badge-0", "badge-1", "badge-2", "badge-3"] {
        let bounds = cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} was not drawn"));
        assert!(bounds.size.width > px(20.) && bounds.size.height == px(18.));
    }
    for name in ["glyph-0", "glyph-1", "glyph-2", "glyph-3", "glyph-4"] {
        assert!(cx.debug_bounds(name).is_some(), "{name} was not drawn");
    }
}

#[gpui::test]
fn a_badge_with_a_handler_is_a_button(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| Gallery {
        clicks: Rc::new(Cell::new(0)),
    });
    let bounds = cx.debug_bounds("badge-1").expect("badge was not drawn");
    click_at(cx, bounds.center());
    assert_eq!(cx.read(|app| view.read(app).clicks.get()), 1);
}

struct PrTree {
    activated: Rc<Cell<Option<usize>>>,
    badge_clicked: Rc<Cell<Option<usize>>>,
}

impl Render for PrTree {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let activated = self.activated.clone();
        let badge_clicked = self.badge_clicked.clone();
        let nodes = vec![
            TreeNodeData::new("plain"),
            TreeNodeData::new("with-pr").pr(TreePr {
                state: PrBadgeState::Open,
                text: "#142".into(),
                tooltip: "#142 · Open".into(),
            }),
        ];
        div().size_full().child(
            div().w(px(280.)).h(px(200.)).child(
                Tree::new("pr-tree", nodes)
                    .on_activate(move |index, _, _| activated.set(Some(index)))
                    .on_pr_click(move |index, _, _, _| badge_clicked.set(Some(index))),
            ),
        )
    }
}

#[gpui::test]
fn a_click_on_a_row_badge_opens_the_badge_and_leaves_the_row_alone(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, |_| PrTree {
        activated: Rc::new(Cell::new(None)),
        badge_clicked: Rc::new(Cell::new(None)),
    });
    let badge = cx
        .debug_bounds("pr-tree-pr-1")
        .expect("the row's badge was not drawn");
    click_at(cx, badge.center());
    let (activated, clicked) = cx.read(|app| {
        let root = view.read(app);
        (root.activated.get(), root.badge_clicked.get())
    });
    assert_eq!(clicked, Some(1), "the badge click must reach the handler");
    assert_eq!(activated, None, "the row must not also be activated");

    let row = cx
        .debug_bounds("pr-tree-node-1")
        .expect("row was not drawn");
    click_at(cx, point_left_of(row));
    assert_eq!(
        cx.read(|app| view.read(app).activated.get()),
        Some(1),
        "a click on the label still activates the row"
    );
}

fn point_left_of(bounds: gpui::Bounds<Pixels>) -> Point<Pixels> {
    gpui::point(bounds.origin.x + px(40.), bounds.center().y)
}
