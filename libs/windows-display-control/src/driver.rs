use crate::{
    device_interface::{DeviceInterface, DisplayControlError},
    protocol, MonitorMode,
};
use std::{
    ffi::c_void,
    io, mem,
    os::windows::ffi::OsStrExt,
    ptr::{null, null_mut, NonNull},
    sync::{mpsc, Mutex},
    thread,
    time::Duration,
};
use windows_sys::{
    core::{w, GUID, HRESULT, PCWSTR},
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{UpdateDriverForPlugAndPlayDevicesW, INSTALLFLAG_FORCE},
            Enumeration::Pnp::{
                SWDeviceCapabilitiesDriverRequired, SWDeviceCapabilitiesRemovable,
                SWDeviceCapabilitiesSilentInstall, SWDeviceLifetimeHandle, SwDeviceClose,
                SwDeviceCreate, SwDeviceSetLifetime, HSWDEVICE, SW_DEVICE_CREATE_INFO,
            },
        },
        System::Com::CoCreateGuid,
    },
};

const DRIVER_INF: &str = "RustDeskIddDriver/RustDeskIddDriver.inf";
const DRIVER_ID: PCWSTR = w!("RustDeskIddDriver");
const INTERFACE: GUID = GUID::from_u128(0x781ef630_72b2_11d2_b852_00c04eaf5272);
const CREATE_TIMEOUT: Duration = Duration::from_secs(10);
const MONITOR_RETRIES: u32 = 20;
static SOFTWARE_DEVICE: Mutex<Option<SoftwareDevice>> = Mutex::new(None);

type CreationResult = mpsc::SyncSender<HRESULT>;

struct SoftwareDevice {
    handle: NonNull<c_void>,
    _creation_result: Box<CreationResult>,
}

// Software device handles support calls across threads. The callback owns no handle.
unsafe impl Send for SoftwareDevice {}

impl Drop for SoftwareDevice {
    fn drop(&mut self) {
        // Windows finishes any callback before returning, while the context is still alive.
        unsafe { SwDeviceClose(self.handle.as_ptr()) };
    }
}

unsafe extern "system" fn device_created(
    _handle: HSWDEVICE,
    result: HRESULT,
    context: *const c_void,
    _instance: PCWSTR,
) {
    let result_sender = unsafe { &*context.cast::<CreationResult>() };
    let _ = result_sender.try_send(result);
}

fn check_hresult(operation: &'static str, result: HRESULT) -> Result<(), DisplayControlError> {
    if result < 0 {
        return Err(DisplayControlError::from_code(operation, result));
    }
    Ok(())
}

fn create_device() -> Result<SoftwareDevice, DisplayControlError> {
    let create_info = SW_DEVICE_CREATE_INFO {
        cbSize: mem::size_of::<SW_DEVICE_CREATE_INFO>() as u32,
        pszInstanceId: DRIVER_ID,
        pszzHardwareIds: w!("RustDeskIddDriver\0"),
        pszzCompatibleIds: w!("RustDeskIddDriver\0"),
        CapabilityFlags: (SWDeviceCapabilitiesRemovable
            | SWDeviceCapabilitiesSilentInstall
            | SWDeviceCapabilitiesDriverRequired) as u32,
        pszDeviceDescription: w!("RustDesk Idd Driver"),
        ..Default::default()
    };
    let (send_result, receive_result) = mpsc::sync_channel(1);
    let creation_result = Box::new(send_result);
    let mut handle = null_mut();
    let result = unsafe {
        SwDeviceCreate(
            DRIVER_ID,
            w!("HTREE\\ROOT\\0"),
            &create_info,
            0,
            null(),
            Some(device_created),
            (&*creation_result as *const CreationResult).cast(),
            &mut handle,
        )
    };
    check_hresult("SwDeviceCreate", result)?;
    let device = SoftwareDevice {
        handle: NonNull::new(handle).expect("successful SwDeviceCreate returns a device handle"),
        _creation_result: creation_result,
    };
    let creation_result = receive_result
        .recv_timeout(CREATE_TIMEOUT)
        .map_err(|error| {
            let kind = match error {
                mpsc::RecvTimeoutError::Timeout => io::ErrorKind::TimedOut,
                mpsc::RecvTimeoutError::Disconnected => io::ErrorKind::BrokenPipe,
            };
            DisplayControlError::new(
                "Wait for software device enumeration",
                io::Error::new(kind, error),
            )
        })?;
    check_hresult("Software device enumeration", creation_result)?;
    let result = unsafe { SwDeviceSetLifetime(device.handle.as_ptr(), SWDeviceLifetimeHandle) };
    check_hresult("SwDeviceSetLifetime", result)?;
    Ok(device)
}

pub fn install_update_driver() -> Result<bool, DisplayControlError> {
    let executable = std::env::current_exe()
        .map_err(|error| DisplayControlError::new("Locate application executable", error))?;
    let inf = executable
        .parent()
        .ok_or_else(|| {
            DisplayControlError::new(
                "Locate virtual display driver INF",
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "application directory is unavailable",
                ),
            )
        })?
        .join(DRIVER_INF);
    inf.metadata()
        .map_err(|error| DisplayControlError::new("Read virtual display driver INF", error))?;
    let inf: Vec<u16> = inf.as_os_str().encode_wide().chain(Some(0)).collect();

    let mut installed = SOFTWARE_DEVICE.lock().unwrap();
    // Plug and play updates require a matching software device to be present.
    let new_device = if installed.is_none() {
        Some(create_device()?)
    } else {
        None
    };
    let mut reboot_required = 0;
    let result = unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            null_mut(),
            DRIVER_ID,
            inf.as_ptr(),
            INSTALLFLAG_FORCE,
            &mut reboot_required,
        )
    };
    if result == 0 {
        return Err(DisplayControlError::from_last_os_error(
            "UpdateDriverForPlugAndPlayDevicesW",
        ));
    }
    if new_device.is_some() {
        *installed = new_device;
    }
    Ok(reboot_required != 0)
}

pub fn plug_in_monitor(index: u32) -> Result<(), DisplayControlError> {
    let mut attempt = 0;
    let device = loop {
        match DeviceInterface::open(&INTERFACE) {
            Ok(device) => break device,
            Err(error) if attempt == MONITOR_RETRIES => return Err(error),
            Err(_) => {
                attempt += 1;
                thread::sleep(Duration::from_secs(1));
            }
        }
    };
    let mut guid = GUID::from_u128(0);
    check_hresult("CoCreateGuid", unsafe { CoCreateGuid(&mut guid) })?;
    let mut container_id = [0; 16];
    container_id[..4].copy_from_slice(&guid.data1.to_le_bytes());
    container_id[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    container_id[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    container_id[8..].copy_from_slice(&guid.data4);
    let message = protocol::plug_in(index, container_id);
    loop {
        let result = device.send(protocol::PLUG_IN, &message);
        if result.is_ok() || attempt == MONITOR_RETRIES {
            return result;
        }
        attempt += 1;
    }
}

pub fn plug_out_monitor(index: u32) -> Result<(), DisplayControlError> {
    DeviceInterface::open(&INTERFACE)?.send(protocol::PLUG_OUT, &protocol::plug_out(index))
}

pub fn update_monitor_modes(index: u32, modes: &[MonitorMode]) -> Result<(), DisplayControlError> {
    let message = protocol::update_modes(index, modes)
        .map_err(|error| DisplayControlError::new("Encode virtual display monitor modes", error))?;
    DeviceInterface::open(&INTERFACE)?.send(protocol::UPDATE_MODES, &message)
}
