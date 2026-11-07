// SPDX-License-Identifier: MIT
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <type_traits>

#ifdef _WIN32
#include <guiddef.h>
#endif

namespace xscc::protocol {

inline constexpr std::uint32_t kVersion = 1;
inline constexpr std::uint32_t kRefreshRateScale = 10000;
// Per-adapter IddCx capacity, not a measured GPU monitor-count limit.
inline constexpr std::uint32_t kConnectorCapacity = 16;
inline constexpr std::uint32_t kMaxModesPerMonitor = 4096;
inline constexpr std::uint32_t kNoConnector = UINT32_MAX;
inline constexpr std::uint32_t kGpuMonitorLimitUnknown = 1;

inline constexpr std::uint32_t kDeviceType = 0x8000;
inline constexpr std::uint32_t kIoctlAccess = 3; // FILE_READ_DATA | FILE_WRITE_DATA
inline constexpr std::uint32_t kIoctlMethod = 0; // METHOD_BUFFERED

constexpr std::uint32_t ControlCode(std::uint32_t function) noexcept {
    return (kDeviceType << 16) | (kIoctlAccess << 14) | (function << 2) | kIoctlMethod;
}

inline constexpr std::uint32_t kIoctlQuery = ControlCode(0x800);
inline constexpr std::uint32_t kIoctlList = ControlCode(0x801);
inline constexpr std::uint32_t kIoctlAdd = ControlCode(0x802);
inline constexpr std::uint32_t kIoctlRemove = ControlCode(0x803);
inline constexpr std::uint32_t kIoctlOwnedList = ControlCode(0x804);

#ifdef _WIN32
inline constexpr GUID kControlInterfaceGuid = {
    0x3b1a9721, 0xeba5, 0x4125, {0xb0, 0x21, 0xdf, 0xce, 0xc0, 0x5e, 0x35, 0xb0}
};
#endif

struct Mode {
    std::uint32_t width;
    std::uint32_t height;
    std::uint32_t refreshRateUnits;
};

// METHOD_BUFFERED input is this header followed by modeCount inline Mode values.
// Query, List, OwnedList and Remove require zero modes; Add requires at least one.
struct RequestHeader {
    std::uint32_t version;
    std::uint32_t size;
    std::uint32_t connectorId;
    std::uint32_t modeCount;
};

// Counts reflect completed IDDCX arrival/departure, not CCD-active desktops.
struct QueryResponse {
    std::uint32_t version;
    std::uint32_t size;
    std::uint32_t connectorCapacity;
    std::uint32_t connectedCount;
    std::uint32_t maxModesPerMonitor;
    std::uint32_t flags;
};

struct ListResponse {
    std::uint32_t version;
    std::uint32_t size;
    std::uint32_t monitorCount;
    std::uint32_t recordSize;
};

// Arrival/departure inventory; preferredMode does not claim CCD active mode.
struct MonitorRecord {
    std::uint32_t connectorId;
    std::uint32_t modeCount;
    Mode preferredMode;
};

struct OperationResponse {
    std::uint32_t version;
    std::uint32_t size;
    std::uint32_t connectorId;
};

static_assert(sizeof(Mode) == 12 && alignof(Mode) == 4);
static_assert(sizeof(RequestHeader) == 16 && offsetof(RequestHeader, modeCount) == 12);
static_assert(sizeof(QueryResponse) == 24);
static_assert(sizeof(ListResponse) == 16);
static_assert(sizeof(MonitorRecord) == 20 && offsetof(MonitorRecord, preferredMode) == 8);
static_assert(sizeof(OperationResponse) == 12);
static_assert(std::is_standard_layout_v<Mode> && std::is_trivially_copyable_v<Mode>);
static_assert(std::is_standard_layout_v<RequestHeader> && std::is_trivially_copyable_v<RequestHeader>);
static_assert(kIoctlQuery == 0x8000e000 && kIoctlRemove == 0x8000e00c);
static_assert(kIoctlOwnedList == 0x8000e010);

constexpr bool RequestSizeForModes(std::uint32_t count, std::uint32_t& size) noexcept {
    if (count > kMaxModesPerMonitor) {
        return false;
    }
    const auto bytes = std::uint64_t{sizeof(RequestHeader)} + std::uint64_t{count} * sizeof(Mode);
    if (bytes > (std::numeric_limits<std::uint32_t>::max)()) {
        return false;
    }
    size = static_cast<std::uint32_t>(bytes);
    return true;
}

constexpr bool ListSizeForMonitors(std::uint32_t count, std::uint32_t& size) noexcept {
    if (count > kConnectorCapacity) {
        return false;
    }
    const auto bytes = std::uint64_t{sizeof(ListResponse)} + std::uint64_t{count} * sizeof(MonitorRecord);
    if (bytes > (std::numeric_limits<std::uint32_t>::max)()) {
        return false;
    }
    size = static_cast<std::uint32_t>(bytes);
    return true;
}

inline bool ReadRequestHeader(const void* buffer, std::size_t bytes, RequestHeader& output) noexcept {
    if (buffer == nullptr || bytes < sizeof(RequestHeader)) {
        return false;
    }
    RequestHeader header{};
    std::memcpy(&header, buffer, sizeof(header));
    std::uint32_t expected{};
    if (header.version != kVersion || !RequestSizeForModes(header.modeCount, expected) ||
        header.size != expected || bytes != expected) {
        return false;
    }
    output = header;
    return true;
}

inline bool ReadRequestMode(const void* buffer, std::size_t bytes, std::uint32_t index,
                            Mode& output) noexcept {
    RequestHeader header{};
    if (!ReadRequestHeader(buffer, bytes, header) || index >= header.modeCount) {
        return false;
    }
    const auto offset = sizeof(RequestHeader) + std::size_t{index} * sizeof(Mode);
    std::memcpy(&output, static_cast<const std::uint8_t*>(buffer) + offset, sizeof(output));
    return true;
}

} // namespace xscc::protocol
