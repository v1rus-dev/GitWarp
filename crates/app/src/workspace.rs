//! The window shell: client-side decorations, the frame and the switch between
//! the welcome and repository content.
//!
//! A single window is opened at start; opening a repository or showing the
//! welcome screen swaps the content of this root view instead of opening
//! another window (on Linux the app quits with its last window).
//!
//! Each content view renders its own full column — titlebar, body, status bar —
//! through the helpers in this module, so the frame only owns the decorations.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, size, App, AppContext, Bounds, Context, CursorStyle,
    Decorations, Div, Entity, HitboxBehavior, InteractiveElement, IntoElement, MouseButton,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ResizeEdge, SharedString, Styled,
    Subscription, Task, Tiling, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions,
};

use git_core::{Error, Git, GitChoice};
use ui::metrics::{BAR, FONT_UI, FS_BASE, R_WINDOW};
use ui::theme::ActiveTheme;
use ui::{overlay, WindowControls};

use crate::credential_dialog::CredentialDialog;
use crate::host_key_dialog::HostKeyDialog;
use crate::network_service::NetworkService;
use crate::open_dialog::OpenRepoDialog;
use crate::repo_view::{
    FocusSearch, OpenBranchSwitcher, OpenRepoSwitcher, RepositoryView, ToggleDetails, ToggleSidebar,
};
use crate::settings_ui::SettingsView;
use crate::welcome::WelcomeView;

actions!(workspace, [OpenRepositoryDialog, OpenSettings]);

/// The shadow and resize border drawn around the client-side frame.
const INSET: Pixels = px(10.);

/// What the frame shows below the decorations.
enum Content {
    /// Before [`Workspace::new`] has built the first content view: never
    /// rendered, but the welcome screen must not be built for a repository
    /// start.
    Empty,
    Welcome(Entity<WelcomeView>),
    Repository(Entity<RepositoryView>),
}

/// The window root: frame, decorations and the current content view.
pub struct Workspace {
    content: Content,
    /// The resolved git, or why there is none; shared by the views.
    git: Result<Arc<Git>, Arc<Error>>,
    /// What the user asked for; [`Workspace::retry_git`] resolves it again.
    git_choice: GitChoice,
    /// The resize edge under the pointer in the frame's inset band, if any.
    /// The band only repaints when this changes, so the cursor follows it.
    last_resize_edge: Option<ResizeEdge>,
    /// The settings window, drawn over the current content. `None` once closed.
    settings: Option<Entity<SettingsView>>,
    /// The Open-from-folder dialog, kept so its state survives the frames.
    dialog: Option<Entity<OpenRepoDialog>>,
    /// A git credential question, while one is open.
    credential: Option<Entity<CredentialDialog>>,
    /// An SSH host-key question, while one is open.
    host_key: Option<Entity<HostKeyDialog>>,
    /// Polls the network service for git's credential and host-key questions.
    _network_poll: Option<Task<()>>,
    /// Repaints when the decorations change (windows forced to server-side
    /// decorations, tiling on maximise/snap).
    _decorations: Subscription,
}

impl Workspace {
    /// Opens `path` as the repository, or the welcome screen when it is `None`.
    pub fn new(
        path: Option<PathBuf>,
        git: Result<Arc<Git>, Arc<Error>>,
        git_choice: GitChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let decorations = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        let mut this = Self {
            // Only a welcome start builds the welcome screen (item: it is
            // expensive and a repository start never shows it).
            content: Content::Empty,
            git,
            git_choice,
            last_resize_edge: None,
            settings: None,
            dialog: None,
            credential: None,
            host_key: None,
            _network_poll: None,
            _decorations: decorations,
        };
        match path {
            Some(path) => this.open_repository(path, window, cx),
            None => this.show_welcome(window, cx),
        }
        this.start_prompt_loop(window, cx);
        this
    }

    /// Git asks for credentials and host-key decisions from a background
    /// thread; this loop brings those questions to the window.
    fn start_prompt_loop(&mut self, window: &Window, cx: &mut Context<Self>) {
        self._network_poll = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            if this
                .update_in(cx, |this, window, cx| this.poll_network_prompts(window, cx))
                .is_err()
            {
                break;
            }
        }));
    }

    /// Opens one credential and one host-key modal at a time and drops the
    /// state once its layer is closed. A question that arrives while another
    /// dialog is open stays queued in the service.
    pub(crate) fn poll_network_prompts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.credential {
            let gone = dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx));
            if gone {
                self.credential = None;
            }
        }
        if let Some(dialog) = &self.host_key {
            let gone = dialog.update(cx, |dialog, cx| dialog.layer_gone(window, cx));
            if gone {
                self.host_key = None;
            }
        }

        if self.credential.is_none() {
            let prompt = cx
                .try_global::<NetworkService>()
                .and_then(NetworkService::try_recv_credential);
            if let Some(prompt) = prompt {
                self.credential = Some(crate::credential_dialog::open(prompt, window, cx));
            }
        }
        if self.host_key.is_none() {
            let request = cx
                .try_global::<NetworkService>()
                .and_then(NetworkService::try_recv_host_key);
            if let Some(request) = request {
                self.host_key = Some(crate::host_key_dialog::open(request, window, cx));
            }
        }
    }

    /// Switches to the repository at `path`.
    pub fn open_repository(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let git = self.git.clone();
        let workspace = cx.weak_entity();
        let view = cx.new(|cx| RepositoryView::new(path.clone(), git, workspace, window, cx));
        let name = repo_name(&path);
        let focus = view.read(cx).focus_handle();
        self.content = Content::Repository(view);
        window.set_window_title(&format!("{name} — GitWarp"));
        // The keyboard belongs to the new content at once, like the welcome
        // list gets it in `show_welcome`.
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Opens `path` in a second window, leaving this one on its repository.
    /// Used by the worktree page's "Open" and the add dialog's "Open in a new
    /// window" option.
    pub fn open_repository_window(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let git = self.git.clone();
        let git_choice = self.git_choice.clone();
        let name = repo_name(&path);
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_decorations: Some(WindowDecorations::Client),
                window_background: WindowBackgroundAppearance::Transparent,
                window_min_size: Some(size(px(620.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{name} — GitWarp").into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                app_id: Some("dev.v1rus.gitwarp".into()),
                ..Default::default()
            },
            move |window, cx| {
                ui::theme::follow_system(window, cx);
                cx.new(|cx| Workspace::new(Some(path), git, git_choice, window, cx))
            },
        );
        if let Err(error) = opened {
            log::warn!("could not open a worktree window: {error}");
            window.refresh();
        }
    }

    /// Switches back to the welcome screen.
    pub fn show_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Welcome(_)) {
            let workspace = cx.weak_entity();
            let view = cx.new(|cx| WelcomeView::new(self.git.clone(), workspace, window, cx));
            self.content = Content::Welcome(view);
        }
        if let Content::Welcome(view) = &self.content {
            // The list takes the keyboard first, so ↑/↓ work without a click.
            let focus = view.read(cx).list_focus();
            window.focus(&focus, cx);
        }
        window.set_window_title("GitWarp");
        cx.notify();
    }

    /// Opens Settings as a window over the current content. The repository or
    /// welcome screen stays underneath and keeps its title.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let root = self.with_repository(cx, |view, _| view.path().to_path_buf());
        let git = self.git.clone();
        let workspace = cx.weak_entity();
        let view = cx.new(|cx| SettingsView::new(root, git, workspace, window, cx));
        let focus = view.read(cx).focus_handle();
        self.settings = Some(view);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the settings window, leaving the content it covered as it was.
    pub fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_none() {
            return;
        }
        match &self.content {
            Content::Repository(view) => {
                let focus = view.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            Content::Welcome(view) => {
                let focus = view.read(cx).list_focus();
                window.focus(&focus, cx);
            }
            Content::Empty => {
                self.show_welcome(window, cx);
            }
        }
        // The Git page may have chosen a different executable; resolve it now
        // so the open repository (or the welcome screen) uses it at once.
        let choice = cx
            .try_global::<crate::AppSettings>()
            .map(|state| state.settings.git_choice())
            .unwrap_or_default();
        if choice != self.git_choice {
            self.git_choice = choice;
            self.retry_git(window, cx);
        }
        cx.notify();
    }

    /// Opens the "Open from folder" dialog, or brings it back when the user
    /// closed its layer with Esc or the scrim and asks for it again.
    pub fn open_repository_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.dialog {
            if let Some(layer) = dialog.read(cx).layer_id() {
                if overlay::layer_view(layer, window, cx).is_some() {
                    return;
                }
            }
        }
        let git = self.git.clone();
        let workspace = cx.weak_entity();
        self.dialog = Some(crate::open_dialog::open(window, cx, git, workspace));
        cx.notify();
    }

    /// Drops the dialog state once its layer is gone.
    fn prune_dialog(&mut self, window: &Window, cx: &App) {
        let Some(dialog) = &self.dialog else {
            return;
        };
        let open = dialog
            .read(cx)
            .layer_id()
            .is_some_and(|layer| overlay::layer_view(layer, window, cx).is_some());
        if !open {
            self.dialog = None;
        }
    }

    /// Resolve git again (the user may have installed it since) and, when the
    /// same repository is still open, reopen it so its history starts over
    /// with the new handle. A repository the user opened in the meantime wins
    /// over the retry.
    pub fn retry_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self.git_choice.clone();
        let path = match &self.content {
            Content::Repository(view) => Some(view.read(cx).path().to_path_buf()),
            Content::Welcome(_) | Content::Empty => None,
        };
        cx.spawn_in(window, async move |this, cx| {
            let resolved = cx
                .background_executor()
                .spawn(async move {
                    Git::resolve(choice)
                        .map(|resolution| {
                            if let Some(reason) = resolution.fallback {
                                log::warn!("bundled git unusable, using system git: {reason}");
                            }
                            Arc::new(resolution.git)
                        })
                        .map_err(Arc::new)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.git = resolved.clone();
                enum Target {
                    Repository(PathBuf),
                    Welcome,
                    None,
                }
                let target = match &this.content {
                    Content::Repository(view) => {
                        Target::Repository(view.read(cx).path().to_path_buf())
                    }
                    Content::Welcome(_) => Target::Welcome,
                    Content::Empty => Target::None,
                };
                match target {
                    // Still the repository the user retried for.
                    Target::Repository(current) if path.as_ref() == Some(&current) => {
                        this.open_repository(current, window, cx);
                    }
                    // Another repository was opened while the resolve ran.
                    Target::Repository(_) | Target::None => {}
                    Target::Welcome => {
                        let view = match &this.content {
                            Content::Welcome(view) => view.clone(),
                            _ => return,
                        };
                        view.update(cx, |view, cx| view.set_git(resolved, cx));
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Runs `f` on the open repository, if there is one.
    fn with_repository<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut RepositoryView, &mut Context<RepositoryView>) -> R,
    ) -> Option<R> {
        match &self.content {
            Content::Repository(view) => Some(view.update(cx, f)),
            Content::Welcome(_) | Content::Empty => None,
        }
    }
}

/// The titlebar/toolbar container shared by the content views.
///
/// The caller appends its items, one of them [`titlebar_fill`], and finishes
/// with [`titlebar_end`] so the window buttons stay at the right end.
pub fn titlebar(cx: &App) -> Div {
    let colors = cx.theme().colors;
    div()
        .h(BAR)
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(6.))
        .bg(colors.bg)
        .border_b_1()
        .border_color(colors.border)
        .text_size(FS_BASE)
        .font_family(FONT_UI)
}

/// The empty stretch of the titlebar: the window drag area.
///
/// The empty area of the bar moves the window; a double-click toggles
/// maximise, a right-click opens the system window menu. Buttons and other
/// controls sit beside it, so pressing them never starts a window move.
pub fn titlebar_fill() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, _| {
            window.show_window_menu(event.position);
        })
}

/// Appends the window controls when the window uses client decorations.
pub fn titlebar_end(bar: Div, client: bool) -> Div {
    if client {
        // Never shrink: the three buttons must stay fully visible at the
        // minimum window width.
        bar.child(
            div()
                .flex_none()
                .child(WindowControls::new("window-controls")),
        )
    } else {
        bar
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_dialog(window, cx);
        let decorations = window.window_decorations();
        // Sent on every frame (the backend dedups): the frame's padding has to
        // match the inset before the layout runs.
        window.set_client_inset(INSET);

        let colors = cx.theme().colors;
        let (client, tiling) = match decorations {
            Decorations::Server => (false, Tiling::default()),
            Decorations::Client { tiling } => (true, tiling),
        };

        let body = match &self.content {
            Content::Empty => div().size_full().into_any_element(),
            Content::Welcome(view) => view.clone().into_any_element(),
            Content::Repository(view) => view.clone().into_any_element(),
        };
        let settings = self.settings.clone();

        // The padded backdrop holds the shadow and the resize band. With
        // server-side decorations the system draws the frame instead, so it
        // gets no padding, border, radius or shadow of ours.
        let mut backdrop = div().size_full().bg(gpui::transparent_black());
        if client {
            backdrop = backdrop
                .when(!(tiling.top || tiling.right), |d| d.rounded_tr(R_WINDOW))
                .when(!(tiling.top || tiling.left), |d| d.rounded_tl(R_WINDOW))
                .when(!(tiling.bottom || tiling.right), |d| d.rounded_br(R_WINDOW))
                .when(!(tiling.bottom || tiling.left), |d| d.rounded_bl(R_WINDOW))
                .when(!tiling.top, |d| d.pt(INSET))
                .when(!tiling.bottom, |d| d.pb(INSET))
                .when(!tiling.left, |d| d.pl(INSET))
                .when(!tiling.right, |d| d.pr(INSET))
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                        let size = window.window_bounds().get_bounds().size;
                        let edge = resize_edge(event.position, INSET, size, tiling);
                        if edge != this.last_resize_edge {
                            this.last_resize_edge = edge;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_down(MouseButton::Left, move |event, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(event.position, INSET, size, tiling) {
                        window.start_window_resize(edge);
                    }
                })
                .child(resize_cursor(tiling));
        }

        let mut frame = div()
            .id("window-frame")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .cursor(CursorStyle::Arrow)
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                // The frame covers the whole padded area, so this is where the
                // pointer leaves the resize band. Forget the edge so that
                // re-entering the band repaints and restores the resize cursor.
                cx.stop_propagation();
                if this.last_resize_edge.take().is_some() {
                    cx.notify();
                }
            }));
        if client {
            frame = frame
                .border_color(colors.border)
                .when(!tiling.top, |d| d.border_t_1())
                .when(!tiling.bottom, |d| d.border_b_1())
                .when(!tiling.left, |d| d.border_l_1())
                .when(!tiling.right, |d| d.border_r_1())
                .when(!(tiling.top || tiling.right), |d| d.rounded_tr(R_WINDOW))
                .when(!(tiling.top || tiling.left), |d| d.rounded_tl(R_WINDOW))
                .when(!(tiling.bottom || tiling.right), |d| d.rounded_br(R_WINDOW))
                .when(!(tiling.bottom || tiling.left), |d| d.rounded_bl(R_WINDOW))
                .when(!tiling.is_tiled(), |d| d.shadow(cx.theme().pop_shadow()));
        }

        let content = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(body);
        let content = if let Some(settings) = settings {
            let colors = colors;
            content.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(32.))
                    .bg(colors.overlay)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        this.close_settings(window, cx);
                    }))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1080.))
                            .h_full()
                            .max_h(px(760.))
                            .rounded(R_WINDOW)
                            .overflow_hidden()
                            .border_1()
                            .border_color(colors.border)
                            .shadow(cx.theme().pop_shadow())
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(settings),
                    ),
            )
        } else {
            content
        };
        let frame = frame.child(content)
            // Menus, dialogs and toasts live inside the frame, above the
            // content, so they cannot spill over the window's shadow band.
            .child(ui::overlay::render_overlays(window, cx));

        let root = backdrop.child(frame);

        // Fallback for the repository shortcuts: when nothing holds the focus,
        // the dispatch path is the window root only and the content view's own
        // handlers never run. When the focus is inside the repository view its
        // handler runs first and stops propagation, so nothing toggles twice.
        let root = root
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.with_repository(cx, |view, cx| view.toggle_sidebar(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleDetails, _, cx| {
                this.with_repository(cx, |view, cx| view.toggle_details(cx));
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                if let Some(handle) = this.with_repository(cx, |view, _| view.search_focus()) {
                    window.focus(&handle, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenRepoSwitcher, window, cx| {
                this.with_repository(cx, |view, cx| view.open_repo_menu(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenBranchSwitcher, window, cx| {
                this.with_repository(cx, |view, cx| view.open_branch_menu(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenRepositoryDialog, window, cx| {
                this.open_repository_dialog(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(window, cx);
            }));

        ui::focus::track_pointer(ui::actions::tab_navigation(root))
    }
}

/// The cursor for the resize band: one shape per edge or corner.
fn resize_cursor(tiling: Tiling) -> impl IntoElement {
    canvas(
        |_bounds, window, _| {
            window.insert_hitbox(
                Bounds::new(
                    point(px(0.), px(0.)),
                    window.window_bounds().get_bounds().size,
                ),
                HitboxBehavior::Normal,
            )
        },
        move |_bounds, hitbox, window, _| {
            let mouse = window.mouse_position();
            let size = window.window_bounds().get_bounds().size;
            let Some(edge) = resize_edge(mouse, INSET, size, tiling) else {
                return;
            };
            window.set_cursor_style(
                match edge {
                    ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                    ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                        CursorStyle::ResizeUpLeftDownRight
                    }
                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                        CursorStyle::ResizeUpRightDownLeft
                    }
                },
                &hitbox,
            );
        },
    )
    .size_full()
    .absolute()
}

/// The edge or corner under `pos`, only within the inset band and only for
/// edges the window manager has not tiled (Zed's CSD does the same): a
/// maximised window must not start a resize from a click near its edge.
fn resize_edge(
    pos: Point<Pixels>,
    inset: Pixels,
    size: gpui::Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let top = !tiling.top && pos.y < inset;
    let bottom = !tiling.bottom && pos.y > size.height - inset;
    let left = !tiling.left && pos.x < inset;
    let right = !tiling.right && pos.x > size.width - inset;
    if top && left {
        Some(ResizeEdge::TopLeft)
    } else if top && right {
        Some(ResizeEdge::TopRight)
    } else if top {
        Some(ResizeEdge::Top)
    } else if bottom && left {
        Some(ResizeEdge::BottomLeft)
    } else if bottom && right {
        Some(ResizeEdge::BottomRight)
    } else if bottom {
        Some(ResizeEdge::Bottom)
    } else if left {
        Some(ResizeEdge::Left)
    } else if right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// The status bar's git label: the version and source, or why there is none.
pub fn git_label(git: &Result<Arc<Git>, Arc<Error>>) -> SharedString {
    match git {
        Ok(git) => format!("git {} · {}", git.version(), git.source()).into(),
        Err(error) => match error.class() {
            git_core::ErrorClass::GitTooOld { found, .. } => format!("git {found} too old").into(),
            _ => "git not found".into(),
        },
    }
}

/// The informational tooltip of the git label: version, source and
/// executable path, or the failure title when git is unusable.
pub fn git_tooltip(git: &Result<Arc<Git>, Arc<Error>>) -> SharedString {
    match git {
        Ok(git) => format!(
            "git {} · {} · {}",
            git.version(),
            git.source(),
            git.path().display()
        )
        .into(),
        Err(error) => crate::history_state::Failure::from_error(error).title(),
    }
}

/// The last path component, for the titlebar.
///
/// Purely textual on purpose: this runs on the UI thread while the repository
/// opens. Recent roots are stored canonical, so there is nothing to resolve.
pub fn repo_name(path: &Path) -> SharedString {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    #[test]
    fn resize_edges_cover_the_band_and_corners() {
        let size = size(px(100.), px(50.));
        let untiled = Tiling::default();
        assert_eq!(
            resize_edge(point(px(1.), px(1.)), INSET, size, untiled),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(99.), px(1.)), INSET, size, untiled),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            resize_edge(point(px(50.), px(1.)), INSET, size, untiled),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(point(px(1.), px(25.)), INSET, size, untiled),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge(point(px(99.), px(49.)), INSET, size, untiled),
            Some(ResizeEdge::BottomRight)
        );
        assert_eq!(
            resize_edge(point(px(50.), px(25.)), INSET, size, untiled),
            None
        );
    }

    #[test]
    fn a_tiled_window_has_no_resize_edges() {
        let size = size(px(100.), px(50.));
        // Maximised: every edge belongs to the window manager.
        let tiled = Tiling {
            top: true,
            bottom: true,
            left: true,
            right: true,
        };
        for pos in [
            point(px(1.), px(1.)),
            point(px(99.), px(1.)),
            point(px(50.), px(1.)),
            point(px(1.), px(25.)),
            point(px(99.), px(25.)),
            point(px(99.), px(49.)),
        ] {
            assert_eq!(resize_edge(pos, INSET, size, tiled), None, "{pos:?}");
        }
        // Snapped to the left: the tiled side and its corners are dead, the
        // others still resize.
        let left = Tiling {
            left: true,
            ..Tiling::default()
        };
        assert_eq!(resize_edge(point(px(1.), px(25.)), INSET, size, left), None);
        assert_eq!(
            resize_edge(point(px(1.), px(1.)), INSET, size, left),
            Some(ResizeEdge::Top),
            "the tiled left edge is dead, the untiled top still resizes"
        );
        assert_eq!(
            resize_edge(point(px(99.), px(25.)), INSET, size, left),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge(point(px(99.), px(49.)), INSET, size, left),
            Some(ResizeEdge::BottomRight)
        );
    }

    #[gpui::test]
    fn the_settings_action_opens_the_pages_and_escape_returns(cx: &mut gpui::TestAppContext) {
        use gpui::{
            point, size, Bounds, VisualTestContext, WindowBackgroundAppearance, WindowBounds,
            WindowDecorations, WindowOptions,
        };

        cx.update(ui::init).expect("ui::init");
        let handle = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1200.), px(800.)),
                    ))),
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        Workspace::new(
                            None,
                            Err(Arc::new(Error::PathNotFound {
                                path: PathBuf::from("/nonexistent"),
                            })),
                            GitChoice::Auto,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open window")
        });
        let mut cx = VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        cx.update(|window, _| window.activate_window());
        assert!(
            cx.debug_bounds("welcome-view").is_some() || cx.debug_bounds("recent-filter").is_some(),
            "the welcome screen did not render"
        );

        cx.update(|window, cx| window.dispatch_action(Box::new(OpenSettings), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("settings-view").is_some(),
            "Ctrl , did not open Settings"
        );
        assert!(
            cx.debug_bounds("settings-page-General").is_some(),
            "the General page is not the first one"
        );
        for selector in [
            "settings-nav-Appearance",
            "settings-nav-Git",
            "settings-nav-Ssh",
            "settings-nav-Keybindings",
            "settings-nav-Diff",
            "settings-nav-Identity",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "the {selector} page is missing from the navigation"
            );
        }

        let git = cx.debug_bounds("settings-nav-Git").expect("Git nav");
        cx.simulate_click(git.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("settings-page-Git").is_some(),
            "clicking Git did not switch the page"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("settings-view").is_none(),
            "Escape did not leave Settings"
        );
        assert!(
            cx.debug_bounds("recent-filter").is_some(),
            "closing Settings did not bring the welcome screen back"
        );
    }

    #[test]
    fn repo_name_is_the_last_component() {
        assert_eq!(repo_name(Path::new("/home/user/gitwarp")), "gitwarp");
        assert_eq!(repo_name(Path::new("/")), "/");
        // No canonicalize on the UI thread: the name is read off the text.
        assert_eq!(
            repo_name(Path::new("/home/user/../user/gitwarp")),
            "gitwarp"
        );
        assert_eq!(repo_name(Path::new("/does/not/exist/notes")), "notes");
        assert_eq!(repo_name(Path::new(".")), ".");
    }
}
