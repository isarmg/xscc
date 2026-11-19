# Stream diagnostics

Select **stream_debug** in the Windows or Android candidate workflow. The
`stream-debug` Cargo profile inherits the Release optimization settings and
enables debug assertions. [Cargo profile settings](https://doc.rust-lang.org/cargo/reference/profiles.html)

The candidate carries a `-debug` suffix and records
the profile in its build info. The packagers inspect the native binary for the
expected diagnostics markers before uploading it. The Release check rejects
any stream diagnostics marker.

## Capture a live session

Use the Windows Host and Android management APK from the same main revision.
Record the TestUFO output refresh rate, both build IDs, the video codec, the
display and the connection start time. On Android, the pipeline events are
available through `adb logcat -v epoch -s XSCC-GPUI:I`. Record the Host log
over the same interval. Take Android wall-clock samples against the Windows
clock at both ends of the run. Use SurfaceFlinger presentation timestamps for
actual phone FPS and the Windows compositor counter for the output clock.

| Stage | Log | What to compare |
| --- | --- | --- |
| Capture | `host_dxgi_pipeline` | Acquire, copy and map time |
| Encode | `host_video_pipeline`, `host_amf_slow` | Captured and encoded FPS, keyed release and conversion delays |
| Send | `host_udp_video_pipeline` | Sent messages and datagrams, sequence, window and in-flight bytes, keyframe timings |
| Receive and assemble | `client_udp_video_pipeline` | Authenticated datagrams, sequence gaps and late packets, missing fragments, queue drops and recovery |
| Order | `client_udp_video_order_gap`, `client_udp_video_stale`, `client_udp_video_reference_ready` | Expected and received frame IDs, discarded frames and recovery |
| Decode and present | `session_pipeline`, `android_frame_pipeline` | Receive, decode and Surface submission rates, callback and application dispatch timing |

Match events by clock-adjusted time, display, epoch and frame ID. The sending
sample includes the last video datagram sequence and completed video message.
The receiving sample includes the highest authenticated sequence and last
reassembled video message. A sequence gap records a hole when it opens. The
late-packet counter records subsequently accepted out-of-order arrivals.

Use actual presentations to check each two-second interval. In a 120 Hz
session, count adjacent one-second intervals below 70 FPS and inspect their
overlapping sender and receiver records. Compare the optimized Debug results
with a Release playback run after a fix to measure the effect of diagnostics.
