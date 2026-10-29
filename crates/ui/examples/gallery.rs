//! Visual catalogue of the design system tokens.

use gpui::{
    div, prelude::*, px, size, AnyElement, AnyView, App, Application, Bounds, ClickEvent, Div,
    FocusHandle, FontWeight, Hsla, IntoElement, Pixels, Render, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};

use ui::focus::RingOffset;
use ui::metrics::{
    BAR, CTL, DOT_R, FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_MD, FS_SM, FS_XL, FS_XS, MONO, R_LG,
    R_MD, R_SM, R_XS,
};
use ui::theme::{self, ActiveTheme, Theme, ThemeColors, ThemeMode};
use ui::{
    actions, focus, overlay, Avatar, Banner, BannerKind, Button, ButtonSize, Checkbox, Chip,
    ChipKind, CountPill, Counter, EmptyState, Icon, IconName, Kbd, Progress, Radio, RefBadge,
    RefKind, SearchField, SegmentItem, Segmented, Skeleton, Spinner, StatusGlyph, Switch, Tab,
    Tabs, TextInput,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Colours,
    Type,
    Fonts,
    Icons,
    Interaction,
    Buttons,
    Badges,
    Toggles,
    Tabs,
    Progress,
    Banners,
    Inputs,
}

impl Section {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "colours" => Self::Colours,
            "type" => Self::Type,
            "fonts" => Self::Fonts,
            "icons" => Self::Icons,
            "interaction" => Self::Interaction,
            "buttons" => Self::Buttons,
            "badges" => Self::Badges,
            "toggles" => Self::Toggles,
            "tabs" => Self::Tabs,
            "progress" => Self::Progress,
            "banners" => Self::Banners,
            "inputs" => Self::Inputs,
            _ => return None,
        })
    }
}

struct Gallery {
    mode: ThemeMode,
    section: Option<Section>,
    focus_a: FocusHandle,
    focus_b: FocusHandle,
    focus_c: FocusHandle,
    checkbox_off: bool,
    checkbox_on: bool,
    checkbox_mixed: bool,
    radio: usize,
    switch_a: bool,
    switch_b: bool,
    segmented: usize,
    tab: usize,
    tabs: Vec<SharedString>,
}

impl Gallery {
    fn new(mode: ThemeMode, section: Option<Section>, cx: &mut Context<Self>) -> Self {
        Self {
            mode,
            section,
            focus_a: cx.focus_handle().tab_index(1).tab_stop(true),
            focus_b: cx.focus_handle().tab_index(2).tab_stop(true),
            focus_c: cx.focus_handle().tab_index(3).tab_stop(true),
            checkbox_off: false,
            checkbox_on: true,
            checkbox_mixed: true,
            radio: 0,
            switch_a: true,
            switch_b: false,
            segmented: 1,
            tab: 0,
            tabs: ["History", "Changes", "Hover"]
                .into_iter()
                .map(SharedString::from)
                .collect(),
        }
    }

    fn shows(&self, section: Section) -> bool {
        self.section.is_none() || self.section == Some(section)
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = theme.colors;
        let mode = self.mode;

        let mut sections: Vec<AnyElement> = Vec::new();
        if self.shows(Section::Colours) {
            sections.push(colours_section(colors).into_any_element());
        }
        if self.shows(Section::Type) {
            sections.push(type_section(colors).into_any_element());
        }
        if self.shows(Section::Fonts) {
            sections.push(fonts_section(colors).into_any_element());
        }
        if self.shows(Section::Icons) {
            sections.push(icons_section(colors).into_any_element());
        }
        if self.shows(Section::Buttons) {
            sections.push(buttons_section(colors).into_any_element());
        }
        if self.shows(Section::Badges) {
            sections.push(badges_section(colors).into_any_element());
        }
        if self.shows(Section::Toggles) {
            sections.push(toggles_section(self, colors, cx).into_any_element());
        }
        if self.shows(Section::Tabs) {
            sections.push(segmented_tabs_section(self, colors, cx).into_any_element());
        }
        if self.shows(Section::Progress) {
            sections.push(progress_section(colors).into_any_element());
        }
        if self.shows(Section::Banners) {
            sections.push(banners_section(colors, cx).into_any_element());
        }
        if self.shows(Section::Inputs) {
            sections.push(inputs_section(colors, window, cx).into_any_element());
        }
        if self.shows(Section::Interaction) {
            sections.push(interaction_section(self, colors, window, cx).into_any_element());
        }
        if self.section.is_none() {
            sections.push(lanes_section(&theme).into_any_element());
        }

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
                    .children(sections),
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

fn card(colors: ThemeColors) -> Div {
    div()
        .p(px(16.))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .rounded(R_MD)
        .flex()
        .flex_col()
        .gap(px(12.))
}

fn component_row(label: &'static str, colors: ThemeColors) -> Div {
    div().flex().items_center().gap(px(8.)).child(
        div()
            .w(px(90.))
            .flex_none()
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(label),
    )
}

fn buttons_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Buttons"))
        .child(
            card(colors)
                .child(
                    component_row("Secondary", colors)
                        .child(Button::new("btn-sec").label("Fetch"))
                        .child(Button::new("btn-sec-dis").label("Fetch").disabled(true))
                        .child(
                            Button::new("btn-sec-icon")
                                .icon(IconName::Branch)
                                .label("Fetch"),
                        )
                        .child(
                            Button::new("btn-sec-only")
                                .icon_only(IconName::Gear)
                                .tooltip("Settings"),
                        )
                        .child(
                            Button::new("btn-sec-sm")
                                .label("Fetch")
                                .size(ButtonSize::Small),
                        ),
                )
                .child(
                    component_row("Primary", colors)
                        .child(Button::primary("btn-pri").label("Commit"))
                        .child(
                            Button::primary("btn-pri-dis")
                                .label("Commit")
                                .disabled(true),
                        )
                        .child(
                            Button::primary("btn-pri-icon")
                                .icon(IconName::Check)
                                .label("Commit"),
                        )
                        .child(
                            Button::primary("btn-pri-only")
                                .icon_only(IconName::Check)
                                .tooltip("Commit"),
                        )
                        .child(
                            Button::primary("btn-pri-sm")
                                .label("Commit")
                                .size(ButtonSize::Small),
                        ),
                )
                .child(
                    component_row("Danger", colors)
                        .child(Button::danger("btn-danger").label("Delete…"))
                        .child(
                            Button::danger("btn-danger-dis")
                                .label("Delete…")
                                .disabled(true),
                        )
                        .child(
                            Button::danger("btn-danger-icon")
                                .icon(IconName::Trash)
                                .label("Delete"),
                        )
                        .child(
                            Button::danger("btn-danger-only")
                                .icon_only(IconName::Trash)
                                .tooltip("Delete"),
                        )
                        .child(
                            Button::danger("btn-danger-sm")
                                .label("Delete")
                                .size(ButtonSize::Small),
                        ),
                )
                .child(
                    component_row("Ghost", colors)
                        .child(Button::ghost("btn-ghost").label("Cancel"))
                        .child(
                            Button::ghost("btn-ghost-dis")
                                .label("Cancel")
                                .disabled(true),
                        )
                        .child(
                            Button::ghost("btn-ghost-icon")
                                .icon(IconName::X)
                                .label("Cancel"),
                        )
                        .child(
                            Button::ghost("btn-ghost-only")
                                .icon_only(IconName::X)
                                .tooltip("Close"),
                        )
                        .child(
                            Button::ghost("btn-ghost-sm")
                                .label("Cancel")
                                .size(ButtonSize::Small),
                        ),
                )
                .child(
                    component_row("Tool", colors)
                        .child(
                            Button::tool("btn-tool")
                                .icon(IconName::Fetch)
                                .label("Fetch"),
                        )
                        .child(
                            Button::tool("btn-tool-on")
                                .icon(IconName::Up)
                                .label("Push")
                                .selected(true),
                        )
                        .child(
                            Button::tool("btn-tool-dis")
                                .icon(IconName::Fetch)
                                .label("Fetch")
                                .disabled(true),
                        )
                        .child(
                            Button::tool("btn-tool-only")
                                .icon_only(IconName::Gear)
                                .tooltip("Settings"),
                        )
                        .child(
                            Button::tool("btn-tool-sm")
                                .label("Stash")
                                .size(ButtonSize::Small),
                        ),
                )
                .child(
                    component_row("Toolbar", colors)
                        .child(
                            Button::tool("btn-bar-fetch")
                                .icon(IconName::Fetch)
                                .label("Fetch"),
                        )
                        .child(
                            Button::tool("btn-bar-pull")
                                .icon(IconName::Down)
                                .label("Pull"),
                        )
                        .child(
                            Button::tool("btn-bar-push")
                                .icon(IconName::Up)
                                .label("Push")
                                .counter(Counter::new("3").accent()),
                        )
                        .child(
                            Button::tool("btn-bar-stash")
                                .icon(IconName::Stash)
                                .label("Stash")
                                .counter(Counter::new("1")),
                        ),
                ),
        )
}

fn badges_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Badges & glyphs"))
        .child(
            card(colors)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(RefBadge::new("ref-local", "feat/lanes"))
                        .child(RefBadge::new("ref-remote", "origin/main").kind(RefKind::Remote))
                        .child(RefBadge::new("ref-tag", "v0.3.0").kind(RefKind::Tag))
                        .child(RefBadge::new("ref-stash", "stash@{0}").kind(RefKind::Stash))
                        .child(RefBadge::new("ref-head", "HEAD").kind(RefKind::Head))
                        .child(Counter::new("3"))
                        .child(Counter::new("6").accent())
                        .child(CountPill::new("12"))
                        .child(Kbd::new("3"))
                        .child(Kbd::new("Ctrl"))
                        .child(StatusGlyph::new('A'))
                        .child(StatusGlyph::new('M'))
                        .child(StatusGlyph::new('D'))
                        .child(StatusGlyph::new('R'))
                        .child(StatusGlyph::new('?'))
                        .child(StatusGlyph::new('U')),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(Chip::new("chip-none", "Local changes"))
                        .child(
                            Chip::new("chip-ok", "Using git 2.56.0")
                                .kind(ChipKind::Ok)
                                .icon(IconName::Check),
                        )
                        .child(
                            Chip::new("chip-warn", "Skipped")
                                .kind(ChipKind::Warn)
                                .icon(IconName::Warn),
                        )
                        .child(
                            Chip::new("chip-err", "Rejected")
                                .kind(ChipKind::Err)
                                .icon(IconName::Err),
                        )
                        .child(
                            Chip::new("chip-acc", "Fetched")
                                .kind(ChipKind::Acc)
                                .icon(IconName::Down),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(Avatar::new("mira@example.com", px(24.)))
                        .child(Avatar::new("feat/lane@example.com", px(32.)))
                        .child(Avatar::new("pablo@example.com", px(44.)))
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("Avatar fallback colours"),
                        ),
                ),
        )
}

fn toggles_section(
    gallery: &Gallery,
    colors: ThemeColors,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    let view = cx.entity();

    let off = gallery.checkbox_off;
    let checkbox_off = {
        let view = view.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.checkbox_off = next;
                cx.notify();
            });
        }
    };
    let on = gallery.checkbox_on;
    let checkbox_on = {
        let view = view.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.checkbox_on = next;
                cx.notify();
            });
        }
    };
    let mixed = gallery.checkbox_mixed;
    let checkbox_mixed = {
        let view = view.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.checkbox_mixed = next;
                cx.notify();
            });
        }
    };

    let radio_for = |index: usize, label: &'static str, id: &'static str, checked: bool| {
        let view = view.clone();
        Radio::new(id)
            .label(label)
            .checked(checked)
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| {
                    this.radio = index;
                    cx.notify();
                });
            })
    };

    let switch_a = gallery.switch_a;
    let toggle_a = {
        let view = view.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.switch_a = next;
                cx.notify();
            });
        }
    };
    let switch_b = gallery.switch_b;
    let toggle_b = {
        let view = view.clone();
        move |next: bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.switch_b = next;
                cx.notify();
            });
        }
    };

    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Toggles"))
        .child(
            card(colors)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(
                            Checkbox::new("chk-off")
                                .label("Off")
                                .checked(off)
                                .on_toggle(checkbox_off),
                        )
                        .child(
                            Checkbox::new("chk-on")
                                .label("On")
                                .checked(on)
                                .on_toggle(checkbox_on),
                        )
                        .child(
                            Checkbox::new("chk-mixed")
                                .label("Mixed")
                                .checked(mixed)
                                .mixed(mixed)
                                .on_toggle(checkbox_mixed),
                        )
                        .child(
                            Checkbox::new("chk-dis")
                                .label("Disabled")
                                .checked(true)
                                .disabled(true),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .child(radio_for(0, "System", "radio-system", gallery.radio == 0))
                        .child(radio_for(1, "Custom", "radio-custom", gallery.radio == 1))
                        .child(
                            Radio::new("radio-dis")
                                .label("Disabled")
                                .checked(false)
                                .disabled(true),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(24.))
                        .child(
                            Switch::new("switch-a")
                                .label("Amend")
                                .checked(switch_a)
                                .on_toggle(toggle_a),
                        )
                        .child(
                            Switch::new("switch-b")
                                .label("Sign commit")
                                .checked(switch_b)
                                .on_toggle(toggle_b),
                        )
                        .child(
                            Switch::new("switch-dis")
                                .label("Disabled")
                                .checked(false)
                                .disabled(true),
                        ),
                ),
        )
}

fn segmented_tabs_section(
    gallery: &Gallery,
    colors: ThemeColors,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    let view = cx.entity();
    let select = {
        let view = view.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.segmented = index;
                cx.notify();
            });
        }
    };
    let select_tab = {
        let view = view.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.tab = index;
                cx.notify();
            });
        }
    };
    let close_tab = {
        let view = view.clone();
        move |index: usize, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                if index < this.tabs.len() {
                    this.tabs.remove(index);
                }
                if !this.tabs.is_empty() && this.tab >= this.tabs.len() {
                    this.tab = this.tabs.len() - 1;
                }
                cx.notify();
            });
        }
    };

    let tabs = Tabs::new("tabs")
        .selected(gallery.tab)
        .tabs(gallery.tabs.iter().cloned().map(|label| {
            if label == "Changes" {
                Tab::new(label).closable()
            } else {
                Tab::new(label)
            }
        }))
        .on_select(select_tab)
        .on_close(close_tab);

    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Segmented & tabs"))
        .child(
            card(colors)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .child(
                            Segmented::new("seg")
                                .items([
                                    SegmentItem::new("History"),
                                    SegmentItem::new("Changes"),
                                    SegmentItem::new("Hover"),
                                ])
                                .selected(gallery.segmented)
                                .on_select(select.clone()),
                        )
                        .child(
                            Segmented::new("seg-ico")
                                .items([
                                    SegmentItem::icon(IconName::Unified),
                                    SegmentItem::icon(IconName::Split),
                                    SegmentItem::icon(IconName::PanelR),
                                ])
                                .selected(0)
                                .small()
                                .on_select(select.clone()),
                        )
                        .child(
                            Segmented::new("seg-dis")
                                .items([SegmentItem::new("One"), SegmentItem::new("Two")])
                                .selected(0)
                                .disabled(true),
                        ),
                )
                .child(tabs),
        )
}

fn progress_section(colors: ThemeColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Progress"))
        .child(
            card(colors)
                .child(
                    component_row("Progress 62%", colors)
                        .child(Progress::new("progress-main", 0.62))
                        .child(div().text_size(FS_SM).text_color(colors.muted).child("62%")),
                )
                .child(
                    component_row("Dialog", colors)
                        .child(Progress::new("progress-dialog", 0.40).dialog()),
                )
                .child(
                    component_row("Indeterminate", colors)
                        .child(Progress::new("progress-indet", 0.).indeterminate()),
                )
                .child(
                    component_row("Spinners", colors)
                        .child(Spinner::new("spinner-12", px(12.)))
                        .child(Spinner::new("spinner-14", px(14.)))
                        .child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("12 · 14"),
                        ),
                )
                .child(
                    component_row("Skeleton", colors)
                        .child(Skeleton::new("skel-1", px(160.)))
                        .child(Skeleton::new("skel-2", px(220.))),
                ),
        )
}

fn banners_section(colors: ThemeColors, cx: &mut Context<Gallery>) -> impl IntoElement {
    let retry = {
        let view = cx.entity();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |_, cx| cx.notify());
        }
    };
    let cancel = {
        let view = cx.entity();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |_, cx| cx.notify());
        }
    };

    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Banners & empty states"))
        .child(
            card(colors)
                .child(
                    Banner::new(BannerKind::Ok, "Using git 2.56.0 (bundled)")
                        .body("The bundled binary is current."),
                )
                .child(
                    Banner::new(BannerKind::Info, "Fetching in the background…")
                        .body("2 new branches in origin."),
                )
                .child(Banner::new(BannerKind::Warn, "Bundled git failed to start").list([
                    "Falling back to the system git",
                    "Some features may be unavailable",
                ]))
                .child(Banner::new(
                    BannerKind::Error,
                    "git 2.28.1 is older than 2.31",
                )),
        )
        .child(
            div()
                .flex()
                .gap(px(16.))
                .child(
                    div()
                        .flex_1()
                        .border_1()
                        .border_color(colors.border)
                        .rounded(R_MD)
                        .bg(colors.surface)
                        .child(
                            EmptyState::new("No repository open")
                                .text("Open a repository or clone one to get started.")
                                .icon(IconName::Folder)
                                .actions(
                                    div()
                                        .flex()
                                        .gap(px(8.))
                                        .child(
                                            Button::ghost("empty-cancel")
                                                .label("Cancel")
                                                .on_click(cancel),
                                        )
                                        .child(
                                            Button::primary("empty-retry")
                                                .label("Open…")
                                                .on_click(retry),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .border_1()
                        .border_color(colors.border)
                        .rounded(R_MD)
                        .bg(colors.surface)
                        .child(
                            EmptyState::new("Repository could not be opened")
                                .text("The path does not contain a git repository.")
                                .error("fatal: not a git repository (or any parent)\nstopped at filesystem boundary"),
                        ),
                ),
        )
}

fn input_field(
    label: &'static str,
    colors: ThemeColors,
    input: impl IntoElement,
) -> impl IntoElement {
    div()
        .w(px(340.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(FS_SM).text_color(colors.muted).child(label))
        .child(input)
}

fn inputs_section(
    colors: ThemeColors,
    window: &mut Window,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    let empty = window.use_keyed_state("gallery-input-empty", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_placeholder("e.g. feature/my-branch", cx);
        input
    });
    let filled = window.use_keyed_state("gallery-input-filled", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_text("feat (graph): clip lanes", cx);
        input
    });
    let error = window.use_keyed_state("gallery-input-error", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_text("not-a-ref", cx);
        input.set_error(true, cx);
        input
    });
    let disabled = window.use_keyed_state("gallery-input-disabled", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_placeholder("Disabled", cx);
        input.set_disabled(true, cx);
        input
    });
    let limited = window.use_keyed_state("gallery-input-limited", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_max_len(Some(24), cx);
        input.set_placeholder("Up to 24 characters", cx);
        input
    });
    let search = window.use_keyed_state("gallery-input-search", cx, |window, cx| {
        let mut input = TextInput::new(window, cx);
        input.set_placeholder("Filter branches, tags…", cx);
        input
    });

    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Inputs"))
        .child(
            card(colors).child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(24.))
                    .child(input_field("Default", colors, empty))
                    .child(input_field("Filled", colors, filled))
                    .child(
                        div()
                            .w(px(340.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(FS_SM)
                                    .text_color(colors.muted)
                                    .child("Error"),
                            )
                            .child(error)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(FS_SM)
                                    .text_color(colors.error)
                                    .child(
                                        Icon::new(IconName::Err).size(px(12.)).color(colors.error),
                                    )
                                    .child("Branch names cannot contain '~'"),
                            ),
                    )
                    .child(input_field("Disabled", colors, disabled))
                    .child(input_field("Max length 24", colors, limited))
                    .child(input_field(
                        "Search",
                        colors,
                        SearchField::new("gallery-search", search).hint("Ctrl K"),
                    )),
            ),
        )
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

/// `--theme dark|light|system`, `--section <name>`; unknown values fall back.
fn parse_args() -> (ThemeMode, Option<Section>) {
    let mut mode = ThemeMode::System;
    let mut section = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--theme" => {
                mode = match args.next().as_deref() {
                    Some("dark") => ThemeMode::Dark,
                    Some("light") => ThemeMode::Light,
                    _ => ThemeMode::System,
                }
            }
            "--section" => section = args.next().as_deref().and_then(Section::parse),
            _ => {}
        }
    }
    (mode, section)
}

fn main() {
    let (mode, section) = parse_args();
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
                    cx.new(|cx| Gallery::new(mode, section, cx))
                },
            )
            .expect("failed to open the gallery window");
            cx.activate(true);
        });
}
