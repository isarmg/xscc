// SPDX-License-Identifier: MIT
#pragma once

#include "ControlProtocol.h"

#include <cstdint>
#include <limits>
#include <numeric>

namespace xscc::protocol {

struct Rational {
    std::uint32_t numerator;
    std::uint32_t denominator;
};

struct ModeTiming {
    std::uint32_t activeWidth;
    std::uint32_t activeHeight;
    std::uint32_t totalWidth;
    std::uint32_t totalHeight;
    std::uint64_t pixelRate;
    Rational hSync;
    Rational vSync;
    std::uint32_t horizontalFrontPorch;
    std::uint32_t horizontalSyncWidth;
    std::uint32_t horizontalBackPorch;
    std::uint32_t verticalFrontPorch;
    std::uint32_t verticalSyncWidth;
    std::uint32_t verticalBackPorch;
};

enum class TimingError {
    None,
    InvalidMode,
    DimensionOverflow,
    PixelRateOverflow,
    FrequencyOverflow,
};

// IDDCX_MONITOR_MODE requires zero; IDDCX_TARGET_MODE requires a nonzero divider.
inline constexpr std::uint32_t kMonitorVSyncDivider = 0;
inline constexpr std::uint32_t kTargetVSyncDivider = 1;

inline bool CheckedMultiply(std::uint64_t left, std::uint64_t right,
                            std::uint64_t& product) noexcept {
    if (right != 0 && left > (std::numeric_limits<std::uint64_t>::max)() / right) {
        return false;
    }
    product = left * right;
    return true;
}

inline bool ReducedFrequency(std::uint64_t left, std::uint64_t right,
                             std::uint32_t denominator, Rational& frequency) noexcept {
    auto divisor = std::gcd(left, std::uint64_t{denominator});
    left /= divisor;
    denominator /= static_cast<std::uint32_t>(divisor);
    divisor = std::gcd(right, std::uint64_t{denominator});
    right /= divisor;
    denominator /= static_cast<std::uint32_t>(divisor);
    std::uint64_t numerator{};
    if (!CheckedMultiply(left, right, numerator) ||
        numerator > (std::numeric_limits<std::uint32_t>::max)()) {
        return false;
    }
    frequency = {static_cast<std::uint32_t>(numerator), denominator};
    return true;
}

inline TimingError BuildModeTiming(const Mode& mode, ModeTiming& output) noexcept {
    if (mode.width == 0 || mode.height == 0 || mode.refreshRateUnits == 0) {
        return TimingError::InvalidMode;
    }
    // Synthetic virtual-only blanking: 80 * 125 makes every 1/10000 Hz exact.
    // This is not a CVT/EDID timing or a claim about a physical output's limits.
    const std::uint64_t totalWidth = ((std::uint64_t{mode.width} + 160 + 79) / 80) * 80;
    const std::uint64_t totalHeight = ((std::uint64_t{mode.height} + 32 + 124) / 125) * 125;
    if (totalWidth > (std::numeric_limits<std::uint32_t>::max)() ||
        totalHeight > (std::numeric_limits<std::uint32_t>::max)()) {
        return TimingError::DimensionOverflow;
    }
    ModeTiming timing{};
    timing.activeWidth = mode.width;
    timing.activeHeight = mode.height;
    timing.totalWidth = static_cast<std::uint32_t>(totalWidth);
    timing.totalHeight = static_cast<std::uint32_t>(totalHeight);
    std::uint64_t pixelsPerRateUnit{};
    if (!CheckedMultiply(totalWidth / 80, totalHeight / 125, pixelsPerRateUnit) ||
        !CheckedMultiply(pixelsPerRateUnit, mode.refreshRateUnits, timing.pixelRate)) {
        return TimingError::PixelRateOverflow;
    }
    if (!ReducedFrequency(mode.refreshRateUnits, totalHeight, kRefreshRateScale, timing.hSync) ||
        !ReducedFrequency(mode.refreshRateUnits, 1, kRefreshRateScale, timing.vSync)) {
        return TimingError::FrequencyOverflow;
    }
    timing.horizontalFrontPorch = 48;
    timing.horizontalSyncWidth = 32;
    timing.horizontalBackPorch = timing.totalWidth - mode.width - 48 - 32;
    timing.verticalFrontPorch = 3;
    timing.verticalSyncWidth = 5;
    timing.verticalBackPorch = timing.totalHeight - mode.height - 3 - 5;
    output = timing;
    return TimingError::None;
}

} // namespace xscc::protocol
