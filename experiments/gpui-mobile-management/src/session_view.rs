use crate::{
    presentation::VideoPresentation,
    theme,
    touch_input::{Geometry, Output, Phase, Point, TouchInput},
};
use base::{message_proto::MouseEvent, session_protocol::input};
use gpui::{
    AppContext, AppLifecyclePhase, Context, FocusHandle, HitboxBehavior, ImageSource, IntoElement,
    ObjectFit, Render, Subscription, TouchEvent, TouchPhase, Window, canvas, div, img, prelude::*,
    px, rgb,
};
use librustdesk::gpui_session::{self, InputCommand, SessionEvent, SessionHandle};
use std::sync::{Arc, mpsc};
use xscc_native_management::manager::NativeManager;

enum Reply {
    Connected(u64, Result<SessionHandle, String>),
}

pub struct SessionView {
    touch: TouchInput,
    direct_touch: bool,
    extent: (u32, u32),
    origin: (i32, i32),
    focus: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    runtime: tokio::runtime::Handle,
    manager: Option<Arc<NativeManager>>,
    selected: Option<String>,
    session: Option<SessionHandle>,
    connecting: Option<tokio::task::JoinHandle<()>>,
    updates: Option<tokio::task::JoinHandle<()>>,
    generation: u64,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    wake: Arc<tokio::sync::Notify>,
    presentation: VideoPresentation,
    notice: String,
    _refresh: gpui::Task<()>,
    _lifecycle: Subscription,
}

impl SessionView {
    pub fn new(
        runtime: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let focus_subscriptions = vec![
            cx.on_blur(&focus, window, |view: &mut Self, _, _| view.release_input()),
            cx.observe_window_activation(window, |view: &mut Self, window, _| {
                if !window.is_window_active() {
                    view.release_input();
                }
            }),
        ];
        let (sender, receiver) = mpsc::channel();
        let wake = Arc::new(tokio::sync::Notify::new());
        let refresh_wake = wake.clone();
        let refresh = cx.spawn(async move |view, cx| {
            loop {
                refresh_wake.notified().await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let window_handle = window.window_handle();
        let weak = cx.entity().downgrade();
        let lifecycle = cx.on_app_lifecycle(move |phase, cx| {
            if matches!(
                phase,
                AppLifecyclePhase::Background | AppLifecyclePhase::Disconnected
            ) {
                let _ = weak.update(cx, |view, cx| {
                    view.stop();
                    cx.notify();
                });
                // Release the cached image before suspension, without waiting for another frame.
                // A closed window already released its renderer resources.
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    let _ = weak.update(cx, |view, _| {
                        if let Err(error) = view.presentation.update(None, window) {
                            view.notice = error;
                        }
                    });
                });
            }
        });
        Self {
            touch: TouchInput::default(),
            direct_touch: true,
            extent: (0, 0),
            origin: (0, 0),
            focus,
            _focus_subscriptions: focus_subscriptions,
            runtime,
            manager: None,
            selected: None,
            session: None,
            connecting: None,
            updates: None,
            generation: 0,
            sender,
            receiver,
            wake,
            presentation: VideoPresentation::default(),
            notice: "选择已授权设备查看画面".into(),
            _refresh: refresh,
            _lifecycle: lifecycle,
        }
    }

    pub fn attach(&mut self, manager: Arc<NativeManager>) {
        self.manager = Some(manager);
    }

    pub fn stop(&mut self) {
        self.release_input();
        self.touch.reset();
        self.extent = (0, 0);
        self.origin = (0, 0);
        self.generation = self.generation.wrapping_add(1);
        if let Some(task) = self.connecting.take() {
            task.abort();
        }
        if let Some(task) = self.updates.take() {
            task.abort();
        }
        drop(self.session.take());
        self.selected = None;
        self.presentation.invalidate();
        self.notice = "会话已断开".into();
        // Dropping a queued result also disconnects its session handle.
        for reply in self.receiver.try_iter() {
            drop(reply);
        }
    }

    pub fn connect(&mut self, device_id: String, cx: &mut Context<Self>) {
        self.stop();
        self.selected = Some(device_id.clone());
        if !self.authorized() {
            self.selected = None;
            self.notice = "设备授权已失效，请刷新服务端绑定。".into();
            cx.notify();
            return;
        }
        let generation = self.generation;
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.notice = "正在建立授权会话…".into();
        let direct_touch = self.direct_touch;
        self.connecting = Some(self.runtime.spawn(async move {
            let result = if direct_touch {
                gpui_session::connect_touch(&device_id).await
            } else {
                gpui_session::connect(&device_id).await
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Connected(generation, result));
            wake.notify_one();
        }));
        cx.notify();
    }

    fn authorized(&self) -> bool {
        let (Some(manager), Some(id)) = (&self.manager, &self.selected) else {
            return false;
        };
        id.parse().ok().is_some_and(|id| manager.target(id).is_ok())
    }

    pub fn check_authorization(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() && !self.authorized() {
            self.stop();
            self.notice = "设备授权已失效，会话已关闭。".into();
            cx.notify();
        }
    }

    fn input_allowed(&self) -> bool {
        self.direct_touch
            && self.authorized()
            && self.extent.0 > 0
            && self.extent.1 > 0
            && self
                .session
                .as_ref()
                .is_some_and(SessionHandle::touch_input_allowed)
    }

    fn release_input(&mut self) {
        let releases = self.touch.cancel();
        self.send_outputs(releases);
    }

    fn send_outputs(&mut self, outputs: Vec<Output>) {
        let Some(session) = &self.session else {
            return;
        };
        let mut failure = None;
        for output in outputs {
            let (mask, x, y) = match output {
                Output::Move(x, y) => (input::MOUSE_TYPE_MOVE, x, y),
                Output::Down(x, y) => (
                    input::MOUSE_TYPE_DOWN | (input::MOUSE_BUTTON_LEFT << 3),
                    x,
                    y,
                ),
                Output::Up(x, y) => (input::MOUSE_TYPE_UP | (input::MOUSE_BUTTON_LEFT << 3), x, y),
                Output::Scroll(x, y) => (input::MOUSE_TYPE_TRACKPAD, x, y),
            };
            if let Err(error) = session.send_input(InputCommand::Mouse(MouseEvent {
                mask,
                x,
                y,
                ..Default::default()
            })) {
                failure = Some(error.to_string());
                break;
            }
        }
        if let Some(error) = failure {
            // Stop sending after a failure; remote release delivery is best-effort.
            self.touch.reset();
            self.stop();
            self.notice = error;
        }
    }

    fn touch_event(
        &mut self,
        event: &TouchEvent,
        geometry: Geometry,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        if !self.input_allowed() {
            self.release_input();
            if matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                // Retire captured contacts even after permission/listener removal.
                let output = self.touch.event(
                    event.id.0,
                    Phase::Cancelled,
                    Point(event.position.x.into(), event.position.y.into()),
                    geometry,
                );
                self.send_outputs(output);
            }
            return;
        }
        if event.phase == TouchPhase::Started {
            self.focus.focus(window, cx);
        }
        let phase = match event.phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended => Phase::Ended,
            TouchPhase::Cancelled => Phase::Cancelled,
        };
        let output = self.touch.event(
            event.id.0,
            phase,
            Point(event.position.x.into(), event.position.y.into()),
            geometry,
        );
        self.send_outputs(output);
        if self.touch.pending() {
            cx.notify();
        }
    }

    fn consume(&mut self, window: &mut Window) {
        while let Ok(Reply::Connected(generation, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            drop(self.connecting.take());
            match result {
                Ok(session) if self.authorized() => {
                    let updates = session.updates();
                    let wake = self.wake.clone();
                    self.updates = Some(self.runtime.spawn(async move {
                        loop {
                            let alive = updates.wait().await;
                            wake.notify_one();
                            if !alive {
                                break;
                            }
                        }
                    }));
                    self.session = Some(session);
                }
                Ok(_) => {
                    self.stop();
                    self.notice = "设备授权已失效。".into();
                }
                Err(error) => {
                    self.stop();
                    self.notice = error;
                }
            }
        }
        if self.selected.is_some() && !self.authorized() {
            self.stop();
            self.notice = "设备授权已失效，会话已关闭。".into();
        }
        let events = self
            .session
            .as_ref()
            .map(|session| session.drain_events())
            .unwrap_or_default();
        for event in events {
            match event {
                SessionEvent::Connected => self.notice = "已连接，等待首帧…".into(),
                SessionEvent::Display { x, y, .. } => {
                    self.release_input();
                    self.origin = (x, y);
                }
                SessionEvent::Permission { name, enabled } if name == "keyboard" && !enabled => {
                    self.release_input();
                    self.notice = "服务端未允许输入 · 仅查看画面".into();
                }
                SessionEvent::Error(error) => {
                    self.stop();
                    self.notice = error;
                    break;
                }
                SessionEvent::Disconnected => {
                    self.stop();
                    break;
                }
                _ => {}
            }
        }
        let frame = self
            .session
            .as_ref()
            .and_then(|session| session.take_frame());
        if let Some(frame) = &frame {
            let extent = (frame.width, frame.height);
            if self.extent != extent {
                self.release_input();
                self.extent = extent;
            }
        }
        if !self.input_allowed() {
            self.release_input();
        }
        let pending = self.touch.flush();
        self.send_outputs(pending);
        match self.presentation.update(frame, window) {
            Ok(Some(_)) => {
                self.notice = if self.input_allowed() {
                    "直接触控 · 单指点击/拖动，双指滚动 · 音频和剪贴板关闭".into()
                } else {
                    "仅查看画面 · 输入未启用，音频和剪贴板关闭".into()
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.stop();
                self.notice = error;
            }
        }
    }
}

impl Drop for SessionView {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.consume(window);
        if self.touch.pending() {
            window.request_animation_frame();
        }
        let palette = theme::palette(window.appearance());
        let generation = self.generation;
        let extent = self.extent;
        let origin = self.origin;
        let input_allowed = self.input_allowed();
        let owner = cx.entity_id();
        let touch = cx.listener(move |view, event: &(TouchEvent, Geometry), window, cx| {
            view.touch_event(&event.0, event.1, generation, window, cx);
        });
        div()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .gap_3()
            .w_full()
            .child(div().child(self.notice.clone()))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(280.))
                    .bg(rgb(0x101010))
                    .overflow_hidden()
                    .children(self.presentation.image().map(|image| {
                        img(ImageSource::Render(image.clone()))
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                    }))
                    .when(input_allowed, |view| {
                        view.child(
                            canvas(
                                |bounds, window, _| {
                                    (bounds, window.insert_hitbox(bounds, HitboxBehavior::Normal))
                                },
                                move |_, (bounds, hitbox), window, _| {
                                    let geometry = Geometry {
                                        bounds: (
                                            bounds.origin.x.into(),
                                            bounds.origin.y.into(),
                                            bounds.size.width.into(),
                                            bounds.size.height.into(),
                                        ),
                                        extent,
                                        origin,
                                    };
                                    window.on_touch_event(
                                        owner,
                                        &hitbox,
                                        move |event, window, cx| {
                                            touch(&(event.clone(), geometry), window, cx)
                                        },
                                    );
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        )
                    }),
            )
            .child(
                div()
                    .id("touch-mode")
                    .min_h(px(44.))
                    .p_3()
                    .child(if self.direct_touch {
                        "切换为仅查看（重连会话）"
                    } else {
                        "启用直接触控（重连会话）"
                    })
                    .on_click(cx.listener(|view, _, _window, cx| {
                        let selected = view.selected.clone();
                        view.release_input();
                        view.direct_touch = !view.direct_touch;
                        if let Some(id) = selected {
                            view.connect(id, cx);
                        }
                        cx.notify();
                    })),
            )
            .when(self.selected.is_some(), |view| {
                view.child(
                    div()
                        .id("disconnect-session")
                        .min_h(px(44.))
                        .p_3()
                        .text_color(rgb(palette.text))
                        .bg(rgb(palette.panel))
                        .child("断开画面")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.stop();
                            cx.notify();
                        })),
                )
            })
    }
}
