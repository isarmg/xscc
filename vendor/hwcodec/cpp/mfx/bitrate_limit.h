#ifndef XSCC_MFX_BITRATE_LIMIT_H
#define XSCC_MFX_BITRATE_LIMIT_H

#include <cstdint>
#include <limits>
#include <mfxstructures.h>

namespace xscc_bitrate {

constexpr uint32_t multiplier_for(uint32_t kbps) {
  return uint32_t((uint64_t(kbps) + 65534) / 65535);
}

constexpr uint32_t target_for(uint32_t kbps) {
  return kbps ? kbps / multiplier_for(kbps) : 0;
}

static_assert(multiplier_for(65535) == 1 && target_for(65535) == 65535,
              "16-bit maximum must remain exact");
static_assert(multiplier_for(65536) == 2 && target_for(65536) == 32768,
              "first wide bitrate must not wrap");
static_assert(multiplier_for(1000000) == 16 && target_for(1000000) == 62500 &&
              uint64_t(target_for(1000000)) * multiplier_for(1000000) == 1000000,
              "one gigabit must be representable without exceeding its limit");

inline uint32_t multiplier(const mfxInfoMFX &info) {
  return info.BRCParamMultiplier ? info.BRCParamMultiplier : 1;
}

inline uint64_t target_kbps(const mfxInfoMFX &info) {
  return uint64_t(info.TargetKbps) * multiplier(info);
}

inline uint64_t maximum_kbps(const mfxInfoMFX &info) {
  return uint64_t(info.MaxKbps) * multiplier(info);
}

inline bool same_format(const mfxInfoMFX &a, const mfxInfoMFX &b) {
  return a.CodecId == b.CodecId && a.CodecProfile == b.CodecProfile &&
         a.CodecLevel == b.CodecLevel &&
         a.FrameInfo.Width == b.FrameInfo.Width &&
         a.FrameInfo.Height == b.FrameInfo.Height &&
         a.FrameInfo.CropX == b.FrameInfo.CropX &&
         a.FrameInfo.CropY == b.FrameInfo.CropY &&
         a.FrameInfo.CropW == b.FrameInfo.CropW &&
         a.FrameInfo.CropH == b.FrameInfo.CropH;
}

inline bool set_limit(mfxInfoMFX &info, uint32_t kbps) {
  constexpr uint32_t max_field = std::numeric_limits<mfxU16>::max();
  if (!kbps || kbps > uint32_t(std::numeric_limits<int32_t>::max() / 1000))
    return false;
  const uint32_t scale = multiplier_for(kbps);
  const uint32_t target = target_for(kbps);
  const uint64_t buffer = uint64_t(info.BufferSizeInKB) * multiplier(info) / scale;
  const uint64_t delay = uint64_t(info.InitialDelayInKB) * multiplier(info) / scale;
  if (!target || target > max_field || buffer > max_field || delay > max_field ||
      (info.BufferSizeInKB && !buffer) || (info.InitialDelayInKB && !delay))
    return false;
  info.BRCParamMultiplier = mfxU16(scale);
  info.TargetKbps = info.MaxKbps = mfxU16(target);
  info.BufferSizeInKB = mfxU16(buffer);
  info.InitialDelayInKB = mfxU16(delay);
  return true;
}

} // namespace xscc_bitrate

#endif
