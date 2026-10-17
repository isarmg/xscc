#!/usr/bin/env python3
"""Bounded CLI checks; these do not establish GUI or physical GPU support."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


def check(binary):
    def run(*arguments, timeout=15):
        return subprocess.run([str(binary), *arguments], capture_output=True,
                              text=True, timeout=timeout, check=False)

    help_result = run('--help')
    if help_result.returncode != 0 or not help_result.stdout.strip():
        raise ValueError(f'--help failed: {help_result.stderr}')
    rejected = run('--check-hwcodec-config')
    prefix = 'RUSTDESK_HWCODEC_CONFIG='
    if rejected.returncode != 2 or prefix in rejected.stdout:
        raise ValueError('Codec probe without --stdout must fail with exit code 2')
    probe = run('--check-hwcodec-config', '--stdout', timeout=45)
    if probe.returncode != 0:
        raise ValueError(f'Codec probe failed ({probe.returncode}): {probe.stderr}')
    lines = [line[len(prefix):] for line in probe.stdout.splitlines() if line.startswith(prefix)]
    if len(lines) != 1:
        raise ValueError('Expected exactly one codec configuration protocol line')
    config = json.loads(lines[0])
    if (not isinstance(config, dict) or 'signature' not in config
            or not isinstance(config.get('ram_encode'), list)
            or not isinstance(config.get('ram_decode'), list)):
        raise ValueError('Invalid codec configuration object')
    print('CLI protocol passed; empty codec lists are valid. GUI, capture and physical GPU support remain unverified.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    args = parser.parse_args()
    try:
        check(args.binary.resolve(strict=True))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        sys.exit(f'CLI validation failed: {error}')
