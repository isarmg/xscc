// SPDX-License-Identifier: MIT
use crate::{protocol, Capabilities, ConnectorId, Error, Mode, Monitor, Result};
use std::{
    ffi::c_void,
    io, mem,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};

#[cfg(not(target_arch = "x86_64"))]
compile_error!("The independent XSCC IDD transport currently supports Windows x86_64 only");

type Handle = *mut c_void;
const INVALID_HANDLE: Handle = -1isize as Handle;
const INSUFFICIENT_BUFFER: i32 = 122;
const NO_MORE_ITEMS: i32 = 259;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}
const INTERFACE: Guid = Guid {
    data1: 0x3b1a9721,
    data2: 0xeba5,
    data3: 0x4125,
    data4: [0xb0, 0x21, 0xdf, 0xce, 0xc0, 0x5e, 0x35, 0xb0],
};

#[repr(C)]
struct InterfaceData {
    size: u32,
    class: Guid,
    flags: u32,
    reserved: usize,
}
#[repr(C)]
struct DeviceData {
    size: u32,
    class: Guid,
    instance: u32,
    reserved: usize,
}
const _: () = assert!(mem::size_of::<Guid>() == 16 && mem::align_of::<Guid>() == 4);
const _: () = assert!(mem::size_of::<InterfaceData>() == 32);
const _: () = assert!(mem::size_of::<DeviceData>() == 32);

#[link(name = "setupapi")]
unsafe extern "system" {
    fn SetupDiGetClassDevsW(
        class: *const Guid,
        enumerator: *const u16,
        parent: Handle,
        flags: u32,
    ) -> Handle;
    fn SetupDiDestroyDeviceInfoList(devices: Handle) -> i32;
    fn SetupDiEnumDeviceInterfaces(
        devices: Handle,
        device: *mut DeviceData,
        class: *const Guid,
        index: u32,
        interface: *mut InterfaceData,
    ) -> i32;
    fn SetupDiGetDeviceInterfaceDetailW(
        devices: Handle,
        interface: *mut InterfaceData,
        detail: *mut c_void,
        bytes: u32,
        required: *mut u32,
        device: *mut DeviceData,
    ) -> i32;
    fn SetupDiGetDeviceInstanceIdW(
        devices: Handle,
        device: *mut DeviceData,
        id: *mut u16,
        characters: u32,
        required: *mut u32,
    ) -> i32;
    fn SetupDiGetDeviceRegistryPropertyW(
        devices: Handle,
        device: *mut DeviceData,
        property: u32,
        kind: *mut u32,
        bytes: *mut u8,
        length: u32,
        required: *mut u32,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn DeviceIoControl(
        device: Handle,
        code: u32,
        input: *const c_void,
        input_bytes: u32,
        output: *mut c_void,
        output_bytes: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

fn os_error(operation: &'static str) -> Error {
    Error::io(operation, io::Error::last_os_error())
}

struct DeviceSet(Handle);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn terminated<'a>(bytes: &'a [u16], operation: &'static str) -> Result<&'a [u16]> {
    let Some(end) = bytes.iter().position(|&value| value == 0) else {
        return Err(Error::protocol(operation, "unterminated device string"));
    };
    if end == 0 {
        return Err(Error::protocol(operation, "empty device string"));
    }
    Ok(&bytes[..end])
}

fn root_identity(devices: &DeviceSet, device: &mut DeviceData) -> Result<bool> {
    let operation = "read XsccIdd device identity";
    let mut required = 0;
    if unsafe { SetupDiGetDeviceInstanceIdW(devices.0, device, ptr::null_mut(), 0, &mut required) }
        != 0
    {
        return Err(Error::protocol(operation, "unexpected empty instance ID"));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(INSUFFICIENT_BUFFER) {
        return Err(Error::io(operation, error));
    }
    if !(2..=32768).contains(&required) {
        return Err(Error::protocol(operation, "invalid instance ID length"));
    }
    let mut instance = vec![0u16; required as usize];
    if unsafe {
        SetupDiGetDeviceInstanceIdW(
            devices.0,
            device,
            instance.as_mut_ptr(),
            required,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(os_error(operation));
    }
    let instance = String::from_utf16(terminated(&instance, operation)?)
        .map_err(|_| Error::protocol(operation, "invalid instance ID encoding"))?;
    if !instance.to_ascii_uppercase().starts_with("ROOT\\XSCCIDD\\") {
        return Ok(false);
    }

    let mut kind = 0;
    required = 0;
    // SPDRP_HARDWAREID is a REG_MULTI_SZ, not a display name or a parent match.
    if unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            devices.0,
            device,
            1,
            &mut kind,
            ptr::null_mut(),
            0,
            &mut required,
        )
    } != 0
    {
        return Err(Error::protocol(operation, "unexpected empty hardware IDs"));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(INSUFFICIENT_BUFFER) {
        return Err(Error::io(operation, error));
    }
    if !(4..=65536).contains(&required) || required % 2 != 0 {
        return Err(Error::protocol(operation, "invalid hardware ID length"));
    }
    let mut bytes = vec![0u8; required as usize];
    let capacity = required;
    if unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            devices.0,
            device,
            1,
            &mut kind,
            bytes.as_mut_ptr(),
            capacity,
            &mut required,
        )
    } == 0
    {
        return Err(os_error(operation));
    }
    if kind != 7 || required > capacity || required < 4 || required % 2 != 0 {
        return Err(Error::protocol(operation, "invalid hardware ID property"));
    }
    let words: Vec<u16> = bytes[..required as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if !words.ends_with(&[0, 0]) {
        return Err(Error::protocol(operation, "unterminated hardware IDs"));
    }
    for id in words[..words.len() - 1].split(|&value| value == 0) {
        let id = String::from_utf16(id)
            .map_err(|_| Error::protocol(operation, "invalid hardware ID encoding"))?;
        if id.eq_ignore_ascii_case("Root\\XsccIdd") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn device_path() -> Result<Vec<u16>> {
    let operation = "enumerate XsccIdd control interface";
    // DIGCF_PRESENT | DIGCF_DEVICEINTERFACE.
    let handle = unsafe { SetupDiGetClassDevsW(&INTERFACE, ptr::null(), ptr::null_mut(), 0x12) };
    if handle == INVALID_HANDLE {
        return Err(os_error(operation));
    }
    let devices = DeviceSet(handle);
    let mut selected = None;
    for index in 0..4096 {
        let mut interface: InterfaceData = unsafe { mem::zeroed() };
        interface.size = mem::size_of::<InterfaceData>() as u32;
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                devices.0,
                ptr::null_mut(),
                &INTERFACE,
                index,
                &mut interface,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(NO_MORE_ITEMS) {
                return selected.ok_or_else(|| {
                    Error::io(
                        operation,
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            "Root\\XsccIdd control interface is not present",
                        ),
                    )
                });
            }
            return Err(Error::io(operation, error));
        }
        let mut required = 0;
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                devices.0,
                &mut interface,
                ptr::null_mut(),
                0,
                &mut required,
                ptr::null_mut(),
            )
        } != 0
        {
            return Err(Error::protocol(
                operation,
                "unexpected empty interface detail",
            ));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(INSUFFICIENT_BUFFER) {
            return Err(Error::io(operation, error));
        }
        if !(8..=65536).contains(&required) || required % 2 != 0 {
            return Err(Error::protocol(
                operation,
                "invalid interface detail length",
            ));
        }
        // The x64 detail cbSize is 8, but DevicePath begins at byte offset 4.
        // usize storage supplies the required native alignment for the API buffer.
        let mut storage = vec![0usize; (required as usize).div_ceil(mem::size_of::<usize>())];
        let detail = storage.as_mut_ptr().cast::<u8>();
        unsafe {
            detail.cast::<u32>().write(8);
        }
        let mut device: DeviceData = unsafe { mem::zeroed() };
        device.size = mem::size_of::<DeviceData>() as u32;
        let capacity = required;
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                devices.0,
                &mut interface,
                detail.cast(),
                capacity,
                &mut required,
                &mut device,
            )
        } == 0
        {
            return Err(os_error(operation));
        }
        if required < 8 || required > capacity || required % 2 != 0 {
            return Err(Error::protocol(operation, "interface detail changed size"));
        }
        if !root_identity(&devices, &mut device)? {
            continue;
        }
        if selected.is_some() {
            return Err(Error::protocol(
                operation,
                "multiple Root\\XsccIdd interfaces; refusing ambiguous owner",
            ));
        }
        let bytes = unsafe { std::slice::from_raw_parts(detail.add(4), required as usize - 4) };
        let words: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let mut path = terminated(&words, operation)?.to_vec();
        path.push(0);
        selected = Some(path);
    }
    Err(Error::protocol(
        operation,
        "control interface enumeration exceeded its bound",
    ))
}

/// A control-file lease. It is deliberately not Clone and never reopens per call.
///
/// Keep this owner alive for its screens. A separate Owner creates a separate
/// lease. Closing/dropping it invokes driver cleanup, which is not a guaranteed
/// bounded operation. Use Remove and verify inventory before ordinary shutdown.
pub struct Owner {
    handle: OwnedHandle,
}

impl Owner {
    pub fn open() -> Result<Self> {
        let path = device_path()?;
        // The driver accepts separate file leases. Read/write sharing permits
        // independent temporary and persistent owners; it does not share a lease.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                0xc000_0000,
                3,
                ptr::null(),
                3,
                0x80,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE {
            return Err(os_error("open XsccIdd control owner"));
        }
        let mut owner = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        };
        owner.query()?;
        Ok(owner)
    }

    fn exchange(
        &mut self,
        operation: &'static str,
        code: u32,
        input: &[u8],
        capacity: usize,
    ) -> Result<Vec<u8>> {
        let mut output = vec![0u8; capacity];
        let mut written = 0;
        if unsafe {
            DeviceIoControl(
                self.handle.as_raw_handle(),
                code,
                input.as_ptr().cast(),
                input.len() as u32,
                output.as_mut_ptr().cast(),
                capacity as u32,
                &mut written,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(os_error(operation));
        }
        if written as usize > capacity {
            return Err(Error::protocol(
                operation,
                "driver returned an oversized response",
            ));
        }
        output.truncate(written as usize);
        Ok(output)
    }

    /// Returns completed arrivals across all owners, not GPU limits or CCD state.
    pub fn query(&mut self) -> Result<Capabilities> {
        let input = protocol::request(None, &[])?;
        protocol::capabilities(&self.exchange("query XsccIdd", protocol::QUERY, &input, 24)?)
    }

    /// Lists every owner's connected monitors. Remove still requires this lease.
    pub fn list(&mut self) -> Result<Vec<Monitor>> {
        let input = protocol::request(None, &[])?;
        protocol::monitors(&self.exchange(
            "list XsccIdd",
            protocol::LIST,
            &input,
            protocol::MAX_LIST_BYTES,
        )?)
    }

    /// Adds one monitor owned by this file. None asks the driver to allocate a connector.
    /// An error does not authorize blindly retrying a mutation; reconcile inventory first.
    pub fn add(&mut self, connector: Option<ConnectorId>, modes: &[Mode]) -> Result<ConnectorId> {
        if modes.is_empty() {
            return Err(Error::protocol(
                "add XsccIdd",
                "at least one mode is required",
            ));
        }
        let input = protocol::request(connector, modes)?;
        protocol::completed(
            &self.exchange("add XsccIdd", protocol::ADD, &input, 12)?,
            connector,
        )
    }

    /// Stops/departs this owner's connector. Zero monitors leaves the adapter running.
    pub fn remove(&mut self, connector: ConnectorId) -> Result<()> {
        let input = protocol::request(Some(connector), &[])?;
        protocol::completed(
            &self.exchange("remove XsccIdd", protocol::REMOVE, &input, 12)?,
            Some(connector),
        )?;
        Ok(())
    }
}
