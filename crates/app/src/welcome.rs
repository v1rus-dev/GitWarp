//! The welcome screen.
//!
//! This step only fills the frame: the real welcome content (recent
//! repositories, open/clone actions) is built in step 2.5.

use gpui::{div, Context, IntoElement, ParentElement, Render, Styled, Window};
use ui::ActiveTheme;

pub struct WelcomeView;

impl WelcomeView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for WelcomeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.bg)
            .text_sm()
            .text_color(colors.muted)
            .child("No repository open")
    }
}
