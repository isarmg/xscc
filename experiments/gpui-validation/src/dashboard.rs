use crate::theme::{self, Palette};
use gpui::{Div, FontWeight, Stateful, div, prelude::*, px, rgb};

pub fn card(palette: Palette) -> Div {
    div()
        .relative()
        .min_w_0()
        .flex_shrink_0()
        .p_5()
        .flex()
        .flex_col()
        .gap_4()
        .child(theme::surface(
            rgb(palette.panel),
            Some(rgb(palette.border)),
            theme::PANEL_EXTENT,
        ))
}

pub fn button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    primary: bool,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .min_h(px(theme::CONTROL_HEIGHT))
        .px_4()
        .py_2()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(if primary {
            palette.on_accent
        } else {
            palette.text
        }))
        .opacity(if enabled { 1. } else { 0.45 })
        .child(theme::surface(
            rgb(if primary {
                palette.accent
            } else {
                palette.panel
            }),
            (!primary).then_some(rgb(palette.border)),
            theme::CONTROL_EXTENT,
        ))
        .child(label.to_owned())
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.opacity(0.85))
                .active(|style| style.opacity(0.65))
                .when(
                    !cfg!(any(target_os = "android", target_os = "ios")),
                    |button| {
                        button
                            .focusable()
                            .tab_index(0)
                            .focus(|style| style.opacity(0.75))
                    },
                )
        })
}

pub fn badge(label: &str, color: u32, background: u32) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_size(px(12.))
        .text_color(rgb(color))
        .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
        .child(label.to_owned())
}

pub fn heading(title: &str, subtitle: &str, palette: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(palette.muted))
                .child(subtitle.to_owned()),
        )
}

pub fn metric(label: &str, value: String, description: &str, palette: Palette) -> Div {
    card(palette)
        .gap_2()
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(palette.muted))
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_size(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.muted))
                .child(description.to_owned()),
        )
}

pub fn monitor(palette: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .w(px(54.))
                .h(px(34.))
                .rounded_md()
                .border_2()
                .border_color(rgb(palette.muted))
                .p_1()
                .child(div().size_full().rounded_sm().bg(rgb(palette.accent_soft))),
        )
        .child(div().w(px(2.)).h(px(6.)).bg(rgb(palette.muted)))
        .child(
            div()
                .w(px(22.))
                .h(px(2.))
                .rounded_full()
                .bg(rgb(palette.muted)),
        )
}
