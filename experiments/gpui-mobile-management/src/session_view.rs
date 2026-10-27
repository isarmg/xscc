use crate::{
    display_state::accepts_display_update,
    presentation::VideoPresentation,
    remote_cursor::{CursorPresentation, PreparedCursor},
    remote_image::remote_image,
    remote_keyboard::{Command, KeyboardControl, KeyboardEvent, KeyboardPage, RemoteKeyboard},
    session_chrome::{ChromeAction, ChromeModel, ChromeMonitor, SessionChrome},
    session_input::{ControlRegions, InputMode, SessionInput},
    session_layout::{self, Panel, Rect, SessionLayout},
    touch_input::{Geometry, Output, Phase, Point},
};
use base::{
    message_proto::{ControlKey, KeyEvent, MouseEvent},
    session_protocol::input,
};
use gpui::{
    AppContext, AppLifecyclePhase, Bounds, Context, Entity, EventEmitter, FocusHandle,
    HitboxBehavior, IntoElement, Pixels, Render, StyleRefinement, Subscription, TouchEvent,
    TouchPhase, Window, canvas, div, prelude::*, px, rgb,
};
use librustdesk::gpui_session::{
    self, BitrateMode, InputCommand, Monitor, ResolutionMode, SessionEvent, SessionHandle,
    SessionQuality, TargetFps, VirtualDisplayRequest,
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, mpsc},
    time::Duration,
};
use xscc_native_management::manager::NativeManager;

pub enum SessionUiEvent {
    BackToDevices,
}

enum Reply {
    Connected(u64, Result<SessionHandle, String>),
}

pub struct SessionView {
    keyboard: Option<Entity<RemoteKeyboard>>,
    keyboard_control: Option<Rc<KeyboardControl>>,
    keyboard_subscription: Option<Subscription>,
    input: SessionInput,
    chrome: Entity<SessionChrome>,
    _chrome_events: Subscription,
    cursor_presentation: CursorPresentation,
    prepared_cursor: PreparedCursor,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    layout: SessionLayout,
    monitors: Vec<Monitor>,
    current_monitor: Option<usize>,
    pending_monitor: Option<usize>,
    connected: bool,
    rail_expanded: bool,
    panel: Panel,
    mouse_buttons: bool,
    keyboard_page: KeyboardPage,
    controls_idle: Option<gpui::Task<()>>,
    presentation_active: bool,
    interactive_session: bool,
    target_fps: Option<TargetFps>,
    quality: SessionQuality,
    view_only: bool,
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
    frame_scheduled: bool,
    notice: String,
    _refresh: gpui::Task<()>,
    _lifecycle: Subscription,
}

impl EventEmitter<SessionUiEvent> for SessionView {}

impl SessionView {
    pub fn new(
        runtime: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chrome = cx.new(SessionChrome::new);
        let chrome_events = cx.subscribe_in(&chrome, window, |view, _, action, window, cx| {
            view.chrome_action(*action, window, cx);
        });
        let focus = cx.focus_handle();
        let focus_subscriptions = vec![
            cx.on_blur(&focus, window, |view: &mut Self, _, cx| {
                view.release_input();
                view.publish_pointer_chrome(cx);
            }),
            cx.observe_window_activation(window, |view: &mut Self, window, cx| {
                if !window.is_window_active() {
                    view.chrome
                        .update(cx, |chrome, cx| chrome.cancel_bitrate(cx));
                    view.release_input();
                    if let Some(control) = &view.keyboard_control {
                        control.cancel();
                    }
                    view.publish_pointer_chrome(cx);
                } else {
                    view.wake.notify_one();
                }
            }),
        ];
        let mut focus_subscriptions = focus_subscriptions;
        focus_subscriptions.push(cx.observe_window_bounds(window, |view, _, cx| {
            view.chrome
                .update(cx, |chrome, cx| chrome.cancel_bitrate(cx));
            view.release_input();
            if let Some(control) = &view.keyboard_control {
                control.cancel();
            }
            view.publish_pointer_chrome(cx);
            view.wake.notify_one();
        }));
        let (sender, receiver) = mpsc::channel();
        let wake = Arc::new(tokio::sync::Notify::new());
        let refresh_wake = wake.clone();
        let refresh = cx.spawn_in(window, async move |view, cx| {
            loop {
                refresh_wake.notified().await;
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
                        view.prepared_cursor = PreparedCursor::default();
                        if let Err(error) = view.cursor_presentation.clear(window) {
                            view.notice = error;
                        }
                        window.set_remote_session_presentation(false);
                        view.presentation_active = false;
                    });
                });
            }
        });
        Self {
            keyboard: None,
            keyboard_control: None,
            keyboard_subscription: None,
            input: SessionInput::default(),
            chrome,
            _chrome_events: chrome_events,
            cursor_presentation: CursorPresentation::default(),
            prepared_cursor: PreparedCursor::default(),
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            layout: SessionLayout::default(),
            monitors: Vec::new(),
            current_monitor: None,
            pending_monitor: None,
            connected: false,
            rail_expanded: true,
            panel: Panel::None,
            mouse_buttons: true,
            keyboard_page: KeyboardPage::Input,
            controls_idle: None,
            presentation_active: false,
            interactive_session: false,
            target_fps: None,
            quality: SessionQuality::default(),
            view_only: false,
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
            frame_scheduled: false,
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
        drop(self.input.reset());
        self.monitors.clear();
        self.current_monitor = None;
        self.pending_monitor = None;
        self.connected = false;
        self.interactive_session = false;
        self.controls_idle.take();
        self.panel = Panel::None;
        self.rail_expanded = true;
        self.prepared_cursor = PreparedCursor::default();
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
        self.presentation.invalidate();
        self.wake.notify_one();
        self.notice = "会话已断开".into();
        // Dropping a queued result also disconnects its session handle.
        for reply in self.receiver.try_iter() {
            drop(reply);
        }
    }

    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop();
        self.panel = Panel::None;
        self.sync_presentation(window);
        self.prepared_cursor = PreparedCursor::default();
        if let Err(error) = self.cursor_presentation.clear(window) {
            self.notice = error;
        }
        self.publish_chrome(cx);
        if let Err(error) = self.presentation.update(None, window) {
            self.notice = error;
        }
        cx.notify();
    }

    pub fn connect(&mut self, device_id: String, window: &mut Window, cx: &mut Context<Self>) {
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
        let interactive = !self.view_only;
        self.interactive_session = interactive;
        self.sync_presentation(window);
        self.connecting = Some(self.runtime.spawn(async move {
            let result = if interactive {
                gpui_session::connect_interactive(&device_id).await
            } else {
                gpui_session::connect(&device_id).await
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(Reply::Connected(generation, result));
            wake.notify_one();
        }));
        self.publish_chrome(cx);
        cx.notify();
    }

    fn authorized(&self) -> bool {
        let (Some(manager), Some(id)) = (&self.manager, &self.selected) else {
            return false;
        };
        id.parse()
            .ok()
            .is_some_and(|id| manager.target_available(id))
    }

    pub fn check_authorization(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() && !self.authorized() {
            self.stop();
            self.notice = "设备授权已失效，会话已关闭。".into();
            self.publish_chrome(cx);
            cx.notify();
        }
    }

    fn input_allowed(&self) -> bool {
        !self.view_only
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
        let releases = self.input.cancel();
        self.send_outputs(releases);
    }

    fn send_outputs(&mut self, outputs: Vec<Output>) {
        let Some(session) = &self.session else {
            return;
        };
        let mut failure = None;
        for output in outputs {
            let update_position = !matches!(output, Output::Scroll(_, _));
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
                Output::MiddleDown(x, y) => (
                    input::MOUSE_TYPE_DOWN | (input::MOUSE_BUTTON_WHEEL << 3),
                    x,
                    y,
                ),
                Output::MiddleUp(x, y) => (
                    input::MOUSE_TYPE_UP | (input::MOUSE_BUTTON_WHEEL << 3),
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
            if update_position {
                session.update_local_cursor_position(x, y);
            }
        }
        if let Some(error) = failure {
            // Stop sending after a failure; remote release delivery is best-effort.
            drop(self.input.reset());
            self.stop();
            self.notice = error;
        }
    }

    fn touch_event(
        &mut self,
        event: &TouchEvent,
        geometry: Geometry,
        controls: ControlRegions,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let phase = match event.phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended => Phase::Ended,
            TouchPhase::Cancelled => Phase::Cancelled,
        };
        if !self.input_allowed() {
            // The raw owner still receives new fingers until the captured sequence ends.
            let output = self.input.cancel_event(event.id.0, phase);
            self.send_outputs(output);
            self.publish_pointer_chrome(cx);
            return;
        }
        if phase == Phase::Started {
            self.controls_idle.take();
            if let Some(control) = &self.keyboard_control {
                control.cancel();
            }
            window.dismiss_virtual_keyboard();
            self.focus.focus(window, cx);
        }
        let had_pending = self.input.pending();
        let output = self.input.event(
            event.id.0,
            phase,
            Point(event.position.x.into(), event.position.y.into()),
            geometry,
            controls,
        );
        self.send_outputs(output);
        if self.input.pending() && !had_pending {
            cx.on_next_frame(window, |view, window, cx| view.flush_touch(window, cx));
        }
        self.prepare_cursor(window);
        self.publish_pointer_chrome(cx);
        if !self.input.active() {
            self.arm_controls_hide(window, cx);
        }
        cx.notify();
    }

    fn flush_touch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_allowed() {
            self.release_input();
        }
        let pending = self.input.flush();
        self.send_outputs(pending);
        self.prepare_cursor(window);
        self.publish_pointer_chrome(cx);
        if self.input.pending() {
            cx.on_next_frame(window, |view, window, cx| view.flush_touch(window, cx));
        }
        cx.notify();
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

    fn request_bitrate(&mut self, limit_kbps: Option<u32>) {
        if !self.connected || !self.authorized() {
            return;
        }
        let Some(session) = &self.session else { return };
        let mode = limit_kbps.map_or(BitrateMode::Auto, BitrateMode::LimitKbps);
        self.notice = match session.set_bitrate_mode(mode) {
            Ok(()) => match limit_kbps {
                Some(value) => format!("已发送码率上限 {value} kbps 请求。"),
                None => "已发送自动码率请求。".into(),
            },
            Err(error) => error.to_string(),
        };
    }

    fn request_resolution(
        &mut self,
        mode: ResolutionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || !self.authorized() || self.view_only {
            return;
        }
        self.release_input();
        self.hide_keyboard(window, cx);
        let Some(session) = &self.session else { return };
        self.notice = match session.set_resolution(mode) {
            Ok(()) => {
                self.extent = (0, 0);
                self.presentation.invalidate();
                if mode.refresh_rate == 0 {
                    format!("已发送 {}×{} 分辨率请求。", mode.width, mode.height)
                } else {
                    format!(
                        "已发送 {}×{} @{} Hz 显示模式请求。",
                        mode.width, mode.height, mode.refresh_rate
                    )
                }
            }
            Err(error) => error.to_string(),
        };
    }

    fn request_virtual_display(
        &mut self,
        request: VirtualDisplayRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || !self.authorized() || self.view_only {
            return;
        }
        self.release_input();
        self.hide_keyboard(window, cx);
        let Some(session) = &self.session else { return };
        self.notice = match session.request_virtual_display(request) {
            Ok(()) => "已发送虚拟屏幕请求，可在屏幕列表查看结果。".into(),
            Err(error) => error.to_string(),
        };
    }

    fn consume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(Reply::Connected(generation, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            changed = true;
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
            changed = true;
        }
        let events = self
            .session
            .as_ref()
            .map(|session| session.drain_events())
            .unwrap_or_default();
        changed |= !events.is_empty();
        for event in events {
            match event {
                SessionEvent::Connected => {
                    self.connected = true;
                    self.notice = "已连接，等待首帧…".into();
                    self.request_fps(TargetFps::Fps60, cx);
                    if let Some(session) = &self.session {
                        if let Err(error) = session.subscribe_remote_cursor() {
                            self.notice = error.to_string();
                        }
                    }
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
                    if self.current_monitor != Some(index)
                        || self.pending_monitor.is_some()
                        || self.origin != (x, y)
                    {
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
                    if let Some(control) = &self.keyboard_control {
                        control.configure(false, false, false);
                        control.cancel();
                    }
                    self.notice = "服务端未允许输入 · 仅查看画面".into();
                }
                SessionEvent::Permission { name, enabled } if name == "keyboard" && enabled => {
                    self.initialize_pointer();
                    self.notice.clear();
                }
                SessionEvent::ControlError(error) => {
                    self.notice = error;
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
        self.sync_presentation(window);
        let audio_changed = self.session.as_ref().is_some_and(|session| {
            let audio = session.audio_status();
            let model = self.chrome.read(cx).model();
            model.audio_on != audio.enabled
                || model.audio_error.as_deref() != audio.error.as_deref()
        });
        if changed || audio_changed {
            self.publish_chrome(cx);
        }
        if !self.frame_scheduled {
            self.frame_scheduled = true;
            cx.on_next_frame(window, |view, window, cx| {
                view.consume(window, cx);
                view.frame_scheduled = false;
                if view.session.is_some() && window.visibility() != gpui::WindowVisibility::Visible
                {
                    return;
                }
                view.prepare_frame(window, cx);
                cx.notify();
            });
        }
    }

    fn prepare_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_frame = self.presentation.image().is_some();
        let mut first_image = !had_frame || self.extent.0 == 0 || self.extent.1 == 0;
        let mut changed = false;
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
                first_image = true;
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
        match self.presentation.update(frame, window) {
            Ok(Some(_)) => {
                if first_image {
                    self.notice.clear();
                    changed = true;
                    self.arm_controls_hide(window, cx);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.stop();
                self.notice = error;
                changed = true;
            }
        }
        if self.input_allowed() {
            if let Some((geometry, controls)) = self.input_geometry() {
                let initialize = first_image || self.input.snapshot().position.is_none();
                let output = self.input.configure(geometry, controls);
                self.send_outputs(output);
                if initialize {
                    self.initialize_pointer();
                }
            }
        }
        self.prepare_cursor(window);
        self.publish_pointer_chrome(cx);
        changed |= had_frame != self.presentation.image().is_some();
        if changed {
            self.publish_chrome(cx);
        }
    }
}

impl SessionView {
    fn sync_presentation(&mut self, window: &mut Window) {
        let active = self.selected.is_some();
        if active != self.presentation_active {
            window.set_remote_session_presentation(active);
            self.presentation_active = active;
        }
    }

    fn input_geometry(&self) -> Option<(Geometry, ControlRegions)> {
        let bounds = self.viewport_bounds.get();
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return None;
        }
        Some(stage_geometry(
            bounds,
            self.layout,
            self.extent,
            self.origin,
        ))
    }

    fn initialize_pointer(&mut self) {
        if self.input.mode() != InputMode::Pointer || !self.input_allowed() {
            return;
        }
        if let Some(position) = self
            .session
            .as_ref()
            .and_then(|session| session.cursor_snapshot().position)
        {
            self.input.set_position(position);
        }
        // An unknown/out-of-screen host point uses the model's clamped center.
        // Align the host once as well, so the initial visible arrow has the same click target.
        if let Some((x, y)) = self.input.snapshot().position {
            self.send_outputs(vec![Output::Move(x, y)]);
        }
    }

    fn prepare_cursor(&mut self, window: &mut Window) {
        let mut snapshot = self.session.as_ref().map(SessionHandle::cursor_snapshot);
        if !self.connected
            || !self.authorized()
            || self.pending_monitor.is_some()
            || self.presentation.image().is_none()
            || self.extent.0 == 0
            || self.extent.1 == 0
        {
            snapshot = None;
        }
        // Pointer-mode painting and clicks share one continuous local position.
        // The host cursor mailbox can still contain an older position after a finger lifts.
        let local = if self.input.mode() == InputMode::Pointer && self.input_allowed() {
            self.input.snapshot().position
        } else {
            if snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.position.is_none())
            {
                snapshot = None;
            }
            None
        };
        self.prepared_cursor =
            match self
                .cursor_presentation
                .prepare(snapshot.as_ref(), local, window)
            {
                Ok(cursor) => cursor,
                Err(error) => {
                    gpui_session::log_cursor_render_failure(&error);
                    PreparedCursor::default()
                }
            };
    }

    fn publish_chrome(&self, cx: &mut Context<Self>) {
        let audio = self.session.as_ref().map(SessionHandle::audio_status);
        let pointer = self.input.snapshot();
        let media = self
            .session
            .as_ref()
            .map(SessionHandle::media_controls)
            .unwrap_or_default();
        let model = ChromeModel {
            layout: self.layout,
            connecting: self.connecting.is_some(),
            connected: self.connected,
            has_frame: self.connected
                && self.extent.0 > 0
                && self.extent.1 > 0
                && self.presentation.image().is_some(),
            notice: self.notice.clone(),
            pointer_mode: self.input.mode() == InputMode::Pointer,
            view_only: self.view_only,
            can_control: self
                .session
                .as_ref()
                .is_some_and(SessionHandle::touch_input_allowed)
                && self.authorized(),
            can_request_control: self.connected && self.authorized() && !self.interactive_session,
            mouse_buttons: self.mouse_buttons,
            rail_expanded: self.rail_expanded,
            panel: self.panel,
            audio_on: audio.as_ref().is_some_and(|audio| audio.enabled),
            audio_error: audio.and_then(|audio| audio.error),
            target_fps: self.target_fps.map(TargetFps::value),
            bitrate_supported: media.bitrate_supported,
            max_bitrate_kbps: media.max_bitrate_kbps,
            bitrate_limit_kbps: match media.bitrate_mode {
                BitrateMode::Auto => None,
                BitrateMode::LimitKbps(value) => Some(value),
            },
            can_change_resolution: media.can_change_resolution && !self.view_only,
            current_resolution: media
                .current_resolution
                .map(|mode| (mode.width, mode.height, mode.refresh_rate)),
            resolutions: media
                .resolutions
                .iter()
                .map(|mode| (mode.width, mode.height, mode.refresh_rate))
                .collect(),
            virtual_display_supported: media.virtual_display_supported,
            virtual_display_driver: media.virtual_display_driver,
            virtual_display_remove_latest_only: media.virtual_display_remove_latest_only,
            can_add_virtual_display: media.can_add_virtual_display && !self.view_only,
            virtual_display_ids: media.virtual_display_ids,
            current_monitor: self.current_monitor,
            pending_monitor: self.pending_monitor,
            monitors: self
                .monitors
                .iter()
                .map(|monitor| ChromeMonitor {
                    index: monitor.index,
                    name: monitor.name.clone(),
                    width: monitor.width,
                    height: monitor.height,
                    available: monitor.available,
                })
                .collect(),
            quality: format!(
                "接收 {} / 解码 {} / UI提交 {} FPS · 解码p95 {} ms · 排队p95 {} ms · {} · RTT {} ms · 编码目标 {} kbps · {}",
                self.quality
                    .received_fps
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.quality
                    .decoded_fps
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.quality
                    .submitted_fps
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.quality
                    .decode_p95_ms
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "—".into()),
                self.quality
                    .ui_queue_p95_ms
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "—".into()),
                self.quality.receive_rate.as_deref().unwrap_or("—"),
                self.quality
                    .round_trip_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.quality
                    .target_bitrate_kbps
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.quality.codec.as_deref().unwrap_or("—"),
            ),
            left_pressed: pointer.left_down,
            right_pressed: pointer.right_down,
            middle_pressed: pointer.middle_down,
            right_click_armed: self.input.right_click_armed(),
        };
        self.chrome.update(cx, |chrome, cx| {
            chrome.set_model(model, cx);
        });
    }

    fn publish_pointer_chrome(&self, cx: &mut Context<Self>) {
        let pointer = self.input.snapshot();
        let changed = {
            let model = self.chrome.read(cx).model();
            model.left_pressed != pointer.left_down
                || model.right_pressed != pointer.right_down
                || model.middle_pressed != pointer.middle_down
                || model.right_click_armed != self.input.right_click_armed()
        };
        if changed {
            self.publish_chrome(cx);
        }
    }

    fn arm_controls_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controls_idle.take();
        let pointer = self.input.snapshot();
        if !self.rail_expanded
            || self.panel != Panel::None
            || self.presentation.image().is_none()
            || self.input.active()
            || pointer.left_down
            || pointer.middle_down
            || pointer.right_down
        {
            return;
        }
        let delay = cx.background_executor().timer(Duration::from_secs(4));
        self.controls_idle = Some(cx.spawn_in(window, async move |view, cx| {
            delay.await;
            let _ = view.update_in(cx, |view, _, cx| {
                let pointer = view.input.snapshot();
                if view.panel == Panel::None
                    && !view.input.active()
                    && !pointer.left_down
                    && !pointer.middle_down
                    && !pointer.right_down
                {
                    view.rail_expanded = false;
                    view.publish_chrome(cx);
                    cx.notify();
                }
            });
        }));
    }

    fn hide_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(keyboard) = &self.keyboard {
            keyboard.update(cx, |keyboard, cx| keyboard.hide(window, cx));
        }
        window.dismiss_virtual_keyboard();
    }

    fn keyboard_page(&mut self, page: KeyboardPage, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_allowed() || self.panel != Panel::Keyboard {
            return;
        }
        self.release_input();
        self.keyboard_page = page;
        if let Some(keyboard) = &self.keyboard {
            keyboard.update(cx, |keyboard, cx| keyboard.show(page, window, cx));
        }
        cx.notify();
    }

    fn chrome_action(&mut self, action: ChromeAction, window: &mut Window, cx: &mut Context<Self>) {
        self.controls_idle.take();
        match action {
            ChromeAction::Exit => {
                self.hide_keyboard(window, cx);
                self.leave(window, cx);
                cx.emit(SessionUiEvent::BackToDevices);
                return;
            }
            ChromeAction::ToggleRail => {
                self.release_input();
                self.rail_expanded = !self.rail_expanded;
                if !self.rail_expanded {
                    self.hide_keyboard(window, cx);
                    self.panel = Panel::None;
                }
            }
            ChromeAction::OpenPanel(panel) => {
                if panel == Panel::Keyboard && !self.input_allowed() {
                    return;
                }
                self.release_input();
                self.rail_expanded = true;
                self.hide_keyboard(window, cx);
                self.panel = panel;
                if panel == Panel::Keyboard {
                    self.keyboard_page(self.keyboard_page, window, cx);
                }
            }
            ChromeAction::ClosePanel => {
                self.release_input();
                self.hide_keyboard(window, cx);
                self.panel = Panel::None;
            }
            ChromeAction::SetPointer(pointer) => {
                let mode = if pointer {
                    InputMode::Pointer
                } else {
                    InputMode::Direct
                };
                if self.input.mode() != mode {
                    self.release_input();
                    self.hide_keyboard(window, cx);
                    let output = self.input.set_mode(mode);
                    self.send_outputs(output);
                    if let Some((geometry, controls)) = self.input_geometry() {
                        let output = self.input.configure(geometry, controls);
                        self.send_outputs(output);
                    }
                    self.initialize_pointer();
                }
            }
            ChromeAction::ToggleMouseButtons => {
                self.release_input();
                self.mouse_buttons = !self.mouse_buttons;
            }
            ChromeAction::ToggleViewOnly => {
                self.release_input();
                self.hide_keyboard(window, cx);
                self.view_only = !self.view_only;
                if self.view_only && self.panel == Panel::Keyboard {
                    self.panel = Panel::None;
                }
                if !self.view_only && !self.interactive_session {
                    if let Some(id) = self.selected.clone() {
                        self.connect(id, window, cx);
                    }
                } else if !self.view_only {
                    self.initialize_pointer();
                }
            }
            ChromeAction::ToggleAudio => {
                if self.connected && self.authorized() {
                    if let Some(session) = &self.session {
                        if let Err(error) =
                            session.set_audio_enabled(!session.audio_status().enabled)
                        {
                            self.notice = error.to_string();
                        }
                    }
                }
            }
            ChromeAction::Fps(fps) => {
                let target = match fps {
                    30 => TargetFps::Fps30,
                    60 => TargetFps::Fps60,
                    90 => TargetFps::Fps90,
                    120 => TargetFps::Fps120,
                    _ => return,
                };
                self.request_fps(target, cx);
            }
            ChromeAction::Bitrate(limit) => self.request_bitrate(limit),
            ChromeAction::Resolution(width, height, refresh_rate) => {
                self.request_resolution(
                    ResolutionMode {
                        width,
                        height,
                        refresh_rate,
                    },
                    window,
                    cx,
                );
            }
            ChromeAction::AddVirtualDisplay => {
                self.request_virtual_display(VirtualDisplayRequest::Add, window, cx);
            }
            ChromeAction::RemoveVirtualDisplay(id) => {
                self.request_virtual_display(VirtualDisplayRequest::Remove(id), window, cx);
            }
            ChromeAction::Monitor(index) => self.switch_monitor(index, window, cx),
            ChromeAction::ArmRightClick => {
                let armed = self.input.right_click_armed();
                self.release_input();
                self.hide_keyboard(window, cx);
                if !armed && self.input_allowed() && !self.input.arm_right_click() {
                    self.notice = "请先松开画面上的触点。".into();
                }
            }
        }
        self.prepare_cursor(window);
        self.publish_chrome(cx);
        self.arm_controls_hide(window, cx);
        cx.notify();
    }
}

fn stage_geometry(
    bounds: Bounds<Pixels>,
    layout: SessionLayout,
    extent: (u32, u32),
    origin: (i32, i32),
) -> (Geometry, ControlRegions) {
    let global = |rect: Rect| {
        (
            f32::from(bounds.origin.x) + rect.x - layout.stage.x,
            f32::from(bounds.origin.y) + rect.y - layout.stage.y,
            rect.width,
            rect.height,
        )
    };
    (
        Geometry {
            bounds: global(layout.video),
            extent,
            origin,
        },
        layout
            .mouse
            .map(|mouse| ControlRegions {
                left: Some(global(mouse.left)),
                right: Some(global(mouse.right)),
                middle: Some(global(mouse.middle)),
            })
            .unwrap_or_default(),
    )
}

fn at(rect: Rect) -> gpui::Div {
    div()
        .absolute()
        .left(px(rect.x))
        .top(px(rect.y))
        .w(px(rect.width))
        .h(px(rect.height))
}

impl Drop for SessionView {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = window.fully_visible_bounds().size;
        let keyboard_height = match self.keyboard_page {
            KeyboardPage::Input => 88.,
            KeyboardPage::Keys => 176.,
            KeyboardPage::Shortcuts => 132.,
        };
        let layout = session_layout::layout(
            visible.width.into(),
            visible.height.into(),
            self.rail_expanded,
            self.panel,
            self.input.mode() == InputMode::Pointer && self.mouse_buttons && self.input_allowed(),
            keyboard_height,
        );
        if self.layout != layout {
            self.layout = layout;
            self.publish_chrome(cx);
        }
        let owner = cx.entity_id();
        let generation = self.generation;
        let extent = self.extent;
        let origin = self.origin;
        let enabled = self.input_allowed();
        let viewport_bounds = self.viewport_bounds.clone();
        let cursor = self.prepared_cursor.clone();
        let touch = cx.listener(
            move |view, event: &(TouchEvent, Geometry, ControlRegions), window, cx| {
                view.touch_event(&event.0, event.1, event.2, generation, window, cx);
            },
        );
        let mut root = div()
            .relative()
            .w(visible.width)
            .h(visible.height)
            .overflow_hidden()
            .track_focus(&self.focus)
            .bg(rgb(0x080a0e))
            .child(
                at(layout.video).overflow_hidden().children(
                    self.presentation
                        .image()
                        .filter(|_| self.connected && self.extent.0 > 0 && self.extent.1 > 0)
                        .map(|image| remote_image(image.clone())),
                ),
            )
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        cursor.paint(bounds, extent, origin, window);
                    },
                )
                .absolute()
                .left(px(layout.video.x))
                .top(px(layout.video.y))
                .w(px(layout.video.width))
                .h(px(layout.video.height)),
            )
            .child(
                self.chrome.clone().cached(
                    StyleRefinement::default()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(visible.width)
                        .h(visible.height),
                ),
            );
        if let Some(keyboard_bounds) = layout.keyboard {
            let mut header = div()
                .h(px(44.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_1();
            for (id, label, page) in [
                ("session-keyboard-input", "输入法", KeyboardPage::Input),
                (
                    "session-keyboard-shortcuts",
                    "快捷键",
                    KeyboardPage::Shortcuts,
                ),
                ("session-keyboard-keys", "控制键", KeyboardPage::Keys),
            ] {
                header = header.child(
                    div()
                        .id(id)
                        .h(px(44.))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.))
                        .bg(rgb(if self.keyboard_page == page {
                            0x267cf5
                        } else {
                            0x272c35
                        }))
                        .text_color(rgb(0xf3f6fa))
                        .text_size(px(13.))
                        .child(label)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.keyboard_page(page, window, cx)
                        })),
                );
            }
            header = header.child(
                div()
                    .id("session-keyboard-close")
                    .size(px(44.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.))
                    .text_color(rgb(0xf3f6fa))
                    .child("×")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.chrome_action(ChromeAction::ClosePanel, window, cx);
                    })),
            );
            root = root.child(
                at(keyboard_bounds)
                    .occlude()
                    .bg(rgb(0x1b1e25))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .children(self.keyboard.clone()),
                    ),
            );
        }
        root.child(
            canvas(
                move |bounds, window, _| {
                    viewport_bounds.set(bounds);
                    (
                        bounds,
                        enabled.then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal)),
                    )
                },
                move |_, (bounds, hitbox), window, _| {
                    let (geometry, controls) = stage_geometry(bounds, layout, extent, origin);
                    if let Some(hitbox) = hitbox {
                        window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                            touch(&(event.clone(), geometry, controls), window, cx);
                        });
                    }
                },
            )
            .absolute()
            .left(px(layout.stage.x))
            .top(px(layout.stage.y))
            .w(px(layout.stage.width))
            .h(px(layout.stage.height)),
        )
    }
}
