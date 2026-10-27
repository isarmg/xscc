# Native session performance

## Report and baseline

The reported case is low frame rate during changing content, including a Xiaomi 13
using VP9. The previously installed package is beta.11, source
`3e280168702fe292278ae7ac922a30f991d8074a`.

A Windows self-connection requested 60 FPS and displayed approximately 30 received
and submitted FPS at 1920x1080, with about 3 ms network RTT. Its desktop content was
not a verified 60 FPS source. This observation does **not** establish a 30 FPS
hardware ceiling. A movie containing 30 unique frames per second is not a valid
test of a 60 FPS capture limit.

The existing Host logs confirm earlier single-viewer VPX/VP9 sessions with one
codec worker. The later desktop connection negotiated H.265, advertised both
H.264/H.265, and received the 60 FPS streaming request. The local hardware probe
reported `h264_amf` and `hevc_amf` encoders. The collected evidence does not show
another concurrent viewer reducing that session's rate, or justify globally
changing QoS or codec thread limits. A process CPU snapshot taken while the
desktop was mostly idle is not a video performance benchmark.

## Corrections

* Android native session packages enable MediaCodec. Startup reads Android's
  REGULAR_CODECS inventory through JNI, accepts hardware AVC/HEVC decoders with
  NV12 output supported by the existing decoder, and initializes the existing
  negotiated capability flags. Unsupported layouts are not advertised. The old
  Java FFI inventory entry remains unchanged. JNI errors retain their cause and
  leave software codecs available; existing decoder failure renegotiation remains.
* Android rendering is paced by the system Choreographer rather than adding a
  fixed 16.7 ms delay after synchronous drawing. The next callback is scheduled
  before drawing so FIFO presentation cannot add another refresh period. Only
  dirty, drawable windows request frames. Pause, detach and destruction cancel
  pending signals. Late native callbacks carry an opaque token and a weak signal,
  never a pointer to the window. Idle windows have no continuous callback loop.
  The existing synchronous RedrawNeeded acknowledgement remains in place.
* A mobile connection explicitly requests 60 FPS using the existing permission
  checked API, as the desktop client already does. Users can still change the
  target. The requested target is not reported as measured throughput.
* Normal desktop GUI startup initializes the existing once-only log facility.
  Existing wall-clock quality snapshots emit one numeric `session_pipeline` JSON
  record per sample interval. No new timer, per-frame log or session identifier is
  added. The record distinguishes received, decoded and submitted frames, decode
  percentiles, UI queue latency and mailbox replacements.

MediaCodec availability is only a capability probe. Actual selected format,
successful frame decoding and measured frame rate must also be checked on the
device. Submission to the GUI is not proof of scanout or completed presentation.
These changes do not change bitrate limits, remote input coordinates, binding
storage, access policy, transport authentication or virtual display configuration.

## Runtime verification

Use the same 1920x1080 display, quality and bitrate cap before and after. Use a
verified 60 FPS animation, not a static desktop or a 30 FPS movie. Test desktop
alone, phone alone, both together and the remaining viewer after one disconnects.
Record the actual codec/backend, requested and effective FPS, received/decoded/
submitted FPS, decode p50/p95, UI queue p95 and drops. Where available, compare
completed presentation separately with PresentMon/ETW or Android frame timelines.

The existing diagnostic animation can provide changing content using
`--diagnostic --width 960 --height 540 --fps 60 --seconds 15 --report <fresh.json>`.
Keep that producer window foreground: its animation callback is throttled when
inactive. Check its source and submitted rates. Its report measures the producer,
not a remote session; measure the managed connection separately.

Android compile/smoke checks and host scene tests do not establish physical phone
performance. Physical device results and before/after numbers belong in the
delivery verification record; do not infer 60 FPS merely from a successful build.

## Regression surface

| Existing file | Changed runtime path and why it is necessary |
| --- | --- |
| `experiments/gpui-mobile-management/Cargo.toml` | Android session feature graph enables its existing native decoder; iOS retains its previous feature set. |
| `experiments/gpui-mobile-management/Cargo.lock` | Add only the existing `ndk 0.7.0` dependency edge to scrap; no package version or checksum changes. |
| `experiments/gpui-mobile-management/src/android.rs` | Initialize real decoder inventory while the Android VM is available, before connection negotiation. |
| `experiments/gpui-mobile-management/src/session_view.rs` | Connected event sends the default 60 FPS request through the existing authorized API. |
| `libs/scrap/src/android/ffi.rs` | Typed inventory setter for the native startup path; existing Java entry is retained. |
| `libs/scrap/src/android/mod.rs` | Feature gated export of the Android inventory module. |
| `src/gpui_session.rs` | Thin Android export and numeric logging from the existing quality snapshot; no new sampling clock. |
| `src/gpui_host.rs` | Normal native GUI logger initialization, using the existing once-only initializer. |
| `vendor/gpui/crates/gpui_android/src/application.rs` | Replace the extra post-draw delay with dirty-window vsync and preserve lifecycle cleanup. |
| `vendor/gpui/crates/gpui_android/src/gpui_android.rs` | Register the Android-only frame clock module. |
| `.github/workflows/gpui-android-smoke.yml` | Require MediaCodec in the actual Android production graph while still rejecting Flutter, FFmpeg and host capture capabilities. |
| `.github/workflows/gpui-windows-beta.yml` | Label rebuilt observable desktop packages beta.12. |
| `.github/workflows/gpui-unix-beta.yml` | Label the matching native desktop packages beta.12. |
| `packaging/gpui-android/app/build.gradle` | Version code/name for a same-signature beta.12 upgrade. |

New implementation modules are `libs/scrap/src/android/decoder_inventory.rs` and
`vendor/gpui/crates/gpui_android/src/frame_clock.rs`. No new third party dependency declaration or test framework is introduced.
The Android path enables its previously declared NDK dependency.

Platform references: [MediaCodecInfo](https://developer.android.com/reference/android/media/MediaCodecInfo),
[MediaCodecList](https://developer.android.com/reference/android/media/MediaCodecList),
[NDK Choreographer](https://developer.android.com/ndk/reference/group/choreographer).
