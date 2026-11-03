//! The sidebar's pull request panel: tabs Mine / Review requested / All, one
//! row per request and a details block with "Open on <Service>" and
//! "Checkout" for the selected one.
//!
//! [`PrModel`] owns the data: it loads a tab's first page on the background
//! executor (the account's token is read from the keychain there, never on the
//! UI thread), keeps the answer in the shared [`pr_logic::cache`] and treats it
//! as current for 60 seconds. [`PrPanel`] draws the model. What the panel
//! cannot do itself (open a page, check a branch out, reconnect the account)
//! leaves as a [`PrEvent`] for the repository view.

use std::collections::HashMap;
use std::time::Instant;

use forge::api::{Auth, PrState, PullRequest};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, Context, Entity, EventEmitter, FontWeight, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Window,
};

use ui::metrics::{FS_SM, FS_XS};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{
    Button, ButtonSize, EmptyState, Icon, IconName, ListRow, SegmentItem, Segmented, Skeleton,
    Tooltip,
};

use crate::accounts::TokenError;
use crate::format::relative_time;
use crate::history_view::unix_now;
use crate::pr_logic::{self, ListError, Listing, PrTab, PrTarget};

/// What the panel asks of the repository view.
#[derive(Debug, Clone)]
pub enum PrEvent {
    /// The account's token was refused: open the account dialog on it.
    Reconnect(String),
    /// Open `url` in the browser; `host` names it in the toast.
    Open { url: String, host: String },
    /// Fetch the request's head into a local branch and switch to it.
    Checkout(Box<(PullRequest, PrTarget)>),
}

impl EventEmitter<PrEvent> for PrModel {}

/// What one tab holds: the last list, whether a load is running and why the
/// last one failed.
#[derive(Debug, Default)]
struct TabData {
    listing: Option<Listing>,
    loading: bool,
    error: Option<ListError>,
}

/// The loaded requests of one repository.
pub struct PrModel {
    targets: Vec<PrTarget>,
    tab: PrTab,
    selected: Option<u64>,
    tabs: HashMap<PrTab, TabData>,
    /// Bumped when the target repository changes; an older answer is dropped.
    generation: u64,
    /// Whether the panel is on screen: nothing is loaded before it is.
    active: bool,
    scroll: ScrollHandle,
}

impl PrModel {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            tab: PrTab::Mine,
            selected: None,
            tabs: HashMap::new(),
            generation: 0,
            active: false,
            scroll: ScrollHandle::new(),
        }
    }

    /// The remote the panel lists: the first target (the current branch's
    /// upstream, else `origin`, else the first with an account).
    fn target(&self) -> Option<&PrTarget> {
        self.targets.first()
    }

    pub fn has_target(&self) -> bool {
        !self.targets.is_empty()
    }

    /// The service of the listed remote.
    pub fn kind(&self) -> Option<forge::ServiceKind> {
        self.target().map(PrTarget::kind)
    }

    /// Replaces the remotes the panel can list. A different repository drops
    /// what was loaded.
    pub fn set_targets(&mut self, targets: Vec<PrTarget>, cx: &mut Context<Self>) {
        if targets == self.targets {
            return;
        }
        let changed =
            self.target().map(PrTarget::cache_key) != targets.first().map(PrTarget::cache_key);
        self.targets = targets;
        if changed {
            self.generation += 1;
            self.tabs.clear();
            self.selected = None;
            if self.active {
                self.load(self.tab, false, cx);
            }
        }
        cx.notify();
    }

    /// The panel appeared or went away. Appearing loads the current tab when
    /// it has nothing yet or is older than a minute.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.load(self.tab, false, cx);
        }
        cx.notify();
    }

    /// Reloads the current tab (the sidebar's Refresh): `force` ignores the
    /// list's age (the user asked); otherwise a list younger than a minute
    /// stays. Does nothing while the panel is not shown.
    pub fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.active {
            self.load(self.tab, force, cx);
        }
    }

    /// Forgets the repository's lists, e.g. after creating a request.
    pub fn invalidate(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.target() {
            if let Ok(mut cache) = pr_logic::cache().lock() {
                cache.invalidate(&target.cache_key());
            }
        }
        self.tabs.clear();
        self.generation += 1;
        // The cache is empty now, so this is the one load (a refresh that
        // follows finds it running).
        if self.active {
            self.load(self.tab, false, cx);
        }
        cx.notify();
    }

    fn set_tab(&mut self, tab: PrTab, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        self.selected = None;
        self.load(tab, false, cx);
        cx.notify();
    }

    fn select(&mut self, number: u64, cx: &mut Context<Self>) {
        self.selected = Some(number);
        cx.notify();
    }

    /// The number the tab counter shows: requests that are still open (or
    /// draft) in the All list, once that has been loaded.
    pub fn open_count(&self) -> Option<usize> {
        let target = self.target()?;
        let from_state = self
            .tabs
            .get(&PrTab::All)
            .and_then(|data| data.listing.as_ref());
        let cache = pr_logic::cache().lock().ok()?;
        let listing = from_state.or_else(|| cache.get(&target.cache_key(), PrTab::All))?;
        Some(
            listing
                .items
                .iter()
                .filter(|pr| matches!(pr.state, PrState::Open | PrState::Draft))
                .count(),
        )
    }

    /// Starts loading `tab`'s first page. The shared cache answers first: a
    /// list younger than a minute is used as it is (unless `force`), an older
    /// one is shown while the new one loads.
    fn load(&mut self, tab: PrTab, force: bool, cx: &mut Context<Self>) {
        let Some(target) = self.target().cloned() else {
            return;
        };
        let key = target.cache_key();
        let cached = pr_logic::cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key, tab).cloned());
        let data = self.tabs.entry(tab).or_default();
        if let Some(listing) = cached {
            let fresh = pr_logic::is_fresh(listing.fetched, Instant::now());
            data.listing = Some(listing);
            if fresh && !force {
                data.error = None;
                return;
            }
        }
        if data.loading {
            return;
        }
        data.loading = true;
        let generation = self.generation;
        let store = crate::accounts::cached_store(cx);
        cx.spawn(async move |this, cx| {
            let job = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { fetch_first_page(store, &job, tab) })
                .await;
            this.update(cx, |model, cx| {
                model.finish(generation, &target, tab, result, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        generation: u64,
        target: &PrTarget,
        tab: PrTab,
        result: Result<Listing, ListError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let data = self.tabs.entry(tab).or_default();
        data.loading = false;
        match result {
            Ok(listing) => {
                if let Ok(mut cache) = pr_logic::cache().lock() {
                    cache.put(&target.cache_key(), tab, listing.clone());
                }
                data.listing = Some(listing);
                data.error = None;
            }
            Err(error) => data.error = Some(error),
        }
        cx.notify();
    }
}

/// Reads the token and asks the service for the tab's first page. Blocking:
/// runs on the background executor.
fn fetch_first_page(
    store: Option<crate::accounts::StoreResult>,
    target: &PrTarget,
    tab: PrTab,
) -> Result<Listing, ListError> {
    let store = store
        .unwrap_or_else(crate::accounts::resolve_store)
        .map_err(|error| ListError::Other(error.to_string()))?;
    // An account without a secret is as good as a revoked one.
    let (token, kind) = crate::accounts::access_credential_for_id(
        store.as_ref(),
        &target.account_id,
        crate::accounts::oauth_allowed(&target.instance),
    )
    .map_err(|error| match error {
        TokenError::Missing | TokenError::NeedsReconnect => ListError::TokenRevoked,
        TokenError::Unreachable(why) => ListError::Offline(why),
        TokenError::Keychain(why) => ListError::Other(why),
    })?;
    let (filter, state) = pr_logic::tab_query(tab, &target.login);
    let page = forge::api::list_pull_requests(
        pr_logic::shared_client(),
        &target.instance,
        &target.coords,
        &Auth::new(Some(&token), kind),
        &filter,
        state,
        1,
    )
    .map_err(|error| pr_logic::list_error(&error))?;
    Ok(Listing {
        items: page.items,
        has_next: page.has_next,
        fetched: Instant::now(),
    })
}

/* ---------------------------------- view --------------------------------- */

/// The state glyph and its colour.
fn state_glyph(state: PrState, colors: ThemeColors) -> (IconName, gpui::Hsla) {
    match state {
        PrState::Open => (IconName::Pr, colors.success),
        PrState::Draft => (IconName::PrDraft, colors.muted),
        PrState::Merged => (IconName::Merge, colors.st_untracked),
        PrState::Closed => (IconName::PrClosed, colors.error),
    }
}

fn state_label(state: PrState) -> SharedString {
    match state {
        PrState::Open => ui::t!("sidebar.prs.state.open"),
        PrState::Draft => ui::t!("sidebar.prs.state.draft"),
        PrState::Merged => ui::t!("sidebar.prs.state.merged"),
        PrState::Closed => ui::t!("sidebar.prs.state.closed"),
    }
}

/// The panel, drawn from a [`PrModel`].
#[derive(IntoElement)]
pub struct PrPanel {
    model: Entity<PrModel>,
}

impl PrPanel {
    pub fn new(model: Entity<PrModel>) -> Self {
        Self { model }
    }
}

impl RenderOnce for PrPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let model = self.model.clone();
        let panel = self.model.read(cx);
        let Some(target) = panel.target().cloned() else {
            return div().into_any_element();
        };
        let nouns = crate::pr_logic::nouns(target.kind());
        let data = panel.tabs.get(&panel.tab);
        let tab = panel.tab;
        let selected = panel.selected;
        let scroll = panel.scroll.clone();
        let now = unix_now();

        let filter = {
            let model = model.clone();
            div().flex_none().w_full().p(px(8.)).child(
                Segmented::new("pr-filter")
                    .small()
                    .items([
                        SegmentItem::new(ui::t!("sidebar.prs.filter.mine")),
                        SegmentItem::new(ui::t!("sidebar.prs.filter.review")),
                        SegmentItem::new(ui::t!("sidebar.prs.filter.all")),
                    ])
                    .selected(tab.index())
                    .on_select(move |index, _, cx| {
                        model.update(cx, |m, cx| m.set_tab(PrTab::from_index(index), cx));
                    }),
            )
        };

        let listing = data.and_then(|d| d.listing.as_ref());
        let error = data.and_then(|d| d.error.as_ref());

        let body = match (listing, error) {
            (None, Some(error)) => {
                failure(error, &target, &nouns, &model, colors).into_any_element()
            }
            (None, None) => skeleton().into_any_element(),
            (Some(listing), error) if listing.items.is_empty() => {
                empty(tab, &nouns, error, colors).into_any_element()
            }
            (Some(listing), error) => rows(
                listing, error, selected, &target, &model, &scroll, now, colors,
            )
            .into_any_element(),
        };

        let details = listing
            .and_then(|l| selected.and_then(|n| l.items.iter().find(|pr| pr.number == n)))
            .map(|pr| details(pr, &target, &model, now, colors));

        div()
            .id("pr-panel")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(filter)
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .when_some(details, |this, details| this.child(details))
            .into_any_element()
    }
}

fn skeleton() -> impl IntoElement {
    div()
        .id("pr-loading")
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(
            [70., 55., 80., 62., 74., 50.]
                .into_iter()
                .enumerate()
                .map(|(index, width)| {
                    div()
                        .h(px(26.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(Skeleton::new(("pr-skeleton-glyph", index), px(14.)))
                        .child(Skeleton::new(("pr-skeleton", index), px(width * 2.)))
                }),
        )
}

/// The panel when nothing could be listed.
fn failure(
    error: &ListError,
    target: &PrTarget,
    nouns: &SharedString,
    model: &Entity<PrModel>,
    colors: ThemeColors,
) -> impl IntoElement {
    let retry = {
        let model = model.clone();
        Button::new("pr-retry")
            .size(ButtonSize::Small)
            .label(ui::t!("sidebar.prs.retry"))
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| {
                    let tab = m.tab;
                    m.load(tab, true, cx);
                });
            })
    };
    let state = match error {
        ListError::TokenRevoked => {
            let id = target.account_id.clone();
            let model = model.clone();
            EmptyState::new(ui::t!("sidebar.prs.err_title", nouns = nouns.clone()))
                .icon(IconName::Err)
                .icon_color(colors.error)
                .text(ui::t!(
                    "sidebar.prs.err_revoked",
                    login = target.login.clone(),
                    host = target.host().to_owned()
                ))
                .actions(
                    Button::primary("pr-reconnect")
                        .size(ButtonSize::Small)
                        .label(ui::t!("sidebar.prs.reconnect"))
                        .on_click(move |_, _, cx| {
                            model.update(cx, |_, cx| cx.emit(PrEvent::Reconnect(id.clone())));
                        }),
                )
        }
        ListError::RateLimited { .. } => EmptyState::new(ui::t!("sidebar.prs.rate_title"))
            .icon(IconName::Warn)
            .icon_color(colors.warning)
            .text(ui::t!(
                "sidebar.prs.rate_text",
                host = target.host().to_owned()
            ))
            .actions(retry),
        ListError::Offline(detail) => EmptyState::new(ui::t!(
            "sidebar.prs.offline_title",
            host = target.host().to_owned()
        ))
        .icon(IconName::Err)
        .icon_color(colors.error)
        .text(detail.clone())
        .actions(retry),
        ListError::Other(detail) => {
            EmptyState::new(ui::t!("sidebar.prs.err_title", nouns = nouns.clone()))
                .icon(IconName::Err)
                .icon_color(colors.error)
                .text(detail.clone())
                .actions(retry)
        }
    };
    div().id("pr-error").w_full().child(state)
}

/// "Couldn't refresh" line above a list that is still shown.
fn stale_line(error: &ListError, colors: ThemeColors) -> impl IntoElement {
    let reason: SharedString = match error {
        ListError::TokenRevoked => ui::t!("sidebar.prs.stale_revoked"),
        ListError::RateLimited { .. } => ui::t!("sidebar.prs.rate_title"),
        ListError::Offline(detail) | ListError::Other(detail) => detail.clone().into(),
    };
    div()
        .flex_none()
        .w_full()
        .px(px(10.))
        .py(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(FS_XS)
        .text_color(colors.warning)
        .child(
            Icon::new(IconName::Warn)
                .size(px(12.))
                .color(colors.warning),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(ui::t!("sidebar.prs.stale_line", reason = reason)),
        )
}

fn empty(
    tab: PrTab,
    nouns: &SharedString,
    error: Option<&ListError>,
    colors: ThemeColors,
) -> impl IntoElement {
    let state = if tab == PrTab::ReviewRequested {
        EmptyState::new(ui::t!("sidebar.prs.empty_review_title"))
            .icon(IconName::Pr)
            .text(ui::t!("sidebar.prs.empty_review_text"))
    } else {
        EmptyState::new(ui::t!("sidebar.prs.empty_title", nouns = nouns.clone()))
            .icon(IconName::Pr)
            .text(ui::t!("sidebar.prs.empty_text", nouns = nouns.clone()))
    };
    div()
        .id("pr-empty")
        .w_full()
        .when_some(error, |this, error| this.child(stale_line(error, colors)))
        .child(state)
}

#[allow(clippy::too_many_arguments)]
fn rows(
    listing: &Listing,
    error: Option<&ListError>,
    selected: Option<u64>,
    target: &PrTarget,
    model: &Entity<PrModel>,
    scroll: &ScrollHandle,
    now: u64,
    colors: ThemeColors,
) -> impl IntoElement {
    let mut list = div()
        .id("pr-list")
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .track_scroll(scroll);
    for pr in &listing.items {
        let (icon, tint) = state_glyph(pr.state, colors);
        let number = pr.number;
        let is_selected = selected == Some(number);
        let glyph_tint = if is_selected {
            colors.selection_fg
        } else {
            tint
        };
        let tip = ui::t!(
            "sidebar.prs.row_tip",
            number = number,
            title = pr.title.clone(),
            author = pr.author.clone(),
            state = state_label(pr.state),
            when = relative_time(now, pr.updated_at)
        );
        let model = model.clone();
        let row = ListRow::new(("pr-row", number as usize))
            .selected(is_selected)
            .list_focused(true)
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.select(number, cx));
            })
            .child(Icon::new(icon).size(px(14.)).color(glyph_tint))
            .child(
                div()
                    .flex_none()
                    .text_size(FS_SM)
                    .text_color(ListRow::secondary_color(colors, is_selected, true))
                    .child(format!("#{number}")),
            )
            .child(div().flex_1().min_w_0().truncate().child(pr.title.clone()));
        list = list.child(
            div()
                .id(("pr-row-tip", number as usize))
                .w_full()
                .tooltip(Tooltip::text(tip))
                .child(row),
        );
    }
    if listing.has_next {
        list = list.child(
            div()
                .px(px(10.))
                .py(px(6.))
                .text_size(FS_XS)
                .text_color(colors.muted)
                .child(ui::t!(
                    "sidebar.prs.more",
                    n = listing.items.len(),
                    service = target.service_name()
                )),
        );
    }
    div()
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .when_some(error, |this, error| this.child(stale_line(error, colors)))
        .child(list)
}

fn details(
    pr: &PullRequest,
    target: &PrTarget,
    model: &Entity<PrModel>,
    now: u64,
    colors: ThemeColors,
) -> impl IntoElement {
    let merged = pr.state == PrState::Merged;
    let open = {
        let model = model.clone();
        let url = pr.web_url.clone();
        let host = target.host().to_owned();
        Button::new("pr-open")
            .size(ButtonSize::Small)
            .icon(IconName::External)
            .label(ui::t!(
                "sidebar.prs.open_on",
                service = target.service_name()
            ))
            .on_click(move |_, _, cx| {
                model.update(cx, |_, cx| {
                    cx.emit(PrEvent::Open {
                        url: url.clone(),
                        host: host.clone(),
                    })
                });
            })
    };
    let checkout = {
        let model = model.clone();
        let pr = pr.clone();
        let target = target.clone();
        let noun = crate::pr_logic::noun(target.kind(), false);
        let mut button = Button::primary("pr-checkout")
            .size(ButtonSize::Small)
            .label(ui::t!("sidebar.prs.checkout"))
            .disabled(merged)
            .on_click(move |_, _, cx| {
                model.update(cx, |_, cx| {
                    cx.emit(PrEvent::Checkout(Box::new((pr.clone(), target.clone()))))
                });
            });
        if merged {
            button = button.tooltip(ui::t!("sidebar.prs.checkout_merged_tip", noun = noun));
        }
        button
    };
    div()
        .id("pr-details")
        .flex_none()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(12.))
        .py(px(10.))
        .bg(colors.bg)
        .border_t_1()
        .border_color(colors.border)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(pr.title.clone()),
        )
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(ui::t!(
                    "sidebar.prs.detail_meta",
                    author = pr.author.clone(),
                    state = state_label(pr.state),
                    head = pr.head_branch.clone(),
                    base = pr.base_branch.clone(),
                    when = relative_time(now, pr.updated_at)
                )),
        )
        .child(div().flex().gap(px(6.)).child(open).child(checkout))
}

#[cfg(test)]
mod tests {
    use forge::api::ReviewState;
    use forge::{Instance, RepoCoords, ServiceKind};
    use gpui::AppContext;

    use super::*;

    fn target(repo: &str) -> PrTarget {
        PrTarget {
            remote: "origin".to_owned(),
            account_id: "acc".to_owned(),
            login: "mira-k".to_owned(),
            instance: Instance::new(ServiceKind::GitHub, "https://github.com").unwrap(),
            coords: RepoCoords {
                namespace: vec!["o".to_owned()],
                repo: repo.to_owned(),
            },
        }
    }

    fn request(number: u64, state: PrState) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR {number}"),
            state,
            review: ReviewState::None,
            author: "mira-k".to_owned(),
            head_branch: format!("feat/{number}"),
            head_repo_full_name: None,
            base_branch: "main".to_owned(),
            updated_at: 0,
            web_url: String::new(),
        }
    }

    fn seed(repo: &str, tab: PrTab, items: Vec<PullRequest>, age: std::time::Duration) {
        pr_logic::cache().lock().unwrap().put(
            &target(repo).cache_key(),
            tab,
            Listing {
                items,
                has_next: false,
                fetched: Instant::now() - age,
            },
        );
    }

    #[gpui::test]
    fn a_fresh_cached_list_is_shown_without_loading(cx: &mut gpui::TestAppContext) {
        seed(
            "fresh",
            PrTab::Mine,
            vec![request(1, PrState::Open)],
            std::time::Duration::from_secs(5),
        );
        seed(
            "fresh",
            PrTab::All,
            vec![
                request(1, PrState::Open),
                request(2, PrState::Draft),
                request(3, PrState::Merged),
                request(4, PrState::Closed),
            ],
            std::time::Duration::from_secs(5),
        );
        let model = cx.new(|_| PrModel::new());
        model.update(cx, |model, cx| {
            model.set_targets(vec![target("fresh")], cx);
            // Nothing is loaded before the panel is shown.
            assert!(model.tabs.is_empty());
            model.set_active(true, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |model, _| {
            let data = model.tabs.get(&PrTab::Mine).expect("the tab was loaded");
            assert!(
                !data.loading,
                "a list younger than a minute is not asked again"
            );
            assert_eq!(data.listing.as_ref().unwrap().items.len(), 1);
            // Merged and closed requests do not count as open.
            assert_eq!(model.open_count(), Some(2));
        });
    }

    #[gpui::test]
    fn another_repository_drops_what_was_loaded(cx: &mut gpui::TestAppContext) {
        seed(
            "first",
            PrTab::Mine,
            vec![request(1, PrState::Open)],
            std::time::Duration::from_secs(1),
        );
        seed(
            "second",
            PrTab::Mine,
            vec![request(7, PrState::Open), request(8, PrState::Open)],
            std::time::Duration::from_secs(1),
        );
        let model = cx.new(|_| PrModel::new());
        model.update(cx, |model, cx| {
            model.set_active(true, cx);
            model.set_targets(vec![target("first")], cx);
            model.select(1, cx);
        });
        model.read_with(cx, |model, _| {
            assert_eq!(model.selected, Some(1));
            assert_eq!(
                model.tabs[&PrTab::Mine]
                    .listing
                    .as_ref()
                    .unwrap()
                    .items
                    .len(),
                1
            );
        });
        model.update(cx, |model, cx| {
            model.set_targets(vec![target("second")], cx)
        });
        model.read_with(cx, |model, _| {
            assert_eq!(model.selected, None);
            assert_eq!(
                model.tabs[&PrTab::Mine]
                    .listing
                    .as_ref()
                    .unwrap()
                    .items
                    .len(),
                2
            );
        });
    }

    #[gpui::test]
    fn another_account_on_the_same_repository_drops_what_was_loaded(cx: &mut gpui::TestAppContext) {
        let mine = target("shared");
        let mut other = target("shared");
        other.account_id = "acc2".to_owned();
        other.login = "someone".to_owned();
        for t in [&mine, &other] {
            pr_logic::cache().lock().unwrap().put(
                &t.cache_key(),
                PrTab::Mine,
                Listing {
                    items: vec![request(
                        if t.login == "mira-k" { 1 } else { 2 },
                        PrState::Open,
                    )],
                    has_next: false,
                    fetched: Instant::now(),
                },
            );
        }
        let model = cx.new(|_| PrModel::new());
        model.update(cx, |model, cx| {
            model.set_active(true, cx);
            model.set_targets(vec![mine], cx);
            model.select(1, cx);
        });
        model.update(cx, |model, cx| model.set_targets(vec![other], cx));
        model.read_with(cx, |model, _| {
            assert_eq!(model.selected, None);
            let items = &model.tabs[&PrTab::Mine].listing.as_ref().unwrap().items;
            assert_eq!(items[0].number, 2, "the other account's own list");
        });
    }

    #[gpui::test]
    fn an_automatic_refresh_keeps_a_fresh_list(cx: &mut gpui::TestAppContext) {
        seed(
            "auto",
            PrTab::Mine,
            vec![request(1, PrState::Open)],
            std::time::Duration::from_secs(5),
        );
        let model = cx.new(|_| PrModel::new());
        model.update(cx, |model, cx| {
            model.set_targets(vec![target("auto")], cx);
            model.set_active(true, cx);
            model.refresh(false, cx);
        });
        model.read_with(cx, |model, _| {
            assert!(!model.tabs[&PrTab::Mine].loading, "within the 60 s window");
        });
    }

    #[gpui::test]
    fn invalidate_forgets_the_cached_lists(cx: &mut gpui::TestAppContext) {
        seed(
            "invalidated",
            PrTab::All,
            vec![request(1, PrState::Open)],
            std::time::Duration::from_secs(1),
        );
        let model = cx.new(|_| PrModel::new());
        model.update(cx, |model, cx| {
            model.set_targets(vec![target("invalidated")], cx);
            // Not shown, so nothing reloads (and nothing asks the network).
            model.invalidate(cx);
        });
        assert!(pr_logic::cache()
            .lock()
            .unwrap()
            .get(&target("invalidated").cache_key(), PrTab::All)
            .is_none());
        model.read_with(cx, |model, _| assert_eq!(model.open_count(), None));
    }
}
