#!/usr/bin/env python3
import importlib.util
import struct
import unittest
import tempfile
import zipfile
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "verify_apk", Path(__file__).with_name("verify-gpui-android-apk.py")
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def elf(alignment=16384, machine=183):
    data = bytearray(120)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", data, 18, machine)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HH", data, 54, 56, 1)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0, 0, 120, 120, alignment)
    return data


class AlignmentTests(unittest.TestCase):
    def test_accepts_16kb_arm64(self):
        module.check_elf(elf())

    def test_rejects_4kb(self):
        with self.assertRaisesRegex(ValueError, "below 16 KB"):
            module.check_elf(elf(4096))

    def test_checks_packaged_zip_alignment(self):
        with tempfile.TemporaryDirectory() as directory:
            apk = Path(directory) / "smoke.apk"
            for aligned in (True, False):
                with self.subTest(aligned=aligned):
                    entry = zipfile.ZipInfo(module.LIBRARY)
                    if aligned:
                        padding = module.PAGE - 30 - len(module.LIBRARY.encode())
                        entry.extra = struct.pack("<HH", 0xFFFF, padding - 4) + bytes(padding - 4)
                    with zipfile.ZipFile(apk, "w") as archive:
                        archive.writestr(entry, elf())
                    if aligned:
                        self.assertEqual(module.verify(apk), [module.LIBRARY])
                    else:
                        with self.assertRaisesRegex(ValueError, "ZIP entry is not 16 KB"):
                            module.verify(apk)


if __name__ == "__main__":
    unittest.main()
