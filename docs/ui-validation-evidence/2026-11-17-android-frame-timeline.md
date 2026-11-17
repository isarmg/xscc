# Android frame timeline candidate, 2026-11-17

This follows beta.22 / 40022, source `39c400e`, and main `8984998`.
The maintainer requested Android 15+ only. The new candidate is beta.23 / 40023,
with package minimum and native application link platform set to API 35.
Historical API 29 records describe previous builds and are not updated.

## Scope and interpretation

This is a presentation-pacing candidate, not a physical-device performance
acceptance result. Device logs and measurements are retained locally.
`render_ms` includes upload, encoder finish and submit, so these stage times must
not be added together. These durations are CPU wall time, not GPU timestamps.
Application callback counts do not independently measure panel scanout.

The pinned WGPU implementation holds the device command-index write lock across
presentation and calls Vulkan `vkQueuePresentKHR`. Moving just that call to a
worker does not remove serialization with queue submission. This candidate
instead supplies the system's presentation timestamp through the existing owner.

## Official mechanism used

- [NDK Choreographer](https://developer.android.com/ndk/reference/group/choreographer):
  API 33+ supplies frame timelines with expected presentation time. The preferred
  timeline comes from Android rather than a guessed frame-duration offset.
- [Android Vulkan frame pacing](https://developer.android.com/games/develop/vulkan/frame-pacing-extensions):
  enable the supported timing extension before scheduling presentation.
- [VkPresentTimeGOOGLE](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentTimeGOOGLE.html):
  desired presentation time is in CLOCK_MONOTONIC nanoseconds on Android, the
  same clock as the Choreographer timestamp. Zero requests immediate presentation.
- [Vulkan present modes](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentModeKHR.html):
  FIFO retains presentation order; Mailbox replaces a pending request. The timed
  path defaults to FIFO so scheduled future frames remain ordered. This is an
  implementation choice derived from these semantics, not a claim that every
  blocking present is fixed by FIFO.
- Pinned [WGPU HAL timing API](https://github.com/gfx-rs/wgpu/blob/3b3506ea31154edc04de64debda0e780cba2639c/wgpu-hal/src/vulkan/mod.rs):
  `Surface::set_next_present_time` preserves WGPU's surface ownership and GPU fences.

The callback copies only the expected timestamp before FrameCallbackData expires.
It travels through the existing Ready state and window owner to the configured
Vulkan surface. There is no added presentation worker, buffer queue, clock
estimator or callback-owned raw pointer. The Vulkan timing feature is enabled
only when advertised. Unsupported adapters retain the prior present-mode choice;
explicit valid mode preferences remain honored, with timestamp hints limited to FIFO.
Per-second GPU logs now include `present_mode` and `display_timing` so the actual
path can be checked without relying on retained startup logs.

## Validation and regression surface

Local WGPU tests: 46 passed, one ignored manual timing fixture. Strict WGPU Clippy
passed. APK verifier and mobile gate: 5 and 4 passed, including rejection of API
29/34 manifests under the new API 35 contract. Android CI checks actual package
minimum SDK, native compilation and the imported frame-timeline entry points.
Physical FPS, presentation delay and reconnect/rotation/background behavior on
the new candidate remain unaccepted until retested.

Runtime changes are limited to Android `frame_clock.rs` (system timing payload),
`application.rs` / `window.rs` (the existing window handoff), WGPU `wgpu_context.rs`
(capability-based feature request), `wgpu_renderer.rs` (timed FIFO metadata), and
`android_render_timing.rs` (existing bounded diagnostics). Desktop rendering,
host capture/encoding, transport and codec frame ownership are unchanged.
Gradle, Android workflow, APK verifier/tests and current support documentation
advance the requested Android minimum; three lockfiles add the already-pinned
Ash dependency needed for WGPU's public Vulkan timing structure.
