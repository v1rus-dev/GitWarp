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
    canvas, div, point, px, App, AppContext, Bounds, Context, CursorStyle, Decorations, Div,
    Entity, HitboxBehavior, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ResizeEdge, SharedString, Styled, Subscription, Tiling,
    Window,
};

use git_core::{Error, Git, GitChoice};
use ui::metrics::{BAR, FONT_UI, FS_BASE, R_WINDOW};
use ui::theme::ActiveTheme;
use ui::WindowControls;

use crate::repo_view::{
    FocusSearch, OpenBranchSwitcher, OpenRepoSwitcher, RepositoryView, ToggleDetails, ToggleSidebar,
};
use crate::welcome::WelcomeView;

/// The shadow and resize border drawn around the client-side frame.
const INSET: Pixels = px(10.);

/// What the frame shows below the decorations.
enum Content {
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
            content: Content::Welcome(cx.new(|cx| WelcomeView::new(git.clone(), cx))),
            git,
            git_choice,
            last_resize_edge: None,
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
        self.content = Content::Repository(view);
        window.set_window_title(&format!("{name} — GitWarp"));
        cx.notify();
    }

    /// Switches back to the welcome screen.
    pub fn show_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Welcome(_)) {
            self.content = Content::Welcome(cx.new(|cx| WelcomeView::new(self.git.clone(), cx)));
        }
        window.set_window_title("GitWarp");
        cx.notify();
    }

    /// Resolve git again (the user may have installed it since) and, when a
    /// repository is open, reopen it so its history starts over with the new
    /// handle.
    pub fn retry_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self.git_choice.clone();
        let path = match &self.content {
            Content::Repository(view) => Some(view.read(cx).path().to_path_buf()),
            Content::Welcome(_) => None,
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
                this.git = resolved;
                match path {
                    Some(path) => this.open_repository(path, window, cx),
                    None => this.show_welcome(window, cx),
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
            Content::Welcome(_) => None,
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
        .on_mouse_down(MouseButton::Left, |event, window, _| {
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
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    let size = window.window_bounds().get_bounds().size;
                    let edge = resize_edge(event.position, INSET, size);
                    if edge != this.last_resize_edge {
                        this.last_resize_edge = edge;
                        cx.notify();
                    }
                }))
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(event.position, INSET, size) {
                        window.start_window_resize(edge);
                    }
                })
                .child(resize_cursor());
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
            }));

        ui::focus::track_pointer(ui::actions::tab_navigation(root))
    }
}

/// The cursor for the resize band: one shape per edge or corner.
fn resize_cursor() -> impl IntoElement {
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
        |_bounds, hitbox, window, _| {
            let mouse = window.mouse_position();
            let size = window.window_bounds().get_bounds().size;
            let Some(edge) = resize_edge(mouse, INSET, size) else {
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

/// The edge or corner under `pos`, only within the inset band.
fn resize_edge(pos: Point<Pixels>, inset: Pixels, size: gpui::Size<Pixels>) -> Option<ResizeEdge> {
    if pos.y < inset && pos.x < inset {
        Some(ResizeEdge::TopLeft)
    } else if pos.y < inset && pos.x > size.width - inset {
        Some(ResizeEdge::TopRight)
    } else if pos.y < inset {
        Some(ResizeEdge::Top)
    } else if pos.y > size.height - inset && pos.x < inset {
        Some(ResizeEdge::BottomLeft)
    } else if pos.y > size.height - inset && pos.x > size.width - inset {
        Some(ResizeEdge::BottomRight)
    } else if pos.y > size.height - inset {
        Some(ResizeEdge::Bottom)
    } else if pos.x < inset {
        Some(ResizeEdge::Left)
    } else if pos.x > size.width - inset {
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

/// The last path component, for the titlebar; "." and ".." resolve first.
pub fn repo_name(path: &Path) -> SharedString {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
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
        assert_eq!(
            resize_edge(point(px(1.), px(1.)), INSET, size),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(99.), px(1.)), INSET, size),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            resize_edge(point(px(50.), px(1.)), INSET, size),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(point(px(1.), px(25.)), INSET, size),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge(point(px(99.), px(49.)), INSET, size),
            Some(ResizeEdge::BottomRight)
        );
        assert_eq!(resize_edge(point(px(50.), px(25.)), INSET, size), None);
    }

    #[test]
    fn repo_name_is_the_last_component() {
        assert_eq!(repo_name(Path::new("/home/user/gitwarp")), "gitwarp");
        assert_eq!(repo_name(Path::new("/")), "/");
    }
}
