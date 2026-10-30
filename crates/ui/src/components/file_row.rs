//! Changed-file rows, with paths truncated from the start.

use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, px, transparent_black, App, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, TextAlign,
    TextRun, Window,
};
use gpui::{AnyElement, Font, Hsla};
use unicode_segmentation::UnicodeSegmentation;

use crate::components::status_glyph::StatusGlyph;
use crate::focus::{self, RingOffset};
use crate::metrics::{FONT_UI, FS_BASE, FS_XS, ROW, R_SM};
use crate::theme::ActiveTheme;

use super::tabular_font;

/// The single horizontal-ellipsis character (U+2026), not three periods.
const ELLIPSIS: char = '\u{2026}';

/// A path split into its muted directory prefix and its file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FittedPath {
    /// The muted part, including the `…` when truncated.
    pub prefix: String,
    /// The file name, drawn in the row's foreground colour.
    pub name: String,
    pub truncated: bool,
}

/// Fit `path` into `max_width`, dropping leading graphemes and prefixing `…`.
///
/// The search is a pure binary search over a caller-provided measuring
/// closure, so it can be tested with a fake monospace measure and used with
/// the real text system at paint time.
pub fn fit_path_start(
    path: &str,
    max_width: Pixels,
    measure: impl Fn(&str) -> Pixels,
) -> FittedPath {
    let split = |text: &str| match text.rfind('/') {
        Some(index) => (text[..=index].to_owned(), text[index + 1..].to_owned()),
        None => (String::new(), text.to_owned()),
    };

    if measure(path) <= max_width {
        let (prefix, name) = split(path);
        return FittedPath {
            prefix,
            name,
            truncated: false,
        };
    }

    let graphemes: Vec<&str> = path.graphemes(true).collect();
    let mut low = 1;
    let mut high = graphemes.len();
    let mut best = None;
    while low <= high {
        let mid = (low + high) / 2;
        let candidate = format!("{ELLIPSIS}{}", graphemes[mid..].concat());
        if measure(&candidate) <= max_width {
            best = Some(mid);
            high = mid - 1;
        } else {
            low = mid + 1;
        }
    }

    match best {
        Some(dropped) => {
            let candidate = format!("{ELLIPSIS}{}", graphemes[dropped..].concat());
            let ellipsis = ELLIPSIS.len_utf8();
            let (prefix, name) = match candidate.rfind('/') {
                Some(index) if index >= ellipsis => (
                    candidate[..=index].to_owned(),
                    candidate[index + 1..].to_owned(),
                ),
                _ => (ELLIPSIS.to_string(), candidate[ellipsis..].to_owned()),
            };
            FittedPath {
                prefix,
                name,
                truncated: true,
            }
        }
        None => FittedPath {
            prefix: ELLIPSIS.to_string(),
            name: String::new(),
            truncated: true,
        },
    }
}

/// A 26px changed-file row.
#[derive(IntoElement)]
pub struct FileRow {
    id: ElementId,
    status: char,
    path: SharedString,
    added: usize,
    deleted: usize,
    selected: bool,
    actions: Vec<AnyElement>,
    on_click: Option<crate::components::ClickHandler>,
}

impl FileRow {
    pub fn new(id: impl Into<ElementId>, status: char, path: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            status,
            path: path.into(),
            added: 0,
            deleted: 0,
            selected: false,
            actions: Vec::new(),
            on_click: None,
        }
    }

    pub fn added(mut self, added: usize) -> Self {
        self.added = added;
        self
    }

    pub fn deleted(mut self, deleted: usize) -> Self {
        self.deleted = deleted;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A caller-provided action button, shown on hover or focus.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for FileRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(false);
        let contains_focus = focus.contains_focused(window, cx);
        let ring = focus::focus_visible(&focus, window, cx);

        let background = if self.selected {
            colors.selection
        } else {
            transparent_black()
        };
        let foreground = if self.selected {
            colors.selection_fg
        } else {
            colors.text
        };
        let dir_color = if self.selected {
            colors.selection_sub
        } else {
            colors.muted
        };
        let count_color = |color: Hsla| {
            if self.selected {
                colors.selection_sub
            } else {
                color
            }
        };

        let path = self.path.clone();
        let path_canvas = canvas(
            move |bounds, window, _| {
                let max_width = bounds.size.width;
                let fitted = fit_path_start(&path, max_width, |text| measure_text(window, text));
                let font = gpui::font(FONT_UI);
                let mut runs = Vec::with_capacity(2);
                if !fitted.prefix.is_empty() {
                    runs.push(text_run(&fitted.prefix, &font, dir_color));
                }
                if !fitted.name.is_empty() {
                    runs.push(text_run(&fitted.name, &font, foreground));
                }
                let text = format!("{}{}", fitted.prefix, fitted.name);
                window
                    .text_system()
                    .shape_line(SharedString::from(text), FS_BASE, &runs, None)
            },
            move |bounds, line, window, cx| {
                let _ = line.paint(bounds.origin, ROW, TextAlign::Left, None, window, cx);
            },
        );

        let path_canvas = path_canvas.flex_1().min_w(px(0.)).h_full();

        let mut counts = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .text_size(FS_XS)
            .line_height(FS_XS)
            .font(tabular_font());
        if self.added > 0 {
            counts = counts.child(
                div()
                    .text_color(count_color(colors.success))
                    .child(format!("+{}", self.added)),
            );
        }
        if self.deleted > 0 {
            counts = counts.child(
                div()
                    .text_color(count_color(colors.error))
                    .child(format!("−{}", self.deleted)),
            );
        }

        let actions = if self.actions.is_empty() {
            None
        } else {
            let mut container = div().flex().flex_none().items_center().gap(px(2.));
            if !contains_focus {
                container = container
                    .opacity(0.)
                    .group_hover("ui-file-row", |style| style.opacity(1.));
            }
            Some(container.children(self.actions))
        };

        let mut row = div()
            .id(self.id)
            .group("ui-file-row")
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(ROW)
            // A row in a scrolling flex column keeps its fixed height.
            .flex_none()
            .ml(px(6.))
            .mr(px(6.))
            .px(px(8.))
            .rounded(R_SM)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .text_color(foreground)
            .bg(background)
            .track_focus(&focus)
            .child(StatusGlyph::new(self.status))
            .child(path_canvas)
            .child(counts)
            .when_some(actions, |this, actions| this.child(actions));

        if !self.selected {
            let hover = colors.hover;
            row = row.hover(move |style| style.bg(hover));
        }

        if let Some(listener) = self.on_click {
            row = row.on_click(move |event, window, cx| listener(event, window, cx));
        }

        if ring {
            row = row.child(focus::focus_ring(RingOffset::Inset2, R_SM, cx));
        }

        row
    }
}

fn measure_text(window: &Window, text: &str) -> Pixels {
    let font = gpui::font(FONT_UI);
    let run = text_run(text, &font, gpui::black());
    window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), FS_BASE, &[run], None)
        .width
}

fn text_run(text: &str, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake monospace font: every character is 7px wide.
    fn mono(text: &str) -> Pixels {
        px(text.chars().count() as f32 * 7.)
    }

    #[test]
    fn a_path_that_fits_keeps_the_directory_split() {
        let fitted = fit_path_start("src/components/main.rs", px(1000.), mono);
        assert!(!fitted.truncated);
        assert_eq!(fitted.prefix, "src/components/");
        assert_eq!(fitted.name, "main.rs");
    }

    #[test]
    fn a_long_path_drops_leading_graphemes_and_prefixes_an_ellipsis() {
        let max = px(140.);
        let fitted = fit_path_start("crates/ui/src/components/file_row.rs", max, mono);
        assert!(fitted.truncated);
        assert_eq!(fitted.prefix.chars().next(), Some(ELLIPSIS));
        assert_eq!(fitted.prefix.matches(ELLIPSIS).count(), 1);
        assert!(!fitted.prefix.contains("..."));
        assert!(fitted.prefix.ends_with('/'));
        assert_eq!(fitted.name, "file_row.rs");
        assert!(mono(&format!("{}{}", fitted.prefix, fitted.name)) <= max);
    }

    #[test]
    fn truncation_that_eats_the_directory_leaves_the_name_ltr() {
        let max = px(70.);
        let fitted = fit_path_start("crates/ui/src/components/file_row.rs", max, mono);
        assert!(fitted.truncated);
        assert_eq!(fitted.prefix.len(), ELLIPSIS.len_utf8());
        assert_eq!(fitted.prefix, ELLIPSIS.to_string());
        assert!(!fitted.prefix.contains("..."));
        assert!(fitted.name.ends_with("row.rs"));
        assert!(mono(&format!("{}{}", fitted.prefix, fitted.name)) <= max);
    }

    #[test]
    fn an_empty_directory_is_drawn_entirely_in_the_name_colour() {
        let fitted = fit_path_start("README.md", px(1000.), mono);
        assert_eq!(fitted.prefix, "");
        assert_eq!(fitted.name, "README.md");
    }
}
