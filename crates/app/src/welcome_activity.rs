//! The "Your activity" card on the welcome screen: one heatmap of the user's
//! commits over the last 12 months across every recent repository.
//!
//! The author is always the global `user.email`. Each reachable recent
//! repository is counted on the background executor and the card redraws as
//! every one finishes. The counts are cached for the session per repository,
//! keyed by the mtime of its `.git`, so coming back to the welcome screen does
//! not run `git log` again. Repositories that are gone or cannot be read are
//! skipped and counted in the footer.
//!
//! Picking a day offers the repositories with commits that day; choosing one
//! opens it with its History already filtered to that day
//! ([`Workspace::open_repository_at_day`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui::prelude::*;
use gpui::{div, px, App, Context, FontWeight, Global, SharedString, Task, WeakEntity, Window};

use git_core::{Error, Git, Repo};
use ui::i18n;
use ui::metrics::{FONT_MONO, FS_LG, FS_SM, R_MD, SP_4, SP_5};
use ui::{
    long_date, menu, ActiveTheme, Button, ButtonSize, HeatLegend, Heatmap, Icon, IconName,
    MenuEntry, MenuItem, SegmentItem, Segmented, Skeleton, Tooltip,
};

use crate::activity_strip::{commits, range_days, LAST_YEAR_DAYS, WEEKS};
use crate::format::{display_path, format_count, home_dir};
use crate::history_view::DayFilter;
use crate::welcome::recent_name;
use crate::workspace::Workspace;

/// Which repositories the card counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    /// Every recent repository.
    All,
    /// The repository selected in the recent list.
    Selected,
}

/// What is known about one recent repository.
#[derive(Clone, Debug)]
enum RepoState {
    Loading,
    /// Commits by me per day over the last 12 months.
    /// The counts and the repository's common git dir: linked worktrees of
    /// one repository share it and are counted once.
    Ready(Arc<HashMap<i32, u32>>, PathBuf),
    /// Why it could not be counted.
    Skipped(String),
}

/// The global `user.email`.
enum Me {
    Loading,
    Known(Option<String>),
}

/// One counted repository, as kept for the session.
#[derive(Clone)]
struct CacheEntry {
    /// mtime of `.git` (and its `HEAD`); `None` never matches.
    stamp: Option<SystemTime>,
    me: String,
    group: PathBuf,
    today: i32,
    counts: Arc<HashMap<i32, u32>>,
}

/// The session's cache of counted repositories, by path.
#[derive(Default)]
struct ActivityCache(HashMap<PathBuf, CacheEntry>);

impl Global for ActivityCache {}

/// A repository with commits on one day.
#[derive(Debug, PartialEq, Eq)]
struct DayEntry {
    path: PathBuf,
    name: String,
    count: u32,
}

pub struct WelcomeActivity {
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    /// The recent repositories, in list order.
    paths: Vec<PathBuf>,
    /// The repository selected in the recent list.
    selected: Option<PathBuf>,
    scope: Scope,
    me: Me,
    repos: HashMap<PathBuf, RepoState>,
    /// The newest "today" any counted repository reported.
    today: i32,
    generation: u64,
    me_task: Option<Task<()>>,
    tasks: Vec<Task<()>>,
}

impl WelcomeActivity {
    pub fn new(
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            git,
            workspace,
            paths: Vec::new(),
            selected: None,
            scope: Scope::All,
            me: Me::Loading,
            repos: HashMap::new(),
            today: 0,
            generation: 0,
            me_task: None,
            tasks: Vec::new(),
        };
        this.start(cx);
        this
    }

    /// Adopts a newly resolved git (after a Retry) and counts again.
    pub fn set_git(&mut self, git: Result<Arc<Git>, Arc<Error>>, cx: &mut Context<Self>) {
        self.git = git;
        self.start(cx);
        cx.notify();
    }

    /// The welcome view tells the card what the recent list holds and which
    /// row is selected; only a change repaints or counts.
    pub fn sync(&mut self, paths: Vec<PathBuf>, selected: Option<PathBuf>, cx: &mut Context<Self>) {
        let mut changed = false;
        if self.selected != selected {
            self.selected = selected;
            changed = true;
        }
        if self.paths != paths {
            self.paths = paths;
            let keep = &self.paths;
            self.repos.retain(|path, _| keep.contains(path));
            if matches!(&self.me, Me::Known(Some(_))) {
                self.load_repos(cx);
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Reads the email, then counts every recent repository.
    fn start(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.repos.clear();
        self.tasks.clear();
        self.today = 0;
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                log::debug!("the activity card has no git: {error}");
                self.me = Me::Known(None);
                self.me_task = None;
                return;
            }
        };
        self.me = Me::Loading;
        self.me_task = Some(cx.spawn(async move |this, cx| {
            let email = cx
                .background_executor()
                .spawn(async move { git_core::global_get(&git, "user.email") })
                .await;
            this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                let email = match email {
                    Ok(email) => email.filter(|email| !email.is_empty()),
                    Err(error) => {
                        log::warn!("could not read the global user.email: {error}");
                        None
                    }
                };
                view.me = Me::Known(email.clone());
                if email.is_some() {
                    view.load_repos(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Starts a count for every recent repository that has none yet.
    fn load_repos(&mut self, cx: &mut Context<Self>) {
        let (Me::Known(Some(me)), Ok(git)) = (&self.me, &self.git) else {
            return;
        };
        let (me, git) = (me.clone(), git.clone());
        let generation = self.generation;
        let missing: Vec<PathBuf> = self
            .paths
            .iter()
            .filter(|path| !self.repos.contains_key(*path))
            .cloned()
            .collect();
        for path in missing {
            self.repos.insert(path.clone(), RepoState::Loading);
            let cached = cx
                .try_global::<ActivityCache>()
                .and_then(|cache| cache.0.get(&path).cloned());
            let (git, me) = (git.clone(), me.clone());
            let task = cx.spawn(async move |this, cx| {
                let counted_path = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { count_repo(git, &counted_path, &me, cached.as_ref()) })
                    .await;
                this.update(cx, |view, cx| {
                    if view.generation != generation || !view.repos.contains_key(&path) {
                        return;
                    }
                    match result {
                        Ok(entry) => {
                            view.today = view.today.max(entry.today);
                            view.repos.insert(
                                path.clone(),
                                RepoState::Ready(entry.counts.clone(), entry.group.clone()),
                            );
                            cx.default_global::<ActivityCache>().0.insert(path, entry);
                        }
                        Err(reason) => {
                            log::info!("{} is skipped in the activity: {reason}", path.display());
                            view.repos.insert(path, RepoState::Skipped(reason));
                        }
                    }
                    cx.notify();
                })
                .ok();
            });
            self.tasks.push(task);
        }
    }

    fn email(&self) -> Option<&str> {
        match &self.me {
            Me::Known(Some(email)) => Some(email),
            _ => None,
        }
    }

    /// The scope actually drawn: "selected" needs a selected row.
    fn effective_scope(&self) -> Scope {
        if self.selected.is_some() {
            self.scope
        } else {
            Scope::All
        }
    }

    /// The days the heatmap draws for the current scope.
    fn shown_counts(&self) -> HashMap<i32, u32> {
        match (self.effective_scope(), &self.selected) {
            (Scope::Selected, Some(path)) => match self.repos.get(path) {
                Some(RepoState::Ready(counts, _)) => counts.as_ref().clone(),
                _ => HashMap::new(),
            },
            _ => sum_counts(
                representatives(&self.paths, &self.repos)
                    .into_iter()
                    .filter_map(|path| match self.repos.get(path) {
                        Some(RepoState::Ready(counts, _)) => Some(counts.as_ref()),
                        _ => None,
                    }),
            ),
        }
    }

    /// A click, Enter or Space on a counted day.
    fn pick_day(&mut self, day: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(me) = self.email().map(str::to_owned) else {
            return;
        };
        let entries = match (self.effective_scope(), &self.selected) {
            (Scope::Selected, Some(path)) => {
                day_entries(std::slice::from_ref(path), &self.repos, day)
            }
            _ => day_entries(&self.paths, &self.repos, day),
        };
        if entries.is_empty() {
            return;
        }
        let open = {
            let workspace = self.workspace.clone();
            move |path: PathBuf, window: &mut Window, cx: &mut App| {
                let filter = DayFilter {
                    day,
                    me: Some(me.clone()),
                };
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.open_repository_at_day(path, Some(filter), window, cx)
                    })
                    .ok();
            }
        };
        if self.effective_scope() == Scope::Selected {
            open(entries[0].path.clone(), window, cx);
            return;
        }
        // The menu opens at the pointer: the heatmap reports the day, not the
        // cell's bounds.
        let mut menu_entries = vec![MenuEntry::Item(
            MenuItem::new(ui::t!(
                "welcome.activity.day_menu_header",
                date = long_date(day)
            ))
            .disabled(true),
        )];
        for entry in entries {
            let open = open.clone();
            let path = entry.path;
            menu_entries.push(MenuEntry::Item(
                MenuItem::new(format!(
                    "{} · {}",
                    entry.name,
                    commits(entry.count as usize)
                ))
                .icon(IconName::Folder)
                .action(move |window, cx| open(path.clone(), window, cx)),
            ));
        }
        menu::open_context_menu(window.mouse_position(), menu_entries, window, cx);
    }

    fn open_identity(&self, window: &mut Window, cx: &mut App) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_settings_at(crate::settings_ui::Page::Identity, window, cx)
            })
            .ok();
    }
}

/// Sums per-day counts of several repositories.
fn sum_counts<'a>(repos: impl Iterator<Item = &'a HashMap<i32, u32>>) -> HashMap<i32, u32> {
    let mut total = HashMap::new();
    for counts in repos {
        for (&day, &count) in counts {
            *total.entry(day).or_insert(0) += count;
        }
    }
    total
}

/// The counted entries with linked worktrees of one repository collapsed to
/// the first of them: `paths` is most recently opened first, so a repository
/// shared by several recent entries is counted once and listed under the
/// newest one.
fn representatives<'a>(
    paths: &'a [PathBuf],
    repos: &HashMap<PathBuf, RepoState>,
) -> Vec<&'a PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .iter()
        .filter(|path| match repos.get(*path) {
            Some(RepoState::Ready(_, group)) => seen.insert(group.clone()),
            _ => false,
        })
        .collect()
}

/// The repositories with commits on `day`, most commits first (then by name).
fn day_entries(paths: &[PathBuf], repos: &HashMap<PathBuf, RepoState>, day: i32) -> Vec<DayEntry> {
    let mut entries: Vec<DayEntry> = representatives(paths, repos)
        .into_iter()
        .filter_map(|path| match repos.get(path) {
            Some(RepoState::Ready(counts, _)) => {
                let count = counts.get(&day).copied().filter(|&count| count > 0)?;
                Some(DayEntry {
                    path: path.clone(),
                    name: recent_name(path),
                    count,
                })
            }
            _ => None,
        })
        .collect();
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    entries
}

/// The skipped repositories with the reason, in list order.
fn skipped<'a>(
    paths: &'a [PathBuf],
    repos: &'a HashMap<PathBuf, RepoState>,
) -> Vec<(&'a PathBuf, &'a str)> {
    paths
        .iter()
        .filter_map(|path| match repos.get(path) {
            Some(RepoState::Skipped(reason)) => Some((path, reason.as_str())),
            _ => None,
        })
        .collect()
}

/// The mtime of the repository's `.git` and its `HEAD`, whichever is newer.
/// A commit or a checkout replaces files inside `.git`, which moves it.
fn head_stamp(path: &Path) -> Option<SystemTime> {
    let dot_git = path.join(".git");
    let dir = std::fs::metadata(&dot_git)
        .and_then(|m| m.modified())
        .ok()?;
    let head = std::fs::metadata(dot_git.join("HEAD"))
        .and_then(|m| m.modified())
        .ok();
    Some(head.map_or(dir, |head| head.max(dir)))
}

/// My commits per day over the last 12 months in one repository. Runs on the
/// background executor.
fn count_repo(
    git: Arc<Git>,
    path: &Path,
    me: &str,
    cached: Option<&CacheEntry>,
) -> Result<CacheEntry, String> {
    if !path.is_dir() {
        return Err(ui::t!("welcome.activity.folder_missing").to_string());
    }
    let stamp = head_stamp(path);
    if let Some(cached) = cached {
        if stamp.is_some() && cached.stamp == stamp && cached.me == me {
            return Ok(cached.clone());
        }
    }
    let repo = Repo::open(git, path).map_err(|e| e.to_string())?;
    let group = git_core::common_dir(&repo).map_err(|e| e.to_string())?;
    let today = git_core::today(&repo).map_err(|e| e.to_string())?;
    // Padded by a day on each side so no timezone can cut a day.
    let since = (i64::from(today - LAST_YEAR_DAYS) - 1) * 86_400;
    let until = (i64::from(today) + 2) * 86_400;
    let activity = git_core::activity(&repo, since, until, Some(me)).map_err(|e| e.to_string())?;
    let counts = activity
        .days
        .iter()
        .filter(|day| day.mine > 0)
        .map(|day| (day.day, day.mine))
        .collect();
    Ok(CacheEntry {
        stamp,
        me: me.to_owned(),
        group,
        today,
        counts: Arc::new(counts),
    })
}

impl Render for WelcomeActivity {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let scope = self.effective_scope();
        let home = home_dir();
        let me = self.email().map(str::to_owned);
        let me_loading = matches!(self.me, Me::Loading);

        // What the current scope covers, and how far along it is.
        let relevant: Vec<&PathBuf> = match (scope, &self.selected) {
            (Scope::Selected, Some(path)) => vec![path],
            _ => self.paths.iter().collect(),
        };
        let ready = relevant
            .iter()
            .filter(|path| matches!(self.repos.get(**path), Some(RepoState::Ready(..))))
            .count();
        let pending = me_loading
            || relevant
                .iter()
                .any(|path| matches!(self.repos.get(*path), None | Some(RepoState::Loading)));
        let unreachable = match (scope, &self.selected) {
            (Scope::Selected, Some(path)) => match self.repos.get(path) {
                Some(RepoState::Skipped(_)) => Some(path.clone()),
                _ => None,
            },
            _ => None,
        };

        let today = if self.today == 0 {
            // Nothing counted yet: the UTC day only places the skeleton.
            (crate::history_view::unix_now() / 86_400) as i32
        } else {
            self.today
        };
        let (start, end) = range_days(None, today);
        let counts = self.shown_counts();
        let in_range: Vec<u32> = counts
            .iter()
            .filter(|(&day, _)| (start..=end).contains(&day))
            .map(|(_, &count)| count)
            .collect();
        let total: usize = in_range.iter().map(|&count| count as usize).sum();
        let thresholds = git_core::levels(&in_range);
        let skeleton = pending && ready == 0;

        let strong = |text: String| {
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(text)
        };
        let line = || {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .whitespace_nowrap()
        };

        // The total next to the title.
        let head = if me.is_none() && !me_loading {
            None
        } else if unreachable.is_some() {
            Some(
                line()
                    .child(ui::t!("welcome.activity.unavailable"))
                    .into_any_element(),
            )
        } else if skeleton {
            Some(Skeleton::new("welcome-activity-total", px(180.)).into_any_element())
        } else if pending {
            Some(
                line()
                    .child(Icon::new(IconName::Fetch).size(px(12.)).color(colors.muted))
                    .child(div().flex().children(i18n::pieces_children(
                        i18n::t_count_pieces(
                            "welcome.activity.counting",
                            total as u64,
                            &[],
                            &["n"],
                        ),
                        vec![strong(format_count(total)).into_any_element()],
                    )))
                    .into_any_element(),
            )
        } else if total == 0 {
            Some(
                line()
                    .child(ui::t!("welcome.activity.none_last_year"))
                    .into_any_element(),
            )
        } else {
            Some(
                line()
                    .child(div().flex().children(i18n::pieces_children(
                        i18n::t_count_pieces(
                            "welcome.activity.commits_last_year",
                            total as u64,
                            &[],
                            &["n"],
                        ),
                        vec![strong(format_count(total)).into_any_element()],
                    )))
                    .into_any_element(),
            )
        };

        // The scope switch.
        let reachable = self
            .paths
            .iter()
            .filter(|path| !matches!(self.repos.get(*path), Some(RepoState::Skipped(_))))
            .count();
        let selected_name: SharedString = match &self.selected {
            Some(path) => recent_name(path).into(),
            None => ui::t!("welcome.activity.selected"),
        };
        let scope_switch = Segmented::new("welcome-activity-scope")
            .small()
            .item(
                SegmentItem::new(ui::t!("welcome.activity.all_recent"))
                    .id("welcome-activity-all")
                    .tooltip(ui::i18n::t_count(
                        "welcome.activity.all_tooltip",
                        reachable as u64,
                        &[],
                    )),
            )
            .item(
                SegmentItem::new(selected_name)
                    .id("welcome-activity-selected")
                    .disabled(self.selected.is_none())
                    .tooltip(ui::t!("welcome.activity.selected_tooltip")),
            )
            .selected(usize::from(scope == Scope::Selected))
            .on_select({
                let view = cx.entity();
                move |index, _, cx| {
                    view.update(cx, |this, cx| {
                        this.scope = if index == 0 {
                            Scope::All
                        } else {
                            Scope::Selected
                        };
                        cx.notify();
                    });
                }
            });

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_size(FS_LG)
                    .font_weight(FontWeight::BOLD)
                    .child(ui::t!("welcome.activity.title")),
            )
            .children(head)
            .child(div().flex_1())
            .child(scope_switch);

        // The panel's body: the grid, or a message in its place.
        let message = |icon: IconName, text: gpui::AnyElement| {
            div()
                .min_h(px(84.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(Icon::new(icon).size(px(16.)).color(colors.muted))
                .child(text)
        };
        let mono = |text: String| {
            div()
                .font_family(FONT_MONO)
                .text_color(colors.text)
                .child(text)
        };
        // One translated sentence with styled slots; the sentence supplies its own spaces.
        let sentence = |pieces, slots| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .children(i18n::pieces_children(pieces, slots))
        };
        let body = if me.is_none() && !me_loading {
            let view = cx.entity();
            message(
                IconName::User,
                line()
                    .whitespace_normal()
                    .flex_wrap()
                    .child(sentence(
                        i18n::t_pieces("welcome.activity.identity", &[], &["key"]),
                        vec![mono("user.email".to_owned()).into_any_element()],
                    ))
                    .child(
                        Button::new("welcome-activity-identity")
                            .size(ButtonSize::Small)
                            .label(ui::t!("welcome.activity.set_identity"))
                            .on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| this.open_identity(window, cx));
                            }),
                    )
                    .into_any_element(),
            )
            .into_any_element()
        } else if let Some(path) = &unreachable {
            message(
                IconName::Warn,
                line()
                    .whitespace_normal()
                    .flex_wrap()
                    .child(sentence(
                        i18n::t_pieces("welcome.activity.unreachable", &[], &["path"]),
                        vec![mono(display_path(path, home.as_deref())).into_any_element()],
                    ))
                    .into_any_element(),
            )
            .into_any_element()
        } else {
            let view = cx.entity();
            let heat_counts = counts;
            let heatmap = Heatmap::new("welcome-activity-heatmap", end, WEEKS, move |day| {
                let count = heat_counts.get(&day).copied().unwrap_or(0);
                (count, git_core::level(count, &thresholds))
            })
            .compact(true)
            .loading(skeleton)
            .on_select(move |day, window, cx| {
                view.update(cx, |this, cx| this.pick_day(day, window, cx));
            });
            div()
                .id("welcome-activity-scroll")
                .min_w_0()
                .overflow_x_scroll()
                .child(heatmap)
                .into_any_element()
        };

        // The footer: who, how many repositories, the legend.
        let footer_left =
            if let Some(me) = me.as_ref().filter(|_| !skeleton && unreachable.is_none()) {
                let mut left = line().child(sentence(
                    i18n::t_pieces("welcome.activity.by", &[], &["email"]),
                    vec![mono(me.clone()).into_any_element()],
                ));
                if scope == Scope::All && !pending {
                    let skipped = skipped(&self.paths, &self.repos);
                    let counted = self.paths.len() - skipped.len();
                    if skipped.is_empty() {
                        left = left.child(ui::i18n::t_count(
                            "welcome.activity.repo_count",
                            self.paths.len() as u64,
                            &[],
                        ));
                    } else {
                        let list = skipped
                            .iter()
                            .map(|(path, reason)| {
                                format!("{} — {reason}", display_path(path, home.as_deref()))
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        let tip = ui::i18n::t_count(
                            "welcome.activity.skipped_tip",
                            skipped.len() as u64,
                            &[("list", &list)],
                        );
                        left = left
                            .child(ui::i18n::t_count(
                                "welcome.activity.counted_of",
                                self.paths.len() as u64,
                                &[("counted", &counted)],
                            ))
                            .child(
                                div()
                                    .id("welcome-activity-skipped")
                                    .flex()
                                    .items_center()
                                    .tooltip(Tooltip::text(tip))
                                    .child(
                                        Icon::new(IconName::Warn)
                                            .size(px(12.))
                                            .color(colors.warning),
                                    ),
                            );
                    }
                }
                if !pending && total == 0 && unreachable.is_none() {
                    left = line()
                        .child(ui::t!("welcome.activity.no_commits_by"))
                        .child(mono(me.clone()));
                }
                Some(left.into_any_element())
            } else {
                None
            };
        let show_legend = !(me.is_none() && !me_loading) && unreachable.is_none();
        let footer = div()
            .flex()
            .items_center()
            .gap(SP_4)
            .children(footer_left)
            .child(div().flex_1())
            .when(show_legend, |this| {
                this.child(HeatLegend::new().compact(true))
            });

        let panel = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SP_4)
            .p(SP_5)
            .bg(colors.bg)
            .border_1()
            .border_color(colors.border)
            .rounded(R_MD)
            .child(body)
            .child(footer);

        div()
            .id("welcome-activity")
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(header)
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(i32, u32)]) -> Arc<HashMap<i32, u32>> {
        Arc::new(pairs.iter().copied().collect())
    }

    /// A counted repository of its own (every call gets a fresh common dir).
    fn ready(pairs: &[(i32, u32)]) -> RepoState {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let group = PathBuf::from(format!("/g/{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        RepoState::Ready(counts(pairs), group)
    }

    fn states(items: &[(&str, RepoState)]) -> (Vec<PathBuf>, HashMap<PathBuf, RepoState>) {
        let paths = items.iter().map(|(p, _)| PathBuf::from(p)).collect();
        let repos = items
            .iter()
            .map(|(p, s)| (PathBuf::from(p), s.clone()))
            .collect();
        (paths, repos)
    }

    #[test]
    fn counts_are_summed_per_day_across_repositories() {
        let a = counts(&[(10, 2), (11, 1)]);
        let b = counts(&[(11, 4), (12, 3)]);
        let total = sum_counts([a.as_ref(), b.as_ref()].into_iter());
        assert_eq!(total, HashMap::from([(10, 2), (11, 5), (12, 3)]));
        assert!(sum_counts(std::iter::empty()).is_empty());
    }

    #[test]
    fn skipped_repositories_are_listed_in_recent_order() {
        let (paths, repos) = states(&[
            ("/r/one", ready(&[(1, 1)])),
            (
                "/r/gone",
                RepoState::Skipped("the folder does not exist".into()),
            ),
            ("/r/busy", RepoState::Loading),
            ("/r/bad", RepoState::Skipped("not a git repository".into())),
        ]);
        let skipped = skipped(&paths, &repos);
        let names: Vec<&str> = skipped.iter().map(|(p, _)| p.to_str().unwrap()).collect();
        assert_eq!(names, ["/r/gone", "/r/bad"]);
        assert_eq!(skipped[0].1, "the folder does not exist");
    }

    #[test]
    fn the_menu_lists_repositories_with_commits_that_day() {
        let (paths, repos) = states(&[
            ("/r/zed", ready(&[(5, 2), (6, 1)])),
            ("/r/alpha", ready(&[(5, 2)])),
            ("/r/big", ready(&[(5, 7)])),
            ("/r/other-day", ready(&[(6, 3)])),
            ("/r/gone", RepoState::Skipped("x".into())),
            ("/r/busy", RepoState::Loading),
        ]);
        let entries = day_entries(&paths, &repos, 5);
        let shown: Vec<(&str, u32)> = entries.iter().map(|e| (e.name.as_str(), e.count)).collect();
        // Most commits first, ties by name; skipped and loading ones never appear.
        assert_eq!(shown, [("big", 7), ("alpha", 2), ("zed", 2)]);
        assert_eq!(entries[0].path, PathBuf::from("/r/big"));
        assert!(day_entries(&paths, &repos, 99).is_empty());
    }

    fn commit(dir: &Path, email: &str) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                &format!("user.email={email}"),
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git is installed");
        assert!(status.success());
    }

    #[test]
    fn a_repository_is_counted_for_me_only_and_a_missing_one_is_skipped() {
        let repo = crate::test_support::TempRepo::new("welcome-activity");
        commit(repo.path(), "me@x.io");
        commit(repo.path(), "me@x.io");
        commit(repo.path(), "other@x.io");

        let git = crate::test_support::git();
        let entry = count_repo(git.clone(), repo.path(), "ME@x.io", None).unwrap();
        assert_eq!(entry.counts.values().sum::<u32>(), 2);

        // The same stamp and email is answered from the cache, whatever the
        // folder holds now.
        let again = count_repo(git.clone(), repo.path(), "ME@x.io", Some(&entry)).unwrap();
        assert!(Arc::ptr_eq(&entry.counts, &again.counts));
        let nobody = count_repo(git.clone(), repo.path(), "nobody@x.io", Some(&entry)).unwrap();
        assert!(nobody.counts.is_empty());

        let gone = repo.path().join("does-not-exist");
        assert!(count_repo(git, &gone, "me@x.io", None).is_err());
    }

    #[test]
    fn worktrees_of_one_repository_are_counted_once() {
        let shared = counts(&[(5, 3), (6, 1)]);
        let group = PathBuf::from("/g/shared");
        let (paths, repos) = states(&[
            // Most recently opened first: the group is listed under `newer`.
            ("/w/newer", RepoState::Ready(shared.clone(), group.clone())),
            ("/w/older", RepoState::Ready(shared, group)),
            ("/w/solo", ready(&[(5, 2)])),
        ]);
        let total =
            sum_counts(representatives(&paths, &repos).into_iter().filter_map(
                |p| match repos.get(p) {
                    Some(RepoState::Ready(c, _)) => Some(c.as_ref()),
                    _ => None,
                },
            ));
        assert_eq!(total, HashMap::from([(5, 5), (6, 1)]));
        let entries = day_entries(&paths, &repos, 5);
        let shown: Vec<(&str, u32)> = entries.iter().map(|e| (e.name.as_str(), e.count)).collect();
        assert_eq!(shown, [("newer", 3), ("solo", 2)]);
        assert_eq!(entries[0].path, PathBuf::from("/w/newer"));
    }

    #[test]
    fn a_real_worktree_falls_into_its_repositorys_group() {
        let repo = crate::test_support::TempRepo::new("welcome-activity-wt");
        commit(repo.path(), "me@x.io");
        let linked = repo.path().with_extension("linked");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["worktree", "add", "-q", "-b", "side"])
            .arg(&linked)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git is installed");
        assert!(status.success());

        let git = crate::test_support::git();
        let a = count_repo(git.clone(), repo.path(), "me@x.io", None).unwrap();
        let b = count_repo(git, &linked, "me@x.io", None).unwrap();
        assert_eq!(a.group, b.group);
        assert_eq!(a.counts, b.counts);

        let paths = vec![linked.clone(), repo.path().to_path_buf()];
        let repos = HashMap::from([
            (linked.clone(), RepoState::Ready(b.counts.clone(), b.group)),
            (
                repo.path().to_path_buf(),
                RepoState::Ready(a.counts.clone(), a.group),
            ),
        ]);
        assert_eq!(representatives(&paths, &repos), vec![&linked]);
        let _ = std::fs::remove_dir_all(&linked);
    }
}
