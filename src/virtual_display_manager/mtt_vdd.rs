#[path = "mtt_vdd/profile.rs"]
mod profile;

use super::windows;
use crate::platform::{self, win_device};
use hbb_common::{anyhow, bail, ResultType};
use profile::{Mode, Profile};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winapi::{
    shared::{
        cfg::CM_PROB_DISABLED,
        guiddef::GUID,
        minwindef::FALSE,
        winerror::{ERROR_INVALID_DATA, ERROR_NO_MORE_ITEMS},
    },
    um::{
        cfgmgr32::{CM_Get_DevNode_Status, CR_SUCCESS},
        handleapi::INVALID_HANDLE_VALUE,
        libloaderapi::{FreeLibrary, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
        setupapi::*,
        winbase::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
        wingdi::{DISPLAY_DEVICEW, DISPLAY_DEVICE_MIRRORING_DRIVER},
        wintrust::*,
        winuser::EnumDisplayDevicesW,
    },
};
use winreg::{
    enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE},
    RegKey,
};

pub(super) const DEVICE_STRING: &str = "Virtual Display Driver\0";
const HARDWARE_ID: &str = r"Root\MttVDD";
const REGISTRY: &str = r"SOFTWARE\MikeTheTech\VirtualDisplayDriver";
const OWNER: &[u8] = b"XSCC owns this VDD configuration v1\n";
const MANIFEST: &str = include_str!("../../packaging/gpui-windows/virtual-display-vdd-driver.json");
static LOCK: Mutex<()> = Mutex::new(());

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn check_path(path: &Path, directory: bool) -> ResultType<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_attributes() & 0x400 != 0 || metadata.is_dir() != directory {
        bail!(
            "VDD path is not a physical {}: {}",
            if directory { "directory" } else { "file" },
            path.display()
        );
    }
    Ok(())
}

fn bounded(path: &Path, maximum: u64) -> ResultType<Vec<u8>> {
    check_path(path, false)?;
    if fs::metadata(path)?.len() > maximum {
        bail!("VDD file exceeds its size bound: {}", path.display());
    }
    Ok(fs::read(path)?)
}

fn bundle_path() -> ResultType<PathBuf> {
    #[cfg(feature = "gpui-host")]
    let executable = crate::gpui_host::windows_host::installed_executable()?;
    #[cfg(not(feature = "gpui-host"))]
    let executable = std::env::current_exe()?;
    let path = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("VDD host directory is unavailable"))?
        .join("mtt_vdd");
    check_path(&path, true)?;
    Ok(path)
}

fn verify_signature(path: &Path) -> ResultType<()> {
    let path_wide = wide(path.as_os_str());
    let mut file: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as _;
    file.pcwszFilePath = path_wide.as_ptr();
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as _;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    unsafe {
        *data.u.pFile_mut() = &mut file;
    }
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
    let mut action = GUID {
        Data1: 0x00aac56b,
        Data2: 0xcd44,
        Data3: 0x11d0,
        Data4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee],
    };
    let result = unsafe {
        WinVerifyTrust(
            INVALID_HANDLE_VALUE as _,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            INVALID_HANDLE_VALUE as _,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
    }
    if result != 0 {
        bail!(
            "VDD signature verification failed for {} (WinTrust {result:#x})",
            path.display()
        );
    }
    Ok(())
}

fn verified_bundle() -> ResultType<PathBuf> {
    let path = bundle_path()?;
    let expected: serde_json::Value = serde_json::from_str(MANIFEST)?;
    let actual: serde_json::Value =
        serde_json::from_slice(&bounded(&path.join("driver-source.json"), 65536)?)?;
    if actual != expected {
        bail!("VDD provenance differs from the compiled source manifest");
    }
    let files = expected
        .get("files")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow::anyhow!("Invalid compiled VDD file inventory"))?;
    for (name, expected) in files {
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            bail!("Invalid compiled VDD filename");
        }
        let bytes = bounded(&path.join(name), 16 * 1024 * 1024)?;
        if expected.as_str() != Some(format!("{:x}", Sha256::digest(&bytes)).as_str()) {
            bail!("VDD payload checksum differs: {name}");
        }
    }
    verify_signature(&path.join("mttvdd.cat"))?;
    verify_signature(&path.join("MttVDD.dll"))?;
    for library in [
        "ntdll.dll",
        "kernel32.dll",
        "advapi32.dll",
        "ole32.dll",
        "dxgi.dll",
        "d3d11.dll",
        "xmllite.dll",
        "shlwapi.dll",
        "avrt.dll",
        "ucrtbase.dll",
    ] {
        let name = wide(OsStr::new(library));
        let module =
            unsafe { LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() {
            bail!(
                "VDD required Windows runtime {library} is unavailable: {}",
                std::io::Error::last_os_error()
            );
        }
        unsafe {
            FreeLibrary(module);
        }
    }
    Ok(path)
}

struct Devices(HDEVINFO);
impl Drop for Devices {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn device() -> ResultType<Option<(Devices, SP_DEVINFO_DATA)>> {
    let handle = unsafe {
        SetupDiGetClassDevsW(null(), null(), null_mut(), DIGCF_ALLCLASSES | DIGCF_PRESENT)
    };
    if handle == INVALID_HANDLE_VALUE {
        bail!(
            "Enumerate VDD devices failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let devices = Devices(handle);
    let mut found = None;
    let mut index = 0;
    loop {
        let mut data: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as _;
        if unsafe { SetupDiEnumDeviceInfo(devices.0, index, &mut data) } == FALSE {
            if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NO_MORE_ITEMS as _) {
                break;
            }
            bail!(
                "Read VDD device failed: {}",
                std::io::Error::last_os_error()
            );
        }
        index += 1;
        let mut ids = [0u16; 2048];
        let mut bytes = 0;
        if unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                devices.0,
                &mut data,
                SPDRP_HARDWAREID,
                null_mut(),
                ids.as_mut_ptr().cast(),
                std::mem::size_of_val(&ids) as _,
                &mut bytes,
            )
        } == FALSE
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INVALID_DATA as _) {
                continue;
            }
            bail!("Read device hardware identifiers failed: {error}");
        }
        if bytes as usize > std::mem::size_of_val(&ids) || bytes % 2 != 0 {
            bail!("Invalid VDD hardware identifier length");
        }
        let matches = ids[..bytes as usize / 2]
            .split(|value| *value == 0)
            .filter(|value| !value.is_empty())
            .any(|value| {
                String::from_utf16_lossy(value).eq_ignore_ascii_case(HARDWARE_ID)
                    || String::from_utf16_lossy(value).eq_ignore_ascii_case("MttVDD")
            });
        if matches {
            if found.is_some() {
                bail!("Multiple VDD adapters exist; refusing to modify an external device");
            }
            found = Some(data);
        }
    }
    Ok(found.map(|data| (devices, data)))
}

fn registered_path() -> ResultType<Option<String>> {
    let key = match RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(REGISTRY, KEY_READ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match key.get_value("VDDPATH") {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn ordinary_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .trim_end_matches(['\\', '/'])
        .to_owned()
}

fn owned_folder(bundle: &Path, present: bool) -> ResultType<PathBuf> {
    let path = bundle.join("config");
    let registered = registered_path()?;
    if registered.as_ref().is_some_and(|value| {
        !value
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(&ordinary_path(&path))
    }) {
        bail!("The existing VDD configuration is managed outside XSCC; refusing to replace it");
    }
    let marker = path.join("owner.txt");
    if path.exists() {
        check_path(&path, true)?;
        if bounded(&marker, 128)? != OWNER {
            bail!("VDD configuration ownership differs");
        }
    } else {
        if present || registered.is_some() {
            bail!("Existing VDD device has no XSCC ownership evidence");
        }
        fs::create_dir(&path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        file.write_all(OWNER)?;
        file.sync_all()?;
    }
    if present && registered.is_none() {
        bail!("Existing VDD device configuration ownership is unknown");
    }
    Ok(path)
}

fn write_profile(path: &Path, xml: &str) -> ResultType<()> {
    if path.exists() {
        check_path(path, false)?;
    }
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_file_name(format!(".vdd-settings-{}-{time}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(xml.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let from = wide(temporary.as_os_str());
    let to = wide(path.as_os_str());
    let result = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == FALSE {
        let cause = std::io::Error::last_os_error();
        fs::remove_file(temporary)?;
        bail!("Publish VDD configuration failed: {cause}");
    }
    Ok(())
}

fn collect_modes() -> ResultType<BTreeSet<Mode>> {
    let mut values = BTreeSet::new();
    let mut index = 0;
    loop {
        let mut display: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        display.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as _;
        if unsafe { EnumDisplayDevicesW(null(), index, &mut display, 0) } == 0 {
            break;
        }
        index += 1;
        let description = String::from_utf16_lossy(&display.DeviceString);
        if display.StateFlags & DISPLAY_DEVICE_MIRRORING_DRIVER != 0
            || description.starts_with(DEVICE_STRING)
            || description.starts_with(super::AMYUNI_IDD_DEVICE_STRING)
        {
            continue;
        }
        let name = String::from_utf16(&display.DeviceName)?;
        for mode in platform::windows::display_modes::supported(&name).resolutions {
            if mode.width > 0 && mode.height > 0 && mode.refresh_rate > 1 {
                let refresh = mode
                    .refresh_rate
                    .checked_mul(10000)
                    .ok_or_else(|| anyhow::anyhow!("Reported VDD mode refresh rate overflow"))?;
                values.insert(
                    Mode::new(mode.width as _, mode.height as _, refresh)
                        .map_err(anyhow::Error::msg)?,
                );
            }
        }
    }
    Ok(values)
}

fn change_device(state: u32) -> ResultType<()> {
    let (devices, mut data) =
        device()?.ok_or_else(|| anyhow::anyhow!("VDD device is not installed"))?;
    let mut change: SP_PROPCHANGE_PARAMS = unsafe { std::mem::zeroed() };
    change.ClassInstallHeader.cbSize = std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as _;
    change.ClassInstallHeader.InstallFunction = DIF_PROPERTYCHANGE;
    change.StateChange = state;
    change.Scope = DICS_FLAG_CONFIGSPECIFIC;
    if unsafe {
        SetupDiSetClassInstallParamsW(
            devices.0,
            &mut data,
            &mut change.ClassInstallHeader,
            std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as _,
        )
    } == FALSE
        || unsafe { SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, devices.0, &mut data) } == FALSE
    {
        bail!(
            "Change VDD device state failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut parameters: SP_DEVINSTALL_PARAMS_W = unsafe { std::mem::zeroed() };
    parameters.cbSize = std::mem::size_of::<SP_DEVINSTALL_PARAMS_W>() as _;
    if unsafe { SetupDiGetDeviceInstallParamsW(devices.0, &mut data, &mut parameters) } == FALSE {
        bail!(
            "Read VDD device state failed: {}",
            std::io::Error::last_os_error()
        );
    }
    if parameters.Flags & (DI_NEEDRESTART | DI_NEEDREBOOT) != 0 {
        bail!("Windows requires a restart to apply the VDD device change");
    }
    Ok(())
}

fn problem() -> ResultType<u32> {
    let (_, data) = device()?.ok_or_else(|| anyhow::anyhow!("VDD device disappeared"))?;
    let mut status = 0;
    let mut problem = 0;
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, data.DevInst, 0) };
    if result != CR_SUCCESS {
        bail!("Read VDD device problem failed (Configuration Manager {result})");
    }
    Ok(problem)
}

pub(super) fn get_monitor_count() -> usize {
    windows::get_device_names(Some(DEVICE_STRING)).len()
}

fn wait_for_count(count: usize) -> ResultType<()> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if get_monitor_count() == count {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Windows did not enumerate the requested {count} VDD monitors (actual {}, device problem {})", get_monitor_count(), problem()?);
}

fn restore_configuration(
    path: &Path,
    previous: Option<&[u8]>,
    registered: Option<&str>,
) -> ResultType<()> {
    match previous {
        Some(bytes) => write_profile(path, std::str::from_utf8(bytes)?)?,
        None if path.exists() => {
            check_path(path, false)?;
            fs::remove_file(path)?;
        }
        None => {}
    }
    let (key, _) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .create_subkey_with_flags(REGISTRY, KEY_READ | KEY_WRITE)?;
    match registered {
        Some(value) => key.set_value("VDDPATH", &value)?,
        None => match key.delete_value("VDDPATH") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

fn configure_count(count: u32, requested: &[virtual_display::MonitorMode]) -> ResultType<()> {
    let bundle = verified_bundle()?;
    let present = device()?.is_some();
    let enabled = present && problem()? != CM_PROB_DISABLED;
    let previous_count = get_monitor_count();
    let registered = registered_path()?;
    let folder = owned_folder(&bundle, present)?;
    let path = folder.join("vdd_settings.xml");
    let previous = if path.exists() {
        Some(bounded(&path, 1024 * 1024)?)
    } else {
        None
    };
    let xml = if count > 0 {
        let source = match &previous {
            Some(bytes) => bytes.clone(),
            None => bounded(&bundle.join("vdd_settings.xml"), 1024 * 1024)?,
        };
        let profile = Profile::parse(String::from_utf8(source)?).map_err(anyhow::Error::msg)?;
        let mut modes = collect_modes()?;
        for mode in requested {
            let refresh = mode
                .sync
                .checked_mul(10000)
                .ok_or_else(|| anyhow::anyhow!("Requested VDD refresh rate overflow"))?;
            modes.insert(Mode::new(mode.width, mode.height, refresh).map_err(anyhow::Error::msg)?);
        }
        Some(profile.render(count, modes).map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    let apply = (|| -> ResultType<()> {
        if let Some(xml) = xml {
            write_profile(&path, &xml)?;
            let (key, _) = RegKey::predef(HKEY_LOCAL_MACHINE)
                .create_subkey_with_flags(REGISTRY, KEY_READ | KEY_WRITE)?;
            key.set_value("VDDPATH", &ordinary_path(&folder))?;
            if !present {
                let mut reboot = false;
                unsafe {
                    win_device::install_driver(
                        bundle.join("MttVDD.inf").to_str().ok_or_else(|| {
                            anyhow::anyhow!("VDD package path is not valid Unicode")
                        })?,
                        HARDWARE_ID,
                        &mut reboot,
                    )?;
                }
                if reboot {
                    bail!("Windows requires a restart to start the VDD driver");
                }
            }
            change_device(DICS_ENABLE)?;
            change_device(DICS_PROPCHANGE)?;
        } else if present {
            change_device(DICS_DISABLE)?;
        }
        wait_for_count(count as usize)
    })();
    if let Err(cause) = apply {
        // An adapter registered by this attempt stays disabled on failure. Keep
        // its owned path so a later authorized retry cannot mistake it for an
        // unrelated adapter; no registered driver or user configuration is deleted.
        let owned = ordinary_path(&folder);
        let restore_path =
            registered
                .as_deref()
                .or(if !present { Some(owned.as_str()) } else { None });
        let configuration = restore_configuration(&path, previous.as_deref(), restore_path);
        let device_state = (|| -> ResultType<()> {
            if device()?.is_some() {
                change_device(if enabled { DICS_ENABLE } else { DICS_DISABLE })?;
                if enabled {
                    change_device(DICS_PROPCHANGE)?;
                }
            }
            wait_for_count(previous_count)
        })();
        let errors: Vec<_> = [configuration, device_state]
            .into_iter()
            .filter_map(|result| result.err())
            .map(|error| format!("{error:#}"))
            .collect();
        if errors.is_empty() {
            return Err(cause
                .context("VDD change failed; previous configuration and display count restored"));
        }
        bail!(
            "VDD change failed: {cause:#}; restoring previous configuration failed: {}",
            errors.join("; ")
        );
    }
    Ok(())
}

pub(super) fn plug_in_headless() -> ResultType<()> {
    let _guard = LOCK.lock().unwrap();
    if get_monitor_count() == 0 {
        configure_count(1, &[])?;
    }
    Ok(())
}

pub(super) fn plug_in_monitor(modes: &[virtual_display::MonitorMode]) -> ResultType<()> {
    let _guard = LOCK.lock().unwrap();
    let count = u32::try_from(get_monitor_count())?
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("VDD monitor count overflow"))?;
    configure_count(count, modes)
}

pub(super) fn plug_out_monitor(index: i32, force_all: bool) -> ResultType<()> {
    let _guard = LOCK.lock().unwrap();
    let count = get_monitor_count();
    if count == 0 {
        bail!("No VDD monitor is available");
    }
    if index != -1 && index as usize != count {
        bail!("Only the most recently added VDD monitor can be removed");
    }
    let target = if index == -1 { 0 } else { count - 1 };
    if target == 0 && !force_all && windows::get_device_names(None).len() <= count {
        bail!("The only remaining display cannot be removed");
    }
    configure_count(u32::try_from(target)?, &[])
}

pub(super) fn reset_all() -> ResultType<()> {
    let bundle = bundle_path()?;
    let folder = bundle.join("config");
    if registered_path()?.as_deref() != Some(ordinary_path(&folder).as_str()) {
        return Ok(());
    }
    if !folder.join("owner.txt").exists() {
        return Ok(());
    }
    if get_monitor_count() > 0 && windows::get_device_names(None).len() > get_monitor_count() {
        plug_out_monitor(-1, false)?;
    }
    Ok(())
}
