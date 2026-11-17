# Android streaming: remove future FIFO presentation holds

Date: 2026-11-17. Baseline: `d4822b5b55309f75fedc79a7f1a231c126cfcb06`.
Candidate: beta.24 / 40024, Android ARM64, minimum Android 15 / API 35.
This record contains public source analysis and synthetic validation only.
Private device observations and diagnostics remain outside the repository.

## Documentation and implementation

- [Vulkan present modes](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentModeKHR.html):
  FIFO retains pending presentations in order. Mailbox replaces a pending image
  with the latest image while retaining synchronization to vertical blanking.
- [VkPresentTimeGOOGLE](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentTimeGOOGLE.html):
  a nonzero desired presentation time forbids presentation before that time.
  It is an earliest-time constraint, not a deadline or a guarantee of completion.
- [Android frame timelines](https://developer.android.com/ndk/reference/group/choreographer):
  the preferred timeline is selected by the system based on device configuration.
  Applications may instead use the earliest available timeline when they do not
  select a future presentation timeline.
- [Android frame pacing](https://developer.android.com/games/sdk/frame-pacing):
  presentation timestamps address early presentation, while long-frame queue
  backpressure also requires synchronization and workload control. Merely adding
  timestamps does not establish that an application has implemented Swappy.
- [Android rendering loops](https://developer.android.com/games/develop/gameloops):
  a Choreographer callback does not guarantee timely rendering; queue pressure
  and callback delays must be measured separately.

Beta.23 changed capable Android devices from Mailbox to timed FIFO. This candidate
removes that policy and its timestamp plumbing rather than adding a second pacing
layer. Android prefers Mailbox when advertised; FIFO remains the synchronized
fallback. Explicit supported surface preferences retain their existing priority.
No future presentation timestamp is supplied in either mode. Android 15 support,
the existing Choreographer callback, the single-slot decoded-frame mailbox, NV12
upload reuse, GPU fences, and the requested swapchain latency are retained.

This is a rollback candidate for device comparison. Neither the Vulkan semantics
nor the rollback alone establishes the cause of every intermittent FPS drop.
The renderer does not wait for a chosen future timeline, but WGPU and the driver
may still block during surface acquisition, submission or presentation.

## Diagnostics and metric interpretation

The existing per-second `android_frame_pipeline` report adds mean and maximum
time from native Choreographer callback receipt to its consumption by
`android_main`. This includes scheduling and foreground-task delay between those
points. It excludes delay before the callback actually begins on the Java main
thread and does not measure panel scanout or GPU duration.

An `android_fps_control` line accompanies an actual adaptive FPS change, reporting
the requested rate, decode work capacity, compressed-frame queue length and
increase/decrease decision. It adds no timer, queue or controller state, and does
not change the existing adaptive-FPS algorithm. A decoder service-time percentile
is not itself evidence of network supply, overall decode throughput or thermals.

`ui_queue_p95_ms` measures owned decoded-frame publication to UI acceptance.
It excludes GPU completion and scanout. `render_ms` includes upload/finish/submit;
these CPU wall times must not be summed as independent GPU stages. The existing
GPU report retains actual present mode and reports `display_timing=false`.

## Validation

Local WGPU library validation passed 48 tests, including both presentation-policy
regressions and the NV12 GPU pixel tests. One manual benchmark remains ignored.
APK verifier tests (5), mobile test-gate tests (4), vendored-source tests (3), and
the four production MediaCodec output-delivery regressions passed. Two small
policy regressions cover Mailbox preference and synchronized FIFO fallback; the
existing NV12 CI also explicitly runs both and rejects an empty test selection.
Package-scoped WGPU Clippy passed with `--no-deps --all-targets -- -D warnings`.
The dependency reports its existing warnings; this is not a workspace-wide
warning-free claim. Native Android build/link/package checks are pending.
Physical-device sustained FPS and intermittent-stall acceptance remain outstanding.

## Changed runtime surfaces

| Existing files | Required behavior change |
| --- | --- |
| `gpui_wgpu/src/wgpu_renderer.rs` | Restore Android Mailbox preference and remove future timestamp submission. Desktop selection, explicit preferences and GPU synchronization retain their behavior. |
| `gpui_wgpu/src/wgpu_context.rs`, its manifest and three lockfiles | Remove the unused timing feature request and direct Ash dependency. WGPU retains its own Vulkan dependencies. |
| Android `frame_clock.rs`, `application.rs`, `window.rs` | Remove the timing handoff and measure callback-to-render-thread delay in the existing state/report. Keep one pending callback and lifecycle cancellation. |
| `src/client/io_loop.rs` | Android-only diagnostics when adaptive FPS actually changes; no change to the decision or wire protocol. |
| Android Gradle/workflow and backend README | Advance candidate identity, retain API 35 and verify the still-used Choreographer import. |
| Existing NV12 workflow | Run both presentation-policy regressions with nonempty-selection checks. |

No new dependency, thread, frame ring, retry policy, display-rate estimator or
host capture/encoding change is included.
