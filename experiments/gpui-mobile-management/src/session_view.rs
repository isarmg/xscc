use crate::{
    display_state::accepts_display_update,
    presentation::VideoPresentation,
    remote_keyboard::{Command, KeyboardControl, KeyboardEvent, RemoteKeyboard},
    theme,
    touch_input::{Geometry, Output, Phase, Point, TouchInput},
};
use base::{
    message_proto::{ControlKey, KeyEvent, MouseEvent},
    session_protocol::input,
};
use gpui::{
    AppContext, AppLifecyclePhase, Context, Entity, FocusHandle, HitboxBehavior, ImageSource,
    IntoElement, ObjectFit, Render, Subscription, TouchEvent, TouchPhase, Window, canvas, div, img,
    prelude::*, px, rgb,
};
use librustdesk::gpui_session::{
    self, InputCommand, Monitor, SessionEvent, SessionHandle, SessionQuality, TargetFps,
};
use std::{
    rc::Rc,
    sync::{Arc, mpsc},
};
use xscc_native_management::manager::NativeManager;

enum Reply {
    Connected(u64, Result<SessionHandle, String>),
}

pub struct SessionView {
    keyboard: Option<Entity<RemoteKeyboard>>,
    keyboard_control: Option<Rc<KeyboardControl>>,
    keyboard_subscription: Option<Subscription>,
    touch: TouchInput,
    monitors: Vec<Monitor>,
    current_monitor: Option<usize>,
    pending_monitor: Option<usize>,
    connected: bool,
    controls_expanded: bool,
    target_fps: Option<TargetFps>,
    quality: SessionQuality,
    direct_touch: bool,
    extent: (u32, u32),
    origin: (i32, i32),
    focus: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    runtime: tokio::runtime::Handle,
    manager: Option<Arc<NativeManager>>,
    selected: Option<String>,
    #[cfg(feature = "test-lan-direct")]
    test_lan: bool,
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
                    if let Some(control) = &view.keyboard_control {
                        control.cancel();
                    }
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
            if matches!(phase, AppLifecyclePhase::Inactive) {
                let _ = weak.update(cx, |view, cx| {
                    view.release_input();
                    if let Some(session) = &view.session {
                        if let Err(error) = session.set_audio_enabled(false) {
                            view.notice = error.to_string();
                        }
                    }
                    cx.notify();
                });
            }
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
            keyboard: None,
            keyboard_control: None,
            keyboard_subscription: None,
            touch: TouchInput::default(),
            monitors: Vec::new(),
            current_monitor: None,
            pending_monitor: None,
            connected: false,
            controls_expanded: false,
            target_fps: None,
            quality: SessionQuality::default(),
            direct_touch: true,
            extent: (0, 0),
            origin: (0, 0),
            focus,
            _focus_subscriptions: focus_subscriptions,
            runtime,
            manager: None,
            selected: None,
            #[cfg(feature = "test-lan-direct")]
            test_lan: false,
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
        if let Some(control) = self.keyboard_control.take() {
            control.configure(false, false, false);
            control.cancel();
        }
        self.keyboard.take();
        self.keyboard_subscription.take();
        self.release_input();
        self.touch.reset();
        self.monitors.clear();
        self.current_monitor = None;
        self.pending_monitor = None;
        self.connected = false;
        self.target_fps = None;
        self.quality = SessionQuality::default();
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
        #[cfg(feature = "test-lan-direct")]
        {
            self.test_lan = false;
        }
        self.presentation.invalidate();
        self.notice = "会话已断开".into();
        // Dropping a queued result also disconnects its session handle.
        for reply in self.receiver.try_iter() {
            drop(reply);
        }
    }

    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop();
        self.controls_expanded = false;
        if let Err(error) = self.presentation.update(None, window) {
            self.notice = error;
        }
        cx.notify();
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
                gpui_session::connect_interactive(&device_id).await
            } else {
                gpui_session::connect(&device_id).await
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Connected(generation, result));
            wake.notify_one();
        }));
        cx.notify();
    }

    #[cfg(feature = "test-lan-direct")]
    pub fn connect_test_lan(&mut self, endpoint: String, cx: &mut Context<Self>) {
        self.stop();
        self.selected = Some(endpoint.clone());
        self.test_lan = true;
        let generation = self.generation;
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        let interactive = self.direct_touch;
        self.notice = "TEST ONLY：正在建立临时 IP 直连…".into();
        self.connecting = Some(self.runtime.spawn(async move {
            let result = gpui_session::connect_test_lan_mobile(&endpoint, interactive)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Connected(generation, result));
            wake.notify_one();
        }));
        cx.notify();
    }

    fn authorized(&self) -> bool {
        #[cfg(feature = "test-lan-direct")]
        if self.test_lan {
            return self.selected.is_some();
        }
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
            && self.pending_monitor.is_none()
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
                Output::RightDown(x, y) => (
                    input::MOUSE_TYPE_DOWN | (input::MOUSE_BUTTON_RIGHT << 3),
                    x,
                    y,
                ),
                Output::RightUp(x, y) => (
                    input::MOUSE_TYPE_UP | (input::MOUSE_BUTTON_RIGHT << 3),
                    x,
                    y,
                ),
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
            if let Some(control) = &self.keyboard_control {
                control.cancel();
            }
            window.dismiss_virtual_keyboard();
            self.focus.focus(window, cx);
        }
        let phase = match event.phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended => Phase::Ended,
            TouchPhase::Cancelled => Phase::Cancelled,
        };
        let was_armed = self.touch.right_click_armed();
        let output = self.touch.event(
            event.id.0,
            phase,
            Point(event.position.x.into(), event.position.y.into()),
            geometry,
        );
        self.send_outputs(output);
        if self.touch.pending() || was_armed != self.touch.right_click_armed() {
            cx.notify();
        }
    }

    fn keyboard_event(&mut self, event: &KeyboardEvent, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        let Some(control) = &self.keyboard_control else {
            return;
        };
        if control.epoch() != event.epoch {
            return;
        }
        let allowed = self.input_allowed()
            && self
                .session
                .as_ref()
                .is_some_and(SessionHandle::keyboard_input_allowed);
        if !allowed {
            control.cancel();
            return;
        }
        let mut failure = None;
        if let Some(session) = &self.session {
            for command in &event.commands {
                let mut key = KeyEvent::new();
                match command {
                    Command::Text(text) => key.set_seq(text.clone()),
                    Command::Shortcut(shortcut) => {
                        key.set_chr(shortcut.character() as u32);
                        key.press = true;
                        key.modifiers.push(ControlKey::Control.into());
                    }
                    Command::Key(control) => {
                        key.set_control_key(*control);
                        key.press = true;
                    }
                }
                if let Err(error) = session.send_input(InputCommand::Key(key)) {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        if let Some(error) = failure {
            control.cancel();
            self.stop();
            self.notice = error;
        }
        cx.notify();
    }

    fn switch_monitor(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.authorized()
            || !self.connected
            || self.pending_monitor.is_some()
            || self.current_monitor == Some(index)
            || !self
                .monitors
                .iter()
                .any(|monitor| monitor.index == index && monitor.available)
        {
            return;
        }
        // Releases must precede the core's pending-display input gate.
        self.release_input();
        if let Some(control) = &self.keyboard_control {
            control.cancel();
        }
        window.dismiss_virtual_keyboard();
        let Some(session) = &self.session else { return };
        match session.switch_monitor(index) {
            Ok(()) => {
                self.pending_monitor = Some(index);
                self.quality = SessionQuality::default();
                self.extent = (0, 0);
                self.presentation.invalidate();
                if let Err(error) = self.presentation.update(None, window) {
                    self.stop();
                    self.notice = error;
                } else {
                    self.notice = format!("正在切换至屏幕 {}… 未响应时请断开重连。", index + 1);
                }
            }
            Err(error) => self.notice = error.to_string(),
        }
        cx.notify();
    }

    fn request_fps(&mut self, target: TargetFps, cx: &mut Context<Self>) {
        if !self.authorized() || !self.connected {
            return;
        }
        let Some(session) = &self.session else { return };
        match session.set_target_fps(target) {
            Ok(()) => self.target_fps = Some(target),
            Err(error) => self.notice = error.to_string(),
        }
        cx.notify();
    }

    fn consume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    let control = KeyboardControl::new(false);
                    let keyboard_control = control.clone();
                    let keyboard = cx.new(|cx| RemoteKeyboard::new(keyboard_control, window, cx));
                    let generation = self.generation;
                    self.keyboard_subscription =
                        Some(cx.subscribe(&keyboard, move |view, _, event, cx| {
                            view.keyboard_event(event, generation, cx);
                        }));
                    self.keyboard = Some(keyboard);
                    self.keyboard_control = Some(control);
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
                SessionEvent::Connected => {
                    self.connected = true;
                    self.notice = "已连接，等待首帧…".into();
                }
                SessionEvent::Monitors(monitors) => {
                    if self.current_monitor.is_some_and(|index| {
                        !monitors
                            .iter()
                            .any(|monitor| monitor.index == index && monitor.available)
                    }) {
                        self.release_input();
                        if let Some(control) = &self.keyboard_control {
                            control.cancel();
                        }
                        self.current_monitor = None;
                        self.extent = (0, 0);
                        self.quality = SessionQuality::default();
                        self.presentation.invalidate();
                        self.notice = "当前屏幕已离线，请选择可用屏幕。".into();
                    }
                    self.monitors = monitors;
                }
                SessionEvent::Quality { index, update } => {
                    if accepts_display_update(self.current_monitor, self.pending_monitor, index) {
                        self.quality.merge(update);
                    }
                }
                SessionEvent::Display { index, x, y, .. } => {
                    self.release_input();
                    if let Some(control) = &self.keyboard_control {
                        control.cancel();
                    }
                    if self.current_monitor != Some(index) || self.pending_monitor.is_some() {
                        self.quality = SessionQuality::default();
                        self.presentation.invalidate();
                        self.extent = (0, 0);
                    }
                    self.current_monitor = Some(index);
                    if self.pending_monitor == Some(index) {
                        self.pending_monitor = None;
                        self.notice = "屏幕已切换，等待首帧…".into();
                    }
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
            .and_then(|session| session.take_frame())
            .filter(|frame| {
                accepts_display_update(self.current_monitor, self.pending_monitor, frame.display)
            });
        if let Some(frame) = &frame {
            let extent = (frame.width, frame.height);
            if self.extent != extent {
                self.release_input();
                if let Some(control) = &self.keyboard_control {
                    control.cancel();
                }
                self.extent = extent;
            }
        }
        if !self.input_allowed() {
            self.release_input();
        }
        if let Some(control) = &self.keyboard_control {
            control.configure(
                self.input_allowed()
                    && self
                        .session
                        .as_ref()
                        .is_some_and(SessionHandle::keyboard_input_allowed),
                self.session
                    .as_ref()
                    .is_some_and(SessionHandle::native_unicode_supported),
                self.session
                    .as_ref()
                    .is_some_and(SessionHandle::common_shortcuts_supported),
            );
        }
        let pending = self.touch.flush();
        self.send_outputs(pending);
        match self.presentation.update(frame, window) {
            Ok(Some(_)) => {
                self.notice = if self.input_allowed() {
                    "直接触控 · 单指点击/拖动，双指滚动 · 剪贴板关闭".into()
                } else {
                    "仅查看画面 · 输入未启用，剪贴板关闭".into()
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
        self.consume(window, cx);
        if self.touch.pending() {
            window.request_animation_frame();
        }
        let palette = theme::palette(window.appearance());
        let audio = self.session.as_ref().map(SessionHandle::audio_status);
        let generation = self.generation;
        let extent = self.extent;
        let origin = self.origin;
        let input_allowed = self.input_allowed();
        let owner = cx.entity_id();
        let touch = cx.listener(move |view, event: &(TouchEvent, Geometry), window, cx| {
            view.touch_event(&event.0, event.1, generation, window, cx);
        });
        let viewport = div()
            .absolute()
            .top(px(44.))
            .bottom_0()
            .left_0()
            .w_full()
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
                            window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                                touch(&(event.clone(), geometry), window, cx)
                            });
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
            });
        let controls = div()
            .id("session-controls-scroll")
            .occlude()
            .absolute()
            .top(px(44.))
            .left_0()
            .w_full()
            .max_h(px((f32::from(window.fully_visible_bounds().size.height)
                - 100.)
                .max(44.)))
            .overflow_y_scroll()
            .p_3()
            .bg(rgb(palette.panel))
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .when(self.connected, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(self.monitors.iter().map(|monitor| {
                            let index = monitor.index;
                            let label = format!(
                                "屏幕 {} · {}×{}{}",
                                index + 1,
                                monitor.width,
                                monitor.height,
                                if !monitor.available {
                                    "（离线）"
                                } else if self.pending_monitor == Some(index) {
                                    "（切换中）"
                                } else if self.current_monitor == Some(index) {
                                    "（当前）"
                                } else {
                                    ""
                                }
                            );
                            div()
                                .id(("monitor", index))
                                .min_h(px(44.))
                                .p_2()
                                .bg(rgb(palette.panel))
                                .child(label)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.switch_monitor(index, window, cx)
                                }))
                        })),
                )
                .child(
                    div().flex().gap_2().children(
                        [TargetFps::Fps30, TargetFps::Fps60]
                            .into_iter()
                            .map(|target| {
                                div()
                                    .id(("target-fps", target.value() as usize))
                                    .min_h(px(44.))
                                    .p_2()
                                    .bg(rgb(palette.panel))
                                    .child(format!(
                                        "请求 {} FPS{}",
                                        target.value(),
                                        if self.target_fps == Some(target) {
                                            "（已请求）"
                                        } else {
                                            ""
                                        }
                                    ))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.request_fps(target, cx)
                                    }))
                            }),
                    ),
                )
                .child(div().child(format!(
                        "解码 {} FPS · 接收 {} · RTT {} ms · {}（目标帧率不代表实际呈现）",
                        self.quality
                            .decoded_fps
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "—".into()),
                        self.quality.receive_rate.as_deref().unwrap_or("—"),
                        self.quality
                            .round_trip_ms
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "—".into()),
                        self.quality.codec.as_deref().unwrap_or("—"),
                    )))
            })
            .when_some(audio, |view, audio| {
                view.child(
                    div()
                        .id("session-audio")
                        .min_h(px(44.))
                        .p_3()
                        .text_color(rgb(palette.text))
                        .bg(rgb(palette.panel))
                        .child(if audio.enabled {
                            "关闭声音"
                        } else {
                            "开启声音（使用系统音量）"
                        })
                        .on_click(cx.listener(|view, _, _, cx| {
                            if let Some(session) = &view.session {
                                if let Err(error) =
                                    session.set_audio_enabled(!session.audio_status().enabled)
                                {
                                    view.notice = error.to_string();
                                }
                            }
                            cx.notify();
                        })),
                )
                .when_some(audio.error, |view, error| view.child(div().child(error)))
            })
            .when(input_allowed, |view| {
                view.child(
                    div()
                        .id("one-shot-right-click")
                        .min_h(px(44.))
                        .p_3()
                        .bg(rgb(palette.panel))
                        .child(if self.touch.right_click_armed() {
                            "右键已准备：轻点画面（点击此处取消）"
                        } else {
                            "下一次轻点使用右键"
                        })
                        .on_click(cx.listener(|view, _, window, cx| {
                            let cancel = view.touch.right_click_armed();
                            view.release_input();
                            if let Some(control) = &view.keyboard_control {
                                control.cancel();
                            }
                            window.dismiss_virtual_keyboard();
                            view.focus.focus(window, cx);
                            if !cancel && view.input_allowed() && !view.touch.arm_right_click() {
                                view.notice = "请先松开画面上的触点。".into();
                            }
                            cx.notify();
                        })),
                )
            })
            .children(self.keyboard.clone())
            .child(
                div()
                    .id("touch-mode")
                    .min_h(px(44.))
                    .p_3()
                    .child(if self.direct_touch {
                        "切换为仅查看（重连会话）"
                    } else {
                        "启用触控与软键盘（重连会话）"
                    })
                    .on_click(cx.listener(|view, _, _window, cx| {
                        let selected = view.selected.clone();
                        view.release_input();
                        view.direct_touch = !view.direct_touch;
                        if let Some(id) = selected {
                            #[cfg(feature = "test-lan-direct")]
                            if view.test_lan {
                                view.connect_test_lan(id, cx);
                                return;
                            }
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
            });
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .bg(rgb(palette.page))
            .when(self.presentation.image().is_some(), |view| {
                view.child(viewport)
            })
            .child(
                div()
                    .relative()
                    .occlude()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(rgb(palette.panel))
                    .px_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(self.presentation.image().is_some(), |view| view.truncate())
                            .child(self.notice.clone()),
                    )
                    .when(self.selected.is_some(), |view| {
                        view.child(
                            div()
                                .id("toggle-session-controls")
                                .occlude()
                                .min_h(px(44.))
                                .p_2()
                                .child(if self.controls_expanded {
                                    "收起控制"
                                } else {
                                    "会话控制"
                                })
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.release_input();
                                    if let Some(control) = &view.keyboard_control {
                                        control.cancel();
                                    }
                                    window.dismiss_virtual_keyboard();
                                    view.controls_expanded = !view.controls_expanded;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(self.controls_expanded && self.selected.is_some(), |view| {
                view.child(controls)
            })
    }
}
