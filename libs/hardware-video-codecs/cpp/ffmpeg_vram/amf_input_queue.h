#ifndef XSCC_AMF_INPUT_QUEUE_H
#define XSCC_AMF_INPUT_QUEUE_H

#include <array>
#include <cstddef>
#include <cstdint>

constexpr bool amf_high_rate_hevc(int framerate, bool hevc) {
  return hevc && framerate >= 240;
}

constexpr unsigned int amf_pending_frame_limit(int framerate, bool hevc) {
  return amf_high_rate_hevc(framerate, hevc) ? 3 : 2;
}

class AmfInputQueue {
public:
  static constexpr std::size_t capacity = 2;
  struct Frame { std::size_t slot; int64_t pts; };

  template<class Ready>
  int available(Ready ready) const {
    if (count_ == capacity) return -1;
    for (std::size_t offset = 0; offset < capacity; ++offset) {
      const auto slot = (next_ + offset) % capacity;
      bool queued = false;
      for (std::size_t i = 0; i < count_; ++i)
        queued |= frames_[(head_ + i) % capacity].slot == slot;
      if (!queued && ready(slot)) return static_cast<int>(slot);
    }
    return -1;
  }
  bool publish(std::size_t slot, int64_t pts) {
    if (slot >= capacity || count_ == capacity) return false;
    for (std::size_t i = 0; i < count_; ++i)
      if (frames_[(head_ + i) % capacity].slot == slot) return false;
    frames_[(head_ + count_) % capacity] = {slot, pts};
    ++count_;
    next_ = (slot + 1) % capacity;
    latest_ = static_cast<int>(slot);
    return true;
  }
  const Frame *front() const {
    return count_ ? &frames_[head_] : nullptr;
  }
  void consumed() {
    if (count_) { head_ = (head_ + 1) % capacity; --count_; }
  }
  int latest() const { return latest_; }

  int reservable_capture() const {
    if (capture_slot_ >= 0) return -1;
    return available([&](std::size_t slot) { return static_cast<int>(slot) != replay_slot_; });
  }
  int reserve_capture() {
    const int slot = reservable_capture();
    if (slot >= 0) capture_slot_ = slot;
    return slot;
  }
  int release_capture() {
    const int slot = capture_slot_;
    capture_slot_ = -1;
    return slot;
  }
  int begin_replay() {
    if (front() || latest_ < 0 || latest_ == capture_slot_ || replay_slot_ >= 0) return -1;
    replay_slot_ = latest_;
    return replay_slot_;
  }
  void end_replay() { replay_slot_ = -1; }
private:
  std::array<Frame, capacity> frames_{};
  std::size_t head_ = 0, count_ = 0, next_ = 0;
  int latest_ = -1;
  int capture_slot_ = -1;
  int replay_slot_ = -1;
};

#endif
