use crate::{
    input,
    text_field::{self, TextField},
};
use base::message_proto::Message;
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, ImageSource, IntoElement, KeyDownEvent,
    KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowOptions, canvas, div, img, prelude::*, px, rgb, size,
};
use librustdesk::gpui_session::{self, SessionEvent, SessionHandle, TargetFps};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use xscc::protocol::InstanceRole;
use xscc_native_management::manager::{Request, Status};

enum Reply {
    Status(Result<Status, String>),
    Action(Result<(), String>),
    Connect(u64, Result<SessionHandle, String>),
}

struct Client {
    runtime: tokio::runtime::Handle,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    server: Entity<TextField>,
    password: Entity<TextField>,
    address_host: Entity<TextField>,
    address_port: Entity<TextField>,
    host: bool,
    initialized: bool,
    status: Option<Status>,
    status_pending: bool,
    action_pending: bool,
    confirm_unbind: bool,
    next_status: Instant,
    notice: String,
    session: Option<SessionHandle>,
    connecting: bool,
    generation: u64,
    connected: bool,
    keyboard_allowed: bool,
    target_fps: TargetFps,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    extent: (u32, u32),
    origin: (i32, i32),
    frame: Option<Arc<RenderImage>>,
    clear_frame: bool,
    keys: HashMap<String, Message>,
    buttons: HashMap<i32, Message>,
    scroll_remainder: (f32, f32),
    _subscriptions: Vec<Subscription>,
}

impl Client {
    fn refresh(&mut self) {
        if !self.initialized || self.status_pending || self.action_pending {
            return;
        }
        self.status_pending = true;
        self.next_status = Instant::now() + Duration::from_secs(2);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let result = gpui_session::management_request(Request::Status {})
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()));
            let _ = sender.send(Reply::Status(result));
        });
    }

    fn action(&mut self, request: Request) {
        if !self.initialized || self.action_pending {
            return;
        }
        self.action_pending = true;
        self.confirm_unbind = false;
        self.notice = "正在处理管理请求…".into();
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let result = gpui_session::management_request(request)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Action(result));
        });
    }

    fn enroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.initialized || self.action_pending {
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
        self.action(Request::Enroll {
            server,
            authorization_code,
            addresses,
        });
        cx.notify();
    }

    fn save_address(&mut self, cx: &mut Context<Self>) {
        if !self.host || !self.initialized || self.action_pending {
            return;
        }
        match crate::host_address::parse(
            self.address_host.read(cx).value(),
            self.address_port.read(cx).value(),
        ) {
            Ok(addresses) => self.action(Request::SetAddresses { addresses }),
            Err(error) => self.notice = error,
        }
        cx.notify();
    }

    fn connect(&mut self, device_id: String) {
        if self.host || self.action_pending || self.connecting || self.session.is_some() {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.connecting = true;
        self.clear_frame = true;
        self.notice = "正在验证授权并连接…".into();
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let result = gpui_session::connect(&device_id)
                .await
                .map_err(|error| error.to_string());
            if let Err(error) = sender.send(Reply::Connect(generation, result)) {
                if let Reply::Connect(_, Ok(session)) = error.0 {
                    session.disconnect();
                }
            }
        });
    }

    fn release_input(&mut self) {
        let Some(session) = &self.session else {
            self.keys.clear();
            self.buttons.clear();
            return;
        };
        for (_, mut message) in self.keys.drain() {
            message.mut_key_event().down = false;
            message.mut_key_event().press = false;
            message.mut_key_event().modifiers.clear();
            if let Err(error) = session.send(message) {
                self.notice = error.to_string();
            }
        }
        for (_, mut message) in self.buttons.drain() {
            let event = message.mut_mouse_event();
            event.mask = (event.mask & !7) | base::session_protocol::input::MOUSE_TYPE_UP;
            if let Err(error) = session.send(message) {
                self.notice = error.to_string();
            }
        }
    }

    fn disconnect(&mut self) {
        self.release_input();
        self.generation = self.generation.wrapping_add(1);
        if let Some(session) = self.session.take() {
            session.disconnect();
        }
        self.connecting = false;
        self.connected = false;
        self.keyboard_allowed = false;
        self.scroll_remainder = (0.0, 0.0);
        self.extent = (0, 0);
        self.origin = (0, 0);
        self.clear_frame = true;
        self.notice = "已断开连接。".into();
    }

    fn poll(&mut self) {
        if !self.initialized {
            return;
        }
        while let Ok(reply) = self.receiver.try_recv() {
            match reply {
                Reply::Status(result) => {
                    self.status_pending = false;
                    match result {
                        Ok(status) => self.status = Some(status),
                        Err(error) => self.notice = error,
                    }
                }
                Reply::Action(result) => {
                    self.action_pending = false;
                    self.notice = match result {
                        Ok(()) => "管理请求完成。".into(),
                        Err(error) => error,
                    };
                    self.next_status = Instant::now();
                }
                Reply::Connect(generation, result) => {
                    if generation != self.generation {
                        if let Ok(session) = result {
                            session.disconnect();
                        }
                        continue;
                    }
                    self.connecting = false;
                    match result {
                        Ok(session) => self.session = Some(session),
                        Err(error) => self.notice = error,
                    }
                }
            }
        }
        let events = self
            .session
            .as_ref()
            .map(|session| session.drain_events())
            .unwrap_or_default();
        for event in events {
            match event {
                SessionEvent::Connecting => self.notice = "正在连接…".into(),
                SessionEvent::Connected => {
                    self.connected = true;
                    self.notice = "已连接；点击画面后发送键鼠。切换窗口会释放已按下的键。".into();
                    self.set_target_fps(self.target_fps);
                }
                SessionEvent::Disconnected => {
                    self.disconnect();
                    break;
                }
                SessionEvent::Error(error) => {
                    self.disconnect();
                    self.notice = error;
                    break;
                }
                SessionEvent::Permission { name, enabled } => {
                    if name == "keyboard" {
                        if !enabled {
                            self.release_input();
                        }
                        self.keyboard_allowed = enabled;
                    }
                }
                SessionEvent::Display {
                    x,
                    y,
                    width: _,
                    height: _,
                } => {
                    self.release_input();
                    self.origin = (x, y);
                }
            }
        }
        if Instant::now() >= self.next_status {
            self.refresh();
        }
    }

    fn set_target_fps(&mut self, target: TargetFps) {
        if !self.connected {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        match session.set_target_fps(target) {
            Ok(()) => {
                self.target_fps = target;
                self.notice = format!(
                    "已请求目标 {} FPS；实际帧率取决于主机、网络和显示能力。",
                    target.value()
                );
            }
            Err(error) => self.notice = error.to_string(),
        }
    }

    fn key(&mut self, key: &gpui::Keystroke, down: bool) {
        if !self.connected || !self.keyboard_allowed {
            return;
        }
        let Some(message) = input::key(key, down) else {
            return;
        };
        if let Some(session) = &self.session {
            if let Err(error) = session.send(message.clone()) {
                self.notice = error.to_string();
                return;
            }
            if down {
                self.keys.insert(key.key.clone(), message);
            } else {
                self.keys.remove(&key.key);
            }
        }
    }

    fn wheel(&mut self, event: &ScrollWheelEvent) {
        if !self.connected || !self.keyboard_allowed {
            return;
        }
        let (x, y, trackpad) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y, false),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y), true),
        };
        self.scroll_remainder.0 += x;
        self.scroll_remainder.1 += y;
        let x = self.scroll_remainder.0.trunc() as i32;
        let y = self.scroll_remainder.1.trunc() as i32;
        self.scroll_remainder.0 -= x as f32;
        self.scroll_remainder.1 -= y as f32;
        if let Some(message) = input::wheel(x, y, trackpad, event.modifiers) {
            if let Some(session) = &self.session {
                if let Err(error) = session.send(message) {
                    self.notice = error.to_string();
                }
            }
        }
    }

    fn pointer(&mut self, position: Point<Pixels>, kind: i32, button: i32) {
        if !self.connected || !self.keyboard_allowed {
            return;
        }
        let bounds = self.bounds.get();
        let message = input::pointer(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            (bounds.size.width.into(), bounds.size.height.into()),
            self.extent,
            kind,
            button,
        );
        // A release outside the letterboxed frame still releases the held button.
        let already_absolute = message.is_none();
        let message = message.or_else(|| {
            (kind == 2)
                .then(|| self.buttons.get(&button).cloned())
                .flatten()
        });
        if let Some(mut message) = message {
            let event = message.mut_mouse_event();
            if kind == 2 {
                event.mask = (event.mask & !7) | base::session_protocol::input::MOUSE_TYPE_UP;
            }
            // Stored press messages already include the display origin.
            if !already_absolute {
                event.x = event.x.saturating_add(self.origin.0);
                event.y = event.y.saturating_add(self.origin.1);
            }
            if let Some(session) = &self.session {
                if let Err(error) = session.send(message.clone()) {
                    self.notice = error.to_string();
                    return;
                }
                if kind == 1 {
                    self.buttons.insert(button, message);
                }
                if kind == 2 {
                    self.buttons.remove(&button);
                }
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl Render for Client {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_frame {
            if let Some(frame) = self.frame.take() {
                if let Err(error) = window.drop_image(frame) {
                    self.notice = error.to_string();
                }
            }
            self.clear_frame = false;
        }
        if let Some(frame) = self
            .session
            .as_ref()
            .and_then(|session| session.take_frame())
        {
            let mut rgba = frame.rgba;
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            if let Some(pixels) = image::RgbaImage::from_raw(frame.width, frame.height, rgba) {
                let next = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                if let Some(previous) = self.frame.replace(next) {
                    if let Err(error) = window.drop_image(previous) {
                        self.notice = error.to_string();
                    }
                }
                self.extent = (frame.width, frame.height);
            }
        }
        let configured = self.status.as_ref().is_some_and(|status| status.configured);
        let status_text = self
            .status
            .as_ref()
            .map(|status| {
                format!(
                    "管理状态：{} · 角色：{:?} · 授权设备：{}",
                    status.phase,
                    status.role,
                    status.peers.len()
                )
            })
            .unwrap_or_else(|| "正在连接本机管理服务…".into());
        let peers = self
            .status
            .as_ref()
            .map(|status| status.peers.clone())
            .unwrap_or_default();
        let bounds = self.bounds.clone();
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
            && !self.connecting
            && self.session.is_none()
            && !self.action_pending;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x101923))
            .text_color(rgb(0xe9f1f8))
            .child(if self.host {
                "XSCC 被控端 · Windows beta"
            } else {
                "XSCC 控制端 · Windows beta"
            })
            .child(status_text)
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
            .child(self.notice.clone())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("refresh")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x23435b))
                            .cursor_pointer()
                            .child("刷新状态")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.refresh();
                                cx.notify();
                            })),
                    )
                    .when(configured, |row| {
                        row.child(
                            div()
                                .id("unbind")
                                .px_3()
                                .py_1()
                                .bg(rgb(0x633434))
                                .cursor_pointer()
                                .child(if self.confirm_unbind {
                                    "确认解绑并关闭连接"
                                } else {
                                    "解绑此实例"
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if view.confirm_unbind {
                                        view.disconnect();
                                        view.action(Request::Unbind {});
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
            .when(self.host && self.initialized, |root| {
                root.child(div().flex().flex_col().gap_2()
                    .child(format!("当前上报访问地址：{current_addresses}"))
                    .child("设置新的上报访问地址（两项留空恢复自动；仅替换上报地址，不修改监听、路由或防火墙）")
                    .child(div().flex().gap_2().child(self.address_host.clone()).child(self.address_port.clone()))
                    .when(configured, |section| section.child(div().id("save-address").px_3().py_1().bg(rgb(0x23435b)).cursor_pointer()
                        .child(if self.action_pending { "正在保存…" } else { "保存上报地址 / 留空恢复自动" })
                        .on_click(cx.listener(|view, _, _, cx| view.save_address(cx)))))
                    .when(!configured, |section| section.child("绑定实例时将一并提交以上地址。")))
            })
            .when(!configured && self.initialized, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("使用服务端中对应角色实例的密码绑定；连接权限由服务端授权列表决定。")
                        .child(self.server.clone())
                        .child(self.password.clone())
                        .child(
                            div()
                                .id("enroll")
                                .px_3()
                                .py_2()
                                .bg(rgb(0x235e87))
                                .cursor_pointer()
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
            })
            .child(if self.host {
                "允许接入的控制端"
            } else {
                "允许连接的被控端"
            })
            .child(
                div()
                    .id("peers")
                    .max_h(px(150.0))
                    .overflow_y_scroll()
                    .children(peers.into_iter().map(|peer| {
                        let id = peer.binding.device_id.to_string();
                        let available = can_connect
                            && peer.online
                            && matches!(peer.role, InstanceRole::Controlled);
                        div()
                            .id(gpui::SharedString::from(id.clone()))
                            .px_3()
                            .py_2()
                            .bg(if available {
                                rgb(0x23435b)
                            } else {
                                rgb(0x1a2b39)
                            })
                            .child(format!(
                                "{} · {}{}",
                                peer.name,
                                if peer.online { "在线" } else { "离线" },
                                if available { " · 点击连接" } else { "" }
                            ))
                            .when(available, |row| {
                                row.cursor_pointer().on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        view.connect(id.clone());
                                        view.focus.focus(window, cx);
                                        cx.notify();
                                    },
                                ))
                            })
                    })),
            )
            .when(!self.host && self.session.is_some(), |root| {
                root.child(
                    div()
                        .flex()
                        .gap_2()
                        .child("请求目标 FPS（非实测）：")
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
                                    .bg(if self.target_fps == target {
                                        rgb(0x235e87)
                                    } else {
                                        rgb(0x23435b)
                                    })
                                    .child(target.value().to_string())
                                    .when(self.connected, |button| {
                                        button.cursor_pointer().on_click(cx.listener(
                                            move |view, _, _, cx| {
                                                view.set_target_fps(target);
                                                cx.notify();
                                            },
                                        ))
                                    })
                            }),
                        ),
                )
            })
            .when(self.connecting || self.session.is_some(), |root| {
                root.child(
                    div()
                        .id("disconnect")
                        .px_3()
                        .py_1()
                        .bg(rgb(0x633434))
                        .cursor_pointer()
                        .child("断开 / 取消连接")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.disconnect();
                            cx.notify();
                        })),
                )
            })
            .when(!self.host, |root| {
                root.child(
                    div()
                        .id("remote-video")
                        .relative()
                        .w_full()
                        .flex_1()
                        .min_h(px(180.0))
                        .bg(rgb(0))
                        .track_focus(&self.focus)
                        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, _| {
                            if view.focus.is_focused(window) {
                                view.key(&event.keystroke, true);
                            }
                        }))
                        .on_key_up(cx.listener(|view, event: &KeyUpEvent, window, _| {
                            if view.focus.is_focused(window) {
                                view.key(&event.keystroke, false);
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &MouseDownEvent, window, cx| {
                                view.focus.focus(window, cx);
                                view.pointer(event.position, 1, 1);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|view, event: &MouseUpEvent, _, _| {
                                view.pointer(event.position, 2, 1)
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|view, event: &MouseUpEvent, _, _| {
                                view.pointer(event.position, 2, 1)
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|view, event: &MouseDownEvent, window, cx| {
                                view.focus.focus(window, cx);
                                view.pointer(event.position, 1, 2);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Right,
                            cx.listener(|view, event: &MouseUpEvent, _, _| {
                                view.pointer(event.position, 2, 2)
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Right,
                            cx.listener(|view, event: &MouseUpEvent, _, _| {
                                view.pointer(event.position, 2, 2)
                            }),
                        )
                        .on_scroll_wheel(
                            cx.listener(|view, event: &ScrollWheelEvent, _, _| view.wheel(event)),
                        )
                        .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, _| {
                            view.pointer(event.position, 0, 0)
                        }))
                        .children(self.frame.as_ref().map(|frame| {
                            img(ImageSource::Render(frame.clone()))
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                        }))
                        .children(
                            self.frame
                                .is_none()
                                .then_some(div().p_4().child("尚无远程画面")),
                        )
                        .child(
                            canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {})
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                        ),
                )
            })
            .child("Beta：按实际能力协商硬件编解码，当前为 CPU 帧上传；零拷贝、多屏切换、音频、剪贴板和远程输入法尚未验收。")
    }
}

pub fn run(runtime: &tokio::runtime::Runtime) -> Result<(), Box<dyn std::error::Error>> {
    let initialization = gpui_session::initialize();
    let initialized = initialization.is_ok();
    let initial_notice = initialization.err().unwrap_or_else(|| "正在初始化…".into());
    let startup_error = Arc::new(Mutex::new(None::<String>));
    let app_error = startup_error.clone();
    let handle = runtime.handle().clone();
    let host = cfg!(feature = "native-host");
    gpui_platform::application().run(move |cx: &mut App| {
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
                cx.new(|cx| {
                    let (sender, receiver) = mpsc::channel();
                    let focus = cx.focus_handle();
                    let subscriptions = vec![
                        cx.on_blur(&focus, window, |view: &mut Client, _, _| {
                            view.release_input()
                        }),
                        cx.observe_window_activation(window, |view: &mut Client, window, _| {
                            if !window.is_window_active() {
                                view.release_input();
                            }
                        }),
                    ];
                    cx.spawn(async move |view, cx| {
                        loop {
                            let interval = view.update(cx, |view: &mut Client, cx| {
                                view.poll();
                                cx.notify();
                                crate::pacing::poll_interval(if view.connected {
                                    Some(view.target_fps.value() as u32)
                                } else if view.connecting || view.session.is_some() {
                                    Some(60)
                                } else {
                                    None
                                })
                            });
                            let Ok(interval) = interval else {
                                break;
                            };
                            cx.background_executor().timer(interval).await;
                        }
                    })
                    .detach();
                    Client {
                        runtime: handle,
                        sender,
                        receiver,
                        server: cx.new(|cx| TextField::new("HTTPS 服务端地址", false, cx)),
                        password: cx.new(|cx| TextField::new("实例密码", true, cx)),
                        address_host: cx
                            .new(|cx| TextField::new("新的域名或 IP（不含端口）", false, cx)),
                        address_port: cx.new(|cx| TextField::new("端口（默认 21118）", false, cx)),
                        host,
                        initialized,
                        status: None,
                        status_pending: false,
                        action_pending: false,
                        confirm_unbind: false,
                        next_status: Instant::now(),
                        notice: initial_notice,
                        session: None,
                        connecting: false,
                        generation: 0,
                        connected: false,
                        keyboard_allowed: false,
                        target_fps: TargetFps::Fps60,
                        focus,
                        bounds: Rc::new(Cell::new(Bounds::default())),
                        extent: (0, 0),
                        origin: (0, 0),
                        frame: None,
                        clear_frame: false,
                        keys: HashMap::new(),
                        buttons: HashMap::new(),
                        scroll_remainder: (0.0, 0.0),
                        _subscriptions: subscriptions,
                    }
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
