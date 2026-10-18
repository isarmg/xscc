use crate::{
    remote_video::{RemoteVideo, VideoState},
    text_field::{self, TextField},
    theme,
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, KeyDownEvent, Render,
    StyleRefinement, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use librustdesk::{
    gpui_host,
    gpui_management::{self, Command, Status},
    gpui_session::TargetFps,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use xscc::protocol::InstanceRole;

enum Reply {
    Status(Result<Status, String>),
    Action(Result<Status, String>),
}

struct ManagementPanel {
    runtime: tokio::runtime::Handle,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    wake: Arc<tokio::sync::Notify>,
    server: Entity<TextField>,
    password: Entity<TextField>,
    address_host: Entity<TextField>,
    address_port: Entity<TextField>,
    #[cfg(feature = "test-lan-direct")]
    lan_endpoint: Entity<TextField>,
    #[cfg(all(
        target_os = "windows",
        feature = "native-host",
        feature = "test-lan-direct"
    ))]
    test_host: Arc<Mutex<crate::test_host::TestHost>>,
    host: bool,
    initialized: bool,
    status: Option<Status>,
    status_pending: bool,
    status_error: Option<String>,
    action_pending: bool,
    confirm_unbind: bool,
    show_login: bool,
    notice: String,
    video: Entity<RemoteVideo>,
    video_state: VideoState,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<gpui::Task<()>>,
}

impl ManagementPanel {
    fn refresh(&mut self) {
        if !self.initialized || self.status_pending || self.action_pending {
            return;
        }
        self.status_pending = true;
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let result = gpui_management::status()
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Status(result));
            wake.notify_one();
        });
    }

    fn can_act(&self) -> bool {
        management_actions_enabled(self.initialized, self.status_pending, self.action_pending)
    }

    fn action(&mut self, request: Command) {
        if !self.can_act() {
            return;
        }
        self.action_pending = true;
        self.confirm_unbind = false;
        self.notice = match &request {
            Command::Enroll { .. } => "正在绑定实例…",
            Command::Resume => "正在重新连接服务端…",
            Command::Unbind => "正在解绑实例…",
            Command::SetAddresses { .. } => "正在保存访问地址…",
        }
        .into();
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let result = gpui_management::execute(request)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Action(result));
            wake.notify_one();
        });
    }

    fn enroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_act() || self.status.is_none() {
            return;
        }
        let server = self.server.read(cx).value().trim().to_owned();
        let authorization_code = self.password.read(cx).value().to_owned();
        if server.is_empty() || authorization_code.is_empty() {
            self.notice = "请填写 HTTPS 服务端地址和实例密码。".into();
            cx.notify();
            return;
        }
        let addresses = if self.host {
            match crate::host_address::parse(
                self.address_host.read(cx).value(),
                self.address_port.read(cx).value(),
            ) {
                Ok(addresses) => addresses,
                Err(error) => {
                    self.notice = error;
                    cx.notify();
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.password
            .update(cx, |field, cx| field.clear(window, cx));
        self.action(Command::Enroll {
            server,
            authorization_code,
            addresses,
        });
        cx.notify();
    }

    fn save_address(&mut self, cx: &mut Context<Self>) {
        if !self.host || !self.can_act() {
            return;
        }
        match crate::host_address::parse(
            self.address_host.read(cx).value(),
            self.address_port.read(cx).value(),
        ) {
            Ok(addresses) => self.action(Command::SetAddresses { addresses }),
            Err(error) => self.notice = error,
        }
        cx.notify();
    }

    fn connect(&mut self, device_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.host || self.action_pending || self.status_error.is_some() {
            return;
        }
        self.video
            .update(cx, |video, cx| video.connect(device_id, window, cx));
    }

    #[cfg(feature = "test-lan-direct")]
    fn connect_test_lan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host
            || !self.initialized
            || self.action_pending
            || self.video_state.connecting
            || self.video_state.has_session
        {
            return;
        }
        let endpoint = self.lan_endpoint.read(cx).value().trim().to_owned();
        self.video
            .update(cx, |video, cx| video.connect_test_lan(endpoint, window, cx));
    }

    fn form_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "test-lan-direct")]
        if !self.host
            && self
                .lan_endpoint
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            let key = &event.keystroke;
            if key.key == "enter"
                && !key.modifiers.shift
                && !key.modifiers.control
                && !key.modifiers.alt
                && !key.modifiers.platform
                && !self.lan_endpoint.read(cx).is_composing()
            {
                self.connect_test_lan(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let fields = if self.host {
            vec![
                self.address_host.clone(),
                self.address_port.clone(),
                self.server.clone(),
                self.password.clone(),
            ]
        } else {
            vec![self.server.clone(), self.password.clone()]
        };
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        let fields = if configured || !self.show_login {
            if self.host {
                &fields[..2]
            } else {
                &fields[..0]
            }
        } else {
            fields.as_slice()
        };
        let Some(index) = fields
            .iter()
            .position(|field| field.read(cx).focus_handle(cx).is_focused(window))
        else {
            return;
        };
        if fields[index].read(cx).is_composing() {
            return;
        }
        let key = &event.keystroke;
        if key.modifiers.control || key.modifiers.alt || key.modifiers.platform {
            return;
        }
        match key.key.as_str() {
            "tab" => {
                let next = if key.modifiers.shift {
                    (index + fields.len() - 1) % fields.len()
                } else {
                    (index + 1) % fields.len()
                };
                fields[next].read(cx).focus_handle(cx).focus(window, cx);
                cx.stop_propagation();
            }
            "enter" if !key.modifiers.shift => {
                if configured {
                    self.save_address(cx);
                } else if self.show_login {
                    self.enroll(window, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn consume_updates(&mut self) {
        while let Ok(reply) = self.receiver.try_recv() {
            match reply {
                Reply::Status(result) => {
                    self.status_pending = false;
                    match result {
                        Ok(status) => {
                            self.status = Some(status);
                            self.status_error = None;
                        }
                        Err(error) => self.status_error = Some(error),
                    }
                }
                Reply::Action(result) => {
                    self.action_pending = false;
                    match result {
                        Ok(status) => {
                            self.notice = if status.configured {
                                "设置已更新。"
                            } else {
                                "已解绑。"
                            }
                            .into();
                            self.status = Some(status);
                            self.status_error = None;
                        }
                        Err(error) => {
                            self.notice = format!("操作未完成：{error}。请检查设置后重试。")
                        }
                    }
                }
            }
        }
    }
}

impl ManagementPanel {
    fn render_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let palette = theme::palette(window.appearance());
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        let idle = self.can_act();
        let can_refresh = idle && !self.status_pending;
        let status_text = self
            .status
            .as_ref()
            .map(|status| management_status(status.configured, &status.phase))
            .unwrap_or(if self.initialized {
                "正在读取绑定状态…"
            } else {
                "本机服务启动失败，请重新启动应用。"
            });
        div()
            .flex()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(if self.host {
                        "XSCC 被控端"
                    } else {
                        "XSCC 控制端"
                    })
                    .child(div().text_color(rgb(palette.muted)).child(status_text)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!configured, |row| {
                        row.child(
                            div()
                                .id("login-menu")
                                .px_3()
                                .py_1()
                                .relative()
                                .min_h(px(theme::CONTROL_HEIGHT))
                                .flex()
                                .items_center()
                                .child(theme::surface(
                                    rgb(palette.panel),
                                    None,
                                    theme::CONTROL_EXTENT,
                                ))
                                .cursor_pointer()
                                .child(if self.show_login {
                                    "收起登录"
                                } else {
                                    "登录"
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.show_login = !view.show_login;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("refresh")
                            .px_3()
                            .py_1()
                            .relative()
                            .min_h(px(theme::CONTROL_HEIGHT))
                            .flex()
                            .items_center()
                            .child(theme::surface(
                                rgb(palette.panel),
                                None,
                                theme::CONTROL_EXTENT,
                            ))
                            .text_color(rgb(if can_refresh {
                                palette.accent
                            } else {
                                palette.muted
                            }))
                            .child(if self.status_pending {
                                "正在刷新…"
                            } else {
                                "刷新状态"
                            })
                            .when(can_refresh, |button| {
                                button
                                    .cursor_pointer()
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.refresh();
                                        cx.notify();
                                    }))
                            }),
                    )
                    .when(
                        configured
                            && self.status.as_ref().is_some_and(|status| {
                                !matches!(status.phase.as_str(), "online" | "connecting")
                            }),
                        |row| {
                            row.child(
                                div()
                                    .id("resume")
                                    .px_3()
                                    .py_1()
                                    .relative()
                                    .min_h(px(theme::CONTROL_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .child(theme::surface(
                                        rgb(palette.panel),
                                        None,
                                        theme::CONTROL_EXTENT,
                                    ))
                                    .text_color(rgb(if idle {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    }))
                                    .child("重新连接服务端")
                                    .when(idle, |button| {
                                        button.cursor_pointer().on_click(cx.listener(
                                            |view, _, _, cx| {
                                                view.action(Command::Resume);
                                                cx.notify();
                                            },
                                        ))
                                    }),
                            )
                        },
                    )
                    .when(configured, |row| {
                        row.child(
                            div()
                                .id("unbind")
                                .px_3()
                                .py_1()
                                .relative()
                                .min_h(px(theme::CONTROL_HEIGHT))
                                .flex()
                                .items_center()
                                .child(theme::surface(
                                    rgb(palette.panel),
                                    None,
                                    theme::CONTROL_EXTENT,
                                ))
                                .text_color(rgb(if idle { palette.danger } else { palette.muted }))
                                .when(idle, |button| button.cursor_pointer())
                                .child(if self.confirm_unbind {
                                    "确认解绑并关闭连接"
                                } else {
                                    "解绑此实例"
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if !view.can_act() {
                                        return;
                                    }
                                    if view.confirm_unbind {
                                        view.video.update(cx, |video, cx| video.disconnect(cx));
                                        view.action(Command::Unbind);
                                    } else {
                                        view.confirm_unbind = true;
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .when(self.confirm_unbind, |row| {
                        row.child(
                            div()
                                .id("cancel-unbind")
                                .relative()
                                .min_h(px(theme::CONTROL_HEIGHT))
                                .flex()
                                .items_center()
                                .child(theme::surface(
                                    rgb(palette.panel),
                                    None,
                                    theme::CONTROL_EXTENT,
                                ))
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .child("取消")
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.confirm_unbind = false;
                                    cx.notify();
                                })),
                        )
                    }),
            )
    }
}

impl Render for ManagementPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(window.appearance());
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        let idle = self.can_act();
        let peers = self
            .status
            .as_ref()
            .map(|status| status.peers.as_slice())
            .unwrap_or_default();
        let current_addresses = self
            .status
            .as_ref()
            .map(|status| {
                if status.addresses.is_empty() {
                    "自动采集本机地址".into()
                } else {
                    status
                        .addresses
                        .iter()
                        .map(|address| format!("{} / 端口 {}", address.host, address.port))
                        .collect::<Vec<_>>()
                        .join("，")
                }
            })
            .unwrap_or_else(|| "正在读取…".into());
        let correct_role = self.status.as_ref().is_some_and(|status| {
            matches!(
                (&status.role, self.host),
                (Some(InstanceRole::Controlled), true) | (Some(InstanceRole::Controller), false)
            )
        });
        let can_connect = correct_role
            && !self.host
            && !self.video_state.connecting
            && !self.video_state.has_session
            && !self.action_pending
            && self.status_error.is_none();
        let panel = div()
            .id("management-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(rgb(palette.page))
            .text_color(rgb(palette.text))
            .text_size(px(theme::BODY_SIZE))
            .line_height(px(theme::LINE_HEIGHT))
            .on_key_down(cx.listener(Self::form_key_down))
            .when(
                configured
                    && self
                        .status
                        .as_ref()
                        .is_some_and(|status| status.role.is_some())
                    && !correct_role,
                |root| {
                    root.child(
                    "实例角色不适用于此端。请在服务器创建对应角色的实例，然后主动解绑并重新绑定。",
                )
                },
            )
            .when(!self.notice.is_empty(), |root| {
                root.child(self.notice.clone())
            })
            .when_some(self.status_error.clone(), |root, error| {
                root.child(div().text_color(rgb(palette.danger)).child(format!(
                    "暂时无法刷新状态，设备列表可能已过期。请检查本机服务或点击刷新。详情：{error}"
                )))
            })
            .when(self.host && self.initialized, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(format!("当前上报访问地址：{current_addresses}"))
                        .child("访问地址（留空自动获取）")
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(self.address_host.clone())
                                .child(self.address_port.clone()),
                        )
                        .when(configured, |section| {
                            section.child(
                                div()
                                    .id("save-address")
                                    .px_3()
                                    .py_1()
                                    .relative()
                                    .min_h(px(theme::CONTROL_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .child(theme::surface(
                                        rgb(palette.panel),
                                        None,
                                        theme::CONTROL_EXTENT,
                                    ))
                                    .text_color(rgb(if idle {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    }))
                                    .when(idle, |button| button.cursor_pointer())
                                    .child(if self.action_pending {
                                        "正在保存…"
                                    } else {
                                        "保存访问地址"
                                    })
                                    .on_click(cx.listener(|view, _, _, cx| view.save_address(cx))),
                            )
                        }),
                )
            })
            .when(
                self.show_login && !configured && self.initialized && self.status.is_some(),
                |root| {
                    root.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("登录并绑定实例")
                            .child("服务端地址")
                            .child(self.server.clone())
                            .child("实例密码")
                            .child(self.password.clone())
                            .child(
                                div()
                                    .id("enroll")
                                    .px_3()
                                    .py_2()
                                    .relative()
                                    .min_h(px(theme::CONTROL_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .child(theme::surface(
                                        rgb(palette.accent),
                                        None,
                                        theme::CONTROL_EXTENT,
                                    ))
                                    .text_color(rgb(palette.page))
                                    .when(idle, |button| button.cursor_pointer())
                                    .child(if !self.initialized || self.action_pending {
                                        "正在绑定…"
                                    } else {
                                        "绑定实例"
                                    })
                                    .on_click(
                                        cx.listener(|view, _, window, cx| view.enroll(window, cx)),
                                    ),
                            ),
                    )
                },
            );
        #[cfg(feature = "test-lan-direct")]
        let panel = panel.when(!self.host, |root| {
            let enabled = self.initialized
                && !self.action_pending
                && !self.video_state.connecting
                && !self.video_state.has_session;
            root.child(
                div()
                    .relative()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(theme::surface(
                        rgb(palette.panel),
                        None,
                        theme::PANEL_EXTENT,
                    ))
                    .child("IP 直连 · 仅测试")
                    .child(
                        div()
                            .text_color(rgb(palette.danger))
                            .child("测试专用：任何可达设备均可直接控制，无身份验证。"),
                    )
                    .child(self.lan_endpoint.clone())
                    .child(
                        div()
                            .id("test-lan-connect")
                            .px_3()
                            .py_1()
                            .relative()
                            .min_h(px(theme::CONTROL_HEIGHT))
                            .flex()
                            .items_center()
                            .child(theme::surface(
                                rgb(palette.page),
                                None,
                                theme::CONTROL_EXTENT,
                            ))
                            .text_color(rgb(if enabled {
                                palette.accent
                            } else {
                                palette.muted
                            }))
                            .child("连接 IP")
                            .when(enabled, |button| {
                                button.cursor_pointer().on_click(cx.listener(
                                    |view, _, window, cx| view.connect_test_lan(window, cx),
                                ))
                            }),
                    ),
            )
        });
        #[cfg(all(target_os = "windows", feature = "native-host", feature = "test-lan-direct"))]
        let panel = panel.when(self.host, |root| {
            let (active, message, address) = {
                let host = self.test_host.lock().unwrap();
                (host.active(), host.message.clone(), host.address)
            };
            let addresses = address.map(gpui_host::test_direct_addresses).unwrap_or_default();
            root.child(div().p_4().flex().flex_col().gap_2()
                .child("IP 直连 · 仅测试")
                .child(div().text_color(rgb(palette.danger)).child("无身份验证、无接入确认：任何能到达监听端口的设备均可控制此电脑，包括公网设备。"))
                .child("监听地址（留空使用 0.0.0.0:21119；不需要绑定管理服务器）")
                .child(self.lan_endpoint.clone())
                .child(message)
                .children(addresses.into_iter().map(|address| format!("手机可填写：{address}")))
                .child("手机不填端口时使用 21119；通配地址不是手机连接目标。退出此窗口会停止测试监听。")
                .child(div().id("test-direct-host-toggle").px_3().py_2().cursor_pointer()
                    .text_color(rgb(palette.accent))
                    .child(if active { "停止 IP 直连" } else { "开启 IP 直连（测试）" })
                    .on_click(cx.listener(|view, _, _, cx| {
                        let mut host = view.test_host.lock().unwrap();
                        if host.active() {
                            host.stop();
                        } else {
                            host.start(view.lan_endpoint.read(cx).value());
                        }
                        cx.notify();
                    }))))
        });
        panel
            .child(if self.host {
                "允许接入的控制端"
            } else {
                "授权主机"
            })
            .when(peers.is_empty(), |root| {
                root.child(div().text_color(rgb(palette.muted)).child(if !configured {
                    "请从上方登录并绑定实例。"
                } else if !correct_role {
                    "请检查服务端连接与实例角色。"
                } else {
                    "暂无授权设备。"
                }))
            })
            .child(
                div()
                    .id("peers")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(peers.iter().map(|peer| {
                        let id = peer.binding.device_id.to_string();
                        let available = can_connect
                            && peer.online
                            && matches!(peer.role, InstanceRole::Controlled);
                        div()
                            .id(gpui::SharedString::from(id.clone()))
                            .p_4()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(theme::surface(
                                rgb(palette.panel),
                                None,
                                theme::PANEL_EXTENT,
                            ))
                            .child(peer.name.clone())
                            .child(div().text_color(rgb(palette.muted)).child(
                                if peer.addresses.is_empty() {
                                    "未上报地址".into()
                                } else {
                                    peer.addresses
                                        .iter()
                                        .map(|address| {
                                            format_peer_address(&address.host, address.port)
                                        })
                                        .collect::<Vec<_>>()
                                        .join("，")
                                },
                            ))
                            .child(
                                div()
                                    .text_color(rgb(if available {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    }))
                                    .child(if peer.online { "在线" } else { "离线" }),
                            )
                            .when(available, |row| {
                                row.cursor_pointer().on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        view.connect(id.clone(), window, cx);
                                        cx.notify();
                                    },
                                ))
                            })
                    })),
            )
            .when(!self.host && !self.video_state.notice.is_empty(), |root| {
                root.child(div().child(self.video_state.notice.clone()))
            })
            .when(!self.host && self.video_state.has_session, |root| {
                root.child(
                    div()
                        .id("fullscreen")
                        .px_3()
                        .py_1()
                        .relative()
                        .min_h(px(theme::CONTROL_HEIGHT))
                        .flex()
                        .items_center()
                        .child(theme::surface(
                            rgb(palette.panel),
                            None,
                            theme::CONTROL_EXTENT,
                        ))
                        .cursor_pointer()
                        .child("全屏 · Ctrl+Alt+Enter")
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.video.read(cx).focus_handle(cx).focus(window, cx);
                            window.toggle_fullscreen();
                            cx.notify();
                        })),
                )
            })
            .when(!self.host && self.video_state.monitors.len() > 1, |root| {
                root.child(div().flex().flex_wrap().gap_2().child("屏幕：").children(
                    self.video_state.monitors.iter().map(|monitor| {
                        let index = monitor.index;
                        let selected = self.video_state.current_monitor == index;
                        let enabled = self.video_state.connected
                            && !self.video_state.switching_monitor
                            && monitor.available
                            && !selected;
                        div()
                            .id(("monitor", index))
                            .px_3()
                            .py_1()
                            .relative()
                            .min_h(px(theme::CONTROL_HEIGHT))
                            .flex()
                            .items_center()
                            .child(theme::surface(
                                rgb(if selected {
                                    palette.accent
                                } else {
                                    palette.panel
                                }),
                                None,
                                theme::CONTROL_EXTENT,
                            ))
                            .text_color(rgb(if selected {
                                palette.page
                            } else if enabled {
                                palette.text
                            } else {
                                palette.muted
                            }))
                            .child(format!(
                                "{} · {} · {}×{}{}",
                                index + 1,
                                monitor.name,
                                monitor.width,
                                monitor.height,
                                if monitor.available {
                                    ""
                                } else {
                                    " · 不可用"
                                }
                            ))
                            .when(enabled, |button| {
                                button.cursor_pointer().on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        view.video.update(cx, |video, cx| {
                                            video.switch_monitor(index, cx)
                                        });
                                        cx.notify();
                                    },
                                ))
                            })
                    }),
                ))
            })
            .when(!self.host && self.video_state.has_session, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child("目标帧率：")
                        .children(
                            [
                                TargetFps::Fps30,
                                TargetFps::Fps60,
                                TargetFps::Fps90,
                                TargetFps::Fps120,
                            ]
                            .into_iter()
                            .map(|target| {
                                div()
                                    .id(("target-fps", target.value() as usize))
                                    .px_3()
                                    .py_1()
                                    .relative()
                                    .min_h(px(theme::CONTROL_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .child(theme::surface(
                                        rgb(if self.video_state.target_fps == target {
                                            palette.accent
                                        } else {
                                            palette.panel
                                        }),
                                        None,
                                        theme::CONTROL_EXTENT,
                                    ))
                                    .text_color(rgb(if self.video_state.target_fps == target {
                                        palette.page
                                    } else {
                                        palette.text
                                    }))
                                    .child(target.value().to_string())
                                    .when(self.video_state.connected, |button| {
                                        button.cursor_pointer().on_click(cx.listener(
                                            move |view, _, _, cx| {
                                                view.video.update(cx, |video, cx| {
                                                    video.request_fps(target, cx)
                                                });
                                                cx.notify();
                                            },
                                        ))
                                    })
                            }),
                        ),
                )
            })
            .when(
                !self.host && self.video_state.connected && !self.video_state.switching_monitor,
                |root| {
                    let quality = &self.video_state.quality;
                    let mut details = vec![format!(
                        "屏幕 {} · 目标 {} FPS",
                        self.video_state.current_monitor + 1,
                        self.video_state.target_fps.value()
                    )];
                    if let Some(fps) = quality.decoded_fps {
                        details.push(format!("解码 {} FPS", fps));
                    }
                    if let Some(codec) = &quality.codec {
                        details.push(format!("编码 {}", codec));
                    }
                    if let Some(rate) = &quality.receive_rate {
                        details.push(format!("接收吞吐 {}", rate));
                    }
                    if let Some(delay) = quality.round_trip_ms {
                        details.push(format!("网络往返 {} ms", delay));
                    }
                    root.child(
                        div()
                            .text_color(rgb(palette.muted))
                            .child(details.join(" · ")),
                    )
                },
            )
            .when(
                self.video_state.connecting || self.video_state.has_session,
                |root| {
                    root.child(
                        div()
                            .id("disconnect")
                            .px_3()
                            .py_1()
                            .relative()
                            .min_h(px(theme::CONTROL_HEIGHT))
                            .flex()
                            .items_center()
                            .child(theme::surface(
                                rgb(palette.panel),
                                None,
                                theme::CONTROL_EXTENT,
                            ))
                            .text_color(rgb(palette.danger))
                            .cursor_pointer()
                            .child(if self.video_state.connecting {
                                "取消连接"
                            } else {
                                "断开连接"
                            })
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.video.update(cx, |video, cx| video.disconnect(cx));
                                cx.notify();
                            })),
                    )
                },
            )
    }
}

struct Client {
    font: gpui::SharedString,
    host: bool,
    management: Entity<ManagementPanel>,
    video: Entity<RemoteVideo>,
    _subscription: Subscription,
}

impl Render for Client {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(window.appearance());
        // Cached views require a definite viewport; panel content scrolls within it.
        let panel_width = (f32::from(window.viewport_size().width) * 0.38).min(420.0);
        let fullscreen_video = !self.host
            && self.management.read(cx).video_state.has_session
            && window.is_fullscreen();
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(!fullscreen_video, |root| root.gap_3().p_4())
            .bg(rgb(palette.page))
            .text_color(rgb(palette.text))
            .font_family(self.font.clone())
            .text_size(px(theme::BODY_SIZE))
            .line_height(px(theme::LINE_HEIGHT))
            .when(!fullscreen_video, |root| {
                root.child(
                    self.management
                        .update(cx, |view, cx| view.render_menu(window, cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap_3()
                    .when(!fullscreen_video, |row| {
                        row.child(
                            div()
                                .h_full()
                                .flex_shrink_0()
                                .when(!self.host, |panel| panel.w(px(panel_width)))
                                .when(self.host, |panel| panel.w_full())
                                .child(
                                    self.management
                                        .clone()
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                        )
                    })
                    .when(!self.host, |root| {
                        root.child(
                            div()
                                .flex_1()
                                .h_full()
                                .min_w(px(0.0))
                                .child(self.video.clone()),
                        )
                    }),
            )
    }
}

pub fn run(runtime: &tokio::runtime::Runtime) -> Result<(), Box<dyn std::error::Error>> {
    let initialization = gpui_host::initialize();
    let initialized = initialization.is_ok();
    let initial_notice = initialization.err().unwrap_or_default();
    let startup_error = Arc::new(Mutex::new(None::<String>));
    let app_error = startup_error.clone();
    let handle = runtime.handle().clone();
    let host = cfg!(feature = "native-host");
    #[cfg(all(
        target_os = "windows",
        feature = "native-host",
        feature = "test-lan-direct"
    ))]
    let test_host = Arc::new(Mutex::new(crate::test_host::TestHost::default()));
    gpui::application().run(move |cx: &mut App| {
        #[cfg(all(
            target_os = "windows",
            feature = "native-host",
            feature = "test-lan-direct"
        ))]
        {
            let test_host = test_host.clone();
            cx.on_app_quit(move |_| {
                test_host.lock().unwrap().stop();
                async {}
            })
            .detach();
        }
        text_field::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1100.0), px(850.0)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(if host { 440.0 } else { 800.0 }), px(480.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(
                        if host {
                            "XSCC 被控端 beta"
                        } else {
                            "XSCC 控制端 beta"
                        }
                        .into(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                window
                    .observe_window_appearance(|window, _| window.refresh())
                    .detach();
                let video = cx.new(|cx| RemoteVideo::new(handle.clone(), window, cx));
                let video_state = video.read(cx).state();
                let font: gpui::SharedString = if window
                    .text_system()
                    .all_font_names()
                    .iter()
                    .any(|name| name == "Sarmg Maple")
                {
                    "Sarmg Maple"
                } else if cfg!(target_os = "macos") {
                    "Menlo"
                } else if cfg!(target_os = "linux") {
                    "monospace"
                } else {
                    "Consolas"
                }
                .into();
                let management = cx.new(|cx| {
                    let (sender, receiver) = mpsc::channel();
                    let subscriptions = vec![cx.subscribe(
                        &video,
                        |view: &mut ManagementPanel, _, state: &VideoState, cx| {
                            view.video_state = state.clone();
                            cx.notify();
                        },
                    )];
                    let wake = Arc::new(tokio::sync::Notify::new());
                    let ui_wake = wake.clone();
                    let update_task = cx.spawn(async move |view, cx| {
                        loop {
                            ui_wake.notified().await;
                            if view
                                .update(cx, |view: &mut ManagementPanel, cx| {
                                    view.consume_updates();
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                    let management_task = cx.spawn(async move |view, cx| {
                        loop {
                            if view
                                .update(cx, |view: &mut ManagementPanel, cx| {
                                    view.refresh();
                                    #[cfg(all(
                                        target_os = "windows",
                                        feature = "native-host",
                                        feature = "test-lan-direct"
                                    ))]
                                    view.test_host.lock().unwrap().poll();
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                            cx.background_executor().timer(Duration::from_secs(2)).await;
                        }
                    });
                    ManagementPanel {
                        runtime: handle,
                        sender,
                        receiver,
                        wake,
                        server: cx.new(|cx| TextField::new("HTTPS 服务端地址", false, cx)),
                        password: cx.new(|cx| TextField::new("实例密码", true, cx)),
                        address_host: cx
                            .new(|cx| TextField::new("新的域名或 IP（不含端口）", false, cx)),
                        address_port: cx.new(|cx| TextField::new("端口（默认 21118）", false, cx)),
                        #[cfg(feature = "test-lan-direct")]
                        lan_endpoint: cx.new(|cx| {
                            TextField::new("IPv4 或 IPv6，带端口如 [::1]:21119", false, cx)
                        }),
                        #[cfg(all(
                            target_os = "windows",
                            feature = "native-host",
                            feature = "test-lan-direct"
                        ))]
                        test_host: test_host.clone(),
                        host,
                        initialized,
                        status: None,
                        status_pending: false,
                        status_error: None,
                        action_pending: false,
                        confirm_unbind: false,
                        show_login: false,
                        notice: initial_notice,
                        video: video.clone(),
                        video_state,
                        _subscriptions: subscriptions,
                        _tasks: vec![update_task, management_task],
                    }
                });
                cx.new(|cx| Client {
                    font,
                    host,
                    _subscription: cx.observe(&management, |_, _, cx| cx.notify()),
                    management,
                    video,
                })
            },
        );
        if let Err(error) = result {
            *app_error.lock().unwrap() = Some(format!("GPUI window failed: {error}"));
            cx.quit();
        }
        cx.activate(true);
    });
    if let Some(error) = startup_error.lock().unwrap().take() {
        return Err(error.into());
    }
    Ok(())
}

fn format_peer_address(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn management_actions_enabled(
    initialized: bool,
    status_pending: bool,
    action_pending: bool,
) -> bool {
    initialized && !status_pending && !action_pending
}

fn management_status(configured: bool, phase: &str) -> &'static str {
    if !configured {
        return "尚未绑定实例";
    }
    match phase {
        "online" => "已连接服务端",
        "connecting" => "正在连接服务端…",
        "revoked" => "实例授权已撤销，请联系管理员或重新绑定。",
        "error" => "服务端连接失败，请检查网络并重新连接。",
        _ => "服务端未连接，可尝试重新连接。",
    }
}

#[cfg(test)]
mod tests {
    use super::{format_peer_address, management_actions_enabled, management_status};

    #[test]
    fn host_cards_display_addresses_with_ports() {
        assert_eq!(
            format_peer_address("192.168.1.8", 21118),
            "192.168.1.8:21118"
        );
        assert_eq!(format_peer_address("host.local", 443), "host.local:443");
        assert_eq!(
            format_peer_address("2001:db8::1", 21118),
            "[2001:db8::1]:21118"
        );
    }

    #[test]
    fn management_actions_wait_for_refresh_before_changing_binding_or_addresses() {
        assert!(management_actions_enabled(true, false, false));
        assert!(!management_actions_enabled(true, true, false));
        assert!(!management_actions_enabled(true, false, true));
        assert!(!management_actions_enabled(false, false, false));
    }

    #[test]
    fn binding_status_offers_recovery_instead_of_internal_phase_names() {
        assert_eq!(management_status(false, "offline"), "尚未绑定实例");
        assert!(management_status(true, "revoked").contains("授权已撤销"));
        assert!(management_status(true, "error").contains("重新连接"));
        assert!(management_status(true, "unexpected").contains("重新连接"));
    }
}
