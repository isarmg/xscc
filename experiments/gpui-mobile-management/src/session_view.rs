use crate::{presentation::VideoPresentation, theme};
use gpui::{
    AppContext, AppLifecyclePhase, Context, ImageSource, IntoElement, ObjectFit, Render,
    Subscription, Window, div, img, prelude::*, px, rgb,
};
use librustdesk::gpui_session::{self, SessionEvent, SessionHandle};
use std::sync::{Arc, mpsc};
use xscc_native_management::manager::NativeManager;

enum Reply {
    Connected(u64, Result<SessionHandle, String>),
}

pub struct SessionView {
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
        self.connecting = Some(self.runtime.spawn(async move {
            let result = gpui_session::connect(&device_id)
                .await
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
        match self.presentation.update(frame, window) {
            Ok(Some(_)) => self.notice = "仅查看画面 · 音频、输入和剪贴板已禁用".into(),
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
        let palette = theme::palette(window.appearance());
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(div().child(self.notice.clone()))
            .child(
                div()
                    .w_full()
                    .h(px(280.))
                    .bg(rgb(0x101010))
                    .overflow_hidden()
                    .children(self.presentation.image().map(|image| {
                        img(ImageSource::Render(image.clone()))
                            .size_full()
                            .object_fit(ObjectFit::Contain)
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
