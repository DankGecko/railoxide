use gpui::{
    AnyElement, Div, ElementId, Entity, FontWeight, IntoElement, ParentElement, SharedString,
    Styled, div, px, relative, rgb,
};
use gpui_component::input::{Input, InputState};
use gpui_component::{Selectable, button::Button};

use crate::theme::{self, APP_TEXT_SIZE};

#[must_use]
pub fn app_input(state: &Entity<InputState>) -> Input {
    Input::new(state).w_full().px(px(8.0))
}

#[must_use]
pub fn app_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    app_button_base(id).child(app_button_label(label))
}

#[must_use]
pub fn app_button_base(id: impl Into<ElementId>) -> Button {
    Button::new(id)
}

#[must_use]
pub fn app_segment_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    accessory: Option<AnyElement>,
) -> Button {
    app_button_base(id).selected(selected).child(
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .child(app_button_label(label))
            .children(accessory),
    )
}

#[must_use]
pub fn app_inline_control_row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().min_w(px(0.0)).child(app_muted_text(label)))
        .child(div().flex_none().child(control))
}

#[must_use]
pub fn app_button_label(label: impl Into<SharedString>) -> Div {
    app_text(label).flex_none()
}

#[must_use]
pub fn app_text(label: impl Into<SharedString>) -> Div {
    div()
        .text_size(APP_TEXT_SIZE)
        .line_height(relative(1.0))
        .child(label.into())
}

#[must_use]
pub fn app_muted_text(label: impl Into<SharedString>) -> Div {
    app_text(label).text_color(rgb(theme::TEXT_MUTED))
}

#[must_use]
pub fn app_strong_text(label: impl Into<SharedString>) -> Div {
    app_text(label)
        .text_color(rgb(theme::TEXT))
        .font_weight(FontWeight::SEMIBOLD)
}
