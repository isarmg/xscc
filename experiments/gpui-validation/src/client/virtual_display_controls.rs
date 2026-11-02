#[path = "../../../shared/virtual_display_controls.rs"]
pub(super) mod form;

use super::{ManagementPanel, Reply, toolbar_controls::ToolbarMenu};
use crate::{dashboard as ui, theme};
use gpui::{Context, Window, div, prelude::*, px, rgb};
use librustdesk::{
    gpui_management,
    gpui_session::{VirtualDisplayConfiguration, VirtualDisplayMode, VirtualDisplayState},
};

pub(super) fn project(state: &VirtualDisplayState) -> form::State {
    form::State {
        protocol_version: state.protocol_version,
        driver: state.driver.clone(),
        status: state.status.clone(),
        message: state.message.clone(),
        configured_count: state.configured_count,
        present_count: state.present_count,
        active_count: state.active_count,
        persistent: state.persistent,
        maximum_count: state.maximum_count,
        displays: state
            .displays
            .iter()
            .map(|screen| form::Screen {
                id: screen.id.clone(),
                name: screen.name.clone(),
                index: screen.index,
                active: screen.active,
                mode: form::Mode {
                    width: screen.width,
                    height: screen.height,
                    refresh_rate_units: screen.refresh_rate_units,
                },
            })
            .collect(),
        modes: state
            .modes
            .iter()
            .map(|mode| form::Mode {
                width: mode.width,
                height: mode.height,
                refresh_rate_units: mode.refresh_rate_units,
            })
            .collect(),
    }
}

enum LocalRequest {
    Query,
    Prepare,
    Configure(VirtualDisplayConfiguration),
}

impl ManagementPanel {
    fn virtual_display_state(&self) -> Option<form::State> {
        if self.host {
            self.local_virtual_display.clone()
        } else {
            self.video_state
                .media_controls
                .virtual_display_configuration
                .as_ref()
                .map(project)
        }
    }

    fn virtual_display_idle(&self) -> bool {
        if self.host {
            self.initialized && !self.local_virtual_pending && !self.action_pending
        } else {
            self.video_state.connected
                && !self.video_state.switching_monitor
                && self.video_state.pending_media.is_none()
                && !self.video_state.virtual_display_pending
        }
    }

    fn virtual_display_edit_enabled(&self) -> bool {
        self.virtual_display_idle()
            && self
                .virtual_display_state()
                .is_some_and(|state| state.configurable())
    }

    fn request_local_virtual_displays(&mut self, request: LocalRequest) {
        if !self.host || !self.virtual_display_idle() {
            return;
        }
        self.local_virtual_pending = true;
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let operation = async move {
                match request {
                    LocalRequest::Query => gpui_management::virtual_display_status().await,
                    LocalRequest::Prepare => gpui_management::prepare_virtual_displays().await,
                    LocalRequest::Configure(configuration) => {
                        gpui_management::configure_virtual_displays(configuration).await
                    }
                }
            };
            let result =
                match tokio::time::timeout(std::time::Duration::from_secs(25), operation).await {
                    Ok(result) => result
                        .map(|state| project(&state))
                        .map_err(|error| error.to_string()),
                    Err(_) => {
                        Err("尚未确认 Windows 操作结果，请查询状态；操作可能仍在主机执行。".into())
                    }
                };
            let _ = sender.send(Reply::VirtualDisplay(result));
            wake.notify_one();
        });
    }

    pub(super) fn query_local_virtual_displays(&mut self) {
        self.request_local_virtual_displays(LocalRequest::Query);
    }

    fn query_virtual_displays(&mut self, cx: &mut Context<Self>) {
        if !self.host
            && !self
                .virtual_display_state()
                .is_some_and(|state| state.protocol_version >= 1)
        {
            return;
        }
        if !self.virtual_display_idle() {
            return;
        }
        if self.host {
            self.query_local_virtual_displays();
        } else {
            self.video
                .update(cx, |video, cx| video.query_virtual_displays(cx));
        }
    }

    fn apply_virtual_display_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.virtual_display_idle()
            || !self
                .virtual_display_state()
                .is_some_and(|state| state.configurable())
        {
            return;
        }
        let configuration = match self.virtual_display.configuration(cx) {
            Ok(value) => value,
            Err(error) => {
                self.virtual_display.error = Some(error);
                cx.notify();
                return;
            }
        };
        let configuration = VirtualDisplayConfiguration {
            count: configuration.count,
            persistent: configuration.persistent,
            modes: configuration
                .mode
                .into_iter()
                .map(|mode| VirtualDisplayMode {
                    width: mode.width,
                    height: mode.height,
                    refresh_rate_units: mode.refresh_rate_units,
                })
                .collect(),
        };
        self.virtual_display.error = None;
        window.dismiss_virtual_keyboard();
        if self.host {
            self.request_local_virtual_displays(LocalRequest::Configure(configuration));
        } else {
            self.video.update(cx, |video, cx| {
                video.configure_virtual_displays(configuration, cx)
            });
        }
        cx.notify();
    }

    pub(super) fn render_virtual_display_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self.toolbar_menu != Some(ToolbarMenu::VirtualDisplays) {
            return self.render_toolbar_dropdown(ToolbarMenu::VirtualDisplays, div(), window, cx);
        }
        let palette = theme::palette(window.appearance());
        let idle = self.virtual_display_idle();
        let state = self
            .virtual_display_state()
            .filter(|state| state.protocol_version >= 1);
        let enabled = idle && state.as_ref().is_some_and(form::State::configurable);
        let mut body = div().flex().flex_col().gap_2().child(
            ui::button(
                "virtual-query",
                if idle {
                    "查询实际状态"
                } else {
                    "处理中，等待确认…"
                },
                false,
                idle && (self.host || state.is_some()),
                palette,
            )
            .w_full()
            .on_click(cx.listener(|view, _, _, cx| {
                view.query_virtual_displays(cx);
                cx.stop_propagation();
                cx.notify();
            })),
        );
        if let Some(state) = &state {
            body = body.child(form::summary(state, palette));
            if self.host && state.status == "not_installed" {
                body = body.child(
                    ui::button("virtual-prepare", "安装签名驱动", false, idle, palette)
                        .w_full()
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.request_local_virtual_displays(LocalRequest::Prepare);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                );
            }
            body = body.child(
                ui::button("virtual-edit", "配置虚拟显示器", false, enabled, palette)
                    .w_full()
                    .on_click(cx.listener(|view, _, window, cx| {
                        if view.virtual_display_idle()
                            && let Some(state) = view
                                .virtual_display_state()
                                .filter(form::State::configurable)
                        {
                            view.virtual_display.begin(&state, window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })),
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
                body = body
                    .child(
                        ui::button(
                            "virtual-use-mode",
                            if self.virtual_display.use_mode {
                                "显示模式：自定义"
                            } else {
                                "显示模式：保持当前"
                            },
                            self.virtual_display.use_mode,
                            enabled,
                            palette,
                        )
                        .w_full()
                        .on_click(cx.listener(|view, _, _, cx| {
                            if view.virtual_display_edit_enabled() {
                                view.virtual_display.use_mode = !view.virtual_display.use_mode;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                    )
                    .child(self.virtual_display.fields(palette))
                    .child(div().flex().gap_2().children(
                        [(false, "会话模式"), (true, "长期保留")].into_iter().map(
                            |(persistent, label)| {
                                ui::button(
                                    if persistent {
                                        "virtual-persistent"
                                    } else {
                                        "virtual-session"
                                    },
                                    label,
                                    self.virtual_display.persistent == persistent,
                                    enabled,
                                    palette,
                                )
                                .flex_1()
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        if view.virtual_display_edit_enabled() {
                                            view.virtual_display.persistent = persistent;
                                        }
                                        cx.stop_propagation();
                                        cx.notify();
                                    },
                                ))
                            },
                        ),
                    ));
                if self.virtual_display.use_mode && !state.modes.is_empty() {
                    body = body.child(
                        div()
                            .id("virtual-supported-modes")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(state.modes.iter().copied().enumerate().map(
                                |(index, mode)| {
                                    ui::button(
                                        ("virtual-mode", index),
                                        &form::mode_label(mode),
                                        false,
                                        enabled,
                                        palette,
                                    )
                                    .w_full()
                                    .min_h(px(32.))
                                    .py_0()
                                    .on_click(cx.listener(
                                        move |view, _, window, cx| {
                                            if view.virtual_display_edit_enabled() {
                                                view.virtual_display.set_mode(mode, window, cx);
                                            }
                                            cx.stop_propagation();
                                            cx.notify();
                                        },
                                    ))
                                },
                            )),
                    );
                }
                body = body.child(
                    ui::button("virtual-apply", "应用配置", true, enabled, palette)
                        .w_full()
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.apply_virtual_display_configuration(window, cx);
                            cx.stop_propagation();
                        })),
                );
            }
        } else if self.host {
            body = body.child(
                div()
                    .text_color(rgb(palette.muted))
                    .child("查询本机虚拟显示器状态后可配置。"),
            );
        } else {
            body = body.child(div().text_size(px(12.)).text_color(rgb(palette.muted)).child("旧版主机仅提供添加 / 移除；升级被控端后可设置数量、显示模式及保留策略。"))
                .child(self.render_legacy_virtual_display_controls(window, cx));
        }
        let notice = if self.host {
            &self.notice
        } else {
            &self.video_state.notice
        };
        if !notice.is_empty() {
            body = body.child(div().text_size(px(12.)).child(notice.clone()));
        }
        if let Some(error) = &self.virtual_display.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            );
        }
        self.render_toolbar_dropdown(ToolbarMenu::VirtualDisplays, body, window, cx)
    }
}
