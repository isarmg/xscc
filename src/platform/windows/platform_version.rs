use std::mem;
use winapi::{
    shared::minwindef::{BOOL, FALSE},
    um::{
        libloaderapi::{GetModuleHandleW, GetProcAddress},
        processthreadsapi::GetCurrentProcess,
        winnt::{HANDLE, IMAGE_FILE_MACHINE_AMD64, OSVERSIONINFOEXW, VER_NT_WORKSTATION},
    },
};
use windows::core::w;

pub(super) fn os_version() -> Option<OSVERSIONINFOEXW> {
    type RtlGetVersion = unsafe extern "system" fn(*mut OSVERSIONINFOEXW) -> i32;
    unsafe {
        let module = GetModuleHandleW(w!("ntdll.dll").as_ptr());
        if module.is_null() {
            return None;
        }
        let address = GetProcAddress(module, c"RtlGetVersion".as_ptr());
        if address.is_null() {
            return None;
        }
        let get_version: RtlGetVersion = mem::transmute(address);
        let mut version = OSVERSIONINFOEXW {
            dwOSVersionInfoSize: mem::size_of::<OSVERSIONINFOEXW>() as u32,
            ..mem::zeroed()
        };
        (get_version(&mut version) == 0).then_some(version)
    }
}

pub(super) fn native_machine() -> Option<u16> {
    type IsWow64Process2 = unsafe extern "system" fn(HANDLE, *mut u16, *mut u16) -> BOOL;
    unsafe {
        let module = GetModuleHandleW(w!("kernel32.dll").as_ptr());
        if module.is_null() {
            return None;
        }
        let address = GetProcAddress(module, c"IsWow64Process2".as_ptr());
        if address.is_null() {
            return None;
        }
        let get_machine: IsWow64Process2 = mem::transmute(address);
        let (mut process, mut native) = (0, 0);
        (get_machine(GetCurrentProcess(), &mut process, &mut native) != FALSE).then_some(native)
    }
}

pub(super) fn supports_desktop() -> bool {
    os_version().is_some_and(|version| {
        version.dwMajorVersion == 10
            && version.dwMinorVersion == 0
            && version.dwBuildNumber >= 22000
            && version.wProductType == VER_NT_WORKSTATION
            && native_machine() == Some(IMAGE_FILE_MACHINE_AMD64)
    })
}
