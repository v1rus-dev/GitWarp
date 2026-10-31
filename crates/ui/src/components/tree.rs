//! The sidebar tree: one focusable, keyboard-navigable list of rows.
//!
//! The caller owns the visible nodes (and their selection and expansion
//! state); the tree only renders them and reports what the user did.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Window,
};
use gpui::{MouseDownEvent, Pixels, Point};

use crate::actions::{Confirm, SelectNext, SelectPrev, TreeActivate, TreeCollapse, TreeExpand};
use crate::components::counter::CountPill;
use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{FONT_UI, FS_XS, R_SM};
use crate::theme::{ActiveTheme, ThemeColors};

use super::tabular_font;

/// What a node shows at its trailing edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeNodeTrailing {
    /// Ahead and behind commit counts, e.g. `↑4` / `↓2`.
    AheadBehind { ahead: usize, behind: usize },
    /// A quiet count pill.
    Count(SharedString),
    /// Muted state text such as `locked`.
    State(SharedString),
}

/// One visible row of a [`Tree`].
#[derive(Clone, Debug)]
pub struct TreeNodeData {
    /// Identifies the node across renders and reordering. Defaults to the
    /// label; set an explicit key when two rows may share a label.
    pub key: SharedString,
    pub label: SharedString,
    pub level: usize,
    pub icon: Option<IconName>,
    pub has_children: bool,
    pub expanded: bool,
    pub selected: bool,
    pub current: bool,
    /// Section headers are uppercase, muted and have no icon.
    pub section: bool,
    pub trailing: Option<TreeNodeTrailing>,
}

impl TreeNodeData {
    pub fn new(label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            key: label.clone(),
            label,
            level: 0,
            icon: None,
            has_children: false,
            expanded: false,
            selected: false,
            current: false,
            section: false,
            trailing: None,
        }
    }

    /// A tree section header.
    pub fn section(label: impl Into<SharedString>) -> Self {
        let mut node = Self::new(label);
        node.section = true;
        node
    }

    /// A stable identity, used to keep the keyboard marker on the same node
    /// when rows above it are collapsed or removed.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = key.into();
        self
    }

    pub fn level(mut self, level: usize) -> Self {
        self.level = level;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn has_children(mut self, has_children: bool) -> Self {
        self.has_children = has_children;
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The currently checked-out branch.
    pub fn current(mut self) -> Self {
        self.current = true;
        self
    }

    pub fn ahead(mut self, ahead: usize) -> Self {
        let behind = match self.trailing {
            Some(TreeNodeTrailing::AheadBehind { behind, .. }) => behind,
            _ => 0,
        };
        self.trailing = Some(TreeNodeTrailing::AheadBehind { ahead, behind });
        self
    }

    pub fn behind(mut self, behind: usize) -> Self {
        let ahead = match self.trailing {
            Some(TreeNodeTrailing::AheadBehind { ahead, .. }) => ahead,
            _ => 0,
        };
        self.trailing = Some(TreeNodeTrailing::AheadBehind { ahead, behind });
        self
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.trailing = Some(TreeNodeTrailing::Count(count.into()));
        self
    }

    pub fn state(mut self, state: impl Into<SharedString>) -> Self {
        self.trailing = Some(TreeNodeTrailing::State(state.into()));
        self
    }

    pub fn trailing(mut self, trailing: TreeNodeTrailing) -> Self {
        self.trailing = Some(trailing);
        self
    }
}

type NodeHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;
type ContextHandler = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static>;

struct TreeState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The marker follows a node key, not a row index, so collapsing or
    /// removing rows above it keeps it on the same node.
    marker_key: Option<SharedString>,
    marker_index: usize,
}

/// The row index of the marker.
///
/// Before any keyboard move (`key` is `None`) the marker starts on the
/// caller's selected node, then on the first row. Once a key exists it wins,
/// with the last known index as the fallback when that node has gone.
fn marker_index(
    keys: &[SharedString],
    selected: Option<usize>,
    key: Option<&SharedString>,
    fallback: usize,
) -> Option<usize> {
    if keys.is_empty() {
        None
    } else {
        Some(
            match key {
                Some(key) => keys
                    .iter()
                    .position(|candidate| candidate == key)
                    .unwrap_or(fallback),
                None => selected.unwrap_or(fallback),
            }
            .min(keys.len() - 1),
        )
    }
}

/// A flat, visible list of tree nodes.
#[derive(IntoElement)]
pub struct Tree {
    id: ElementId,
    nodes: Vec<TreeNodeData>,
    nav: bool,
    horizontal_scroll: bool,
    focus_handle: Option<FocusHandle>,
    scroll_handle: Option<ScrollHandle>,
    on_activate: Option<NodeHandler>,
    on_toggle: Option<NodeHandler>,
    on_expand: Option<NodeHandler>,
    on_collapse: Option<NodeHandler>,
    on_open: Option<NodeHandler>,
    on_context_menu: Option<ContextHandler>,
}

impl Tree {
    pub fn new(id: impl Into<ElementId>, nodes: Vec<TreeNodeData>) -> Self {
        Self {
            id: id.into(),
            nodes,
            nav: false,
            horizontal_scroll: false,
            focus_handle: None,
            scroll_handle: None,
            on_activate: None,
            on_toggle: None,
            on_expand: None,
            on_collapse: None,
            on_open: None,
            on_context_menu: None,
        }
    }

    /// The settings-navigation variant: no indentation, 10px left padding.
    pub fn nav(mut self) -> Self {
        self.nav = true;
        self
    }

    /// Lets long labels widen the rows and scrolls the tree horizontally
    /// instead of truncating them.
    pub fn horizontal_scroll(mut self) -> Self {
        self.horizontal_scroll = true;
        self
    }

    /// Uses an existing focus handle instead of the tree's own keyed one.
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }

    /// Uses an existing scroll handle, so a [`Scrollbar`](crate::Scrollbar)
    /// can share it. Without one the tree keeps its own handle and still
    /// scrolls the keyboard marker into view.
    pub fn scroll_handle(mut self, handle: ScrollHandle) -> Self {
        self.scroll_handle = Some(handle);
        self
    }

    /// Enter, Space or a click on a leaf.
    pub fn on_activate(
        mut self,
        listener: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(listener));
        self
    }

    /// A click on an expandable row; the caller flips its expansion state.
    pub fn on_toggle(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(listener));
        self
    }

    /// → on a collapsed expandable row.
    pub fn on_expand(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_expand = Some(Rc::new(listener));
        self
    }

    /// ← on an expanded expandable row.
    pub fn on_collapse(
        mut self,
        listener: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_collapse = Some(Rc::new(listener));
        self
    }

    /// Double-click or double activation of a leaf node; parents toggle instead.
    pub fn on_open(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(listener));
        self
    }

    /// Right-click, with the pointer position in window coordinates.
    pub fn on_context_menu(
        mut self,
        listener: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for Tree {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let id_name = self.id.to_string();
        let keys: Vec<SharedString> = self.nodes.iter().map(|node| node.key.clone()).collect();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| TreeState {
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            marker_key: None,
            marker_index: 0,
        });
        let focus = self
            .focus_handle
            .clone()
            .unwrap_or_else(|| state.read(cx).focus.clone())
            .tab_stop(true);
        let scroll = self
            .scroll_handle
            .clone()
            .unwrap_or_else(|| state.read(cx).scroll.clone());
        let ring_visible = focus::focus_visible(&focus, window, cx);
        let selected_index = self.nodes.iter().position(|node| node.selected);
        let keyboard_index = {
            let state = state.read(cx);
            marker_index(
                &keys,
                selected_index,
                state.marker_key.as_ref(),
                state.marker_index,
            )
        };

        let horizontal = self.horizontal_scroll;
        let mut root = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .pb(px(8.))
            .when(horizontal, |this| {
                // Rows size to their content and the tree scrolls sideways.
                this.items_start().px(px(6.)).overflow_x_scroll()
            })
            .track_scroll(&scroll)
            .overflow_y_scroll()
            .key_context("ui-tree")
            .track_focus(&focus);

        // Keyboard navigation: ↓ / ↑ move the marker, Enter / Space activate,
        // ← / → collapse or expand an expandable row.
        {
            let state = state.clone();
            let keys = keys.clone();
            let scroll = scroll.clone();
            root = root.on_action(move |_: &SelectNext, window: &mut Window, cx: &mut App| {
                let next = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let Some(current) = next {
                    let next = (current + 1).min(keys.len() - 1);
                    state.update(cx, |state, _| {
                        state.marker_key = Some(keys[next].clone());
                        state.marker_index = next;
                    });
                    scroll.scroll_to_item(next);
                }
                window.refresh();
            });
        }
        {
            let state = state.clone();
            let keys = keys.clone();
            let scroll = scroll.clone();
            root = root.on_action(move |_: &SelectPrev, window: &mut Window, cx: &mut App| {
                let current = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let Some(current) = current {
                    let next = current.saturating_sub(1);
                    state.update(cx, |state, _| {
                        state.marker_key = Some(keys[next].clone());
                        state.marker_index = next;
                    });
                    scroll.scroll_to_item(next);
                }
                window.refresh();
            });
        }
        {
            let state = state.clone();
            let keys = keys.clone();
            let on_activate = self.on_activate.clone();
            root = root.on_action(move |_: &Confirm, window: &mut Window, cx: &mut App| {
                let index = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let (Some(index), Some(listener)) = (index, &on_activate) {
                    listener(index, window, cx);
                }
            });
        }
        {
            let state = state.clone();
            let keys = keys.clone();
            let on_activate = self.on_activate.clone();
            root = root.on_action(move |_: &TreeActivate, window: &mut Window, cx: &mut App| {
                let index = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let (Some(index), Some(listener)) = (index, &on_activate) {
                    listener(index, window, cx);
                }
            });
        }
        {
            let state = state.clone();
            let keys = keys.clone();
            let on_collapse = self.on_collapse.clone();
            let flags: Vec<(bool, bool)> = self
                .nodes
                .iter()
                .map(|node| (node.has_children, node.expanded))
                .collect();
            root = root.on_action(move |_: &TreeCollapse, window: &mut Window, cx: &mut App| {
                let index = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let Some(index) = index {
                    if flags[index] == (true, true) {
                        if let Some(listener) = &on_collapse {
                            listener(index, window, cx);
                        }
                    }
                }
            });
        }
        {
            let state = state.clone();
            let keys = keys.clone();
            let on_expand = self.on_expand.clone();
            let flags: Vec<(bool, bool)> = self
                .nodes
                .iter()
                .map(|node| (node.has_children, node.expanded))
                .collect();
            root = root.on_action(move |_: &TreeExpand, window: &mut Window, cx: &mut App| {
                let index = {
                    let state = state.read(cx);
                    marker_index(
                        &keys,
                        selected_index,
                        state.marker_key.as_ref(),
                        state.marker_index,
                    )
                };
                if let Some(index) = index {
                    if flags[index] == (true, false) {
                        if let Some(listener) = &on_expand {
                            listener(index, window, cx);
                        }
                    }
                }
            });
        }

        for (index, node) in self.nodes.iter().enumerate() {
            let selected = node.selected;
            let current = node.current;
            let section = node.section;
            let has_children = node.has_children;
            let expanded = node.expanded;
            let marked = keyboard_index == Some(index) && ring_visible;

            let foreground = if selected {
                colors.selection_fg
            } else if section {
                colors.muted
            } else {
                colors.text
            };
            let secondary = if selected {
                colors.selection_sub
            } else {
                colors.muted
            };
            let icon_color = if current {
                colors.accent
            } else if selected {
                colors.selection_sub
            } else {
                colors.muted
            };

            let twisty: gpui::AnyElement = if has_children {
                Icon::new(if expanded {
                    IconName::ChevD
                } else {
                    IconName::ChevR
                })
                .size(px(12.))
                .color(secondary)
                .into_any_element()
            } else {
                div().w(px(12.)).h(px(12.)).into_any_element()
            };

            let icon_slot = if section {
                None
            } else {
                Some(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(node.icon, |this, icon| {
                            this.child(Icon::new(icon).size(px(14.)).color(icon_color))
                        }),
                )
            };

            let label = if section {
                node.label.to_uppercase()
            } else {
                node.label.to_string()
            };
            let label_size = if section {
                FS_XS
            } else {
                cx.theme().ui_font_size
            };
            let label_weight = if section || current {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            };

            let pad_left = if self.nav {
                px(10.)
            } else if section {
                px(6.)
            } else {
                px(6. + 14. * node.level as f32)
            };

            let row_id = ElementId::named_usize(id_name.clone(), index);
            let debug_name = id_name.clone();
            let label_element = if horizontal {
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .font_weight(label_weight)
                    .child(label)
            } else {
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_weight(label_weight)
                    .child(label)
            };

            let mut row = div()
                .id(row_id)
                .relative()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(cx.theme().row_height)
                // The tree is a flex column inside a scroll container: without
                // this every row shrinks below its fixed height when the
                // content overflows, instead of the tree scrolling.
                .flex_none()
                .when(horizontal, |this| this.min_w_full())
                .when(!horizontal, |this| this.ml(px(6.)).mr(px(6.)))
                .pl(pad_left)
                .pr(px(8.))
                .rounded(R_SM)
                .text_size(label_size)
                .line_height(label_size)
                .font_family(FONT_UI)
                .text_color(foreground)
                .when(section, |this| this.mt(px(10.)))
                .debug_selector(move || format!("{debug_name}-node-{index}"))
                .child(twisty)
                .when_some(icon_slot, |this, slot| this.child(slot))
                .child(label_element);

            if let Some(trailing) = node.trailing.as_ref() {
                row = row.child(trailing_element(trailing, selected, secondary, colors));
            }

            if current {
                row = row.child(
                    div()
                        .absolute()
                        .left(px(-4.))
                        .top(px(4.))
                        .bottom(px(4.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(colors.accent),
                );
            }

            if selected {
                row = row.bg(colors.selection);
            } else {
                let hover = colors.hover;
                row = row.hover(move |style| style.bg(hover));
            }

            {
                let state = state.clone();
                let focus = focus.clone();
                let key = node.key.clone();
                let on_open = self.on_open.clone();
                let on_toggle = self.on_toggle.clone();
                let on_activate = self.on_activate.clone();
                row = row.on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        // Focus the tree ourselves: the listeners below may
                        // open a menu or dialog that must keep the focus.
                        window.focus(&focus, cx);
                        state.update(cx, |state, _| {
                            state.marker_key = Some(key.clone());
                            state.marker_index = index;
                        });
                        if has_children {
                            // A parent toggles on every click, however fast
                            // the series: the second click of a double click
                            // must collapse what the first one expanded.
                            if let Some(listener) = &on_toggle {
                                listener(index, window, cx);
                            }
                        } else if event.click_count >= 2 {
                            if let Some(listener) = &on_open {
                                listener(index, window, cx);
                            }
                        } else if let Some(listener) = &on_activate {
                            listener(index, window, cx);
                        }
                        // Stop the tree's own track_focus handler from taking
                        // focus back from a dialog/menu opened above.
                        window.prevent_default();
                    },
                );
            }
            {
                let state = state.clone();
                let focus = focus.clone();
                let key = node.key.clone();
                let on_context_menu = self.on_context_menu.clone();
                row = row.on_mouse_down(
                    MouseButton::Right,
                    move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        window.focus(&focus, cx);
                        state.update(cx, |state, _| {
                            state.marker_key = Some(key.clone());
                            state.marker_index = index;
                        });
                        if let Some(listener) = &on_context_menu {
                            listener(index, event.position, window, cx);
                        }
                        window.prevent_default();
                    },
                );
            }

            if marked {
                row = row.child(focus::focus_ring(RingOffset::Inset2, R_SM, cx));
            }

            root = root.child(row);
        }

        root
    }
}

fn trailing_element(
    trailing: &TreeNodeTrailing,
    selected: bool,
    secondary: Hsla,
    colors: ThemeColors,
) -> gpui::AnyElement {
    match trailing {
        TreeNodeTrailing::AheadBehind { ahead, behind } => {
            let mut row = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .text_size(FS_XS)
                .line_height(FS_XS)
                .font(tabular_font());
            if *ahead > 0 {
                row = row.child(
                    div()
                        .text_color(if selected { secondary } else { colors.success })
                        .child(format!("↑{ahead}")),
                );
            }
            if *behind > 0 {
                row = row.child(
                    div()
                        .text_color(if selected { secondary } else { colors.warning })
                        .child(format!("↓{behind}")),
                );
            }
            row.into_any_element()
        }
        TreeNodeTrailing::Count(value) => CountPill::new(value.clone())
            .selected(selected)
            .into_any_element(),
        TreeNodeTrailing::State(value) => div()
            .flex_none()
            .text_size(FS_XS)
            .line_height(FS_XS)
            .text_color(secondary)
            .child(value.clone())
            .into_any_element(),
    }
}
