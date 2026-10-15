#[cfg(all(feature = "capture", quartz))]
extern crate block;
#[macro_use]
extern crate cfg_if;
pub use hbb_common::libc;
#[cfg(all(feature = "capture", dxgi))]
extern crate winapi;

pub use common::*;

#[cfg(all(feature = "capture", quartz))]
pub mod quartz;

#[cfg(all(target_os = "linux", feature = "wayland"))]
pub mod wayland;

#[cfg(all(feature = "capture", dxgi))]
pub mod dxgi;

// JNI codec discovery and client clipboard support also serve codec-only builds.
#[cfg(target_os = "android")]
pub mod android;

mod common;
