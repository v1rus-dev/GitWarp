//! Bundled fonts: Inter, JetBrains Mono and Noto Sans JP.
//!
//! Inter and JetBrains Mono cover Latin and Cyrillic; Noto Sans JP is the
//! CJK fallback (see [`fallbacks`]), so Japanese text renders on machines
//! without a system CJK font.

use std::borrow::Cow;

use gpui::{App, Font, FontFallbacks};

use crate::metrics::{FONT_CJK, FONT_UI};

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/NotoSansJP-Regular.otf"),
    include_bytes!("../assets/fonts/NotoSansJP-Medium.otf"),
    include_bytes!("../assets/fonts/NotoSansJP-Bold.otf"),
];

/// Registers the bundled font files with the text system.
pub fn load(cx: &App) -> gpui::Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect())
}

/// Fallback list for the UI and mono families: glyphs they lack (CJK) come
/// from the bundled Noto Sans JP. Set once on the root text style; elements
/// that only call `font_family` inherit it.
pub fn fallbacks() -> FontFallbacks {
    FontFallbacks::from_fonts(vec![FONT_CJK.to_owned()])
}

/// The base UI font for a window's root element: Inter plus [`fallbacks`].
/// Descendants that switch to `FONT_MONO` via `font_family` keep the fallbacks.
pub fn ui_font() -> Font {
    let mut font = gpui::font(FONT_UI);
    font.fallbacks = Some(fallbacks());
    font
}
