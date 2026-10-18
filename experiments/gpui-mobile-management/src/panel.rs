use crate::{
    management::{self, Command, Event},
    text_field::TextField,
    theme,
};
use gpui::{
    AppContext, AppLifecyclePhase, Context, Entity, IntoElement, Render, Subscription, Window, div,
    prelude::*, px, rgb,
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};
use xscc::protocol::InstanceRole;
use xscc_native_management::manager::Status;

#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "test-lan-direct"
))]
const TEST_LAN_WARNING: &str =
    "TEST ONLY · 临时 IP 直连，无身份认证；任何可达设备均可控制主机。正式发布删除。";

pub struct Panel {
    commands: tokio::sync::mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    lifecycle: tokio::sync::watch::Sender<bool>,
    server: Entity<TextField>,
    code: Entity<TextField>,
    #[cfg(all(
        any(target_os = "ios", target_os = "android"),
        feature = "test-lan-direct"
    ))]
    test_lan_address: Entity<TextField>,
    status: Option<Status>,
    initialized: bool,
    active: bool,
    busy: bool,
    confirm_unbind: bool,
    binding_expanded: bool,
    session_visible: bool,
    notice: String,
    #[cfg(all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    ))]
    session: Entity<crate::session_view::SessionView>,
    _updates: gpui::Task<()>,
    _lifecycle: Subscription,
}

impl Panel {
    pub fn new(
        directory: Result<PathBuf, String>,
        runtime: tokio::runtime::Handle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        #[cfg(target_os = "android")]
        let active =
            _window.is_window_active() && _window.visibility() == gpui::WindowVisibility::Visible;
        #[cfg(not(target_os = "android"))]
        let active = true;
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let (lifecycle_sender, lifecycle_receiver) = tokio::sync::watch::channel(active);
        let (events, incoming) = mpsc::channel();
        let wake = Arc::new(tokio::sync::Notify::new());
        runtime.spawn(management::run(
            directory,
            receiver,
            lifecycle_receiver,
            events,
            wake.clone(),
        ));
        let updates = cx.spawn(async move |view, cx| {
            loop {
                wake.notified().await;
                if view
                    .update(cx, |view: &mut Self, cx| {
                        view.consume(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let view = cx.entity().downgrade();
        let lifecycle = cx.on_app_lifecycle(move |phase, cx| {
            // The observer is owned by the view; a disappearing view needs no update.
            let _ = view.update(cx, |view, cx| {
                match phase {
                    AppLifecyclePhase::Background | AppLifecyclePhase::Disconnected => {
                        view.active = false;
                        view.confirm_unbind = false;
                        view.session_visible = false;
                        if let Some(status) = &mut view.status {
                            status.peers.clear();
                        }
                        view.lifecycle.send_replace(false);
                    }
                    AppLifecyclePhase::Active => {
                        view.active = true;
                        view.lifecycle.send_replace(true);
                    }
                    _ => {}
                }
                cx.notify();
            });
        });
        Self {
            commands,
            events: incoming,
            lifecycle: lifecycle_sender,
            server: cx.new(|cx| TextField::new("https://服务端地址", false, cx)),
            code: cx.new(|cx| TextField::new("控制端实例密码", true, cx)),
            #[cfg(all(
                any(target_os = "ios", target_os = "android"),
                feature = "test-lan-direct"
            ))]
            test_lan_address: cx.new(|cx| {
                TextField::new(
                    "测试主机 IP 或 IP:端口；IPv6 带端口时使用 [地址]:端口",
                    false,
                    cx,
                )
            }),
            status: None,
            initialized: false,
            active,
            busy: false,
            confirm_unbind: false,
            binding_expanded: false,
            session_visible: false,
            notice: "正在读取本机绑定…".into(),
            #[cfg(all(
                any(target_os = "ios", target_os = "android"),
                feature = "mobile-session"
            ))]
            session: cx
                .new(|cx| crate::session_view::SessionView::new(runtime.clone(), _window, cx)),
            _updates: updates,
            _lifecycle: lifecycle,
        }
    }

    fn consume(&mut self, _cx: &mut Context<Self>) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                #[cfg(all(
                    any(target_os = "ios", target_os = "android"),
                    feature = "mobile-session"
                ))]
                Event::SessionReady(manager) => {
                    self.session.update(_cx, |view, _| view.attach(manager))
                }
                Event::Opened(status) => {
                    self.initialized = true;
                    self.status = Some(status);
                    self.notice.clear();
                }
                Event::Status(mut status) => {
                    if !self.active {
                        status.peers.clear();
                    }
                    self.status = Some(status);
                    #[cfg(all(
                        any(target_os = "ios", target_os = "android"),
                        feature = "mobile-session"
                    ))]
                    self.session
                        .update(_cx, |view, cx| view.check_authorization(cx));
                }
                Event::Action(result) => {
                    self.busy = false;
                    self.notice = result.err().unwrap_or_default();
                }
                Event::Lifecycle(result) => {
                    if let Err(error) = result {
                        self.notice = error;
                    }
                }
                Event::Failed(error) => {
                    self.initialized = false;
                    self.busy = false;
                    self.status = None;
                    self.notice = error;
                }
            }
        }
    }

    fn enabled(&self) -> bool {
        self.initialized && self.active && !self.busy
    }

    fn action(&mut self, command: Command, _cx: &mut Context<Self>) {
        if !self.enabled() {
            return;
        }
        #[cfg(all(
            any(target_os = "ios", target_os = "android"),
            feature = "mobile-session"
        ))]
        self.session.update(_cx, |view, cx| {
            view.stop();
            cx.notify();
        });
        match self.commands.try_send(command) {
            Ok(()) => {
                self.busy = true;
                self.confirm_unbind = false;
                self.notice = "正在处理…".into();
            }
            Err(error) => self.notice = format!("管理操作未提交：{error}"),
        }
    }

    fn enroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled()
            || self.code.read(cx).is_composing()
            || self.server.read(cx).is_composing()
        {
            return;
        }
        let server = self.server.read(cx).value().trim().to_owned();
        let code = self.code.read(cx).value().to_owned();
        if server.is_empty() || code.is_empty() {
            self.notice = "请填写 HTTPS 服务端地址和控制端实例密码。".into();
        } else {
            self.code.update(cx, |field, cx| field.clear(window, cx));
            window.dismiss_virtual_keyboard();
            self.action(Command::Enroll { server, code }, cx);
        }
        cx.notify();
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(window.appearance());
        let visible = window.fully_visible_bounds();
        #[cfg(all(
            any(target_os = "ios", target_os = "android"),
            feature = "mobile-session"
        ))]
        if self.session_visible {
            return div()
                .size_full()
                .bg(rgb(palette.page))
                .text_color(rgb(palette.text))
                .child(
                    div()
                        .absolute()
                        .left(visible.origin.x)
                        .top(visible.origin.y)
                        .w(visible.size.width)
                        .h(visible.size.height)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("back-to-hosts")
                                .min_h(px(44.))
                                .px_4()
                                .py_2()
                                .child("‹ 返回主机列表")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    window.dismiss_virtual_keyboard();
                                    view.session
                                        .update(cx, |session, cx| session.leave(window, cx));
                                    view.session_visible = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .child(self.session.clone()),
                        ),
                );
        }
        let enabled = self.enabled();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .relative()
                .min_h(px(theme::CONTROL_HEIGHT))
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .justify_center()
                .opacity(if enabled { 1. } else { 0.5 })
                .child(theme::surface(
                    rgb(palette.panel),
                    Some(rgb(palette.border)),
                    theme::CONTROL_EXTENT,
                ))
                .child(label)
        };
        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .max_w(px(720.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_2xl().child("XSCC"))
                    .child(
                        div()
                            .id("toggle-server-binding")
                            .relative()
                            .min_h(px(44.))
                            .px_3()
                            .py_2()
                            .child(theme::surface(
                                rgb(palette.panel),
                                Some(rgb(palette.border)),
                                theme::CONTROL_EXTENT,
                            ))
                            .child(if self.binding_expanded {
                                "收起服务器"
                            } else {
                                "绑定服务器"
                            })
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.binding_expanded = !view.binding_expanded;
                                view.confirm_unbind = false;
                                window.dismiss_virtual_keyboard();
                                cx.notify();
                            })),
                    ),
            );
        if self.binding_expanded {
            content = content
                .child(self.server.clone())
                .child(self.code.clone())
                .child(
                    button("enroll", "绑定 / 重新授权")
                        .on_click(cx.listener(|view, _, window, cx| view.enroll(window, cx))),
                )
                .child(button("resume", "重新连接服务端").on_click(cx.listener(
                    |view, _, window, cx| {
                        window.dismiss_virtual_keyboard();
                        view.action(Command::Resume, cx);
                        cx.notify();
                    },
                )));
            if let Some(status) = &self.status {
                let state = match status.phase.as_str() {
                    "online" => "在线",
                    "offline" => "离线",
                    _ => status.phase.as_str(),
                };
                content = content.child(div().child(format!("服务端：{state}")));
                if let Some(server) = &status.server {
                    content = content.child(
                        div()
                            .text_color(rgb(palette.muted))
                            .child(format!("已保存服务端：{server}")),
                    );
                }
                if let Some(binding) = &status.binding {
                    content = content.child(
                        div()
                            .text_color(rgb(palette.muted))
                            .child(format!("本机实例：{}", binding.device_id)),
                    );
                }
                if status.configured {
                    content = content.child(
                        button(
                            "unbind",
                            if self.confirm_unbind {
                                "确认解除本机绑定"
                            } else {
                                "解除绑定"
                            },
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            if !view.enabled() {
                                return;
                            }
                            window.dismiss_virtual_keyboard();
                            if view.confirm_unbind {
                                view.action(Command::Unbind, cx);
                            } else {
                                view.confirm_unbind = true;
                            }
                            cx.notify();
                        })),
                    );
                    if self.confirm_unbind {
                        content = content
                            .child("解除本机绑定后，需要重新输入实例密码。")
                            .child(button("cancel-unbind", "取消").on_click(cx.listener(
                                |view, _, _, cx| {
                                    view.confirm_unbind = false;
                                    cx.notify();
                                },
                            )));
                    }
                }
            }
        }
        content = content.child(div().text_lg().child("主机"));
        let peers: Vec<_> = self
            .status
            .as_ref()
            .into_iter()
            .flat_map(|status| &status.peers)
            .filter(|peer| peer.role == InstanceRole::Controlled)
            .collect();
        if peers.is_empty() {
            content = content.child(div().text_color(rgb(palette.muted)).child("暂无授权主机"));
        }
        for peer in peers {
            let id = peer.binding.device_id.to_string();
            let connectable = peer.online && !peer.addresses.is_empty();
            let addresses = peer
                .addresses
                .iter()
                .map(|address| {
                    if address.host.contains(':') {
                        format!("[{}]:{}", address.host, address.port)
                    } else {
                        format!("{}:{}", address.host, address.port)
                    }
                })
                .collect::<Vec<_>>()
                .join("、");
            let card = div()
                .id(gpui::SharedString::from(format!("host-{id}")))
                .relative()
                .w_full()
                .aspect_ratio(3. / 2.)
                .p_4()
                .flex()
                .flex_col()
                .justify_between()
                .overflow_hidden()
                .child(theme::surface(
                    rgb(palette.panel),
                    Some(rgb(palette.border)),
                    theme::PANEL_EXTENT,
                ))
                .child(div().text_lg().child(peer.name.clone()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(if addresses.is_empty() {
                            "地址：未知".to_owned()
                        } else {
                            format!("地址：{addresses}")
                        })
                        .child("系统：未知")
                        .child(if peer.online {
                            "状态：在线"
                        } else {
                            "状态：离线"
                        }),
                )
                .child(div().text_color(rgb(palette.muted)).child(if connectable {
                    "点击连接"
                } else {
                    "暂不可连接"
                }));
            #[cfg(all(
                any(target_os = "ios", target_os = "android"),
                feature = "mobile-session"
            ))]
            let card = card.on_click(cx.listener(move |view, _, window, cx| {
                if !view.enabled() || !connectable {
                    return;
                }
                window.dismiss_virtual_keyboard();
                view.session
                    .update(cx, |session, cx| session.connect(id.clone(), cx));
                view.session_visible = true;
                cx.notify();
            }));
            content = content.child(card);
        }
        #[cfg(all(
            any(target_os = "ios", target_os = "android"),
            feature = "test-lan-direct"
        ))]
        {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(palette.danger))
                            .child(TEST_LAN_WARNING),
                    )
                    .child(self.test_lan_address.clone())
                    .child(
                        div()
                            .id("XSCC_MOBILE_TEST_IP_DIRECT_V2")
                            .relative()
                            .min_h(px(44.))
                            .p_3()
                            .child(theme::surface(
                                rgb(palette.panel),
                                Some(rgb(palette.border)),
                                theme::CONTROL_EXTENT,
                            ))
                            .opacity(if self.active { 1. } else { 0.5 })
                            .child("连接输入的 IP（测试）")
                            .on_click(cx.listener(|view, _, window, cx| {
                                if !view.active || view.test_lan_address.read(cx).is_composing() {
                                    return;
                                }
                                let endpoint =
                                    view.test_lan_address.read(cx).value().trim().to_owned();
                                window.dismiss_virtual_keyboard();
                                view.session.update(cx, |session, cx| {
                                    session.connect_test_lan(endpoint, cx)
                                });
                                view.session_visible = true;
                                cx.notify();
                            })),
                    ),
            );
        }
        if !self.notice.is_empty() {
            content = content.child(
                div()
                    .text_color(rgb(palette.danger))
                    .child(self.notice.clone()),
            );
        }
        div()
            .size_full()
            .bg(rgb(palette.page))
            .text_color(rgb(palette.text))
            .text_size(px(theme::BODY_SIZE))
            .line_height(px(theme::LINE_HEIGHT))
            .child(
                div()
                    .absolute()
                    .left(visible.origin.x)
                    .top(visible.origin.y)
                    .w(visible.size.width)
                    .h(visible.size.height)
                    .id("mobile-management-scroll")
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(content),
            )
    }
}
