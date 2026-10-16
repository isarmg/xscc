#!/usr/bin/env python3
"""Exercise a probe on a dedicated Xvfb display; capture only its own window."""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import struct
import subprocess
import time
import zlib


class XImage(C.Structure):
    _fields_ = [("width", C.c_int), ("height", C.c_int), ("xoffset", C.c_int), ("format", C.c_int),
                ("data", C.c_void_p), ("byte_order", C.c_int), ("bitmap_unit", C.c_int),
                ("bitmap_bit_order", C.c_int), ("bitmap_pad", C.c_int), ("depth", C.c_int),
                ("bytes_per_line", C.c_int), ("bits_per_pixel", C.c_int),
                ("red_mask", C.c_ulong), ("green_mask", C.c_ulong), ("blue_mask", C.c_ulong)]


def screenshot(window, path):
    x = C.CDLL("libX11.so.6")
    x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [C.c_char_p], C.c_void_p
    x.XCloseDisplay.argtypes = [C.c_void_p]
    x.XGetGeometry.argtypes = [C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_int),
                              C.POINTER(C.c_int), *([C.POINTER(C.c_uint)] * 4)]
    x.XGetImage.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_int, C.c_uint, C.c_uint, C.c_ulong, C.c_int]
    x.XGetImage.restype = C.POINTER(XImage)
    x.XDestroyImage.argtypes = [C.POINTER(XImage)]
    display = x.XOpenDisplay(None)
    if not display:
        raise RuntimeError("Xvfb display is unavailable")
    image = None
    try:
        root, left, top = C.c_ulong(), C.c_int(), C.c_int()
        width, height, border, depth = C.c_uint(), C.c_uint(), C.c_uint(), C.c_uint()
        if not x.XGetGeometry(display, window, C.byref(root), C.byref(left), C.byref(top),
                              C.byref(width), C.byref(height), C.byref(border), C.byref(depth)):
            raise RuntimeError("probe window geometry unavailable")
        image = x.XGetImage(display, window, 0, 0, width, height, C.c_ulong(-1), 2)
        if not image:
            raise RuntimeError("probe screenshot unavailable")
        header = image.contents
        if header.byte_order != 0 or header.bits_per_pixel != 32 or (header.red_mask, header.green_mask, header.blue_mask) != (0xff0000, 0xff00, 0xff):
            raise RuntimeError("unsupported Xvfb pixel format")
        pixels = C.string_at(header.data, header.bytes_per_line * header.height)
        scanlines = bytearray()
        for y in range(header.height):
            row = pixels[y * header.bytes_per_line:y * header.bytes_per_line + header.width * 4]
            rgba = bytearray(len(row))
            rgba[0::4], rgba[1::4], rgba[2::4], rgba[3::4] = row[2::4], row[1::4], row[0::4], b'\xff' * header.width
            scanlines += b'\x00' + rgba
        def chunk(name, data):
            return struct.pack('>I', len(data)) + name + data + struct.pack('>I', zlib.crc32(name + data))
        path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', header.width, header.height, 8, 6, 0, 0, 0))
                         + chunk(b'IDAT', zlib.compress(scanlines)) + chunk(b'IEND', b''))
    finally:
        if image:
            x.XDestroyImage(image)
        x.XCloseDisplay(display)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--framework', choices=['slint', 'gpui'], required=True)
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--ivf', type=Path)
    args = parser.parse_args()
    if os.environ.get('WAYLAND_DISPLAY'):
        parser.error('use a dedicated Xvfb display with WAYLAND_DISPLAY unset')
    args.output.mkdir(parents=True, exist_ok=False)
    report = args.output.resolve() / 'probe.json'
    command = [str(args.exe.resolve()), '--seconds', '15', '--report', str(report)]
    if args.ivf:
        command += ['--ivf', str(args.ivf.resolve())]
    with (args.output / 'stdout.log').open('w') as out, (args.output / 'stderr.log').open('w') as err:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=out, stderr=err)
        try:
            deadline = time.monotonic() + 12
            window = None
            while process.poll() is None and time.monotonic() < deadline:
                result = subprocess.run(['xdotool', 'search', '--all', '--onlyvisible', '--pid', str(process.pid), '--name', '.*'], capture_output=True, text=True)
                if result.returncode == 0 and result.stdout.strip():
                    window = result.stdout.splitlines()[0]
                    break
                time.sleep(0.2)
            if window is None:
                raise RuntimeError('probe did not expose its own visible X11 window')
            time.sleep(2)
            subprocess.run(['xdotool', 'windowfocus', '--sync', window], check=True)
            subprocess.run(['xdotool', 'mousemove', '--window', window, '500', '400', 'click', '1'], check=True)
            time.sleep(1)
            subprocess.run(['xdotool', 'key', '--delay', '150', 'a', 'Left', 'Return', 'ctrl+s'], check=True)
            subprocess.run(['xdotool', 'windowsize', window, '900', '700'], check=True)
            time.sleep(2)
            screenshot(int(window), args.output / 'window.png')
            code = process.wait(timeout=30)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    evidence = json.loads(report.read_text()) if report.exists() else None
    passed = code == 0 and evidence is not None and evidence['key_down'] >= 3 and evidence['key_up'] >= 3 and evidence['pointer_events'] >= 2
    result = {'framework': args.framework, 'x11_automated_input_passed': passed, 'exit_code': code,
              'manual_input_method_verified': False, 'physical_device_verified': False}
    (args.output / 'input-check.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
    return int(not passed)


if __name__ == '__main__':
    raise SystemExit(main())
