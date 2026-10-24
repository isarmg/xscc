use crate::{
    dashboard as ui,
    remote_video::{RemoteVideo, VideoState},
    text_field::{self, TextField},
    theme,
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, KeyDownEvent, Render,
    StyleRefinement, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    point, prelude::*, px, rgb, size,
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

    fn form_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
        let online = self.status_error.is_none()
            && self
                .status
                .as_ref()
                .is_some_and(|status| status.phase == "online");
        let status_text = if self.status_error.is_some() {
            "状态待同步"
        } else {
            self.status
                .as_ref()
                .map(|status| management_status(status.configured, &status.phase))
                .unwrap_or(if self.initialized {
                    "正在读取状态"
                } else {
                    "本机服务不可用"
                })
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_4()
            .pb_4()
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(44.))
                            .rounded_xl()
                            .bg(rgb(palette.accent))
                            .text_color(rgb(palette.on_accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(19.))
                            .child("X"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(if self.host {
                                        "XSCC 被控端"
                                    } else {
                                        "XSCC 控制端"
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.muted))
                                    .child(if self.host {
                                        "管理这台设备的远程访问"
                                    } else {
                                        "你的设备，随时连接"
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(ui::badge(
                        status_text,
                        if online {
                            palette.success
                        } else {
                            palette.muted
                        },
                        palette.panel,
                    ))
                    .when(!configured, |row| {
                        row.child(
                            ui::button(
                                "login-menu",
                                if self.show_login {
                                    "收起绑定"
                                } else {
                                    "绑定实例"
                                },
                                true,
                                self.initialized,
                                palette,
                            )
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.initialized {
                                    view.show_login = !view.show_login;
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .child(
                        ui::button(
                            "refresh",
                            if self.status_pending {
                                "刷新中…"
                            } else {
                                "刷新"
                            },
                            false,
                            idle,
                            palette,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.refresh();
                            cx.notify();
                        })),
                    )
                    .when(
                        configured
                            && self.status.as_ref().is_some_and(|status| {
                                !matches!(status.phase.as_str(), "online" | "connecting")
                            }),
                        |row| {
                            row.child(
                                ui::button("resume", "重新连接", true, idle, palette).on_click(
                                    cx.listener(|view, _, _, cx| {
                                        view.action(Command::Resume);
                                        cx.notify();
                                    }),
                                ),
                            )
                        },
                    )
                    .when(configured, |row| {
                        row.child(
                            ui::button(
                                "unbind",
                                if self.confirm_unbind {
                                    "确认解绑并断开"
                                } else {
                                    "解绑实例"
                                },
                                false,
                                idle,
                                palette,
                            )
                            .text_color(rgb(palette.danger))
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
                            ui::button("cancel-unbind", "取消", false, true, palette).on_click(
                                cx.listener(|view, _, _, cx| {
                                    view.confirm_unbind = false;
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
    }

    fn render_binding(&self, palette: theme::Palette, cx: &mut Context<Self>) -> gpui::Div {
        ui::card(palette)
            .child(ui::heading(
                "绑定管理实例",
                "输入管理端提供的信息，加载你的授权设备。",
                palette,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_color(rgb(palette.muted)).child("服务端地址"))
                    .child(self.server.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_color(rgb(palette.muted)).child(if self.host {
                        "被控端实例密码"
                    } else {
                        "控制端实例密码"
                    }))
                    .child(self.password.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        ui::button(
                            "enroll",
                            if self.action_pending {
                                "正在绑定…"
                            } else {
                                "绑定并连接"
                            },
                            true,
                            self.can_act(),
                            palette,
                        )
                        .on_click(cx.listener(|view, _, window, cx| view.enroll(window, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child("实例密码由管理端提供，仅用于绑定此设备。"),
                    ),
            )
    }

    fn render_addresses(
        &self,
        current: &str,
        palette: theme::Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        ui::card(palette)
            .child(ui::heading(
                "设备连接地址",
                "向已授权的控制端上报可访问地址，留空则自动获取。",
                palette,
            ))
            .child(
                div()
                    .rounded_lg()
                    .p_3()
                    .bg(rgb(palette.page))
                    .text_color(rgb(palette.muted))
                    .child(format!("当前地址：{current}")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(170.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("域名或 IP")
                            .child(self.address_host.clone()),
                    )
                    .child(
                        div()
                            .w(px(160.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("端口")
                            .child(self.address_port.clone()),
                    ),
            )
            .when(configured, |section| {
                section.child(
                    ui::button(
                        "save-address",
                        if self.action_pending {
                            "正在保存…"
                        } else {
                            "保存地址"
                        },
                        false,
                        self.can_act(),
                        palette,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.save_address(cx))),
                )
            })
    }
}

impl Render for ManagementPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(window.appearance());
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
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
                    "自动获取本机地址".into()
                } else {
                    status
                        .addresses
                        .iter()
                        .map(|address| format_peer_address(&address.host, address.port))
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
        let current_peers = self.status_error.is_none()
            && self
                .status
                .as_ref()
                .is_some_and(|status| status.phase == "online");
        let panel_width = if !self.host && self.video_state.connected {
            (f32::from(window.viewport_size().width) * 0.38).min(420.)
        } else {
            f32::from(window.viewport_size().width) - 48.
        };
        let content_width = (panel_width - 32.).max(1.);
        let can_connect = current_peers
            && correct_role
            && !self.host
            && !self.video_state.connecting
            && !self.video_state.has_session
            && !self.action_pending;
        let online_count = if current_peers {
            peers.iter().filter(|peer| peer.online).count().to_string()
        } else {
            "—".into()
        };
        let panel = div().id("management-panel").size_full().overflow_y_scroll()
            .flex().flex_col().gap_5().p_4()
            .bg(rgb(palette.page)).text_color(rgb(palette.text))
            .text_size(px(theme::BODY_SIZE)).line_height(px(theme::LINE_HEIGHT))
            .on_key_down(cx.listener(Self::form_key_down))
            .child(ui::heading(
                if self.host { "访问工作台" } else { "设备工作台" },
                if self.host { "查看本机连接信息，管理获准访问的设备。" } else { "在一个地方查看并连接你的授权设备。" }, palette))
            .when(!self.notice.is_empty(), |root| root.child(
                div().p_3().rounded_lg().bg(rgb(palette.accent_soft)).child(self.notice.clone())))
            .when_some(self.status_error.clone(), |root, error| root.child(
                ui::card(palette).gap_2().text_color(rgb(palette.danger))
                    .child("设备状态暂不可用")
                    .child(div().text_size(px(13.)).child(format!("列表可能已过期，请刷新或检查本机服务。{error}")))))
            .when(configured && self.status.as_ref().is_some_and(|status| status.role.is_some()) && !correct_role, |root| root.child(
                ui::card(palette).text_color(rgb(palette.danger))
                    .child("此实例角色与当前应用不匹配。请在管理端创建对应角色的实例，再解绑并重新绑定。")))
            .when(configured && !self.video_state.has_session, |root| root.child(
                div().flex_shrink_0().grid().grid_cols(if content_width < 560. { 1 } else { 3 }).gap_3()
                    .child(ui::metric(if self.host { "授权控制端" } else { "授权设备" }, if configured { peers.len().to_string() } else { "—".into() }, "由管理端分配访问权限", palette))
                    .child(ui::metric("当前在线", online_count, if current_peers { "已同步最新设备状态" } else { "连接管理端后同步" }, palette))
                    .child(ui::metric("管理连接", if current_peers { "已连接" } else if configured { "待连接" } else { "未绑定" }.into(), "使用实例授权建立连接", palette))))
            .when(!configured && self.show_login && self.initialized && self.status.is_some(), |root| root.child(self.render_binding(palette, cx)))
            .when(configured, |root| {
                let status = self.status.as_ref();
                root.child(ui::card(palette).gap_2()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("当前管理实例"))
                    .when_some(status.and_then(|status| status.server.as_ref()), |card, server| {
                        card.child(div().text_color(rgb(palette.muted)).overflow_hidden().child(server.clone()))
                    })
                    .when_some(status.and_then(|status| status.binding.as_ref()), |card, binding| {
                        card.child(div().text_size(px(12.)).text_color(rgb(palette.muted)).child(format!("实例编号：{}", binding.device_id)))
                    }))
            })
            .when(self.host && self.initialized, |root| root.child(self.render_addresses(&current_addresses, palette, cx)))
            .child(div().flex().items_center().justify_between().gap_2()
                .child(div().text_size(px(18.)).font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if self.host { "允许访问的控制端" } else { "我的设备" }))
                .child(div().text_size(px(12.)).text_color(rgb(palette.muted)).child(format!("{} 台设备", peers.len()))))
            .when(peers.is_empty(), |root| {
                let (title, description) = if !configured {
                    ("从第一台设备开始", "绑定管理实例后，获准连接的设备会显示在这里。")
                } else if !correct_role {
                    ("需要检查实例角色", "请确认此实例属于当前控制端或被控端。")
                } else if !current_peers {
                    ("等待设备状态同步", "连接管理端后即可获取最新授权列表。")
                } else {
                    ("暂无授权设备", "请在管理端为此实例添加访问授权。")
                };
                root.child(ui::card(palette).items_center().justify_center().py_8().gap_3()
                    .child(ui::monitor(palette))
                    .child(div().text_size(px(17.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                    .child(div().text_color(rgb(palette.muted)).child(description))
                    .when(!configured && !self.show_login, |empty| empty.child(
                        ui::button("empty-bind", "绑定管理实例", true, self.initialized, palette)
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.initialized { view.show_login = true; cx.notify(); }
                            })))))
            })
            .child(div().id("peers").flex_shrink_0().grid().grid_cols(crate::window_layout::card_columns(content_width)).gap_4()
                .children(peers.iter().map(|peer| {
                    let id = peer.binding.device_id.to_string();
                    let available = can_connect && peer.online && matches!(peer.role, InstanceRole::Controlled);
                    let address = if peer.addresses.is_empty() { "等待设备上报地址".into() } else {
                        peer.addresses.iter().map(|address| format_peer_address(&address.host, address.port)).collect::<Vec<_>>().join("，")
                    };
                    ui::card(palette).id(gpui::SharedString::from(id.clone())).gap_3()
                        .child(div().flex().items_center().justify_between().gap_2()
                            .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::SEMIBOLD).text_size(px(16.)).child(peer.name.clone()))
                            .child(ui::badge(crate::window_layout::peer_status(current_peers.then_some(peer.online)), if current_peers && peer.online { palette.success } else { palette.muted }, palette.page)))
                        .child(div().h(px(96.)).rounded_lg().bg(rgb(palette.page)).flex().items_center().justify_center().child(ui::monitor(palette)))
                        .child(div().text_size(px(12.)).text_color(rgb(palette.muted)).overflow_hidden().child(address))
                        .when(!self.host, |card| card.child(
                            div().relative().min_h(px(theme::CONTROL_HEIGHT)).flex().items_center().justify_between().px_3().py_2()
                                .child(theme::surface(rgb(if available { palette.accent_soft } else { palette.page }), None, theme::CONTROL_EXTENT))
                                .text_color(rgb(if available { palette.accent } else { palette.muted }))
                                .child(if available { "连接设备" } else if self.video_state.connecting { "正在连接…" } else if self.video_state.has_session { "会话进行中" } else if !current_peers { "等待状态同步" } else { "暂不可连接" })
                                .child(if available { "→" } else { "" })))
                        .when(available, |card| card.cursor_pointer().hover(|style| style.opacity(0.9))
                            .on_click(cx.listener(move |view, _, window, cx| { view.connect(id.clone(), window, cx); cx.notify(); })))
                })));
        panel
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
        let remote_visible = !self.host && self.management.read(cx).video_state.connected;
        let fullscreen_video = remote_visible && window.is_fullscreen();
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(!fullscreen_video, |root| root.gap_4().p_6())
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
                                .when(remote_visible, |panel| panel.w(px(panel_width)))
                                .when(!remote_visible, |panel| panel.w_full())
                                .child(
                                    self.management
                                        .clone()
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                        )
                    })
                    .when(remote_visible, |root| {
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
    gpui::application().run(move |cx: &mut App| {
        text_field::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let work_area = cx
            .primary_display()
            .map(|display| display.visible_bounds())
            .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1024.), px(768.))));
        let (initial, minimum) = crate::window_layout::window_sizes(
            [
                f32::from(work_area.size.width),
                f32::from(work_area.size.height),
            ],
            host,
        );
        let bounds = Bounds::new(
            point(
                work_area.center().x - px(initial[0] / 2.),
                work_area.center().y - px(initial[1] / 2.),
            ),
            size(px(initial[0]), px(initial[1])),
        );
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(minimum[0]), px(minimum[1]))),
                is_resizable: true,
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
                let fonts = window.text_system().all_font_names();
                let preferred = if cfg!(target_os = "windows") {
                    ["Microsoft YaHei UI", "Segoe UI", "Arial"]
                } else if cfg!(target_os = "macos") {
                    ["PingFang SC", "Helvetica Neue", "Arial"]
                } else {
                    ["Noto Sans CJK SC", "Noto Sans", "DejaVu Sans"]
                };
                let font: gpui::SharedString = preferred
                    .into_iter()
                    .find(|candidate| fonts.iter().any(|name| name.as_str() == *candidate))
                    .unwrap_or("sans-serif")
                    .into();
                let management = cx.new(|cx| {
                    let (sender, receiver) = mpsc::channel();
                    let subscriptions = vec![cx.subscribe_in(
                        &video,
                        window,
                        |view: &mut ManagementPanel, _, state: &VideoState, window, cx| {
                            if view.video_state.connected
                                && !state.connected
                                && window.is_fullscreen()
                            {
                                window.toggle_fullscreen();
                            }
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
                        host,
                        initialized,
                        status: None,
                        status_pending: false,
                        status_error: None,
                        action_pending: false,
                        confirm_unbind: false,
                        show_login: true,
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
