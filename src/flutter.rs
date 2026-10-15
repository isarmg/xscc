use crate::{
    client::*,
    flutter_ffi::{EventToUI, SessionID},
    ui_session_interface::{io_loop, InvokeUiSession, Session},
};
use flutter_rust_bridge::StreamSink;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use hbb_common::dlopen::{
    symbor::{Library, Symbol},
    Error as LibError,
};
use hbb_common::{
    anyhow::anyhow, bail, config::LocalConfig, get_version_number, log,
    rendezvous_proto::ConnType, ResultType,
};
use base::message_proto::*;
use serde::Serialize;
use serde_json::json;
#[cfg(target_os = "windows")]
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    os::raw::{c_char, c_int, c_void},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, RwLock,
    },
};

/// tag "main" for [Desktop Main Page] and [Mobile (Client and Server)] (the mobile don't need multiple windows, only one global event stream is needed)
/// tag "cm" only for [Desktop CM Page]
pub(crate) const APP_TYPE_MAIN: &str = "main";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) const APP_TYPE_CM: &str = "cm";
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) const APP_TYPE_CM: &str = "main";

#[cfg(target_os = "macos")]
const CORE_MAIN_EXIT_SUCCESS: i32 = 0;
#[cfg(target_os = "macos")]
const CORE_MAIN_EXIT_FAILURE: i32 = 1;

// Do not remove the following constants.
// Uncomment them when they are used.
// pub(crate) const APP_TYPE_DESKTOP_REMOTE: &str = "remote";
// pub(crate) const APP_TYPE_DESKTOP_PORT_FORWARD: &str = "port forward";

pub type FlutterSession = Arc<Session<FlutterHandler>>;

lazy_static::lazy_static! {
    pub(crate) static ref CUR_SESSION_ID: RwLock<SessionID> = Default::default(); // For desktop only
    static ref GLOBAL_EVENT_STREAM: RwLock<HashMap<String, StreamSink<String>>> = Default::default(); // rust to dart event channel
}

#[cfg(target_os = "windows")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = load_plugin_in_app_path("texture_rgba_renderer_plugin.dll");
}

#[cfg(target_os = "linux")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = Library::open("libtexture_rgba_renderer_plugin.so");
}

#[cfg(target_os = "macos")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_RGBA_RENDERER_PLUGIN: Result<Library, LibError> = Library::open_self();
}

#[cfg(target_os = "windows")]
lazy_static::lazy_static! {
    pub static ref TEXTURE_GPU_RENDERER_PLUGIN: Result<Library, LibError> = load_plugin_in_app_path("flutter_gpu_texture_renderer_plugin.dll");
}

// Move this function into `src/platform/windows.rs` if there're more calls to load plugins.
// Load dll with full path.
#[cfg(target_os = "windows")]
fn load_plugin_in_app_path(dll_name: &str) -> Result<Library, LibError> {
    match std::env::current_exe() {
        Ok(exe_file) => {
            if let Some(cur_dir) = exe_file.parent() {
                let full_path = cur_dir.join(dll_name);
                if !full_path.exists() {
                    Err(LibError::OpeningLibraryError(IoError::new(
                        IoErrorKind::NotFound,
                        format!("{} not found", dll_name),
                    )))
                } else {
                    Library::open(full_path)
                }
            } else {
                Err(LibError::OpeningLibraryError(IoError::new(
                    IoErrorKind::Other,
                    format!(
                        "Invalid exe parent for {}",
                        exe_file.to_string_lossy().as_ref()
                    ),
                )))
            }
        }
        Err(e) => Err(LibError::OpeningLibraryError(e)),
    }
}

/// FFI for rustdesk core's main entry.
/// Return true if the app should continue running with UI(possibly Flutter), false if the app should exit.
/// On macOS, command-only paths exit here because the Swift caller ignores
/// the return value; update failures must therefore use a non-zero exit status.
#[cfg(not(windows))]
#[no_mangle]
pub extern "C" fn rustdesk_core_main() -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use hbb_common::libc;

        // Native runners bypass Rust's startup, which normally ignores SIGPIPE.
        if unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) } == libc::SIG_ERR {
            eprintln!("Failed to ignore SIGPIPE: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    match crate::core_main::core_main() {
        Some(crate::core_main::CoreMainAction::StartUi(_)) => return true,
        Some(crate::core_main::CoreMainAction::ExitFailure(err)) => {
            eprintln!("Command failed: {err}");
            log::error!("Command failed: {err}");
            #[cfg(target_os = "macos")]
            std::process::exit(CORE_MAIN_EXIT_FAILURE);
        }
        None => {
            #[cfg(target_os = "macos")]
            std::process::exit(CORE_MAIN_EXIT_SUCCESS);
        }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub extern "C" fn handle_applicationShouldOpenUntitledFile() {
    crate::platform::macos::handle_application_should_open_untitled_file();
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rustdesk_core_main_args(args_len: *mut c_int) -> *mut *mut c_char {
    unsafe { std::ptr::write(args_len, 0) };
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        match crate::core_main::core_main() {
            Some(crate::core_main::CoreMainAction::StartUi(args)) => {
                return rust_args_to_c_args(args, args_len);
            }
            Some(crate::core_main::CoreMainAction::ExitFailure(err)) => {
                log::error!("Command failed: {err}");
            }
            None => {}
        }
        return std::ptr::null_mut() as _;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return std::ptr::null_mut() as _;
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rustdesk_is_disable_installation() -> c_int {
    hbb_common::config::is_disable_installation() as c_int
}

// https://gist.github.com/iskakaushik/1c5b8aa75c77479c33c4320913eebef6
#[cfg(windows)]
fn rust_args_to_c_args(args: Vec<String>, outlen: *mut c_int) -> *mut *mut c_char {
    let mut v = vec![];

    // Let's fill a vector with null-terminated strings
    for s in args {
        match CString::new(s) {
            Ok(s) => v.push(s),
            Err(_) => return std::ptr::null_mut() as _,
        }
    }

    // Turning each null-terminated string into a pointer.
    // `into_raw` takes ownershop, gives us the pointer and does NOT drop the data.
    let mut out = v.into_iter().map(|s| s.into_raw()).collect::<Vec<_>>();

    // Make sure we're not wasting space.
    out.shrink_to_fit();
    debug_assert!(out.len() == out.capacity());

    // Get the pointer to our vector.
    let len = out.len();
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);

    // Let's write back the length the caller can expect
    unsafe { std::ptr::write(outlen, len as c_int) };

    // Finally return the data
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn free_c_args(ptr: *mut *mut c_char, len: c_int) {
    let len = len as usize;

    // Get back our vector.
    // Previously we shrank to fit, so capacity == length.
    let v = Vec::from_raw_parts(ptr, len, len);

    // Now drop one string at a time.
    for elem in v {
        let s = CString::from_raw(elem);
        std::mem::drop(s);
    }

    // Afterwards the vector will be dropped and thus freed.
}

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn get_rustdesk_app_name(buffer: *mut u16, length: i32) -> i32 {
    let name = crate::platform::wide_string(&crate::get_app_name());
    if length > name.len() as i32 {
        std::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len());
        return 0;
    }
    -1
}

#[derive(Default)]
struct SessionHandler {
    event_stream: Option<StreamSink<EventToUI>>,
    // displays of current session.
    // We need this variable to check if the display is in use before pushing rgba to flutter.
    displays: Vec<usize>,
    renderer: VideoRenderer,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum RenderType {
    PixelBuffer,
    #[cfg(feature = "vram")]
    Texture,
}

#[derive(Clone)]
pub struct FlutterHandler {
    // ui session id -> display handler data
    session_handlers: Arc<RwLock<HashMap<SessionID, SessionHandler>>>,
    display_rgbas: Arc<RwLock<HashMap<usize, RgbaData>>>,
    peer_info: Arc<RwLock<PeerInfo>>,
    use_texture_render: Arc<AtomicBool>,
}

impl Default for FlutterHandler {
    fn default() -> Self {
        Self {
            session_handlers: Default::default(),
            display_rgbas: Default::default(),
            peer_info: Default::default(),
            use_texture_render: Arc::new(
                AtomicBool::new(crate::ui_interface::use_texture_render()),
            ),
        }
    }
}

#[derive(Default, Clone)]
struct RgbaData {
    // SAFETY: [rgba] is guarded by [rgba_valid], and it's safe to reach [rgba] with `rgba_valid == true`.
    // We must check the `rgba_valid` before reading [rgba].
    data: Vec<u8>,
    valid: bool,
}

pub type FlutterRgbaRendererPluginOnRgba = unsafe extern "C" fn(
    texture_rgba: *mut c_void,
    buffer: *const u8,
    len: c_int,
    width: c_int,
    height: c_int,
    dst_rgba_stride: c_int,
);

#[cfg(feature = "vram")]
pub type FlutterGpuTextureRendererPluginCApiSetTexture =
    unsafe extern "C" fn(output: *mut c_void, texture: *mut c_void);

#[cfg(feature = "vram")]
pub type FlutterGpuTextureRendererPluginCApiGetAdapterLuid = unsafe extern "C" fn() -> i64;

pub(super) type TextureRgbaPtr = usize;

struct DisplaySessionInfo {
    // TextureRgba pointer in flutter native.
    texture_rgba_ptr: TextureRgbaPtr,
    size: (usize, usize),
    #[cfg(feature = "vram")]
    gpu_output_ptr: usize,
    notify_render_type: Option<RenderType>,
}

// Video Texture Renderer in Flutter
#[derive(Clone)]
struct VideoRenderer {
    is_support_multi_ui_session: bool,
    map_display_sessions: Arc<RwLock<HashMap<usize, DisplaySessionInfo>>>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    on_rgba_func: Option<Symbol<'static, FlutterRgbaRendererPluginOnRgba>>,
    #[cfg(feature = "vram")]
    on_texture_func: Option<Symbol<'static, FlutterGpuTextureRendererPluginCApiSetTexture>>,
}

impl Default for VideoRenderer {
    fn default() -> Self {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let on_rgba_func = match &*TEXTURE_RGBA_RENDERER_PLUGIN {
            Ok(lib) => {
                let find_sym_res = unsafe {
                    lib.symbol::<FlutterRgbaRendererPluginOnRgba>("FlutterRgbaRendererPluginOnRgba")
                };
                match find_sym_res {
                    Ok(sym) => Some(sym),
                    Err(e) => {
                        log::error!("Failed to find symbol FlutterRgbaRendererPluginOnRgba, {e}");
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to load texture rgba renderer plugin, {e}");
                None
            }
        };
        #[cfg(feature = "vram")]
        let on_texture_func = match &*TEXTURE_GPU_RENDERER_PLUGIN {
            Ok(lib) => {
                let find_sym_res = unsafe {
                    lib.symbol::<FlutterGpuTextureRendererPluginCApiSetTexture>(
                        "FlutterGpuTextureRendererPluginCApiSetTexture",
                    )
                };
                match find_sym_res {
                    Ok(sym) => Some(sym),
                    Err(e) => {
                        log::error!("Failed to find symbol FlutterGpuTextureRendererPluginCApiSetTexture, {e}");
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to load texture gpu renderer plugin, {e}");
                None
            }
        };

        Self {
            map_display_sessions: Default::default(),
            is_support_multi_ui_session: false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            on_rgba_func,
            #[cfg(feature = "vram")]
            on_texture_func,
        }
    }
}

impl VideoRenderer {
    #[inline]
    fn set_size(&mut self, display: usize, width: usize, height: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if let Some(info) = sessions_lock.get_mut(&display) {
            info.size = (width, height);
            info.notify_render_type = None;
        } else {
            sessions_lock.insert(
                display,
                DisplaySessionInfo {
                    texture_rgba_ptr: usize::default(),
                    size: (width, height),
                    #[cfg(feature = "vram")]
                    gpu_output_ptr: usize::default(),
                    notify_render_type: None,
                },
            );
        }
    }

    fn register_pixelbuffer_texture(&self, display: usize, ptr: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if ptr == 0 {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.texture_rgba_ptr != usize::default() {
                    info.texture_rgba_ptr = usize::default();
                }
                #[cfg(feature = "vram")]
                if info.gpu_output_ptr != usize::default() {
                    return;
                }
            }
            sessions_lock.remove(&display);
        } else {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.texture_rgba_ptr != usize::default()
                    && info.texture_rgba_ptr != ptr as TextureRgbaPtr
                {
                    log::warn!(
                        "texture_rgba_ptr is not null and not equal to ptr, replace {} to {}",
                        info.texture_rgba_ptr,
                        ptr
                    );
                }
                info.texture_rgba_ptr = ptr as _;
                info.notify_render_type = None;
            } else {
                if ptr != 0 {
                    sessions_lock.insert(
                        display,
                        DisplaySessionInfo {
                            texture_rgba_ptr: ptr as _,
                            size: (0, 0),
                            #[cfg(feature = "vram")]
                            gpu_output_ptr: usize::default(),
                            notify_render_type: None,
                        },
                    );
                }
            }
        }
    }

    #[cfg(feature = "vram")]
    pub fn register_gpu_output(&self, display: usize, ptr: usize) {
        let mut sessions_lock = self.map_display_sessions.write().unwrap();
        if ptr == 0 {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.gpu_output_ptr != usize::default() {
                    info.gpu_output_ptr = usize::default();
                }
                if info.texture_rgba_ptr != usize::default() {
                    return;
                }
            }
            sessions_lock.remove(&display);
        } else {
            if let Some(info) = sessions_lock.get_mut(&display) {
                if info.gpu_output_ptr != usize::default() && info.gpu_output_ptr != ptr {
                    log::error!(
                        "gpu_output_ptr is not null and not equal to ptr, relace {} to {}",
                        info.gpu_output_ptr,
                        ptr
                    );
                }
                info.gpu_output_ptr = ptr as _;
                info.notify_render_type = None;
            } else {
                if ptr != usize::default() {
                    sessions_lock.insert(
                        display,
                        DisplaySessionInfo {
                            texture_rgba_ptr: usize::default(),
                            size: (0, 0),
                            gpu_output_ptr: ptr,
                            notify_render_type: None,
                        },
                    );
                }
            }
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn on_rgba(&self, display: usize, rgba: &scrap::ImageRgb) -> bool {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        let opt_info = if !self.is_support_multi_ui_session {
            write_lock.values_mut().next()
        } else {
            write_lock.get_mut(&display)
        };
        let Some(info) = opt_info else {
            return false;
        };
        if info.texture_rgba_ptr == usize::default() {
            return false;
        }

        if info.size.0 != rgba.w || info.size.1 != rgba.h {
            log::error!(
                "width/height mismatch: ({},{}) != ({},{})",
                info.size.0,
                info.size.1,
                rgba.w,
                rgba.h
            );
            // Peer info's handling is async and may be late than video frame's handling
            // Allow peer info not set, but not allow wrong width/height for correct local cursor position
            if info.size != (0, 0) {
                return false;
            }
        }
        if let Some(func) = &self.on_rgba_func {
            unsafe {
                func(
                    info.texture_rgba_ptr as _,
                    rgba.raw.as_ptr() as _,
                    rgba.raw.len() as _,
                    rgba.w as _,
                    rgba.h as _,
                    rgba.align() as _,
                )
            };
        }
        if info.notify_render_type != Some(RenderType::PixelBuffer) {
            info.notify_render_type = Some(RenderType::PixelBuffer);
            true
        } else {
            false
        }
    }

    #[cfg(feature = "vram")]
    pub fn on_texture(&self, display: usize, texture: *mut c_void) -> bool {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        let opt_info = if !self.is_support_multi_ui_session {
            write_lock.values_mut().next()
        } else {
            write_lock.get_mut(&display)
        };
        let Some(info) = opt_info else {
            return false;
        };
        if info.gpu_output_ptr == usize::default() {
            return false;
        }
        if let Some(func) = &self.on_texture_func {
            unsafe { func(info.gpu_output_ptr as _, texture) };
        }
        if info.notify_render_type != Some(RenderType::Texture) {
            info.notify_render_type = Some(RenderType::Texture);
            true
        } else {
            false
        }
    }

    pub fn reset_all_display_render_type(&self) {
        let mut write_lock = self.map_display_sessions.write().unwrap();
        write_lock
            .values_mut()
            .map(|v| v.notify_render_type = None)
            .count();
    }
}

impl SessionHandler {
    pub fn on_waiting_for_image_dialog_show(&self) {
        self.renderer.reset_all_display_render_type();
        // rgba array render will notify every frame
    }
}

impl FlutterHandler {
    /// Push an event to all the event queues.
    /// An event is stored as json in the event queues.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the event.
    /// * `event` - Fields of the event content.
    pub fn push_event<V>(&self, name: &str, event: &[(&str, V)], excludes: &[&SessionID])
    where
        V: Sized + Serialize + Clone,
    {
        self.push_event_(name, event, &[], excludes);
    }

    pub fn push_event_to<V>(&self, name: &str, event: &[(&str, V)], include: &[&SessionID])
    where
        V: Sized + Serialize + Clone,
    {
        self.push_event_(name, event, include, &[]);
    }

    pub fn push_event_<V>(
        &self,
        name: &str,
        event: &[(&str, V)],
        includes: &[&SessionID],
        excludes: &[&SessionID],
    ) where
        V: Sized + Serialize + Clone,
    {
        let mut h: HashMap<&str, serde_json::Value> =
            event.iter().map(|(k, v)| (*k, json!(*v))).collect();
        debug_assert!(h.get("name").is_none());
        h.insert("name", json!(name));
        let out = serde_json::ser::to_string(&h).unwrap_or("".to_owned());
        for (sid, session) in self.session_handlers.read().unwrap().iter() {
            let mut push = false;
            if includes.is_empty() {
                if !excludes.contains(&sid) {
                    push = true;
                }
            } else {
                if includes.contains(&sid) {
                    push = true;
                }
            }
            if push {
                if let Some(stream) = &session.event_stream {
                    stream.add(EventToUI::Event(out.clone()));
                }
            }
        }
    }

    pub(crate) fn close_event_stream(&self, session_id: SessionID) {
        // to-do: Make sure the following logic is correct.
        // No need to remove the display handler, because it will be removed when the connection is closed.
        if let Some(session) = self.session_handlers.write().unwrap().get_mut(&session_id) {
            try_send_close_event(&session.event_stream);
        }
    }

    fn make_displays_msg(displays: &Vec<DisplayInfo>) -> String {
        let mut msg_vec = Vec::new();
        for ref d in displays.iter() {
            let mut h: HashMap<&str, i32> = Default::default();
            h.insert("x", d.x);
            h.insert("y", d.y);
            h.insert("width", d.width);
            h.insert("height", d.height);
            h.insert("cursor_embedded", if d.cursor_embedded { 1 } else { 0 });
            if let Some(original_resolution) = d.original_resolution.as_ref() {
                h.insert("original_width", original_resolution.width);
                h.insert("original_height", original_resolution.height);
            }
            // Don't convert scale (x 100) to i32 directly.
            // (d.scale * 100.0f64) as i32 may produces inaccuracies.
            //
            // Example: GNOME Wayland with Fractional Scaling enabled:
            // - Physical resolution: 2560x1600
            // - Logical resolution: 1074x1065
            // - Scale factor: 150%
            // Passing physical dimensions and scale factor prevents accurate logical resolution calculation
            // since 2560/1.5 = 1706.666... (rounded to 1706.67) and 1600/1.5 = 1066.666... (rounded to 1066.67)
            // h.insert("scale", (d.scale * 100.0f64) as i32);

            // Send scaled_width for accurate logical scale calculation.
            if d.scale > 0.0 {
                let scaled_width = (d.width as f64 / d.scale).round() as i32;
                h.insert("scaled_width", scaled_width);
            }
            msg_vec.push(h);
        }
        serde_json::ser::to_string(&msg_vec).unwrap_or("".to_owned())
    }

    pub fn update_use_texture_render(&self) {
        self.use_texture_render
            .store(crate::ui_interface::use_texture_render(), Ordering::Relaxed);
        self.display_rgbas.write().unwrap().clear();
    }
}

impl InvokeUiSession for FlutterHandler {
    // On the event stream, so that it keeps its order with the cursor_id events around it.
    fn set_cursor_data(&self, cd: CursorData) {
        let colors = cd.colors.to_vec();
        for session in self.session_handlers.read().unwrap().values() {
            if let Some(stream) = &session.event_stream {
                stream.add(EventToUI::Cursor {
                    id: cd.id.to_string(),
                    hotx: cd.hotx,
                    hoty: cd.hoty,
                    width: cd.width,
                    height: cd.height,
                    colors: colors.clone(),
                });
            }
        }
    }

    fn set_cursor_id(&self, id: String) {
        self.push_event("cursor_id", &[("id", &id.to_string())], &[]);
    }

    fn set_cursor_position(&self, cp: CursorPosition) {
        self.push_event(
            "cursor_position",
            &[("x", &cp.x.to_string()), ("y", &cp.y.to_string())],
            &[],
        );
    }

    /// unused in flutter, use switch_display or set_peer_info
    fn set_display(&self, _x: i32, _y: i32, _w: i32, _h: i32, _cursor_embedded: bool, _scale: f64) {}

    fn update_privacy_mode(&self) {
        self.push_event::<&str>("update_privacy_mode", &[], &[]);
    }

    fn set_permission(&self, name: &str, value: bool) {
        self.push_event("permission", &[(name, &value.to_string())], &[]);
    }

    // unused in flutter
    fn close_success(&self) {}

    fn update_quality_status(&self, status: QualityStatus) {
        const NULL: String = String::new();
        self.push_event(
            "update_quality_status",
            &[
                ("speed", &status.speed.map_or(NULL, |it| it)),
                (
                    "fps",
                    &serde_json::ser::to_string(&status.fps).unwrap_or(NULL.to_owned()),
                ),
                ("delay", &status.delay.map_or(NULL, |it| it.to_string())),
                (
                    "target_bitrate",
                    &status.target_bitrate.map_or(NULL, |it| it.to_string()),
                ),
                (
                    "codec_format",
                    &status.codec_format.map_or(NULL, |it| it.to_string()),
                ),
                ("chroma", &status.chroma.map_or(NULL, |it| it.to_string())),
            ],
            &[],
        );
    }

    fn set_connection_type(&self, is_secured: bool, direct: bool, stream_type: &str) {
        self.push_event(
            "connection_ready",
            &[
                ("secure", &is_secured.to_string()),
                ("direct", &direct.to_string()),
                ("stream_type", &stream_type.to_string()),
            ],
            &[],
        );
    }

    fn set_fingerprint(&self, fingerprint: String) {
        self.push_event("fingerprint", &[("fingerprint", &fingerprint)], &[]);
    }

    // unused in flutter
    fn adapt_size(&self) {}

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        let use_texture_render = self.use_texture_render.load(Ordering::Relaxed);
        self.on_rgba_flutter_texture_render(use_texture_render, display, rgba);
        if !use_texture_render {
            self.on_rgba_soft_render(display, rgba);
        }
    }

    #[inline]
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        self.on_rgba_soft_render(display, rgba);
    }

    #[inline]
    #[cfg(feature = "vram")]
    fn on_texture(&self, display: usize, texture: *mut c_void) {
        if !self.use_texture_render.load(Ordering::Relaxed) {
            return;
        }
        for (_, session) in self.session_handlers.read().unwrap().iter() {
            if session.renderer.on_texture(display, texture) {
                if let Some(stream) = &session.event_stream {
                    stream.add(EventToUI::Texture(display, true));
                }
            }
        }
    }

    fn set_peer_info(&self, pi: &PeerInfo) {
        let displays = Self::make_displays_msg(&pi.displays);
        let mut features: HashMap<&str, bool> = Default::default();
        for ref f in pi.features.iter() {
            features.insert("privacy_mode", f.privacy_mode);
        }
        // compatible with 1.1.9
        if get_version_number(&pi.version) < get_version_number("1.2.0") {
            features.insert("privacy_mode", false);
        }
        let features = serde_json::ser::to_string(&features).unwrap_or("".to_owned());
        let resolutions = serialize_resolutions(&pi.resolutions.resolutions);
        *self.peer_info.write().unwrap() = pi.clone();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(&pi.version);
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let is_support_multi_ui_session = false;
        self.session_handlers
            .write()
            .unwrap()
            .values_mut()
            .for_each(|h| {
                h.renderer.is_support_multi_ui_session = is_support_multi_ui_session;
            });
        self.push_event(
            "peer_info",
            &[
                ("username", &pi.username),
                ("hostname", &pi.hostname),
                ("platform", &pi.platform),
                ("sas_enabled", &pi.sas_enabled.to_string()),
                ("displays", &displays),
                ("version", &pi.version),
                ("features", &features),
                ("current_display", &pi.current_display.to_string()),
                ("resolutions", &resolutions),
                ("platform_additions", &pi.platform_additions),
            ],
            &[],
        );
    }

    fn set_displays(&self, displays: &Vec<DisplayInfo>) {
        self.peer_info.write().unwrap().displays = displays.clone();
        self.push_event(
            "sync_peer_info",
            &[("displays", &Self::make_displays_msg(displays))],
            &[],
        );
    }

    fn set_platform_additions(&self, data: &str) {
        self.push_event(
            "sync_platform_additions",
            &[("platform_additions", &data)],
            &[],
        )
    }

    fn set_multiple_windows_session(&self, sessions: Vec<WindowsSession>) {
        let mut msg_vec = Vec::new();
        let mut sessions = sessions;
        for d in sessions.drain(..) {
            let mut h: HashMap<&str, String> = Default::default();
            h.insert("sid", d.sid.to_string());
            h.insert("name", d.name);
            msg_vec.push(h);
        }
        self.push_event(
            "set_multiple_windows_session",
            &[(
                "windows_sessions",
                &serde_json::ser::to_string(&msg_vec).unwrap_or("".to_owned()),
            )],
            &[],
        );
    }

    fn is_multi_ui_session(&self) -> bool {
        self.session_handlers.read().unwrap().len() > 1
    }

    fn set_current_display(&self, disp_idx: i32) {
        if self.is_multi_ui_session() {
            return;
        }
        self.push_event(
            "follow_current_display",
            &[("display_idx", &disp_idx.to_string())],
            &[],
        );
    }

    fn on_connected(&self, _conn_type: ConnType) {}

    fn msgbox(&self, msgtype: &str, title: &str, text: &str, link: &str, retry: bool) {
        let has_retry = if retry { "true" } else { "" };
        self.push_event(
            "msgbox",
            &[
                ("type", msgtype),
                ("title", title),
                ("text", text),
                ("link", link),
                ("hasRetry", has_retry),
            ],
            &[],
        );
    }

    fn cancel_msgbox(&self, tag: &str) {
        self.push_event("cancel_msgbox", &[("tag", tag)], &[]);
    }

    fn switch_display(&self, display: &SwitchDisplay) {
        let resolutions = serialize_resolutions(&display.resolutions.resolutions);
        self.push_event(
            "switch_display",
            &[
                ("display", &display.display.to_string()),
                ("x", &display.x.to_string()),
                ("y", &display.y.to_string()),
                ("width", &display.width.to_string()),
                ("height", &display.height.to_string()),
                (
                    "cursor_embedded",
                    &{
                        if display.cursor_embedded {
                            1
                        } else {
                            0
                        }
                    }
                    .to_string(),
                ),
                ("resolutions", &resolutions),
                (
                    "original_width",
                    &display.original_resolution.width.to_string(),
                ),
                (
                    "original_height",
                    &display.original_resolution.height.to_string(),
                ),
            ],
            &[],
        );
    }

    fn update_block_input_state(&self, on: bool) {
        self.push_event(
            "update_block_input_state",
            &[("input_state", if on { "on" } else { "off" })],
            &[],
        );
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn clipboard(&self, content: String) {
        self.push_event("clipboard", &[("content", &content)], &[]);
    }

    fn portable_service_running(&self, running: bool) {
        self.push_event(
            "portable_service_running",
            &[("running", running.to_string().as_str())],
            &[],
        );
    }

    #[inline]
    fn get_rgba(&self, _display: usize) -> *const u8 {
        if let Some(rgba_data) = self.display_rgbas.read().unwrap().get(&_display) {
            if rgba_data.valid {
                return rgba_data.data.as_ptr();
            }
        }
        std::ptr::null_mut()
    }

    #[inline]
    fn next_rgba(&self, _display: usize) {
        if let Some(rgba_data) = self.display_rgbas.write().unwrap().get_mut(&_display) {
            rgba_data.valid = false;
        }
    }

    fn handle_screenshot_resp(&self, sid: String, msg: String) {
        match SessionID::from_str(&sid) {
            Ok(sid) => self.push_event_to("screenshot", &[("msg", json!(msg))], &[&sid]),
            Err(e) => {
                // Unreachable!
                log::error!("Failed to parse sid \"{}\", {}", sid, e);
            }
        }
    }

}

impl FlutterHandler {
    #[inline]
    fn on_rgba_soft_render(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        // If the current rgba is not fetched by flutter, i.e., is valid.
        // We give up sending a new event to flutter.
        let mut rgba_write_lock = self.display_rgbas.write().unwrap();
        if let Some(rgba_data) = rgba_write_lock.get_mut(&display) {
            if rgba_data.valid {
                return;
            } else {
                rgba_data.valid = true;
            }
            // Return the rgba buffer to the video handler for reusing allocated rgba buffer.
            std::mem::swap::<Vec<u8>>(&mut rgba.raw, &mut rgba_data.data);
        } else {
            let mut rgba_data = RgbaData::default();
            std::mem::swap::<Vec<u8>>(&mut rgba.raw, &mut rgba_data.data);
            rgba_data.valid = true;
            rgba_write_lock.insert(display, rgba_data);
        }
        drop(rgba_write_lock);

        let mut is_sent = false;
        let is_multi_sessions = self.is_multi_ui_session();
        for h in self.session_handlers.read().unwrap().values() {
            // The soft renderer does not support multi-displays session for now.
            if h.displays.len() > 1 {
                continue;
            }
            // If there're multiple ui sessions, we only notify the ui session that has the display.
            if is_multi_sessions {
                if !h.displays.contains(&display) {
                    continue;
                }
            }
            if let Some(stream) = &h.event_stream {
                stream.add(EventToUI::Rgba(display));
                is_sent = true;
            }
        }
        // We need `is_sent` here. Because we use texture render for multi-displays session.
        //
        // Eg. We have two windows, one is display 1, the other is displays 0&1.
        // When image of display 0 is received, we will not send the event.
        //
        // 1. "display 1" will not send the event.
        // 2. "displays 0&1" will not send the event. Because it uses texutre render for now.
        if !is_sent {
            if let Some(rgba_data) = self.display_rgbas.write().unwrap().get_mut(&display) {
                rgba_data.valid = false;
            }
        }
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn on_rgba_flutter_texture_render(
        &self,
        use_texture_render: bool,
        display: usize,
        rgba: &mut scrap::ImageRgb,
    ) {
        for (_, session) in self.session_handlers.read().unwrap().iter() {
            if use_texture_render || session.displays.len() > 1 {
                if session.renderer.on_rgba(display, rgba) {
                    if let Some(stream) = &session.event_stream {
                        stream.add(EventToUI::Texture(display, false));
                    }
                }
            }
        }
    }
}

// This function is only used for the default connection session.
pub fn session_add_existed(
    peer_id: String,
    session_id: SessionID,
    displays: Vec<i32>,
) -> ResultType<()> {
    let conn_type = ConnType::DEFAULT_CONN;
    sessions::insert_peer_session_id(peer_id, conn_type, session_id, displays);
    Ok(())
}

/// Create a new remote session with the given id.
///
/// # Arguments
///
/// * `id` - The identifier of the remote session with prefix. Regex: [\w]*[\_]*[\d]+
pub fn session_add(
    session_id: &SessionID,
    id: &str,
    password: String,
    is_shared_password: bool,
    conn_token: Option<String>,
) -> ResultType<FlutterSession> {
    let conn_type = ConnType::DEFAULT_CONN;

    // to-do: check the same id session.
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        if session.lc.read().unwrap().conn_type != conn_type {
            bail!("same session id is found with different conn type?");
        }
        // The same session is added before?
        bail!("same session id is found");
    }

    LocalConfig::set_remote_id(&id);

    let mut preset_password = password.clone();
    let shared_password = if is_shared_password {
        // To achieve a flexible password application order, we don't treat shared password as a preset password.
        preset_password = Default::default();
        Some(password)
    } else {
        None
    };

    let session: Session<FlutterHandler> = Session {
        password: preset_password,
        server_keyboard_enabled: Arc::new(RwLock::new(true)),
        server_clipboard_enabled: Arc::new(RwLock::new(true)),
        reconnect_count: Arc::new(AtomicUsize::new(0)),
        ..Default::default()
    };

    session.lc.write().unwrap().initialize(
        id.to_owned(),
        conn_type,
        get_adapter_luid(),
        shared_password,
        conn_token,
    );

    let session = Arc::new(session.clone());
    sessions::insert_session(session_id.to_owned(), conn_type, session.clone());

    Ok(session)
}

/// start a session with the given id.
///
/// # Arguments
///
/// * `id` - The identifier of the remote session with prefix. Regex: [\w]*[\_]*[\d]+
/// * `events2ui` - The events channel to ui.
pub fn session_start_(
    session_id: &SessionID,
    id: &str,
    event_stream: StreamSink<EventToUI>,
) -> ResultType<()> {
    // is_connected is used to indicate whether to start a peer connection. For two cases:
    // 1. "Move tab to new window"
    // 2. multi ui session within the same peer connection.
    let mut is_connected = false;
    let mut is_found = false;
    for s in sessions::get_sessions() {
        if let Some(h) = s.session_handlers.write().unwrap().get_mut(session_id) {
            is_connected = h.event_stream.is_some();
            try_send_close_event(&h.event_stream);
            h.event_stream = Some(event_stream);
            is_found = true;
            break;
        }
    }
    if !is_found {
        bail!(
            "No session with peer id {}, session id: {}",
            id,
            session_id.to_string()
        );
    }

    if let Some(session) = sessions::get_session_by_session_id(session_id) {
        let is_first_ui_session = session.session_handlers.read().unwrap().len() == 1;
        if !is_connected && is_first_ui_session {
            log::info!(
                "Session {} start, use texture render: {}",
                id,
                session.use_texture_render.load(Ordering::Relaxed)
            );
            let session = (*session).clone();
            std::thread::spawn(move || {
                let round = session.connection_round_state.lock().unwrap().new_round();
                io_loop(session, round);
            });
        }
        Ok(())
    } else {
        bail!("No session with peer id {}", id)
    }
}

#[inline]
fn try_send_close_event(event_stream: &Option<StreamSink<EventToUI>>) {
    if let Some(stream) = &event_stream {
        stream.add(EventToUI::Event("close".to_owned()));
    }
}

#[cfg(not(target_os = "ios"))]
pub fn update_text_clipboard_required() {
    let is_required = sessions::get_sessions()
        .iter()
        .filter(|s| s.connection_round_state.lock().unwrap().is_connected())
        .any(|s| s.is_default() && s.is_text_clipboard_required());
    #[cfg(target_os = "android")]
    let _ = scrap::android::ffi::call_clipboard_manager_enable_client_clipboard(is_required);
    Client::set_is_text_clipboard_required(is_required);
}

#[cfg(not(target_os = "ios"))]
pub fn send_clipboard_msg(msg: Message) {
    send_clipboard_msg_impl(msg, None);
}

// `except_session_id` is the session the content came from, to avoid sending it back.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn send_clipboard_msg_to_other_sessions(msg: Message, except_session_id: u64) {
    send_clipboard_msg_impl(msg, Some(except_session_id));
}

#[cfg(not(target_os = "ios"))]
fn send_clipboard_msg_impl(msg: Message, except_session_id: Option<u64>) {
    for s in sessions::get_sessions() {
        if !s.is_default() {
            continue;
        }
        if let Some(except_session_id) = except_session_id {
            if s.lc.read().unwrap().session_id == except_session_id {
                continue;
            }
        }
        if s.is_text_clipboard_required() {
            // Check if the client supports multi clipboards
            if let Some(message::Union::MultiClipboards(multi_clipboards)) = &msg.union {
                let version = s.ui_handler.peer_info.read().unwrap().version.clone();
                let platform = s.ui_handler.peer_info.read().unwrap().platform.clone();
                if let Some(msg_out) = crate::clipboard::get_msg_if_not_support_multi_clip(
                    &version,
                    &platform,
                    multi_clipboards,
                ) {
                    s.send(Data::Message(msg_out));
                    continue;
                }
            }
            s.send(Data::Message(msg.clone()));
        }
    }
}

// Server Side
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod connection_manager {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::ui_cm_interface::InvokeUiCM;

    use super::GLOBAL_EVENT_STREAM;

    #[derive(Clone)]
    struct FlutterHandler {}

    impl InvokeUiCM for FlutterHandler {
        fn add_connection(&self, client: &crate::ui_cm_interface::Client) {
            let client_json = serde_json::to_string(&client).unwrap_or("".into());
            // send to UI, refresh widget
            self.push_event("add_connection", &[("client", &client_json)]);
        }

        fn remove_connection(&self, id: i32, close: bool) {
            self.push_event(
                "on_client_remove",
                &[("id", &id.to_string()), ("close", &close.to_string())],
            );
        }

        fn change_theme(&self, dark: String) {
            self.push_event("theme", &[("dark", &dark)]);
        }

        fn change_language(&self) {
            self.push_event::<&str>("language", &[]);
        }

        fn show_elevation(&self, show: bool) {
            self.push_event("show_elevation", &[("show", &show.to_string())]);
        }

    }

    impl FlutterHandler {
        fn push_event<V>(&self, name: &str, event: &[(&str, V)])
        where
            V: Sized + serde::Serialize + Clone,
        {
            let mut h: HashMap<&str, serde_json::Value> =
                event.iter().map(|(k, v)| (*k, json!(*v))).collect();
            debug_assert!(h.get("name").is_none());
            h.insert("name", json!(name));

            if let Some(s) = GLOBAL_EVENT_STREAM.read().unwrap().get(super::APP_TYPE_CM) {
                s.add(serde_json::ser::to_string(&h).unwrap_or("".to_owned()));
            } else {
                println!(
                    "Push event {} failed. No {} event stream found.",
                    name,
                    super::APP_TYPE_CM
                );
            };
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn start_listen_ipc() {
        use crate::ui_cm_interface::{start_ipc, ConnectionManager};

        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);

        let cm = ConnectionManager {
            ui_handler: FlutterHandler {},
        };
        std::thread::spawn(move || start_ipc(cm));
    }

    #[inline]
    pub fn cm_init() {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        start_listen_ipc();
    }
}

pub fn get_cur_session_id() -> SessionID {
    CUR_SESSION_ID.read().unwrap().clone()
}

pub fn get_cur_peer_id() -> String {
    sessions::get_peer_id_by_session_id(&get_cur_session_id(), ConnType::DEFAULT_CONN)
        .unwrap_or("".to_string())
}

pub fn set_cur_session_id(session_id: SessionID) {
    let mut current = CUR_SESSION_ID.write().unwrap();
    if *current != session_id {
        crate::keyboard::shortcuts::transfer_released_modifiers(&current, &session_id);
        *current = session_id;
    }
}

#[inline]
fn serialize_resolutions(resolutions: &Vec<Resolution>) -> String {
    #[derive(Debug, serde::Serialize)]
    struct ResolutionSerde {
        width: i32,
        height: i32,
    }

    let mut v = vec![];
    resolutions
        .iter()
        .map(|r| {
            v.push(ResolutionSerde {
                width: r.width,
                height: r.height,
            })
        })
        .count();
    serde_json::ser::to_string(&v).unwrap_or("".to_string())
}

fn char_to_session_id(c: *const char) -> ResultType<SessionID> {
    if c.is_null() {
        bail!("Session id ptr is null");
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(c as _) };
    let str = cstr.to_str()?;
    SessionID::from_str(str).map_err(|e| anyhow!("{:?}", e))
}

pub fn session_get_rgba_size(session_id: SessionID, display: usize) -> usize {
    if let Some(session) = sessions::get_session_by_session_id(&session_id) {
        return session
            .display_rgbas
            .read()
            .unwrap()
            .get(&display)
            .map_or(0, |rgba| rgba.data.len());
    }
    0
}

#[no_mangle]
pub extern "C" fn session_get_rgba(session_uuid_str: *const char, display: usize) -> *const u8 {
    if let Ok(session_id) = char_to_session_id(session_uuid_str) {
        if let Some(s) = sessions::get_session_by_session_id(&session_id) {
            return s.ui_handler.get_rgba(display);
        }
    }

    std::ptr::null()
}

pub fn session_next_rgba(session_id: SessionID, display: usize) {
    if let Some(s) = sessions::get_session_by_session_id(&session_id) {
        return s.ui_handler.next_rgba(display);
    }
}

#[inline]
pub fn session_set_size(session_id: SessionID, display: usize, width: usize, height: usize) {
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .write()
            .unwrap()
            .get_mut(&session_id)
        {
            // If the session is the first connection, displays is not set yet.
            // `displays`` is set while switching displays or adding a new session.
            if !h.displays.contains(&display) {
                h.displays.push(display);
            }
            h.renderer.set_size(display, width, height);
            break;
        }
    }
}

#[inline]
pub fn session_register_pixelbuffer_texture(session_id: SessionID, display: usize, ptr: usize) {
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .read()
            .unwrap()
            .get(&session_id)
        {
            h.renderer.register_pixelbuffer_texture(display, ptr);
            break;
        }
    }
}

#[inline]
pub fn session_register_gpu_texture(_session_id: SessionID, _display: usize, _output_ptr: usize) {
    #[cfg(feature = "vram")]
    for s in sessions::get_sessions() {
        if let Some(h) = s
            .ui_handler
            .session_handlers
            .read()
            .unwrap()
            .get(&_session_id)
        {
            h.renderer.register_gpu_output(_display, _output_ptr);
            break;
        }
    }
}

#[inline]
#[cfg(not(feature = "vram"))]
pub fn get_adapter_luid() -> Option<i64> {
    None
}

#[cfg(feature = "vram")]
pub fn get_adapter_luid() -> Option<i64> {
    if !crate::ui_interface::use_texture_render() {
        return None;
    }
    let get_adapter_luid_func = match &*TEXTURE_GPU_RENDERER_PLUGIN {
        Ok(lib) => {
            let find_sym_res = unsafe {
                lib.symbol::<FlutterGpuTextureRendererPluginCApiGetAdapterLuid>(
                    "FlutterGpuTextureRendererPluginCApiGetAdapterLuid",
                )
            };
            match find_sym_res {
                Ok(sym) => Some(sym),
                Err(e) => {
                    log::error!("Failed to find symbol FlutterGpuTextureRendererPluginCApiGetAdapterLuid, {e}");
                    None
                }
            }
        }
        Err(e) => {
            log::error!("Failed to load texture gpu renderer plugin, {e}");
            None
        }
    };
    let adapter_luid = match get_adapter_luid_func {
        Some(get_adapter_luid_func) => unsafe { Some(get_adapter_luid_func()) },
        None => Default::default(),
    };
    return adapter_luid;
}

#[inline]
pub fn push_session_event(session_id: &SessionID, name: &str, event: Vec<(&str, &str)>) {
    if let Some(s) = sessions::get_session_by_session_id(session_id) {
        s.push_event(name, &event, &[]);
    }
}

#[inline]
pub fn push_global_event(channel: &str, event: String) -> Option<bool> {
    Some(GLOBAL_EVENT_STREAM.read().unwrap().get(channel)?.add(event))
}

#[inline]
pub fn get_global_event_channels() -> Vec<String> {
    GLOBAL_EVENT_STREAM
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

pub fn start_global_event_stream(s: StreamSink<String>, app_type: String) -> ResultType<()> {
    let app_type_values = app_type.split(",").collect::<Vec<&str>>();
    let mut lock = GLOBAL_EVENT_STREAM.write().unwrap();
    if !lock.contains_key(app_type_values[0]) {
        lock.insert(app_type_values[0].to_string(), s);
    } else {
        if let Some(_) = lock.insert(app_type.clone(), s) {
            log::warn!(
                "Global event stream of type {} is started before, but now removed",
                app_type
            );
        }
    }
    Ok(())
}

pub fn stop_global_event_stream(app_type: String) {
    let _ = GLOBAL_EVENT_STREAM.write().unwrap().remove(&app_type);
}

#[inline]
fn session_send_touch_scale(
    session_id: SessionID,
    v: &serde_json::Value,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("v").and_then(|s| s.as_i64()) {
        Some(scale) => {
            if let Some(session) = sessions::get_session_by_session_id(&session_id) {
                session.send_touch_scale(scale as _, alt, ctrl, shift, command);
            }
        }
        None => {}
    }
}

#[inline]
fn session_send_touch_pan(
    session_id: SessionID,
    v: &serde_json::Value,
    pan_event: &str,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("v") {
        Some(v) => match (
            v.get("x").and_then(|x| x.as_i64()),
            v.get("y").and_then(|y| y.as_i64()),
        ) {
            (Some(x), Some(y)) => {
                if let Some(session) = sessions::get_session_by_session_id(&session_id) {
                    session
                        .send_touch_pan_event(pan_event, x as _, y as _, alt, ctrl, shift, command);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn session_send_touch_event(
    session_id: SessionID,
    v: &serde_json::Value,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) {
    match v.get("t").and_then(|t| t.as_str()) {
        Some("scale") => session_send_touch_scale(session_id, v, alt, ctrl, shift, command),
        Some(pan_event) => {
            session_send_touch_pan(session_id, v, pan_event, alt, ctrl, shift, command)
        }
        _ => {}
    }
}

pub fn session_send_pointer(session_id: SessionID, msg: String) {
    if let Ok(m) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&msg) {
        let alt = m.get("alt").is_some();
        let ctrl = m.get("ctrl").is_some();
        let shift = m.get("shift").is_some();
        let command = m.get("command").is_some();
        match (m.get("k"), m.get("v")) {
            (Some(k), Some(v)) => match k.as_str() {
                Some("touch") => session_send_touch_event(session_id, v, alt, ctrl, shift, command),
                _ => {}
            },
            _ => {}
        }
    }
}

#[inline]
pub fn session_on_waiting_for_image_dialog_show(session_id: SessionID) {
    for s in sessions::get_sessions() {
        if let Some(h) = s.session_handlers.write().unwrap().get_mut(&session_id) {
            h.on_waiting_for_image_dialog_show();
        }
    }
}

#[inline]
pub fn get_cur_session() -> Option<FlutterSession> {
    sessions::get_session_by_session_id(&*CUR_SESSION_ID.read().unwrap())
}

#[inline]
pub fn try_sync_peer_option(
    session: &FlutterSession,
    cur_id: &SessionID,
    key: &str,
    _value: Option<serde_json::Value>,
) {
    let mut event = Vec::new();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if key == "view-only" {
        event = vec![
            ("k", json!(key.to_string())),
            ("v", json!(session.lc.read().unwrap().view_only.v)),
        ];
    }
    if ["keyboard_mode", "input_source"].contains(&key) {
        event = vec![("k", json!(key.to_string())), ("v", json!(""))];
    }
    if !event.is_empty() {
        session.push_event("sync_peer_option", &event, &[cur_id]);
    }
}

pub(super) fn session_update_virtual_display(session: &FlutterSession, index: i32, on: bool) {
    let virtual_display_key = "virtual-display";
    let displays = session.get_option(virtual_display_key.to_owned());
    if !on {
        if index == -1 {
            if !displays.is_empty() {
                session.set_option(virtual_display_key.to_owned(), "".to_owned());
            }
        } else {
            let mut vdisplays = displays.split(',').collect::<Vec<_>>();
            let len = vdisplays.len();
            if index == 0 {
                // 0 means we can't toggle the virtual display by index.
                vdisplays.remove(vdisplays.len() - 1);
            } else {
                if let Some(i) = vdisplays.iter().position(|&x| x == index.to_string()) {
                    vdisplays.remove(i);
                }
            }
            if vdisplays.len() != len {
                session.set_option(
                    virtual_display_key.to_owned(),
                    vdisplays.join(",").to_owned(),
                );
            }
        }
    } else {
        let mut vdisplays = displays
            .split(',')
            .map(|x| x.to_string())
            .collect::<Vec<_>>();
        let len = vdisplays.len();
        if index == 0 {
            vdisplays.push(index.to_string());
        } else {
            if !vdisplays.iter().any(|x| *x == index.to_string()) {
                vdisplays.push(index.to_string());
            }
        }
        if vdisplays.len() != len {
            session.set_option(
                virtual_display_key.to_owned(),
                vdisplays.join(",").to_owned(),
            );
        }
    }
}

// sessions mod is used to avoid the big lock of sessions' map.
pub mod sessions {

    use super::*;

    lazy_static::lazy_static! {
        // peer -> peer session, peer session -> ui sessions
        static ref SESSIONS: RwLock<HashMap<(String, ConnType), FlutterSession>> = Default::default();
    }

    #[inline]
    pub fn get_session_count(peer_id: String, conn_type: ConnType) -> usize {
        SESSIONS
            .read()
            .unwrap()
            .get(&(peer_id, conn_type))
            .map(|s| s.ui_handler.session_handlers.read().unwrap().len())
            .unwrap_or(0)
    }

    #[inline]
    pub fn get_peer_id_by_session_id(id: &SessionID, conn_type: ConnType) -> Option<String> {
        SESSIONS
            .read()
            .unwrap()
            .iter()
            .find_map(|((peer_id, t), s)| {
                if *t == conn_type
                    && s.ui_handler
                        .session_handlers
                        .read()
                        .unwrap()
                        .contains_key(id)
                {
                    Some(peer_id.clone())
                } else {
                    None
                }
            })
    }

    #[inline]
    pub fn get_session_by_session_id(id: &SessionID) -> Option<FlutterSession> {
        SESSIONS
            .read()
            .unwrap()
            .values()
            .find(|s| {
                s.ui_handler
                    .session_handlers
                    .read()
                    .unwrap()
                    .contains_key(id)
            })
            .cloned()
    }

    #[inline]
    pub fn get_session_by_peer_id(peer_id: String, conn_type: ConnType) -> Option<FlutterSession> {
        SESSIONS.read().unwrap().get(&(peer_id, conn_type)).cloned()
    }

    #[inline]
    pub fn remove_session_by_session_id(id: &SessionID) -> Option<FlutterSession> {
        let mut remove_peer_key = None;
        for (peer_key, s) in SESSIONS.write().unwrap().iter_mut() {
            let mut write_lock = s.ui_handler.session_handlers.write().unwrap();
            let remove_ret = write_lock.remove(id);
            match remove_ret {
                Some(_) => {
                    if write_lock.is_empty() {
                        remove_peer_key = Some(peer_key.clone());
                    } else {
                        check_remove_unused_displays(None, id, s, &write_lock);
                    }
                    break;
                }
                None => {}
            }
        }
        let s = SESSIONS.write().unwrap().remove(&remove_peer_key?);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        update_session_count_to_server();
        s
    }

    /// Close every client session, returning how many peer sessions were closed.
    ///
    /// Used when the UI is gone but the process keeps running, e.g. the Android
    /// task is swiped away from recents while a foreground service keeps the
    /// process alive. The orphaned `io_loop` would otherwise keep answering
    /// `TestDelay`, so the peer never hits its inactivity timeout and the
    /// session stays established with no way to close it.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn close_all_sessions() -> usize {
        // Release held keys before draining: the release path sends through
        // `get_cur_session()`, which resolves against SESSIONS, so draining
        // first would take TO_RELEASE and then silently drop every key-up,
        // leaving the key stuck on the controlled side. A no-op when nothing
        // is held.
        crate::keyboard::release_remote_keys("map");
        // Drain so the map lock is released before closing each session.
        let sessions: Vec<FlutterSession> = SESSIONS
            .write()
            .unwrap()
            .drain()
            .map(|(_, session)| session)
            .collect();
        for session in sessions.iter() {
            let session_ids: Vec<SessionID> = session
                .ui_handler
                .session_handlers
                .read()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            for session_id in session_ids {
                session.close_event_stream(session_id);
            }
            session.close();
        }
        sessions.len()
    }

    /// Check if removing a session by session_id would result in removing the entire peer.
    ///
    /// Returns:
    /// - `true`: The session exists and removing it would leave the peer with no other sessions,
    ///           so the entire peer would be removed (equivalent to `remove_session_by_session_id` returning `Some`)
    /// - `false`: The session doesn't exist, or it exists but the peer has other sessions,
    ///            so the peer would not be removed (equivalent to `remove_session_by_session_id` returning `None`)
    #[inline]
    pub fn would_remove_peer_by_session_id(id: &SessionID) -> bool {
        for (_peer_key, s) in SESSIONS.read().unwrap().iter() {
            let read_lock = s.ui_handler.session_handlers.read().unwrap();
            if read_lock.contains_key(id) {
                // Found the session, check if it's the only one for this peer
                return read_lock.len() == 1;
            }
        }
        // Session not found
        false
    }

    fn check_remove_unused_displays(
        current: Option<usize>,
        session_id: &SessionID,
        session: &FlutterSession,
        handlers: &HashMap<SessionID, SessionHandler>,
    ) {
        // Set capture displays if some are not used any more.
        let mut remains_displays = HashSet::new();
        if let Some(current) = current {
            remains_displays.insert(current);
        }
        for (k, h) in handlers.iter() {
            if k == session_id {
                continue;
            }
            remains_displays.extend(
                h.renderer
                    .map_display_sessions
                    .read()
                    .unwrap()
                    .keys()
                    .cloned(),
            );
        }
        if !remains_displays.is_empty() {
            session.capture_displays(
                vec![],
                vec![],
                remains_displays.iter().map(|d| *d as i32).collect(),
            );
        }
    }

    pub fn session_switch_display(is_desktop: bool, session_id: SessionID, value: Vec<i32>) {
        for s in SESSIONS.read().unwrap().values() {
            let mut write_lock = s.ui_handler.session_handlers.write().unwrap();
            if let Some(h) = write_lock.get_mut(&session_id) {
                h.displays = value.iter().map(|x| *x as usize).collect::<_>();
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                let displays_refresh = value.clone();
                if value.len() == 1 {
                    // Switch display.
                    // This operation will also cause the peer to send a switch display message.
                    // The switch display message will contain `SupportedResolutions`, which is useful when changing resolutions.
                    s.switch_display(value[0]);
                    // Reset the valid flag of the display.
                    s.next_rgba(value[0] as usize);

                    if !is_desktop {
                        s.capture_displays(vec![], vec![], value);
                    } else {
                        // Check if other displays are needed.
                        if value.len() == 1 {
                            check_remove_unused_displays(
                                Some(value[0] as _),
                                &session_id,
                                &s,
                                &write_lock,
                            );
                        }
                    }
                } else {
                    // Try capture all displays.
                    s.capture_displays(vec![], vec![], value);
                }
                // When switching display, we also need to send "Refresh display" message.
                // On the controlled side:
                // 1. If this display is not currently captured -> Refresh -> Message "Refresh display" is not required.
                // One more key frame (first frame) will be sent because the refresh message.
                // 2. If this display is currently captured -> Not refresh -> Message "Refresh display" is required.
                // Without the message, the control side cannot see the latest display image.
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(
                        &s.ui_handler.peer_info.read().unwrap().version,
                    );
                    if is_support_multi_ui_session {
                        for display in displays_refresh.iter() {
                            s.refresh_video(*display);
                        }
                    }
                }
                break;
            }
        }
    }

    #[inline]
    pub fn insert_session(session_id: SessionID, conn_type: ConnType, session: FlutterSession) {
        SESSIONS
            .write()
            .unwrap()
            .entry((session.get_id(), conn_type))
            .or_insert(session)
            .ui_handler
            .session_handlers
            .write()
            .unwrap()
            .insert(session_id, Default::default());
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        update_session_count_to_server();
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn update_session_count_to_server() {
        crate::ipc::update_controlling_session_count(SESSIONS.read().unwrap().len()).ok();
    }

    #[inline]
    pub fn insert_peer_session_id(
        peer_id: String,
        conn_type: ConnType,
        session_id: SessionID,
        displays: Vec<i32>,
    ) -> bool {
        if let Some(s) = SESSIONS.read().unwrap().get(&(peer_id, conn_type)) {
            let mut h = SessionHandler::default();
            h.displays = displays.iter().map(|x| *x as usize).collect::<_>();
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            let is_support_multi_ui_session = crate::common::is_support_multi_ui_session(
                &s.ui_handler.peer_info.read().unwrap().version,
            );
            #[cfg(any(target_os = "android", target_os = "ios"))]
            let is_support_multi_ui_session = false;
            h.renderer.is_support_multi_ui_session = is_support_multi_ui_session;
            let _ = s
                .ui_handler
                .session_handlers
                .write()
                .unwrap()
                .insert(session_id, h);
            // If the session is a single display session, it may be a software rgba rendered display.
            // If this is the second time the display is opened, the old valid flag may be true.
            if displays.len() == 1 {
                s.ui_handler.next_rgba(displays[0] as usize);
            }
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn get_sessions() -> Vec<FlutterSession> {
        SESSIONS.read().unwrap().values().cloned().collect()
    }

    #[inline]
    #[cfg(not(target_os = "ios"))]
    pub fn has_sessions_running(conn_type: ConnType) -> bool {
        SESSIONS.read().unwrap().iter().any(|((_, r#type), s)| {
            *r#type == conn_type && s.session_handlers.read().unwrap().len() != 0
        })
    }

    #[inline]
    #[cfg(not(target_os = "ios"))]
    pub fn has_connected_sessions_running(conn_type: ConnType) -> bool {
        SESSIONS.read().unwrap().iter().any(|((_, r#type), s)| {
            *r#type == conn_type
                && s.session_handlers.read().unwrap().len() != 0
                && s.connection_round_state.lock().unwrap().is_connected()
        })
    }
}
