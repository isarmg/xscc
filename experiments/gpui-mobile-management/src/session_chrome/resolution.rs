use super::{ChromeAction, MediaMenu, SessionChrome};
use crate::text_field::TextField;
use gpui::{
    AppContext, Context, Div, Entity, EntityInputHandler, Focusable, KeyDownEvent, Window, div,
    prelude::*, px, rgb,
};

pub(super) struct ResolutionEditor {
    width: Entity<TextField>,
    height: Entity<TextField>,
    custom: bool,
    error: Option<String>,
}

impl ResolutionEditor {
    pub(super) fn new(cx: &mut Context<SessionChrome>) -> Self {
        Self {
            width: cx.new(|cx| TextField::new("宽度（像素）", false, cx)),
            height: cx.new(|cx| TextField::new("高度（像素）", false, cx)),
            custom: false,
            error: None,
        }
    }
}

fn resolution_mode(
    current: Option<(i32, i32, u32)>,
    supported: &[(i32, i32, u32)],
    width: i32,
    height: i32,
) -> Result<(i32, i32, u32), String> {
    let (_, _, refresh_rate) = current.ok_or("正在读取主机分辨率。")?;
    let mode = (width, height, refresh_rate);
    if supported.contains(&mode) {
        Ok(mode)
    } else if refresh_rate == 0 {
        Err(format!(
            "主机未上报支持 {width}×{height} 的当前刷新率模式。"
        ))
    } else {
        Err(format!("主机不支持 {width}×{height} @{refresh_rate} Hz。"))
    }
}

impl SessionChrome {
    pub(super) fn resolution_enabled(&self) -> bool {
        self.model.connected
            && self.model.pending_monitor.is_none()
            && self.model.can_change_resolution
            && self.model.current_resolution.is_some()
    }

    fn apply_resolution(&mut self, width: i32, height: i32, cx: &mut Context<Self>) {
        if !self.resolution_enabled() {
            return;
        }
        match resolution_mode(
            self.model.current_resolution,
            &self.model.resolutions,
            width,
            height,
        ) {
            Ok(mode) => {
                self.resolution.error = None;
                self.media_menu = None;
                if self.model.current_resolution != Some(mode) {
                    cx.emit(ChromeAction::Resolution(mode.0, mode.1, mode.2));
                }
            }
            Err(error) => self.resolution.error = Some(error),
        }
        cx.notify();
    }

    fn begin_custom_resolution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.resolution_enabled() {
            return;
        }
        if !self.resolution.custom
            && let Some((width, height, _)) = self.model.current_resolution
        {
            for (input, value) in [
                (&self.resolution.width, width),
                (&self.resolution.height, height),
            ] {
                input.update(cx, |field, cx| {
                    let end = field.value().encode_utf16().count();
                    field.replace_text_in_range(Some(0..end), &value.to_string(), window, cx);
                });
            }
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

    fn apply_custom_resolution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.resolution_enabled()
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
                self.apply_resolution(width, height, cx);
                if self.resolution.error.is_none() {
                    window.dismiss_virtual_keyboard();
                }
            }
            _ => {
                self.resolution.error = Some("宽度和高度必须是正整数。".into());
                cx.notify();
            }
        }
    }

    pub(super) fn resolution_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.media_menu != Some(MediaMenu::Resolution) {
            return;
        }
        let key = &event.keystroke;
        if key.key == "enter"
            && !event.is_held
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
            self.apply_custom_resolution(window, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn resolution_controls(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let enabled = self.resolution_enabled();
        let mut body = div().flex().flex_col().gap_1();
        for (index, (label, width, height)) in [
            ("1K · 1920×1080", 1920, 1080),
            ("2K · 2560×1440", 2560, 1440),
            ("4K · 3840×2160", 3840, 2160),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = !self.resolution.custom
                && self
                    .model
                    .current_resolution
                    .is_some_and(|mode| (mode.0, mode.1) == (width, height));
            body = body.child(
                div()
                    .id(format!("session-resolution-option-{index}"))
                    .min_h(px(48.))
                    .px_3()
                    .rounded(px(14.))
                    .flex()
                    .items_center()
                    .bg(rgb(if selected {
                        super::ACCENT
                    } else {
                        super::RAISED
                    }))
                    .text_color(rgb(if selected {
                        crate::theme::LIGHT.on_accent
                    } else {
                        super::TEXT
                    }))
                    .opacity(if enabled { 1. } else { 0.42 })
                    .child(label)
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.resolution.custom = false;
                                view.apply_resolution(width, height, cx);
                                cx.stop_propagation();
                            }))
                    }),
            );
        }
        let controller = window.display(cx).map(|display| {
            let size = display.bounds().size;
            let scale = window.scale_factor();
            (
                (f32::from(size.width) * scale).round() as i32,
                (f32::from(size.height) * scale).round() as i32,
            )
        });
        body = body
            .child(
                div()
                    .id("session-resolution-option-controller")
                    .min_h(px(48.))
                    .px_3()
                    .rounded(px(14.))
                    .bg(rgb(super::RAISED))
                    .flex()
                    .items_center()
                    .opacity(if enabled && controller.is_some() {
                        1.
                    } else {
                        0.42
                    })
                    .child("控制端分辨率")
                    .when(enabled && controller.is_some(), |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if let Some((width, height)) = controller {
                                    view.resolution.custom = false;
                                    view.apply_resolution(width, height, cx);
                                }
                                cx.stop_propagation();
                            }))
                    }),
            )
            .child(
                div()
                    .id("session-resolution-option-custom")
                    .min_h(px(48.))
                    .px_3()
                    .rounded(px(14.))
                    .bg(rgb(super::RAISED))
                    .flex()
                    .items_center()
                    .opacity(if enabled { 1. } else { 0.42 })
                    .child("自定义分辨率")
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.begin_custom_resolution(window, cx);
                                cx.stop_propagation();
                            }))
                    }),
            );
        if self.resolution.custom {
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
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
                        ),
                )
                .child(
                    div()
                        .id("session-resolution-apply")
                        .min_h(px(48.))
                        .rounded(px(14.))
                        .bg(rgb(super::ACCENT))
                        .text_color(rgb(crate::theme::LIGHT.on_accent))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("应用")
                        .when(enabled, |button| {
                            button
                                .cursor_pointer()
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.apply_custom_resolution(window, cx);
                                    cx.stop_propagation();
                                }))
                        }),
                );
        }
        body.when_some(self.resolution.error.clone(), |body, error| {
            body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(crate::theme::LIGHT.danger))
                    .child(error),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::resolution_mode;

    #[test]
    fn resolution_selection_preserves_current_refresh_and_rejects_unsupported_modes() {
        let modes = [(1920, 1080, 60), (2560, 1440, 60), (3840, 2160, 30)];
        assert_eq!(
            resolution_mode(Some((1920, 1080, 60)), &modes, 2560, 1440).unwrap(),
            (2560, 1440, 60)
        );
        assert!(resolution_mode(Some((1920, 1080, 60)), &modes, 3840, 2160).is_err());
        assert!(resolution_mode(Some((1920, 1080, 60)), &modes, 2222, 1111).is_err());
        assert!(resolution_mode(None, &modes, 1920, 1080).is_err());
    }
}
