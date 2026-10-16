#!/usr/bin/env python3
"""Run the two experimental executables sequentially with identical frame inputs."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def run_probe(name, executable, args, output):
    report = output / f"{name}.json"
    command = [str(executable.resolve()), "--width", str(args.width), "--height", str(args.height),
               "--fps", str(args.fps), "--seconds", str(args.seconds), "--report", str(report.resolve())]
    if args.ivf:
        command += ["--ivf", str(args.ivf.resolve())]
    if name == "slint":
        command += ["--renderer", args.slint_renderer]
    resources = output / f"{name}-resources.json"
    if Path("/usr/bin/time").is_file():
        command = ["/usr/bin/time", "-f", '{"user_seconds":%U,"system_seconds":%S,"max_rss_kib":%M}',
                   "-o", str(resources.resolve()), *command]
    started = time.monotonic()
    with (output / f"{name}.stdout").open("x") as stdout, (output / f"{name}.stderr").open("x") as stderr:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   start_new_session=os.name == "posix")
        timed_out = False
        try:
            code = process.wait(timeout=args.seconds + 45)
        except subprocess.TimeoutExpired:
            timed_out = True
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGTERM)
            else:
                process.terminate()
            try:
                code = process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
                code = process.wait()
    return {"framework": name, "exit_code": code, "timeout": timed_out,
            "wall_seconds": time.monotonic() - started,
            "report": json.loads(report.read_text()) if report.exists() else None,
            "stderr": f"{name}.stderr"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--slint", type=Path, required=True)
    parser.add_argument("--gpui", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ivf", type=Path)
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--seconds", type=int, default=15)
    parser.add_argument("--slint-renderer", choices=["wgpu", "software"], default="wgpu")
    args = parser.parse_args()
    if not (1 <= args.width <= 3840 and 1 <= args.height <= 2160 and 1 <= args.fps <= 120 and 1 <= args.seconds <= 600):
        parser.error("invalid dimensions, FPS, or duration")
    if not args.slint.is_file() or not args.gpui.is_file():
        parser.error("build both executable probes first")
    args.output.mkdir(parents=True, exist_ok=False)
    results = [run_probe(name, executable, args, args.output) for name, executable in [("slint", args.slint), ("gpui", args.gpui)]]
    summary = {"schema": 1, "replacement_decision": "reserved_for_user", "results": results}
    (args.output / "comparison.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    return int(any(result["exit_code"] != 0 for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
