//! The window buttons of a custom (client-side) titlebar.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, svg, App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, WindowControlArea,
};

use crate::icon::IconName;
use crate::metrics::BAR;
use crate::theme::ActiveTheme;

/// Minimise, maximise/restore and close: three ghost buttons.
///
/// Presentational: it knows nothing about git or the app's content. On Linux it
/// calls the window methods itself, so the caller only decides *whether* to show
/// it. On Windows each button is a native hit-test area instead, so the OS runs
/// the action (and offers the Windows 11 snap layouts on the maximise button);
/// marking the areas is all that is needed and the click handlers stay off.
///
/// Render it only for client-side decorations: with server-side decorations the
/// system already draws its own buttons.
#[derive(IntoElement)]
pub struct WindowControls {
    id: SharedString,
    /// The button height; defaults to the toolbar height [`BAR`].
    height: Pixels,
}

impl WindowControls {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            height: BAR,
        }
    }

    /// Overrides the button height (the Windows drag strip is slimmer than the
    /// toolbar it would otherwise match).
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let base = self.id;
        let h = self.height;
        // Maximise toggles to restore, so the glyph has to follow the state.
        let max_icon = if window.is_maximized() {
            IconName::Restore
        } else {
            IconName::Max
        };

        div()
            .ml(px(8.))
            .h_full()
            .flex()
            .items_center()
            .child(control(
                format!("{base}-min").into(),
                IconName::Min,
                colors.muted,
                colors.hover,
                colors.text,
                h,
                WindowControlArea::Min,
                |window| window.minimize_window(),
            ))
            .child(control(
                format!("{base}-max").into(),
                max_icon,
                colors.muted,
                colors.hover,
                colors.text,
                h,
                WindowControlArea::Max,
                |window| window.zoom_window(),
            ))
            .child(control(
                format!("{base}-close").into(),
                IconName::X,
                colors.muted,
                colors.error,
                colors.accent_fg,
                h,
                WindowControlArea::Close,
                |window| window.remove_window(),
            ))
    }
}

/// One button: `hover_bg` and `hover_fg` are the colours while the pointer is
/// over it. Its `id` also names the hover group of the icon, so both the
/// background and the icon colour change together.
#[allow(clippy::too_many_arguments)]
fn control(
    id: SharedString,
    icon: IconName,
    resting: Hsla,
    hover_bg: Hsla,
    hover_fg: Hsla,
    height: Pixels,
    area: WindowControlArea,
    action: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    let group = id.clone();
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .w(px(32.))
        .h(height)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(0.))
        .hover(move |style| style.bg(hover_bg))
        .map(|button| {
            if cfg!(target_os = "windows") {
                // Windows owns the button: the hit area makes its hit-test report
                // HTMIN/HTMAX/HTCLOSE and the OS runs the action on release. The
                // click handlers must stay off — a `stop_propagation` on the
                // synthetic non-client mouse-down would cancel that native press.
                button.window_control_area(area)
            } else {
                // The bar behind these buttons starts a window move on mouse-down,
                // so the press must not reach it; the action itself runs on a
                // completed click (press and release on the same button).
                button
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, _| action(window))
            }
        })
        .child(
            svg()
                .path(icon.path())
                .w(px(14.))
                .h(px(14.))
                .text_color(resting)
                .group_hover(group, move |style| style.text_color(hover_fg)),
        )
}
