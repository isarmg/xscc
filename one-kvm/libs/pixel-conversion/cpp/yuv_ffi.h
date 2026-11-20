#ifndef YUV_FFI_H
#define YUV_FFI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int YUY2ToI420(const uint8_t* src_yuy2, int src_stride_yuy2,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int YUY2ToNV12(const uint8_t* src_yuy2, int src_stride_yuy2,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_uv, int dst_stride_uv,
               int width, int height);

int UYVYToI420(const uint8_t* src_uyvy, int src_stride_uyvy,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int I422ToI420(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_u, int src_stride_u,
               const uint8_t* src_v, int src_stride_v,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int I444ToI420(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_u, int src_stride_u,
               const uint8_t* src_v, int src_stride_v,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int I420ToNV12(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_u, int src_stride_u,
               const uint8_t* src_v, int src_stride_v,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_uv, int dst_stride_uv,
               int width, int height);

int NV12ToI420(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_uv, int src_stride_uv,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int NV21ToI420(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_vu, int src_stride_vu,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_u, int dst_stride_u,
               uint8_t* dst_v, int dst_stride_v,
               int width, int height);

int NV21ToNV12(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_vu, int src_stride_vu,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_uv, int dst_stride_uv,
               int width, int height);

void SplitUVPlane(const uint8_t* src_uv, int src_stride_uv,
                  uint8_t* dst_u, int dst_stride_u,
                  uint8_t* dst_v, int dst_stride_v,
                  int width, int height);

int RGB24ToI420(const uint8_t* src_rgb24, int src_stride_rgb24,
                uint8_t* dst_y, int dst_stride_y,
                uint8_t* dst_u, int dst_stride_u,
                uint8_t* dst_v, int dst_stride_v,
                int width, int height);

int RAWToI420(const uint8_t* src_raw, int src_stride_raw,
              uint8_t* dst_y, int dst_stride_y,
              uint8_t* dst_u, int dst_stride_u,
              uint8_t* dst_v, int dst_stride_v,
              int width, int height);

int RGB24ToNV12(const uint8_t* src_rgb24, int src_stride_rgb24,
                uint8_t* dst_y, int dst_stride_y,
                uint8_t* dst_uv, int dst_stride_uv,
                int width, int height);

int H444ToARGB(const uint8_t* src_y, int src_stride_y,
               const uint8_t* src_u, int src_stride_u,
               const uint8_t* src_v, int src_stride_v,
               uint8_t* dst_argb, int dst_stride_argb,
               int width, int height);

int MJPGToNV12(const uint8_t* sample, size_t sample_size,
               uint8_t* dst_y, int dst_stride_y,
               uint8_t* dst_uv, int dst_stride_uv,
               int src_width, int src_height,
               int dst_width, int dst_height);

int MJPGSize(const uint8_t* sample, size_t sample_size,
             int* width, int* height);

#ifdef __cplusplus
}
#endif

#endif
