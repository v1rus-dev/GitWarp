//! A select control: an input-looking button that opens a menu of options.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, point, px, size, App, Bounds, ElementId, FocusHandle, IntoElement, ParentElement,
    Pixels, RenderOnce, SharedString, Styled, Window,
};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::actions::SelectNext;
use crate::components::menu::{self, MenuEntry, MenuItem};
use crate::focus::{self, RingOffset};
use crate::icon::{Icon, IconName};
use crate::metrics::{CTL, FONT_UI, FS_BASE, R_SM};
use crate::theme::ActiveTheme;

type BoundsCell = Rc<Cell<Option<Bounds<Pixels>>>>;

/// A select control. The current value is marked with a check in its menu.
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    disabled: bool,
    focus_handle: Option<FocusHandle>,
    on_change: Option<crate::components::SelectHandler>,
}

impl Select {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: 0,
            disabled: false,
            focus_handle: None,
            on_change: None,
        }
    }

    /// Uses the caller's focus handle, e.g. to list the select in a
    /// [`Dialog::focus_order`](crate::Dialog::focus_order).
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }

    pub fn options(mut self, options: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(listener));
        self
    }

    fn entries(&self) -> Vec<MenuEntry> {
        self.options
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let mut item = MenuItem::new(label.clone());
                if index == self.selected {
                    item = item.icon(IconName::Check);
                }
                let on_change = self.on_change.clone();
                item = item.action(move |window, cx| {
                    if let Some(on_change) = &on_change {
                        on_change(index, window, cx);
                    }
                });
                MenuEntry::Item(item)
            })
            .collect()
    }
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focus = match self.disabled {
            true => None,
            false => Some(
                self.focus_handle
                    .clone()
                    .unwrap_or_else(|| {
                        window
                            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
                            .read(cx)
                            .clone()
                    })
                    .tab_stop(true),
            ),
        };
        let ring = focus
            .as_ref()
            .is_some_and(|focus| focus::focus_visible(focus, window, cx));
        let bounds: BoundsCell = window
            .use_keyed_state(
                ElementId::Name(format!("{}-bounds", self.id).into()),
                cx,
                |_, _| Rc::new(Cell::new(None)),
            )
            .read(cx)
            .clone();

        let label = self.options.get(self.selected).cloned().unwrap_or_default();
        let entries = self.entries();
        // The menu opens on the current value (a native `<select>` does), or on
        // the first item when the value is not among the options.
        let highlight = Some(if self.selected < self.options.len() {
            self.selected
        } else {
            0
        });
        let bounds_slot = bounds.clone();
        let open = {
            let bounds = bounds.clone();
            move |window: &mut Window, cx: &mut App| {
                let anchor = bounds
                    .get()
                    .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))));
                menu::open_dropdown_at(anchor, entries.clone(), highlight, window, cx);
            }
        };

        let mut root = div()
            .id(self.id)
            .relative()
            .h(CTL)
            .px(px(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .rounded(R_SM)
            .border_1()
            .border_color(colors.border_strong)
            .bg(colors.bg)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .text_color(if self.disabled {
                colors.disabled
            } else {
                colors.text
            })
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, _, _| {
                        bounds_slot.set(Some(bounds));
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(Icon::new(IconName::ChevD).size(px(12.)).color(colors.muted));

        if self.disabled {
            root = root.opacity(0.5).cursor_not_allowed();
        } else {
            let hover_border = colors.muted;
            let open_click = open.clone();
            let open_key = open;
            root = root
                .cursor_pointer()
                .key_context("ui-select")
                .track_focus(focus.as_ref().expect("enabled select has a focus handle"))
                .hover(move |style| style.border_color(hover_border))
                .on_click(move |_, window, cx| open_click(window, cx))
                // Enter and Space open the menu through the click handler;
                // Down opens it too. The menu takes over arrows, Enter, Esc.
                .on_action(move |_: &SelectNext, window, cx| open_key(window, cx));
        }

        if ring {
            root = root.child(focus::focus_ring(RingOffset::Outset1, R_SM, cx));
        }
        root
    }
}
