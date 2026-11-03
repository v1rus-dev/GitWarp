//! The Activity strip above the History list: a commit heatmap with a legend,
//! an author switch (Me / Everyone), a range select and the day filter's bar.
//!
//! The counts come from one `git log` on the background executor (the last
//! weeks first, then the whole history), are cached here and are only
//! recomputed when the history reloads. Picking a day filters the History
//! list through [`HistoryView::set_day_filter`]; the history owns the filter,
//! this view only reads it back to draw the selection and the bar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, point, px, App, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription,
    Task, WeakEntity, Window,
};

use git_core::{Activity, Error, Git, Repo};
use ui::actions::Cancel;
use ui::i18n;
use ui::metrics::{FONT_MONO, FONT_UI, FS_SM};

use crate::text_runs::{mono, strong};
use ui::{
    long_date, ActiveTheme, Button, ButtonSize, HeatLegend, Heatmap, Icon, IconName, Kbd,
    SegmentItem, Segmented, Select, Skeleton,
};

use crate::format::format_count;
use crate::history_view::{DayFilter, HistoryView};
use crate::workspace::Workspace;

/// Columns of the heatmap: a year plus the partial week on each side.
pub(crate) const WEEKS: usize = 53;
/// The days the heatmap's "last 12 months" total counts.
pub(crate) const LAST_YEAR_DAYS: i32 = 365;
/// How far back the quick first count looks.
const PARTIAL_DAYS: i32 = 56;
/// How many past years the range select offers.
const YEARS_SHOWN: usize = 5;
/// The strip's header height, the same as the details dock's.
const HEADER: f32 = 34.;
/// Narrowest the side column may get; the grid scrolls before it shrinks below.
const SIDE_MIN_W: f32 = 160.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Me,
    Everyone,
}

#[derive(Debug)]
enum Status {
    /// Nothing counted yet.
    Loading,
    /// The recent weeks are in; days before `from` are still being counted.
    Partial {
        from: i32,
    },
    Ready,
    Failed(String),
}

/// What one counting run found.
struct Counted {
    today: i32,
    me: Option<String>,
    activity: Activity,
}

/// Commit counts per day for one author scope.
type Counts = Rc<HashMap<i32, u32>>;

/// The numbers the heatmap draws for the current author and range.
#[derive(Clone)]
struct Shown {
    counts: Counts,
    thresholds: [u32; 4],
    total: usize,
    end_day: i32,
}

/// Whether the Activity statistics are switched on in Settings (on until a
/// settings file says otherwise).
pub(crate) fn enabled(cx: &App) -> bool {
    cx.try_global::<crate::AppSettings>()
        .is_none_or(|state| state.settings.show_activity)
}

pub struct ActivityStrip {
    path: PathBuf,
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    history: Entity<HistoryView>,
    /// Redraws the selection and the bar when the history's filter changes
    /// (also from its own Esc).
    _history_observer: Subscription,
    collapsed: bool,
    /// Off while the Activity statistics setting is off: nothing is counted.
    enabled: bool,
    who: Who,
    /// `None` is "Last 12 months".
    year: Option<i32>,
    status: Status,
    today: i32,
    me: Option<String>,
    mine: Counts,
    all: Counts,
    /// Years with commits, newest first, at most [`YEARS_SHOWN`].
    years: Vec<i32>,
    shown: Option<Shown>,
    generation: u64,
    task: Option<Task<()>>,
    /// Scroll of the heatmap: it sits at the newest week (the right end)
    /// unless the user scrolled away (design `activity.js` opens at the end).
    scroll: ScrollHandle,
}

/// The UTC day number of the system clock: where the loading skeleton's grid
/// ends before git has said which local day it is (the first count replaces it).
fn provisional_today() -> i32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    i32::try_from(secs / 86_400).unwrap_or(0)
}

impl ActivityStrip {
    pub fn new(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        history: Entity<HistoryView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let collapsed = cx
            .try_global::<crate::AppSettings>()
            .is_some_and(|state| state.settings.activity_collapsed);
        let observer = cx.observe(&history, |_, _, cx| cx.notify());
        let mut strip = Self {
            path,
            git,
            workspace,
            history,
            _history_observer: observer,
            collapsed,
            enabled: enabled(cx),
            who: Who::Me,
            year: None,
            status: Status::Loading,
            today: provisional_today(),
            me: None,
            mine: Rc::default(),
            all: Rc::default(),
            years: Vec::new(),
            shown: None,
            generation: 0,
            task: None,
            scroll: ScrollHandle::new(),
        };
        strip.load(cx);
        strip
    }

    /// Counts again, e.g. after a commit moved HEAD. The old numbers stay on
    /// screen until the new ones land, so the grid does not flash.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    /// Follows the Settings switch: turning it off stops any count in flight,
    /// turning it on counts again.
    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.load(cx);
        } else {
            self.generation += 1;
            self.task = None;
        }
        cx.notify();
    }

    /// The heatmap's scroll state (tests check it opens at the newest week).
    #[cfg(test)]
    pub(crate) fn scroll_handle(&self) -> ScrollHandle {
        self.scroll.clone()
    }

    /// The day the strip counted up to (tests aim the keyboard cursor with it).
    #[cfg(test)]
    pub(crate) fn today(&self) -> i32 {
        self.today
    }

    pub(crate) fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self.status, Status::Ready)
    }

    #[cfg(test)]
    pub(crate) fn total(&self) -> Option<usize> {
        self.shown.as_ref().map(|shown| shown.total)
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        if self.shown.is_none() {
            self.status = Status::Loading;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.status = Status::Failed(error.to_string());
                return;
            }
        };
        let path = self.path.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let opened = executor
                .spawn(async move {
                    let repo = Repo::open(git, &path).map_err(|e| e.to_string())?;
                    let today = git_core::today(&repo).map_err(|e| e.to_string())?;
                    let me = git_core::my_email(&repo).map_err(|e| e.to_string())?;
                    Ok::<_, String>((repo, today, me))
                })
                .await;
            let (repo, today, me) = match opened {
                Ok(opened) => opened,
                Err(message) => {
                    this.update(cx, |view, cx| view.fail(generation, message, cx))
                        .ok();
                    return;
                }
            };
            // The whole history starts counting now; the recent weeks are
            // counted beside it so a big repository paints them first. Days
            // are padded by one on each side so no timezone can cut a day.
            let until = (i64::from(today) + 2) * 86_400;
            let full = {
                let (repo, me) = (repo.clone(), me.clone());
                executor.spawn(async move {
                    git_core::activity(&repo, 0, until, me.as_deref()).map_err(|e| e.to_string())
                })
            };
            let since = (i64::from(today - PARTIAL_DAYS) - 1) * 86_400;
            let partial = {
                let me = me.clone();
                executor.spawn(async move {
                    git_core::activity(&repo, since, until, me.as_deref())
                        .map_err(|e| e.to_string())
                })
            };
            if let Ok(activity) = partial.await {
                let counted = Counted {
                    today,
                    me: me.clone(),
                    activity,
                };
                this.update(cx, |view, cx| view.apply_partial(generation, counted, cx))
                    .ok();
            }
            let result = full.await.map(|activity| Counted {
                today,
                me,
                activity,
            });
            this.update(cx, |view, cx| match result {
                Ok(counted) => view.apply_full(generation, counted, cx),
                Err(message) => view.fail(generation, message, cx),
            })
            .ok();
        }));
        cx.notify();
    }

    fn fail(&mut self, generation: u64, message: String, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        log::warn!("could not count the commits per day: {message}");
        self.status = Status::Failed(message);
        cx.notify();
    }

    /// The recent weeks arrived; ignored when the full count already landed.
    fn apply_partial(&mut self, generation: u64, counted: Counted, cx: &mut Context<Self>) {
        if generation != self.generation || !matches!(self.status, Status::Loading) {
            return;
        }
        self.adopt(&counted);
        self.status = Status::Partial {
            from: counted.today - PARTIAL_DAYS,
        };
        self.recompute();
        cx.notify();
    }

    fn apply_full(&mut self, generation: u64, counted: Counted, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.adopt(&counted);
        self.status = Status::Ready;
        // The set of years may have shrunk on a reload.
        if self.year.is_some_and(|year| !self.years.contains(&year)) {
            self.year = None;
        }
        self.recompute();
        cx.notify();
    }

    /// Takes a run's numbers over; the counts are per day, so a partial run
    /// and the full one differ only in how far back they reach.
    fn adopt(&mut self, counted: &Counted) {
        self.today = counted.today;
        self.me = counted.me.clone();
        let mut mine = HashMap::new();
        let mut all = HashMap::new();
        for day in &counted.activity.days {
            if day.all > 0 {
                all.insert(day.day, day.all);
            }
            if day.mine > 0 {
                mine.insert(day.day, day.mine);
            }
        }
        self.mine = Rc::new(mine);
        self.all = Rc::new(all);
        if self.me.is_none() {
            // Without an identity there is nothing to call "mine".
            self.who = Who::Everyone;
        }
        let mut years: Vec<i32> = self
            .all
            .keys()
            .map(|&day| git_core::civil_from_days(day).0)
            .collect();
        years.sort_unstable_by(|a, b| b.cmp(a));
        years.dedup();
        years.truncate(YEARS_SHOWN);
        self.years = years;
    }

    /// Rebuilds what the heatmap draws after the data, author or range changed.
    fn recompute(&mut self) {
        let counts = match self.who {
            Who::Me => self.mine.clone(),
            Who::Everyone => self.all.clone(),
        };
        let (start, end) = range_days(self.year, self.today);
        let in_range: Vec<u32> = counts
            .iter()
            .filter(|(&day, _)| (start..=end).contains(&day))
            .map(|(_, &count)| count)
            .collect();
        self.shown = Some(Shown {
            thresholds: git_core::levels(&in_range),
            total: in_range.iter().map(|&c| c as usize).sum(),
            counts,
            end_day: end,
        });
    }

    pub(crate) fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        crate::update_settings(cx, move |settings| settings.activity_collapsed = collapsed);
        cx.notify();
    }

    /// Switching author or range changes the numbers the filter was picked
    /// from, so the filter goes away with them.
    fn set_who(&mut self, who: Who, cx: &mut Context<Self>) {
        if self.who == who || (who == Who::Me && self.me.is_none()) {
            return;
        }
        self.who = who;
        self.clear_filter(cx);
        self.recompute();
        cx.notify();
    }

    fn set_year(&mut self, year: Option<i32>, cx: &mut Context<Self>) {
        if self.year == year {
            return;
        }
        self.year = year;
        self.clear_filter(cx);
        self.recompute();
        cx.notify();
    }

    fn clear_filter(&mut self, cx: &mut Context<Self>) {
        self.history
            .update(cx, |history, cx| history.set_day_filter(None, cx));
    }

    /// A click, Enter or Space on `day`: filter to it, or drop the filter when
    /// it is already the filtered day.
    pub(crate) fn pick_day(&mut self, day: i32, cx: &mut Context<Self>) {
        let Some(shown) = &self.shown else {
            return;
        };
        if !matches!(self.status, Status::Ready) || !shown.counts.contains_key(&day) {
            return;
        }
        let already = self
            .history
            .read(cx)
            .day_filter()
            .is_some_and(|filter| filter.day == day);
        let filter = (!already).then(|| DayFilter {
            day,
            me: match self.who {
                Who::Me => self.me.clone(),
                Who::Everyone => None,
            },
        });
        self.history
            .update(cx, |history, cx| history.set_day_filter(filter, cx));
    }

    fn open_identity(&self, window: &mut Window, cx: &mut App) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_settings_at(crate::settings_ui::Page::Identity, window, cx)
            })
            .ok();
    }
}

/// The first and last day the total counts: the last 365 days up to today, or
/// the calendar year (up to today for the current one).
pub(crate) fn range_days(year: Option<i32>, today: i32) -> (i32, i32) {
    match year {
        None => (today - LAST_YEAR_DAYS + 1, today),
        Some(year) => {
            let end = git_core::days_from_civil(year, 12, 31).min(today);
            (git_core::days_from_civil(year, 1, 1), end)
        }
    }
}

/// `1 commit` / `4 commits`.
pub(crate) fn commits(count: usize) -> String {
    ui::i18n::t_count(
        "activity.commits",
        count as u64,
        &[("n", &format_count(count))],
    )
    .to_string()
}

impl ActivityStrip {
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let collapsed = self.collapsed;
        let tip = if collapsed {
            ui::t!("activity.header.expand")
        } else {
            ui::t!("activity.header.collapse")
        };
        let toggle = Button::ghost("activity-toggle")
            .size(ButtonSize::Xs)
            .icon_only(if collapsed {
                IconName::ChevR
            } else {
                IconName::ChevD
            })
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, _, cx| this.set_collapsed(!this.collapsed, cx)));

        let mine = self.who == Who::Me;
        // Shrinks first (clipped, not wrapped) when the header is too narrow;
        // the controls on the right keep their full size.
        let total = div()
            .flex()
            .flex_shrink_1()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .whitespace_nowrap();
        let total = match (&self.status, &self.shown) {
            (Status::Failed(_), _) => total.child(ui::t!("activity.total.unavailable")),
            (Status::Loading, _) | (_, None) => {
                total.child(Skeleton::new("activity-total-skeleton", px(180.)))
            }
            (Status::Partial { .. }, Some(shown)) => total
                .child(Icon::new(IconName::Fetch).size(px(12.)).color(colors.muted))
                .child(div().flex().children(i18n::pieces_children(
                    i18n::t_count_pieces(
                        "activity.total.counting",
                        shown.total as u64,
                        &[],
                        &["n"],
                    ),
                    vec![strong(colors.text, format_count(shown.total)).into_any_element()],
                ))),
            (Status::Ready, Some(shown)) => {
                if self.all.is_empty() {
                    total.child(ui::t!("activity.total.none"))
                } else if shown.total == 0 && mine {
                    total.child(match self.year {
                        Some(year) => ui::t!("activity.total.none_mine_year", year = year),
                        None => ui::t!("activity.total.none_mine_last_year"),
                    })
                } else {
                    let n = shown.total as u64;
                    let pieces = match (mine, self.year) {
                        (true, Some(year)) => i18n::t_count_pieces(
                            "activity.total.mine_year",
                            n,
                            &[("year", &year)],
                            &["n"],
                        ),
                        (true, None) => {
                            i18n::t_count_pieces("activity.total.mine_last_year", n, &[], &["n"])
                        }
                        (false, Some(year)) => i18n::t_count_pieces(
                            "activity.total.all_year",
                            n,
                            &[("year", &year)],
                            &["n"],
                        ),
                        (false, None) => {
                            i18n::t_count_pieces("activity.total.all_last_year", n, &[], &["n"])
                        }
                    };
                    total.child(div().flex().children(i18n::pieces_children(
                        pieces,
                        vec![strong(colors.text, format_count(shown.total)).into_any_element()],
                    )))
                }
            }
        };

        // Design: the legend stays while loading; it goes with an error or an
        // empty repository.
        let show_legend = !(collapsed
            || matches!(self.status, Status::Failed(_))
            || matches!(self.status, Status::Ready) && self.all.is_empty());
        // The identity is unknown until the first count (and after an error),
        // so `Me` is only disabled once git said there is no user.email.
        let me_disabled =
            self.me.is_none() && matches!(self.status, Status::Ready | Status::Partial { .. });
        let me_tip = match &self.me {
            Some(email) => ui::t!("activity.author.me_tip", email = email),
            None => ui::t!("activity.author.me_tip_unset"),
        };
        let author = Segmented::new("activity-author")
            .small()
            .item(
                SegmentItem::new(ui::t!("activity.author.me"))
                    .with_icon(IconName::User)
                    .id("activity-author-me")
                    .disabled(me_disabled)
                    .tooltip(me_tip),
            )
            .item(
                SegmentItem::new(ui::t!("activity.author.everyone")).id("activity-author-everyone"),
            )
            .selected(if mine { 0 } else { 1 })
            .on_select({
                let view = cx.entity();
                move |index, _, cx| {
                    view.update(cx, |this, cx| {
                        this.set_who(if index == 0 { Who::Me } else { Who::Everyone }, cx);
                    });
                }
            });

        let mut options: Vec<SharedString> = vec![ui::t!("activity.range.last_12_months")];
        options.extend(self.years.iter().map(|year| year.to_string().into()));
        let selected = self
            .year
            .and_then(|year| self.years.iter().position(|&y| y == year))
            .map_or(0, |position| position + 1);
        let years = self.years.clone();
        let range = Select::new("activity-range")
            .options(options)
            .selected(selected)
            .on_change({
                let view = cx.entity();
                move |index, _, cx| {
                    let year = index.checked_sub(1).and_then(|i| years.get(i).copied());
                    view.update(cx, |this, cx| this.set_year(year, cx));
                }
            });

        div()
            .h(px(HEADER))
            .flex_none()
            .pl(px(12.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .font_family(FONT_UI)
            .child(toggle)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(ui::metrics::FS_BASE)
                    .child(ui::t!("activity.header.title")),
            )
            .child(total)
            .child(div().flex_1())
            // The legend gives way (clipped) before the segment and select do.
            .when(show_legend, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_shrink_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(HeatLegend::new()),
                )
            })
            .child(div().flex_none().child(author))
            .child(div().flex_none().child(range))
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let filter = self.history.read(cx).day_filter().cloned();
        let body = div()
            .flex()
            .items_start()
            .gap(px(24.))
            .px(px(16.))
            .pb(px(12.));

        if let Status::Failed(message) = &self.status {
            let view = cx.entity();
            return body.child(
                div()
                    .flex_1()
                    .min_h(px(98.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(Icon::new(IconName::Err).size(px(16.)).color(colors.error))
                    .child(
                        // Content-width: Retry sits right after the message.
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(ui::t!("activity.error.title")),
                            )
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .child(SharedString::from(message.clone())),
                            ),
                    )
                    .child(
                        Button::new("activity-retry")
                            .size(ButtonSize::Small)
                            .icon(IconName::Fetch)
                            .label(ui::t!("activity.error.retry"))
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| this.load(cx));
                            }),
                    ),
            );
        }

        let loading = matches!(self.status, Status::Loading) || self.shown.is_none();
        let pending_from = match self.status {
            Status::Partial { from } => Some(from),
            _ => None,
        };
        let (counts, thresholds, end_day) = match &self.shown {
            Some(shown) => (shown.counts.clone(), shown.thresholds, shown.end_day),
            None => (Rc::default(), [0; 4], self.today),
        };
        let selected = filter.as_ref().map(|filter| filter.day);
        let view = cx.entity();
        let select_view = view.clone();
        let mut heatmap = Heatmap::new("activity-heatmap", end_day, WEEKS, {
            let counts = counts.clone();
            move |day| {
                let count = counts.get(&day).copied().unwrap_or(0);
                (count, git_core::level(count, &thresholds))
            }
        })
        .selected(selected)
        .loading(loading)
        .pending_from(pending_from)
        .on_select(move |day, _, cx| {
            select_view.update(cx, |this, cx| this.pick_day(day, cx));
        });
        if filter.is_some() {
            // Esc in the grid drops the filter; without one the grid lets it
            // travel on.
            heatmap = heatmap.on_clear(move |_, cx| {
                view.update(cx, |this, cx| this.clear_filter(cx));
            });
        }

        // Design `.act-side { flex: 1 1 0; min-width: 0 }`: the grid keeps the
        // width it needs and the side column takes what is left.
        // Not in the design (it stops at ~1200 px): a minimum so that the grid
        // shrinks and scrolls first, instead of the text collapsing to one
        // letter per line in a narrow window.
        let side = div()
            .flex_1()
            .min_w(px(SIDE_MIN_W))
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(18.))
            .text_size(FS_SM)
            .text_color(colors.muted);
        let side = self.render_side(side, filter.as_ref(), loading, cx);

        // Keep the newest week in view: while the scroll sits at its end (or
        // was never moved) re-pin it, so a resize keeps the end too. The paint
        // clamps the huge offset to the real end.
        let at_end = self.scroll.offset().x <= -self.scroll.max_offset().x + px(1.);
        if at_end {
            self.scroll.set_offset(point(px(-1.0e6), px(0.)));
        }

        body.child(
            div()
                .id("activity-scroll")
                // Design `.heat-scroll { flex: 0 1 auto; overflow-x: auto }`:
                // the grid shrinks and scrolls when the row is too narrow.
                // A flex box (not gpui's default block), so the grid keeps its
                // own width and overflows instead of stretching to the box.
                .flex()
                .min_w_0()
                .overflow_x_scroll()
                .track_scroll(&self.scroll)
                .child(heatmap),
        )
        .child(side)
    }

    fn render_side(
        &mut self,
        side: gpui::Div,
        filter: Option<&DayFilter>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let colors = cx.theme().colors;
        let link = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(colors.accent)
                .hover(|this| this.underline())
                .child(label)
        };

        if loading {
            return side
                .child(Skeleton::new("activity-side-1", px(140.)))
                .child(Skeleton::new("activity-side-2", px(110.)));
        }
        if self.me.is_none() {
            let view = cx.entity();
            return side
                .child(i18n::sentence(
                    i18n::t_pieces("activity.side.unset", &[], &["key"]),
                    vec![mono(colors.text, "user.email".to_owned()).into_any_element()],
                ))
                .child(
                    link(
                        "activity-set-identity",
                        ui::t!("activity.side.set_identity"),
                    )
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.open_identity(window, cx));
                    }),
                );
        }
        if self.all.is_empty() {
            return side.child(ui::t!("activity.side.first_commit"));
        }
        if self.who == Who::Me && self.mine.is_empty() {
            let view = cx.entity();
            return side
                .child(
                    // The design ends the line with a full stop right after the address.
                    i18n::sentence(
                        i18n::t_pieces("activity.side.none_by", &[], &["email"]),
                        vec![mono(colors.text, self.me.clone().unwrap_or_default())
                            .into_any_element()],
                    ),
                )
                .child(
                    link(
                        "activity-show-everyone",
                        ui::t!("activity.side.show_everyone"),
                    )
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.set_who(Who::Everyone, cx));
                    }),
                );
        }
        if let (Some(filter), Some(shown)) = (filter, &self.shown) {
            let count = self
                .history
                .read(cx)
                .day_filter_count()
                .or_else(|| shown.counts.get(&filter.day).map(|&c| c as usize))
                .unwrap_or(0);
            return side
                .child(strong(colors.text, long_date(filter.day)))
                .child(ui::t!("activity.side.filtered", commits = commits(count)));
        }
        if matches!(self.status, Status::Partial { .. }) {
            return side.child(i18n::sentence(
                i18n::t_pieces("activity.side.partial", &[], &["command"]),
                vec![mono(colors.text, "git log".to_owned()).into_any_element()],
            ));
        }
        let keys = |text: &'static str| Kbd::new(text);
        let pair = |a: &'static str, b: &'static str| {
            div()
                .flex()
                .gap(px(4.))
                .child(keys(a))
                .child(keys(b))
                .into_any_element()
        };
        side.child(ui::t!("activity.side.hint")).child(
            // Wraps only when the side column is too narrow, as in the
            // design; items wrap whole, never mid-word.
            i18n::sentence(
                i18n::t_pieces(
                    "activity.side.keys",
                    &[],
                    &["day_keys", "week_keys", "filter_key"],
                ),
                vec![
                    pair("↑", "↓"),
                    pair("←", "→"),
                    keys("Enter").into_any_element(),
                ],
            )
            .whitespace_nowrap(),
        )
    }

    /// The accent-soft bar under the strip while a day filter is on.
    fn render_filter_bar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let colors = cx.theme().colors;
        let history = self.history.read(cx);
        let filter = history.day_filter()?.clone();
        let count = history.day_filter_count();
        let noun_n = count.unwrap_or(0) as u64;
        let pieces = if filter.me.is_some() {
            i18n::t_count_pieces("activity.filter.mine", noun_n, &[], &["n", "date"])
        } else {
            i18n::t_count_pieces("activity.filter.all", noun_n, &[], &["n", "date"])
        };
        let view = cx.entity();
        Some(
            div()
                .id("activity-day-filter")
                .h(px(30.))
                .flex_none()
                .pl(px(16.))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(colors.accent_soft)
                .border_b_1()
                .border_color(colors.border)
                .font_family(FONT_UI)
                .text_size(FS_SM)
                .child(
                    Icon::new(IconName::Lanes)
                        .size(px(14.))
                        .color(colors.accent),
                )
                .child(div().flex().items_center().children(i18n::pieces_children(
                    pieces,
                    vec![
                            strong(colors.text, count.map_or("…".to_owned(), format_count))
                                .into_any_element(),
                            strong(colors.text, long_date(filter.day)).into_any_element(),
                        ],
                )))
                .child(div().flex_1())
                .child(
                    Button::new("activity-clear-filter")
                        .size(ButtonSize::Small)
                        .label(ui::t!("activity.filter.clear"))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.clear_filter(cx));
                        }),
                )
                .child(Kbd::new("Esc")),
        )
    }
}

impl Render for ActivityStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let collapsed = self.collapsed;
        let strip = div()
            .id("activity-strip")
            .flex_none()
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .text_color(colors.text)
            // Esc anywhere in the strip drops the filter; with none it goes on.
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.history.read(cx).day_filter().is_some() {
                    this.clear_filter(cx);
                } else {
                    cx.propagate();
                }
            }))
            .child(self.render_header(cx))
            .when(!collapsed, |this| this.child(self.render_body(cx)));
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(strip)
            .children(self.render_filter_bar(cx))
    }
}
