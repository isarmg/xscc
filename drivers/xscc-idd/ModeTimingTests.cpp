// SPDX-License-Identifier: MIT
#include "ModeTiming.h"

#include <array>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <vector>

#ifdef _WIN32
#include <Windows.h>
#include <winioctl.h>
static_assert(xscc::protocol::kIoctlAdd ==
              CTL_CODE(xscc::protocol::kDeviceType, 0x802, METHOD_BUFFERED,
                       FILE_READ_DATA | FILE_WRITE_DATA));
#endif

using namespace xscc::protocol;

namespace {

void Require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAILED: " << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

void ProtocolBuffersAreExactAndAllowZeroMonitors() {
    std::uint32_t bytes{};
    Require(RequestSizeForModes(0, bytes) && bytes == 16, "zero-mode request header");
    Require(ListSizeForMonitors(0, bytes) && bytes == 16, "zero-monitor inventory");
    Require(!ListSizeForMonitors(kConnectorCapacity + 1, bytes), "connector budget");
    Require(RequestSizeForModes(kMaxModesPerMonitor, bytes) && bytes == 49168,
            "mode-list byte budget");
    Require(!RequestSizeForModes(UINT32_MAX, bytes), "oversized mode count");

    RequestHeader header{kVersion, 16, kNoConnector, 0};
    RequestHeader decoded{};
    Require(ReadRequestHeader(&header, sizeof(header), decoded), "query/list/remove header");
    Require(decoded.modeCount == 0, "zero-screen request remains zero");
    Require(!ReadRequestHeader(nullptr, sizeof(header), decoded), "null input rejected");
    Require(!ReadRequestHeader(&header, sizeof(header) - 1, decoded), "truncated header");
    header.version = kVersion + 1;
    Require(!ReadRequestHeader(&header, sizeof(header), decoded), "unknown version");
    header.version = kVersion;
    header.modeCount = UINT32_MAX;
    Require(!ReadRequestHeader(&header, sizeof(header), decoded), "count cannot overflow size");

    header = {kVersion, 28, 3, 1};
    Mode requested{1733, 977, 1234567};
    // The reader must not assume alignment of a caller-provided byte buffer.
    std::array<std::uint8_t, 29> packet{};
    auto* input = packet.data() + 1;
    std::memcpy(input, &header, sizeof(header));
    std::memcpy(input + sizeof(header), &requested, sizeof(requested));
    Require(ReadRequestHeader(input, 28, decoded), "inline modes accepted");
    Mode mode{};
    Require(ReadRequestMode(input, 28, 0, mode), "inline mode copied");
    Require(mode.width == 1733 && mode.height == 977 && mode.refreshRateUnits == 1234567,
            "inline mode precision preserved");
    Require(!ReadRequestMode(input, 28, 1, mode), "out-of-range inline mode");
    Require(!ReadRequestHeader(input, 27, decoded), "truncated inline payload");
    Require(!ReadRequestHeader(input, 29, decoded), "extra bytes rejected");
}

void DynamicModesPreserveExactRefresh() {
    const std::array<Mode, 9> modes{{
        {1920, 1080, 600000},
        {1920, 1080, 1200000},
        {2560, 1440, 2400000},
        {3840, 2160, 1200000},
        {7680, 4320, 1440000},
        {7680, 4320, 2400000},
        {1920, 1080, 599400},
        {1920, 1080, 599999},
        {1733, 977, 1234567},
    }};
    for (const auto& mode : modes) {
        ModeTiming timing{};
        Require(BuildModeTiming(mode, timing) == TimingError::None, "supported numeric timing");
        Require(timing.activeWidth == mode.width && timing.activeHeight == mode.height,
                "active image size unchanged");
        Require(timing.totalWidth > mode.width && timing.totalHeight > mode.height,
                "nonzero virtual blanking");
        Require(timing.horizontalFrontPorch + timing.horizontalSyncWidth +
                    timing.horizontalBackPorch == timing.totalWidth - timing.activeWidth,
                "horizontal blanking is consistent");
        Require(timing.verticalFrontPorch + timing.verticalSyncWidth +
                    timing.verticalBackPorch == timing.totalHeight - timing.activeHeight,
                "vertical blanking is consistent");
        Require(std::uint64_t{timing.vSync.numerator} * kRefreshRateScale ==
                    std::uint64_t{mode.refreshRateUnits} * timing.vSync.denominator,
                "exact 1/10000 Hz refresh");
        Require(std::uint64_t{timing.hSync.numerator} * timing.vSync.denominator ==
                    std::uint64_t{timing.vSync.numerator} * timing.totalHeight *
                        timing.hSync.denominator,
                "horizontal sync matches exact vertical refresh");
        Require(timing.pixelRate * timing.vSync.denominator ==
                    std::uint64_t{timing.totalWidth} * timing.totalHeight * timing.vSync.numerator,
                "integer pixel clock matches both sync rates");
        if (mode.width == 7680 && mode.refreshRateUnits >= 1440000) {
            Require(timing.pixelRate > UINT32_MAX, "8K high-refresh pixel clock uses 64 bits");
        }
    }
    Require(kMonitorVSyncDivider == 0 && kTargetVSyncDivider == 1,
            "monitor and target divider contracts differ");
}

void OverflowAndInvalidModesFailWithoutPartialOutput() {
    struct RejectedMode {
        Mode mode;
        TimingError error;
    };
    const std::array<RejectedMode, 7> cases{{
        {{0, 1080, 1200000}, TimingError::InvalidMode},
        {{1920, 0, 1200000}, TimingError::InvalidMode},
        {{1920, 1080, 0}, TimingError::InvalidMode},
        {{UINT32_MAX, 1080, 1200000}, TimingError::DimensionOverflow},
        {{1920, UINT32_MAX, 1200000}, TimingError::DimensionOverflow},
        {{4000000000u, 4000000000u, UINT32_MAX}, TimingError::PixelRateOverflow},
        {{16, 1000000, 1234567}, TimingError::FrequencyOverflow},
    }};
    for (const auto& rejected : cases) {
        ModeTiming timing{};
        timing.activeWidth = 42;
        timing.pixelRate = 123;
        Require(BuildModeTiming(rejected.mode, timing) == rejected.error,
                "invalid or unrepresentable mode rejected");
        Require(timing.activeWidth == 42 && timing.pixelRate == 123,
                "failed conversion cannot publish a partial timing");
    }
}

} // namespace

int main() {
    ProtocolBuffersAreExactAndAllowZeroMonitors();
    DynamicModesPreserveExactRefresh();
    OverflowAndInvalidModesFailWithoutPartialOutput();
    std::cout << "3 XSCC IDD protocol/timing CPU tests passed\n";
    return EXIT_SUCCESS;
}
