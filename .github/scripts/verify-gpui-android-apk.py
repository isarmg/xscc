#!/usr/bin/env python3
"""Verify packaged ARM64 ELF and 16 KB native library alignment (not runtime)."""
import argparse
import re
import struct
import zipfile
from pathlib import Path
from gpui_mobile_test_gate import verify_test_capability

PAGE = 16384
LIBRARY = "lib/arm64-v8a/libxscc_gpui_android_smoke.so"


def check_elf(data):
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("Expected little-endian ELF64")
    if struct.unpack_from("<H", data, 18)[0] != 183:
        raise ValueError("Expected AArch64 ELF")
    offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 54)
    if entry_size < 56 or offset + entry_size * count > len(data):
        raise ValueError("Invalid ELF program headers")
    loads = 0
    for i in range(count):
        kind, _, file_offset, virtual_address, _, _, _, alignment = struct.unpack_from(
            "<IIQQQQQQ", data, offset + entry_size * i
        )
        if kind == 1:  # PT_LOAD
            loads += 1
            if alignment < PAGE or alignment & (alignment - 1):
                raise ValueError("ELF LOAD alignment is below 16 KB or not a power of two")
            if (virtual_address - file_offset) % PAGE:
                raise ValueError("ELF LOAD addresses are not 16 KB congruent")
    if not loads:
        raise ValueError("ELF has no LOAD segments")


def verify(apk, library=LIBRARY):
    raw = Path(apk).read_bytes()
    with zipfile.ZipFile(apk) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("Duplicate APK entries")
        libraries = [n for n in names if n.startswith("lib/") and n.endswith(".so")]
        if library not in libraries:
            raise ValueError("GPUI native library missing")
        for name in libraries:
            if not name.startswith("lib/arm64-v8a/"):
                raise ValueError("Unexpected ABI: " + name)
            info = archive.getinfo(name)
            if info.compress_type != zipfile.ZIP_STORED:
                raise ValueError("Native library must be uncompressed: " + name)
            if raw[info.header_offset:info.header_offset + 4] != b"PK\x03\x04":
                raise ValueError("Invalid ZIP local header")
            name_len, extra_len = struct.unpack_from("<HH", raw, info.header_offset + 26)
            data_offset = info.header_offset + 30 + name_len + extra_len
            if data_offset % PAGE:
                raise ValueError("Native ZIP entry is not 16 KB aligned: " + name)
            check_elf(archive.read(name))
        return libraries


def check_manifest(badging, tree, package="org.xscc.gpui.smoke", mode="smoke"):
    if not re.search(r"^package: name='" + re.escape(package) + r"'", badging, re.M):
        raise ValueError("Unexpected application ID")
    for line in ("sdkVersion:'29'", "targetSdkVersion:'36'", "native-code: 'arm64-v8a'"):
        if line not in badging.splitlines():
            raise ValueError("Unexpected SDK or ABI")
    permissions = set(re.findall(r"^uses-permission(?:-sdk-\d+)?: name='([^']+)'", badging, re.M))
    internal = package + ".DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"
    allowed = {internal}
    if mode == "management":
        allowed.add("android.permission.INTERNET")
        if "android.permission.INTERNET" not in permissions:
            raise ValueError("Management preview lacks INTERNET")
    elif mode != "smoke":
        raise ValueError("Unknown permission mode")
    if permissions - allowed:
        raise ValueError("Unexpected permissions: " + repr(sorted(permissions - allowed)))
    if not re.search(r"android:allowBackup\([^)]*\)=\(type 0x12\)0x0\b", tree):
        raise ValueError("APK backup must be disabled")
    if internal in permissions:
        blocks = re.split(r"(?m)^\s*E: ", tree)
        declarations = [b for b in blocks if b.startswith("permission ")
                        and ('"' + internal + '"') in b]
        if len(declarations) != 1 or not re.search(
                r"android:protectionLevel\([^)]*\)=\(type 0x11\)0x2\b", declarations[0]):
            raise ValueError("Internal receiver permission must be signature protected")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk")
    parser.add_argument("--expected-package", default="org.xscc.gpui.smoke")
    parser.add_argument("--expected-library", default="xscc_gpui_android_smoke")
    parser.add_argument("--permission-mode", choices=("smoke", "management"), default="smoke")
    parser.add_argument("--test-lan-direct", action="store_true")
    parser.add_argument("--badging", required=True)
    parser.add_argument("--manifest-tree", required=True)
    args = parser.parse_args()
    if args.test_lan_direct and args.expected_package != "org.sarmg.xscc.gpui.testdirect":
        raise ValueError("Test-only APK requires its distinct application ID")
    if not args.test_lan_direct and args.expected_package == "org.sarmg.xscc.gpui.testdirect":
        raise ValueError("Test-only application ID requires test package mode")
    with zipfile.ZipFile(args.apk) as archive:
        verify_test_capability(archive.read("lib/arm64-v8a/lib" + args.expected_library + ".so"), args.test_lan_direct)
    if args.test_lan_direct and "application-label:'XSCC TEST LAN'" not in Path(args.badging).read_text():
        raise ValueError("Missing TEST application label")
    check_manifest(Path(args.badging).read_text(), Path(args.manifest_tree).read_text(),
                   args.expected_package, args.permission_mode)
    print("Verified ELF/ZIP 16 KB alignment: " + ", ".join(verify(args.apk, "lib/arm64-v8a/lib" + args.expected_library + ".so")))
