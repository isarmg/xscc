#!/usr/bin/env python3
"""Regression checks for separating managed and unauthenticated TEST artifacts."""
import unittest
import importlib.util
from pathlib import Path
import tempfile
from gpui_mobile_test_gate import MARKER, verify_test_capability


class MobileTestGateTests(unittest.TestCase):
    def test_package_mode_must_match_actual_binary(self):
        verify_test_capability(b'ordinary managed application')
        verify_test_capability(b'native code' + MARKER + b'warning', True)
        with self.assertRaises(ValueError):
            verify_test_capability(b'native code' + MARKER)
        with self.assertRaises(ValueError):
            verify_test_capability(b'ordinary managed application', True)

    def test_ios_packager_rejects_mislabeled_binary_before_native_tools(self):
        spec = importlib.util.spec_from_file_location('ios_package', Path(__file__).with_name('package-gpui-ios.py'))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'binary'
            for data, test_mode in [(MARKER, False), (b'managed only', True)]:
                binary.write_bytes(data)
                with self.subTest(test_mode=test_mode), self.assertRaisesRegex(ValueError, 'capability'):
                    module.package(binary, Path(directory) / 'output',
                                   playback_preview=True, test_lan_direct=test_mode)


if __name__ == '__main__':
    unittest.main()
