use super::{ManagementPanel, toolbar_controls::ToolbarMenu};
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
    pub(super) fn resolution_edit_enabled(&self) -> bool {
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
            let enabled = self.resolution_edit_enabled();
            self.apply_custom_resolution(cx);
            if enabled
                && self.resolution.error.is_none()
                && self.toolbar_menu == Some(ToolbarMenu::Resolution)
            {
                self.toolbar_menu = None;
                self.video.read(cx).focus_handle(cx).focus(window, cx);
                cx.notify();
            }
            cx.stop_propagation();
        }
    }
}

impl ManagementPanel {
    pub(super) fn render_resolution_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = theme::palette(window.appearance());
        let enabled = self.resolution_edit_enabled();
        let current = self.video_state.media_controls.current_resolution;
        let content = div()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                [
                    ("1K · 1920×1080", 1920, 1080),
                    ("2K · 2560×1440", 2560, 1440),
                    ("4K · 3840×2160", 3840, 2160),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (label, width, height))| {
                    ui::button(
                        ("session-resolution-option", index),
                        label,
                        !self.resolution.custom
                            && current
                                .is_some_and(|mode| mode.width == width && mode.height == height),
                        enabled,
                        palette,
                    )
                    .w_full()
                    .min_h(px(32.))
                    .h(px(32.))
                    .py_0()
                    .justify_start()
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if view.resolution_edit_enabled() {
                            view.resolution.custom = false;
                            view.apply_resolution_dimensions(width, height, cx);
                            if view.resolution.error.is_none() {
                                view.toolbar_menu = None;
                                view.video.read(cx).focus_handle(cx).focus(window, cx);
                            }
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                }),
            )
            .child(
                ui::button(
                    "session-resolution-option-controller",
                    "控制端分辨率",
                    false,
                    enabled && controller_resolution(window, cx).is_some(),
                    palette,
                )
                .w_full()
                .min_h(px(32.))
                .h(px(32.))
                .py_0()
                .justify_start()
                .on_click(cx.listener(|view, _, window, cx| {
                    if view.resolution_edit_enabled()
                        && let Some((width, height)) = controller_resolution(window, cx)
                    {
                        view.resolution.custom = false;
                        view.apply_resolution_dimensions(width, height, cx);
                        if view.resolution.error.is_none() {
                            view.toolbar_menu = None;
                            view.video.read(cx).focus_handle(cx).focus(window, cx);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                })),
            )
            .child(
                ui::button(
                    "session-resolution-option-custom",
                    "自定义分辨率",
                    self.resolution.custom,
                    enabled,
                    palette,
                )
                .w_full()
                .min_h(px(32.))
                .h(px(32.))
                .py_0()
                .justify_start()
                .on_click(cx.listener(|view, _, window, cx| {
                    if view.resolution_edit_enabled() {
                        view.begin_custom_resolution(window, cx);
                        view.resolution
                            .width
                            .read(cx)
                            .focus_handle(cx)
                            .focus(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                })),
            )
            .when(self.resolution.custom, |menu| {
                menu.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_2()
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
                                "session-resolution-menu-apply",
                                "应用",
                                false,
                                enabled,
                                palette,
                            )
                            .flex_shrink_0()
                            .on_click(cx.listener(
                                |view, _, window, cx| {
                                    if view.resolution_edit_enabled()
                                        && !view.resolution.width.read(cx).is_composing()
                                        && !view.resolution.height.read(cx).is_composing()
                                    {
                                        view.apply_custom_resolution(cx);
                                        if view.resolution.error.is_none() {
                                            view.toolbar_menu = None;
                                            view.video.read(cx).focus_handle(cx).focus(window, cx);
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                },
                            )),
                        ),
                )
            })
            .when_some(self.resolution.error.clone(), |menu, error| {
                menu.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.danger))
                        .child(error),
                )
            });
        self.render_toolbar_dropdown(ToolbarMenu::Resolution, content, window, cx)
    }
}

impl ManagementPanel {
    pub(super) fn render_refresh_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = theme::palette(window.appearance());
        let controls = &self.video_state.media_controls;
        let current = controls.current_resolution;
        let enabled = self.resolution_edit_enabled();
        let mut rates = controls
            .resolutions
            .iter()
            .filter(|mode| {
                current.is_some_and(|current| {
                    mode.width == current.width && mode.height == current.height
                })
            })
            .map(|mode| mode.refresh_rate)
            .filter(|&rate| rate != 0)
            .collect::<Vec<_>>();
        rates.sort_unstable();
        rates.dedup();
        let content = div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rates.iter().map(|&rate| {
                ui::button(
                    ("session-refresh-option", rate as usize),
                    &format!("{rate} Hz"),
                    current.is_some_and(|mode| mode.refresh_rate == rate),
                    enabled,
                    palette,
                )
                .w_full()
                .min_h(px(32.))
                .h(px(32.))
                .py_0()
                .justify_start()
                .on_click(cx.listener(move |view, _, window, cx| {
                    if view.resolution_edit_enabled()
                        && let Some(current) = view.video_state.media_controls.current_resolution
                    {
                        if current.refresh_rate != rate {
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
                        }
                        view.toolbar_menu = None;
                        view.video.read(cx).focus_handle(cx).focus(window, cx);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
            }))
            .when(rates.is_empty(), |menu| {
                menu.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.muted))
                        .child("主机未上报当前分辨率可调节的刷新率。"),
                )
            });
        self.render_toolbar_dropdown(ToolbarMenu::RefreshRate, content, window, cx)
    }
}
