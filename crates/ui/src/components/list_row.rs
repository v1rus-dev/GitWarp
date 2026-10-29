//! Generic 26px list rows: history commits, pick lists and friends.

use gpui::{
    div, px, transparent_black, App, ElementId, Hsla, IntoElement, ParentElement, RenderOnce,
    Styled, Window,
};
use gpui::{AnyElement, InteractiveElement, StatefulInteractiveElement};

use crate::focus::{self, RingOffset};
use crate::metrics::{FONT_UI, FS_BASE, ROW, R_MD};
use crate::theme::{ActiveTheme, ThemeColors};

use super::ClickHandler;

/// A 26px row with the four list states.
#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    selected: bool,
    list_focused: bool,
    keyboard: bool,
    on_click: Option<ClickHandler>,
    children: Vec<AnyElement>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            list_focused: false,
            keyboard: false,
            on_click: None,
            children: Vec::new(),
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Whether the list that owns this row has focus. A selected row is
    /// `selection_inactive` while its list is unfocused.
    pub fn list_focused(mut self, focused: bool) -> Self {
        self.list_focused = focused;
        self
    }

    /// The keyboard marker: an inset 2px focus ring.
    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.keyboard = keyboard;
        self
    }

    pub fn on_click(
        mut self,
        listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    /// The colour for secondary text in a row with the given state.
    pub fn secondary_color(colors: ThemeColors, selected: bool, list_focused: bool) -> Hsla {
        if selected {
            if list_focused {
                colors.selection_sub
            } else {
                colors.muted
            }
        } else {
            colors.muted
        }
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let background = if self.selected {
            if self.list_focused {
                colors.selection
            } else {
                colors.selection_inactive
            }
        } else {
            transparent_black()
        };
        let foreground = if self.selected && self.list_focused {
            colors.selection_fg
        } else {
            colors.text
        };

        let debug = format!("list-row-{}", self.id);
        let mut row = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(ROW)
            // A uniform_list lays each item out as a root with a definite
            // width and does not stretch its children, so the row itself must
            // fill its wrapper. The wrapper below owns the 6px side gutters.
            .w_full()
            .debug_selector(move || debug.clone())
            .rounded(R_MD)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .text_color(foreground)
            .bg(background)
            .children(self.children);

        if !self.selected {
            let hover = colors.hover;
            row = row.hover(move |style| style.bg(hover));
        }

        if let Some(listener) = self.on_click {
            row = row.on_click(move |event, window, cx| listener(event, window, cx));
        }

        if self.keyboard {
            row = row.child(focus::focus_ring(RingOffset::Inset2, R_MD, cx));
        }

        // Padding (rather than margins on the row) works in both a plain flex
        // column and a uniform_list, where the item root is laid out with a
        // definite width and does not flex-stretch its child. The wrapper is
        // the flex item in a column list, so it must not shrink either.
        div().w_full().px(px(6.)).flex_none().child(row)
    }
}
