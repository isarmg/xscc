#ifndef XSCC_AMF_KEYFRAME_REQUEST_H
#define XSCC_AMF_KEYFRAME_REQUEST_H

// Accessed only by the video encoder's owner thread. Repeated refreshes coalesce;
// a failed/EAGAIN submission must retain the request for the next input.
class AmfKeyframeRequest {
public:
  bool request(bool supported) {
    if (supported)
      pending_ = true;
    return supported;
  }
  bool pending() const { return pending_; }
  void submitted(int result) {
    if (result >= 0)
      pending_ = false;
  }

private:
  bool pending_ = false;
};

#endif
