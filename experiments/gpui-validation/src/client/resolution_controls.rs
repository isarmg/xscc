use super::ManagementPanel;
use crate::{dashboard as ui, remote_video::MediaRequest, text_field::TextField, theme};
use gpui::{
    App, Context, Entity, EntityInputHandler, Focusable, KeyDownEvent, Window, div, prelude::*, px,
    rgb,
};
use librustdesk::gpui_session::ResolutionMode;

pub(super) struct ResolutionEditor {
    width: Entity<TextField>,
    height: Entity<TextField>,
    custom: bool,
    error: Option<String>,
}

impl ResolutionEditor {
    pub(super) fn new(cx: &mut Context<ManagementPanel>) -> Self {
        Self {
            width: cx.new(|cx| TextField::new("宽度（像素）", false, cx)),
            height: cx.new(|cx| TextField::new("高度（像素）", false, cx)),
            custom: false,
            error: None,
        }
    }
}

fn controller_resolution(window: &Window, cx: &App) -> Option<(i32, i32)> {
    window.display(cx).map(|display| {
        let size = display.bounds().size;
        let scale = window.scale_factor();
        (
            (f32::from(size.width) * scale).round() as i32,
            (f32::from(size.height) * scale).round() as i32,
        )
    })
}

impl ManagementPanel {
    fn resolution_edit_enabled(&self) -> bool {
        let state = &self.video_state;
        state.connected
            && !state.switching_monitor
            && state.pending_media.is_none()
            && state.media_controls.can_change_resolution
            && state.media_controls.current_resolution.is_some()
    }

    fn apply_resolution_dimensions(&mut self, width: i32, height: i32, cx: &mut Context<Self>) {
        if !self.resolution_edit_enabled() {
            return;
        }
        let Some(current) = self.video_state.media_controls.current_resolution else {
            return;
        };
        let mode = ResolutionMode {
            width,
            height,
            refresh_rate: current.refresh_rate,
        };
        if !self.video_state.media_controls.resolutions.contains(&mode) {
            self.resolution.error = Some(if mode.refresh_rate == 0 {
                format!("主机未上报支持 {width}×{height} 的当前刷新率模式。")
            } else {
                format!(
                    "主机不支持 {width}×{height} @{} Hz，请先选择兼容的刷新率。",
                    mode.refresh_rate
                )
            });
        } else {
            self.resolution.error = None;
            if current != mode {
                self.video.update(cx, |video, cx| {
                    video.request_media(MediaRequest::Resolution(mode), cx)
                });
            }
        }
        cx.notify();
    }

    fn begin_custom_resolution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.resolution.custom || !self.resolution_edit_enabled() {
            return;
        }
        let Some(current) = self.video_state.media_controls.current_resolution else {
            return;
        };
        for (input, value) in [
            (&self.resolution.width, current.width),
            (&self.resolution.height, current.height),
        ] {
            input.update(cx, |field, cx| {
                let end = field.value().encode_utf16().count();
                field.replace_text_in_range(Some(0..end), &value.to_string(), window, cx);
            });
        }
        self.resolution.custom = true;
        self.resolution.error = None;
        self.resolution
            .width
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    fn apply_custom_resolution(&mut self, cx: &mut Context<Self>) {
        if !self.resolution_edit_enabled()
            || self.resolution.width.read(cx).is_composing()
            || self.resolution.height.read(cx).is_composing()
        {
            return;
        }
        let width = self.resolution.width.read(cx).value().trim().parse::<i32>();
        let height = self
            .resolution
            .height
            .read(cx)
            .value()
            .trim()
            .parse::<i32>();
        match (width, height) {
            (Ok(width), Ok(height)) if width > 0 && height > 0 => {
                self.apply_resolution_dimensions(width, height, cx)
            }
            _ => {
                self.resolution.error = Some("宽度和高度必须是正整数。".into());
                cx.notify();
            }
        }
    }

    fn resolution_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.resolution.width.read(cx).is_composing()
            || self.resolution.height.read(cx).is_composing()
        {
            return;
        }
        let key = &event.keystroke;
        if key.key == "enter"
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.platform
            && (self
                .resolution
                .width
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
                || self
                    .resolution
                    .height
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window))
        {
            self.apply_custom_resolution(cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn render_resolution_controls(
        &self,
        window: &Window,
        palette: theme::Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let controls = &self.video_state.media_controls;
        let current = controls.current_resolution;
        let enabled = self.resolution_edit_enabled();
        let local_resolution = controller_resolution(window, cx);
        let mut refresh_rates = controls
            .resolutions
            .iter()
            .filter(|mode| {
                current.is_some_and(|current| {
                    current.width == mode.width && current.height == mode.height
                })
            })
            .map(|mode| mode.refresh_rate)
            .filter(|&rate| rate != 0)
            .collect::<Vec<_>>();
        refresh_rates.sort_unstable();
        refresh_rates.dedup();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("分辨率"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(
                                current
                                    .map(|mode| format!("当前 {}×{}", mode.width, mode.height))
                                    .unwrap_or("等待主机上报当前分辨率…".into()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(
                                [("1K", 1920, 1080), ("2K", 2560, 1440), ("4K", 3840, 2160)]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, (label, width, height))| {
                                        let selected = !self.resolution.custom
                                            && current.is_some_and(|mode| {
                                                mode.width == width && mode.height == height
                                            });
                                        ui::button(
                                            ("session-resolution-preset", index),
                                            label,
                                            selected,
                                            enabled && !selected,
                                            palette,
                                        )
                                        .on_click(
                                            cx.listener(move |view, _, _, cx| {
                                                if view.resolution_edit_enabled() {
                                                    view.resolution.custom = false;
                                                    view.apply_resolution_dimensions(
                                                        width, height, cx,
                                                    );
                                                }
                                            }),
                                        )
                                    }),
                            )
                            .child(
                                ui::button(
                                    "session-resolution-controller",
                                    "控制端分辨率",
                                    !self.resolution.custom
                                        && current.zip(local_resolution).is_some_and(
                                            |(mode, (width, height))| {
                                                mode.width == width && mode.height == height
                                            },
                                        ),
                                    enabled && local_resolution.is_some(),
                                    palette,
                                )
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        if view.resolution_edit_enabled()
                                            && let Some((width, height)) =
                                                controller_resolution(window, cx)
                                        {
                                            view.resolution.custom = false;
                                            view.apply_resolution_dimensions(width, height, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                ui::button(
                                    "session-resolution-custom",
                                    "自定义分辨率",
                                    self.resolution.custom,
                                    enabled && !self.resolution.custom,
                                    palette,
                                )
                                .on_click(cx.listener(
                                    |view, _, window, cx| view.begin_custom_resolution(window, cx),
                                )),
                            ),
                    )
                    .when(self.resolution.custom, |section| {
                        section.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .on_key_down(cx.listener(Self::resolution_key_down))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.resolution.width.clone()),
                                )
                                .child("×")
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.resolution.height.clone()),
                                )
                                .child(
                                    ui::button(
                                        "session-resolution-apply",
                                        "应用",
                                        false,
                                        enabled,
                                        palette,
                                    )
                                    .flex_shrink_0()
                                    .on_click(cx.listener(
                                        |view, _, _, cx| view.apply_custom_resolution(cx),
                                    )),
                                ),
                        )
                    })
                    .when_some(self.resolution.error.clone(), |section, error| {
                        section.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.danger))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(if enabled {
                                "分辨率保留当前刷新率，仅应用主机支持的模式。"
                            } else {
                                "主机未授权显示调节、没有可用模式，或正在处理其他调节。"
                            }),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(palette.border)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("刷新率"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(
                                current
                                    .map(|mode| {
                                        if mode.refresh_rate == 0 {
                                            "当前刷新率未上报".into()
                                        } else {
                                            format!("当前 {} Hz", mode.refresh_rate)
                                        }
                                    })
                                    .unwrap_or("等待主机上报刷新率…".into()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(refresh_rates.iter().map(|&rate| {
                                let selected =
                                    current.is_some_and(|mode| mode.refresh_rate == rate);
                                ui::button(
                                    ("session-refresh-rate", rate as usize),
                                    &format!("{rate} Hz"),
                                    selected,
                                    enabled && !selected,
                                    palette,
                                )
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        if view.resolution_edit_enabled()
                                            && let Some(current) =
                                                view.video_state.media_controls.current_resolution
                                            && current.refresh_rate != rate
                                        {
                                            view.resolution.error = None;
                                            view.video.update(cx, |video, cx| {
                                                video.request_media(
                                                    MediaRequest::Resolution(ResolutionMode {
                                                        refresh_rate: rate,
                                                        ..current
                                                    }),
                                                    cx,
                                                )
                                            });
                                            cx.notify();
                                        }
                                    },
                                ))
                            })),
                    )
                    .when(refresh_rates.is_empty(), |section| {
                        section.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.muted))
                                .child("主机未上报当前分辨率可调节的刷新率。"),
                        )
                    }),
            )
    }
}
