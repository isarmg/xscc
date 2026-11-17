# Android high-refresh follow-up, 2026-11-17

The reported failure is sustained frame rate below the selected 120 Hz or higher,
sometimes decreasing when the source display rate increases. This follow-up is
based on main `785e91dc9c080d2e98f785f50160d1dbdddba0a3`. It improves two measured
costs in the Android owned-NV12 path; it does **not** establish sustained phone
performance acceptance.

## Findings and changes

1. Each changed NV12 image used two `Queue::write_texture` calls. Use the pinned
   wgpu `StagingBelt::allocate` API to copy the owned planes directly into reusable
   mapped upload buffers, then encode their texture copies before drawing. This
   follows the existing Android BGRA uploader's approach. Rows are padded to the
   texture-copy alignment; odd chroma dimensions retain ceiling division. Only
   completed buffers are remapped, through `finish_and_recall_on_submit` and a
   nonblocking device poll. There is no additional presentation thread or queue.
2. The shader performed four RGB conversions and eight plane loads per output
   pixel even at pixel-aligned 1:1 size. In that case all other bilinear weights
   are zero. Use the fragment's pixel coordinate to perform one conversion and
   two loads. Scaled or fractionally positioned images retain the original
   convert-then-interpolate path, including its limited-range BT.601 rounding.
3. `android_gpu_pipeline upload_ms` started after NV12 preparation and omitted
   that upload. Start the timer before preparing NV12 and atlas uploads. These
   remain CPU stage timings, not GPU timestamps or SurfaceFlinger scanout times.
4. If recording fails after a copy is encoded, discard the NV12 upload resources
   so a retry cannot mistake an unsubmitted copy for the current static image.
   Submitted commands retain their own GPU resource references.
5. Advance the Android management candidate to beta.22 / versionCode 40022 so
   installation can be distinguished from beta.21 and earlier beta.1-labelled
   CI artifacts. Package ID and signing setup remain unchanged.

## Official sources checked

- [wgpu 30 StagingBelt](https://wgpu.rs/doc/wgpu/util/struct.StagingBelt.html):
  allocation, mapping and submit-before-reuse ordering. The implementation was
  also checked against the actual pinned source at
  `3b3506ea31154edc04de64debda0e780cba2639c`, including the dropped-encoder warning.
- [WGSL fragment position](https://www.w3.org/TR/WGSL/#position-builtin-value):
  fragment position is in framebuffer coordinates. This supports the 1:1 path;
  it does not justify replacing interpolation for scaled images.
- [Android frame rate API](https://developer.android.com/media/optimize/performance/frame-rate):
  the requested refresh rate is a platform hint, not a guaranteed delivered rate.
- [Android rendering performance](https://developer.android.com/topic/performance/issues/render):
  high refresh rates reduce the frame deadline. At 120 FPS the interval is about
  8.33 ms; at 144 it is 6.94 ms and at 240 it is 4.17 ms.
- [Android native Choreographer](https://developer.android.com/ndk/reference/group/choreographer)
  and [render-loop guidance](https://developer.android.com/games/develop/gameloops):
  pacing and submission backlog matter independently of the source display mode.

The existing mobile target already clamps the requested stream FPS to the
reported same-resolution local display capability when available. A 120 Hz phone
does not request a 240 FPS stream merely because the remote display is set to
240 Hz. That does not remove the host's cost of driving the higher source mode.
The latter is a possible contributor, not a measured cause in this phone run.

## Local measurements

Windows, AMD Vega 8 integrated GPU, Vulkan, AMD driver 26.1.1, debug Rust build.
The existing manual upload/draw fixture used the same opaque BGRA payload in all
comparison runs. Each round used 20 warm-up and 120 measured frames, with a GPU
drain after every two draws. CPU enqueue time includes upload, encoding and
submission; it excludes decoder work, Android presentation and scanout.

| NV12 1920x1080 path | CPU enqueue p50, two rounds | Total 140 draws including drain, two rounds |
| --- | --- | --- |
| Previous queue texture upload | 4.969 / 5.376 ms | 0.857 / 0.872 s |
| Mapped upload, previous shader | 2.859 / 2.925 ms | 0.640 / 0.980 s |
| Mapped upload, 1:1 shader | 2.391 / 2.190 ms | 0.452 / 0.430 s |

The mapped-only total is variable, so the CPU enqueue reduction must not be
converted into an end-to-end FPS claim. BGRA CPU medians in the final run were
4.534 / 4.913 ms, useful as a same-run control rather than phone performance.

A temporary manual timestamp-query probe isolated GPU render-pass time after
both inputs had been uploaded. It requested `TIMESTAMP_QUERY` on a separate test
device, warmed up 20 draws and sampled 80 per round. The probe is excluded from
product code and CI; the existing manual fixture remains available.

| GPU-only p50, two rounds | Previous shader | 1:1 fast path |
| --- | --- | --- |
| 1920x1080 source and output | 1.401 / 1.399 ms | 0.905 / 0.960 ms |
| 1920x1080 source scaled to 1280x720 | 0.681 / 0.618 ms | 0.616 / 0.617 ms |
| 1920x1080 source scaled to 2400x1350 | 2.166 / 2.162 ms | 2.221 / 2.225 ms |

Scaling remains a more expensive path. The shader fast path applies only to
pixel-aligned 1:1 drawing. These sequential runs are diagnostic measurements,
not a controlled phone power/thermal benchmark. A separate three-gather shader
experiment increased the 1080p GPU median to 1.579 / 1.581 ms; it was reverted.

## Verification and regression surface

- `cargo test --offline --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib -- --test-threads=1`:
  46 passed, one explicitly ignored manual benchmark. The existing GPU pixel
  test now also checks integer translation, fractional translation, scaling and
  odd-sized chroma. Static redraw, replacement and size changes remain covered.
- `cargo test --offline --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib wgpu_renderer::nv12::tests::compare_full_hd_upload_and_draw -- --ignored --exact --nocapture`:
  the manual benchmark selects one test; no empty-test success is accepted.
- `python .github/scripts/test-gpui-android-apk.py` and
  `python .github/scripts/test-gpui-mobile-test-gate.py`: 5 and 4 passed.
- `cargo clippy --offline --locked --manifest-path vendor/gpui/Cargo.toml -p gpui_wgpu --lib --tests --no-deps -- -D warnings`:
  passed. Existing GPUI dependency warnings are outside the changed crate.
- ARM64 Android and Mesa GPU workflow results are recorded with the produced
  candidate artifact once those builds complete.

Existing runtime paths changed are limited to `gpui_wgpu/src/wgpu_renderer/nv12.rs`
(Android-only upload and retry), `wgpu_renderer.rs` (the necessary command encoder
hook and Android upload timing), and `shaders.wgsl` (NV12 drawing). The Gradle and
Android workflow defaults identify the candidate. Desktop rendering, host
capture/encoding, codec ownership, bounded video queues and the frame clock are
unchanged. No decoded codec buffer is retained after release.

Current phone receive/decode/UI counters, actual display mode and a sustained
SurfaceFlinger sample are still required to distinguish a source/transport
bottleneck from missed Android presentation deadlines. ADB cannot start in this
execution policy because localhost TCP connections are denied; no phone install
or new physical performance measurement is claimed in this follow-up.
