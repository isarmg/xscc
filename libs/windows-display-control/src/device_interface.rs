use std::{
    fmt, io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW, CM_MapCrToWin32Err,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_BUFFER_SMALL, CR_SUCCESS,
        },
        Foundation::{
            ERROR_GEN_FAILURE, ERROR_NO_MORE_ITEMS, ERROR_RETRY, GENERIC_READ, GENERIC_WRITE,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{CreateFileW, OPEN_EXISTING},
        System::IO::DeviceIoControl,
    },
};

const AMYUNI_INTERFACE: GUID = GUID::from_u128(0xb5ffd75f_da40_4353_8ff8_b6daf6f1d8ca);
const AMYUNI_MONITOR_CONTROL: u32 = 2307084;

#[derive(Debug)]
pub struct DisplayControlError {
    operation: &'static str,
    source: io::Error,
}

impl DisplayControlError {
    pub(crate) fn new(operation: &'static str, source: io::Error) -> Self {
        Self { operation, source }
    }

    pub(crate) fn from_last_os_error(operation: &'static str) -> Self {
        Self::new(operation, io::Error::last_os_error())
    }

    pub(crate) fn from_code(operation: &'static str, code: i32) -> Self {
        Self::new(operation, io::Error::from_raw_os_error(code))
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for DisplayControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed ({})", self.operation, self.source)
    }
}

impl std::error::Error for DisplayControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn config_error(operation: &'static str, status: u32) -> DisplayControlError {
    let code = unsafe { CM_MapCrToWin32Err(status, ERROR_GEN_FAILURE) };
    DisplayControlError::from_code(operation, code as i32)
}

fn device_path(interface: &GUID) -> Result<Vec<u16>, DisplayControlError> {
    for _ in 0..4 {
        let mut length = 0;
        let status = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut length,
                interface,
                null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if status != CR_SUCCESS {
            return Err(config_error("CM_Get_Device_Interface_List_SizeW", status));
        }
        let mut paths = vec![0; length as usize];
        let status = unsafe {
            CM_Get_Device_Interface_ListW(
                interface,
                null(),
                paths.as_mut_ptr(),
                length,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        match status {
            CR_BUFFER_SMALL => continue,
            CR_SUCCESS => {
                if paths.first().is_some_and(|&first| first != 0) && paths.contains(&0) {
                    return Ok(paths);
                }
                return Err(DisplayControlError::from_code(
                    "Find virtual display device interface",
                    ERROR_NO_MORE_ITEMS as i32,
                ));
            }
            status => return Err(config_error("CM_Get_Device_Interface_ListW", status)),
        }
    }
    Err(DisplayControlError::from_code(
        "Read changing virtual display device interface list",
        ERROR_RETRY as i32,
    ))
}

pub(crate) struct DeviceInterface(OwnedHandle);

impl DeviceInterface {
    pub(crate) fn open(interface: &GUID) -> Result<Self, DisplayControlError> {
        let path = device_path(interface)?;
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(DisplayControlError::from_last_os_error("CreateFileW"));
        }
        // CreateFileW transfers the valid handle to the caller.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    pub(crate) fn send(&self, control: u32, input: &[u8]) -> Result<(), DisplayControlError> {
        let input_size = u32::try_from(input.len()).map_err(|error| {
            DisplayControlError::new(
                "Encode virtual display control request",
                io::Error::new(io::ErrorKind::InvalidInput, error),
            )
        })?;
        let mut bytes_returned = 0;
        let result = unsafe {
            DeviceIoControl(
                self.0.as_raw_handle(),
                control,
                input.as_ptr().cast(),
                input_size,
                null_mut(),
                0,
                &mut bytes_returned,
                null_mut(),
            )
        };
        if result == 0 {
            return Err(DisplayControlError::from_last_os_error("DeviceIoControl"));
        }
        Ok(())
    }
}

pub fn set_amyuni_monitor(add: bool) -> Result<(), DisplayControlError> {
    let command = if add { 0x10u32 } else { 0 };
    DeviceInterface::open(&AMYUNI_INTERFACE)?.send(AMYUNI_MONITOR_CONTROL, &command.to_le_bytes())
}
