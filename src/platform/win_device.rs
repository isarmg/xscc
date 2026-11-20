use hbb_common::{log, thiserror};
use std::{
    ffi::OsStr,
    io,
    ops::{Deref, DerefMut},
    os::windows::ffi::OsStrExt,
    ptr::null_mut,
    result::Result,
};
use winapi::{
    shared::{
        guiddef::GUID,
        minwindef::{BOOL, DWORD, FALSE, MAX_PATH, PBOOL, TRUE},
        ntdef::LPCWSTR,
        windef::HWND,
        winerror::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS},
    },
    um::{
        cfgmgr32::MAX_DEVICE_ID_LEN,
        handleapi::INVALID_HANDLE_VALUE,
        setupapi::*,
    },
};

mod install_parameters;

#[link(name = "Newdev")]
extern "system" {
    fn UpdateDriverForPlugAndPlayDevicesW(
        hwnd_parent: HWND,
        hardware_id: LPCWSTR,
        full_inf_path: LPCWSTR,
        install_flags: DWORD,
        b_reboot_required: PBOOL,
    ) -> BOOL;
}

#[derive(thiserror::Error, Debug)]
pub enum DeviceError {
    #[error("Failed to call {0}, {1:?}")]
    WinApiLastErr(String, io::Error),
    #[error("Failed to call {0}, returns {1}")]
    WinApiErrCode(String, DWORD),
    #[error("{0}")]
    Raw(String),
}

impl DeviceError {
    #[inline]
    fn new_api_last_err(api: &str) -> Self {
        Self::WinApiLastErr(api.to_string(), io::Error::last_os_error())
    }
}

struct DeviceInfo(HDEVINFO);

impl DeviceInfo {
    fn setup_di_create_device_info_list(class_guid: &mut GUID) -> Result<Self, DeviceError> {
        let dev_info = unsafe { SetupDiCreateDeviceInfoList(class_guid, null_mut()) };
        if dev_info == INVALID_HANDLE_VALUE {
            return Err(DeviceError::new_api_last_err("SetupDiCreateDeviceInfoList"));
        }

        Ok(Self(dev_info))
    }

    fn setup_di_get_class_devs_ex_w(
        class_guid: *const GUID,
        flags: DWORD,
    ) -> Result<Self, DeviceError> {
        let dev_info = unsafe {
            SetupDiGetClassDevsExW(
                class_guid,
                null_mut(),
                null_mut(),
                flags,
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if dev_info == INVALID_HANDLE_VALUE {
            return Err(DeviceError::new_api_last_err("SetupDiGetClassDevsExW"));
        }
        Ok(Self(dev_info))
    }
}

impl Drop for DeviceInfo {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

impl Deref for DeviceInfo {
    type Target = HDEVINFO;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DeviceInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub unsafe fn install_driver(
    inf_path: &str,
    hardware_id: &str,
    reboot_required: &mut bool,
) -> Result<(), DeviceError> {
    let driver_inf_path = OsStr::new(inf_path)
        .encode_wide()
        .chain(Some(0).into_iter())
        .collect::<Vec<u16>>();
    let install_flags = install_parameters::install_flags(hardware_id);
    let hardware_id = install_parameters::hardware_id_multi_sz(hardware_id);

    let mut class_guid: GUID = std::mem::zeroed();
    let mut class_name: [u16; 32] = [0; 32];

    if SetupDiGetINFClassW(
        driver_inf_path.as_ptr(),
        &mut class_guid,
        class_name.as_mut_ptr(),
        class_name.len() as _,
        null_mut(),
    ) == FALSE
    {
        return Err(DeviceError::new_api_last_err("SetupDiGetINFClassW"));
    }

    let dev_info = DeviceInfo::setup_di_create_device_info_list(&mut class_guid)?;

    let mut dev_info_data = SP_DEVINFO_DATA {
        cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as _,
        ClassGuid: class_guid,
        DevInst: 0,
        Reserved: 0,
    };
    if SetupDiCreateDeviceInfoW(
        *dev_info,
        class_name.as_ptr(),
        &class_guid,
        null_mut(),
        null_mut(),
        DICD_GENERATE_ID,
        &mut dev_info_data,
    ) == FALSE
    {
        return Err(DeviceError::new_api_last_err("SetupDiCreateDeviceInfoW"));
    }

    if SetupDiSetDeviceRegistryPropertyW(
        *dev_info,
        &mut dev_info_data,
        SPDRP_HARDWAREID,
        hardware_id.as_ptr() as _,
        (hardware_id.len() * 2) as _,
    ) == FALSE
    {
        return Err(DeviceError::new_api_last_err(
            "SetupDiSetDeviceRegistryPropertyW",
        ));
    }

    if SetupDiCallClassInstaller(DIF_REGISTERDEVICE, *dev_info, &mut dev_info_data) == FALSE {
        return Err(DeviceError::new_api_last_err("SetupDiCallClassInstaller"));
    }

    let mut reboot_required_ = FALSE;
    if UpdateDriverForPlugAndPlayDevicesW(
        null_mut(),
        hardware_id.as_ptr(),
        driver_inf_path.as_ptr(),
        install_flags,
        &mut reboot_required_,
    ) == FALSE
    {
        let cause = DeviceError::new_api_last_err("UpdateDriverForPlugAndPlayDevicesW");
        // Only remove the root devnode registered by this attempt, not a shared driver package.
        if SetupDiCallClassInstaller(DIF_REMOVE, *dev_info, &mut dev_info_data) == FALSE {
            let cleanup = DeviceError::new_api_last_err("SetupDiCallClassInstaller(DIF_REMOVE)");
            return Err(DeviceError::Raw(format!(
                "{cause}; removing the newly registered device failed: {cleanup}"
            )));
        }
        let mut parameters: SP_DEVINSTALL_PARAMS_W = std::mem::zeroed();
        parameters.cbSize = std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as _;
        if SetupDiGetDeviceInstallParamsW(*dev_info, &mut dev_info_data, &mut parameters) == FALSE {
            let cleanup = DeviceError::new_api_last_err("SetupDiGetDeviceInstallParamsW");
            return Err(DeviceError::Raw(format!(
                "{cause}; checking newly registered device removal failed: {cleanup}"
            )));
        }
        if parameters.Flags & (DI_NEEDRESTART | DI_NEEDREBOOT) != 0 {
            *reboot_required = true;
            return Err(DeviceError::Raw(format!(
                "{cause}; Windows requires a restart to remove the newly registered device"
            )));
        }
        return Err(cause);
    }
    let mut parameters: SP_DEVINSTALL_PARAMS_W = std::mem::zeroed();
    parameters.cbSize = std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as _;
    if SetupDiGetDeviceInstallParamsW(*dev_info, &mut dev_info_data, &mut parameters) == FALSE {
        return Err(DeviceError::new_api_last_err(
            "SetupDiGetDeviceInstallParamsW",
        ));
    }
    *reboot_required =
        reboot_required_ == TRUE || parameters.Flags & (DI_NEEDRESTART | DI_NEEDREBOOT) != 0;

    Ok(())
}

unsafe fn is_same_hardware_id(
    dev_info: &DeviceInfo,
    devinfo_data: &mut SP_DEVINFO_DATA,
    hardware_id: &str,
) -> Result<bool, DeviceError> {
    let mut cur_hardware_id = [0u16; MAX_DEVICE_ID_LEN];
    if SetupDiGetDeviceRegistryPropertyW(
        **dev_info,
        devinfo_data,
        SPDRP_HARDWAREID,
        null_mut(),
        cur_hardware_id.as_mut_ptr() as _,
        cur_hardware_id.len() as _,
        null_mut(),
    ) == FALSE
    {
        return Err(DeviceError::new_api_last_err(
            "SetupDiGetDeviceRegistryPropertyW",
        ));
    }

    let cur_hardware_id = String::from_utf16_lossy(&cur_hardware_id)
        .trim_end_matches(char::from(0))
        .to_string();
    Ok(cur_hardware_id == hardware_id)
}

pub unsafe fn uninstall_driver(
    hardware_id: &str,
    reboot_required: &mut bool,
) -> Result<(), DeviceError> {
    let dev_info =
        DeviceInfo::setup_di_get_class_devs_ex_w(null_mut(), DIGCF_ALLCLASSES | DIGCF_PRESENT)?;

    let mut device_info_list_detail = SP_DEVINFO_LIST_DETAIL_DATA_W {
        cbSize: std::mem::size_of::<SP_DEVINFO_LIST_DETAIL_DATA_W>() as _,
        ClassGuid: std::mem::zeroed(),
        RemoteMachineHandle: null_mut(),
        RemoteMachineName: [0; SP_MAX_MACHINENAME_LENGTH],
    };
    if SetupDiGetDeviceInfoListDetailW(*dev_info, &mut device_info_list_detail) == FALSE {
        return Err(DeviceError::new_api_last_err(
            "SetupDiGetDeviceInfoListDetailW",
        ));
    }

    let mut devinfo_data = SP_DEVINFO_DATA {
        cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as _,
        ClassGuid: std::mem::zeroed(),
        DevInst: 0,
        Reserved: 0,
    };

    let mut device_index = 0;
    loop {
        if SetupDiEnumDeviceInfo(*dev_info, device_index, &mut devinfo_data) == FALSE {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as _) {
                break;
            }
            return Err(DeviceError::WinApiLastErr(
                "SetupDiEnumDeviceInfo".to_string(),
                err,
            ));
        }

        match is_same_hardware_id(&dev_info, &mut devinfo_data, hardware_id) {
            Ok(false) => {
                device_index += 1;
                continue;
            }
            Err(e) => {
                log::error!("Failed to call is_same_hardware_id, {:?}", e);
                device_index += 1;
                continue;
            }
            _ => {}
        }

        let mut remove_device_params = SP_REMOVEDEVICE_PARAMS {
            ClassInstallHeader: SP_CLASSINSTALL_HEADER {
                cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as _,
                InstallFunction: DIF_REMOVE,
            },
            Scope: DI_REMOVEDEVICE_GLOBAL,
            HwProfile: 0,
        };

        if SetupDiSetClassInstallParamsW(
            *dev_info,
            &mut devinfo_data,
            &mut remove_device_params.ClassInstallHeader,
            std::mem::size_of::<SP_REMOVEDEVICE_PARAMS>() as _,
        ) == FALSE
        {
            return Err(DeviceError::new_api_last_err(
                "SetupDiSetClassInstallParams",
            ));
        }

        if SetupDiCallClassInstaller(DIF_REMOVE, *dev_info, &mut devinfo_data) == FALSE {
            return Err(DeviceError::new_api_last_err("SetupDiCallClassInstaller"));
        }

        let mut device_params = SP_DEVINSTALL_PARAMS_W {
            cbSize: std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as _,
            Flags: 0,
            FlagsEx: 0,
            hwndParent: null_mut(),
            InstallMsgHandler: None,
            InstallMsgHandlerContext: null_mut(),
            FileQueue: null_mut(),
            ClassInstallReserved: 0,
            Reserved: 0,
            DriverPath: [0; MAX_PATH],
        };

        if SetupDiGetDeviceInstallParamsW(*dev_info, &mut devinfo_data, &mut device_params) == FALSE
        {
            log::error!(
                "Failed to call SetupDiGetDeviceInstallParamsW, {:?}",
                io::Error::last_os_error()
            );
        } else {
            if device_params.Flags & (DI_NEEDRESTART | DI_NEEDREBOOT) != 0 {
                *reboot_required = true;
            }
        }

        device_index += 1;
    }

    Ok(())
}
