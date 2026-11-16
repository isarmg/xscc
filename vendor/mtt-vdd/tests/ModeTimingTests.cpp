#define NOMINMAX
#include <windows.h>
#include <stdexcept>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <sstream>

using std::stringstream;
// The extracted production target-mode function logs its calculations only.
static void vddlog(const char*, const char*) {}
#include "ModeTimingUnderTest.h"

static void require(bool condition) {
    if (!condition) throw std::runtime_error("Invalid monitor or target signal timing");
}

static void check(const DISPLAYCONFIG_VIDEO_SIGNAL_INFO& signal, UINT width, UINT height, UINT rn, UINT rd) {
    require(signal.activeSize.cx == width && signal.activeSize.cy == height);
    require(signal.totalSize.cx >= width && signal.totalSize.cy >= height);
    require(signal.scanLineOrdering == DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE);
    require(signal.vSyncFreq.Denominator != 0 && signal.hSyncFreq.Denominator != 0);
    require(std::uint64_t(signal.vSyncFreq.Numerator) * rd == std::uint64_t(rn) * signal.vSyncFreq.Denominator);
    const double pixels = double(signal.totalSize.cx) * signal.totalSize.cy;
    const double hz = double(rn) / rd;
    require(std::abs(double(signal.pixelRate) / pixels - hz) <= 1.0 / pixels + 1e-9);
    require(std::abs(double(signal.hSyncFreq.Numerator) / signal.hSyncFreq.Denominator - hz * signal.totalSize.cy) < 1e-6);
}

static void pair(UINT width, UINT height, UINT rn, UINT rd) {
    check(dispinfo(width, height, rn, rd), width, height, rn, rd);
    DISPLAYCONFIG_VIDEO_SIGNAL_INFO target{};
    CreateTargetMode(target, width, height, rn, rd);
    check(target, width, height, rn, rd);
}

int main() try {
    for (UINT hz : {60u, 120u, 240u}) {
        pair(1920, 1080, hz, 1);
        pair(2560, 1440, hz, 1);
        pair(1080, 1920, hz, 1);
    }
    pair(3840, 2160, 60000, 1001);
    pair(1920, 1080, 5994, 25);
    pair(7680, 4320, 240, 1);
    const auto large = dispinfo(7680, 4320, 240, 1);
    require(large.pixelRate > std::numeric_limits<UINT32>::max());
    std::puts("3 MTT mode timing CPU tests passed");
} catch (const std::exception& error) {
    std::fprintf(stderr, "%s\n", error.what());
    return 1;
}
