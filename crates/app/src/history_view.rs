//! The history screen: column header + commit graph and commit list, loaded in
//! the background and painted through a virtualized list (only visible rows are
//! built).
//!
//! The view also owns the repository metadata the shell needs — HEAD and the
//! refs — because it is the view that opens the [`Repo`].

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    canvas, container_query, div, px, uniform_list, AnyElement, AsyncApp, ClipboardItem,
    ContentMask, Context, Div, FontWeight, IntoElement, Pixels, Render, SharedString, StyledText,
    Task, TextRun, TextStyle, WeakEntity, Window,
};

use git_core::{CommitInfo, Error, Git, Head, LogOptions, LogStream, Oid, Ref, Repo};
use git_graph::{GraphBuilder, Row};
use ui::metrics::{FONT_MONO, FONT_UI, FS_SM};
use ui::theme::ThemeColors;
use ui::{
    ActiveTheme, Button, ButtonSize, EmptyState, Icon, IconName, Progress, Skeleton, Spinner,
};

use crate::format::format_count;
use crate::graph_paint::{graph_width, paint_row};
use crate::history_state::{
    percent_text, progress_fraction, skeleton_percent, source_label, Failure, LoadState,
    SkeletonColumn,
};
use crate::theme;
use crate::workspace::Workspace;

/// Rows are delivered to the UI in pages of this size.
const PAGE: usize = 2_000;

/// Column widths of the history table, shared by the header and the rows.
const COL_GAP: f32 = 18.;
/// Author and date columns on a wide pane.
const COL_AUTHOR: f32 = 130.;
const COL_DATE: f32 = 104.;
/// The narrower author and date columns below [`PANE_MEDIUM`].
const COL_AUTHOR_NARROW: f32 = 104.;
const COL_DATE_NARROW: f32 = 76.;
const COL_HASH: f32 = 64.;
/// The pane widths where the table drops a column, as the design's container
/// queries do: 560 px loses Hash, 460 px loses Author.
const PANE_MEDIUM: f32 = 560.;
const PANE_NARROW: f32 = 460.;
/// The graph column's design range: at most 480 px on a wide pane and 112 px
/// from 560 px down; on top of that it never takes more than a share of the
/// pane, so the summary always keeps room.
const GRAPH_MAX_WIDE: f32 = 480.;
const GRAPH_MAX_NARROW: f32 = 112.;
const GRAPH_SHARE_WIDE: f32 = 0.40;
const GRAPH_SHARE_MEDIUM: f32 = 0.40;
const GRAPH_SHARE_NARROW: f32 = 0.36;
/// The header and the rows share this side margin and padding.
const SIDE_MARGIN: f32 = 6.;
const PAD_LEFT: f32 = 14.;
const PAD_RIGHT: f32 = 18.;
/// The graph column never gets narrower than this, even with one lane.
const GRAPH_MIN: f32 = 40.;
/// The summary column is flexible; its skeleton bars are percentages of this.
const SKELETON_SUMMARY_REF: f32 = 360.;
/// Skeleton rows shown until the first page of history arrives.
const SKELETON_ROWS: usize = 10;

type Page = Result<Vec<(CommitInfo, Row<Oid>)>, Failure>;

/// The history table's columns for one pane width, shared by the header, the
/// rows and the skeleton rows so the three stay aligned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    /// Width of the graph column.
    pub graph: f32,
    /// Width of the author column; `None` once the pane is too narrow.
    pub author: Option<f32>,
    /// Width of the date column.
    pub date: f32,
    /// Width of the hash column; `None` once the pane is too narrow.
    pub hash: Option<f32>,
}

/// The table layout for a history pane `pane_width` wide whose graph needs
/// `lanes` lanes.
///
/// This is the design's container-query table: the graph column shrinks to a
/// share of the pane (and to 112 px from 560 px down), the hash column goes at
/// 560 px and the author column at 460 px.
pub fn columns(pane_width: f32, lanes: usize) -> Columns {
    let lanes_width = f32::from(graph_width(lanes)).max(GRAPH_MIN);
    let (max, share) = if pane_width <= PANE_NARROW {
        (GRAPH_MAX_NARROW, GRAPH_SHARE_NARROW)
    } else if pane_width <= PANE_MEDIUM {
        (GRAPH_MAX_NARROW, GRAPH_SHARE_MEDIUM)
    } else {
        (GRAPH_MAX_WIDE, GRAPH_SHARE_WIDE)
    };
    let graph = lanes_width.min(max).min(share * pane_width).max(GRAPH_MIN);

    if pane_width <= PANE_NARROW {
        Columns {
            graph,
            author: None,
            date: COL_DATE_NARROW,
            hash: None,
        }
    } else if pane_width <= PANE_MEDIUM {
        Columns {
            graph,
            author: Some(COL_AUTHOR_NARROW),
            date: COL_DATE_NARROW,
            hash: None,
        }
    } else {
        Columns {
            graph,
            author: Some(COL_AUTHOR),
            date: COL_DATE,
            hash: Some(COL_HASH),
        }
    }
}

struct Item {
    commit: CommitInfo,
    row: Rc<Row<Oid>>,
}

/// What the sidebar knows about the repository's refs.
#[derive(Debug, Default)]
enum RefsState {
    #[default]
    Loading,
    Loaded(Vec<Ref>),
    Failed,
}

pub struct HistoryView {
    /// The path the view was opened for; retried as-is.
    path: PathBuf,
    /// The resolved git, or why there is none. Retrying the resolution is the
    /// workspace's job; this is the handle for this repository.
    git: Result<Arc<Git>, Arc<Error>>,
    workspace: WeakEntity<Workspace>,
    /// The repository root, once `Repo::open` succeeded.
    root: Option<PathBuf>,
    items: Vec<Item>,
    max_lanes: usize,
    selected: Option<usize>,
    state: LoadState,
    /// Bumped for every load; results from an older load are dropped.
    generation: u64,
    /// The running load. Dropping it cancels the page task and the git log.
    task: Option<Task<()>>,
    /// The `commit_count` probe that feeds the load bar; cancelled with the
    /// load.
    count_task: Option<Task<()>>,
    /// `None` until the background read finishes; `Err` leaves the toolbar
    /// showing the placeholder.
    head: Option<Result<Head, String>>,
    refs: RefsState,
    /// The table layout for the pane's last measured width, so the virtualized
    /// rows paint with the same columns the header was built for.
    columns: Columns,
}

impl HistoryView {
    /// Starts in the loading state; opening the repository, reading HEAD, the
    /// refs and the history all run in the background and fill in the rows or
    /// the failure as they arrive. `git` is resolved once per process by the
    /// caller.
    pub fn open(
        path: PathBuf,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            path,
            git,
            workspace,
            root: None,
            items: Vec::new(),
            max_lanes: 1,
            selected: None,
            state: LoadState::Loading { total: None },
            generation: 0,
            task: None,
            count_task: None,
            head: None,
            refs: RefsState::Loading,
            // Replaced with the pane's real width in the first prepaint.
            columns: columns(1200., 1),
        };
        view.start(cx);
        view
    }

    /// Start a fresh load: a new task, a new generation and the loading state.
    fn start(&mut self, cx: &mut Context<Self>) {
        // Dropping the old tasks stops their git processes before new ones
        // start.
        self.task = None;
        self.count_task = None;
        self.generation += 1;
        let generation = self.generation;
        self.state = LoadState::Loading { total: None };

        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                self.state = LoadState::Failed(Failure::from_error(error));
                return;
            }
        };
        let path = self.path.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = Self::run(git, path, generation, &this, cx).await;
            this.update(cx, |view, cx| view.finish(generation, result, cx))
                .ok();
        }));
    }

    /// Open the repository and stream its history into the view. A closed
    /// window just ends the task with `Ok`.
    async fn run(
        git: Arc<Git>,
        path: PathBuf,
        generation: u64,
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) -> Result<(), Failure> {
        let executor = cx.background_executor().clone();
        let repo = executor
            .spawn(async move { Repo::open(git, &path) })
            .await
            .map_err(|e| Failure::from_error(&e))?;

        // The repository opened: remember it for the switcher, whatever the
        // history walk does next.
        let root = repo.root().to_path_buf();
        cx.update(|cx| {
            crate::update_settings(cx, |settings| {
                settings.touch_recent(root.clone(), unix_now());
            });
        });

        // HEAD and the refs feed the toolbar and the sidebar; read them in the
        // background so the log is not held back.
        let meta_repo = repo.clone();
        let meta_root = root.clone();
        let meta_this = this.clone();
        cx.spawn(async move |cx| {
            let (head, refs) = cx
                .background_executor()
                .spawn(async move { (meta_repo.head(), meta_repo.refs()) })
                .await;
            meta_this
                .update(cx, |view, cx| view.set_meta(meta_root, head, refs, cx))
                .ok();
        })
        .detach();

        // The count feeds the load bar's percentage; it runs beside the log so
        // the first page is not delayed. A failure only loses the percentage.
        // The view keeps the handle so cancelling the load cancels the count.
        let counter = repo.clone();
        let count_this = this.clone();
        let count_task = cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move { counter.commit_count() })
                .await;
            match result {
                Ok(total) => {
                    count_this
                        .update(cx, |view, cx| view.set_total(generation, total, cx))
                        .ok();
                }
                Err(e) => log::warn!("could not count the commits: {e}"),
            }
        });
        this.update(cx, |view, _cx| view.count_task = Some(count_task))
            .ok();

        // Without a commit-graph file the first screen waits for the whole
        // history walk. Write one for next time, in parallel with this load.
        let writer = repo.clone();
        executor
            .spawn(async move {
                if !writer.has_commit_graph() {
                    if let Err(e) = writer.write_commit_graph() {
                        log::warn!("could not write the commit-graph: {e}");
                    }
                }
            })
            .detach();

        // Both the git process and the layout run off the UI thread; the
        // UI thread only appends finished pages and repaints.
        let stream = executor
            .spawn(async move { repo.log(&LogOptions::default()) })
            .await
            .map_err(|e| Failure::from_error(&e))?;
        let mut state = Some((stream, GraphBuilder::new()));
        while let Some((stream, graph)) = state.take() {
            let (stream, graph, page) = executor
                .spawn(async move { next_page(stream, graph) })
                .await;
            let page = page?;
            if page.is_empty() {
                break;
            }
            if this.update(cx, |view, cx| view.append(page, cx)).is_err() {
                return Ok(()); // the window was closed: stop reading
            }
            state = Some((stream, graph));
        }
        Ok(())
    }

    fn append(&mut self, page: Vec<(CommitInfo, Row<Oid>)>, cx: &mut Context<Self>) {
        for (commit, row) in page {
            self.max_lanes = self.max_lanes.max(row.width);
            self.items.push(Item {
                commit,
                row: Rc::new(row),
            });
        }
        cx.notify();
    }

    /// The commit count arrived; only the current load may set it.
    fn set_total(&mut self, generation: u64, total: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        if let LoadState::Loading { total: slot } = &mut self.state {
            *slot = Some(total);
            cx.notify();
        }
    }

    fn set_meta(
        &mut self,
        root: PathBuf,
        head: Result<Head, Error>,
        refs: Result<Vec<Ref>, Error>,
        cx: &mut Context<Self>,
    ) {
        self.root = Some(root);
        match head {
            Ok(head) => self.head = Some(Ok(head)),
            Err(e) => {
                log::warn!("could not read HEAD: {e}");
                self.head = Some(Err(e.to_string()));
            }
        }
        self.refs = match refs {
            Ok(refs) => RefsState::Loaded(refs),
            Err(e) => {
                log::warn!("could not read the refs: {e}");
                RefsState::Failed
            }
        };
        cx.notify();
    }

    fn finish(&mut self, generation: u64, result: Result<(), Failure>, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.state = match result {
            Ok(()) => LoadState::Loaded,
            Err(failure) => LoadState::Failed(failure),
        };
        cx.notify();
    }

    /// Stop the load; the rows that arrived stay on screen.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_loading() {
            return;
        }
        self.task = None;
        self.count_task = None;
        self.state = LoadState::Cancelled;
        cx.notify();
    }

    /// Start over: drop the rows and the graph, then load again.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.items.clear();
        self.max_lanes = 1;
        self.selected = None;
        self.start(cx);
        cx.notify();
    }

    /// `git init` the folder, then load the new repository.
    fn init_repository(&mut self, cx: &mut Context<Self>) {
        let Ok(git) = self.git.clone() else {
            return;
        };
        let path = self.path.clone();
        self.task = None;
        self.count_task = None;
        self.generation += 1;
        self.state = LoadState::Loading { total: None };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Repo::init(git, &path) })
                .await;
            this.update(cx, |view, cx| match result {
                Ok(_) => view.retry(cx),
                Err(error) => {
                    view.state = LoadState::Failed(Failure::from_error(&error));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The repository root, once `Repo::open` succeeded.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// HEAD, once the background read finished and succeeded.
    pub fn head(&self) -> Option<&Head> {
        self.head.as_ref().and_then(|head| head.as_ref().ok())
    }

    /// The refs, once loaded; `None` while loading or after a failure.
    pub fn refs(&self) -> Option<&[Ref]> {
        match &self.refs {
            RefsState::Loaded(refs) => Some(refs),
            RefsState::Loading | RefsState::Failed => None,
        }
    }

    /// What the centre pane is showing, for the status bar.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Whether the history is still streaming in.
    pub fn is_loading(&self) -> bool {
        matches!(self.state, LoadState::Loading { .. })
    }

    /// How many commits have arrived so far.
    pub fn commit_count(&self) -> usize {
        self.items.len()
    }

    /// The short hash of the selected commit, for the details header.
    pub fn selected_short(&self) -> Option<SharedString> {
        self.selected
            .and_then(|ix| self.items.get(ix))
            .map(|item| item.commit.id.short().into())
    }

    fn render_header(&self, columns: Columns, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        // A 6px gutter around the header, so it lines up with the rows.
        let mut head = div()
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pl(px(PAD_LEFT))
            .pr(px(PAD_RIGHT))
            .flex()
            .items_center()
            .gap(px(COL_GAP))
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.muted)
            .child(div().w(px(columns.graph)).flex_none().child("Graph"))
            .child(div().flex_1().min_w_0().truncate().child("Summary"));
        if let Some(author) = columns.author {
            head = head.child(div().w(px(author)).flex_none().child("Author"));
        }
        head = head.child(div().w(px(columns.date)).flex_none().child("Date"));
        if let Some(hash) = columns.hash {
            head = head.child(div().w(px(hash)).flex_none().child("Hash"));
        }
        div().w_full().px(px(SIDE_MARGIN)).child(head)
    }

    /// The one-line load bar under the header.
    fn render_load_bar(&self, loaded: u64, total: Option<u64>, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let progress = match progress_fraction(loaded, total) {
            Some(value) => Progress::new("history-progress", value).fill(),
            None => Progress::new("history-progress", 0.).indeterminate().fill(),
        };
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(8.))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .child(Spinner::new("history-spinner", px(14.)))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text)
                    .child(format!(
                        "Loading… {} commits",
                        format_count(loaded as usize)
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(progress),
            )
            .when_some(percent_text(loaded, total), |this, percent| {
                this.child(div().flex_none().text_color(colors.muted).child(percent))
            })
            .child(
                Button::new("history-cancel")
                    .label("Cancel")
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
            )
    }

    /// Ten placeholder rows while the first page of history is on its way.
    fn render_skeletons(&self, columns: Columns, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        div()
            .w_full()
            .flex()
            .flex_col()
            .children((0..SKELETON_ROWS).map(|index| {
                let mut row = div()
                    .w_full()
                    .h(theme::ROW_HEIGHT)
                    .pl(px(PAD_LEFT))
                    .pr(px(PAD_RIGHT))
                    .flex()
                    .items_center()
                    .gap(px(COL_GAP))
                    .child(graph_skeleton(px(columns.graph), colors))
                    // The summary bar is a percentage of a fixed reference
                    // width; clip it so a narrow pane's bar cannot run into
                    // the author column.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(Skeleton::new(
                                format!("history-skeleton-summary-{index}"),
                                skeleton_width(SkeletonColumn::Summary, index, columns),
                            )),
                    );
                if let Some(author) = columns.author {
                    row = row.child(div().w(px(author)).flex_none().child(Skeleton::new(
                        format!("history-skeleton-author-{index}"),
                        skeleton_width(SkeletonColumn::Author, index, columns),
                    )));
                }
                row = row.child(div().w(px(columns.date)).flex_none().child(Skeleton::new(
                    format!("history-skeleton-date-{index}"),
                    skeleton_width(SkeletonColumn::Date, index, columns),
                )));
                if let Some(hash) = columns.hash {
                    row = row.child(div().w(px(hash)).flex_none().child(Skeleton::new(
                        format!("history-skeleton-hash-{index}"),
                        skeleton_width(SkeletonColumn::Hash, index, columns),
                    )));
                }
                div().w_full().px(px(SIDE_MARGIN)).child(row)
            }))
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        let colors = cx.theme().colors;
        let ui_theme = cx.theme().clone();
        let columns = self.columns;
        range
            .map(|ix| {
                let item = &self.items[ix];
                let row = item.row.clone();
                let selected = self.selected == Some(ix);
                let c = &item.commit;
                let background = if selected {
                    colors.selection
                } else {
                    colors.bg
                };
                let row_theme = ui_theme.clone();

                // The 6px gutter lives on the wrapper: a uniform_list lays
                // each item out with a definite width and does not stretch it,
                // so margins on the row itself would overflow.
                let mut row_div = div()
                    .id(ix)
                    .h(theme::ROW_HEIGHT)
                    .w_full()
                    .pl(px(PAD_LEFT))
                    .pr(px(PAD_RIGHT))
                    .flex()
                    .items_center()
                    .gap(px(COL_GAP))
                    .text_size(FS_SM)
                    .when(selected, |d| d.bg(colors.selection))
                    .when(!selected, |d| d.hover(|d| d.bg(colors.hover)))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.selected = Some(ix);
                        cx.notify();
                    }))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                // Clip to the graph column so lanes past its
                                // edge are cut instead of drawn over Summary.
                                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                    paint_row(&row, bounds, background, &row_theme, window)
                                })
                            },
                        )
                        .w(px(columns.graph))
                        .h_full()
                        .flex_none(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.text)
                            .child(c.summary.clone()),
                    );
                if let Some(author) = columns.author {
                    row_div = row_div.child(
                        div()
                            .w(px(author))
                            .flex_none()
                            .truncate()
                            .text_color(colors.muted)
                            .child(c.author_name.clone()),
                    );
                }
                row_div = row_div.child(
                    div()
                        .w(px(columns.date))
                        .flex_none()
                        .truncate()
                        .text_color(colors.muted)
                        .child(format_time(c.author_time)),
                );
                if let Some(hash) = columns.hash {
                    row_div = row_div.child(
                        div()
                            .w(px(hash))
                            .flex_none()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_color(colors.muted)
                            .child(c.id.short()),
                    );
                }
                div().w_full().px(px(SIDE_MARGIN)).child(row_div)
            })
            .collect()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        uniform_list(
            "history",
            self.items.len(),
            cx.processor(|view, range, _window, cx| view.render_rows(range, cx)),
        )
        .flex_1()
        .min_h_0()
        .w_full()
        .into_any_element()
    }

    fn render_body(&mut self, columns: Columns, cx: &mut Context<Self>) -> AnyElement {
        // The state is small and cloned so the render helpers can take `&mut
        // self` for their click listeners.
        let state = self.state.clone();
        match state {
            LoadState::Loading { total } => {
                let loaded = self.items.len() as u64;
                let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
                column = column.child(self.render_load_bar(loaded, total, cx));
                if self.items.is_empty() {
                    column
                        .child(self.render_skeletons(columns, cx))
                        .into_any_element()
                } else {
                    column.child(self.render_list(cx)).into_any_element()
                }
            }
            LoadState::Loaded => {
                if self.items.is_empty() {
                    self.render_no_commits(cx)
                } else {
                    self.render_list(cx)
                }
            }
            LoadState::Cancelled => {
                if self.items.is_empty() {
                    self.render_cancelled(cx)
                } else {
                    self.render_list(cx)
                }
            }
            LoadState::Failed(failure) => {
                if self.items.is_empty() {
                    self.render_failure(&failure, cx)
                } else {
                    self.render_incomplete(&failure, cx)
                }
            }
        }
    }

    /// Empty repository: an invitation to make the first commit.
    fn render_no_commits(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        let body = BodyText::new()
            .plain("This repository is empty. Stage some files and make your first commit to see history here.");
        let actions = div()
            .flex()
            .gap(px(8.))
            .child(
                Button::primary("empty-no-commits-primary")
                    .label("Go to Local changes")
                    .disabled(true)
                    .tooltip("Local changes arrive in a later version"),
            )
            .child(
                Button::new("empty-no-commits-secondary")
                    .label("Add .gitignore…")
                    .disabled(true)
                    .tooltip("Local changes arrive in a later version"),
            );
        centred(
            EmptyState::new("No commits yet")
                .icon(IconName::Git)
                .icon_color(colors.muted)
                .body(body.element(&style))
                .actions(actions),
        )
    }

    /// The user stopped the load before it finished.
    fn render_cancelled(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let actions = div().flex().gap(px(8.)).child(
            Button::primary("empty-cancelled-retry")
                .label("Retry")
                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
        );
        centred(
            EmptyState::new("Loading cancelled")
                .icon(IconName::Fetch)
                .icon_color(cx.theme().colors.muted)
                .text("The history was not loaded completely.")
                .actions(actions),
        )
    }

    fn render_failure(&mut self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let style = body_style(colors);
        match failure {
            Failure::NotARepository(path) => {
                let path = path.display().to_string();
                let body = BodyText::new()
                    .code(&path)
                    .plain(" is not inside a git working tree.");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-not-a-repo-init")
                            .icon(IconName::Init)
                            .label("Initialize repository here")
                            .on_click(cx.listener(|this, _, _, cx| this.init_repository(cx))),
                    )
                    .child(
                        Button::new("empty-not-a-repo-open")
                            .label("Open another…")
                            .disabled(true)
                            .tooltip("Coming soon"),
                    );
                centred(
                    EmptyState::new("Not a git repository")
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::PathNotFound(path) => {
                let path = path.display().to_string();
                let body = BodyText::new().code(&path).plain(" does not exist.");
                let actions = div().flex().gap(px(8.)).child(
                    Button::primary("empty-path-missing-retry")
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                );
                centred(
                    EmptyState::new("Folder not found")
                        .icon(IconName::Folder)
                        .icon_color(colors.muted)
                        .body(body.element(&style))
                        .actions(actions),
                )
            }
            Failure::GitNotFound => {
                let body = BodyText::new()
                    .plain("GitWarp could not find a usable git. The bundled copy is missing and there is no ")
                    .code("git")
                    .plain(" in PATH.");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-not-found-settings")
                            .icon(IconName::Gear)
                            .label("Open Git settings")
                            .disabled(true)
                            .tooltip("Settings arrive in a later version"),
                    )
                    .child(self.retry_git_button("empty-git-not-found-retry", cx));
                centred(
                    EmptyState::new("git was not found")
                        .body(body.element(&style))
                        .icon(IconName::Err)
                        .icon_color(colors.error)
                        .actions(actions),
                )
            }
            Failure::GitTooOld {
                found,
                required,
                source,
                path,
            } => {
                let body = BodyText::new()
                    .plain("GitWarp needs git ")
                    .plain(&required.to_string())
                    .plain(" or newer. ")
                    .plain(source_label(*source))
                    .plain(" git at ")
                    .code(&path.display().to_string())
                    .plain(" reports ")
                    .plain(&found.to_string())
                    .plain(".");
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-too-old-settings")
                            .icon(IconName::Gear)
                            .label("Open Git settings")
                            .disabled(true)
                            .tooltip("Settings arrive in a later version"),
                    )
                    .child(self.retry_git_button("empty-git-too-old-retry", cx));
                centred(
                    EmptyState::new(failure.title())
                        .body(body.element(&style))
                        .icon(IconName::Warn)
                        .icon_color(colors.warning)
                        .actions(actions),
                )
            }
            Failure::Git {
                command,
                code,
                stderr,
            } => {
                let body = BodyText::new()
                    .plain("The command ")
                    .code(&format!("git {command}"));
                let body = match code {
                    Some(code) => body.plain(&format!(" failed with exit code {code}.")),
                    None => body.plain(" was stopped."),
                };
                let actions = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::primary("empty-git-error-retry")
                            .icon(IconName::Fetch)
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("empty-git-error-copy")
                            .icon(IconName::Copy)
                            .label("Copy details")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let LoadState::Failed(failure) = &this.state {
                                    if let Some(details) = failure.copy_details() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(details));
                                    }
                                }
                            })),
                    );
                let state = EmptyState::new(failure.title())
                    .body(body.element(&style))
                    .icon(IconName::Err)
                    .icon_color(colors.error)
                    .actions(actions);
                // Git always says something on stderr; an empty box would only
                // be noise for a process that was killed.
                let state = if stderr.is_empty() {
                    state
                } else {
                    state.error(stderr.clone())
                };
                centred(state)
            }
        }
    }

    /// Retry through the workspace: it resolves git again and reopens the
    /// repository with the new handle.
    fn retry_git_button(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        let workspace = self.workspace.clone();
        Button::new(id)
            .label("Retry")
            .on_click(cx.listener(move |_, _, window, cx| {
                workspace
                    .update(cx, |workspace, cx| workspace.retry_git(window, cx))
                    .ok();
            }))
    }

    /// The log broke after rows had arrived: keep the rows and say what
    /// happened in a strip under the header.
    fn render_incomplete(&mut self, failure: &Failure, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div().flex_1().min_h_0().w_full().flex().flex_col();
        if let Failure::Git { command, .. } = failure {
            let colors = cx.theme().colors;
            column = column.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(6.))
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(Icon::new(IconName::Err).size(px(14.)).color(colors.error))
                    .child(format!("History is incomplete: git {command} failed"))
                    .child(div().flex_1())
                    .child(
                        Button::new("history-incomplete-retry")
                            .label("Retry")
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                    .child(
                        Button::new("history-incomplete-copy")
                            .label("Copy details")
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let LoadState::Failed(failure) = &this.state {
                                    if let Some(details) = failure.copy_details() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(details));
                                    }
                                }
                            })),
                    ),
            );
        }
        column.child(self.render_list(cx)).into_any_element()
    }
}

impl HistoryView {
    /// The whole table for a pane this wide: the header, then the body for the
    /// current state. Called with the width the pane was actually assigned.
    fn render_at(&mut self, pane_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let columns = columns(pane_width, self.max_lanes.max(1));
        // The virtualized rows are painted after this and must agree with the
        // header.
        self.columns = columns;
        let body = self.render_body(columns, cx);
        // The column header stays above every state.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .child(self.render_header(columns, cx))
            .child(body)
            .into_any_element()
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The pane's own width is only known after layout. A container query
        // hands it back in prepaint, so the table is rebuilt for the width the
        // pane really got — no measure-and-notify round trip, no stale frame.
        let view = cx.entity().downgrade();
        container_query(move |size, _window, cx| {
            view.update(cx, |view, cx| view.render_at(f32::from(size.width), cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
    }
}

/// A body sentence with inline code runs, laid out as one wrapped paragraph.
struct BodyText {
    text: String,
    code: Vec<std::ops::Range<usize>>,
}

impl BodyText {
    fn new() -> Self {
        Self {
            text: String::new(),
            code: Vec::new(),
        }
    }

    /// A plain run in the body font.
    fn plain(mut self, text: &str) -> Self {
        self.text.push_str(text);
        self
    }

    /// A run of inline code, drawn in the mono font at the same colour.
    fn code(mut self, text: &str) -> Self {
        let start = self.text.len();
        self.text.push_str(text);
        self.code.push(start..self.text.len());
        self
    }

    /// The styled runs for [`Self::element`]: one per plain or code stretch.
    ///
    /// `StyledText::with_font_family_overrides` has no visible effect in this
    /// GPUI snapshot, so the runs are built explicitly; only the family
    /// differs, the size comes from the parent's text style.
    fn runs(&self, style: &TextStyle) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut start = 0;
        for range in &self.code {
            if start < range.start {
                runs.push(style.to_run(range.start - start));
            }
            let mut code_style = style.clone();
            code_style.font_family = FONT_MONO.into();
            runs.push(code_style.to_run(range.len()));
            start = range.end;
        }
        if start < self.text.len() {
            runs.push(style.to_run(self.text.len() - start));
        }
        runs
    }

    fn element(self, style: &TextStyle) -> StyledText {
        let runs = self.runs(style);
        StyledText::new(self.text).with_runs(runs)
    }
}

/// The empty-state body's text style: muted, 12px, the UI font. The code runs
/// are the same, only with the mono family.
fn body_style(colors: ThemeColors) -> TextStyle {
    TextStyle {
        color: colors.muted,
        font_family: FONT_UI.into(),
        font_size: FS_SM.into(),
        ..TextStyle::default()
    }
}

/// The empty states share the centre of the area under the header.
fn centred(state: EmptyState) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(FS_SM)
        .child(state)
        .into_any_element()
}

/// The graph column's placeholder: a lane line and one dot per row.
fn graph_skeleton(width: Pixels, colors: ThemeColors) -> Div {
    div()
        .w(width)
        .h_full()
        .flex_none()
        .relative()
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(7.))
                .w(px(2.))
                .bg(colors.border_strong),
        )
        .child(
            div()
                .absolute()
                .left(px(3.5))
                .top(px(8.5))
                .w(px(9.))
                .h(px(9.))
                .rounded(px(4.5))
                .bg(colors.border_strong),
        )
}

/// A skeleton bar's width in pixels for its column.
fn skeleton_width(column: SkeletonColumn, index: usize, columns: Columns) -> Pixels {
    let fraction = skeleton_percent(column, index) / 100.;
    let width = match column {
        SkeletonColumn::Summary => SKELETON_SUMMARY_REF,
        SkeletonColumn::Author => columns.author.unwrap_or(0.),
        SkeletonColumn::Date => columns.date,
        SkeletonColumn::Hash => columns.hash.unwrap_or(0.),
    };
    px(width * fraction)
}

fn next_page(
    mut stream: LogStream,
    mut graph: GraphBuilder<Oid>,
) -> (LogStream, GraphBuilder<Oid>, Page) {
    let page = stream
        .next_chunk(PAGE)
        .map_err(|e| Failure::from_error(&e))
        .map(|commits| {
            commits
                .into_iter()
                .map(|c| {
                    let row = graph.push(c.id, &c.parents);
                    (c, row)
                })
                .collect()
        });
    (stream, graph, page)
}

/// Seconds since the unix epoch; 0 if the clock is before it.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DD HH:MM` in UTC. Local time and relative dates come later.
fn format_time(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn formats_unix_time() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
    }

    #[test]
    fn columns_drop_the_wide_columns_at_the_container_queries() {
        // 560 px is the design's first breakpoint: Hash goes, Author and Date
        // take their narrow widths.
        let medium = columns(560., 30);
        assert_eq!(medium.hash, None);
        assert_eq!(medium.author, Some(COL_AUTHOR_NARROW));
        assert_eq!(medium.date, COL_DATE_NARROW);
        assert!(medium.graph <= 112., "{}", medium.graph);
        assert!(medium.graph <= 0.40 * 560. + f32::EPSILON);

        // 460 px is the second: Author goes too.
        let narrow = columns(460., 30);
        assert_eq!(narrow.hash, None);
        assert_eq!(narrow.author, None);
        assert_eq!(narrow.date, COL_DATE_NARROW);
        assert!(
            narrow.graph <= 112_f32.min(0.36 * 460.) + f32::EPSILON,
            "{}",
            narrow.graph
        );

        // A wide pane keeps all five columns.
        let wide = columns(1200., 2);
        assert_eq!(wide.author, Some(COL_AUTHOR));
        assert_eq!(wide.date, COL_DATE);
        assert_eq!(wide.hash, Some(COL_HASH));
    }

    #[test]
    fn columns_cap_the_graph_so_the_summary_keeps_its_room() {
        // 30 lanes need 356 px: `graph_width` stops drawing lanes past
        // MAX_GRAPH_LANES, so the 480 px design cap never binds at 1200 px.
        assert_eq!(f32::from(graph_width(30)), 356.);
        assert_eq!(columns(1200., 30).graph, 356.);
        // At 800 px the 40 % share binds before the lane width does.
        assert_eq!(columns(800., 30).graph, 320.);
        // A graph narrower than the share keeps the lanes' width.
        assert_eq!(columns(800., 2).graph, f32::from(graph_width(2)));
        // The 40 % share holds on any wide pane.
        assert_eq!(columns(700., 30).graph, 280.);
        // Never below the floor, however narrow the pane or the graph.
        assert_eq!(columns(300., 1).graph, GRAPH_MIN);
        assert_eq!(columns(100., 30).graph, GRAPH_MIN);
    }

    #[test]
    fn skeleton_widths_are_cell_percentages() {
        let wide = columns(1200., 1);
        assert_eq!(skeleton_width(SkeletonColumn::Summary, 0, wide), px(144.));
        assert_eq!(
            skeleton_width(SkeletonColumn::Summary, 1, wide),
            px(SKELETON_SUMMARY_REF * 0.77)
        );
        assert_eq!(skeleton_width(SkeletonColumn::Author, 0, wide), px(65.));
        assert_eq!(skeleton_width(SkeletonColumn::Date, 4, wide), px(62.4));
        assert_eq!(skeleton_width(SkeletonColumn::Hash, 4, wide), px(51.2));
        // The narrow table uses the narrow date column.
        let narrow = columns(460., 1);
        assert_eq!(
            skeleton_width(SkeletonColumn::Date, 4, narrow),
            px(COL_DATE_NARROW * (skeleton_percent(SkeletonColumn::Date, 4) / 100.))
        );
    }

    #[test]
    fn body_text_tracks_its_code_runs() {
        let body = BodyText::new()
            .plain("GitWarp needs git ")
            .plain("2.31.0")
            .plain(" or newer. System git at ")
            .code("/usr/bin/git")
            .plain(" reports 2.28.1.");
        assert_eq!(body.code.len(), 1);
        let range = body.code[0].clone();
        assert_eq!(&body.text[range], "/usr/bin/git");
    }

    #[test]
    fn body_runs_use_the_mono_font_for_code_only() {
        let body = BodyText::new()
            .plain("The command ")
            .code("git log --all --topo-order")
            .plain(" failed with exit code 128.");
        let style = body_style(ui::Theme::light().colors);
        let runs = body.runs(&style);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            (runs[0].len, runs[1].len, runs[2].len),
            (
                "The command ".len(),
                "git log --all --topo-order".len(),
                " failed with exit code 128.".len(),
            )
        );
        assert_eq!(runs[0].font.family.as_ref(), FONT_UI);
        assert_eq!(runs[1].font.family.as_ref(), FONT_MONO);
        assert_eq!(runs[2].font.family.as_ref(), FONT_UI);
        // Same colour across the sentence.
        assert!(runs.iter().all(|run| run.color == style.color));
        assert!(runs.iter().all(|run| run.font.weight == style.font_weight));
    }
}
