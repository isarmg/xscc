#!/usr/bin/env python3
"""Download a pinned build asset without forwarding GitHub credentials to storage."""
import argparse
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import time
from urllib import error, parse, request


HTTP_TIMEOUT = 30
TOTAL_TIMEOUT = 600
METADATA_LIMIT = 4 * 1024 * 1024
CHUNK_SIZE = 1024 * 1024


class DownloadError(Exception):
    pass


def _https_url(url):
    try:
        parts = parse.urlsplit(url)
        parts.port
    except ValueError:
        raise DownloadError('Invalid download URL') from None
    if (parts.scheme != 'https' or not parts.hostname
            or parts.username is not None or parts.password is not None):
        raise DownloadError('Only HTTPS URLs without credentials are supported')
    return parts


def _github_release(url):
    parts = _https_url(url)
    if parts.hostname.lower() != 'github.com' or parts.port not in (None, 443):
        return None
    pieces = parts.path.split('/')
    if len(pieces) < 5 or pieces[3:5] != ['releases', 'download']:
        return None
    if len(pieces) < 7 or not all(pieces[1:]):
        raise DownloadError('Invalid GitHub release asset URL')
    owner, repo = pieces[1:3]
    if not all(re.fullmatch(r'[A-Za-z0-9_.-]+', value)
               and value not in ('.', '..') for value in (owner, repo)):
        raise DownloadError('Invalid GitHub repository in asset URL')
    tag = parse.unquote('/'.join(pieces[5:-1]))
    name = parse.unquote(pieces[-1])
    if not tag or not name or '/' in name or '\\' in name:
        raise DownloadError('Invalid GitHub release tag or asset name')
    return owner, repo, tag, name


class _SafeRedirect(request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parts = _https_url(newurl)
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and (parts.hostname.lower() != 'api.github.com'
                                       or parts.port not in (None, 443)):
            redirected.remove_header('Authorization')
        return redirected


def _request(url, accept):
    parts = _https_url(url)
    headers = {'User-Agent': 'xscc-pinned-asset-downloader', 'Accept': accept}
    if parts.hostname.lower() == 'api.github.com' and parts.port in (None, 443):
        headers['X-GitHub-Api-Version'] = '2022-11-28'
        token = os.environ.get('GH_TOKEN', '').strip()
        if token:
            headers['Authorization'] = 'Bearer ' + token
    return request.Request(url, headers=headers)


def _remaining(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise DownloadError('Asset download exceeded its total time budget')
    return min(HTTP_TIMEOUT, remaining)


def _open(url, accept, deadline):
    return request.build_opener(_SafeRedirect()).open(
        _request(url, accept), timeout=_remaining(deadline))


def _github_asset(release, deadline):
    owner, repo, tag, name = release
    endpoint = (f'https://api.github.com/repos/{owner}/{repo}/releases/tags/'
                + parse.quote(tag, safe=''))
    payload = bytearray()
    with _open(endpoint, 'application/vnd.github+json', deadline) as response:
        while True:
            _remaining(deadline)
            chunk = response.read1(min(CHUNK_SIZE, METADATA_LIMIT + 1 - len(payload)))
            if not chunk:
                break
            payload.extend(chunk)
            if len(payload) > METADATA_LIMIT:
                raise DownloadError('GitHub release metadata exceeds the size limit')
    try:
        metadata = json.loads(payload)
        assets = metadata['assets']
        matches = [asset for asset in assets if asset.get('name') == name]
        if len(matches) != 1:
            raise ValueError()
        asset = matches[0]
        asset_id, size = asset['id'], asset['size']
        if (type(asset_id) is not int or asset_id <= 0
                or type(size) is not int or size < 0):
            raise ValueError()
    except (ValueError, KeyError, TypeError, AttributeError):
        raise DownloadError('GitHub release metadata has no unique matching asset') from None
    return (f'https://api.github.com/repos/{owner}/{repo}/releases/assets/{asset_id}',
            size, asset.get('digest'))


def _digest(algorithm, expected):
    if algorithm not in ('sha256', 'sha512') or not isinstance(expected, str):
        raise DownloadError('Expected a SHA256 or SHA512 pin')
    if not re.fullmatch(r'[0-9a-fA-F]{' + str(hashlib.new(algorithm).digest_size * 2) + '}', expected):
        raise DownloadError('Invalid pinned digest')
    return expected.lower()


def download_asset(url, destination, expected_digest, algorithm='sha512', expected_size=None):
    """Verify the caller's mandatory pin before atomically replacing destination."""
    expected_digest = _digest(algorithm, expected_digest)
    if expected_size is not None and (type(expected_size) is not int or expected_size < 0):
        raise DownloadError('Expected size must be a nonnegative integer')
    release = _github_release(url)
    deadline = time.monotonic() + TOTAL_TIMEOUT
    temporary = None
    phase = 'GitHub release metadata' if release else 'HTTPS asset download'
    try:
        api_size, api_digest = None, None
        if release:
            url, api_size, api_digest = _github_asset(release, deadline)
            if expected_size is not None and expected_size != api_size:
                raise DownloadError('GitHub asset size disagrees with the caller pin')
        supplemental = None
        if isinstance(api_digest, str) and ':' in api_digest:
            api_algorithm, value = api_digest.split(':', 1)
            if api_algorithm in ('sha256', 'sha512'):
                supplemental = (hashlib.new(api_algorithm), _digest(api_algorithm, value))
        phase = 'GitHub asset API download' if release else phase
        destination = Path(destination)
        destination.parent.mkdir(parents=True, exist_ok=True)
        checksum, length = hashlib.new(algorithm), 0
        with tempfile.NamedTemporaryFile(dir=destination.parent, prefix='.' + destination.name + '.',
                                         suffix='.part', delete=False) as output:
            temporary = Path(output.name)
            with _open(url, 'application/octet-stream', deadline) as response:
                while True:
                    _remaining(deadline)
                    chunk = response.read1(CHUNK_SIZE)
                    if not chunk:
                        break
                    length += len(chunk)
                    if any(size is not None and length > size for size in (expected_size, api_size)):
                        raise DownloadError('Downloaded asset exceeds its pinned size')
                    checksum.update(chunk)
                    if supplemental:
                        supplemental[0].update(chunk)
                    output.write(chunk)
        _remaining(deadline)
        if any(size is not None and length != size for size in (expected_size, api_size)):
            raise DownloadError('Downloaded asset size does not match its pin')
        if not hmac.compare_digest(checksum.hexdigest(), expected_digest):
            raise DownloadError('Downloaded asset does not match its mandatory ' + algorithm.upper() + ' pin')
        if supplemental and not hmac.compare_digest(supplemental[0].hexdigest(), supplemental[1]):
            raise DownloadError('Downloaded asset disagrees with the supplemental GitHub digest')
        os.replace(temporary, destination)
        temporary = None
        return destination
    except error.HTTPError as exc:
        raise DownloadError(f'{phase} failed (HTTP {exc.code})') from None
    except (error.URLError, TimeoutError) as exc:
        raise DownloadError(f'{phase} failed ({type(exc).__name__})') from None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', required=True, type=Path)
    pins = parser.add_mutually_exclusive_group(required=True)
    pins.add_argument('--sha256')
    pins.add_argument('--sha512')
    parser.add_argument('--size', type=int)
    args = parser.parse_args()
    try:
        download_asset(args.url, args.output, args.sha256 or args.sha512,
                       'sha256' if args.sha256 else 'sha512', args.size)
    except (DownloadError, OSError) as exc:
        print(f'Pinned asset download failed: {exc}', file=sys.stderr)
        return 1
    print('Pinned asset verified and saved')
    return 0


if __name__ == '__main__':
    sys.exit(main())
