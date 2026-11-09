// SPDX-License-Identifier: MIT
#include "../source/Virtual Display Driver (HDR)/MttVDD/SwapChainStop.h"
#include <cassert>
#include <cstdint>
#include <iostream>
using xscc_mtt::LoopAction;

static void continuously_ready_producer_still_stops() {
    bool stopped = false;
    unsigned acquisitions = 0;
    for (unsigned iteration = 0; iteration < 1000; ++iteration) {
        const auto action = xscc_mtt::before_acquire(stopped ? 0u : 258u);
        if (action != LoopAction::Continue) {
            assert(action == LoopAction::Stop);
            break;
        }
        // Mock AcquireBuffer always succeeds: no E_PENDING can provide a stop check.
        ++acquisitions;
        if (acquisitions == 128) stopped = true;
    }
    assert(stopped && acquisitions == 128);
}

static void stop_wins_when_both_events_are_ready() {
    // Mock WaitForMultipleObjects selects the lowest-index signaled handle.
    const bool signaled[] = {true, true}; // production array: terminate, available
    std::uint32_t wait = 0xffffffffu;
    for (std::uint32_t index = 0; index < 2; ++index) {
        if (signaled[index]) { wait = index; break; }
    }
    assert(xscc_mtt::after_pending_wait(wait) == LoopAction::Stop);
    assert(xscc_mtt::after_pending_wait(1) == LoopAction::Continue);
    assert(xscc_mtt::after_pending_wait(258) == LoopAction::Continue);
}

static void invalid_wait_never_resumes_acquisition() {
    assert(xscc_mtt::before_acquire(0xffffffffu) == LoopAction::Error);
    assert(xscc_mtt::before_acquire(0x80) == LoopAction::Error);
    assert(xscc_mtt::after_pending_wait(0xffffffffu) == LoopAction::Error);
    assert(xscc_mtt::after_pending_wait(0x80) == LoopAction::Error);
}

int main() {
    continuously_ready_producer_still_stops();
    stop_wins_when_both_events_are_ready();
    invalid_wait_never_resumes_acquisition();
    std::cout << "3 MTT cancellation CPU tests passed\n";
}
