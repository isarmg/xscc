#ifndef XSCC_AMF_INPUT_QUEUE_H
#define XSCC_AMF_INPUT_QUEUE_H

#include <array>
#include <cstddef>
#include <cstdint>
#include <optional>

class AmfInputQueue {
public:
  static constexpr std::size_t capacity = 2;
  struct Frame { std::size_t slot; int64_t pts; };

  template<class Ready>
  std::optional<std::size_t> available(Ready ready) const {
    if (count_ == capacity) return std::nullopt;
    for (std::size_t offset = 0; offset < capacity; ++offset) {
      const auto slot = (next_ + offset) % capacity;
      bool queued = false;
      for (std::size_t i = 0; i < count_; ++i)
        queued |= frames_[(head_ + i) % capacity].slot == slot;
      if (!queued && ready(slot)) return slot;
    }
    return std::nullopt;
  }
  bool publish(std::size_t slot, int64_t pts) {
    if (slot >= capacity || count_ == capacity) return false;
    for (std::size_t i = 0; i < count_; ++i)
      if (frames_[(head_ + i) % capacity].slot == slot) return false;
    frames_[(head_ + count_) % capacity] = {slot, pts};
    ++count_;
    next_ = (slot + 1) % capacity;
    latest_ = slot;
    return true;
  }
  std::optional<Frame> front() const {
    return count_ ? std::optional<Frame>(frames_[head_]) : std::nullopt;
  }
  void consumed() {
    if (count_) { head_ = (head_ + 1) % capacity; --count_; }
  }
  std::optional<std::size_t> latest() const { return latest_; }
private:
  std::array<Frame, capacity> frames_{};
  std::size_t head_ = 0, count_ = 0, next_ = 0;
  std::optional<std::size_t> latest_;
};

#endif
