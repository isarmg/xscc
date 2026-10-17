#!/usr/bin/env python3
"""Validate a device archive and package an unsigned, user-signable IPA on macOS."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile
import zipfile


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_metadata(info):
    require(info.get('CFBundleIdentifier') == 'org.sarmg.xscc.beta', 'Unexpected bundle ID')
    require(info.get('CFBundlePackageType') == 'APPL', 'Not an application bundle')
    require(info.get('CFBundleSupportedPlatforms') == ['iPhoneOS'], 'Not a physical-device app')
    require(info.get('MinimumOSVersion') == '26.0', 'Expected iOS 26.0 deployment target')
    require(set(info.get('UIDeviceFamily', [])) == {1, 2}, 'Expected iPhone and iPad support')


def validate_macho(path, main=False):
    require(command('xcrun', 'lipo', '-archs', str(path)) == 'arm64', f'Not ARM64-only: {path}')
    build = command('xcrun', 'vtool', '-show-build', str(path))
    require(re.search(r'platform\s+IOS\s', build + '\n') is not None,
            f'Not an iOS device Mach-O: {path}')
    if main:
        require(re.search(r'minos\s+26\.0(?:\.0)?\s', build + '\n') is not None,
                'Runner Mach-O deployment target is not 26.0')
    return build


def package(archive, output):
    app = archive / 'Products/Applications/Runner.app'
    require(app.is_dir(), f'Missing device archive application: {app}')
    with (app / 'Info.plist').open('rb') as source:
        info = plistlib.load(source)
    validate_metadata(info)
    executable = app / info['CFBundleExecutable']
    require(executable.is_file() and os.access(executable, os.X_OK), 'Runner is not executable')
    macho = validate_macho(executable, main=True)
    for name in ('App', 'Flutter'):
        binary = app / f'Frameworks/{name}.framework/{name}'
        require(binary.is_file(), f'Missing embedded {name} framework')
        validate_macho(binary)
    require((app / 'Frameworks/App.framework/flutter_assets').is_dir(), 'Missing Flutter assets')
    symbols = command('xcrun', 'nm', '-gU', str(executable))
    exported = {line.split()[-1] for line in symbols.splitlines() if line.split()}
    require('_session_get_rgba' in exported, 'Missing linked native RGBA bridge')
    header = Path(__file__).resolve().parents[2] / 'flutter/ios/Runner/bridge_generated.h'
    bridge_symbols = {'_' + name for name in re.findall(r'\b(wire_\w+)\s*\(', header.read_text())}
    require(bool(bridge_symbols & exported),
            'No generated header wire entry point is exported by Runner')
    require(not (app / 'embedded.mobileprovision').exists(), 'Unexpected provisioning profile')
    require(not (app / '_CodeSignature').exists(), 'Unexpected application signature')
    sha = os.environ.get('GITHUB_SHA') or command('git', 'rev-parse', 'HEAD')
    require(re.fullmatch(r'[0-9a-f]{40}', sha) is not None, 'Invalid source revision')
    name = f'xscc-{info["CFBundleShortVersionString"]}-{sha[:12]}-ios26-arm64-unsigned.ipa'
    output.mkdir(parents=True, exist_ok=True)
    ipa = (output / name).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-ios-') as temporary:
        payload = Path(temporary) / 'Payload'
        payload.mkdir()
        # ditto preserves executable modes and framework symlinks; keep Payload at ZIP root.
        subprocess.run(['ditto', str(app), str(payload / app.name)], check=True)
        subprocess.run(['ditto', '-c', '-k', '--keepParent', '--norsrc', str(payload), str(ipa)], check=True)
    with zipfile.ZipFile(ipa) as zipped:
        require(f'Payload/Runner.app/{info["CFBundleExecutable"]}' in zipped.namelist(),
                'IPA does not contain the application executable')
        require(all(name == 'Payload' or name.startswith('Payload/') for name in zipped.namelist()),
                'Unexpected IPA root')
    with ipa.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    (output / f'{name}.sha256').write_text(f'{digest}  {name}\n')
    metadata = {
        'source_commit': sha,
        'artifact': name,
        'sha256': digest,
        'bundle_id': info['CFBundleIdentifier'],
        'version': info['CFBundleShortVersionString'],
        'build': info['CFBundleVersion'],
        'platform': 'iOS/iPadOS 26 ARM64 physical device',
        'role': 'controller only',
        'signing': 'UNSIGNED: user must sign app and embedded frameworks with matching provisioning before installation',
        'verified': ['device archive metadata', 'ARM64 device Mach-O and minimum OS', 'Flutter frameworks and assets', 'linked Rust bridge symbols', 'Payload layout'],
        'not_verified': ['signature and provisioning', 'physical-device installation', 'live remote session'],
        'xcode': command('xcodebuild', '-version'),
        'iphoneos_sdk': command('xcrun', '--sdk', 'iphoneos', '--show-sdk-version'),
        'runner_build_commands': macho,
    }
    (output / 'build-info.json').write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + '\n')
    print(f'Prepared {ipa}; unsigned and not directly installable.')


if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit('Usage: package-ios-beta.py ARCHIVE.xcarchive OUTPUT_DIRECTORY')
    package(Path(sys.argv[1]), Path(sys.argv[2]))
