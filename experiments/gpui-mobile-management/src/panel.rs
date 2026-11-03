#[path = "enrollment_input.rs"]
mod enrollment_input;
#[path = "../../gpui-validation/src/dashboard.rs"]
mod ui;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnbindConfirmation {
    Remote,
    Local,
}

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
    confirm_unbind: Option<UnbindConfirmation>,
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
                        view.confirm_unbind = None;
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
            server: cx.new(|cx| TextField::new("服务端地址", false, cx)),
            code: cx.new(|cx| TextField::new("控制端实例密码", true, cx)),
            status: None,
            initialized: false,
            active,
            busy: false,
            confirm_unbind: None,
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
                    self.binding_expanded = !status.configured;
                    self.status = Some(status);
                    self.notice.clear();
                }
                Event::Status(mut status) => {
                    if !status.configured
                        && self
                            .status
                            .as_ref()
                            .is_some_and(|previous| previous.configured)
                    {
                        self.binding_expanded = true;
                        self.confirm_unbind = None;
                    }
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
                        self.binding_expanded = false;
                        window.dismiss_virtual_keyboard();
                    }
                    self.notice = result.err().unwrap_or_default();
                }
                Event::Action(result) => {
                    self.busy = false;
                    self.notice = result.err().unwrap_or_else(|| "操作完成。".into());
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
        if !matches!(&command, Command::Refresh) {
            self.session.update(_cx, |view, cx| {
                view.stop();
                cx.notify();
            });
        }
        let pending = match &command {
            Command::Enroll { .. } => "正在绑定实例…",
            Command::Resume => "正在重新连接…",
            Command::Refresh => "正在刷新设备状态…",
            Command::Unbind => "正在解绑实例…",
            Command::ForgetLocal => "正在清除此机绑定…",
        };
        match self.commands.try_send(command) {
            Ok(()) => {
                self.busy = true;
                self.confirm_unbind = None;
                self.notice = pending.into();
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
                .bg(rgb(0x000000))
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
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        let current_peers = self.active
            && self
                .status
                .as_ref()
                .is_some_and(|status| status.phase == "online");
        let correct_role = self
            .status
            .as_ref()
            .is_some_and(|status| status.role == Some(InstanceRole::Controller));
        let peers: Vec<_> = self
            .status
            .as_ref()
            .into_iter()
            .flat_map(|status| &status.peers)
            .filter(|peer| peer.role == InstanceRole::Controlled)
            .collect();
        let state = if !self.initialized {
            "正在读取状态"
        } else if !configured {
            "尚未绑定实例"
        } else {
            match self.status.as_ref().map(|status| status.phase.as_str()) {
                Some("online") => "管理连接 已连接",
                Some("connecting") => "管理连接 连接中…",
                Some("revoked") => "管理连接 授权已撤销",
                Some("error") => "管理连接 连接失败",
                _ => "管理连接 未连接",
            }
        };
        let button = |id: &'static str, label: &str, primary: bool| {
            ui::button(id, label, primary, enabled, palette)
        };
        let header_button = |id: &'static str, label: &str, primary: bool, available: bool| {
            header_button(id, label, primary, available, palette)
        };
        let mut header = div()
            .id("mobile-titlebar-actions")
            .w_full()
            .min_h(px(theme::CONTROL_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .overflow_x_scroll()
            .whitespace_nowrap();
        if configured {
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_3()
                    .text_size(px(12.))
                    .children(
                        [
                            ("授权设备", peers.len().to_string()),
                            (
                                "当前在线",
                                if current_peers {
                                    peers.iter().filter(|peer| peer.online).count().to_string()
                                } else {
                                    "—".into()
                                },
                            ),
                        ]
                        .into_iter()
                        .map(|(label, value)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().text_color(rgb(palette.muted)).child(label))
                                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(value))
                        }),
                    ),
            );
        }
        header = header
            .child(
                ui::badge(
                    state,
                    if current_peers {
                        palette.success
                    } else {
                        palette.muted
                    },
                    palette.panel,
                )
                .flex_shrink_0(),
            )
            .child(
                header_button(
                    "toggle-server-binding",
                    if configured {
                        if self.binding_expanded {
                            "收起实例信息"
                        } else {
                            "实例信息"
                        }
                    } else if self.binding_expanded {
                        "收起绑定"
                    } else {
                        "绑定实例"
                    },
                    true,
                    self.initialized && self.active,
                )
                .flex_shrink_0()
                .on_click(cx.listener(|view, _, window, cx| {
                    if !view.initialized || !view.active {
                        return;
                    }
                    view.binding_expanded = !view.binding_expanded;
                    view.confirm_unbind = None;
                    window.dismiss_virtual_keyboard();
                    cx.notify();
                })),
            )
            .child(
                header_button("refresh", "刷新", false, enabled)
                    .flex_shrink_0()
                    .on_click(cx.listener(|view, _, window, cx| {
                        window.dismiss_virtual_keyboard();
                        view.action(Command::Refresh, cx);
                        cx.notify();
                    })),
            );
        if configured {
            if self
                .status
                .as_ref()
                .is_some_and(|status| !matches!(status.phase.as_str(), "online" | "connecting"))
            {
                header = header.child(
                    header_button("resume", "重新连接", false, enabled)
                        .flex_shrink_0()
                        .on_click(cx.listener(|view, _, window, cx| {
                            window.dismiss_virtual_keyboard();
                            view.action(Command::Resume, cx);
                            cx.notify();
                        })),
                );
            }
            header = header.child(
                header_button("unbind", "解绑实例", false, enabled)
                    .flex_shrink_0()
                    .text_color(rgb(palette.danger))
                    .on_click(cx.listener(|view, _, window, cx| {
                        if !view.enabled() {
                            return;
                        }
                        window.dismiss_virtual_keyboard();
                        view.confirm_unbind = Some(UnbindConfirmation::Remote);
                        cx.notify();
                    })),
            );
        }
        let content_width = (f32::from(visible.size.width) - 32.).clamp(1., 1200.);
        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .max_w(px(1200.))
            .child(header);
        // Keep action feedback beside the menu instead of below the device list.
        if !self.notice.is_empty() {
            content = content.child(
                div()
                    .id("management-action-notice")
                    .text_color(rgb(palette.muted))
                    .child(self.notice.clone()),
            );
        }
        if let Some(confirmation) = self.confirm_unbind {
            content = content.child(
                ui::card(palette)
                    .id("unbind-confirmation")
                    .p_3()
                    .gap_2()
                    .child(match confirmation {
                        UnbindConfirmation::Remote => "解除服务器绑定后，需要重新输入实例密码。",
                        UnbindConfirmation::Local => {
                            "仅删除此机绑定，不会撤销服务器记录；重新绑定前在网页重置对应实例密码。"
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                button("confirm-unbind", "确认", false)
                                    .debug_selector(|| "confirm-unbind".into())
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        let Some(confirmation) = view.confirm_unbind else {
                                            return;
                                        };
                                        window.dismiss_virtual_keyboard();
                                        view.action(
                                            match confirmation {
                                                UnbindConfirmation::Remote => Command::Unbind,
                                                UnbindConfirmation::Local => Command::ForgetLocal,
                                            },
                                            cx,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(button("cancel-unbind", "取消", false).on_click(cx.listener(
                                |view, _, _, cx| {
                                    view.confirm_unbind = None;
                                    cx.notify();
                                },
                            ))),
                    ),
            );
        }
        if self.binding_expanded {
            if configured {
                let status = self.status.as_ref();
                content = content.child(
                    ui::card(palette)
                        .p_4()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(rgb(palette.muted))
                                .child("当前管理实例")
                                .when_some(
                                    status.and_then(|status| status.server.as_ref()),
                                    |row, server| row.child(server.clone()),
                                )
                                .when_some(
                                    status.and_then(|status| status.binding.as_ref()),
                                    |row, binding| {
                                        row.child(format!("实例编号：{}", binding.device_id))
                                    },
                                ),
                        )
                        .child(button("forget-local", "仅清除此机绑定", false).on_click(
                            cx.listener(|view, _, window, cx| {
                                if !view.enabled() {
                                    return;
                                }
                                window.dismiss_virtual_keyboard();
                                view.confirm_unbind = Some(UnbindConfirmation::Local);
                                cx.notify();
                            }),
                        )),
                );
            } else {
                content =
                    content.child(
                        ui::card(palette)
                            .p_4()
                            .child(ui::heading("绑定管理实例", "", palette))
                            .child(self.server.clone())
                            .child(self.code.clone())
                            .child(button("enroll", "绑定并连接", true).on_click(
                                cx.listener(|view, _, window, cx| view.enroll(window, cx)),
                            )),
                    );
            }
        }
        if configured && !correct_role {
            content = content.child(
                div()
                    .text_color(rgb(palette.danger))
                    .child("此实例角色与当前应用不匹配，请使用控制端实例重新绑定。"),
            );
        }
        content = content.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("我的设备"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.muted))
                        .child(format!("{} 台设备", peers.len())),
                ),
        );
        if peers.is_empty() {
            content = content.child(
                ui::card(palette)
                    .items_center()
                    .justify_center()
                    .py_8()
                    .child(ui::monitor(palette))
                    .child(if !configured {
                        "绑定管理实例后即可查看设备"
                    } else if !correct_role {
                        "需要检查实例角色"
                    } else if !current_peers {
                        "等待设备状态同步"
                    } else {
                        "暂无授权设备"
                    }),
            );
        }
        let mut devices = div()
            .id("peers")
            .flex_shrink_0()
            .grid()
            .grid_cols(card_columns(content_width))
            .gap_4();
        for peer in peers {
            let id = peer.binding.device_id.to_string();
            let connectable = enabled
                && current_peers
                && correct_role
                && peer.online
                && !peer.addresses.is_empty();
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
                .join("，");
            let card = ui::card(palette)
                .id(gpui::SharedString::from(format!("host-{id}")))
                .aspect_ratio(3. / 2.)
                .min_h(px(0.))
                .p_3()
                .gap_2()
                .child(
                    div()
                        .h(px(20.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .line_height(px(20.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_size(px(16.))
                                .child(peer.name.clone()),
                        )
                        .child(
                            ui::badge(
                                if !current_peers {
                                    "状态未知"
                                } else if peer.online {
                                    "在线"
                                } else {
                                    "离线"
                                },
                                if current_peers && peer.online {
                                    palette.success
                                } else {
                                    palette.muted
                                },
                                palette.page,
                            )
                            .h(px(20.))
                            .py_0()
                            .text_size(px(11.))
                            .line_height(px(16.)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_hidden()
                        .rounded_lg()
                        .bg(rgb(palette.page))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::monitor(palette)),
                )
                .child(
                    div()
                        .h(px(16.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(rgb(palette.muted))
                        .truncate()
                        .child(if addresses.is_empty() {
                            "等待设备上报地址".to_owned()
                        } else {
                            addresses
                        }),
                )
                .child(
                    div()
                        .relative()
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .child(theme::surface(
                            rgb(if connectable {
                                palette.accent_soft
                            } else {
                                palette.page
                            }),
                            None,
                            theme::CONTROL_EXTENT,
                        ))
                        .text_color(rgb(if connectable {
                            palette.accent
                        } else {
                            palette.muted
                        }))
                        .child(if connectable {
                            "连接设备"
                        } else if !current_peers {
                            "等待状态同步"
                        } else {
                            "暂不可连接"
                        })
                        .child(if connectable { "→" } else { "" }),
                );
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
            devices = devices.child(card);
        }
        content = content.child(devices);
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

// The rounded surface is compact; the complete 44px row remains tappable.
fn header_button(
    id: &'static str,
    label: &str,
    primary: bool,
    enabled: bool,
    palette: theme::Palette,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(|| id.into())
        .relative()
        .h(px(theme::CONTROL_HEIGHT))
        .min_h(px(theme::CONTROL_HEIGHT))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .line_height(px(20.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(if primary {
            palette.on_accent
        } else {
            palette.text
        }))
        .opacity(if enabled { 1. } else { 0.45 })
        .child(
            div()
                .absolute()
                .top(px(6.))
                .bottom(px(6.))
                .left_0()
                .right_0()
                .child(theme::surface(
                    rgb(if primary {
                        palette.accent
                    } else {
                        palette.panel
                    }),
                    (!primary).then_some(rgb(palette.border)),
                    theme::CONTROL_EXTENT,
                )),
        )
        .child(label.to_owned())
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.opacity(0.85))
                .active(|style| style.opacity(0.65))
        })
}

fn card_columns(width: f32) -> u16 {
    ((width + 16.) / 252.).floor().clamp(1., 4.) as u16
}

#[cfg(test)]
mod tests {
    use super::card_columns;

    #[test]
    fn device_cards_follow_desktop_columns_on_phone_and_tablet() {
        assert_eq!(card_columns(328.), 1);
        assert_eq!(card_columns(608.), 2);
        assert_eq!(card_columns(992.), 4);
        assert_eq!(card_columns(2400.), 4);
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod interaction_tests {
    use super::*;
    use gpui_test_support::{TestAppContext, TouchEvent, TouchId, TouchPhase, point, size};

    fn tap(cx: &mut gpui_test_support::VisualTestContext, position: gpui::Point<gpui::Pixels>) {
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            cx.simulate_event(TouchEvent {
                id: TouchId(1),
                phase,
                position,
                force: None,
                predicted_position: None,
            });
        }
    }

    #[gpui_test_support::test]
    fn unbind_confirms_once_and_busy_actions_keep_local_menu_responsive(cx: &mut TestAppContext) {
        let (commands, mut pending) = tokio::sync::mpsc::channel(1);
        let (events, incoming) = mpsc::channel();
        let (lifecycle, _) = tokio::sync::watch::channel(true);
        let (panel, cx) = cx.add_window_view(|_, cx| Panel {
            commands,
            events: incoming,
            lifecycle,
            server: cx.new(|cx| TextField::new("服务端地址", false, cx)),
            code: cx.new(|cx| TextField::new("控制端实例密码", true, cx)),
            status: Some(Status {
                configured: true,
                server: None,
                phase: "online".into(),
                binding: None,
                role: Some(InstanceRole::Controller),
                addresses: vec![],
                peers: vec![],
            }),
            initialized: true,
            active: true,
            busy: false,
            confirm_unbind: None,
            binding_expanded: false,
            session_visible: false,
            notice: String::new(),
            _updates: gpui::Task::ready(()),
            _lifecycle: cx.on_app_lifecycle(|_, _| {}),
        });
        cx.simulate_resize(size(px(1000.), px(720.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let toggle = cx.debug_bounds("toggle-server-binding").unwrap();
        assert_eq!(toggle.size.height, px(44.));
        // The first touch is outside the 32px decoration but inside its 44px envelope.
        let edge = point(toggle.center().x, toggle.top() + px(2.));
        tap(cx, edge);
        cx.update(|_, cx| assert!(panel.read(cx).binding_expanded));
        tap(cx, edge);
        cx.update(|_, cx| assert!(!panel.read(cx).binding_expanded));
        let unbind = cx.debug_bounds("unbind").unwrap();
        tap(cx, unbind.center());
        cx.update(|_, cx| {
            assert!(panel.read(cx).confirm_unbind == Some(UnbindConfirmation::Remote))
        });
        assert!(pending.try_recv().is_err(), "the first tap must not unbind");
        assert_eq!(
            cx.debug_bounds("unbind").unwrap(),
            unbind,
            "confirmation must not move the menu target"
        );
        let confirm = cx.debug_bounds("confirm-unbind").unwrap();
        tap(cx, confirm.center());
        assert!(matches!(pending.try_recv(), Ok(Command::Unbind)));
        cx.update(|_, cx| {
            let view = panel.read(cx);
            assert!(view.busy);
            assert!(view.confirm_unbind.is_none());
            assert_eq!(view.notice, "正在解绑实例…");
        });
        tap(cx, unbind.center());
        assert!(
            pending.try_recv().is_err(),
            "busy repeats must not enqueue another unbind"
        );
        let toggle = cx.debug_bounds("toggle-server-binding").unwrap();
        tap(cx, toggle.center());
        cx.update(|_, cx| {
            assert!(
                panel.read(cx).binding_expanded,
                "local menu must remain usable while a network action runs"
            )
        });
        events
            .send(Event::Action(Err("management authorization denied".into())))
            .unwrap();
        cx.update(|window, cx| panel.update(cx, |view, cx| view.consume(window, cx)));
        cx.update(|_, cx| {
            let view = panel.read(cx);
            assert!(!view.busy);
            assert_eq!(view.notice, "management authorization denied");
        });
    }
}
