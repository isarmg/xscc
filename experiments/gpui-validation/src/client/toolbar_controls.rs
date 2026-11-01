use super::ManagementPanel;
use crate::{dashboard as ui, theme};
use gpui::{
    Anchor, AnchoredPositionMode, Context, Div, Focusable, KeyDownEvent, Window, anchored,
    deferred, div, point, prelude::*, px, rgb,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolbarMenu {
    Resolution,
    RefreshRate,
    Bitrate,
    VirtualDisplays,
}

impl ManagementPanel {
    pub(super) fn toolbar_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" && self.toolbar_menu.is_some() {
            self.toolbar_menu = None;
            if self.video_state.connected {
                self.video.read(cx).focus_handle(cx).focus(window, cx);
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn render_toolbar_dropdown(
        &self,
        menu: ToolbarMenu,
        content: Div,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = theme::palette(window.appearance());
        let fullscreen = window.is_fullscreen();
        let open = self.toolbar_menu == Some(menu);
        let (id, label, width, enabled) = match menu {
            ToolbarMenu::Resolution => (
                "session-resolution-dropdown",
                "分辨率  ▾".to_owned(),
                360_f32,
                self.resolution_edit_enabled(),
            ),
            ToolbarMenu::RefreshRate => (
                "session-refresh-dropdown",
                "刷新率  ▾".to_owned(),
                168_f32,
                self.video_state.connected,
            ),
            ToolbarMenu::Bitrate => (
                "session-bitrate-dropdown",
                "码率  ▾".to_owned(),
                440_f32,
                self.video_state.connected,
            ),
            ToolbarMenu::VirtualDisplays => (
                "session-virtual-displays-dropdown",
                "虚拟显示器  ▾".to_owned(),
                360_f32,
                self.video_state.connected,
            ),
        };
        let trigger = ui::button(id, &label, open, enabled, palette)
            .when(!fullscreen, |button| {
                button
                    .min_h(px(28.))
                    .h(px(28.))
                    .py_0()
                    .px_2()
                    .text_size(px(12.))
                    .line_height(px(18.))
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                let enabled = match menu {
                    ToolbarMenu::Resolution => view.resolution_edit_enabled(),
                    ToolbarMenu::RefreshRate
                    | ToolbarMenu::Bitrate
                    | ToolbarMenu::VirtualDisplays => view.video_state.connected,
                };
                if enabled {
                    view.toolbar_menu = Some(menu);
                    cx.notify();
                }
                cx.stop_propagation();
            }));
        div()
            .relative()
            .flex_shrink_0()
            .on_key_down(cx.listener(Self::toolbar_menu_key_down))
            .child(trigger)
            .when(open, |wrapper| {
                wrapper.child(
                    deferred(
                        div().absolute().top_0().left_0().child(
                            anchored()
                                .anchor(Anchor::TopLeft)
                                .position_mode(AnchoredPositionMode::Local)
                                .position(point(
                                    px(0.),
                                    px(if fullscreen {
                                        theme::CONTROL_HEIGHT + 4.
                                    } else {
                                        32.
                                    }),
                                ))
                                .snap_to_window_with_margin(px(8.))
                                .child(
                                    div()
                                        .id((id, 0usize))
                                        .relative()
                                        .w(px(width.min(
                                            (f32::from(window.viewport_size().width) - 16.).max(1.),
                                        )))
                                        .max_h(px((f32::from(window.viewport_size().height) - 16.)
                                            .max(1.)))
                                        .overflow_y_scroll()
                                        .p_2()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .whitespace_normal()
                                        .occlude()
                                        .on_key_down(cx.listener(Self::toolbar_menu_key_down))
                                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                                        .on_mouse_down_out(cx.listener(move |view, _, _, cx| {
                                            if view.toolbar_menu == Some(menu) {
                                                view.toolbar_menu = None;
                                                cx.notify();
                                            }
                                        }))
                                        .child(theme::surface(
                                            rgb(palette.panel),
                                            Some(rgb(palette.border)),
                                            theme::PANEL_EXTENT,
                                        ))
                                        .child(content),
                                ),
                        ),
                    )
                    .priority(2),
                )
            })
    }
}
