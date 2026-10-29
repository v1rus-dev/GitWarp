//! Git status glyphs: a coloured letter in a box.

use gpui::{
    div, px, App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, Styled, Window,
};

use crate::metrics::R_SM;
use crate::theme::{ActiveTheme, ThemeColors};

/// A one-letter git status marker.
#[derive(IntoElement)]
pub struct StatusGlyph {
    letter: char,
}

impl StatusGlyph {
    pub fn new(letter: char) -> Self {
        Self { letter }
    }
}

fn color_for(letter: char, colors: ThemeColors) -> Hsla {
    match letter.to_ascii_uppercase() {
        'A' => colors.st_added,
        'M' => colors.st_modified,
        'D' => colors.st_deleted,
        'R' => colors.st_renamed,
        '?' => colors.st_untracked,
        'U' => colors.st_conflict,
        _ => colors.muted,
    }
}

impl RenderOnce for StatusGlyph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = color_for(self.letter, cx.theme().colors);
        div()
            .w(px(16.))
            .h(px(16.))
            .rounded(R_SM)
            .flex()
            .items_center()
            .justify_center()
            .bg(color.opacity(0.16))
            .border_1()
            .border_color(color.opacity(0.40))
            .text_color(color)
            .text_size(px(10.))
            .line_height(px(10.))
            .font_weight(FontWeight::BOLD)
            .child(self.letter.to_string())
    }
}
