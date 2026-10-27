use super::str_to_device_name;
use base::message_proto::{Resolution, SupportedResolutions};
use hbb_common::{bail, ResultType};
use winapi::{
    shared::ntdef::NULL,
    um::{
        wingdi::{
            DEVMODEW, DM_DISPLAYFREQUENCY, DM_DISPLAYORIENTATION, DM_PELSHEIGHT, DM_PELSWIDTH,
        },
        winuser::*,
    },
};

fn modes(name: &str) -> Vec<DEVMODEW> {
    let device = str_to_device_name(name);
    let mut values = Vec::new();
    let mut index = 0;
    loop {
        let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as _;
        if unsafe {
            EnumDisplaySettingsExW(
                device.as_ptr(),
                index,
                &mut mode,
                EDS_RAWMODE | EDS_ROTATEDMODE,
            )
        } == 0
        {
            break;
        }
        if mode.dmBitsPerPel == 32 && mode.dmPelsWidth > 0 && mode.dmPelsHeight > 0 {
            values.push(mode);
        }
        index += 1;
    }
    values
}

fn resolution(mode: &DEVMODEW) -> Resolution {
    Resolution {
        width: mode.dmPelsWidth as _,
        height: mode.dmPelsHeight as _,
        refresh_rate: if mode.dmDisplayFrequency > 1 {
            mode.dmDisplayFrequency
        } else {
            0
        },
        ..Default::default()
    }
}

pub(crate) fn current(name: &str) -> ResultType<Resolution> {
    let device = str_to_device_name(name);
    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
    mode.dmSize = std::mem::size_of::<DEVMODEW>() as _;
    if unsafe { EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) } == 0 {
        bail!(
            "Read current display mode failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(resolution(&mode))
}

pub(crate) fn supported(name: &str) -> SupportedResolutions {
    let mut resolutions: Vec<_> = modes(name).iter().map(resolution).collect();
    resolutions.sort_by_key(|mode| (mode.width, mode.height, mode.refresh_rate));
    resolutions.dedup();
    let current_resolution = match current(name) {
        Ok(mode) => Some(mode),
        Err(error) => {
            hbb_common::throttled_log!(std::time::Duration::from_secs(30), warn, "{error}");
            None
        }
    };
    SupportedResolutions {
        resolutions,
        current_resolution: current_resolution.into(),
        ..Default::default()
    }
}

pub(crate) fn change(name: &str, requested: &Resolution) -> ResultType<()> {
    let mut mode = modes(name)
        .into_iter()
        .find(|mode| {
            let value = resolution(mode);
            value.width == requested.width
                && value.height == requested.height
                && value.refresh_rate == requested.refresh_rate
        })
        .ok_or_else(|| {
            hbb_common::anyhow::anyhow!("The display driver does not offer this mode")
        })?;
    let device = str_to_device_name(name);
    mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_DISPLAYORIENTATION;
    let test =
        unsafe { ChangeDisplaySettingsExW(device.as_ptr(), &mut mode, NULL as _, CDS_TEST, NULL) };
    if test != DISP_CHANGE_SUCCESSFUL {
        bail!("Windows rejected the requested display mode (result {test})");
    }
    let result = unsafe {
        ChangeDisplaySettingsExW(
            device.as_ptr(),
            &mut mode,
            NULL as _,
            CDS_UPDATEREGISTRY | CDS_GLOBAL | CDS_RESET,
            NULL,
        )
    };
    if result != DISP_CHANGE_SUCCESSFUL {
        bail!("Apply display mode failed (result {result})");
    }
    let actual = current(name)?;
    if actual.width != requested.width
        || actual.height != requested.height
        || actual.refresh_rate != requested.refresh_rate
    {
        bail!(
            "Windows applied {}×{} at {} Hz instead of the requested mode",
            actual.width,
            actual.height,
            actual.refresh_rate
        );
    }
    Ok(())
}
