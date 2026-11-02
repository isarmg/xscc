use super::{ACCENT, ChromeAction, Div, MUTED, MediaMenu, Panel, RAISED, SessionChrome, TEXT};
use gpui::{
    Bounds, Context, DispatchPhase, FocusHandle, HitboxBehavior, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Stateful, TouchEvent,
    TouchId, TouchPhase, Window, canvas, div, fill, point, prelude::*, px, rgb, size,
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
    focus: FocusHandle,
    draft: Draft,
    error: String,
}

impl BitrateEditor {
    pub(super) fn new(cx: &mut Context<SessionChrome>) -> Self {
        Self {
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
            && self.model.panel == Panel::Display
            && self.media_menu == Some(MediaMenu::Bitrate))
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

    pub(super) fn bitrate_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.bitrate.focus.is_focused(window) {
            return;
        }
        if let Some(range) = self.bitrate_range() {
            let value = self
                .bitrate
                .draft
                .preview
                .or(self.model.bitrate_limit_kbps)
                .unwrap_or(8_000)
                .clamp(range.min_kbps(), range.max_kbps());
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
        if self.bitrate.focus.is_focused(window) {
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
            .unwrap_or(8_000)
            .clamp(range.min_kbps(), range.max_kbps());
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
                                rgb(ACCENT),
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

    pub(super) fn bitrate_label(&self) -> String {
        let range = BitrateRange::for_host(self.model.max_bitrate_kbps);
        let value = self
            .bitrate
            .draft
            .preview
            .or(self.model.bitrate_limit_kbps)
            .unwrap_or(8_000);
        range
            .map(|range| {
                format!(
                    "码率 · {} Mbps",
                    format_mbps(value.clamp(range.min_kbps(), range.max_kbps()))
                )
            })
            .unwrap_or_else(|| "码率".into())
    }

    pub(super) fn bitrate_controls(&self, cx: &mut Context<Self>) -> Div {
        let ready = self.model.connected
            && self.model.bitrate_supported
            && self.model.pending_monitor.is_none();
        let mut body = div().flex().flex_col().gap_2();
        if let Some(range) = BitrateRange::for_host(self.model.max_bitrate_kbps) {
            body = body
                .child(self.bitrate_slider(range, ready, cx))
                .child(slider_ticks());
        }
        body.when(!self.bitrate.error.is_empty(), |body| {
            body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(crate::theme::LIGHT.danger))
                    .child(self.bitrate.error.clone()),
            )
        })
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

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use crate::session_chrome::ChromeModel;
    use gpui_test_support::TestAppContext;
    use std::{cell::RefCell, rc::Rc};

    #[gpui_test_support::test]
    fn slider_only_emits_on_release_and_cancellation_discards_preview(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|_, cx| {
            let mut editor = SessionChrome::new(cx);
            editor.media_menu = Some(MediaMenu::Bitrate);
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
        assert!(emitted.borrow().is_empty());
        for phase in [TouchPhase::Ended, TouchPhase::Ended] {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.slider_event(pointer, phase, position, bounds, window, cx)
                })
            });
        }
        assert_eq!(
            *emitted.borrow(),
            vec![ChromeAction::Bitrate(Some(300_000))]
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
        assert_eq!(emitted.borrow().len(), 1);
    }
}
