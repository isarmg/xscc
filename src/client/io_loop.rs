#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::clipboard::{clipboard_listener, update_clipboard, ClipboardSide};
#[cfg(not(any(target_os = "ios")))]
use crate::clipboard::CLIPBOARD_INTERVAL;
use crate::{
    client::{
        self, Client, Data, Interface, MediaData, MediaSender,
        QualityStatus, SEC30,
    },
    ui_session_interface::{InvokeUiSession, Session},
};

// Empirical no-data window before exposing the restart reconnect state to the UI.
// Restart msgbox text is kept as a legacy UI fallback; Flutter handles the type as a control event.
const RESTART_REMOTE_DEVICE_NO_DATA_TIMEOUT: Duration = Duration::from_secs(5);
use base::message_proto::{permission_info::Permission, *};
use crossbeam_queue::ArrayQueue;
use hbb_common::{
    allow_err,
    config,
    log,
    protobuf::Message as _,
    rendezvous_proto::ConnType,
    timeout,
    tokio::{
        self,
        sync::mpsc,
        time::{self, Duration, Instant},
    },
    Stream,
};
use scrap::CodecFormat;
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, RwLock,
    },
};

pub struct Remote<T: InvokeUiSession> {
    handler: Session<T>,
    audio_sender: MediaSender,
    receiver: mpsc::UnboundedReceiver<Data>,
    sender: mpsc::UnboundedSender<Data>,
    timer: crate::RustDeskInterval,
    // Set after PeerInfo for this round, not when the transport connects.
    is_connected: bool,
    // Whether the scheduled initial snapshot may still be sent.
    // The connection loop clears this when handling its result or a live clipboard update.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    initial_clipboard_pending: bool,
    first_frame: bool,
    data_count: Arc<AtomicUsize>,
    video_format: CodecFormat,
    elevation_requested: bool,
    peer_info: ParsedPeerInfo,
    video_threads: HashMap<usize, VideoThread>,
    chroma: Arc<RwLock<Option<Chroma>>>,
    sent_close_reason: bool,
    cursor_dedupe: CursorDedupe,
}

#[derive(Default)]
struct ParsedPeerInfo {
    platform: String,
    is_installed: bool,
    idd_impl: String,
}

impl ParsedPeerInfo {
    fn is_support_virtual_display(&self) -> bool {
        self.is_installed
            && self.platform == "Windows"
            && (self.idd_impl == "rustdesk_idd" || self.idd_impl == "amyuni_idd")
    }
}

impl<T: InvokeUiSession> Remote<T> {
    pub fn new(
        handler: Session<T>,
        receiver: mpsc::UnboundedReceiver<Data>,
        sender: mpsc::UnboundedSender<Data>,
    ) -> Self {
        Self {
            handler,
            audio_sender: crate::client::start_audio_thread(),
            receiver,
            sender,
            timer: crate::rustdesk_interval(time::interval(SEC30)),
            is_connected: false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            initial_clipboard_pending: false,
            first_frame: false,
            data_count: Arc::new(AtomicUsize::new(0)),
            video_format: CodecFormat::Unknown,
            elevation_requested: false,
            peer_info: Default::default(),
            video_threads: Default::default(),
            chroma: Default::default(),
            sent_close_reason: false,
            cursor_dedupe: Default::default(),
        }
    }

    pub async fn io_loop(&mut self, round: u32) {
        #[cfg(all(feature = "hwcodec", not(any(target_os = "android", target_os = "ios"))))]
        if let Err(err) = tokio::task::spawn_blocking(client::get_hwcodec_config).await {
            log::error!("Local hardware codec probe task failed: {err}");
        }
        let mut received = false;
        let conn_type = self.handler.lc.read().unwrap().conn_type;
        match Client::start(
            &self.handler.get_id(),
            conn_type,
            self.handler.clone(),
        )
        .await
        {
            Ok((mut peer, pk, stream_type)) => {
                let direct = true;
                self.handler
                    .connection_round_state
                    .lock()
                    .unwrap()
                    .set_connected();
                let is_secured = peer.is_secured();
                self.handler
                    .set_connection_type(is_secured, direct, stream_type); // flutter -> connection_ready
                if !is_secured || !client::confirm_direct_identity(
                    &self.handler, &mut self.receiver, &pk,
                ).await {
                    self.send_close_reason(&mut peer, "Device identity was not trusted").await;
                    self.handle_disconnected(round);
                    return;
                }
                peer.set_max_packet_length(usize::MAX);
                self.handler.update_direct(Some(direct));
                if conn_type == ConnType::DEFAULT_CONN {
                    self.handler
                        .set_fingerprint(crate::common::pk_to_fingerprint(pk));
                }

                let mut status_timer =
                    crate::rustdesk_interval(time::interval(Duration::new(1, 0)));
                let mut fps_instant = Instant::now();

                let mut last_recv_time = Instant::now();


                loop {
                    tokio::select! {
                        res = peer.next() => {
                            if let Some(res) = res {
                                match res {
                                    Err(err) => {
                                        self.handler.on_establish_connection_error(format!("{err:#}"));
                                        break;
                                    }
                                    Ok(ref bytes) => {
                                        last_recv_time = Instant::now();
                                        if !received {
                                            received = true;
                                            self.handler.update_received(true);
                                        }
                                        self.data_count.fetch_add(bytes.len(), Ordering::Relaxed);
                                        if !self.handle_msg_from_peer(bytes, &mut peer).await {
                                            break
                                        }
                                    }
                                }
                            } else {
                                if self.handler.is_restarting_remote_device() {
                                    log::info!("Restart remote device");
                                    self.handler.msgbox("restarting", "Restarting remote device", "Connection in progress. Please wait.", "");
                                } else {
                                    log::info!("Reset by the peer");
                                    self.handler.msgbox("error", "Connection Error", "Reset by the peer", "");
                                }
                                break;
                            }
                        }
                        d = self.receiver.recv() => {
                            if let Some(d) = d {
                                if !self.handle_msg_from_ui(d, &mut peer).await {
                                    break;
                                }
                            }
                        }
                        _ = self.timer.tick() => {
                            if last_recv_time.elapsed() >= SEC30 {
                                self.handler.msgbox("error", "Connection Error", "Timeout", "");
                                break;
                            }
                        }
                        _ = status_timer.tick() => {
                            if self.handler.is_restarting_remote_device()
                                && last_recv_time.elapsed() >= RESTART_REMOTE_DEVICE_NO_DATA_TIMEOUT
                            {
                                self.handler.msgbox("restarting-show", "Restarting remote device", "Connection in progress. Please wait.", "");
                                break;
                            }
                            let elapsed = fps_instant.elapsed().as_millis();
                            if elapsed < 1000 {
                                continue;
                            }
                            fps_instant = Instant::now();
                            let mut speed = self.data_count.swap(0, Ordering::Relaxed);
                            speed = speed * 1000 / elapsed as usize;
                            let speed = format!("{:.2}kB/s", speed as f32 / 1024 as f32);

                            let fps = self.video_threads.iter().map(|(k, v)| {
                                // Correcting the inaccuracy of status_timer
                                (k.clone(), (*v.frame_count.read().unwrap() as i32) * 1000 / elapsed as i32)
                            }).collect::<HashMap<usize, i32>>();
                            self.video_threads.iter().for_each(|(_, v)| {
                                *v.frame_count.write().unwrap() = 0;
                            });
                            self.fps_control(direct, fps.clone());
                            let chroma = self.chroma.read().unwrap().clone();
                            let chroma = match chroma {
                                Some(Chroma::I444) => "4:4:4",
                                Some(Chroma::I420) => "4:2:0",
                                None => "-",
                            };
                            let chroma = Some(chroma.to_string());
                            let codec_format = if self.video_format == CodecFormat::Unknown {
                                None
                            } else {
                                Some(self.video_format.clone())
                            };
                            self.handler.update_quality_status(QualityStatus {
                                speed: Some(speed),
                                fps,
                                chroma,
                                codec_format,
                                ..Default::default()
                            });
                        }
                    }
                }
                log::debug!("Exit io_loop of id={}", self.handler.get_id());

            }
            Err(err) => {
                self.handler.on_establish_connection_error(format!("{err:#}"));
            }
        }
        self.handle_disconnected(round);
    }

    fn handle_disconnected(&self, round: u32) {
        // set_disconnected_ok is used to check if new connection round is started.
        let _set_disconnected_ok = self
            .handler
            .connection_round_state
            .lock()
            .unwrap()
            .set_disconnected(round);

        #[cfg(not(target_os = "ios"))]
        if self.handler.is_default() && _set_disconnected_ok {
            // Other sessions may keep the listener running after this one disconnects.
            #[cfg(feature = "flutter")]
            crate::flutter::update_text_clipboard_required();

            Client::try_stop_clipboard();
        }

    }



    async fn send_close_reason(&mut self, peer: &mut Stream, reason: &str) {
        if self.sent_close_reason {
            return;
        }
        let mut misc = Misc::new();
        misc.set_close_reason(reason.to_owned());
        let mut msg = Message::new();
        msg.set_misc(misc);
        allow_err!(peer.send(&msg).await);
        self.sent_close_reason = true;
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn start_initial_clipboard_sync(&mut self) {
        let peer_info = {
            let lc = self.handler.lc.read().unwrap();
            lc.peer_info
                .as_ref()
                .map(|pi| (pi.version.clone(), pi.platform.clone()))
        };
        let Some((peer_version, peer_platform)) = peer_info else {
            log::error!("Cannot start initial clipboard sync without peer information");
            return;
        };

        self.initial_clipboard_pending = true;
        let sender = self.sender.clone();
        let permission_config = self.handler.get_permission_config();
        // Clipboard access and encoding must not block the connection loop.
        let read_clipboard = move || {
            // Capture after readiness, before reading; later changes invalidate this snapshot.
            let generation = clipboard_listener::current_generation();
            let msg_out = if permission_config.is_text_clipboard_required() {
                crate::clipboard::get_current_clipboard_msg(
                    &peer_version,
                    &peer_platform,
                    crate::clipboard::ClipboardSide::Client,
                )
            } else {
                None
            };
            let msg_out = msg_out.filter(|_| permission_config.is_text_clipboard_required());
            // Empty or failed reads must also finish the pending initial-sync attempt.
            if let Err(err) = sender.send(Data::InitialClipboard(generation, msg_out)) {
                log::debug!("Failed to send initial clipboard result: {}", err);
            }
        };
        #[cfg(target_os = "linux")]
        self.spawn_initial_clipboard_read_after_ready(read_clipboard);
        #[cfg(not(target_os = "linux"))]
        tokio::task::spawn_blocking(read_clipboard);
    }

    // Linux listener creation returns before X11/Wayland has subscribed to changes.
    // Capture after readiness so changes during the read can invalidate the snapshot.
    // A separate task keeps a stalled backend from blocking the connection loop.
    #[cfg(target_os = "linux")]
    fn spawn_initial_clipboard_read_after_ready(&self, read_clipboard: impl FnOnce() + Send + 'static) {
        // Initial-sync wait budget, not a protocol-defined startup deadline.
        const CLIPBOARD_READY_TIMEOUT: Duration = Duration::from_secs(5);

        let sender = self.sender.clone();
        tokio::spawn(async move {
            let ready = tokio::select! {
                result = time::timeout(CLIPBOARD_READY_TIMEOUT, clipboard_listener::wait_for_ready()) => {
                    match result {
                        Ok(ready) => ready,
                        Err(err) => Err(err.into()),
                    }
                }
                _ = sender.closed() => return,
            };
            if let Err(err) = ready {
                log::error!(
                    "Failed to wait for clipboard listener readiness (limit {:?}): {}",
                    CLIPBOARD_READY_TIMEOUT,
                    err,
                );
                let generation = clipboard_listener::current_generation();
                if let Err(err) = sender.send(Data::InitialClipboard(generation, None)) {
                    log::debug!("Failed to send initial clipboard result: {}", err);
                }
                return;
            }
            tokio::task::spawn_blocking(read_clipboard);
        });
    }

    async fn handle_msg_from_ui(&mut self, data: Data, peer: &mut Stream) -> bool {
        match data {
            Data::Close => {
                self.send_close_reason(peer, "").await;
                return false;
            }
            Data::Login((os_username, os_password, password, remember)) => {
                self.handler
                    .handle_login_from_ui(os_username, os_password, password, remember, peer)
                    .await;
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Data::InitialClipboard(generation, msg) => {
                // A live update supersedes any initial snapshot still being prepared.
                if !self.initial_clipboard_pending {
                    return true;
                }
                self.initial_clipboard_pending = false;
                if !self.handler.is_text_clipboard_required() {
                    return true;
                }
                if generation != clipboard_listener::current_generation() {
                    // A clipboard change or listener restart can invalidate the snapshot
                    // without sending a live update that supersedes it.
                    drop(msg);
                    self.start_initial_clipboard_sync();
                    return true;
                }
                if let Some(msg) = msg {
                    allow_err!(peer.send(&msg).await);
                }
            }
            Data::Message(msg) => {
                // The Flutter clipboard broadcast is process-wide, so a clipboard can reach this
                // round's queue before the round has logged in; it is dropped here, on the round
                // itself.
                #[cfg(feature = "flutter")]
                if !self.is_connected
                    && matches!(
                        msg.union.as_ref(),
                        Some(message::Union::Clipboard(_))
                            | Some(message::Union::MultiClipboards(_))
                    )
                {
                    return true;
                }
                match &msg.union {
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    Some(message::Union::Clipboard(_)) | Some(message::Union::MultiClipboards(_)) => {
                        self.initial_clipboard_pending = false;
                    }
                    Some(message::Union::Misc(misc)) => match misc.union {
                        Some(misc::Union::RefreshVideo(_)) => {
                            self.video_threads.iter().for_each(|(_, v)| {
                                *v.discard_queue.write().unwrap() = true;
                            });
                        }
                        Some(misc::Union::RefreshVideoDisplay(display)) => {
                            if let Some(v) = self.video_threads.get_mut(&(display as usize)) {
                                *v.discard_queue.write().unwrap() = true;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                allow_err!(peer.send(&msg).await);
            }
            Data::ElevateDirect => {
                let mut request = ElevationRequest::new();
                request.set_direct(true);
                let mut misc = Misc::new();
                misc.set_elevation_request(request);
                let mut msg = Message::new();
                msg.set_misc(misc);
                allow_err!(peer.send(&msg).await);
                self.elevation_requested = true;
            }
            Data::ElevateWithLogon(username, password) => {
                let mut request = ElevationRequest::new();
                request.set_logon(ElevationRequestWithLogon {
                    username,
                    password,
                    ..Default::default()
                });
                let mut misc = Misc::new();
                misc.set_elevation_request(request);
                let mut msg = Message::new();
                msg.set_misc(misc);
                allow_err!(peer.send(&msg).await);
                self.elevation_requested = true;
            }
            Data::ResetDecoder(display) => match display {
                Some(display) => {
                    if let Some(v) = self.video_threads.get_mut(&display) {
                        v.video_sender.send(MediaData::Reset).ok();
                    }
                }
                None => {
                    for (_, v) in self.video_threads.iter_mut() {
                        v.video_sender.send(MediaData::Reset).ok();
                    }
                }
            },
            Data::TakeScreenshot((display, sid)) => {
                let mut msg = Message::new();
                msg.set_screenshot_request(ScreenshotRequest {
                    display,
                    sid,
                    ..Default::default()
                });
                allow_err!(peer.send(&msg).await);
            }
            _ => {}
        }
        true
    }

    async fn send_toggle_virtual_display_msg(&self, peer: &mut Stream) {
        if !self.peer_info.is_support_virtual_display() {
            return;
        }
        let lc = self.handler.lc.read().unwrap();
        let displays = lc.get_option("virtual-display");
        for d in displays.split(',') {
            if let Ok(index) = d.parse::<i32>() {
                let mut misc = Misc::new();
                misc.set_toggle_virtual_display(ToggleVirtualDisplay {
                    display: index,
                    on: true,
                    ..Default::default()
                });
                let mut msg_out = Message::new();
                msg_out.set_misc(misc);
                allow_err!(peer.send(&msg_out).await);
            }
        }
    }

    async fn send_toggle_privacy_mode_msg(&self, peer: &mut Stream) {
        let lc = self.handler.lc.read().unwrap();
        if lc.version >= hbb_common::get_version_number("1.2.4")
            && lc.get_toggle_option("privacy-mode")
        {
            let impl_key = lc.get_option("privacy-mode-impl-key");
            if impl_key == crate::privacy_mode::PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY
                && !self.peer_info.is_support_virtual_display()
            {
                return;
            }
            let mut misc = Misc::new();
            misc.set_toggle_privacy_mode(TogglePrivacyMode {
                impl_key,
                on: true,
                ..Default::default()
            });
            let mut msg_out = Message::new();
            msg_out.set_misc(misc);
            allow_err!(peer.send(&msg_out).await);
        }
    }

    fn contains_key_frame(vf: &VideoFrame) -> bool {
        use video_frame::Union::*;
        match &vf.union {
            Some(vf) => match vf {
                Vp8s(f) | Vp9s(f) | Av1s(f) | H264s(f) | H265s(f) => f.frames.iter().any(|e| e.key),
                _ => false,
            },
            None => false,
        }
    }

    // Currently, this function only considers decoding speed and queue length, not network delay.
    // The controlled end can consider auto fps as the maximum decoding fps.
    #[inline]
    fn fps_control(&mut self, direct: bool, real_fps_map: HashMap<usize, i32>) {
        self.video_threads.iter_mut().for_each(|(k, v)| {
            let real_fps = real_fps_map.get(k).cloned().unwrap_or_default();
            if real_fps == 0 {
                v.fps_control.inactive_counter += 1;
            } else {
                v.fps_control.inactive_counter = 0;
            }
        });
        let custom_fps = self.handler.lc.read().unwrap().custom_fps.clone();
        let custom_fps = custom_fps.lock().unwrap().clone();
        let mut custom_fps = custom_fps.unwrap_or(30);
        if custom_fps < 5 || custom_fps > 120 {
            custom_fps = 30;
        }
        let inactive_threshold = 15;
        let max_queue_len = self
            .video_threads
            .iter()
            .map(|v| v.1.video_queue.read().unwrap().len())
            .max()
            .unwrap_or_default();
        let min_decode_fps = self
            .video_threads
            .iter()
            .filter(|v| v.1.fps_control.inactive_counter < inactive_threshold)
            .map(|v| *v.1.decode_fps.read().unwrap())
            .min()
            .flatten();
        let Some(min_decode_fps) = min_decode_fps else {
            return;
        };
        let mut limited_fps = if direct {
            min_decode_fps * 9 / 10 // 30 got 27
        } else {
            min_decode_fps * 4 / 5 // 30 got 24
        };
        if limited_fps > custom_fps {
            limited_fps = custom_fps;
        }
        let last_auto_fps = self.handler.lc.read().unwrap().last_auto_fps.clone();
        let displays = self.video_threads.keys().cloned().collect::<Vec<_>>();
        let mut fps_trending = |display: usize| {
            let thread = self.video_threads.get_mut(&display)?;
            let ctl = &mut thread.fps_control;
            let len = thread.video_queue.read().unwrap().len();
            let decode_fps = thread.decode_fps.read().unwrap().clone()?;
            let last_auto_fps = last_auto_fps.clone().unwrap_or(custom_fps as _);
            if ctl.inactive_counter > inactive_threshold {
                return None;
            }
            if len > 1 && last_auto_fps > limited_fps || len > std::cmp::max(1, decode_fps / 2) {
                ctl.idle_counter = 0;
                return Some(false);
            }
            if len <= 1 {
                ctl.idle_counter += 1;
                if ctl.idle_counter > 3 && last_auto_fps + 3 <= limited_fps {
                    return Some(true);
                }
            }
            if len > 1 {
                ctl.idle_counter = 0;
            }
            None
        };
        let trendings: Vec<_> = displays.iter().map(|k| fps_trending(*k)).collect();
        let should_decrease = trendings.iter().any(|v| *v == Some(false));
        let should_increase = !should_decrease && trendings.iter().any(|v| *v == Some(true));
        if last_auto_fps.is_none() || should_decrease || should_increase {
            // limited_fps to ensure decoding is faster than encoding
            let mut auto_fps = limited_fps;
            if should_decrease && limited_fps < max_queue_len {
                auto_fps = limited_fps / 2;
            }
            if auto_fps < 1 {
                auto_fps = 1;
            }
            if Some(auto_fps) != last_auto_fps {
                let mut misc = Misc::new();
                misc.set_option(OptionMessage {
                    custom_fps: auto_fps as _,
                    ..Default::default()
                });
                let mut msg = Message::new();
                msg.set_misc(misc);
                self.sender.send(Data::Message(msg)).ok();
                log::info!("Set fps to {}", auto_fps);
                self.handler.lc.write().unwrap().last_auto_fps = Some(auto_fps);
            }
        }
        // send refresh
        for (display, thread) in self.video_threads.iter_mut() {
            let ctl = &mut thread.fps_control;
            let video_queue = thread.video_queue.read().unwrap();
            let tolerable = std::cmp::min(min_decode_fps, video_queue.capacity() / 2);
            if ctl.refresh_times < 20 // enough
                    && (video_queue.len() > tolerable
                            && (ctl.refresh_times == 0 || ctl.last_refresh_instant.map(|t|t.elapsed().as_secs() > 10).unwrap_or(false)))
            {
                // Refresh causes client set_display, left frames cause flickering.
                drop(video_queue);
                self.handler.refresh_video(*display as _);
                log::info!("Refresh display {} to reduce delay", display);
                ctl.refresh_times += 1;
                ctl.last_refresh_instant = Some(Instant::now());
            }
        }
    }

    async fn handle_msg_from_peer(&mut self, data: &[u8], peer: &mut Stream) -> bool {
        if let Ok(msg_in) = Message::parse_from_bytes(&data) {
            if base::session_protocol::is_legacy_role_switch_message(&msg_in) {
                self.send_close_reason(peer, "Runtime role switching is not supported")
                    .await;
                return false;
            }
            match msg_in.union {
                Some(message::Union::VideoFrame(vf)) => {
                    if !self.first_frame {
                        self.first_frame = true;
                        self.handler.close_success();
                        self.handler.adapt_size();
                        self.send_toggle_virtual_display_msg(peer).await;
                        self.send_toggle_privacy_mode_msg(peer).await;
                    }
                    self.video_format = CodecFormat::from(&vf);

                    let display = vf.display as usize;
                    if !self.video_threads.contains_key(&display) {
                        self.new_video_thread(display);
                    }
                    let Some(thread) = self.video_threads.get_mut(&display) else {
                        return true;
                    };
                    if Self::contains_key_frame(&vf) {
                        thread
                            .video_sender
                            .send(MediaData::VideoFrame(Box::new(vf)))
                            .ok();
                    } else {
                        let video_queue = thread.video_queue.read().unwrap();
                        if video_queue.force_push(vf).is_some() {
                            drop(video_queue);
                            self.handler.refresh_video(display as _);
                        } else {
                            thread.video_sender.send(MediaData::VideoQueue).ok();
                        }
                    }
                }
                Some(message::Union::Hash(hash)) => {
                    if !self
                        .handler
                        .handle_hash(&self.handler.password.clone(), hash, peer)
                        .await
                    {
                        return false;
                    }
                }
                Some(message::Union::LoginResponse(lr)) => match lr.union {
                    Some(login_response::Union::Error(err)) => {
                        if err == client::REQUIRE_2FA {
                            self.handler.lc.write().unwrap().enable_trusted_devices =
                                lr.enable_trusted_devices;
                        }
                        if !self.handler.handle_login_error(&err) {
                            return false;
                        }
                    }
                    Some(login_response::Union::PeerInfo(pi)) => {
                        let peer_version = pi.version.clone();
                        let peer_platform = pi.platform.clone();
                        self.set_peer_info(&pi);
                        self.handler.handle_peer_info(pi);
                        if self.handler.is_default() {
                            // Startup notifications must see the current sessions' clipboard requirements.
                            #[cfg(feature = "flutter")]
                            #[cfg(not(target_os = "ios"))]
                            crate::flutter::update_text_clipboard_required();

                            #[cfg(feature = "flutter")]
                            #[cfg(not(target_os = "ios"))]
                            let rx = Client::try_start_clipboard(None);
                            #[cfg(not(feature = "flutter"))]
                            #[cfg(not(any(target_os = "android", target_os = "ios")))]
                            let rx = Client::try_start_clipboard(Some(
                                crate::client::ClientClipboardContext {
                                    cfg: self.handler.get_permission_config(),
                                    tx: self.sender.clone(),
                                },
                            ));
                            // To make sure current text clipboard data is updated.
                            #[cfg(not(target_os = "ios"))]
                            if let Some(mut rx) = rx {
                                timeout(CLIPBOARD_INTERVAL, rx.recv()).await.ok();
                            }

                            // `is_connected`` becomes true after the first PeerInfo.
                            // Reconnects create a new Remote. Refreshes after a generation change
                            // use a separate entry point and are not blocked by this guard.
                            #[cfg(not(any(target_os = "android", target_os = "ios")))]
                            if !self.is_connected
                                && self.handler.is_text_clipboard_required()
                                && self.handler.lc.read().unwrap().sync_init_clipboard.v
                            {
                                self.start_initial_clipboard_sync();
                            }
                            // to-do: Android, is `sync_init_clipboard` really needed?
                            // https://github.com/rustdesk/rustdesk/discussions/9010
                        }

                        self.is_connected = true;
                    }
                    _ => {}
                },
                Some(message::Union::CursorData(cd)) if self.dedupes_cursors() => {
                    self.set_cursor_data_by_content(cd);
                }
                Some(message::Union::CursorId(id)) if self.dedupes_cursors() => {
                    self.handler
                        .set_cursor_id(self.cursor_dedupe.id(id).to_string());
                }
                Some(message::Union::CursorData(cd)) => {
                    let id = cd.id;
                    #[cfg(feature = "flutter")]
                    let compressed = cd.colors.clone();
                    match decode_cursor_data(cd) {
                        Ok(cd) => {
                            #[cfg(feature = "flutter")]
                            self.keep_cursor_shape(cursor_shape(id, &cd, &compressed));
                            self.handler.set_cursor_data(cd)
                        }
                        Err(err) => {
                            log::warn!("Rejected cursor {id}: {err}");
                            // The peer shows it now: selected, the UI treats it as a shape it
                            // lacks, as it will when the peer selects it again.
                            self.handler.set_cursor_id(id.to_string());
                        }
                    }
                }
                Some(message::Union::CursorId(id)) => {
                    self.handler.set_cursor_id(id.to_string());
                }
                Some(message::Union::CursorPosition(cp)) => {
                    self.handler.set_cursor_position(cp);
                }
                Some(message::Union::Clipboard(cb)) => {
                    let clipboard_allowed = {
                        let lc = self.handler.lc.read().unwrap();
                        !lc.disable_clipboard.v && !lc.view_only.v
                    };
                    if clipboard_allowed {
                        #[cfg(all(
                            feature = "flutter",
                            not(any(target_os = "android", target_os = "ios"))
                        ))]
                        if self.handler.is_text_clipboard_required()
                            && crate::clipboard::is_sync_clipboard_between_sessions_enabled()
                        {
                            let mut msg = Message::new();
                            msg.set_clipboard(cb.clone());
                            let session_id = self.handler.lc.read().unwrap().session_id;
                            crate::flutter::send_clipboard_msg_to_other_sessions(msg, session_id);
                        }
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        {
                            self.initial_clipboard_pending = false;
                            update_clipboard(vec![cb], ClipboardSide::Client);
                        }
                        #[cfg(target_os = "ios")]
                        {
                            let content = if cb.compress {
                                hbb_common::compress::decompress(&cb.content)
                            } else {
                                cb.content.into()
                            };
                            if let Ok(content) = String::from_utf8(content) {
                                self.handler.clipboard(content);
                            }
                        }
                        #[cfg(target_os = "android")]
                        crate::clipboard::handle_msg_clipboard(cb);
                    }
                }
                Some(message::Union::MultiClipboards(mut _mcb)) => {
                    _mcb.clipboards.retain(|c| c.format.enum_value() == Ok(ClipboardFormat::Text));
                    if _mcb.clipboards.is_empty() {
                        return true;
                    }
                    let clipboard_allowed = {
                        let lc = self.handler.lc.read().unwrap();
                        !lc.disable_clipboard.v && !lc.view_only.v
                    };
                    if clipboard_allowed {
                        #[cfg(all(
                            feature = "flutter",
                            not(any(target_os = "android", target_os = "ios"))
                        ))]
                        if self.handler.is_text_clipboard_required()
                            && crate::clipboard::is_sync_clipboard_between_sessions_enabled()
                        {
                            let mut msg = Message::new();
                            msg.set_multi_clipboards(_mcb.clone());
                            let session_id = self.handler.lc.read().unwrap().session_id;
                            crate::flutter::send_clipboard_msg_to_other_sessions(msg, session_id);
                        }
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        {
                            self.initial_clipboard_pending = false;
                            update_clipboard(_mcb.clipboards, ClipboardSide::Client);
                        }
                        #[cfg(target_os = "ios")]
                        {
                            if let Some(cb) = _mcb
                                .clipboards
                                .iter()
                                .find(|c| c.format.enum_value() == Ok(ClipboardFormat::Text))
                            {
                                let content = if cb.compress {
                                    hbb_common::compress::decompress(&cb.content)
                                } else {
                                    cb.content.to_vec()
                                };
                                if let Ok(content) = String::from_utf8(content) {
                                    self.handler.clipboard(content);
                                }
                            }
                        }
                        #[cfg(target_os = "android")]
                        crate::clipboard::handle_msg_multi_clipboards(_mcb);
                    }
                }
                Some(message::Union::Misc(misc)) => match misc.union {
                    Some(misc::Union::AudioFormat(f)) => {
                        self.audio_sender.send(MediaData::AudioFormat(f)).ok();
                    }
                    Some(misc::Union::PermissionInfo(p)) => {
                        log::info!("Change permission {:?} -> {}", p.permission, p.enabled);
                        // https://github.com/rustdesk/rustdesk/issues/3703#issuecomment-1474734754
                        match p.permission.enum_value() {
                            Ok(Permission::Keyboard) => {
                                *self.handler.server_keyboard_enabled.write().unwrap() = p.enabled;
                                #[cfg(feature = "flutter")]
                                #[cfg(not(target_os = "ios"))]
                                crate::flutter::update_text_clipboard_required();

                                self.handler.set_permission("keyboard", p.enabled);
                            }
                            Ok(Permission::Clipboard) => {
                                *self.handler.server_clipboard_enabled.write().unwrap() = p.enabled;
                                #[cfg(feature = "flutter")]
                                #[cfg(not(target_os = "ios"))]
                                crate::flutter::update_text_clipboard_required();
                                self.handler.set_permission("clipboard", p.enabled);
                            }
                            Ok(Permission::Audio) => {
                                self.handler.set_permission("audio", p.enabled);
                            }
                            Ok(Permission::Restart) => {
                                self.handler.set_permission("restart", p.enabled);
                            }
                            Ok(Permission::BlockInput) => {
                                self.handler.set_permission("block_input", p.enabled);
                            }
                            Ok(Permission::PrivacyMode) => {
                                self.handler.set_permission("privacy_mode", p.enabled);
                            }
                            _ => {}
                        }
                    }
                    Some(misc::Union::SwitchDisplay(s)) => {
                        #[cfg(feature = "mediacodec")]
                        if s.width > 0 && s.height > 0 {
                            if let Some(display) = self.handler.lc.write().unwrap().peer_info.as_mut()
                                .and_then(|peer| peer.displays.get_mut(s.display as usize))
                            {
                                display.width = s.width;
                                display.height = s.height;
                            }
                        }
                        self.handler.handle_peer_switch_display(&s);
                        if let Some(thread) = self.video_threads.get_mut(&(s.display as usize)) {
                            thread.video_sender.send(MediaData::Reset).ok();
                        }

                        let mut scale = 1.0;
                        if let Some(pi) = &self.handler.lc.read().unwrap().peer_info {
                            if let Some(d) = pi.displays.get(s.display as usize) {
                                scale = d.scale;
                            }
                        }

                        if s.width > 0 && s.height > 0 {
                            self.handler.set_display(
                                s.x,
                                s.y,
                                s.width,
                                s.height,
                                s.cursor_embedded,
                                scale,
                            );
                        }
                    }
                    Some(misc::Union::CloseReason(c)) => {
                        self.sent_close_reason = true; // The controlled end will close, no need to send close reason
                        self.handler.msgbox("error", "Connection Error", &c, "");
                        return false;
                    }
                    Some(misc::Union::BackNotification(notification)) => {
                        if !self.handle_back_notification(notification).await {
                            return false;
                        }
                    }
                    Some(misc::Union::Uac(uac)) => {
                        let keyboard = self.handler.server_keyboard_enabled.read().unwrap().clone();
                        #[cfg(feature = "flutter")]
                        {
                            if uac && keyboard {
                                self.handler.msgbox(
                                    "on-uac",
                                    "Prompt",
                                    "Please wait for confirmation of UAC...",
                                    "",
                                );
                            } else {
                                self.handler.cancel_msgbox("on-uac");
                                self.handler.cancel_msgbox("wait-uac");
                                self.handler.cancel_msgbox("elevation-error");
                            }
                        }
                        #[cfg(not(feature = "flutter"))]
                        {
                            let msgtype = "custom-uac-nocancel";
                            let title = "Prompt";
                            let text = "Please wait for confirmation of UAC...";
                            let link = "";
                            if uac && keyboard {
                                self.handler.msgbox(msgtype, title, text, link);
                            } else {
                                self.handler.cancel_msgbox(&format!(
                                    "{}-{}-{}-{}",
                                    msgtype, title, text, link,
                                ));
                            }
                        }
                    }
                    Some(misc::Union::ForegroundWindowElevated(elevated)) => {
                        let keyboard = self.handler.server_keyboard_enabled.read().unwrap().clone();
                        #[cfg(feature = "flutter")]
                        {
                            if elevated && keyboard {
                                self.handler.msgbox(
                                    "on-foreground-elevated",
                                    "Prompt",
                                    "elevated_foreground_window_tip",
                                    "",
                                );
                            } else {
                                self.handler.cancel_msgbox("on-foreground-elevated");
                                self.handler.cancel_msgbox("wait-uac");
                                self.handler.cancel_msgbox("elevation-error");
                            }
                        }
                        #[cfg(not(feature = "flutter"))]
                        {
                            let msgtype = "custom-elevated-foreground-nocancel";
                            let title = "Prompt";
                            let text = "elevated_foreground_window_tip";
                            let link = "";
                            if elevated && keyboard {
                                self.handler.msgbox(msgtype, title, text, link);
                            } else {
                                self.handler.cancel_msgbox(&format!(
                                    "{}-{}-{}-{}",
                                    msgtype, title, text, link,
                                ));
                            }
                        }
                    }
                    Some(misc::Union::ElevationResponse(err)) => {
                        if err.is_empty() {
                            self.handler.msgbox("wait-uac", "", "", "");
                        } else {
                            self.handler.cancel_msgbox("wait-uac");
                            self.handler
                                .msgbox("elevation-error", "Elevation Error", &err, "");
                        }
                    }
                    Some(misc::Union::PortableServiceRunning(b)) => {
                        self.handler.portable_service_running(b);
                        if self.elevation_requested && b {
                            self.handler.msgbox(
                                "custom-nocancel-success",
                                "Successful",
                                "Elevate successfully",
                                "",
                            );
                        }
                    }
                    Some(misc::Union::SupportedEncoding(e)) => {
                        log::info!("update supported encoding:{:?}", e);
                        self.handler.lc.write().unwrap().supported_encoding = e;
                    }
                    Some(misc::Union::FollowCurrentDisplay(d_idx)) => {
                        self.handler.set_current_display(d_idx);
                    }
                    _ => {}
                },
                Some(message::Union::TestDelay(t)) => {
                    self.handler.handle_test_delay(t, peer).await;
                }
                Some(message::Union::AudioFrame(frame)) => {
                    if !self.handler.lc.read().unwrap().disable_audio.v {
                        self.audio_sender
                            .send(MediaData::AudioFrame(Box::new(frame)))
                            .ok();
                    }
                }
                Some(message::Union::MessageBox(msgbox)) => {
                    let mut link = msgbox.link;
                    if let Some(v) = config::HELPER_URL.get(&link as &str) {
                        link = v.to_string();
                    } else {
                        log::warn!("Message box ignore link {} for security", &link);
                        link = "".to_string();
                    }
                    self.handler
                        .msgbox(&msgbox.msgtype, &msgbox.title, &msgbox.text, &link);
                }
                Some(message::Union::PeerInfo(pi)) => {
                    #[cfg(feature = "mediacodec")]
                    if let Some(peer) = self.handler.lc.write().unwrap().peer_info.as_mut() {
                        peer.displays = pi.displays.clone();
                    }
                    self.handler.set_displays(&pi.displays);
                    self.handler.set_platform_additions(&pi.platform_additions);
                }
                Some(message::Union::ScreenshotResponse(response)) => {
                    crate::client::screenshot::set_screenshot(response.data);
                    self.handler
                        .handle_screenshot_resp(response.sid, response.msg);
                }
                _ => {}
            }
        }
        true
    }

    fn set_peer_info(&mut self, pi: &PeerInfo) {
        self.peer_info.platform = pi.platform.clone();

        if let Ok(platform_additions) =
            serde_json::from_str::<HashMap<String, serde_json::Value>>(&pi.platform_additions)
        {
            self.peer_info.is_installed = platform_additions
                .get("is_installed")
                .map(|v| v.as_bool())
                .flatten()
                .unwrap_or(false);
            self.peer_info.idd_impl = platform_additions
                .get("idd_impl")
                .map(|v| v.as_str())
                .flatten()
                .unwrap_or_default()
                .to_string();
        }
    }

    async fn handle_back_notification(&mut self, notification: BackNotification) -> bool {
        match notification.union {
            Some(back_notification::Union::BlockInputState(state)) => {
                self.handle_back_msg_block_input(
                    state.enum_value_or(back_notification::BlockInputState::BlkStateUnknown),
                    notification.details,
                )
                .await;
            }
            Some(back_notification::Union::PrivacyModeState(state)) => {
                if !self
                    .handle_back_msg_privacy_mode(
                        state.enum_value_or(back_notification::PrivacyModeState::PrvStateUnknown),
                        notification.details,
                        notification.impl_key,
                    )
                    .await
                {
                    return false;
                }
            }
            _ => {}
        }
        true
    }

    #[inline(always)]
    fn update_block_input_state(&mut self, on: bool) {
        self.handler.update_block_input_state(on);
    }

    async fn handle_back_msg_block_input(
        &mut self,
        state: back_notification::BlockInputState,
        details: String,
    ) {
        match state {
            back_notification::BlockInputState::BlkOnSucceeded => {
                self.update_block_input_state(true);
            }
            back_notification::BlockInputState::BlkOnFailed => {
                self.handler.msgbox(
                    "custom-error",
                    "Block user input",
                    if details.is_empty() {
                        "Failed"
                    } else {
                        &details
                    },
                    "",
                );
                self.update_block_input_state(false);
            }
            back_notification::BlockInputState::BlkOffSucceeded => {
                self.update_block_input_state(false);
            }
            back_notification::BlockInputState::BlkOffFailed => {
                self.handler.msgbox(
                    "custom-error",
                    "Unblock user input",
                    if details.is_empty() {
                        "Failed"
                    } else {
                        &details
                    },
                    "",
                );
            }
            _ => {}
        }
    }

    #[inline(always)]
    fn update_privacy_mode(&mut self, impl_key: String, on: bool) {
        let mut config = self.handler.load_config();
        config.privacy_mode.v = on;
        if on {
            // For compatibility, version < 1.2.4, the default value is 'privacy_mode_impl_mag'.
            let impl_key = if impl_key.is_empty() {
                "privacy_mode_impl_mag".to_string()
            } else {
                impl_key
            };
            config
                .options
                .insert("privacy-mode-impl-key".to_string(), impl_key);
        }
        self.handler.save_config(config);

        self.handler.update_privacy_mode();
    }

    async fn handle_back_msg_privacy_mode(
        &mut self,
        state: back_notification::PrivacyModeState,
        details: String,
        impl_key: String,
    ) -> bool {
        match state {
            back_notification::PrivacyModeState::PrvOnByOther => {
                self.handler.msgbox(
                    "error",
                    "Connecting...",
                    "Someone turns on privacy mode, exit",
                    "",
                );
                return false;
            }
            back_notification::PrivacyModeState::PrvNotSupported => {
                self.handler
                    .msgbox("custom-error", "Privacy mode", "Unsupported", "");
                self.update_privacy_mode(impl_key, false);
            }
            back_notification::PrivacyModeState::PrvOnSucceeded => {
                self.handler
                    .msgbox("custom-nocancel", "Privacy mode", "Enter privacy mode", "");
                self.update_privacy_mode(impl_key, true);
            }
            back_notification::PrivacyModeState::PrvOnFailedDenied => {
                self.handler
                    .msgbox("custom-error", "Privacy mode", "Peer denied", "");
                self.update_privacy_mode(impl_key, false);
            }
            back_notification::PrivacyModeState::PrvOnFailedPlugin
            | back_notification::PrivacyModeState::PrvOnFailed => {
                self.handler.msgbox(
                    "custom-error",
                    "Privacy mode",
                    if details.is_empty() {
                        "Failed"
                    } else {
                        &details
                    },
                    "",
                );
                self.update_privacy_mode(impl_key, false);
            }
            back_notification::PrivacyModeState::PrvOffSucceeded => {
                self.handler
                    .msgbox("custom-nocancel", "Privacy mode", "Exit privacy mode", "");
                self.update_privacy_mode(impl_key, false);
            }
            back_notification::PrivacyModeState::PrvOffByPeer => {
                self.handler
                    .msgbox("custom-error", "Privacy mode", "Peer exit", "");
                self.update_privacy_mode(impl_key, false);
            }
            back_notification::PrivacyModeState::PrvOffFailed => {
                self.handler.msgbox(
                    "custom-error",
                    "Privacy mode",
                    if details.is_empty() {
                        "Failed to turn off"
                    } else {
                        &details
                    },
                    "",
                );
            }
            back_notification::PrivacyModeState::PrvOffUnknown => {
                self.handler
                    .msgbox("custom-error", "Privacy mode", "Turned off", "");
                // log::error!("Privacy mode is turned off with unknown reason");
                self.update_privacy_mode(impl_key, false);
            }
            _ => {}
        }
        true
    }

    fn new_video_thread(&mut self, display: usize) {
        let video_queue = Arc::new(RwLock::new(ArrayQueue::new(client::VIDEO_QUEUE_SIZE)));
        let (video_sender, video_receiver) = std::sync::mpsc::channel::<MediaData>();
        let decode_fps = Arc::new(RwLock::new(None));
        let frame_count = Arc::new(RwLock::new(0));
        let discard_queue = Arc::new(RwLock::new(false));
        let video_thread = VideoThread {
            video_queue: video_queue.clone(),
            video_sender,
            decode_fps: decode_fps.clone(),
            frame_count: frame_count.clone(),
            fps_control: Default::default(),
            discard_queue: discard_queue.clone(),
        };
        let handler = self.handler.ui_handler.clone();
        crate::client::start_video_thread(
            self.handler.clone(),
            display,
            video_receiver,
            video_queue,
            decode_fps,
            self.chroma.clone(),
            discard_queue,
            move |display: usize,
                  data: &mut scrap::ImageRgb,
                  _texture: *mut c_void,
                  pixelbuffer: bool| {
                *frame_count.write().unwrap() += 1;
                if pixelbuffer {
                    handler.on_rgba(display, data);
                } else {
                    #[cfg(all(feature = "vram", feature = "flutter"))]
                    handler.on_texture(display, _texture);
                }
            },
        );
        self.video_threads.insert(display, video_thread);
    }

    /// A shape that decoded, for the UI to ask for again; see `Session::cursor_shapes`.
    #[cfg(feature = "flutter")]
    fn keep_cursor_shape(&self, shape: CursorData) {
        self.handler
            .cursor_shapes
            .write()
            .unwrap()
            .insert(shape.id, shape);
    }

    fn dedupes_cursors(&self) -> bool {
        !crate::is_peer_naming_cursors_by_content(self.handler.lc.read().unwrap().version)
    }

    /// A shape the UI already has, under whatever handle, is only selected.
    fn set_cursor_data_by_content(&mut self, cd: CursorData) {
        let peer_id = cd.id;
        let id = CursorDedupe::name(&cd);
        if self.cursor_dedupe.has_decoded(id) {
            self.cursor_dedupe.record(peer_id, id);
            self.handler.set_cursor_id(id.to_string());
            return;
        }
        #[cfg(feature = "flutter")]
        let compressed = cd.colors.clone();
        match decode_cursor_data(cd) {
            Ok(mut cd) => {
                cd.id = id;
                self.cursor_dedupe.record(peer_id, id);
                #[cfg(feature = "flutter")]
                self.keep_cursor_shape(cursor_shape(id, &cd, &compressed));
                self.handler.set_cursor_data(cd);
            }
            Err(err) => {
                log::warn!("Rejected cursor {peer_id}: {err}");
                self.handler
                    .set_cursor_id(self.cursor_dedupe.rejected(peer_id, id).to_string());
            }
        }
    }
}

/// A shape that decoded, still compressed, under the id the UI knows it by. Copied once it
/// decoded: the message's colors may be a slice of a larger received buffer, which a clone
/// would keep alive.
#[cfg(feature = "flutter")]
fn cursor_shape(id: u64, cd: &CursorData, compressed: &[u8]) -> CursorData {
    CursorData {
        id,
        hotx: cd.hotx,
        hoty: cd.hoty,
        width: cd.width,
        height: cd.height,
        colors: compressed.to_vec().into(),
        ..Default::default()
    }
}

/// The RGBA of a shape kept by `Session::cursor_shapes`, for the UI to draw it again.
#[cfg(any(feature = "flutter", test))]
pub(crate) fn kept_cursor_rgba(shape: CursorData) -> hbb_common::ResultType<CursorData> {
    decode_cursor_data(shape)
}

// Both UI handlers receive validated, uncompressed RGBA from the receive loop.
fn decode_cursor_data(data: CursorData) -> hbb_common::ResultType<CursorData> {
    use hbb_common::{anyhow::anyhow, bail};

    // Limit decoded cursor data to 1 MiB before JSON serialization.
    const MAX_CURSOR_SIZE: i32 = 512;
    const RGBA_CHANNELS: usize = 4;

    let mut cd = data;
    if !(1..=MAX_CURSOR_SIZE).contains(&cd.width) || !(1..=MAX_CURSOR_SIZE).contains(&cd.height) {
        bail!("invalid source size {}x{}", cd.width, cd.height);
    }
    if !(0..cd.width).contains(&cd.hotx) || !(0..cd.height).contains(&cd.hoty) {
        bail!(
            "hotspot ({},{}) is outside the cursor image",
            cd.hotx,
            cd.hoty
        );
    }
    let expected = (cd.width as usize)
        .checked_mul(cd.height as usize)
        .and_then(|pixels| pixels.checked_mul(RGBA_CHANNELS))
        .ok_or_else(|| anyhow!("cursor RGBA size overflow"))?;
    let max_compressed_size = zstd::zstd_safe::compress_bound(expected);
    if cd.colors.len() > max_compressed_size {
        bail!(
            "compressed cursor data too large: {} bytes (limit {max_compressed_size})",
            cd.colors.len()
        );
    }
    let colors = zstd::bulk::decompress(&cd.colors, expected)?;
    if colors.len() != expected {
        bail!(
            "invalid RGBA length: expected {expected}, got {}",
            colors.len()
        );
    }
    cd.colors = colors.into();
    Ok(cd)
}

/// Gives the UI one id per shape for a peer that names shapes by handle, so the UI decodes and
/// keeps a shape once however many handles it arrives under. Nothing is dropped for the
/// connection: the peer sends a shape once and may select any handle it named again.
#[derive(Default)]
struct CursorDedupe {
    ids: HashMap<u64, u64>,
    // The shapes given to the UI, by content id: whether one that arrives needs decoding.
    decoded: HashSet<u64>,
}

impl CursorDedupe {
    /// Hashes the compressed colors: one peer compresses the same pixels to the same bytes, and
    /// a shape seen before is then never decompressed again.
    fn name(cd: &CursorData) -> u64 {
        crate::cursor_content_id(cd.width, cd.height, cd.hotx, cd.hoty, &cd.colors)
    }

    fn id(&self, peer_id: u64) -> u64 {
        self.ids.get(&peer_id).copied().unwrap_or(peer_id)
    }

    fn has_decoded(&self, id: u64) -> bool {
        self.decoded.contains(&id)
    }

    /// Names the handle once its shape decoded, now or before.
    fn record(&mut self, peer_id: u64, id: u64) {
        self.ids.insert(peer_id, id);
        self.decoded.insert(id);
    }

    /// Names a known handle by the content of a shape that did not decode, which is never
    /// marked decoded: the UI has no shape under it, now or when the handle is selected again,
    /// and the shape the handle named before is not shown in its place. A handle never seen is
    /// not filed, so what a peer sends that does not decode takes no room.
    fn rejected(&mut self, peer_id: u64, id: u64) -> u64 {
        if let Some(named) = self.ids.get_mut(&peer_id) {
            *named = id;
        }
        id
    }
}

#[derive(Debug, Default)]
struct FpsControl {
    refresh_times: usize,
    last_refresh_instant: Option<Instant>,
    idle_counter: usize,
    inactive_counter: usize,
}

struct VideoThread {
    video_queue: Arc<RwLock<ArrayQueue<VideoFrame>>>,
    video_sender: MediaSender,
    decode_fps: Arc<RwLock<Option<usize>>>,
    frame_count: Arc<RwLock<usize>>,
    discard_queue: Arc<RwLock<bool>>,
    fps_control: FpsControl,
}

impl Drop for VideoThread {
    fn drop(&mut self) {
        // since channels are buffered, messages sent before the disconnect will still be properly received.
        *self.discard_queue.write().unwrap() = true;
    }
}

#[cfg(test)]
#[cfg(feature = "flutter")]
mod tests {
    use super::*;
    use crate::flutter::FlutterHandler;

    /// A round's `Remote` over a loopback pair, before any login: what it sends to `peer`
    /// arrives at `far`.
    async fn remote_and_peer() -> (Remote<FlutterHandler>, Stream, Stream) {
        let listener = hbb_common::tcp::new_listener("127.0.0.1:0", false)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (peer, accepted) = tokio::join!(
            hbb_common::socket_client::connect_tcp(addr.to_string(), 3000),
            listener.accept()
        );
        let (accepted, far_addr) = accepted.unwrap();
        let far = Stream::Tcp(hbb_common::tcp::FramedStream::from(accepted, far_addr));
        let (sender, receiver) = mpsc::unbounded_channel::<Data>();
        let remote = Remote::new(Session::<FlutterHandler>::default(), receiver, sender);
        (remote, peer.unwrap(), far)
    }

    async fn arrives(far: &mut Stream) -> bool {
        matches!(hbb_common::timeout(300, far.next()).await, Ok(Some(Ok(_))))
    }

    fn clipboard() -> Data {
        let mut msg = Message::new();
        msg.set_clipboard(Clipboard {
            content: b"copied while this login was pending".to_vec().into(),
            ..Default::default()
        });
        Data::Message(msg)
    }

    fn auth_2fa() -> Data {
        let mut msg = Message::new();
        msg.set_auth_2fa(Auth2FA {
            code: "123456".to_owned(),
            ..Default::default()
        });
        Data::Message(msg)
    }

    #[tokio::test]
    async fn non_desktop_sessions_are_rejected_before_connecting() {
        for conn_type in [
            ConnType::FILE_TRANSFER,
            ConnType::PORT_FORWARD,
            ConnType::RDP,
            ConnType::VIEW_CAMERA,
            ConnType::TERMINAL,
        ] {
            let result = Client::start("", conn_type, Session::<FlutterHandler>::default()).await;
            assert!(matches!(result, Err(err) if err.to_string() == "Unsupported session type"));
        }
    }

    #[tokio::test]
    async fn legacy_role_switch_messages_close_before_and_after_login() {
        let mut request = Message::new();
        let mut misc = Misc::new();
        misc.set_switch_sides_request(SwitchSidesRequest::default());
        request.set_misc(misc);
        let mut back = Message::new();
        let mut misc = Misc::new();
        misc.set_switch_back(SwitchBack::default());
        back.set_misc(misc);
        let mut response = Message::new();
        response.set_switch_sides_response(SwitchSidesResponse::default());
        for connected in [false, true] {
            for message in [&request, &back, &response] {
                let (mut remote, mut peer, mut far) = remote_and_peer().await;
                remote.is_connected = connected;
                assert!(
                    !remote
                        .handle_msg_from_peer(&message.write_to_bytes().unwrap(), &mut peer)
                        .await
                );
                let bytes = hbb_common::timeout(1000, far.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let reply = Message::parse_from_bytes(&bytes).unwrap();
                assert_eq!(
                    reply.misc().close_reason(),
                    "Runtime role switching is not supported"
                );
                assert_eq!(remote.is_connected, connected);
                assert!(remote.handler.lc.read().unwrap().password.is_empty());
            }
        }
    }

    // A clipboard queued before this round's login stays here; what the login itself sends
    // through the same queue does not.
    #[tokio::test]
    async fn a_clipboard_queued_before_this_rounds_login_is_dropped() {
        let (mut remote, mut peer, mut far) = remote_and_peer().await;
        assert!(!remote.is_connected);
        assert!(remote.handle_msg_from_ui(clipboard(), &mut peer).await);
        assert!(
            !arrives(&mut far).await,
            "a clipboard went out before the login"
        );
        assert!(remote.handle_msg_from_ui(auth_2fa(), &mut peer).await);
        assert!(arrives(&mut far).await, "the 2FA code was held back");

        remote.is_connected = true;
        assert!(remote.handle_msg_from_ui(clipboard(), &mut peer).await);
        assert!(
            arrives(&mut far).await,
            "a clipboard after the login was held back"
        );
    }
}

#[cfg(test)]
mod cursor_dedupe_tests {
    use super::*;

    fn shape(id: u64, hotx: i32, colors: &[u8]) -> CursorData {
        CursorData {
            id,
            hotx,
            width: 2,
            height: 2,
            colors: colors.to_vec().into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_shape_sent_under_a_new_handle_keeps_its_id() {
        let mut dedupe = CursorDedupe::default();
        let first = CursorDedupe::name(&shape(1, 0, b"arrow"));
        assert!(!dedupe.has_decoded(first));
        dedupe.record(1, first);

        let again = CursorDedupe::name(&shape(2, 0, b"arrow"));
        assert_eq!(again, first);
        assert!(dedupe.has_decoded(again));
        dedupe.record(2, again);
        assert_eq!(dedupe.id(2), first);
        assert_eq!(dedupe.id(1), first);
    }
}

#[cfg(test)]
mod kept_cursor_tests {
    use super::*;

    fn compressed(width: i32, height: i32) -> CursorData {
        let rgba = vec![7u8; (width * height * 4) as usize];
        CursorData {
            id: 1,
            width,
            height,
            colors: hbb_common::compress::compress(&rgba).into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_kept_shape_is_checked_again_when_it_is_read() {
        assert_eq!(kept_cursor_rgba(compressed(4, 4)).unwrap().colors.len(), 64);
        assert!(
            kept_cursor_rgba(compressed(513, 1)).is_err(),
            "over the size cap"
        );
        let mut bomb = compressed(4, 4);
        bomb.colors = hbb_common::compress::compress(&vec![0u8; 1 << 20]).into();
        assert!(kept_cursor_rgba(bomb).is_err(), "more pixels than its size");
    }
}
