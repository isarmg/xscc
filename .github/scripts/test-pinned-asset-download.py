#!/usr/bin/env python3
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location(
    'pinned_asset_download', Path(__file__).with_name('pinned_asset_download.py'))
downloader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(downloader)


class PinnedAssetDownloadTests(unittest.TestCase):
    def test_bad_pin_preserves_existing_destination_and_cleans_partial(self):
        payload = b'new asset bytes'
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / 'asset.zip'
            destination.write_bytes(b'previous verified asset')
            with mock.patch.object(downloader, '_open', return_value=io.BytesIO(payload)):
                with self.assertRaisesRegex(downloader.DownloadError, 'mandatory SHA512 pin'):
                    downloader.download_asset('https://example.org/asset.zip', destination, '0' * 128)
            self.assertEqual(destination.read_bytes(), b'previous verified asset')
            self.assertEqual(list(Path(directory).iterdir()), [destination])
            with mock.patch.object(downloader, '_open', return_value=io.BytesIO(payload)):
                downloader.download_asset('https://example.org/asset.zip', destination,
                                          hashlib.sha256(payload).hexdigest(), 'sha256', len(payload))
            self.assertEqual(destination.read_bytes(), payload)
            self.assertEqual(list(Path(directory).iterdir()), [destination])
            mfx_pin = ('5d2ffc4ec2ba0e5859d01d2e072f75436ebc3e62e0f6580b5bb8b9f82fe588e7'
                       '558a46a1fdfa0297a782c0eeb8f50322258d0dd9e41d927cc9be496727b61e44')
            with mock.patch.object(downloader, '_open', side_effect=AssertionError('Network used')):
                downloader.download_asset(
                    'https://github.com/lu-zero/mfx_dispatch/commit/'
                    'd6241243f85a0d947bdfe813006686a930edef24.patch?full_index=1',
                    destination, mfx_pin, expected_size=2816)
            self.assertEqual(hashlib.sha512(destination.read_bytes()).hexdigest(), mfx_pin)
            self.assertEqual(list(Path(directory).iterdir()), [destination])

    def test_release_url_boundary_and_encoded_tag(self):
        self.assertEqual(downloader._github_release(
            'https://github.com/Kitware/CMake/releases/download/v4%2F4/cmake%20file.zip'),
            ('Kitware', 'CMake', 'v4/4', 'cmake file.zip'))
        for url in ('https://github.com.evil.invalid/a/b/releases/download/v1/file',
                    'https://github.com/a/b/archive/v1.tar.gz'):
            self.assertIsNone(downloader._github_release(url))
        for url in ('http://github.com/a/b/releases/download/v1/file',
                    'https://secret@github.com/a/b/releases/download/v1/file',
                    'https://github.com/a/b/releases/download/v1',
                    'https://github.com/a/b/releases/download/v1/file%2Fother'):
            with self.subTest(url=url), self.assertRaises(downloader.DownloadError):
                downloader._github_release(url)

        for url, mapped in (
                ('https://github.com/libjpeg-turbo/libjpeg-turbo/archive/3.2.0.tar.gz',
                 'https://codeload.github.com/libjpeg-turbo/libjpeg-turbo/tar.gz/3.2.0'),
                ('https://github.com/owner/repo/archive/refs/tags/v1.zip',
                 'https://codeload.github.com/owner/repo/zip/refs/tags/v1')):
            self.assertEqual(downloader._github_archive(url), mapped)
        for url in ('https://github.com.evil.invalid/a/b/archive/v1.tar.gz',
                    'https://github.com/a/b/archive/v1.tar.gz?download=1',
                    'https://github.com/a/b/archive/v1.zip#fragment',
                    'https://github.com/a/b/releases/download/v1/file.zip'):
            self.assertIsNone(downloader._github_archive(url))

    def test_auth_is_only_sent_to_github_api_and_removed_on_storage_redirect(self):
        with mock.patch.dict(os.environ, {'GH_TOKEN': 'unit-test-token'}):
            api = downloader._request('https://api.github.com/repos/a/b/releases/assets/1',
                                      'application/octet-stream')
            self.assertEqual(api.get_header('Authorization'), 'Bearer unit-test-token')
            self.assertIsNone(downloader._request('https://github.com/a/b', '*/*')
                              .get_header('Authorization'))
            redirected = downloader._SafeRedirect().redirect_request(
                api, None, 302, 'Found', {},
                'https://release-assets.githubusercontent.com/example?token=not-for-logs')
            self.assertIsNone(redirected.get_header('Authorization'))
            with self.assertRaises(downloader.DownloadError):
                downloader._SafeRedirect().redirect_request(
                    api, None, 302, 'Found', {}, 'http://release-assets.githubusercontent.com/example')


if __name__ == '__main__':
    unittest.main()
