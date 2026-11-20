#ifndef XSCC_GPU_RETIREMENT_H
#define XSCC_GPU_RETIREMENT_H

#include <algorithm>
#include <chrono>

// Every phase receives this same deadline. A driver call which itself never
// returns cannot be cancelled safely here; the capture guard must stay non-idle.
using GpuRetirementDeadline = std::chrono::steady_clock::time_point;

inline GpuRetirementDeadline gpu_retirement_deadline(int timeout_ms) {
  return std::chrono::steady_clock::now() +
         std::chrono::milliseconds((std::max)(0, (std::min)(timeout_ms, 1500)));
}

inline bool gpu_retirement_expired(GpuRetirementDeadline deadline) {
  return std::chrono::steady_clock::now() >= deadline;
}

inline int drain_encoder_for_retirement(AVCodecContext *codec, AVPacket *packet,
                                        bool &drain_sent,
                                        GpuRetirementDeadline deadline) {
  if (!codec || !packet)
    return AVERROR(EINVAL);
  if (codec->codec && codec->codec->name &&
      strstr(codec->codec->name, "_amf")) {
    // The pinned AMF patch applies this live value to the actual component
    // before QueryOutput, including a previously synchronous <=60 FPS encoder.
    int ret = av_opt_set_int(codec->priv_data, "query_timeout", 0, 0);
    if (ret < 0)
      return ret;
  }
  for (;;) {
    if (gpu_retirement_expired(deadline))
      return AVERROR(ETIMEDOUT);
    if (!drain_sent) {
      int ret = avcodec_send_frame(codec, nullptr);
      if (ret >= 0 || ret == AVERROR_EOF)
        drain_sent = true;
      else if (ret != AVERROR(EAGAIN))
        return ret;
    }
    if (gpu_retirement_expired(deadline))
      return AVERROR(ETIMEDOUT);
    int ret = avcodec_receive_packet(codec, packet);
    if (ret == AVERROR_EOF)
      return 0; // AMF reaches this only after Drain + QueryOutput == AMF_EOF.
    if (ret >= 0) {
      av_packet_unref(packet); // Retired tail belongs to the old stream.
    } else if (ret != AVERROR(EAGAIN)) {
      return ret;
    } else {
      Sleep(1);
    }
  }
}

inline int wait_context_for_retirement(ID3D11Device *device,
                                       ID3D11DeviceContext *context,
                                       ComPtr<ID3D11Query> &event,
                                       GpuRetirementDeadline deadline) {
  if (!device || !context)
    return AVERROR(EINVAL);
  if (gpu_retirement_expired(deadline))
    return AVERROR(ETIMEDOUT);
  D3D11_QUERY_DESC desc{D3D11_QUERY_EVENT, 0};
  HRESULT hr = S_OK;
  if (!event) {
    hr = device->CreateQuery(&desc, event.GetAddressOf());
    if (FAILED(hr))
      return static_cast<int>(hr);
    if (!event)
      return AVERROR(EIO);
    context->End(event.Get());
    context->Flush(); // Submission only; GetData below proves completion.
  }
  for (;;) {
    if (gpu_retirement_expired(deadline))
      return AVERROR(ETIMEDOUT);
    BOOL complete = FALSE;
    hr = context->GetData(event.Get(), &complete, sizeof(complete),
                          D3D11_ASYNC_GETDATA_DONOTFLUSH);
    if (FAILED(hr))
      return static_cast<int>(hr);
    HRESULT removed = device->GetDeviceRemovedReason();
    if (FAILED(removed))
      return static_cast<int>(removed);
    if (hr == S_OK && complete)
      return 0;
    Sleep(1);
  }
}

inline int wait_capture_for_retirement(
    ID3D11Device *device, ComPtr<ID3D11DeviceContext> &context,
    ComPtr<ID3D11Query> &event, GpuRetirementDeadline deadline) {
  if (!device)
    return 0; // Caller explicitly identified a CPU/GDI capture.
  if (!context)
    device->GetImmediateContext(context.GetAddressOf());
  return wait_context_for_retirement(device, context.Get(), event, deadline);
}

#endif
