//! The window shell: client-side decorations, the titlebar, the status bar and
//! the switch between the welcome and repository content.
//!
//! A single window is opened at start; opening a repository or showing the
//! welcome screen swaps the content of this root view instead of opening
//! another window (on Linux the app quits with its last window).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, point, px, AppContext, Bounds, Context, CursorStyle, Decorations, Div, Entity,
    HitboxBehavior, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    Render, ResizeEdge, SharedString, Styled, Subscription, Tiling, Window,
};

use git_core::Git;
use ui::metrics::{BAR, FONT_UI, FS_BASE, FS_SM, R_WINDOW, STATUS};
use ui::theme::ActiveTheme;
use ui::{Icon, IconName, WindowControls};

use crate::history_view::HistoryView;
use crate::welcome::WelcomeView;

/// The shadow and resize border drawn around the client-side frame.
const INSET: Pixels = px(10.);

/// What the frame shows below the titlebar.
enum Content {
    Welcome(Entity<WelcomeView>),
    Repository(Entity<HistoryView>),
}

/// The window root: frame, titlebar, content and status bar.
pub struct Workspace {
    content: Content,
    git: Result<Arc<Git>, String>,
    /// The repository name in the titlebar; `None` on the welcome screen.
    repo_name: Option<SharedString>,
    /// Repaints when the decorations change (windows forced to server-side
    /// decorations, tiling on maximise/snap).
    _decorations: Subscription,
}

impl Workspace {
    /// Opens `path` as the repository, or the welcome screen when it is `None`.
    pub fn new(
        path: Option<PathBuf>,
        git: Result<Arc<Git>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let decorations = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        let mut this = Self {
            content: Content::Welcome(cx.new(WelcomeView::new)),
            git,
            repo_name: None,
            _decorations: decorations,
        };
        match path {
            Some(path) => this.open_repository(path, window, cx),
            None => this.show_welcome(window, cx),
        }
        this
    }

    /// Switches to the history of the repository at `path`.
    pub fn open_repository(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let git = self.git.clone();
        let view = cx.new(|cx| HistoryView::open(path.clone(), git, cx));
        let name = repo_name(&path);
        self.content = Content::Repository(view);
        self.repo_name = Some(name.clone());
        window.set_window_title(&format!("{name} — GitWarp"));
        cx.notify();
    }

    /// Switches back to the welcome screen.
    pub fn show_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Welcome(_)) {
            self.content = Content::Welcome(cx.new(WelcomeView::new));
        }
        self.repo_name = None;
        window.set_window_title("GitWarp");
        cx.notify();
    }

    fn render_titlebar(&self, client: bool, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let leading = match &self.content {
            Content::Welcome(_) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(4.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(Icon::new(IconName::Git).size(px(16.)))
                .child("GitWarp")
                .into_any_element(),
            Content::Repository(_) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(Icon::new(IconName::Folder).size(px(14.)))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .max_w(px(140.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(self.repo_name.clone().unwrap_or_default()),
                )
                .into_any_element(),
        };

        let mut bar = div()
            .h(BAR)
            .px(px(12.))
            .flex()
            .items_center()
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border)
            // The empty area of the bar moves the window; a double-click
            // toggles maximise. Right-click opens the system window menu.
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
            .child(leading)
            .child(div().flex_1().h_full());
        if client {
            bar = bar.child(WindowControls::new("window-controls"));
        }
        bar
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let git = match &self.git {
            Ok(git) => format!("git {} · {}", git.version(), git.source()),
            Err(_) => "git not found".to_owned(),
        };
        div()
            .h(STATUS)
            .px(px(8.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(6.))
            .bg(colors.sidebar)
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
            .child(git)
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

        let titlebar = self.render_titlebar(client, cx);
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
                .on_mouse_move(|_, window, _| window.refresh())
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
            .on_mouse_move(|_, _, cx| cx.stop_propagation());
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
            .child(titlebar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(body),
            )
            .child(self.render_status_bar(cx))
            // Menus, dialogs and toasts live inside the frame, above the
            // content, so they cannot spill over the window's shadow band.
            .child(ui::overlay::render_overlays(window, cx));

        let root = backdrop.child(frame);
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

/// The last path component, for the titlebar; "." and ".." resolve first.
fn repo_name(path: &Path) -> SharedString {
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
}
