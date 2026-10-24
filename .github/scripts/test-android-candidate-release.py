#!/usr/bin/env python3
"""Offline regression checks for the one verified Android release publisher."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.request

spec = importlib.util.spec_from_file_location('publisher', Path(__file__).with_name('publish-android-candidate.py'))
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)


def metadata():
    return ({'id': p.RUN, 'head_sha': p.SOURCE, 'path': '.github/workflows/gpui-android-smoke.yml',
             'status': 'completed', 'conclusion': 'success', 'repository': {'full_name': p.REPOSITORY}},
            {'id': p.ARTIFACT, 'name': p.ARTIFACT_NAME, 'digest': 'sha256:' + p.ZIP_SHA,
             'expired': False, 'workflow_run': {'id': p.RUN, 'head_sha': p.SOURCE}})


class FakeGitHub:
    def __init__(self):
        self.run, self.artifact = metadata()
        self.release = {'id': p.RELEASE, 'tag_name': p.TAG, 'target_commitish': p.SOURCE,
                        'draft': True, 'prerelease': True, 'assets': [], 'html_url': 'https://github.com/isarmg/xscc/releases/tag/' + p.TAG,
                        'upload_url': 'https://uploads.github.com/repos/isarmg/xscc/releases/' + str(p.RELEASE) + '/assets{?name,label}'}
        self.writes = []
        self.downloads = 0

    def json(self, path, method='GET', value=None):
        if path == '/releases/' + str(p.RELEASE):
            if method == 'PATCH':
                self.writes.append((method, value))
                self.release.update(value)
            return copy.deepcopy(self.release)
        if path.startswith('/git/ref/tags/'):
            return {'object': {'type': 'commit', 'sha': p.SOURCE}}
        if path.startswith('/actions/runs/'):
            return self.run
        if path.startswith('/actions/artifacts/'):
            return self.artifact
        raise AssertionError((method, path))

    def download(self, path):
        self.downloads += 1
        path.write_bytes(b'candidate fixture')

    def upload(self, release, name, data):
        item = {'name': name, 'size': len(data), 'digest': 'sha256:' + p.digest(data), 'state': 'uploaded'}
        self.writes.append(('upload', name))
        self.release['assets'].append(item)
        return item


class PublishingTests(unittest.TestCase):
    def test_candidate_source_workflow_and_digest_must_match(self):
        run, artifact = metadata()
        p.validate_candidate_metadata(run, artifact, p.SOURCE, p.RUN)
        for field, value in [('head_sha', '0' * 40), ('conclusion', 'failure'), ('path', '.github/workflows/untrusted.yml')]:
            with self.subTest(field=field), self.assertRaises(p.InvalidCandidate):
                p.validate_candidate_metadata(dict(run, **{field: value}), artifact, p.SOURCE, p.RUN)
        for field, value in [('digest', 'sha256:' + '0' * 64), ('expired', True), ('name', 'smoke')]:
            with self.subTest(field=field), self.assertRaises(p.InvalidCandidate):
                p.validate_candidate_metadata(run, dict(artifact, **{field: value}), p.SOURCE, p.RUN)

    def test_redirect_and_existing_assets_fail_closed(self):
        request = urllib.request.Request(p.API + '/actions/artifacts/1/zip', headers={'Authorization': 'Bearer fixture'})
        handler = p.SafeRedirect()
        redirected = handler.redirect_request(request, None, 302, 'Found', {}, 'https://example.blob.core.windows.net/candidate.zip')
        self.assertFalse(redirected.has_header('Authorization'))
        with self.assertRaises(p.InvalidCandidate):
            handler.redirect_request(request, None, 302, 'Found', {}, 'http://example.invalid/archive.zip')
        with self.assertRaises(p.InvalidCandidate):
            p.check_assets([{'name': p.APK_NAME, 'size': 0, 'state': 'starter', 'digest': None}], complete=False)
        with self.assertRaises(p.InvalidCandidate):
            p.check_assets([], complete=True)
        with self.assertRaises(p.InvalidCandidate):
            p.check_release(dict(FakeGitHub().release, target_commitish='0' * 40))

    def test_draft_publishes_only_after_digests_and_published_rerun_is_read_only(self):
        github = FakeGitHub()
        data = b'APK fixture'
        def archive(_, directory):
            apk = directory / p.APK_NAME
            apk.write_bytes(data)
            return apk
        with patch.object(p, 'APK_SHA', p.digest(data)), patch.object(p, 'APK_SIZE', len(data)), \
                patch.object(p, 'validate_archive', side_effect=archive), patch.object(p, 'validate_apk'):
            url = p.publish(github, p.SOURCE, p.RUN, Path('/unused'))
            self.assertEqual(url, github.release['html_url'])
            self.assertEqual(len(github.writes), 4)
            self.assertEqual(github.writes[-1], ('PATCH', {'draft': False, 'prerelease': True, 'make_latest': 'false'}))
            writes = copy.deepcopy(github.writes)
            self.assertEqual(p.publish(github, p.SOURCE, p.RUN, Path('/unused')), url)
            self.assertEqual(github.writes, writes)
            self.assertEqual(github.downloads, 1)
            github.release['assets'][0]['digest'] = 'sha256:' + '0' * 64
            with self.assertRaises(p.InvalidCandidate):
                p.publish(github, p.SOURCE, p.RUN, Path('/unused'))
            self.assertEqual(github.writes, writes)


if __name__ == '__main__':
    unittest.main()
