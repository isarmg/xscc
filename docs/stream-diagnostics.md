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
| Encode | `host_video_pipeline`, `host_amf_slow` | Captured and encoded FPS, encoder bitrate, keyed release and conversion delays |
| Send | `host_udp_video_pipeline` | Sent messages and datagrams, sequence, window and in-flight bytes, keyframe timings |
| Receive and assemble | `client_udp_video_pipeline` | Authenticated datagrams, sequence gaps and late packets, missing fragments, queue drops and recovery |
| Order | `client_udp_video_order_gap`, `client_udp_video_gap_recovered`, `client_udp_video_stale`, `client_udp_video_reference_ready` | Expected and received frame IDs, discarded frames and recovery |
| Decode and submit | `session_pipeline`, `android_fps_control` | Receive, decode and Surface submission rates, adaptive frame rate decisions |
| Schedule and render | `android_frame_pipeline`, `android_gpu_pipeline` | Callback and application dispatch timing, GPU acquire, submit and present time |

Match events by clock-adjusted time, display, epoch and frame ID.
The encoder reports `encoded_output_frame_count` for individual frames and
`encoded_output_count` for messages, which can carry several frames.
The sending sample includes the last video datagram sequence and completed
video message. It counts selectively retransmitted delta fragments, incoming
feedback reports and congestion-window reductions. It records the longest
interval between consecutive feedback reports. For the slowest completed
keyframe, it separates time waiting for window space, pacing tokens and feedback. Retry
lateness counts time after the requested retry time, including scheduler delay
and work on other traffic.
The receiving sample includes the highest authenticated sequence and last
reassembled video message. A sequence gap records a hole when it opens. The
late-packet counter records subsequently accepted out-of-order arrivals.
The receiving sample counts original and repeated feedback together in
`feedback_reports_sent`. After receiving no fresh packets for one report
interval, the receiver repeats the last report once. Compare
`max_feedback_send_gap_ms` and `max_feedback_send_ms` with the Host's received
feedback gaps to locate delays before or after the phone's socket send.
`session_management_gate` records the reason when the app closes on a
management availability or authorization check.

Compatible peers negotiate video forward error correction at login. Video
messages with enough data shards carry up to one interleaved XOR parity shard
per five data shards. The Host accounts for parity in the UDP send window
and bitrate budget. Parity yields to queued fresh data and is skipped if it
would wait on the pacer. Compare `sent_video_parity_datagrams`,
`received_video_parity_datagrams` and `fec_recovered_fragments` in paired logs.

Use actual presentations to check each two-second interval. In a 120 Hz
session, count adjacent one-second intervals below 70 FPS and inspect their
overlapping sender and receiver records. Compare the optimized Debug results
with a Release playback run after a fix to measure the effect of diagnostics.
