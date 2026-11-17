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
the existing Choreographer callback, the two-slot Android decoded-frame mailbox, NV12
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
warning-free claim. Android smoke and streaming management build/link/package
checks passed in the native CI run linked below.
Physical-device sustained FPS and intermittent-stall acceptance remain outstanding.

## Public CI evidence

Source commit: `1c6c10e560ad27c22a33526816148b91e6768986`.

- [Source and package checks](https://github.com/isarmg/xscc/actions/runs/44106816181): passed.
- [Software Vulkan pixels and presentation policies](https://github.com/isarmg/xscc/actions/runs/44106816127): passed, including both Android policy tests and the existing Linux NV12 scene tests.
- [Windows, macOS and Linux framework validation](https://github.com/isarmg/xscc/actions/runs/44106817166): all passed.
- [Android smoke application](https://github.com/isarmg/xscc/actions/runs/44106816430/job/134121293733): passed native compilation, Java checks, release assembly, native imports and APK validation. This is not the streaming management client.
- [Android streaming management client](https://github.com/isarmg/xscc/actions/runs/44106816430/job/134121292134): passed native compilation/linking, Java tests, optimized release assembly, signature verification, API 35 manifest validation, 16 KB ELF/ZIP alignment and native entry-point/privacy checks.

[Management candidate ZIP](https://github.com/isarmg/xscc/actions/runs/44106816430/artifacts/14340016179), artifact `14340016179`, contains the installable streaming APK.
The separate smoke artifact is not the streaming client.

CI verified the actual management APK metadata:

- Package: `org.sarmg.xscc.gpui.preview`.
- Version: `4.0.0-beta.24` / `40024`; minimum SDK `35`, target SDK `36`; ARM64.
- Stable v2 signing certificate SHA-256: `7e2584d12ca65c34bc01facc08b77657764c6405f0c9e2050bdd1df84b2753c9`, matching the preceding preview candidates.
- GitHub-reported artifact ZIP bytes: `37559339`.
- GitHub-reported artifact ZIP SHA-256: `c7f65212a25e99171e27173d58e05f5c69450be0936b239cc97a5977a06d6189`.

Artifact identity and package checks are established by CI; no local binary
redownload, physical installation, scanout measurement or sustained-FPS
acceptance is claimed.

## Source audit correction

The Android production mailbox has capacity two (`Mailbox::default` in
`src/gpui_session.rs`), while other platforms use capacity one. The Android
consumer uses a two-frame age budget and skips an older frame only when another
frame is available. It preserves the final image of a static desktop. A queue
age near two display intervals alone is therefore not proof of an unbounded
queue or the cause of a presentation stall.

Both existing production admission tests in `src/gpui_session/frames.rs` were
run directly on the Windows host and passed. The burst test explicitly shows
that replacing the two-slot policy with a single slot can discard one output
from each pair. These are admission-policy checks, not a device throughput test.

UI frame acceptance runs in the mobile next-frame callback before the same
thread calls GPUI presentation, WGPU `Queue::present`, and the Vulkan HAL. The
pinned WGPU core holds the device command-index write lock around HAL
presentation, serializing it with subsequent GPU submissions. This identifies
how a long presentation call stalls UI acceptance; it does not identify which
lock, driver operation, fence or compositor operation caused that call to wait.

The two present-mode tests validate mode selection only. They do not reproduce
the reported high-refresh stall. This candidate removes a presentation policy
change; it must not be treated as an established root-cause fix.

### Required runtime evidence

The checked-in `tools/diagnostics/collect-android-streaming-trace.ps1` records a
30-second system trace with scheduling, CPU frequency/idle, graphics/Binder
annotations and SurfaceFlinger frame timelines, together with package version,
matching pipeline logs and before/after display state. Run it from the repository
root while the existing client is streaming:

```powershell
powershell -NoProfile -File .\tools\diagnostics\collect-android-streaming-trace.ps1
```

The default destination is a timestamped directory under the host temporary
directory. `-Seconds` and `-OutputDirectory` select another duration/destination;
`-Serial` selects a device when ADB has more than one connection. The syntax was
checked with the installed Windows PowerShell parser. Actual device recording
has not been executed in this session.

Configuration follows [Perfetto system tracing](https://perfetto.dev/docs/getting-started/system-tracing)
and [SurfaceFlinger frame timelines](https://perfetto.dev/docs/data-sources/frametimeline).
Trace-source availability must be checked in the resulting trace; an app timing
track or a specific vendor GPU fence track is not assumed to exist.

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
