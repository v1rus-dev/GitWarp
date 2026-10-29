//! The history screen: commit graph + commit list, loaded in the background
//! and painted through a virtualized list (only visible rows are built).

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, uniform_list, AsyncApp, Context, IntoElement, SharedString,
    WeakEntity, Window,
};

use git_core::{CommitInfo, Git, Head, LogOptions, LogStream, Oid, Repo};
use git_graph::{GraphBuilder, Row};
use ui::ActiveTheme;

use crate::graph_paint::{graph_width, paint_row};
use crate::theme;

/// Rows are delivered to the UI in pages of this size.
const PAGE: usize = 2_000;

type Page = Result<Vec<(CommitInfo, Row<Oid>)>, String>;

struct Item {
    commit: CommitInfo,
    row: Rc<Row<Oid>>,
}

pub struct HistoryView {
    title: SharedString,
    /// e.g. ` · git 2.56.0 (system)`, appended to the status line.
    git_suffix: SharedString,
    items: Vec<Item>,
    max_lanes: usize,
    selected: Option<usize>,
    loading: bool,
    error: Option<SharedString>,
}

impl HistoryView {
    /// Starts in the loading state; opening the repository, reading HEAD and
    /// the history all run in the background and fill in the title, rows or
    /// error as they arrive. `git` is resolved once per process by the caller.
    pub fn open(path: PathBuf, git: Result<Arc<Git>, String>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            title: path.display().to_string().into(),
            // Shown next to the status, also when opening the repository fails.
            git_suffix: SharedString::default(),
            items: Vec::new(),
            max_lanes: 1,
            selected: None,
            loading: true,
            error: None,
        };
        match git {
            Ok(git) => {
                view.git_suffix = format!(" · git {} ({})", git.version(), git.source()).into();
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

        // HEAD is only needed for the title: don't hold the log back.
        let head_repo = repo.clone();
        let title_this = this.clone();
        cx.spawn(async move |cx| {
            let title = cx
                .background_executor()
                .spawn(async move { title_for(&head_repo) })
                .await;
            title_this
                .update(cx, |view, cx| {
                    view.title = title.into();
                    cx.notify();
                })
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

    fn fail(&mut self, message: impl ToString, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(message.to_string().into());
        cx.notify();
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        let colors = cx.theme().colors;
        let ui_theme = cx.theme().clone();
        let graph_w = graph_width(self.max_lanes);
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

                div()
                    .id(ix)
                    .h(theme::ROW_HEIGHT)
                    .w_full()
                    .flex()
                    .items_center()
                    .text_sm()
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
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(colors.text)
                            .child(c.summary.clone()),
                    )
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .px_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(colors.muted)
                            .child(c.author_name.clone()),
                    )
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .px_2()
                            .text_color(colors.muted)
                            .child(format_time(c.author_time)),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .px_2()
                            .font_family(ui::metrics::FONT_MONO)
                            .text_color(colors.muted)
                            .child(c.id.short()),
                    )
            })
            .collect()
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let git = &self.git_suffix;
        let status: SharedString = match (&self.error, self.loading) {
            (Some(e), _) => format!("{e}{git}"),
            (None, true) => format!("Loading… {} commits{git}", self.items.len()),
            (None, false) => format!("{} commits{git}", self.items.len()),
        }
        .into();

        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(ui::metrics::FONT_UI)
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if self.error.is_some() {
                                colors.error
                            } else {
                                colors.muted
                            })
                            .child(status),
                    ),
            )
            .child(
                uniform_list(
                    "history",
                    self.items.len(),
                    cx.processor(|view, range, _window, cx| view.render_rows(range, cx)),
                )
                .flex_1()
                .w_full(),
            );

        // The workspace that hosts this view owns the overlay host and the
        // shared root focus handling.
        root
    }
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

fn title_for(repo: &Repo) -> String {
    let name = repo
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.root().display().to_string());
    match repo.head() {
        Ok(Head::Branch { name: branch, .. }) => format!("{name} — {branch}"),
        Ok(Head::Detached(oid)) => format!("{name} — detached at {}", oid.short()),
        Err(_) => name,
    }
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
    #[test]
    fn formats_unix_time() {
        assert_eq!(super::format_time(0), "1970-01-01 00:00");
        assert_eq!(super::format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(super::format_time(951_782_400), "2000-02-29 00:00");
    }
}
