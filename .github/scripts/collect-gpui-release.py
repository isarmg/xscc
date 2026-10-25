#!/usr/bin/env python3
"""Collect successful native candidates for one immutable source revision."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import zipfile


def api(endpoint, **parameters):
    arguments = ['gh', 'api', '--method', 'GET', endpoint]
    for key, value in parameters.items():
        arguments += ['-f', f'{key}={value}']
    return json.loads(subprocess.check_output(arguments, text=True))


def expected_artifacts(version, sha):
    return {
        'gpui-windows-beta.yml': {
            f'xscc-{role}-{version}-windows11-x86_64-managed-unsigned': 'windows'
            for role in ('controller', 'host')
        },
        'gpui-unix-beta.yml': {
            f'xscc-{role}-{version}-{platform}-managed-unsigned': 'unix'
            for role in ('controller', 'host') for platform in ('linux-x86_64', 'macos-arm64')
        },
        'gpui-android-smoke.yml': {f'xscc-gpui-android-management-{sha}': 'android'},
        'gpui-ios-management.yml': {
            f'xscc-gpui-interactive-playback-preview-ios26-arm64-unsigned-{sha}': 'ios'
        },
    }


def verify_metadata(directory, kind, sha, version):
    if kind == 'windows':
        records = list(directory.glob('*-build-info.json'))
        if len(records) != 1:
            raise ValueError('Missing unique Windows build metadata')
        info = json.loads(records[0].read_text())
        valid = info['source_sha'] == sha and info['package_version'] == version and info['package_mode'] == 'managed'
    elif kind == 'ios':
        info = json.loads((directory / 'build-info.json').read_text())
        valid = info['source_commit'] == sha and info['test_lan_direct'] is False
    else:
        file = 'build-manifest.txt' if kind == 'unix' else 'BUILD-INFO.txt'
        info = dict(line.split('=', 1) for line in (directory / file).read_text().splitlines() if '=' in line)
        valid = (info.get('source') == sha and info.get('package_mode') == 'managed') if kind == 'unix' else (
            info.get('commit') == sha and info.get('test_lan_direct') == 'false')
    if not valid:
        raise ValueError(f'Wrong source revision or test-only capability in {kind} candidate')


def verify_checksums(directory):
    manifests = [file for file in directory.iterdir() if file.name.endswith(('.sha256', 'SHA256SUMS.txt')) or file.name == 'SHA256SUMS']
    if not manifests:
        raise ValueError('Candidate has no checksum manifest')
    for manifest in manifests:
        for line in manifest.read_text().splitlines():
            match = re.fullmatch(r'([0-9a-fA-F]{64})\s+\*?(.+)', line)
            if not match:
                raise ValueError(f'Invalid checksum record in {manifest.name}')
            name = match[2]
            file = directory / name
            if Path(name).name != name or not file.is_file():
                raise ValueError(f'Invalid checksum target: {name}')
            if hashlib.sha256(file.read_bytes()).hexdigest() != match[1].lower():
                raise ValueError(f'Checksum mismatch: {name}')


def extract(archive, output):
    with zipfile.ZipFile(archive) as zipped:
        for entry in zipped.infolist():
            name = Path(entry.filename)
            if name.is_absolute() or '..' in name.parts or '\\' in entry.filename:
                raise ValueError('Invalid artifact archive path')
        zipped.extractall(output)


def collect(repo, sha, version, output):
    output.mkdir(parents=True, exist_ok=False)
    provenance = []
    for workflow, expected in expected_artifacts(version, sha).items():
        runs = api(f'repos/{repo}/actions/workflows/{workflow}/runs', head_sha=sha, event='push', per_page=100)['workflow_runs']
        runs = [run for run in runs if run['head_sha'] == sha and run['head_branch'] in ('main', 'feat/management-prep')]
        if not runs:
            raise ValueError(f'No source-matched build for {workflow}')
        run = max(runs, key=lambda run: run['id'])
        if run['status'] != 'completed' or run['conclusion'] != 'success':
            raise ValueError(f'{workflow} has not passed: {run["html_url"]}')
        artifacts = api(f'repos/{repo}/actions/runs/{run["id"]}/artifacts', per_page=100)['artifacts']
        for name, kind in expected.items():
            matches = [artifact for artifact in artifacts if artifact['name'] == name and not artifact['expired']]
            if len(matches) != 1:
                raise ValueError(f'Missing unique release artifact: {name}')
            artifact = matches[0]
            with tempfile.TemporaryDirectory() as temporary:
                temporary = Path(temporary)
                archive = temporary / 'artifact.zip'
                with archive.open('wb') as stream:
                    subprocess.run(['gh', 'api', f'repos/{repo}/actions/artifacts/{artifact["id"]}/zip'], stdout=stream, check=True)
                digest = 'sha256:' + hashlib.sha256(archive.read_bytes()).hexdigest()
                if artifact.get('digest') != digest:
                    raise ValueError(f'Artifact archive digest mismatch: {name}')
                directory = temporary / 'files'
                extract(archive, directory)
                verify_metadata(directory, kind, sha, version)
                verify_checksums(directory)
                for file in directory.iterdir():
                    if not file.is_file():
                        raise ValueError(f'Unexpected nested release payload: {name}')
                    destination = file.name if file.name.startswith('xscc-') else f'{name}--{file.name}'
                    if (output / destination).exists():
                        raise ValueError(f'Release filename collision: {destination}')
                    shutil.copy2(file, output / destination)
            provenance.append({'artifact': name, 'id': artifact['id'], 'digest': digest, 'run': run['html_url'], 'source_sha': sha})
    (output / 'release-provenance.json').write_text(json.dumps({'source_sha': sha, 'version': version, 'artifacts': provenance}, indent=2) + '\n')
    files = sorted(output.iterdir())
    (output / 'SHA256SUMS').write_text(''.join(f'{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}\n' for file in files))
    print(f'Collected {len(provenance)} source-matched native candidates, {len(files) + 1} release assets')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.sha) or not re.fullmatch(r'\d+\.\d+\.\d+-beta\.\d+', args.version):
        parser.error('Expected a complete commit SHA and a beta package version')
    collect(os.environ['GITHUB_REPOSITORY'], args.sha, args.version, args.output)
