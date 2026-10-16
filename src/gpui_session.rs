//! Native desktop session adapter. The UI never owns a second management identity.
use crate::{
    client::{io_loop::Remote, Data, QualityStatus},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::*;
use crossbeam_queue::ArrayQueue;
use hbb_common::{bail, rendezvous_proto::ConnType, tokio, ResultType};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};
pub use xscc_native_management::manager::Request;

#[derive(Debug)]
pub struct Frame {
    pub display: usize,
    pub width: u32,
    pub height: u32,
    /// Bytes per row, always width * 4 for the packed BGRA8 payload.
    pub stride: usize,
    /// Owned CPU pixels. The decoder can reuse its buffer after this copy returns.
    pub bgra: Vec<u8>,
}

/// Input-only commands; authentication and other protocol messages are not representable.
#[derive(Clone, Debug)]
pub enum InputCommand {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

/// Requested cadence; the host and transport retain their capability and QoS limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetFps {
    Fps30,
    Fps60,
    Fps90,
    Fps120,
}
impl TargetFps {
    pub fn value(self) -> i32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
            Self::Fps90 => 90,
            Self::Fps120 => 120,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Connecting,
    Connected,
    Disconnected,
    Error(String),
    Permission {
        name: String,
        enabled: bool,
    },
    Display {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

struct Mailbox {
    frame: ArrayQueue<Frame>,
    events: ArrayQueue<SessionEvent>,
    connected: AtomicBool,
    keyboard: AtomicBool,
    closed: AtomicBool,
    updates: tokio::sync::Notify,
}
impl Default for Mailbox {
    fn default() -> Self {
        Self {
            frame: ArrayQueue::new(1),
            events: ArrayQueue::new(64),
            connected: AtomicBool::new(false),
            keyboard: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            updates: tokio::sync::Notify::new(),
        }
    }
}
#[derive(Clone, Default)]
struct Handler(Arc<Mailbox>);
impl Handler {
    fn event(&self, event: SessionEvent) {
        self.0.events.force_push(event);
        self.0.updates.notify_one();
    }
}

/// One logical UI consumer. Notifications coalesce; payloads stay in the session mailbox.
pub struct SessionUpdates(Arc<Mailbox>);
impl SessionUpdates {
    /// Drain pending events/frames after every wake, including the final false result.
    /// False means the session is closed and the consumer should stop waiting.
    pub async fn wait(&self) -> bool {
        let notified = self.0.updates.notified();
        if self.0.closed.load(Ordering::Acquire) {
            return false;
        }
        notified.await;
        !self.0.closed.load(Ordering::Acquire)
    }
}

pub struct SessionHandle {
    session: Session<Handler>,
}
impl SessionHandle {
    pub fn updates(&self) -> SessionUpdates {
        SessionUpdates(self.session.ui_handler.0.clone())
    }
    pub fn take_frame(&self) -> Option<Frame> {
        let mailbox = &self.session.ui_handler.0;
        if mailbox.closed.load(Ordering::Acquire) {
            mailbox.frame.pop();
            return None;
        }
        mailbox.frame.pop()
    }
    pub fn drain_events(&self) -> Vec<SessionEvent> {
        let queue = &self.session.ui_handler.0.events;
        (0..64).map_while(|_| queue.pop()).collect()
    }
    pub fn send_input(&self, input: InputCommand) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        if !mailbox.keyboard.load(Ordering::Acquire) {
            bail!("Remote input permission is disabled");
        }
        let mut message = Message::new();
        match input {
            InputCommand::Key(event) => message.set_key_event(event),
            InputCommand::Mouse(event) => message.set_mouse_event(event),
        }
        let sender = self.session.sender.read().unwrap();
        let Some(sender) = sender.as_ref() else {
            bail!("Session is disconnected");
        };
        sender
            .send(Data::Message(message))
            .map_err(|_| hbb_common::anyhow::anyhow!("Session is disconnected"))
    }
    pub fn set_target_fps(&self, target: TargetFps) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        let sender = self.session.sender.read().unwrap();
        let Some(sender) = sender.as_ref() else {
            bail!("Session is disconnected");
        };
        let message = self
            .session
            .lc
            .write()
            .unwrap()
            .set_custom_fps(target.value(), false);
        sender
            .send(Data::Message(message))
            .map_err(|_| hbb_common::anyhow::anyhow!("Session is disconnected"))
    }
    pub fn disconnect(&self) {
        self.session
            .ui_handler
            .0
            .closed
            .store(true, Ordering::Release);
        self.session
            .ui_handler
            .0
            .connected
            .store(false, Ordering::Release);
        self.session.close();
        self.session.ui_handler.0.frame.pop();
        self.session.ui_handler.0.updates.notify_one();
    }
}
impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.disconnect();
    }
}

pub async fn management_request(request: Request) -> ResultType<serde_json::Value> {
    crate::management::rpc(request).await
}

pub async fn connect(device_id: &str) -> ResultType<SessionHandle> {
    let device_id = uuid::Uuid::parse_str(device_id)?;
    // Resolve through the existing authenticated owner. Never accept an address or identity
    // asserted by the UI; the transport resolves again immediately before the handshake.
    management_request(Request::Target { device_id }).await?;
    let handler = Handler::default();
    handler.event(SessionEvent::Connecting);
    let session = Session {
        ui_handler: handler.clone(),
        server_keyboard_enabled: Arc::new(RwLock::new(true)),
        server_clipboard_enabled: Arc::new(RwLock::new(false)),
        ..Default::default()
    };
    session.lc.write().unwrap().initialize(
        format!("managed:{device_id}"),
        ConnType::DEFAULT_CONN,
        None,
        None,
        None,
    );
    let round = session.connection_round_state.lock().unwrap().new_round();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    *session.sender.write().unwrap() = Some(sender.clone());
    let running = session.clone();
    tokio::spawn(async move {
        let mut remote = Remote::new(running, receiver, sender);
        remote.io_loop(round).await;
        handler.0.connected.store(false, Ordering::Release);
        handler.0.frame.pop();
        handler.event(SessionEvent::Disconnected);
        handler.0.closed.store(true, Ordering::Release);
        handler.0.updates.notify_one();
    });
    Ok(SessionHandle { session })
}

// Startup hooks are kept separate from the session adapter for the native role entry points.
pub fn initialize() -> Result<(), String> {
    crate::gpui_host::initialize()
}
pub fn handle_background_command() -> bool {
    crate::gpui_host::handle_background_command()
}

impl InvokeUiSession for Handler {
    fn on_rgba(&self, display: usize, image: &mut scrap::ImageRgb) {
        if self.0.closed.load(Ordering::Acquire) {
            return;
        }
        if let Some(frame) = copy_frame(display, image) {
            if self.0.closed.load(Ordering::Acquire) {
                return;
            }
            self.0.frame.force_push(frame);
            self.0.updates.notify_one();
        }
    }
    fn set_display(&self, x: i32, y: i32, width: i32, height: i32, _: bool, _: f64) {
        self.event(SessionEvent::Display {
            x,
            y,
            width,
            height,
        });
    }
    fn switch_display(&self, display: &SwitchDisplay) {
        self.set_display(
            display.x,
            display.y,
            display.width,
            display.height,
            false,
            1.0,
        );
    }
    fn set_peer_info(&self, peer: &PeerInfo) {
        if let Some(display) = peer.displays.get(peer.current_display as usize) {
            self.set_display(
                display.x,
                display.y,
                display.width,
                display.height,
                false,
                1.0,
            );
        }
        self.0.connected.store(true, Ordering::Release);
        self.event(SessionEvent::Connected);
        self.event(SessionEvent::Permission {
            name: "keyboard".to_owned(),
            enabled: self.0.keyboard.load(Ordering::Acquire),
        });
    }
    fn set_permission(&self, name: &str, enabled: bool) {
        if name == "keyboard" {
            self.0.keyboard.store(enabled, Ordering::Release);
        }
        self.event(SessionEvent::Permission {
            name: name.to_owned(),
            enabled,
        });
    }
    fn msgbox(&self, kind: &str, title: &str, text: &str, _: &str, _: bool) {
        if kind.contains("error") || kind.contains("password") {
            self.event(SessionEvent::Error(format!("{title}: {text}")));
        }
    }
    fn close_success(&self) {}
    #[cfg(feature = "vram")]
    fn on_texture(&self, _: usize, _: *mut std::ffi::c_void) {
        self.event(SessionEvent::Error(
            "GPU texture import is unavailable in the CPU beta renderer".into(),
        ));
    }
    #[cfg(feature = "flutter")]
    fn is_multi_ui_session(&self) -> bool {
        false
    }
    fn set_cursor_data(&self, _: CursorData) {}
    fn set_cursor_id(&self, _: String) {}
    fn set_cursor_position(&self, _: CursorPosition) {}
    fn set_displays(&self, _: &Vec<DisplayInfo>) {}
    fn set_platform_additions(&self, _: &str) {}
    fn on_connected(&self, _: ConnType) {}
    fn update_privacy_mode(&self) {}
    fn update_quality_status(&self, _: QualityStatus) {}
    fn set_connection_type(&self, _: bool, _: bool, _: &str) {}
    fn set_fingerprint(&self, _: String) {}
    fn update_block_input_state(&self, _: bool) {}
    fn adapt_size(&self) {}
    fn cancel_msgbox(&self, _: &str) {}
    fn portable_service_running(&self, _: bool) {}
    fn get_rgba(&self, _: usize) -> *const u8 {
        std::ptr::null()
    }
    fn next_rgba(&self, _: usize) {}
    fn set_multiple_windows_session(&self, _: Vec<WindowsSession>) {}
    fn set_current_display(&self, _: i32) {}
    fn handle_screenshot_resp(&self, _: String, _: String) {}
}

fn copy_frame(display: usize, image: &scrap::ImageRgb) -> Option<Frame> {
    if matches!(image.fmt, scrap::ImageFormat::Raw) {
        return None;
    }
    if image.w == 0 || image.h == 0 || image.w > 16384 || image.h > 16384 {
        return None;
    }
    let row = image.w.checked_mul(4)?;
    let align = image.align.max(1);
    let stride = row
        .checked_add(align - 1)?
        .checked_div(align)?
        .checked_mul(align)?;
    if image.raw.len() < stride.checked_mul(image.h)? {
        return None;
    }
    let mut bgra = Vec::with_capacity(row.checked_mul(image.h)?);
    for source in image.raw.chunks_exact(stride).take(image.h) {
        bgra.extend_from_slice(&source[..row]);
    }
    match image.fmt {
        scrap::ImageFormat::ABGR => {
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        scrap::ImageFormat::ARGB => {}
        scrap::ImageFormat::Raw => return None,
    }
    Some(Frame {
        display,
        width: image.w as u32,
        height: image.h as u32,
        stride: row,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bgra_keeps_channels_and_removes_padding() {
        let image = scrap::ImageRgb {
            raw: vec![1, 2, 3, 255, 0, 0, 0, 0, 4, 5, 6, 255, 0, 0, 0, 0],
            w: 1,
            h: 2,
            fmt: scrap::ImageFormat::ARGB,
            align: 8,
        };
        let frame = copy_frame(0, &image).unwrap();
        assert_eq!((frame.width, frame.height, frame.stride), (1, 2, 4));
        assert_eq!(frame.bgra, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let rgba = scrap::ImageRgb {
            fmt: scrap::ImageFormat::ABGR,
            ..image
        };
        assert_eq!(
            copy_frame(0, &rgba).unwrap().bgra,
            vec![3, 2, 1, 255, 6, 5, 4, 255]
        );
        let raw = scrap::ImageRgb {
            fmt: scrap::ImageFormat::Raw,
            ..rgba
        };
        assert!(copy_frame(0, &raw).is_none());
    }
    #[tokio::test]
    async fn latest_frame_replaces_old_frame_without_waiting() {
        let handler = Handler::default();
        let updates = SessionUpdates(handler.0.clone());
        let mut image = scrap::ImageRgb {
            raw: vec![1, 2, 3, 255],
            w: 1,
            h: 1,
            fmt: scrap::ImageFormat::ABGR,
            align: 1,
        };
        handler.on_rgba(0, &mut image);
        image.raw[0] = 9;
        handler.on_rgba(0, &mut image);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), updates.wait())
                .await
                .unwrap()
        );
        assert_eq!(handler.0.frame.pop().unwrap().bgra[2], 9);
        assert!(handler.0.frame.pop().is_none());
    }
    #[test]
    fn typed_input_preserves_key_order_and_rejects_disconnected_input() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        handle
            .session
            .ui_handler
            .0
            .connected
            .store(true, Ordering::Release);
        for down in [true, false] {
            handle
                .send_input(InputCommand::Key(KeyEvent {
                    down,
                    ..Default::default()
                }))
                .unwrap();
        }
        for down in [true, false] {
            let Data::Message(message) = receiver.try_recv().unwrap() else {
                panic!("Expected key message");
            };
            let Some(message::Union::KeyEvent(key)) = message.union else {
                panic!("Expected key input");
            };
            assert_eq!(key.down, down);
        }
        handle.disconnect();
        assert!(handle
            .send_input(InputCommand::Key(KeyEvent::default()))
            .is_err());
    }
    #[test]
    fn target_fps_requires_connected_session_and_sends_only_requested_option() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        assert!(handle.set_target_fps(TargetFps::Fps60).is_err());
        assert!(receiver.try_recv().is_err());
        handle
            .session
            .ui_handler
            .0
            .connected
            .store(true, Ordering::Release);
        for target in [
            TargetFps::Fps30,
            TargetFps::Fps60,
            TargetFps::Fps90,
            TargetFps::Fps120,
        ] {
            handle.set_target_fps(target).unwrap();
            let Data::Message(message) = receiver.try_recv().unwrap() else {
                panic!("Expected option message");
            };
            let Some(message::Union::Misc(misc)) = message.union else {
                panic!("Expected misc message");
            };
            let Some(misc::Union::Option(option)) = misc.union else {
                panic!("Expected FPS option");
            };
            assert_eq!(
                option,
                OptionMessage {
                    custom_fps: target.value(),
                    ..Default::default()
                }
            );
        }
        handle.disconnect();
        assert!(handle.set_target_fps(TargetFps::Fps120).is_err());
    }
    #[tokio::test]
    async fn updates_coalesce_before_waiting_and_wake_on_close() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let updates = handle.updates();
        handle.session.ui_handler.event(SessionEvent::Connecting);
        handle.session.ui_handler.event(SessionEvent::Connected);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), updates.wait())
                .await
                .unwrap()
        );
        assert_eq!(handle.drain_events().len(), 2);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), updates.wait())
                .await
                .is_err()
        );
        let close = async {
            tokio::task::yield_now().await;
            handle.disconnect();
        };
        let (open, ()) = tokio::join!(updates.wait(), close);
        assert!(!open);
        assert!(!updates.wait().await);
        // A decoder callback that arrives after close cannot restore the image.
        let mut image = scrap::ImageRgb {
            raw: vec![1, 2, 3, 255],
            w: 1,
            h: 1,
            fmt: scrap::ImageFormat::ARGB,
            align: 1,
        };
        handle.session.ui_handler.on_rgba(0, &mut image);
        assert!(handle.take_frame().is_none());
    }
}
