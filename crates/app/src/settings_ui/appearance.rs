//! Settings → Appearance: theme cards, accent swatches, density, type sizes
//! and commit-graph style, with a live history preview.

use gpui::prelude::FluentBuilder;
use gpui::{div, px, AnyElement, Context, Hsla, IntoElement, ParentElement, Styled, Window};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use ui::metrics::{FONT_MONO, FS_SM, R_MD, R_SM};
use ui::theme::{ActiveTheme, ThemeColors};
use ui::{Icon, IconName};

use super::form;
use super::{group, heading, SettingsView};
use crate::format::{absolute_time, relative_time};
use crate::settings::{DateFormat, Density, ThemePref, ACCENT_VALUES};

/// Theme, the stable element-id suffix, and the catalog key of its label.
const THEMES: [(ThemePref, &str, &str); 3] = [
    (ThemePref::Dark, "Dark", "settings.appearance.theme.dark"),
    (ThemePref::Light, "Light", "settings.appearance.theme.light"),
    (
        ThemePref::System,
        "System",
        "settings.appearance.theme.system",
    ),
];

/// Density and the catalog key of its label.
const DENSITIES: [(Density, &str); 3] = [
    (Density::Compact, "settings.appearance.density.compact"),
    (Density::Normal, "settings.appearance.density.normal"),
    (Density::Roomy, "settings.appearance.density.roomy"),
];

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        ui::t!("settings.page.appearance"),
        ui::t!("settings.appearance.subtitle"),
    ));

    // Theme cards and accent swatches share one bordered list in the design.
    let cards = {
        let mut cards = div().w_full().flex().gap(px(12.)).p(px(12.));
        for (theme, id_name, label_key) in THEMES {
            let label = ui::i18n::t(label_key);
            let selected = settings.theme == theme;
            let entity = cx.entity();
            let thumb = theme_thumb(theme, selected, colors);
            let card = div()
                .id(format!("settings-theme-{id_name}"))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(8.))
                .rounded(R_MD)
                .border_1()
                .border_color(if selected {
                    colors.accent
                } else {
                    colors.border
                })
                .when(selected, |this| this.bg(colors.accent_soft))
                .cursor_pointer()
                .hover(move |style| style.border_color(colors.border_strong))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |view, cx| {
                        view.change_appearance(cx, |settings| settings.theme = theme);
                    });
                })
                .child(thumb)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(FS_SM)
                        .when(selected, |this| this.text_color(colors.text))
                        .child(
                            div()
                                .w(px(12.))
                                .h(px(12.))
                                .rounded(px(14.))
                                .border_1()
                                .border_color(if selected {
                                    colors.accent
                                } else {
                                    colors.border_strong
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(selected, |this| {
                                    this.child(
                                        div().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(colors.accent),
                                    )
                                }),
                        )
                        .child(label),
                );
            cards = cards.child(card);
        }
        cards
    };

    let accents = {
        let mut swatches = div().flex().items_center().gap(px(8.));
        for accent in ACCENT_VALUES {
            let selected = settings.accent == accent;
            let color = cx.theme().accent_color(accent.into());
            let entity = cx.entity();
            swatches = swatches.child(
                div()
                    .id(format!("settings-accent-{accent:?}"))
                    .w(px(28.))
                    .h(px(28.))
                    .rounded(px(14.))
                    .bg(color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |this| this.border_2().border_color(colors.text))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |view, cx| {
                            view.change_appearance(cx, |settings| settings.accent = accent);
                        });
                    })
                    .when(selected, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .size(px(14.))
                                .color(colors.accent_fg),
                        )
                    }),
            );
        }
        swatches
    };

    let accent_row = {
        let mut list = div()
            .w_full()
            .flex()
            .flex_col()
            .border_1()
            .rounded(R_SM)
            .overflow_hidden()
            .child(cards);
        list = list.child(div().w_full().h(px(1.)).bg(colors.border));
        list = list.child(form::row(
            "settings-accent-row",
            ui::t!("settings.appearance.accent.title"),
            ui::t!("settings.appearance.accent.desc"),
            accents,
            false,
        ));
        list
    };
    page = page.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .max_w(px(760.))
            .child(
                div()
                    .text_size(ui::metrics::FS_XS)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(ui::t!("settings.appearance.theme_heading")),
            )
            .child(accent_row),
    );

    let density = {
        let entity = cx.entity();
        form::segmented(
            "settings-density".into(),
            DENSITIES.iter().map(|(_, key)| ui::i18n::t(key)),
            DENSITIES
                .iter()
                .position(|(density, _)| *density == settings.density)
                .unwrap_or(1),
            false,
            move |index, _, cx| {
                let density = DENSITIES[index.min(DENSITIES.len() - 1)].0;
                entity.update(cx, |view, cx| {
                    view.change_appearance(cx, |settings| settings.density = density);
                });
            },
        )
    };
    let ui_size = {
        let entity = cx.entity();
        form::segmented(
            "settings-ui-size".into(),
            ["12", "13", "14"],
            usize::from(settings.ui_font_size.saturating_sub(12)),
            false,
            move |index, _, cx| {
                let size = 12 + index.min(2) as u8;
                entity.update(cx, |view, cx| {
                    view.change_appearance(cx, |settings| settings.ui_font_size = size);
                });
            },
        )
    };
    let mono_size = form::stepper(
        "settings-mono-size",
        settings.mono_font_size.to_string(),
        settings.mono_font_size > crate::settings::MONO_SIZE_MIN,
        settings.mono_font_size < crate::settings::MONO_SIZE_MAX,
        colors,
        {
            let entity = cx.entity();
            move |_, cx| {
                entity.update(cx, |view, cx| view.step_mono_size(-1, cx));
            }
        },
        {
            let entity = cx.entity();
            move |_, cx| {
                entity.update(cx, |view, cx| view.step_mono_size(1, cx));
            }
        },
    );

    page = page.child(group(
        ui::t!("settings.appearance.group.lists"),
        vec![
            form::row(
                "settings-density-row",
                ui::t!("settings.appearance.density.title"),
                ui::t!("settings.appearance.density.desc"),
                density,
                false,
            ),
            form::row(
                "settings-ui-size-row",
                ui::t!("settings.appearance.ui_size.title"),
                ui::t!("settings.appearance.ui_size.desc"),
                ui_size,
                false,
            ),
            form::row(
                "settings-mono-size-row",
                ui::t!("settings.appearance.mono_size.title"),
                ui::t!("settings.appearance.mono_size.desc"),
                mono_size,
                false,
            ),
        ],
    ));

    let colored = {
        let entity = cx.entity();
        form::switch(
            "settings-graph-colored".into(),
            settings.graph_colored,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.graph_colored = checked);
                });
            },
        )
    };
    let line_width = {
        let entity = cx.entity();
        form::segmented(
            "settings-line-width".into(),
            [
                ui::t!("settings.appearance.px", n = 1),
                ui::t!("settings.appearance.px", n = 2),
                ui::t!("settings.appearance.px", n = 3),
            ],
            usize::from(settings.graph_line_width.saturating_sub(1)),
            false,
            move |index, _, cx| {
                let width = (index.min(2) + 1) as u8;
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.graph_line_width = width);
                });
            },
        )
    };
    let dates = {
        let entity = cx.entity();
        let selected = usize::from(settings.date_format == DateFormat::Absolute);
        form::segmented(
            "settings-date-format".into(),
            [
                ui::t!("settings.appearance.dates.relative"),
                ui::t!("settings.appearance.dates.absolute"),
            ],
            selected,
            false,
            move |index, _, cx| {
                let format = if index == 1 {
                    DateFormat::Absolute
                } else {
                    DateFormat::Relative
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.date_format = format);
                });
            },
        )
    };
    page = page.child(group(
        ui::t!("settings.appearance.group.graph"),
        vec![
            form::row(
                "settings-colored-row",
                ui::t!("settings.appearance.colored.title"),
                ui::t!("settings.appearance.colored.desc"),
                colored,
                false,
            ),
            form::row(
                "settings-line-width-row",
                ui::t!("settings.appearance.line_width.title"),
                "",
                line_width,
                false,
            ),
            form::row(
                "settings-dates-row",
                ui::t!("settings.appearance.dates.title"),
                ui::t!("settings.appearance.dates.desc"),
                dates,
                false,
            ),
        ],
    ));

    let preview = history_preview(&settings, colors, cx);
    page = page.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .max_w(px(760.))
            .child(
                div()
                    .text_size(ui::metrics::FS_XS)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(ui::t!("settings.diff.preview")),
            )
            .child(
                div()
                    .border_1()
                    .rounded(R_MD)
                    .overflow_hidden()
                    .bg(colors.bg)
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(6.))
                            .border_b_1()
                            .border_color(colors.border)
                            .bg(colors.surface)
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(ui::t!("settings.appearance.preview_header")),
                    )
                    .child(preview),
            ),
    );

    page.into_any_element()
}

fn theme_thumb(theme: ThemePref, _selected: bool, colors: ThemeColors) -> AnyElement {
    // Two columns of bars; System is split light/dark like the design.
    let dark = theme == ThemePref::Dark;
    let (thumb_bg, side, bar, first) = if dark {
        (0x181b20, 0x15171c, 0x4a515d, 0x6ea4f7)
    } else {
        (0xffffff, 0xfbfcfe, 0xb9c0cb, 0x2a66d6)
    };
    let bars = |first_color: u32| {
        let mut column = div().flex().flex_col().gap(px(4.)).p(px(6.)).flex_1();
        for index in 0..3 {
            column = column.child(div().h(px(4.)).rounded(px(2.)).bg(gpui::rgb(if index == 0 {
                first_color
            } else {
                bar
            })));
        }
        column
    };
    let body = if theme == ThemePref::System {
        div()
            .h(px(64.))
            .rounded(R_SM)
            .overflow_hidden()
            .flex()
            .border_1()
            .border_color(colors.border)
            .child(div().flex_1().bg(gpui::rgb(0xffffff)).child(bars(0x2a66d6)))
            .child(div().flex_1().bg(gpui::rgb(0x181b20)).child(bars(0x6ea4f7)))
    } else {
        div()
            .h(px(64.))
            .rounded(R_SM)
            .overflow_hidden()
            .flex()
            .border_1()
            .border_color(colors.border)
            .bg(gpui::rgb(thumb_bg))
            .child(div().w(px(56.)).h_full().bg(gpui::rgb(side)))
            .child(bars(first))
    };
    body.into_any_element()
}

fn history_preview(
    settings: &crate::settings::Settings,
    colors: ThemeColors,
    _cx: &Context<SettingsView>,
) -> AnyElement {
    let row_height = settings.density.row_height();
    let rows = [
        (
            "8d41f6e",
            "fix(settings): list both reasons when bundled and system git fail",
            600,
            true,
        ),
        (
            "b7a3d15",
            "feat(settings): validate custom git path, require git ≥ 2.31",
            1100,
            false,
        ),
        (
            "c19e0a4",
            "refactor(history): extract lane collapse into its own module",
            2900,
            false,
        ),
    ];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut list = div().flex().flex_col().py(px(4.));
    for (hash, summary, minutes, selected) in rows {
        let time = now.saturating_sub(minutes * 60);
        let label = match settings.date_format {
            DateFormat::Relative => relative_time(now, time),
            DateFormat::Absolute => absolute_time(time as i64),
        };
        let lane = preview_lane(row_height, settings, colors, hash);
        list = list.child(
            div()
                .h(px(row_height))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(10.))
                .when(selected, |this| this.bg(colors.selection))
                .child(lane)
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(f32::from(settings.ui_font_size) - 1.))
                        .text_color(colors.muted)
                        .child(hash),
                )
                .child(div().flex_1().min_w_0().truncate().child(summary))
                .child(div().text_size(FS_SM).text_color(colors.muted).child(label)),
        );
    }
    let code_size = px(f32::from(settings.mono_font_size));
    list = list.child(
        div()
            .mt(px(4.))
            .pt(px(6.))
            .px(px(12.))
            .border_t_1()
            .border_color(colors.border)
            .font_family(FONT_MONO)
            .text_size(code_size)
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().text_color(colors.code_kw).child("pub fn"))
            .child(div().text_color(colors.code_fn).child("lane_x"))
            .child(
                div()
                    .text_color(colors.text)
                    .child("(&self, col: usize) -> f32"),
            ),
    );
    list.into_any_element()
}

fn preview_lane(
    row_height: f32,
    settings: &crate::settings::Settings,
    colors: ThemeColors,
    hash: &str,
) -> AnyElement {
    let width = ui::metrics::LANE_W * 2. + px(2.);
    let lane = |x: f32, hue: Hsla, dot: bool| {
        let mut element = div()
            .absolute()
            .top(px(0.))
            .left(px(x))
            .w(px(settings.graph_line_width as f32))
            .h(px(row_height))
            .bg(hue);
        if dot {
            element = element.child(
                div()
                    .absolute()
                    .top(px(row_height / 2. - 4.))
                    .left(px(-3.))
                    .w(px(9.))
                    .h(px(9.))
                    .rounded(px(4.5))
                    .bg(hue),
            );
        }
        element
    };
    let neutral = colors.muted;
    let (left_color, right_color) = if settings.graph_colored {
        (colors.info, colors.st_untracked)
    } else {
        (neutral, neutral)
    };
    let dot_left = hash.starts_with('8') || hash.starts_with('c');
    div()
        .relative()
        .w(width)
        .h(px(row_height))
        .flex_none()
        .child(lane(8., left_color, dot_left))
        .child(lane(22., right_color, !dot_left))
        .into_any_element()
}

impl SettingsView {
    pub(super) fn step_mono_size(&mut self, delta: i16, cx: &mut Context<Self>) {
        let current = self.settings(cx).mono_font_size;
        let next = (i16::from(current) + delta).clamp(
            i16::from(crate::settings::MONO_SIZE_MIN),
            i16::from(crate::settings::MONO_SIZE_MAX),
        ) as u8;
        self.set_mono_size(next, cx);
    }

    pub(super) fn set_mono_size(&mut self, size: u8, cx: &mut Context<Self>) {
        let size = size.clamp(
            crate::settings::MONO_SIZE_MIN,
            crate::settings::MONO_SIZE_MAX,
        );
        self.change_appearance(cx, |settings| settings.mono_font_size = size);
    }
}
