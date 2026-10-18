#!/usr/bin/env python3
"""Reject undeclared test capabilities before packaging; never starts a session."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def check(binary, mode):
    if mode not in ('managed', 'test-lan-direct'):
        raise ValueError('Expected package mode managed or test-lan-direct')
    result = subprocess.run([str(binary), '--build-capabilities'], capture_output=True,
                            text=True, timeout=15, check=False)
    expected = f'XSCC_TEST_LAN_DIRECT={int(mode == "test-lan-direct")}'
    if mode == 'test-lan-direct':
        expected += '\ntest_ip_scope=any-reachable-ipv4-or-ipv6'
    if result.returncode != 0 or result.stdout.strip() != expected:
        raise ValueError(f'Binary capabilities do not match explicit package mode {mode}: '
                         f'{result.stdout!r} {result.stderr!r}')
    return {'mode': mode, 'test_lan_direct': mode == 'test-lan-direct',
            'test_ip_scope': 'any-reachable-ipv4-or-ipv6' if mode == 'test-lan-direct' else None,
            'distribution': 'TEST ONLY - never promote to a production release'
            if mode == 'test-lan-direct' else 'managed beta candidate'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        metadata = check(args.binary.resolve(strict=True), os.environ.get('XSCC_PACKAGE_MODE', 'managed'))
        if args.output:
            args.output.write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
            if metadata['test_lan_direct']:
                args.output.with_name('TEST-ONLY.txt').write_text(
                    'TEST ONLY: unauthenticated unrestricted IP direct access is enabled.\n'
                    'Any reachable IP is allowed; anyone reaching the test listener can control the host. Never distribute as a production release.\n\n'
                    + (Path(__file__).resolve().parents[2] / 'docs/test-lan-direct.md').read_text(encoding='utf-8'),
                    encoding='utf-8')
        print(json.dumps(metadata))
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        sys.exit(f'Package capability gate failed: {error}')
