//! Visual catalogue of the design system tokens.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, point, prelude::*, px, size, uniform_list, AnyElement, AnyView, App, Bounds,
    ClickEvent, Div, ElementId, Entity, FocusHandle, FontWeight, Hsla, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Render, ScrollStrategy, SharedString, Subscription, TitlebarOptions,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions,
};

use ui::focus::RingOffset;
use ui::metrics::{
    BAR, CTL, DOT_R, FONT_MONO, FONT_UI, FS_BASE, FS_LG, FS_MD, FS_SM, FS_XL, FS_XS, MONO, R_LG,
    R_MD, R_SM, R_XS,
};
use ui::theme::{self, ActiveTheme, Theme, ThemeColors, ThemeMode};
use ui::{
    actions, dialog, focus, menu, overlay, toast, Avatar, Banner, BannerKind, Button, ButtonSize,
    Checkbox, Chip, ChipKind, CountPill, Counter, Dialog, DialogFocus, EmptyState, FileRow, Icon,
    IconName, Kbd, ListRow, MenuEntry, MenuItem, Progress, Radio, RefBadge, RefKind, SearchField,
    SegmentItem, Segmented, Select, Skeleton, Spinner, Splitter, StatusGlyph, Switch, Tab, Tabs,
    TextInput, TextInputEvent, ToastKind, Tree, TreeNodeData, WindowControls,
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
    Floating,
    Lists,
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
            "floating" => Self::Floating,
            "lists" => Self::Lists,
            _ => return None,
        })
    }
}

/// The floating component to open on the first frame, for visual review.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenOnStart {
    Menu,
    Select,
    Dialog,
    Toasts,
}

impl OpenOnStart {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "menu" => Self::Menu,
            "select" => Self::Select,
            "dialog" => Self::Dialog,
            "toasts" => Self::Toasts,
            _ => return None,
        })
    }
}

/// The Select's options; shared with the menu it opens.
const SELECT_OPTIONS: [&str; 4] = ["main", "develop", "feat/lanes", "release/0.3"];

/// The history sample: summary, author and relative date per row.
const LIST_SUMMARIES: [&str; 6] = [
    "Clip graph lanes at the viewport edge",
    "Keep keyboard focus on the history row",
    "Add worktree and stash sections to the sidebar",
    "Fix selection colour in the light theme",
    "Virtualise the changed-file list",
    "Show ahead/behind counts on local branches",
];
const LIST_AUTHORS: [&str; 4] = ["Mira Chen", "Pablo Ruiz", "Ari Okafor", "Jun Park"];

/// Changed-file sample rows: status, path, added, deleted.
const FILE_ROWS: [(&str, &str, usize, usize); 6] = [
    (
        "M",
        "crates/ui/src/components/file_row.rs",
        48,
        12,
    ),
    (
        "A",
        "crates/ui/src/components/scrollbar.rs",
        210,
        0,
    ),
    (
        "M",
        "crates/ui/src/components/splitter.rs",
        96,
        4,
    ),
    (
        "D",
        "crates/ui/src/components/legacy_row.rs",
        0,
        74,
    ),
    (
        "?",
        "crates/app/src/features/history/graph/lane_layout/virtualization/overscan/commit_graph_renderer/gpu_painter/lane_batching/segment_cache/incremental_rebuild/persistent_cache_writer.rs",
        0,
        0,
    ),
    (
        "M",
        "docs/design/components/lists.html",
        18,
        2,
    ),
];

/// The four toast demos: id, label, kind and message.
const TOAST_DEMOS: [(&str, &str, ToastKind, &str); 4] = [
    ("toast-ok", "Ok toast", ToastKind::Ok, "Branch created"),
    (
        "toast-info",
        "Info toast",
        ToastKind::Info,
        "Fetching origin…",
    ),
    ("toast-warn", "Warn toast", ToastKind::Warn, "Detached HEAD"),
    (
        "toast-error",
        "Error toast",
        ToastKind::Error,
        "Push rejected: non-fast-forward",
    ),
];

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
    select: usize,
    branch_subscription: Option<Subscription>,
    menu_area_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    select_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    sidebar_width: Pixels,
    tree_branches_open: bool,
    tree_feat_open: bool,
    tree_remotes_open: bool,
    tree_tags_open: bool,
    tree_worktrees_open: bool,
    tree_selected: Option<SharedString>,
    tree_nodes: Vec<TreeNodeData>,
    list_scroll: UniformListScrollHandle,
    list_selected: usize,
    list_keyboard: bool,
    list_focus: FocusHandle,
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
            select: 0,
            branch_subscription: None,
            menu_area_bounds: Rc::new(Cell::new(None)),
            select_bounds: Rc::new(Cell::new(None)),
            sidebar_width: px(240.),
            tree_branches_open: true,
            tree_feat_open: true,
            tree_remotes_open: true,
            tree_tags_open: true,
            tree_worktrees_open: true,
            tree_selected: Some("idle-lanes".into()),
            tree_nodes: Vec::new(),
            list_scroll: UniformListScrollHandle::new(),
            list_selected: 2,
            list_keyboard: false,
            list_focus: cx.focus_handle().tab_stop(true),
        }
    }

    /// The flat, visible tree sample; expansion flags are kept on the gallery.
    fn build_tree_nodes(&self) -> Vec<TreeNodeData> {
        let selected = |label: &str| {
            self.tree_selected
                .as_ref()
                .is_some_and(|value| value.as_str() == label)
        };
        let mut nodes = Vec::new();

        nodes.push(
            TreeNodeData::section("Local branches")
                .has_children(true)
                .expanded(self.tree_branches_open)
                .count("2"),
        );
        if self.tree_branches_open {
            nodes.push(
                TreeNodeData::new("main")
                    .level(1)
                    .icon(IconName::Branch)
                    .current()
                    .ahead(4)
                    .selected(selected("main")),
            );
            nodes.push(
                TreeNodeData::new("feat/")
                    .level(1)
                    .icon(IconName::Folder)
                    .has_children(true)
                    .expanded(self.tree_feat_open)
                    .count("2"),
            );
            if self.tree_feat_open {
                nodes.push(
                    TreeNodeData::new("idle-lanes")
                        .level(2)
                        .icon(IconName::Branch)
                        .ahead(1)
                        .selected(selected("idle-lanes")),
                );
                nodes.push(
                    TreeNodeData::new("palette")
                        .level(2)
                        .icon(IconName::Branch)
                        .behind(2)
                        .selected(selected("palette")),
                );
            }
        }

        nodes.push(
            TreeNodeData::section("Remotes")
                .has_children(true)
                .expanded(self.tree_remotes_open)
                .count("1"),
        );
        if self.tree_remotes_open {
            nodes.push(
                TreeNodeData::new("origin")
                    .level(1)
                    .icon(IconName::Cloud)
                    .selected(selected("origin")),
            );
        }

        nodes.push(
            TreeNodeData::section("Tags")
                .has_children(true)
                .expanded(self.tree_tags_open)
                .count("1"),
        );
        if self.tree_tags_open {
            nodes.push(
                TreeNodeData::new("v0.3.0")
                    .level(1)
                    .icon(IconName::Tag)
                    .selected(selected("v0.3.0")),
            );
        }

        nodes.push(
            TreeNodeData::section("Stashes")
                .has_children(true)
                .expanded(false)
                .count("0"),
        );

        nodes.push(
            TreeNodeData::section("Worktrees")
                .has_children(true)
                .expanded(self.tree_worktrees_open)
                .count("1"),
        );
        if self.tree_worktrees_open {
            nodes.push(
                TreeNodeData::new("feature-side")
                    .level(1)
                    .icon(IconName::Worktree)
                    .state("locked")
                    .selected(selected("feature-side")),
            );
        }

        nodes
    }

    /// Flip expansion for the node at `index` in the last rendered tree.
    fn toggle_tree_node(&mut self, index: usize) {
        let Some(label) = self.tree_nodes.get(index).map(|node| node.label.clone()) else {
            return;
        };
        let open = match self.tree_expanded(&label) {
            Some(open) => !open,
            None => return,
        };
        self.set_tree_expanded(&label, open);
    }

    fn tree_expanded(&self, label: &str) -> Option<bool> {
        Some(match label {
            "Local branches" => self.tree_branches_open,
            "feat/" => self.tree_feat_open,
            "Remotes" => self.tree_remotes_open,
            "Tags" => self.tree_tags_open,
            "Worktrees" => self.tree_worktrees_open,
            _ => return None,
        })
    }

    fn set_tree_expanded(&mut self, label: &str, open: bool) {
        match label {
            "Local branches" => self.tree_branches_open = open,
            "feat/" => self.tree_feat_open = open,
            "Remotes" => self.tree_remotes_open = open,
            "Tags" => self.tree_tags_open = open,
            "Worktrees" => self.tree_worktrees_open = open,
            _ => {}
        }
    }

    fn select_tree_node(&mut self, index: usize) {
        if let Some(node) = self.tree_nodes.get(index) {
            if !node.has_children && !node.section {
                self.tree_selected = Some(node.label.clone());
            }
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
        if self.shows(Section::Floating) {
            sections.push(floating_section(self, colors, cx).into_any_element());
        }
        if self.shows(Section::Lists) {
            sections.push(lists_section(self, colors, window, cx).into_any_element());
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
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .h(BAR)
                        .pl(px(12.))
                        .bg(colors.bg)
                        .border_b_1()
                        .border_color(colors.border)
                        .child("Titlebar")
                        .child(div().flex_1())
                        .child(WindowControls::new("gallery-window-controls")),
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
                        .child(Avatar::new("mira@example.com", px(24.)).name("Mira Okonkwo"))
                        .child(Avatar::new("feat/lane@example.com", px(32.)).name("Lin Zhao"))
                        .child(Avatar::new("pablo@example.com", px(44.)).name("Pablo Ruiz"))
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
                        )
                        .child(
                            // One disabled option inside a live control.
                            Segmented::new("seg-item-dis")
                                .items([
                                    SegmentItem::new("History"),
                                    SegmentItem::new("Changes")
                                        .disabled(true)
                                        .tooltip("Local changes arrive in a later stage"),
                                ])
                                .selected(0),
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
                    component_row("Filled (stretches)", colors)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .child(Progress::new("progress-fill", 0.62).fill()),
                        )
                        .child(div().text_size(FS_SM).text_color(colors.muted).child("62%")),
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

fn floating_section(
    gallery: &Gallery,
    colors: ThemeColors,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    let view = cx.entity();
    let select = {
        let view = view.clone();
        Select::new("gallery-select")
            .options(SELECT_OPTIONS)
            .selected(gallery.select)
            .on_change(move |index, _, cx| {
                view.update(cx, |this, cx| {
                    this.select = index;
                    cx.notify();
                });
            })
    };

    let open_branch = {
        let view = view.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.open_branch_dialog(window, cx);
            });
        }
    };

    let mut toasts = div().flex().items_center().gap(px(8.));
    for (id, label, kind, message) in TOAST_DEMOS {
        toasts = toasts.child(Button::new(id).label(label).on_click(move |_, window, cx| {
            toast::show(kind, message, window, cx);
        }));
    }

    let menu_area_bounds = gallery.menu_area_bounds.clone();
    let select_bounds = gallery.select_bounds.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Floating"))
        .child(
            card(colors)
                .child(
                    div()
                        .id("gallery-menu-area")
                        .relative()
                        .h(px(120.))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(R_MD)
                        .border_1()
                        .border_color(colors.border_strong)
                        .bg(colors.bg)
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .cursor_context_menu()
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|_, event: &MouseDownEvent, window, cx| {
                                menu::open_context_menu(
                                    event.position,
                                    history_menu_entries(),
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            canvas(
                                |bounds, _, _| bounds,
                                move |_, bounds, _, _| menu_area_bounds.set(Some(bounds)),
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .child("Right-click here for the history-row context menu"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("Select"),
                        )
                        .child(
                            div().relative().w(px(220.)).child(select).child(
                                canvas(
                                    |bounds, _, _| bounds,
                                    move |_, bounds, _, _| select_bounds.set(Some(bounds)),
                                )
                                .absolute()
                                .inset_0(),
                            ),
                        )
                        .child(
                            Button::primary("open-branch-dialog")
                                .label("Create branch…")
                                .on_click(open_branch),
                        ),
                )
                .child(toasts),
        )
}

/// The history row's context menu, with a disabled extra item.
fn history_menu_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::Item(
            MenuItem::new("Copy commit hash")
                .icon(IconName::Copy)
                .shortcut("Ctrl+C")
                .action(|window, cx| {
                    toast::show(ToastKind::Ok, "Copied commit hash", window, cx);
                }),
        ),
        MenuEntry::Item(
            MenuItem::new("Create branch here…")
                .icon(IconName::Branch)
                .shortcut("B")
                .action(|window, cx| {
                    toast::show(ToastKind::Ok, "Created branch at this commit", window, cx);
                }),
        ),
        MenuEntry::Item(
            MenuItem::new("Create tag here…")
                .icon(IconName::Tag)
                .action(|window, cx| {
                    toast::show(ToastKind::Ok, "Created tag at this commit", window, cx);
                }),
        ),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem::new("Cherry-pick").action(|window, cx| {
            toast::show(ToastKind::Info, "Cherry-picked onto main", window, cx);
        })),
        MenuEntry::Item(MenuItem::new("Revert commit").icon(IconName::Undo).action(
            |window, cx| {
                toast::show(ToastKind::Info, "Reverted commit", window, cx);
            },
        )),
        MenuEntry::Item(
            MenuItem::new("Reset main to here…")
                .danger()
                .action(|window, cx| {
                    toast::show(ToastKind::Error, "Reset main to this commit", window, cx);
                }),
        ),
        MenuEntry::Item(
            MenuItem::new("Copy patch")
                .shortcut("Ctrl+Shift+C")
                .disabled(true),
        ),
    ]
}

/// The Select's options as menu entries, for the `--open select` review flag.
fn select_menu_entries(view: Entity<Gallery>, selected: usize) -> Vec<MenuEntry> {
    SELECT_OPTIONS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let mut item = MenuItem::new(*label);
            if index == selected {
                item = item.icon(IconName::Check);
            }
            let view = view.clone();
            MenuEntry::Item(item.action(move |_, cx| {
                view.update(cx, |this, cx| {
                    this.select = index;
                    cx.notify();
                });
            }))
        })
        .collect()
}

impl Gallery {
    fn open_branch_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let layer = Rc::new(Cell::new(None));
        let body = cx.new(|cx| BranchBody::new(window, layer.clone(), cx));
        let name = body.read(cx).name.clone();
        let footer = cx.new(|cx| BranchFooter::new(name.clone(), layer.clone(), cx));

        let name_focus = name.read(cx).focus_handle().clone();
        let (switch_focus, busy_focus) = body.read(cx).focus_handles();
        let (cancel_focus, create_focus, on_create) = footer.read(cx).dialog_parts();
        let on_default = on_create.clone();

        let build_body = body.clone();
        let build_footer = footer.clone();
        let focus_name = name.clone();
        let autofocus = name_focus.clone();
        let focus_switch = switch_focus.clone();
        let focus_busy = busy_focus.clone();
        let focus_cancel = cancel_focus.clone();
        let default_button = create_focus.clone();

        let (layer_id, host) = dialog::open_with_view(window, cx, move |_, cx| {
            let empty = focus_name.read(cx).text().trim().is_empty();
            let mut focus = vec![
                DialogFocus::input(autofocus.clone()),
                DialogFocus::control(focus_switch.clone()),
                DialogFocus::control(focus_busy.clone()),
                DialogFocus::control(focus_cancel.clone()),
            ];
            if !empty {
                focus.push(DialogFocus::control(default_button.clone()));
            }
            Dialog::new("Create branch")
                .icon(IconName::Branch)
                .body(build_body.clone())
                .footer(build_footer.clone())
                .hint("The new branch starts at HEAD.")
                .width(px(480.))
                .focus_order(focus)
                .autofocus(autofocus.clone())
                .default_button(default_button.clone())
                .on_default({
                    let on_default = on_default.clone();
                    move |window, cx| on_default.as_ref()(window, cx)
                })
        });
        layer.set(Some(layer_id));
        self.branch_subscription =
            Some(cx.subscribe(&name, move |_, _, _: &TextInputEvent, cx| {
                host.update(cx, |_, cx| cx.notify());
            }));
        name
    }

    /// Opens the floating component requested by `--open`, after the first frame.
    fn open_on_start(&mut self, open: OpenOnStart, window: &mut Window, cx: &mut Context<Self>) {
        match open {
            OpenOnStart::Menu => {
                let area = self.menu_area_bounds.get().unwrap_or_else(|| {
                    Bounds::new(point(px(40.), px(40.)), size(px(220.), px(120.)))
                });
                menu::open_context_menu(area.center(), history_menu_entries(), window, cx);
                // The menu opens unhighlighted; two steps down land on the
                // second enabled item.
                window.on_next_frame(|window, cx| {
                    window.dispatch_action(Box::new(actions::SelectNext), cx);
                    window.dispatch_action(Box::new(actions::SelectNext), cx);
                });
                window.refresh();
            }
            OpenOnStart::Select => {
                if let Some(anchor) = self.select_bounds.get() {
                    menu::open_dropdown(
                        anchor,
                        select_menu_entries(cx.entity(), self.select),
                        window,
                        cx,
                    );
                }
            }
            OpenOnStart::Dialog => {
                let name = self.open_branch_dialog(window, cx);
                name.update(cx, |input, cx| input.set_text("feat/ssh-keys", cx));
            }
            OpenOnStart::Toasts => {
                for (_, _, kind, message) in TOAST_DEMOS {
                    toast::show(kind, message, window, cx);
                }
            }
        }
    }
}

struct BranchBody {
    name: Entity<TextInput>,
    layer: Rc<Cell<Option<overlay::LayerId>>>,
    switch: bool,
    busy: bool,
    switch_focus: FocusHandle,
    busy_focus: FocusHandle,
}

impl BranchBody {
    fn new(
        window: &mut Window,
        layer: Rc<Cell<Option<overlay::LayerId>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("feature/my-branch", cx);
            input
        });
        Self {
            name,
            layer,
            switch: true,
            busy: false,
            switch_focus: cx.focus_handle().tab_stop(true),
            busy_focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn focus_handles(&self) -> (FocusHandle, FocusHandle) {
        (self.switch_focus.clone(), self.busy_focus.clone())
    }
}

impl Render for BranchBody {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let view = cx.entity();
        let switch = self.switch;
        let switch_toggle = {
            let view = view.clone();
            move |next: bool, _: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.switch = next;
                    cx.notify();
                });
            }
        };
        let busy = self.busy;
        let layer = self.layer.clone();
        let busy_toggle = {
            let view = view.clone();
            move |next: bool, window: &mut Window, cx: &mut App| {
                if let Some(layer) = layer.get() {
                    dialog::set_busy(layer, next, window, cx);
                }
                view.update(cx, |this, cx| {
                    this.busy = next;
                    cx.notify();
                });
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("Branch name"),
            )
            .child(self.name.clone())
            .child(
                Checkbox::new("branch-switch")
                    .label("Switch to the new branch after creating it")
                    .checked(switch)
                    .focus_handle(self.switch_focus.clone())
                    .on_toggle(switch_toggle),
            )
            .child(
                Checkbox::new("branch-busy")
                    .label("Simulate busy")
                    .checked(busy)
                    .focus_handle(self.busy_focus.clone())
                    .on_toggle(busy_toggle),
            )
            .when(busy, |this| {
                this.child(
                    div()
                        .text_size(FS_SM)
                        .text_color(colors.muted)
                        .child("Busy: Esc, the scrim, Enter and Close are disabled."),
                )
            })
    }
}

struct BranchFooter {
    name: Entity<TextInput>,
    layer: Rc<Cell<Option<overlay::LayerId>>>,
    cancel_focus: FocusHandle,
    create_focus: FocusHandle,
    on_create: Action,
    _subscription: Subscription,
}

/// A callback for the branch dialog's controls.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

impl BranchFooter {
    fn new(
        name: Entity<TextInput>,
        layer: Rc<Cell<Option<overlay::LayerId>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let on_create: Action = {
            let name = name.clone();
            let layer = layer.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let branch = name.read(cx).text().trim().to_owned();
                if branch.is_empty() {
                    return;
                }
                toast::show(
                    ToastKind::Ok,
                    format!("Created and checked out {branch}"),
                    window,
                    cx,
                );
                if let Some(layer) = layer.get() {
                    dialog::close(layer, window, cx);
                }
            })
        };
        let subscription = cx.subscribe(&name, |_, _, _: &TextInputEvent, cx| cx.notify());
        Self {
            name,
            layer,
            cancel_focus: cx.focus_handle().tab_stop(true),
            create_focus: cx.focus_handle().tab_stop(true),
            on_create,
            _subscription: subscription,
        }
    }

    fn dialog_parts(&self) -> (FocusHandle, FocusHandle, Action) {
        (
            self.cancel_focus.clone(),
            self.create_focus.clone(),
            self.on_create.clone(),
        )
    }
}

impl Render for BranchFooter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.name.read(cx).text().trim().is_empty();
        let layer = self.layer.clone();
        let on_create = self.on_create.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::ghost("branch-cancel")
                    .label("Cancel")
                    .min_width(px(72.))
                    .focus_handle(self.cancel_focus.clone())
                    .on_click(move |_, window, cx| {
                        if let Some(layer) = layer.get() {
                            dialog::close(layer, window, cx);
                        }
                    }),
            )
            .child(
                Button::primary("branch-create")
                    .label("Create branch")
                    .min_width(px(72.))
                    .disabled(empty)
                    .focus_handle(self.create_focus.clone())
                    .on_click(move |_, window, cx| on_create.as_ref()(window, cx)),
            )
    }
}

fn lists_section(
    gallery: &mut Gallery,
    colors: ThemeColors,
    window: &mut Window,
    cx: &mut Context<Gallery>,
) -> impl IntoElement {
    let view = cx.entity();
    let nodes = gallery.build_tree_nodes();
    gallery.tree_nodes = nodes.clone();

    let tree = Tree::new("gallery-tree", nodes)
        .on_toggle({
            let view = view.clone();
            move |index, _, cx| {
                view.update(cx, |this, cx| {
                    this.toggle_tree_node(index);
                    cx.notify();
                });
            }
        })
        .on_activate({
            let view = view.clone();
            move |index, _, cx| {
                view.update(cx, |this, cx| {
                    this.select_tree_node(index);
                    cx.notify();
                });
            }
        })
        .on_expand({
            let view = view.clone();
            move |index, _, cx| {
                view.update(cx, |this, cx| {
                    if let Some(label) = this.tree_nodes.get(index).map(|node| node.label.clone()) {
                        this.set_tree_expanded(&label, true);
                    }
                    cx.notify();
                });
            }
        })
        .on_collapse({
            let view = view.clone();
            move |index, _, cx| {
                view.update(cx, |this, cx| {
                    if let Some(label) = this.tree_nodes.get(index).map(|node| node.label.clone()) {
                        this.set_tree_expanded(&label, false);
                    }
                    cx.notify();
                });
            }
        })
        .on_open({
            let view = view.clone();
            move |index, _, cx| {
                view.update(cx, |this, cx| {
                    this.select_tree_node(index);
                    cx.notify();
                });
            }
        });

    let sidebar = div()
        .w(gallery.sidebar_width)
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(colors.sidebar)
        .border_r_1()
        .border_color(colors.border)
        .child(tree);

    let splitter = Splitter::vertical("gallery-sidebar-splitter")
        .size(gallery.sidebar_width)
        .bounds(px(160.), px(420.))
        .on_resize({
            let view = view.clone();
            move |size, _, cx| {
                view.update(cx, |this, cx| {
                    this.sidebar_width = size;
                    cx.notify();
                });
            }
        })
        .on_reset({
            let view = view.clone();
            move |_, cx| {
                view.update(cx, |this, cx| {
                    this.sidebar_width = px(240.);
                    cx.notify();
                });
            }
        });

    let list_focus = gallery.list_focus.clone();
    let list_focused = list_focus.is_focused(window);
    let selected = gallery.list_selected;
    let keyboard_marker = gallery.list_keyboard && list_focused;
    let list_scroll = gallery.list_scroll.clone();
    let rows_view = view.clone();
    let rows_focus = list_focus.clone();
    let rows = uniform_list("gallery-list", 30, move |range, _, _cx| {
        range
            .map(|index| {
                let is_selected = index == selected;
                let secondary = ListRow::secondary_color(colors, is_selected, list_focused);
                let summary = LIST_SUMMARIES[index % LIST_SUMMARIES.len()];
                let author = LIST_AUTHORS[index % LIST_AUTHORS.len()];
                let row = ListRow::new(ElementId::named_usize("gallery-list-row", index))
                    .selected(is_selected)
                    .list_focused(list_focused)
                    .keyboard(is_selected && keyboard_marker)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .pl(px(14.))
                            .pr(px(18.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(54.))
                                    .font_family(FONT_MONO)
                                    .text_size(FS_SM)
                                    .text_color(secondary)
                                    .child(format!("{:07x}", 0x03a4_f2c1 + index * 0x101)),
                            )
                            .child(div().flex_1().min_w(px(0.)).truncate().child(summary))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(92.))
                                    .text_size(FS_SM)
                                    .text_color(secondary)
                                    .truncate()
                                    .child(author),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(52.))
                                    .text_size(FS_SM)
                                    .text_color(secondary)
                                    .child(format!("{}d ago", 1 + index % 12)),
                            ),
                    );
                let view = rows_view.clone();
                let focus = rows_focus.clone();
                row.on_click(move |_, window, cx| {
                    window.focus(&focus, cx);
                    view.update(cx, |this, cx| {
                        this.list_selected = index;
                        this.list_keyboard = false;
                        cx.notify();
                    });
                })
            })
            .collect::<Vec<_>>()
    })
    .track_scroll(&list_scroll)
    .flex_1()
    .h_full();

    let list_header = div()
        .h(px(34.))
        .px(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(colors.border)
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child("History")
        .child("30 commits");

    let list_area = div()
        .id("gallery-list-area")
        .flex_1()
        .min_h(px(0.))
        .flex()
        .track_focus(&list_focus)
        .on_action(cx.listener(|this, _: &actions::SelectNext, _, cx| {
            this.list_selected = (this.list_selected + 1).min(29);
            this.list_keyboard = true;
            this.list_scroll
                .scroll_to_item(this.list_selected, ScrollStrategy::Center);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &actions::SelectPrev, _, cx| {
            this.list_selected = this.list_selected.saturating_sub(1);
            this.list_keyboard = true;
            this.list_scroll
                .scroll_to_item(this.list_selected, ScrollStrategy::Center);
            cx.notify();
        }))
        .child(rows)
        .child(ui::Scrollbar::uniform(
            "gallery-list-scrollbar",
            list_scroll,
        ));

    let list_pane = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(list_header)
        .child(list_area);

    let browser = div()
        .w_full()
        .h(px(360.))
        .flex()
        .rounded(R_MD)
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .child(sidebar)
        .child(splitter)
        .child(list_pane);

    let mut files = div()
        .flex()
        .flex_col()
        .rounded(R_MD)
        .border_1()
        .border_color(colors.border)
        .overflow_hidden();
    for (index, (status, path, added, deleted)) in FILE_ROWS.iter().enumerate() {
        let row = FileRow::new(
            ElementId::named_usize("gallery-file", index),
            status.chars().next().unwrap_or('M'),
            *path,
        )
        .added(*added)
        .deleted(*deleted)
        .selected(index == 2)
        .action(
            Button::ghost(ElementId::Name(format!("gallery-file-undo-{index}").into()))
                .icon_only(IconName::Undo)
                .size(ButtonSize::Xs)
                .tooltip("Revert changes"),
        )
        .action(
            Button::ghost(ElementId::Name(format!("gallery-file-copy-{index}").into()))
                .icon_only(IconName::Copy)
                .size(ButtonSize::Xs)
                .tooltip("Copy path"),
        )
        .action(
            Button::ghost(ElementId::Name(
                format!("gallery-file-trash-{index}").into(),
            ))
            .icon_only(IconName::Trash)
            .size(ButtonSize::Xs)
            .tooltip("Discard changes"),
        );
        files = files.child(row);
    }

    div()
        .flex()
        .flex_col()
        .gap(px(24.))
        .child(heading("Lists & rows"))
        .child(browser)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(FS_XS)
                        .text_color(colors.muted)
                        .child("Changed files — start-truncated paths and hover actions"),
                )
                .child(files),
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
            window.focus(&focus, cx);
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
            window.focus(&focus, cx);
        }))
        .child("Open nested popover")
}

/// `--theme dark|light|system`, `--section <name>`, `--open menu|select|dialog|toasts`;
/// unknown values fall back.
fn parse_args() -> (ThemeMode, Option<Section>, Option<OpenOnStart>) {
    let mut mode = ThemeMode::System;
    let mut section = None;
    let mut open = None;
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
            "--open" => open = args.next().as_deref().and_then(OpenOnStart::parse),
            _ => {}
        }
    }
    (mode, section, open)
}

fn main() {
    let (mode, section, open) = parse_args();
    gpui_platform::application()
        .with_assets(ui::Assets)
        .run(move |cx: &mut App| {
            if let Err(e) = ui::init(cx) {
                eprintln!("failed to load the bundled fonts: {e}");
            }
            theme::set_mode(mode, cx);
            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            let mut root = None;
            let window = cx
                .open_window(
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
                        let view = cx.new(|cx| Gallery::new(mode, section, cx));
                        root = Some(view.clone());
                        view
                    },
                )
                .expect("failed to open the gallery window");

            if let (Some(open), Some(root)) = (open, root) {
                // The first frame records the review targets' bounds; the second
                // callback runs after that frame, so the menu, select and dialog
                // can be placed from real layout.
                window
                    .update(cx, |_, window, _| {
                        window.on_next_frame(move |window, _| {
                            window.on_next_frame(move |window, cx| {
                                root.update(cx, |gallery, cx| {
                                    gallery.open_on_start(open, window, cx);
                                });
                            });
                            window.refresh();
                        });
                    })
                    .ok();
            }
            cx.activate(true);
        });
}
