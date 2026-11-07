// SPDX-License-Identifier: MIT
// Frame consumption derives from VirtualDrivers/Virtual-Display-Driver 25.7.23 (MIT).
#include "SwapChainSession.h"

#include <avrt.h>
#include <d3d11.h>
#include <dxgi1_4.h>
#include <wrl/client.h>

#include <cstdlib>
#include <stdexcept>
#include <system_error>
#include <utility>

namespace xscc {
namespace {

constexpr NTSTATUS WaitFailure = static_cast<NTSTATUS>(0xC0000001L);
constexpr NTSTATUS StopTimeout = static_cast<NTSTATUS>(0xC00000B5L);
constexpr NTSTATUS InvalidSessionState = static_cast<NTSTATUS>(0xC0000184L);

[[noreturn]] void ThrowLastError(const char* operation) {
    throw std::system_error(static_cast<int>(GetLastError()), std::system_category(), operation);
}

bool Stopping(HANDLE stopEvent, IDDCX_ADAPTER adapter) noexcept {
    const DWORD result = WaitForSingleObject(stopEvent, 0);
    if (result == WAIT_OBJECT_0) {
        return true;
    }
    if (result != WAIT_TIMEOUT) {
        Fatal(adapter, WaitFailure);
    }
    return false;
}

HRESULT ConsumeFrames(IDDCX_ADAPTER adapter, IDDCX_SWAPCHAIN chain,
                      IDXGIDevice* device, HANDLE stopEvent, HANDLE frameEvent) noexcept {
    if (Stopping(stopEvent, adapter)) {
        return S_OK;
    }
    IDARG_IN_SWAPCHAINSETDEVICE setDevice = {};
    setDevice.pDevice = device;
    HRESULT result = IddCxSwapChainSetDevice(chain, &setDevice);
    if (FAILED(result)) {
        return result;
    }

    while (!Stopping(stopEvent, adapter)) {
        Microsoft::WRL::ComPtr<IDXGIResource> surface;
        if (IDD_IS_FUNCTION_AVAILABLE(IddCxSwapChainReleaseAndAcquireBuffer2)) {
            IDARG_IN_RELEASEANDACQUIREBUFFER2 input = {};
            input.Size = sizeof(input);
            IDARG_OUT_RELEASEANDACQUIREBUFFER2 output = {};
            result = IddCxSwapChainReleaseAndAcquireBuffer2(chain, &input, &output);
            if (SUCCEEDED(result)) {
                surface.Attach(output.MetaData.pSurface);
            }
        } else {
            IDARG_OUT_RELEASEANDACQUIREBUFFER output = {};
            result = IddCxSwapChainReleaseAndAcquireBuffer(chain, &output);
            if (SUCCEEDED(result)) {
                surface.Attach(output.MetaData.pSurface);
            }
        }
        if (result == E_PENDING) {
            // Stop is first so a continuously signaled frame event cannot win over cancellation.
            const HANDLE events[] = {stopEvent, frameEvent};
            const DWORD wait = WaitForMultipleObjects(2, events, FALSE, 16);
            if (wait == WAIT_OBJECT_0) {
                return S_OK;
            }
            if (wait == WAIT_OBJECT_0 + 1 || wait == WAIT_TIMEOUT) {
                continue;
            }
            Fatal(adapter, WaitFailure);
        }
        if (FAILED(result) || Stopping(stopEvent, adapter)) {
            return result;
        }
        surface.Reset();
        if (Stopping(stopEvent, adapter)) {
            return S_OK;
        }
        result = IddCxSwapChainFinishedProcessingFrame(chain);
        if (FAILED(result)) {
            return result;
        }
    }
    return S_OK;
}

} // namespace

[[noreturn]] void Fatal(IDDCX_ADAPTER adapter, NTSTATUS status) noexcept {
    if (adapter) {
        IDARG_IN_REPORTCRITICALERROR error = {};
        error.MajorErrorCode = 0x58;
        error.MinorErrorCode = static_cast<ULONG>(status) & 0xff;
        (void)IddCxReportCriticalError(adapter, &error);
    }
    // A failed critical-error DDI, or teardown without a valid adapter, must not resume execution.
    EXCEPTION_RECORD record = {};
    record.ExceptionCode = 0xC0000602;
    record.ExceptionAddress = reinterpret_cast<void*>(&Fatal);
    record.NumberParameters = 1;
    record.ExceptionInformation[0] = static_cast<ULONG_PTR>(static_cast<ULONG>(status));
    RaiseFailFastException(&record, nullptr, 0);
    std::abort();
}

SwapChainSession::SwapChainSession(IDDCX_ADAPTER adapter, IDDCX_SWAPCHAIN chain,
                                   LUID renderAdapter, HANDLE frameEvent)
    : adapter_(adapter), chain_(chain), render_adapter_(renderAdapter) {
    if (!adapter || !chain || !frameEvent || frameEvent == INVALID_HANDLE_VALUE) {
        throw std::invalid_argument("Invalid swap-chain assignment");
    }
    stop_event_.value = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!stop_event_.value) {
        ThrowLastError("CreateEventW(stop)");
    }
    const HANDLE process = GetCurrentProcess();
    if (!DuplicateHandle(process, frameEvent, process, &frame_event_.value,
                         0, FALSE, DUPLICATE_SAME_ACCESS)) {
        ThrowLastError("DuplicateHandle(frame)");
    }
}

std::shared_ptr<SwapChainSession> SwapChainSession::Start(
    IDDCX_ADAPTER adapter, IDDCX_SWAPCHAIN chain, LUID renderAdapter, HANDLE frameEvent) {
    auto session = std::shared_ptr<SwapChainSession>(
        new SwapChainSession(adapter, chain, renderAdapter, frameEvent));
    auto argument = std::make_unique<std::shared_ptr<SwapChainSession>>(session);
    const HANDLE thread = CreateThread(nullptr, 0, WorkerEntry, argument.get(), 0, nullptr);
    if (!thread) {
        ThrowLastError("CreateThread(swap-chain)");
    }
    session->thread_.value = thread;
    argument.release();
    return session;
}

SwapChainSession::~SwapChainSession() noexcept {
    if (thread_.value && !Finished()) {
        // The device registry must retain this session until the native thread actually exits.
        Fatal(nullptr, InvalidSessionState);
    }
}

bool SwapChainSession::Finished() const noexcept {
    if (!thread_.value) {
        return true;
    }
    const DWORD result = WaitForSingleObject(thread_.value, 0);
    if (result == WAIT_OBJECT_0) {
        return true;
    }
    if (result != WAIT_TIMEOUT) {
        Fatal(nullptr, WaitFailure);
    }
    return false;
}

void SwapChainSession::StopAndWait(bool adapterValid) noexcept {
    const IDDCX_ADAPTER adapter = adapterValid ? adapter_ : nullptr;
    if (!SetEvent(stop_event_.value)) {
        Fatal(adapter, WaitFailure);
    }
    if (worker_id_.load(std::memory_order_acquire) == GetCurrentThreadId()) {
        // WdfObjectDelete may reenter an unassign/cleanup callback on its owning worker.
        // Deletion is already in progress; this worker will make no further swap-chain DDI calls.
        const Phase phase = phase_.load(std::memory_order_acquire);
        if (phase == Phase::Deleting) {
            return;
        }
        // Windows can reuse the thread id after the original worker has exited.
        if (phase == Phase::Finished && Finished()) {
            return;
        }
        Fatal(adapter, InvalidSessionState);
    }
    if (!adapterValid && !Finished()) {
        Fatal(nullptr, InvalidSessionState);
    }
    const DWORD result = WaitForSingleObject(thread_.value, StopBudgetMs);
    if (result != WAIT_OBJECT_0) {
        Fatal(adapter, result == WAIT_TIMEOUT ? StopTimeout : WaitFailure);
    }
}

DWORD WINAPI SwapChainSession::WorkerEntry(void* argument) noexcept {
    std::unique_ptr<std::shared_ptr<SwapChainSession>> owner(
        static_cast<std::shared_ptr<SwapChainSession>*>(argument));
    auto session = std::move(*owner);
    owner.reset();
    session->worker_id_.store(GetCurrentThreadId(), std::memory_order_release);
    session->RunWorker();
    return 0;
}

void SwapChainSession::RunWorker() noexcept {
    DWORD taskIndex = 0;
    const HANDLE task = AvSetMmThreadCharacteristicsW(L"Distribution", &taskIndex);
    {
        Microsoft::WRL::ComPtr<IDXGIFactory4> factory;
        Microsoft::WRL::ComPtr<IDXGIAdapter1> renderAdapter;
        Microsoft::WRL::ComPtr<ID3D11Device> device;
        Microsoft::WRL::ComPtr<ID3D11DeviceContext> context;
        Microsoft::WRL::ComPtr<IDXGIDevice> dxgiDevice;
        HRESULT result = S_OK;
        if (!Stopping(stop_event_.value, adapter_)) {
            result = CreateDXGIFactory2(0, IID_PPV_ARGS(&factory));
        }
        if (SUCCEEDED(result) && !Stopping(stop_event_.value, adapter_) && factory) {
            result = factory->EnumAdapterByLuid(render_adapter_, IID_PPV_ARGS(&renderAdapter));
        }
        if (SUCCEEDED(result) && !Stopping(stop_event_.value, adapter_) && renderAdapter) {
            const D3D_FEATURE_LEVEL levels[] = {D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0};
            result = D3D11CreateDevice(
                renderAdapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, levels, ARRAYSIZE(levels),
                D3D11_SDK_VERSION, &device, nullptr, &context);
        }
        if (SUCCEEDED(result) && !Stopping(stop_event_.value, adapter_) && device) {
            result = device.As(&dxgiDevice);
        }
        if (SUCCEEDED(result) && !Stopping(stop_event_.value, adapter_) && dxgiDevice) {
            result = ConsumeFrames(adapter_, chain_, dxgiDevice.Get(),
                                   stop_event_.value, frame_event_.value);
        }
        if (FAILED(result) && !Stopping(stop_event_.value, adapter_)) {
            // Deleting an unsuccessful chain causes reassignment; avoid a tight create/fail loop.
            const DWORD wait = WaitForSingleObject(stop_event_.value, 250);
            if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT) {
                Fatal(adapter_, WaitFailure);
            }
        }
        phase_.store(Phase::Deleting, std::memory_order_release);
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(chain_));
        chain_ = nullptr;
    }
    if (task) {
        AvRevertMmThreadCharacteristics(task);
    }
    phase_.store(Phase::Finished, std::memory_order_release);
}

} // namespace xscc
