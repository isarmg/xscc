#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('package_ios', Path(__file__).with_name('package-ios-beta.py'))
package_ios = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_ios)


class DevicePackageTests(unittest.TestCase):
    def setUp(self):
        self.info = {
            'CFBundleIdentifier': 'org.sarmg.xscc.beta',
            'CFBundlePackageType': 'APPL',
            'CFBundleSupportedPlatforms': ['iPhoneOS'],
            'MinimumOSVersion': '26.0',
            'UIDeviceFamily': [1, 2],
        }

    def test_device_candidate_is_accepted(self):
        package_ios.validate_metadata(self.info)

    def test_simulator_cannot_be_packaged_as_device_candidate(self):
        self.info['CFBundleSupportedPlatforms'] = ['iPhoneSimulator']
        with self.assertRaisesRegex(ValueError, 'physical-device'):
            package_ios.validate_metadata(self.info)

    def test_unexpected_identity_or_supported_devices_are_rejected(self):
        for key, value in [('CFBundleIdentifier', 'com.carriez.flutterHbb'),
                           ('MinimumOSVersion', '15.0'), ('UIDeviceFamily', [1])]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                package_ios.validate_metadata({**self.info, key: value})


if __name__ == '__main__':
    unittest.main()
