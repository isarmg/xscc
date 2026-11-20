use crate::{
    platform::macos_virtual_display::{self, Mode, VirtualDisplay},
    privacy_mode,
    server::display_service,
};
use base::{
    message_proto::{Resolution, SupportedResolutions},
    session_protocol::virtual_display::{
        VirtualDisplayConfiguration, VirtualDisplayMode, VirtualDisplayScreen, VirtualDisplayState,
        PROTOCOL_VERSION,
    },
};
use hbb_common::{bail, ResultType};
use std::{
    num::NonZeroU32,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

const DRIVER: &str = "cg_virtual_display";
const MAX_DISPLAYS: u32 = 4;
const MAX_MODES: usize = 32;
const REMOVAL_PENDING: &str = "macOS has not confirmed removal of the virtual displays";
const DEFAULT_MODES: [VirtualDisplayMode; 3] = [
    VirtualDisplayMode {
        width: 1920,
        height: 1080,
        refresh_rate_units: 600_000,
    },
    VirtualDisplayMode {
        width: 2560,
        height: 1440,
        refresh_rate_units: 600_000,
    },
    VirtualDisplayMode {
        width: 3840,
        height: 2160,
        refresh_rate_units: 600_000,
    },
];

struct Configuration {
    modes: Vec<VirtualDisplayMode>,
    displays: Vec<Arc<VirtualDisplay>>,
}

#[derive(Default)]
struct Manager {
    configuration: Option<Configuration>,
    retiring: Vec<Arc<VirtualDisplay>>,
    serial: u32,
}

static MANAGER: LazyLock<Mutex<Manager>> = LazyLock::new(Mutex::default);
// Serialize WindowServer changes. Querying the inventory does not take this lock.
static CHANGE: Mutex<()> = Mutex::new(());

pub(crate) fn before_privacy_change() -> ResultType<MutexGuard<'static, ()>> {
    CHANGE
        .try_lock()
        .map_err(|_| hbb_common::anyhow::anyhow!("Virtual display configuration is in progress"))
}

fn validate_configuration(config: &VirtualDisplayConfiguration) -> ResultType<()> {
    if config.persistent {
        bail!("macOS virtual displays are available for the current host process only");
    }
    if config.count > MAX_DISPLAYS || config.modes.len() > MAX_MODES {
        bail!("macOS supports up to {MAX_DISPLAYS} XSCC virtual displays and {MAX_MODES} modes");
    }
    for mode in &config.modes {
        if !(320..=7680).contains(&mode.width)
            || !(200..=4320).contains(&mode.height)
            || !(100_000..=2_400_000).contains(&mode.refresh_rate_units)
        {
            bail!("macOS virtual display modes must be within 320×200 to 7680×4320 and 10–240 Hz");
        }
    }
    Ok(())
}

fn base_state() -> VirtualDisplayState {
    VirtualDisplayState {
        protocol_version: PROTOCOL_VERSION,
        driver: DRIVER.into(),
        maximum_count: Some(MAX_DISPLAYS),
        ..VirtualDisplayState::default()
    }
}

fn query() -> ResultType<VirtualDisplayState> {
    let mut state = base_state();
    if !macos_virtual_display::available() {
        state.message =
            "Virtual displays require macOS 27 or later and an available CGVirtualDisplay API"
                .into();
        return Ok(state);
    }

    let (mut managed_displays, retiring) = {
        let manager = MANAGER.lock().unwrap();
        let configured = match &manager.configuration {
            Some(configuration) => {
                state.modes = configuration.modes.clone();
                configuration.displays.clone()
            }
            None => {
                state.modes = DEFAULT_MODES.to_vec();
                Vec::new()
            }
        };
        (configured, manager.retiring.clone())
    };
    state.status = "ready".into();
    state.message.clear();
    state.configured_count = u32::try_from(managed_displays.len())?;
    let retiring = retiring.into_iter().filter(|display| display.is_online());
    managed_displays.extend(retiring);
    if managed_displays.len() > state.configured_count as usize {
        state.status = "error".into();
        state.message = REMOVAL_PENDING.into();
    }
    if managed_displays.is_empty() {
        return Ok(state);
    }

    let displays = display_service::try_get_displays()?;
    for managed in managed_displays {
        let name = managed.id().to_string();
        let index = displays.iter().position(|display| {
            display.name() == name && display.is_online() && managed.is_online()
        });
        let active = index.is_some() && managed.is_active();
        let mode = if active { Some(managed.mode()?) } else { None };
        state.present_count += u32::from(index.is_some());
        state.active_count += u32::from(active);
        state.displays.push(VirtualDisplayScreen {
            id: name.clone(),
            name,
            index: index.filter(|_| active),
            active,
            width: mode.map_or(0, |mode| mode.width),
            height: mode.map_or(0, |mode| mode.height),
            refresh_rate_units: mode.map_or(0, |mode| mode.refresh_units),
        });
    }
    Ok(state)
}

pub(crate) fn configuration_state() -> VirtualDisplayState {
    query().unwrap_or_else(|error| VirtualDisplayState {
        status: "error".into(),
        message: format!("{error:#}"),
        ..base_state()
    })
}

pub(crate) fn get_platform_additions() -> serde_json::Map<String, serde_json::Value> {
    let mut additions = serde_json::Map::new();
    if macos_virtual_display::available() {
        additions.insert(
            "virtual_display_configuration".into(),
            serde_json::json!(configuration_state()),
        );
    }
    additions
}

pub(crate) fn prepare_configuration() -> ResultType<VirtualDisplayState> {
    query()
}

pub(crate) fn supported_resolutions(name: &str) -> Option<SupportedResolutions> {
    let (display, modes) = managed_display(name)?;
    let mut resolutions = modes.into_iter().map(to_resolution).collect::<Vec<_>>();
    resolutions.sort_by_key(|mode| (mode.width, mode.height, mode.refresh_rate));
    resolutions.dedup();
    let current = display.mode().ok().map(|mode| {
        to_resolution(VirtualDisplayMode {
            width: mode.width,
            height: mode.height,
            refresh_rate_units: mode.refresh_units,
        })
    });
    Some(SupportedResolutions {
        resolutions,
        current_resolution: current.into(),
        ..Default::default()
    })
}

pub(crate) fn change_session_mode(
    name: &str,
    requested: &Resolution,
    authorize: impl FnOnce() -> ResultType<()>,
) -> Option<ResultType<()>> {
    managed_display(name)?;
    Some((|| {
        let _change = CHANGE.lock().unwrap();
        authorize()?;
        require_privacy_off()?;
        let (display, modes) = managed_display(name)
            .ok_or_else(|| hbb_common::anyhow::anyhow!("The virtual display has been removed"))?;
        let current = display.mode()?;
        let mode = modes
            .into_iter()
            .filter(|mode| {
                let wire = to_resolution(*mode);
                wire.width == requested.width
                    && wire.height == requested.height
                    && (requested.refresh_rate == 0 || wire.refresh_rate == requested.refresh_rate)
            })
            .min_by_key(|mode| mode.refresh_rate_units.abs_diff(current.refresh_units))
            .ok_or_else(|| {
                hbb_common::anyhow::anyhow!("The requested virtual display mode is unavailable")
            })?;
        display.set_mode(native_mode(&mode))?;
        Ok(())
    })())
}

fn managed_display(name: &str) -> Option<(Arc<VirtualDisplay>, Vec<VirtualDisplayMode>)> {
    let id = name.parse::<u32>().ok()?;
    let manager = MANAGER.lock().unwrap();
    let configuration = manager.configuration.as_ref()?;
    let display = configuration
        .displays
        .iter()
        .find(|display| display.id() == id)?;
    Some((display.clone(), configuration.modes.clone()))
}

fn to_resolution(mode: VirtualDisplayMode) -> Resolution {
    Resolution {
        width: mode.width as i32,
        height: mode.height as i32,
        refresh_rate: ((u64::from(mode.refresh_rate_units) + 5000) / 10_000) as u32,
        ..Default::default()
    }
}

fn native_mode(mode: &VirtualDisplayMode) -> Mode {
    Mode {
        width: mode.width,
        height: mode.height,
        refresh_units: mode.refresh_rate_units,
    }
}

fn require_privacy_off() -> ResultType<()> {
    if privacy_mode::get_privacy_mode_conn_id()
        .is_some_and(|id| id != privacy_mode::INVALID_PRIVACY_MODE_CONN_ID)
    {
        bail!("Turn off privacy mode before changing macOS virtual displays");
    }
    Ok(())
}

pub(crate) fn configure_displays(
    config: &VirtualDisplayConfiguration,
) -> ResultType<VirtualDisplayState> {
    configure_session_displays(config, || Ok(()))
}

pub(crate) fn configure_session_displays(
    config: &VirtualDisplayConfiguration,
    authorize: impl Fn() -> ResultType<()>,
) -> ResultType<VirtualDisplayState> {
    validate_configuration(config)?;
    if !macos_virtual_display::available() {
        bail!("Virtual displays require macOS 27 or later and an available CGVirtualDisplay API");
    }
    let _change = CHANGE.lock().unwrap();
    authorize()?;
    require_privacy_off()?;
    confirm_removal()?;

    let (modes, previous) = {
        let manager = MANAGER.lock().unwrap();
        let mut modes = if config.modes.is_empty() {
            manager
                .configuration
                .as_ref()
                .map_or_else(|| DEFAULT_MODES.to_vec(), |previous| previous.modes.clone())
        } else {
            config.modes.clone()
        };
        let mut seen = std::collections::BTreeSet::new();
        modes.retain(|mode| seen.insert(*mode));
        let displays = manager
            .configuration
            .as_ref()
            .filter(|previous| previous.modes == modes)
            .map(|previous| previous.displays.clone())
            .unwrap_or_default();
        (modes, displays)
    };
    let count = usize::try_from(config.count)?;
    let retained = previous
        .iter()
        .take(count)
        .take_while(|display| display.is_active())
        .count();
    drop(previous);
    let native_modes = modes.iter().map(native_mode).collect::<Vec<_>>();
    // Hold existing displays until all new displays are active so creation failures leave
    // the previous configuration in place.
    let mut created = Vec::with_capacity(count - retained);
    for index in retained..count {
        authorize()?;
        let serial = {
            let mut manager = MANAGER.lock().unwrap();
            let serial = manager
                .serial
                .checked_add(1)
                .and_then(NonZeroU32::new)
                .ok_or_else(|| {
                    hbb_common::anyhow::anyhow!("Virtual display serial numbers exhausted")
                })?;
            manager.serial = serial.get();
            serial
        };
        created.push(Arc::new(VirtualDisplay::create(
            &format!("XSCC Virtual Display {}", index + 1),
            serial,
            &native_modes,
        )?));
    }
    authorize()?;

    {
        let mut manager = MANAGER.lock().unwrap();
        let mut displays = manager
            .configuration
            .take()
            .map(|previous| previous.displays)
            .unwrap_or_default();
        manager.retiring = displays.split_off(retained);
        displays.extend(created);
        manager.configuration = Some(Configuration { modes, displays });
    }

    let removal = confirm_removal();
    display_service::check_displays_changed()?;
    removal?;
    query()
}

fn confirm_removal() -> ResultType<()> {
    let displays = MANAGER.lock().unwrap().retiring.clone();
    for display in &displays {
        display.close();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while displays.iter().any(|display| display.is_online()) {
        if Instant::now() >= deadline {
            bail!("{REMOVAL_PENDING}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    MANAGER.lock().unwrap().retiring.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_temporary_custom_display_and_removal() {
        let config = VirtualDisplayConfiguration {
            count: 1,
            persistent: false,
            modes: vec![VirtualDisplayMode {
                width: 2560,
                height: 1440,
                refresh_rate_units: 599_401,
            }],
        };
        assert!(validate_configuration(&config).is_ok());
        assert!(validate_configuration(&VirtualDisplayConfiguration {
            count: 0,
            modes: Vec::new(),
            ..config
        })
        .is_ok());
    }

    #[test]
    fn rejects_persistent_or_unbounded_display_operations() {
        let config = VirtualDisplayConfiguration {
            count: 1,
            persistent: false,
            modes: DEFAULT_MODES.to_vec(),
        };
        assert!(validate_configuration(&VirtualDisplayConfiguration {
            persistent: true,
            ..config.clone()
        })
        .is_err());
        assert!(validate_configuration(&VirtualDisplayConfiguration {
            count: MAX_DISPLAYS + 1,
            ..config.clone()
        })
        .is_err());
        assert!(validate_configuration(&VirtualDisplayConfiguration {
            modes: vec![VirtualDisplayMode {
                refresh_rate_units: u32::MAX,
                ..DEFAULT_MODES[0]
            }],
            ..config
        })
        .is_err());
    }
}
