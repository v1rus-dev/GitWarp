//! The titlebar worktree switcher (Ctrl Alt W): a button after the branch
//! switcher and the popover under it. The popover filters the worktrees as
//! you type and, when the text is a branch name no worktree has checked out,
//! offers to create (or open) a worktree for it on the spot.
//!
//! The list, the create flow and the switch itself belong to
//! [`WorktreesModel`]; this module is the popover over them. What a query
//! shows is pure ([`items_for`], [`rows`]) so it is unit tested without a
//! window.

use std::borrow::Cow;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, point, px, App, AppContext, Bounds, ClickEvent, Context, Div, Entity,
    FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement, StrikethroughStyle, Styled,
    Subscription, TextStyle, WeakEntity, Window,
};

use ui::actions::{Confirm, SelectNext, SelectPrev};
use ui::focus::{focus_ring, RingOffset};
use ui::metrics::{FONT_MONO, FONT_UI, FS_BASE, FS_SM, FS_XS, R_MD, R_SM};
use ui::overlay::{self, LayerId};
use ui::{
    ActiveTheme, Button, ButtonSize, Chip, Icon, IconName, Kbd, Scrollbar, SearchField, Skeleton,
    TextInput, TextInputEvent, ThemeColors, Tooltip,
};

use crate::format::display_path;
use crate::palette::{self, fuzzy};
use crate::repo_view::{OpenWorktreeSwitcher, TOOL_H};
use crate::worktree_glyphs::worktree_glyphs;
use crate::worktrees_model::{BranchCheck, WorktreeEntry, WorktreesModel};

actions!(
    worktree_switcher,
    [Jump1, Jump2, Jump3, Jump4, Jump5, Jump6, Jump7, Jump8, Jump9]
);

const CONTEXT: &str = "WorktreeSwitcher";

/// Binds the popover's own keys in its key context: Alt 1…9 jump to the
/// first nine worktrees. The key that opens it is bound with the other
/// repository shortcuts.
pub(crate) fn bind_keys(cx: &mut App) {
    // Tab must not walk the focus out of the popover.
    let mut bindings = vec![
        ui::actions::key_binding(cx, "tab", palette::Ignore, Some(CONTEXT)),
        ui::actions::key_binding(cx, "shift-tab", palette::Ignore, Some(CONTEXT)),
    ];
    macro_rules! jump {
        ($($key:literal => $action:ident),*) => {
            $(bindings.push(ui::actions::key_binding(cx, $key, $action, Some(CONTEXT)));)*
        };
    }
    jump!(
        "alt-1" => Jump1, "alt-2" => Jump2, "alt-3" => Jump3, "alt-4" => Jump4,
        "alt-5" => Jump5, "alt-6" => Jump6, "alt-7" => Jump7, "alt-8" => Jump8,
        "alt-9" => Jump9
    );
    cx.bind_keys(bindings);
}

// ----- what a query shows (pure) -----

/// One selectable line of the popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    /// A worktree the query matches; the positions are the matched characters
    /// of its name and of its branch line.
    Worktree {
        path: PathBuf,
        name_positions: Vec<usize>,
        branch_positions: Vec<usize>,
    },
    /// "Create “x” in a new worktree", or "Open “x”…" when the branch exists
    /// already (locally, or on a remote, where a tracking branch is made).
    Create { name: String, existing: bool },
    /// The typed text is not a branch name; shown, never runnable.
    Invalid,
}

/// The branch line of a worktree.
fn branch_label(entry: &WorktreeEntry) -> Cow<'_, str> {
    entry.branch().map_or_else(
        || Cow::Owned(ui::i18n::t("worktrees.detached_head").to_string()),
        Cow::Borrowed,
    )
}

/// The matched characters of the name and the branch line, when `query`
/// matches the worktree's name, branch or path.
fn match_entry(entry: &WorktreeEntry, query: &str) -> Option<(Vec<usize>, Vec<usize>)> {
    if query.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let name = fuzzy(query, &entry.name()).map(|found| found.positions);
    let branch = fuzzy(query, &branch_label(entry)).map(|found| found.positions);
    let path = entry
        .path()
        .to_string_lossy()
        .to_lowercase()
        .contains(&query.to_lowercase());
    if name.is_none() && branch.is_none() && !path {
        return None;
    }
    Some((name.unwrap_or_default(), branch.unwrap_or_default()))
}

/// The lines for `query`: the worktrees it matches in the model's order
/// (current first, then the most recently opened), then the create line.
/// `check` is what the model says typing the trimmed `query` means.
pub(crate) fn items_for(entries: &[WorktreeEntry], query: &str, check: &BranchCheck) -> Vec<Item> {
    let query = query.trim();
    let mut items: Vec<Item> = entries
        .iter()
        .filter_map(|entry| {
            let (name_positions, branch_positions) = match_entry(entry, query)?;
            Some(Item::Worktree {
                path: entry.path().to_path_buf(),
                name_positions,
                branch_positions,
            })
        })
        .collect();
    match check {
        // A branch that is checked out somewhere is that worktree's row.
        BranchCheck::Empty | BranchCheck::InUse { .. } => {}
        BranchCheck::Invalid => items.push(Item::Invalid),
        BranchCheck::Existing | BranchCheck::Tracking(_) => items.push(Item::Create {
            name: query.to_owned(),
            existing: true,
        }),
        BranchCheck::New => items.push(Item::Create {
            name: query.to_owned(),
            existing: false,
        }),
    }
    items
}

/// One line of the list: a section header or an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A section header; holds the catalog key of its title.
    Header(&'static str),
    Item(usize),
}

/// The list's lines: "WORKTREES" before the worktrees, "CREATE" or "NOT
/// AVAILABLE" before the create line.
pub(crate) fn rows(items: &[Item]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len() + 2);
    let mut worktrees = false;
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Worktree { .. } if !worktrees => {
                worktrees = true;
                rows.push(Row::Header("worktrees.switcher.section_worktrees"));
            }
            Item::Worktree { .. } => {}
            Item::Create { .. } => rows.push(Row::Header("worktrees.switcher.section_create")),
            Item::Invalid => rows.push(Row::Header("worktrees.switcher.section_unavailable")),
        }
        rows.push(Row::Item(index));
    }
    rows
}

/// The item Alt `n` (1-based) jumps to: the `n`th worktree, only while the
/// query is empty (the badges are only shown then).
pub(crate) fn jump_target(items: &[Item], query: &str, n: usize) -> Option<usize> {
    if !query.trim().is_empty() || n == 0 {
        return None;
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, Item::Worktree { .. }))
        .nth(n - 1)
        .map(|(index, _)| index)
}

/// `selected` moved by `delta` over `len` items, wrapping at both ends.
pub(crate) fn wrap(selected: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (selected as isize + delta).rem_euclid(len as isize) as usize
}

// ----- the titlebar button -----

/// The titlebar button: worktree icon, bold current name, a warning dot when
/// it has local changes, chevron. Pressed while the popover is open.
#[allow(clippy::too_many_arguments)]
pub(crate) fn titlebar_button(
    name: SharedString,
    name_max: Option<f32>,
    padding: f32,
    changed: usize,
    open: bool,
    key_cap: Option<String>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    colors: ThemeColors,
) -> impl IntoElement {
    let (fg, icon) = if open {
        (colors.selection_fg, colors.selection_fg)
    } else {
        (colors.text, colors.muted)
    };
    let mut row = div()
        .id("worktree-switcher-button")
        .relative()
        .h(px(TOOL_H))
        .px(px(padding))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(R_SM)
        .cursor_pointer()
        .text_color(fg)
        .when(open, |row| row.bg(colors.selection))
        .when(!open, |row| row.hover(move |style| style.bg(colors.hover)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(Icon::new(IconName::Worktree).size(px(14.)).color(icon));
    row = match key_cap {
        Some(cap) => row.tooltip(Tooltip::with_kbd(
            ui::i18n::t("worktrees.switcher.tooltip"),
            cap,
        )),
        None => row.tooltip(Tooltip::text(ui::i18n::t("worktrees.switcher.tooltip"))),
    };
    if let Some(max) = name_max {
        row = row.child(
            div()
                .max_w(px(max))
                .truncate()
                .font_weight(FontWeight::BOLD)
                .child(name),
        );
    }
    row.when(changed > 0, |row| {
        // The dot says "has changes"; the changed-file count is in the
        // popover's row glyphs.
        row.child(
            div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(colors.warning),
        )
    })
    .child(Icon::new(IconName::ChevD).size(px(12.)).color(icon))
    .child(
        canvas(
            |bounds, _, _| bounds,
            move |_, painted, _, _| bounds.set(Some(painted)),
        )
        .absolute()
        .inset_0(),
    )
}

// ----- the popover -----

/// Opens the add-worktree dialog; the repository view owns it.
pub(crate) type OpenAdd = Rc<dyn Fn(&mut Window, &mut App)>;

/// Opens the popover under `anchor` and focuses its filter. Closing it (Esc,
/// a click outside, a pick) returns the focus to where it was.
pub(crate) fn open(
    model: Entity<WorktreesModel>,
    anchor: Bounds<Pixels>,
    add: OpenAdd,
    window: &mut Window,
    cx: &mut App,
) -> Entity<WorktreeSwitcher> {
    let input = cx.new(|cx| {
        let mut input = TextInput::new(window, cx);
        input.set_placeholder(ui::i18n::t("worktrees.switcher.placeholder"), cx);
        input
    });
    let view = cx.new(|cx| WorktreeSwitcher::new(model, input.clone(), add, window, cx));
    let position = point(anchor.left(), anchor.bottom() + px(4.));
    let layer = overlay::open_popover(position, view.clone().into(), window, cx);
    view.update(cx, |view, _| view.layer = Some(layer));
    let focus = input.read(cx).focus_handle().clone();
    window.focus(&focus, cx);
    view
}

pub(crate) struct WorktreeSwitcher {
    layer: Option<LayerId>,
    model: Entity<WorktreesModel>,
    input: Entity<TextInput>,
    add: OpenAdd,
    query: String,
    items: Vec<Item>,
    /// The folder the create line would use, shown under it.
    create_path: Option<String>,
    selected: usize,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl WorktreeSwitcher {
    fn new(
        model: Entity<WorktreesModel>,
        input: Entity<TextInput>,
        add: OpenAdd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed(text) => this.set_query(text.clone(), cx),
                    TextInputEvent::Submit => this.activate(this.selected, true, window, cx),
                },
            ),
            // The list may finish loading, or change under a prune, while open.
            cx.observe(&model, |this, _, cx| this.refresh(cx)),
        ];
        let mut this = Self {
            layer: None,
            model,
            input,
            add,
            query: String::new(),
            items: Vec::new(),
            create_path: None,
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// Closes the popover, returning the focus to where it was.
    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            overlay::close(layer, window, cx);
        }
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.selected = 0;
        self.refresh(cx);
        self.scroll.scroll_to_item(0);
    }

    /// Recomputes the lines for the current query.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let check = model.check_branch(&self.query);
        self.items = items_for(model.entries(), &self.query, &check);
        self.create_path = self.items.iter().find_map(|item| match item {
            Item::Create { name, .. } => {
                Some(display_path(&model.preview_path(name, cx), model.home()))
            }
            _ => None,
        });
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.selected = wrap(self.selected, delta, self.items.len());
        self.scroll_to_selected();
        cx.notify();
    }

    /// Scrolls the selected line into view; the first line of a section takes
    /// its header along.
    fn scroll_to_selected(&self) {
        let rows = rows(&self.items);
        let index = rows
            .iter()
            .position(|row| matches!(row, Row::Item(i) if *i == self.selected))
            .unwrap_or(0);
        let header = index
            .checked_sub(1)
            .is_some_and(|before| matches!(rows[before], Row::Header(_)));
        self.scroll
            .scroll_to_item(if header { index - 1 } else { index });
    }

    /// Alt `n`: switches to the `n`th worktree while the query is empty.
    fn jump(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = jump_target(&self.items, &self.query, n) {
            self.activate(index, true, window, cx);
        }
    }

    /// Runs line `index` and closes the popover. `keyboard` is whether a key
    /// picked it: the focus restore then waits for the key-up, so the key
    /// release cannot activate the control that regains the focus.
    fn activate(
        &mut self,
        index: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.items.get(index).cloned() else {
            return;
        };
        if item == Item::Invalid {
            return;
        }
        if keyboard {
            overlay::defer_focus_restore(window, cx);
        }
        self.close(window, cx);
        let model = self.model.clone();
        match item {
            Item::Worktree { path, .. } => {
                model.update(cx, |model, cx| model.switch_to(&path, window, cx));
            }
            Item::Create { name, .. } => {
                model.update(cx, |model, cx| model.create(&name, window, cx));
            }
            Item::Invalid => {}
        }
        if keyboard {
            overlay::clear_deferred_focus_restore(window, cx);
        }
    }

    fn add_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        (self.add.clone())(window, cx);
    }

    /// "Prune N missing" drops the records and keeps the popover open on the
    /// shorter list.
    fn prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.prune(window, cx));
    }

    fn render_section(label_key: &'static str, colors: ThemeColors) -> Div {
        div()
            .flex_none()
            .pt(px(6.))
            .px(px(8.))
            .pb(px(2.))
            .text_size(FS_XS)
            .font_weight(FontWeight::BOLD)
            .text_color(colors.muted)
            .child(ui::i18n::t(label_key).to_uppercase())
    }

    /// The row shell: highlight ring, hover, click.
    fn row_shell(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let colors = cx.theme().colors;
        let selected = index == self.selected;
        div()
            .id(("worktree-switcher-item", index))
            .debug_selector(move || format!("worktree-switcher-item-{index}"))
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(40.))
            .px(px(8.))
            .py(px(4.))
            .rounded(R_SM)
            .cursor_pointer()
            .when(selected, |row| row.bg(colors.hover))
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .when(selected, |row| {
                row.child(focus_ring(RingOffset::Inset2, R_SM, cx))
            })
            // The highlight follows the pointer, as in the design.
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.selected != index {
                    this.selected = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate(index, false, window, cx);
            }))
    }

    fn render_worktree(
        &self,
        index: usize,
        entry: &WorktreeEntry,
        name_positions: &[usize],
        branch_positions: &[usize],
        badge: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().colors;
        let current = entry.is_current();
        let missing = entry.missing;
        let name_style = TextStyle {
            color: if missing { colors.muted } else { colors.text },
            font_family: FONT_UI.into(),
            font_size: FS_BASE.into(),
            font_weight: if current {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            },
            strikethrough: missing.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..TextStyle::default()
        };
        let branch_style = TextStyle {
            color: colors.muted,
            font_family: FONT_MONO.into(),
            font_size: FS_SM.into(),
            ..TextStyle::default()
        };
        let name = entry.name();
        let icon = if entry.is_main() {
            IconName::Git
        } else {
            IconName::Worktree
        };
        self.row_shell(index, cx)
            .when(current, |row| {
                row.child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(6.))
                        .bottom(px(6.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(colors.accent),
                )
            })
            .child(Icon::new(icon).size(px(16.)).color(if current {
                colors.accent
            } else {
                colors.muted
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().min_w_0().truncate().child(palette::highlighted_in(
                                &name,
                                name_positions,
                                name_style,
                                colors.accent,
                            )))
                            .when(entry.is_main(), |row| {
                                row.child(Chip::new(
                                    ("worktree-switcher-default", index),
                                    ui::i18n::t("worktrees.switcher.default"),
                                ))
                            }),
                    )
                    .child(div().min_w_0().truncate().text_size(FS_SM).child(
                        palette::highlighted_in(
                            &branch_label(entry),
                            branch_positions,
                            branch_style,
                            colors.accent,
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .child(worktree_glyphs(
                        &format!("worktree-switcher-{index}"),
                        entry,
                        false,
                        colors,
                    ))
                    .when_some(badge, |row, n| row.child(Kbd::new(format!("Alt+{n}")))),
            )
    }

    fn render_create(
        &self,
        index: usize,
        name: &str,
        existing: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().colors;
        let title = if existing {
            ui::t!("worktrees.switcher.open_new", name = name)
        } else {
            ui::t!("worktrees.switcher.create_new", name = name)
        };
        self.row_shell(index, cx)
            .child(Icon::new(IconName::Plus).size(px(16.)).color(colors.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(FS_SM)
                            .font_family(FONT_MONO)
                            .text_color(colors.muted)
                            .child(self.create_path.clone().unwrap_or_default()),
                    ),
            )
            .child(Kbd::new("Enter"))
    }

    /// The "Not available" line: muted, not runnable, no hover.
    fn render_invalid(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = index == self.selected;
        div()
            .id(("worktree-switcher-item", index))
            .debug_selector(move || format!("worktree-switcher-item-{index}"))
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(40.))
            .px(px(8.))
            .py(px(4.))
            .rounded(R_SM)
            .when(selected, |row| row.bg(colors.hover))
            .when(selected, |row| {
                row.child(focus_ring(RingOffset::Inset2, R_SM, cx))
            })
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.selected != index {
                    this.selected = index;
                    cx.notify();
                }
            }))
            .child(Icon::new(IconName::Plus).size(px(16.)).color(colors.muted))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.muted)
                    .child(ui::i18n::t("worktrees.toast.invalid_branch")),
            )
    }

    fn render_footer(&self, missing: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .p(px(6.))
            .border_t_1()
            .border_color(colors.border)
            .child(
                Button::ghost("worktree-switcher-add")
                    .size(ButtonSize::Small)
                    .icon(IconName::Plus)
                    .label(ui::i18n::t("worktrees.quick.add"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_worktree(window, cx))),
            )
            .when(missing > 0, |row| {
                row.child(
                    Button::ghost("worktree-switcher-prune")
                        .size(ButtonSize::Small)
                        .icon(IconName::Trash)
                        .label(ui::t!("worktrees.view.prune", n = missing))
                        .on_click(cx.listener(|this, _, window, cx| this.prune(window, cx))),
                )
            })
            .child(div().flex_1())
            .child(Kbd::new("↑↓"))
            .child(Kbd::new("Enter"))
            .child(Kbd::new("Esc"))
    }
}

impl Render for WorktreeSwitcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (loading, missing) = {
            let model = self.model.read(cx);
            (
                !model.is_loaded() || (model.is_loading() && model.entries().is_empty()),
                model.entries().iter().filter(|entry| entry.missing).count(),
            )
        };

        let body = if loading {
            div()
                .id("worktree-switcher-loading")
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .children(
                    [220., 160., 200.]
                        .into_iter()
                        .enumerate()
                        .map(|(i, width)| {
                            Skeleton::new(("worktree-switcher-skeleton", i), px(width))
                        }),
                )
                .into_any_element()
        } else if self.items.is_empty() {
            div()
                .px(px(12.))
                .py(px(20.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .flex()
                .justify_center()
                .child(ui::i18n::t("worktrees.switcher.empty"))
                .into_any_element()
        } else {
            let mut list = div()
                .id("worktree-switcher-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .p(px(4.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll);
            let mut badge = 0;
            for row in rows(&self.items) {
                list = match row {
                    Row::Header(label) => list.child(Self::render_section(label, colors)),
                    Row::Item(index) => match self.items[index].clone() {
                        Item::Worktree {
                            path,
                            name_positions,
                            branch_positions,
                        } => {
                            badge += 1;
                            let entry = self.model.read(cx).entry(&path).cloned();
                            match entry {
                                Some(entry) => list.child(self.render_worktree(
                                    index,
                                    &entry,
                                    &name_positions,
                                    &branch_positions,
                                    (badge <= 9 && self.query.trim().is_empty()).then_some(badge),
                                    cx,
                                )),
                                None => list,
                            }
                        }
                        Item::Create { name, existing } => {
                            list.child(self.render_create(index, &name, existing, cx))
                        }
                        Item::Invalid => list.child(self.render_invalid(index, cx)),
                    },
                };
            }
            div()
                .flex()
                .flex_row()
                .min_h_0()
                .max_h(px(300.))
                .child(list)
                .child(Scrollbar::vertical(
                    "worktree-switcher-scrollbar",
                    self.scroll.clone(),
                ))
                .into_any_element()
        };

        macro_rules! jumps {
            ($root:ident; $($action:ident => $n:expr),*) => {
                $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.jump($n, window, cx)
                })))*
            };
        }
        let root = div()
            .debug_selector(|| "worktree-switcher".to_owned())
            .key_context(CONTEXT)
            .w(px(380.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border_strong)
            .rounded(R_MD)
            .shadow(cx.theme().pop_shadow())
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &palette::Ignore, _, _| {
                let _ = this;
            }))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                this.activate(this.selected, true, window, cx);
            }))
            // Ctrl Alt W again closes it.
            .on_action(cx.listener(|this, _: &OpenWorktreeSwitcher, window, cx| {
                this.close(window, cx);
            }));
        let root = jumps!(root;
            Jump1 => 1, Jump2 => 2, Jump3 => 3, Jump4 => 4, Jump5 => 5,
            Jump6 => 6, Jump7 => 7, Jump8 => 8, Jump9 => 9);
        root.child(
            div()
                .flex_none()
                .px(px(8.))
                .pt(px(8.))
                .pb(px(4.))
                .child(SearchField::new(
                    "worktree-switcher-filter",
                    self.input.clone(),
                )),
        )
        .child(body)
        .child(self.render_footer(missing, cx))
    }
}

/// A weak handle the repository view keeps to know whether the popover is
/// open (the button is pressed then).
pub(crate) type Handle = WeakEntity<WorktreeSwitcher>;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use git_core::Worktree;

    use super::*;

    fn entry(path: &str, current: bool, main: bool, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            worktree: Worktree {
                path: PathBuf::from(path),
                branch: branch.map(str::to_owned),
                head: None,
                bare: false,
                detached: branch.is_none(),
                locked: None,
                prunable: false,
                current,
                main,
            },
            changes: Vec::new(),
            missing: false,
            head_summary: None,
            ahead: 0,
            behind: 0,
            has_upstream: false,
            last_used: None,
        }
    }

    fn entries() -> Vec<WorktreeEntry> {
        vec![
            entry("/r/repo", true, true, Some("main")),
            entry("/r/repo-hotfix", false, false, Some("fix/hot")),
            entry("/r/review", false, false, None),
        ]
    }

    fn paths(items: &[Item]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|item| match item {
                Item::Worktree { path, .. } => path.to_str(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_empty_query_lists_every_worktree_in_model_order() {
        let items = items_for(&entries(), "", &BranchCheck::Empty);
        assert_eq!(paths(&items), ["/r/repo", "/r/repo-hotfix", "/r/review"]);
        assert_eq!(items.len(), 3, "no create line without a query");
    }

    #[test]
    fn a_query_filters_by_name_branch_and_path() {
        let by_name = items_for(&entries(), "hotfix", &BranchCheck::New);
        assert_eq!(paths(&by_name), ["/r/repo-hotfix"]);
        let by_branch = items_for(
            &entries(),
            "fix/hot",
            &BranchCheck::InUse {
                path: PathBuf::from("/r/repo-hotfix"),
                name: "repo-hotfix".to_owned(),
            },
        );
        assert_eq!(paths(&by_branch), ["/r/repo-hotfix"]);
        let detached = items_for(&entries(), "detached", &BranchCheck::New);
        assert_eq!(paths(&detached), ["/r/review"]);
        // `/r/` only appears in the folders' paths.
        let by_path = items_for(&entries(), "/r/rev", &BranchCheck::Invalid);
        assert_eq!(paths(&by_path), ["/r/review"]);
    }

    #[test]
    fn matches_come_with_the_highlighted_characters() {
        let items = items_for(&entries(), "hot", &BranchCheck::New);
        let Item::Worktree {
            name_positions,
            branch_positions,
            ..
        } = &items[0]
        else {
            panic!("the first line is a worktree");
        };
        assert_eq!(name_positions, &[5, 6, 7]);
        assert_eq!(branch_positions, &[4, 5, 6]);
    }

    #[test]
    fn a_new_branch_name_offers_to_create_it() {
        let items = items_for(&entries(), "feat/new", &BranchCheck::New);
        assert_eq!(
            items,
            vec![Item::Create {
                name: "feat/new".to_owned(),
                existing: false
            }]
        );
    }

    #[test]
    fn existing_local_and_remote_branches_offer_to_open_them() {
        for check in [
            BranchCheck::Existing,
            BranchCheck::Tracking("origin/idle".to_owned()),
        ] {
            let items = items_for(&entries(), " idle ", &check);
            assert_eq!(
                items,
                vec![Item::Create {
                    name: "idle".to_owned(),
                    existing: true
                }],
                "the typed name is trimmed"
            );
        }
    }

    #[test]
    fn a_branch_in_use_has_no_create_line() {
        let check = BranchCheck::InUse {
            path: PathBuf::from("/r/repo"),
            name: "repo".to_owned(),
        };
        let items = items_for(&entries(), "main", &check);
        assert_eq!(paths(&items), ["/r/repo"]);
        assert!(items
            .iter()
            .all(|item| matches!(item, Item::Worktree { .. })));
    }

    #[test]
    fn an_invalid_name_shows_the_unavailable_line() {
        let items = items_for(&entries(), "bad..name", &BranchCheck::Invalid);
        assert_eq!(items, vec![Item::Invalid]);
    }

    #[test]
    fn rows_put_a_header_before_each_section() {
        let items = items_for(&entries(), "hot", &BranchCheck::New);
        assert_eq!(
            rows(&items),
            vec![
                Row::Header("worktrees.switcher.section_worktrees"),
                Row::Item(0),
                Row::Header("worktrees.switcher.section_create"),
                Row::Item(1)
            ]
        );
        let invalid = items_for(&[], "x y", &BranchCheck::Invalid);
        assert_eq!(
            rows(&invalid),
            vec![
                Row::Header("worktrees.switcher.section_unavailable"),
                Row::Item(0)
            ]
        );
        assert!(rows(&[]).is_empty());
    }

    #[test]
    fn alt_numbers_pick_worktrees_only_while_the_query_is_empty() {
        let items = items_for(&entries(), "", &BranchCheck::Empty);
        assert_eq!(jump_target(&items, "", 1), Some(0));
        assert_eq!(jump_target(&items, "  ", 3), Some(2));
        assert_eq!(jump_target(&items, "", 4), None);
        assert_eq!(jump_target(&items, "", 0), None);
        assert_eq!(jump_target(&items, "x", 1), None);
        // The create line is not numbered.
        let with_create = items_for(&entries(), "zzz", &BranchCheck::New);
        assert_eq!(jump_target(&with_create, "", 1), None);
    }

    #[test]
    fn the_highlight_wraps_at_both_ends() {
        assert_eq!(wrap(0, -1, 3), 2);
        assert_eq!(wrap(2, 1, 3), 0);
        assert_eq!(wrap(1, 1, 3), 2);
        assert_eq!(wrap(0, 1, 0), 0);
    }

    // ----- the popover in a real window -----

    use std::process::Command;

    use git_core::Repo;
    use gpui::{TestAppContext, VisualTestContext};

    use crate::settings::{AfterCreate, WorktreeSettings};
    use crate::test_support::{git, open_workspace, TempRepo};
    use crate::workspace::Workspace;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?} failed");
    }

    fn sibling(repo: &TempRepo, suffix: &str) -> PathBuf {
        let path = repo.path().parent().unwrap().join(format!(
            "{}-{suffix}",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    /// The window with `repo` open and its worktrees loaded.
    fn open_repo(
        repo: &TempRepo,
        after: AfterCreate,
        base: &std::path::Path,
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, Entity<Workspace>) {
        let (mut cx, workspace) = open_workspace(git(), cx);
        cx.update(|_, cx| {
            cx.set_global(crate::AppSettings {
                path: None,
                settings: crate::settings::Settings {
                    worktrees: WorktreeSettings {
                        base: base.display().to_string(),
                        template: "{branch}".to_owned(),
                        after_create: after,
                        copy_env: false,
                        ..WorktreeSettings::default()
                    },
                    ..crate::settings::Settings::default()
                },
                save_in_flight: false,
                dirty: false,
            });
            bind_keys(cx);
        });
        let path = repo.path().to_path_buf();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_repository(path, window, cx)
            });
        });
        cx.run_until_parked();
        (cx, workspace)
    }

    fn open_switcher(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.dispatch_action(Box::new(OpenWorktreeSwitcher), cx));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("worktree-switcher").is_some(),
            "the switcher did not open"
        );
    }

    fn root_of(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) -> PathBuf {
        cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .expect("a repository is open")
                .read(cx)
                .worktrees_model()
                .read(cx)
                .root()
                .canonicalize()
                .expect("the root exists")
        })
    }

    #[gpui::test]
    fn escape_closes_the_popover_and_returns_the_focus(cx: &mut TestAppContext) {
        let repo = TempRepo::new("wtsw-escape");
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        let base = sibling(&repo, "wts");
        let (mut cx, workspace) = open_repo(&repo, AfterCreate::Switch, &base, cx);
        let before = cx.update(|window, cx| window.focused(cx));

        open_switcher(&mut cx);
        let panel = cx.debug_bounds("worktree-switcher").unwrap();
        assert_eq!(panel.size.width, px(380.));
        // The only worktree is the current, default one.
        assert!(cx.debug_bounds("worktree-switcher-item-0").is_some());
        let pressed = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .unwrap()
                .read(cx)
                .worktree_switcher_open()
        });
        assert!(pressed, "the button is pressed while the popover is open");

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("worktree-switcher").is_none(),
            "Esc did not close it"
        );
        assert_eq!(cx.update(|window, cx| window.focused(cx)), before);
        let pressed = cx.update(|_, cx| {
            workspace
                .read(cx)
                .repository_for_test()
                .unwrap()
                .read(cx)
                .worktree_switcher_open()
        });
        assert!(!pressed, "the button is released after closing");
    }

    #[gpui::test]
    fn alt_number_switches_to_that_worktree(cx: &mut TestAppContext) {
        let repo = TempRepo::new("wtsw-alt");
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        run(repo.path(), &["branch", "extra"]);
        let linked = sibling(&repo, "linked");
        Repo::open(git(), repo.path())
            .unwrap()
            .add_worktree(&linked, "extra")
            .unwrap();
        let base = sibling(&repo, "wts");
        let (mut cx, workspace) = open_repo(&repo, AfterCreate::Switch, &base, cx);

        open_switcher(&mut cx);
        // The current worktree is first, the other one is Alt 2.
        cx.simulate_keystrokes("alt-2");
        cx.run_until_parked();
        cx.run_until_parked();

        assert!(cx.debug_bounds("worktree-switcher").is_none());
        assert_eq!(
            root_of(&mut cx, &workspace),
            linked.canonicalize().unwrap(),
            "Alt 2 did not open the second worktree"
        );
        let _ = std::fs::remove_dir_all(&linked);
    }

    #[gpui::test]
    fn alt_number_is_ignored_once_a_query_is_typed(cx: &mut TestAppContext) {
        let repo = TempRepo::new("wtsw-alt-query");
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        run(repo.path(), &["branch", "extra"]);
        let linked = sibling(&repo, "linked-q");
        Repo::open(git(), repo.path())
            .unwrap()
            .add_worktree(&linked, "extra")
            .unwrap();
        let base = sibling(&repo, "wts");
        let (mut cx, workspace) = open_repo(&repo, AfterCreate::Switch, &base, cx);

        open_switcher(&mut cx);
        cx.simulate_input("x");
        cx.run_until_parked();
        cx.simulate_keystrokes("alt-1");
        cx.run_until_parked();

        assert!(cx.debug_bounds("worktree-switcher").is_some(), "still open");
        assert_eq!(
            root_of(&mut cx, &workspace),
            repo.path().canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(&linked);
    }

    #[gpui::test]
    fn enter_on_the_create_line_makes_the_worktree(cx: &mut TestAppContext) {
        let repo = TempRepo::new("wtsw-create");
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        let base = sibling(&repo, "wts");
        let (mut cx, workspace) = open_repo(&repo, AfterCreate::Switch, &base, cx);

        open_switcher(&mut cx);
        cx.simulate_input("feat/new");
        cx.run_until_parked();
        // No worktree matches, so the create line is the only (selected) one.
        assert!(cx.debug_bounds("worktree-switcher-item-0").is_some());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.run_until_parked();

        let expected = base.join("feat-new");
        assert!(expected.is_dir(), "the worktree folder was not created");
        assert!(cx.debug_bounds("worktree-switcher").is_none());
        // After creating: switch (the setting's default) to the new worktree.
        assert_eq!(
            root_of(&mut cx, &workspace),
            expected.canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[gpui::test]
    fn an_invalid_name_cannot_be_run(cx: &mut TestAppContext) {
        let repo = TempRepo::new("wtsw-invalid");
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        run(repo.path(), &["add", "a.txt"]);
        run(repo.path(), &["commit", "-qm", "one"]);
        let base = sibling(&repo, "wts");
        let (mut cx, _workspace) = open_repo(&repo, AfterCreate::Switch, &base, cx);

        open_switcher(&mut cx);
        cx.simulate_input("bad..name");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("worktree-switcher").is_some(),
            "Enter on the unavailable line must leave the popover open"
        );
        assert!(!base.exists(), "nothing was created");
    }
}
