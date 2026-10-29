//! The window buttons of a custom (client-side) titlebar.

use gpui::{
    div, px, svg, App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::icon::IconName;
use crate::metrics::BAR;
use crate::theme::ActiveTheme;

/// Minimise, maximise/restore and close: three 32×40 ghost buttons.
///
/// Presentational: it knows nothing about git or the app's content. It calls
/// the window methods itself, so the caller only decides *whether* to show it.
/// Render it only for client-side decorations: with server-side decorations
/// the system already draws its own buttons.
#[derive(IntoElement)]
pub struct WindowControls {
    id: SharedString,
}

impl WindowControls {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self { id: id.into() }
    }
}

impl RenderOnce for WindowControls {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let base = self.id;

        div()
            .ml(px(8.))
            .flex()
            .items_center()
            .child(control(
                format!("{base}-min").into(),
                IconName::Min,
                colors.muted,
                colors.hover,
                colors.text,
                |window| window.minimize_window(),
            ))
            .child(control(
                format!("{base}-max").into(),
                IconName::Max,
                colors.muted,
                colors.hover,
                colors.text,
                |window| window.zoom_window(),
            ))
            .child(control(
                format!("{base}-close").into(),
                IconName::X,
                colors.muted,
                colors.error,
                colors.accent_fg,
                |window| window.remove_window(),
            ))
    }
}

/// One button: `hover_bg` and `hover_fg` are the colours while the pointer is
/// over it. Its `id` also names the hover group of the icon, so both the
/// background and the icon colour change together.
fn control(
    id: SharedString,
    icon: IconName,
    resting: Hsla,
    hover_bg: Hsla,
    hover_fg: Hsla,
    action: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    let group = id.clone();
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .w(px(32.))
        .h(BAR)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(0.))
        .hover(move |style| style.bg(hover_bg))
        // The bar behind these buttons starts a window move on mouse-down, so
        // the press must not reach it; the action itself runs on a completed
        // click (press and release on the same button), like a native button.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, _| action(window))
        .child(
            svg()
                .path(icon.path())
                .w(px(14.))
                .h(px(14.))
                .text_color(resting)
                .group_hover(group, move |style| style.text_color(hover_fg)),
        )
}
