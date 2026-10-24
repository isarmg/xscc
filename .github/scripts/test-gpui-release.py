#!/usr/bin/env python3
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('collect-gpui-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_all_five_platforms_and_both_desktop_roles_are_required(self):
        groups = release.expected_artifacts('0.1.0-beta.2', 'a' * 40)
        names = [name for group in groups.values() for name in group]
        self.assertEqual(len(names), 8)
        for platform in ('windows11-x86_64', 'linux-x86_64', 'macos-arm64'):
            for role in ('controller', 'host'):
                self.assertIn(f'xscc-{role}-0.1.0-beta.2-{platform}-managed-unsigned', names)
        self.assertFalse(any('test-lan-direct' in name for name in names))

    def test_metadata_rejects_wrong_revision_and_test_candidates(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            file = directory / 'xscc-host-build-info.json'
            info = {'source_sha': 'a' * 40, 'package_version': '0.1.0-beta.2', 'package_mode': 'managed'}
            file.write_text(json.dumps(info))
            release.verify_metadata(directory, 'windows', 'a' * 40, '0.1.0-beta.2')
            with self.assertRaises(ValueError):
                release.verify_metadata(directory, 'windows', 'b' * 40, '0.1.0-beta.2')
            info['package_mode'] = 'test-lan-direct'
            file.write_text(json.dumps(info))
            with self.assertRaises(ValueError):
                release.verify_metadata(directory, 'windows', 'a' * 40, '0.1.0-beta.2')

    def test_corrupt_or_incomplete_assets_fail_checksum_validation(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            file = directory / 'client.zip'
            file.write_bytes(b'candidate')
            digest = hashlib.sha256(file.read_bytes()).hexdigest()
            (directory / 'SHA256SUMS').write_text(f'{digest}  client.zip\n')
            release.verify_checksums(directory)
            file.write_bytes(b'corrupted')
            with self.assertRaises(ValueError):
                release.verify_checksums(directory)
            file.unlink()
            with self.assertRaises(ValueError):
                release.verify_checksums(directory)


if __name__ == '__main__':
    unittest.main()
