//! Segmented controls.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::components::Tooltip;
use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{CTL_SM, FS_BASE, FS_SM, R_SM};
use crate::theme::ActiveTheme;

/// One option of a [`Segmented`] control.
pub struct SegmentItem {
    id: Option<ElementId>,
    label: Option<SharedString>,
    icon: Option<IconName>,
    disabled: bool,
    tooltip: Option<SharedString>,
}

impl SegmentItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            label: Some(label.into()),
            icon: None,
            disabled: false,
            tooltip: None,
        }
    }

    pub fn icon(icon: IconName) -> Self {
        Self {
            id: None,
            label: None,
            icon: Some(icon),
            disabled: false,
            tooltip: None,
        }
    }

    /// A single disabled option; the rest of the control stays interactive.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A tooltip on this option, e.g. for icon-only segments.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// A stable identity for the segment; defaults to its label or icon, so
    /// focus stays on the same segment when items are added or removed.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A row of mutually exclusive segments.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    items: Vec<SegmentItem>,
    selected: usize,
    small: bool,
    disabled: bool,
    on_select: Option<crate::components::SelectHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            small: false,
            disabled: false,
            on_select: None,
        }
    }

    pub fn item(mut self, item: SegmentItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = SegmentItem>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_select(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let id_name = self.id.to_string();
        let bar_id = self.id.clone();
        let count = self.items.len();
        let height = if self.small { CTL_SM } else { px(24.) };
        let padding = if self.small { px(8.) } else { px(10.) };

        let mut container = div()
            .id(self.id)
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded(R_SM)
            .border_1()
            .border_color(colors.border_strong)
            .bg(colors.bg);

        let mut label_occurrences: HashMap<String, usize> = HashMap::new();
        for (index, item) in self.items.into_iter().enumerate() {
            let selected = index == self.selected;
            let item_disabled = self.disabled || item.disabled;
            // Key by identity, not position; duplicate labels/icons get
            // `label#n` so each item keeps its own focus handle.
            let key = match &item.id {
                Some(id) => id.to_string(),
                None => {
                    let base = item
                        .label
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("{:?}", item.icon));
                    let occurrence = label_occurrences.entry(base.clone()).or_insert(0);
                    *occurrence += 1;
                    format!("{base}#{occurrence}")
                }
            };
            let child_id = ElementId::NamedChild(Box::new(bar_id.clone()), SharedString::from(key));
            let focus = window
                .use_keyed_state(child_id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone()
                .tab_stop(!item_disabled);
            let ring = !item_disabled && focus::focus_visible(&focus, window, cx);
            let foreground = if item_disabled {
                colors.disabled
            } else if selected {
                colors.selection_fg
            } else {
                colors.muted
            };

            let mut segment = div()
                .id(child_id)
                .debug_selector({
                    let name = format!("{id_name}-segment-{index}");
                    move || name
                })
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(height)
                .px(padding)
                .key_context("ui-segmented")
                .text_size(if self.small { FS_SM } else { FS_BASE })
                .text_color(foreground)
                .when(selected, |this| this.bg(colors.selection))
                .when(index + 1 < count, |this| {
                    this.border_r_1().border_color(colors.border)
                })
                .when_some(item.icon, |this, icon| {
                    this.child(Icon::new(icon).size(px(14.)).color(foreground))
                })
                .when_some(item.label, |this, label| this.child(label));

            if item_disabled {
                segment = segment.cursor_not_allowed();
            } else {
                segment = segment.cursor_pointer().track_focus(&focus);
                if !selected {
                    segment = segment.hover(|style| style.bg(colors.hover).text_color(colors.text));
                }
                if let Some(listener) = self.on_select.clone() {
                    segment = segment.on_click(move |_, window, cx| listener(index, window, cx));
                }
            }

            if let Some(tooltip) = item.tooltip {
                segment = segment.tooltip(Tooltip::text(tooltip));
            }

            if ring {
                segment = segment.child(focus::focus_ring(RingOffset::Inset2, R_SM, cx));
            }
            container = container.child(segment);
        }

        if self.disabled {
            container.opacity(0.45)
        } else {
            container
        }
    }
}
