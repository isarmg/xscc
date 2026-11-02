use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Binding {
    pub manager_id: String,
    pub device_id: String,
    pub installation_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Session {
    pub authenticated: bool,
    pub csrf_token: String,
    pub expires_in_secs: u64,
    pub controller: Binding,
    pub controlled: Binding,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManagementStatus {
    pub configured: bool,
    pub phase: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Info {
    pub version: String,
    pub device_info: Option<DeviceInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceInfo {
    pub hostname: String,
    pub cpu_model: String,
    pub cpu_usage: f64,
    pub memory_total: u64,
    pub memory_used: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StreamStatus {
    pub state: String,
    pub device: Option<String>,
    pub fps: f64,
    pub clients: u32,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MsdImage {
    pub id: String,
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DriveFile {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCatalog {
    pub video: Vec<VideoDevice>,
    pub serial: Vec<SerialDevice>,
    pub audio: Vec<AudioDevice>,
    pub udc: Vec<UdcDevice>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UdcDevice {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SetupStatus {
    pub initialized: bool,
    pub needs_setup: bool,
    #[serde(default)]
    pub platform: Platform,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Platform {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub hid: Feature,
    #[serde(default)]
    pub msd: Feature,
    #[serde(default)]
    pub otg: Feature,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Feature {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub backends: Vec<String>,
}

impl Platform {
    pub fn supports_media(&self) -> bool {
        self.mode.is_empty() || self.msd.available
    }

    pub fn supports_otg(&self) -> bool {
        self.mode.is_empty() || self.otg.available
    }

    pub fn supports_hid(&self, backend: &str) -> bool {
        backend == "none"
            || self.hid.backends.iter().any(|value| value == backend)
            || (self.hid.backends.is_empty() && self.mode != "windows" && backend != "windows")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct VideoDevice {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SerialDevice {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MsdStatus {
    pub available: bool,
    pub state: MsdState,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MsdState {
    pub disk_mode: String,
    pub slot_capacity: u32,
    pub mounted_count: u32,
    pub mounted_media: Vec<MountedMedia>,
    pub usb_reenumerating: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MountedMedia {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub cdrom: bool,
    pub read_only: bool,
    pub size: u64,
}

pub fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", value as f64 / 1_073_741_824.)
    } else {
        format!("{:.1} MiB", value as f64 / 1_048_576.)
    }
}
