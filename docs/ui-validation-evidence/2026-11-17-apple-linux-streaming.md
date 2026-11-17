# Apple and Linux streaming: documentation-based changes

Date: 2026-11-17. Baseline: `6fb01f78c9d003e4466ab9b50fee3d194d9e3386`.
Targets: iOS/iPadOS 26, macOS 26 on Apple Silicon, Linux x86_64 on Wayland.
This record contains public source analysis and synthetic validation only.

## Official references and implementation

| Reference | Finding and resulting change |
| --- | --- |
| [Apple CAMetalLayer](https://developer.apple.com/documentation/quartzcore/cametallayer.md) and [Metal drawable best practices](https://developer.apple.com/library/archive/documentation/3DDrawing/Conceptual/MTLBestPracticesGuide/Drawables.html) | Drawables come from a limited pool. Scope each onscreen draw in an autorelease pool and prepare CPU instance data before acquiring the drawable. |
| [CADisplayLink.preferredFrameRateRange](https://developer.apple.com/documentation/quartzcore/cadisplaylink/preferredframeraterange.md) | A requested rate is subject to system policy. The existing ProMotion configuration and high-refresh Info.plist key remain enabled. |
| [NSScreen.maximumFramesPerSecond](https://developer.apple.com/documentation/appkit/nsscreen/maximumframespersecond.md) | Use the native display capability already exposed by GPUI, including its maximum for variable-refresh displays. |
| [FFmpeg AVFrame](https://ffmpeg.org/doxygen/trunk/structAVFrame.html) and [libyuv formats](https://chromium.googlesource.com/libyuv/libyuv/+/HEAD/docs/formats.md) | Respect independent plane strides and visible dimensions. Keep Linux H.264/H.265 decoded NV12 or I420 in packed owned NV12 instead of expanding it to BGRA on the CPU. |
| [Wayland wl_surface.frame](https://pq.pages.freedesktop.org/wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_surface-request-frame) | Its timestamp has an undefined epoch. Record callback dispatch receipt in the process clock for profiling; do not infer compositor queue age from CLOCK_MONOTONIC. |

The iOS, macOS and Linux stream request is capped using `Window::refresh_interval`.
A remote 240 Hz mode on a reported local 120 Hz display requests a 120 FPS stream;
the remote display mode remains 240 Hz. Lower remote rates remain lower. Unknown
or zero display intervals retain the remote request. This bounds work by the
reported display capability; it does not measure the current scanout rate.

Desktop display capability is stored next to the existing target FPS and updated
by the existing window display observer API. Its weak entity reference and owned
subscription follow the existing window lifecycle. There is no polling timer.
Android and Windows retain their existing FPS policies.

## Resource ownership and scope

Metal still returns instance buffers to the pool only from the GPU completion
handler. Both transaction-based and normal presentation ordering are preserved.
Offscreen and headless rendering are unchanged. No in-flight limit, pool size,
GPU fence or presentation synchronization is removed. The existing macOS
CVDisplayLink teardown protection remains in place; deprecation alone is not
evidence of a performance bottleneck and no clock backend migration is included.

The Linux native H.264/H.265 RAM decoder path copies validated, decoder-owned
NV12/I420 planes into an owned packed NV12 image before decoder reuse. The
existing bounded frame mailbox and WGPU upload synchronization remain in use.
The shared Android NV12 shader and texture upload handle Linux surface primitives.
VP8, VP9 and AV1 keep the BGRA path. No new thread, presentation queue, frame ring,
clock estimator, dependency or lockfile change is introduced.

This is not zero-copy. The FFmpeg wrapper already accepts positive-stride owned
planes; this change does not add support for negative AVFrame strides. The
existing BT.601 limited-range conversion policy is preserved, with no new HDR or
color-management claim. At 1920 x 1080, packed NV12 carries 3,110,400 bytes versus
8,294,400 for BGRA, a theoretical 62.5% reduction in owned pixel payload before
GPU row alignment. This is not a measured FPS, total bandwidth or network result.

The Wayland change corrects profiler clock semantics only. The callback request,
commit and redraw pacing logic are unchanged.

## Validation

Completed locally before the execution environment disconnected:

- Four production YUV-plane tests passed with Rust edition 2018, covering padded
  odd dimensions, independent NV12/I420 strides, invalid layouts and 1080p data.
- The WGPU library suite passed 46 tests with one manual benchmark ignored on
  the Windows host. Linux-only scene presentation was not exercised there.
- Package-scoped WGPU Clippy passed with `--no-deps --all-targets -- -D warnings`.
  A broader initial run reported four pre-existing dependency test-helper
  diagnostics (three deprecated atomic methods and dead test-platform methods);
  those unrelated helpers were left unchanged.
- Mobile and desktop formatting checks, workflow boundary validation, three
  vendored-source contract tests and whitespace checks passed.
- The shared native UI test command was still compiling when the environment
  disconnected. Its local result is unconfirmed.

CI now explicitly runs the full Linux NV12 scene presentation regression, the
four decoder-plane tests, the real `HwRamDecoderImage::to_fmt` conversion
regression, and existing Wayland lifecycle tests. Filters reject empty test
runs. Existing native Apple and Unix product builds validate platform integration.
CI conclusions and run links will be recorded after completion.

Physical Apple/Linux high-refresh devices are unavailable in this session.
Sustained FPS, frame latency, thermal behavior, rotation, reconnect, occlusion
and multi-display acceptance still require matched before/after device tests.
Neither a compilation nor a software Vulkan pixel test establishes 120 FPS.

## Changed runtime surfaces

| Existing files | Why the change is needed |
| --- | --- |
| `gpui_apple/src/metal_renderer.rs` | Per-draw pool and late drawable acquisition on iOS/macOS. |
| `scrap/common/{mod,nv12,hwcodec}.rs` | Owned NV12 output with stride validation and the existing decoder API regression. |
| `src/{client,gpui_session}.rs` | Select and carry NV12 only on the Linux native H.264/H.265 path. |
| `session-model/src/lib.rs`, `native-ui/src/presentation.rs` | Carry plane format through the shared UI boundary and apply the display ceiling. |
| `gpui/src/{assets,scene,window}.rs`, `gpui_wgpu/src/wgpu_renderer.rs` | Paint Linux NV12 through the existing scene, shader and upload path. |
| `gpui_linux/src/linux/wayland/client.rs` | Remove the unsupported timestamp-epoch inference and its assumption-specific tests. |
| Desktop `remote_video{,/media_controls}.rs`, mobile `session_view.rs` | Apply display capability to target FPS and desktop display changes. |
| Three existing CI files | Exercise the affected decoder, GPU scene and Wayland paths. |

The prerequisite commit only formats two existing mobile files
(`display_state.rs` and `session_view.rs`) to unblock the existing iOS formatting
check. It does not alter mobile behavior. No unrelated refactor is included.
