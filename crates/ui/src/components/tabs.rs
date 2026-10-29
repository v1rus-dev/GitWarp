//! Tab bars.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, transparent_black, App, ElementId, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{FS_BASE, R_SM};
use crate::theme::ActiveTheme;

/// One tab in a [`Tabs`] bar.
pub struct Tab {
    id: Option<ElementId>,
    label: SharedString,
    icon: Option<IconName>,
    closable: bool,
}

impl Tab {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            label: label.into(),
            icon: None,
            closable: false,
        }
    }

    /// A stable identity for the tab. Focus and element state are keyed by
    /// this (or by the label when unset), so closing another tab keeps the
    /// keyboard focus on the same tab. Set it when labels can repeat.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Show a close button on hover or while selected.
    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }
}

/// A horizontal tab bar.
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<Tab>,
    selected: usize,
    disabled: bool,
    on_select: Option<crate::components::SelectHandler>,
    on_close: Option<crate::components::SelectHandler>,
}

impl Tabs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            selected: 0,
            disabled: false,
            on_select: None,
            on_close: None,
        }
    }

    pub fn tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = Tab>) -> Self {
        self.tabs.extend(tabs);
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
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

    pub fn on_close(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let bar_id = self.id.clone();

        let mut bar = div()
            .id(self.id)
            .flex()
            .items_end()
            .gap(px(2.))
            .px(px(8.))
            .w_full()
            .bg(colors.bg)
            .border_b_1()
            .border_color(colors.border);

        let mut label_occurrences: HashMap<SharedString, usize> = HashMap::new();
        for (index, tab) in self.tabs.into_iter().enumerate() {
            let selected = index == self.selected;
            // Key by the tab's own identity, never by its position: removing
            // a tab must not hand its focus handle to the next tab. Duplicate
            // labels get `label#n` so they do not share a focus handle.
            let key = match &tab.id {
                Some(id) => id.to_string(),
                None => {
                    let occurrence = label_occurrences.entry(tab.label.clone()).or_insert(0);
                    *occurrence += 1;
                    format!("{}#{}", tab.label, *occurrence)
                }
            };
            let child_id =
                ElementId::NamedChild(Box::new(bar_id.clone()), SharedString::from(key.clone()));
            let focus = window
                .use_keyed_state(child_id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone()
                .tab_stop(!self.disabled);
            let ring = !self.disabled && focus::focus_visible(&focus, window, cx);
            let foreground = if selected { colors.text } else { colors.muted };

            let mut element = div()
                .id(child_id.clone())
                .group("ui-tab")
                .relative()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(30.))
                .px(px(10.))
                .mb(px(-1.))
                .key_context("ui-tab")
                .border_b_2()
                .border_color(if selected {
                    colors.accent
                } else {
                    transparent_black()
                })
                .text_size(FS_BASE)
                .text_color(foreground)
                .when_some(tab.icon, |this, icon| {
                    this.child(Icon::new(icon).size(px(14.)).color(foreground))
                })
                .child(tab.label);

            if tab.closable {
                let mut close = div()
                    .id(ElementId::NamedChild(
                        Box::new(child_id.clone()),
                        SharedString::from("close"),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.))
                    .h(px(16.))
                    .rounded(R_SM)
                    .opacity(if selected { 1. } else { 0. })
                    .group_hover("ui-tab", |style| style.opacity(1.))
                    .child(Icon::new(IconName::X).size(px(12.)).color(colors.muted));

                if self.disabled {
                    close = close.cursor_not_allowed();
                } else {
                    close = close
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.active));
                    if let Some(listener) = self.on_close.clone() {
                        close = close.on_click(move |_, window, cx| {
                            listener(index, window, cx);
                            cx.stop_propagation();
                        });
                    }
                }
                element = element.child(close);
            }

            if self.disabled {
                element = element.cursor_not_allowed();
            } else {
                element = element.cursor_pointer().track_focus(&focus);
                if !selected {
                    element = element.hover(|style| style.bg(colors.hover).text_color(colors.text));
                }
                if let Some(listener) = self.on_select.clone() {
                    element = element.on_click(move |_, window, cx| listener(index, window, cx));
                }
            }

            if ring {
                element = element.child(focus::focus_ring(RingOffset::Outset1, R_SM, cx));
            }
            bar = bar.child(element);
        }

        bar
    }
}
