#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "ios", target_arch = "aarch64"),
    all(target_os = "android", target_arch = "aarch64"),
)))]
compile_error!("Supported targets: Windows/Linux x86_64 and macOS/iOS/Android ARM64");

#[cfg(any(test, not(target_os = "linux")))]
mod audio_resampler;
mod keyboard;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod management;
/// cbindgen:ignore
pub mod platform;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use platform::{
    clip_cursor, get_cursor, get_cursor_data, get_cursor_pos, get_focused_display,
    set_cursor_pos, start_os_service,
};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
/// cbindgen:ignore
mod server;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use self::server::*;
mod client;
mod lan;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod direct_server;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use self::direct_server::*;
/// cbindgen:ignore
pub mod common;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod ipc;
pub(crate) mod update_metadata;
pub(crate) mod update_hash;
#[cfg(target_os = "macos")]
mod manual_update;
mod version;
pub use version::*;
#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
mod bridge_generated;
#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
pub mod flutter;
#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
pub mod flutter_ffi;
use common::*;
mod auth_2fa;
#[cfg(not(target_os = "ios"))]
mod clipboard;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod core_main;
mod lang;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod tray;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod whiteboard;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod updater;

mod ui_cm_interface;
mod ui_interface;
mod ui_session_interface;

mod hbbs_http;

pub mod privacy_mode;

#[cfg(windows)]
pub mod virtual_display_manager;
