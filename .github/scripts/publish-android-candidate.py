#!/usr/bin/env python3
"""Publish one already-verified APK; this never builds application source."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile

REPOSITORY = 'isarmg/xscc'
SOURCE = '9918623889e2e4e77e988c254a590496a53595e4'
RUN = 40109234041
ARTIFACT = 12566012112
RELEASE = 422829541
VERSION = '0.1.0-beta.3.2'
TAG = 'android-v' + VERSION
ZIP_SHA = 'cb1c1ab5e0d20d46527ee93b4c7420706494d45cedba94933ef5b92acfcf4ca0'
APK_SHA = '69bfdd7ca7a3eef57ea9d6586e80c8ed566e5726450f14412ac5304869475d91'
APK_SIZE = 57160484
CERT_SHA = '7e2584d12ca65c34bc01facc08b77657764c6405f0c9e2050bdd1df84b2753c9'
ARTIFACT_NAME = 'xscc-gpui-android-management-' + SOURCE
ARTIFACT_APK = 'xscc-gpui-android-arm64-management-' + SOURCE[:12] + '.apk'
APK_NAME = 'XSCC-Android-' + VERSION + '-arm64.apk'
API = 'https://api.github.com/repos/' + REPOSITORY
INSTALL_BYTES = """XSCC 安卓控制端预览版 0.1.0-beta.3.2
适用：Android 10及以上，ARM64。版本号3，应用名“XSCC 控制端预览”。

本版修复实例绑定密码输入：仅清理首尾空白，提交前按共享协议检查36位小写英文字母或数字。内部字符与大小写不会被静默改写；无效输入保留以便修正，错误说明不会显示密码。

已安装beta.3.1的用户可直接覆盖安装本版：包名与固定预览签名保持一致，版本号由2升至3，本机绑定和设置保留，无需先卸载。
更早使用一次性随机签名的旧预览包与固定预览签名不同，只有这些旧包首次替换需要卸载。卸载会清除本机数据；若实例已绑定，须在服务端“重置实例密码”后重新绑定。

保留beta.3.1的密码键盘提交、长按粘贴、受保护管理存储和私网HTTP/WS管理连接修复。本地输入框粘贴不依赖远程会话，远程剪贴板同步仍未启用。

APK已通过CI原生跨编译、DEX隐私、签名、ARM64及16KiB对齐门禁；尚未进行本机连接安卓设备的实机验收。
源码：9918623889e2e4e77e988c254a590496a53595e4
构建：https://github.com/isarmg/xscc/actions/runs/40109234041
APK SHA256：69bfdd7ca7a3eef57ea9d6586e80c8ed566e5726450f14412ac5304869475d91
固定预览证书SHA256：7e2584d12ca65c34bc01facc08b77657764c6405f0c9e2050bdd1df84b2753c9
""".encode("utf-8")


class InvalidCandidate(RuntimeError):
    pass


def require(condition, reason):
    if not condition:
        raise InvalidCandidate(reason)


def digest(data):
    return hashlib.sha256(data).hexdigest()


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        target = urllib.parse.urlsplit(new_url)
        require(target.scheme == 'https' and target.hostname and not target.username
                and not target.password, 'Refusing a non-HTTPS download redirect')
        require(request.get_method() in ('GET', 'HEAD'), 'Refusing a mutation redirect')
        redirected = super().redirect_request(request, response, code, message, headers, new_url)
        if urllib.parse.urlsplit(request.full_url).netloc != target.netloc:
            redirected.remove_header('Authorization')
        return redirected


class GitHubError(RuntimeError):
    def __init__(self, status):
        self.status = status
        super().__init__('GitHub request failed with HTTP ' + str(status))


class GitHub:
    def __init__(self, token):
        self.token = token
        self.opener = urllib.request.build_opener(SafeRedirect())

    def open(self, url, method='GET', body=None, media='application/vnd.github+json'):
        require(urllib.parse.urlsplit(url).hostname in ('api.github.com', 'uploads.github.com')
                and url.startswith('https://'), 'Unexpected authenticated GitHub endpoint')
        request = urllib.request.Request(url, data=body, method=method, headers={
            'Authorization': 'Bearer ' + self.token,
            'Accept': 'application/vnd.github+json',
            'X-GitHub-Api-Version': '2022-11-28',
            'User-Agent': 'xscc-verified-android-publisher',
            'Content-Type': media,
        })
        try:
            return self.opener.open(request, timeout=180)
        except urllib.error.HTTPError as error:
            raise GitHubError(error.code) from None
        except (urllib.error.URLError, TimeoutError):
            raise InvalidCandidate('GitHub transport failed; no release was marked complete') from None

    def json(self, path, method='GET', value=None):
        body = None if value is None else json.dumps(value).encode('utf-8')
        with self.open(API + path, method, body, 'application/json') as response:
            return json.load(response)

    def download(self, path):
        with self.open(API + '/actions/artifacts/' + str(ARTIFACT) + '/zip') as response, path.open('wb') as output:
            total = 0
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                require(total <= 40 * 1024 * 1024, 'Artifact download exceeds the candidate size limit')
                output.write(chunk)

    def upload(self, release, name, data):
        address = release['upload_url'].split('{', 1)[0]
        expected = 'https://uploads.github.com/repos/' + REPOSITORY + '/releases/' + str(release['id']) + '/assets'
        require(address == expected, 'Unexpected release upload endpoint')
        media = 'application/vnd.android.package-archive' if name.endswith('.apk') else 'text/plain; charset=utf-8'
        with self.open(address + '?' + urllib.parse.urlencode({'name': name}), 'POST', data, media) as response:
            return json.load(response)


def validate_candidate_metadata(run, artifact, source, run_id):
    require(source == SOURCE and run_id == RUN, 'This publisher is pinned to the reviewed Android hotfix')
    require(run.get('id') == RUN and run.get('head_sha') == SOURCE
            and run.get('path') == '.github/workflows/gpui-android-smoke.yml'
            and run.get('status') == 'completed' and run.get('conclusion') == 'success'
            and run.get('repository', {}).get('full_name') == REPOSITORY,
            'Candidate run source, workflow or successful result does not match')
    require(artifact.get('id') == ARTIFACT and artifact.get('name') == ARTIFACT_NAME
            and artifact.get('workflow_run', {}).get('id') == RUN
            and artifact.get('workflow_run', {}).get('head_sha') == SOURCE
            and artifact.get('digest') == 'sha256:' + ZIP_SHA and not artifact.get('expired'),
            'Candidate artifact source, name, digest or lifetime does not match')


def validate_archive(path, directory):
    require(digest(path.read_bytes()) == ZIP_SHA, 'Downloaded artifact ZIP digest mismatch')
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        require(len(names) == len(set(names)) and len(names) <= 32, 'Duplicate or excessive artifact members')
        require(sum(item.file_size for item in archive.infolist()) <= 150 * 1024 * 1024,
                'Artifact uncompressed size exceeds the limit')
        for item in archive.infolist():
            require(item.filename and '/' not in item.filename and '\\' not in item.filename
                    and ':' not in item.filename and item.filename not in ('.', '..')
                    and not stat.S_ISLNK(item.external_attr >> 16) and not item.flag_bits & 1,
                    'Unsafe artifact member')
        require([name for name in names if name.endswith('.apk')] == [ARTIFACT_APK],
                'Artifact must contain exactly the management APK')
        info = {}
        for line in archive.read('BUILD-INFO.txt').decode('utf-8').splitlines():
            key, value = line.split('=', 1)
            require(key not in info, 'Duplicate BUILD-INFO field')
            info[key] = value
        for key, value in {'commit': SOURCE, 'ui': 'GPUI', 'abi': 'arm64-v8a',
                           'min_sdk': '29', 'signing': 'repository-preview-key',
                           'test_lan_direct': 'false',
                           'local_field_paste': 'explicit-foreground-text-only'}.items():
            require(info.get(key) == value, 'BUILD-INFO mismatch: ' + key)
        sums = {}
        for line in archive.read('SHA256SUMS').decode('utf-8').splitlines():
            match = re.fullmatch(r'([0-9a-f]{64})  ([^/\\]+)', line)
            require(match is not None and match[2] not in sums, 'Invalid artifact SHA256SUMS')
            sums[match[2]] = match[1]
        require(set(sums) == {ARTIFACT_APK, 'Cargo.lock'}, 'Unexpected artifact checksum entries')
        for name, expected in sums.items():
            require(digest(archive.read(name)) == expected, 'Artifact member checksum mismatch: ' + name)
        apk = archive.read(ARTIFACT_APK)
        require(digest(apk) == APK_SHA and len(apk) == APK_SIZE, 'Pinned APK digest or size mismatch')
        require(archive.read('dex-privacy.txt').decode('utf-8').startswith('DEX privacy gate passed;'),
                'Successful DEX privacy evidence is missing')
        output = directory / APK_NAME
        output.write_bytes(apk)
        return output


def validate_apk(apk, tools):
    def output(tool, *args):
        result = subprocess.run([str(tools / tool), *args, str(apk)], capture_output=True, text=True)
        require(result.returncode == 0, tool + ' verification failed')
        return result.stdout
    signing = output('apksigner', 'verify', '--verbose', '--print-certs')
    certificates = re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]{64})$', signing, re.M)
    require([value.lower() for value in certificates] == [CERT_SHA]
            and 'Verified using v2 scheme (APK Signature Scheme v2): true' in signing,
            'APK signature or preview signer mismatch')
    manifest = output('aapt', 'dump', 'badging')
    package = re.search(r"^package: name='([^']+)' versionCode='([^']+)' versionName='([^']+)'", manifest, re.M)
    require(package is not None and package.groups() == ('org.sarmg.xscc.gpui.preview', '3', VERSION),
            'Actual APK package or version mismatch')


def expected_assets():
    sums = (APK_SHA + '  ' + APK_NAME + '\r\n').encode('utf-8')
    return {APK_NAME: (APK_SHA, APK_SIZE), 'SHA256SUMS': (digest(sums), len(sums)),
            'INSTALL.txt': (digest(INSTALL_BYTES), len(INSTALL_BYTES))}


def check_assets(assets, complete):
    expected = expected_assets()
    names = [item['name'] for item in assets]
    require(len(names) == len(set(names)) and set(names) <= set(expected), 'Unexpected release assets')
    if complete:
        require(set(names) == set(expected), 'Published release is incomplete; refusing to modify it')
    for item in assets:
        sha, size = expected[item['name']]
        require(item.get('state') == 'uploaded' and item.get('size') == size
                and item.get('digest') == 'sha256:' + sha,
                'Existing release asset differs: ' + item['name'])


def find_release(github):
    try:
        return github.json('/releases/' + str(RELEASE))
    except GitHubError as error:
        if error.status != 404:
            raise
    for page in range(1, 21):
        items = github.json('/releases?per_page=100&page=' + str(page))
        matches = [item for item in items if item['tag_name'] == TAG]
        require(len(matches) <= 1, 'Multiple matching releases')
        if matches:
            return matches[0]
        if len(items) < 100:
            return None
    raise InvalidCandidate('Release listing exceeds the bounded search')


def check_release(release):
    require(release.get('tag_name') == TAG and release.get('target_commitish') == SOURCE
            and release.get('prerelease') is True, 'Release source or scope differs; refusing to overwrite')


def check_tag(github, required=False):
    try:
        obj = github.json('/git/ref/tags/' + TAG)['object']
    except GitHubError as error:
        if error.status == 404 and not required:
            return
        raise
    for _ in range(5):
        require(re.fullmatch('[0-9a-f]{40}', obj.get('sha', '')) is not None, 'Invalid release tag identity')
        if obj.get('type') != 'tag':
            require(obj.get('type') == 'commit' and obj['sha'] == SOURCE, 'Release tag points to another source')
            return
        obj = github.json('/git/tags/' + obj['sha'])['object']
    raise InvalidCandidate('Release tag nesting exceeds the limit')


def publish(github, source, run_id, tools):
    require(source == SOURCE and run_id == RUN, 'Unexpected source SHA or candidate run')
    release = find_release(github)
    if release:
        check_release(release)
        check_assets(release['assets'], complete=not release['draft'])
        check_tag(github, required=not release['draft'])
        if not release['draft']:
            return release['html_url']
    run = github.json('/actions/runs/' + str(run_id))
    artifact = github.json('/actions/artifacts/' + str(ARTIFACT))
    validate_candidate_metadata(run, artifact, source, run_id)
    with tempfile.TemporaryDirectory(prefix='xscc-android-release-') as temp:
        directory = Path(temp)
        path = directory / 'candidate.zip'
        github.download(path)
        apk = validate_archive(path, directory)
        validate_apk(apk, tools)
        if release is None:
            check_tag(github)
            release = github.json('/releases', 'POST', {
                'tag_name': TAG, 'target_commitish': SOURCE, 'draft': True,
                'prerelease': True, 'make_latest': 'false',
                'name': 'Android ' + VERSION + ': binding password input fix',
                'body': 'Android ARM64 preview hotfix. Source: ' + SOURCE + '\nBuild: https://github.com/' + REPOSITORY + '/actions/runs/' + str(RUN),
            })
            check_release(release)
        payloads = {APK_NAME: apk.read_bytes(),
                    'SHA256SUMS': (APK_SHA + '  ' + APK_NAME + '\r\n').encode('utf-8'),
                    'INSTALL.txt': INSTALL_BYTES}
        for name, data in payloads.items():
            if not any(item['name'] == name for item in release['assets']):
                item = github.upload(release, name, data)
                check_assets([item], complete=False)
        release = github.json('/releases/' + str(release['id']))
        check_release(release)
        check_assets(release['assets'], complete=True)
        check_tag(github, required=not release['draft'])
        if not release['draft']:
            return release['html_url']
        release = github.json('/releases/' + str(release['id']), 'PATCH',
                              {'tag_name': TAG, 'target_commitish': SOURCE,
                               'draft': False, 'prerelease': True, 'make_latest': 'false'})
        check_release(release)
        require(not release['draft'], 'Release remained a draft')
        check_assets(release['assets'], complete=True)
        check_tag(github, required=True)
        return release['html_url']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-sha', default=SOURCE)
    parser.add_argument('--run-id', default=RUN, type=int)
    parser.add_argument('--android-tools', type=Path, required=True)
    args = parser.parse_args()
    require(os.environ.get('GITHUB_REPOSITORY', REPOSITORY) == REPOSITORY, 'Unexpected publishing repository')
    token = os.environ.get('GH_TOKEN')
    require(bool(token), 'GH_TOKEN is required')
    print(publish(GitHub(token), args.source_sha, args.run_id, args.android_tools))


if __name__ == '__main__':
    try:
        main()
    except (InvalidCandidate, GitHubError, OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        raise SystemExit(str(error)) from None
