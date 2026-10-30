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

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, App, AppContext, Bounds, Context, CursorStyle, Decorations,
    Div, Entity, HitboxBehavior, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ResizeEdge, SharedString, Styled, Subscription, Tiling,
    Window,
};

use git_core::{Error, Git, GitChoice};
use ui::metrics::{BAR, FONT_UI, FS_BASE, R_WINDOW};
use ui::theme::ActiveTheme;
use ui::{overlay, WindowControls};

use crate::open_dialog::OpenRepoDialog;
use crate::repo_view::{
    FocusSearch, OpenBranchSwitcher, OpenRepoSwitcher, RepositoryView, ToggleDetails, ToggleSidebar,
};
use crate::welcome::WelcomeView;

actions!(workspace, [OpenRepositoryDialog]);

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
    /// The Open-from-folder dialog, kept so its state survives the frames.
    dialog: Option<Entity<OpenRepoDialog>>,
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
            dialog: None,
            _decorations: decorations,
        };
        match path {
            Some(path) => this.open_repository(path, window, cx),
            None => this.show_welcome(window, cx),
        }
        this
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
            Content::Welcome(_) => None,
            Content::Empty => None,
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

        let frame = frame
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(body),
            )
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
