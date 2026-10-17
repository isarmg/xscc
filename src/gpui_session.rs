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
use xscc_native_management::manager::Request;
#[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
pub use crate::client::mobile_audio::AudioStatus;

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

#[cfg(any(test, all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session")))]
fn mobile_pointer_permitted(enabled: bool, input: &InputCommand) -> bool {
    enabled && matches!(input, InputCommand::Mouse(_))
}

#[cfg(any(test, all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session")))]
fn mobile_keyboard_permitted(enabled: bool, platform: &str, input: &InputCommand) -> bool {
    if !enabled || !matches!(platform, "Mac OS" | "Windows" | "Linux") { return false; }
    let InputCommand::Key(event) = input else { return false; };
    if event.down || event.mode.enum_value() != Ok(KeyboardMode::Legacy) { return false; }
    if !event.modifiers.is_empty() {
        // Only three one-shot legacy shortcuts. The existing Windows/Linux host
        // handler releases modifiers it presses within the same input operation.
        return matches!(platform, "Windows" | "Linux") && event.press
            && event.modifiers.len() == 1
            && event.modifiers[0].enum_value() == Ok(ControlKey::Control)
            && matches!(event.union, Some(key_event::Union::Chr(97 | 122 | 115)));
    }
    match &event.union {
        Some(key_event::Union::Seq(text)) => !event.press && !text.is_empty() && text.chars().all(|ch| {
            !ch.is_control() && (platform != "Linux" || ch.is_ascii() && (' '..='~').contains(&ch))
        }),
        Some(key_event::Union::ControlKey(key)) => event.press && matches!(key.enum_value(), Ok(
            ControlKey::Backspace | ControlKey::Delete | ControlKey::Return | ControlKey::Tab |
            ControlKey::Escape | ControlKey::LeftArrow | ControlKey::RightArrow |
            ControlKey::UpArrow | ControlKey::DownArrow
        )),
        _ => false,
    }
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
pub struct Monitor {
    pub index: usize,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub available: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SessionQuality {
    /// Successful decoder output callbacks per second, before GPUI presentation.
    pub decoded_fps: Option<i32>,
    pub receive_rate: Option<String>,
    pub codec: Option<String>,
    pub round_trip_ms: Option<i32>,
}

impl SessionQuality {
    pub fn merge(&mut self, update: Self) {
        if update.decoded_fps.is_some() {
            self.decoded_fps = update.decoded_fps;
        }
        if update.receive_rate.is_some() {
            self.receive_rate = update.receive_rate;
        }
        if update.codec.is_some() {
            self.codec = update.codec;
        }
        if update.round_trip_ms.is_some() {
            self.round_trip_ms = update.round_trip_ms;
        }
    }
}

#[derive(Default)]
struct Monitors {
    list: Vec<Monitor>,
    current: usize,
    pending: Option<usize>,
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
    Monitors(Vec<Monitor>),
    Quality {
        index: usize,
        update: SessionQuality,
    },
    Display {
        index: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

struct Mailbox {
    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    platform: RwLock<String>,
    frame: ArrayQueue<Frame>,
    events: ArrayQueue<SessionEvent>,
    monitors: RwLock<Monitors>,
    connected: AtomicBool,
    keyboard: AtomicBool,
    closed: AtomicBool,
    updates: tokio::sync::Notify,
}
impl Default for Mailbox {
    fn default() -> Self {
        Self {
            #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
            platform: RwLock::new(String::new()),
            frame: ArrayQueue::new(1),
            events: ArrayQueue::new(64),
            monitors: RwLock::new(Monitors::default()),
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
        let monitors = mailbox.monitors.read().unwrap();
        mailbox
            .frame
            .pop()
            .filter(|frame| monitors.pending.is_none() && frame.display == monitors.current)
    }
    pub fn drain_events(&self) -> Vec<SessionEvent> {
        let queue = &self.session.ui_handler.0.events;
        (0..64).map_while(|_| queue.pop()).collect()
    }
    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    pub fn touch_input_allowed(&self) -> bool {
        let mailbox = &self.session.ui_handler.0;
        self.session.lc.read().unwrap().mobile_touch_enabled
            && mailbox.connected.load(Ordering::Acquire)
            && !mailbox.closed.load(Ordering::Acquire)
            && mailbox.keyboard.load(Ordering::Acquire)
            && mailbox.monitors.read().unwrap().pending.is_none()
    }

    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    pub fn keyboard_input_allowed(&self) -> bool {
        let mailbox = &self.session.ui_handler.0;
        self.session.lc.read().unwrap().mobile_keyboard_enabled
            && mailbox.connected.load(Ordering::Acquire)
            && !mailbox.closed.load(Ordering::Acquire)
            && mailbox.keyboard.load(Ordering::Acquire)
            && mailbox.monitors.read().unwrap().pending.is_none()
            && matches!(mailbox.platform.read().unwrap().as_str(), "Mac OS" | "Windows" | "Linux")
    }

    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    pub fn native_unicode_supported(&self) -> bool {
        matches!(self.session.ui_handler.0.platform.read().unwrap().as_str(), "Mac OS" | "Windows")
    }

    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    pub fn common_shortcuts_supported(&self) -> bool {
        self.keyboard_input_allowed()
            && matches!(self.session.ui_handler.0.platform.read().unwrap().as_str(), "Windows" | "Linux")
    }

    pub fn send_input(&self, input: InputCommand) -> ResultType<()> {
        #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
        {
            let config = self.session.lc.read().unwrap();
            if !mobile_pointer_permitted(config.mobile_touch_enabled, &input)
                && !mobile_keyboard_permitted(config.mobile_keyboard_enabled,
                    &self.session.ui_handler.0.platform.read().unwrap(), &input) {
                bail!("Mobile input capability or target text support does not permit this command");
            }
        }
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        if mailbox.monitors.read().unwrap().pending.is_some() {
            bail!("Display switch is pending");
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
    pub fn switch_monitor(&self, index: usize) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        {
            let sender = self.session.sender.read().unwrap();
            if sender.as_ref().is_none_or(|sender| sender.is_closed()) {
                bail!("Session is disconnected");
            }
        }
        {
            let mut monitors = mailbox.monitors.write().unwrap();
            if !monitors
                .list
                .iter()
                .any(|monitor| monitor.index == index && monitor.available)
            {
                bail!("Display is unavailable");
            }
            if monitors.pending.is_some() {
                bail!("Display switch is pending");
            }
            if monitors.current == index {
                return Ok(());
            }
            monitors.pending = Some(index);
            mailbox.frame.pop();
        }
        self.session.switch_display(index as i32);
        Ok(())
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
    #[cfg(all(
        any(target_os = "ios", target_os = "android"),
        feature = "gpui-mobile-session"
    ))]
    pub fn audio_status(&self) -> AudioStatus {
        self.session
            .lc
            .read()
            .unwrap()
            .mobile_audio
            .as_ref()
            .map(|audio| audio.status())
            .unwrap_or_else(|| AudioStatus {
                error: Some("Audio playback is unavailable.".into()),
                ..Default::default()
            })
    }

    #[cfg(all(
        any(target_os = "ios", target_os = "android"),
        feature = "gpui-mobile-session"
    ))]
    pub fn set_audio_enabled(&self, enabled: bool) -> ResultType<()> {
        let config = self.session.lc.read().unwrap();
        if enabled && !config.mobile_audio_allowed {
            bail!("The remote device has disabled audio.");
        }
        let audio = config
            .mobile_audio
            .as_ref()
            .ok_or_else(|| hbb_common::anyhow::anyhow!("Audio playback is unavailable."))?;
        if !enabled {
            audio.set_enabled(false);
        }
        let mailbox = &self.session.ui_handler.0;
        if enabled
            && (!mailbox.connected.load(Ordering::Acquire)
                || mailbox.closed.load(Ordering::Acquire))
        {
            bail!("Session is not connected");
        }
        let mut option = OptionMessage::new();
        option.disable_audio = if enabled {
            option_message::BoolOption::No
        } else {
            option_message::BoolOption::Yes
        }
        .into();
        let mut misc = Misc::new();
        misc.set_option(option);
        let mut message = Message::new();
        message.set_misc(misc);
        let sender = self.session.sender.read().unwrap();
        if let Some(sender) = sender.as_ref() {
            sender
                .send(Data::Message(message))
                .map_err(|_| hbb_common::anyhow::anyhow!("Session is disconnected"))?;
        } else if enabled {
            bail!("Session is disconnected");
        }
        if enabled {
            audio.set_enabled(true);
        }
        Ok(())
    }

    pub fn disconnect(&self) {
        #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
        if let Some(audio) = &self.session.lc.read().unwrap().mobile_audio {
            audio.stop();
        }
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

pub async fn connect(device_id: &str) -> ResultType<SessionHandle> {
    connect_mode(device_id, false, false).await
}

#[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
pub async fn connect_touch(device_id: &str) -> ResultType<SessionHandle> {
    connect_mode(device_id, true, false).await
}

#[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
pub async fn connect_interactive(device_id: &str) -> ResultType<SessionHandle> {
    connect_mode(device_id, true, true).await
}

async fn connect_mode(device_id: &str, _touch_enabled: bool, _keyboard_enabled: bool) -> ResultType<SessionHandle> {
    let device_id = uuid::Uuid::parse_str(device_id)?;
    // Resolve through the existing authenticated owner. Never accept an address or identity
    // asserted by the UI; the transport resolves again immediately before the handshake.
    crate::management::rpc(Request::Target { device_id }).await?;
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
    #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
    {
        let mut config = session.lc.write().unwrap();
        config.mobile_touch_enabled = _touch_enabled;
        config.mobile_keyboard_enabled = _keyboard_enabled;
        config.mobile_audio_allowed = true;
        let mailbox = Arc::downgrade(&handler.0);
        match crate::client::mobile_audio::MobileAudio::new(move || {
            if let Some(mailbox) = mailbox.upgrade() {
                mailbox.updates.notify_one();
            }
        }) {
            Ok(audio) => config.mobile_audio = Some(audio),
            Err(error) => handler.event(SessionEvent::Error(format!(
                "Audio playback unavailable: {error}"
            ))),
        }
    }
    let round = session.connection_round_state.lock().unwrap().new_round();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    *session.sender.write().unwrap() = Some(sender.clone());
    let running = session.clone();
    tokio::spawn(async move {
        #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
        let audio = running.lc.read().unwrap().mobile_audio.clone();
        let mut remote = Remote::new(running, receiver, sender);
        remote.io_loop(round).await;
        #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
        if let Some(audio) = audio {
            audio.stop();
        }
        handler.0.closed.store(true, Ordering::Release);
        handler.0.connected.store(false, Ordering::Release);
        handler.0.frame.pop();
        handler.event(SessionEvent::Disconnected);
        handler.0.updates.notify_one();
    });
    Ok(SessionHandle { session })
}

#[cfg(feature = "test-lan-direct")]
pub async fn connect_test_lan(endpoint: &str) -> ResultType<SessionHandle> {
    let endpoint = crate::test_lan::target(endpoint)?;
    let handler = Handler::default();
    handler.event(SessionEvent::Connecting);
    let session = Session {
        ui_handler: handler.clone(),
        server_keyboard_enabled: Arc::new(RwLock::new(true)),
        server_clipboard_enabled: Arc::new(RwLock::new(false)),
        ..Default::default()
    };
    session.lc.write().unwrap().initialize(
        format!("{}{}", crate::test_lan::PREFIX, endpoint),
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
        handler.0.closed.store(true, Ordering::Release);
        handler.0.connected.store(false, Ordering::Release);
        handler.0.frame.pop();
        handler.event(SessionEvent::Disconnected);
        handler.0.updates.notify_one();
    });
    Ok(SessionHandle { session })
}

impl InvokeUiSession for Handler {
    fn on_rgba(&self, display: usize, image: &mut scrap::ImageRgb) {
        if self.0.closed.load(Ordering::Acquire) {
            return;
        }
        {
            let monitors = self.0.monitors.read().unwrap();
            if monitors.pending.is_some() || monitors.current != display {
                return;
            }
        }
        if let Some(frame) = copy_frame(display, image) {
            let monitors = self.0.monitors.read().unwrap();
            if monitors.pending.is_some() || monitors.current != display {
                return;
            }
            if self.0.closed.load(Ordering::Acquire) {
                return;
            }
            // Replacing an unread frame needs no second UI wake: the pending
            // render will take the newest frame from the same mailbox.
            if self.0.frame.force_push(frame).is_none() {
                self.0.updates.notify_one();
            }
        }
    }
    fn set_display(&self, x: i32, y: i32, width: i32, height: i32, _: bool, _: f64) {
        self.event(SessionEvent::Display {
            index: self.0.monitors.read().unwrap().current,
            x,
            y,
            width,
            height,
        });
    }
    fn switch_display(&self, display: &SwitchDisplay) {
        if display.display < 0 || display.width <= 0 || display.height <= 0 {
            return;
        }
        {
            let mut monitors = self.0.monitors.write().unwrap();
            monitors.current = display.display as usize;
            monitors.pending = None;
            self.0.frame.pop();
        }
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
        #[cfg(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session"))]
        { *self.0.platform.write().unwrap() = peer.platform.clone(); }
        self.set_displays(&peer.displays);
        self.0.monitors.write().unwrap().current = peer.current_display.max(0) as usize;
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
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn clipboard(&self, _: String) {}
    fn set_cursor_data(&self, _: CursorData) {}
    fn set_cursor_id(&self, _: String) {}
    fn set_cursor_position(&self, _: CursorPosition) {}
    fn set_displays(&self, displays: &Vec<DisplayInfo>) {
        let list: Vec<_> = displays
            .iter()
            .enumerate()
            .map(|(index, display)| Monitor {
                index,
                name: display.name.clone(),
                width: display.width,
                height: display.height,
                available: display.online && display.width > 0 && display.height > 0,
            })
            .collect();
        let pending_unavailable = {
            let mut monitors = self.0.monitors.write().unwrap();
            let pending_unavailable = monitors.pending.is_some_and(|pending| {
                !list
                    .iter()
                    .any(|monitor| monitor.index == pending && monitor.available)
            });
            monitors.list = list.clone();
            pending_unavailable
        };
        self.event(SessionEvent::Monitors(list));
        if pending_unavailable {
            self.event(SessionEvent::Error(
                "Requested display disconnected. Reconnect and select an available display.".into(),
            ));
        }
    }
    fn set_platform_additions(&self, _: &str) {}
    fn on_connected(&self, _: ConnType) {}
    fn update_privacy_mode(&self) {}
    fn update_quality_status(&self, status: QualityStatus) {
        let monitors = self.0.monitors.read().unwrap();
        if self.0.closed.load(Ordering::Acquire) || monitors.pending.is_some() {
            return;
        }
        self.event(SessionEvent::Quality {
            index: monitors.current,
            update: SessionQuality {
                decoded_fps: status.fps.get(&monitors.current).copied(),
                receive_rate: status.speed,
                codec: status.codec_format.map(|codec| codec.to_string()),
                round_trip_ms: status.delay.filter(|delay| *delay > 0),
            },
        });
    }
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
    fn disconnect_discards_queued_and_late_decoder_frames() {
        let handle = SessionHandle { session: Session::default() };
        let mut image = scrap::ImageRgb { raw: vec![1, 2, 3, 255], w: 1, h: 1, fmt: scrap::ImageFormat::ARGB, align: 1 };
        handle.session.ui_handler.on_rgba(0, &mut image);
        assert!(handle.take_frame().is_some());
        handle.session.ui_handler.on_rgba(0, &mut image);
        handle.disconnect();
        assert!(handle.take_frame().is_none());
        handle.session.ui_handler.on_rgba(0, &mut image);
        assert!(handle.take_frame().is_none());
    }

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
    #[test]
    fn quality_reports_active_decoder_output_and_merges_partial_network_updates() {
        let handler = Handler::default();
        handler.0.monitors.write().unwrap().current = 1;
        handler.update_quality_status(QualityStatus {
            fps: [(0, 90), (1, 57)].into_iter().collect(),
            speed: Some("123.00kB/s".into()),
            codec_format: Some(scrap::CodecFormat::VP9),
            ..Default::default()
        });
        handler.update_quality_status(QualityStatus {
            delay: Some(23),
            ..Default::default()
        });
        let mut quality = SessionQuality::default();
        for _ in 0..2 {
            let SessionEvent::Quality { index, update } = handler.0.events.pop().unwrap() else {
                panic!("expected quality update")
            };
            assert_eq!(index, 1);
            quality.merge(update);
        }
        assert_eq!(quality.decoded_fps, Some(57));
        assert_eq!(quality.receive_rate.as_deref(), Some("123.00kB/s"));
        assert_eq!(quality.codec, Some(scrap::CodecFormat::VP9.to_string()));
        assert_eq!(quality.round_trip_ms, Some(23));
        handler.0.monitors.write().unwrap().pending = Some(0);
        handler.update_quality_status(QualityStatus::default());
        assert!(handler.0.events.pop().is_none());
    }

    #[test]
    fn monitor_selection_accepts_only_available_advertised_displays() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        let handler = &handle.session.ui_handler;
        let online = DisplayInfo {
            width: 1920,
            height: 1080,
            online: true,
            ..Default::default()
        };
        handler.set_displays(&vec![online.clone(), online, DisplayInfo::default()]);
        assert!(handle.switch_monitor(1).is_err());
        handler.0.connected.store(true, Ordering::Release);
        handler.0.keyboard.store(false, Ordering::Release);
        assert!(handle.switch_monitor(2).is_err());
        assert!(handle.switch_monitor(99).is_err());
        assert!(receiver.try_recv().is_err());
        handle.switch_monitor(1).unwrap();
        let Data::Message(message) = receiver.try_recv().unwrap() else {
            panic!("expected monitor request")
        };
        assert_eq!(message.misc().switch_display().display, 1);
        if !crate::ui_interface::use_texture_render() {
            let Data::Message(message) = receiver.try_recv().unwrap() else {
                panic!("expected single-display capture")
            };
            assert_eq!(message.misc().capture_displays().set, vec![1]);
        }
        assert!(handle.switch_monitor(0).is_err());
    }

    #[test]
    fn removed_pending_monitor_reports_failure_without_resuming_input() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        let handler = &handle.session.ui_handler;
        let online = DisplayInfo {
            width: 1,
            height: 1,
            online: true,
            ..Default::default()
        };
        handler.set_peer_info(&PeerInfo {
            displays: vec![online.clone(), online.clone(), online.clone()],
            ..Default::default()
        });
        handle.switch_monitor(2).unwrap();
        handler.set_displays(&vec![online.clone(), online, DisplayInfo::default()]);
        assert!(handle.drain_events().iter().any(|event| matches!(
            event, SessionEvent::Error(message) if message == "Requested display disconnected. Reconnect and select an available display."
        )));
        assert!(handle
            .send_input(InputCommand::Key(KeyEvent::default()))
            .is_err());
        assert!(handle.switch_monitor(1).is_err());
    }

    #[test]
    fn monitor_switch_waits_for_host_and_rejects_old_display_frames() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        let handler = &handle.session.ui_handler;
        let online = DisplayInfo {
            width: 1,
            height: 1,
            online: true,
            ..Default::default()
        };
        handler.set_peer_info(&PeerInfo {
            displays: vec![online.clone(), online],
            ..Default::default()
        });
        let mut image = scrap::ImageRgb {
            raw: vec![1, 2, 3, 255],
            w: 1,
            h: 1,
            fmt: scrap::ImageFormat::ABGR,
            align: 1,
        };
        handler.on_rgba(0, &mut image);
        handle.switch_monitor(1).unwrap();
        assert!(handle.take_frame().is_none());
        handler.on_rgba(0, &mut image);
        handler.on_rgba(1, &mut image);
        assert!(handle.take_frame().is_none());
        handler.switch_display(&SwitchDisplay {
            display: 1,
            x: -1920,
            y: 100,
            width: 1,
            height: 1,
            ..Default::default()
        });
        handler.on_rgba(1, &mut image);
        handler.on_rgba(0, &mut image);
        assert_eq!(handle.take_frame().unwrap().display, 1);
        assert!(handle.drain_events().iter().any(|event| matches!(
            event,
            SessionEvent::Display {
                index: 1,
                x: -1920,
                y: 100,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn latest_frame_replaces_old_frame_without_waiting() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let handler = &handle.session.ui_handler;
        let updates = handle.updates();
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
        // The notification relay can wake before the UI renders. Further
        // replacements must not wake it again until the pending frame is taken.
        image.raw[0] = 10;
        handler.on_rgba(0, &mut image);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), updates.wait())
                .await
                .is_err()
        );
        assert_eq!(handle.take_frame().unwrap().bgra[2], 10);
        assert!(handle.take_frame().is_none());
        image.raw[0] = 11;
        handler.on_rgba(0, &mut image);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), updates.wait())
                .await
                .unwrap()
        );
        assert_eq!(handle.take_frame().unwrap().bgra[2], 11);
    }
    #[test]
    fn mobile_keyboard_gates_unicode_controls_and_view_only() {
        let text = |value: &str| {
            let mut event = KeyEvent::new();
            event.set_seq(value.into());
            InputCommand::Key(event)
        };
        for platform in ["Mac OS", "Windows"] {
            assert!(mobile_keyboard_permitted(true, platform, &text("你好😀")));
            assert!(!mobile_keyboard_permitted(false, platform, &text("a")));
        }
        assert!(mobile_keyboard_permitted(true, "Linux", &text("ASCII !")));
        for value in ["你好", "😀", "a\nb", "\t", "\u{7f}"] {
            assert!(!mobile_keyboard_permitted(true, "Linux", &text(value)));
        }
        assert!(!mobile_keyboard_permitted(true, "unknown", &text("a")));
        let mut key = KeyEvent::new(); key.press = true; key.set_control_key(ControlKey::Return);
        assert!(mobile_keyboard_permitted(true, "Linux", &InputCommand::Key(key.clone())));
        key.modifiers.push(ControlKey::Control.into());
        assert!(!mobile_keyboard_permitted(true, "Linux", &InputCommand::Key(key)));
    }

    #[test]
    fn mobile_pointer_requires_explicit_capability_and_rejects_keys() {
        let pointer = InputCommand::Mouse(MouseEvent::default());
        assert!(!mobile_pointer_permitted(false, &pointer));
        assert!(mobile_pointer_permitted(true, &pointer));
        assert!(!mobile_pointer_permitted(true, &InputCommand::Key(KeyEvent::default())));
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
        if cfg!(all(any(target_os = "ios", target_os = "android"), feature = "gpui-mobile-session")) {
            assert!(handle.send_input(InputCommand::Key(KeyEvent::default())).is_err());
            assert!(receiver.try_recv().is_err());
            return;
        }
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
    #[test]
    fn mobile_shortcuts_are_three_one_shot_control_chords_only() {
        for platform in ["Windows", "Linux"] {
            for chr in ['a', 'z', 's'] {
                let mut key = KeyEvent::new();
                key.set_chr(chr as u32);
                key.press = true;
                key.modifiers.push(ControlKey::Control.into());
                let input = InputCommand::Key(key.clone());
                assert!(mobile_keyboard_permitted(true, platform, &input));
                assert!(!mobile_keyboard_permitted(false, platform, &input));
                assert!(!mobile_keyboard_permitted(true, "Mac OS", &input));
                assert!(!mobile_keyboard_permitted(true, "unknown", &input));
                key.down = true;
                assert!(!mobile_keyboard_permitted(true, platform, &InputCommand::Key(key.clone())));
                key.down = false;
                key.press = false;
                assert!(!mobile_keyboard_permitted(true, platform, &InputCommand::Key(key.clone())));
                key.press = true;
                key.mode = KeyboardMode::Map.into();
                assert!(!mobile_keyboard_permitted(true, platform, &InputCommand::Key(key.clone())));
                key.mode = KeyboardMode::Legacy.into();
                key.modifiers.push(ControlKey::Shift.into());
                assert!(!mobile_keyboard_permitted(true, platform, &InputCommand::Key(key.clone())));
                key.modifiers = vec![ControlKey::Alt.into()];
                assert!(!mobile_keyboard_permitted(true, platform, &InputCommand::Key(key.clone())));
            }
        }
        for chr in ['c', 'v', 'x', 'q', '中'] {
            let mut key = KeyEvent::new(); key.set_chr(chr as u32); key.press = true;
            key.modifiers.push(ControlKey::Control.into());
            assert!(!mobile_keyboard_permitted(true, "Linux", &InputCommand::Key(key)));
        }
    }

}
