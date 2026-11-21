#ifndef XSCC_AMF_WORKER_H
#define XSCC_AMF_WORKER_H

#include <array>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <memory>
#include <mutex>
#include <thread>
#include <utility>
#include <windows.h>

class AmfWorker {
  class Event {
  public:
    explicit Event(HANDLE handle) : handle_(handle) {}
    ~Event() {
      if (handle_) CloseHandle(handle_);
    }
    Event(const Event &) = delete;
    Event &operator=(const Event &) = delete;
    HANDLE get() const { return handle_; }

  private:
    HANDLE handle_;
  };

  struct PacketDeleter {
    void operator()(AVPacket *packet) const { av_packet_free(&packet); }
  };
  using Packet = std::unique_ptr<AVPacket, PacketDeleter>;
  static constexpr std::size_t packet_limit = 8;

  Event wake_{CreateEventW(nullptr, FALSE, FALSE, nullptr)};
  Event finished_{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
  Event timer_{CreateWaitableTimerExW(nullptr, nullptr, 0x2,
                                    TIMER_MODIFY_STATE | SYNCHRONIZE)};
  std::thread thread_;
  std::atomic<bool> stop_{false};
  std::array<Packet, packet_limit> packets_;
  std::size_t packet_count_ = 0;

public:
  // Protect slot ownership, published frames and packets. Driver calls never hold it.
  std::mutex mailbox;
  std::mutex codec_control;
  std::atomic<bool> keyframe{false};
  std::atomic<int64_t> replay_pts{-1};

  ~AmfWorker() { join(); }

  void join() {
    request_stop();
    if (thread_.joinable()) thread_.join();
  }

  template <typename Run>
  bool start(Run run) {
    if (!wake_.get() || !finished_.get() || !timer_.get()) return false;
    thread_ = std::thread([this, run] {
      run();
      SetEvent(finished_.get());
    });
    return true;
  }

  void notify() { SetEvent(wake_.get()); }

  bool stopping() const { return stop_.load(); }

  bool wait(bool pending) {
    if (!pending) return WaitForSingleObject(wake_.get(), INFINITE) == WAIT_OBJECT_0;
    LARGE_INTEGER delay{};
    delay.QuadPart = -3500;
    if (!SetWaitableTimer(timer_.get(), &delay, 0, nullptr, nullptr, FALSE)) return false;
    const HANDLE ready[]{wake_.get(), timer_.get()};
    return WaitForMultipleObjects(2, ready, FALSE, INFINITE) != WAIT_FAILED;
  }

  bool stop(std::chrono::steady_clock::time_point deadline) {
    request_stop();
    if (!thread_.joinable()) return true;
    const auto remaining = deadline - std::chrono::steady_clock::now();
    const auto wait_ms = remaining <= std::chrono::steady_clock::duration::zero()
        ? 0 : std::chrono::duration_cast<std::chrono::milliseconds>(remaining).count();
    if (WaitForSingleObject(finished_.get(), static_cast<DWORD>(wait_ms)) != WAIT_OBJECT_0)
      return false;
    thread_.join();
    return true;
  }

  bool packet_space() {
    std::lock_guard<std::mutex> guard(mailbox);
    return packet_count_ < packet_limit;
  }

  bool publish_packet(const AVPacket *packet) {
    Packet owned(av_packet_clone(packet));
    if (!owned) return false;
    std::lock_guard<std::mutex> guard(mailbox);
    // Only the codec worker publishes packets. The consumer can only free space.
    if (packet_count_ == packet_limit) return false;
    packets_[packet_count_++] = std::move(owned);
    return true;
  }

  void collect_packets(EncodeCallback callback, void *obj) {
    std::array<Packet, packet_limit> ready;
    std::size_t count;
    {
      std::lock_guard<std::mutex> guard(mailbox);
      if (!packet_count_) return;
      count = packet_count_;
      packet_count_ = 0;
      packets_.swap(ready);
    }
    notify();
    if (!callback) return;
    for (std::size_t i = 0; i < count; ++i) {
      const auto &packet = ready[i];
      callback(packet->data, packet->size, packet->flags & AV_PKT_FLAG_KEY, obj, packet->pts);
    }
  }

private:
  void request_stop() {
    stop_.store(true);
    if (wake_.get()) notify();
  }
};

#endif
