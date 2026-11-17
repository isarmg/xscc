# 2026-11-17 session pipeline handoff

Further changes and performance testing stopped at the maintainer's request. Android sustained high-refresh performance is **not accepted**. This record consolidates retained code, rejected experiments, installed versions and measurement limits; it is not a completion claim.

## Final retained implementation

- Windows host: two owned AMF BGRA input slots, keyed-mutex ownership, bounded encoder admission before consuming another capture, nonblocking output polling, prepared-input retention across codec backpressure, static-tail output handling, and the existing GPU retirement fences. Capture textures are copied while owned, not retained after Desktop Duplication releases them. The original native compiler standard is retained; the failed global C++17 build flag was removed.
- Windows capture: normal WouldBlock/no-new-frame conditions no longer select GDI for GPUI. Genuine capture errors retain recovery/fallback. A successful DXGI frame clears the fallback marker after any recovery attempt. Repeated 90/144/240/120 mode changes were observed without the earlier 20-50FPS GDI/RAM degradation; this does not establish complete end-to-end performance acceptance.
- Android: choose Mailbox only when actual surface capabilities report support, preserve an explicit supported preference, otherwise retain Fifo. All attempted presentation-worker/image-pool/device-progress lifecycle changes are removed. The final Android renderer/window/frame-clock/image-upload source matches beta.19; packaging versionCode is 40021.

## Rejected experiments and measured limits

| Candidate / workload | Receive / decode / UI median FPS | Actual SurfaceFlinger observations | Result |
| --- | --- | --- | --- |
| Host17 + Android14, return to 1080p120 | 118 / 118 / 89 | Earlier closed-menu windows about 89Hz | Existing UI bottleneck |
| Android18, owned-image worker, 1080p120 minute | 114.5 / 113.5 / 88 | Steady 88.95 / 89.48 / 88.96Hz | Rejected and removed |
| Android19, Mailbox, 1080p120 minute | 116.5 / 116 / 105.5 | Short windows about 95-117Hz | Improvement was not sustained acceptance |
| Android19, after 90 / 144 / 240 / 120 switches | UI 88 / 88 / 89 / 89 | Not accepted as a stable high-refresh fix | Still unresolved |
| Android20, worker with completion poll moved off UI, 1080p120 minute | 99.5 / 99 / 86 | 86.87 / 87.89 / 88.93Hz | Rejected and removed |
| Android21 | Not retested after replacement | Mailbox startup configuration verified | Removes failed worker; not a new performance claim |

In beta.18, native present wait moved into the next image-upload poll. Pinned wgpu Queue::present holds command_indices exclusively; even PollType::Poll reads that lock. Beta.20 removed that upload wait (median upload 0.81ms), but final queue submission then waited and the worker cycle remained about 10.63ms. Moving the same serialized work between threads did not solve the reported problem. Both implementations are absent from the final tree.

A 15-second beta.19 CPU sample attributed approximately 31% of sampled cycles to NV12ToARGBRow_NEON and 14.6% to UI-thread memcpy. These identify costs, not proof that changing either would resolve the remaining frame-rate limit. Native decoder-surface output was investigated but **not implemented**.

The machine/scene was not held invariant across all runs. Some samples include source/transport stalls and notifications. UI acceptance counts differ from GPU completion and scanout. Aggregate data and full short-window distributions are in [the metrics record](2026-11-17-session-pipeline-metrics.json). Do not replace them with a peak-FPS completion claim.

## Delivered machine state

- Installed Windows host: 4.0.0-beta.17, source `00e5999a269c1e7067692ff0e007b7e93a40fa17`; service and executable identity verified. Windows controller was not updated in this round.
- Installed Xiaomi13 app: 4.0.0-beta.21 / 40021, source `74bf4a30e61279d328dc1cde0c95b409c7894987`. Artifact digest, APK digest, stable signing certificate and installed package version were checked. Replacement installation retained application data and binding. This replaces the rejected beta.20 runtime.
- Existing DISPLAY40 was reused. Last native read was 1920x1080@120Hz; physical DISPLAY1 remained primary at 60Hz. No virtual display was created/removed and no driver was replaced in this round. Virtual-display create/remove stability was not reaccepted.
- USB-test stay-awake was restored to the original disabled value. No new native-surface/codec/interface work is included.

## Validation and regression surface

AMF CPU regression commands passed: `.github/scripts/test-gpui-amf-keyframe.py --cc clang --cxx clang++` and `.github/scripts/test-gpui-gpu-retirement.py --cxx clang++`. The Windows beta.17 host/controller product build succeeded (run 44013831855); the Android beta.21 smoke/management product build succeeded (run 44026991297). Android performance acceptance and the remaining background/resume/orientation matrix are incomplete. Successful compilation is not performance acceptance.

Existing runtime files changed: Windows `src/server/video_service.rs` (capture admission and false fallback), `libs/scrap/src/common/codec.rs` / `vram.rs` (thin encoder readiness forwarding), `vendor/hwcodec/cpp/ffmpeg_vram/ffmpeg_vram_encode.cpp` / `ffmpeg_vram_ffi.h` and `vendor/hwcodec/src/vram/encode.rs` (owned AMF inputs and polling), and Android `vendor/gpui/crates/gpui_wgpu/src/wgpu_renderer.rs` (capability-based default present mode). The new AMF queue header owns bounded slot metadata. Packaging changes only advance Android versionCode. The AMF regression script and Windows workflow run the corresponding CPU regressions. No other platform's presentation implementation is changed.
