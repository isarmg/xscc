#[path = "enrollment_input.rs"]
mod enrollment_input;

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
use xscc_native_management::{enrollment::ServerOrigin, manager::Status};

pub struct Panel {
    commands: tokio::sync::mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    lifecycle: tokio::sync::watch::Sender<bool>,
    server: Entity<TextField>,
    code: Entity<TextField>,
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
    #[cfg(all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    ))]
    _session_events: Subscription,
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
        let updates = cx.spawn_in(_window, async move |view, cx| {
            loop {
                wake.notified().await;
                if view
                    .update_in(cx, |view: &mut Self, window, cx| {
                        view.consume(window, cx);
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
        #[cfg(all(
            any(target_os = "ios", target_os = "android"),
            feature = "mobile-session"
        ))]
        let session =
            cx.new(|cx| crate::session_view::SessionView::new(runtime.clone(), _window, cx));
        #[cfg(all(
            any(target_os = "ios", target_os = "android"),
            feature = "mobile-session"
        ))]
        let session_events = cx.subscribe_in(&session, _window, |view, _, event, _, cx| {
            match event {
                crate::session_view::SessionUiEvent::BackToDevices => view.session_visible = false,
            }
            cx.notify();
        });
        Self {
            commands,
            events: incoming,
            lifecycle: lifecycle_sender,
            server: cx.new(|cx| TextField::new("服务端地址（本机可用 HTTP）", false, cx)),
            code: cx.new(|cx| TextField::new("控制端实例密码", true, cx)),
            status: None,
            initialized: false,
            active,
            busy: false,
            confirm_unbind: false,
            binding_expanded: true,
            session_visible: false,
            notice: "正在读取本机绑定…".into(),
            #[cfg(all(
                any(target_os = "ios", target_os = "android"),
                feature = "mobile-session"
            ))]
            session,
            #[cfg(all(
                any(target_os = "ios", target_os = "android"),
                feature = "mobile-session"
            ))]
            _session_events: session_events,
            _updates: updates,
            _lifecycle: lifecycle,
        }
    }

    fn consume(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
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
                Event::Enrolled(result) => {
                    self.busy = false;
                    if result.is_ok() {
                        self.code.update(_cx, |field, cx| field.clear(window, cx));
                    }
                    self.notice = result.err().unwrap_or_default();
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
        if server.is_empty() {
            self.notice = "请填写服务端地址。".into();
        } else if ServerOrigin::parse(&server).is_err() {
            self.notice = "服务端地址无效：HTTP 仅支持本机或私网地址；公网请使用 HTTPS，地址不能包含路径或账号。".into();
        } else {
            let code = match enrollment_input::instance_password(self.code.read(cx).value()) {
                Ok(code) => code.to_owned(),
                Err(message) => {
                    self.notice = message;
                    cx.notify();
                    return;
                }
            };
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
                        .child(self.session.clone()),
                );
        }
        let enabled = self.enabled();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .relative()
                .min_h(px(theme::CONTROL_HEIGHT))
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(if primary {
                    palette.on_accent
                } else {
                    palette.text
                }))
                .opacity(if enabled { 1. } else { 0.5 })
                .child(theme::surface(
                    rgb(if primary {
                        palette.accent
                    } else {
                        palette.panel
                    }),
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
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("XSCC"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(palette.muted))
                                    .child("设备工作台"),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-server-binding")
                            .relative()
                            .min_h(px(44.))
                            .px_3()
                            .py_2()
                            .child(theme::surface(
                                rgb(palette.accent_soft),
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
                    button("enroll", "绑定 / 重新授权", true)
                        .on_click(cx.listener(|view, _, window, cx| view.enroll(window, cx))),
                )
                .child(
                    button("resume", "重新连接服务端", false).on_click(cx.listener(
                        |view, _, window, cx| {
                            window.dismiss_virtual_keyboard();
                            view.action(Command::Resume, cx);
                            cx.notify();
                        },
                    )),
                );
            if let Some(status) = &self.status {
                let state = match status.phase.as_str() {
                    "online" => "在线",
                    "offline" => "离线",
                    _ => status.phase.as_str(),
                };
                content = content.child(
                    div()
                        .text_color(rgb(if status.phase == "online" {
                            palette.success
                        } else {
                            palette.muted
                        }))
                        .child(format!("服务端：{state}")),
                );
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
                            false,
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
                            .child(button("cancel-unbind", "取消", false).on_click(cx.listener(
                                |view, _, _, cx| {
                                    view.confirm_unbind = false;
                                    cx.notify();
                                },
                            )));
                    }
                }
            }
        }
        content = content.child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("我的设备"),
        );
        let peers: Vec<_> = self
            .status
            .as_ref()
            .into_iter()
            .flat_map(|status| &status.peers)
            .filter(|peer| peer.role == InstanceRole::Controlled)
            .collect();
        if peers.is_empty() {
            content = content.child(
                div()
                    .text_color(rgb(palette.muted))
                    .child("绑定管理实例并添加访问授权后，设备会显示在这里。"),
            );
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
                .min_h(px(180.))
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
                    .update(cx, |session, cx| session.connect(id.clone(), window, cx));
                view.session_visible = true;
                cx.notify();
            }));
            content = content.child(card);
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
