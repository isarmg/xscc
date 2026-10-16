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
    /// Tightly packed RGBA8. This beta copies decoded CPU pixels, not a GPU texture.
    pub rgba: Vec<u8>,
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
}
impl Default for Mailbox {
    fn default() -> Self {
        Self {
            frame: ArrayQueue::new(1),
            events: ArrayQueue::new(64),
            connected: AtomicBool::new(false),
            keyboard: AtomicBool::new(true),
            closed: AtomicBool::new(false),
        }
    }
}
#[derive(Clone, Default)]
struct Handler(Arc<Mailbox>);
impl Handler {
    fn event(&self, event: SessionEvent) {
        self.0.events.force_push(event);
    }
}

pub struct SessionHandle {
    session: Session<Handler>,
}
impl SessionHandle {
    pub fn take_frame(&self) -> Option<Frame> {
        self.session.ui_handler.0.frame.pop()
    }
    pub fn drain_events(&self) -> Vec<SessionEvent> {
        let queue = &self.session.ui_handler.0.events;
        (0..64).filter_map(|_| queue.pop()).collect()
    }
    pub fn send(&self, message: Message) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        if !mailbox.keyboard.load(Ordering::Acquire) {
            bail!("Remote input permission is disabled");
        }
        // The native surface exposes input only; it cannot inject login or trust decisions.
        if !matches!(
            message.union.as_ref(),
            Some(message::Union::KeyEvent(_)) | Some(message::Union::MouseEvent(_))
        ) {
            bail!("Unsupported native input message");
        }
        let sender = self.session.sender.read().unwrap();
        let Some(sender) = sender.as_ref() else {
            bail!("Session is disconnected");
        };
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
        handler.0.closed.store(true, Ordering::Release);
        handler.0.frame.pop();
        handler.event(SessionEvent::Disconnected);
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
            self.0.frame.force_push(frame);
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
    let mut rgba = Vec::with_capacity(row.checked_mul(image.h)?);
    for source in image.raw.chunks_exact(stride).take(image.h) {
        rgba.extend_from_slice(&source[..row]);
    }
    match image.fmt {
        scrap::ImageFormat::ARGB => {
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        scrap::ImageFormat::ABGR => {}
        scrap::ImageFormat::Raw => return None,
    }
    Some(Frame {
        display,
        width: image.w as u32,
        height: image.h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bgra_is_converted_and_padding_removed() {
        let image = scrap::ImageRgb {
            raw: vec![1, 2, 3, 255, 0, 0, 0, 0, 4, 5, 6, 255, 0, 0, 0, 0],
            w: 1,
            h: 2,
            fmt: scrap::ImageFormat::ARGB,
            align: 8,
        };
        assert_eq!(
            copy_frame(0, &image).unwrap().rgba,
            vec![3, 2, 1, 255, 6, 5, 4, 255]
        );
    }
    #[test]
    fn latest_frame_replaces_old_frame_without_waiting() {
        let handler = Handler::default();
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
        assert_eq!(handler.0.frame.pop().unwrap().rgba[0], 9);
        assert!(handler.0.frame.pop().is_none());
    }
    #[test]
    fn native_surface_rejects_login_and_input_after_disconnect() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        handle
            .session
            .ui_handler
            .0
            .connected
            .store(true, Ordering::Release);
        let mut login = Message::new();
        login.set_login_request(LoginRequest::default());
        assert!(handle
            .send(login)
            .unwrap_err()
            .to_string()
            .contains("Unsupported"));
        handle.disconnect();
        let mut key = Message::new();
        key.set_key_event(KeyEvent::default());
        assert!(handle
            .send(key)
            .unwrap_err()
            .to_string()
            .contains("not connected"));
    }
}
