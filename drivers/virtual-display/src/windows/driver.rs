use super::{
    Colour, DATA_ERROR, Device, INVALID_DEVICE_STATE, Monitor, SUCCESS, Signal, Status,
    UNSUCCESSFUL, active_profile, advertise_modes, graphics,
    wdk::{self, abi, idd, size, wdf},
};
use std::{ffi::c_void, mem::offset_of, ptr};

#[repr(transparent)]
struct ContextInfo(abi::WDF_OBJECT_CONTEXT_TYPE_INFO);

// UMDF uses the stable descriptor address as the context type identity. Both descriptors
// and their names are immutable for the lifetime of the driver.
unsafe impl Sync for ContextInfo {}

static DEVICE_CONTEXT: ContextInfo = ContextInfo(abi::WDF_OBJECT_CONTEXT_TYPE_INFO {
    Size: size::<abi::WDF_OBJECT_CONTEXT_TYPE_INFO>(),
    ContextName: c"DisplayDeviceContext".as_ptr(),
    ContextSize: size::<*mut Device>() as usize,
    UniqueType: ptr::addr_of!(DEVICE_CONTEXT.0),
    EvtDriverGetUniqueContextType: None,
});

static MONITOR_CONTEXT: ContextInfo = ContextInfo(abi::WDF_OBJECT_CONTEXT_TYPE_INFO {
    Size: size::<abi::WDF_OBJECT_CONTEXT_TYPE_INFO>(),
    ContextName: c"DisplayMonitorContext".as_ptr(),
    ContextSize: size::<*mut Monitor>() as usize,
    UniqueType: ptr::addr_of!(MONITOR_CONTEXT.0),
    EvtDriverGetUniqueContextType: None,
});

fn attributes(
    context: &'static ContextInfo,
    cleanup: abi::PFN_WDF_OBJECT_CONTEXT_CLEANUP,
) -> abi::WDF_OBJECT_ATTRIBUTES {
    abi::WDF_OBJECT_ATTRIBUTES {
        Size: size::<abi::WDF_OBJECT_ATTRIBUTES>(),
        EvtCleanupCallback: cleanup,
        ExecutionLevel: abi::WdfExecutionLevelInheritFromParent,
        SynchronizationScope: abi::WdfSynchronizationScopeInheritFromParent,
        ContextTypeInfo: &context.0,
        ..Default::default()
    }
}

unsafe fn context_slot<T>(object: abi::WDFOBJECT, info: &'static ContextInfo) -> *mut *mut T {
    unsafe {
        wdf!(
            WdfObjectGetTypedContextWorkerTableIndex,
            PFN_WDFOBJECTGETTYPEDCONTEXTWORKER,
            object,
            info.0.UniqueType,
        )
        .cast()
    }
}

unsafe fn monitor_state(monitor: abi::IDDCX_MONITOR) -> *mut Monitor {
    unsafe { *context_slot(monitor.cast(), &MONITOR_CONTEXT) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn DriverEntry(
    driver: abi::PDRIVER_OBJECT,
    registry: abi::PCUNICODE_STRING,
) -> Status {
    let mut configuration = abi::WDF_DRIVER_CONFIG {
        Size: size::<abi::WDF_DRIVER_CONFIG>(),
        EvtDriverDeviceAdd: Some(add_device),
        ..Default::default()
    };
    unsafe {
        wdf!(
            WdfDriverCreateTableIndex,
            PFN_WDFDRIVERCREATE,
            driver,
            registry,
            ptr::null_mut(),
            &mut configuration,
            ptr::null_mut()
        )
    }
}

unsafe extern "C" fn add_device(_: abi::WDFDRIVER, mut init: abi::PWDFDEVICE_INIT) -> Status {
    let mut power = abi::WDF_PNPPOWER_EVENT_CALLBACKS {
        Size: size::<abi::WDF_PNPPOWER_EVENT_CALLBACKS>(),
        EvtDeviceD0Entry: Some(enter_d0),
        ..Default::default()
    };
    unsafe {
        wdf!(
            WdfDeviceInitSetPnpPowerEventCallbacksTableIndex,
            PFN_WDFDEVICEINITSETPNPPOWEREVENTCALLBACKS,
            init,
            &mut power
        );
    }

    let mut configuration = abi::IDD_CX_CLIENT_CONFIG {
        Size: wdk::client_config_size(),
        EvtIddCxAdapterInitFinished: Some(adapter_ready),
        EvtIddCxMonitorGetDefaultDescriptionModes: Some(default_modes),
        EvtIddCxMonitorAssignSwapChain: Some(assign_swapchain),
        EvtIddCxMonitorUnassignSwapChain: Some(unassign_swapchain),
        ..Default::default()
    };
    // The IddCx structure size also governs which callback fields an older runtime reads.
    if (configuration.Size as usize)
        > offset_of!(abi::IDD_CX_CLIENT_CONFIG, EvtIddCxAdapterQueryTargetInfo)
    {
        configuration.EvtIddCxAdapterQueryTargetInfo = Some(query_target_info);
        configuration.EvtIddCxMonitorSetDefaultHdrMetaData = Some(set_hdr_metadata);
        configuration.EvtIddCxParseMonitorDescription2 = Some(parse_description2);
        configuration.EvtIddCxMonitorQueryTargetModes2 = Some(query_modes2);
        configuration.EvtIddCxAdapterCommitModes2 = Some(commit_modes2);
        configuration.EvtIddCxMonitorSetGammaRamp = Some(set_gamma_ramp);
    } else {
        configuration.EvtIddCxParseMonitorDescription = Some(parse_description);
        configuration.EvtIddCxMonitorQueryTargetModes = Some(query_modes);
        configuration.EvtIddCxAdapterCommitModes = Some(commit_modes);
    }
    let status = unsafe {
        idd!(
            IddCxDeviceInitConfigTableIndex,
            PFN_IDDCXDEVICEINITCONFIG,
            init,
            &configuration
        )
    };
    if status < 0 {
        return status;
    }

    let mut device = ptr::null_mut();
    let mut attributes = attributes(&DEVICE_CONTEXT, Some(destroy_device));
    let status = unsafe {
        wdf!(
            WdfDeviceCreateTableIndex,
            PFN_WDFDEVICECREATE,
            &mut init,
            &mut attributes,
            &mut device
        )
    };
    if status < 0 {
        return status;
    }
    let state = match Device::load() {
        Ok(state) => state,
        Err(error) => {
            super::diagnostic(&error.to_string());
            return DATA_ERROR;
        }
    };
    unsafe {
        context_slot::<Device>(device.cast(), &DEVICE_CONTEXT)
            .write(Box::into_raw(Box::new(state)));
        idd!(
            IddCxDeviceInitializeTableIndex,
            PFN_IDDCXDEVICEINITIALIZE,
            device
        )
    }
}

unsafe extern "C" fn enter_d0(device: abi::WDFDEVICE, _: abi::WDF_POWER_DEVICE_STATE) -> Status {
    let state = unsafe { *context_slot::<Device>(device.cast(), &DEVICE_CONTEXT) };
    match unsafe { state.as_ref() } {
        Some(state) => state.start(device),
        None => INVALID_DEVICE_STATE,
    }
}

unsafe extern "C" fn destroy_device(object: abi::WDFOBJECT) {
    let state = unsafe { *context_slot::<Device>(object, &DEVICE_CONTEXT) };
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
    }
}

unsafe extern "C" fn destroy_monitor(object: abi::WDFOBJECT) {
    let state = unsafe { *context_slot::<Monitor>(object, &MONITOR_CONTEXT) };
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
    }
}

pub(super) fn initialize_adapter(device: abi::WDFDEVICE, count: u32) -> Status {
    let mut version = abi::IDDCX_ENDPOINT_VERSION {
        Size: size::<abi::IDDCX_ENDPOINT_VERSION>(),
        MajorVer: 1,
        ..Default::default()
    };
    let friendly_name = super::wide("VirtualDisplayDriver Device");
    let manufacturer = super::wide("MikeTheTech");
    let model = super::wide("VirtualDisplayDriver Model");
    let mut caps = abi::IDDCX_ADAPTER_CAPS {
        Size: size::<abi::IDDCX_ADAPTER_CAPS>(),
        Flags: if wdk::idd_available(abi::IddCxSwapChainReleaseAndAcquireBuffer2TableIndex) {
            abi::IDDCX_ADAPTER_FLAGS_CAN_PROCESS_FP16
        } else {
            0
        },
        MaxMonitorsSupported: count,
        EndPointDiagnostics: abi::IDDCX_ENDPOINT_DIAGNOSTIC_INFO {
            Size: size::<abi::IDDCX_ENDPOINT_DIAGNOSTIC_INFO>(),
            TransmissionType: abi::IDDCX_TRANSMISSION_TYPE_WIRED_OTHER,
            pEndPointFriendlyName: friendly_name.as_ptr(),
            pEndPointModelName: model.as_ptr(),
            pEndPointManufacturerName: manufacturer.as_ptr(),
            pHardwareVersion: &mut version,
            pFirmwareVersion: &mut version,
            GammaSupport: abi::IDDCX_FEATURE_IMPLEMENTATION_NONE,
        },
        ..Default::default()
    };
    let input = abi::IDARG_IN_ADAPTER_INIT {
        WdfDevice: device,
        pCaps: &mut caps,
        ..Default::default()
    };
    let mut output = abi::IDARG_OUT_ADAPTER_INIT::default();
    unsafe {
        idd!(
            IddCxAdapterInitAsyncTableIndex,
            PFN_IDDCXADAPTERINITASYNC,
            &input,
            &mut output
        )
    }
}

unsafe extern "C" fn adapter_ready(
    adapter: abi::IDDCX_ADAPTER,
    input: *const abi::IDARG_IN_ADAPTER_INIT_FINISHED,
) -> Status {
    let status = unsafe { (*input).AdapterInitStatus };
    if status < 0 {
        return status;
    }
    let Some(profile) = active_profile() else {
        return INVALID_DEVICE_STATE;
    };
    if wdk::idd_available(abi::IddCxAdapterSetRenderAdapterTableIndex) {
        let name = super::wide(profile.render_adapter.as_deref().unwrap_or_default());
        if let Some(luid) = graphics::select_adapter(&name) {
            let preferred = abi::IDARG_IN_ADAPTERSETRENDERADAPTER {
                PreferredRenderAdapter: luid,
            };
            unsafe {
                idd!(
                    IddCxAdapterSetRenderAdapterTableIndex,
                    PFN_IDDCXADAPTERSETRENDERADAPTER,
                    adapter,
                    &preferred
                )
            };
        }
    }
    for index in 0..profile.monitor_count {
        let status = arrive_monitor(adapter, index, &profile);
        if status < 0 {
            return status;
        }
    }
    SUCCESS
}

fn arrive_monitor(
    adapter: abi::IDDCX_ADAPTER,
    index: u32,
    profile: &std::sync::Arc<super::Profile>,
) -> Status {
    let mut info = abi::IDDCX_MONITOR_INFO {
        Size: size::<abi::IDDCX_MONITOR_INFO>(),
        MonitorType: abi::DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI,
        ConnectorIndex: index,
        MonitorDescription: abi::IDDCX_MONITOR_DESCRIPTION {
            Size: size::<abi::IDDCX_MONITOR_DESCRIPTION>(),
            Type: abi::IDDCX_MONITOR_DESCRIPTION_TYPE_EDID,
            DataSize: profile.edid.len() as u32,
            pData: profile.edid.as_ptr().cast_mut().cast(),
        },
        MonitorContainerId: abi::GUID::default(),
    };
    if unsafe { abi::CoCreateGuid(&mut info.MonitorContainerId) } < 0 {
        return UNSUCCESSFUL;
    }

    let mut attributes = attributes(&MONITOR_CONTEXT, Some(destroy_monitor));
    let input = abi::IDARG_IN_MONITORCREATE {
        ObjectAttributes: &mut attributes,
        pMonitorInfo: &mut info,
    };
    let mut monitor = abi::IDARG_OUT_MONITORCREATE::default();
    let status = unsafe {
        idd!(
            IddCxMonitorCreateTableIndex,
            PFN_IDDCXMONITORCREATE,
            adapter,
            &input,
            &mut monitor
        )
    };
    if status < 0 {
        return status;
    }
    let state = Box::new(Monitor::new(std::sync::Arc::clone(profile)));
    let mut output = abi::IDARG_OUT_MONITORARRIVAL::default();
    let status = unsafe {
        context_slot::<Monitor>(monitor.MonitorObject.cast(), &MONITOR_CONTEXT)
            .write(Box::into_raw(state));
        idd!(
            IddCxMonitorArrivalTableIndex,
            PFN_IDDCXMONITORARRIVAL,
            monitor.MonitorObject,
            &mut output
        )
    };
    if status < 0 {
        unsafe { delete_object(monitor.MonitorObject.cast()) };
    }
    status
}

pub(super) unsafe fn delete_object(object: *mut c_void) {
    unsafe { wdf!(WdfObjectDeleteTableIndex, PFN_WDFOBJECTDELETE, object) };
}

fn video_signal(signal: &Signal, target: bool) -> abi::DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
    let mut extra = abi::DISPLAYCONFIG_VIDEO_SIGNAL_INFO__bindgen_ty_1__bindgen_ty_1::default();
    extra.set_videoStandard(255);
    extra.set_vSyncFreqDivider(u32::from(target));
    abi::DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
        pixelRate: signal.pixel_rate,
        hSyncFreq: abi::DISPLAYCONFIG_RATIONAL {
            Numerator: signal.horizontal_numerator,
            Denominator: signal.horizontal_denominator,
        },
        vSyncFreq: abi::DISPLAYCONFIG_RATIONAL {
            Numerator: signal.vertical_numerator,
            Denominator: signal.vertical_denominator,
        },
        activeSize: abi::DISPLAYCONFIG_2DREGION {
            cx: signal.width,
            cy: signal.height,
        },
        totalSize: abi::DISPLAYCONFIG_2DREGION {
            cx: signal.total_width,
            cy: signal.total_height,
        },
        __bindgen_anon_1: abi::DISPLAYCONFIG_VIDEO_SIGNAL_INFO__bindgen_ty_1 {
            AdditionalSignalInfo: extra,
        },
        scanLineOrdering: abi::DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE,
    }
}

fn wire_colour(colour: &Colour) -> abi::IDDCX_WIRE_BITS_PER_COMPONENT {
    let sdr = if colour.sdr_bits == 10 {
        abi::IDDCX_BITS_PER_COMPONENT_10
    } else {
        abi::IDDCX_BITS_PER_COMPONENT_8
    };
    let hdr = if colour.hdr_bits == 12 {
        abi::IDDCX_BITS_PER_COMPONENT_12
    } else {
        abi::IDDCX_BITS_PER_COMPONENT_10
    };
    let mut output = abi::IDDCX_WIRE_BITS_PER_COMPONENT::default();
    *match colour.format {
        1 => &mut output.YCbCr444,
        2 => &mut output.YCbCr422,
        3 => &mut output.YCbCr420,
        _ => &mut output.Rgb,
    } = sdr | hdr;
    output
}

fn monitor_mode(signal: &Signal, _: &Colour) -> abi::IDDCX_MONITOR_MODE {
    abi::IDDCX_MONITOR_MODE {
        Size: size::<abi::IDDCX_MONITOR_MODE>(),
        Origin: abi::IDDCX_MONITOR_MODE_ORIGIN_DRIVER,
        MonitorVideoSignalInfo: video_signal(signal, false),
    }
}

fn monitor_mode2(signal: &Signal, colour: &Colour) -> abi::IDDCX_MONITOR_MODE2 {
    abi::IDDCX_MONITOR_MODE2 {
        Size: size::<abi::IDDCX_MONITOR_MODE2>(),
        Origin: abi::IDDCX_MONITOR_MODE_ORIGIN_DRIVER,
        MonitorVideoSignalInfo: video_signal(signal, false),
        BitsPerComponent: wire_colour(colour),
    }
}

fn target_mode(signal: &Signal, _: &Colour) -> abi::IDDCX_TARGET_MODE {
    abi::IDDCX_TARGET_MODE {
        Size: size::<abi::IDDCX_TARGET_MODE>(),
        TargetVideoSignalInfo: abi::DISPLAYCONFIG_TARGET_MODE {
            targetVideoSignalInfo: video_signal(signal, true),
        },
        ..Default::default()
    }
}

fn target_mode2(signal: &Signal, colour: &Colour) -> abi::IDDCX_TARGET_MODE2 {
    abi::IDDCX_TARGET_MODE2 {
        Size: size::<abi::IDDCX_TARGET_MODE2>(),
        TargetVideoSignalInfo: abi::DISPLAYCONFIG_TARGET_MODE {
            targetVideoSignalInfo: video_signal(signal, true),
        },
        BitsPerComponent: wire_colour(colour),
        ..Default::default()
    }
}

unsafe extern "C" fn parse_description(
    input: *const abi::IDARG_IN_PARSEMONITORDESCRIPTION,
    output: *mut abi::IDARG_OUT_PARSEMONITORDESCRIPTION,
) -> Status {
    unsafe {
        (*output).PreferredMonitorModeIdx = 0;
        advertise_modes(
            None,
            false,
            (*input).MonitorModeBufferInputCount,
            (*input).pMonitorModes,
            &raw mut (*output).MonitorModeBufferOutputCount,
            monitor_mode,
        )
    }
}

unsafe extern "C" fn parse_description2(
    input: *const abi::IDARG_IN_PARSEMONITORDESCRIPTION2,
    output: *mut abi::IDARG_OUT_PARSEMONITORDESCRIPTION,
) -> Status {
    unsafe {
        (*output).PreferredMonitorModeIdx = 0;
        advertise_modes(
            None,
            false,
            (*input).MonitorModeBufferInputCount,
            (*input).pMonitorModes,
            &raw mut (*output).MonitorModeBufferOutputCount,
            monitor_mode2,
        )
    }
}

unsafe extern "C" fn default_modes(
    monitor: abi::IDDCX_MONITOR,
    input: *const abi::IDARG_IN_GETDEFAULTDESCRIPTIONMODES,
    output: *mut abi::IDARG_OUT_GETDEFAULTDESCRIPTIONMODES,
) -> Status {
    unsafe {
        (*output).PreferredMonitorModeIdx = 0;
        advertise_modes(
            monitor_state(monitor).as_ref(),
            false,
            (*input).DefaultMonitorModeBufferInputCount,
            (*input).pDefaultMonitorModes,
            &raw mut (*output).DefaultMonitorModeBufferOutputCount,
            monitor_mode,
        )
    }
}

unsafe extern "C" fn query_modes(
    monitor: abi::IDDCX_MONITOR,
    input: *const abi::IDARG_IN_QUERYTARGETMODES,
    output: *mut abi::IDARG_OUT_QUERYTARGETMODES,
) -> Status {
    unsafe {
        advertise_modes(
            monitor_state(monitor).as_ref(),
            true,
            (*input).TargetModeBufferInputCount,
            (*input).pTargetModes,
            &raw mut (*output).TargetModeBufferOutputCount,
            target_mode,
        )
    }
}

unsafe extern "C" fn query_modes2(
    monitor: abi::IDDCX_MONITOR,
    input: *const abi::IDARG_IN_QUERYTARGETMODES2,
    output: *mut abi::IDARG_OUT_QUERYTARGETMODES,
) -> Status {
    unsafe {
        advertise_modes(
            monitor_state(monitor).as_ref(),
            true,
            (*input).TargetModeBufferInputCount,
            (*input).pTargetModes,
            &raw mut (*output).TargetModeBufferOutputCount,
            target_mode2,
        )
    }
}

unsafe extern "C" fn query_target_info(
    _: abi::IDDCX_ADAPTER,
    _: *mut abi::IDARG_IN_QUERYTARGET_INFO,
    output: *mut abi::IDARG_OUT_QUERYTARGET_INFO,
) -> Status {
    let Some(profile) = active_profile() else {
        return INVALID_DEVICE_STATE;
    };
    unsafe {
        (*output).TargetCaps =
            abi::IDDCX_TARGET_CAPS_HIGH_COLOR_SPACE | abi::IDDCX_TARGET_CAPS_WIDE_COLOR_SPACE;
        (*output).DitheringSupport = wire_colour(&profile.colour);
    }
    SUCCESS
}

unsafe extern "C" fn commit_modes(
    _: abi::IDDCX_ADAPTER,
    _: *const abi::IDARG_IN_COMMITMODES,
) -> Status {
    SUCCESS
}
unsafe extern "C" fn commit_modes2(
    _: abi::IDDCX_ADAPTER,
    _: *const abi::IDARG_IN_COMMITMODES2,
) -> Status {
    SUCCESS
}
unsafe extern "C" fn set_hdr_metadata(
    _: abi::IDDCX_MONITOR,
    _: *const abi::IDARG_IN_MONITOR_SET_DEFAULT_HDR_METADATA,
) -> Status {
    SUCCESS
}
unsafe extern "C" fn set_gamma_ramp(
    _: abi::IDDCX_MONITOR,
    _: *const abi::IDARG_IN_SET_GAMMARAMP,
) -> Status {
    SUCCESS
}

unsafe extern "C" fn assign_swapchain(
    monitor: abi::IDDCX_MONITOR,
    input: *const abi::IDARG_IN_SETSWAPCHAIN,
) -> Status {
    if let Some(state) = unsafe { monitor_state(monitor).as_ref() } {
        let input = unsafe { &*input };
        if state.assign(
            input.hSwapChain,
            input.hNextSurfaceAvailable,
            input.RenderAdapterLuid,
        ) >= 0
        {
            return SUCCESS;
        }
    }
    abi::XSCC_ABANDON_SWAPCHAIN
}

unsafe extern "C" fn unassign_swapchain(monitor: abi::IDDCX_MONITOR) -> Status {
    let Some(state) = (unsafe { monitor_state(monitor).as_ref() }) else {
        return INVALID_DEVICE_STATE;
    };
    state.unassign();
    SUCCESS
}
