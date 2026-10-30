//! The welcome screen.
//!
//! This step only fills the frame: the real welcome content (recent
//! repositories, open/clone actions) is built in step 2.5.

use std::sync::Arc;

use gpui::{
    div, px, Context, Decorations, FontWeight, IntoElement, ParentElement, Render, Styled, Window,
};

use git_core::{Error, Git};
use ui::metrics::{FONT_UI, FS_BASE, FS_SM, STATUS};
use ui::theme::ActiveTheme;
use ui::{Icon, IconName};

use crate::workspace::{git_label, titlebar, titlebar_end, titlebar_fill};

pub struct WelcomeView {
    /// Resolved once per process by the caller; shown in the status bar.
    git: Result<Arc<Git>, Arc<Error>>,
}

impl WelcomeView {
    pub fn new(git: Result<Arc<Git>, Arc<Error>>, _cx: &mut Context<Self>) -> Self {
        Self { git }
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let colors = cx.theme().colors;
        let git = git_label(&self.git);

        let bar = titlebar(cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::Git).size(px(16.)))
                    .child("GitWarp"),
            )
            .child(titlebar_fill());

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .child(titlebar_end(bar, client))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(colors.muted)
                    .child("No repository open"),
            )
            .child(
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
                    .child(git),
            )
    }
}
