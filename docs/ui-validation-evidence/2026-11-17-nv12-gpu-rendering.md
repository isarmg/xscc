# Android NV12 rendering: evidence and acceptance limits

Baseline: `85ddbf47e226070c920305f53ff0c1a070b328b6` on `main`.
This change removes the profiled CPU RGB conversion and BGRA atlas transfers from
the Android MediaCodec callback path. Sustained physical-device 1080p120
acceptance remains pending; a build or a desktop GPU fixture does not establish it.

## Code review before selecting the change

The repository layout, manifests, build gates and retained performance records
were reviewed first. The streaming review followed the complete producer to
consumer path, including backpressure and resource retirement:

| Area | Source reviewed | Relevant contract |
| --- | --- | --- |
| Host capture and scheduling | `src/server/video_service.rs`, `libs/scrap/src/common/dxgi.rs`, `libs/scrap/src/dxgi/`, `src/server/video_service/` | Acquire/release ownership, no-frame versus capture failure, bounded encoder admission |
| Hardware and software codecs | `libs/scrap/src/common/{codec,vram,hwcodec,mediacodec}.rs`, `vendor/hwcodec/` owned-input and retirement code | Prepared input survives backpressure; all decoder outputs are drained |
| Transport and receive scheduling | `libs/base/src/direct_transport/`, `crates/session-transport/`, `src/client.rs`, `src/client/io_loop.rs` | Reliable reference-frame delivery, bounded video work, no input/output count equivalence |
| Decoded-frame ownership and pacing | `src/gpui_session.rs`, `src/gpui_session/{frames,pixels,metrics}.rs`, `src/client/presentation.rs` | Bounded owned allocations; preserve the static final frame and metric epochs |
| Application presentation | `crates/session-model/`, `crates/native-ui/`, `apps/mobile/` session presentation | Decoding stays off GPUI; UI acceptance is distinct from scanout |
| GPU presentation | GPUI `assets.rs`, `window.rs`, `scene.rs`; WGPU renderer, atlas, image upload, shaders and Android window/frame clock | Scene order, clipping, upload lifetime, surface capability selection, completion/recovery |

Vendored frameworks were inspected at the interfaces and implementations involved
in streaming. Archived Flutter sources and the independently built `one-kvm/`
product are outside this runtime path; this is not a claim that every vendored or
unrelated source line was read.

## Evidence and official API constraints

The retained [pipeline record](2026-11-17-android-owned-presentation.md) reports
118 receive/decode FPS with approximately 89 UI FPS, and a beta.19 CPU sample
attributes about 31% of sampled cycles to `NV12ToARGBRow_NEON` and 14.6% to UI
`memcpy`. Those measurements identify avoidable work, not a guarantee of 120 FPS.
Previous worker/thread and image-pool experiments were rejected; they are not
reintroduced here.

Official sources consulted:

- [Android MediaCodec](https://developer.android.com/reference/android/media/MediaCodec),
  especially raw video buffers, output ownership and Surface output. Native
  Surface output avoids application buffer mapping/copying, but the current
  decoder is configured for ByteBuffer output. Its bytes cannot be retained
  after releasing the codec output buffer.
- [wgpu Queue](https://wgpu.rs/doc/wgpu/struct.Queue.html#method.write_texture)
  and the repository's pinned wgpu source at
  [`3b3506ea31154edc04de64debda0e780cba2639c`](https://github.com/gfx-rs/wgpu/tree/3b3506ea31154edc04de64debda0e780cba2639c).
  `write_texture` snapshots borrowed input into staging; writes execute with the
  next queue submission. A second application-owned RGB snapshot is unnecessary.
  The 256-byte buffer-to-texture row alignment requirement does not apply to
  `Queue::write_texture`.
- [libyuv conversion definitions](https://chromium.googlesource.com/libyuv/libyuv/+/refs/heads/main/include/libyuv/convert_argb.h).
  The previous `NV12ToARGB` conversion uses limited-range BT.601. The GPU path
  retains that interpretation, with the existing BGRA image filtering, rather
  than silently switching matrices or expanding the scope to HDR/color metadata.
- [DXGI AcquireNextFrame](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe)
  and [FFmpeg send/receive](https://ffmpeg.org/doxygen/trunk/group__lavc__encdec.html)
  informed the host ownership/backpressure review. No host scheduling, GPU fence
  or capture-fallback changes are needed for this Android upload change.

## Resulting path

MediaCodec copies only the cropped, stride-validated Y and UV planes into the
existing owned decoder allocation, releases its output buffer, then publishes
that allocation through the existing bounded mailbox. GPUI retains the same
owner through its scene. The WGPU renderer uploads borrowed planes as `R8Unorm`
and `Rg8Unorm`, converts to RGB in the video shader and composites in scene order.

Texture allocations are reused only at the current dimensions and retained only
for the current scene's surfaces. Queue submission ordering protects a texture
being reused after the preceding draw. Decoder allocations remain immutable while
owned by an image; no borrowed codec buffer crosses release or a thread boundary.
Resolution changes, static-image redraw and non-video overlays retain their
existing ownership and pacing. This is **not native decoder-Surface zero-copy**:
one codec-buffer copy and wgpu's staging copy remain.

For 1920 x 1080:

| Per-frame pixel payload | Before | After |
| --- | ---: | ---: |
| Owned decoded output / texture upload | 8,294,400 BGRA bytes | 3,110,400 NV12 bytes |
| Application BGRA atlas snapshot | 8,294,400 bytes | 0 |
| BGRA mapped-staging fill | 8,294,400 bytes | 0 |

The pixel payload is 62.5% smaller. At 120 new frames/second, upload input is
995,328,000 versus 373,248,000 bytes/second, excluding alignment and driver
overheads. These are byte counts, not measured link bandwidth or FPS gains.

## Validation

Reproducible commands (run from the repository root):

```powershell
cargo test --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib
cargo test --locked --manifest-path vendor/gpui/Cargo.toml -p gpui --lib assets::tests
python .github/scripts/test-gpui-android-each-output.py
cargo test --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib compare_full_hd_upload_and_draw -- --ignored --nocapture --test-threads=1
```

The Android workflow also compiles the production `common/nv12.rs` directly with
`rustc --test`, verifies both test names are selected, and runs them. The Windows
GPU workflow requires both NV12 pixel regressions to pass; the manual timing
fixture stays ignored in regular CI. Cases cover padded/cropped odd dimensions,
truncated planes, exact plane ownership, GPU color channel order and clipping,
same-size replacement, resize, static final-image redraw and resource clearing.

Local results:

- WGPU library: **46 passed, 0 failed, 1 manual timing fixture ignored**.
- Owned-image regressions: **3 passed**; production NV12 plane-copy tests:
  **2 passed**; codec output-delivery regressions: **4 passed**.
- Android package/test-selection script regressions: **9 passed**.
- `cargo clippy ... -p gpui_wgpu --lib --tests --no-deps -- -D warnings` passed.
  Existing dependency warnings were not changed.
- `cargo check --locked -p xscc-native-ui` passed on Windows with the product
  dependency lockfile.
- The unrelated transport library compiled, but its complete suite was **7 passed,
  11 failed**: failures were loopback TCP connections and UDP handshake timeouts in
  this environment. Those transport source files are unchanged. The suite is not
  reported as passing.

Manual fixture: Windows, AMD Vega 8 integrated GPU, Vulkan, AMD 26.1.1 driver,
Rust debug profile, 1920x1080, 20 warmup + 120 measured draws per round, maximum
two queued draws, alternating BGRA/NV12/NV12/BGRA. Both paths start with owned
pixels; **codec decoding, CPU NV12-to-BGRA conversion and codec-plane copying are
excluded**. There is no swapchain/present or scanout measurement. Initial runs
concurrent with compilation fluctuated heavily and are not used for a speedup
claim. A subsequent isolated run produced:

| Path / round | CPU enqueue p50 / p95 (ms) | 140 draws including GPU drain (s) |
| --- | ---: | ---: |
| BGRA atlas / 1 | 4.588 / 6.125 | 0.820 |
| NV12 / 1 | 4.812 / 6.332 | 0.786 |
| NV12 / 2 | 4.867 / 6.384 | 0.810 |
| BGRA atlas / 2 | 4.538 / 6.474 | 0.784 |

This fixture shows similar upload/draw throughput, with slightly higher NV12 CPU
enqueue time on this desktop GPU. It does **not** show an upload-speed or
end-to-end FPS improvement. The change removes the separately profiled CPU RGB
conversion and BGRA intermediates; whether that produces the required sustained
phone performance still needs the device comparison below. The Android product
build is additionally required in CI; local Windows tests do not compile its
platform-specific hooks.

## Required device acceptance

Use the same Windows host, stream content, 1920x1080@120 display mode, Android
refresh policy and network for baseline and candidate. After warmup, retain all
per-second receive/decode/UI metrics and independent SurfaceFlinger presentation
intervals for at least a minute. Repeat after 90/144/240/120 rate changes and
background/resume/orientation changes; include a static final frame and overlays.
Compare distributions and drops rather than a peak counter. Re-sample CPU cycles
to verify that the removed RGB conversion and BGRA copies no longer dominate.
No physical device is reachable from this execution environment, so no new
SurfaceFlinger, battery, thermal or sustained-120-FPS result is claimed.

## Regression surface and minimization

| Existing files | Necessary behavior change |
| --- | --- |
| `libs/scrap/src/common/mediacodec.rs`, `common/mod.rs`, `src/client.rs` | Android GPUI hardware callbacks request packed NV12 instead of CPU RGB; ordinary/software decoding retains ARGB. Add the Android image tag and its luma stride. |
| `src/gpui_session.rs`, `crates/session-model/src/lib.rs` | Carry the Android pixel-layout tag across the existing owned-frame boundary; default trait behavior remains BGRA. The bounded queue, pool, admission and metrics are unchanged. |
| `crates/native-ui/src/presentation.rs` | Validate the layout and construct the owned NV12 image at the single UI handoff. |
| GPUI `assets.rs`, `scene.rs`, `window.rs` | Retain original NV12 allocation and put Android video in the existing surface primitive stream; apply current crop, mask, radii, opacity and ordering. BGRA uses its existing path. |
| WGPU `wgpu_renderer.rs`, `shaders.wgsl` | Prepare/draw Android Y/UV planes within the existing frame submission. New implementation lives in `wgpu_renderer/nv12.rs`; other platforms do not enable this surface path. |
| `vendor/gpui/upstream.json` | Record the maintained vendor patch. |
| Android and Windows GPU workflows | Run the plane-layout and real GPU pixel regressions with nonempty selection checks. |

New application/codec logic lives in `src/gpui_session/nv12.rs` and
`libs/scrap/src/common/nv12.rs`. No transport/protocol, bitrate, frame-rate
adaptation, host capture, AMF input slots, completion fences, presentation worker,
display service, driver, signing identity or packaging version changes are made.
