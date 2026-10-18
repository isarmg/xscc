#!/usr/bin/env python3
"""Capability/package regression checks without starting a listener."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GATE = load('check-gpui-package-mode')
MACOS = load('package-gpui-macos')


class PackageModeTests(unittest.TestCase):
    def test_binary_capability_must_match_declared_mode(self):
        for mode in ('managed', 'test-lan-direct'):
            for enabled in (0, 1):
                with self.subTest(mode=mode, enabled=enabled), patch.object(
                        GATE.subprocess, 'run', return_value=subprocess.CompletedProcess(
                            [], 0, f'XSCC_TEST_LAN_DIRECT={enabled}\n'
                            + ('test_ip_scope=any-reachable-ipv4-or-ipv6\n' if enabled else ''), '')):
                    if bool(enabled) == (mode == 'test-lan-direct'):
                        self.assertEqual(GATE.check(Path('binary'), mode)['test_lan_direct'], bool(enabled))
                    else:
                        with self.assertRaises(ValueError):
                            GATE.check(Path('binary'), mode)

    def test_missing_malformed_and_failed_capability_are_rejected(self):
        for status, output in ((0, ''), (0, 'XSCC_TEST_LAN_DIRECT=1\nextra'),
                               (1, 'XSCC_TEST_LAN_DIRECT=0')):
            with patch.object(GATE.subprocess, 'run', return_value=subprocess.CompletedProcess(
                    [], status, output, '')):
                with self.assertRaises(ValueError):
                    GATE.check(Path('binary'), 'managed')
        with self.assertRaises(ValueError):
            GATE.check(Path('binary'), 'production')

    def test_legacy_or_mismatched_scope_cannot_be_packaged_as_unrestricted(self):
        for output in ('XSCC_TEST_LAN_DIRECT=1',
                       'XSCC_TEST_LAN_DIRECT=1\ntest_ip_scope=private-subnet',
                       'XSCC_TEST_LAN_DIRECT=0\ntest_ip_scope=any-reachable-ipv4-or-ipv6'):
            for mode in ('managed', 'test-lan-direct'):
                with self.subTest(output=output, mode=mode), patch.object(
                        GATE.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, output, '')):
                    with self.assertRaises(ValueError):
                        GATE.check(Path('binary'), mode)

    def test_test_mode_does_not_allow_stable_versions(self):
        with self.assertRaises(ValueError):
            MACOS.bundle_info('host', '1.0.0')
        result = subprocess.run(['bash', str(SCRIPTS / 'package-gpui-linux.sh'),
                                 'host', '1.0.0', '/unused', '/unused'],
                                env={'XSCC_PACKAGE_MODE': 'test-lan-direct'},
                                capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Expected version', result.stderr)


if __name__ == '__main__':
    unittest.main()
