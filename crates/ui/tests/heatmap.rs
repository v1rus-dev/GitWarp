//! GPUI interaction tests for the commit-activity heatmap.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    div, point, prelude::*, px, Context, Entity, IntoElement, Modifiers, Render, TestAppContext,
    VisualTestContext, Window,
};

use ui::{actions, focus, Heatmap};

/// 2026-10-14, a Wednesday: row 2 of the newest week, so four days of that
/// week lie in the future.
const END: i32 = 20_740;
/// The 53-column grid starts on the Monday 52 weeks before the newest week's Monday.
const START: i32 = END - 2 - 52 * 7;

struct Root {
    selected: Rc<RefCell<Vec<i32>>>,
    cleared: Rc<Cell<u32>>,
    loading: bool,
    pending_from: Option<i32>,
}

impl Root {
    fn new() -> Self {
        Self {
            selected: Rc::default(),
            cleared: Rc::default(),
            loading: false,
            pending_from: None,
        }
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let cleared = self.cleared.clone();
        let content = div().size_full().relative().child(
            div().absolute().left(px(40.)).top(px(40.)).child(
                Heatmap::new("hm", END, 53, |day| (day.rem_euclid(5) as u32, 1))
                    .loading(self.loading)
                    .pending_from(self.pending_from)
                    .on_select(move |day, _, _| selected.borrow_mut().push(day))
                    .on_clear(move |_, _| cleared.set(cleared.get() + 1)),
            ),
        );
        focus::track_pointer(actions::tab_navigation(content))
    }
}

fn start(
    cx: &mut TestAppContext,
    build: impl FnOnce() -> Root,
) -> (Entity<Root>, &mut VisualTestContext) {
    cx.update(ui::init).expect("ui::init");
    let (view, cx) = cx.add_window_view(|_, _| build());
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

fn picked(cx: &VisualTestContext, view: &Entity<Root>) -> Vec<i32> {
    cx.read(|app| view.read(app).selected.borrow().clone())
}

/// Presses `keys`, then Enter, and returns the newest selection.
fn pick_after(cx: &mut VisualTestContext, view: &Entity<Root>, keys: &str) -> Option<i32> {
    if !keys.is_empty() {
        cx.simulate_keystrokes(keys);
    }
    cx.simulate_keystrokes("enter");
    picked(cx, view).last().copied()
}

#[gpui::test]
fn arrow_keys_move_the_focused_day(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, Root::new);
    cx.simulate_keystrokes("tab");

    // The cursor starts on the newest day.
    assert_eq!(pick_after(cx, &view, ""), Some(END));
    // ↑ is the previous day, ↓ the next.
    assert_eq!(pick_after(cx, &view, "up"), Some(END - 1));
    assert_eq!(pick_after(cx, &view, "up"), Some(END - 2));
    assert_eq!(pick_after(cx, &view, "down"), Some(END - 1));
    // ← is the previous week, → the next.
    assert_eq!(pick_after(cx, &view, "left"), Some(END - 8));
    assert_eq!(pick_after(cx, &view, "right"), Some(END - 1));
    // Home and End jump to the first and last day.
    assert_eq!(pick_after(cx, &view, "home"), Some(START));
    assert_eq!(pick_after(cx, &view, "left up"), Some(START));
    assert_eq!(pick_after(cx, &view, "end"), Some(END));
    // Space selects like Enter.
    cx.simulate_keystrokes("up space");
    assert_eq!(picked(cx, &view).last().copied(), Some(END - 1));
}

#[gpui::test]
fn future_days_are_not_selectable(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, Root::new);
    cx.simulate_keystrokes("tab");

    // Stepping past the newest day, by day or by week, stays on it.
    assert_eq!(pick_after(cx, &view, "down down down down down"), Some(END));
    assert_eq!(pick_after(cx, &view, "right"), Some(END));
    assert_eq!(pick_after(cx, &view, "end down"), Some(END));
    assert!(picked(cx, &view).iter().all(|&day| day <= END));
}

#[gpui::test]
fn escape_clears(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, Root::new);
    cx.simulate_keystrokes("tab escape");
    assert_eq!(cx.read(|app| view.read(app).cleared.get()), 1);
}

#[gpui::test]
fn counting_and_loading_days_are_not_selectable(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, || Root {
        pending_from: Some(END - 5),
        ..Root::new()
    });
    cx.simulate_keystrokes("tab");
    assert_eq!(pick_after(cx, &view, ""), Some(END));
    // Older than the pending boundary: Enter does nothing.
    let before = picked(cx, &view).len();
    cx.simulate_keystrokes("left enter");
    assert_eq!(picked(cx, &view).len(), before);

    let (view, cx) = start(cx, || Root {
        loading: true,
        ..Root::new()
    });
    cx.simulate_keystrokes("tab enter");
    assert!(picked(cx, &view).is_empty());
}

#[gpui::test]
fn clicking_a_day_selects_it_and_hover_shows_the_tooltip(cx: &mut TestAppContext) {
    let (view, cx) = start(cx, Root::new);
    // Grid origin: 40 + 28 (weekday column) by 40 + 14 + 4 (month row + gap);
    // the newest day is column 52, row 2 of 12px pitch cells.
    let newest = point(
        px(40. + 28. + 52. * 12. + 5.),
        px(40. + 18. + 2. * 12. + 5.),
    );
    cx.simulate_click(newest, Modifiers::default());
    assert_eq!(picked(cx, &view), vec![END]);
    cx.simulate_mouse_move(newest - point(px(12.), px(0.)), None, Modifiers::default());
    cx.simulate_mouse_move(newest, None, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("hm-tip").is_some(), "no tooltip on hover");
}
