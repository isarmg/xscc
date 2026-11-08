// SPDX-License-Identifier: MIT
#pragma once

#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <wudfwdm.h>
#include <wdf.h>
#include <iddcx.h>

#include <atomic>
#include <memory>

namespace xscc {

[[noreturn]] void Fatal(IDDCX_ADAPTER adapter, NTSTATUS status) noexcept;

class SwapChainSession final {
public:
    // On failure before thread creation, Start throws without deleting the supplied chain.
    // After success retain this object until DeleteReturned() and Finished() are both true.
    static std::shared_ptr<SwapChainSession> Start(
        IDDCX_ADAPTER adapter, IDDCX_SWAPCHAIN chain, LUID renderAdapter, HANDLE frameEvent);

    ~SwapChainSession() noexcept;
    void StopAndWait(bool adapterValid = true) noexcept;
    void RetireFromCallback() noexcept;
    void WaitForExit() const noexcept;
    bool DeleteReturned() const noexcept;
    bool ReadyForMonitorCleanup() const noexcept;
    // Uses the native thread handle: reaching the end of RunWorker is not thread completion.
    bool Finished() const noexcept;

    SwapChainSession(const SwapChainSession&) = delete;
    SwapChainSession& operator=(const SwapChainSession&) = delete;

private:
    struct Handle final {
        Handle() noexcept = default;
        ~Handle() noexcept {
            if (value && value != INVALID_HANDLE_VALUE) {
                CloseHandle(value);
            }
        }
        Handle(const Handle&) = delete;
        Handle& operator=(const Handle&) = delete;
        HANDLE value = nullptr;
    };

    enum class DeleteOwner { Unclaimed, Worker, Callback };
    static constexpr DWORD StopBudgetMs = 1500;

    SwapChainSession(IDDCX_ADAPTER adapter, IDDCX_SWAPCHAIN chain,
                     LUID renderAdapter, HANDLE frameEvent);
    static DWORD WINAPI WorkerEntry(void* argument) noexcept;
    void RunWorker() noexcept;
    void DeleteChainOnCurrentThread() noexcept;
    void WaitForDeletionAndExit(IDDCX_ADAPTER adapter) const noexcept;
    bool DeletingOnCurrentThread() const noexcept;

    const IDDCX_ADAPTER adapter_;
    IDDCX_SWAPCHAIN chain_;
    const LUID render_adapter_;
    Handle stop_event_;
    Handle frame_quiesced_event_;
    Handle delete_returned_event_;
    Handle frame_event_;
    Handle thread_;
    std::atomic<DWORD> worker_id_{0};
    std::atomic<DeleteOwner> delete_owner_{DeleteOwner::Unclaimed};
    std::atomic<DWORD> deleting_thread_id_{0};
};

} // namespace xscc
