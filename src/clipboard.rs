#[cfg(not(target_os = "android"))]
use arboard::{ClipboardData, ClipboardFormat};
#[cfg(target_os = "linux")]
use arboard::{LinuxClipboardKind, SetExtLinux};
use hbb_common::{bail, log, ResultType};
use base::message_proto::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub const CLIPBOARD_NAME: &'static str = "clipboard";
pub const CLIPBOARD_INTERVAL: u64 = 333;

pub const OPTION_ALLOW_SYNC_CLIPBOARD_BETWEEN_SESSIONS: &str =
    "allow-sync-clipboard-between-sessions";

#[cfg(all(feature = "flutter", not(any(target_os = "android", target_os = "ios"))))]
pub fn is_sync_clipboard_between_sessions_enabled() -> bool {
    hbb_common::config::option2bool(
        OPTION_ALLOW_SYNC_CLIPBOARD_BETWEEN_SESSIONS,
        &hbb_common::config::LocalConfig::get_option(OPTION_ALLOW_SYNC_CLIPBOARD_BETWEEN_SESSIONS),
    )
}

// This format is used to store the flag in the clipboard.
const RUSTDESK_CLIPBOARD_OWNER_FORMAT: &'static str = "dyn.com.rustdesk.owner";

#[cfg(not(target_os = "android"))]
lazy_static::lazy_static! {
    static ref ARBOARD_MTX: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
    // For updating in server and getting content in cm.
    // Clipboard on Linux is "server--clients" mode.
    // The clipboard content is owned by the server and passed to the clients when requested.
    // Plain text is the only exception, it does not require the server to be present.
    static ref CLIPBOARD_CTX: Arc<Mutex<Option<ClipboardContext>>> = Arc::new(Mutex::new(None));
}

#[cfg(not(target_os = "android"))]
const CLIPBOARD_GET_MAX_RETRY: usize = 3;
#[cfg(not(target_os = "android"))]
const CLIPBOARD_GET_RETRY_INTERVAL_DUR: Duration = Duration::from_millis(33);

#[cfg(not(target_os = "android"))]
const SUPPORTED_FORMATS: &[ClipboardFormat] = &[
    ClipboardFormat::Text,
    ClipboardFormat::Special(RUSTDESK_CLIPBOARD_OWNER_FORMAT),
];

#[cfg(not(target_os = "android"))]
pub fn check_clipboard(
    ctx: &mut Option<ClipboardContext>,
    side: ClipboardSide,
    force: bool,
) -> Option<Message> {
    let (msg, _) = read_clipboard_message(ctx, side, force)?;
    Some(msg)
}

#[cfg(target_os = "linux")]
pub fn peek_clipboard(
    ctx: &mut Option<ClipboardContext>,
    side: ClipboardSide,
    force: bool,
) -> Option<Message> {
    let (msg, _) = read_clipboard_message(ctx, side, force)?;
    Some(msg)
}

#[cfg(not(target_os = "android"))]
fn read_clipboard_message(
    ctx: &mut Option<ClipboardContext>,
    side: ClipboardSide,
    force: bool,
) -> Option<(Message, MultiClipboards)> {
    if ctx.is_none() {
        *ctx = ClipboardContext::new().ok();
    }
    let ctx2 = ctx.as_mut()?;
    match ctx2.get(side, force) {
        Ok(content) => {
            if !content.is_empty() {
                let mut msg = Message::new();
                let clipboards = proto::create_multi_clipboards(content);
                msg.set_multi_clipboards(clipboards.clone());
                return Some((msg, clipboards));
            }
        }
        Err(e) => {
            log::error!("Failed to get clipboard content. {}", e);
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn check_clipboard_cm() -> ResultType<MultiClipboards> {
    let mut ctx = CLIPBOARD_CTX.lock().unwrap();
    if ctx.is_none() {
        match ClipboardContext::new() {
            Ok(x) => {
                *ctx = Some(x);
            }
            Err(e) => {
                hbb_common::bail!("Failed to create clipboard context: {}", e);
            }
        }
    }
    if let Some(ctx) = ctx.as_mut() {
        let content = ctx.get(ClipboardSide::Host, false)?;
        let clipboards = proto::create_multi_clipboards(content);
        Ok(clipboards)
    } else {
        hbb_common::bail!("Failed to create clipboard context");
    }
}

#[cfg(not(target_os = "android"))]
fn update_clipboard_(multi_clipboards: Vec<Clipboard>, side: ClipboardSide) {
    let to_update_data = proto::from_multi_clipboards(multi_clipboards);
    if to_update_data.is_empty() {
        return;
    }
    do_update_clipboard_(to_update_data, side);
}

#[cfg(not(target_os = "android"))]
fn do_update_clipboard_(mut to_update_data: Vec<ClipboardData>, side: ClipboardSide) {
    let mut ctx = CLIPBOARD_CTX.lock().unwrap();
    if ctx.is_none() {
        match ClipboardContext::new() {
            Ok(x) => {
                *ctx = Some(x);
            }
            Err(e) => {
                log::error!("Failed to create clipboard context: {}", e);
                return;
            }
        }
    }
    if let Some(ctx) = ctx.as_mut() {
        to_update_data = append_owner_marker(to_update_data, side);
        if let Err(e) = ctx.set(&to_update_data) {
            log::debug!("Failed to set clipboard: {}", e);
        } else {
            log::debug!("{} updated on {}", CLIPBOARD_NAME, side);
        }
    }
}

#[cfg(not(target_os = "android"))]
fn append_owner_marker(mut data: Vec<ClipboardData>, side: ClipboardSide) -> Vec<ClipboardData> {
    data.push(ClipboardData::Special((
        RUSTDESK_CLIPBOARD_OWNER_FORMAT.to_owned(),
        side.get_owner_data(),
    )));
    data
}

#[cfg(target_os = "linux")]
pub fn set_text_clipboard_with_owner_sync(text: &str, side: ClipboardSide) -> ResultType<()> {
    let mut ctx = CLIPBOARD_CTX.lock().unwrap();
    if ctx.is_none() {
        *ctx = Some(ClipboardContext::new()?);
    }
    let clipboard_ctx = match ctx.as_mut() {
        Some(ctx) => ctx,
        None => bail!("Failed to create clipboard context"),
    };
    let data = append_owner_marker(vec![ClipboardData::Text(text.to_owned())], side);
    clipboard_ctx.set_with_owner_marker_for_linux(&data)
}

#[cfg(not(target_os = "android"))]
pub fn update_clipboard(multi_clipboards: Vec<Clipboard>, side: ClipboardSide) {
    std::thread::spawn(move || {
        update_clipboard_(multi_clipboards, side);
    });
}

#[cfg(not(target_os = "android"))]
pub struct ClipboardContext {
    inner: arboard::Clipboard,
}

#[cfg(not(target_os = "android"))]
#[allow(unreachable_code)]
impl ClipboardContext {
    pub fn new() -> ResultType<ClipboardContext> {
        let board;
        #[cfg(not(target_os = "linux"))]
        {
            board = arboard::Clipboard::new()?;
        }
        #[cfg(target_os = "linux")]
        {
            let mut i = 1;
            loop {
                // Try 5 times to create clipboard
                // Arboard::new() connect to X server or Wayland compositor, which should be OK most times
                // But sometimes, the connection may fail, so we retry here.
                match arboard::Clipboard::new() {
                    Ok(x) => {
                        board = x;
                        break;
                    }
                    Err(e) => {
                        if i == 5 {
                            return Err(e.into());
                        } else {
                            std::thread::sleep(std::time::Duration::from_millis(30 * i));
                        }
                    }
                }
                i += 1;
            }
        }

        Ok(ClipboardContext { inner: board })
    }

    fn get_formats(&mut self, formats: &[ClipboardFormat]) -> ResultType<Vec<ClipboardData>> {
        // If there're multiple threads or processes trying to access the clipboard at the same time,
        // the previous clipboard owner will fail to access the clipboard.
        // `GetLastError()` will return `ERROR_CLIPBOARD_NOT_OPEN` (OSError(1418): Thread does not have a clipboard open) at this time.
        // See https://github.com/rustdesk-org/arboard/blob/747ab2d9b40a5c9c5102051cf3b0bb38b4845e60/src/platform/windows.rs#L34
        //
        // This is a common case on Windows, so we retry here.
        // Related issues:
        // https://github.com/rustdesk/rustdesk/issues/9263
        // https://github.com/rustdesk/rustdesk/issues/9222#issuecomment-2329233175
        for i in 0..CLIPBOARD_GET_MAX_RETRY {
            match self.inner.get_formats(formats) {
                Ok(data) => {
                    return Ok(data
                        .into_iter()
                        .filter(|c| !matches!(c, arboard::ClipboardData::None))
                        .collect())
                }
                Err(e) => match e {
                    arboard::Error::ClipboardOccupied => {
                        log::debug!("Failed to get clipboard formats, clipboard is occupied, retrying... {}", i + 1);
                        std::thread::sleep(CLIPBOARD_GET_RETRY_INTERVAL_DUR);
                    }
                    _ => {
                        log::error!("Failed to get clipboard formats, {}", e);
                        return Err(e.into());
                    }
                },
            }
        }
        bail!("Failed to get clipboard formats, clipboard is occupied, {CLIPBOARD_GET_MAX_RETRY} retries failed");
    }

    pub fn get(&mut self, side: ClipboardSide, force: bool) -> ResultType<Vec<ClipboardData>> {
        self.get_formats_filter(SUPPORTED_FORMATS, side, force)
    }

    fn get_formats_filter(
        &mut self,
        formats: &[ClipboardFormat],
        side: ClipboardSide,
        force: bool,
    ) -> ResultType<Vec<ClipboardData>> {
        let _lock = ARBOARD_MTX.lock().unwrap();
        let data = self.get_formats(formats)?;
        if data.is_empty() {
            return Ok(data);
        }
        if !force {
            for c in data.iter() {
                if let ClipboardData::Special((s, d)) = c {
                    if s == RUSTDESK_CLIPBOARD_OWNER_FORMAT && side.is_owner(d) {
                        return Ok(vec![]);
                    }
                }
            }
        }
        Ok(data
            .into_iter()
            .filter(|c| match c {
                // Skip synchronizing empty text to the remote clipboard.
                ClipboardData::Text(text) => !text.is_empty(),
                _ => false,
            })
            .collect())
    }

    fn set(&mut self, data: &[ClipboardData]) -> ResultType<()> {
        let _lock = ARBOARD_MTX.lock().unwrap();
        self.inner.set_formats(data)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn set_with_owner_marker_for_linux(&mut self, data: &[ClipboardData]) -> ResultType<()> {
        let _lock = ARBOARD_MTX.lock().unwrap();
        self.inner
            .set()
            .clipboard(LinuxClipboardKind::Clipboard)
            .formats(data)?;
        if let Err(e) = self
            .inner
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .formats(data)
        {
            log::warn!("Failed to set PRIMARY clipboard with owner marker: {}", e);
        }
        Ok(())
    }
}

pub fn is_support_multi_clipboard(peer_version: &str, peer_platform: &str) -> bool {
    use hbb_common::get_version_number;
    if get_version_number(peer_version) < get_version_number("1.3.0") {
        return false;
    }
    if ["", &hbb_common::whoami::Platform::Ios.to_string()].contains(&peer_platform) {
        return false;
    }
    if "Android" == peer_platform && get_version_number(peer_version) < get_version_number("1.3.3")
    {
        return false;
    }
    true
}

#[cfg(not(target_os = "android"))]
pub fn get_current_clipboard_msg(
    peer_version: &str,
    peer_platform: &str,
    side: ClipboardSide,
) -> Option<Message> {
    // Clipboard changes can be skipped while synchronization is disabled.
    #[cfg(target_os = "linux")]
    if !clipboard_listener::is_ready() {
        log::error!("Cannot read initial clipboard before the listener is ready");
        return None;
    }
    let ctx = match ClipboardContext::new() {
        Ok(ctx) => ctx,
        Err(err) => {
            log::error!("Failed to create clipboard context for initial sync: {}", err);
            return None;
        }
    };
    let (msg, multi_clipboards) = read_clipboard_message(&mut Some(ctx), side, true)?;
    if multi_clipboards.clipboards.is_empty() {
        return None;
    }

    if is_support_multi_clipboard(peer_version, peer_platform) {
        Some(msg)
    } else {
        // Find the first text clipboard and send it.
        multi_clipboards
            .clipboards
            .iter()
            .find(|c| c.format.enum_value() == Ok(base::message_proto::ClipboardFormat::Text))
            .map(|c| {
                let mut msg = Message::new();
                msg.set_clipboard(c.clone());
                msg
            })
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ClipboardSide {
    Host,
    Client,
}

impl ClipboardSide {
    // 01: the clipboard is owned by the host
    // 10: the clipboard is owned by the client
    fn get_owner_data(&self) -> Vec<u8> {
        match self {
            ClipboardSide::Host => vec![0b01],
            ClipboardSide::Client => vec![0b10],
        }
    }

    fn is_owner(&self, data: &[u8]) -> bool {
        if data.len() == 0 {
            return false;
        }
        data[0] & 0b11 != 0
    }
}

impl std::fmt::Display for ClipboardSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardSide::Host => write!(f, "host"),
            ClipboardSide::Client => write!(f, "client"),
        }
    }
}

pub use proto::get_msg_if_not_support_multi_clip;
mod proto {
    #[cfg(not(target_os = "android"))]
    use arboard::ClipboardData;
    use hbb_common::{
        compress::{compress as compress_func, decompress},
    };
    use base::message_proto::{Clipboard, ClipboardFormat, Message, MultiClipboards};

    fn text_to_proto(s: String) -> Clipboard {
        let compressed = compress_func(s.as_bytes());
        let compress = compressed.len() < s.as_bytes().len();
        let content = if compress {
            compressed
        } else {
            s.bytes().collect::<Vec<u8>>()
        };
        Clipboard {
            compress,
            content: content.into(),
            format: ClipboardFormat::Text.into(),
            ..Default::default()
        }
    }

    #[cfg(not(target_os = "android"))]
    fn clipboard_data_to_proto(data: ClipboardData) -> Option<Clipboard> {
        match data {
            ClipboardData::Text(s) => Some(text_to_proto(s)),
            _ => None,
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn create_multi_clipboards(vec_data: Vec<ClipboardData>) -> MultiClipboards {
        MultiClipboards {
            clipboards: vec_data
                .into_iter()
                .filter_map(clipboard_data_to_proto)
                .collect(),
            ..Default::default()
        }
    }

    #[cfg(not(target_os = "android"))]
    fn from_clipboard(clipboard: Clipboard) -> Option<ClipboardData> {
        if clipboard.format.enum_value() != Ok(ClipboardFormat::Text) {
            return None;
        }
        let data = if clipboard.compress {
            decompress(&clipboard.content)
        } else {
            clipboard.content.into()
        };
        String::from_utf8(data).ok().map(ClipboardData::Text)
    }

    #[cfg(not(target_os = "android"))]
    pub fn from_multi_clipboards(multi_clipboards: Vec<Clipboard>) -> Vec<ClipboardData> {
        multi_clipboards
            .into_iter()
            .filter_map(from_clipboard)
            .collect()
    }

    pub fn get_msg_if_not_support_multi_clip(
        version: &str,
        platform: &str,
        multi_clipboards: &MultiClipboards,
    ) -> Option<Message> {
        if crate::clipboard::is_support_multi_clipboard(version, platform) {
            return None;
        }

        // Find the first text clipboard and send it.
        multi_clipboards
            .clipboards
            .iter()
            .find(|c| c.format.enum_value() == Ok(ClipboardFormat::Text))
            .map(|c| {
                let mut msg = Message::new();
                msg.set_clipboard(c.clone());
                msg
            })
    }

    #[cfg(all(test, not(target_os = "android")))]
    mod tests {
        use super::{create_multi_clipboards, from_clipboard, from_multi_clipboards, text_to_proto};
        use arboard::ClipboardData;
        use base::message_proto::{Clipboard, ClipboardFormat};

        #[test]
        fn text_round_trips_with_and_without_compression() {
            for text in ["Hi".to_owned(), "文字 clipboard ".repeat(100)] {
                let clipboard = text_to_proto(text.clone());
                assert_eq!(clipboard.compress, text.len() > 100);
                assert!(matches!(
                    from_clipboard(clipboard),
                    Some(ClipboardData::Text(restored)) if restored == text
                ));
            }
        }

        #[test]
        fn sends_only_text_and_keeps_owner_marker_local() {
            let clipboards = create_multi_clipboards(vec![
                ClipboardData::Text("text".to_owned()),
                ClipboardData::Html("<p>html</p>".to_owned()),
                ClipboardData::Special((
                    super::super::RUSTDESK_CLIPBOARD_OWNER_FORMAT.to_owned(),
                    vec![1],
                )),
            ]);
            assert_eq!(clipboards.clipboards.len(), 1);
            assert_eq!(
                clipboards.clipboards[0].format.enum_value(),
                Ok(ClipboardFormat::Text)
            );
        }

        #[test]
        fn ignores_non_text_and_invalid_utf8_from_peer() {
            let mut clipboards = vec![text_to_proto("text".to_owned())];
            // Retired rich-text, image, and custom formats retain their wire numbers.
            for format in [1, 2, 21, 22, 23, 31, i32::MAX] {
                clipboards.push(Clipboard {
                    format: hbb_common::protobuf::EnumOrUnknown::from_i32(format),
                    content: b"ignored".to_vec().into(),
                    ..Default::default()
                });
            }
            clipboards.push(Clipboard {
                format: ClipboardFormat::Text.into(),
                content: vec![0xff].into(),
                ..Default::default()
            });
            let restored = from_multi_clipboards(clipboards);
            assert_eq!(restored.len(), 1);
            assert!(matches!(&restored[0], ClipboardData::Text(text) if text == "text"));
        }
    }
}

#[cfg(target_os = "android")]
pub fn handle_msg_clipboard(mut cb: Clipboard) {
    use hbb_common::protobuf::Message;

    if cb.format.enum_value() != Ok(ClipboardFormat::Text) {
        return;
    }
    if cb.compress {
        cb.content = bytes::Bytes::from(hbb_common::compress::decompress(&cb.content));
    }
    let multi_clips = MultiClipboards {
        clipboards: vec![cb],
        ..Default::default()
    };
    if let Ok(bytes) = multi_clips.write_to_bytes() {
        let _ = scrap::android::ffi::call_clipboard_manager_update_clipboard(&bytes);
    }
}

#[cfg(target_os = "android")]
pub fn handle_msg_multi_clipboards(mut mcb: MultiClipboards) {
    use hbb_common::protobuf::Message;

    mcb.clipboards
        .retain(|cb| cb.format.enum_value() == Ok(ClipboardFormat::Text));
    if mcb.clipboards.is_empty() {
        return;
    }
    for cb in mcb.clipboards.iter_mut() {
        if cb.compress {
            cb.content = bytes::Bytes::from(hbb_common::compress::decompress(&cb.content));
        }
    }
    if let Ok(bytes) = mcb.write_to_bytes() {
        let _ = scrap::android::ffi::call_clipboard_manager_update_clipboard(&bytes);
    }
}

#[cfg(target_os = "android")]
pub fn get_clipboards_msg(client: bool) -> Option<Message> {
    let mut clipboards = scrap::android::ffi::get_clipboards(client)?;
    clipboards
        .clipboards
        .retain(|cb| cb.format.enum_value() == Ok(ClipboardFormat::Text));
    if clipboards.clipboards.is_empty() {
        return None;
    }
    let mut msg = Message::new();
    for c in &mut clipboards.clipboards {
        let compressed = hbb_common::compress::compress(&c.content);
        let compress = compressed.len() < c.content.len();
        if compress {
            c.content = compressed.into();
        }
        c.compress = compress;
    }
    msg.set_multi_clipboards(clipboards);
    Some(msg)
}

// We need this mod to notify multiple subscribers when the clipboard changes.
// Because only one clipboard master(listener) can trigger the clipboard change event multiple listeners are created on Linux(x11).
// https://github.com/rustdesk-org/clipboard-master/blob/4fb62e5b62fb6350d82b571ec7ba94b3cd466695/src/master/x11.rs#L226
#[cfg(not(target_os = "android"))]
pub mod clipboard_listener {
    use clipboard_master::{CallbackResult, ClipboardHandler, Master, Shutdown};
    #[cfg(target_os = "linux")]
    use hbb_common::tokio::sync::watch;
    use hbb_common::{bail, log, ResultType};
    use std::{
        collections::HashMap,
        io,
        sync::atomic::{AtomicUsize, Ordering},
        sync::mpsc::{channel, Sender},
        sync::{Arc, Mutex},
        thread::JoinHandle,
    };

    lazy_static::lazy_static! {
        pub static ref CLIPBOARD_LISTENER: Arc<Mutex<ClipboardListener>> = Default::default();
    }

    static CLIPBOARD_GENERATION: AtomicUsize = AtomicUsize::new(0);

    pub enum ClipboardEvent {
        Changed,
        Stop,
        StopWithError(io::Error),
    }

    pub fn current_generation() -> usize {
        CLIPBOARD_GENERATION.load(Ordering::SeqCst)
    }

    #[cfg(target_os = "linux")]
    pub fn is_ready() -> bool {
        let listener = CLIPBOARD_LISTENER.lock().unwrap();
        let ready = listener
            .ready
            .as_ref()
            .map(|ready| *ready.borrow())
            .unwrap_or(false);
        ready
            && listener
                .handle
                .as_ref()
                .map(|(_, h)| !h.is_finished())
                .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    pub async fn wait_for_ready() -> ResultType<()> {
        let Some(mut ready) = CLIPBOARD_LISTENER.lock().unwrap().ready.clone() else {
            bail!("Clipboard listener has not started");
        };
        let is_ready = *ready.borrow_and_update();
        if !is_ready {
            ready.changed().await?;
        }
        Ok(())
    }

    struct Handler {
        subscribers: Arc<Mutex<HashMap<String, Sender<ClipboardEvent>>>>,
        #[cfg(target_os = "linux")]
        ready: Option<watch::Sender<bool>>,
    }

    impl ClipboardHandler for Handler {
        #[cfg(target_os = "linux")]
        fn on_clipboard_ready(&mut self) {
            // A restart must invalidate older snapshots, including on X11 or an empty selection.
            CLIPBOARD_GENERATION.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = self.ready.take() {
                if let Err(err) = tx.send(true) {
                    log::debug!("Failed to report clipboard listener readiness: {}", err);
                }
            }
        }

        fn on_clipboard_change(&mut self) -> CallbackResult {
            // Empty or unsupported contents still invalidate an initial snapshot.
            CLIPBOARD_GENERATION.fetch_add(1, Ordering::SeqCst);
            let sub_lock = self.subscribers.lock().unwrap();
            for tx in sub_lock.values() {
                tx.send(ClipboardEvent::Changed).ok();
            }
            CallbackResult::Next
        }

        fn on_clipboard_initial_selection(&mut self) -> CallbackResult {
            // Text initial synchronization is per session, not a host clipboard change.
            CallbackResult::Next
        }

        fn on_clipboard_error(&mut self, error: io::Error) -> CallbackResult {
            let msg = format!("Clipboard listener error: {}", error);
            let sub_lock = self.subscribers.lock().unwrap();
            for tx in sub_lock.values() {
                tx.send(ClipboardEvent::StopWithError(io::Error::new(
                    io::ErrorKind::Other,
                    msg.clone(),
                )))
                .ok();
            }
            #[cfg(target_os = "linux")]
            if self.ready.is_some() {
                // Subscribers must receive the error before a failed startup stops.
                return CallbackResult::StopWithError(error);
            }
            CallbackResult::Next
        }
    }

    #[derive(Default)]
    pub struct ClipboardListener {
        subscribers: Arc<Mutex<HashMap<String, Sender<ClipboardEvent>>>>,
        handle: Option<(Shutdown, JoinHandle<()>)>,
        #[cfg(target_os = "linux")]
        ready: Option<watch::Receiver<bool>>,
    }

    pub fn subscribe(name: String, tx: Sender<ClipboardEvent>) -> ResultType<()> {
        log::info!("Subscribe clipboard listener: {}", &name);
        let mut listener_lock = CLIPBOARD_LISTENER.lock().unwrap();
        listener_lock
            .subscribers
            .lock()
            .unwrap()
            .insert(name.clone(), tx);

        cleanup_stale_listener(&mut listener_lock);
        if listener_lock.handle.is_none() {
            log::info!("Start clipboard listener thread");
            #[cfg(target_os = "linux")]
            let (tx_ready, rx_ready) = watch::channel(false);
            let handler = Handler {
                subscribers: listener_lock.subscribers.clone(),
                #[cfg(target_os = "linux")]
                ready: Some(tx_ready),
            };
            let (tx_start_res, rx_start_res) = channel();
            let h = start_clipboard_master_thread(handler, tx_start_res);
            let shutdown = match rx_start_res.recv() {
                Ok((Some(s), _)) => s,
                Ok((None, err)) => {
                    bail!(err);
                }

                Err(e) => {
                    bail!("Failed to create clipboard listener: {}", e);
                }
            };
            listener_lock.handle = Some((shutdown, h));
            #[cfg(target_os = "linux")]
            {
                // Initial-sync tasks await backend readiness without blocking the connection loop.
                listener_lock.ready = Some(rx_ready);
            }
            log::info!("Clipboard listener thread started");
        }

        log::info!("Clipboard listener subscribed: {}", name);
        Ok(())
    }

    fn cleanup_stale_listener(listener: &mut ClipboardListener) {
        if !listener
            .handle
            .as_ref()
            .map(|(_, h)| h.is_finished())
            .unwrap_or(false)
        {
            return;
        }
        if let Some((shutdown, h)) = listener.handle.take() {
            log::warn!("Cleaning up stale clipboard listener handle");
            if let Err(e) = h.join() {
                log::error!("Clipboard listener thread panicked during stale cleanup: {:?}", e);
            }
            drop(shutdown);
        }
    }

    pub fn unsubscribe(name: &str) {
        log::info!("Unsubscribe clipboard listener: {}", name);
        let mut listener_lock = CLIPBOARD_LISTENER.lock().unwrap();
        let is_empty = {
            let mut sub_lock = listener_lock.subscribers.lock().unwrap();
            if let Some(tx) = sub_lock.remove(name) {
                tx.send(ClipboardEvent::Stop).ok();
            }
            sub_lock.is_empty()
        };
        if is_empty {
            if let Some((shutdown, h)) = listener_lock.handle.take() {
                log::info!("Stop clipboard listener thread");
                shutdown.signal();
                h.join().ok();
                log::info!("Clipboard listener thread stopped");
            }
        }
        log::info!("Clipboard listener unsubscribed: {}", name);
    }

    fn start_clipboard_master_thread(
        handler: impl ClipboardHandler + Send + 'static,
        tx_start_res: Sender<(Option<Shutdown>, String)>,
    ) -> JoinHandle<()> {
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessage#:~:text=The%20window%20must%20belong%20to%20the%20current%20thread.
        let h = std::thread::spawn(move || match Master::new(handler) {
            Ok(mut master) => {
                tx_start_res
                    .send((Some(master.shutdown_channel()), "".to_owned()))
                    .ok();
                log::debug!("Clipboard listener started");
                if let Err(err) = master.run() {
                    log::error!("Failed to run clipboard listener: {}", err);
                } else {
                    log::debug!("Clipboard listener stopped");
                }
            }
            Err(err) => {
                tx_start_res
                    .send((
                        None,
                        format!("Failed to create clipboard listener: {}", err),
                    ))
                    .ok();
            }
        });
        h
    }
}
