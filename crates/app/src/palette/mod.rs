//! The command palette (Ctrl+P): one input that searches the app's commands,
//! the branches, the loaded commits and the tracked files of the open
//! repository, and runs the picked result.
//!
//! The matcher and the ranking are pure and live in [`matcher`] and
//! [`sources`]; this module is the modal view over them. It runs nothing
//! itself: a pick closes the palette and then hands the target to the
//! workspace or the repository view, which already own those flows.

mod matcher;
mod sources;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, px, size, App, AppContext, Context, Decorations, Entity, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, ScrollHandle,
    Size, StatefulInteractiveElement, Styled, StyledText, Subscription, Task, TextRun, TextStyle,
    WeakEntity, Window,
};

use git_core::{Error, Git};
use ui::actions::{Confirm, SelectFirst, SelectLast, SelectNext, SelectPrev};
use ui::metrics::{FONT_UI, FS_BASE, FS_SM, FS_XS, R_LG, R_SM};
use ui::overlay::{self, LayerId};
use ui::{
    ActiveTheme, Icon, IconName, Kbd, RefBadge, RefKind, Scrollbar, TextInput, TextInputEvent,
};

pub(crate) use matcher::fuzzy;
pub(crate) use sources::{Command, Group, Hit, Recent, Sources, Target, ViewState};

use crate::repo_view::RepositoryView;
use crate::settings_ui::Page;
use crate::workspace::Workspace;

actions!(palette, [OpenPalette, PagePrev, PageNext, Ignore]);

/// Rows a PgUp / PgDn press moves.
const PAGE: usize = 10;
const PLACEHOLDER: &str = "Type a command, branch, commit hash or file…";

/// Binds the palette's own keys in its key context. Called once at startup;
/// the key that opens it is bound with the other repository shortcuts.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("pageup", PagePrev, Some(CONTEXT)),
        gpui::KeyBinding::new("pagedown", PageNext, Some(CONTEXT)),
        // Tab must not walk the focus out of the palette.
        gpui::KeyBinding::new("tab", Ignore, Some(CONTEXT)),
        gpui::KeyBinding::new("shift-tab", Ignore, Some(CONTEXT)),
    ]);
}

const CONTEXT: &str = "CommandPalette";

/// What the repository view hands over to open a palette.
pub(crate) struct Init {
    pub repo: WeakEntity<RepositoryView>,
    pub workspace: WeakEntity<Workspace>,
    pub sources: Sources,
    pub git: Result<Arc<Git>, Arc<Error>>,
    pub root: PathBuf,
}

/// Opens the palette as a modal and focuses its input. Closing it (Esc, a
/// click outside, a pick) returns the focus to where it was.
pub(crate) fn open(init: Init, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| {
        let mut input = TextInput::new(window, cx);
        // The row around it draws the palette's chrome, not the input.
        input.set_borderless(true);
        input.set_placeholder(PLACEHOLDER, cx);
        input
    });
    let view = cx.new(|cx| PaletteView::new(init, input.clone(), window, cx));
    let layer = overlay::open_modal(view.clone().into(), window, cx);
    view.update(cx, |view, _| view.layer = Some(layer));
    let focus = input.read(cx).focus_handle().clone();
    window.focus(&focus, cx);
}

/// One line of the results list.
enum Row {
    Header(Group),
    Hit(usize),
}

/// The list's lines: a header before the first hit of each group.
fn rows(hits: &[Hit]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(hits.len() + 4);
    let mut last = None;
    for (index, hit) in hits.iter().enumerate() {
        if last != Some(hit.group) {
            rows.push(Row::Header(hit.group));
            last = Some(hit.group);
        }
        rows.push(Row::Hit(index));
    }
    rows
}

pub(crate) struct PaletteView {
    layer: Option<LayerId>,
    input: Entity<TextInput>,
    _input_events: Subscription,
    sources: Sources,
    query: String,
    hits: Vec<Hit>,
    selected: usize,
    scroll: ScrollHandle,
    repo: WeakEntity<RepositoryView>,
    workspace: WeakEntity<Workspace>,
    /// The `git ls-files` read; dropped with the palette.
    _files: Option<Task<()>>,
}

impl PaletteView {
    fn new(
        init: Init,
        input: Entity<TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Changed(text) => this.set_query(text.clone(), cx),
                TextInputEvent::Submit => this.run(this.selected, true, window, cx),
            },
        );
        let mut this = Self {
            layer: None,
            input,
            _input_events: events,
            sources: init.sources,
            query: String::new(),
            hits: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            repo: init.repo,
            workspace: init.workspace,
            _files: None,
        };
        this.refresh(cx);
        this._files = Some(this.load_files(init.git, init.root, cx));
        this
    }

    /// Reads the tracked files once, on the background executor, and adds the
    /// Files group when they arrive.
    fn load_files(
        &self,
        git: Result<Arc<Git>, Arc<Error>>,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let Ok(git) = git else {
                return;
            };
            let files = cx
                .background_executor()
                .spawn(async move { sources::tracked_files(&git, &root) })
                .await;
            this.update(cx, |this, cx| match files {
                Ok(files) => {
                    this.sources.set_files(files);
                    // The Files group is last, so the selection keeps its row.
                    this.refresh(cx);
                }
                Err(error) => log::warn!("the palette could not list the files: {error}"),
            })
            .ok();
        })
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.selected = 0;
        self.refresh(cx);
        self.scroll.scroll_to_item(0);
    }

    /// Recomputes the hits for the current query.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let recent = cx.try_global::<Recent>().cloned().unwrap_or_default();
        self.hits = sources::search(&self.query, &self.sources, &recent);
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            overlay::close(layer, window, cx);
        }
    }

    fn move_selection(&mut self, to: isize, cx: &mut Context<Self>) {
        let last = self.hits.len().saturating_sub(1) as isize;
        let next = to.clamp(0, last) as usize;
        if next != self.selected {
            self.selected = next;
            self.scroll_to_selected();
            cx.notify();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.move_selection(self.selected as isize + delta, cx);
    }

    /// Scrolls the selected hit into view; the first hit of a group takes its
    /// header along.
    fn scroll_to_selected(&self) {
        let rows = rows(&self.hits);
        let index = rows
            .iter()
            .position(|row| matches!(row, Row::Hit(i) if *i == self.selected))
            .unwrap_or(0);
        let header = index
            .checked_sub(1)
            .is_some_and(|before| matches!(rows[before], Row::Header(_)));
        self.scroll
            .scroll_to_item(if header { index - 1 } else { index });
    }

    /// Runs hit `index` and closes the palette. `keyboard` is whether Enter
    /// picked it: the focus restore then waits for the key-up, so the key
    /// release cannot activate the control that regains the focus.
    fn run(&mut self, index: usize, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.hits.get(index).cloned() else {
            return;
        };
        match &hit.target {
            Target::Command(command) => cx.default_global::<Recent>().touch_command(*command),
            Target::Branch { name, .. } => cx.default_global::<Recent>().touch_branch(name),
            Target::Commit(_) | Target::File(_) => {}
        }
        if keyboard {
            overlay::defer_focus_restore(window, cx);
        }
        self.close(window, cx);
        self.dispatch(hit, window, cx);
        if keyboard {
            overlay::clear_deferred_focus_restore(window, cx);
        }
    }

    /// Hands the picked target to the owner of that flow, after the palette
    /// has closed.
    fn dispatch(&self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        match hit.target {
            Target::Command(command) if command.is_workspace_level() => {
                self.workspace
                    .update(cx, |workspace, cx| match command {
                        Command::OpenSettings(Page::General) => workspace.open_settings(window, cx),
                        Command::OpenSettings(page) => workspace.open_settings_at(page, window, cx),
                        Command::OpenRepository => workspace.open_repository_dialog(window, cx),
                        Command::Clone => workspace.open_clone_dialog(window, cx),
                        Command::CloseRepository => workspace.show_welcome(window, cx),
                        _ => {}
                    })
                    .ok();
            }
            Target::Command(command) => {
                self.repo
                    .update(cx, |view, cx| view.run_palette_command(command, window, cx))
                    .ok();
            }
            // The checked-out branch is already where the user wants to be.
            Target::Branch { .. } if hit.current => {}
            Target::Branch { name, remote } => {
                self.repo
                    .update(cx, |view, cx| {
                        view.palette_checkout(name, remote, window, cx)
                    })
                    .ok();
            }
            Target::Commit(oid) => {
                self.repo
                    .update(cx, |view, cx| view.palette_reveal_commit(oid, window, cx))
                    .ok();
            }
            Target::File(path) => {
                self.repo
                    .update(cx, |view, cx| view.palette_reveal_file(path, window, cx))
                    .ok();
            }
        }
    }

    /// A hit's title with the matched characters in the accent colour, bold.
    fn title(hit: &Hit, colors: ui::ThemeColors) -> StyledText {
        highlighted(&hit.title, &hit.positions, colors)
    }

    fn render_row(&self, index: usize, hit: &Hit, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = index == self.selected;
        let trailing = if hit.current {
            Some(
                RefBadge::new(format!("palette-current-{index}"), "current")
                    .kind(RefKind::Head)
                    .into_any_element(),
            )
        } else if let Target::Command(command) = &hit.target {
            command
                .key_cap(cx)
                .map(|cap| Kbd::new(cap).into_any_element())
        } else {
            None
        };
        div()
            .id(format!("palette-item-{index}"))
            .debug_selector(move || format!("palette-item-{index}"))
            .h(px(28.))
            .px(px(8.))
            .gap(px(10.))
            .flex()
            .items_center()
            .flex_none()
            .rounded(R_SM)
            .cursor_pointer()
            .when(selected, |row| row.bg(colors.selection))
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run(index, false, window, cx);
            }))
            .child(
                Icon::new(hit.group.icon())
                    .size(px(14.))
                    .color(colors.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(Self::title(hit, colors)),
            )
            .children(trailing)
    }

    fn render_footer(&self, colors: ui::ThemeColors) -> impl IntoElement {
        let hint = |keys: Vec<&'static str>, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys.into_iter().map(Kbd::new))
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(6.))
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(hint(vec!["↑", "↓"], "navigate"))
            .child(hint(vec!["Enter"], "run"))
            .child(hint(vec![">"], "commands"))
            .child(hint(vec!["@"], "branches"))
            .child(hint(vec!["#"], "commits"))
            .child(div().flex_1())
            .child(match self.hits.len() {
                1 => "1 result".to_owned(),
                n => format!("{n} results"),
            })
    }
}

/// `text` with the characters at `positions` (character indices, as the
/// matcher reports them) in the accent colour, bold. The titlebar worktree
/// switcher highlights its matches the same way.
pub(crate) fn highlighted(text: &str, positions: &[usize], colors: ui::ThemeColors) -> StyledText {
    let style = TextStyle {
        color: colors.text,
        font_family: FONT_UI.into(),
        font_size: FS_BASE.into(),
        ..TextStyle::default()
    };
    highlighted_in(text, positions, style, colors.accent)
}

/// [`highlighted`] over a caller's base `style`: the matched characters turn
/// `accent` and bold, the rest keep the style.
pub(crate) fn highlighted_in(
    text: &str,
    positions: &[usize],
    style: TextStyle,
    accent: gpui::Hsla,
) -> StyledText {
    let mut matched = style.clone();
    matched.color = accent;
    matched.font_weight = FontWeight::BOLD;
    let mut runs: Vec<TextRun> = Vec::new();
    let mut cursor = 0;
    for range in matcher::byte_ranges(text, positions) {
        if cursor < range.start {
            runs.push(style.to_run(range.start - cursor));
        }
        runs.push(matched.to_run(range.len()));
        cursor = range.end;
    }
    if cursor < text.len() {
        runs.push(style.to_run(text.len() - cursor));
    }
    StyledText::new(text.to_owned()).with_runs(runs)
}

/// The area the overlay layer covers: the window less the client-side frame's
/// shadow band.
fn overlay_area(window: &Window) -> Size<Pixels> {
    let viewport = window.viewport_size();
    match (window.window_decorations(), window.client_inset()) {
        (Decorations::Client { .. }, Some(inset)) => size(
            (viewport.width - inset * 2.).max(px(0.)),
            (viewport.height - inset * 2.).max(px(0.)),
        ),
        _ => viewport,
    }
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let area = overlay_area(window);

        let input_row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(40.))
            .px(px(12.))
            .border_b_1()
            .border_color(colors.border)
            .text_color(colors.muted)
            .child(
                Icon::new(IconName::Search)
                    .size(px(16.))
                    .color(colors.muted),
            )
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(Kbd::new("Esc"));

        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .p(px(4.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for row in rows(&self.hits) {
            list = match row {
                Row::Header(group) => list.child(
                    div()
                        .flex_none()
                        .pt(px(8.))
                        .px(px(8.))
                        .pb(px(2.))
                        .text_size(FS_XS)
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.muted)
                        .child(group.header()),
                ),
                Row::Hit(index) => {
                    let hit = self.hits[index].clone();
                    list.child(self.render_row(index, &hit, cx))
                }
            };
        }
        let results = if self.hits.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_h(px(120.))
                .text_color(colors.muted)
                .child(format!("No results for “{}”", self.query.trim()))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_row()
                .min_h_0()
                .max_h(px(380.))
                .child(list)
                .child(Scrollbar::vertical(
                    "palette-scrollbar",
                    self.scroll.clone(),
                ))
                .into_any_element()
        };

        let panel = div()
            .debug_selector(|| "palette".to_owned())
            .w(px(600.))
            .max_h(px(480.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_LG)
            .shadow(cx.theme().pop_shadow())
            .font_family(FONT_UI)
            .text_color(colors.text)
            // The panel is the modal; a click on it must not reach the
            // area around it, which closes the palette.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(input_row)
            .child(results)
            .child(self.render_footer(colors));

        // The modal layer centres its view; the palette sits 72px from the top
        // instead, so the view fills the layer and aligns the panel itself.
        div()
            .w(area.width)
            .h(area.height)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(72.))
            .key_context(CONTEXT)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.move_selection(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
                this.move_selection(this.hits.len() as isize - 1, cx);
            }))
            .on_action(cx.listener(|this, _: &PageNext, _, cx| this.step(PAGE as isize, cx)))
            .on_action(cx.listener(|this, _: &PagePrev, _, cx| this.step(-(PAGE as isize), cx)))
            .on_action(cx.listener(|_, _: &Ignore, _, _| {}))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                this.run(this.selected, true, window, cx);
            }))
            // Ctrl P again closes it.
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.close(window, cx)))
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::*;
    use crate::test_support::{git, open_workspace, TempRepo};

    fn run_git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Commits `file` with its content (and message) set to `text`.
    fn commit(repo: &Path, file: &str, text: &str) {
        std::fs::write(repo.join(file), text).expect("write the file");
        run_git(repo, &["add", file]);
        run_git(repo, &["commit", "-q", "-m", text]);
    }

    /// The workspace window with `repo` open and its history loaded.
    fn open_repo(
        repo: &TempRepo,
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, Entity<Workspace>) {
        let (mut cx, workspace) = open_workspace(git(), cx);
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx)
            });
        });
        cx.run_until_parked();
        (cx, workspace)
    }

    fn open_palette(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.dispatch_action(Box::new(OpenPalette), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("palette").is_some(),
            "the palette did not open"
        );
    }

    /// Types `query` and presses Enter, then lets the picked flow finish.
    fn pick(cx: &mut VisualTestContext, query: &str) {
        cx.simulate_input(query);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("palette").is_none(),
            "Enter left the palette open"
        );
    }

    fn selected_summary(
        cx: &mut VisualTestContext,
        workspace: &Entity<Workspace>,
    ) -> Option<String> {
        cx.update(|_, cx| {
            let view = workspace.read(cx).repository_for_test()?;
            let history = view.read(cx).history();
            let commit = history.read(cx).selected_commit()?;
            Some(commit.summary.clone())
        })
    }

    #[gpui::test]
    fn escape_closes_the_palette_and_returns_the_focus(cx: &mut TestAppContext) {
        let repo = TempRepo::new("palette-escape");
        commit(repo.path(), "a.txt", "one");
        let (mut cx, _workspace) = open_repo(&repo, cx);
        let before = cx.update(|window, cx| window.focused(cx));

        open_palette(&mut cx);
        let panel = cx.debug_bounds("palette").expect("the palette rendered");
        assert_eq!(panel.size.width, px(600.));
        assert!(
            panel.origin.y >= px(72.),
            "the panel sits 72px from the top"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        assert!(cx.debug_bounds("palette").is_none(), "Esc did not close it");
        assert_eq!(cx.update(|window, cx| window.focused(cx)), before);
    }

    #[gpui::test]
    fn enter_on_a_branch_checks_it_out(cx: &mut TestAppContext) {
        let repo = TempRepo::new("palette-branch");
        commit(repo.path(), "a.txt", "one");
        run_git(repo.path(), &["branch", "zzfeature"]);
        let (mut cx, _workspace) = open_repo(&repo, cx);

        open_palette(&mut cx);
        pick(&mut cx, "zzfeat");

        assert_eq!(
            run_git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "zzfeature"
        );
    }

    #[gpui::test]
    fn enter_on_a_command_runs_it(cx: &mut TestAppContext) {
        let repo = TempRepo::new("palette-command");
        commit(repo.path(), "a.txt", "one");
        let (mut cx, _workspace) = open_repo(&repo, cx);
        assert!(cx.debug_bounds("sidebar-filter").is_some());

        open_palette(&mut cx);
        pick(&mut cx, "toggle sidebar");
        assert!(
            cx.debug_bounds("sidebar-filter").is_none(),
            "the sidebar stayed open"
        );

        open_palette(&mut cx);
        pick(&mut cx, "switch local");
        assert!(
            cx.debug_bounds("changes-view").is_some(),
            "Local changes did not open"
        );
    }

    #[gpui::test]
    fn a_hash_prefix_selects_the_commit_in_history(cx: &mut TestAppContext) {
        let repo = TempRepo::new("palette-hash");
        commit(repo.path(), "a.txt", "first");
        let first = run_git(repo.path(), &["rev-parse", "HEAD"]);
        commit(repo.path(), "b.txt", "second");
        let (mut cx, workspace) = open_repo(&repo, cx);
        assert_eq!(
            selected_summary(&mut cx, &workspace).as_deref(),
            Some("second")
        );

        open_palette(&mut cx);
        pick(&mut cx, &first[..7]);

        assert_eq!(
            selected_summary(&mut cx, &workspace).as_deref(),
            Some("first")
        );
    }

    #[gpui::test]
    fn a_clean_file_selects_its_latest_commit_and_a_changed_one_opens_local_changes(
        cx: &mut TestAppContext,
    ) {
        let repo = TempRepo::new("palette-file");
        commit(repo.path(), "a.txt", "first");
        commit(repo.path(), "b.txt", "second");
        let (mut cx, workspace) = open_repo(&repo, cx);

        open_palette(&mut cx);
        pick(&mut cx, "a.txt");
        assert_eq!(
            selected_summary(&mut cx, &workspace).as_deref(),
            Some("first")
        );
        assert!(cx.debug_bounds("changes-view").is_none());

        std::fs::write(repo.path().join("b.txt"), "edited").expect("edit b.txt");
        open_palette(&mut cx);
        pick(&mut cx, "b.txt");
        assert!(
            cx.debug_bounds("changes-view").is_some(),
            "Local changes did not open"
        );
    }

    #[gpui::test]
    fn a_query_without_a_match_says_so(cx: &mut TestAppContext) {
        let repo = TempRepo::new("palette-empty");
        commit(repo.path(), "a.txt", "one");
        let (mut cx, _workspace) = open_repo(&repo, cx);

        open_palette(&mut cx);
        cx.simulate_input("qqqzzzjjj");
        cx.run_until_parked();

        assert!(cx.debug_bounds("palette").is_some());
        assert!(cx.debug_bounds("palette-item-0").is_none());
    }
}
