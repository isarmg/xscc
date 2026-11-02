pub mod config;
pub mod devices;
pub mod extensions;
pub mod terminal;

mod atx_api;
mod audio_api;
mod computer_use;
mod hid_api;
mod inventory;
#[cfg(unix)]
mod msd_api;
mod setup;
mod stream;
mod system;
mod update_api;
mod webrtc;

pub use atx_api::*;
pub use audio_api::*;
pub use computer_use::*;
pub use hid_api::*;
pub use inventory::*;
#[cfg(unix)]
pub use msd_api::*;
pub use setup::*;
pub use stream::*;
pub use system::*;
pub use update_api::*;
pub use webrtc::*;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::config::StreamMode;
use crate::diagnostics::{get_device_info, get_disk_space, DeviceInfo, DiskSpaceInfo};
use crate::error::{AppError, Result};
use crate::platform::PlatformCapabilities;
use crate::runtime::ConfigApplyOptions;
use crate::state::AppState;
use crate::update::{UpdateChannel, UpdateOverviewResponse, UpdateStatusResponse};
use crate::utils::list_serial_ports;
use crate::video::codec::{
    build_hardware_self_check_runtime_error, run_hardware_self_check, BitratePreset,
    VideoEncoderSelfCheckResponse,
};
use crate::video::codec_constraints::codec_to_id;

#[derive(serde::Serialize)]
pub struct LoginResponse {
    pub success: bool,
    pub message: Option<String>,
}
