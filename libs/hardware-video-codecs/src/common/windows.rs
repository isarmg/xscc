use super::AdapterVendor;
use ::windows::{
    core::{Interface, Result},
    Win32::{
        Foundation::HANDLE,
        Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
        },
    },
};
use std::{
    mem::size_of_val,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::OnceLock,
};

const CODEC_VENDORS: [u32; 3] = [
    AdapterVendor::ADAPTER_VENDOR_NVIDIA as u32,
    AdapterVendor::ADAPTER_VENDOR_AMD as u32,
    AdapterVendor::ADAPTER_VENDOR_INTEL as u32,
];

pub(super) fn gpu_signature() -> u64 {
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(error) => {
            log::error!("Failed to create the DXGI factory for the GPU signature: {error}");
            return u64::MAX;
        }
    };

    let mut signature = 0_u64;
    for index in 0..=u32::MAX {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if !CODEC_VENDORS.contains(&description.VendorId) {
            continue;
        }
        signature = signature
            .wrapping_add(u64::from(description.VendorId))
            .wrapping_add(u64::from(description.DeviceId))
            .wrapping_add(u64::from(description.SubSysId))
            .wrapping_add(u64::from(description.Revision));
        if let Ok(version) = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) } {
            signature = signature.wrapping_add(version as u64);
        }
    }
    signature
}

// The unnamed job's sole handle remains in this process. Windows closes it on
// exit, including forced exit, and then terminates any remaining codec probes.
static PROBE_JOB: OnceLock<OwnedHandle> = OnceLock::new();

pub(super) fn assign_child_to_job(process_id: u32) -> Result<()> {
    let process = unsafe {
        let raw = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, process_id)?;
        OwnedHandle::from_raw_handle(raw.0)
    };
    let job = match PROBE_JOB.get() {
        Some(existing) => existing,
        None => {
            let created = new_probe_job()?;
            PROBE_JOB.get_or_init(|| created)
        }
    };
    unsafe {
        AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(process.as_raw_handle()))
    }
}

fn new_probe_job() -> Result<OwnedHandle> {
    unsafe {
        let raw = CreateJobObjectW(None, None)?;
        let owned = OwnedHandle::from_raw_handle(raw.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            size_of_val(&limits) as u32,
        )?;
        Ok(owned)
    }
}
