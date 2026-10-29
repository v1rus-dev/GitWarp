//! Visual catalogue of the design system tokens.

use gpui::{
    div, prelude::*, px, size, AnyView, App, Application, Bounds, ClickEvent, FocusHandle,
    FontWeight, Hsla, IntoElement, Pixels, Render, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use ui::focus::RingOffset;
use ui::metrics::{
    BAR, CTL, DOT_R, FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_MD, FS_SM, FS_XL, FS_XS, MONO, R_LG,
    R_MD, R_SM, R_XS,
};
use ui::theme::{self, ActiveTheme, Theme, ThemeColors, ThemeMode};
use ui::{actions, focus, overlay, Icon, IconName};

struct Gallery {
    mode: ThemeMode,
    focus_a: FocusHandle,
    focus_b: FocusHandle,
    focus_c: FocusHandle,
}

impl Gallery {
    fn new(mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        Self {
            mode,
            focus_a: cx.focus_handle().tab_index(1).tab_stop(true),
            focus_b: cx.focus_handle().tab_index(2).tab_stop(true),
            focus_c: cx.focus_handle().tab_index(3).tab_stop(true),
        }
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = theme.colors;
        let mode = self.mode;

        let content = div()
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
                    .child(fonts_section(colors))
                    .child(icons_section(colors))
                    .child(interaction_section(self, colors, window, cx))
                    .child(lanes_section(&theme)),
            );

        let content = focus::track_pointer(actions::tab_navigation(content));
        content.child(overlay::render_overlays(window, cx))
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

fn fonts_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Fonts"))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(font_row(
                    "Inter 400",
                    false,
                    FontWeight::NORMAL,
                    false,
                    colors,
                ))
                .child(font_row(
                    "Inter 500",
                    false,
                    FontWeight::MEDIUM,
                    false,
                    colors,
                ))
                .child(font_row(
                    "Inter 600",
                    false,
                    FontWeight::SEMIBOLD,
                    false,
                    colors,
                ))
                .child(font_row(
                    "Inter 700",
                    false,
                    FontWeight::BOLD,
                    false,
                    colors,
                ))
                .child(font_row(
                    "JetBrains Mono Regular",
                    true,
                    FontWeight::NORMAL,
                    false,
                    colors,
                ))
                .child(font_row(
                    "JetBrains Mono Bold",
                    true,
                    FontWeight::BOLD,
                    false,
                    colors,
                ))
                .child(font_row(
                    "JetBrains Mono Italic",
                    true,
                    FontWeight::NORMAL,
                    true,
                    colors,
                )),
        )
}

fn font_row(
    label: &'static str,
    mono: bool,
    weight: FontWeight,
    italic: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(170.))
                .flex_none()
                .text_size(FS_XS)
                .text_color(colors.muted)
                .child(label),
        )
        .child(
            div()
                .font_family(if mono { FONT_MONO } else { FONT_UI })
                .text_size(if mono { MONO } else { FS_BASE })
                .font_weight(weight)
                .when(italic, |style| style.italic())
                .text_color(colors.text)
                .child("GitWarp 0123 — The quick brown fox"),
        )
}

fn icons_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Icons"))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(12.))
                .children(IconName::ALL.map(|name| {
                    div()
                        .w(px(78.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.))
                        .child(Icon::new(name).size(px(16.)).color(colors.text))
                        .child(
                            div()
                                .text_size(FS_XS)
                                .text_color(colors.muted)
                                .child(kebab(name)),
                        )
                })),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(12.))
                .children([10_f32, 12., 14., 16., 22.].map(|size| {
                    Icon::new(IconName::Branch)
                        .size(px(size))
                        .color(colors.text)
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(Icon::new(IconName::Warn).color(colors.warning))
                .child(Icon::new(IconName::Err).color(colors.error))
                .child(Icon::new(IconName::Check).color(colors.success))
                .child(Icon::new(IconName::Info).color(colors.info)),
        )
}

/// The file stem of an icon's asset path, e.g. `chev-r`.
fn kebab(name: IconName) -> &'static str {
    name.path()
        .trim_start_matches("icons/")
        .trim_end_matches(".svg")
}

fn interaction_section(
    gallery: &Gallery,
    colors: ThemeColors,
    window: &Window,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Interaction"))
        .child(
            div()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(format!(
                    "Keyboard modality: {}",
                    if focus::is_keyboard(cx) { "on" } else { "off" }
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(focus_box(
                    "A",
                    &gallery.focus_a,
                    RingOffset::Outset1,
                    colors,
                    window,
                    cx,
                ))
                .child(focus_box(
                    "B",
                    &gallery.focus_b,
                    RingOffset::Outset2,
                    colors,
                    window,
                    cx,
                ))
                .child(focus_box(
                    "C",
                    &gallery.focus_c,
                    RingOffset::Inset2,
                    colors,
                    window,
                    cx,
                ))
                .child(open_modal_button(colors, cx)),
        )
}

fn focus_box(
    label: &'static str,
    handle: &FocusHandle,
    offset: RingOffset,
    colors: ThemeColors,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let visible = focus::focus_visible(handle, window, cx);
    let mut box_ = div()
        .id(label)
        .w(px(80.))
        .h(px(26.))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded(R_SM)
        .border_1()
        .border_color(colors.border_strong)
        .bg(colors.surface)
        .track_focus(handle)
        .child(label);
    if visible {
        box_ = box_.child(focus::focus_ring(offset, R_SM, cx));
    }
    box_
}

fn open_modal_button(colors: ThemeColors, cx: &mut Context<Gallery>) -> impl IntoElement {
    div()
        .id("open-modal")
        .h(CTL)
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(R_SM)
        .border_1()
        .border_color(colors.border_strong)
        .bg(colors.surface)
        .text_color(colors.text)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_click(cx.listener(|_, _, window, cx| {
            let panel = cx.new(ModalPanel::new);
            let focus = panel.read(cx).focus.clone();
            overlay::open_modal(AnyView::from(panel), window, cx);
            window.focus(&focus);
        }))
        .child("Open modal")
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

struct ModalPanel {
    focus: FocusHandle,
}

impl ModalPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Render for ModalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .w(px(360.))
            .h(px(160.))
            .rounded(R_LG)
            .border_1()
            .border_color(colors.border_strong)
            .bg(colors.surface)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .track_focus(&self.focus)
            .child("Modal layer — Esc or click outside to close")
            .child(open_popover_button(colors, cx))
    }
}

struct PopoverPanel {
    focus: FocusHandle,
}

impl PopoverPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Render for PopoverPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .w(px(220.))
            .h(px(80.))
            .rounded(R_MD)
            .border_1()
            .border_color(colors.border_strong)
            .bg(colors.surface)
            .p(px(12.))
            .flex()
            .items_center()
            .track_focus(&self.focus)
            .child("Popover layer — Esc or click outside to close")
    }
}

fn open_popover_button(colors: ThemeColors, cx: &mut Context<ModalPanel>) -> impl IntoElement {
    div()
        .id("open-popover")
        .h(CTL)
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(R_SM)
        .border_1()
        .border_color(colors.border_strong)
        .bg(colors.bg)
        .text_color(colors.text)
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover))
        .on_click(cx.listener(|_, event: &ClickEvent, window, cx| {
            let panel = cx.new(PopoverPanel::new);
            let focus = panel.read(cx).focus.clone();
            overlay::open_popover(event.position(), AnyView::from(panel), window, cx);
            window.focus(&focus);
        }))
        .child("Open nested popover")
}

/// `--theme dark|light|system`; anything else stays on the system theme.
fn parse_mode(mut args: impl Iterator<Item = String>) -> ThemeMode {
    while let Some(arg) = args.next() {
        if arg == "--theme" {
            return match args.next().as_deref() {
                Some("dark") => ThemeMode::Dark,
                Some("light") => ThemeMode::Light,
                _ => ThemeMode::System,
            };
        }
    }
    ThemeMode::System
}

fn main() {
    let mode = parse_mode(std::env::args().skip(1));
    Application::new()
        .with_assets(ui::Assets)
        .run(move |cx: &mut App| {
            if let Err(e) = ui::init(cx) {
                eprintln!("failed to load the bundled fonts: {e}");
            }
            theme::set_mode(mode, cx);
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
                    cx.new(|cx| Gallery::new(mode, cx))
                },
            )
            .expect("failed to open the gallery window");
            cx.activate(true);
        });
}
