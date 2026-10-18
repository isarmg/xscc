#!/usr/bin/env python3
"""Capture a real packaged macOS controller window on a CI graphical session."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import time

SWIFT = r'''
import AppKit
import CoreGraphics
import Metal
import Foundation
let mode = CommandLine.arguments[1]
var result: [String: Any] = [:]
if mode == "window" {
    let pid = Int32(CommandLine.arguments[2])!
    let app = NSRunningApplication(processIdentifier: pid)
    app?.activate(options: [.activateAllWindows])
    let all = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    result["windows"] = all.filter {
        ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }
} else if mode == "image" {
    guard let data = NSData(contentsOfFile: CommandLine.arguments[2]),
          let image = NSBitmapImageRep(data: data as Data) else { exit(2) }
    var colors = Set<String>()
    for y in stride(from: 0, to: image.pixelsHigh, by: max(1, image.pixelsHigh / 100)) {
        for x in stride(from: 0, to: image.pixelsWide, by: max(1, image.pixelsWide / 100)) {
            if let color = image.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) {
                colors.insert("\(Int(color.redComponent * 255)),\(Int(color.greenComponent * 255)),\(Int(color.blueComponent * 255))")
            }
        }
    }
    result = ["width": image.pixelsWide, "height": image.pixelsHigh, "sampled_colors": colors.count]
} else {
    result = ["screen_recording_preflight": CGPreflightScreenCaptureAccess(),
              "metal_device": MTLCreateSystemDefaultDevice()?.name ?? "unavailable",
              "screens": NSScreen.screens.count,
              "session": CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]]
}
let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
print(String(decoding: data, as: UTF8.self))
'''


def capture(app, output):
    output.mkdir(parents=True, exist_ok=True)
    report = {'platform': 'macOS CI runner, native packaged controller',
              'status': 'not-captured', 'app': str(app),
              'source_sha': os.environ.get('APP_SOURCE_SHA', os.environ.get('GITHUB_SHA', 'unknown')),
              'workflow_sha': os.environ.get('GITHUB_SHA', 'unknown'),
              'run_id': os.environ.get('GITHUB_RUN_ID', 'local'),
              'scope': 'Real initial application window only; no remote connection or physical-device acceptance',
              'visual_review': 'required'}
    process = None
    try:
        if sys.platform != 'darwin':
            raise RuntimeError('macOS is required for native window capture')
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        binary = app / 'Contents/MacOS' / info['CFBundleExecutable']
        report['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        report['os'] = subprocess.check_output(['sw_vers'], text=True)
        report['console_user'] = subprocess.check_output(['stat', '-f', '%Su', '/dev/console'], text=True).strip()
        with tempfile.TemporaryDirectory(prefix='gpui-macos-capture-') as temporary:
            source = Path(temporary) / 'probe.swift'
            helper = Path(temporary) / 'probe'
            source.write_text(SWIFT)
            subprocess.run(['xcrun', 'swiftc', str(source), '-o', str(helper)], check=True, timeout=90)

            def probe(*arguments):
                return json.loads(subprocess.check_output([str(helper), *arguments], text=True, timeout=15))

            report['preflight'] = probe('preflight')
            if not report['preflight']['screens']:
                raise RuntimeError('No graphical display in this CI session')
            with (output / 'application.log').open('w') as log:
                process = subprocess.Popen([str(binary)], stdout=log, stderr=subprocess.STDOUT)
                report['pid'] = process.pid
                deadline = time.monotonic() + 45
                windows = []
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(f'Application exited before capture: {process.returncode}')
                    windows = probe('window', str(process.pid))['windows']
                    windows = [w for w in windows if w.get('kCGWindowBounds', {}).get('Width', 0) >= 300
                               and w.get('kCGWindowBounds', {}).get('Height', 0) >= 200]
                    if windows:
                        break
                    time.sleep(1)
                report['windows'] = windows
                if not windows:
                    raise RuntimeError('No visible application window for the launched PID')
                window = max(windows, key=lambda w: w['kCGWindowBounds']['Width'] * w['kCGWindowBounds']['Height'])
                time.sleep(3)
                if process.poll() is not None:
                    raise RuntimeError('Application exited before its first screenshot')
                png = output / 'macos-controller.png'
                result = subprocess.run(['/usr/sbin/screencapture', '-x', '-o', '-l', str(window['kCGWindowNumber']), str(png)],
                                        capture_output=True, text=True, timeout=20)
                report['screencapture'] = {'exit_code': result.returncode, 'stderr': result.stderr}
                if result.returncode or not png.is_file():
                    raise RuntimeError('Native window screenshot failed; recording permission may be unavailable')
                report['image'] = probe('image', str(png))
                if report['image']['width'] < 300 or report['image']['height'] < 200 or report['image']['sampled_colors'] < 8:
                    png.rename(output / 'rejected-capture.png')
                    raise RuntimeError('Capture is too small or visually uniform; not accepted as an app screenshot')
                report['screenshot_sha256'] = hashlib.sha256(png.read_bytes()).hexdigest()
                report['status'] = 'captured-pending-visual-review'
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        (output / 'evidence.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    capture(args.app.resolve(), args.output.resolve())
