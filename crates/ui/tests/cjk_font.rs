//! The bundled Noto Sans JP font supplies the CJK glyphs that Inter lacks.
//!
//! `#[gpui::test]` uses a mock text system with no glyph data, so these tests build
//! the context on the real platform text system (headless) instead.

use gpui::{font, px, TestAppContext, TestDispatcher, TextRun};
#[cfg(not(target_os = "windows"))]
use ui::metrics::FONT_CJK;
use ui::metrics::{FONT_MONO, FONT_UI};

fn real_text_context() -> TestAppContext {
    let text_system = gpui_platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    cx.update(ui::init).expect("ui::init");
    cx
}

// DirectWrite (Windows) does not expose a dynamically-loaded font family for
// name-based `resolve_font` lookups, so this assertion cannot pass there. The
// real CJK rendering path is still covered by the layout test below, which does
// pass on Windows through the font fallbacks.
#[cfg(not(target_os = "windows"))]
#[test]
fn noto_sans_jp_covers_cjk_and_inter_does_not() {
    let cx = real_text_context();
    let system = cx.text_system();
    let size = px(14.);

    let inter = system.resolve_font(&font(FONT_UI));
    let noto = system.resolve_font(&font(FONT_CJK));
    assert_ne!(inter, noto, "Noto Sans JP family is not registered");
    // `advance` fails when the font has no glyph for the character.
    assert!(
        system.advance(noto, size, '日').is_ok(),
        "Noto Sans JP lacks 日"
    );
    assert!(
        system.advance(inter, size, '日').is_err(),
        "Inter unexpectedly has 日"
    );
}

#[test]
fn base_families_with_fallbacks_lay_out_japanese_without_notdef() {
    let mut cx = real_text_context();
    let window = cx.add_empty_window();
    let text = "日本語";
    for family in [FONT_UI, FONT_MONO] {
        let mut f = font(family);
        f.fallbacks = Some(ui::fonts::fallbacks());
        let runs = [TextRun {
            len: text.len(),
            font: f,
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let layout =
            window.update(|window, _| window.text_system().layout_line(text, px(14.), &runs, None));
        let glyphs: Vec<_> = layout.runs.iter().flat_map(|r| r.glyphs.iter()).collect();
        assert_eq!(glyphs.len(), 3, "{family}: expected one glyph per char");
        assert!(
            glyphs.iter().all(|g| g.id.0 != 0),
            "{family}: .notdef glyph in 日本語"
        );
    }
}
