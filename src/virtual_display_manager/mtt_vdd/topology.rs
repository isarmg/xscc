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
        let flags = if all {
            QDC_ALL_PATHS
        } else {
            QDC_ONLY_ACTIVE_PATHS
        } | QDC_VIRTUAL_REFRESH_RATE_AWARE;
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
        requested: Option<VirtualDisplayMode>,
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
        let (mut right, y) = topology
            .paths
            .iter()
            .filter_map(|path| topology.modes.get(path.sourceInfo.modeInfoIdx as usize))
            .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            .map(|mode| {
                let source = unsafe { mode.u.sourceMode() };
                let top = i64::from(source.position.y);
                let y = if preferred.is_some_and(|mode| {
                    top < i64::from(mode.height) && top + i64::from(source.height) > 0
                }) {
                    0
                } else {
                    source.position.y
                };
                (i64::from(source.position.x) + i64::from(source.width), y)
            })
            // Retain y=0 when any rightmost source gives it an edge contact.
            .max_by_key(|(right, y)| (*right, *y == 0))
            .unwrap_or((0, 0));
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
                    y,
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
            path.targetInfo.refreshRate = rate(mode.refresh_rate_units);
            path.flags &= !DISPLAYCONFIG_PATH_BOOST_REFRESH_RATE;
            used.insert((
                path.sourceInfo.adapterId.HighPart,
                path.sourceInfo.adapterId.LowPart,
                path.sourceInfo.id,
            ));
            topology.paths.push(path);
        }
        // A fallback activates new paths; only an explicit mode changes existing ones.
        if let Some(mode) = requested {
            let mut right = 0i64;
            let mut preserve_positions = true;
            let mut sources = Vec::new();
            for path in &topology.paths {
                let owned = Self::owned_id(path, ids)?.is_some();
                if let Some(info) = topology.modes.get(path.sourceInfo.modeInfoIdx as usize) {
                    if info.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                        let source = unsafe { info.u.sourceMode() };
                        sources.push((path.sourceInfo.modeInfoIdx, *source, owned));
                        if owned {
                            preserve_positions &=
                                source.width == mode.width && source.height == mode.height;
                        } else {
                            right =
                                right.max(i64::from(source.position.x) + i64::from(source.width));
                        }
                    }
                }
            }
            if !preserve_positions
                && sources.len() == topology.paths.len()
                && used.len() == topology.paths.len()
            {
                let original: Vec<_> = sources.iter().map(|(_, source, _)| *source).collect();
                let resized: Vec<_> = sources
                    .iter()
                    .map(|(_, source, owned)| {
                        let mut source = *source;
                        if *owned {
                            source.width = mode.width;
                            source.height = mode.height;
                        }
                        source
                    })
                    .collect();
                if present.len() == ids.len() && retains_source_positions(&original, &resized) {
                    preserve_positions = true;
                } else if let Some(positions) = reflow_chain(&original, &resized) {
                    for ((index, _, _), position) in sources.iter().zip(positions) {
                        unsafe { topology.modes[*index as usize].u.sourceMode_mut() }.position =
                            position;
                    }
                    preserve_positions = true;
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
                resize_source(source, mode, &mut right, preserve_positions)?;
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
                path.targetInfo.refreshRate = rate(mode.refresh_rate_units);
                path.flags &= !DISPLAYCONFIG_PATH_BOOST_REFRESH_RATE;
            }
        }
        topology.apply()
    }
}

fn resize_source(
    source: &mut DISPLAYCONFIG_SOURCE_MODE,
    mode: VirtualDisplayMode,
    right: &mut i64,
    preserve_positions: bool,
) -> ResultType<()> {
    // A refresh-only change must not move the desktop or change its primary display.
    if !preserve_positions {
        source.position = winapi::shared::windef::POINTL {
            x: i32::try_from(*right)?,
            y: 0,
        };
        *right = right
            .checked_add(i64::from(mode.width))
            .ok_or_else(|| anyhow::anyhow!("Virtual desktop placement overflow"))?;
    }
    source.width = mode.width;
    source.height = mode.height;
    Ok(())
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

// Only use this with the unchanged set of active sources returned by Windows.
// Keeping every original contact preserves that desktop's connected region.
fn retains_source_positions(
    original: &[DISPLAYCONFIG_SOURCE_MODE],
    resized: &[DISPLAYCONFIG_SOURCE_MODE],
) -> bool {
    let bounds = |source: &DISPLAYCONFIG_SOURCE_MODE| {
        let x = i64::from(source.position.x);
        let y = i64::from(source.position.y);
        [
            x,
            y,
            x + i64::from(source.width),
            y + i64::from(source.height),
        ]
    };
    let contacts = |a: [i64; 4], b: [i64; 4]| {
        let horizontal = a[0].max(b[0]) < a[2].min(b[2]);
        let vertical = a[1].max(b[1]) < a[3].min(b[3]);
        [
            vertical && a[2] == b[0],
            vertical && b[2] == a[0],
            horizontal && a[3] == b[1],
            horizontal && b[3] == a[1],
        ]
    };
    for (i, source) in resized.iter().enumerate() {
        let a = bounds(source);
        if a.into_iter().any(|value| i16::try_from(value).is_err()) {
            return false;
        }
        for (j, other) in resized.iter().enumerate().skip(i + 1) {
            let b = bounds(other);
            if a[0].max(b[0]) < a[2].min(b[2]) && a[1].max(b[1]) < a[3].min(b[3]) {
                return false;
            }
            if contacts(bounds(&original[i]), bounds(&original[j]))
                .into_iter()
                .zip(contacts(a, b))
                .any(|(before, after)| before && !after)
            {
                return false;
            }
        }
    }
    true
}

// Reflow only straight chains; ambiguous layouts retain the existing placement.
fn reflow_chain(
    original: &[DISPLAYCONFIG_SOURCE_MODE],
    resized: &[DISPLAYCONFIG_SOURCE_MODE],
) -> Option<Vec<winapi::shared::windef::POINTL>> {
    let primary = original
        .iter()
        .position(|source| source.position.x == 0 && source.position.y == 0)?;
    for vertical in [false, true] {
        let components = |source: &DISPLAYCONFIG_SOURCE_MODE| {
            if vertical {
                [
                    i64::from(source.position.y),
                    i64::from(source.position.x),
                    i64::from(source.height),
                    i64::from(source.width),
                ]
            } else {
                [
                    i64::from(source.position.x),
                    i64::from(source.position.y),
                    i64::from(source.width),
                    i64::from(source.height),
                ]
            }
        };
        let mut order: Vec<_> = (0..original.len()).collect();
        order.sort_by_key(|index| components(&original[*index])[0]);
        if order.windows(2).any(|pair| {
            let a = components(&original[pair[0]]);
            let b = components(&original[pair[1]]);
            let new_a = components(&resized[pair[0]]);
            let new_b = components(&resized[pair[1]]);
            a[0] + a[2] != b[0]
                || (a[1] + a[3]).min(b[1] + b[3]) <= a[1].max(b[1])
                || (new_a[1] + new_a[3]).min(new_b[1] + new_b[3]) <= new_a[1].max(new_b[1])
        }) {
            continue;
        }
        let pivot = order.iter().position(|index| *index == primary)?;
        let mut positions: Vec<_> = resized.iter().map(components).collect();
        for rank in pivot + 1..order.len() {
            let (current, previous) = (order[rank], order[rank - 1]);
            let anchor = positions[previous];
            positions[current][0] = anchor[0] + anchor[2];
        }
        for rank in (0..pivot).rev() {
            let (current, next) = (order[rank], order[rank + 1]);
            let anchor = positions[next];
            positions[current][0] = anchor[0] - positions[current][2];
        }
        // Outside documented GDI desktop bounds, retain the existing native path.
        if positions.iter().any(|p| {
            [p[0], p[1], p[0] + p[2], p[1] + p[3]]
                .into_iter()
                .any(|value| i16::try_from(value).is_err())
        }) {
            return None;
        }
        return positions
            .into_iter()
            .map(|p| {
                Some(winapi::shared::windef::POINTL {
                    x: i32::try_from(if vertical { p[1] } else { p[0] }).ok()?,
                    y: i32::try_from(if vertical { p[0] } else { p[1] }).ok()?,
                })
            })
            .collect();
    }
    None
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
            for (step, hz) in [120, 240, 120, 240].into_iter().enumerate() {
                config.modes[0].refresh_rate_units = hz * 10000;
                let state = if step < 2 {
                    configuration::configure(&config)?
                } else {
                    let name = configuration::state()
                        .displays
                        .into_iter()
                        .find(|screen| screen.active)
                        .ok_or_else(|| anyhow::anyhow!("No active VDD for the refresh menu check"))?
                        .name;
                    let mode = crate::platform::windows::display_modes::supported(&name)
                        .resolutions
                        .into_iter()
                        .find(|mode| {
                            mode.width == 1920 && mode.height == 1080 && mode.refresh_rate == hz
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!("The menu does not offer the declared {hz} Hz mode")
                        })?;
                    crate::platform::windows::display_modes::change(&name, &mode)?;
                    configuration::state()
                };
                if state.active_count != 1
                    || state
                        .displays
                        .iter()
                        .any(|screen| !screen.active || screen.refresh_rate_units != hz * 10000)
                {
                    bail!(
                        "Windows did not confirm the requested {hz} Hz desktop: {}",
                        state.message
                    );
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

    fn source(width: u32, height: u32, x: i32, y: i32) -> DISPLAYCONFIG_SOURCE_MODE {
        DISPLAYCONFIG_SOURCE_MODE {
            width,
            height,
            pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
            position: winapi::shared::windef::POINTL { x, y },
        }
    }

    #[test]
    fn stationary_resize_keeps_l_shaped_and_grid_layouts() {
        for original in [
            vec![
                source(1920, 1080, 0, 0),
                source(1920, 1080, 0, -1080),
                source(1920, 1080, 1920, -1080),
            ],
            vec![
                source(1920, 1080, 1920, 1080),
                source(1920, 1080, 0, 0),
                source(1920, 1080, 1920, 0),
                source(1920, 1080, 0, 1080),
            ],
        ] {
            for (width, height) in [(1280, 720), (2560, 1440)] {
                let mut resized = original.clone();
                resized[0].width = width;
                resized[0].height = height;
                assert!(retains_source_positions(&original, &resized));
            }
        }
    }

    #[test]
    fn stationary_resize_declines_overlap_changed_sides_and_desktop_overflow() {
        let original = [source(100, 100, 0, 0), source(100, 100, 100, 50)];
        let resized = [source(150, 50, 0, 0), original[1]];
        // The same pair still touches, but now on a different edge.
        assert!(!retains_source_positions(&original, &resized));
        let original = [
            source(100, 100, 0, 0),
            source(100, 100, 100, 0),
            source(100, 100, 100, 100),
            source(100, 100, 0, 150),
        ];
        let mut resized = original;
        resized[0].height = 200;
        // Original contacts remain, but the first and last screen now overlap.
        assert!(!retains_source_positions(&original, &resized));
        let original = [source(1920, 1080, 0, 0)];
        assert!(retains_source_positions(
            &original,
            &[source(32767, 1080, 0, 0)]
        ));
        assert!(!retains_source_positions(
            &original,
            &[source(32768, 1080, 0, 0)]
        ));
    }

    #[test]
    fn chain_resize_keeps_primary_order_and_negative_coordinates() {
        let original = [
            source(1920, 1080, -1920, 100),
            source(1920, 1080, 0, 0),
            source(1920, 1080, 1920, 200),
        ];
        for (width, height) in [(1280, 720), (2560, 1440)] {
            let mut resized = original;
            for screen in &mut resized[..2] {
                screen.width = width;
                screen.height = height;
            }
            let positions = reflow_chain(&original, &resized).unwrap();
            assert_eq!(
                positions.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
                [(-(width as i32), 100), (0, 0), (width as i32, 200)]
            );
        }
        let original = [
            source(1920, 1080, 200, -1080),
            source(1920, 1080, 0, 0),
            source(1920, 1080, 100, 1080),
        ];
        let mut resized = original;
        for screen in &mut resized[1..] {
            screen.width = 1280;
            screen.height = 720;
        }
        let positions = reflow_chain(&original, &resized).unwrap();
        assert_eq!(
            positions.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            [(200, -1080), (0, 0), (100, 720)]
        );
    }

    #[test]
    fn chain_reflow_does_not_guess_new_contacts_or_clone_layouts() {
        let original = [source(1920, 1080, 0, 0), source(1920, 1080, 1920, 1000)];
        let positions = reflow_chain(&original, &original).unwrap();
        assert_eq!(
            positions.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            [(0, 0), (1920, 1000)]
        );
        let mut resized = original;
        resized[0] = source(1280, 720, 0, 0);
        assert!(reflow_chain(&original, &resized).is_none());
        let grid = [
            source(1920, 2160, -1920, 0),
            source(1920, 1080, 0, 0),
            source(1920, 1080, 0, 1080),
            source(1920, 2160, 1920, 0),
        ];
        assert!(reflow_chain(&grid, &grid).is_none());
        let clones = [source(1920, 1080, 0, 0); 2];
        assert!(reflow_chain(&clones, &clones).is_none());
    }

    #[test]
    fn chain_reflow_leaves_desktop_bounds_to_the_existing_fallback() {
        let original = [source(1920, 1080, 0, 0), source(1, 1080, 1920, 0)];
        let mut resized = original;
        resized[0].width = 32766;
        assert!(reflow_chain(&original, &resized).is_some());
        resized[0].width = 32767;
        assert!(reflow_chain(&original, &resized).is_none());
        let original = [source(1920, 1080, -1920, 0), source(1, 1080, 0, 0)];
        let mut resized = original;
        resized[0].width = 32768;
        assert_eq!(reflow_chain(&original, &resized).unwrap()[0].x, -32768);
        resized[0].width = 32769;
        assert!(reflow_chain(&original, &resized).is_none());
    }

    #[test]
    fn refresh_only_keeps_the_virtual_primary_and_extended_positions() {
        let mode = VirtualDisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_units: 1200000,
        };
        for (x, y) in [(0, 0), (-1920, 0), (0, -1080)] {
            let mut source = DISPLAYCONFIG_SOURCE_MODE {
                width: 1920,
                height: 1080,
                pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
                position: winapi::shared::windef::POINTL { x, y },
            };
            let mut right = 3840;
            resize_source(&mut source, mode, &mut right, true).unwrap();
            assert_eq!((source.position.x, source.position.y), (x, y));
            assert_eq!(right, 3840);
            resize_source(
                &mut source,
                VirtualDisplayMode {
                    width: 2560,
                    ..mode
                },
                &mut right,
                false,
            )
            .unwrap();
            assert_eq!((source.position.x, source.position.y), (3840, 0));
            assert_eq!((source.width, source.height), (2560, 1080));
            assert_eq!(right, 6400);
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
