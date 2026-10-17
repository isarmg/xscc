#!/usr/bin/env python3
"""Verify packaged ARM64 ELF and 16 KB native library alignment (not runtime)."""
import argparse
import struct
import zipfile
from pathlib import Path

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


def verify(apk):
    raw = Path(apk).read_bytes()
    with zipfile.ZipFile(apk) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("Duplicate APK entries")
        libraries = [n for n in names if n.startswith("lib/") and n.endswith(".so")]
        if LIBRARY not in libraries:
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk")
    args = parser.parse_args()
    print("Verified ELF/ZIP 16 KB alignment: " + ", ".join(verify(args.apk)))
