#!/usr/bin/env python3
import copy
import hashlib
import importlib.util
import json
import re
from pathlib import Path
import tempfile
import unittest
from unittest import mock

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

    @staticmethod
    def native_jobs(workflow, sha):
        jobs = [dict(id=index, name=name, run_id=17, head_sha=sha,
                     status='completed', conclusion='success',
                     steps=[dict(name=step, status='completed', conclusion='success') for step in steps])
                for index, (name, steps) in enumerate(release.required_native_jobs(workflow).items(), 1)]
        return dict(total_count=len(jobs), jobs=jobs)

    def test_collect_accepts_main_and_feature_only_for_the_exact_successful_source(self):
        sha = 'a' * 40
        cases = [(workflow, 'workflow_dispatch', 'main', sha, 'success', True)
                 for workflow in release.expected_artifacts('0.1.0-beta.2', sha)]
        cases += [('gpui-windows-beta.yml', event, branch, source, conclusion, accepted)
                  for event, branch, source, conclusion, accepted in (
                      ('push', 'main', sha, 'success', True),
                      ('push', 'feat/management-prep', sha, 'success', True),
                      ('push', 'unreviewed', sha, 'success', False),
                      ('push', 'main', 'b' * 40, 'success', False),
                      ('push', 'main', sha, 'failure', False),
                      ('pull_request', 'main', sha, 'success', False),
                  )]
        for workflow, event, branch, source, conclusion, accepted in cases:
            with self.subTest(workflow=workflow, event=event, branch=branch, source=source, conclusion=conclusion), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / 'release'
                run = dict(id=17, head_sha=source, head_branch=branch, event=event,
                           status='completed', conclusion=conclusion, html_url='https://example.invalid/run/17')
                def api(endpoint, **parameters):
                    if endpoint.endswith('/' + workflow + '/runs'):
                        self.assertEqual(parameters, {'head_sha': sha, 'per_page': 100})
                        return {'workflow_runs': [run]}
                    if endpoint.endswith('/runs/17/jobs'):
                        self.assertEqual(parameters, {'filter': 'latest', 'per_page': 100})
                        return self.native_jobs(workflow, sha)
                    self.assertEqual(endpoint, 'repos/isarmg/xscc/actions/runs/17/artifacts')
                    return {'artifacts': []}
                with mock.patch.object(release, 'expected_artifacts', return_value={workflow: {}}), mock.patch.object(release, 'api', side_effect=api):
                    if accepted:
                        release.collect('isarmg/xscc', sha, '0.1.0-beta.4', output)
                        self.assertEqual(json.loads((output / 'release-provenance.json').read_text())['source_sha'], sha)
                    else:
                        with self.assertRaises(ValueError):
                            release.collect('isarmg/xscc', sha, '0.1.0-beta.4', output)

    def test_collect_rejects_build_only_partial_or_foreign_native_validation(self):
        sha = 'a' * 40
        for workflow, damage in (
            ('gpui-windows-beta.yml', 'build-only'),
            ('gpui-android-smoke.yml', 'build-only'),
            ('gpui-unix-beta.yml', 'missing-role'),
            ('gpui-ios-management.yml', 'missing-role'),
            ('gpui-unix-beta.yml', 'foreign-source'),
            ('gpui-ios-management.yml', 'failed-job'),
            ('gpui-windows-beta.yml', 'missing-step'),
            ('gpui-android-smoke.yml', 'incomplete-page'),
        ):
            with self.subTest(workflow=workflow, damage=damage), tempfile.TemporaryDirectory() as temporary:
                records = copy.deepcopy(self.native_jobs(workflow, sha))
                if damage == 'build-only':
                    for job in records['jobs']:
                        job['steps'][0]['conclusion'] = 'skipped'
                elif damage == 'missing-role':
                    records['jobs'].pop()
                    records['total_count'] -= 1
                elif damage == 'foreign-source':
                    records['jobs'][0]['head_sha'] = 'b' * 40
                elif damage == 'failed-job':
                    records['jobs'][0]['conclusion'] = 'failure'
                elif damage == 'missing-step':
                    records['jobs'][0]['steps'].pop()
                else:
                    records['total_count'] += 1
                run = dict(id=17, head_sha=sha, head_branch='main', event='workflow_dispatch',
                           status='completed', conclusion='success', html_url='https://example.invalid/run/17')
                def api(endpoint, **parameters):
                    if endpoint.endswith('/runs'):
                        return {'workflow_runs': [run]}
                    self.assertTrue(endpoint.endswith('/runs/17/jobs'))
                    return records
                with mock.patch.object(release, 'expected_artifacts', return_value={workflow: {}}), mock.patch.object(release, 'api', side_effect=api):
                    with self.assertRaises(ValueError):
                        release.collect('isarmg/xscc', sha, '0.1.0-beta.4', Path(temporary) / 'release')

    def test_required_validation_names_match_current_native_producers(self):
        workflows = Path(__file__).resolve().parents[1] / 'workflows'
        for workflow in release.expected_artifacts('0.1.0-beta.2', 'a' * 40):
            declared = set(re.findall(r'^\s+- name: (.+)$', (workflows / workflow).read_text(), re.MULTILINE))
            for job, steps in release.required_native_jobs(workflow).items():
                with self.subTest(workflow=workflow, job=job):
                    self.assertTrue(set(steps) <= declared, set(steps) - declared)

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
