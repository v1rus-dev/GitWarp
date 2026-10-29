//! The history screen: column header + commit graph and commit list, loaded in
//! the background and painted through a virtualized list (only visible rows are
//! built).
//!
//! The view also owns the repository metadata the shell needs — HEAD and the
//! refs — because it is the view that opens the [`Repo`].

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, uniform_list, AsyncApp, Context, IntoElement, Pixels,
    SharedString, WeakEntity, Window,
};

use git_core::{CommitInfo, Git, Head, LogOptions, LogStream, Oid, Ref, Repo};
use git_graph::{GraphBuilder, Row};
use ui::metrics::{FONT_MONO, FS_SM};
use ui::ActiveTheme;

use crate::graph_paint::{graph_width, paint_row};
use crate::theme;

/// Rows are delivered to the UI in pages of this size.
const PAGE: usize = 2_000;

/// Column widths of the history table, shared by the header and the rows.
const COL_GAP: f32 = 18.;
const COL_AUTHOR: f32 = 130.;
const COL_DATE: f32 = 104.;
const COL_HASH: f32 = 64.;
/// The header and the rows share this side margin and padding.
const SIDE_MARGIN: f32 = 6.;
const PAD_LEFT: f32 = 14.;
const PAD_RIGHT: f32 = 18.;
/// The graph column never gets narrower than this, even with one lane.
const GRAPH_MIN: f32 = 40.;

type Page = Result<Vec<(CommitInfo, Row<Oid>)>, String>;

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
    /// The repository root, once `Repo::open` succeeded.
    root: Option<PathBuf>,
    items: Vec<Item>,
    max_lanes: usize,
    selected: Option<usize>,
    loading: bool,
    error: Option<SharedString>,
    /// `None` until the background read finishes; `Err` leaves the toolbar
    /// showing the placeholder.
    head: Option<Result<Head, String>>,
    refs: RefsState,
}

impl HistoryView {
    /// Starts in the loading state; opening the repository, reading HEAD, the
    /// refs and the history all run in the background and fill in the rows or
    /// the error as they arrive. `git` is resolved once per process by the
    /// caller.
    pub fn open(path: PathBuf, git: Result<Arc<Git>, String>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root: None,
            items: Vec::new(),
            max_lanes: 1,
            selected: None,
            loading: true,
            error: None,
            head: None,
            refs: RefsState::Loading,
        };
        match git {
            Ok(git) => {
                cx.spawn(async move |this, cx| {
                    if let Err(e) = Self::run(git, path, &this, cx).await {
                        this.update(cx, |view, cx| view.fail(e, cx)).ok();
                    }
                })
                .detach();
            }
            Err(e) => {
                view.loading = false;
                view.error = Some(e.into());
            }
        }
        view
    }

    /// Open the repository and stream its history into the view. An `Err` is
    /// shown to the user; a closed window just ends the task with `Ok`.
    async fn run(
        git: Arc<Git>,
        path: PathBuf,
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) -> Result<(), String> {
        let executor = cx.background_executor().clone();
        let repo = executor
            .spawn(async move { Repo::open(git, &path) })
            .await
            .map_err(|e| e.to_string())?;

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
            .map_err(|e| e.to_string())?;
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
        this.update(cx, |view, cx| {
            view.loading = false;
            cx.notify();
        })
        .ok();
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

    fn set_meta(
        &mut self,
        root: PathBuf,
        head: Result<Head, git_core::Error>,
        refs: Result<Vec<Ref>, git_core::Error>,
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

    fn fail(&mut self, message: impl ToString, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(message.to_string().into());
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

    /// Whether the history is still streaming in.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// How many commits have arrived so far.
    pub fn commit_count(&self) -> usize {
        self.items.len()
    }

    /// The error that stopped the load, if any.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The short hash of the selected commit, for the details header.
    pub fn selected_short(&self) -> Option<SharedString> {
        self.selected
            .and_then(|ix| self.items.get(ix))
            .map(|item| item.commit.id.short().into())
    }

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let graph = graph_column(self.max_lanes);
        // A 6px gutter around the header, so it lines up with the rows.
        div().w_full().px(px(SIDE_MARGIN)).child(
            div()
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
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.muted)
                .child(div().w(graph).flex_none().child("Graph"))
                .child(div().flex_1().min_w_0().truncate().child("Summary"))
                .child(div().w(px(COL_AUTHOR)).flex_none().child("Author"))
                .child(div().w(px(COL_DATE)).flex_none().child("Date"))
                .child(div().w(px(COL_HASH)).flex_none().child("Hash")),
        )
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        let colors = cx.theme().colors;
        let ui_theme = cx.theme().clone();
        let graph_w = graph_column(self.max_lanes);
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
                div().w_full().px(px(SIDE_MARGIN)).child(
                    div()
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
                                    paint_row(&row, bounds, background, &row_theme, window)
                                },
                            )
                            .w(graph_w)
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
                        )
                        .child(
                            div()
                                .w(px(COL_AUTHOR))
                                .flex_none()
                                .truncate()
                                .text_color(colors.muted)
                                .child(c.author_name.clone()),
                        )
                        .child(
                            div()
                                .w(px(COL_DATE))
                                .flex_none()
                                .truncate()
                                .text_color(colors.muted)
                                .child(format_time(c.author_time)),
                        )
                        .child(
                            div()
                                .w(px(COL_HASH))
                                .flex_none()
                                .truncate()
                                .font_family(FONT_MONO)
                                .text_color(colors.muted)
                                .child(c.id.short()),
                        ),
                )
            })
            .collect()
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(ui::metrics::FONT_UI)
            .child(self.render_header(cx))
            .child(
                uniform_list(
                    "history",
                    self.items.len(),
                    cx.processor(|view, range, _window, cx| view.render_rows(range, cx)),
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
    }
}

/// The width of the graph column: the lanes the history needs, but at least
/// [`GRAPH_MIN`] so the header and the cells never collapse.
fn graph_column(lanes: usize) -> Pixels {
    graph_width(lanes).max(px(GRAPH_MIN))
}

fn next_page(
    mut stream: LogStream,
    mut graph: GraphBuilder<Oid>,
) -> (LogStream, GraphBuilder<Oid>, Page) {
    let page = stream
        .next_chunk(PAGE)
        .map_err(|e| e.to_string())
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
fn unix_now() -> u64 {
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
    fn graph_column_has_a_floor() {
        assert_eq!(graph_column(1), px(GRAPH_MIN));
        assert_eq!(graph_column(24), graph_width(24));
        assert!(graph_column(24) > px(GRAPH_MIN));
    }
}
