// SPDX-License-Identifier: MIT
// Independent XSCC maintenance change; no WDF, GPU, or thread ownership operations.
#pragma once
#include <cstdint>
namespace xscc_mtt {
enum class LoopAction { Continue, Stop, Error };
// Windows wait values; keep the driver adapter's static assertions in sync.
constexpr LoopAction before_acquire(std::uint32_t result) {
    return result == 0 ? LoopAction::Stop : result == 258 ? LoopAction::Continue : LoopAction::Error;
}
// The pending wait array MUST put terminate at index zero, frame-ready at index one.
constexpr LoopAction after_pending_wait(std::uint32_t result) {
    return result == 0 ? LoopAction::Stop : (result == 1 || result == 258) ? LoopAction::Continue : LoopAction::Error;
}
}
