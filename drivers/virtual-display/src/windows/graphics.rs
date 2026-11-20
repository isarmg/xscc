use super::{
    Frame, SUCCESS, Status, UNSUCCESSFUL,
    wdk::{self, abi, idd, size},
};
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
};

pub(super) struct Com<T>(NonNull<T>);

impl<T> Com<T> {
    fn from_output(raw: *mut T, status: Status) -> Result<Self, Status> {
        let owned = NonNull::new(raw).map(Self);
        if status < 0 {
            return Err(status);
        }
        owned.ok_or(UNSUCCESSFUL)
    }

    fn as_ptr(&self) -> *mut T {
        self.0.as_ptr()
    }

    fn query<U>(&self, iid: *const abi::IID) -> Result<Com<U>, Status> {
        let unknown = self.as_ptr().cast::<abi::IUnknown>();
        let mut result = ptr::null_mut();
        let status = unsafe {
            ((*(*unknown).lpVtbl)
                .QueryInterface
                .expect("invariant: IUnknown query method"))(unknown, iid, &mut result)
        };
        Com::from_output(result.cast(), status)
    }
}

impl<T> Drop for Com<T> {
    fn drop(&mut self) {
        let unknown = self.as_ptr().cast::<abi::IUnknown>();
        unsafe {
            ((*(*unknown).lpVtbl)
                .Release
                .expect("invariant: IUnknown release method"))(unknown);
        }
    }
}

fn factory<T>(iid: *const abi::IID) -> Result<Com<T>, Status> {
    let mut output = ptr::null_mut();
    let status = unsafe { abi::CreateDXGIFactory1(iid, &mut output) };
    Com::from_output(output.cast(), status)
}

fn can_render(luid: abi::LUID) -> bool {
    let mut opened = abi::D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    // The SDK declares a const pointer, but this DDI writes hAdapter into the record.
    if unsafe { abi::D3DKMTOpenAdapterFromLuid(ptr::addr_of_mut!(opened)) } < 0 {
        return false;
    }

    let mut kind = abi::D3DKMT_ADAPTERTYPE::default();
    let query = abi::D3DKMT_QUERYADAPTERINFO {
        hAdapter: opened.hAdapter,
        Type: abi::KMTQAITYPE_ADAPTERTYPE,
        pPrivateDriverData: ptr::addr_of_mut!(kind).cast(),
        PrivateDriverDataSize: size::<abi::D3DKMT_ADAPTERTYPE>(),
    };
    let status = unsafe { abi::D3DKMTQueryAdapterInfo(&query) };
    let closed = unsafe {
        abi::D3DKMTCloseAdapter(&abi::D3DKMT_CLOSEADAPTER {
            hAdapter: opened.hAdapter,
        })
    };
    let flags = unsafe { kind.__bindgen_anon_1.__bindgen_anon_1 };
    status >= 0
        && closed >= 0
        && flags.RenderSupported() != 0
        && flags.IndirectDisplayDevice() == 0
        && flags.SoftwareDevice() == 0
}

pub(super) fn select_adapter(name: &[u16]) -> Option<abi::LUID> {
    let factory = factory::<abi::IDXGIFactory1>(ptr::addr_of!(abi::IID_IDXGIFactory1)).ok()?;
    let factory = factory.as_ptr();
    let enumerate = unsafe { (*(*factory).lpVtbl).EnumAdapters1 }?;
    let mut selected = None;
    let mut most_memory = 0;
    for index in 0.. {
        let mut candidate = ptr::null_mut();
        let status = unsafe { enumerate(factory, index, &mut candidate) };
        let Ok(candidate) = Com::from_output(candidate, status) else {
            break;
        };
        let mut description = abi::DXGI_ADAPTER_DESC1::default();
        let get_description = unsafe { (*(*candidate.as_ptr()).lpVtbl).GetDesc1 }?;
        if unsafe { get_description(candidate.as_ptr(), &mut description) } < 0
            || !can_render(description.AdapterLuid)
        {
            continue;
        }
        if name.first().is_some_and(|first| *first != 0)
            && unsafe {
                abi::CompareStringOrdinal(
                    name.as_ptr(),
                    -1,
                    description.Description.as_ptr(),
                    -1,
                    1,
                )
            } == abi::CSTR_EQUAL as i32
        {
            return Some(description.AdapterLuid);
        }
        if selected.is_none() || description.DedicatedVideoMemory > most_memory {
            selected = Some(description.AdapterLuid);
            most_memory = description.DedicatedVideoMemory;
        }
    }
    selected
}

pub(super) fn bind_swapchain(
    chain: abi::IDDCX_SWAPCHAIN,
    luid: abi::LUID,
) -> Result<Com<abi::IDXGIDevice>, Status> {
    let factory = factory::<abi::IDXGIFactory4>(ptr::addr_of!(abi::IID_IDXGIFactory4))?;
    let enumerate =
        unsafe { (*(*factory.as_ptr()).lpVtbl).EnumAdapterByLuid }.ok_or(UNSUCCESSFUL)?;
    let mut adapter: *mut c_void = ptr::null_mut();
    let status = unsafe {
        enumerate(
            factory.as_ptr(),
            luid,
            ptr::addr_of!(abi::IID_IDXGIAdapter1),
            &mut adapter,
        )
    };
    let adapter = Com::<abi::IDXGIAdapter1>::from_output(adapter.cast(), status)?;

    let mut d3d = ptr::null_mut();
    let status = unsafe {
        abi::D3D11CreateDevice(
            adapter.as_ptr().cast(),
            abi::D3D_DRIVER_TYPE_UNKNOWN,
            ptr::null_mut(),
            abi::D3D11_CREATE_DEVICE_BGRA_SUPPORT as u32,
            ptr::null(),
            0,
            abi::D3D11_SDK_VERSION,
            &mut d3d,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    let d3d = Com::from_output(d3d, status)?;
    let device: Com<abi::IDXGIDevice> = d3d.query(ptr::addr_of!(abi::IID_IDXGIDevice))?;
    let input = abi::IDARG_IN_SWAPCHAINSETDEVICE {
        pDevice: device.as_ptr(),
    };
    let status = unsafe {
        idd!(
            IddCxSwapChainSetDeviceTableIndex,
            PFN_IDDCXSWAPCHAINSETDEVICE,
            chain,
            &input
        )
    };
    if status < 0 {
        return Err(status);
    }
    Ok(device)
}

pub(super) fn acquire_frame(chain: abi::IDDCX_SWAPCHAIN) -> Result<Frame, Status> {
    let (status, surface, number) =
        if wdk::idd_available(abi::IddCxSwapChainReleaseAndAcquireBuffer2TableIndex) {
            let mut input = abi::IDARG_IN_RELEASEANDACQUIREBUFFER2 {
                Size: size::<abi::IDARG_IN_RELEASEANDACQUIREBUFFER2>(),
                ..Default::default()
            };
            let mut output = abi::IDARG_OUT_RELEASEANDACQUIREBUFFER2::default();
            let status = unsafe {
                idd!(
                    IddCxSwapChainReleaseAndAcquireBuffer2TableIndex,
                    PFN_IDDCXSWAPCHAINRELEASEANDACQUIREBUFFER2,
                    chain,
                    &mut input,
                    &mut output
                )
            };
            (
                status,
                output.MetaData.pSurface,
                output.MetaData.PresentationFrameNumber,
            )
        } else {
            let mut output = abi::IDARG_OUT_RELEASEANDACQUIREBUFFER::default();
            let status = unsafe {
                idd!(
                    IddCxSwapChainReleaseAndAcquireBufferTableIndex,
                    PFN_IDDCXSWAPCHAINRELEASEANDACQUIREBUFFER,
                    chain,
                    &mut output
                )
            };
            (
                status,
                output.MetaData.pSurface,
                output.MetaData.PresentationFrameNumber,
            )
        };
    let _surface = NonNull::new(surface).map(Com);
    if status != SUCCESS {
        return Err(status);
    }

    let mut observed = abi::LARGE_INTEGER::default();
    if unsafe { abi::QueryPerformanceCounter(&mut observed) } == 0 {
        return Err(UNSUCCESSFUL);
    }
    Ok(Frame {
        acquired_at: unsafe { observed.QuadPart } as u64,
        number,
    })
}

pub(super) fn finish_frame(chain: abi::IDDCX_SWAPCHAIN, frame: Frame) -> Status {
    let status = unsafe {
        idd!(
            IddCxSwapChainFinishedProcessingFrameTableIndex,
            PFN_IDDCXSWAPCHAINFINISHEDPROCESSINGFRAME,
            chain
        )
    };
    if status < 0 {
        return status;
    }

    // The virtual driver discards the surface. The Host captures it independently.
    let report = abi::IDARG_IN_REPORTFRAMESTATISTICS {
        FrameStatistics: abi::IDDCX_FRAME_STATISTICS {
            Size: size::<abi::IDDCX_FRAME_STATISTICS>(),
            PresentationFrameNumber: frame.number,
            FrameStatus: abi::IDDCX_FRAME_STATUS_DROPPED,
            FrameSliceTotal: 1,
            FrameAcquireQpcTime: frame.acquired_at,
            ..Default::default()
        },
    };
    unsafe {
        idd!(
            IddCxSwapChainReportFrameStatisticsTableIndex,
            PFN_IDDCXSWAPCHAINREPORTFRAMESTATISTICS,
            chain,
            &report
        )
    }
}
