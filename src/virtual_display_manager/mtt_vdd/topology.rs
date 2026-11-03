use super::*;
use base::session_protocol::virtual_display::{VirtualDisplayMode, VirtualDisplayScreen};
use std::collections::{BTreeMap, BTreeSet};
use winapi::um::{cfgmgr32::CM_Get_Parent, wingdi::*};

const QDC_VIRTUAL_REFRESH_RATE_AWARE: u32 = 0x40;
const SDC_VIRTUAL_REFRESH_RATE_AWARE: u32 = 0x20000;
const DISPLAYCONFIG_PATH_BOOST_REFRESH_RATE: u32 = 0x10;

/// A newly enumerated PnP monitor can precede its Windows desktop path.
#[derive(Debug)]
pub(super) struct MonitorPathPending(String);

impl std::fmt::Display for MonitorPathPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Windows 尚未提供虚拟显示器的桌面路径：{}", self.0)
    }
}
impl std::error::Error for MonitorPathPending {}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
    fn QueryDisplayConfig(
        flags: u32,
        paths: *mut u32,
        path_info: *mut DISPLAYCONFIG_PATH_INFO,
        modes: *mut u32,
        mode_info: *mut DISPLAYCONFIG_MODE_INFO,
        topology: *mut u32,
    ) -> i32;
    fn DisplayConfigGetDeviceInfo(header: *mut DISPLAYCONFIG_DEVICE_INFO_HEADER) -> i32;
    fn SetDisplayConfig(
        paths: u32,
        path_info: *const DISPLAYCONFIG_PATH_INFO,
        modes: u32,
        mode_info: *const DISPLAYCONFIG_MODE_INFO,
        flags: u32,
    ) -> i32;
}

fn text(value: &[u16]) -> String {
    String::from_utf16_lossy(value)
        .trim_end_matches('\0')
        .into()
}
fn normalized(value: &str) -> String {
    value
        .trim_end_matches('\0')
        .trim_start_matches(r"\\?\")
        .replace('#', "\\")
        .to_ascii_lowercase()
}
fn matches_id(path: &str, id: &str) -> bool {
    let path = normalized(path);
    let id = normalized(id);
    path == id || path.starts_with(&(id + "\\{"))
}

pub(super) fn monitor_ids() -> ResultType<Vec<String>> {
    let Some((_, root)) = device()? else {
        return Ok(Vec::new());
    };
    let monitor_class = GUID {
        Data1: 0x4d36e96e,
        Data2: 0xe325,
        Data3: 0x11ce,
        Data4: [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
    };
    let handle = unsafe { SetupDiGetClassDevsW(&monitor_class, null(), null_mut(), DIGCF_PRESENT) };
    if handle == INVALID_HANDLE_VALUE {
        bail!(
            "Enumerate VDD monitors failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let devices = Devices(handle);
    let mut ids = BTreeSet::new();
    for index in 0..4096 {
        let mut data: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as _;
        if unsafe { SetupDiEnumDeviceInfo(devices.0, index, &mut data) } == FALSE {
            if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NO_MORE_ITEMS as _) {
                return Ok(ids.into_iter().collect());
            }
            bail!(
                "Read VDD monitor failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let mut node = data.DevInst;
        let mut owned = false;
        for _ in 0..32 {
            if node == root.DevInst {
                owned = true;
                break;
            }
            let mut parent = 0;
            if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
                break;
            }
            node = parent;
        }
        if !owned {
            continue;
        }
        let mut id = [0u16; 512];
        if unsafe {
            SetupDiGetDeviceInstanceIdW(
                devices.0,
                &mut data,
                id.as_mut_ptr(),
                id.len() as _,
                null_mut(),
            )
        } == FALSE
        {
            bail!(
                "Read VDD monitor identity failed: {}",
                std::io::Error::last_os_error()
            );
        }
        ids.insert(text(&id));
    }
    bail!("VDD monitor enumeration exceeded its resource bound")
}

#[derive(Clone)]
pub(super) struct Topology {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

impl Topology {
    pub(super) fn query(all: bool) -> ResultType<Self> {
        let flags = (if all {
            QDC_ALL_PATHS
        } else {
            QDC_ONLY_ACTIVE_PATHS
        }) | QDC_VIRTUAL_REFRESH_RATE_AWARE;
        for _ in 0..4 {
            let (mut paths, mut modes) = (0, 0);
            let result = unsafe { GetDisplayConfigBufferSizes(flags, &mut paths, &mut modes) };
            if result != 0 {
                bail!("Read Windows display topology size failed ({result})");
            }
            if paths > 16384 || modes > 65536 {
                bail!("Windows display topology exceeds its resource bound");
            }
            let mut path_info = vec![unsafe { std::mem::zeroed() }; paths as usize];
            let mut mode_info = vec![unsafe { std::mem::zeroed() }; modes as usize];
            let result = unsafe {
                QueryDisplayConfig(
                    flags,
                    &mut paths,
                    path_info.as_mut_ptr(),
                    &mut modes,
                    mode_info.as_mut_ptr(),
                    null_mut(),
                )
            };
            if result == 122 {
                continue;
            }
            if result != 0 {
                bail!("Read Windows display topology failed ({result})");
            }
            path_info.truncate(paths as usize);
            mode_info.truncate(modes as usize);
            return Ok(Self {
                paths: path_info,
                modes: mode_info,
            });
        }
        bail!("Windows display topology kept changing; retry the operation")
    }

    fn source_name(path: &DISPLAYCONFIG_PATH_INFO) -> ResultType<String> {
        let mut name: DISPLAYCONFIG_SOURCE_DEVICE_NAME = unsafe { std::mem::zeroed() };
        name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            _type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: std::mem::size_of_val(&name) as _,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        };
        let result = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
        if result != 0 {
            bail!("Read display source name failed ({result})");
        }
        Ok(text(&name.viewGdiDeviceName))
    }
    fn target_name(path: &DISPLAYCONFIG_PATH_INFO) -> ResultType<String> {
        let mut name: DISPLAYCONFIG_TARGET_DEVICE_NAME = unsafe { std::mem::zeroed() };
        name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            _type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: std::mem::size_of_val(&name) as _,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        };
        let result = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
        if result != 0 {
            bail!("Read display target name failed ({result})");
        }
        Ok(text(&name.monitorDevicePath))
    }
    fn owned_id(path: &DISPLAYCONFIG_PATH_INFO, ids: &[String]) -> ResultType<Option<String>> {
        let target = Self::target_name(path)?;
        Ok(ids.iter().find(|id| matches_id(&target, id)).cloned())
    }

    pub(super) fn screens(&self, ids: &[String]) -> ResultType<Vec<VirtualDisplayScreen>> {
        let mut values: BTreeMap<String, VirtualDisplayScreen> = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    VirtualDisplayScreen {
                        id: id.clone(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let capture = scrap::Display::all()?;
        for path in &self.paths {
            if path.targetInfo.targetAvailable == FALSE {
                continue;
            }
            let Some(id) = Self::owned_id(path, ids)? else {
                continue;
            };
            let active = path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0;
            if values.get(&id).is_some_and(|screen| screen.active) {
                continue;
            }
            let name = Self::source_name(path)?;
            let index = capture
                .iter()
                .position(|display| normalized(&display.name()) == normalized(&name));
            let mut screen = VirtualDisplayScreen {
                id: id.clone(),
                name,
                active,
                index,
                ..Default::default()
            };
            if active {
                if let Some(mode) = self.modes.get(path.sourceInfo.modeInfoIdx as usize) {
                    if mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                        let source = unsafe { mode.u.sourceMode() };
                        screen.width = source.width;
                        screen.height = source.height;
                    }
                }
                let target = self
                    .modes
                    .get(path.targetInfo.modeInfoIdx as usize)
                    .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET)
                    .ok_or_else(|| anyhow::anyhow!("Missing active VDD target signal"))?;
                let signal = &unsafe { target.u.targetMode() }.targetVideoSignalInfo;
                screen.refresh_rate_units = active_refresh_rate(path, signal)?;
            }
            values.insert(id, screen);
        }
        Ok(values.into_values().collect())
    }

    pub(super) fn restore(&self) -> ResultType<()> {
        self.apply()
    }

    fn apply(&self) -> ResultType<()> {
        let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_VIRTUAL_REFRESH_RATE_AWARE;
        let result = unsafe {
            SetDisplayConfig(
                self.paths.len() as _,
                self.paths.as_ptr(),
                self.modes.len() as _,
                self.modes.as_ptr(),
                flags | SDC_VALIDATE,
            )
        };
        if result != 0 {
            bail!("Windows rejected the requested display topology ({result})");
        }
        let result = unsafe {
            SetDisplayConfig(
                self.paths.len() as _,
                self.paths.as_ptr(),
                self.modes.len() as _,
                self.modes.as_ptr(),
                flags | SDC_APPLY | SDC_SAVE_TO_DATABASE,
            )
        };
        if result != 0 {
            bail!("Apply Windows display topology failed ({result})");
        }
        Ok(())
    }

    pub(super) fn configure(
        ids: &[String],
        preferred: Option<VirtualDisplayMode>,
    ) -> ResultType<()> {
        let mut topology = Self::query(true)?;
        let all = topology.paths.clone();
        topology
            .paths
            .retain(|path| path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0);
        let mut used: BTreeSet<_> = topology
            .paths
            .iter()
            .map(|path| {
                (
                    path.sourceInfo.adapterId.HighPart,
                    path.sourceInfo.adapterId.LowPart,
                    path.sourceInfo.id,
                )
            })
            .collect();
        let mut present = BTreeSet::new();
        for path in &topology.paths {
            if let Some(id) = Self::owned_id(path, ids)? {
                present.insert(id);
            }
        }
        let mut right = topology
            .modes
            .iter()
            .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            .map(|mode| {
                let source = unsafe { mode.u.sourceMode() };
                i64::from(source.position.x) + i64::from(source.width)
            })
            .max()
            .unwrap_or(0);
        for id in ids {
            if present.contains(id) {
                continue;
            }
            let candidate = all
                .iter()
                .find(|path| {
                    !used.contains(&(
                        path.sourceInfo.adapterId.HighPart,
                        path.sourceInfo.adapterId.LowPart,
                        path.sourceInfo.id,
                    )) && Self::owned_id(path, std::slice::from_ref(id))
                        .is_ok_and(|value| value.is_some())
                })
                .ok_or_else(|| MonitorPathPending(id.clone()))?;
            let mut path = *candidate;
            path.flags |= DISPLAYCONFIG_PATH_ACTIVE;
            path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
            path.targetInfo.scaling = DISPLAYCONFIG_SCALING_IDENTITY;
            path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
            let mode = preferred.ok_or_else(|| {
                anyhow::anyhow!("No preferred VDD mode is available to activate a new display")
            })?;
            let mut source: DISPLAYCONFIG_MODE_INFO = unsafe { std::mem::zeroed() };
            source.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
            source.id = path.sourceInfo.id;
            source.adapterId = path.sourceInfo.adapterId;
            *unsafe { source.u.sourceMode_mut() } = DISPLAYCONFIG_SOURCE_MODE {
                width: mode.width,
                height: mode.height,
                pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
                position: winapi::shared::windef::POINTL {
                    x: i32::try_from(right)?,
                    y: 0,
                },
            };
            right = right
                .checked_add(i64::from(mode.width))
                .ok_or_else(|| anyhow::anyhow!("Virtual desktop placement overflow"))?;
            path.sourceInfo.modeInfoIdx = u32::try_from(topology.modes.len())?;
            topology.modes.push(source);
            let mut target: DISPLAYCONFIG_MODE_INFO = unsafe { std::mem::zeroed() };
            target.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_TARGET;
            target.id = path.targetInfo.id;
            target.adapterId = path.targetInfo.adapterId;
            *unsafe { target.u.targetMode_mut() } = target_mode(mode)?;
            path.targetInfo.modeInfoIdx = u32::try_from(topology.modes.len())?;
            topology.modes.push(target);
            path.flags &= !DISPLAYCONFIG_PATH_BOOST_REFRESH_RATE;
            path.targetInfo.refreshRate = rate(mode.refresh_rate_units);
            used.insert((
                path.sourceInfo.adapterId.HighPart,
                path.sourceInfo.adapterId.LowPart,
                path.sourceInfo.id,
            ));
            topology.paths.push(path);
        }
        if let Some(mode) = preferred {
            let mut right = 0i64;
            for path in &topology.paths {
                if Self::owned_id(path, ids)?.is_none() {
                    if let Some(info) = topology.modes.get(path.sourceInfo.modeInfoIdx as usize) {
                        if info.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                            let source = unsafe { info.u.sourceMode() };
                            right =
                                right.max(i64::from(source.position.x) + i64::from(source.width));
                        }
                    }
                }
            }
            for path in &mut topology.paths {
                if Self::owned_id(path, ids)?.is_none() {
                    continue;
                }
                let source = topology
                    .modes
                    .get_mut(path.sourceInfo.modeInfoIdx as usize)
                    .ok_or_else(|| anyhow::anyhow!("Missing VDD source mode"))?;
                if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                    bail!("Invalid VDD source mode");
                }
                let source = unsafe { source.u.sourceMode_mut() };
                source.width = mode.width;
                source.height = mode.height;
                source.position = winapi::shared::windef::POINTL {
                    x: i32::try_from(right)?,
                    y: 0,
                };
                right = right
                    .checked_add(i64::from(mode.width))
                    .ok_or_else(|| anyhow::anyhow!("Virtual desktop placement overflow"))?;
                path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
                path.targetInfo.scaling = DISPLAYCONFIG_SCALING_IDENTITY;
                path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
                let target = topology
                    .modes
                    .get_mut(path.targetInfo.modeInfoIdx as usize)
                    .ok_or_else(|| anyhow::anyhow!("Missing VDD target mode"))?;
                if target.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_TARGET {
                    bail!("Invalid VDD target mode");
                }
                *unsafe { target.u.targetMode_mut() } = target_mode(mode)?;
                path.flags &= !DISPLAYCONFIG_PATH_BOOST_REFRESH_RATE;
                path.targetInfo.refreshRate = rate(mode.refresh_rate_units);
            }
        }
        topology.apply()
    }
}

fn active_refresh_rate(
    path: &DISPLAYCONFIG_PATH_INFO,
    signal: &DISPLAYCONFIG_VIDEO_SIGNAL_INFO,
) -> ResultType<u32> {
    let rate = signal.vSyncFreq;
    if rate.Numerator == 0 || rate.Denominator == 0 {
        bail!("Windows reported an invalid active VDD signal rate");
    }
    let divider = unsafe { signal.u.AdditionalSignalInfo() }
        .vSyncFreqDivider()
        .max(1);
    let denominator = u64::from(rate.Denominator) * u64::from(divider);
    let signal_units =
        u32::try_from((u64::from(rate.Numerator) * 10000 + denominator / 2) / denominator)?;
    // Windows 11 can expose a 60 Hz desktop on a faster target signal.
    let desktop = path.targetInfo.refreshRate;
    if desktop.Numerator == 0 || desktop.Denominator == 0 {
        bail!("Windows reported an invalid active VDD desktop refresh rate");
    }
    let denominator = u64::from(desktop.Denominator);
    let desktop_units =
        u32::try_from((u64::from(desktop.Numerator) * 10000 + denominator / 2) / denominator)?;
    Ok(desktop_units.min(signal_units))
}

fn rate(units: u32) -> DISPLAYCONFIG_RATIONAL {
    let (mut a, mut b) = (units, 10000);
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    DISPLAYCONFIG_RATIONAL {
        Numerator: units / a,
        Denominator: 10000 / a,
    }
}
fn target_mode(mode: VirtualDisplayMode) -> ResultType<DISPLAYCONFIG_TARGET_MODE> {
    mode.validate()?;
    let rate = rate(mode.refresh_rate_units);
    let mut target: DISPLAYCONFIG_TARGET_MODE = unsafe { std::mem::zeroed() };
    let signal = &mut target.targetVideoSignalInfo;
    signal.activeSize = DISPLAYCONFIG_2DREGION {
        cx: mode.width,
        cy: mode.height,
    };
    signal.totalSize = signal.activeSize;
    signal.vSyncFreq = rate;
    signal.hSyncFreq = DISPLAYCONFIG_RATIONAL {
        Numerator: rate.Numerator.checked_mul(mode.height).ok_or_else(|| {
            anyhow::anyhow!("VDD horizontal signal rate exceeds the driver representation")
        })?,
        Denominator: rate.Denominator,
    };
    signal.pixelRate = u64::from(rate.Numerator)
        .checked_mul(u64::from(mode.width))
        .and_then(|value| value.checked_mul(u64::from(mode.height)))
        .ok_or_else(|| anyhow::anyhow!("VDD pixel signal rate overflow"))?
        / u64::from(rate.Denominator);
    signal.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
    let additional = unsafe { signal.u.AdditionalSignalInfo_mut() };
    additional.set_videoStandard(255);
    additional.set_vSyncFreqDivider(1);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "creates a display only on an explicitly enabled disposable GitHub runner"]
    fn disposable_runner_applies_high_refresh_and_removes_the_display() -> ResultType<()> {
        use base::session_protocol::virtual_display::VirtualDisplayConfiguration;
        use winapi::um::winuser::{EnumDisplaySettingsW, ENUM_CURRENT_SETTINGS};

        for (name, expected) in [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("RUNNER_OS", "Windows"),
            ("XSCC_VALIDATE_VIRTUAL_DISPLAY_DRIVER", "true"),
        ] {
            if std::env::var(name).as_deref() != Ok(expected) {
                bail!("Hardware check requires an explicitly enabled disposable Windows runner");
            }
        }
        let original = Topology::query(false)?;
        if original.paths.is_empty() || device()?.is_some() || registered_path()?.is_some() {
            bail!("Refusing hardware check without an existing desktop or over an existing VDD");
        }
        let mut config = VirtualDisplayConfiguration {
            count: 1,
            modes: vec![VirtualDisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate_units: 1200000,
            }],
            persistent: false,
        };
        let check = (|| -> ResultType<()> {
            for hz in [120, 240] {
                config.modes[0].refresh_rate_units = hz * 10000;
                let state = configuration::configure(&config)?;
                if state.active_count != 1
                    || state
                        .displays
                        .iter()
                        .any(|screen| !screen.active || screen.refresh_rate_units != hz * 10000)
                {
                    bail!("Windows did not confirm the requested {hz} Hz desktop");
                }
                for screen in &state.displays {
                    let name = wide(OsStr::new(&screen.name));
                    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
                    mode.dmSize = std::mem::size_of_val(&mode) as _;
                    if unsafe {
                        EnumDisplaySettingsW(name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode)
                    } == 0
                        || (mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency)
                            != (1920, 1080, hz)
                    {
                        bail!("Windows current DEVMODE did not confirm {hz} Hz");
                    }
                }
                println!("VDD_CI_DESKTOP_HZ={hz}");
            }
            Ok(())
        })();
        config.count = 0;
        let cleanup = configuration::configure(&config);
        let state = cleanup?;
        check?;
        if state.active_count != 0 || state.present_count != 0 || state.status != "disabled" {
            bail!("VDD remained present or active after the removal request");
        }
        println!("VDD_CI_REMOVED=0");
        Ok(())
    }

    #[test]
    fn desktop_refresh_does_not_inherit_a_faster_target_signal() {
        let target = target_mode(VirtualDisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_units: 2400000,
        })
        .unwrap();
        let mut path: DISPLAYCONFIG_PATH_INFO = unsafe { std::mem::zeroed() };
        path.targetInfo.refreshRate = rate(600000);
        assert_eq!(
            active_refresh_rate(&path, &target.targetVideoSignalInfo).unwrap(),
            600000
        );
        for hz in [120, 240] {
            path.targetInfo.refreshRate = rate(hz * 10000);
            assert_eq!(
                active_refresh_rate(&path, &target.targetVideoSignalInfo).unwrap(),
                hz * 10000
            );
        }
    }

    #[test]
    fn high_refresh_target_modes_preserve_the_idd_desktop_update_rate() {
        for hz in [120, 240] {
            let target = target_mode(VirtualDisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate_units: hz * 10000,
            })
            .unwrap();
            let signal = target.targetVideoSignalInfo;
            let additional = unsafe { signal.u.AdditionalSignalInfo() };
            assert_eq!(additional.videoStandard(), 255);
            assert_eq!(additional.vSyncFreqDivider(), 1);
            assert_eq!(
                signal.vSyncFreq.Numerator,
                hz * signal.vSyncFreq.Denominator * additional.vSyncFreqDivider()
            );
        }
    }
}
