// SPDX-License-Identifier: MIT
// IddCx setup patterns derived from MTT 25.7.23; see UPSTREAM.json.
#define WIN32_NO_STATUS
#include <windows.h>
#undef WIN32_NO_STATUS
#include <ntstatus.h>
#include "SwapChainSession.h"
#include <initguid.h>
#include <devpkey.h>
#include "ControlProtocol.h"
#include "ModeTiming.h"
#include <algorithm>
#include <atomic>
#include <map>
#include <mutex>
#include <new>
#include <vector>

namespace xscc {
using namespace protocol;
struct DeviceData;
struct FileLease {
    std::weak_ptr<DeviceData> device;
    std::atomic<bool> closing{false};
};
enum class MonitorPhase { Creating, Connected, Removing, Departed };
struct MonitorData {
    std::shared_ptr<DeviceData> owner;
    const std::shared_ptr<FileLease> lease;
    std::uint32_t connector;
    const std::vector<Mode> modes;
    IDDCX_MONITOR monitor = nullptr;
    std::atomic<MonitorPhase> phase{MonitorPhase::Creating};
    std::mutex sessionMutex;
    std::shared_ptr<SwapChainSession> current;
    MonitorData(std::shared_ptr<DeviceData> device, std::shared_ptr<FileLease> file,
                std::uint32_t id, std::vector<Mode> values)
        : owner(std::move(device)), lease(std::move(file)), connector(id), modes(std::move(values)) {}
};
struct DeviceData {
    WDFDEVICE device = nullptr;
    std::atomic<IDDCX_ADAPTER> adapter{nullptr};
    GUID identity{};
    std::atomic<bool> initializing{false};
    std::atomic<bool> ready{false};
    std::atomic<bool> closing{false};
    std::mutex monitorsMutex;
    std::map<std::uint32_t, std::shared_ptr<MonitorData>> monitors;
    std::mutex sessionsMutex;
    std::vector<std::shared_ptr<SwapChainSession>> sessions;
};
struct FileContext { std::shared_ptr<FileLease>* value; };
struct DeviceContext { std::shared_ptr<DeviceData>* value; };
struct AdapterContext { std::shared_ptr<DeviceData>* value; };
struct MonitorContext { std::shared_ptr<MonitorData>* value; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(FileContext, GetFileContext)
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(DeviceContext, GetDeviceContext)
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(AdapterContext, GetAdapterContext)
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(MonitorContext, GetMonitorContext)

EVT_WDF_DRIVER_DEVICE_ADD DeviceAdd;
EVT_WDF_DEVICE_FILE_CREATE FileCreate;
EVT_WDF_FILE_CLEANUP FileCleanup;
EVT_WDF_OBJECT_CONTEXT_CLEANUP FileContextCleanup;
EVT_WDF_DEVICE_D0_ENTRY DeviceD0Entry;
EVT_WDF_OBJECT_CONTEXT_CLEANUP DeviceCleanup;
EVT_WDF_OBJECT_CONTEXT_CLEANUP AdapterCleanup;
EVT_WDF_OBJECT_CONTEXT_CLEANUP MonitorCleanup;
EVT_IDD_CX_ADAPTER_INIT_FINISHED AdapterInitFinished;
EVT_IDD_CX_ADAPTER_COMMIT_MODES CommitModes;
EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION ParseMonitorDescription;
EVT_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES DefaultModes;
EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES TargetModes;
EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN AssignSwapChain;
EVT_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN UnassignSwapChain;
EVT_IDD_CX_DEVICE_IO_CONTROL DeviceIoControl;

static std::shared_ptr<MonitorData> Monitor(IDDCX_MONITOR object) noexcept {
    auto* wrapper = GetMonitorContext(object);
    if (!wrapper || !wrapper->value) Fatal(nullptr, STATUS_INVALID_DEVICE_STATE);
    return *wrapper->value;
}
static bool StopMonitor(const std::shared_ptr<MonitorData>& monitor) noexcept {
    std::shared_ptr<SwapChainSession> session;
    {
        std::lock_guard lock(monitor->sessionMutex);
        session = monitor->current;
    }
    if (!session) return true;
    session->StopAndWait();
    if (!session->Finished()) return false; // Same-worker deletion reentry retains ownership.
    std::lock_guard lock(monitor->sessionMutex);
    if (monitor->current == session) monitor->current.reset();
    return true;
}
static DISPLAYCONFIG_VIDEO_SIGNAL_INFO Signal(const Mode& mode, UINT divider) noexcept {
    ModeTiming timing{};
    if (BuildModeTiming(mode, timing) != TimingError::None)
        Fatal(nullptr, STATUS_INVALID_PARAMETER);
    DISPLAYCONFIG_VIDEO_SIGNAL_INFO signal{};
    signal.pixelRate = timing.pixelRate;
    signal.hSyncFreq = {timing.hSync.numerator, timing.hSync.denominator};
    signal.vSyncFreq = {timing.vSync.numerator, timing.vSync.denominator};
    signal.activeSize = {timing.activeWidth, timing.activeHeight};
    signal.totalSize = {timing.totalWidth, timing.totalHeight};
    signal.AdditionalSignalInfo.videoStandard = 255;
    signal.AdditionalSignalInfo.vSyncFreqDivider = divider;
    signal.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
    return signal;
}

_Use_decl_annotations_
NTSTATUS ParseMonitorDescription(const IDARG_IN_PARSEMONITORDESCRIPTION* input,
                                 IDARG_OUT_PARSEMONITORDESCRIPTION* output) {
    UNREFERENCED_PARAMETER(input);
    output->MonitorModeBufferOutputCount = 0;
    output->PreferredMonitorModeIdx = 0;
    // Arrivals omit EDID; DefaultModes supplies each monitor's own mode table.
    return STATUS_NOT_SUPPORTED;
}
_Use_decl_annotations_
NTSTATUS DefaultModes(IDDCX_MONITOR object, const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* input,
                      IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* output) {
    const auto monitor = Monitor(object);
    const auto count = static_cast<UINT>(monitor->modes.size());
    output->DefaultMonitorModeBufferOutputCount = count;
    output->PreferredMonitorModeIdx = 0;
    if (input->DefaultMonitorModeBufferInputCount == 0) return STATUS_SUCCESS;
    if (input->DefaultMonitorModeBufferInputCount < count) return STATUS_BUFFER_TOO_SMALL;
    for (UINT i = 0; i < count; ++i) {
        IDDCX_MONITOR_MODE value{};
        value.Size = sizeof(value);
        value.Origin = IDDCX_MONITOR_MODE_ORIGIN_DRIVER;
        value.MonitorVideoSignalInfo = Signal(monitor->modes[i], kMonitorVSyncDivider);
        input->pDefaultMonitorModes[i] = value;
    }
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS TargetModes(IDDCX_MONITOR object, const IDARG_IN_QUERYTARGETMODES* input,
                     IDARG_OUT_QUERYTARGETMODES* output) {
    const auto monitor = Monitor(object);
    const auto count = static_cast<UINT>(monitor->modes.size());
    output->TargetModeBufferOutputCount = count;
    if (input->TargetModeBufferInputCount == 0) return STATUS_SUCCESS;
    if (input->TargetModeBufferInputCount < count) return STATUS_BUFFER_TOO_SMALL;
    for (UINT i = 0; i < count; ++i) {
        IDDCX_TARGET_MODE value{};
        value.Size = sizeof(value);
        value.TargetVideoSignalInfo.targetVideoSignalInfo =
            Signal(monitor->modes[i], kTargetVSyncDivider);
        input->pTargetModes[i] = value;
    }
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS CommitModes(IDDCX_ADAPTER adapter, const IDARG_IN_COMMITMODES* input) {
    UNREFERENCED_PARAMETER(adapter);
    UNREFERENCED_PARAMETER(input);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS AssignSwapChain(IDDCX_MONITOR object, const IDARG_IN_SETSWAPCHAIN* input) {
    const auto monitor = Monitor(object);
    const auto device = monitor->owner;
    if (!StopMonitor(monitor)) Fatal(device->adapter, STATUS_INVALID_DEVICE_STATE);
    try {
        std::lock_guard monitorLock(monitor->sessionMutex);
        if (monitor->current && !monitor->current->Finished())
            Fatal(device->adapter, STATUS_INVALID_DEVICE_STATE);
        if (device->closing.load() || monitor->phase.load() == MonitorPhase::Removing ||
            monitor->phase.load() == MonitorPhase::Departed)
            return STATUS_GRAPHICS_INDIRECT_DISPLAY_ABANDON_SWAPCHAIN;
        // Reserve before starting: accepted workers must never face a later allocation failure.
        std::lock_guard registryLock(device->sessionsMutex);
        std::erase_if(device->sessions, [](const auto& value) { return value->Finished(); });
        device->sessions.reserve(device->sessions.size() + 1);
        auto session = SwapChainSession::Start(
            device->adapter, input->hSwapChain, input->RenderAdapterLuid, input->hNextSurfaceAvailable);
        device->sessions.push_back(session);
        monitor->current = std::move(session);
        return STATUS_SUCCESS;
    } catch (...) {
        Fatal(device->adapter, STATUS_INSUFFICIENT_RESOURCES);
    }
}
_Use_decl_annotations_
NTSTATUS UnassignSwapChain(IDDCX_MONITOR object) {
    StopMonitor(Monitor(object));
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
void MonitorCleanup(WDFOBJECT object) {
    auto* wrapper = GetMonitorContext(object);
    if (!wrapper || !wrapper->value) return;
    const auto monitor = *wrapper->value;
    {
        std::lock_guard lock(monitor->sessionMutex);
        if (monitor->current && !monitor->current->Finished())
            Fatal(nullptr, STATUS_INVALID_DEVICE_STATE);
        monitor->current.reset();
    }
    {
        std::lock_guard lock(monitor->owner->monitorsMutex);
        monitor->phase.store(MonitorPhase::Departed);
        auto found = monitor->owner->monitors.find(monitor->connector);
        if (found != monitor->owner->monitors.end() && found->second == monitor)
            monitor->owner->monitors.erase(found);
    }
    delete wrapper->value;
    wrapper->value = nullptr;
}
_Use_decl_annotations_
void AdapterCleanup(WDFOBJECT object) {
    auto* wrapper = GetAdapterContext(object);
    if (!wrapper || !wrapper->value) return;
    {
        const auto device = *wrapper->value;
        device->ready.store(false);
        device->closing.store(true);
        std::lock_guard lock(device->sessionsMutex);
        for (const auto& session : device->sessions)
            if (!session->Finished()) Fatal(nullptr, STATUS_INVALID_DEVICE_STATE);
        device->sessions.clear();
        {
            std::lock_guard monitorsLock(device->monitorsMutex);
            for (const auto& entry : device->monitors)
                entry.second->phase.store(MonitorPhase::Departed);
            device->monitors.clear();
        }
        device->adapter.store(nullptr);
        device->initializing.store(false);
    }
    delete wrapper->value;
    wrapper->value = nullptr;
}
_Use_decl_annotations_
void DeviceCleanup(WDFOBJECT object) {
    auto* wrapper = GetDeviceContext(object);
    if (!wrapper || !wrapper->value) return;
    const auto device = *wrapper->value;
    device->closing.store(true);
    device->ready.store(false);
    {
        std::lock_guard lock(device->sessionsMutex);
        for (const auto& session : device->sessions)
            if (!session->Finished()) Fatal(nullptr, STATUS_INVALID_DEVICE_STATE);
        device->sessions.clear();
    }
    {
        std::lock_guard lock(device->monitorsMutex);
        device->monitors.clear();
    }
    delete wrapper->value;
    wrapper->value = nullptr;
}

static NTSTATUS RemoveMonitor(const std::shared_ptr<DeviceData>& device,
                              const std::shared_ptr<FileLease>& lease, std::uint32_t connector);

static NTSTATUS AddMonitor(const std::shared_ptr<DeviceData>& device,
                           const std::shared_ptr<FileLease>& lease,
                           const RequestHeader& header, const void* bytes, size_t length,
                           std::uint32_t& connector) {
    std::vector<Mode> modes;
    modes.reserve(header.modeCount);
    for (std::uint32_t i = 0; i < header.modeCount; ++i) {
        Mode mode{};
        ModeTiming timing{};
        if (!ReadRequestMode(bytes, length, i, mode) ||
            BuildModeTiming(mode, timing) != TimingError::None)
            return STATUS_INVALID_PARAMETER;
        modes.push_back(mode);
    }
    connector = header.connectorId;
    std::shared_ptr<MonitorData> monitor;
    std::unique_ptr<std::shared_ptr<MonitorData>> holder;
    {
        std::lock_guard lock(device->monitorsMutex);
        if (lease->closing.load()) return STATUS_FILE_CLOSED;
        if (connector == kNoConnector) {
            for (connector = 0; connector < kConnectorCapacity; ++connector)
                if (!device->monitors.contains(connector)) break;
        }
        if (connector >= kConnectorCapacity) return STATUS_INSUFFICIENT_RESOURCES;
        if (device->monitors.contains(connector)) return STATUS_OBJECT_NAME_COLLISION;
        monitor = std::make_shared<MonitorData>(device, lease, connector, std::move(modes));
        holder = std::make_unique<std::shared_ptr<MonitorData>>(monitor);
        device->monitors.emplace(connector, monitor);
    }
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, MonitorContext);
    attributes.EvtCleanupCallback = MonitorCleanup;
    IDDCX_MONITOR_INFO info{};
    info.Size = sizeof(info);
    info.MonitorType = DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED;
    info.ConnectorIndex = connector;
    info.MonitorDescription.Size = sizeof(info.MonitorDescription);
    info.MonitorDescription.Type = IDDCX_MONITOR_DESCRIPTION_TYPE_EDID;
    // No EDID: DefaultModes reports exactly this monitor's validated mode table.
    info.MonitorContainerId = device->identity;
    info.MonitorContainerId.Data1 ^= connector;
    IDARG_IN_MONITORCREATE create{};
    create.ObjectAttributes = &attributes;
    create.pMonitorInfo = &info;
    IDARG_OUT_MONITORCREATE created{};
    auto status = IddCxMonitorCreate(device->adapter, &create, &created);
    if (NT_SUCCESS(status)) {
        monitor->monitor = created.MonitorObject;
        GetMonitorContext(created.MonitorObject)->value = holder.release();
        IDARG_OUT_MONITORARRIVAL arrival{};
        status = lease->closing.load() ? STATUS_FILE_CLOSED :
            IddCxMonitorArrival(created.MonitorObject, &arrival);
        if (NT_SUCCESS(status)) {
            {
                std::lock_guard lock(device->monitorsMutex);
                if (monitor->phase.load() != MonitorPhase::Creating)
                    return STATUS_DEVICE_REMOVED;
                monitor->phase.store(MonitorPhase::Connected);
            }
            if (!lease->closing.load()) return STATUS_SUCCESS;
            const auto removed = RemoveMonitor(device, lease, connector);
            if (!NT_SUCCESS(removed) && removed != STATUS_NOT_FOUND && removed != STATUS_DEVICE_BUSY &&
                removed != STATUS_ACCESS_DENIED)
                Fatal(device->adapter, removed);
            return STATUS_FILE_CLOSED;
        }
        if (!StopMonitor(monitor)) Fatal(device->adapter, STATUS_INVALID_DEVICE_STATE);
        WdfObjectDelete(created.MonitorObject);
    }
    std::lock_guard lock(device->monitorsMutex);
    const auto found = device->monitors.find(connector);
    if (found != device->monitors.end() && found->second == monitor)
        device->monitors.erase(found);
    return status;
}
static NTSTATUS RemoveMonitor(const std::shared_ptr<DeviceData>& device,
                              const std::shared_ptr<FileLease>& lease, std::uint32_t connector) {
    std::shared_ptr<MonitorData> monitor;
    {
        std::lock_guard lock(device->monitorsMutex);
        const auto found = device->monitors.find(connector);
        if (found == device->monitors.end()) return STATUS_NOT_FOUND;
        monitor = found->second;
        if (monitor->lease != lease) return STATUS_ACCESS_DENIED;
        if (monitor->phase.load() != MonitorPhase::Connected) return STATUS_DEVICE_BUSY;
        monitor->phase.store(MonitorPhase::Removing);
    }
    // No map/session/registry lock spans frame retirement or monitor departure.
    if (!StopMonitor(monitor)) Fatal(device->adapter, STATUS_INVALID_DEVICE_STATE);
    const auto status = IddCxMonitorDeparture(monitor->monitor);
    {
        std::lock_guard lock(device->monitorsMutex);
        if (NT_SUCCESS(status)) {
            monitor->phase.store(MonitorPhase::Departed);
            const auto found = device->monitors.find(connector);
            if (found != device->monitors.end() && found->second == monitor)
                device->monitors.erase(found);
        } else if (monitor->phase.load() == MonitorPhase::Removing) {
            monitor->phase.store(MonitorPhase::Connected);
        }
    }
    if (!NT_SUCCESS(status) && lease->closing.load()) Fatal(device->adapter, status);
    return status;
}

static NTSTATUS HandleControl(const std::shared_ptr<DeviceData>& device, WDFREQUEST request,
                              size_t outputLength, size_t inputLength, ULONG code,
                              size_t& written) {
    const auto file = WdfRequestGetFileObject(request);
    auto* fileContext = file ? GetFileContext(file) : nullptr;
    if (!fileContext || !fileContext->value) return STATUS_ACCESS_DENIED;
    const auto lease = *fileContext->value;
    if (lease->closing.load()) return STATUS_FILE_CLOSED;
    void* input = nullptr;
    auto status = WdfRequestRetrieveInputBuffer(request, sizeof(RequestHeader), &input, nullptr);
    if (!NT_SUCCESS(status)) return status;
    RequestHeader header{};
    if (!ReadRequestHeader(input, inputLength, header)) return STATUS_INVALID_PARAMETER;
    const bool add = code == kIoctlAdd;
    const bool list = code == kIoctlList || code == kIoctlOwnedList;
    if ((add && (header.modeCount == 0 ||
                 (header.connectorId >= kConnectorCapacity && header.connectorId != kNoConnector))) ||
        (!add && header.modeCount != 0))
        return STATUS_INVALID_PARAMETER;
    if ((code == kIoctlQuery || list) && header.connectorId != kNoConnector)
        return STATUS_INVALID_PARAMETER;
    if (code == kIoctlRemove && header.connectorId >= kConnectorCapacity)
        return STATUS_INVALID_PARAMETER;
    if (code != kIoctlQuery && !list && code != kIoctlAdd && code != kIoctlRemove)
        return STATUS_INVALID_DEVICE_REQUEST;
    if (!device->ready.load() || device->closing.load()) return STATUS_DEVICE_NOT_READY;

    const auto needed = (code == kIoctlQuery) ? sizeof(QueryResponse) :
                        list ? sizeof(ListResponse) : sizeof(OperationResponse);
    void* output = nullptr;
    status = WdfRequestRetrieveOutputBuffer(request, needed, &output, nullptr);
    if (!NT_SUCCESS(status)) return status;
    if (code == kIoctlQuery) {
        std::lock_guard lock(device->monitorsMutex);
        const auto count = std::count_if(device->monitors.begin(), device->monitors.end(),
            [](const auto& entry) { return entry.second->phase.load() == MonitorPhase::Connected; });
        const QueryResponse value{kVersion, sizeof(QueryResponse), kConnectorCapacity,
            static_cast<std::uint32_t>(count), kMaxModesPerMonitor, kGpuMonitorLimitUnknown};
        std::memcpy(output, &value, sizeof(value));
        written = sizeof(value);
        return STATUS_SUCCESS;
    }
    if (list) {
        std::lock_guard lock(device->monitorsMutex);
        const auto visible = [&](const auto& monitor) {
            return monitor->phase.load() == MonitorPhase::Connected &&
                   (code != kIoctlOwnedList || monitor->lease == lease);
        };
        const auto count = static_cast<std::uint32_t>(std::count_if(
            device->monitors.begin(), device->monitors.end(),
            [&](const auto& entry) { return visible(entry.second); }));
        std::uint32_t total{};
        if (!ListSizeForMonitors(count, total)) return STATUS_INVALID_PARAMETER;
        if (outputLength < total) return STATUS_BUFFER_TOO_SMALL;
        const ListResponse value{kVersion, total, count, sizeof(MonitorRecord)};
        std::memcpy(output, &value, sizeof(value));
        auto* position = static_cast<std::uint8_t*>(output) + sizeof(value);
        for (const auto& [id, monitor] : device->monitors) {
            if (!visible(monitor)) continue;
            const MonitorRecord record{id, static_cast<std::uint32_t>(monitor->modes.size()),
                                       monitor->modes.front()};
            std::memcpy(position, &record, sizeof(record));
            position += sizeof(record);
        }
        written = total;
        return STATUS_SUCCESS;
    }
    std::uint32_t connector = header.connectorId;
    status = add ? AddMonitor(device, lease, header, input, inputLength, connector) :
                   RemoveMonitor(device, lease, connector);
    if (!NT_SUCCESS(status)) return status;
    const OperationResponse value{kVersion, sizeof(OperationResponse), connector};
    std::memcpy(output, &value, sizeof(value));
    written = sizeof(value);
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
void DeviceIoControl(WDFDEVICE object, WDFREQUEST request, size_t outputLength,
                     size_t inputLength, ULONG code) {
    auto* wrapper = GetDeviceContext(object);
    if (!wrapper || !wrapper->value) {
        WdfRequestComplete(request, STATUS_DEVICE_NOT_READY);
        return;
    }
    size_t written = 0;
    NTSTATUS status;
    try {
        status = HandleControl(*wrapper->value, request, outputLength, inputLength, code, written);
    } catch (const std::bad_alloc&) {
        status = STATUS_INSUFFICIENT_RESOURCES;
    } catch (...) {
        Fatal(nullptr, STATUS_UNSUCCESSFUL);
    }
    WdfRequestCompleteWithInformation(request, status, written);
}

_Use_decl_annotations_
void FileCreate(WDFDEVICE object, WDFREQUEST request, WDFFILEOBJECT file) {
    try {
        auto lease = std::make_shared<FileLease>();
        lease->device = *GetDeviceContext(object)->value;
        auto holder = std::make_unique<std::shared_ptr<FileLease>>(std::move(lease));
        GetFileContext(file)->value = holder.release();
        WdfRequestComplete(request, STATUS_SUCCESS);
    } catch (...) {
        WdfRequestComplete(request, STATUS_INSUFFICIENT_RESOURCES);
    }
}
_Use_decl_annotations_
void FileCleanup(WDFFILEOBJECT file) {
    auto* context = GetFileContext(file);
    if (!context || !context->value) return;
    const auto lease = *context->value;
    lease->closing.store(true);
    const auto device = lease->device.lock();
    if (!device || device->closing.load()) return;
    for (;;) {
        std::uint32_t connector = kNoConnector;
        {
            std::lock_guard lock(device->monitorsMutex);
            for (const auto& [id, monitor] : device->monitors) {
                if (monitor->lease == lease && monitor->phase.load() == MonitorPhase::Connected) {
                    connector = id;
                    break;
                }
            }
        }
        if (connector == kNoConnector) return;
        const auto status = RemoveMonitor(device, lease, connector);
        if (!NT_SUCCESS(status) && status != STATUS_NOT_FOUND && status != STATUS_DEVICE_BUSY &&
            status != STATUS_ACCESS_DENIED)
            Fatal(device->adapter, status);
    }
}
_Use_decl_annotations_
void FileContextCleanup(WDFOBJECT object) {
    auto* context = GetFileContext(object);
    delete context->value;
    context->value = nullptr;
}

_Use_decl_annotations_
NTSTATUS AdapterInitFinished(IDDCX_ADAPTER object, const IDARG_IN_ADAPTER_INIT_FINISHED* input) {
    auto* wrapper = GetAdapterContext(object);
    if (!wrapper || !wrapper->value) Fatal(nullptr, STATUS_INVALID_DEVICE_STATE);
    if (!NT_SUCCESS(input->AdapterInitStatus)) Fatal(nullptr, input->AdapterInitStatus);
    (*wrapper->value)->ready.store(true);
    // Zero monitors is intentional; Add is the only arrival path.
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS DeviceD0Entry(WDFDEVICE object, WDF_POWER_DEVICE_STATE previous) {
    UNREFERENCED_PARAMETER(previous);
    const auto device = *GetDeviceContext(object)->value;
    if (device->initializing.exchange(true)) return STATUS_SUCCESS;
    device->closing.store(false);
    try {
        auto holder = std::make_unique<std::shared_ptr<DeviceData>>(device);
        WDF_OBJECT_ATTRIBUTES attributes;
        WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, AdapterContext);
        attributes.EvtCleanupCallback = AdapterCleanup;
        IDDCX_ENDPOINT_VERSION version{};
        version.Size = sizeof(version);
        version.MajorVer = 1;
        IDDCX_ADAPTER_CAPS caps{};
        caps.Size = sizeof(caps);
        caps.MaxMonitorsSupported = kConnectorCapacity;
        caps.EndPointDiagnostics.Size = sizeof(caps.EndPointDiagnostics);
        caps.EndPointDiagnostics.GammaSupport = IDDCX_FEATURE_IMPLEMENTATION_NONE;
        caps.EndPointDiagnostics.TransmissionType = IDDCX_TRANSMISSION_TYPE_WIRED_OTHER;
        caps.EndPointDiagnostics.pEndPointFriendlyName = L"XSCC Virtual Display";
        caps.EndPointDiagnostics.pEndPointManufacturerName = L"XSCC";
        caps.EndPointDiagnostics.pEndPointModelName = L"XSCC IddCx";
        caps.EndPointDiagnostics.pFirmwareVersion = &version;
        caps.EndPointDiagnostics.pHardwareVersion = &version;
        IDARG_IN_ADAPTER_INIT initialize{};
        initialize.WdfDevice = object;
        initialize.pCaps = &caps;
        initialize.ObjectAttributes = &attributes;
        IDARG_OUT_ADAPTER_INIT initialized{};
        const auto status = IddCxAdapterInitAsync(&initialize, &initialized);
        if (!NT_SUCCESS(status)) return status;
        device->adapter = initialized.AdapterObject;
        GetAdapterContext(initialized.AdapterObject)->value = holder.release();
        return STATUS_SUCCESS;
    } catch (...) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
}
static NTSTATUS LoadIdentity(WDFDEVICE object, GUID& identity) {
    WDF_DEVICE_PROPERTY_DATA property;
    WDF_DEVICE_PROPERTY_DATA_INIT(&property, &DEVPKEY_Device_InstanceId);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ParentObject = object;
    WDFMEMORY memory = nullptr;
    DEVPROPTYPE type = 0;
    const auto status = WdfDeviceAllocAndQueryPropertyEx(
        object, &property, PagedPool, &attributes, &memory, &type);
    if (!NT_SUCCESS(status)) return status;
    size_t length = 0;
    const auto* bytes = static_cast<const std::uint8_t*>(WdfMemoryGetBuffer(memory, &length));
    if (type != DEVPROP_TYPE_STRING || length < sizeof(wchar_t) || length > 8192) {
        WdfObjectDelete(memory);
        return STATUS_INVALID_PARAMETER;
    }
    std::uint64_t first = 14695981039346656037ull, second = 1099511628211ull;
    for (size_t i = 0; i < length; ++i) {
        first = (first ^ bytes[i]) * 1099511628211ull;
        second = (second ^ bytes[i]) * 14695981039346656037ull;
    }
    std::memcpy(&identity, &first, sizeof(first));
    std::memcpy(reinterpret_cast<std::uint8_t*>(&identity) + sizeof(first), &second, sizeof(second));
    WdfObjectDelete(memory);
    return STATUS_SUCCESS;
}
_Use_decl_annotations_
NTSTATUS DeviceAdd(WDFDRIVER driver, PWDFDEVICE_INIT initialize) {
    UNREFERENCED_PARAMETER(driver);
    try {
        auto data = std::make_shared<DeviceData>();
        auto holder = std::make_unique<std::shared_ptr<DeviceData>>(data);
        WDF_PNPPOWER_EVENT_CALLBACKS power;
        WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&power);
        power.EvtDeviceD0Entry = DeviceD0Entry;
        WdfDeviceInitSetPnpPowerEventCallbacks(initialize, &power);
        WDF_FILEOBJECT_CONFIG files;
        WDF_FILEOBJECT_CONFIG_INIT(&files, FileCreate, nullptr, FileCleanup);
        WDF_OBJECT_ATTRIBUTES fileAttributes;
        WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&fileAttributes, FileContext);
        fileAttributes.EvtCleanupCallback = FileContextCleanup;
        WdfDeviceInitSetFileObjectConfig(initialize, &files, &fileAttributes);
        IDD_CX_CLIENT_CONFIG config;
        IDD_CX_CLIENT_CONFIG_INIT(&config);
        config.EvtIddCxAdapterInitFinished = AdapterInitFinished;
        config.EvtIddCxAdapterCommitModes = CommitModes;
        config.EvtIddCxParseMonitorDescription = ParseMonitorDescription;
        config.EvtIddCxMonitorGetDefaultDescriptionModes = DefaultModes;
        config.EvtIddCxMonitorQueryTargetModes = TargetModes;
        config.EvtIddCxMonitorAssignSwapChain = AssignSwapChain;
        config.EvtIddCxMonitorUnassignSwapChain = UnassignSwapChain;
        config.EvtIddCxDeviceIoControl = DeviceIoControl;
        auto status = IddCxDeviceInitConfig(initialize, &config);
        if (!NT_SUCCESS(status)) return status;
        WDF_OBJECT_ATTRIBUTES attributes;
        WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, DeviceContext);
        attributes.EvtCleanupCallback = DeviceCleanup;
        WDFDEVICE object = nullptr;
        status = WdfDeviceCreate(&initialize, &attributes, &object);
        if (!NT_SUCCESS(status)) return status;
        GetDeviceContext(object)->value = holder.release();
        data->device = object;
        status = LoadIdentity(object, data->identity);
        if (!NT_SUCCESS(status)) return status;
        status = IddCxDeviceInitialize(object);
        if (!NT_SUCCESS(status)) return status;
        return WdfDeviceCreateDeviceInterface(object, &kControlInterfaceGuid, nullptr);
    } catch (...) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
}
} // namespace xscc

extern "C" DRIVER_INITIALIZE DriverEntry;
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT object, PUNICODE_STRING path) {
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, xscc::DeviceAdd);
    return WdfDriverCreate(object, path, WDF_NO_OBJECT_ATTRIBUTES, &config, WDF_NO_HANDLE);
}
