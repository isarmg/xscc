use super::{input_service::*, *};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::clipboard::{update_clipboard, ClipboardSide};
#[cfg(target_os = "android")]
use crate::keyboard::client::map_key_to_control_key;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::platform::WallPaperRemover;
#[cfg(windows)]
use crate::portable_service::client as portable_client;
#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::{common::DEVICE_NAME, flutter::connection_manager::start_channel};
use crate::{display_service, ipc, privacy_mode, video_service, VERSION};
use base::{
    config::keys,
    message_proto::{option_message::BoolOption, permission_info::Permission},
    session_protocol::{
        LOGIN_MSG_2FA_WRONG, LOGIN_MSG_NO_PASSWORD_ACCESS, LOGIN_MSG_OFFLINE, LOGIN_MSG_PASSWORD_WRONG,
        LOGIN_SCREEN_WAYLAND, REQUIRE_2FA,
    },
};
use cidr_utils::cidr::IpCidr;
#[cfg(target_os = "android")]
use hbb_common::protobuf::EnumOrUnknown;
use hbb_common::{
    config::{
        self, decode_permanent_password_h1_from_storage, decode_preset_password_h1_from_storage,
        local_permanent_password_storage_is_usable_for_auth,
        preset_permanent_password_storage_is_usable_for_auth, Config, TrustedDevice,
    },
    futures::{SinkExt, StreamExt},
    get_time, get_version_number,
    password_security::{self as password, ApproveMode},
    sha2::{Digest, Sha256},
    sleep, timeout,
    tokio::{
        sync::mpsc,
        time::{self, Duration, Instant},
    },
};
#[cfg(any(target_os = "android", target_os = "ios"))]
use scrap::android::{call_main_service_key_event, call_main_service_pointer_input};
use serde_json::json;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv6Addr},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc as std_mpsc,
    },
};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use system_shutdown;

#[cfg(windows)]
use crate::virtual_display_manager;
pub type Sender = mpsc::UnboundedSender<(Instant, Arc<Message>)>;

const FAILURE_IDX_ID_WHITELIST: usize = 2;
// How long a rejection counts, so also how long a blocked address stays blocked. Longer
// throttles enumeration harder; shorter limits collateral on whitelisted neighbours.
const ID_WHITELIST_FAILURE_DECAY_MINUTES: i32 = 10;

/// A connection not authorized within this long of starting is closed, however alive it
/// keeps itself: a wrong password, a pending 2FA or an accept prompt still unanswered.
/// The controller reconnects on its own and the prompt
/// comes back. A connection that says nothing at all goes at the 30 s idle timeout already.
const LOGIN_GRACE: Duration = Duration::from_secs(180);
/// Connections between accept and authorization, across every transport: the resource bound.
/// At this many a further arrival is refused and the oldest is told to go, one at a time.
const MAX_UNAUTHORIZED_CONNS: usize = 64;
/// Of those, how many one address may hold at once: a quarter of the room. A fairness cap
/// against the cheapest flood, one host with one address, not a security boundary: any pool
/// of addresses passes it, and the bound above is what holds. Meaningful only while the
/// address is the controller's directly connected source address.
const MAX_UNAUTHORIZED_CONNS_PER_ADDR: usize = 16;
/// The largest message a connection may send before it authorizes. Until then a peer sends only
/// a public key, a login request, a test delay and a close reason, none of which carries an
/// unbounded field - a server hands the login request's avatar out as a URL, and only a custom
/// client that inlines an image into the avatar option instead reaches this. Sized to the read
/// buffer tungstenite allocates per WebSocket connection regardless, so there the cap costs
/// nothing beyond a floor already paid; with MAX_UNAUTHORIZED_CONNS it holds them to 8 MiB in
/// all, against the 1 GiB a single one could make us hold before.
pub const MAX_UNAUTHORIZED_MESSAGE: usize = 128 * 1024;

/// A place among the unauthorized connections, taken before the identity handshake and given
/// back on drop: at authorization, or when the connection ends first. The count of live
/// guards is the bound; an evicted one is told to go and keeps its place until it has.
pub struct UnauthorizedID {
    id: i32,
    shared: Arc<UnauthorizedShared>,
}

struct UnauthorizedShared {
    evicted: AtomicBool,
    notify: hbb_common::tokio::sync::Notify,
}

/// Admit a connection from `ip` among the unauthorized ones. `None` when that address already
/// holds its share, or when the global limit is reached: then the oldest connection is told to
/// go, unless one is on its way out already, and this one is refused rather than let in on a
/// place that is still occupied. At most one connection is ever on its way out, so a burst of
/// refused arrivals clears no more room than a single one. The controller retries on its own.
pub fn admit_unauthorized(id: i32, ip: IpAddr) -> Option<UnauthorizedID> {
    let mut conns = UNAUTHORIZED_CONNS.lock().unwrap();
    if conns.iter().filter(|(_, held, _)| *held == ip).count() >= MAX_UNAUTHORIZED_CONNS_PER_ADDR {
        return None;
    }
    if conns.len() >= MAX_UNAUTHORIZED_CONNS {
        if let Some((_, _, oldest)) = conns.first() {
            if !oldest.evicted.swap(true, Ordering::AcqRel) {
                oldest.notify.notify_one();
            }
        }
        return None;
    }
    let shared = Arc::new(UnauthorizedShared {
        evicted: AtomicBool::new(false),
        notify: hbb_common::tokio::sync::Notify::new(),
    });
    conns.push((id, ip, shared.clone()));
    Some(UnauthorizedID { id, shared })
}

impl UnauthorizedID {
    /// Whether this connection was told to go to make room for a newer one.
    pub fn is_evicted(&self) -> bool {
        self.shared.evicted.load(Ordering::Acquire)
    }

    /// Resolves once this connection is told to go; at once if it already was.
    pub async fn evicted(&self) {
        if self.is_evicted() {
            return;
        }
        self.shared.notify.notified().await;
    }
}

impl Drop for UnauthorizedID {
    fn drop(&mut self) {
        UNAUTHORIZED_CONNS
            .lock()
            .unwrap()
            .retain(|(id, _, _)| *id != self.id);
    }
}

/// Resolves when the connection holding `unauthorized` is evicted; never once it has
/// authorized and given its place back.
async fn unauthorized_evicted(unauthorized: &Option<UnauthorizedID>) {
    match unauthorized {
        Some(u) => u.evicted().await,
        None => std::future::pending().await,
    }
}

/// Resolves at the login deadline of a connection started at `started`; never once it has
/// authorized.
async fn login_deadline(authorized: bool, started: Instant) {
    if authorized {
        return std::future::pending().await;
    }
    time::sleep_until(started + LOGIN_GRACE).await
}

lazy_static::lazy_static! {
    // Connections between accept and authorization, oldest first; see admit_unauthorized.
    static ref UNAUTHORIZED_CONNS: Mutex<Vec<(i32, IpAddr, Arc<UnauthorizedShared>)>> = Default::default();
    // [0] password, [1] 2FA, [2] ID whitelist.
    // Bucket 2 is separate so its rejections do not touch the password / 2FA budgets. It is
    // decayed in `check_id_whitelist` and cleared on auth, never on a bare id match.
    static ref LOGIN_FAILURES: [Arc::<Mutex<HashMap<String, (i32, i32, i32)>>>; 3] = Default::default();
    static ref SESSIONS: Arc::<Mutex<HashMap<SessionKey, Session>>> = Default::default();
    static ref ALIVE_CONNS: Arc::<Mutex<Vec<i32>>> = Default::default();
    pub static ref AUTHED_CONNS: Arc::<Mutex<Vec<AuthedConn>>> = Default::default();
    pub static ref CONTROL_PERMISSIONS_ARRAY: Arc::<Mutex<Vec<(i32, ControlPermissions)>>> = Default::default();
    static ref WAKELOCK_SENDER: Arc::<Mutex<std::sync::mpsc::Sender<(usize, usize)>>> = Arc::new(Mutex::new(start_wakelock_thread()));
    static ref WAKELOCK_KEEP_AWAKE_OPTION: Arc::<Mutex<Option<bool>>> = Default::default();
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Avoid data-dependent early exits.
    let mut x: u8 = 0;
    for i in 0..a.len() {
        x |= a[i] ^ b[i];
    }
    x == 0
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
lazy_static::lazy_static! {
    static ref WALLPAPER_REMOVER: Arc<Mutex<Option<WallPaperRemover>>> = Default::default();
}
pub static CLICK_TIME: AtomicI64 = AtomicI64::new(0);
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub static MOUSE_MOVE_TIME: AtomicI64 = AtomicI64::new(0);

#[derive(Clone, Default)]
pub struct ConnInner {
    id: i32,
    tx: Option<Sender>,
    tx_video: Option<Sender>,
}

struct InputMouse {
    msg: MouseEvent,
    conn_id: i32,
    username: String,
    argb: u32,
    simulate: bool,
    show_cursor: bool,
}

enum MessageInput {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Mouse(InputMouse),
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Key((KeyEvent, bool)),
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Pointer((PointerDeviceEvent, i32)),
    BlockOn,
    BlockOff,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SessionKey {
    peer_id: String,
    name: String,
    session_id: u64,
}

#[derive(Clone, Debug)]
struct Session {
    last_recv_time: Arc<Mutex<Instant>>,
    random_password: String,
    tfa: bool,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct StartCmIpcPara {
    rx_to_cm: mpsc::UnboundedReceiver<ipc::Data>,
    tx_from_cm: mpsc::UnboundedSender<ipc::Data>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum AuthConnType {
    Remote,
}

impl AuthConnType {
    fn as_str(self) -> &'static str {
        match self {
            AuthConnType::Remote => "remote",
        }
    }
}

pub struct Connection {
    inner: ConnInner,
    display_idx: usize,
    stream: super::Stream,
    server: super::ServerPtrWeak,
    hash: Hash,
    timer: crate::RustDeskInterval,
    tx_to_cm: mpsc::UnboundedSender<ipc::Data>,
    authorized: bool,
    // The place among the unauthorized connections; given back at authorization.
    unauthorized_id: Option<UnauthorizedID>,
    require_2fa: Option<totp_rs::TOTP>,
    awaiting_2fa: bool,
    keyboard: bool,
    clipboard: bool,
    audio: bool,
    restart: bool,
    block_input: bool,
    privacy_mode: bool,
    control_permissions: Option<ControlPermissions>,
    last_test_delay: Option<Instant>,
    network_delay: u32,
    lock_after_session_end: bool,
    show_remote_cursor: bool,
    // by peer
    ip: String,
    // by peer
    disable_keyboard: bool,
    // by peer
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    show_my_cursor: bool,
    // by peer
    disable_clipboard: bool,
    // by peer
    disable_audio: bool,
    tx_input: std_mpsc::Sender<MessageInput>,
    // handle input messages
    video_ack_required: bool,
    lr: LoginRequest,
    // Authentication retries may update credentials, but not the requested session scope.
    // A digest, so no peer-controlled strings are retained.
    login_scope: Option<[u8; 32]>,
    peer_argb: u32,
    session_last_recv_time: Option<Arc<Mutex<Instant>>>,
    #[cfg(windows)]
    portable: PortableState,
    options_in_login: Option<OptionMessage>,
    #[cfg(not(any(target_os = "ios")))]
    pressed_modifiers: HashSet<rdev::Key>,
    closed: bool,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    start_cm_ipc_para: Option<StartCmIpcPara>,
    auto_disconnect_timer: Option<(Instant, u64)>,
    authed_conn_id: Option<self::raii::AuthedConnID>,
    last_supported_encoding: Option<SupportedEncoding>,
    services_subed: bool,
    #[cfg(target_os = "macos")]
    retina: Retina,
    follow_remote_cursor: bool,
    follow_remote_window: bool,
    multi_ui_session: bool,
    tx_from_authed: mpsc::UnboundedSender<ipc::Data>,
    // Used to avoid too many repeated scope violation warnings.
    scope_violation_messages: HashSet<&'static str>,
}

impl ConnInner {
    pub fn new(id: i32, tx: Option<Sender>, tx_video: Option<Sender>) -> Self {
        Self { id, tx, tx_video }
    }
}

impl Subscriber for ConnInner {
    #[inline]
    fn id(&self) -> i32 {
        self.id
    }

    #[inline]
    fn send(&mut self, msg: Arc<Message>) {
        // Send SwitchDisplay on the same channel as VideoFrame to avoid send order problems.
        let tx_by_video = match &msg.union {
            Some(message::Union::VideoFrame(_)) => true,
            Some(message::Union::Misc(misc)) => match &misc.union {
                Some(misc::Union::SwitchDisplay(_)) => true,
                _ => false,
            },
            _ => false,
        };
        let tx = if tx_by_video {
            self.tx_video.as_mut()
        } else {
            self.tx.as_mut()
        };
        tx.map(|tx| {
            allow_err!(tx.send((Instant::now(), msg)));
        });
    }
}

const TEST_DELAY_TIMEOUT: Duration = Duration::from_secs(1);
const SEC30: Duration = Duration::from_secs(30);
const H1: Duration = Duration::from_secs(3600);
const MILLI1: Duration = Duration::from_millis(1);
const SEND_TIMEOUT_VIDEO: u64 = 12_000;
const SESSION_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the DRM backend can serve a Wayland login screen here.
///
/// A cold cache probes off-thread; admission still requires a definitive `Available` verdict.
#[cfg(all(target_os = "linux", feature = "drm"))]
fn drm_can_serve_login_screen() -> bool {
    super::drm_capturer::availability_cached() == super::drm_capturer::Availability::Available
}

/// Without the feature nothing can capture a Wayland greeter, so the refusal stands.
#[cfg(all(target_os = "linux", not(feature = "drm")))]
fn drm_can_serve_login_screen() -> bool {
    false
}

impl Connection {
    pub async fn start(
        addr: SocketAddr,
        stream: super::Stream,
        id: i32,
        server: super::ServerPtrWeak,
        meta: super::ConnectionMeta,
        unauthorized: UnauthorizedID,
    ) {
        let super::ConnectionMeta {
            control_permissions,
            ..
        } = meta;
        // Android is not supported yet, so we always set control_permissions to None.
        #[cfg(target_os = "android")]
        let control_permissions = None;
        let _raii_id = raii::ConnectionID::new(id);
        let _raii_control_permissions_id =
            raii::ControlPermissionsID::new(id, &control_permissions);
        let salt = Config::get_effective_permanent_password_salt();
        let hash = Hash {
            salt,
            challenge: Config::get_auto_password(6),
            ..Default::default()
        };
        let (tx_from_cm_holder, mut rx_from_cm) = mpsc::unbounded_channel::<ipc::Data>();
        // holding tx_from_cm_holder to avoid cpu burning of rx_from_cm.recv when all sender closed
        let tx_from_cm = tx_from_cm_holder.clone();
        let (tx_to_cm, rx_to_cm) = mpsc::unbounded_channel::<ipc::Data>();
        let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Arc<Message>)>();
        let (tx_video, mut rx_video) = mpsc::unbounded_channel::<(Instant, Arc<Message>)>();
        let (tx_input, _rx_input) = std_mpsc::channel();
        let (tx_from_authed, mut rx_from_authed) = mpsc::unbounded_channel::<ipc::Data>();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let tx_cloned = tx.clone();
        let mut conn = Self {
            inner: ConnInner {
                id,
                tx: Some(tx),
                tx_video: Some(tx_video),
            },
            require_2fa: crate::auth_2fa::get_2fa(None),
            awaiting_2fa: false,
            // Defer display enumeration until login succeeds. Monitor login replaces this
            // with the primary index returned with the refreshed display snapshot.
            display_idx: 0,
            stream,
            server,
            hash,
            timer: crate::rustdesk_interval(time::interval(SEC30)),
            tx_to_cm,
            authorized: false,
            unauthorized_id: Some(unauthorized),
            keyboard: Self::permission(keys::OPTION_ENABLE_KEYBOARD, &control_permissions),
            clipboard: Self::permission(keys::OPTION_ENABLE_CLIPBOARD, &control_permissions),
            audio: Self::permission(keys::OPTION_ENABLE_AUDIO, &control_permissions),
            // to-do: make sure is the option correct here
            restart: Self::permission(keys::OPTION_ENABLE_REMOTE_RESTART, &control_permissions),
            block_input: Self::permission(keys::OPTION_ENABLE_BLOCK_INPUT, &control_permissions),
            privacy_mode: Self::permission(keys::OPTION_ENABLE_PRIVACY_MODE, &control_permissions),
            control_permissions,
            last_test_delay: None,
            network_delay: 0,
            lock_after_session_end: false,
            show_remote_cursor: false,
            follow_remote_cursor: false,
            follow_remote_window: false,
            multi_ui_session: false,
            ip: "".to_owned(),
            disable_audio: false,
            disable_clipboard: false,
            disable_keyboard: false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            show_my_cursor: false,
            tx_input,
            video_ack_required: false,
            lr: Default::default(),
            login_scope: None,
            peer_argb: 0u32,
            session_last_recv_time: None,
            #[cfg(windows)]
            portable: Default::default(),
            options_in_login: None,
            #[cfg(not(any(target_os = "ios")))]
            pressed_modifiers: Default::default(),
            closed: false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            start_cm_ipc_para: Some(StartCmIpcPara {
                rx_to_cm,
                tx_from_cm,
            }),
            auto_disconnect_timer: None,
            authed_conn_id: None,
            last_supported_encoding: None,
            services_subed: false,
            #[cfg(target_os = "macos")]
            retina: Retina::default(),
            tx_from_authed,
            scope_violation_messages: HashSet::new(),
        };
        let addr = hbb_common::try_into_v4(addr);
        if !conn.on_open(addr).await {
            conn.closed = true;
            // sleep to ensure msg got received.
            sleep(1.).await;
            return;
        }
        #[cfg(target_os = "android")]
        start_channel(rx_to_cm, tx_from_cm);
        #[cfg(target_os = "android")]
        conn.send_permission(Permission::Keyboard, conn.keyboard)
            .await;
        #[cfg(not(target_os = "android"))]
        if !conn.keyboard {
            conn.send_permission(Permission::Keyboard, false).await;
        }
        if !conn.clipboard {
            conn.send_permission(Permission::Clipboard, false).await;
        }
        if !conn.audio {
            conn.send_permission(Permission::Audio, false).await;
        }
        if !conn.restart {
            conn.send_permission(Permission::Restart, false).await;
        }
        if !conn.block_input {
            conn.send_permission(Permission::BlockInput, false).await;
        }
        if !conn.privacy_mode {
            conn.send_permission(Permission::PrivacyMode, false).await;
        }
        let mut test_delay_timer =
            crate::rustdesk_interval(time::interval_at(Instant::now(), TEST_DELAY_TIMEOUT));
        let mut last_recv_time = Instant::now();
        let started = Instant::now();

        conn.stream.set_send_timeout(SEND_TIMEOUT_VIDEO);

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        std::thread::spawn(move || Self::handle_input(_rx_input, tx_cloned));
        let mut second_timer = crate::rustdesk_interval(time::interval(Duration::from_secs(1)));

        loop {
            tokio::select! {
                // Both end an unauthorized connection at once, not on the next timer tick:
                // told to go to make room, or past the grace for its authorization. Neither
                // fires once the connection has authorized.
                _ = unauthorized_evicted(&conn.unauthorized_id) => {
                    conn.on_close("Timeout", true).await;
                    break;
                }
                _ = login_deadline(conn.authorized, started) => {
                    conn.on_close("Timeout", true).await;
                    break;
                }
                Some(data) = rx_from_cm.recv() => {
                    match data {
                        ipc::Data::Authorize => {

                            conn.require_2fa.take();
                            if !conn.send_logon_response_and_keep_alive().await {
                                break;
                            }
                        }
                        ipc::Data::Close => {
                            conn.send_close_reason_no_retry("").await;
                            conn.on_close("connection manager", true).await;
                            break;
                        }
                        // The connection manager's window went away rather than a person
                        // disconnecting this peer. End the session exactly as above, but do not
                        // send the manual close reason: it is the one thing that stops the peer
                        // from retrying, and on a logout the retry is the whole point - it is
                        // what puts the peer back on the login screen a moment later.
                        #[cfg(target_os = "linux")]
                        ipc::Data::CmWindowClosed => {
                            conn.on_close("connection manager window closed", true).await;
                            break;
                        }
                        ipc::Data::CmErr(e) => {
                            if e != "expected" {
                                // cm closed before connection
                                conn.on_close(&format!("connection manager error: {}", e), false).await;
                                break;
                            }
                        }
                        ipc::Data::SwitchPermission{name, enabled} => {
                            log::info!("Change permission {} -> {}", name, enabled);
                            if &name == "keyboard" {
                                conn.keyboard = enabled;
                                conn.send_permission(Permission::Keyboard, enabled).await;
                                if let Some(s) = conn.server.upgrade() {
                                    s.write().unwrap().subscribe(
                                        super::clipboard_service::NAME,
                                        conn.inner.clone(), conn.can_sub_clipboard_service());
                                    s.write().unwrap().subscribe(
                                        NAME_CURSOR,
                                        conn.inner.clone(), enabled || conn.show_remote_cursor);
                                }
                            } else if &name == "clipboard" {
                                conn.clipboard = enabled;
                                conn.send_permission(Permission::Clipboard, enabled).await;
                                if let Some(s) = conn.server.upgrade() {
                                    s.write().unwrap().subscribe(
                                        super::clipboard_service::NAME,
                                        conn.inner.clone(), conn.can_sub_clipboard_service());
                                }
                            } else if &name == "audio" {
                                conn.audio = enabled;
                                conn.send_permission(Permission::Audio, enabled).await;
                                if conn.authorized {
                                    if let Some(s) = conn.server.upgrade() {
                                        s.write().unwrap().subscribe(
                                            super::audio_service::NAME,
                                            conn.inner.clone(), conn.audio_enabled());
                                    }
                                }
                            } else if &name == "restart" {
                                conn.restart = enabled;
                                conn.send_permission(Permission::Restart, enabled).await;
                            } else if &name == "block_input" {
                                conn.block_input = enabled;
                                conn.send_permission(Permission::BlockInput, enabled).await;
                            } else if &name == "privacy_mode" {
                                // Keep permission state and runtime state consistent:
                                // when revoking the permission, try to leave privacy mode first.
                                // Otherwise we could end up in an inconsistent state where
                                // permission looks disabled while privacy mode is still active.
                                if !enabled && privacy_mode::is_in_privacy_mode() {
                                    if let Some(conn_id) = privacy_mode::get_privacy_mode_conn_id() {
                                        if conn_id == conn.inner.id() {
                                            let impl_key =
                                                privacy_mode::get_cur_impl_key().unwrap_or_default();
                                            let turn_off_res =
                                                privacy_mode::turn_off_privacy(conn_id, None);
                                            match turn_off_res {
                                                Some(Ok(_)) => {
                                                    let msg_out = crate::common::make_privacy_mode_msg(
                                                        back_notification::PrivacyModeState::PrvOffByPeer,
                                                        impl_key.clone(),
                                                    );
                                                    conn.send(msg_out).await;
                                                }
                                                _ => {
                                                    let msg_out = Self::turn_off_privacy_result_to_msg(
                                                        turn_off_res,
                                                        impl_key,
                                                    );
                                                    conn.send(msg_out).await;
                                                    // Turn-off failed, so revert CM's optimistic toggle
                                                    // and keep the previous permission value.
                                                    conn.send_to_cm(ipc::Data::SwitchPermission {
                                                        name: "privacy_mode".to_owned(),
                                                        enabled: conn.privacy_mode,
                                                    });
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                                conn.privacy_mode = enabled;
                                conn.send_permission(Permission::PrivacyMode, enabled).await;
                            }
                        }
                        ipc::Data::RawMessage(bytes) => {
                            allow_err!(conn.stream.send_raw(bytes).await);
                        }
                        ipc::Data::PrivacyModeState((_, state, impl_key)) => {
                            let msg_out = match state {
                                privacy_mode::PrivacyModeState::OffSucceeded => {
                                    crate::common::make_privacy_mode_msg(
                                        back_notification::PrivacyModeState::PrvOffSucceeded,
                                        impl_key,
                                    )
                                }
                                privacy_mode::PrivacyModeState::OffByPeer => {
                                    crate::common::make_privacy_mode_msg(
                                        back_notification::PrivacyModeState::PrvOffByPeer,
                                        impl_key,
                                    )
                                }
                                privacy_mode::PrivacyModeState::OffUnknown => {
                                     crate::common::make_privacy_mode_msg(
                                        back_notification::PrivacyModeState::PrvOffUnknown,
                                        impl_key,
                                    )
                                }
                            };
                            conn.send(msg_out).await;
                        }
                        #[cfg(windows)]
                        ipc::Data::DataPortableService(ipc::DataPortableService::RequestStart) => {
                            if let Err(e) = portable_client::start_portable_service(portable_client::StartPara::Direct) {
                                log::error!("Failed to start portable service from cm: {:?}", e);
                            }
                        }
                        _ => {}
                    }
                },
                res = conn.stream.next() => {
                    if let Some(res) = res {
                        match res {
                            Err(err) => {
                                conn.on_close(&err.to_string(), true).await;
                                break;
                            },
                            Ok(bytes) => {
                                last_recv_time = Instant::now();
                                conn.session_last_recv_time.as_mut().map(|t| *t.lock().unwrap() = Instant::now());
                                if let Ok(msg_in) = Message::parse_from_bytes(&bytes) {
                                    if !conn.on_message(msg_in).await {
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        conn.on_close("Reset by the peer", true).await;
                        break;
                    }
                },

                Some((instant, value)) = rx_video.recv() => {
                    if !conn.video_ack_required {
                        if let Some(message::Union::VideoFrame(vf)) = &value.union {
                            video_service::notify_video_frame_fetched(vf.display as usize, id, Some(instant.into()));
                        }
                    }
                    if let Err(err) = conn.stream.send(&value as &Message).await {
                        conn.on_close(&err.to_string(), false).await;
                        break;
                    }
                },
                Some((instant, value)) = rx.recv() => {
                    let latency = instant.elapsed().as_millis() as i64;
                    #[allow(unused_mut)]
                    let mut msg = value;

                    if latency > 1000 {
                        match &msg.union {
                            Some(message::Union::AudioFrame(_)) => {
                                // log::info!("audio frame latency {}", instant.elapsed().as_secs_f32());
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match &msg.union {
                        Some(message::Union::Misc(m)) => {
                            match &m.union {
                                Some(misc::Union::StopService(_)) => {
                                    conn.send_close_reason_no_retry("").await;
                                    conn.on_close("stop service", false).await;
                                    break;
                                }
                                _ => {},
                            }
                        }
                        Some(message::Union::PeerInfo(_pi)) => {
                            conn.refresh_video_display(None);
                            #[cfg(target_os = "macos")]
                            conn.retina.set_displays(&_pi.displays);
                        }
                        Some(message::Union::CursorPosition(pos)) => {
                            #[cfg(not(any(target_os = "android", target_os = "ios")))]
                            {
                                if conn.follow_remote_cursor {
                                    conn.handle_cursor_switch_display(pos.clone()).await;
                                }
                            }
                            #[cfg(target_os = "macos")]
                            if let Some(new_msg) = conn.retina.on_cursor_pos(&pos, conn.display_idx) {
                                msg = Arc::new(new_msg);
                            }
                        }
                        Some(message::Union::MultiClipboards(_multi_clipboards)) => {
                            #[cfg(not(target_os = "ios"))]
                            if let Some(msg_out) = crate::clipboard::get_msg_if_not_support_multi_clip(&conn.lr.version, &conn.lr.my_platform, _multi_clipboards) {
                                if let Err(err) = conn.stream.send(&msg_out).await {
                                    conn.on_close(&err.to_string(), false).await;
                                    break;
                                }
                                continue;
                            }
                        }
                        _ => {}
                    }

                    let msg: &Message = &msg;
                    if let Err(err) = conn.stream.send(msg).await {
                        conn.on_close(&err.to_string(), false).await;
                        break;
                    }
                },
                Some(data) = rx_from_authed.recv() => {
                    match data {
                        ipc::Data::Close => {
                            conn.send(crate::common::make_privacy_mode_msg(
                                back_notification::PrivacyModeState::PrvOnByOther,
                                String::new(),
                            )).await;
                            conn.on_close("privacy mode", false).await;
                            break;
                        }
                        _ => {}
                    }
                }
                _ = second_timer.tick() => {
                    #[cfg(windows)]
                    conn.portable_check();
                    raii::AuthedConnID::check_wake_lock_on_setting_changed();
                    if let Some((instant, minute)) = conn.auto_disconnect_timer.as_ref() {
                        if instant.elapsed().as_secs() > minute * 60 {
                            conn.send_close_reason_no_retry("Connection failed due to inactivity").await;
                            conn.on_close("auto disconnect", true).await;
                            break;
                        }
                    }
                    #[cfg(feature = "hwcodec")]
                    conn.update_supported_encoding();
                }
                _ = test_delay_timer.tick() => {
                    if last_recv_time.elapsed() >= SEC30 {
                        conn.on_close("Timeout", true).await;
                        break;
                    }
                    // The control end will jump out of the loop after receiving LoginResponse and will not reply to the TestDelay
                    if conn.last_test_delay.is_none() {
                        conn.last_test_delay = Some(Instant::now());
                        let mut msg_out = Message::new();
                        msg_out.set_test_delay(TestDelay{
                            last_delay: conn.network_delay,
                            target_bitrate: video_service::VIDEO_QOS.lock().unwrap().bitrate(),
                            ..Default::default()
                        });
                        conn.send(msg_out.into()).await;
                    }
                    if conn.is_authed_remote_conn() {
                        if let Some(last_test_delay) = conn.last_test_delay {
                            video_service::VIDEO_QOS.lock().unwrap().user_delay_response_elapsed(id, last_test_delay.elapsed().as_millis());
                        }
                    }
                }
            }
        }

        if let Some(video_privacy_conn_id) = privacy_mode::get_privacy_mode_conn_id() {
            if video_privacy_conn_id == id {
                let _ = Self::turn_off_privacy_to_msg(id, String::new());
            }
        }
        video_service::notify_video_frame_fetched_by_conn_id(id, None);

        if let Some(s) = conn.server.upgrade() {
            let mut s = s.write().unwrap();
            s.remove_connection(&conn.inner);
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            try_stop_record_cursor_pos();
        }
        conn.on_close("End", true).await;
        log::info!("#{} connection loop exited", id);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn handle_input(receiver: std_mpsc::Receiver<MessageInput>, tx: Sender) {
        let mut block_input_mode = false;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            rdev::set_mouse_extra_info(enigo::ENIGO_INPUT_EXTRA_VALUE);
            rdev::set_keyboard_extra_info(enigo::ENIGO_INPUT_EXTRA_VALUE);
        }
        #[cfg(target_os = "macos")]
        reset_input_ondisconn();
        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(v) => match v {
                    MessageInput::Mouse(mouse_input) => {
                        handle_mouse(
                            &mouse_input.msg,
                            mouse_input.conn_id,
                            mouse_input.username,
                            mouse_input.argb,
                            mouse_input.simulate,
                            mouse_input.show_cursor,
                        );
                    }
                    MessageInput::Key((mut msg, press)) => {
                        // Set the press state to false, use `down` only in `handle_key()`.
                        msg.press = false;
                        if press {
                            msg.down = true;
                        }
                        handle_key(&msg);
                        if press {
                            msg.down = false;
                            handle_key(&msg);
                        }
                    }
                    MessageInput::Pointer((msg, id)) => {
                        handle_pointer(&msg, id);
                    }
                    MessageInput::BlockOn => {
                        let (ok, msg) = crate::platform::block_input(true);
                        if ok {
                            block_input_mode = true;
                        } else {
                            Self::send_block_input_error(
                                &tx,
                                back_notification::BlockInputState::BlkOnFailed,
                                msg,
                            );
                        }
                    }
                    MessageInput::BlockOff => {
                        let (ok, msg) = crate::platform::block_input(false);
                        if ok {
                            block_input_mode = false;
                        } else {
                            Self::send_block_input_error(
                                &tx,
                                back_notification::BlockInputState::BlkOffFailed,
                                msg,
                            );
                        }
                    }
                },
                Err(err) => {
                    if block_input_mode {
                        let _ = crate::platform::block_input(true);
                    }
                    if std_mpsc::RecvTimeoutError::Disconnected == err {
                        break;
                    }
                }
            }
        }
        log::debug!("Input thread exited");
    }

    async fn send_permission(&mut self, permission: Permission, enabled: bool) {
        let mut misc = Misc::new();
        misc.set_permission_info(PermissionInfo {
            permission: permission.into(),
            enabled,
            ..Default::default()
        });
        let mut msg_out = Message::new();
        msg_out.set_misc(misc);
        self.send(msg_out).await;
    }

    async fn check_privacy_mode_on(&mut self) -> bool {
        if privacy_mode::is_in_privacy_mode() {
            self.send_login_error("Someone turns on privacy mode, exit")
                .await;
            false
        } else {
            true
        }
    }

    async fn check_whitelist(&mut self, addr: &SocketAddr) -> bool {
        let whitelist: Vec<String> = Config::get_option("whitelist")
            .split(",")
            .filter(|x| !x.is_empty())
            .map(|x| x.to_owned())
            .collect();
        if !whitelist.is_empty()
            && whitelist
                .iter()
                .filter(|x| x == &"0.0.0.0")
                .next()
                .is_none()
            && whitelist
                .iter()
                .filter(|x| IpCidr::from_str(x).map_or(false, |y| y.contains(addr.ip())))
                .next()
                .is_none()
        {
            self.send_login_error("Your ip is blocked by the peer")
                .await;
            hbb_common::throttled_log!(
                Duration::from_secs(30),
                warn,
                "Connection rejected: IpWhitelist"
            );
            return false;
        }
        true
    }

    async fn check_id_whitelist(&mut self) -> bool {
        let id_whitelist: Vec<String> = Config::get_option(keys::OPTION_ID_WHITELIST)
            .split(',')
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty())
            .collect();
        if id_whitelist.is_empty() {
            return true;
        }
        // Limit before matching, or a match returning early would never touch the counter and
        // leave enumeration unthrottled. Not cleared here: `my_id` is self-reported, so that
        // would let anyone holding one allowed id reset the budget between probes.
        self.decay_id_whitelist_failures();
        let (failure, res) = self.check_failure(FAILURE_IDX_ID_WHITELIST).await;
        if !res {
            return false;
        }
        if id_whitelist_allows(&id_whitelist, &self.lr.my_id) {
            return true;
        }
        self.update_failure(failure, false, FAILURE_IDX_ID_WHITELIST);
        self.send_login_error("Your ID is blocked by the peer")
            .await;
        hbb_common::throttled_log!(
            Duration::from_secs(30),
            warn,
            "Connection rejected: IdWhitelist"
        );
        false
    }

    // What `check_failure` consults: the source address, plus shared IPv6 prefixes.
    fn failure_keys(&self) -> Vec<String> {
        let mut keys = vec![self.ip.clone()];
        if let Some((p64, p56, p48)) = self.get_ipv6_prefixes() {
            keys.extend([p64, p56, p48]);
        }
        keys
    }

    // Only this connection's own keys, so it stays O(1) instead of scanning the map.
    fn decay_id_whitelist_failures(&self) {
        decay_stale_failures(
            &mut LOGIN_FAILURES[FAILURE_IDX_ID_WHITELIST].lock().unwrap(),
            &self.failure_keys(),
            (get_time() / 60_000) as i32,
            ID_WHITELIST_FAILURE_DECAY_MINUTES,
        );
    }

    // Not `update_failure(.., true, ..)`: it no-ops when the peer's own address has no entry,
    // normal on IPv6, leaving the shared prefixes that are what actually block it.
    fn clear_id_whitelist_failures(&self) {
        clear_failures(
            &mut LOGIN_FAILURES[FAILURE_IDX_ID_WHITELIST].lock().unwrap(),
            &self.failure_keys(),
        );
    }

    async fn on_open(&mut self, addr: SocketAddr) -> bool {
        log::debug!("#{} Connection opened from {}.", self.inner.id, addr);
        if !self.check_whitelist(&addr).await {
            return false;
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if crate::is_server() && Config::get_option("allow-only-conn-window-open") == "Y" {
            if !crate::check_process("", !crate::platform::is_root()) {
                self.send_login_error("The main window is not open").await;
                return false;
            }
        }
        self.ip = addr.ip().to_string();
        let mut msg_out = Message::new();
        msg_out.set_hash(self.hash.clone());
        self.send(msg_out).await;
        true
    }

    // Returns whether this connection should be kept alive.
    // `true` does not necessarily mean authorization succeeded (e.g. REQUIRE_2FA case).
    async fn send_logon_response_and_keep_alive(&mut self) -> bool {
        if self.authorized {
            return true;
        }
        if self.require_2fa.is_some() && !self.is_recent_session(true) {
            self.require_2fa.as_ref().map(|totp| {
                let bot = crate::auth_2fa::TelegramBot::get();
                let bot = match bot {
                    Ok(Some(bot)) => bot,
                    Err(err) => {
                        log::error!("Failed to get telegram bot: {}", err);
                        return;
                    }
                    _ => return,
                };
                let code = totp.generate_current();
                if let Ok(code) = code {
                    let text = format!(
                        "2FA code: {}\n\nA new connection has been established to your device with ID {}. The source IP address is {}.",
                        code,
                        Config::get_id(),
                        self.ip,
                    );
                    tokio::spawn(async move {
                        if let Err(err) =
                            crate::auth_2fa::send_2fa_code_to_telegram(&text, bot).await
                        {
                            log::error!("Failed to send 2fa code to telegram bot: {}", err);
                        }
                    });
                }
            });
            self.awaiting_2fa = true;
            self.send_login_error(REQUIRE_2FA).await;
            // Keep the connection alive so the client can continue with 2FA.
            return true;
        }
        self.awaiting_2fa = false;
        self.stream.set_max_packet_length(usize::MAX);
        self.authorized = true;
        self.unauthorized_id = None;
        // One-time means gone once it has let a peer in, not once that peer
        // leaves. This session's later logins come in on the password the
        // session remembers, so they are not affected.
        password::update_temporary_password();
        // Releases the budget `check_id_whitelist` charges against this address: only a peer
        // that got this far proved more than a self-reported id.
        self.clear_id_whitelist_failures();
        let auth_conn_type = AuthConnType::Remote;
        self.authed_conn_id = Some(self::raii::AuthedConnID::new(
            self.inner.id(),
            auth_conn_type,
            self.session_key(),
            self.tx_from_authed.clone(),
        ));
        self.session_last_recv_time = SESSIONS
            .lock()
            .unwrap()
            .get(&self.session_key())
            .map(|s| s.last_recv_time.clone());
        #[allow(unused_mut)]
        let mut username = crate::platform::get_active_username();
        let mut res = LoginResponse::new();
        let mut pi = PeerInfo {
            username: username.clone(),
            version: VERSION.to_owned(),
            ..Default::default()
        };

        #[cfg(not(target_os = "android"))]
        {
            pi.hostname = crate::whoami_hostname();
            pi.platform = hbb_common::whoami::platform().to_string();
        }
        #[cfg(target_os = "android")]
        {
            pi.hostname = DEVICE_NAME.lock().unwrap().clone();
            pi.platform = "Android".into();
        }
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
        let mut platform_additions = serde_json::Map::new();
        #[cfg(target_os = "linux")]
        {
            if crate::platform::current_is_wayland() {
                platform_additions.insert("is_wayland".into(), json!(true));
            }
        }
        #[cfg(target_os = "windows")]
        {
            platform_additions.insert(
                "is_installed".into(),
                json!(crate::platform::is_installed()),
            );
            if crate::platform::is_installed() {
                platform_additions.extend(virtual_display_manager::get_platform_additions());
            }
            platform_additions.insert(
                "supported_privacy_mode_impl".into(),
                json!(privacy_mode::get_supported_privacy_mode_impl()),
            );
        }
        #[cfg(target_os = "macos")]
        {
            platform_additions.insert(
                "supported_privacy_mode_impl".into(),
                json!(privacy_mode::get_supported_privacy_mode_impl()),
            );
        }

        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        if !platform_additions.is_empty() {
            pi.platform_additions = serde_json::to_string(&platform_additions).unwrap_or("".into());
        }

        #[cfg(target_os = "linux")]
        if self.is_remote() {
            let mut msg = "".to_string();
            #[cfg(feature = "drm")]
            if crate::platform::linux::is_wayland_session_available() {
                super::drm_capturer::settle_unknown_availability().await;
            }
            // Refuse only while nothing can capture a Wayland greeter: the DRM path can.
            if crate::platform::linux::is_login_screen_wayland() && !drm_can_serve_login_screen() {
                msg = LOGIN_SCREEN_WAYLAND.to_owned()
            } else {
                let dtype = crate::platform::linux::get_display_server();
                if dtype != crate::platform::linux::DISPLAY_SERVER_WAYLAND
                {
                    msg = format!(
                        "Unsupported display server type \"{}\", wayland expected",
                        dtype
                    );
                }
            }
            if !msg.is_empty() {
                res.set_error(msg);
                let mut msg_out = Message::new();
                msg_out.set_login_response(res);
                self.send(msg_out).await;
                return true;
            }
        }
        #[allow(unused_mut)]
        let mut sas_enabled = false;
        #[cfg(windows)]
        if crate::platform::is_root() {
            sas_enabled = true;
        }
        pi.username = username;
        pi.sas_enabled = sas_enabled;
        pi.features = Some(Features {
            privacy_mode: privacy_mode::is_privacy_mode_supported(),
            ..Default::default()
        })
        .into();

        let mut sub_service = false;
        #[allow(unused_mut)]
        let mut wait_session_id_confirm = false;
        #[cfg(windows)]
        self.handle_windows_specific_session(&mut pi, &mut wait_session_id_confirm);
        {
            let supported_encoding = scrap::codec::Encoder::supported_encoding();
            self.last_supported_encoding = Some(supported_encoding.clone());
            log::info!("peer info supported_encoding: {:?}", supported_encoding);
            pi.encoding = Some(supported_encoding).into();
            if let Some(msg_out) = super::display_service::is_inited_msg() {
                self.send(msg_out).await;
            }

            try_activate_screen();

            match super::display_service::update_get_sync_displays_on_login().await {
                Err(err) => {
                    res.set_error(format!("{}", err));
                }
                Ok((displays, primary_display_idx)) => {
                    // For compatibility with old versions, we need to send the displays to the peer.
                    // But the displays may be updated later, before creating the video capturer.
                    #[cfg(target_os = "macos")]
                    {
                        self.retina.set_displays(&displays);
                    }
                    // A separate primary lookup here could race with display hot-plug.
                    self.display_idx = primary_display_idx;
                    pi.displays = displays;
                    pi.current_display = self.display_idx as _;
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    {
                        pi.resolutions = Some(SupportedResolutions {
                            resolutions: pi
                                .displays
                                .get(self.display_idx)
                                .map(|d| crate::platform::resolutions(&d.name))
                                .unwrap_or(vec![]),
                            ..Default::default()
                        })
                        .into();
                    }
                    res.set_peer_info(pi);
                    sub_service = true;

                    #[cfg(target_os = "linux")]
                    {
                        // use rdp_input when uinput is not available in wayland. Ex: flatpak
                        if input_service::wayland_use_rdp_input() {
                            let _ = setup_rdp_input().await;
                        }
                    }
                }
            }
            self.on_remote_authorized();
        }
        let mut msg_out = Message::new();
        msg_out.set_login_response(res);
        self.send(msg_out).await;
        self.update_scoped_login_options().await;
        if sub_service && !wait_session_id_confirm {
            self.try_sub_monitor_services();
        }
        true
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn try_sub_monitor_services(&mut self) {
        let is_remote = self.is_remote();
        if is_remote && !self.services_subed {
            self.services_subed = true;
            if let Some(s) = self.server.upgrade() {
                let mut noperms = Vec::new();
                if !self.peer_keyboard_enabled() && !self.show_remote_cursor {
                    noperms.push(NAME_CURSOR);
                }
                if !self.show_remote_cursor {
                    noperms.push(NAME_POS);
                }
                if !self.follow_remote_window {
                    noperms.push(NAME_WINDOW_FOCUS);
                }
                if !self.can_sub_clipboard_service() {
                    noperms.push(super::clipboard_service::NAME);
                }
                if !self.audio_enabled() {
                    noperms.push(super::audio_service::NAME);
                }
                let mut s = s.write().unwrap();
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                let _h = try_start_record_cursor_pos();
                self.auto_disconnect_timer = Self::get_auto_disconenct_timer();
                s.try_add_monitor_service(self.display_idx);
                s.add_monitor_connection(self.inner.clone(), &noperms, self.display_idx);
            }
        }
    }

    #[cfg(windows)]
    fn handle_windows_specific_session(
        &mut self,
        pi: &mut PeerInfo,
        wait_session_id_confirm: &mut bool,
    ) {
        let sessions = crate::platform::get_available_sessions(true);
        if let Some(current_sid) = crate::platform::get_current_process_session_id() {
            if crate::platform::is_installed()
                && crate::platform::is_share_rdp()
                && raii::AuthedConnID::remote_conn_count() == 1
                && sessions.len() > 1
                && sessions.iter().any(|e| e.sid == current_sid)
                && get_version_number(&self.lr.version) >= get_version_number("1.2.4")
            {
                pi.windows_sessions = Some(WindowsSessions {
                    sessions,
                    current_sid,
                    ..Default::default()
                })
                .into();
                *wait_session_id_confirm = true;
            }
        }
    }

    fn on_remote_authorized(&self) {
        self.update_codec_on_login();
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if config::option2bool(
            "allow-remove-wallpaper",
            &Config::get_option("allow-remove-wallpaper"),
        ) {
            // multi connections set once
            let mut wallpaper = WALLPAPER_REMOVER.lock().unwrap();
            if wallpaper.is_none() {
                match crate::platform::WallPaperRemover::new() {
                    Ok(remover) => {
                        *wallpaper = Some(remover);
                    }
                    Err(e) => {
                        log::info!("create wallpaper remover failed: {:?}", e);
                    }
                }
            }
        }
    }

    fn peer_keyboard_enabled(&self) -> bool {
        self.keyboard && !self.disable_keyboard
    }

    fn clipboard_enabled(&self) -> bool {
        self.clipboard && !self.disable_clipboard
    }

    #[inline]
    fn can_sub_clipboard_service(&self) -> bool {
        self.clipboard_enabled()
            && self.peer_keyboard_enabled()
            && crate::get_builtin_option(keys::OPTION_ONE_WAY_CLIPBOARD_REDIRECTION) != "Y"
    }

    fn audio_enabled(&self) -> bool {
        self.audio && !self.disable_audio
    }

    fn try_start_cm(&mut self, peer_id: String, name: String, authorized: bool) {
        self.send_to_cm(ipc::Data::Login {
            id: self.inner.id(),
            peer_id,
            name,
            avatar: self.lr.avatar.clone(),
            authorized,
            keyboard: self.keyboard,
            clipboard: self.clipboard,
            audio: self.audio,
            restart: self.restart,
            block_input: self.block_input,
            privacy_mode: self.privacy_mode,
        });
    }

    #[inline]
    fn send_to_cm(&mut self, data: ipc::Data) {
        self.tx_to_cm.send(data).ok();
    }

    async fn send_login_error<T: std::string::ToString>(&mut self, err: T) {
        let mut msg_out = Message::new();
        let mut res = LoginResponse::new();
        res.set_error(err.to_string());
        if err.to_string() == REQUIRE_2FA {
            res.enable_trusted_devices = Self::enable_trusted_devices();
        }
        msg_out.set_login_response(res);
        self.send(msg_out).await;
    }

    #[inline]
    pub fn send_block_input_error(
        s: &Sender,
        state: back_notification::BlockInputState,
        details: String,
    ) {
        let mut misc = Misc::new();
        let mut back_notification = BackNotification {
            details,
            ..Default::default()
        };
        back_notification.set_block_input_state(state);
        misc.set_back_notification(back_notification);
        let mut msg_out = Message::new();
        msg_out.set_misc(misc);
        s.send((Instant::now(), Arc::new(msg_out))).ok();
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn input_mouse(
        &self,
        msg: MouseEvent,
        conn_id: i32,
        username: String,
        argb: u32,
        simulate: bool,
        show_cursor: bool,
    ) {
        self.tx_input
            .send(MessageInput::Mouse(InputMouse {
                msg,
                conn_id,
                username,
                argb,
                simulate,
                show_cursor,
            }))
            .ok();
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn input_pointer(&self, msg: PointerDeviceEvent, conn_id: i32) {
        self.tx_input
            .send(MessageInput::Pointer((msg, conn_id)))
            .ok();
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn input_key(&self, msg: KeyEvent, press: bool) {
        // to-do: if is the legacy mode, and the key is function key "LockScreen".
        // Switch to the primary display.
        self.tx_input.send(MessageInput::Key((msg, press))).ok();
    }

    // Auto Login: the password and Return the client would otherwise type itself, typed only if a
    // check right before finds the logon or lock screen.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    async fn type_auto_login_password(&self, password: String) {
        // A peer can send this at any rate, hence the throttled logs.
        let at_logon_screen = tokio::task::spawn_blocking(crate::platform::is_logon_screen)
            .await
            .unwrap_or_else(|e| {
                hbb_common::throttled_log!(
                    Duration::from_secs(60),
                    error,
                    "Failed to check the logon screen: {}",
                    e
                );
                false
            });
        if !at_logon_screen {
            hbb_common::throttled_log!(
                Duration::from_secs(60),
                info,
                "Auto login dropped, not at the logon or lock screen"
            );
            return;
        }
        let mut key_event = KeyEvent::new();
        key_event.mode = KeyboardMode::Legacy.into();
        key_event.press = true;
        key_event.set_seq(password);
        self.input_key(key_event.clone(), false);
        key_event.set_control_key(ControlKey::Return);
        CLICK_TIME.store(get_time(), Ordering::SeqCst);
        MOUSE_MOVE_TIME.store(get_time(), Ordering::SeqCst);
        self.input_key(key_event, true);
    }

    fn verify_h1(&self, h1: &[u8]) -> bool {
        let mut hasher2 = Sha256::new();
        hasher2.update(h1);
        hasher2.update(self.hash.challenge.as_bytes());
        // A normal `==` on slices may short-circuit on the first mismatch, which can leak how many leading
        // bytes matched via timing. In typical remote scenarios this is difficult to exploit due to network
        // jitter, changing challenges, and login attempt throttling, but a constant-time comparison here is
        // low-cost defensive programming.
        constant_time_eq(&hasher2.finalize()[..], &self.lr.password[..])
    }

    fn validate_password_plain(&self, password: &str) -> bool {
        if password.is_empty() {
            return false;
        }

        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        hasher.update(self.hash.salt.as_bytes());
        let h1_plain = hasher.finalize();
        self.verify_h1(&h1_plain[..])
    }

    fn validate_password_storage(&self, storage: &str) -> bool {
        if storage.is_empty() {
            return false;
        }

        // Use strict decode success to detect hashed storage.
        // If decode fails, treat as legacy plaintext storage for compatibility.
        if let Some(h1) = decode_permanent_password_h1_from_storage(storage) {
            return self.verify_h1(&h1[..]);
        }

        // Legacy plaintext storage path.
        self.validate_password_plain(storage)
    }

    fn validate_preset_password_storage(&self, storage: &str, salt: &str) -> bool {
        if salt.is_empty() {
            return self.validate_password_plain(storage);
        }
        let Some(h1) = decode_preset_password_h1_from_storage(storage) else {
            return false;
        };
        self.verify_h1(&h1[..])
    }

    // This is coarse brute-force protection for the current temporary password value.
    // We only care whether the active temporary password itself was presented correctly,
    // not whether later authorization steps succeed. A successful temporary-password
    // match clears this state immediately, and the counter also resets whenever the
    // temporary password changes or is rotated.
    fn check_update_temporary_password(&self, temporary_password_success: bool) {
        const MAX_CONSECUTIVE_FAILURES: i32 = 10;
        #[derive(Default)]
        struct State {
            password: String,
            failures: i32,
        }
        lazy_static::lazy_static! {
            static ref TEMPORARY_PASSWORD_FAILURES: Mutex<State> =
                Mutex::new(State::default());
        }

        if !password::temporary_enabled() {
            return;
        }

        let mut state = TEMPORARY_PASSWORD_FAILURES.lock().unwrap();
        let current_password = password::temporary_password();
        if current_password.is_empty() {
            return;
        }
        if state.password != current_password {
            state.password = current_password;
            state.failures = 0;
        }

        if temporary_password_success {
            state.failures = 0;
            return;
        }
        state.failures += 1;

        if state.failures < MAX_CONSECUTIVE_FAILURES {
            return;
        }

        password::update_temporary_password();
        let new_password = password::temporary_password();
        log::warn!(
            "Temporary password rotated after too many consecutive wrong attempts: failures={}, ip={}",
            state.failures,
            self.ip,
        );
        state.password = new_password;
        state.failures = 0;
    }

    fn validate_password(&mut self, allow_permanent_password: bool) -> bool {
        if password::temporary_enabled() {
            let password = password::temporary_password();
            if self.validate_password_plain(&password) {
                raii::AuthedConnID::update_or_insert_session(
                    self.session_key(),
                    Some(password),
                    Some(false),
                );
                self.check_update_temporary_password(true);
                return true;
            }
        }
        if password::permanent_enabled() || allow_permanent_password {
            let print_fallback = || {
                if allow_permanent_password && !password::permanent_enabled() {
                    log::info!("Permanent password accepted via logon-screen fallback");
                }
            };
            // Strictly check storage usability before auth so malformed encrypted/hash storage
            // cannot fall back to being accepted as legacy plaintext.
            let (local_storage, local_salt) =
                Config::get_local_permanent_password_storage_and_salt();
            if !local_storage.is_empty() {
                if local_permanent_password_storage_is_usable_for_auth(&local_storage, &local_salt)
                    && self.validate_password_storage(&local_storage)
                {
                    print_fallback();
                    return true;
                }
            } else {
                let (hard, salt) = Config::get_preset_password_storage_and_salt();
                if preset_permanent_password_storage_is_usable_for_auth(&hard, &salt)
                    && self.validate_preset_password_storage(&hard, &salt)
                {
                    print_fallback();
                    return true;
                }
            }
        }
        false
    }

    fn is_recent_session(&mut self, tfa: bool) -> bool {
        SESSIONS
            .lock()
            .unwrap()
            .retain(|_, s| s.last_recv_time.lock().unwrap().elapsed() < SESSION_TIMEOUT);
        let session = SESSIONS
            .lock()
            .unwrap()
            .get(&self.session_key())
            .map(|s| s.to_owned());
        // last_recv_time is a mutex variable shared with connection, can be updated lively.
        if let Some(session) = session {
            if !self.lr.password.is_empty()
                && (tfa && session.tfa
                    || !tfa && self.validate_password_plain(&session.random_password))
            {
                log::info!("is recent session");
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn is_permission_enabled_locally(enable_prefix_option: &str) -> bool {
        #[cfg(feature = "flutter")]
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let access_mode = Config::get_option("access-mode");
            if access_mode == "full" {
                return true;
            } else if access_mode == "view" {
                return false;
            }
        }
        config::option2bool(
            enable_prefix_option,
            &Config::get_option(enable_prefix_option),
        )
    }

    fn permission(
        enable_prefix_option: &str,
        control_permissions: &Option<ControlPermissions>,
    ) -> bool {
        use hbb_common::rendezvous_proto::control_permissions::Permission;
        if let Some(control_permissions) = control_permissions {
            let permission = match enable_prefix_option {
                keys::OPTION_ENABLE_KEYBOARD => Some(Permission::keyboard),
                keys::OPTION_ENABLE_CLIPBOARD => Some(Permission::clipboard),
                keys::OPTION_ENABLE_AUDIO => Some(Permission::audio),
                keys::OPTION_ENABLE_REMOTE_RESTART => Some(Permission::restart),
                keys::OPTION_ENABLE_BLOCK_INPUT => Some(Permission::block_input),
                keys::OPTION_ENABLE_PRIVACY_MODE => Some(Permission::privacy_mode),
                _ => None,
            };
            if let Some(permission) = permission {
                if let Some(enabled) =
                    crate::get_control_permission(control_permissions.permissions, permission)
                {
                    return enabled;
                }
            }
        }
        Self::is_permission_enabled_locally(enable_prefix_option)
    }

    fn update_codec_on_login(&self) {
        use scrap::codec::{Encoder, EncodingUpdate::*};
        if let Some(o) = self.lr.clone().option.as_ref() {
            if let Some(q) = o.supported_decoding.clone().take() {
                Encoder::update(Update(self.inner.id(), q));
            } else {
                Encoder::update(NewOnlyVP9(self.inner.id()));
            }
        } else {
            Encoder::update(NewOnlyVP9(self.inner.id()));
        }
    }

    #[inline]
    fn enable_trusted_devices() -> bool {
        config::option2bool(
            keys::OPTION_ENABLE_TRUSTED_DEVICES,
            &Config::get_option(keys::OPTION_ENABLE_TRUSTED_DEVICES),
        )
    }

    fn supports_login_request(lr: &LoginRequest) -> bool {
        lr.union.is_none()
    }

    // Approval and whitelist decisions must stay bound to the same controller identity and
    // session scope across authentication retries.
    fn login_scope_digest(lr: &LoginRequest) -> [u8; 32] {
        let mut hasher = Sha256::new();
        // Length-prefixed so adjacent fields cannot alias.
        let mut push = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        push(lr.my_id.as_bytes());
        push(b"remote");
        hasher.finalize().into()
    }

    fn login_scope_kind(lr: &LoginRequest) -> &'static str {
        if Self::supports_login_request(lr) {
            "remote"
        } else {
            "unsupported"
        }
    }

    async fn check_login_scope(&mut self, lr: &LoginRequest) -> bool {
        let requested = Self::login_scope_digest(lr);
        match self.login_scope {
            Some(initial) if initial != requested => {
                // self.lr still holds the first accepted request, whose scope is the latched one.
                log::warn!(
                    "Rejected login scope change: conn_id={}, initial={}, requested={}",
                    self.inner.id(),
                    Self::login_scope_kind(&self.lr),
                    Self::login_scope_kind(lr),
                );
                self.send_login_error("Connection not allowed").await;
                false
            }
            Some(_) => true,
            None => {
                self.login_scope = Some(requested);
                true
            }
        }
    }

    async fn handle_login_request_without_validation(&mut self, lr: &LoginRequest) {
        self.lr = lr.clone();
        self.peer_argb = crate::str2color(&format!("{}{}", &lr.my_id, &lr.my_platform), 0xff);
        if let Some(o) = lr.option.as_ref() {
            self.options_in_login = Some(o.clone());
        }
        if self.require_2fa.is_some() && !lr.hwid.is_empty() && Self::enable_trusted_devices() {
            let devices = Config::get_trusted_devices();
            if let Some(device) = devices.iter().find(|d| d.hwid == lr.hwid) {
                if !device.outdate()
                    && device.id == lr.my_id
                    && device.name == lr.my_name
                    && device.platform == lr.my_platform
                {
                    log::info!("2FA bypassed by trusted devices");

                    self.require_2fa = None;
                }
            }
        }
        self.video_ack_required = lr.video_ack_required;
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn try_start_cm_ipc(&mut self) {
        if let Some(p) = self.start_cm_ipc_para.take() {
            tokio::spawn(async move {
                #[cfg(windows)]
                let tx_from_cm_clone = p.tx_from_cm.clone();
                if let Err(err) = start_ipc(p.rx_to_cm, p.tx_from_cm).await {
                    log::warn!("ipc to connection manager exit: {}", err);
                    // https://github.com/rustdesk/rustdesk-server-pro/discussions/382#discussioncomment-10525725, cm may start failed
                    #[cfg(windows)]
                    if !crate::platform::is_prelogin()
                        && !err.to_string().contains(crate::platform::EXPLORER_EXE)
                    {
                        allow_err!(tx_from_cm_clone.send(Data::CmErr(err.to_string())));
                    }
                }
            });
            #[cfg(all(windows, feature = "flutter"))]
            std::thread::spawn(move || {
                if crate::is_server() && !crate::check_process("--tray", false) {
                    crate::platform::run_as_user(vec!["--tray"]).ok();
                }
            });
        }
    }

    async fn on_message(&mut self, msg: Message) -> bool {
        // Legacy wire tags remain decodable, but never grant authorization or reverse roles.
        if base::session_protocol::is_legacy_role_switch_message(&msg) {
            self.send_login_error("Runtime role switching is not supported")
                .await;
            return false;
        }
        if let Some(message::Union::Misc(misc)) = &msg.union {
            // Move the CloseReason forward, as this message needs to be received when unauthorized, especially for kcp.
            if let Some(misc::Union::CloseReason(s)) = &misc.union {
                log::info!("receive close reason: {}", s);
                self.on_close("Peer close", true).await;
                raii::AuthedConnID::check_remove_session(self.inner.id(), self.session_key());
                return false;
            }
        }
        if self.authorized {
            if matches!(msg.union.as_ref(), Some(message::Union::LoginRequest(_))) {
                return true;
            }
            if let Some(message) = self.authorized_scope_violation(&msg) {
                return self.handle_authorized_scope_violation(message).await;
            }
        }
        // After handling CloseReason messages, proceed to process other message types
        if let Some(message::Union::LoginRequest(lr)) = msg.union {
            if !Self::supports_login_request(&lr) {
                self.send_login_error("Only remote desktop sessions are supported")
                    .await;
                return false;
            }
            if !self.check_login_scope(&lr).await {
                return false;
            }
            self.awaiting_2fa = false;
            self.handle_login_request_without_validation(&lr).await;
            if self.authorized {
                return true;
            }
            if !self.check_id_whitelist().await {
                return false;
            }
            if !self.check_privacy_mode_on().await {
                return false;
            }
            self.stream.set_send_timeout(SEND_TIMEOUT_VIDEO);

            if !crate::common::is_direct_ip_access(&lr.username) && lr.username != Config::get_id()
            {
                self.send_login_error(LOGIN_MSG_OFFLINE)
                    .await;
                return false;
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            self.try_start_cm_ipc();

            // https://github.com/rustdesk/rustdesk-server-pro/discussions/646
            // `is_logon` is used to check login with `OPTION_ALLOW_LOGON_SCREEN_PASSWORD` == "Y".
            // `is_logon_ui()` is a fallback for logon UI detection on Windows.
            #[cfg(target_os = "windows")]
            let is_logon = || {
                crate::platform::is_prelogin() || crate::platform::is_locked() || {
                    match crate::platform::is_logon_ui() {
                        Ok(result) => result,
                        Err(e) => {
                            log::error!("Failed to detect logon UI: {:?}", e);
                            false
                        }
                    }
                }
            };
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let is_logon = || crate::platform::is_prelogin() || crate::platform::is_locked();
            #[cfg(any(target_os = "android", target_os = "ios"))]
            let is_logon = || crate::platform::is_prelogin();

            let allow_logon_screen_password =
                crate::get_builtin_option(keys::OPTION_ALLOW_LOGON_SCREEN_PASSWORD) == "Y"
                    && is_logon();

            if (password::approve_mode() == ApproveMode::Click && !allow_logon_screen_password)
                || password::approve_mode() == ApproveMode::Both && !password::has_valid_password()
            {
                self.try_start_cm(lr.my_id, lr.my_name, false);
                if hbb_common::get_version_number(&lr.version)
                    >= hbb_common::get_version_number("1.2.0")
                {
                    self.send_login_error(LOGIN_MSG_NO_PASSWORD_ACCESS)
                        .await;
                }
                return true;
            } else if self.is_recent_session(false) {
                if !self.send_logon_response_and_keep_alive().await {
                    return false;
                }
                self.try_start_cm(lr.my_id.clone(), lr.my_name.clone(), self.authorized);
            } else if lr.password.is_empty() {
                self.try_start_cm(lr.my_id, lr.my_name, false);
            } else {
                let (failure, res) = self.check_failure(0).await;
                if !res {
                    return true;
                }
                if !self.validate_password(allow_logon_screen_password) {
                    self.update_failure(failure, false, 0);
                    self.check_update_temporary_password(false);
                    self.send_login_error(LOGIN_MSG_PASSWORD_WRONG)
                        .await;
                    self.try_start_cm(lr.my_id, lr.my_name, false);
                } else {
                    self.update_failure(failure, true, 0);
                    if !self.send_logon_response_and_keep_alive().await {
                        return false;
                    }
                    self.try_start_cm(lr.my_id, lr.my_name, self.authorized);
                }
            }
        } else if let Some(message::Union::Auth2fa(tfa)) = msg.union {
            // A 2FA response may arrive after click authorization has completed.
            // Ignore it unless this connection is still waiting for the response.
            if !self.awaiting_2fa {
                return true;
            }
            let (failure, res) = self.check_failure(1).await;
            if !res {
                return true;
            }
            if let Some(totp) = self.require_2fa.as_ref() {
                if let Ok(res) = totp.check_current(&tfa.code) {
                    if res {
                        self.update_failure(failure, true, 1);
                        self.require_2fa.take();

                        raii::AuthedConnID::set_session_2fa(self.session_key());
                        if !self.send_logon_response_and_keep_alive().await {
                            return false;
                        }
                        self.try_start_cm(
                            self.lr.my_id.to_owned(),
                            self.lr.my_name.to_owned(),
                            self.authorized,
                        );
                        if !tfa.hwid.is_empty() && Self::enable_trusted_devices() {
                            Config::add_trusted_device(TrustedDevice {
                                hwid: tfa.hwid,
                                time: hbb_common::get_time(),
                                id: self.lr.my_id.clone(),
                                name: self.lr.my_name.clone(),
                                platform: self.lr.my_platform.clone(),
                            });
                        }
                    } else {
                        self.update_failure(failure, false, 1);
                        self.send_login_error(LOGIN_MSG_2FA_WRONG)
                            .await;
                    }
                }
            }
        } else if let Some(message::Union::TestDelay(t)) = msg.union {
            if t.from_client {
                let mut msg_out = Message::new();
                msg_out.set_test_delay(t);
                self.inner.send(msg_out.into());
            } else {
                if let Some(tm) = self.last_test_delay {
                    self.last_test_delay = None;
                    let new_delay = tm.elapsed().as_millis() as u32;
                    video_service::VIDEO_QOS
                        .lock()
                        .unwrap()
                        .user_network_delay(self.inner.id(), new_delay);
                    self.network_delay = new_delay;
                }
            }
        } else if self.authorized {
            match msg.union {
                #[allow(unused_mut)]
                Some(message::Union::MouseEvent(mut me)) => {
                    #[cfg(any(target_os = "android", target_os = "ios"))]
                    if let Err(e) = call_main_service_pointer_input("mouse", me.mask, me.x, me.y) {
                        log::debug!("call_main_service_pointer_input fail:{}", e);
                    }
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    if self.peer_keyboard_enabled() {
                        if is_left_up(&me) {
                            CLICK_TIME.store(get_time(), Ordering::SeqCst);
                        } else {
                            MOUSE_MOVE_TIME.store(get_time(), Ordering::SeqCst);
                        }
                        #[cfg(target_os = "macos")]
                        self.retina.on_mouse_event(&mut me, self.display_idx);
                        self.input_mouse(
                            me,
                            self.inner.id(),
                            self.lr.my_name.clone(),
                            self.peer_argb,
                            true,
                            self.show_my_cursor,
                        );
                    } else if self.show_my_cursor {
                        #[cfg(target_os = "macos")]
                        self.retina.on_mouse_event(&mut me, self.display_idx);
                        self.input_mouse(
                            me,
                            self.inner.id(),
                            self.lr.my_name.clone(),
                            self.peer_argb,
                            false,
                            true,
                        );
                    }
                    self.update_auto_disconnect_timer();
                }
                Some(message::Union::PointerDeviceEvent(pde)) => {
                    #[cfg(any(target_os = "android", target_os = "ios"))]
                    if let Err(e) = match pde.union {
                        Some(pointer_device_event::Union::TouchEvent(touch)) => match touch.union {
                            Some(touch_event::Union::PanStart(pan_start)) => {
                                call_main_service_pointer_input(
                                    "touch",
                                    4,
                                    pan_start.x,
                                    pan_start.y,
                                )
                            }
                            Some(touch_event::Union::PanUpdate(pan_update)) => {
                                call_main_service_pointer_input(
                                    "touch",
                                    5,
                                    pan_update.x,
                                    pan_update.y,
                                )
                            }
                            Some(touch_event::Union::PanEnd(pan_end)) => {
                                call_main_service_pointer_input("touch", 6, pan_end.x, pan_end.y)
                            }
                            _ => Ok(()),
                        },
                        _ => Ok(()),
                    } {
                        log::debug!("call_main_service_pointer_input fail:{}", e);
                    }
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    if self.peer_keyboard_enabled() {
                        MOUSE_MOVE_TIME.store(get_time(), Ordering::SeqCst);
                        self.input_pointer(pde, self.inner.id());
                    }
                    self.update_auto_disconnect_timer();
                }
                #[cfg(any(target_os = "ios"))]
                Some(message::Union::KeyEvent(..)) => {}
                #[cfg(any(target_os = "android"))]
                Some(message::Union::KeyEvent(mut me)) => {
                    let key = match me.mode.enum_value() {
                        Ok(KeyboardMode::Map) => {
                            Some(crate::keyboard::keycode_to_rdev_key(me.chr()))
                        }
                        Ok(KeyboardMode::Translate) => {
                            if let Some(key_event::Union::Chr(code)) = me.union {
                                Some(crate::keyboard::keycode_to_rdev_key(code & 0x0000FFFF))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                    .filter(crate::keyboard::is_modifier);

                    let is_press =
                        (me.press || me.down) && !(crate::is_modifier(&me) || key.is_some());

                    if let Some(key) = key {
                        if is_press {
                            self.pressed_modifiers.insert(key);
                        } else {
                            self.pressed_modifiers.remove(&key);
                        }
                    }

                    let mut modifiers = vec![];

                    for key in self.pressed_modifiers.iter() {
                        if let Some(control_key) = map_key_to_control_key(key) {
                            modifiers.push(EnumOrUnknown::new(control_key));
                        }
                    }

                    me.modifiers = modifiers;

                    let encode_result = me.write_to_bytes();

                    match encode_result {
                        Ok(data) => {
                            let result = call_main_service_key_event(&data);
                            if let Err(e) = result {
                                log::debug!("call_main_service_key_event fail: {}", e);
                            }
                        }
                        Err(e) => {
                            log::debug!("encode key event fail: {}", e);
                        }
                    }
                }
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                Some(message::Union::KeyEvent(me)) => {
                    if self.peer_keyboard_enabled() {
                        if is_enter(&me) {
                            CLICK_TIME.store(get_time(), Ordering::SeqCst);
                        }
                        // https://github.com/rustdesk/rustdesk/issues/8633
                        MOUSE_MOVE_TIME.store(get_time(), Ordering::SeqCst);

                        let key = match me.mode.enum_value() {
                            Ok(KeyboardMode::Map) => {
                                Some(crate::keyboard::keycode_to_rdev_key(me.chr()))
                            }
                            Ok(KeyboardMode::Translate) => {
                                if let Some(key_event::Union::Chr(code)) = me.union {
                                    Some(crate::keyboard::keycode_to_rdev_key(code & 0x0000FFFF))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                        .filter(crate::keyboard::is_modifier);

                        // handle all down as press
                        // fix unexpected repeating key on remote linux, seems also fix abnormal alt/shift, which
                        // make sure all key are released
                        // https://github.com/rustdesk/rustdesk/issues/6793
                        let is_press = if cfg!(target_os = "linux") {
                            (me.press || me.down) && !(crate::is_modifier(&me) || key.is_some())
                        } else {
                            me.press
                        };

                        if let Some(key) = key {
                            if is_press {
                                self.pressed_modifiers.insert(key);
                            } else {
                                self.pressed_modifiers.remove(&key);
                            }
                        }

                        if is_press {
                            match me.union {
                                Some(key_event::Union::Unicode(_))
                                | Some(key_event::Union::Seq(_)) => {
                                    self.input_key(me, false);
                                }
                                _ => {
                                    self.input_key(me, true);
                                }
                            }
                        } else {
                            self.input_key(me, false);
                        }
                    }
                    self.update_auto_disconnect_timer();
                }
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                Some(message::Union::OsLogin(login)) => {
                    if self.peer_keyboard_enabled() {
                        self.type_auto_login_password(login.password).await;
                    }
                    self.update_auto_disconnect_timer();
                }
                Some(message::Union::Clipboard(cb)) => {
                    if self.should_handle_text_clipboard_message()
                        && self.clipboard_enabled()
                        && cb.format.enum_value() == Ok(ClipboardFormat::Text)
                    {
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        update_clipboard(vec![cb], ClipboardSide::Host);
                        // ios as the controlled side is actually not supported for now.
                        // The following code is only used to preserve the logic of handling text clipboard on mobile.
                        #[cfg(target_os = "ios")]
                        {
                            let content = if cb.compress {
                                hbb_common::compress::decompress(&cb.content)
                            } else {
                                cb.content.into()
                            };
                            if let Ok(content) = String::from_utf8(content) {
                                let data =
                                    HashMap::from([("name", "clipboard"), ("content", &content)]);
                                if let Ok(data) = serde_json::to_string(&data) {
                                    let _ = crate::flutter::push_global_event(
                                        crate::flutter::APP_TYPE_MAIN,
                                        data,
                                    );
                                }
                            }
                        }
                        #[cfg(target_os = "android")]
                        crate::clipboard::handle_msg_clipboard(cb);
                    }
                }
                Some(message::Union::MultiClipboards(mut _mcb)) => {
                    _mcb.clipboards
                        .retain(|cb| cb.format.enum_value() == Ok(ClipboardFormat::Text));
                    if self.should_handle_text_clipboard_message() && self.clipboard_enabled() {
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        update_clipboard(_mcb.clipboards, ClipboardSide::Host);
                        #[cfg(target_os = "android")]
                        crate::clipboard::handle_msg_multi_clipboards(_mcb);
                    }
                }
                Some(message::Union::Misc(misc)) => match misc.union {
                    Some(misc::Union::SwitchDisplay(s)) => {
                        self.handle_switch_display(s).await;
                    }
                    Some(misc::Union::CaptureDisplays(displays)) => {
                        let add = displays.add.iter().map(|d| *d as usize).collect::<Vec<_>>();
                        let sub = displays.sub.iter().map(|d| *d as usize).collect::<Vec<_>>();
                        let set = displays.set.iter().map(|d| *d as usize).collect::<Vec<_>>();
                        self.capture_displays(&add, &sub, &set).await;
                    }
                    #[cfg(windows)]
                    Some(misc::Union::ToggleVirtualDisplay(t)) => {
                        self.toggle_virtual_display(t).await;
                    }
                    Some(misc::Union::TogglePrivacyMode(t)) => {
                        self.toggle_privacy_mode(t).await;
                    }
                    Some(misc::Union::Option(o)) => {
                        if self.authed_conn_type() == Some(AuthConnType::Remote) {
                            self.update_options(&o).await;
                        }
                    }
                    Some(misc::Union::RefreshVideo(r)) => {
                        if self.should_handle_render_broadcast_message() {
                            if r {
                                // Refresh all videos.
                                // Compatibility with old versions and sciter(remote).
                                self.refresh_video_display(None);
                            }
                            self.update_auto_disconnect_timer();
                        }
                    }
                    Some(misc::Union::RefreshVideoDisplay(display)) => {
                        if self.should_handle_render_broadcast_message() {
                            self.refresh_video_display(Some(display as usize));
                            self.update_auto_disconnect_timer();
                        }
                    }
                    Some(misc::Union::VideoReceived(_)) => {
                        video_service::notify_video_frame_fetched_by_conn_id(
                            self.inner.id,
                            Some(Instant::now().into()),
                        );
                    }
                    Some(misc::Union::RestartRemoteDevice(_)) => {
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        if self.restart {
                            // force_reboot, not work on linux vm and macos 14
                            #[cfg(any(target_os = "linux", target_os = "windows"))]
                            match system_shutdown::force_reboot() {
                                Ok(_) => log::info!("Restart by the peer"),
                                Err(e) => log::error!("Failed to restart: {}", e),
                            }
                            #[cfg(any(target_os = "linux", target_os = "macos"))]
                            match system_shutdown::reboot() {
                                Ok(_) => log::info!("Restart by the peer"),
                                Err(e) => log::error!("Failed to restart: {}", e),
                            }
                        }
                    }
                    #[cfg(windows)]
                    Some(misc::Union::ElevationRequest(r)) => match r.union {
                        Some(elevation_request::Union::Direct(_)) => {
                            self.handle_elevation_request(portable_client::StartPara::Direct)
                                .await;
                        }
                        Some(elevation_request::Union::Logon(r)) => {
                            self.handle_elevation_request(portable_client::StartPara::Logon(
                                r.username, r.password,
                            ))
                            .await;
                        }
                        _ => {}
                    },
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    Some(misc::Union::ChangeResolution(r)) => {
                        self.change_resolution(None, &r);
                    }
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    Some(misc::Union::ChangeDisplayResolution(dr)) => {
                        self.change_resolution(Some(dr.display as _), &dr.resolution);
                    }
                    Some(misc::Union::AutoAdjustFps(fps)) => video_service::VIDEO_QOS
                        .lock()
                        .unwrap()
                        .user_auto_adjust_fps(self.inner.id(), fps),
                    #[cfg(windows)]
                    Some(misc::Union::SelectedSid(sid)) => {
                        if let Some(current_process_sid) =
                            crate::platform::get_current_process_session_id()
                        {
                            let sessions = crate::platform::get_available_sessions(false);
                            if crate::platform::is_installed()
                                && crate::platform::is_share_rdp()
                                && raii::AuthedConnID::remote_conn_count() == 1
                                && sessions.len() > 1
                                && current_process_sid != sid
                                && sessions.iter().any(|e| e.sid == sid)
                            {
                                std::thread::spawn(move || {
                                    let _ = ipc::connect_to_user_session(Some(sid));
                                });
                                return false;
                            }
                            self.try_sub_monitor_services();
                        }
                    }
                    Some(misc::Union::MessageQuery(mq)) => {
                        if let Some(msg_out) = video_service::make_display_changed_msg(
                            mq.switch_display as _,
                            None,
                            self.video_source(),
                        ) {
                            self.send(msg_out).await;
                        }
                    }
                    // Only to a connection the cursor service would send the shape to.
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    Some(misc::Union::RequestCursorData(id)) => {
                        if self.is_remote()
                            && (self.peer_keyboard_enabled() || self.show_remote_cursor)
                        {
                            if let Some(msg) = input_service::cursor_data_message(id) {
                                self.send((*msg).clone()).await;
                            }
                        }
                    }
                    _ => {}
                },
                Some(message::Union::ScreenshotRequest(request)) => {
                    if let Some(tx) = self.inner.tx.clone() {
                        crate::video_service::set_take_screenshot(
                            self.video_source(),
                            request.display as _,
                            request.sid.clone(),
                            tx,
                        );
                        self.refresh_video_display(Some(request.display as usize));
                    }
                }
                _ => {}
            }
        }
        true
    }

    // Try to parse connection IP as IPv6 address, returning /64, /56, and /48 prefixes.
    // Parsing an IPv4 address just returns None.
    // note: we specifically don't use hbb_common::is_ipv6_str to avoid divergence issues
    // between its regex and the system std::net::Ipv6Addr implementation.
    fn get_ipv6_prefixes(&self) -> Option<(String, String, String)> {
        fn mask_u128(addr: u128, prefix: u8) -> u128 {
            let mask = if prefix == 0 || prefix > 128 {
                0
            } else {
                (!0u128) << (128 - prefix)
            };
            addr & mask
        }
        // eliminate zone-ids like "fe80::1%eth0"
        let ip_only = self.ip.split('%').next().unwrap_or(&self.ip).trim();
        let ip = Ipv6Addr::from_str(ip_only).ok()?;

        let as_u128 = u128::from_be_bytes(ip.octets());

        let p64 = Ipv6Addr::from(mask_u128(as_u128, 64).to_be_bytes()).to_string() + "/64";
        let p56 = Ipv6Addr::from(mask_u128(as_u128, 56).to_be_bytes()).to_string() + "/56";
        let p48 = Ipv6Addr::from(mask_u128(as_u128, 48).to_be_bytes()).to_string() + "/48";

        Some((p64, p56, p48))
    }

    fn bump_failure_entry(mut cur: (i32, i32, i32), time: i32) -> (i32, i32, i32) {
        if cur.0 == time {
            cur.1 += 1;
            cur.2 += 1;
        } else {
            cur.0 = time;
            cur.1 = 1;
            cur.2 += 1;
        }
        cur
    }

    fn update_failure(&self, (failure, time): ((i32, i32, i32), i32), remove: bool, i: usize) {
        let map_mutex = &LOGIN_FAILURES[i];
        if remove {
            if failure.0 != 0 {
                if let Some((p64, p56, p48)) = self.get_ipv6_prefixes() {
                    let mut m = map_mutex.lock().unwrap();
                    m.remove(&p64);
                    m.remove(&p56);
                    m.remove(&p48);
                    m.remove(&self.ip);
                } else {
                    map_mutex.lock().unwrap().remove(&self.ip);
                }
            }
            return;
        }
        // Bump the prefixes, fetching existing values
        if let Some((p64, p56, p48)) = self.get_ipv6_prefixes() {
            let mut m = map_mutex.lock().unwrap();
            for key in [p64, p56, p48] {
                let cur = m.get(&key).copied().unwrap_or((0, 0, 0));
                m.insert(key, Self::bump_failure_entry(cur, time));
            }
            let current_ip = m.get(&self.ip).copied().unwrap_or((0, 0, 0));
            m.insert(self.ip.clone(), Self::bump_failure_entry(current_ip, time));
        } else {
            // Re-read the full IP bucket in case another failed attempt updated it.
            let mut m = map_mutex.lock().unwrap();
            let current_ip = m.get(&self.ip).copied().unwrap_or((0, 0, 0));
            m.insert(self.ip.clone(), Self::bump_failure_entry(current_ip, time));
        }
    }

    async fn check_failure_ipv6_prefix(
        &mut self,
        i: usize,
        time: i32,
        prefix: &str,
        prefix_num: i8,
        thresh: i32,
    ) -> Option<(((i32, i32, i32), i32), bool)> {
        let failure_prefix = LOGIN_FAILURES[i]
            .lock()
            .unwrap()
            .get(prefix)
            .copied()
            .unwrap_or((0, 0, 0));

        if failure_prefix.2 > thresh {
            self.send_login_error(format!(
                "Too many wrong attempts for IPv6 prefix /{}",
                prefix_num
            ))
            .await;
            hbb_common::throttled_log!(
                Duration::from_secs(30),
                warn,
                "Connection rejected: ExceedIPv6PrefixAttempts"
            );
            Some(((failure_prefix, time), false))
        } else {
            None
        }
    }

    async fn check_failure(&mut self, i: usize) -> (((i32, i32, i32), i32), bool) {
        let time = (get_time() / 60_000) as i32;

        // IPv6 addresses are cheap to make so we check prefix/netblock as well
        if let Some((p64, p56, p48)) = self.get_ipv6_prefixes() {
            if let Some(res) = self.check_failure_ipv6_prefix(i, time, &p64, 64, 60).await {
                return res;
            }
            if let Some(res) = self.check_failure_ipv6_prefix(i, time, &p56, 56, 80).await {
                return res;
            }
            if let Some(res) = self.check_failure_ipv6_prefix(i, time, &p48, 48, 100).await {
                return res;
            }
        }

        // checks IPv6 and IPv4 direct addresses
        let failure = LOGIN_FAILURES[i]
            .lock()
            .unwrap()
            .get(&self.ip)
            .copied()
            .unwrap_or((0, 0, 0));

        let res = if failure.2 > 30 {
            self.send_login_error("Too many wrong attempts").await;
            hbb_common::throttled_log!(
                Duration::from_secs(30),
                warn,
                "Connection rejected: ExceedThirtyAttempts"
            );
            false
        } else if time == failure.0 && failure.1 > 6 {
            self.send_login_error("Please try 1 minute later").await;
            hbb_common::throttled_log!(
                Duration::from_secs(30),
                warn,
                "Connection rejected: SixAttemptsWithinOneMinute"
            );
            false
        } else {
            true
        };
        ((failure, time), res)
    }

    fn refresh_video_display(&self, display: Option<usize>) {
        video_service::refresh();
        self.server.upgrade().map(|s| {
            s.read().unwrap().set_video_service_opt(
                display.map(|d| (self.video_source(), d)),
                video_service::OPTION_REFRESH,
                super::service::SERVICE_OPTION_VALUE_TRUE,
            );
        });
    }

    async fn handle_switch_display(&mut self, s: SwitchDisplay) {
        let display_idx = s.display as usize;
        if self.display_idx != display_idx {
            if let Some(server) = self.server.upgrade() {
                if !self.switch_display_to(display_idx, server.clone()) {
                    return;
                }

                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                if s.width != 0 && s.height != 0 {
                    self.change_resolution(
                        None,
                        &Resolution {
                            width: s.width,
                            height: s.height,
                            ..Default::default()
                        },
                    );
                }
            }

            // Send display changed message.
            // 1. For compatibility with old versions ( < 1.2.4 ).
            // 2. Sciter version.
            // 3. Update `SupportedResolutions`.
            if let Some(msg_out) =
                video_service::make_display_changed_msg(self.display_idx, None, self.video_source())
            {
                self.send(msg_out).await;
            }
        }
    }

    fn video_source_count(video_source: VideoSource) -> usize {
        match video_source {
            VideoSource::Monitor => display_service::get_sync_displays().len(),
        }
    }

    fn video_source(&self) -> VideoSource {
        VideoSource::Monitor
    }

    fn switch_display_to(&mut self, display_idx: usize, server: Arc<RwLock<Server>>) -> bool {
        let source_count = Self::video_source_count(self.video_source());
        if display_idx >= source_count {
            // Do not remap an explicit switch: its resolution belongs to the requested source.
            log::warn!(
                "Ignore switch to invalid {:?} index {}, available source count: {}",
                self.video_source(),
                display_idx,
                source_count
            );
            return false;
        }

        let new_service_name = video_service::get_service_name(self.video_source(), display_idx);
        let old_service_name =
            video_service::get_service_name(self.video_source(), self.display_idx);
        let mut lock = server.write().unwrap();
        if !lock.contains(&new_service_name) {
            lock.add_service(Box::new(video_service::new(
                self.video_source(),
                display_idx,
            )));
        }
        // For versions greater than 1.2.4, a `CaptureDisplays` message will be sent immediately.
        // Unnecessary capturers will be removed then.
        if !crate::common::is_support_multi_ui_session(&self.lr.version) {
            lock.subscribe(&old_service_name, self.inner.clone(), false);
        }
        lock.subscribe(&new_service_name, self.inner.clone(), true);
        self.display_idx = display_idx;
        true
    }

    #[cfg(windows)]
    async fn handle_elevation_request(&mut self, para: portable_client::StartPara) {
        let mut err;
        if !self.keyboard {
            err = "No permission".to_string();
        } else {
            err = "No need to elevate".to_string();
            if !crate::platform::is_installed() && !portable_client::running() {
                err = portable_client::start_portable_service(para)
                    .err()
                    .map_or("".to_string(), |e| e.to_string());
            }
        }

        let mut misc = Misc::new();
        misc.set_elevation_response(err);
        let mut msg = Message::new();
        msg.set_misc(misc);
        self.send(msg).await;
        self.update_auto_disconnect_timer();
    }

    async fn capture_displays(&mut self, add: &[usize], sub: &[usize], set: &[usize]) {
        let video_source = self.video_source();
        let source_count = Self::video_source_count(video_source);
        // Only add/set can create services; sub only narrows existing subscriptions.
        let valid_add = add
            .iter()
            .copied()
            .filter(|display| *display < source_count)
            .collect::<Vec<_>>();
        let valid_sub = sub
            .iter()
            .copied()
            .filter(|display| *display < source_count)
            .collect::<Vec<_>>();
        let valid_set = set
            .iter()
            .copied()
            .filter(|display| *display < source_count)
            .collect::<Vec<_>>();
        let invalid_count =
            add.len() + sub.len() + set.len() - valid_add.len() - valid_sub.len() - valid_set.len();
        if invalid_count != 0 {
            log::warn!(
                "Ignore {} invalid {:?} indices, available source count: {}",
                invalid_count,
                video_source,
                source_count
            );
        }
        // Passing an invalid sub request as an empty exclude list would unsubscribe all services.
        if (!add.is_empty() && valid_add.is_empty())
            || (add.is_empty() && !sub.is_empty() && valid_sub.is_empty())
            || (add.is_empty() && sub.is_empty() && !set.is_empty() && valid_set.is_empty())
        {
            return;
        }

        if let Some(server) = self.server.upgrade() {
            let mut lock = server.write().unwrap();
            for display in valid_add.iter() {
                let service_name = video_service::get_service_name(video_source, *display);
                if !lock.contains(&service_name) {
                    lock.add_service(Box::new(video_service::new(video_source, *display)));
                }
            }
            for display in valid_set.iter() {
                let service_name = video_service::get_service_name(video_source, *display);
                if !lock.contains(&service_name) {
                    lock.add_service(Box::new(video_service::new(video_source, *display)));
                }
            }
            if !add.is_empty() {
                lock.capture_displays(self.inner.clone(), video_source, &valid_add, true, false);
            } else if !sub.is_empty() {
                lock.capture_displays(self.inner.clone(), video_source, &valid_sub, false, true);
            } else {
                lock.capture_displays(self.inner.clone(), video_source, &valid_set, true, true);
            }
            self.multi_ui_session = lock.get_subbed_displays_count(self.inner.id()) > 1;
            if self.follow_remote_window {
                lock.subscribe(
                    NAME_WINDOW_FOCUS,
                    self.inner.clone(),
                    !self.multi_ui_session,
                );
            }
            drop(lock);
        }
    }

    #[cfg(windows)]
    async fn toggle_virtual_display(&mut self, t: ToggleVirtualDisplay) {
        let make_msg = |text: String| {
            let mut msg_out = Message::new();
            let res = MessageBox {
                msgtype: "nook-nocancel-hasclose".to_owned(),
                title: "Virtual display".to_owned(),
                text,
                link: "".to_owned(),
                ..Default::default()
            };
            msg_out.set_message_box(res);
            msg_out
        };

        if t.on {
            if !virtual_display_manager::is_virtual_display_supported() {
                self.send(make_msg("idd_not_support_under_win10_2004_tip".to_string()))
                    .await;
            } else {
                if let Err(e) = virtual_display_manager::plug_in_monitor(t.display as _, Vec::new())
                {
                    log::error!("Failed to plug in virtual display: {}", e);
                    self.send(make_msg(format!(
                        "Failed to plug in virtual display: {}",
                        e
                    )))
                    .await;
                }
            }
        } else {
            if let Err(e) = virtual_display_manager::plug_out_monitor(t.display, false, true) {
                log::error!("Failed to plug out virtual display {}: {}", t.display, e);
                self.send(make_msg(format!(
                    "Failed to plug out virtual displays: {}",
                    e
                )))
                .await;
            }
        }
    }

    async fn toggle_privacy_mode(&mut self, t: TogglePrivacyMode) {
        if t.on {
            self.turn_on_privacy(t.impl_key).await;
        } else {
            self.turn_off_privacy(t.impl_key).await;
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn change_resolution(&mut self, d: Option<usize>, r: &Resolution) {
        if self.keyboard {
            if let Ok(displays) = display_service::try_get_displays() {
                let display_idx = d.unwrap_or(self.display_idx);
                if let Some(display) = displays.get(display_idx) {
                    let name = display.name();
                    #[cfg(windows)]
                    if let Some(_ok) =
                        virtual_display_manager::rustdesk_idd::change_resolution_if_is_virtual_display(
                            &name,
                            r.width as _,
                            r.height as _,
                        )
                    {
                        return;
                    }
                    #[allow(unused_mut)]
                    let mut record_changed = true;
                    #[cfg(windows)]
                    if virtual_display_manager::amyuni_idd::is_my_display(&name) {
                        record_changed = false;
                    }
                    #[cfg(not(target_os = "macos"))]
                    let scale = 1.0;
                    #[cfg(target_os = "macos")]
                    let scale = display.scale();
                    let original = (
                        ((display.width() as f64) / scale).round() as _,
                        (display.height() as f64 / scale).round() as _,
                    );
                    if record_changed {
                        display_service::set_last_changed_resolution(
                            &name,
                            original,
                            (r.width, r.height),
                        );
                    }
                    if let Err(e) =
                        crate::platform::change_resolution(&name, r.width as _, r.height as _)
                    {
                        log::error!(
                            "Failed to change resolution '{}' to ({},{}): {:?}",
                            &name,
                            r.width,
                            r.height,
                            e
                        );
                    }
                }
            }
        }
    }

    async fn update_options(&mut self, o: &OptionMessage) {
        log::info!("Option update: {:?}", o);
        if let Ok(q) = o.image_quality.enum_value() {
            let image_quality;
            if let ImageQuality::NotSet = q {
                if o.custom_image_quality > 0 {
                    image_quality = o.custom_image_quality;
                } else {
                    image_quality = -1;
                }
            } else {
                image_quality = q.value();
            }
            if image_quality > 0 {
                video_service::VIDEO_QOS
                    .lock()
                    .unwrap()
                    .user_image_quality(self.inner.id(), image_quality);
            }
        }
        if o.custom_fps > 0 {
            video_service::VIDEO_QOS
                .lock()
                .unwrap()
                .user_custom_fps(self.inner.id(), o.custom_fps as _);
        }
        if let Some(q) = o.supported_decoding.clone().take() {
            scrap::codec::Encoder::update(scrap::codec::EncodingUpdate::Update(self.inner.id(), q));
        }
        if let Ok(q) = o.lock_after_session_end.enum_value() {
            if q != BoolOption::NotSet {
                self.lock_after_session_end = q == BoolOption::Yes;
            }
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Ok(q) = o.show_remote_cursor.enum_value() {
            if q != BoolOption::NotSet {
                self.show_remote_cursor = q == BoolOption::Yes;
                if let Some(s) = self.server.upgrade() {
                    s.write().unwrap().subscribe(
                        NAME_CURSOR,
                        self.inner.clone(),
                        self.peer_keyboard_enabled() || self.show_remote_cursor,
                    );
                    s.write().unwrap().subscribe(
                        NAME_POS,
                        self.inner.clone(),
                        self.show_remote_cursor,
                    );
                }
            }
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Ok(q) = o.follow_remote_cursor.enum_value() {
            if q != BoolOption::NotSet {
                self.follow_remote_cursor = q == BoolOption::Yes;
            }
        }
        if let Ok(q) = o.follow_remote_window.enum_value() {
            if q != BoolOption::NotSet {
                self.follow_remote_window = q == BoolOption::Yes;
                if let Some(s) = self.server.upgrade() {
                    s.write().unwrap().subscribe(
                        NAME_WINDOW_FOCUS,
                        self.inner.clone(),
                        self.follow_remote_window,
                    );
                }
            }
        }
        if let Ok(q) = o.disable_audio.enum_value() {
            if q != BoolOption::NotSet {
                self.disable_audio = q == BoolOption::Yes;
                if let Some(s) = self.server.upgrade() {
                    s.write().unwrap().subscribe(
                        super::audio_service::NAME,
                        self.inner.clone(),
                        self.audio_enabled(),
                    );
                }
            }
        }
        if let Ok(q) = o.disable_clipboard.enum_value() {
            if q != BoolOption::NotSet {
                self.disable_clipboard = q == BoolOption::Yes;
                if let Some(s) = self.server.upgrade() {
                    s.write().unwrap().subscribe(
                        super::clipboard_service::NAME,
                        self.inner.clone(),
                        self.can_sub_clipboard_service(),
                    );
                }
            }
        }
        if let Ok(q) = o.disable_keyboard.enum_value() {
            if q != BoolOption::NotSet {
                self.disable_keyboard = q == BoolOption::Yes;
                if let Some(s) = self.server.upgrade() {
                    s.write().unwrap().subscribe(
                        super::clipboard_service::NAME,
                        self.inner.clone(),
                        self.can_sub_clipboard_service(),
                    );
                    s.write().unwrap().subscribe(
                        NAME_CURSOR,
                        self.inner.clone(),
                        self.peer_keyboard_enabled() || self.show_remote_cursor,
                    );
                }
            }
        }
        // For compatibility with old versions ( < 1.2.4 ).
        if hbb_common::get_version_number(&self.lr.version)
            < hbb_common::get_version_number("1.2.4")
        {
            if let Ok(q) = o.privacy_mode.enum_value() {
                if self.keyboard {
                    match q {
                        BoolOption::Yes => {
                            self.turn_on_privacy("".to_owned()).await;
                        }
                        BoolOption::No => {
                            self.turn_off_privacy("".to_owned()).await;
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Ok(q) = o.block_input.enum_value() {
            if self.keyboard && self.block_input {
                match q {
                    BoolOption::Yes => {
                        self.tx_input.send(MessageInput::BlockOn).ok();
                    }
                    BoolOption::No => {
                        self.tx_input.send(MessageInput::BlockOff).ok();
                    }
                    _ => {}
                }
            } else {
                if q != BoolOption::NotSet {
                    let state = if q == BoolOption::Yes {
                        back_notification::BlockInputState::BlkOnFailed
                    } else {
                        back_notification::BlockInputState::BlkOffFailed
                    };
                    if let Some(tx) = &self.inner.tx {
                        Self::send_block_input_error(tx, state, "No permission".to_string());
                    }
                }
            }
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Ok(q) = o.show_my_cursor.enum_value() {
            if q != BoolOption::NotSet {
                use crate::whiteboard;
                self.show_my_cursor = q == BoolOption::Yes;
                #[cfg(target_os = "linux")]
                let is_linux_supported = crate::whiteboard::is_supported();
                #[cfg(not(target_os = "linux"))]
                let is_linux_supported = false;
                let not_support_msg = if cfg!(target_os = "linux") && !is_linux_supported {
                    "This feature is not supported on Wayland."
                } else {
                    ""
                };
                if q == BoolOption::Yes {
                    if not_support_msg.is_empty() {
                        whiteboard::register_whiteboard(whiteboard::get_key_cursor(self.inner.id));
                    } else {
                        let mut msg_out = Message::new();
                        let res = MessageBox {
                            msgtype: "nook-nocancel-hasclose".to_owned(),
                            title: "Show my cursor".to_owned(),
                            text: not_support_msg.to_owned(),
                            link: "".to_owned(),
                            ..Default::default()
                        };
                        msg_out.set_message_box(res);
                        self.send(msg_out).await;
                    }
                } else {
                    if not_support_msg.is_empty() {
                        whiteboard::unregister_whiteboard(whiteboard::get_key_cursor(
                            self.inner.id,
                        ));
                    }
                }
            }
        }
    }

    async fn turn_on_privacy(&mut self, impl_key: String) {
        if !self.is_authed_remote_conn() || !self.privacy_mode {
            let msg_out = crate::common::make_privacy_mode_msg(
                back_notification::PrivacyModeState::PrvOnFailedDenied,
                impl_key,
            );
            self.send(msg_out).await;
            return;
        }

        let msg_out = if !privacy_mode::is_privacy_mode_supported() {
            crate::common::make_privacy_mode_msg_with_details(
                back_notification::PrivacyModeState::PrvNotSupported,
                "Unsupported. 1 Multi-screen is not supported. 2 Please confirm the license is activated.".to_string(),
                impl_key,
            )
        } else {
            let is_pre_privacy_on = privacy_mode::is_in_privacy_mode();
            let pre_impl_key = privacy_mode::get_cur_impl_key();

            if is_pre_privacy_on {
                if let Some(pre_impl_key) = pre_impl_key {
                    if !privacy_mode::is_current_privacy_mode_impl(&pre_impl_key) {
                        let off_msg = crate::common::make_privacy_mode_msg(
                            back_notification::PrivacyModeState::PrvOffSucceeded,
                            pre_impl_key,
                        );
                        self.send(off_msg).await;
                    }
                }
            }

            let turn_on_res = privacy_mode::turn_on_privacy(&impl_key, self.inner.id).await;
            match turn_on_res {
                Some(Ok(res)) => {
                    if res {
                        let err_msg = privacy_mode::check_privacy_mode_err(
                            self.inner.id,
                            self.display_idx,
                            5_000,
                        );
                        if err_msg.is_empty() {
                            privacy_mode::close_other_remote_connections_for_privacy(self.inner.id);
                            crate::common::make_privacy_mode_msg(
                                back_notification::PrivacyModeState::PrvOnSucceeded,
                                impl_key,
                            )
                        } else {
                            log::error!(
                                "Check privacy mode failed: {}, turn off privacy mode.",
                                &err_msg
                            );
                            let _ = Self::turn_off_privacy_to_msg(self.inner.id, String::new());
                            crate::common::make_privacy_mode_msg_with_details(
                                back_notification::PrivacyModeState::PrvOnFailed,
                                err_msg,
                                impl_key,
                            )
                        }
                    } else {
                        crate::common::make_privacy_mode_msg(
                            back_notification::PrivacyModeState::PrvOnFailed,
                            impl_key,
                        )
                    }
                }
                Some(Err(e)) => {
                    log::error!("Failed to turn on privacy mode. {}", e);
                    if privacy_mode::is_in_privacy_mode() {
                        // Use this connection's ID so a failed request cannot disable another
                        // connection's privacy mode: INVALID_PRIVACY_MODE_CONN_ID would bypass
                        // the ownership check (see `check_off_conn_id`).
                        let _ = Self::turn_off_privacy_to_msg(self.inner.id, String::new());
                    }
                    crate::common::make_privacy_mode_msg_with_details(
                        back_notification::PrivacyModeState::PrvOnFailed,
                        e.to_string(),
                        impl_key,
                    )
                }
                None => crate::common::make_privacy_mode_msg_with_details(
                    back_notification::PrivacyModeState::PrvOffFailed,
                    "Not supported".to_string(),
                    impl_key,
                ),
            }
        };
        self.send(msg_out).await;
    }

    async fn turn_off_privacy(&mut self, impl_key: String) {
        let msg_out = if !privacy_mode::is_privacy_mode_supported() {
            crate::common::make_privacy_mode_msg_with_details(
                back_notification::PrivacyModeState::PrvNotSupported,
                // This error message is used for magnifier. It is ok to use it here.
                "Unsupported. 1 Multi-screen is not supported. 2 Please confirm the license is activated.".to_string(),
                impl_key,
            )
        } else {
            Self::turn_off_privacy_to_msg(self.inner.id, impl_key)
        };
        self.send(msg_out).await;
    }

    pub fn turn_off_privacy_to_msg(_conn_id: i32, impl_key: String) -> Message {
        Self::turn_off_privacy_result_to_msg(
            privacy_mode::turn_off_privacy(_conn_id, None),
            impl_key,
        )
    }

    fn turn_off_privacy_result_to_msg(
        turn_off_res: Option<hbb_common::ResultType<()>>,
        impl_key: String,
    ) -> Message {
        match turn_off_res {
            Some(Ok(_)) => crate::common::make_privacy_mode_msg(
                back_notification::PrivacyModeState::PrvOffSucceeded,
                impl_key,
            ),
            Some(Err(e)) => {
                log::error!("Failed to turn off privacy mode {}", e);
                crate::common::make_privacy_mode_msg_with_details(
                    back_notification::PrivacyModeState::PrvOffFailed,
                    e.to_string(),
                    impl_key,
                )
            }
            None => crate::common::make_privacy_mode_msg_with_details(
                back_notification::PrivacyModeState::PrvOffFailed,
                "Not supported".to_string(),
                impl_key,
            ),
        }
    }

    async fn on_close(&mut self, reason: &str, lock: bool) {
        if self.closed {
            return;
        }
        self.closed = true;
        log::info!("#{} Connection closed: {}", self.inner.id(), reason);
        if lock
            && self.lock_after_session_end
            && self.keyboard
            && !raii::AuthedConnID::session_reconnected(self.inner.id(), &self.session_key())
        {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            lock_screen().await;
        }
        self.tx_to_cm.send(ipc::Data::Close).ok();
    }

    // The `reason` should be consistent with `check_if_retry` if not empty
    async fn send_close_reason_no_retry(&mut self, reason: &str) {
        let mut misc = Misc::new();
        if reason.is_empty() {
            misc.set_close_reason("Closed manually by the peer".to_string());
        } else {
            misc.set_close_reason(reason.to_string());
        }
        let mut msg_out = Message::new();
        msg_out.set_misc(misc);
        self.send(msg_out).await;
        raii::AuthedConnID::check_remove_session(self.inner.id(), self.session_key());
    }

    #[inline]
    async fn send(&mut self, msg: Message) {
        allow_err!(self.stream.send(&msg).await);
    }

    pub fn alive_conns() -> Vec<i32> {
        ALIVE_CONNS.lock().unwrap().clone()
    }

    #[cfg(windows)]
    fn portable_check(&mut self) {
        if self.portable.is_installed || !self.is_remote() || !self.keyboard {
            return;
        }
        let running = portable_client::running();
        let show_elevation = !running;
        self.send_to_cm(ipc::Data::DataPortableService(
            ipc::DataPortableService::CmShowElevation(show_elevation),
        ));
        if self.authorized {
            let p = &mut self.portable;
            if Some(running) != p.last_running {
                p.last_running = Some(running);
                let mut misc = Misc::new();
                misc.set_portable_service_running(running);
                let mut msg = Message::new();
                msg.set_misc(misc);
                self.inner.send(msg.into());
            }
            let uac = crate::video_service::IS_UAC_RUNNING.lock().unwrap().clone();
            if p.last_uac != uac {
                p.last_uac = uac;
                if !uac || !running {
                    let mut misc = Misc::new();
                    misc.set_uac(uac);
                    let mut msg = Message::new();
                    msg.set_misc(misc);
                    self.inner.send(msg.into());
                }
            }
            let foreground_window_elevated = crate::video_service::IS_FOREGROUND_WINDOW_ELEVATED
                .lock()
                .unwrap()
                .clone();
            if p.last_foreground_window_elevated != foreground_window_elevated {
                p.last_foreground_window_elevated = foreground_window_elevated;
                if !foreground_window_elevated || !running {
                    let mut misc = Misc::new();
                    misc.set_foreground_window_elevated(foreground_window_elevated);
                    let mut msg = Message::new();
                    msg.set_misc(misc);
                    self.inner.send(msg.into());
                }
            }
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn release_pressed_modifiers(&mut self) {
        for modifier in self.pressed_modifiers.iter() {
            rdev::simulate(&rdev::EventType::KeyRelease(*modifier)).ok();
        }
        self.pressed_modifiers.clear();
    }

    fn get_auto_disconenct_timer() -> Option<(Instant, u64)> {
        if Config::get_option("allow-auto-disconnect") == "Y" {
            let mut minute: u64 = Config::get_option("auto-disconnect-timeout")
                .parse()
                .unwrap_or(10);
            if minute == 0 {
                minute = 10;
            }
            Some((Instant::now(), minute))
        } else {
            None
        }
    }

    fn update_auto_disconnect_timer(&mut self) {
        self.auto_disconnect_timer
            .as_mut()
            .map(|t| t.0 = Instant::now());
    }

    #[cfg(feature = "hwcodec")]
    fn update_supported_encoding(&mut self) {
        let Some(last) = &self.last_supported_encoding else {
            return;
        };
        let usable = scrap::codec::Encoder::usable_encoding();
        let Some(usable) = usable else {
            return;
        };
        if usable.vp8 != last.vp8
            || usable.av1 != last.av1
            || usable.h264 != last.h264
            || usable.h265 != last.h265
        {
            let mut misc: Misc = Misc::new();
            let supported_encoding = SupportedEncoding {
                vp8: usable.vp8,
                av1: usable.av1,
                h264: usable.h264,
                h265: usable.h265,
                ..last.clone()
            };
            log::info!("update supported encoding: {:?}", supported_encoding);
            self.last_supported_encoding = Some(supported_encoding.clone());
            misc.set_supported_encoding(supported_encoding);
            let mut msg = Message::new();
            msg.set_misc(misc);
            self.inner.send(msg.into());
        };
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    async fn handle_cursor_switch_display(&mut self, pos: CursorPosition) {
        if self.multi_ui_session {
            return;
        }
        let displays = super::display_service::get_sync_displays();
        let d_index = displays.iter().position(|d| {
            let scale = d.scale;
            pos.x >= d.x
                && pos.y >= d.y
                && (pos.x - d.x) as f64 * scale < d.width as f64
                && (pos.y - d.y) as f64 * scale < d.height as f64
        });
        if let Some(d_index) = d_index {
            if self.display_idx != d_index {
                let mut misc = Misc::new();
                misc.set_follow_current_display(d_index as i32);
                let mut msg_out = Message::new();
                msg_out.set_misc(misc);
                self.send(msg_out).await;
            }
        }
    }

    #[inline]
    fn session_key(&self) -> SessionKey {
        SessionKey {
            peer_id: self.lr.my_id.clone(),
            name: self.lr.my_name.clone(),
            session_id: self.lr.session_id,
        }
    }

    fn is_authed_remote_conn(&self) -> bool {
        if let Some(id) = self.authed_conn_id.as_ref() {
            return id.conn_type() == AuthConnType::Remote;
        }
        false
    }

    fn should_handle_render_broadcast_message(&self) -> bool {
        matches!(self.authed_conn_type(), Some(AuthConnType::Remote))
    }

    fn should_handle_text_clipboard_message(&self) -> bool {
        matches!(self.authed_conn_type(), Some(AuthConnType::Remote))
    }

    fn authed_conn_type(&self) -> Option<AuthConnType> {
        self.authed_conn_id.as_ref().map(|id| id.conn_type())
    }

    async fn handle_authorized_scope_violation(&mut self, message: &'static str) -> bool {
        let conn_type = self
            .authed_conn_type()
            .map(AuthConnType::as_str)
            .unwrap_or("unknown");
        let is_first = self.scope_violation_messages.insert(message);
        if is_first {
            log::warn!(
                "Received out-of-scope message in {} session: {}",
                conn_type,
                message
            );
        } else {
            log::debug!(
                "Received repeated out-of-scope message in {} session: {}",
                conn_type,
                message
            );
        }
        if Config::get_bool_option(keys::OPTION_ALLOW_SCOPE_VIOLATION_CLOSE) {
            self.send_close_reason_no_retry("Connection not allowed")
                .await;
            self.on_close("Session scope violation", true).await;
            return false;
        }
        true
    }

    fn authorized_scope_violation(&self, msg: &Message) -> Option<&'static str> {
        let Some(conn_type) = self.authed_conn_type() else {
            return (!Self::is_connection_housekeeping_message(msg)).then_some("session.auth_type");
        };
        Self::authorized_message_scope_violation(conn_type, msg)
    }

    async fn update_scoped_login_options(&mut self) {
        let Some(option) = self.options_in_login.take() else {
            return;
        };
        let Some(conn_type) = self.authed_conn_type() else {
            // Unreachable, but just in case, we drop the options if the connection type is unknown.
            log::warn!(
                "Dropping scoped login options because authorized connection type is unknown"
            );
            return;
        };
        let (scoped, violation) = Self::scoped_login_option(conn_type, &option);
        if let Some(message) = violation {
            log::debug!(
                "Filtering {} session login options outside scope: {}",
                conn_type.as_str(),
                message
            );
        }
        if let Some(option) = scoped {
            self.update_options(&option).await;
        }
    }

    fn scoped_login_option(
        _conn_type: AuthConnType,
        option: &OptionMessage,
    ) -> (Option<OptionMessage>, Option<&'static str>) {
        (Some(option.clone()), None)
    }

    fn authorized_message_scope_violation(
        _conn_type: AuthConnType,
        msg: &Message,
    ) -> Option<&'static str> {
        match msg.union.as_ref() {
            Some(message::Union::Clipboard(cb))
                if cb.format.enum_value() != Ok(ClipboardFormat::Text) =>
            {
                Some("clipboard.format")
            }
            Some(message::Union::MultiClipboards(cb))
                if cb
                    .clipboards
                    .iter()
                    .any(|cb| cb.format.enum_value() != Ok(ClipboardFormat::Text)) =>
            {
                Some("clipboard.format")
            }
            _ => None,
        }
    }

    fn is_connection_housekeeping_message(msg: &Message) -> bool {
        match msg.union.as_ref() {
            Some(message::Union::LoginRequest(_)) => true,
            Some(message::Union::TestDelay(_)) => true,
            Some(message::Union::Misc(misc)) => {
                matches!(misc.union.as_ref(), Some(misc::Union::CloseReason(_)))
            }
            _ => false,
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
// IPC bootstrap summary:
// - Start CM when missing, then bridge bidirectional messages between this task and CM IPC.
async fn start_ipc(
    mut rx_to_cm: mpsc::UnboundedReceiver<ipc::Data>,
    tx_from_cm: mpsc::UnboundedSender<ipc::Data>,
) -> ResultType<()> {
    use hbb_common::anyhow::anyhow;

    loop {
        if !crate::platform::is_prelogin() {
            break;
        }
        sleep(1.).await;
    }
    let mut stream = None;
    if let Ok(s) = crate::ipc::connect(1000, "_cm").await {
        stream = Some(s);
    }
    if stream.is_none() {
        let args = vec!["--cm"];
        let run_done;
        if crate::platform::is_root() {
            let mut res = Ok(None);
            for _ in 0..10 {
                #[cfg(not(any(target_os = "linux")))]
                {
                    log::debug!("Start cm");
                    res = crate::platform::run_as_user(args.clone());
                }
                #[cfg(target_os = "linux")]
                {
                    log::debug!("Start cm");
                    res = crate::platform::run_as_user(args.clone(), None, None::<(&str, &str)>);
                }
                if res.is_ok() {
                    break;
                }
                log::error!("Failed to run cm: {res:?}");
                sleep(1.).await;
            }
            if let Some(task) = res? {
                super::CHILD_PROCESS.lock().unwrap().push(task);
            }
            run_done = true;
        } else {
            run_done = false;
        }
        if !run_done {
            log::debug!("Start cm");
            super::CHILD_PROCESS
                .lock()
                .unwrap()
                .push(crate::run_me(args)?);
        }
        for _ in 0..20 {
            sleep(0.3).await;
            if let Ok(s) = crate::ipc::connect(1000, "_cm").await {
                stream = Some(s);
                break;
            }
        }
    }
    if stream.is_none() {
        bail!("Failed to connect to connection manager");
    }

    let mut stream = stream.ok_or(anyhow!("none stream"))?;
    loop {
        tokio::select! {
            res = stream.next() => {
                match res {
                    Err(err) => {
                        return Err(err.into());
                    }
                    Ok(Some(data)) => {
                        match data {
                            ipc::Data::ClickTime(_)=> {
                                let ct = CLICK_TIME.load(Ordering::SeqCst);
                                let data = ipc::Data::ClickTime(ct);
                                stream.send(&data).await?;
                            }
                            _ => {
                                tx_from_cm.send(data)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            res = rx_to_cm.recv() => {
                match res {
                    Some(data) => {
                        stream.send(&data).await?;
                    }
                    None => {
                        bail!("expected");
                    }
                }
            }
        }
    }
}

// in case screen is sleep and blank, here to activate it
fn try_activate_screen() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        mouse_move_relative(-6, -6);
        std::thread::sleep(std::time::Duration::from_millis(30));
        mouse_move_relative(6, 6);
    });
}

fn start_wakelock_thread() -> std::sync::mpsc::Sender<(usize, usize)> {
    // Check if we should keep awake during incoming sessions
    use crate::platform::{get_wakelock, WakeLock};
    let (tx, rx) = std::sync::mpsc::channel::<(usize, usize)>();
    std::thread::spawn(move || {
        let mut wakelock: Option<WakeLock> = None;
        let mut last_display = false;
        loop {
            match rx.recv() {
                Ok((conn_count, remote_count)) => {
                    let keep_awake = config::Config::get_bool_option(
                        keys::OPTION_KEEP_AWAKE_DURING_INCOMING_SESSIONS,
                    );
                    *WAKELOCK_KEEP_AWAKE_OPTION.lock().unwrap() = Some(keep_awake);
                    if conn_count == 0 || !keep_awake {
                        if wakelock.is_some() {
                            wakelock = None;
                            log::info!("drop wakelock");
                        }
                    } else {
                        let mut display = remote_count > 0;
                        if let Some(_w) = wakelock.as_mut() {
                            if display != last_display {
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                {
                                    log::info!("set wakelock display to {display}");
                                    if let Err(e) = _w.set_display(display) {
                                        log::error!(
                                            "failed to set wakelock display to {display}: {e:?}"
                                        );
                                    }
                                }
                            }
                        } else {
                            if cfg!(target_os = "linux") {
                                display = true;
                            }
                            wakelock = Some(get_wakelock(display));
                        }
                        last_display = display;
                    }
                }
                Err(e) => {
                    log::error!("wakelock receive error: {e:?}");
                    break;
                }
            }
        }
    });
    tx
}

#[cfg(windows)]
pub struct PortableState {
    pub last_uac: bool,
    pub last_foreground_window_elevated: bool,
    pub last_running: Option<bool>,
    pub is_installed: bool,
}

#[cfg(windows)]
impl Default for PortableState {
    fn default() -> Self {
        Self {
            is_installed: crate::platform::is_installed(),
            last_uac: Default::default(),
            last_foreground_window_elevated: Default::default(),
            last_running: Default::default(),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        self.release_pressed_modifiers();
    }
}

extern "C" fn connection_shutdown_hook() {
    // https://stackoverflow.com/questions/35980148/why-does-an-atexit-handler-panic-when-it-accesses-stdout
    // Please make sure there is no print in the call stack
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        *WALLPAPER_REMOVER.lock().unwrap() = None;
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
struct Retina {
    displays: Vec<DisplayInfo>,
}

#[cfg(target_os = "macos")]
impl Retina {
    #[inline]
    fn set_displays(&mut self, displays: &Vec<DisplayInfo>) {
        self.displays = displays.clone();
    }

    #[inline]
    fn on_mouse_event(&mut self, e: &mut MouseEvent, current: usize) {
        let evt_type = e.mask & crate::input::MOUSE_TYPE_MASK;
        // Delta-based events do not contain absolute coordinates.
        // Avoid applying Retina coordinate scaling to them.
        if evt_type == crate::input::MOUSE_TYPE_WHEEL
            || evt_type == crate::input::MOUSE_TYPE_TRACKPAD
            || evt_type == crate::input::MOUSE_TYPE_MOVE_RELATIVE
        {
            return;
        }
        let Some(d) = self.displays.get(current) else {
            return;
        };
        let s = d.scale;
        if s > 1.0 && e.x >= d.x && e.y >= d.y && e.x < d.x + d.width && e.y < d.y + d.height {
            e.x = d.x + ((e.x - d.x) as f64 / s) as i32;
            e.y = d.y + ((e.y - d.y) as f64 / s) as i32;
        }
    }

    #[inline]
    fn on_cursor_pos(&mut self, pos: &CursorPosition, current: usize) -> Option<Message> {
        let Some(d) = self.displays.get(current) else {
            return None;
        };
        let s = d.scale;
        if s > 1.0
            && pos.x >= d.x
            && pos.y >= d.y
            && (pos.x - d.x) as f64 * s < d.width as f64
            && (pos.y - d.y) as f64 * s < d.height as f64
        {
            let mut pos = pos.clone();
            pos.x = d.x + ((pos.x - d.x) as f64 * s) as i32;
            pos.y = d.y + ((pos.y - d.y) as f64 * s) as i32;
            let mut msg = Message::new();
            msg.set_cursor_position(pos);
            return Some(msg);
        }
        None
    }
}

/// Get control permission state from CONTROL_PERMISSIONS_ARRAY.
/// Returns: Some(false) if any disable, Some(true) if any enable (and no disable), None if not set.
pub fn get_control_permission_state(
    permission: hbb_common::rendezvous_proto::control_permissions::Permission,
    disable_if_has_disabled: bool,
) -> Option<bool> {
    let control_permissions = CONTROL_PERMISSIONS_ARRAY.lock().unwrap();
    let mut has_enable = false;
    let mut has_disable = false;
    for (_, cp) in control_permissions.iter() {
        match crate::get_control_permission(cp.permissions, permission) {
            Some(false) => has_disable = true,
            Some(true) => has_enable = true,
            None => {}
        }
    }
    if disable_if_has_disabled {
        if has_disable {
            Some(false)
        } else if has_enable {
            Some(true)
        } else {
            None
        }
    } else {
        if has_enable {
            Some(true)
        } else if has_disable {
            Some(false)
        } else {
            None
        }
    }
}

pub struct AuthedConn {
    pub conn_id: i32,
    pub conn_type: AuthConnType,
    pub session_key: SessionKey,
    pub sender: mpsc::UnboundedSender<Data>,
}

mod raii {
    // ALIVE_CONNS: all connections, including unauthorized connections
    // AUTHED_CONNS: all authorized connections
    // CONTROL_PERMISSIONS_ARRAY: all non-None control permissions

    use super::*;
    pub struct ConnectionID(i32);

    impl ConnectionID {
        pub fn new(id: i32) -> Self {
            ALIVE_CONNS.lock().unwrap().push(id);
            Self(id)
        }
    }

    impl Drop for ConnectionID {
        fn drop(&mut self) {
            let mut active_conns_lock = ALIVE_CONNS.lock().unwrap();
            active_conns_lock.retain(|&c| c != self.0);
        }
    }

    pub struct AuthedConnID(i32, AuthConnType);

    impl AuthedConnID {
        pub(super) fn is_newer_session_remote(c: &AuthedConn, id: i32, key: &SessionKey) -> bool {
            c.conn_id > id && c.conn_type == AuthConnType::Remote && &c.session_key == key
        }

        /// Whether a newer remote control connection of this session has replaced this one. A
        /// controlling peer whose link dies reconnects while the connection it left behind runs
        /// on here until its own timeout; locking for that one would lock a session that has
        /// already resumed on its replacement.
        pub fn session_reconnected(id: i32, key: &SessionKey) -> bool {
            let conns = AUTHED_CONNS.lock().unwrap();
            conns
                .iter()
                .any(|c| Self::is_newer_session_remote(c, id, key))
        }

        pub fn new(
            conn_id: i32,
            conn_type: AuthConnType,
            session_key: SessionKey,
            sender: mpsc::UnboundedSender<Data>,
        ) -> Self {
            AUTHED_CONNS.lock().unwrap().push(AuthedConn {
                conn_id,
                conn_type,
                session_key,
                sender,
            });
            Self::check_wake_lock();
            use std::sync::Once;
            static _ONCE: Once = Once::new();
            _ONCE.call_once(|| {
                shutdown_hooks::add_shutdown_hook(connection_shutdown_hook);
            });
            if conn_type == AuthConnType::Remote {
                video_service::VIDEO_QOS
                    .lock()
                    .unwrap()
                    .on_connection_open(conn_id);
            }
            Self(conn_id, conn_type)
        }

        fn check_wake_lock() {
            let conn_count = AUTHED_CONNS.lock().unwrap().len();
            let remote_count = AUTHED_CONNS
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.conn_type == AuthConnType::Remote)
                .count();
            allow_err!(WAKELOCK_SENDER
                .lock()
                .unwrap()
                .send((conn_count, remote_count)));
        }

        pub fn check_wake_lock_on_setting_changed() {
            let current =
                config::Config::get_bool_option(keys::OPTION_KEEP_AWAKE_DURING_INCOMING_SESSIONS);
            let cached = *WAKELOCK_KEEP_AWAKE_OPTION.lock().unwrap();
            if cached != Some(current) {
                Self::check_wake_lock();
            }
        }

        #[cfg(windows)]
        pub fn remote_conn_count() -> usize {
            AUTHED_CONNS
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.conn_type == AuthConnType::Remote)
                .count()
        }

        pub fn check_remove_session(conn_id: i32, key: SessionKey) {
            let mut lock = SESSIONS.lock().unwrap();
            let contains = lock.contains_key(&key);
            if contains {
                // No two remote connections with the same session key, just for ensure.
                let is_remote = AUTHED_CONNS
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|c| c.conn_id == conn_id && c.conn_type == AuthConnType::Remote);
                // If there are 2 connections with the same peer_id and session_id, a remote connection and a file transfer or port forward connection,
                // If any of the connections is closed allowing retry, this will not be called;
                // If the file transfer/port forward connection is closed with no retry, the session should be kept for remote control menu action;
                // If the remote connection is closed with no retry, keep the session is not reasonable in case there is a retry button in the remote side, and ignore network fluctuations.
                let another_remote = AUTHED_CONNS.lock().unwrap().iter().any(|c| {
                    c.conn_id != conn_id
                        && c.session_key == key
                        && c.conn_type == AuthConnType::Remote
                });
                if is_remote || !another_remote {
                    lock.remove(&key);
                    log::info!("remove session");
                } else {
                    // Keep the session if there is another remote connection with same peer_id and session_id.
                    log::info!("skip remove session");
                }
            }
        }

        pub fn update_or_insert_session(
            key: SessionKey,
            password: Option<String>,
            tfa: Option<bool>,
        ) {
            let mut lock = SESSIONS.lock().unwrap();
            let session = lock.get_mut(&key);
            if let Some(session) = session {
                if let Some(password) = password {
                    session.random_password = password;
                }
                if let Some(tfa) = tfa {
                    session.tfa = tfa;
                }
            } else {
                lock.insert(
                    key,
                    Session {
                        random_password: password.unwrap_or_default(),
                        tfa: tfa.unwrap_or_default(),
                        last_recv_time: Arc::new(Mutex::new(Instant::now())),
                    },
                );
            }
        }

        pub fn set_session_2fa(key: SessionKey) {
            let mut lock = SESSIONS.lock().unwrap();
            let session = lock.get_mut(&key);
            if let Some(session) = session {
                session.tfa = true;
            } else {
                lock.insert(
                    key,
                    Session {
                        last_recv_time: Arc::new(Mutex::new(Instant::now())),
                        random_password: "".to_owned(),
                        tfa: true,
                    },
                );
            }
        }

        pub fn conn_type(&self) -> AuthConnType {
            self.1
        }
    }

    impl Drop for AuthedConnID {
        fn drop(&mut self) {
            if self.1 == AuthConnType::Remote {
                scrap::codec::Encoder::update(scrap::codec::EncodingUpdate::Remove(self.0));
                video_service::VIDEO_QOS
                    .lock()
                    .unwrap()
                    .on_connection_close(self.0);
            }
            // Clear per-connection state to avoid stale behavior if conn ids are reused.
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            clear_relative_mouse_active(self.0);
            AUTHED_CONNS.lock().unwrap().retain(|c| c.conn_id != self.0);
            let remote_count = AUTHED_CONNS
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.conn_type == AuthConnType::Remote)
                .count();
            if remote_count == 0 {
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                {
                    *WALLPAPER_REMOVER.lock().unwrap() = None;
                }
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                display_service::restore_resolutions();
                #[cfg(windows)]
                let _ = virtual_display_manager::reset_all();
                #[cfg(target_os = "linux")]
                if self.1 == AuthConnType::Remote {
                    wayland::try_close_portal_session();
                }
            }
            Self::check_wake_lock();
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                use crate::whiteboard;
                whiteboard::unregister_whiteboard(whiteboard::get_key_cursor(self.0));
            }
        }
    }

    pub struct ControlPermissionsID {
        id: i32,
        control_permissions: Option<ControlPermissions>,
    }

    impl Drop for ControlPermissionsID {
        fn drop(&mut self) {
            if self.control_permissions.is_some() {
                let mut lock = CONTROL_PERMISSIONS_ARRAY.lock().unwrap();
                lock.retain(|(conn_id, _)| *conn_id != self.id);
            }
        }
    }
    impl ControlPermissionsID {
        pub fn new(id: i32, control_permissions: &Option<ControlPermissions>) -> Self {
            if let Some(s) = control_permissions {
                CONTROL_PERMISSIONS_ARRAY
                    .lock()
                    .unwrap()
                    .push((id, s.clone()));
            }
            Self {
                id,
                control_permissions: control_permissions.clone(),
            }
        }
    }
}

// An empty whitelist allows everyone.
//
// A peer connecting across servers reports `<its id>@<its own server>` (see
// `create_login_msg`), so the bare id is matched as well. That suffix is self-asserted and
// unsigned, so matching only the full form would reject the honest cross-server peer while
// an attacker just reports the bare id: it can produce false rejects but no true ones.
fn id_whitelist_allows(id_whitelist: &[String], my_id: &str) -> bool {
    if id_whitelist.is_empty() {
        return true;
    }
    let bare_id = my_id.split('@').next().unwrap_or(my_id);
    id_whitelist
        .iter()
        .any(|x| wildcard_match(x, my_id) || wildcard_match(x, bare_id))
}

// Drop `keys` whose last failure (`.0`, in minutes) is at least `window` old. A backwards
// clock gives a negative age and keeps the entry, so it never widens access.
fn decay_stale_failures(
    failures: &mut HashMap<String, (i32, i32, i32)>,
    keys: &[String],
    now: i32,
    window: i32,
) {
    for key in keys {
        if failures
            .get(key)
            .is_some_and(|v| now.saturating_sub(v.0) >= window)
        {
            failures.remove(key);
        }
    }
}

// Unconditionally forget `keys`, unlike `update_failure`'s remove path which requires the
// per-address entry to exist.
fn clear_failures(failures: &mut HashMap<String, (i32, i32, i32)>, keys: &[String]) {
    for key in keys {
        failures.remove(key);
    }
}

// Simple glob matching for the ID whitelist: '*' matches any sequence of characters
// (including the empty one), '?' matches exactly one character. Case-insensitive.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let t: Vec<char> = text.trim().to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi + 1, ti));
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod test {
    #[allow(unused)]
    use super::*;

    // The registry is process-global and the harness runs tests in parallel threads, so every
    // test that admits connections holds this first; a poisoned lock is still a lock.
    static UNAUTHORIZED_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn unauthorized_count() -> usize {
        UNAUTHORIZED_CONNS.lock().unwrap().len()
    }

    #[test]
    fn test_unauthorized_admission_is_per_address() {
        let _serial = UNAUTHORIZED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let a: IpAddr = "203.0.113.1".parse().unwrap();
        let b: IpAddr = "203.0.113.2".parse().unwrap();
        let held: Vec<_> = (0..MAX_UNAUTHORIZED_CONNS_PER_ADDR as i32)
            .map(|i| admit_unauthorized(1_000_000 + i, a).unwrap())
            .collect();
        assert!(admit_unauthorized(1_000_100, a).is_none());
        assert!(
            held.iter().all(|u| !u.is_evicted()),
            "a refusal evicts nobody"
        );
        let other = admit_unauthorized(1_000_101, b).unwrap();
        assert!(!other.is_evicted());
        drop(held);
        assert!(admit_unauthorized(1_000_102, a).is_some());
    }

    /// A loopback TCP connection as create_tcp_connection sees it, and the controller's end,
    /// which never speaks: the connection stalls in the identity handshake.
    async fn stalled_incoming() -> (Stream, Stream, SocketAddr) {
        let listener = hbb_common::tcp::new_listener("127.0.0.1:0", false)
            .await
            .unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let controller = hbb_common::socket_client::connect_tcp(host, 3000)
            .await
            .unwrap();
        let (accepted, addr) = listener.accept().await.unwrap();
        let served = Stream::Tcp(hbb_common::tcp::FramedStream::from(accepted, addr));
        (served, controller, addr)
    }

    // Live connections, not bookkeeping: with the limit reached by connections stalled in the
    // handshake, one more arrival is refused and the oldest handshake is ended at once, not on
    // a timer tick, so the live count never exceeds the limit and a place opens only then.
    #[tokio::test]
    async fn test_unauthorized_limit_bounds_live_handshakes() {
        let _serial = UNAUTHORIZED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let server = crate::server::new_for_test();
        let mut controllers = Vec::new();
        let mut handshakes = Vec::new();
        for i in 0..MAX_UNAUTHORIZED_CONNS {
            let (served, controller, _) = stalled_incoming().await;
            controllers.push(controller);
            // Each from an address of its own, so only the global limit is in play.
            let addr: SocketAddr = format!("192.0.2.{}:1", i + 1).parse().unwrap();
            let server = server.clone();
            handshakes.push(tokio::spawn(async move {
                crate::server::create_tcp_connection(server, served, addr, Default::default())
                    .await
            }));
        }
        for _ in 0..200 {
            if unauthorized_count() == MAX_UNAUTHORIZED_CONNS {
                break;
            }
            hbb_common::sleep(0.02).await;
        }
        assert_eq!(unauthorized_count(), MAX_UNAUTHORIZED_CONNS);
        assert!(handshakes.iter().all(|h| !h.is_finished()));

        let (served, _controller, _) = stalled_incoming().await;
        let addr: SocketAddr = "192.0.2.200:1".parse().unwrap();
        let refused = crate::server::create_tcp_connection(
            server.clone(),
            served,
            addr,
            Default::default(),
        )
        .await;
        assert!(refused.is_err());
        let ended = hbb_common::timeout(2000, handshakes.remove(0)).await;
        assert!(
            matches!(ended, Ok(Ok(Err(_)))),
            "the oldest handshake ends on eviction"
        );
        assert_eq!(unauthorized_count(), MAX_UNAUTHORIZED_CONNS - 1);
        assert!(
            handshakes.iter().all(|h| !h.is_finished()),
            "only the oldest was ended"
        );

        drop(controllers);
        for h in handshakes {
            assert!(
                matches!(hbb_common::timeout(3000, h).await, Ok(Ok(Err(_)))),
                "a stalled handshake ends when its controller goes"
            );
        }
        assert_eq!(unauthorized_count(), 0, "no handshake outlives the test");
    }

    // The cap is on before the identity handshake reads. A header declaring one byte over it,
    // written to the wire as the codec would read it, ends the handshake on the header alone -
    // the payload is neither waited for nor read - and releases the place the connection held.
    #[tokio::test]
    async fn test_unauthorized_frame_is_refused_on_its_header() {
        use hbb_common::tokio::io::AsyncWriteExt;
        let _serial = UNAUTHORIZED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let server = crate::server::new_for_test();
        let listener = hbb_common::tcp::new_listener("127.0.0.1:0", false)
            .await
            .unwrap();
        let (controller, accepted) = tokio::join!(
            tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        let (mut controller, (accepted, addr)) = (controller.unwrap(), accepted.unwrap());
        let served = Stream::Tcp(hbb_common::tcp::FramedStream::from(accepted, addr));
        let handshake = tokio::spawn(async move {
            crate::server::create_tcp_connection(server, served, addr, Default::default())
                .await
        });
        let n = MAX_UNAUTHORIZED_MESSAGE + 1;
        controller
            .write_all(&(((n << 2) | 0x3) as u32).to_le_bytes())
            .await
            .unwrap();
        match hbb_common::timeout(2000, handshake).await {
            Ok(Ok(Err(e))) => assert!(e.to_string().contains("Too big packet"), "{}", e),
            Ok(Ok(Ok(_))) => panic!("a frame over the cap was accepted"),
            Ok(Err(e)) => panic!("the handshake task panicked: {}", e),
            Err(_) => panic!("the handshake waited for a payload the header should have refused"),
        }
        assert_eq!(unauthorized_count(), 0);
    }

    #[test]
    fn login_scope_latches_session_scope_across_login_retries() {
        let first = LoginRequest {
            my_id: "peer".to_owned(),
            ..Default::default()
        };
        let scope = |lr: &LoginRequest| Connection::login_scope_digest(lr);

        // A retry may carry new credentials, profile data, options, and unknown fields.
        let mut retry = first.clone();
        retry.password = "secret".into();
        retry.hwid = "hwid".into();
        retry.os_login = Some(OSLogin {
            username: "admin".to_owned(),
            ..Default::default()
        })
        .into();
        retry.my_name = "New Display Name".to_owned();
        retry.avatar = "data:image/png;base64,AAAA".to_owned();
        retry
            .special_fields
            .mut_unknown_fields()
            .add_varint(9999, 1);
        assert_eq!(scope(&first), scope(&retry));

        // It may not change the controller identity.
        let mut rotated_id = first.clone();
        rotated_id.my_id = "rotated-id".to_owned();
        assert_ne!(scope(&first), scope(&rotated_id));
    }

    #[test]
    fn test_wildcard_match() {
        // Exact match.
        assert!(wildcard_match("123456789", "123456789"));
        assert!(!wildcard_match("123456789", "123456780"));
        assert!(!wildcard_match("12345678", "123456789"));
        assert!(!wildcard_match("123456789", "12345678"));
        // Case-insensitive.
        assert!(wildcard_match("MyCustomId", "mycustomid"));
        // '*' matches any sequence.
        assert!(wildcard_match("*", "123456789"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "*abc"));
        assert!(wildcard_match("123*", "123456789"));
        assert!(wildcard_match("123*", "123"));
        assert!(wildcard_match("12*", "12*9"));
        assert!(!wildcard_match("123*", "124456789"));
        assert!(wildcard_match("*789", "123456789"));
        assert!(wildcard_match("1*9", "123456789"));
        assert!(wildcard_match("1*4*9", "123456789"));
        assert!(!wildcard_match("1*4*9", "123456780"));
        assert!(wildcard_match("*456*", "123456789"));
        // '?' matches exactly one character.
        assert!(wildcard_match("12345678?", "123456789"));
        assert!(!wildcard_match("123456789?", "123456789"));
        assert!(wildcard_match("???456???", "123456789"));
        assert!(wildcard_match("1?3*7?9", "123456789"));
        // Whitespace around entries is ignored.
        assert!(wildcard_match(" 123456789 ", "123456789"));
    }

    #[test]
    fn test_id_whitelist_allows() {
        let list = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // An empty whitelist allows everyone.
        assert!(id_whitelist_allows(&[], "123456789"));

        // Same server: the peer reports a bare id.
        assert!(id_whitelist_allows(&list(&["123456789"]), "123456789"));
        assert!(!id_whitelist_allows(&list(&["123456789"]), "987654321"));

        // Cross server: the peer appends its own server, which must not reject it.
        assert!(id_whitelist_allows(
            &list(&["123456789"]),
            "123456789@example.com:21116"
        ));
        // Cross server from web, whose server is a WebSocket URI.
        assert!(id_whitelist_allows(
            &list(&["123456789"]),
            "123456789@wss://example.com:21118/ws/id"
        ));
        // A different id is still rejected, suffix or not.
        assert!(!id_whitelist_allows(
            &list(&["123456789"]),
            "987654321@example.com:21116"
        ));

        // An entry pinned to one server keeps matching that exact form.
        assert!(id_whitelist_allows(
            &list(&["123456789@example.com:21116"]),
            "123456789@example.com:21116"
        ));
        assert!(!id_whitelist_allows(
            &list(&["123456789@example.com:21116"]),
            "123456789@other.com:21116"
        ));
        // ... and no longer matches the bare id, which is the point of pinning.
        assert!(!id_whitelist_allows(
            &list(&["123456789@example.com:21116"]),
            "123456789"
        ));

        // Wildcards keep working on both forms.
        assert!(id_whitelist_allows(&list(&["abc*"]), "abcdef"));
        assert!(id_whitelist_allows(
            &list(&["abc*"]),
            "abcdef@example.com:21116"
        ));
        assert!(id_whitelist_allows(
            &list(&["*"]),
            "123456789@example.com:21116"
        ));

        // Any entry of the list is enough.
        assert!(id_whitelist_allows(
            &list(&["111111111", "123456789", "222222222"]),
            "123456789@example.com:21116"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retina() {
        let mut retina = Retina {
            displays: vec![DisplayInfo {
                x: 10,
                y: 10,
                width: 1000,
                height: 1000,
                scale: 2.0,
                ..Default::default()
            }],
        };
        let mut mouse: MouseEvent = MouseEvent {
            x: 510,
            y: 510,
            ..Default::default()
        };
        retina.on_mouse_event(&mut mouse, 0);
        assert_eq!(mouse.x, 260);
        assert_eq!(mouse.y, 260);
        let pos = CursorPosition {
            x: 260,
            y: 260,
            ..Default::default()
        };
        let msg = retina.on_cursor_pos(&pos, 0).unwrap();
        let pos = msg.cursor_position();
        assert_eq!(pos.x, 510);
        assert_eq!(pos.y, 510);
    }

    fn msg(set: impl FnOnce(&mut Message)) -> Message {
        let mut msg = Message::new();
        set(&mut msg);
        msg
    }

    #[test]
    fn only_desktop_login_requests_are_supported() {
        assert!(Connection::supports_login_request(&LoginRequest::new()));
        let mut login = LoginRequest::new();
        login.set_file_transfer(FileTransfer::new());
        assert!(!Connection::supports_login_request(&login));
        login.set_port_forward(PortForward::new());
        assert!(!Connection::supports_login_request(&login));
        login.set_view_camera(ViewCamera::new());
        assert!(!Connection::supports_login_request(&login));
        login.set_terminal(Terminal::new());
        assert!(!Connection::supports_login_request(&login));
    }

    #[test]
    fn retired_wire_login_payloads_remain_rejected() {
        let payloads: &[&[u8]] = &[
            &[0x3a, 3, 0x0a, 1, b'/'],
            &[0x42, 3, 0x0a, 1, b'x'],
            &[0x7a, 0],
            &[0x82, 1, 3, 0x0a, 1, b'x'],
        ];
        for payload in payloads {
            let login: LoginRequest = hbb_common::protobuf::Message::parse_from_bytes(payload)
                .expect("legacy login payload must decode as a retired session kind");
            assert!(!Connection::supports_login_request(&login));
        }
    }

    #[test]
    fn desktop_clipboard_accepts_only_text_formats() {
        let text = Clipboard::new();
        let mut non_text = Clipboard::new();
        non_text.format = hbb_common::protobuf::EnumOrUnknown::from_i32(999);
        let scope = |message: &Message| {
            Connection::authorized_message_scope_violation(AuthConnType::Remote, message)
        };
        assert_eq!(scope(&msg(|m| m.set_clipboard(text.clone()))), None);
        assert_eq!(
            scope(&msg(|m| m.set_clipboard(non_text.clone()))),
            Some("clipboard.format")
        );
        assert_eq!(
            scope(&msg(|m| m.set_multi_clipboards(MultiClipboards {
                clipboards: vec![text, non_text],
                ..Default::default()
            }))),
            Some("clipboard.format")
        );
    }

    #[test]
    fn only_a_newer_remote_control_of_the_same_session_keeps_the_screen_unlocked() {
        let replaced_by = super::raii::AuthedConnID::is_newer_session_remote;

        let key = |session_id, peer: &str| SessionKey {
            peer_id: peer.to_owned(),
            name: "".to_owned(),
            session_id,
        };
        let conn = |conn_id, conn_type, session_key| AuthedConn {
            conn_id,
            conn_type,
            session_key,
            sender: mpsc::unbounded_channel().0,
        };
        let mine = key(7, "peer");
        let remote = AuthConnType::Remote;

        assert!(replaced_by(&conn(3, remote, mine.clone()), 2, &mine));
        // An older one, and itself: of connections ending at once only the last still locks.
        assert!(!replaced_by(&conn(1, remote, mine.clone()), 2, &mine));
        assert!(!replaced_by(&conn(2, remote, mine.clone()), 2, &mine));
        // Another session of this peer, and another peer on the same session id: `SessionKey`
        // is all three fields, and either of those is someone else's screen to lock.
        assert!(!replaced_by(&conn(3, remote, key(8, "peer")), 2, &mine));
        assert!(!replaced_by(&conn(3, remote, key(7, "other")), 2, &mine));
    }
}
