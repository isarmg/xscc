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

bool Signaled(HANDLE event) noexcept {
    const DWORD result = WaitForSingleObject(event, 0);
    if (result != WAIT_OBJECT_0 && result != WAIT_TIMEOUT) {
        Fatal(nullptr, WaitFailure);
    }
    return result == WAIT_OBJECT_0;
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
        UINT frameNumber = 0;
        if (IDD_IS_FUNCTION_AVAILABLE(IddCxSwapChainReleaseAndAcquireBuffer2)) {
            IDARG_IN_RELEASEANDACQUIREBUFFER2 input = {};
            input.Size = sizeof(input);
            IDARG_OUT_RELEASEANDACQUIREBUFFER2 output = {};
            result = IddCxSwapChainReleaseAndAcquireBuffer2(chain, &input, &output);
            if (SUCCEEDED(result)) {
                surface.Attach(output.MetaData.pSurface);
                frameNumber = output.MetaData.PresentationFrameNumber;
            }
        } else {
            IDARG_OUT_RELEASEANDACQUIREBUFFER output = {};
            result = IddCxSwapChainReleaseAndAcquireBuffer(chain, &output);
            if (SUCCEEDED(result)) {
                surface.Attach(output.MetaData.pSurface);
                frameNumber = output.MetaData.PresentationFrameNumber;
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
        if (FAILED(result)) {
            return result;
        }
        LARGE_INTEGER acquired{};
        if (!QueryPerformanceCounter(&acquired)) {
            return E_FAIL;
        }
        if (Stopping(stopEvent, adapter)) {
            return S_OK;
        }
        surface.Reset();
        if (Stopping(stopEvent, adapter)) {
            return S_OK;
        }
        result = IddCxSwapChainFinishedProcessingFrame(chain);
        if (FAILED(result)) {
            return result;
        }
        IDARG_IN_REPORTFRAMESTATISTICS report{};
        report.FrameStatistics.Size = sizeof(report.FrameStatistics);
        report.FrameStatistics.PresentationFrameNumber = frameNumber;
        // This local sink discards frames; it neither encodes nor transmits them.
        report.FrameStatistics.FrameStatus = IDDCX_FRAME_STATUS_DROPPED;
        report.FrameStatistics.FrameSliceTotal = 1;
        report.FrameStatistics.FrameAcquireQpcTime = static_cast<UINT64>(acquired.QuadPart);
        const NTSTATUS status = IddCxSwapChainReportFrameStatistics(chain, &report);
        if (!NT_SUCCESS(status)) {
            return HRESULT_FROM_NT(status);
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
    frame_quiesced_event_.value = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!frame_quiesced_event_.value) {
        ThrowLastError("CreateEventW(frame-quiesced)");
    }
    delete_returned_event_.value = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!delete_returned_event_.value) {
        ThrowLastError("CreateEventW(delete-returned)");
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
    if (thread_.value && (!DeleteReturned() || !Finished())) {
        // Both the deleting caller and the native worker must have retired.
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

bool SwapChainSession::DeleteReturned() const noexcept {
    return Signaled(delete_returned_event_.value);
}

bool SwapChainSession::DeletingOnCurrentThread() const noexcept {
    const DWORD thread = deleting_thread_id_.load(std::memory_order_acquire);
    return thread != 0 && thread == GetCurrentThreadId();
}

bool SwapChainSession::ReadyForMonitorCleanup() const noexcept {
    return Signaled(frame_quiesced_event_.value) &&
           (DeleteReturned() || DeletingOnCurrentThread());
}

void SwapChainSession::WaitForDeletionAndExit(IDDCX_ADAPTER adapter) const noexcept {
    const ULONGLONG started = GetTickCount64();
    DWORD result = WaitForSingleObject(delete_returned_event_.value, StopBudgetMs);
    if (result != WAIT_OBJECT_0) {
        Fatal(adapter, result == WAIT_TIMEOUT ? StopTimeout : WaitFailure);
    }
    const ULONGLONG elapsed = GetTickCount64() - started;
    const DWORD remaining = elapsed < StopBudgetMs ?
        static_cast<DWORD>(StopBudgetMs - elapsed) : 0;
    result = WaitForSingleObject(thread_.value, remaining);
    if (result != WAIT_OBJECT_0) {
        Fatal(adapter, result == WAIT_TIMEOUT ? StopTimeout : WaitFailure);
    }
}

void SwapChainSession::StopAndWait(bool adapterValid) noexcept {
    const IDDCX_ADAPTER adapter = adapterValid ? adapter_ : nullptr;
    if (!SetEvent(stop_event_.value)) {
        Fatal(adapter, WaitFailure);
    }
    if (DeletingOnCurrentThread()) {
        // Only synchronous reentry on this deleting thread may bypass its own wait.
        return;
    }
    if (worker_id_.load(std::memory_order_acquire) == GetCurrentThreadId() && !Finished()) {
        Fatal(adapter, InvalidSessionState);
    }
    if (!adapterValid && !Finished()) {
        Fatal(nullptr, InvalidSessionState);
    }
    WaitForDeletionAndExit(adapter);
}

void SwapChainSession::RetireFromCallback() noexcept {
    DeleteOwner expected = DeleteOwner::Unclaimed;
    if (!delete_owner_.compare_exchange_strong(expected, DeleteOwner::Callback,
                                              std::memory_order_acq_rel)) {
        StopAndWait();
        return;
    }
    if (!SetEvent(stop_event_.value)) {
        Fatal(adapter_, WaitFailure);
    }
    const DWORD result = WaitForSingleObject(frame_quiesced_event_.value, StopBudgetMs);
    if (result != WAIT_OBJECT_0) {
        Fatal(adapter_, result == WAIT_TIMEOUT ? StopTimeout : WaitFailure);
    }
    DeleteChainOnCurrentThread();
}

void SwapChainSession::DeleteChainOnCurrentThread() noexcept {
    deleting_thread_id_.store(GetCurrentThreadId(), std::memory_order_release);
    WdfObjectDelete(reinterpret_cast<WDFOBJECT>(chain_));
    chain_ = nullptr;
    if (!SetEvent(delete_returned_event_.value)) {
        Fatal(nullptr, WaitFailure);
    }
    deleting_thread_id_.store(0, std::memory_order_release);
}

void SwapChainSession::WaitForExit() const noexcept {
    if (!DeleteReturned() || !Signaled(frame_quiesced_event_.value)) {
        Fatal(nullptr, InvalidSessionState);
    }
    const DWORD result = WaitForSingleObject(thread_.value, StopBudgetMs);
    if (result != WAIT_OBJECT_0) {
        Fatal(nullptr, result == WAIT_TIMEOUT ? StopTimeout : WaitFailure);
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
        DeleteOwner expected = DeleteOwner::Unclaimed;
        const bool ownsDelete = delete_owner_.compare_exchange_strong(
            expected, DeleteOwner::Worker, std::memory_order_acq_rel);
        if (!SetEvent(frame_quiesced_event_.value)) {
            Fatal(adapter_, WaitFailure);
        }
        if (ownsDelete) {
            DeleteChainOnCurrentThread();
        }
    }
    if (task) {
        AvRevertMmThreadCharacteristics(task);
    }
}

} // namespace xscc
