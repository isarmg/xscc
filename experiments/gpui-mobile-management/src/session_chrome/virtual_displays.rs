#[path = "../../../shared/virtual_display_controls.rs"]
pub(crate) mod form;

use super::{ACCENT, ChromeAction, MUTED, SessionChrome};
use gpui::{Context, div, prelude::*, px, rgb};

impl SessionChrome {
    pub(super) fn virtual_display_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let palette = crate::theme::LIGHT;
        let Some(state) = &self.model.virtual_display_configuration else {
            return div();
        };
        let idle = self.model.connected
            && self.model.pending_monitor.is_none()
            && !self.model.virtual_display_pending;
        let enabled =
            idle && self.model.can_control && !self.model.view_only && state.configurable();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(form::summary(state, palette))
            .child(
                self.button(
                    cx,
                    "virtual-query",
                    if self.model.virtual_display_pending {
                        "处理中，等待确认…"
                    } else {
                        "查询实际状态"
                    },
                    false,
                    idle,
                    ChromeAction::QueryVirtualDisplays,
                )
                .w_full(),
            )
            .child(
                div()
                    .id("virtual-edit")
                    .min_h(px(48.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(14.))
                    .bg(rgb(ACCENT))
                    .text_color(rgb(palette.on_accent))
                    .opacity(if enabled { 1. } else { 0.42 })
                    .child("配置虚拟显示器")
                    .when(enabled, |button| {
                        button.on_click(cx.listener(|view, _, window, cx| {
                            if let Some(state) = view
                                .model
                                .virtual_display_configuration
                                .clone()
                                .filter(form::State::configurable)
                            {
                                view.virtual_display.begin(&state, window, cx);
                            }
                            window.dismiss_virtual_keyboard();
                            cx.stop_propagation();
                            cx.notify();
                        }))
                    }),
            );
        for screen in &state.displays {
            body = body.child(div().text_size(px(12.)).child(format!(
                "{} · {} · {}",
                if screen.name.is_empty() {
                    &screen.id
                } else {
                    &screen.name
                },
                form::mode_label(screen.mode),
                if screen.active {
                    "可捕获"
                } else {
                    "未活动"
                }
            )));
        }
        if self.virtual_display.editing {
            let count = self.virtual_display.count_controls(
                state.maximum_count,
                enabled,
                palette,
                cx,
                cx.listener(|view, increase: &bool, window, cx| {
                    if view.model.connected
                        && view.model.pending_monitor.is_none()
                        && !view.model.virtual_display_pending
                        && view.model.can_control
                        && !view.model.view_only
                        && let Some(state) = view
                            .model
                            .virtual_display_configuration
                            .as_ref()
                            .filter(|state| state.configurable())
                    {
                        view.virtual_display
                            .step_count(*increase, state.maximum_count, window, cx);
                    }
                    window.dismiss_virtual_keyboard();
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
            body = body
                .child(
                    div()
                        .id("virtual-use-mode")
                        .min_h(px(48.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(px(14.))
                        .bg(rgb(palette.accent_soft))
                        .opacity(if enabled { 1. } else { 0.42 })
                        .child(if self.virtual_display.use_mode {
                            "显示模式：自定义"
                        } else {
                            "显示模式：保持当前"
                        })
                        .when(enabled, |button| {
                            button.on_click(cx.listener(|view, _, _, cx| {
                                view.virtual_display.use_mode = !view.virtual_display.use_mode;
                                cx.stop_propagation();
                                cx.notify();
                            }))
                        }),
                )
                .child(self.virtual_display.fields(palette, count))
                .child(div().flex().gap_2().children(
                    [(false, "会话模式"), (true, "长期保留")].into_iter().map(
                        |(persistent, label)| {
                            div()
                                .id(if persistent {
                                    "virtual-persistent"
                                } else {
                                    "virtual-session"
                                })
                                .flex_1()
                                .min_h(px(48.))
                                .px_2()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(14.))
                                .bg(rgb(if self.virtual_display.persistent == persistent {
                                    ACCENT
                                } else {
                                    palette.accent_soft
                                }))
                                .text_color(rgb(if self.virtual_display.persistent == persistent {
                                    palette.on_accent
                                } else {
                                    palette.text
                                }))
                                .opacity(if enabled { 1. } else { 0.42 })
                                .child(label)
                                .when(enabled, |button| {
                                    button.on_click(cx.listener(move |view, _, _, cx| {
                                        view.virtual_display.persistent = persistent;
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                                })
                        },
                    ),
                ));
            if self.virtual_display.use_mode && !state.modes.is_empty() {
                body =
                    body.child(
                        div()
                            .id("virtual-supported-modes")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(state.modes.iter().copied().enumerate().map(
                                |(index, mode)| {
                                    div()
                                        .id(("virtual-mode", index))
                                        .min_h(px(48.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(10.))
                                        .bg(rgb(palette.accent_soft))
                                        .opacity(if enabled { 1. } else { 0.42 })
                                        .child(form::mode_label(mode))
                                        .when(enabled, |button| {
                                            button.on_click(cx.listener(
                                                move |view, _, window, cx| {
                                                    view.virtual_display.set_mode(mode, window, cx);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                },
                                            ))
                                        })
                                },
                            )),
                    );
            }
            body = body.child(
                div()
                    .id("virtual-apply")
                    .min_h(px(48.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(14.))
                    .bg(rgb(ACCENT))
                    .text_color(rgb(palette.on_accent))
                    .opacity(if enabled { 1. } else { 0.42 })
                    .child("应用配置")
                    .when(enabled, |button| {
                        button.on_click(cx.listener(|view, _, window, cx| {
                            match view.virtual_display.configuration(cx) {
                                Ok(configuration) => {
                                    view.virtual_display.error = None;
                                    window.dismiss_virtual_keyboard();
                                    cx.emit(ChromeAction::ConfigureVirtualDisplays(configuration));
                                }
                                Err(error) => view.virtual_display.error = Some(error),
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                    }),
            );
        }
        if let Some(error) = &self.virtual_display.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            );
        }
        if !self.model.notice.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(self.model.notice.clone()),
            );
        }
        body
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use crate::{
        session_chrome::{ChromeModel, MediaMenu},
        session_layout::{Panel, Rect},
    };
    use gpui_test_support::{TestAppContext, TouchEvent, TouchId, TouchPhase, size};
    use std::{cell::Cell, rc::Rc};

    fn tap(cx: &mut gpui_test_support::VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("visible virtual display control: {selector}"));
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            cx.simulate_event(TouchEvent {
                id: TouchId(1),
                phase,
                position: bounds.center(),
                force: None,
                predicted_position: None,
            });
        }
    }

    #[gpui_test_support::test]
    fn count_buttons_preserve_existing_values_and_disable_while_pending(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let mut editor = SessionChrome::new(cx);
            editor.media_menu = Some(MediaMenu::VirtualDisplays);
            editor.model = ChromeModel {
                connected: true,
                can_control: true,
                panel: Panel::Display,
                virtual_display_configuration: Some(form::State {
                    protocol_version: 1,
                    status: "ready".into(),
                    configured_count: 12,
                    persistent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            editor.model.layout.panel = Some(Rect {
                x: 0.,
                y: 0.,
                width: 500.,
                height: 1100.,
            });
            let state = editor.model.virtual_display_configuration.clone().unwrap();
            editor.virtual_display.begin(&state, window, cx);
            editor
        });
        let emitted = Rc::new(Cell::new(false));
        let events = emitted.clone();
        let _subscription = cx
            .update(|window, cx| window.subscribe(&editor, cx, move |_, _, _, _| events.set(true)));
        cx.simulate_resize(size(px(800.), px(1100.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let configuration = |cx: &mut gpui_test_support::VisualTestContext| {
            cx.update(|_, cx| editor.read(cx).virtual_display.configuration(cx).unwrap())
        };
        tap(cx, "virtual-count-plus");
        tap(cx, "virtual-count-minus");
        assert!(!emitted.get(), "stepping must not send a host request");
        assert_eq!(
            configuration(cx),
            form::Configuration {
                count: 11,
                mode: None,
                persistent: true
            }
        );
        for _ in 0..12 {
            tap(cx, "virtual-count-minus");
        }
        assert_eq!(configuration(cx).count, 0);
        for _ in 0..9 {
            tap(cx, "virtual-count-plus");
        }
        assert_eq!(configuration(cx).count, 8);
        for pending in [true, false] {
            cx.update(|_, cx| {
                editor.update(cx, |view, cx| {
                    let mut model = view.model.clone();
                    model.virtual_display_pending = pending;
                    view.set_model(model, cx);
                })
            });
            tap(cx, "virtual-count-minus");
            assert_eq!(
                configuration(cx).count,
                if pending { 8 } else { 7 },
                "pending taps must not change the draft; confirmed controls must recover"
            );
        }
        assert!(
            !emitted.get(),
            "only applying the draft may send a host request"
        );
    }
}
