#!/usr/bin/env python3
"""Stage the pinned AGP, Kotlin, and lint Maven artifacts outside the checkout."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
from pathlib import Path, PurePosixPath
import re

from pinned_asset_download import DownloadError, download_asset


REPOSITORIES = {
    'google': 'https://dl.google.com/dl/android/maven2/',
    'central': 'https://repo.maven.apache.org/maven2/',
}
CHECKOUT = Path(__file__).resolve().parents[2]
ANDROID_PACKAGE = CHECKOUT / 'packaging/gpui-android'
MANIFEST = ANDROID_PACKAGE / 'toolchain-maven.sha256'


def prepare(destination):
    if destination.is_relative_to(CHECKOUT):
        raise ValueError('The Android toolchain cache must be outside the checkout')

    artifacts = []
    for line in MANIFEST.read_text(encoding='ascii').splitlines():
        digest, _, relative = line.partition('  ')
        path = PurePosixPath(relative)
        if (not re.fullmatch(r'[a-f0-9]{64}', digest) or len(path.parts) < 2
                or path.parts[0] not in REPOSITORIES or '..' in path.parts):
            raise ValueError(f'Invalid toolchain manifest entry {line!r}')
        if (ANDROID_PACKAGE / 'vendor/maven' / path).exists():
            raise ValueError(f'Toolchain artifact is present in the application snapshot: {path}')
        repository, *coordinates = path.parts
        artifacts.append((digest, path, REPOSITORIES[repository] + '/'.join(coordinates)))

    def stage(artifact):
        digest, relative, url = artifact
        path = destination / relative
        if path.is_file():
            with path.open('rb') as cached:
                if hashlib.file_digest(cached, 'sha256').hexdigest() == digest:
                    return
        try:
            download_asset(url, path, digest, algorithm='sha256')
        except DownloadError as error:
            raise ValueError(f'{relative} could not be staged. {error}') from error

    with ThreadPoolExecutor(max_workers=8) as pool:
        for _ in pool.map(stage, artifacts):
            pass
    print(f'Verified {len(artifacts)} Android toolchain artifacts in {destination}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path, help='External Maven cache directory')
    args = parser.parse_args()
    try:
        prepare(args.destination.expanduser().resolve())
    except (OSError, ValueError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
