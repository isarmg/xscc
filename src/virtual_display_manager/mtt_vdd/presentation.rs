use super::{
    configuration,
    topology::{self, Topology},
    LOCK,
};
use crate::server::video_service::{wait_for_capture_idle, CaptureIdleGuard};
use hbb_common::{anyhow, bail, ResultType};
use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use winapi::um::wingdi::*;

#[derive(Clone)]
struct Preference {
    original_primary: String,
    preferred: String,
}

static PREFERENCE: Mutex<Option<Preference>> = Mutex::new(None);

struct Candidate {
    topology: Topology,
    original_primary: String,
    preferred: String,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim_end_matches('\0')
        .eq_ignore_ascii_case(b.trim_end_matches('\0'))
}

fn source<'a>(
    topology: &'a Topology,
    path: &DISPLAYCONFIG_PATH_INFO,
) -> ResultType<&'a DISPLAYCONFIG_SOURCE_MODE> {
    let mode = topology
        .modes
        .get(path.sourceInfo.modeInfoIdx as usize)
        .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
        .ok_or_else(|| anyhow::anyhow!("Missing active display source mode"))?;
    Ok(unsafe { mode.u.sourceMode() })
}

fn rate(topology: &Topology, path: &DISPLAYCONFIG_PATH_INFO) -> ResultType<u32> {
    let mode = topology
        .modes
        .get(path.targetInfo.modeInfoIdx as usize)
        .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET)
        .ok_or_else(|| anyhow::anyhow!("Missing active display target mode"))?;
    topology::active_refresh_rate(path, &unsafe { mode.u.targetMode() }.targetVideoSignalInfo)
}

fn primary(topology: &Topology) -> ResultType<&DISPLAYCONFIG_PATH_INFO> {
    for path in &topology.paths {
        let position = source(topology, path)?.position;
        if position.x == 0 && position.y == 0 {
            return Ok(path);
        }
    }
    bail!("Windows has no active primary display path")
}

fn candidate(name: &str) -> ResultType<Option<Candidate>> {
    let topology = Topology::query(false)?;
    let mut selected = None;
    for path in &topology.paths {
        if same_name(&Topology::source_name(path)?, name) {
            selected = Some(path);
            break;
        }
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    if Topology::owned_id(selected, &topology::monitor_ids()?)?.is_none() {
        return Ok(None);
    }
    let position = source(&topology, selected)?.position;
    if position.x == 0 && position.y == 0 {
        return Ok(None);
    }
    let current = primary(&topology)?;
    if rate(&topology, selected)? <= rate(&topology, current)? {
        return Ok(None);
    }
    let state = configuration::query()?;
    if state.status != "ready" {
        bail!("Cannot prefer this virtual display: {}", state.message);
    }
    if state.persistent {
        return Ok(None);
    }
    let original_primary = Topology::target_name(current)?;
    let preferred = Topology::target_name(selected)?;
    Ok(Some(Candidate {
        topology,
        original_primary,
        preferred,
    }))
}

pub(super) fn active() -> bool {
    PREFERENCE.lock().unwrap().is_some()
}

pub(super) fn needs_preference(name: &str) -> ResultType<bool> {
    Ok(candidate(name)?.is_some())
}

fn translated_positions(positions: &[(i32, i32)], primary: usize) -> ResultType<Vec<(i32, i32)>> {
    let &(x, y) = positions
        .get(primary)
        .ok_or_else(|| anyhow::anyhow!("Primary display source disappeared"))?;
    positions
        .iter()
        .map(|&(left, top)| {
            Ok((
                left.checked_sub(x)
                    .ok_or_else(|| anyhow::anyhow!("Desktop X translation overflow"))?,
                top.checked_sub(y)
                    .ok_or_else(|| anyhow::anyhow!("Desktop Y translation overflow"))?,
            ))
        })
        .collect()
}

fn apply_primary(mut topology: Topology, target: &str) -> ResultType<()> {
    let mut selected = None;
    let mut indices = BTreeSet::new();
    for path in &topology.paths {
        source(&topology, path)?;
        let index = path.sourceInfo.modeInfoIdx as usize;
        indices.insert(index);
        if same_name(&Topology::target_name(path)?, target) {
            selected = Some(index);
        }
    }
    let selected =
        selected.ok_or_else(|| anyhow::anyhow!("Primary display target disconnected"))?;
    let indices: Vec<_> = indices.into_iter().collect();
    let positions: Vec<_> = indices
        .iter()
        .map(|&index| {
            let position = unsafe { topology.modes[index].u.sourceMode() }.position;
            (position.x, position.y)
        })
        .collect();
    let primary_index = indices
        .iter()
        .position(|&index| index == selected)
        .expect("invariant: selected active source is in the source set");
    let translated = translated_positions(&positions, primary_index)?;
    for (index, (x, y)) in indices.into_iter().zip(translated) {
        unsafe { topology.modes[index].u.sourceMode_mut() }.position =
            winapi::shared::windef::POINTL { x, y };
    }
    let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | topology::SDC_VIRTUAL_REFRESH_RATE_AWARE;
    for operation in [SDC_VALIDATE, SDC_APPLY] {
        let result = unsafe {
            topology::SetDisplayConfig(
                topology.paths.len() as _,
                topology.paths.as_ptr(),
                topology.modes.len() as _,
                topology.modes.as_ptr(),
                flags | operation,
            )
        };
        if result != 0 {
            bail!("Windows rejected the temporary primary display ({result})");
        }
    }
    let actual = Topology::query(false)?;
    if !same_name(&Topology::target_name(primary(&actual)?)?, target) {
        bail!("Windows did not confirm the temporary primary display");
    }
    Ok(())
}

fn capture_names() -> ResultType<Vec<String>> {
    let displays = scrap::Display::all()?;
    Ok(displays.iter().map(|display| display.name()).collect())
}

pub(super) fn prefer_locked(name: &str) -> ResultType<bool> {
    let Some(mut candidate) = candidate(name)? else {
        return Ok(false);
    };
    if active() {
        suspend_locked()?;
        candidate = match self::candidate(name)? {
            Some(candidate) => candidate,
            None => return Ok(false),
        };
    }
    let names = capture_names()?;
    *PREFERENCE.lock().unwrap() = Some(Preference {
        original_primary: candidate.original_primary,
        preferred: candidate.preferred.clone(),
    });
    let applied = (|| -> ResultType<()> {
        apply_primary(candidate.topology, &candidate.preferred)?;
        if capture_names()? != names {
            bail!(
                "Windows changed display capture ordering; automatic high-refresh presentation is unsupported"
            );
        }
        Ok(())
    })();
    if let Err(cause) = applied {
        return match suspend_locked() {
            Ok(_) => Err(cause),
            Err(restoration) => {
                Err(cause.context(format!("Primary restoration also failed: {restoration:#}")))
            }
        };
    }
    Ok(true)
}

pub(super) fn suspend_locked() -> ResultType<Option<String>> {
    let Some(previous) = PREFERENCE.lock().unwrap().clone() else {
        return Ok(None);
    };
    let topology = Topology::query(false)?;
    let mut original_present = false;
    for path in &topology.paths {
        if same_name(&Topology::target_name(path)?, &previous.original_primary) {
            original_present = true;
            break;
        }
    }
    if !original_present {
        *PREFERENCE.lock().unwrap() = None;
        return Ok(None);
    }
    let current = Topology::target_name(primary(&topology)?)?;
    if !same_name(&current, &previous.preferred) {
        *PREFERENCE.lock().unwrap() = None;
        return Ok(None);
    }
    let started = std::time::Instant::now();
    hbb_common::throttled_log!(
        Duration::from_secs(1), info,
        "session_display_cleanup stage=primary_restore_begin"
    );
    let result = apply_primary(topology, &previous.original_primary);
    hbb_common::throttled_log!(
        Duration::from_secs(1), info,
        "session_display_cleanup stage=primary_restore_end elapsed_ms={} success={}",
        started.elapsed().as_millis(), result.is_ok()
    );
    result?;
    *PREFERENCE.lock().unwrap() = None;
    Ok(Some(previous.preferred))
}

pub(super) fn complete_removal(
    restoration: ResultType<Option<String>>,
    removal: ResultType<()>,
) -> ResultType<()> {
    match (restoration, removal) {
        (Ok(_), result) => result,
        (Err(error), Ok(())) => {
            Err(error.context("Display cleanup completed but primary restoration failed"))
        }
        (Err(restoration), Err(removal)) => {
            Err(removal.context(format!("Primary restoration also failed: {restoration:#}")))
        }
    }
}

pub(super) fn resume_locked(target: String) -> ResultType<()> {
    let topology = Topology::query(false)?;
    for path in &topology.paths {
        if same_name(&Topology::target_name(path)?, &target) {
            prefer_locked(&Topology::source_name(path)?)?;
            break;
        }
    }
    Ok(())
}

pub(crate) struct PersistentDisplayChangeGuard {
    preferred: Option<String>,
    _lock: MutexGuard<'static, ()>,
    _idle: CaptureIdleGuard<'static>,
}

impl PersistentDisplayChangeGuard {
    pub(crate) fn finish(self) -> ResultType<()> {
        if let Some(target) = self.preferred {
            resume_locked(target)?;
        }
        Ok(())
    }
}

pub(super) fn before_persistent_change() -> ResultType<Option<PersistentDisplayChangeGuard>> {
    let lock = LOCK.lock().unwrap();
    if PREFERENCE.lock().unwrap().is_none() {
        return Ok(None);
    }
    let idle = wait_for_capture_idle(Duration::from_secs(5)).map_err(anyhow::Error::msg)?;
    let preferred = suspend_locked()?;
    Ok(Some(PersistentDisplayChangeGuard {
        preferred,
        _lock: lock,
        _idle: idle,
    }))
}

#[cfg(test)]
mod tests {
    use super::{anyhow, complete_removal, translated_positions};

    #[test]
    fn removal_result_keeps_both_restore_and_removal_failures() {
        let restoration = Err(anyhow::anyhow!("primary restore failed"));
        let error = complete_removal(restoration, Err(anyhow::anyhow!("device removal failed")))
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("primary restore failed"));
        assert!(message.contains("device removal failed"));
        assert!(complete_removal(Err(anyhow::anyhow!("primary restore failed")), Ok(())).is_err());
    }

    #[test]
    fn temporary_primary_preserves_extended_desktop_geometry() {
        assert_eq!(
            translated_positions(&[(0, 0), (1920, 120), (-1280, -80)], 1).unwrap(),
            vec![(-1920, -120), (0, 0), (-3200, -200)]
        );
    }

    #[test]
    fn restoring_current_layout_accepts_reordered_and_rebuilt_sources() {
        assert_eq!(
            translated_positions(&[(-3200, -200), (0, 0), (-1920, -120)], 2).unwrap(),
            vec![(-1280, -80), (1920, 120), (0, 0)]
        );
        assert!(translated_positions(&[(i32::MIN, 0), (i32::MAX, 0)], 1).is_err());
    }
}
