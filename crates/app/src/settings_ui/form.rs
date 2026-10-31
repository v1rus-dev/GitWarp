//! The building blocks shared by the settings pages: rows, groups, fields and
//! the controls laid out the way the design's `settings.css` lays them out.

use gpui::prelude::FluentBuilder;
use gpui::InteractiveElement;
use gpui::{
    div, px, AnyElement, App, ElementId, IntoElement, ParentElement, SharedString, Styled, Window,
};

use ui::metrics::{FS_BASE, FS_SM, R_SM};
use ui::theme::ActiveTheme;
use ui::{Button, ButtonSize, Checkbox, Segmented, Select, Switch};

/// A two-column settings row: title and description on the left, a control on
/// the right. Disabled rows dim their text like the design.
pub fn row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
    disabled: bool,
) -> AnyElement {
    let id = id.into();
    div()
        .id(id)
        .w_full()
        .min_h(px(44.))
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(16.))
        .border_b_1()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(FS_BASE)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .child(
                    div()
                        .max_w(px(520.))
                        .text_size(FS_SM)
                        .child(description.into()),
                )
                .when(disabled, |this| this.opacity(0.5)),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .child(control),
        )
        .into_any_element()
}

/// A full-width field row, for text inputs and custom commands.
pub fn field_row(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(12.))
        .py(px(10.))
        .border_b_1()
        .child(div().text_size(FS_SM).child(title.into()))
        .child(control)
        .when_some(description, |this, description| {
            this.child(div().text_size(FS_SM).child(description))
        })
        .into_any_element()
}

/// A select with the design's 180px width.
pub fn select(
    id: ElementId,
    labels: impl IntoIterator<Item = impl Into<SharedString>>,
    selected: usize,
    disabled: bool,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .w(px(180.))
        .child(
            Select::new(id)
                .options(labels)
                .selected(selected)
                .disabled(disabled)
                .on_change(on_change),
        )
        .into_any_element()
}

/// A switch with the design's 32px hit area.
pub fn switch(
    id: ElementId,
    checked: bool,
    disabled: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Switch::new(id)
        .checked(checked)
        .disabled(disabled)
        .on_toggle(on_toggle)
        .into_any_element()
}

/// A checkbox with a "Confirm" label.
pub fn checkbox(
    id: ElementId,
    label: impl Into<SharedString>,
    checked: bool,
    disabled: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Checkbox::new(id)
        .label(label)
        .checked(checked)
        .disabled(disabled)
        .on_toggle(on_toggle)
        .into_any_element()
}

/// A segmented control from label/value pairs.
pub fn segmented(
    id: ElementId,
    labels: impl IntoIterator<Item = impl Into<SharedString>>,
    selected: usize,
    disabled: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let mut control = Segmented::new(id);
    for label in labels {
        control = control.item(ui::SegmentItem::new(label));
    }
    control
        .selected(selected)
        .disabled(disabled)
        .on_select(on_select)
        .into_any_element()
}

/// The numeric stepper: minus, value box, plus.
pub fn stepper(
    id: &'static str,
    value: impl Into<SharedString>,
    can_decrease: bool,
    can_increase: bool,
    colors: ui::ThemeColors,
    on_decrease: impl Fn(&mut Window, &mut App) + 'static,
    on_increase: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            Button::new(ElementId::Name(format!("{id}-minus").into()))
                .icon_only(ui::IconName::Minus)
                .size(ButtonSize::Small)
                .disabled(!can_decrease)
                .tooltip("Decrease")
                .on_click(move |_, window, cx| on_decrease(window, cx)),
        )
        .child(
            div()
                .id(ElementId::Name(format!("{id}-value").into()))
                .w(px(48.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(R_SM)
                .border_1()
                .border_color(colors.border_strong)
                .font_family(ui::metrics::FONT_MONO)
                .text_size(FS_SM)
                .child(value.into()),
        )
        .child(
            Button::new(ElementId::Name(format!("{id}-plus").into()))
                .icon_only(ui::IconName::Plus)
                .size(ButtonSize::Small)
                .disabled(!can_increase)
                .tooltip("Increase")
                .on_click(move |_, window, cx| on_increase(window, cx)),
        )
        .into_any_element()
}

/// Muted help text under a field.
pub fn help(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    div()
        .text_size(FS_SM)
        .text_color(colors.muted)
        .child(text.into())
        .into_any_element()
}

/// A red validation message.
pub fn error(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(FS_SM)
        .text_color(colors.error)
        .child(
            ui::Icon::new(ui::IconName::Err)
                .size(px(12.))
                .color(colors.error),
        )
        .child(text.into())
        .into_any_element()
}

/// A green success message.
pub fn success(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(FS_SM)
        .text_color(colors.success)
        .child(
            ui::Icon::new(ui::IconName::Check)
                .size(px(12.))
                .color(colors.success),
        )
        .child(text.into())
        .into_any_element()
}
