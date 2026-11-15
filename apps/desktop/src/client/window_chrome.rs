use gpui::{CursorStyle, MouseButton, ResizeEdge, div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
enum WindowAction {
    Close,
    Minimize,
    Fullscreen,
}

pub(super) fn traffic_lights(fullscreen: bool) -> gpui::Div {
    if cfg!(target_os = "macos") && !fullscreen {
        return div().w(px(74.)).h(px(28.)).flex_shrink_0();
    }
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

pub(super) fn resize_handles() -> gpui::Div {
    div().absolute().top_0().left_0().size_full().children(
        [
            ResizeEdge::Top,
            ResizeEdge::Bottom,
            ResizeEdge::Left,
            ResizeEdge::Right,
            ResizeEdge::TopLeft,
            ResizeEdge::TopRight,
            ResizeEdge::BottomLeft,
            ResizeEdge::BottomRight,
        ]
        .into_iter()
        .map(|edge| {
            let region = div().absolute();
            let (region, cursor) = match edge {
                ResizeEdge::Top => (
                    region.top_0().left(px(8.)).right(px(8.)).h(px(4.)),
                    CursorStyle::ResizeUpDown,
                ),
                ResizeEdge::Bottom => (
                    region.bottom_0().left(px(8.)).right(px(8.)).h(px(4.)),
                    CursorStyle::ResizeUpDown,
                ),
                ResizeEdge::Left => (
                    region.left_0().top(px(8.)).bottom(px(8.)).w(px(4.)),
                    CursorStyle::ResizeLeftRight,
                ),
                ResizeEdge::Right => (
                    region.right_0().top(px(8.)).bottom(px(8.)).w(px(4.)),
                    CursorStyle::ResizeLeftRight,
                ),
                ResizeEdge::TopLeft => (
                    region.top_0().left_0().size(px(8.)),
                    CursorStyle::ResizeUpLeftDownRight,
                ),
                ResizeEdge::TopRight => (
                    region.top_0().right_0().size(px(8.)),
                    CursorStyle::ResizeUpRightDownLeft,
                ),
                ResizeEdge::BottomLeft => (
                    region.bottom_0().left_0().size(px(8.)),
                    CursorStyle::ResizeUpRightDownLeft,
                ),
                ResizeEdge::BottomRight => (
                    region.bottom_0().right_0().size(px(8.)),
                    CursorStyle::ResizeUpLeftDownRight,
                ),
            };
            region.occlude().cursor(cursor).on_mouse_down(
                MouseButton::Left,
                move |_, window, cx| {
                    window.start_window_resize(edge);
                    cx.stop_propagation();
                },
            )
        }),
    )
}
