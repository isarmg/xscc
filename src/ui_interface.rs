use base::config::keys::{self, *};
#[cfg(any(target_os = "android", target_os = "ios"))]
use hbb_common::password_security;
use hbb_common::{
    allow_err,
    bytes::Bytes,
    config::{self, Config, LocalConfig, PeerConfig},
    log,
    tokio,
};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use hbb_common::{
    sleep,
    tokio::{sync::mpsc, time},
};
use serde_derive::Serialize;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::process::Child;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::common::SOFTWARE_UPDATE_URL;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::ipc;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub type Children = Arc<Mutex<(bool, HashMap<(String, String), Child>)>>;

#[derive(Clone, Debug, Serialize)]
pub struct UiStatus {
    pub status_num: i32,
    #[cfg(not(feature = "flutter"))]
    pub key_confirmed: bool,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub mouse_time: i64,
    #[cfg(not(feature = "flutter"))]
    pub id: String,
    #[cfg(feature = "flutter")]
    pub video_conn_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginDeviceInfo {
    pub os: String,
    pub r#type: String,
    pub name: String,
}

lazy_static::lazy_static! {
    static ref UI_STATUS : Arc<Mutex<UiStatus>> = Arc::new(Mutex::new(UiStatus{
        status_num: 0,
        #[cfg(not(feature = "flutter"))]
        key_confirmed: false,
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        mouse_time: 0,
        #[cfg(not(feature = "flutter"))]
        id: "".to_owned(),
        #[cfg(feature = "flutter")]
        video_conn_count: 0,
    }));
    static ref TEMPORARY_PASSWD : Arc<Mutex<String>> = Arc::new(Mutex::new("".to_owned()));
    static ref IS_REMOTE_MODIFY_ENABLED_BY_CONTROL_PERMISSIONS : Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
lazy_static::lazy_static! {
    static ref OPTION_SYNCED: Arc<Mutex<bool>> = Default::default();
    static ref OPTIONS : Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(Config::get_options()));
    pub static ref SENDER : Mutex<mpsc::UnboundedSender<ipc::Data>> = Mutex::new(check_connect_status(true));
    static ref CHILDREN : Children = Default::default();
}



#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn get_id() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Config::get_id();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return ipc::get_id();
}

pub fn get_local_address() -> String {
    use std::net::{IpAddr, SocketAddr};

    let port = get_option(OPTION_DIRECT_ACCESS_PORT)
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .unwrap_or(21118);
    let interfaces = default_net::get_default_interface()
        .ok()
        .into_iter()
        .chain(default_net::get_interfaces());
    for interface in interfaces {
        if let Some(address) = interface.ipv4.iter().map(|network| network.addr)
            .find(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local())
        {
            return SocketAddr::new(IpAddr::V4(address), port).to_string();
        }
        if let Some(address) = interface.ipv6.iter().map(|network| network.addr)
            .find(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast()
                && ip.segments()[0] & 0xffc0 != 0xfe80)
        {
            return SocketAddr::new(IpAddr::V6(address), port).to_string();
        }
    }
    String::new()
}

#[inline]
pub fn goto_install() {
    allow_err!(crate::run_me(vec!["--install"]));
    std::process::exit(0);
}

#[inline]
pub fn install_me(_options: String, _path: String, _silent: bool, _debug: bool) {
    #[cfg(windows)]
    std::thread::spawn(move || {
        allow_err!(crate::platform::windows::install_me(
            &_options, _path, _silent, _debug
        ));
        std::process::exit(0);
    });
}

#[inline]
pub fn update_me(_path: String) {
    goto_install();
}

#[inline]
pub fn run_without_install() {
    crate::run_me(vec!["--noinstall"]).ok();
    std::process::exit(0);
}

#[inline]
pub fn show_run_without_install() -> bool {
    let mut it = std::env::args();
    if let Some(tmp) = it.next() {
        if crate::is_setup(&tmp) {
            return it.next() == None;
        }
    }
    false
}

#[inline]
pub fn refresh_options() {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        *OPTIONS.lock().unwrap() = Config::get_options();
    }
}

#[inline]
pub fn get_option<T: AsRef<str>>(key: T) -> String {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let map = OPTIONS.lock().unwrap();
        if let Some(v) = map.get(key.as_ref()) {
            v.to_owned()
        } else {
            "".to_owned()
        }
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Config::get_option(key.as_ref())
    }
}

#[inline]
pub fn use_texture_render() -> bool {
    #[cfg(target_os = "android")]
    return false;
    #[cfg(target_os = "ios")]
    return false;

    #[cfg(target_os = "macos")]
    return cfg!(feature = "flutter")
        && LocalConfig::get_option(keys::OPTION_TEXTURE_RENDER) == "Y";

    #[cfg(target_os = "linux")]
    return cfg!(feature = "flutter")
        && LocalConfig::get_option(keys::OPTION_TEXTURE_RENDER) != "N";

    #[cfg(target_os = "windows")]
    {
        if !cfg!(feature = "flutter") {
            return false;
        }
        LocalConfig::get_option(keys::OPTION_TEXTURE_RENDER) != "N"
    }
}

#[inline]
pub fn is_option_fixed(key: &str) -> bool {
    config::OVERWRITE_DISPLAY_SETTINGS
        .read()
        .unwrap()
        .contains_key(key)
        || config::OVERWRITE_LOCAL_SETTINGS
            .read()
            .unwrap()
            .contains_key(key)
        || config::OVERWRITE_SETTINGS.read().unwrap().contains_key(key)
}

#[inline]
pub fn get_local_option(key: String) -> String {
    crate::get_local_option(&key)
}

#[inline]
#[cfg(feature = "flutter")]
pub fn get_hard_option(key: String) -> String {
    config::HARD_SETTINGS
        .read()
        .unwrap()
        .get(&key)
        .cloned()
        .unwrap_or_default()
}

#[inline]
pub fn get_builtin_option(key: &str) -> String {
    crate::get_builtin_option(key)
}

#[inline]
pub fn set_local_option(key: String, value: String) {
    LocalConfig::set_option(key.clone(), value);
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn get_local_flutter_option(key: String) -> String {
    LocalConfig::get_flutter_option(&key)
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn set_local_flutter_option(key: String, value: String) {
    LocalConfig::set_flutter_option(key, value);
}

#[cfg(feature = "flutter")]
#[inline]
pub fn get_kb_layout_type() -> String {
    LocalConfig::get_kb_layout_type()
}

#[cfg(feature = "flutter")]
#[inline]
pub fn set_kb_layout_type(kb_layout_type: String) {
    LocalConfig::set_kb_layout_type(kb_layout_type);
}

#[inline]
pub fn peer_has_password(id: String) -> bool {
    !PeerConfig::load(&id).password.is_empty()
}

#[inline]
pub fn forget_password(id: String) {
    let mut c = PeerConfig::load(&id);
    c.password.clear();
    c.store(&id);
}

#[inline]
pub fn get_peer_option(id: String, name: String) -> String {
    let c = PeerConfig::load(&id);
    c.options.get(&name).unwrap_or(&"".to_owned()).to_owned()
}

#[inline]
#[cfg(feature = "flutter")]
pub fn get_peer_flutter_option(id: String, name: String) -> String {
    let c = PeerConfig::load(&id);
    c.ui_flutter.get(&name).unwrap_or(&"".to_owned()).to_owned()
}

#[inline]
#[cfg(feature = "flutter")]
pub fn set_peer_flutter_option(id: String, name: String, value: String) {
    let mut c = PeerConfig::load(&id);
    if value.is_empty() {
        c.ui_flutter.remove(&name);
    } else {
        c.ui_flutter.insert(name, value);
    }
    c.store(&id);
}

#[inline]
pub fn set_peer_option(id: String, name: String, value: String) {
    let mut c = PeerConfig::load(&id);
    if value.is_empty() {
        c.options.remove(&name);
    } else {
        c.options.insert(name, value);
    }
    c.store(&id);
}

#[inline]
pub fn get_options() -> String {
    let options = {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            OPTIONS.lock().unwrap()
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            Config::get_options()
        }
    };
    let mut m = serde_json::Map::new();
    for (k, v) in options.iter() {
        m.insert(k.into(), v.to_owned().into());
    }
    serde_json::to_string(&m).unwrap_or_default()
}

#[inline]
#[cfg(feature = "flutter")]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn get_sound_inputs() -> Vec<String> {
    let mut a = Vec::new();
    #[cfg(not(target_os = "linux"))]
    {
        fn get_sound_inputs_() -> Vec<String> {
            let mut out = Vec::new();
            use cpal::traits::{DeviceTrait, HostTrait};
            // Do not use `cpal::host_from_id(cpal::HostId::ScreenCaptureKit)` for feature = "screencapturekit"
            // Because we explicitly handle the "System Sound" device.
            let host = cpal::default_host();
            if let Ok(devices) = host.devices() {
                for device in devices {
                    if device.default_input_config().is_err() {
                        continue;
                    }
                    if let Ok(name) = device.name() {
                        out.push(name);
                    }
                }
            }
            out
        }

        let inputs = Arc::new(Mutex::new(Vec::new()));
        let cloned = inputs.clone();
        // can not call below in UI thread, because conflict with sciter sound com initialization
        std::thread::spawn(move || *cloned.lock().unwrap() = get_sound_inputs_())
            .join()
            .ok();
        for name in inputs.lock().unwrap().drain(..) {
            a.push(name);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let inputs: Vec<String> = crate::platform::linux::get_pa_sources()
            .drain(..)
            .map(|x| x.1)
            .collect();

        for name in inputs {
            a.push(name);
        }
    }
    a
}

#[inline]
pub fn set_options(m: HashMap<String, String>) {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        *OPTIONS.lock().unwrap() = m.clone();
        ipc::set_options(m).ok();
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    Config::set_options(m);
}

#[inline]
pub fn set_option(key: String, value: String) {
    if &key == "stop-service" {
        #[cfg(target_os = "macos")]
        {
            let is_stop = value == "Y";
            if is_stop && crate::platform::uninstall_service(true, false) {
                return;
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            if crate::platform::is_installed() {
                if value == "Y" {
                    if crate::platform::uninstall_service(true, false) {
                        return;
                    }
                } else {
                    if crate::platform::install_service() {
                        return;
                    }
                }
                return;
            }
        }
    } else if &key == "audio-input" {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        crate::audio_service::restart();
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let mut options = OPTIONS.lock().unwrap();
        if value.is_empty() {
            options.remove(&key);
        } else {
            options.insert(key.clone(), value.clone());
        }
        ipc::set_options(options.clone()).ok();
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Config::set_option(key, value);
    }
}

#[inline]
pub fn install_path() -> String {
    #[cfg(windows)]
    return crate::platform::windows::get_install_info().1;
    #[cfg(not(windows))]
    return "".to_owned();
}

#[inline]
pub fn install_options() -> String {
    #[cfg(windows)]
    return crate::platform::windows::get_install_options();
    #[cfg(not(windows))]
    return "{}".to_owned();
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[inline]
pub fn is_installed() -> bool {
    crate::platform::is_installed()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[inline]
pub fn is_installed() -> bool {
    false
}

#[inline]
pub fn is_installed_lower_version() -> bool {
    #[cfg(not(windows))]
    return false;
    #[cfg(windows)]
    {
        let b = crate::platform::windows::get_reg("BuildDate");
        return crate::BUILD_DATE.cmp(&b).is_gt();
    }
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn get_mouse_time() -> f64 {
    UI_STATUS.lock().unwrap().mouse_time as f64
}

#[inline]
pub fn check_mouse_time() {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let sender = SENDER.lock().unwrap();
        allow_err!(sender.send(ipc::Data::MouseMoveTime(0)));
    }
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn get_connect_status() -> UiStatus {
    UI_STATUS.lock().unwrap().clone()
}

#[inline]
pub fn temporary_password() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return password_security::temporary_password();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return TEMPORARY_PASSWD.lock().unwrap().clone();
}

#[inline]
pub fn update_temporary_password() {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    password_security::update_temporary_password();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    allow_err!(ipc::update_temporary_password());
}

#[inline]
pub fn is_permanent_password_set() -> bool {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Config::has_permanent_password();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let daemon_is_set = ipc::is_permanent_password_set();
        // `daemon_is_set` is authoritative for the return value. Local storage is only used to
        // decide whether we should attempt a sync to clear stale user-side state.
        let local_storage_is_empty = if daemon_is_set {
            true
        } else {
            let (storage, _) = Config::get_local_permanent_password_storage_and_salt();
            storage.is_empty()
        };
        if daemon_is_set || !local_storage_is_empty {
            allow_err!(ipc::sync_permanent_password_storage_from_daemon());
        }
        daemon_is_set
    }
}

#[inline]
pub fn is_local_permanent_password_set() -> bool {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Config::has_local_permanent_password();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        allow_err!(ipc::sync_permanent_password_storage_from_daemon());
        Config::has_local_permanent_password()
    }
}

pub fn set_permanent_password_with_result(password: String) -> bool {
    if config::Config::is_disable_change_permanent_password() {
        return false;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        return config::Config::set_permanent_password(&password);
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        match crate::ipc::set_permanent_password_with_ack(password) {
            Ok(ok) => ok,
            Err(err) => {
                log::warn!("Failed to set permanent password via IPC: {err}");
                false
            }
        }
    }
}

#[inline]
pub fn get_peer(id: String) -> PeerConfig {
    PeerConfig::load(&id)
}

#[inline]
pub fn get_fav() -> Vec<String> {
    LocalConfig::get_fav()
}

#[inline]
pub fn store_fav(fav: Vec<String>) {
    LocalConfig::set_fav(fav);
}

#[inline]
pub fn is_process_trusted(_prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::is_process_trusted(_prompt);
    #[cfg(not(target_os = "macos"))]
    return true;
}

#[inline]
pub fn is_can_screen_recording(_prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::is_can_screen_recording(_prompt);
    #[cfg(not(target_os = "macos"))]
    return true;
}

#[inline]
pub fn is_installed_daemon(_prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::is_installed_daemon(_prompt);
    #[cfg(not(target_os = "macos"))]
    return true;
}

#[inline]
#[cfg(feature = "flutter")]
pub fn is_can_input_monitoring(_prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::is_can_input_monitoring(_prompt);
    #[cfg(not(target_os = "macos"))]
    return true;
}

#[inline]
pub fn get_error() -> String {
    #[cfg(target_os = "linux")]
    {
        let dtype = crate::platform::linux::get_display_server();
        if crate::platform::linux::DISPLAY_SERVER_WAYLAND == dtype {
            return crate::server::wayland::common_get_error();
        }
        if dtype != crate::platform::linux::DISPLAY_SERVER_WAYLAND {
            return format!(
                "{} {}, {}",
                crate::client::translate("Unsupported display server".to_owned()),
                dtype,
                "wayland expected",
            );
        }
    }
    return "".to_owned();
}

#[inline]
pub fn is_login_wayland() -> bool {
    #[cfg(target_os = "linux")]
    return crate::platform::linux::is_login_wayland();
    #[cfg(not(target_os = "linux"))]
    return false;
}

#[inline]
pub fn current_is_wayland() -> bool {
    #[cfg(target_os = "linux")]
    return crate::platform::linux::current_is_wayland();
    #[cfg(not(target_os = "linux"))]
    return false;
}

#[inline]
pub fn get_new_version() -> String {
    SOFTWARE_UPDATE_URL
        .lock()
        .unwrap()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

#[inline]
pub fn get_version() -> String {
    crate::VERSION.to_owned()
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn get_app_name() -> String {
    crate::get_app_name()
}

#[cfg(windows)]
#[inline]
pub fn create_shortcut(_id: String) {
    crate::platform::windows::create_shortcut(&_id).ok();
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn discover() {
    std::thread::spawn(move || {
        allow_err!(crate::lan::discover());
    });
}

#[cfg(feature = "flutter")]
pub fn peer_to_map(id: String, p: PeerConfig) -> HashMap<&'static str, String> {
    use hbb_common::sodiumoxide::base64;
    HashMap::<&str, String>::from_iter([
        ("id", id),
        ("username", p.info.username.clone()),
        ("hostname", p.info.hostname.clone()),
        ("platform", p.info.platform.clone()),
        (
            "alias",
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ),
        (
            "hash",
            base64::encode(p.password, base64::Variant::Original),
        ),
    ])
}

#[cfg(feature = "flutter")]
pub fn peer_exists(id: &str) -> bool {
    PeerConfig::exists(id)
}

#[inline]
pub fn get_lan_peers() -> Vec<HashMap<&'static str, String>> {
    config::LanPeers::load()
        .peers
        .iter()
        .filter_map(|peer| {
            let endpoint = crate::lan::peer_endpoint(peer)?;
            Some(HashMap::<&str, String>::from_iter([
                ("id", endpoint),
                ("username", peer.username.clone()),
                ("hostname", peer.hostname.clone()),
                ("platform", peer.platform.clone()),
                ("online", peer.online.to_string()),
            ]))
        })
        .collect()
}

#[inline]
pub fn remove_discovered(id: String) {
    let mut peers = config::LanPeers::load().peers;
    peers.retain(|peer| {
        peer.id != id && crate::lan::peer_endpoint(peer).as_deref() != Some(id.as_str())
    });
    config::LanPeers::store(&peers);
}

#[inline]
pub fn get_uuid() -> String {
    crate::encode64(hbb_common::get_uuid())
}

#[inline]
pub fn get_langs() -> String {
    use serde_json::json;
    let hide_cjk = crate::lang::cjk_ui_unavailable();
    let mut x: Vec<(&str, String)> = crate::lang::LANGS
        .iter()
        .filter(|a| !hide_cjk || !crate::lang::is_cjk_lang(a.0))
        .map(|a| (a.0, format!("{} ({})", a.1, a.0)))
        .collect();
    x.sort_by(|a, b| a.0.cmp(b.0));
    json!(x).to_string()
}

#[inline]
pub fn get_api_server() -> String {
    get_option("api-server")
}

#[inline]
pub fn has_hwcodec() -> bool {
    // Has real hardware codec using gpu
    (cfg!(feature = "hwcodec") && cfg!(not(target_os = "ios"))) || cfg!(feature = "mediacodec")
}

#[inline]
pub fn has_vram() -> bool {
    cfg!(feature = "vram")
}

#[cfg(feature = "flutter")]
#[inline]
pub fn supported_hwdecodings() -> (bool, bool) {
    let decoding =
        scrap::codec::Decoder::supported_decodings(None, use_texture_render(), None, &vec![]);
    #[allow(unused_mut)]
    let (mut h264, mut h265) = (decoding.ability_h264 > 0, decoding.ability_h265 > 0);
    #[cfg(feature = "vram")]
    {
        // supported_decodings check runtime luid
        let vram = scrap::vram::VRamDecoder::possible_available_without_check();
        if vram.0 {
            h264 = true;
        }
        if vram.1 {
            h265 = true;
        }
    }
    (h264, h265)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[inline]
pub fn is_root() -> bool {
    crate::platform::is_root()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[inline]
pub fn is_root() -> bool {
    false
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn check_super_user_permission() -> bool {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    return crate::platform::check_super_user_permission().unwrap_or(false);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return true;
}

#[cfg(not(any(target_os = "android", target_os = "ios", feature = "flutter")))]
pub fn check_zombie() {
    let mut deads = Vec::new();
    loop {
        let mut lock = CHILDREN.lock().unwrap();
        let mut n = 0;
        for (id, c) in lock.1.iter_mut() {
            if let Ok(Some(_)) = c.try_wait() {
                deads.push(id.clone());
                n += 1;
            }
        }
        for ref id in deads.drain(..) {
            lock.1.remove(id);
        }
        if n > 0 {
            lock.0 = true;
        }
        drop(lock);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios", feature = "flutter")))]
pub fn recent_sessions_updated() -> bool {
    let mut children = CHILDREN.lock().unwrap();
    if children.0 {
        children.0 = false;
        true
    } else {
        false
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", feature = "flutter")))]
pub fn new_remote(id: String, remote_type: String) {
    let mut lock = CHILDREN.lock().unwrap();
    let args = vec![format!("--{}", remote_type), id.clone()];
    let key = (id.clone(), remote_type.clone());
    if let Some(c) = lock.1.get_mut(&key) {
        if let Ok(Some(_)) = c.try_wait() {
            lock.1.remove(&key);
        } else {
            if remote_type == "rdp" {
                allow_err!(c.kill());
                std::thread::sleep(std::time::Duration::from_millis(30));
                c.try_wait().ok();
                lock.1.remove(&key);
            } else {
                return;
            }
        }
    }
    match crate::run_me(args) {
        Ok(child) => {
            lock.1.insert(key, child);
        }
        Err(err) => {
            log::error!("Failed to spawn remote: {}", err);
        }
    }
}

// Make sure `SENDER` is inited here.
#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn start_option_status_sync() {
    let _sender = SENDER.lock().unwrap();
}

// not call directly
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn check_connect_status(reconnect: bool) -> mpsc::UnboundedSender<ipc::Data> {
    let (tx, rx) = mpsc::unbounded_channel::<ipc::Data>();
    std::thread::spawn(move || check_connect_status_(reconnect, rx));
    tx
}

#[cfg(feature = "flutter")]
pub fn set_user_default_option(key: String, value: String) {
    use hbb_common::config::UserDefaultConfig;
    UserDefaultConfig::load().set(key, value);
}

#[cfg(feature = "flutter")]
pub fn get_user_default_option(key: String) -> String {
    use hbb_common::config::UserDefaultConfig;
    UserDefaultConfig::load().get(&key)
}

pub fn get_fingerprint() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return crate::common::pk_to_fingerprint(Config::get_key_pair().1);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return ipc::get_fingerprint();
}

#[inline]
pub fn get_login_device_info() -> LoginDeviceInfo {
    LoginDeviceInfo {
        // std::env::consts::OS is better than whoami::platform() here.
        os: std::env::consts::OS.to_owned(),
        r#type: "client".to_owned(),
        name: crate::common::hostname(),
    }
}

#[inline]
pub fn get_login_device_info_json() -> String {
    serde_json::to_string(&get_login_device_info()).unwrap_or("{}".to_string())
}

// notice: avoiding create ipc connection repeatedly,
// because windows named pipe has serious memory leak issue.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tokio::main(flavor = "current_thread")]
async fn check_connect_status_(reconnect: bool, rx: mpsc::UnboundedReceiver<ipc::Data>) {
    #[cfg(not(feature = "flutter"))]
    let mut key_confirmed = false;
    let mut rx = rx;
    let mut mouse_time = 0;
    #[cfg(feature = "flutter")]
    let mut video_conn_count = 0;
    #[cfg(not(feature = "flutter"))]
    let mut id = "".to_owned();
    let is_cm = crate::common::is_cm();

    loop {
        if let Ok(mut c) = ipc::connect(1000, "").await {
            let mut timer = crate::rustdesk_interval(time::interval(time::Duration::from_secs(1)));
            loop {
                tokio::select! {
                    res = c.next() => {
                        match res {
                            Err(err) => {
                                log::error!("ipc connection closed: {}", err);
                                if is_cm {
                                    crate::ui_cm_interface::quit_cm();
                                }
                                break;
                            }
                            #[cfg(not(any(target_os = "android", target_os = "ios")))]
                            Ok(Some(ipc::Data::MouseMoveTime(v))) => {
                                mouse_time = v;
                                UI_STATUS.lock().unwrap().mouse_time = v;
                            }
                            Ok(Some(ipc::Data::Options(Some(v)))) => {
                                *OPTIONS.lock().unwrap() = v;
                                *OPTION_SYNCED.lock().unwrap() = true;
                            }
                            Ok(Some(ipc::Data::Config((name, Some(value))))) => {
                                if name == "id" {
                                    #[cfg(not(feature = "flutter"))]
                                    {
                                        id = value;
                                    }
                                } else if name == "temporary-password" {
                                    *TEMPORARY_PASSWD.lock().unwrap() = value;
                                }
                            }
                            #[cfg(feature = "flutter")]
                            Ok(Some(ipc::Data::VideoConnCount(Some(n)))) => {
                                video_conn_count = n;
                            }
                            Ok(Some(ipc::Data::OnlineStatus(Some((mut x, _c))))) => {
                                if x > 0 {
                                    x = 1
                                }
                                #[cfg(not(feature = "flutter"))]
                                {
                                    key_confirmed = _c;
                                }
                                *UI_STATUS.lock().unwrap() = UiStatus {
                                    status_num: x as _,
                                    #[cfg(not(feature = "flutter"))]
                                    key_confirmed: _c,
                                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                                    mouse_time,
                                    #[cfg(not(feature = "flutter"))]
                                    id: id.clone(),
                                    #[cfg(feature = "flutter")]
                                    video_conn_count,
                                };
                            }
                            Ok(Some(ipc::Data::ControlPermissionsRemoteModify(v))) => {
                                *IS_REMOTE_MODIFY_ENABLED_BY_CONTROL_PERMISSIONS.lock().unwrap() = v;
                            }
                            _ => {}
                        }
                    }
                    Some(data) = rx.recv() => {
                        allow_err!(c.send(&data).await);
                    }
                    _ = timer.tick() => {
                        c.send(&ipc::Data::OnlineStatus(None)).await.ok();
                        c.send(&ipc::Data::Options(None)).await.ok();
                        c.send(&ipc::Data::Config(("id".to_owned(), None))).await.ok();
                        c.send(&ipc::Data::Config(("temporary-password".to_owned(), None))).await.ok();
                        #[cfg(feature = "flutter")]
                        c.send(&ipc::Data::VideoConnCount(None)).await.ok();
                        c.send(&ipc::Data::ControlPermissionsRemoteModify(None)).await.ok();
                    }
                }
            }
        }
        if !reconnect {
            OPTIONS
                .lock()
                .unwrap()
                .insert("ipc-closed".to_owned(), "Y".to_owned());
            break;
        }
        *UI_STATUS.lock().unwrap() = UiStatus {
            status_num: -1,
            #[cfg(not(feature = "flutter"))]
            key_confirmed,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            mouse_time,
            #[cfg(not(feature = "flutter"))]
            id: id.clone(),
            #[cfg(feature = "flutter")]
            video_conn_count,
        };
        sleep(1.).await;
    }
}

#[allow(dead_code)]
pub fn option_synced() -> bool {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        OPTION_SYNCED.lock().unwrap().clone()
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        true
    }
}

#[cfg(any(target_os = "android", feature = "flutter"))]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tokio::main(flavor = "current_thread")]
pub(crate) async fn send_to_cm(data: &ipc::Data) {
    if let Ok(mut c) = ipc::connect(1000, "_cm").await {
        c.send(data).await.ok();
    }
}

pub fn support_remove_wallpaper() -> bool {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    return crate::platform::WallPaperRemover::support();
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    return false;
}

pub fn has_valid_2fa() -> bool {
    let raw = get_option("2fa");
    crate::auth_2fa::get_2fa(Some(raw)).is_some()
}

pub fn generate2fa() -> String {
    crate::auth_2fa::generate2fa()
}

pub fn verify2fa(code: String) -> bool {
    let res = crate::auth_2fa::verify2fa(code);
    if res {
        refresh_options();
    }
    res
}

pub fn has_valid_bot() -> bool {
    crate::auth_2fa::TelegramBot::get().map_or(false, |bot| bot.is_some())
}

pub fn verify_bot(token: String) -> String {
    match crate::auth_2fa::get_chatid_telegram(&token) {
        Err(err) => err.to_string(),
        Ok(None) => {
            "To activate the bot, simply send a message beginning with a forward slash (\"/\") like \"/hello\" to its chat.".to_owned()
        }
        _ => "".to_owned(),
    }
}

pub fn check_hwcodec() {
    #[cfg(all(feature = "hwcodec", not(any(target_os = "android", target_os = "ios"))))]
    std::thread::spawn(crate::client::get_hwcodec_config);

    #[cfg(feature = "hwcodec")]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();

        ONCE.call_once(|| {
            if crate::platform::is_installed() {
                ipc::notify_server_to_check_hwcodec().ok();
                ipc::client_get_hwcodec_config_thread(3);
            } else {
                scrap::hwcodec::start_check_process();
            }
        })
    }
}

#[cfg(feature = "flutter")]
pub fn get_unlock_pin() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return String::default();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return ipc::get_unlock_pin();
}

#[cfg(feature = "flutter")]
pub fn set_unlock_pin(pin: String) -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return String::default();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    match ipc::set_unlock_pin(pin, true) {
        Ok(_) => String::default(),
        Err(err) => err.to_string(),
    }
}

#[cfg(feature = "flutter")]
pub fn get_trusted_devices() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return Config::get_trusted_devices_json();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return ipc::get_trusted_devices();
}

#[cfg(feature = "flutter")]
pub fn remove_trusted_devices(json: &str) {
    let hwids = serde_json::from_str::<Vec<Bytes>>(json).unwrap_or_default();
    #[cfg(any(target_os = "android", target_os = "ios"))]
    Config::remove_trusted_devices(&hwids);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    ipc::remove_trusted_devices(hwids);
}

#[cfg(feature = "flutter")]
pub fn clear_trusted_devices() {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    Config::clear_trusted_devices();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    ipc::clear_trusted_devices();
}

#[cfg(feature = "flutter")]
pub fn max_encrypt_len() -> usize {
    hbb_common::config::ENCRYPT_MAX_LEN
}

pub fn is_remote_modify_enabled_by_control_permissions() -> Option<bool> {
    *IS_REMOTE_MODIFY_ENABLED_BY_CONTROL_PERMISSIONS
        .lock()
        .unwrap()
}
