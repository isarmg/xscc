#!/usr/bin/env python3
"""Check the native stream diagnostics boundary on the packaged binary."""

import argparse
from pathlib import Path


HOST = (
    b"host_dxgi_pipeline",
    b"host_video_pipeline",
    b"host_amf_transfer=",
    b"host_amf_slow phase=",
    b"host_udp_video_pipeline",
)
CLIENT = (
    b"client_udp_video_pipeline",
    b"client_udp_video_order_gap",
    b"client_udp_video_stale",
    b"client_udp_video_reference_ready",
    b"session_pipeline",
)
ANDROID = (b"android_frame_pipeline", b"android_gpu_pipeline")
ANDROID_SESSION = (b"android_fps_control",)
REQUIRED = {
    "host": HOST,
    "controller": CLIENT,
    "management": CLIENT + ANDROID + ANDROID_SESSION,
    "landirect": CLIENT + ANDROID + ANDROID_SESSION,
    "smoke": ANDROID,
}
MARKERS = HOST + CLIENT + ANDROID + ANDROID_SESSION


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--profile", choices=("release", "stream-debug"), required=True)
    parser.add_argument("--role", choices=REQUIRED, required=True)
    args = parser.parse_args()

    data = args.binary.read_bytes()
    if not data:
        raise SystemExit("Stream diagnostics gate received an empty binary")
    if args.profile == "release":
        unexpected = [tag.decode() for tag in MARKERS if tag in data]
        if unexpected:
            raise SystemExit(f"Release contains stream diagnostics: {', '.join(unexpected)}")
    else:
        missing = [tag.decode() for tag in REQUIRED[args.role] if tag not in data]
        if missing:
            raise SystemExit(f"Debug stream diagnostics are missing: {', '.join(missing)}")
    print(f"Stream diagnostics verified for {args.role} ({args.profile})")


if __name__ == "__main__":
    main()
