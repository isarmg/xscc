#[path = "../../../shared/bitrate_controls.rs"]
mod bitrate;

use super::ManagementPanel;
use crate::{
    dashboard as ui,
    remote_video::{MediaRequest, VideoState},
    text_field::TextField,
    theme,
};
use bitrate::{BitrateRange, format_mbps};
use gpui::{
    Bounds, Context, Entity, EntityInputHandler, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, canvas, div, prelude::*,
    px, relative, rgb,
};
use librustdesk::gpui_session::BitrateMode;
use std::{cell::Cell, rc::Rc};

pub(super) struct BitrateEditor {
    input: Entity<TextField>,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    preview: Option<u32>,
    dragging: bool,
    error: Option<String>,
}

impl BitrateEditor {
    pub(super) fn new(cx: &mut Context<ManagementPanel>) -> Self {
        Self {
            input: cx.new(|cx| TextField::new("输入 Mbps，例如 37.125", false, cx)),
            focus: cx.focus_handle(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            preview: None,
            dragging: false,
            error: None,
        }
    }
}

impl ManagementPanel {
    fn bitrate_range(&self) -> Option<BitrateRange> {
        BitrateRange::for_host(self.video_state.media_controls.max_bitrate_kbps)
    }

    fn bitrate_edit_enabled(&self) -> bool {
        let state = &self.video_state;
        state.connected
            && state.media_controls.bitrate_supported
            && !state.switching_monitor
            && state.pending_media.is_none()
            && self.bitrate_range().is_some()
    }

    fn bitrate_preview(&self) -> Option<u32> {
        let range = self.bitrate_range()?;
        let confirmed = match self.video_state.media_controls.bitrate_mode {
            BitrateMode::Auto => 8000,
            BitrateMode::LimitKbps(kbps) => kbps,
        };
        Some(
            self.bitrate
                .preview
                .unwrap_or(confirmed)
                .clamp(range.min_kbps(), range.max_kbps()),
        )
    }

    fn set_bitrate_preview(&mut self, kbps: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.bitrate.preview = Some(kbps);
        self.bitrate.error = None;
        let text = format_mbps(kbps);
        self.bitrate.input.update(cx, |field, cx| {
            let end = field.value().encode_utf16().count();
            field.replace_text_in_range(Some(0..end), &text, window, cx);
        });
        cx.notify();
    }

    pub(super) fn sync_bitrate_editor(
        &mut self,
        state: &VideoState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !state.connected
            || !state.media_controls.bitrate_supported
            || state.switching_monitor
            || state.pending_media.is_some()
        {
            self.bitrate.dragging = false;
        }
        let previous = &self.video_state;
        let changed = previous.connected != state.connected
            || previous.media_controls.bitrate_mode != state.media_controls.bitrate_mode
            || previous.media_controls.max_bitrate_kbps != state.media_controls.max_bitrate_kbps;
        if !changed
            || self.bitrate.dragging
            || self
                .bitrate
                .input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            return;
        }
        self.bitrate.preview = None;
        self.bitrate.error = None;
        if let Some(range) = BitrateRange::for_host(state.media_controls.max_bitrate_kbps) {
            let kbps = match state.media_controls.bitrate_mode {
                BitrateMode::Auto => 8000,
                BitrateMode::LimitKbps(kbps) => kbps,
            }
            .clamp(range.min_kbps(), range.max_kbps());
            self.set_bitrate_preview(kbps, window, cx);
        }
    }

    fn preview_bitrate_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.bitrate_edit_enabled() {
            self.bitrate.dragging = false;
            return;
        }
        let Some(range) = self.bitrate_range() else {
            return;
        };
        let bounds = self.bitrate.bounds.get();
        if bounds.size.width <= px(0.) {
            return;
        }
        let fraction = f64::from((position.x - bounds.left()) / bounds.size.width);
        self.set_bitrate_preview(range.at_fraction(fraction), window, cx);
    }

    fn finish_bitrate_drag(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !std::mem::take(&mut self.bitrate.dragging) {
            return;
        }
        self.preview_bitrate_at(event.position, window, cx);
        self.apply_bitrate_input(window, cx);
        cx.stop_propagation();
    }

    fn apply_bitrate_input(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.bitrate_edit_enabled() || self.bitrate.input.read(cx).is_composing() {
            return;
        }
        let Some(range) = self.bitrate_range() else {
            return;
        };
        match range.parse_mbps(self.bitrate.input.read(cx).value()) {
            Ok(kbps) => {
                self.bitrate.preview = Some(kbps);
                self.bitrate.error = None;
                let mode = BitrateMode::LimitKbps(kbps);
                if self.video_state.media_controls.bitrate_mode != mode {
                    self.video.update(cx, |video, cx| {
                        video.request_media(MediaRequest::Bitrate(mode), cx)
                    });
                }
            }
            Err(error) => self.bitrate.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn bitrate_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.bitrate.input.read(cx);
        let field_focused = input.focus_handle(cx).is_focused(window);
        if input.is_composing() {
            return;
        }
        let slider_focused = self.bitrate.focus.is_focused(window);
        let key = &event.keystroke;
        if (!field_focused && !slider_focused)
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.platform
        {
            return;
        }
        if key.key == "enter" || (slider_focused && key.key == "space") {
            self.apply_bitrate_input(window, cx);
            cx.stop_propagation();
        } else if slider_focused && self.bitrate_edit_enabled() {
            let Some(range) = self.bitrate_range() else {
                return;
            };
            let Some(kbps) = self.bitrate_preview() else {
                return;
            };
            let value = match key.key.as_str() {
                "left" | "down" => range.step(kbps, false),
                "right" | "up" => range.step(kbps, true),
                "home" => range.min_kbps(),
                "end" => range.max_kbps(),
                _ => return,
            };
            self.set_bitrate_preview(value, window, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn render_bitrate_editor(
        &self,
        palette: theme::Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let range = self.bitrate_range();
        let enabled = self.bitrate_edit_enabled();
        let current = self.video_state.media_controls.bitrate_mode;
        let fraction = range
            .zip(self.bitrate_preview())
            .map(|(range, kbps)| range.fraction(kbps))
            .unwrap_or(0.);
        let allowed = range
            .map(|range| range.fraction(range.max_kbps()))
            .unwrap_or(0.);
        let bounds = self.bitrate.bounds.clone();
        div().flex().flex_col().gap_2()
            .on_key_down(cx.listener(Self::bitrate_key_down))
            .child(div().text_size(px(12.)).text_color(rgb(palette.muted)).child(
                self.bitrate_preview().map(|kbps| format!("预览上限 {} Mbps", format_mbps(kbps))).unwrap_or("等待主机能力…".into())))
            .child(div().id("session-bitrate-slider").track_focus(&self.bitrate.focus).tab_index(0)
                .w_full().h(px(44.)).px(px(10.)).relative()
                .opacity(if enabled {1.} else {0.45})
                .when(enabled, |track| track.cursor_pointer())
                .on_mouse_down(MouseButton::Left, cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    if view.bitrate_edit_enabled() {
                        view.bitrate.focus.focus(window, cx);
                        view.bitrate.dragging = true;
                        view.preview_bitrate_at(event.position, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                    if view.bitrate.dragging {
                        if event.pressed_button == Some(MouseButton::Left) {
                            view.preview_bitrate_at(event.position, window, cx);
                        } else { view.bitrate.dragging = false; }
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_bitrate_drag))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::finish_bitrate_drag))
                .child(div().relative().size_full()
                    .child(canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {}).absolute().size_full())
                    .child(div().absolute().top(px(19.)).h(px(6.)).w_full().rounded_full().bg(rgb(palette.border)))
                    .child(div().absolute().top(px(19.)).h(px(6.)).w(relative(allowed as f32)).rounded_full().bg(rgb(palette.muted)))
                    .child(div().absolute().top(px(19.)).h(px(6.)).w(relative(fraction as f32)).rounded_full().bg(rgb(palette.accent)))
                    .children([0.,0.25,0.5,0.75,1.].into_iter().map(|position| div().absolute().left(relative(position)).top(px(28.)).w(px(1.)).h(px(5.)).bg(rgb(palette.muted))))
                    .child(div().absolute().left(relative(fraction as f32)).ml(px(-9.)).top(px(13.)).size(px(18.)).rounded_full().bg(rgb(palette.accent)))))
            .child(div().relative().w_full().h(px(22.)).px(px(10.)).child(div().relative().size_full().children(
                [("1",0.),("10",0.25),("100",0.5),("300",0.75),("1000",1.)].into_iter().enumerate().map(|(index,(label,position))| {
                    div().absolute().left(relative(position)).w(px(44.)).ml(px(if index==0 {0.} else if index==4 {-44.} else {-22.}))
                        .when(index==0, |text| text.text_left()).when(index==4, |text| text.text_right()).when(index>0 && index<4, |text| text.text_center())
                        .text_size(px(11.)).text_color(rgb(palette.muted)).child(label)
                }))))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(self.bitrate.input.clone()))
                .child(div().text_size(px(12.)).child("Mbps")))
            .when_some(self.bitrate.error.clone(), |section, error| section.child(div().text_size(px(12.)).text_color(rgb(palette.danger)).child(error)))
            .child(div().flex().flex_wrap().gap_2()
                .child(ui::button("session-bitrate-apply", "应用上限", false, enabled, palette)
                    .on_click(cx.listener(|view, _, window, cx| view.apply_bitrate_input(window, cx))))
                .child(ui::button("session-bitrate-auto", "自动 · 解除上限", current == BitrateMode::Auto,
                    enabled && current != BitrateMode::Auto, palette)
                    .on_click(cx.listener(|view, _, _, cx| {
                        if view.bitrate_edit_enabled() && view.video_state.media_controls.bitrate_mode != BitrateMode::Auto {
                            view.bitrate.error = None;
                            view.video.update(cx, |video,cx| video.request_media(MediaRequest::Bitrate(BitrateMode::Auto), cx));
                        }
                    }))))
            .child(div().text_size(px(11.)).text_color(rgb(palette.muted)).child(
                "滑杆四段等宽，步进 1 / 10 / 30 / 100 Mbps；松手提交。输入可精确到 0.001 Mbps，按 Enter 或应用提交。"))
    }
}
