#!/usr/bin/env python3
"""Package a native GPUI device executable as an unsigned, self-signable IPA."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
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
    require(info.get('CFBundleIdentifier') == 'org.sarmg.xscc.gpui.preview', 'Unexpected bundle ID')
    require(info.get('CFBundleExecutable') == 'xscc-gpui-mobile-management', 'Unexpected executable')
    require(info.get('CFBundlePackageType') == 'APPL', 'Not an app')
    require(info.get('CFBundleSupportedPlatforms') == ['iPhoneOS'], 'Not a device app')
    require(info.get('MinimumOSVersion') == '26.0', 'Expected minimum iOS 26.0')
    require(set(info.get('UIDeviceFamily', [])) == {1, 2}, 'Expected iPhone and iPad')
    scene = info.get('UIApplicationSceneManifest', {})
    require(scene.get('UIApplicationSupportsMultipleScenes') is False, 'GPUI supports one scene')
    require(scene.get('UISceneConfigurations', {}).get('UIWindowSceneSessionRoleApplication') == [{
        'UISceneConfigurationName': 'Default Configuration',
        'UISceneDelegateClassName': 'GPUIIosSceneDelegate',
    }], 'Missing GPUI UIKit scene entry point')


def validate_binary(binary):
    require(command('xcrun', 'lipo', '-archs', str(binary)) == 'arm64', 'Expected ARM64 only')
    require('EXECUTE' in command('xcrun', 'otool', '-hv', str(binary)), 'Not a Mach-O executable')
    build = command('xcrun', 'vtool', '-show-build', str(binary))
    require(re.search(r'platform\s+IOS\s', build + '\n'), 'Not an iOS device Mach-O')
    require(re.search(r'minos\s+26\.0(?:\.0)?\s', build + '\n'), 'Expected Mach-O minimum iOS 26.0')
    libraries = command('xcrun', 'otool', '-L', str(binary)).splitlines()[1:]
    for library in libraries:
        require(library.strip().startswith(('/System/Library/', '/usr/lib/')),
                f'Unbundled dynamic dependency: {library}')
    return build


def preview_capabilities(view_only):
    return {
        'management': True,
        'software_video_preview': view_only,
        'audio': False,
        'remote_input': False,
        'clipboard': False,
    }


def package(binary, output, view_only=False):
    require(binary.is_file(), 'Missing built device executable')
    template = Path(__file__).resolve().parents[2] / 'packaging/gpui-ios/Info.plist'
    info = plistlib.loads(template.read_bytes())
    validate_metadata(info)
    build = validate_binary(binary)
    sha = os.environ.get('GITHUB_SHA') or command('git', 'rev-parse', 'HEAD')
    require(re.fullmatch('[0-9a-f]{40}', sha), 'Invalid source revision')
    info['CFBundleVersion'] = os.environ.get('GITHUB_RUN_NUMBER', '1')
    mode = 'view-only' if view_only else 'management'
    name = f'xscc-gpui-{mode}-{sha[:12]}-ios26-arm64-unsigned.ipa'
    output.mkdir(parents=True, exist_ok=True)
    ipa = (output / name).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-gpui-ios-') as temporary:
        payload = Path(temporary) / 'Payload'
        app = payload / 'XSCC.app'
        app.mkdir(parents=True)
        executable = app / info['CFBundleExecutable']
        shutil.copy2(binary, executable)
        executable.chmod(0o755)
        # Apple linkers can emit an ad-hoc signature even without a signing identity.
        if 'LC_CODE_SIGNATURE' in command('xcrun', 'otool', '-l', str(executable)):
            subprocess.run(['codesign', '--remove-signature', str(executable)], check=True)
        require('LC_CODE_SIGNATURE' not in command('xcrun', 'otool', '-l', str(executable)),
                'Unexpected executable signature')
        (app / 'Info.plist').write_bytes(plistlib.dumps(info))
        subprocess.run(['ditto', '-c', '-k', '--keepParent', '--norsrc', str(payload), str(ipa)], check=True)
    with zipfile.ZipFile(ipa) as zipped:
        require(f'Payload/XSCC.app/{info["CFBundleExecutable"]}' in zipped.namelist(), 'Missing executable')
        require(all(n == 'Payload' or n.startswith('Payload/') for n in zipped.namelist()), 'Invalid IPA root')
        require(not any(n.endswith('embedded.mobileprovision') or '/_CodeSignature/' in n
                        for n in zipped.namelist()), 'Unexpected signing material')
    with ipa.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    (output / f'{name}.sha256').write_text(f'{digest}  {name}\n')
    (output / 'build-info.json').write_text(json.dumps({
        'source_commit': sha, 'artifact': name, 'sha256': digest,
        'bundle_id': info['CFBundleIdentifier'], 'build': info['CFBundleVersion'],
        'platform': 'iOS/iPadOS 26 ARM64 physical device',
        'scope': ('GPUI view-only preview: management and software video; not full remote control'
                  if view_only else 'GPUI management preview: binding and authorized host list; no video session'),
        'declared_capabilities': preview_capabilities(view_only),
        'signing': 'UNSIGNED: requires your own matching provisioning and app signature before installation',
        'verified': ['ARM64 device executable', 'minimum iOS 26.0', 'system dynamic dependencies', 'Payload layout'],
        'not_verified': ['physical-device launch', 'management interaction on device', 'authorized remote connection and first video frame', 'signing and provisioning'],
        'xcode': command('xcodebuild', '-version'), 'macho_build': build,
    }, ensure_ascii=False, indent=2) + '\n')
    print(f'Prepared {ipa}; requires user signing before installation.')


if __name__ == '__main__':
    if len(sys.argv) not in (3, 4) or (len(sys.argv) == 4 and sys.argv[3] != '--view-only'):
        raise SystemExit('Usage: package-gpui-ios.py DEVICE_EXECUTABLE OUTPUT_DIRECTORY [--view-only]')
    package(Path(sys.argv[1]), Path(sys.argv[2]), view_only=len(sys.argv) == 4)
