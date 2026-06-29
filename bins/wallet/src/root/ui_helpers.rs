use gpui::{
    App, ElementId, IntoElement, ParentElement, Pixels, SharedString, Styled, Window, div, img,
    prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{
    Disableable, Icon, Sizable,
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::{Dialog, DialogButtonProps},
    tag::Tag,
};
use ui::clipboard::clipboard_with_toast;
use ui::controls::{app_button_base, app_muted_text, app_strong_text, app_text};
use ui::icons;
use ui::theme::{self, APP_FONT_FAMILY, APP_TEXT_SIZE};

use crate::assets::WalletIconSource;

const DIALOG_CONTENT_HORIZONTAL_INSET: Pixels = px(56.0);

pub(super) fn rgb_with_alpha(hex: u32, alpha: f32) -> gpui::Rgba {
    let mut color = rgb(hex);
    color.a = alpha;
    color
}

pub(super) fn count_label(count: usize, singular: &'static str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

pub(super) fn centered_message(message: impl Into<SharedString>) -> gpui::Div {
    let message = message.into();
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(theme::TEXT_SUBTLE))
        .child(message)
}

pub(super) fn secondary_dialog_content_width(dialog_width: Pixels) -> Pixels {
    (dialog_width - DIALOG_CONTENT_HORIZONTAL_INSET).max(px(0.0))
}

pub(super) fn dialog_max_height(window: &Window) -> Pixels {
    window.viewport_size().height * 0.84
}

pub(super) fn dialog_content_max_height(window: &Window) -> Pixels {
    window.viewport_size().height * 0.74
}

pub(super) fn scrollable_dialog_content(
    max_height: Pixels,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .max_h(max_height)
        .min_h(px(0.0))
        .overflow_y_scrollbar()
        .child(content)
}

#[derive(Clone, Copy)]
pub(super) struct ConfirmationDialogProps {
    title: &'static str,
    body: &'static str,
    detail: Option<&'static str>,
    confirm_text: &'static str,
    confirm_variant: ButtonVariant,
}

impl ConfirmationDialogProps {
    pub(super) const fn danger(
        title: &'static str,
        body: &'static str,
        detail: Option<&'static str>,
        confirm_text: &'static str,
    ) -> Self {
        Self {
            title,
            body,
            detail,
            confirm_text,
            confirm_variant: ButtonVariant::Danger,
        }
    }
}

pub(super) fn confirmation_dialog(
    dialog: Dialog,
    props: ConfirmationDialogProps,
    dialog_width: Pixels,
    dialog_max_height: Pixels,
    content_max_height: Pixels,
) -> Dialog {
    let content_width = secondary_dialog_content_width(dialog_width);
    dialog
        .w(dialog_width)
        .max_h(dialog_max_height)
        .title(app_strong_text(props.title))
        .button_props(
            DialogButtonProps::default()
                .ok_text(props.confirm_text)
                .ok_variant(props.confirm_variant),
        )
        .confirm()
        .child(scrollable_dialog_content(
            content_max_height,
            confirmation_dialog_content(props, content_width),
        ))
}

fn confirmation_dialog_content(props: ConfirmationDialogProps, content_width: Pixels) -> gpui::Div {
    div()
        .w(content_width)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            app_text(props.body)
                .line_height(px(20.0))
                .whitespace_normal(),
        )
        .when_some(props.detail, |this, detail| {
            this.child(
                app_muted_text(detail)
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .whitespace_normal(),
            )
        })
}

pub(super) fn app_panel(bg: u32, border: u32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p(px(12.0))
        .rounded_md()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
}

pub(super) fn app_refresh_button(
    id: impl Into<ElementId>,
    tooltip: impl Into<SharedString>,
    refreshing: bool,
    enabled: bool,
    on_refresh: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    let button = app_button_base(id)
        .ghost()
        .xsmall()
        .compact()
        .icon(Icon::empty().path(icons::refresh_ccw_icon_path()))
        .tooltip(tooltip)
        .loading(refreshing)
        .disabled(refreshing || !enabled);

    if enabled && !refreshing {
        button.on_click(move |_event, window, cx| {
            cx.stop_propagation();
            on_refresh(window, cx);
        })
    } else {
        button
    }
}

pub(super) fn app_status_tag(label: impl Into<SharedString>, color: u32) -> impl IntoElement {
    Tag::custom(
        rgb_with_alpha(color, 0.12).into(),
        rgb(color).into(),
        rgb(color).into(),
    )
    .small()
    .rounded_full()
    .child(label.into())
}

pub(super) fn copyable_mono_field(
    label: &'static str,
    value: String,
    button_id: impl Into<ElementId>,
) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(72.0))
                .flex_none()
                .text_color(rgb(theme::TEXT_MUTED))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .p(px(8.0))
                .rounded_sm()
                .bg(rgb(theme::BACKGROUND))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .font_family(APP_FONT_FAMILY)
                .text_size(APP_TEXT_SIZE)
                .text_color(rgb(theme::TEXT))
                .child(SharedString::from(value.clone())),
        )
        .child(clipboard_with_toast(button_id, value))
}

pub(super) fn app_stepper_container() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_0()
        .p(px(10.0))
        .rounded_md()
        .bg(rgb(theme::SURFACE_HOVER_SUBTLE))
        .border_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
}

pub(super) fn app_step_row(
    marker: impl IntoElement,
    body: impl IntoElement,
    is_last: bool,
    color: u32,
    connector_min_height: Pixels,
    connector_opacity: Option<f32>,
) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(marker)
                .children((!is_last).then(|| {
                    let connector = div()
                        .w(px(2.0))
                        .flex_1()
                        .min_h(connector_min_height)
                        .my(px(3.0))
                        .rounded_full()
                        .bg(rgb(color));
                    if let Some(opacity) = connector_opacity {
                        connector.opacity(opacity)
                    } else {
                        connector
                    }
                })),
        )
        .child(body)
}

pub(super) fn labeled_field(
    label: impl Into<SharedString>,
    content: impl IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(app_muted_text(label))
        .child(content)
}

pub(super) fn token_label_row(
    label: SharedString,
    icon_path: Option<WalletIconSource>,
    icon_size: Pixels,
) -> gpui::Div {
    let mut row = div().flex().items_center().gap_1();
    if let Some(path) = icon_path {
        row = row.child(img(path).size(icon_size).rounded_full().flex_none());
    }
    row.child(label)
}
