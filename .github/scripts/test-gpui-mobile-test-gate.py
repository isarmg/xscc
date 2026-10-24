#!/usr/bin/env python3
"""Regression checks for separating managed and unauthenticated TEST artifacts."""
import unittest
import importlib.util
from pathlib import Path
import tempfile
from gpui_mobile_test_gate import MARKER, LEGACY_MARKER, verify_test_capability


class MobileTestGateTests(unittest.TestCase):
    def test_package_mode_must_match_actual_binary(self):
        verify_test_capability(b'ordinary managed application')
        verify_test_capability(b'native code' + MARKER + b'warning', True)
        with self.assertRaises(ValueError):
            verify_test_capability(b'native code' + MARKER)
        with self.assertRaises(ValueError):
            verify_test_capability(b'ordinary managed application', True)

    def test_legacy_or_mixed_scope_markers_are_rejected(self):
        for binary in (LEGACY_MARKER, LEGACY_MARKER + MARKER):
            for test_mode in (False, True):
                with self.subTest(binary=binary, test_mode=test_mode), self.assertRaises(ValueError):
                    verify_test_capability(binary, test_mode)

    def test_application_workflows_only_produce_managed_candidates(self):
        root = Path(__file__).resolve().parents[2]
        android = (root / '.github/workflows/gpui-android-smoke.yml').read_text()
        ios = (root / '.github/workflows/gpui-ios-management.yml').read_text()
        self.assertIn('flavor: management', android)
        self.assertNotIn('flavor: landirect', android)
        self.assertNotIn('mode: TEST-IP-direct', ios)
        for platform in ('windows', 'unix'):
            workflow = (root / f'.github/workflows/gpui-{platform}-beta.yml').read_text()
            self.assertIn('XSCC_PACKAGE_MODE: managed', workflow)
            self.assertNotIn('inputs.test_lan_direct', workflow)

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
