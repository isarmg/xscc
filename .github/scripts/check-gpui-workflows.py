#!/usr/bin/env python3
"""Keep active product automation native and local workflow references valid."""
from pathlib import Path, PurePosixPath
import hashlib
import json
import re
import tomllib
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / '.github/workflows'
REQUIRED = (
    'gpui-windows-beta.yml', 'gpui-unix-beta.yml', 'gpui-android-smoke.yml',
    'gpui-ios-management.yml', 'gpui-windows-surface.yml', 'management.yml',
    'gpui-release.yml',
)
RETIRED = (
    'bridge.yml', 'flutter-build.yml', 'flutter-ci.yml', 'flutter-nightly.yml',
    'flutter-tag.yml', 'mobile-beta.yml', 'ios-beta.yml', 'playground.yml', 'fdroid.yml',
)
SDK = re.compile(
    r'flutter-action|flutter_rust_bridge_codegen|(?-i:flutter/)|'
    r'\bflutter\s+(?:pub|build|test|analyze)\b|--flutter\b', re.I,
)


def check_hwcodec_source(snapshot):
    metadata = json.loads((snapshot / 'upstream.json').read_text())
    changes = set(metadata['local_changes'])
    imported = set()
    for line in (snapshot / metadata['source_inventory']).read_text().splitlines():
        digest, relative = line.split('  ', 1)
        path = PurePosixPath(relative)
        if (not re.fullmatch(r'[0-9a-f]{64}', digest) or path.is_absolute()
                or '..' in path.parts or relative in imported):
            raise ValueError('Invalid hwcodec source inventory entry')
        imported.add(relative)
        if relative not in changes:
            actual = hashlib.sha256((snapshot / relative).read_bytes()).hexdigest()
            if actual != digest:
                raise ValueError(f'Unrecorded hwcodec source modification: {relative}')
    if len(imported) != metadata['source_file_count']:
        raise ValueError('hwcodec imported source count differs from provenance')
    for relative in changes:
        if not (snapshot / relative).is_file():
            raise ValueError(f'Missing recorded hwcodec local change: {relative}')


def check():
    check_hwcodec_source(ROOT / 'vendor/hwcodec')
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    for product in ('experiments/gpui-validation', 'experiments/gpui-mobile-management'):
        if (product not in manifest['workspace']['members']
                or 'workspace' in tomllib.loads((ROOT / product / 'Cargo.toml').read_text())
                or (ROOT / product / 'Cargo.lock').exists()):
            raise ValueError(f'Product must share the core workspace and Cargo.lock: {product}')
    features = manifest['features']
    pending, enabled = list(features.get('default', ())), set()
    while pending:
        feature = pending.pop()
        if feature in enabled:
            continue
        enabled.add(feature)
        dependency = feature.removeprefix('dep:').split('/', 1)[0].rstrip('?')
        if dependency in {'flutter', 'flutter_rust_bridge', 'flutter_rust_bridge_macros', 'dart-sys'}:
            raise ValueError(f'Legacy UI enabled by Cargo defaults: {feature}')
        pending.extend(features.get(feature, ()))
    if 'gpui-session' not in enabled:
        raise ValueError('Cargo defaults must select the native GPUI session')
    legacy = subprocess.run(
        [sys.executable, str(ROOT / 'build.py'), '--flutter', '--dry-run'],
        cwd=ROOT, capture_output=True, text=True, timeout=10, check=False,
    )
    if legacy.returncode == 0 or 'retired' not in legacy.stderr or 'GPUI' not in legacy.stderr:
        raise ValueError('Legacy build.py must reject packaging and direct users to GPUI')
    if 'experiments/gpui-validation/Cargo.toml' not in legacy.stderr:
        raise ValueError('Legacy build.py must identify the native GPUI build manifest')
    for name in REQUIRED:
        if not (WORKFLOWS / name).is_file():
            raise ValueError(f'Missing native product workflow: {name}')
    for name in RETIRED:
        if (WORKFLOWS / name).exists():
            raise ValueError(f'Retired UI workflow is active: {name}')
    for path in sorted((*WORKFLOWS.glob('*.yml'), *WORKFLOWS.glob('*.yaml'))):
        text = path.read_text()
        if SDK.search(text):
            raise ValueError(f'Legacy UI toolchain in active workflow: {path.name}')
        for reference in re.findall(r'uses:\s*(\./[^\s#]+)', text):
            target = ROOT / reference
            if not (target.is_file() or (target / 'action.yml').is_file()
                    or (target / 'action.yaml').is_file()):
                raise ValueError(f'Missing local workflow/action: {path.name}: {reference}')
    print('Native GPUI workflows and local references validated.')


if __name__ == '__main__':
    check()
