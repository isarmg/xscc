use crate::{input, presentation::VideoPresentation};
use base::message_proto::Message;
use gpui::{
    Bounds, Context, DispatchPhase, EventEmitter, FocusHandle, ImageSource, IntoElement,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Subscription, Window, canvas, div, img,
    prelude::*, rgb,
};
use librustdesk::gpui_session::{self, InputCommand, SessionEvent, SessionHandle, TargetFps};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, mpsc},
};

#[derive(Clone)]
pub struct VideoState {
    pub connecting: bool,
    pub has_session: bool,
    pub connected: bool,
    pub target_fps: TargetFps,
    pub notice: String,
}

enum Reply {
    Connect(u64, Result<SessionHandle, String>),
    SessionClosed(u64),
}

pub struct RemoteVideo {
    runtime: tokio::runtime::Handle,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    wake: Arc<tokio::sync::Notify>,
    session_updates: Option<tokio::task::JoinHandle<()>>,
    session: Option<SessionHandle>,
    connecting: bool,
    generation: u64,
    connected: bool,
    keyboard_allowed: bool,
    target_fps: TargetFps,
    notice: String,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    extent: (u32, u32),
    origin: (i32, i32),
    video: VideoPresentation,
    keys: HashMap<String, Message>,
    buttons: HashMap<i32, Message>,
    scroll_remainder: (f32, f32),
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<gpui::Task<()>>,
}

impl EventEmitter<VideoState> for RemoteVideo {}

impl RemoteVideo {
    pub fn new(
        runtime: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.on_blur(&focus, window, |view: &mut Self, _, cx| {
                if view.release_input() {
                    view.publish(cx);
                }
            }),
            cx.observe_window_activation(window, |view: &mut Self, window, cx| {
                if !window.is_window_active() && view.release_input() {
                    view.publish(cx);
                }
            }),
        ];
        let wake = Arc::new(tokio::sync::Notify::new());
        let ui_wake = wake.clone();
        let task = cx.spawn(async move |view, cx| {
            loop {
                ui_wake.notified().await;
                if view
                    .update(cx, |view: &mut Self, cx| {
                        if view.consume_updates() {
                            view.publish(cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            runtime,
            sender,
            receiver,
            wake,
            session_updates: None,
            session: None,
            connecting: false,
            generation: 0,
            connected: false,
            keyboard_allowed: false,
            target_fps: TargetFps::Fps60,
            notice: String::new(),
            focus,
            bounds: Rc::new(Cell::new(Bounds::default())),
            extent: (0, 0),
            origin: (0, 0),
            video: VideoPresentation::default(),
            keys: HashMap::new(),
            buttons: HashMap::new(),
            scroll_remainder: (0.0, 0.0),
            _subscriptions: subscriptions,
            _tasks: vec![task],
        }
    }

    pub fn state(&self) -> VideoState {
        VideoState {
            connecting: self.connecting,
            has_session: self.session.is_some(),
            connected: self.connected,
            target_fps: self.target_fps,
            notice: self.notice.clone(),
        }
    }

    fn publish(&self, cx: &mut Context<Self>) {
        cx.emit(self.state());
    }

    pub fn connect(&mut self, device_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.start_connect(device_id);
        self.focus.focus(window, cx);
        self.publish(cx);
        cx.notify();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.close();
        self.publish(cx);
        cx.notify();
    }

    pub fn request_fps(&mut self, target: TargetFps, cx: &mut Context<Self>) {
        self.set_target_fps(target);
        self.publish(cx);
        cx.notify();
    }

    fn start_connect(&mut self, device_id: String) {
        if self.connecting || self.session.is_some() {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.connecting = true;
        self.video.invalidate();
        self.notice = "正在验证授权并连接…".into();
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let result = gpui_session::connect(&device_id)
                .await
                .map_err(|error| error.to_string());
            if let Err(error) = sender.send(Reply::Connect(generation, result))
                && let Reply::Connect(_, Ok(session)) = error.0
            {
                session.disconnect();
            }
            wake.notify_one();
        });
    }

    fn release_input(&mut self) -> bool {
        self.scroll_remainder = (0.0, 0.0);
        let Some(session) = &self.session else {
            self.keys.clear();
            self.buttons.clear();
            return false;
        };
        let mut failed = false;
        for (_, pressed) in self.keys.drain() {
            let message = input::release_key(&pressed);
            if let Err(error) = session.send_input(InputCommand::Key(message.key_event().clone())) {
                self.notice = error.to_string();
                failed = true;
            }
        }
        for (_, mut message) in self.buttons.drain() {
            let event = message.mut_mouse_event();
            event.mask = (event.mask & !7) | base::session_protocol::input::MOUSE_TYPE_UP;
            event.modifiers.clear();
            if let Err(error) =
                session.send_input(InputCommand::Mouse(message.mouse_event().clone()))
            {
                self.notice = error.to_string();
                failed = true;
            }
        }
        failed
    }

    fn close(&mut self) {
        self.release_input();
        if let Some(task) = self.session_updates.take() {
            task.abort();
        }
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
        self.video.invalidate();
        self.notice = "已断开连接。".into();
    }

    fn consume_updates(&mut self) -> bool {
        let mut changed = false;
        let mut closed_generation = None;
        while let Ok(reply) = self.receiver.try_recv() {
            changed = true;
            match reply {
                Reply::SessionClosed(generation) => closed_generation = Some(generation),
                Reply::Connect(generation, result) => {
                    if generation != self.generation {
                        if let Ok(session) = result {
                            session.disconnect();
                        }
                        continue;
                    }
                    self.connecting = false;
                    match result {
                        Ok(session) => {
                            let updates = session.updates();
                            let wake = self.wake.clone();
                            let sender = self.sender.clone();
                            self.session_updates = Some(self.runtime.spawn(async move {
                                loop {
                                    let active = updates.wait().await;
                                    if !active {
                                        let _ = sender.send(Reply::SessionClosed(generation));
                                    }
                                    wake.notify_one();
                                    if !active {
                                        break;
                                    }
                                }
                            }));
                            self.session = Some(session);
                        }
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
        changed |= !events.is_empty();
        for event in events {
            match event {
                SessionEvent::Connecting => self.notice = "正在连接…".into(),
                SessionEvent::Connected => {
                    self.connected = true;
                    self.notice = "已连接；点击画面后发送键鼠。切换窗口会释放已按下的键。".into();
                    self.set_target_fps(self.target_fps);
                }
                SessionEvent::Disconnected => {
                    self.close();
                    break;
                }
                SessionEvent::Error(error) => {
                    self.close();
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
        if closed_generation == Some(self.generation) && self.session.is_some() {
            self.close();
            changed = true;
        }
        changed
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

    fn key(&mut self, key: &gpui::Keystroke, down: bool) -> bool {
        if !self.connected || !self.keyboard_allowed {
            return false;
        }
        let message = if down {
            input::key(key, true)
        } else {
            self.keys.get(&key.key).map(input::release_key)
        };
        let Some(message) = message else {
            return false;
        };
        if let Some(session) = &self.session {
            if let Err(error) = session.send_input(InputCommand::Key(message.key_event().clone())) {
                self.notice = error.to_string();
                return true;
            }
            if down {
                self.keys.entry(key.key.clone()).or_insert(message);
            } else {
                self.keys.remove(&key.key);
            }
        }
        false
    }

    fn wheel(&mut self, event: &ScrollWheelEvent) -> bool {
        if !self.connected || !self.keyboard_allowed {
            return false;
        }
        let (x, y, trackpad) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y, false),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y), true),
        };
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        self.scroll_remainder.0 += x;
        self.scroll_remainder.1 += y;
        let x = self.scroll_remainder.0.trunc() as i32;
        let y = self.scroll_remainder.1.trunc() as i32;
        self.scroll_remainder.0 -= x as f32;
        self.scroll_remainder.1 -= y as f32;
        if let Some(message) = input::wheel(x, y, trackpad, event.modifiers)
            && let Some(session) = &self.session
            && let Err(error) =
                session.send_input(InputCommand::Mouse(message.mouse_event().clone()))
        {
            self.notice = error.to_string();
            return true;
        }
        false
    }

    fn pointer(
        &mut self,
        position: Point<Pixels>,
        kind: i32,
        button: i32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        if !self.connected || !self.keyboard_allowed {
            return false;
        }
        let bounds = self.bounds.get();
        if kind == 2 && !self.buttons.contains_key(&button) {
            return false;
        }
        let message = input::pointer_with_capture(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            (bounds.size.width.into(), bounds.size.height.into()),
            self.extent,
            kind,
            button,
            kind != 1 && !self.buttons.is_empty(),
        );
        // Preserve release even if the frame or viewport became unavailable during a drag.
        let already_absolute = message.is_none();
        let message = message.or_else(|| {
            (kind == 2)
                .then(|| self.buttons.get(&button).cloned())
                .flatten()
        });
        if let Some(mut message) = message {
            let event = message.mut_mouse_event();
            event.modifiers = input::modifier_keys(modifiers);
            if kind == 2 {
                event.mask = (event.mask & !7) | base::session_protocol::input::MOUSE_TYPE_UP;
            }
            // Stored press messages already include the display origin.
            if !already_absolute {
                event.x = event.x.saturating_add(self.origin.0);
                event.y = event.y.saturating_add(self.origin.1);
            }
            if let Some(session) = &self.session {
                if let Err(error) =
                    session.send_input(InputCommand::Mouse(message.mouse_event().clone()))
                {
                    self.notice = error.to_string();
                    return true;
                }
                // Focus-loss releases should use the last drag position, not jump to its start.
                for pressed in self.buttons.values_mut() {
                    pressed.mut_mouse_event().x = message.mouse_event().x;
                    pressed.mut_mouse_event().y = message.mouse_event().y;
                }
                if kind == 1 {
                    self.buttons.insert(button, message);
                }
                if kind == 2 {
                    self.buttons.remove(&button);
                }
            }
        }
        false
    }
}

impl Drop for RemoteVideo {
    fn drop(&mut self) {
        self.close();
    }
}

impl Render for RemoteVideo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.session.as_ref().and_then(SessionHandle::take_frame);
        match self.video.update(frame, window) {
            Ok(Some(extent)) => self.extent = extent,
            Ok(None) => {}
            Err(error) => {
                self.notice = error;
                self.publish(cx);
            }
        }
        let bounds = self.bounds.clone();
        let drag_move = cx.listener(|view, event: &MouseMoveEvent, _, cx| {
            if !view.buttons.is_empty()
                && !view.bounds.get().contains(&event.position)
                && view.pointer(event.position, 0, 0, event.modifiers)
            {
                view.publish(cx);
            }
        });
        div()
            .id("remote-video")
            .relative()
            .size_full()
            .bg(rgb(0))
            .text_color(rgb(crate::theme::DARK.muted))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if view.focus.is_focused(window)
                    && event.keystroke.key == "enter"
                    && event.keystroke.modifiers.control
                    && event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.platform
                {
                    if !event.is_held {
                        if view.release_input() {
                            view.publish(cx);
                        }
                        window.toggle_fullscreen();
                    }
                    cx.stop_propagation();
                    return;
                }
                if view.focus.is_focused(window) && view.key(&event.keystroke, true) {
                    view.publish(cx);
                }
            }))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, window, cx| {
                if view.focus.is_focused(window) && view.key(&event.keystroke, false) {
                    view.publish(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.focus.focus(window, cx);
                    if view.pointer(event.position, 1, 1, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 1, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 1, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.focus.focus(window, cx);
                    if view.pointer(event.position, 1, 2, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 2, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 2, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.focus.focus(window, cx);
                    if view.pointer(event.position, 1, 3, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 3, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
                    if view.pointer(event.position, 2, 3, event.modifiers) {
                        view.publish(cx);
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _, cx| {
                if view.wheel(event) {
                    view.publish(cx);
                }
            }))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                if view.pointer(event.position, 0, 0, event.modifiers) {
                    view.publish(cx);
                }
            }))
            .children(self.video.image().map(|frame| {
                img(ImageSource::Render(frame.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
            }))
            .children(
                self.video
                    .image()
                    .is_none()
                    .then_some(div().p_4().child("尚无远程画面")),
            )
            .child(
                canvas(
                    move |area, _, _| bounds.set(area),
                    move |_, _, window, _| {
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                drag_move(event, window, cx);
                            }
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn replies_before_ui_wait_coalesce_without_losing_terminal_reply() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let wake = tokio::sync::Notify::new();
            let (sender, receiver) = mpsc::channel();
            sender
                .send(Reply::Connect(7, Err("Connection failed".into())))
                .unwrap();
            wake.notify_one();
            sender.send(Reply::SessionClosed(7)).unwrap();
            wake.notify_one();
            tokio::time::timeout(Duration::from_millis(100), wake.notified())
                .await
                .unwrap();
            assert!(matches!(receiver.try_recv(), Ok(Reply::Connect(7, Err(_)))));
            assert!(matches!(receiver.try_recv(), Ok(Reply::SessionClosed(7))));
            assert!(receiver.try_recv().is_err());
        });
    }
}
