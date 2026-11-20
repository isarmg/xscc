// Copyright (c) Microsoft Corporation
// SPDX-License-Identifier: MIT

#define NOMINMAX
#include <windows.h>
#include <wudfwdm.h>
#include <wdf.h>
#include <iddcx.h>
#include <d3d11.h>
#include <d3dkmthk.h>
#include <dxgi1_5.h>
#include <wrl/client.h>
#include <climits>
#include <cstring>

using Microsoft::WRL::ComPtr;

struct Signal {
    UINT64 pixel_rate;
    UINT width, height, total_width, total_height;
    UINT vertical_numerator, vertical_denominator;
    UINT horizontal_numerator, horizontal_denominator;
};

struct Colour {
    UINT format, sdr_bits, hdr_bits;
};

struct Frame {
    INT64 acquired_at;
    UINT number;
};

using WriteMode = void (*)(void*, UINT, const Signal*, const Colour*);
static_assert(sizeof(Signal) == 40 && sizeof(Colour) == 12 && sizeof(Frame) == 16);

extern "C" {
NTSTATUS xscc_create_device_state(void** state);
void xscc_destroy_device_state(void* state);
NTSTATUS xscc_start_device(void* state, void* device);
NTSTATUS xscc_start_monitors(void* adapter);
void* xscc_create_monitor_state();
void xscc_destroy_monitor_state(void* state);
NTSTATUS xscc_advertise_modes(void* state, int target, UINT capacity, void* modes, UINT* count, WriteMode write);
NTSTATUS xscc_adapter_colour(Colour* colour);
NTSTATUS xscc_assign_swapchain(void* state, void* chain, HANDLE ready, INT64 luid);
NTSTATUS xscc_unassign_swapchain(void* state);
}

struct DisplayDeviceContext { void* state; };
struct DisplayMonitorContext { void* state; };
WDF_DECLARE_CONTEXT_TYPE(DisplayDeviceContext);
WDF_DECLARE_CONTEXT_TYPE(DisplayMonitorContext);

extern "C" DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD AddDevice;
EVT_WDF_DEVICE_D0_ENTRY EnterD0;
EVT_WDF_OBJECT_CONTEXT_CLEANUP DestroyDevice;
EVT_WDF_OBJECT_CONTEXT_CLEANUP DestroyMonitor;
EVT_IDD_CX_ADAPTER_INIT_FINISHED AdapterReady;
EVT_IDD_CX_ADAPTER_COMMIT_MODES CommitModes;
EVT_IDD_CX_ADAPTER_COMMIT_MODES2 CommitModes2;
EVT_IDD_CX_ADAPTER_QUERY_TARGET_INFO QueryTargetInfo;
EVT_IDD_CX_MONITOR_SET_DEFAULT_HDR_METADATA SetHdrMetadata;
EVT_IDD_CX_MONITOR_SET_GAMMA_RAMP SetGammaRamp;
EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION ParseDescription;
EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION2 ParseDescription2;
EVT_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES DefaultModes;
EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES QueryModes;
EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES2 QueryModes2;
EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN AssignSwapchain;
EVT_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN UnassignSwapchain;

_Use_decl_annotations_
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING registry) {
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, AddDevice);
    return WdfDriverCreate(driver, registry, WDF_NO_OBJECT_ATTRIBUTES, &config, WDF_NO_HANDLE);
}

_Use_decl_annotations_
NTSTATUS AddDevice(WDFDRIVER, PWDFDEVICE_INIT init) {
    WDF_PNPPOWER_EVENT_CALLBACKS power;
    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&power);
    power.EvtDeviceD0Entry = EnterD0;
    WdfDeviceInitSetPnpPowerEventCallbacks(init, &power);

    IDD_CX_CLIENT_CONFIG config;
    IDD_CX_CLIENT_CONFIG_INIT(&config);
    config.EvtIddCxAdapterInitFinished = AdapterReady;
    config.EvtIddCxMonitorGetDefaultDescriptionModes = DefaultModes;
    config.EvtIddCxMonitorAssignSwapChain = AssignSwapchain;
    config.EvtIddCxMonitorUnassignSwapChain = UnassignSwapchain;
    if (IDD_IS_FIELD_AVAILABLE(IDD_CX_CLIENT_CONFIG, EvtIddCxAdapterQueryTargetInfo)) {
        config.EvtIddCxAdapterQueryTargetInfo = QueryTargetInfo;
        config.EvtIddCxMonitorSetDefaultHdrMetaData = SetHdrMetadata;
        config.EvtIddCxParseMonitorDescription2 = ParseDescription2;
        config.EvtIddCxMonitorQueryTargetModes2 = QueryModes2;
        config.EvtIddCxAdapterCommitModes2 = CommitModes2;
        config.EvtIddCxMonitorSetGammaRamp = SetGammaRamp;
    } else {
        config.EvtIddCxParseMonitorDescription = ParseDescription;
        config.EvtIddCxMonitorQueryTargetModes = QueryModes;
        config.EvtIddCxAdapterCommitModes = CommitModes;
    }
    auto status = IddCxDeviceInitConfig(init, &config);
    if (!NT_SUCCESS(status)) return status;

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, DisplayDeviceContext);
    attributes.EvtCleanupCallback = DestroyDevice;
    WDFDEVICE device = nullptr;
    status = WdfDeviceCreate(&init, &attributes, &device);
    if (!NT_SUCCESS(status)) return status;
    status = xscc_create_device_state(&WdfObjectGet_DisplayDeviceContext(device)->state);
    if (!NT_SUCCESS(status)) return status;
    return IddCxDeviceInitialize(device);
}

_Use_decl_annotations_
NTSTATUS EnterD0(WDFDEVICE device, WDF_POWER_DEVICE_STATE) {
    return xscc_start_device(WdfObjectGet_DisplayDeviceContext(device)->state, device);
}

_Use_decl_annotations_
void DestroyDevice(WDFOBJECT device) {
    xscc_destroy_device_state(WdfObjectGet_DisplayDeviceContext(device)->state);
}

_Use_decl_annotations_
void DestroyMonitor(WDFOBJECT monitor) {
    xscc_destroy_monitor_state(WdfObjectGet_DisplayMonitorContext(monitor)->state);
}

extern "C" UINT xscc_configuration_path(WCHAR* buffer, UINT capacity) {
    DWORD bytes = capacity * static_cast<DWORD>(sizeof(WCHAR));
    const auto status = RegGetValueW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\MikeTheTech\\VirtualDisplayDriver",
                                     L"VDDPATH", RRF_RT_REG_SZ, nullptr, buffer, &bytes);
    if (status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND) return 0;
    if (status == ERROR_SUCCESS && bytes > sizeof(WCHAR) && bytes % sizeof(WCHAR) == 0) {
        // Reject a value with embedded nulls and trailing data.
        const auto length = bytes / sizeof(WCHAR) - 1;
        for (UINT i = 0; i < length; ++i) {
            if (buffer[i] == L'\0') {
                SetLastError(ERROR_INVALID_DATA);
                return UINT_MAX;
            }
        }
        return static_cast<UINT>(length);
    }
    SetLastError(status == ERROR_SUCCESS ? ERROR_INVALID_DATA : static_cast<DWORD>(status));
    return UINT_MAX;
}

extern "C" NTSTATUS xscc_initialize_adapter(void* device, UINT count) {
    IDDCX_ENDPOINT_VERSION version{};
    version.Size = sizeof(version);
    version.MajorVer = 1;

    IDDCX_ADAPTER_CAPS caps{};
    caps.Size = sizeof(caps);
    if (IDD_IS_FUNCTION_AVAILABLE(IddCxSwapChainReleaseAndAcquireBuffer2)) {
        caps.Flags = IDDCX_ADAPTER_FLAGS_CAN_PROCESS_FP16;
    }
    caps.MaxMonitorsSupported = count;
    auto& endpoint = caps.EndPointDiagnostics;
    endpoint.Size = sizeof(endpoint);
    endpoint.TransmissionType = IDDCX_TRANSMISSION_TYPE_WIRED_OTHER;
    endpoint.GammaSupport = IDDCX_FEATURE_IMPLEMENTATION_NONE;
    endpoint.pEndPointFriendlyName = L"VirtualDisplayDriver Device";
    endpoint.pEndPointManufacturerName = L"MikeTheTech";
    endpoint.pEndPointModelName = L"VirtualDisplayDriver Model";
    endpoint.pHardwareVersion = &version;
    endpoint.pFirmwareVersion = &version;

    IDARG_IN_ADAPTER_INIT input{};
    input.WdfDevice = static_cast<WDFDEVICE>(device);
    input.pCaps = &caps;
    IDARG_OUT_ADAPTER_INIT output{};
    return IddCxAdapterInitAsync(&input, &output);
}

_Use_decl_annotations_
NTSTATUS AdapterReady(IDDCX_ADAPTER adapter, const IDARG_IN_ADAPTER_INIT_FINISHED* input) {
    if (!NT_SUCCESS(input->AdapterInitStatus)) return input->AdapterInitStatus;
    return xscc_start_monitors(adapter);
}

extern "C" NTSTATUS xscc_arrive_monitor(void* adapter, UINT index, const BYTE* edid, UINT length) {
    IDDCX_MONITOR_INFO info{};
    info.Size = sizeof(info);
    info.MonitorType = DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI;
    info.ConnectorIndex = index;
    info.MonitorDescription.Size = sizeof(info.MonitorDescription);
    info.MonitorDescription.Type = IDDCX_MONITOR_DESCRIPTION_TYPE_EDID;
    info.MonitorDescription.DataSize = length;
    info.MonitorDescription.pData = const_cast<BYTE*>(edid);
    if (FAILED(CoCreateGuid(&info.MonitorContainerId))) return STATUS_UNSUCCESSFUL;

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, DisplayMonitorContext);
    attributes.EvtCleanupCallback = DestroyMonitor;
    IDARG_IN_MONITORCREATE input{};
    input.ObjectAttributes = &attributes;
    input.pMonitorInfo = &info;
    IDARG_OUT_MONITORCREATE monitor{};
    auto status = IddCxMonitorCreate(static_cast<IDDCX_ADAPTER>(adapter), &input, &monitor);
    if (!NT_SUCCESS(status)) return status;

    auto* context = WdfObjectGet_DisplayMonitorContext(monitor.MonitorObject);
    context->state = xscc_create_monitor_state();
    if (context->state == nullptr) {
        WdfObjectDelete(monitor.MonitorObject);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    IDARG_OUT_MONITORARRIVAL output{};
    status = IddCxMonitorArrival(monitor.MonitorObject, &output);
    if (!NT_SUCCESS(status)) WdfObjectDelete(monitor.MonitorObject);
    return status;
}

static DISPLAYCONFIG_VIDEO_SIGNAL_INFO WriteSignal(const Signal& signal, bool target) {
    DISPLAYCONFIG_VIDEO_SIGNAL_INFO output{};
    output.pixelRate = signal.pixel_rate;
    output.activeSize = {signal.width, signal.height};
    output.totalSize = {signal.total_width, signal.total_height};
    output.vSyncFreq = {signal.vertical_numerator, signal.vertical_denominator};
    output.hSyncFreq = {signal.horizontal_numerator, signal.horizontal_denominator};
    output.AdditionalSignalInfo.videoStandard = 255;
    output.AdditionalSignalInfo.vSyncFreqDivider = target ? 1U : 0U;
    output.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
    return output;
}

static void WriteColour(IDDCX_WIRE_BITS_PER_COMPONENT& output, const Colour& colour) {
    const auto sdr = colour.sdr_bits == 10U ? IDDCX_BITS_PER_COMPONENT_10 : IDDCX_BITS_PER_COMPONENT_8;
    const auto hdr = colour.hdr_bits == 12U ? IDDCX_BITS_PER_COMPONENT_12 : IDDCX_BITS_PER_COMPONENT_10;
    const auto bits = sdr | hdr;
    output = {};
    switch (colour.format) {
        case 1: output.YCbCr444 = bits; break;
        case 2: output.YCbCr422 = bits; break;
        case 3: output.YCbCr420 = bits; break;
        default: output.Rgb = bits; break;
    }
}

template<class Mode> static void WriteMonitorMode(void* output, UINT index, const Signal* signal, const Colour* colour) {
    auto& mode = static_cast<Mode*>(output)[index];
    mode = {};
    mode.Size = sizeof(mode);
    mode.Origin = IDDCX_MONITOR_MODE_ORIGIN_DRIVER;
    mode.MonitorVideoSignalInfo = WriteSignal(*signal, false);
    if constexpr (requires { mode.BitsPerComponent; }) WriteColour(mode.BitsPerComponent, *colour);
}

template<class Mode> static void WriteTargetMode(void* output, UINT index, const Signal* signal, const Colour* colour) {
    auto& mode = static_cast<Mode*>(output)[index];
    mode = {};
    mode.Size = sizeof(mode);
    mode.TargetVideoSignalInfo.targetVideoSignalInfo = WriteSignal(*signal, true);
    if constexpr (requires { mode.BitsPerComponent; }) WriteColour(mode.BitsPerComponent, *colour);
}

_Use_decl_annotations_
NTSTATUS ParseDescription(const IDARG_IN_PARSEMONITORDESCRIPTION* input, IDARG_OUT_PARSEMONITORDESCRIPTION* output) {
    output->PreferredMonitorModeIdx = 0;
    return xscc_advertise_modes(nullptr, 0, input->MonitorModeBufferInputCount, input->pMonitorModes,
                                &output->MonitorModeBufferOutputCount, WriteMonitorMode<IDDCX_MONITOR_MODE>);
}

_Use_decl_annotations_
NTSTATUS ParseDescription2(const IDARG_IN_PARSEMONITORDESCRIPTION2* input, IDARG_OUT_PARSEMONITORDESCRIPTION* output) {
    output->PreferredMonitorModeIdx = 0;
    return xscc_advertise_modes(nullptr, 0, input->MonitorModeBufferInputCount, input->pMonitorModes,
                                &output->MonitorModeBufferOutputCount, WriteMonitorMode<IDDCX_MONITOR_MODE2>);
}

_Use_decl_annotations_
NTSTATUS DefaultModes(IDDCX_MONITOR monitor, const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* input,
                      IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* output) {
    output->PreferredMonitorModeIdx = 0;
    return xscc_advertise_modes(WdfObjectGet_DisplayMonitorContext(monitor)->state, 0,
                                input->DefaultMonitorModeBufferInputCount, input->pDefaultMonitorModes,
                                &output->DefaultMonitorModeBufferOutputCount, WriteMonitorMode<IDDCX_MONITOR_MODE>);
}

_Use_decl_annotations_
NTSTATUS QueryModes(IDDCX_MONITOR monitor, const IDARG_IN_QUERYTARGETMODES* input, IDARG_OUT_QUERYTARGETMODES* output) {
    return xscc_advertise_modes(WdfObjectGet_DisplayMonitorContext(monitor)->state, 1,
                                input->TargetModeBufferInputCount, input->pTargetModes,
                                &output->TargetModeBufferOutputCount, WriteTargetMode<IDDCX_TARGET_MODE>);
}

_Use_decl_annotations_
NTSTATUS QueryModes2(IDDCX_MONITOR monitor, const IDARG_IN_QUERYTARGETMODES2* input, IDARG_OUT_QUERYTARGETMODES* output) {
    return xscc_advertise_modes(WdfObjectGet_DisplayMonitorContext(monitor)->state, 1,
                                input->TargetModeBufferInputCount, input->pTargetModes,
                                &output->TargetModeBufferOutputCount, WriteTargetMode<IDDCX_TARGET_MODE2>);
}

_Use_decl_annotations_
NTSTATUS QueryTargetInfo(IDDCX_ADAPTER, IDARG_IN_QUERYTARGET_INFO*, IDARG_OUT_QUERYTARGET_INFO* output) {
    Colour colour{};
    const auto status = xscc_adapter_colour(&colour);
    if (!NT_SUCCESS(status)) return status;
    output->TargetCaps = IDDCX_TARGET_CAPS_HIGH_COLOR_SPACE | IDDCX_TARGET_CAPS_WIDE_COLOR_SPACE;
    WriteColour(output->DitheringSupport, colour);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS CommitModes(IDDCX_ADAPTER, const IDARG_IN_COMMITMODES*) { return STATUS_SUCCESS; }

_Use_decl_annotations_
NTSTATUS CommitModes2(IDDCX_ADAPTER, const IDARG_IN_COMMITMODES2*) { return STATUS_SUCCESS; }

_Use_decl_annotations_
NTSTATUS SetHdrMetadata(IDDCX_MONITOR, const IDARG_IN_MONITOR_SET_DEFAULT_HDR_METADATA*) { return STATUS_SUCCESS; }

_Use_decl_annotations_
NTSTATUS SetGammaRamp(IDDCX_MONITOR, const IDARG_IN_SET_GAMMARAMP*) { return STATUS_SUCCESS; }

_Use_decl_annotations_
NTSTATUS AssignSwapchain(IDDCX_MONITOR monitor, const IDARG_IN_SETSWAPCHAIN* input) {
    INT64 luid = 0;
    static_assert(sizeof(luid) == sizeof(input->RenderAdapterLuid));
    memcpy(&luid, &input->RenderAdapterLuid, sizeof(luid));
    const auto status = xscc_assign_swapchain(WdfObjectGet_DisplayMonitorContext(monitor)->state,
                                              input->hSwapChain, input->hNextSurfaceAvailable, luid);
    return NT_SUCCESS(status) ? STATUS_SUCCESS : STATUS_GRAPHICS_INDIRECT_DISPLAY_ABANDON_SWAPCHAIN;
}

_Use_decl_annotations_
NTSTATUS UnassignSwapchain(IDDCX_MONITOR monitor) {
    return xscc_unassign_swapchain(WdfObjectGet_DisplayMonitorContext(monitor)->state);
}

static bool CanRender(LUID luid) {
    D3DKMT_OPENADAPTERFROMLUID opened{};
    opened.AdapterLuid = luid;
    if (!NT_SUCCESS(D3DKMTOpenAdapterFromLuid(&opened))) return false;

    D3DKMT_ADAPTERTYPE type{};
    D3DKMT_QUERYADAPTERINFO query{};
    query.hAdapter = opened.hAdapter;
    query.Type = KMTQAITYPE_ADAPTERTYPE;
    query.pPrivateDriverData = &type;
    query.PrivateDriverDataSize = sizeof(type);
    const auto status = D3DKMTQueryAdapterInfo(&query);

    D3DKMT_CLOSEADAPTER closed{};
    closed.hAdapter = opened.hAdapter;
    const auto closed_status = D3DKMTCloseAdapter(&closed);
    return NT_SUCCESS(status) && NT_SUCCESS(closed_status)
        && type.RenderSupported && !type.IndirectDisplayDevice && !type.SoftwareDevice;
}

extern "C" void xscc_prefer_render_adapter(void* adapter, const WCHAR* name) {
    if (!IDD_IS_FUNCTION_AVAILABLE(IddCxAdapterSetRenderAdapter)) return;
    ComPtr<IDXGIFactory1> factory;
    if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) return;

    SIZE_T most_memory = 0;
    bool found = false;
    IDARG_IN_ADAPTERSETRENDERADAPTER configuration{};
    for (UINT index = 0; ; ++index) {
        ComPtr<IDXGIAdapter1> candidate;
        if (FAILED(factory->EnumAdapters1(index, &candidate))) break;

        DXGI_ADAPTER_DESC1 description{};
        if (FAILED(candidate->GetDesc1(&description)) || !CanRender(description.AdapterLuid)) continue;
        if (*name && CompareStringOrdinal(name, -1, description.Description, -1, TRUE) == CSTR_EQUAL) {
            configuration.PreferredRenderAdapter = description.AdapterLuid;
            found = true;
            break;
        }
        if (!found || description.DedicatedVideoMemory > most_memory) {
            configuration.PreferredRenderAdapter = description.AdapterLuid;
            found = true;
            most_memory = description.DedicatedVideoMemory;
        }
    }
    if (found) IddCxAdapterSetRenderAdapter(static_cast<IDDCX_ADAPTER>(adapter), &configuration);
}

extern "C" NTSTATUS xscc_bind_swapchain(void* chain, INT64 value, void** owned_device) {
    LUID luid{};
    memcpy(&luid, &value, sizeof(luid));
    ComPtr<IDXGIFactory4> factory;
    auto status = CreateDXGIFactory1(IID_PPV_ARGS(&factory));
    if (FAILED(status)) return status;

    ComPtr<IDXGIAdapter1> adapter;
    status = factory->EnumAdapterByLuid(luid, IID_PPV_ARGS(&adapter));
    if (FAILED(status)) return status;

    ComPtr<ID3D11Device> d3d;
    status = D3D11CreateDevice(adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                               nullptr, 0, D3D11_SDK_VERSION, &d3d, nullptr, nullptr);
    if (FAILED(status)) return status;

    ComPtr<IDXGIDevice> device;
    status = d3d.As(&device);
    if (FAILED(status)) return status;

    IDARG_IN_SWAPCHAINSETDEVICE input{};
    input.pDevice = device.Get();
    status = IddCxSwapChainSetDevice(static_cast<IDDCX_SWAPCHAIN>(chain), &input);
    if (SUCCEEDED(status)) *owned_device = device.Detach();
    return status;
}

extern "C" void xscc_release_device(void* device) {
    static_cast<IDXGIDevice*>(device)->Release();
}

extern "C" NTSTATUS xscc_acquire_frame(void* chain, Frame* frame) {
    const auto swapchain = static_cast<IDDCX_SWAPCHAIN>(chain);
    ComPtr<IDXGIResource> surface;
    HRESULT status;
    if (IDD_IS_FUNCTION_AVAILABLE(IddCxSwapChainReleaseAndAcquireBuffer2)) {
        IDARG_IN_RELEASEANDACQUIREBUFFER2 input{};
        input.Size = sizeof(input);
        IDARG_OUT_RELEASEANDACQUIREBUFFER2 output{};
        status = IddCxSwapChainReleaseAndAcquireBuffer2(swapchain, &input, &output);
        surface.Attach(output.MetaData.pSurface);
        frame->number = output.MetaData.PresentationFrameNumber;
    } else {
        IDARG_OUT_RELEASEANDACQUIREBUFFER output{};
        status = IddCxSwapChainReleaseAndAcquireBuffer(swapchain, &output);
        surface.Attach(output.MetaData.pSurface);
        frame->number = output.MetaData.PresentationFrameNumber;
    }
    if (status != S_OK) return status;

    LARGE_INTEGER observed{};
    if (!QueryPerformanceCounter(&observed)) return STATUS_UNSUCCESSFUL;
    frame->acquired_at = observed.QuadPart;
    return STATUS_SUCCESS;
}

extern "C" NTSTATUS xscc_finish_frame(void* chain, const Frame* frame) {
    const auto swapchain = static_cast<IDDCX_SWAPCHAIN>(chain);
    const auto status = IddCxSwapChainFinishedProcessingFrame(swapchain);
    if (FAILED(status)) return status;

    // The virtual driver discards the surface. The Host captures it independently.
    IDARG_IN_REPORTFRAMESTATISTICS report{};
    report.FrameStatistics.Size = sizeof(report.FrameStatistics);
    report.FrameStatistics.PresentationFrameNumber = frame->number;
    report.FrameStatistics.FrameStatus = IDDCX_FRAME_STATUS_DROPPED;
    report.FrameStatistics.FrameSliceTotal = 1;
    report.FrameStatistics.FrameAcquireQpcTime = frame->acquired_at;
    return IddCxSwapChainReportFrameStatistics(swapchain, &report);
}

extern "C" void xscc_delete_swapchain(void* chain) {
    WdfObjectDelete(static_cast<IDDCX_SWAPCHAIN>(chain));
}
