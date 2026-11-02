//! The commit-activity heatmap: one square per day, 53 weeks by 7 weekdays.
//!
//! Days are `i32` counts of days since the Unix epoch (a local calendar date),
//! the same unit `git_core::activity` produces; the crate itself knows nothing
//! about git. Weeks start on Monday.

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, ease_in_out, px, Animation, AnimationExt, AnyElement, App, ElementId, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use crate::actions::{
    Cancel, Confirm, HeatmapActivate, HeatmapWeekNext, HeatmapWeekPrev, SelectFirst, SelectLast,
    SelectNext, SelectPrev,
};
use crate::components::skeleton::shimmer_opacity;
use crate::focus;
use crate::metrics::{FONT_UI, FS_SM, FS_XS, R_SM, SP_2};
use crate::theme::ActiveTheme;

/// Gap between cells.
const GAP: f32 = 2.;
/// Cell edge, regular and compact.
const CELL: f32 = 10.;
const CELL_COMPACT: f32 = 8.;
/// The weekday-label column.
const WEEKDAY_W: f32 = 28.;
/// The month-label row.
const MONTH_H: f32 = 14.;
/// Cell corner radius.
const RADIUS: f32 = 2.;
/// Width of the box the tooltip label is centred in (the label shrinks to its text).
const TIP_BOX_W: f32 = 280.;
/// The room a tooltip needs above a cell: its height plus the 6 px gap.
const TIP_FLIP_ROOM: f32 = 34.;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

type DayHandler = Rc<dyn Fn(i32, &mut Window, &mut App) + 'static>;
type ClearHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type DataFn = Rc<dyn Fn(i32) -> (u32, u8) + 'static>;
type TooltipFn = Rc<dyn Fn(i32, u32) -> SharedString + 'static>;

/// A GitHub-style contribution graph.
///
/// `data` maps a day to `(commit count, heat level 0..=4)`. The grid is one
/// focus stop: ↑/↓ step a day, ←/→ a week, Home/End jump to the first/last day,
/// Enter/Space select, Esc clears.
#[derive(IntoElement)]
pub struct Heatmap {
    id: ElementId,
    end_day: i32,
    weeks: usize,
    data: DataFn,
    compact: bool,
    selected: Option<i32>,
    loading: bool,
    pending_from: Option<i32>,
    on_select: Option<DayHandler>,
    on_clear: Option<ClearHandler>,
    tooltip: Option<TooltipFn>,
}

impl Heatmap {
    /// `end_day` is the last (newest) day, normally today; `weeks` columns end
    /// with the week that contains it.
    pub fn new(
        id: impl Into<ElementId>,
        end_day: i32,
        weeks: usize,
        data: impl Fn(i32) -> (u32, u8) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            end_day,
            weeks: weeks.max(1),
            data: Rc::new(data),
            compact: false,
            selected: None,
            loading: false,
            pending_from: None,
            on_select: None,
            on_clear: None,
            tooltip: None,
        }
    }

    /// 8px cells instead of 10px.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// The day drawn with a `text` outline.
    pub fn selected(mut self, day: Option<i32>) -> Self {
        self.selected = day;
        self
    }

    /// Every cell is a pulsing skeleton and nothing can be selected.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Cells older than this day are skeletons (counting is still running).
    pub fn pending_from(mut self, day: Option<i32>) -> Self {
        self.pending_from = day;
        self
    }

    /// A click, Enter or Space on a counted day.
    pub fn on_select(mut self, listener: impl Fn(i32, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(listener));
        self
    }

    /// Esc while the grid is focused.
    pub fn on_clear(mut self, listener: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_clear = Some(Rc::new(listener));
        self
    }

    /// The tooltip text for a day and its count; defaults to
    /// `"5 commits on Wed, 14 Oct 2026"`.
    pub fn tooltip(mut self, text: impl Fn(i32, u32) -> SharedString + 'static) -> Self {
        self.tooltip = Some(Rc::new(text));
        self
    }
}

/// The legend: "Less [0][1][2][3][4] More".
#[derive(IntoElement)]
pub struct HeatLegend {
    compact: bool,
}

impl HeatLegend {
    pub fn new() -> Self {
        Self { compact: false }
    }

    /// 8px swatches instead of 10px, to match a compact [`Heatmap`].
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }
}

impl Default for HeatLegend {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for HeatLegend {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let cell = px(if self.compact { CELL_COMPACT } else { CELL });
        let label = |text: &'static str| {
            div()
                .text_size(FS_XS)
                .text_color(colors.muted)
                .whitespace_nowrap()
                .child(text)
        };
        div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .font_family(FONT_UI)
            .child(label("Less").mr(SP_2))
            .children(
                colors
                    .heat
                    .map(|color| div().size(cell).flex_none().rounded(px(RADIUS)).bg(color)),
            )
            .child(label("More").ml(SP_2))
    }
}

/// The keyed state that outlives a render.
struct HeatmapState {
    focus: FocusHandle,
    /// The keyboard cursor; `None` until the first key press.
    cursor: Option<i32>,
    hover: Option<i32>,
}

/// The drawable day span and the selection rules, shared by render and keys.
#[derive(Clone, Copy)]
struct Span {
    start: i32,
    end: i32,
    loading: bool,
    pending_from: Option<i32>,
}

impl Span {
    fn bounds(end_day: i32, weeks: usize) -> (i32, i32) {
        let week_start = end_day - weekday_row(end_day);
        (week_start - (weeks as i32 - 1) * 7, end_day)
    }

    fn clamp(self, day: i32) -> i32 {
        day.clamp(self.start, self.end)
    }

    fn is_pending(self, day: i32) -> bool {
        self.loading || self.pending_from.is_some_and(|from| day < from)
    }

    /// Days after the end are not drawn; counted days are the only selectable ones.
    fn selectable(self, day: i32) -> bool {
        (self.start..=self.end).contains(&day) && !self.is_pending(day)
    }
}

/// The keyboard cursor after a move of `delta` days, kept inside the grid.
fn step(cursor: i32, delta: i32, start: i32, end: i32) -> i32 {
    (cursor + delta).clamp(start, end)
}

/// 0 for Monday … 6 for Sunday; day 0 (1970-01-01) was a Thursday.
fn weekday_row(day: i32) -> i32 {
    (day + 3).rem_euclid(7)
}

/// The Gregorian date of a day count (Hinnant's `civil_from_days`).
fn civil_from_days(day: i32) -> (i32, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i32::from(m <= 2);
    (y, m, d)
}

/// `Wed, 14 Oct 2026`.
fn long_date(day: i32) -> String {
    let (year, month, date) = civil_from_days(day);
    format!(
        "{}, {date} {} {year}",
        WEEKDAYS[weekday_row(day) as usize],
        MONTHS[month as usize - 1]
    )
}

/// `5 commits on Wed, 14 Oct 2026`, `1 commit on …`, `No commits on …`.
fn default_tooltip(day: i32, count: u32) -> SharedString {
    let date = long_date(day);
    match count {
        0 => format!("No commits on {date}"),
        1 => format!("1 commit on {date}"),
        n => format!("{n} commits on {date}"),
    }
    .into()
}

/// The month labels as `(column, name)`: the month name sits over the column
/// holding the 1st, and is dropped when less than 3 columns follow the
/// previous label. The first column is labelled when it starts a month's first
/// three weeks.
fn month_labels(start: i32, end: i32, weeks: usize) -> Vec<(usize, &'static str)> {
    let mut labels: Vec<(usize, &'static str)> = Vec::new();
    for col in 0..weeks {
        let column_start = start + col as i32 * 7;
        let first_of_month = (column_start..column_start + 7)
            .filter(|&day| day <= end)
            .map(civil_from_days)
            .find(|&(_, _, date)| date == 1);
        let spaced = labels.last().is_none_or(|&(last, _)| col - last >= 3);
        if let Some((_, month, _)) = first_of_month {
            if spaced {
                labels.push((col, MONTHS[month as usize - 1]));
            }
        } else if col == 0 {
            let (_, month, date) = civil_from_days(column_start);
            if date <= 21 {
                labels.push((0, MONTHS[month as usize - 1]));
            }
        }
    }
    labels
}

impl RenderOnce for Heatmap {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let id_name = self.id.to_string();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| HeatmapState {
            focus: cx.focus_handle(),
            cursor: None,
            hover: None,
        });
        let focus = state.read(cx).focus.clone().tab_stop(true);
        let keyboard_ring = focus::focus_visible(&focus, window, cx);

        let (start, end) = Span::bounds(self.end_day, self.weeks);
        let span = Span {
            start,
            end,
            loading: self.loading,
            pending_from: self.pending_from,
        };
        let cell_f = if self.compact { CELL_COMPACT } else { CELL };
        let cell = px(cell_f);
        let pitch = cell_f + GAP;
        let grid_w = self.weeks as f32 * pitch - GAP;
        let grid_h = 7. * pitch - GAP;
        let cursor = state
            .read(cx)
            .cursor
            .map(|day| span.clamp(day))
            .unwrap_or_else(|| span.clamp(self.selected.unwrap_or(end)));
        let hover = state.read(cx).hover;

        let tooltip_text = {
            let custom = self.tooltip.clone();
            let data = self.data.clone();
            move |day: i32| -> SharedString {
                let (count, _) = data(day);
                match &custom {
                    Some(text) => text(day, count),
                    None => default_tooltip(day, count),
                }
            }
        };

        let mut grid = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .gap(px(GAP))
            .w(px(grid_w))
            .h(px(grid_h))
            .key_context("ui-heatmap")
            .debug_selector({
                let name = format!("{id_name}-grid");
                move || name
            })
            .track_focus(&focus);

        // Pointer: focus the grid on any press so the keyboard continues from there.
        {
            let focus = focus.clone();
            grid = grid.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
            });
        }

        // Keyboard.
        let move_to = {
            let state = state.clone();
            let selected = self.selected;
            Rc::new(
                move |delta: Option<i32>, to: Option<i32>, window: &mut Window, cx: &mut App| {
                    let current = state
                        .read(cx)
                        .cursor
                        .map(|day| span.clamp(day))
                        .unwrap_or_else(|| span.clamp(selected.unwrap_or(span.end)));
                    let next = match (delta, to) {
                        (Some(delta), _) => step(current, delta, span.start, span.end),
                        (None, Some(day)) => span.clamp(day),
                        (None, None) => current,
                    };
                    state.update(cx, |state, _| state.cursor = Some(next));
                    window.refresh();
                },
            )
        };
        let activate = {
            let state = state.clone();
            let selected = self.selected;
            let on_select = self.on_select.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let day = state
                    .read(cx)
                    .cursor
                    .map(|day| span.clamp(day))
                    .unwrap_or_else(|| span.clamp(selected.unwrap_or(span.end)));
                if let Some(listener) = &on_select {
                    if span.selectable(day) {
                        listener(day, window, cx);
                    }
                }
            })
        };
        {
            let (up, down, prev, next, first, last) = (
                move_to.clone(),
                move_to.clone(),
                move_to.clone(),
                move_to.clone(),
                move_to.clone(),
                move_to,
            );
            let (confirm, space) = (activate.clone(), activate);
            let on_clear = self.on_clear.clone();
            let clear_focus = focus.clone();
            grid = grid
                .on_action(move |_: &SelectPrev, window, cx| up(Some(-1), None, window, cx))
                .on_action(move |_: &SelectNext, window, cx| down(Some(1), None, window, cx))
                .on_action(move |_: &HeatmapWeekPrev, window, cx| prev(Some(-7), None, window, cx))
                .on_action(move |_: &HeatmapWeekNext, window, cx| next(Some(7), None, window, cx))
                .on_action(move |_: &SelectFirst, window, cx| {
                    first(None, Some(span.start), window, cx);
                })
                .on_action(move |_: &SelectLast, window, cx| {
                    last(None, Some(span.end), window, cx);
                })
                .on_action(move |_: &Confirm, window, cx| confirm(window, cx))
                .on_action(move |_: &HeatmapActivate, window, cx| space(window, cx))
                .on_action(move |_: &Cancel, window, cx| match &on_clear {
                    // The clear re-renders the strip; keep the grid focused so
                    // the keyboard continues from the day (activity.js refocus).
                    Some(listener) => {
                        listener(window, cx);
                        clear_focus.focus(window, cx);
                    }
                    // Nothing to clear: let an enclosing dialog or panel see Esc.
                    None => cx.propagate(),
                });
        }

        // Cells, column by column (weeks), 7 weekdays each.
        for col in 0..self.weeks {
            let mut column = div().flex().flex_col().gap(px(GAP));
            for row in 0..7 {
                let day = start + (col * 7 + row) as i32;
                if day > end {
                    // Future days are not drawn but keep their slot.
                    column = column.child(div().size(cell).flex_none());
                    continue;
                }
                let index = col * 7 + row;
                let (_, level) = (self.data)(day);
                let level = usize::from(level).min(4);
                let selected = self.selected == Some(day);
                let radius = px(RADIUS);
                let base = div()
                    .id(("cell", index))
                    .size(cell)
                    .flex_none()
                    .rounded(radius)
                    .bg(colors.heat[if span.is_pending(day) { 0 } else { level }]);
                let element: AnyElement = if span.is_pending(day) {
                    base.with_animation(
                        ("shimmer", index),
                        Animation::new(Duration::from_millis(1400))
                            .repeat()
                            .with_easing(ease_in_out),
                        |this, delta| this.opacity(shimmer_opacity(delta)),
                    )
                    .into_any_element()
                } else {
                    let state = state.clone();
                    let focus = focus.clone();
                    let on_select = self.on_select.clone();
                    let border = colors.border_strong;
                    base.when(selected, |this| this.border_1().border_color(colors.text))
                        .when(!selected, move |this| {
                            this.hover(move |style| style.border_1().border_color(border))
                        })
                        .on_hover({
                            let state = state.clone();
                            move |hovered, window, cx| {
                                state.update(cx, |state, _| {
                                    if *hovered {
                                        state.hover = Some(day);
                                    } else if state.hover == Some(day) {
                                        state.hover = None;
                                    }
                                });
                                window.refresh();
                            }
                        })
                        .when_some(on_select, move |this, listener| {
                            this.cursor_pointer().on_click(move |_, window, cx| {
                                focus.focus(window, cx);
                                state.update(cx, |state, _| state.cursor = Some(day));
                                listener(day, window, cx);
                            })
                        })
                        .into_any_element()
                };
                column = column.child(element);
            }
            grid = grid.child(column);
        }

        // The keyboard focus ring and the instant tooltip, anchored to one cell.
        let anchor = hover.or(keyboard_ring.then_some(cursor));
        if keyboard_ring {
            let offset = (cursor - start) as usize;
            let ring = px(3.);
            grid = grid.child(
                div()
                    .absolute()
                    .left(px((offset / 7) as f32 * pitch) - ring)
                    .top(px((offset % 7) as f32 * pitch) - ring)
                    .size(cell + ring * 2.)
                    .rounded(px(RADIUS + 2.))
                    .border_2()
                    .border_color(colors.focus),
            );
        }
        if let Some(day) = anchor.filter(|day| (start..=end).contains(day)) {
            let offset = (day - start) as usize;
            let centre = (offset / 7) as f32 * pitch + cell_f / 2.;
            let top = (offset % 7) as f32 * pitch;
            let half = TIP_BOX_W / 2.;
            let max_left = (grid_w - TIP_BOX_W).max(0.);
            let left = (centre - half).clamp(0., max_left);
            let label: SharedString = if span.is_pending(day) {
                format!("Counting commits on {}", long_date(day)).into()
            } else {
                tooltip_text(day)
            };
            let tip_name = format!("{id_name}-tip");
            grid = grid.child(
                div()
                    .absolute()
                    .left(px(left))
                    // Above the cell; the top rows have no room for it inside
                    // the strip (it would be clipped), so it flips below.
                    .map(|this| {
                        if top < TIP_FLIP_ROOM {
                            this.top(px(top + cell_f + 6.))
                        } else {
                            this.bottom(px(grid_h - top + 6.))
                        }
                    })
                    .w(px(TIP_BOX_W))
                    .flex()
                    .map(|this| {
                        if left <= 0. && centre < half {
                            this.justify_start()
                        } else if left >= max_left && centre > grid_w - half {
                            this.justify_end()
                        } else {
                            this.justify_center()
                        }
                    })
                    .child(
                        div()
                            .debug_selector(move || tip_name)
                            .px(px(8.))
                            .py(px(4.))
                            .bg(colors.surface)
                            .border_1()
                            .border_color(colors.border_strong)
                            .rounded(R_SM)
                            .shadow(cx.theme().pop_shadow())
                            .font_family(FONT_UI)
                            .text_size(FS_SM)
                            .text_color(colors.text)
                            .whitespace_nowrap()
                            .child(label),
                    ),
            );
        }

        let weekday_column = div()
            .w(px(WEEKDAY_W))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .text_size(FS_XS)
            .text_color(colors.muted)
            .font_family(FONT_UI)
            .children((0..7).map(|row| {
                div()
                    .h(cell)
                    .flex()
                    .items_center()
                    .child(if row % 2 == 0 && row < 6 {
                        WEEKDAYS[row]
                    } else {
                        ""
                    })
            }));

        let months = div()
            .relative()
            .h(px(MONTH_H))
            .ml(px(WEEKDAY_W))
            .w(px(grid_w))
            .font_family(FONT_UI)
            .text_size(FS_XS)
            .text_color(colors.muted)
            .children(
                month_labels(start, end, self.weeks)
                    .into_iter()
                    .map(|(col, name)| {
                        div()
                            .absolute()
                            .top(px(0.))
                            .left(px(col as f32 * pitch))
                            .whitespace_nowrap()
                            .child(name)
                    }),
            );

        div()
            .flex()
            .flex_col()
            // Keeps its full width inside a scroll box instead of shrinking to it.
            .flex_none()
            .gap(SP_2)
            .child(months)
            .child(div().flex().child(weekday_column).child(grid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-14, a Wednesday.
    const OCT_14_2026: i32 = 20_740;

    #[test]
    fn civil_dates_round_trip_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(OCT_14_2026), (2026, 10, 14));
    }

    #[test]
    fn weekdays_start_on_monday() {
        assert_eq!(weekday_row(0), 3); // Thursday
        assert_eq!(weekday_row(OCT_14_2026), 2); // Wednesday
        assert_eq!(weekday_row(OCT_14_2026 - 2), 0); // Monday
        assert_eq!(weekday_row(-1), 2); // Wednesday, 1969-12-31
    }

    #[test]
    fn tooltip_text_follows_the_design() {
        assert_eq!(
            default_tooltip(OCT_14_2026, 5),
            "5 commits on Wed, 14 Oct 2026"
        );
        assert_eq!(
            default_tooltip(OCT_14_2026, 1),
            "1 commit on Wed, 14 Oct 2026"
        );
        assert_eq!(
            default_tooltip(OCT_14_2026, 0),
            "No commits on Wed, 14 Oct 2026"
        );
    }

    #[test]
    fn the_grid_ends_in_the_week_of_the_end_day() {
        let (start, end) = Span::bounds(OCT_14_2026, 53);
        assert_eq!(end, OCT_14_2026);
        assert_eq!(weekday_row(start), 0, "the grid starts on a Monday");
        assert_eq!(start + 52 * 7, OCT_14_2026 - 2);
    }

    #[test]
    fn the_cursor_stays_inside_the_grid() {
        let (start, end) = Span::bounds(OCT_14_2026, 53);
        assert_eq!(step(end, 1, start, end), end);
        assert_eq!(step(end, 7, start, end), end);
        assert_eq!(step(start, -1, start, end), start);
        assert_eq!(step(end, -7, start, end), end - 7);
    }

    #[test]
    fn month_labels_skip_crowded_columns() {
        let (start, end) = Span::bounds(OCT_14_2026, 53);
        let labels = month_labels(start, end, 53);
        assert!(labels.windows(2).all(|pair| pair[1].0 - pair[0].0 >= 3));
        assert_eq!(labels.last().map(|label| label.1), Some("Oct"));
        // About one label per month over a year.
        assert!((12..=13).contains(&labels.len()), "{labels:?}");
    }

    #[test]
    fn only_counted_days_in_range_are_selectable() {
        let (start, end) = Span::bounds(OCT_14_2026, 53);
        let span = Span {
            start,
            end,
            loading: false,
            pending_from: Some(OCT_14_2026 - 10),
        };
        assert!(span.selectable(end));
        assert!(!span.selectable(end + 1), "future");
        assert!(!span.selectable(end - 11), "still counting");
        let loading = Span {
            loading: true,
            ..span
        };
        assert!(!loading.selectable(end));
    }
}
