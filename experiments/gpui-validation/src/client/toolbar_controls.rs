use super::ManagementPanel;
use crate::{dashboard as ui, remote_video::MediaRequest, theme};
use gpui::{
    Anchor, AnchoredPositionMode, Bounds, Context, Div, Focusable, KeyDownEvent, Pixels, Window,
    anchored, deferred, div, point, prelude::*, px, rgb,
};
use librustdesk::gpui_session::ResolutionMode;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolbarMenu {
    Display,
    Resolution,
    RefreshRate,
    Bitrate,
    VirtualDisplays,
}

impl ToolbarMenu {
    fn toggle(self, current: Option<Self>) -> Option<Self> {
        (current != Some(self)).then_some(self)
    }
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
        let trigger_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let measured_trigger_bounds = trigger_bounds.clone();
        let (id, label, width, enabled) = match menu {
            ToolbarMenu::Display => (
                "session-display-dropdown",
                self.video_state
                    .monitors
                    .iter()
                    .find(|monitor| monitor.index == self.video_state.current_monitor)
                    .map_or_else(
                        || "显示器  ▾".into(),
                        |monitor| {
                            format!(
                                "显示器 · 屏幕 {} · {}×{}  ▾",
                                monitor.index + 1,
                                monitor.width,
                                monitor.height
                            )
                        },
                    ),
                360_f32,
                self.display_switch_enabled(),
            ),
            ToolbarMenu::Resolution => (
                "session-resolution-dropdown",
                "分辨率  ▾".to_owned(),
                360_f32,
                self.resolution_edit_enabled(),
            ),
            ToolbarMenu::RefreshRate => (
                "session-refresh-rate-dropdown",
                self.video_state
                    .media_controls
                    .current_resolution
                    .filter(|mode| mode.refresh_rate > 0)
                    .map_or_else(
                        || "刷新率 · 未知  ▾".into(),
                        |mode| format!("刷新率 · {} Hz  ▾", mode.refresh_rate),
                    ),
                168_f32,
                self.resolution_edit_enabled()
                    && !refresh_modes(
                        self.video_state.media_controls.current_resolution,
                        &self.video_state.media_controls.resolutions,
                    )
                    .is_empty(),
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
                if fullscreen {
                    2. * theme::CONTROL_HEIGHT + 72.
                } else {
                    128.
                },
                if self.host {
                    self.initialized
                } else {
                    self.video_state.connected
                },
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
            .when(menu == ToolbarMenu::VirtualDisplays, |button| {
                button.w(px(width))
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                let enabled = match menu {
                    ToolbarMenu::Display => view.display_switch_enabled(),
                    ToolbarMenu::Resolution => view.resolution_edit_enabled(),
                    ToolbarMenu::VirtualDisplays if view.host => view.initialized,
                    ToolbarMenu::RefreshRate => {
                        view.resolution_edit_enabled()
                            && !refresh_modes(
                                view.video_state.media_controls.current_resolution,
                                &view.video_state.media_controls.resolutions,
                            )
                            .is_empty()
                    }
                    ToolbarMenu::Bitrate | ToolbarMenu::VirtualDisplays => {
                        view.video_state.connected
                    }
                };
                if enabled {
                    view.toolbar_menu = menu.toggle(view.toolbar_menu);
                    if view.toolbar_menu == Some(ToolbarMenu::VirtualDisplays) {
                        view.query_virtual_displays(cx);
                    }
                    cx.notify();
                }
                cx.stop_propagation();
            }));
        div()
            .relative()
            .flex_shrink_0()
            .on_key_down(cx.listener(Self::toolbar_menu_key_down))
            .on_children_prepainted(move |bounds, _, _| {
                measured_trigger_bounds.set(bounds.first().copied());
            })
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
                                        .on_mouse_down_out(cx.listener(
                                            move |view, event, _, cx| {
                                                // Let the trigger's click toggle its own open menu.
                                                if trigger_bounds.get().is_some_and(|bounds| {
                                                    bounds.contains(&event.position)
                                                }) {
                                                    return;
                                                }
                                                if view.toolbar_menu == Some(menu) {
                                                    view.toolbar_menu = None;
                                                    cx.notify();
                                                }
                                            },
                                        ))
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

    fn display_switch_enabled(&self) -> bool {
        let state = &self.video_state;
        state.connected
            && !state.switching_monitor
            && state.pending_media.is_none()
            && !state.virtual_display_pending
            && state.monitors.iter().any(|monitor| monitor.available)
    }

    pub(super) fn render_display_dropdown(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = theme::palette(window.appearance());
        let state = &self.video_state;
        let content = div()
            .id("session-display-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(320.))
            .overflow_y_scroll()
            .children(state.monitors.iter().map(|monitor| {
                let index = monitor.index;
                ui::button(
                    ("session-monitor", index),
                    &format!(
                        "屏幕 {} · {} · {}×{}{}",
                        index + 1,
                        monitor.name,
                        monitor.width,
                        monitor.height,
                        if monitor.available {
                            ""
                        } else {
                            " · 不可用"
                        }
                    ),
                    state.current_monitor == index,
                    self.display_switch_enabled() && monitor.available,
                    palette,
                )
                .w_full()
                .min_h(px(32.))
                .h(px(32.))
                .py_0()
                .justify_start()
                .truncate()
                .on_click(cx.listener(move |view, _, window, cx| {
                    if view.display_switch_enabled()
                        && view.video_state.current_monitor != index
                        && view
                            .video_state
                            .monitors
                            .iter()
                            .any(|monitor| monitor.index == index && monitor.available)
                    {
                        view.video
                            .update(cx, |video, cx| video.switch_monitor(index, cx));
                    }
                    view.toolbar_menu = None;
                    if view.video_state.connected {
                        view.video.read(cx).focus_handle(cx).focus(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }))
            }));
        self.render_toolbar_dropdown(ToolbarMenu::Display, div().child(content), window, cx)
    }

    pub(super) fn render_fps_dropdown(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = theme::palette(window.appearance());
        let controls = &self.video_state.media_controls;
        let content = div().flex().flex_col().gap_1().children(
            refresh_modes(controls.current_resolution, &controls.resolutions)
                .into_iter()
                .enumerate()
                .map(|(index, mode)| {
                    ui::button(
                        ("session-refresh-rate-option", index),
                        &format!("{} Hz", mode.refresh_rate),
                        controls.current_resolution == Some(mode),
                        self.resolution_edit_enabled(),
                        palette,
                    )
                    .w_full()
                    .min_h(px(32.))
                    .h(px(32.))
                    .py_0()
                    .justify_start()
                    .on_click(cx.listener(move |view, _, window, cx| {
                        let controls = &view.video_state.media_controls;
                        if view.resolution_edit_enabled()
                            && controls.current_resolution != Some(mode)
                            && refresh_modes(controls.current_resolution, &controls.resolutions)
                                .contains(&mode)
                        {
                            view.video.update(cx, |video, cx| {
                                video.request_media(MediaRequest::Resolution(mode), cx)
                            });
                        }
                        view.toolbar_menu = None;
                        if view.video_state.connected {
                            view.video.read(cx).focus_handle(cx).focus(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                }),
        );
        self.render_toolbar_dropdown(ToolbarMenu::RefreshRate, content, window, cx)
    }
}

fn refresh_modes(
    current: Option<ResolutionMode>,
    supported: &[ResolutionMode],
) -> Vec<ResolutionMode> {
    let Some(current) = current.filter(|mode| mode.refresh_rate > 0) else {
        return Vec::new();
    };
    let mut modes: Vec<_> = supported
        .iter()
        .copied()
        .filter(|mode| {
            mode.width == current.width && mode.height == current.height && mode.refresh_rate > 0
        })
        .collect();
    modes.sort_unstable_by_key(|mode| mode.refresh_rate);
    modes.dedup();
    modes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_toolbar_menu_toggles_open_closed_open() {
        for menu in [
            ToolbarMenu::Display,
            ToolbarMenu::Resolution,
            ToolbarMenu::RefreshRate,
            ToolbarMenu::Bitrate,
            ToolbarMenu::VirtualDisplays,
        ] {
            let open = menu.toggle(None);
            assert!(open == Some(menu));
            let closed = menu.toggle(open);
            assert!(closed.is_none());
            assert!(menu.toggle(closed) == Some(menu));
        }
    }

    #[test]
    fn activating_a_different_menu_switches_the_open_menu() {
        let mut open = None;
        for menu in [
            ToolbarMenu::Display,
            ToolbarMenu::Resolution,
            ToolbarMenu::RefreshRate,
            ToolbarMenu::Bitrate,
            ToolbarMenu::VirtualDisplays,
            ToolbarMenu::Display,
        ] {
            open = menu.toggle(open);
            assert!(open == Some(menu));
        }
    }

    #[test]
    fn refresh_menu_uses_only_reported_hz_for_the_current_size_and_requires_known_current_hz() {
        let current = ResolutionMode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        };
        let modes = [
            ResolutionMode {
                refresh_rate: 240,
                ..current
            },
            ResolutionMode {
                width: 3840,
                height: 2160,
                refresh_rate: 144,
            },
            current,
            ResolutionMode {
                refresh_rate: 75,
                ..current
            },
            ResolutionMode {
                refresh_rate: 0,
                ..current
            },
            current,
        ];
        assert_eq!(
            refresh_modes(Some(current), &modes)
                .iter()
                .map(|mode| mode.refresh_rate)
                .collect::<Vec<_>>(),
            [60, 75, 240]
        );
        assert!(refresh_modes(None, &modes).is_empty());
        assert!(
            refresh_modes(
                Some(ResolutionMode {
                    refresh_rate: 0,
                    ..current
                }),
                &modes
            )
            .is_empty()
        );
    }
}
