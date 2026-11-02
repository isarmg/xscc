//! Physical monitor discovery shared by desktop capture and absolute input.

use std::{io, mem::size_of, ptr};

use windows_sys::{
    core::BOOL,
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{
            EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW,
            ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFOEXW,
        },
        System::{RemoteDesktop::ProcessIdToSessionId, Threading::GetCurrentProcessId},
        UI::HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
    },
};

use crate::error::{AppError, Result};

pub const DESKTOP_DEVICE_PREFIX: &str = "desktop:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopMonitor {
    pub id: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: Option<u32>,
}

pub(crate) struct PhysicalCoordinates(DPI_AWARENESS_CONTEXT);

impl PhysicalCoordinates {
    pub(crate) fn enter() -> io::Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(previous))
    }
}

impl Drop for PhysicalCoordinates {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

pub fn monitors() -> Result<Vec<DesktopMonitor>> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    // Services can still enumerate external DirectShow devices, but Session 0
    // does not own the logged-in user's desktop.
    if session == 0 {
        return Ok(Vec::new());
    }

    struct Discovery {
        monitors: Vec<DesktopMonitor>,
        error: Option<io::Error>,
    }

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        context: LPARAM,
    ) -> BOOL {
        let discovery = unsafe { &mut *(context as *mut Discovery) };
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } == 0 {
            discovery.error = Some(io::Error::last_os_error());
            return 0;
        }
        let length = info
            .szDevice
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(info.szDevice.len());
        let id = match String::from_utf16(&info.szDevice[..length]) {
            Ok(id) => id,
            Err(error) => {
                discovery.error = Some(io::Error::new(io::ErrorKind::InvalidData, error));
                return 0;
            }
        };
        let bounds = info.monitorInfo.rcMonitor;
        let (Some(width), Some(height)) = (
            bounds.right.checked_sub(bounds.left),
            bounds.bottom.checked_sub(bounds.top),
        ) else {
            discovery.error = Some(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned overflowing monitor bounds",
            ));
            return 0;
        };
        if width <= 0 || height <= 0 {
            discovery.error = Some(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned empty monitor bounds",
            ));
            return 0;
        }
        let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
        mode.dmSize = size_of::<DEVMODEW>() as u16;
        let refresh_hz = if unsafe {
            EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode)
        } != 0
            && mode.dmDisplayFrequency > 1
        {
            Some(mode.dmDisplayFrequency)
        } else {
            None
        };
        discovery.monitors.push(DesktopMonitor {
            id,
            left: bounds.left,
            top: bounds.top,
            width: width as u32,
            height: height as u32,
            refresh_hz,
        });
        1
    }

    let _coordinates = PhysicalCoordinates::enter()?;
    let mut discovery = Discovery {
        monitors: Vec::new(),
        error: None,
    };
    let success = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect),
            &mut discovery as *mut Discovery as LPARAM,
        )
    };
    if let Some(error) = discovery.error {
        return Err(error.into());
    }
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    discovery
        .monitors
        .sort_by(|left, right| left.id.cmp(&right.id));
    Ok(discovery.monitors)
}

pub fn monitor_for_path(path: &str) -> Result<DesktopMonitor> {
    let id = path
        .strip_prefix(DESKTOP_DEVICE_PREFIX)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AppError::BadRequest("Expected a desktop monitor device path".into()))?;
    monitors()?
        .into_iter()
        .find(|monitor| monitor.id == id)
        .ok_or_else(|| AppError::NotFound(format!("Windows desktop monitor {id}")))
}
