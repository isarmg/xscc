use super::{BLUE, ChromeAction, Div, MUTED, Panel, RAISED, SessionChrome, TEXT};
use crate::text_field::TextField;
use gpui::{
    AppContext, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, HitboxBehavior,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Stateful, TouchEvent, TouchId, TouchPhase, Window, canvas, div, fill, point, prelude::*,
    px, rgb, size,
};

#[path = "../../../shared/bitrate_controls.rs"]
mod controls;
use controls::{BitrateRange, format_mbps};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pointer {
    Touch(TouchId),
    Mouse,
}

#[derive(Clone, Copy)]
struct Drag {
    pointer: Pointer,
    bounds: Bounds<Pixels>,
}

#[derive(Default)]
struct Draft {
    preview: Option<u32>,
    drag: Option<Drag>,
}

impl Draft {
    fn finish(&mut self, pointer: Pointer) -> Option<u32> {
        if self.drag.is_some_and(|drag| drag.pointer == pointer) {
            self.drag = None;
            self.preview.take()
        } else {
            None
        }
    }

    fn cancel(&mut self) {
        self.drag = None;
        self.preview = None;
    }
}

pub(super) struct BitrateEditor {
    input: Entity<TextField>,
    focus: FocusHandle,
    draft: Draft,
    error: String,
}

impl BitrateEditor {
    pub(super) fn new(cx: &mut Context<SessionChrome>) -> Self {
        Self {
            input: cx.new(|cx| TextField::new("精确上限，例如 8.125", false, cx)),
            focus: cx.focus_handle(),
            draft: Draft::default(),
            error: String::new(),
        }
    }

    pub(super) fn cancel(&mut self) {
        self.draft.cancel();
    }
}

impl SessionChrome {
    fn bitrate_range(&self) -> Option<BitrateRange> {
        (self.model.connected
            && self.model.bitrate_supported
            && self.model.pending_monitor.is_none()
            && self.model.panel == Panel::Display)
            .then(|| BitrateRange::for_host(self.model.max_bitrate_kbps))
            .flatten()
    }

    pub(crate) fn cancel_bitrate(&mut self, cx: &mut Context<Self>) {
        if self.bitrate.draft.preview.is_some() || self.bitrate.draft.drag.is_some() {
            self.bitrate.draft.cancel();
            cx.notify();
        }
    }

    fn commit_bitrate(&mut self, value: u32, cx: &mut Context<Self>) {
        let Some(range) = self.bitrate_range() else {
            self.bitrate.draft.cancel();
            return;
        };
        if value < range.min_kbps() || value > range.max_kbps() {
            self.bitrate.error =
                format!("码率必须在 1–{} Mbps 之间", format_mbps(range.max_kbps()));
        } else {
            self.bitrate.error.clear();
            cx.emit(ChromeAction::Bitrate(Some(value)));
        }
        cx.notify();
    }

    fn apply_bitrate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(range) = self.bitrate_range() else {
            return;
        };
        let input = self.bitrate.input.read(cx);
        if input.is_composing() {
            self.bitrate.error = "请先完成输入法组字，再应用。".into();
        } else {
            match range.parse_mbps(input.value()) {
                Ok(value) => {
                    self.bitrate.draft.cancel();
                    self.commit_bitrate(value, cx);
                    window.dismiss_virtual_keyboard();
                }
                Err(error) => self.bitrate.error = error.to_string(),
            }
        }
        cx.notify();
    }

    pub(super) fn bitrate_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .bitrate
            .input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            if event.keystroke.key == "enter" && !self.bitrate.input.read(cx).is_composing() {
                if !event.is_held {
                    self.apply_bitrate(window, cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        if !self.bitrate.focus.is_focused(window) {
            return;
        }
        if let Some(range) = self.bitrate_range() {
            let value = self
                .bitrate
                .draft
                .preview
                .or(self.model.bitrate_limit_kbps)
                .unwrap_or(range.min_kbps());
            match event.keystroke.key.as_str() {
                "left" | "down" => self.bitrate.draft.preview = Some(range.step(value, false)),
                "right" | "up" => self.bitrate.draft.preview = Some(range.step(value, true)),
                "enter" if !event.is_held => {
                    if let Some(value) = self.bitrate.draft.preview.take() {
                        self.commit_bitrate(value, cx);
                    }
                }
                "escape" => self.bitrate.draft.cancel(),
                _ => {}
            }
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn bitrate_key_up(
        &mut self,
        _: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bitrate.focus.is_focused(window)
            || self
                .bitrate
                .input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            cx.stop_propagation();
        }
    }

    fn slider_event(
        &mut self,
        pointer: Pointer,
        phase: TouchPhase,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .bitrate
            .draft
            .drag
            .is_some_and(|drag| drag.bounds != bounds)
        {
            self.bitrate.draft.cancel();
            cx.notify();
            return;
        }
        if phase == TouchPhase::Cancelled {
            if self
                .bitrate
                .draft
                .drag
                .is_some_and(|drag| drag.pointer == pointer)
            {
                self.bitrate.draft.cancel();
                cx.notify();
            }
            return;
        }
        let Some(range) = self.bitrate_range() else {
            self.bitrate.draft.cancel();
            return;
        };
        let width: f32 = (bounds.size.width - px(24.)).into();
        if width <= 0. {
            return;
        }
        if phase == TouchPhase::Started {
            if !bounds.contains(&position) || self.bitrate.draft.drag.is_some() {
                return;
            }
            self.bitrate.draft.drag = Some(Drag { pointer, bounds });
            window.focus(&self.bitrate.focus, cx);
            self.bitrate.error.clear();
        }
        if !self
            .bitrate
            .draft
            .drag
            .is_some_and(|drag| drag.pointer == pointer)
        {
            return;
        }
        let x: f32 = (position.x - bounds.left() - px(12.)).into();
        self.bitrate.draft.preview = Some(range.at_fraction((x / width) as f64));
        if phase == TouchPhase::Ended
            && let Some(value) = self.bitrate.draft.finish(pointer)
        {
            self.commit_bitrate(value, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn bitrate_slider(
        &self,
        range: BitrateRange,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let value = self
            .bitrate
            .draft
            .preview
            .or(self.model.bitrate_limit_kbps)
            .unwrap_or(range.min_kbps());
        let fraction = range.fraction(value) as f32;
        let available = range.fraction(range.max_kbps()) as f32;
        let owner = cx.entity_id();
        let touch = cx.listener(|view, event: &(TouchEvent, Bounds<Pixels>), window, cx| {
            view.slider_event(
                Pointer::Touch(event.0.id),
                event.0.phase,
                event.0.position,
                event.1,
                window,
                cx,
            );
        });
        let mouse = cx.listener(
            |view, event: &(TouchPhase, Point<Pixels>, Bounds<Pixels>), window, cx| {
                view.slider_event(Pointer::Mouse, event.0, event.1, event.2, window, cx);
            },
        );
        let moved = cx.listener(
            |view, event: &(TouchPhase, Point<Pixels>, Bounds<Pixels>), window, cx| {
                view.slider_event(Pointer::Mouse, event.0, event.1, event.2, window, cx);
            },
        );
        let ended = cx.listener(
            |view, event: &(Point<Pixels>, Bounds<Pixels>), window, cx| {
                view.slider_event(
                    Pointer::Mouse,
                    TouchPhase::Ended,
                    event.0,
                    event.1,
                    window,
                    cx,
                );
            },
        );
        div()
            .id("session-bitrate-slider")
            .w_full()
            .h(px(48.))
            .track_focus(&self.bitrate.focus)
            .tab_index(0)
            .opacity(if ready { 1. } else { 0.42 })
            .child(
                canvas(
                    move |bounds, window, _| {
                        (
                            bounds,
                            ready.then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal)),
                        )
                    },
                    move |_, (bounds, hitbox), window, _| {
                        let width = (bounds.size.width - px(24.)).max(px(0.));
                        let x = bounds.left() + px(12.);
                        let y = bounds.top() + px(22.);
                        window.paint_quad(
                            fill(Bounds::new(point(x, y), size(width, px(4.))), rgb(RAISED))
                                .corner_radii(px(2.)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(point(x, y), size(width * available, px(4.))),
                                rgb(MUTED),
                            )
                            .corner_radii(px(2.)),
                        );
                        window.paint_quad(
                            fill(
                                Bounds::new(point(x, y), size(width * fraction, px(4.))),
                                rgb(BLUE),
                            )
                            .corner_radii(px(2.)),
                        );
                        for index in 0..=4 {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(x + width * (index as f32 / 4.), y - px(4.)),
                                    size(px(1.), px(12.)),
                                ),
                                rgb(MUTED),
                            ));
                        }
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(x + width * fraction - px(10.), y - px(8.)),
                                    size(px(20.), px(20.)),
                                ),
                                rgb(TEXT),
                            )
                            .corner_radii(px(10.)),
                        );
                        if let Some(hitbox) = hitbox {
                            window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                                touch(&(event.clone(), bounds), window, cx)
                            });
                            window.on_mouse_event(
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Bubble
                                        && event.button == MouseButton::Left
                                        && hitbox.is_hovered(window)
                                    {
                                        mouse(
                                            &(TouchPhase::Started, event.position, bounds),
                                            window,
                                            cx,
                                        );
                                    }
                                },
                            );
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Bubble {
                                        let phase =
                                            if event.pressed_button == Some(MouseButton::Left) {
                                                TouchPhase::Moved
                                            } else {
                                                TouchPhase::Cancelled
                                            };
                                        moved(&(phase, event.position, bounds), window, cx);
                                    }
                                },
                            );
                            window.on_mouse_event(
                                move |event: &MouseUpEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Bubble
                                        && event.button == MouseButton::Left
                                    {
                                        ended(&(event.position, bounds), window, cx);
                                    }
                                },
                            );
                        }
                    },
                )
                .size_full(),
            )
    }

    pub(super) fn bitrate_controls(&self, cx: &mut Context<Self>) -> Div {
        if !self.model.bitrate_supported {
            return div()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child("远端未提供码率上限控制。");
        }
        let ready = self.model.connected
            && self.model.bitrate_supported
            && self.model.pending_monitor.is_none();
        let confirmed = self
            .model
            .bitrate_limit_kbps
            .map(|value| format!("{} Mbps", format_mbps(value)))
            .unwrap_or("自动（无手动上限）".into());
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .child(format!("主机已确认：{confirmed}")),
            )
            .child(self.button(
                cx,
                "session-bitrate-auto",
                "自动／解除上限",
                self.model.bitrate_limit_kbps.is_none(),
                ready,
                ChromeAction::Bitrate(None),
            ));
        if let Some(range) = BitrateRange::for_host(self.model.max_bitrate_kbps) {
            body = body.child(self.bitrate_slider(range, ready, cx))
                .child(slider_ticks())
                .when_some(self.bitrate.draft.preview, |body,value|body.child(div().text_size(px(13.)).text_color(rgb(BLUE)).child(format!("本地预览：{} Mbps；松手或按 Enter 提交",format_mbps(value)))))
                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(format!("主机允许 1–{} Mbps；滑块每段步长依次为 1／10／30／100 Mbps，右侧灰区不可用。",format_mbps(range.max_kbps()))))
                .child(div().text_size(px(12.)).child("精确上限（Mbps，最多 3 位小数）"));
            if ready {
                body = body.child(self.bitrate.input.clone()).child(
                    div()
                        .id("session-bitrate-apply")
                        .min_h(px(48.))
                        .px_3()
                        .rounded(px(14.))
                        .bg(rgb(BLUE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("应用精确上限")
                        .on_click(
                            cx.listener(|view, _, window, cx| view.apply_bitrate(window, cx)),
                        ),
                );
            } else {
                body = body.child(
                    div()
                        .min_h(px(48.))
                        .text_color(rgb(MUTED))
                        .child("连接或切屏完成后可调节"),
                );
            }
        } else {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("主机未提供 1 Mbps 以上的手动上限调节。"),
            );
        }
        body.when(!self.bitrate.error.is_empty(), |body| {
            body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xf08585))
                    .child(self.bitrate.error.clone()),
            )
        })
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child("调节的是码率上限；画面与网络需要较低码率时仍会自适应降低。"),
        )
    }
}

fn slider_ticks() -> Div {
    div()
        .relative()
        .w_full()
        .h(px(20.))
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(
            div()
                .absolute()
                .left(px(12.))
                .right(px(12.))
                .h_full()
                .children(
                    ["1", "10", "100", "300", "1000"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| {
                            div()
                                .absolute()
                                .left(gpui::relative(index as f32 / 4.))
                                .ml(px(-24.))
                                .w(px(48.))
                                .flex()
                                .justify_center()
                                .child(label)
                        }),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_chrome::ChromeModel;
    use gpui_test_support::TestAppContext;
    use std::{cell::RefCell, rc::Rc};

    #[gpui_test_support::test]
    fn local_input_and_slider_only_emit_on_explicit_confirmation(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|_, cx| {
            let mut editor = SessionChrome::new(cx);
            editor.model = ChromeModel {
                connected: true,
                bitrate_supported: true,
                max_bitrate_kbps: 1_000_000,
                panel: Panel::Display,
                ..Default::default()
            };
            editor.model.layout.panel = Some(crate::session_layout::Rect {
                x: 0.,
                y: 0.,
                width: 500.,
                height: 700.,
            });
            editor
        });
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let events = emitted.clone();
        let _subscription = cx.update(|window, cx| {
            window.subscribe(&editor, cx, move |_, event, _, _| {
                events.borrow_mut().push(*event)
            })
        });
        cx.update(|window, cx| {
            let focus = editor.read(cx).bitrate.input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.run_until_parked();
        cx.simulate_input("12.345");
        assert_eq!(
            cx.update(|_, cx| editor.read(cx).bitrate.input.read(cx).value().to_owned()),
            "12.345"
        );
        assert!(emitted.borrow().is_empty());
        cx.simulate_keystrokes("enter");
        assert_eq!(*emitted.borrow(), vec![ChromeAction::Bitrate(Some(12_345))]);

        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(424.), px(48.)));
        let pointer = Pointer::Touch(TouchId(7));
        let position = point(px(312.), px(24.));
        for phase in [TouchPhase::Started, TouchPhase::Moved] {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.slider_event(pointer, phase, position, bounds, window, cx)
                })
            });
        }
        assert_eq!(emitted.borrow().len(), 1);
        for phase in [TouchPhase::Ended, TouchPhase::Ended] {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.slider_event(pointer, phase, position, bounds, window, cx)
                })
            });
        }
        assert_eq!(
            *emitted.borrow(),
            vec![
                ChromeAction::Bitrate(Some(12_345)),
                ChromeAction::Bitrate(Some(300_000))
            ]
        );
        for phase in [
            TouchPhase::Started,
            TouchPhase::Cancelled,
            TouchPhase::Ended,
        ] {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.slider_event(Pointer::Mouse, phase, position, bounds, window, cx)
                })
            });
        }
        assert_eq!(emitted.borrow().len(), 2);
    }
}
