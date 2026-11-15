# Native session performance

## 2026-11-15: bound UDP production to initial transmission completion

On the Xiaomi 13, the 1080p/120 Hz/8 Mbps session configured Qualcomm HEVC at
120 FPS, but Android recorded only 784 decoder outputs over 51.307 seconds.
During the same session the host produced and enqueued 115–119 frames/s for
several seconds. Enqueue counts do not prove delivery. A physical-screen
comparison received and decoded approximately 60 frames/s with the phone display
running at 120 Hz; this does not validate the high-rate virtual-screen case.

The v4 UDP publication path excluded its producer from the existing frame-fetched
barrier. At 120 FPS, a large initial keyframe filled the two-frame queue; admitted
deltas could also expire behind that keyframe. Dropping reference frames forced
another keyframe and repeated the cycle. A real loopback socket regression at
8 Mbps with a 300 KB initial keyframe and 4 KB deltas produced 361 frames but
delivered only 9 over three seconds before the correction.

The host now participates in the existing producer barrier. Its UDP sender
releases that barrier after a frame's initial transmission completes or is
discarded, rather than at enqueue or dequeue. Retransmissions do not release a
subsequent frame's barrier. The congestion estimator also recognizes this
waiting producer without requiring an already queued successor, preserving
encoder bandwidth feedback on a constrained path. Frames discarded before UDP readiness, full queue
admission and format replacement also release the producer. The old queue bound,
wire bitrate ceiling, delta deadlines, authenticated delivery and keyframe gap
checks remain. This prevents production of dependent frames while the initial
keyframe is still being serialized; it does not grow the queue or weaken loss
recovery. The same regression delivered 313 of 314 produced frames with zero
refresh requests over three seconds after the correction. Those are transport
fixture measurements, not an Android high-refresh acceptance result.

Changed runtime paths: `crates/session-transport/src/lib.rs` adds the host
producer's release notification at initial-send completion/discard;
`src/server/connection.rs` registers that notification and restores participation
in the existing capture/encode barrier. TCP-only sending and client decode/render
behavior are unchanged. No primary-display change or driver installation is part
of this correction.

## 2026-11-04: Windows virtual-display content cadence

On this Windows host, VSync paced native drawing near 60 FPS with a 60 Hz physical
primary, although the owned virtual output's VBlank measured 120 Hz. Temporarily
making that virtual output primary produced 119.5 submitted frames/s and 117.7
DesktopDup updates/s. These are local native drawing/capture measurements, not a
browser UFO test or Android end-to-end acceptance.

As requested on 2026-11-15, session display selection and mode changes no longer
automatically promote a virtual display to primary. Adding or selecting a virtual
display must preserve the user's primary display. The measurements above describe
the former workaround, not a guarantee of high-rate content on a secondary output.
Capture-idle, GPU retirement, mode-change and temporary-display cleanup guards
remain in place. High-refresh transport performance is investigated separately.

## 2026-11-02: measured Android/Windows follow-up

A USB-connected Xiaomi 13 running beta.18 used the Qualcomm HEVC hardware decoder.
With a separate continuously changing Windows fixture painting 61–64 FPS, the
installed Host requested 60 FPS but produced 30–31 fresh frames/s. Its synchronous
capture averaged 16–18 ms and combined encode/enqueue averaged 13–14 ms. A retained
2.5-second Android native trace counted 75 decoded outputs (30 FPS) and 152 GPU
present submissions (60.8/s). GPU submissions are not physical scanout measurements.

The Windows RAM DXGI path now uses two owned staging textures to overlap GPU
readback with encoding. It preserves the acquired desktop texture lifetime,
bounded storage, static first/final-frame delivery, and existing GDI/system-memory/
VRAM paths. No encoded queue growth, authorization relaxation, or QoS bypass is
introduced. Numeric per-second metrics separate capture acquire/copy/map and
encoding/enqueue, with actual backend fields.

Android ordinary clean-frame callbacks no longer force a duplicate presentation.
Native forced redraw, new surfaces and failed-presentation retries remain. The
remote session holds the screen awake and restores the previous window flag on
exit. Decoder timing did not establish a codec stall, so this change leaves
MediaCodec negotiation and decode timeouts intact.

Changed existing paths are Windows RAM capture in `libs/scrap/src/dxgi/mod.rs`,
streaming backend diagnostics and per-second Host stage measurements in
`src/server/video_service.rs` and `src/server/video_service/metrics.rs`,
Android presentation in `vendor/gpui/crates/gpui_android/src/window.rs` (recorded in
`vendor/gpui/upstream.json`), and session awake/restore in Android
`RemoteSessionPresentation.java`. Windows CI runs the exact three readback
regressions and the versioned virtual-display protocol regression; the Android
management package increments to beta.19/code19. Device before/after results are
recorded separately after installing that exact source, not inferred from builds.


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
| `apps/mobile/Cargo.toml` | Android session feature graph enables its existing native decoder; iOS retains its previous feature set. |
| `apps/mobile/Cargo.lock` | Add only the existing `ndk 0.7.0` dependency edge to scrap; no package version or checksum changes. |
| `apps/mobile/src/android.rs` | Initialize real decoder inventory while the Android VM is available, before connection negotiation. |
| `apps/mobile/src/session_view.rs` | Connected event sends the default 60 FPS request through the existing authorized API. |
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
