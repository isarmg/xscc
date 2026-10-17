#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import plistlib
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('package_gpui_ios', Path(__file__).with_name('package-gpui-ios.py'))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class DevicePreviewTests(unittest.TestCase):
    def test_device_metadata(self):
        info = plistlib.loads((Path(__file__).resolve().parents[2] / 'packaging/gpui-ios/Info.plist').read_bytes())
        package.validate_metadata(info)
        for key, value in [('CFBundleSupportedPlatforms', ['iPhoneSimulator']),
                           ('MinimumOSVersion', '26.5'), ('UIDeviceFamily', [1]),
                           ('UIApplicationSceneManifest', {})]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                package.validate_metadata({**info, key: value})

    def test_device_executable(self):
        with patch.object(package, 'command', side_effect=[
            'arm64', 'MH_MAGIC_64 ARM64 EXECUTE', 'platform IOS\nminos 26.0\nsdk 26.5',
            'app:\n /usr/lib/libSystem.B.dylib (compatibility version 1.0.0)',
        ]):
            package.validate_binary(Path('app'))

    def test_reject_wrong_platform_and_unbundled_library(self):
        for build, library in [('platform MACOS\nminos 26.0', '/usr/lib/libSystem.B.dylib'),
                               ('platform IOS\nminos 26.0', '@rpath/missing.dylib')]:
            with self.subTest(build=build, library=library), patch.object(package, 'command', side_effect=[
                'arm64', 'MH_MAGIC_64 ARM64 EXECUTE', build, f'app:\n {library}',
            ]), self.assertRaises(ValueError):
                package.validate_binary(Path('app'))


if __name__ == '__main__':
    unittest.main()
