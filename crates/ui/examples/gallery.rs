//! Visual catalogue of the design system tokens.

use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, ClickEvent, FontWeight, Hsla, IntoElement,
    Pixels, Render, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use ui::metrics::{
    BAR, CTL, DOT_R, FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_MD, FS_SM, FS_XL, FS_XS, MONO, R_SM,
    R_XS,
};
use ui::theme::{self, ActiveTheme, Theme, ThemeColors, ThemeMode};

struct Gallery {
    mode: ThemeMode,
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let mode = self.mode;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .child(
                div()
                    .h(BAR)
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(colors.surface)
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("GitWarp UI gallery"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(mode_button(
                                "System",
                                mode == ThemeMode::System,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    this.mode = ThemeMode::System;
                                    theme::set_mode(ThemeMode::System, cx);
                                }),
                            ))
                            .child(mode_button(
                                "Dark",
                                mode == ThemeMode::Dark,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    this.mode = ThemeMode::Dark;
                                    theme::set_mode(ThemeMode::Dark, cx);
                                }),
                            ))
                            .child(mode_button(
                                "Light",
                                mode == ThemeMode::Light,
                                colors,
                                cx.listener(|this, _, _, cx| {
                                    this.mode = ThemeMode::Light;
                                    theme::set_mode(ThemeMode::Light, cx);
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .id("gallery-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(colours_section(colors))
                    .child(type_section(colors))
                    .child(lanes_section(theme)),
            )
    }
}

fn mode_button(
    label: &'static str,
    active: bool,
    colors: ThemeColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(label)
        .h(CTL)
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(R_SM)
        .border_1()
        .border_color(if active {
            colors.selection
        } else {
            colors.border
        })
        .bg(if active {
            colors.selection
        } else {
            colors.surface
        })
        .text_color(if active {
            colors.selection_fg
        } else {
            colors.text
        })
        .cursor_pointer()
        .hover(move |style| {
            if active {
                style
            } else {
                style.bg(colors.hover)
            }
        })
        .on_click(on_click)
        .child(label)
}

fn heading(text: &'static str) -> impl IntoElement {
    div()
        .text_size(FS_MD)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn group_title(text: &'static str, colors: ThemeColors) -> impl IntoElement {
    div().text_size(FS_XS).text_color(colors.muted).child(text)
}

fn colours_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Colours"))
        .child(colour_group(
            "Surfaces",
            &[
                ("bg", colors.bg),
                ("surface", colors.surface),
                ("sidebar", colors.sidebar),
                ("hover", colors.hover),
                ("active", colors.active),
                ("selection", colors.selection),
                ("selection_fg", colors.selection_fg),
                ("selection_sub", colors.selection_sub),
                ("selection_inactive", colors.selection_inactive),
                ("border", colors.border),
                ("border_strong", colors.border_strong),
                ("overlay", colors.overlay),
            ],
            colors,
        ))
        .child(colour_group(
            "Text",
            &[
                ("text", colors.text),
                ("muted", colors.muted),
                ("disabled", colors.disabled),
            ],
            colors,
        ))
        .child(colour_group(
            "Accent & state",
            &[
                ("accent", colors.accent),
                ("accent_hover", colors.accent_hover),
                ("accent_fg", colors.accent_fg),
                ("accent_soft", colors.accent_soft),
                ("focus", colors.focus),
                ("error", colors.error),
                ("warning", colors.warning),
                ("success", colors.success),
                ("info", colors.info),
            ],
            colors,
        ))
        .child(colour_group(
            "Git status",
            &[
                ("st_added", colors.st_added),
                ("st_modified", colors.st_modified),
                ("st_deleted", colors.st_deleted),
                ("st_renamed", colors.st_renamed),
                ("st_untracked", colors.st_untracked),
                ("st_conflict", colors.st_conflict),
            ],
            colors,
        ))
        .child(colour_group(
            "Diff",
            &[
                ("diff_add_bg", colors.diff_add_bg),
                ("diff_add_strong", colors.diff_add_strong),
                ("diff_del_bg", colors.diff_del_bg),
                ("diff_del_strong", colors.diff_del_strong),
                ("diff_hunk_bg", colors.diff_hunk_bg),
            ],
            colors,
        ))
        .child(colour_group(
            "Code",
            &[
                ("code_kw", colors.code_kw),
                ("code_str", colors.code_str),
                ("code_num", colors.code_num),
                ("code_com", colors.code_com),
                ("code_ty", colors.code_ty),
                ("code_fn", colors.code_fn),
            ],
            colors,
        ))
}

fn colour_group(
    title: &'static str,
    items: &[(&'static str, Hsla)],
    colors: ThemeColors,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(group_title(title, colors))
        .child(
            div().flex().flex_wrap().gap(px(8.)).children(
                items
                    .iter()
                    .map(|&(name, color)| swatch(name, color, colors)),
            ),
        )
}

fn swatch(name: &'static str, color: Hsla, colors: ThemeColors) -> impl IntoElement {
    div()
        .w(px(150.))
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(32.))
                .h(px(20.))
                .flex_none()
                .rounded(R_XS)
                .bg(color)
                .border_1()
                .border_color(colors.border),
        )
        .child(div().text_size(FS_SM).text_color(colors.muted).child(name))
}

fn type_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Type"))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(type_row("FS_XS · 11", FS_XS, false, colors))
                .child(type_row("FS_SM · 12", FS_SM, false, colors))
                .child(type_row("FS_BASE · 13", FS_BASE, false, colors))
                .child(type_row("FS_MD · 14", FS_MD, false, colors))
                .child(type_row("FS_LG · 16", FS_LG, false, colors))
                .child(type_row("FS_XL · 20", FS_XL, false, colors))
                .child(type_row("MONO · 12", MONO, true, colors)),
        )
}

fn type_row(
    label: &'static str,
    size: Pixels,
    mono: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_size(FS_XS)
                .text_color(colors.muted)
                .child(label),
        )
        .child(
            div()
                .when(mono, |style| style.font_family(FONT_MONO))
                .text_size(size)
                .text_color(colors.text)
                .child("Ag 0123 — The quick brown fox"),
        )
}

fn lanes_section(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Graph lanes"))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children((0..16).map(|lane| {
                    div()
                        .w(DOT_R * 2.)
                        .h(DOT_R * 2.)
                        .flex_none()
                        .rounded(DOT_R)
                        .bg(theme.lane_color(lane))
                })),
        )
}

fn main() {
    Application::new().run(|cx: &mut App| {
        theme::init(cx);
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitWarp UI gallery".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                theme::follow_system(window, cx);
                cx.new(|_| Gallery {
                    mode: ThemeMode::System,
                })
            },
        )
        .expect("failed to open the gallery window");
        cx.activate(true);
    });
}
