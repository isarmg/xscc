use gpui::{div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
enum WindowAction {
    Close,
    Minimize,
    Fullscreen,
}

pub(super) fn traffic_lights(fullscreen: bool) -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .children(
            [
                ("window-close", 0xff5f57, "×", WindowAction::Close),
                ("window-minimize", 0xffbd2e, "−", WindowAction::Minimize),
                (
                    "window-fullscreen",
                    0x28c840,
                    if fullscreen { "↙" } else { "↗" },
                    WindowAction::Fullscreen,
                ),
            ]
            .into_iter()
            .map(|(id, color, glyph, action)| {
                div()
                    .id(id)
                    .w(px(22.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .occlude()
                    .hover(|style| style.opacity(0.75))
                    .child(
                        div()
                            .size(px(14.))
                            .rounded_full()
                            .bg(rgb(color))
                            .text_color(rgb(0x24312b))
                            .text_size(px(10.))
                            .line_height(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph),
                    )
                    .on_click(move |_, window, cx| {
                        match action {
                            WindowAction::Close => window.remove_window(),
                            WindowAction::Minimize => window.minimize_window(),
                            WindowAction::Fullscreen => {
                                window.toggle_fullscreen();
                                window.refresh();
                            }
                        }
                        cx.stop_propagation();
                    })
            }),
        )
}
