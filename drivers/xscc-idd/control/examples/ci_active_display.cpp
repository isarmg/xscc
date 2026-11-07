// SPDX-License-Identifier: MIT
// CI-only: never installed or called by the product. Native calls have no wall-clock guarantee.
#define NOMINMAX
#include <windows.h>
#include <setupapi.h>
#include <d3d11.h>
#include <dxgi.h>
#include <wrl/client.h>
#include <algorithm>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <set>
#include <stdexcept>
#include <string>
#include <system_error>
#include <vector>

using Microsoft::WRL::ComPtr;
using Clock = std::chrono::steady_clock;
constexpr UINT QueryFlags = QDC_VIRTUAL_REFRESH_RATE_AWARE;
constexpr UINT ApplyFlags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_VIRTUAL_REFRESH_RATE_AWARE;
struct Topology { std::vector<DISPLAYCONFIG_PATH_INFO> paths; std::vector<DISPLAYCONFIG_MODE_INFO> modes; };

void Check(LONG result, const char* operation) {
    if (result != ERROR_SUCCESS) throw std::system_error(result, std::system_category(), operation);
}
void CheckHr(HRESULT result, const char* operation) {
    if (FAILED(result)) throw std::system_error(static_cast<int>(result), std::system_category(), operation);
}
std::wstring Env(const wchar_t* key) {
    wchar_t value[2048]{};
    const DWORD length = GetEnvironmentVariableW(key, value, ARRAYSIZE(value));
    if (!length || length >= ARRAYSIZE(value)) throw std::runtime_error("Missing/overlong CI environment");
    return value;
}
void Guard(const std::wstring& id) {
    if (Env(L"GITHUB_ACTIONS") != L"true" || Env(L"RUNNER_ENVIRONMENT") != L"github-hosted" ||
        Env(L"RUNNER_OS") != L"Windows" || Env(L"XSCC_IDD_CI_RUNTIME_SMOKE") != L"1")
        throw std::runtime_error("Active display probe is restricted to disposable GitHub Windows CI");
    const auto event = Env(L"GITHUB_EVENT_NAME");
    if (event != L"workflow_dispatch" && !(event == L"push" && Env(L"GITHUB_REF") == L"refs/heads/ci/owned-idd-runtime-20261107"))
        throw std::runtime_error("Not the explicit CI trigger");
    const auto run = Env(L"GITHUB_RUN_ID"), attempt = Env(L"GITHUB_RUN_ATTEMPT");
    if (run.find_first_not_of(L"0123456789") != std::wstring::npos || attempt.find_first_not_of(L"0123456789") != std::wstring::npos ||
        id != L"ROOT\\XSCCIDD\\CI_" + run + L"_" + attempt) throw std::runtime_error("Not this job's exact root instance");
}
Topology Query(UINT kind) {
    for (int retry = 0; retry < 4; ++retry) {
        UINT paths = 0, modes = 0;
        Check(GetDisplayConfigBufferSizes(kind | QueryFlags, &paths, &modes), "GetDisplayConfigBufferSizes");
        if (paths > 1024 || modes > 4096) throw std::runtime_error("CCD inventory exceeds CI budget");
        Topology result{std::vector<DISPLAYCONFIG_PATH_INFO>(paths), std::vector<DISPLAYCONFIG_MODE_INFO>(modes)};
        const LONG status = QueryDisplayConfig(kind | QueryFlags, &paths, result.paths.data(), &modes, result.modes.data(), nullptr);
        if (status == ERROR_INSUFFICIENT_BUFFER) continue;
        Check(status, "QueryDisplayConfig");
        result.paths.resize(paths); result.modes.resize(modes);
        return result;
    }
    throw std::runtime_error("CCD inventory did not settle");
}
bool Owned(const DISPLAYCONFIG_PATH_INFO& path, const std::wstring& expected) {
    DISPLAYCONFIG_ADAPTER_NAME name{};
    name.header = {DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME, sizeof(name), path.targetInfo.adapterId, 0};
    Check(DisplayConfigGetDeviceInfo(&name.header), "CCD adapter interface");
    const HDEVINFO set = SetupDiCreateDeviceInfoList(nullptr, nullptr);
    if (set == INVALID_HANDLE_VALUE) Check(static_cast<LONG>(GetLastError()), "SetupDiCreateDeviceInfoList");
    struct Close { HDEVINFO value; ~Close() { SetupDiDestroyDeviceInfoList(value); } } close{set};
    SP_DEVICE_INTERFACE_DATA interfaceData{}; interfaceData.cbSize = sizeof(interfaceData);
    if (!SetupDiOpenDeviceInterfaceW(set, name.adapterDevicePath, 0, &interfaceData)) Check(static_cast<LONG>(GetLastError()), "SetupDiOpenDeviceInterfaceW");
    DWORD size = 0;
    SetupDiGetDeviceInterfaceDetailW(set, &interfaceData, nullptr, 0, &size, nullptr);
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || size < sizeof(SP_DEVICE_INTERFACE_DETAIL_DATA_W) || size > 16384)
        throw std::runtime_error("Invalid adapter interface detail size");
    std::vector<BYTE> bytes(size);
    auto* detail = reinterpret_cast<SP_DEVICE_INTERFACE_DETAIL_DATA_W*>(bytes.data()); detail->cbSize = sizeof(*detail);
    SP_DEVINFO_DATA info{}; info.cbSize = sizeof(info);
    if (!SetupDiGetDeviceInterfaceDetailW(set, &interfaceData, detail, size, nullptr, &info)) Check(static_cast<LONG>(GetLastError()), "SetupDiGetDeviceInterfaceDetailW");
    wchar_t id[512]{};
    if (!SetupDiGetDeviceInstanceIdW(set, &info, id, ARRAYSIZE(id), nullptr)) Check(static_cast<LONG>(GetLastError()), "SetupDiGetDeviceInstanceIdW");
    if (_wcsicmp(id, expected.c_str()) != 0) return false;
    wchar_t hardware[512]{}; DWORD type = 0;
    if (!SetupDiGetDeviceRegistryPropertyW(set, &info, SPDRP_HARDWAREID, &type, reinterpret_cast<PBYTE>(hardware), sizeof(hardware), &size))
        Check(static_cast<LONG>(GetLastError()), "Read exact adapter hardware identity");
    if (type != REG_MULTI_SZ || size > sizeof(hardware) || size < 2 * sizeof(wchar_t) || size % sizeof(wchar_t) ||
        hardware[size / sizeof(wchar_t) - 1] != 0 || hardware[size / sizeof(wchar_t) - 2] != 0)
        throw std::runtime_error("Malformed adapter hardware IDs");
    for (const wchar_t* item = hardware; *item; item += wcslen(item) + 1)
        if (_wcsicmp(item, L"Root\\XsccIdd") == 0) return true;
    throw std::runtime_error("Exact root does not have Root\\XsccIdd hardware ID");
}
void Apply(Topology value) {
    Check(SetDisplayConfig(static_cast<UINT>(value.paths.size()), value.paths.data(), static_cast<UINT>(value.modes.size()), value.modes.data(), ApplyFlags | SDC_VALIDATE), "Validate supplied CCD layout");
    Check(SetDisplayConfig(static_cast<UINT>(value.paths.size()), value.paths.data(), static_cast<UINT>(value.modes.size()), value.modes.data(), ApplyFlags | SDC_APPLY), "Apply supplied CCD layout");
}
void Restore(const Topology& original) {
    Apply(original);
    const auto actual = Query(QDC_ONLY_ACTIVE_PATHS);
    if (actual.paths.size() != original.paths.size()) throw std::runtime_error("Restored active path count differs");
    for (const auto& expected : original.paths) {
        const auto path = std::find_if(actual.paths.begin(), actual.paths.end(), [&](const auto& candidate) {
            return candidate.targetInfo.adapterId.HighPart == expected.targetInfo.adapterId.HighPart &&
                candidate.targetInfo.adapterId.LowPart == expected.targetInfo.adapterId.LowPart && candidate.targetInfo.id == expected.targetInfo.id;
        });
        if (path == actual.paths.end() || path->sourceInfo.modeInfoIdx >= actual.modes.size() || path->targetInfo.modeInfoIdx >= actual.modes.size() ||
            expected.sourceInfo.modeInfoIdx >= original.modes.size() || expected.targetInfo.modeInfoIdx >= original.modes.size()) throw std::runtime_error("Restored CCD path/modes missing");
        const auto& a = actual.modes[path->sourceInfo.modeInfoIdx].sourceMode;
        const auto& b = original.modes[expected.sourceInfo.modeInfoIdx].sourceMode;
        const auto& x = actual.modes[path->targetInfo.modeInfoIdx].targetMode.targetVideoSignalInfo;
        const auto& y = original.modes[expected.targetInfo.modeInfoIdx].targetMode.targetVideoSignalInfo;
        if (a.width != b.width || a.height != b.height || a.position.x != b.position.x || a.position.y != b.position.y ||
            x.activeSize.cx != y.activeSize.cx || x.activeSize.cy != y.activeSize.cy || !x.vSyncFreq.Denominator || !y.vSyncFreq.Denominator ||
            static_cast<UINT64>(x.vSyncFreq.Numerator) * y.vSyncFreq.Denominator != static_cast<UINT64>(y.vSyncFreq.Numerator) * x.vSyncFreq.Denominator)
            throw std::runtime_error("Original CCD dimensions/position/rate were not restored");
    }
}
void Save(const Topology& value, const std::filesystem::path& file) {
    if (std::filesystem::exists(file)) throw std::runtime_error("Fresh baseline file required");
    const UINT header[] = {0x58534343, static_cast<UINT>(value.paths.size()), static_cast<UINT>(value.modes.size())};
    std::ofstream stream(file, std::ios::binary | std::ios::out);
    stream.write(reinterpret_cast<const char*>(header), sizeof(header));
    stream.write(reinterpret_cast<const char*>(value.paths.data()), static_cast<std::streamsize>(value.paths.size() * sizeof(DISPLAYCONFIG_PATH_INFO)));
    stream.write(reinterpret_cast<const char*>(value.modes.data()), static_cast<std::streamsize>(value.modes.size() * sizeof(DISPLAYCONFIG_MODE_INFO)));
    stream.flush(); if (!stream) throw std::runtime_error("Failed to save baseline layout");
}
Topology Load(const std::filesystem::path& file, const std::wstring& id) {
    std::ifstream stream(file, std::ios::binary);
    UINT header[3]{}; stream.read(reinterpret_cast<char*>(header), sizeof(header));
    if (!stream || header[0] != 0x58534343 || !header[1] || header[1] > 1024 || header[2] > 4096)
        throw std::runtime_error("Invalid baseline header");
    Topology value{std::vector<DISPLAYCONFIG_PATH_INFO>(header[1]), std::vector<DISPLAYCONFIG_MODE_INFO>(header[2])};
    stream.read(reinterpret_cast<char*>(value.paths.data()), static_cast<std::streamsize>(value.paths.size() * sizeof(DISPLAYCONFIG_PATH_INFO)));
    stream.read(reinterpret_cast<char*>(value.modes.data()), static_cast<std::streamsize>(value.modes.size() * sizeof(DISPLAYCONFIG_MODE_INFO)));
    if (!stream || stream.peek() != std::char_traits<char>::eof()) throw std::runtime_error("Truncated or overlong baseline layout");
    for (const auto& path : value.paths) if (Owned(path, id)) throw std::runtime_error("Baseline unexpectedly includes the owned virtual target");
    return value;
}
DISPLAYCONFIG_PATH_INFO UniqueTarget(const Topology& all, const std::wstring& id) {
    std::set<std::pair<UINT64, UINT>> targets;
    DISPLAYCONFIG_PATH_INFO result{};
    for (const auto& path : all.paths) {
        if (path.targetInfo.targetAvailable && Owned(path, id)) {
            const auto luid = (static_cast<UINT64>(static_cast<UINT32>(path.targetInfo.adapterId.HighPart)) << 32) | path.targetInfo.adapterId.LowPart;
            targets.emplace(luid, path.targetInfo.id); result = path;
        }
    }
    if (targets.size() != 1) throw std::runtime_error("Expected exactly one available target on this job's adapter");
    return result;
}
bool Active(const Topology& value, const std::wstring& id, DISPLAYCONFIG_SOURCE_MODE* source = nullptr) {
    bool found = false;
    for (const auto& path : value.paths) {
        if (!Owned(path, id)) continue;
        if (found) throw std::runtime_error("Multiple active targets on the CI adapter");
        found = true;
        if (path.sourceInfo.modeInfoIdx >= value.modes.size() || path.targetInfo.modeInfoIdx >= value.modes.size())
            throw std::runtime_error("Missing actual CCD mode indexes");
        const auto& src = value.modes[path.sourceInfo.modeInfoIdx];
        const auto& dst = value.modes[path.targetInfo.modeInfoIdx];
        const auto& signal = dst.targetMode.targetVideoSignalInfo;
        if (src.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE || dst.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_TARGET ||
            src.sourceMode.width != 1280 || src.sourceMode.height != 720 || !signal.vSyncFreq.Denominator ||
            static_cast<UINT64>(signal.vSyncFreq.Numerator) != 120ull * signal.vSyncFreq.Denominator ||
            signal.activeSize.cx != 1280 || signal.activeSize.cy != 720)
            throw std::runtime_error("Actual active CCD source/target is not 1280x720@120Hz");
        if (source) *source = src.sourceMode;
    }
    return found;
}
LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) { return DefWindowProcW(window, message, wparam, lparam); }
void Render(const std::wstring& id, const std::filesystem::path& directory, const Topology& original, bool deactivate) {
    auto topology = Query(QDC_ONLY_ACTIVE_PATHS);
    if (!Active(topology, id)) {
        auto path = UniqueTarget(Query(QDC_ALL_PATHS), id);
        DISPLAYCONFIG_TARGET_PREFERRED_MODE preferred{};
        preferred.header = {DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE, sizeof(preferred), path.targetInfo.adapterId, path.targetInfo.id};
        Check(DisplayConfigGetDeviceInfo(&preferred.header), "Read owned target preferred mode");
        LONG right = 0;
        for (const auto& current : topology.paths) {
            if (current.sourceInfo.modeInfoIdx >= topology.modes.size()) throw std::runtime_error("Missing baseline source mode");
            const auto& source = topology.modes[current.sourceInfo.modeInfoIdx].sourceMode;
            right = (std::max)(right, source.position.x + static_cast<LONG>(source.width));
        }
        DISPLAYCONFIG_MODE_INFO source{}; source.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE; source.id = path.sourceInfo.id; source.adapterId = path.sourceInfo.adapterId;
        source.sourceMode = {1280, 720, DISPLAYCONFIG_PIXELFORMAT_32BPP, {right, 0}};
        path.sourceInfo.modeInfoIdx = static_cast<UINT>(topology.modes.size()); topology.modes.push_back(source);
        DISPLAYCONFIG_MODE_INFO target{}; target.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_TARGET; target.id = path.targetInfo.id; target.adapterId = path.targetInfo.adapterId; target.targetMode = preferred.targetMode;
        path.targetInfo.modeInfoIdx = static_cast<UINT>(topology.modes.size()); topology.modes.push_back(target);
        path.flags = DISPLAYCONFIG_PATH_ACTIVE; path.targetInfo.refreshRate = {120, 1}; path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        topology.paths.push_back(path); Apply(topology);
    }
    DISPLAYCONFIG_SOURCE_MODE source{};
    if (!Active(Query(QDC_ONLY_ACTIVE_PATHS), id, &source)) throw std::runtime_error("Owned target did not become active");
    std::cout << "active_ccd=1280x720@120/1 position=" << source.position.x << ',' << source.position.y << std::endl;
    if (!SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)) Check(static_cast<LONG>(GetLastError()), "SetProcessDpiAwarenessContext");
    WNDCLASSW type{}; type.lpfnWndProc = WindowProc; type.hInstance = GetModuleHandleW(nullptr); type.lpszClassName = L"XsccIddDisposableActiveProbe";
    if (!RegisterClassW(&type)) Check(static_cast<LONG>(GetLastError()), "RegisterClassW");
    const HWND window = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, type.lpszClassName, L"XSCC isolated frame producer", WS_POPUP,
        source.position.x, source.position.y, 1280, 720, nullptr, nullptr, type.hInstance, nullptr);
    if (!window) Check(static_cast<LONG>(GetLastError()), "CreateWindowExW");
    struct Destroy { HWND value; ~Destroy() { DestroyWindow(value); } } destroy{window};
    DXGI_SWAP_CHAIN_DESC description{}; description.BufferDesc = {1280, 720, {120, 1}, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED, DXGI_MODE_SCALING_UNSPECIFIED};
    description.SampleDesc.Count = 1; description.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT; description.BufferCount = 2; description.OutputWindow = window; description.Windowed = TRUE; description.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context; ComPtr<IDXGISwapChain> chain;
    CheckHr(D3D11CreateDeviceAndSwapChain(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
        &description, &chain, &device, nullptr, &context), "Create WARP producer");
    ComPtr<ID3D11Texture2D> buffer; CheckHr(chain->GetBuffer(0, IID_PPV_ARGS(&buffer)), "Read producer backbuffer");
    ComPtr<ID3D11RenderTargetView> view; CheckHr(device->CreateRenderTargetView(buffer.Get(), nullptr, &view), "Create producer view");
    ShowWindow(window, SW_SHOWNOACTIVATE);
    const auto started = Clock::now(); auto nextQuery = started; UINT64 presents = 0; bool departed = false;
    while (Clock::now() - started < std::chrono::seconds(18)) {
        MSG message{}; while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&message); DispatchMessageW(&message); }
        if (deactivate && Clock::now() - started >= std::chrono::seconds(5)) {
            std::cout << "phase=os_deactivate_requested_while_owner_remains" << std::endl;
            Restore(original);
            if (Active(Query(QDC_ONLY_ACTIVE_PATHS), id)) throw std::runtime_error("Owned target remains active after OS deactivation");
            departed = true; break;
        }
        if (Clock::now() >= nextQuery) {
            if (!Active(Query(QDC_ONLY_ACTIVE_PATHS), id)) { departed = true; break; }
            nextQuery = Clock::now() + std::chrono::milliseconds(100);
        }
        const float color[] = {presents % 2 ? 0.9f : 0.1f, presents % 3 ? 0.2f : 0.8f, 0.4f, 1.0f};
        context->ClearRenderTargetView(view.Get(), color);
        const HRESULT result = chain->Present(1, 0);
        if (FAILED(result)) { if (!Active(Query(QDC_ONLY_ACTIVE_PATHS), id)) { departed = true; break; } CheckHr(result, "Present producer frame"); }
        ++presents;
        if (presents == 1) {
            std::ofstream ready(directory / L"ready.tmp"); ready << "active-confirmed\n"; ready.close();
            if (!ready || !MoveFileExW((directory / L"ready.tmp").c_str(), (directory / L"active-ready.txt").c_str(), 0)) throw std::runtime_error("Failed to publish active readiness");
        }
    }
    std::cout << "producer_presents=" << presents << " elapsed_ms=" << std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - started).count()
        << " owned_target_departed=" << departed << "; Present counts do not establish visible FPS" << std::endl;
    if (!departed) throw std::runtime_error("Owned active target did not depart while producer was running");
}
int wmain(int argc, wchar_t** argv) {
    try {
        if (argc != 4) throw std::runtime_error("Expected --snapshot/--run/--restore exact-root dedicated-directory");
        const std::wstring operation = argv[1], id = argv[2]; Guard(id);
        const std::filesystem::path directory = argv[3], baseline = directory / L"baseline.bin";
        DWORD session = 0; if (!ProcessIdToSessionId(GetCurrentProcessId(), &session)) Check(static_cast<LONG>(GetLastError()), "ProcessIdToSessionId");
        std::cout << "process_session=" << session << " console_session=" << WTSGetActiveConsoleSessionId() << std::endl;
        if (operation == L"--snapshot") {
            const auto value = Query(QDC_ONLY_ACTIVE_PATHS);
            if (value.paths.empty()) throw std::runtime_error("No active original desktop paths");
            for (const auto& path : value.paths) if (Owned(path, id)) throw std::runtime_error("Snapshot already contains an owned target");
            Save(value, baseline); return 0;
        }
        const auto original = Load(baseline, id);
        if (operation == L"--restore") { Restore(original); std::cout << "original_layout_restored=1" << std::endl; return 0; }
        if (operation != L"--run" && operation != L"--deactivate") throw std::runtime_error("Unexpected CI helper operation");
        try { Render(id, directory, original, operation == L"--deactivate"); }
        catch (...) { try { Restore(original); } catch (const std::exception& error) { std::cerr << "restore_failure=" << error.what() << std::endl; } throw; }
        Restore(original); std::cout << "original_layout_restored=1" << std::endl; return 0;
    } catch (const std::system_error& error) { std::cerr << "XSCC_IDD_ACTIVE_HELPER=failed native_code=" << error.code().value() << ": " << error.what() << std::endl; return 1;
    } catch (const std::exception& error) { std::cerr << "XSCC_IDD_ACTIVE_HELPER=failed: " << error.what() << std::endl; return 1; }
}
