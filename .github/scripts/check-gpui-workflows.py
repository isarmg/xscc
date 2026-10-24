#!/usr/bin/env python3
"""Keep active product automation native and local workflow references valid."""
from pathlib import Path
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


def check():
    features = tomllib.loads((ROOT / 'Cargo.toml').read_text())['features']
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
