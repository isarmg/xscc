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
    let mut value = resolution(&mode);
    if let Some(screen) = crate::virtual_display_manager::virtual_display_mode(name)? {
        value.width = i32::try_from(screen.width)?;
        value.height = i32::try_from(screen.height)?;
        value.refresh_rate = u32::try_from((u64::from(screen.refresh_rate_units) + 5000) / 10000)?;
    }
    Ok(value)
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
    change_guarded(name, requested, false)
}

pub(crate) fn change_for_session(name: &str, requested: &Resolution) -> ResultType<()> {
    change_guarded(name, requested, true)
}

fn matches_mode(current: &Resolution, requested: &Resolution) -> bool {
    (current.width, current.height) == (requested.width, requested.height)
        && (requested.refresh_rate == 0 || current.refresh_rate == requested.refresh_rate)
}

fn change_guarded(name: &str, requested: &Resolution, remember: bool) -> ResultType<()> {
    let presentation = crate::virtual_display_manager::before_display_mode_change()?;
    let original = current(name)?;
    if !matches_mode(&original, requested) {
        if remember && !crate::virtual_display_manager::amyuni_idd::is_my_display(name) {
            // Record under the mode-change guard, including partial Windows failures.
            crate::server::display_service::set_last_changed_resolution(
                name,
                original,
                (requested.width, requested.height),
            );
        }
        if requested.refresh_rate == 0 {
            super::apply_display_size(
                name,
                usize::try_from(requested.width)?,
                usize::try_from(requested.height)?,
            )?;
        } else {
            let applied = match &presentation {
                Some(guard) => guard.change_virtual_mode(name, requested)?,
                None => false,
            };
            if !applied {
                apply_mode(name, requested)?;
            }
        }
    }
    if let Some(presentation) = presentation {
        presentation.finish()?;
    }
    Ok(())
}

pub(crate) fn restore(
    name: &str,
    original: &Resolution,
    _idle: &crate::server::video_service::CaptureIdleGuard<'_>,
) -> ResultType<()> {
    if !matches_mode(&current(name)?, original) {
        if original.refresh_rate != 0 {
            apply_mode(name, original)?;
        } else {
            super::apply_display_size(name, original.width as usize, original.height as usize)?;
        }
    }
    Ok(())
}

fn apply_mode(name: &str, requested: &Resolution) -> ResultType<()> {
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
    if !matches_mode(&actual, requested) {
        bail!(
            "Windows applied {}×{} at {} Hz instead of the requested mode",
            actual.width,
            actual.height,
            actual.refresh_rate
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_dimensions_at_a_different_refresh_rate_need_restoration() {
        let saved = Resolution {
            width: 1920,
            height: 1080,
            refresh_rate: 120,
            ..Default::default()
        };
        let live = Resolution {
            refresh_rate: 60,
            ..saved.clone()
        };
        assert!(!matches_mode(&live, &saved));
        assert!(matches_mode(&saved, &saved));
    }
}
