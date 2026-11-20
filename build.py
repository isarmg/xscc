#!/usr/bin/env python3

if __name__ == "__main__":
    raise SystemExit(
        "Legacy Flutter packaging has been retired; XSCC uses GPUI.\n"
        "Controller: cargo run --locked --manifest-path apps/desktop/Cargo.toml --features live-session\n"
        "Host: cargo run --locked --manifest-path apps/desktop/Cargo.toml --features native-host\n"
        "Native packages are built by the gpui-* platform workflows."
    )

import os
import glob
import contextlib
import pathlib
import platform
import zipfile
import urllib.request
import shutil
import hashlib
import re
import subprocess
import argparse
import sys
from pathlib import Path

# Capture the repository root before packaging changes the working directory to flutter/.
REPO_ROOT = os.path.dirname(os.path.abspath(__file__))

windows = platform.platform().startswith('Windows')
osx = platform.platform().startswith(
    'Darwin') or platform.platform().startswith("macOS")
hbb_name = 'rustdesk' + ('.exe' if windows else '')
exe_path = 'target/release/' + hbb_name
if windows:
    flutter_build_dir = 'build/windows/x64/runner/Release/'
elif osx:
    flutter_build_dir = 'build/macos/Build/Products/Release/'
else:
    flutter_build_dir = 'build/linux/x64/release/bundle/'
flutter_build_dir_2 = f'flutter/{flutter_build_dir}'
skip_cargo = False


def get_deb_arch() -> str:
    custom_arch = os.environ.get("DEB_ARCH")
    if custom_arch not in (None, 'amd64'):
        raise ValueError('Linux packages support only x86_64 (DEB_ARCH=amd64)')
    return 'amd64'

def get_deb_extra_depends() -> str:
    return ""

def system2(cmd):
    exit_code = os.system(cmd)
    if exit_code != 0:
        sys.stderr.write(f"Error occurred when executing: `{cmd}`. Exiting.\n")
        sys.exit(-1)


def get_version():
    with open("Cargo.toml", encoding="utf-8") as fh:
        for line in fh:
            if line.startswith("version"):
                return line.replace("version", "").replace("=", "").replace('"', '').strip()
    return ''


def parse_rc_features(feature):
    available_features = {}
    apply_features = {}
    if not feature:
        feature = []

    def platform_check(platforms):
        if windows:
            return 'windows' in platforms
        elif osx:
            return 'osx' in platforms
        else:
            return 'linux' in platforms

    def get_all_features():
        features = []
        for (feat, feat_info) in available_features.items():
            if platform_check(feat_info['platform']):
                features.append(feat)
        return features

    if isinstance(feature, str) and feature.upper() == 'ALL':
        return get_all_features()
    elif isinstance(feature, list):
        if windows:
            # download third party is deprecated, we use github ci instead.
            # feature.append('PrivacyMode')
            pass
        for feat in feature:
            if isinstance(feat, str) and feat.upper() == 'ALL':
                return get_all_features()
            if feat in available_features:
                if platform_check(available_features[feat]['platform']):
                    apply_features[feat] = available_features[feat]
            else:
                print(f'Unrecognized feature {feat}')
        return apply_features
    else:
        raise Exception(f'Unsupported features param {feature}')


def make_parser():
    parser = argparse.ArgumentParser(description='Build script.')
    parser.add_argument(
        '-f',
        '--feature',
        dest='feature',
        metavar='N',
        type=str,
        nargs='+',
        default='',
        help='Integrate features, windows only.'
             'Available: [Not used for now]. Special value is "ALL" and empty "". Default is empty.')
    parser.add_argument('--flutter', action='store_true',
                        help='Build Flutter package (the only supported UI)', default=True)
    parser.add_argument(
        '--hwcodec',
        action='store_true',
        help='Enable feature hwcodec' + (
            '' if windows or osx else ', need libva-dev.')
    )
    parser.add_argument(
        '--vram',
        action='store_true',
        help='Enable feature vram, only available on windows now.'
    )
    parser.add_argument(
        '--portable',
        action='store_true',
        help='Build windows portable'
    )
    parser.add_argument(
        '--drm',
        action='store_true',
        help='Linux only: build the DRM/KMS capture backend (bundles libdrmtap.so, '
             'dlopen-ed in-process by the root service). Off by default.'
    )
    parser.add_argument(
        '--print-features',
        action='store_true',
        help='Print the cargo feature list these flags select, and exit without building. For a '
             'caller that runs its own cargo line and then packages with --skip-cargo: it can ask '
             'for the list rather than repeat it, so the two cannot drift.'
    )
    parser.add_argument(
        '--skip-cargo',
        action='store_true',
        help='Skip cargo build process, only flutter version + Linux supported currently'
    )
    if windows:
        parser.add_argument(
            '--skip-portable-pack',
            action='store_true',
            help='Skip packing, only flutter version + Windows supported'
        )
    parser.add_argument(
        "--package",
        type=str
    )
    if osx:
        parser.add_argument(
            '--screencapturekit',
            action='store_true',
            help='Enable feature screencapturekit'
        )
    return parser


# Downloading third party resources is deprecated.
# We can use this function in an offline build environment.
# Even in an online environment, we recommend building third-party resources yourself.
def download_extract_features(features, res_dir):
    import re

    proxy = ''

    def req(url):
        if not proxy:
            return url
        else:
            r = urllib.request.Request(url)
            r.set_proxy(proxy, 'http')
            r.set_proxy(proxy, 'https')
            return r

    for (feat, feat_info) in features.items():
        includes = feat_info['include'] if 'include' in feat_info and feat_info['include'] else []
        includes = [re.compile(p) for p in includes]
        excludes = feat_info['exclude'] if 'exclude' in feat_info and feat_info['exclude'] else []
        excludes = [re.compile(p) for p in excludes]

        print(f'{feat} download begin')
        download_filename = feat_info['zip_url'].split('/')[-1]
        checksum_md5_response = urllib.request.urlopen(
            req(feat_info['checksum_url']))
        for line in checksum_md5_response.read().decode('utf-8').splitlines():
            if line.split()[1] == download_filename:
                checksum_md5 = line.split()[0]
                filename, _headers = urllib.request.urlretrieve(feat_info['zip_url'],
                                                                download_filename)
                md5 = hashlib.md5(open(filename, 'rb').read()).hexdigest()
                if checksum_md5 != md5:
                    raise Exception(f'{feat} download failed')
                print(f'{feat} download end. extract bein')
                zip_file = zipfile.ZipFile(filename)
                zip_list = zip_file.namelist()
                for f in zip_list:
                    file_exclude = False
                    for p in excludes:
                        if p.match(f) is not None:
                            file_exclude = True
                            break
                    if file_exclude:
                        continue

                    file_include = False if includes else True
                    for p in includes:
                        if p.match(f) is not None:
                            file_include = True
                            break
                    if file_include:
                        print(f'extract file {f}')
                        zip_file.extract(f, res_dir)
                zip_file.close()
                os.remove(download_filename)
                print(f'{feat} extract end')


def external_resources(flutter, args, res_dir):
    features = parse_rc_features(args.feature)
    if not features:
        return

    print(f'Build with features {list(features.keys())}')
    if os.path.isdir(res_dir) and not os.path.islink(res_dir):
        shutil.rmtree(res_dir)
    elif os.path.exists(res_dir):
        raise Exception(f'Find file {res_dir}, not a directory')
    os.makedirs(res_dir, exist_ok=True)
    download_extract_features(features, res_dir)
    if flutter:
        os.makedirs(flutter_build_dir_2, exist_ok=True)
        for f in pathlib.Path(res_dir).iterdir():
            print(f'{f}')
            if f.is_file():
                shutil.copy2(f, flutter_build_dir_2)
            else:
                shutil.copytree(f, f'{flutter_build_dir_2}{f.stem}')


def linux_packaging_branch():
    """Which packaging path `main()` will take on THIS host.

    MUST mirror the elif chain in main() (pacman / yum / zypper / else), and exists so `--drm` can
    refuse a branch that is not drm-aware instead of silently producing a stock-named package with
    the capture backend compiled in. Only the final `deb` branch reaches `build_flutter_deb`, which
    is what bundles libdrmtap, renames the package, adds Conflicts/Provides and asserts the staged
    binary really is a drm build.
    """
    if os.path.isfile('/usr/bin/pacman'):
        return 'pacman'
    if os.path.isfile('/usr/bin/yum'):
        return 'yum'
    if os.path.isfile('/usr/bin/zypper'):
        return 'zypper'
    return 'deb'


def get_features(args):
    features = []
    if args.hwcodec:
        features.append('hwcodec')
    if args.vram:
        features.append('vram')
    features.append('flutter')
    if args.drm:
        # Say so rather than quietly handing back a stock build: the backend is Linux-only, so on
        # any other host the flag cannot be honoured and the resulting binary would look like a
        # DRM build without being one.
        if windows or osx:
            raise Exception('--drm is Linux only')
        # And only on the deb branch. The other three Linux paths (pacman/yum/zypper) package
        # straight from `target/release` without bundling libdrmtap, without the rename, without
        # Conflicts/Provides and without assert_staged_binary_is_drm() -- so they would emit a
        # package NAMED `rustdesk` carrying the consent-bypass backend and the root-side uinput
        # injection. The separate package name is the informed consent this feature rests on, so
        # refuse rather than ship a stock-named build of it.
        branch = linux_packaging_branch()
        if branch != 'deb':
            raise Exception(
                f'--drm is only supported on the deb packaging path; this host would package via '
                f'{branch}, which cannot bundle libdrmtap or name the package distinctly')
        features.append('drm')
        # The display wake is its own compile gate on top of `drm`, and the unattended package is
        # exactly where it belongs: that variant exists to reach a machine nobody is sitting at,
        # and a machine whose screen went dark is the case it is for. Dropping `drm-wake` from
        # this line builds the same capture backend with no wake code in the binary at all.
        # It is ALSO switchable at runtime; see OPTION_ENABLE_DRM_DISPLAY_WAKE.
        features.append('drm-wake')
    if osx:
        if args.screencapturekit:
            features.append('screencapturekit')
    print("features:", features)
    return features


def generate_control_file(version):
    control_file_path = "../res/DEBIAN/control"
    system2('/bin/rm -rf %s' % control_file_path)

    floor = measured_glibc_floor()
    print(f'[package] libc6 floor measured at {floor}')
    content = """Package: rustdesk
Section: net
Priority: optional
Version: %s
Architecture: %s
Maintainer: rustdesk <info@rustdesk.com>
Homepage: https://rustdesk.com
Depends: libgtk-3-0t64 | libgtk-3-0, libasound2t64 | libasound2, libsystemd0, curl, libva2, libva-drm2, libva-x11-2, libgstreamer-plugins-base1.0-0, gstreamer1.0-pipewire%s, libc6 (>= %s)
Recommends: libayatana-appindicator3-1
Description: A remote control software.

""" % (version, get_deb_arch(), get_deb_extra_depends(), floor)
    file = open(control_file_path, "w")
    file.write(content)
    file.close()


def ffi_bindgen_function_refactor():
    # workaround ffigen
    system2(
        'sed -i "s/ffi.NativeFunction<ffi.Bool Function(DartPort/ffi.NativeFunction<ffi.Uint8 Function(DartPort/g" flutter/lib/generated_bridge.dart')


DRM_CAPTURE_SOURCE = os.path.join(REPO_ROOT, 'vendor', 'drm-screen-capture')
DRM_CAPTURE_BUILD = os.path.join(REPO_ROOT, 'build', 'native-dependencies', 'drm-screen-capture')


def _single_real_so(paths, where):
    # Return the one real libdrmtap.so.0.* object among `paths`, failing if there are zero or several.
    # glob order is arbitrary, so silently taking [0] could ship a stale or wrong-arch object left
    # over from an earlier build; a mismatch should fail the build loudly instead.
    real = sorted(p for p in paths if os.path.isfile(p) and not os.path.islink(p))
    if len(real) != 1:
        raise Exception(
            f'expected exactly one real libdrmtap.so.0.* in {where}, found {len(real)}: {real}')
    return real[0]


def build_libdrmtap_so():
    prebuilt_dir = os.environ.get('DRMTAP_PREBUILT_DIR')
    if prebuilt_dir:
        if (os.path.realpath(prebuilt_dir) != os.path.realpath(DRM_CAPTURE_BUILD)
                and os.environ.get('DRMTAP_ALLOW_UNPINNED') != '1'):
            raise ValueError('Set DRMTAP_ALLOW_UNPINNED=1 to use a DRM capture library from another build.')
        build_dir = prebuilt_dir
    else:
        build_dir = DRM_CAPTURE_BUILD
        setup = ['meson', 'setup']
        if os.path.isfile(os.path.join(build_dir, 'build.ninja')):
            setup.append('--reconfigure')
        subprocess.run(
            setup + [build_dir, DRM_CAPTURE_SOURCE, '--buildtype=release',
                     '--wrap-mode=nodownload', '-Degl=enabled'],
            check=True,
        )
        subprocess.run(['meson', 'compile', '-C', build_dir, 'drmtap'], check=True)

    so = _single_real_so(glob.glob(os.path.join(build_dir, 'libdrmtap.so.0.*')), build_dir)
    _assert_so_has_egl(so)
    _assert_so_has_no_helper(so)
    return so


def _assert_so_has_no_helper(so_path):
    # Asserted on the ARTIFACT for the same reason the EGL check is: a build flag cannot notice a
    # stale object left by an earlier build, or one substituted by hand, and `-Dhelper=disabled` is
    # silently accepted by meson versions that predate the option. What the option removes is the
    # whole fork/exec path, so the produced .so imports no socketpair and carries none of the
    # helper search paths; both go to zero, measured on libdrmtap 0.5.6 (4 -> 0 and 3 -> 0).
    #
    # This matters because the object is loaded by the ROOT service. With the path compiled in, a
    # caller that ever lost CAP_SYS_ADMIN would exec whatever sits at the first of six hardcoded
    # locations that passes access(X_OK), without checking its owner or its mode.
    with open(so_path, 'rb') as f:
        blob = f.read()
    found = [m.decode() for m in (b'socketpair', b'/usr/libexec/drmtap-helper') if m in blob]
    if found:
        raise Exception(
            f'{so_path} still carries the privileged-helper fallback ({", ".join(found)}). '
            f'Use the repository DRM screen capture sources: rustdesk captures as root and never uses '
            f'the helper, so this is a fork/exec path in the privileged process that nothing '
            f'reaches. Delete the DRM capture build directory and rebuild.')


def _assert_so_has_egl(so_path):
    # libdrmtap treats egl/glesv2 as OPTIONAL dependencies: without their headers and pkg-config
    # files, meson silently builds a CPU-only stub. That stub still exports every symbol the loader
    # checks for, so nothing downstream notices -- and the split architecture depends entirely on the
    # unprivileged side EGL-detiling the scanout it receives. The result is a build where DRM capture
    # quietly degrades to PipeWire on every tiled-scanout host, which is most of them.
    #
    # Assert on the ARTIFACT rather than passing an option that demands it: `-Degl=enabled` exists
    # only in libdrmtap past 0.4.15, and checking what was actually produced also catches a stale or
    # hand-substituted object, which a build flag cannot.
    #
    # EGL is reached by lazy dlopen, on purpose, so that the privileged service never links the GPU
    # stack. That means there is no DT_NEEDED to look for and an ELF-level check reports "no EGL" on a
    # perfectly good library; the dlopen name and an extension symbol are what a CPU-only stub really
    # lacks.
    try:
        with open(so_path, 'rb') as f:
            blob = f.read()
    except OSError as err:
        raise Exception(f'cannot read the built libdrmtap at {so_path}: {err}') from err
    missing = [m for m in (b'libEGL.so.1', b'eglCreateImageKHR') if m not in blob]
    if missing:
        raise Exception(
            f'{so_path} looks like a CPU-only libdrmtap stub (missing '
            f'{", ".join(m.decode() for m in missing)}): the EGL detile path the split capture '
            'depends on is not in it, and DRM capture would silently fall back to PipeWire. '
            'Install the EGL development packages and rebuild (Debian/Ubuntu: libegl-dev '
            'libgles2-mesa-dev; Arch: mesa libglvnd).')


DRM_PACKAGE_NAME = 'rustdesk-unattended-wayland'


def assert_so_satisfies_the_runtime_abi_gate(so_path):
    """The .so we are about to ship must be one the RUNTIME will actually accept.

    `abi_accepted` in libs/scrap/src/common/drmtap_dl.rs is the only place the pinned library's
    version is ever validated, and it runs at dlopen time on the USER's machine. Nothing in the
    build or in CI compared the two, so the pin and the gate could drift apart and every existing
    assertion would still pass: the EGL check does not look at the version, the CI symbol contract
    does not call drmtap_version(), and the deb-contents regex matches any `libdrmtap.so.0.X.Y`.
    A green pipeline could therefore produce a deb in which DRM capture can never start, and the
    only symptom on the host is one log line before it falls back to the portal.

    So parse the gate out of the Rust and apply it here, to the object being staged. This is the
    same rule, not a copy of the numbers: if someone bumps the constants, this reads the new ones.
    """
    m = re.search(r'libdrmtap\.so\.(\d+)\.(\d+)\.(\d+)', os.path.basename(so_path))
    if not m:
        # Not a versioned soname (a local dev build, say). The gate cannot be evaluated, and
        # inventing a verdict would be worse than saying so.
        print(f'[drm] cannot read a version out of {so_path}; skipping the ABI-gate cross-check')
        return
    so_ver = tuple(int(g) for g in m.groups())
    # REPO_ROOT, not abspath(__file__): both callers have chdir'd into flutter/ by now.
    gate_path = os.path.join(REPO_ROOT, 'libs', 'scrap', 'src', 'common', 'drmtap_dl.rs')
    with open(gate_path) as f:
        gate_src = f.read()

    def _const(name):
        mm = re.search(rf'const {name}: c_int = (\d+);', gate_src)
        return int(mm.group(1)) if mm else None

    major, minor = _const('DRMTAP_ABI_MAJOR'), _const('DRMTAP_ABI_MINOR')
    mm = re.search(r'const DRMTAP_MIN_MINOR_PATCH: \(c_int, c_int\) = \((\d+), (\d+)\);', gate_src)
    floor = (int(mm.group(1)), int(mm.group(2))) if mm else None
    if major is None or minor is None or floor is None:
        raise Exception(
            'could not parse the libdrmtap ABI gate out of drmtap_dl.rs (DRMTAP_ABI_MAJOR / '
            'DRMTAP_ABI_MINOR / DRMTAP_MIN_MINOR_PATCH). The gate moved and this check did not; '
            'fix the check rather than removing it, or the pin and the gate can drift silently.')
    accepted = so_ver[0] == major and so_ver[1] == minor and (so_ver[1], so_ver[2]) >= floor
    if not accepted:
        raise Exception(
            f'the libdrmtap being packaged is {so_ver[0]}.{so_ver[1]}.{so_ver[2]}, which the '
            f'runtime loader would REFUSE: drmtap_dl.rs accepts exactly major {major}, minor '
            f'{minor}, patch >= {floor[1]}. Shipping it produces a deb whose DRM capture can never '
            'start. Move the build pin and the gate together, or fix whichever one is wrong.')
    print(f'[drm] libdrmtap {so_ver[0]}.{so_ver[1]}.{so_ver[2]} satisfies the runtime ABI gate '
          f'(major {major}, minor {minor}, patch >= {floor[1]})')


def stage_libdrmtap_into_deb(so_path):
    # Put the built libdrmtap object plus its soname symlink into the staged deb. Only the soname
    # symlink is needed: libdrmtap is resolved by ABSOLUTE path (/usr/lib/rustdesk/libdrmtap.so.0) at
    # the in-process dlopen site (drmtap_dl.rs), so the deb does NOT drop /usr/lib/rustdesk into the
    # system-wide /etc/ld.so.conf.d search path, which would let this private library shadow a system
    # library for every binary on the host (Debian Policy 10.2 forbids that). No ld.so.conf.d drop-in
    # and no ldconfig trigger are shipped, so the stock postinst is used unchanged.
    assert_so_satisfies_the_runtime_abi_gate(so_path)
    so_basename = os.path.basename(so_path)
    system2('mkdir -p tmpdeb/usr/lib/rustdesk')
    # Quoted: so_path comes from the repo root or from DRMTAP_PREBUILT_DIR, either of which can
    # contain a space, and an unquoted interpolation would split the argument and fail obscurely.
    system2(f'cp "{so_path}" tmpdeb/usr/lib/rustdesk/')
    system2(f'ln -sf "{so_basename}" tmpdeb/usr/lib/rustdesk/libdrmtap.so.0')


def _max_glibc_minor(path):
    # Inspect required symbol versions, not arbitrary strings or exported libc versions.
    result = subprocess.run(
        ['readelf', '--version-info', str(path)], check=True, capture_output=True,
        text=True, env={**os.environ, 'LC_ALL': 'C'})
    needs = result.stdout.partition('Version needs section')[2]
    return max((int(m) for m in re.findall(r'Name: GLIBC_2\.(\d+)', needs)), default=0)


def measured_glibc_floor(staging="tmpdeb/usr"):
    # Include the Flutter engine and plugins as well as the executable and optional libdrmtap.
    paths = []
    for path in pathlib.Path(staging).rglob('*'):
        if path.is_file() and not path.is_symlink():
            with path.open('rb') as f:
                if f.read(4) == b'\x7fELF':
                    paths.append(str(path))
    minor = max((_max_glibc_minor(p) for p in paths), default=0)
    if not minor:
        raise Exception(
            f'could not measure a GLIBC_2.x floor from any staged object ({paths or "none found"}); '
            'refusing to ship a package with an undeclared libc6 floor')
    return f'2.{minor}'


def retarget_control_to_drm_variant():
    # Rewrite the control file that generate_control_file just produced, instead of parameterizing that
    # function: variant-specific metadata lives here. The variant installs the same files
    # as the stock package, so it must
    # conflict with and replace it: you install one or the other, never both. It also needs libdrmtap's
    # own runtime deps, which the stock package has no reason to carry.
    path = '../res/DEBIAN/control'
    with open(path) as f:
        lines = f.readlines()
    out = []
    for line in lines:
        if line.startswith('Package: rustdesk'):
            out.append(f'Package: {DRM_PACKAGE_NAME}\n')
            out.append('Conflicts: rustdesk\nReplaces: rustdesk\nProvides: rustdesk\n')
        elif line.startswith('Depends:'):
            # 2.4.101 is where drmModeGetFB2 landed; below it libdrmtap loads and can never capture.
            out.append(line.rstrip('\n') + ', libdrm2 (>= 2.4.101), libegl1, libgles2\n')
        else:
            out.append(line)
    body = ''.join(out)
    # Fail loudly rather than silently shipping a package that says `rustdesk`: a stock control file
    # that stopped matching either anchor would otherwise produce a variant deb wearing the stock name.
    if f'Package: {DRM_PACKAGE_NAME}\n' not in body or 'libegl1' not in body:
        raise Exception(f'could not retarget {path} to the drm variant; upstream control layout changed')
    with open(path, 'w') as f:
        f.write(body)


def build_flutter_deb(version, features):
    if not skip_cargo:
        system2(f'cargo build --locked --features {features} --lib --release')
        ffi_bindgen_function_refactor()
    os.chdir('flutter')
    system2('flutter build linux --release')
    system2('mkdir -p tmpdeb/usr/bin/')
    system2('mkdir -p tmpdeb/usr/share/rustdesk')
    system2('mkdir -p tmpdeb/usr/share/rustdesk/files/systemd/')
    system2('mkdir -p tmpdeb/usr/share/icons/hicolor/256x256/apps/')
    system2('mkdir -p tmpdeb/usr/share/icons/hicolor/scalable/apps/')
    system2('mkdir -p tmpdeb/usr/share/applications/')
    system2('mkdir -p tmpdeb/usr/share/polkit-1/actions')
    system2('rm tmpdeb/usr/bin/rustdesk || true')
    system2(
        f'cp -r {flutter_build_dir}/* tmpdeb/usr/share/rustdesk/')
    system2(
        'cp ../res/rustdesk.service tmpdeb/usr/share/rustdesk/files/systemd/')
    system2(
        'cp ../res/128x128@2x.png tmpdeb/usr/share/icons/hicolor/256x256/apps/rustdesk.png')
    system2(
        'cp ../res/scalable.svg tmpdeb/usr/share/icons/hicolor/scalable/apps/rustdesk.svg')
    system2(
        'cp ../res/rustdesk.desktop tmpdeb/usr/share/applications/rustdesk.desktop')
    system2(
        'cp ../res/rustdesk-link.desktop tmpdeb/usr/share/applications/rustdesk-link.desktop')
    system2(
        "echo \"#!/bin/sh\" >> tmpdeb/usr/share/rustdesk/files/polkit && chmod a+x tmpdeb/usr/share/rustdesk/files/polkit")
    # Bundle libdrmtap.so only when this build actually enabled the `drm` feature, so stock packages
    # stay exactly what they were. The root service dlopens it in-process by absolute path.
    # `features` is the comma-joined string, so split it: a bare substring test would also match any
    # future feature merely containing "drm" (drm-lease, vaapi-drm) and rename the deb to the
    # consent-bypass variant without --drm ever being passed.
    ships_so = 'drm' in features.split(',')
    if ships_so:
        # Same artifact assertion as the --package path. Under --skip-cargo nothing here rebuilt the
        # binary, so `features` says what was ASKED for while the staged bundle can be anything.
        assert_staged_binary_is_drm()
        stage_libdrmtap_into_deb(build_libdrmtap_so())

    system2('mkdir -p tmpdeb/DEBIAN')
    generate_control_file(version)
    if ships_so:
        retarget_control_to_drm_variant()
    system2('cp -a ../res/DEBIAN/* tmpdeb/DEBIAN/')
    md5_file_folder("tmpdeb/")
    system2('dpkg-deb -b tmpdeb rustdesk.deb;')

    system2('/bin/rm -rf tmpdeb/')
    system2('/bin/rm -rf ../res/DEBIAN/control')
    os.rename('rustdesk.deb', '../rustdesk-%s.deb' % version)
    if ships_so:
        # Named apart from the stock package so installing the consent-free variant is a deliberate act.
        os.rename('../rustdesk-%s.deb' % version, f'../{DRM_PACKAGE_NAME}-{version}.deb')
    os.chdir("..")


DRMTAP_DLOPEN_MARKER = b'/usr/lib/rustdesk/libdrmtap.so.0'
# Present only when `drm-wake` is compiled in: the runtime option constant is itself
# #[cfg(feature = "drm-wake")] (src/ipc/drm.rs). The dlopen marker above cannot stand in for it -
# `--features drm` alone produces a binary that carries the dlopen path and NO wake code, and that
# is exactly the deb this assertion is here to refuse.
DRMTAP_WAKE_MARKER = b'enable-drm-display-wake'


def _carries_drmtap_marker(path, marker=DRMTAP_DLOPEN_MARKER):
    # Chunked, with an overlap of len(marker)-1 so the marker cannot be missed at a chunk boundary:
    # librustdesk.so is ~45 MB and there is no reason to hold it all in memory, and the `with`
    # closes deterministically instead of relying on refcounting.
    with open(path, 'rb') as f:
        tail = b''
        while True:
            chunk = f.read(1 << 20)
            if not chunk:
                return False
            if marker in tail + chunk:
                return True
            tail = chunk[-(len(marker) - 1):]


def assert_staged_binary_is_drm():
    """The staged BINARY must really be a drm build before it is named the unattended-wayland
    variant. That package conflicts with and replaces the stock one, so shipping a stock binary
    under that name produces something that can never capture and cannot be installed alongside
    what it replaced. The marker is the absolute dlopen path from drmtap_dl.rs, present only when
    the feature is compiled in -- assert what was produced, not what was asked for.

    Called from BOTH packaging paths. It used to guard only one of them, and `--skip-cargo` (which
    is how CI packages) reaches the other, where nothing had rebuilt the binary at all.
    """
    binaries = [p for p in glob.glob('tmpdeb/usr/share/rustdesk/lib/librustdesk.so')
                + glob.glob('tmpdeb/usr/share/rustdesk/rustdesk') if os.path.isfile(p)]
    if not any(_carries_drmtap_marker(p) for p in binaries):
        raise Exception(
            f'--drm was requested but the staged bundle does not look like a drm build (no '
            f'{DRMTAP_DLOPEN_MARKER.decode()} dlopen path in {binaries or "any staged binary"}); '
            'refusing to package it as the unattended-wayland variant, which conflicts with and '
            'replaces the stock package but could never capture')
    # And the WAKE half. `--drm` enables `drm-wake` too (see get_features), and the deb is named and
    # documented as the variant that can reach a machine whose screen has gone dark. The dlopen
    # marker above does not distinguish them: `--features drm` alone carries it and has no wake code
    # at all. Asserting only the first half is how a deb can be named for a feature it does not have.
    if not any(_carries_drmtap_marker(p, DRMTAP_WAKE_MARKER) for p in binaries):
        raise Exception(
            f'--drm was requested but the staged binary has no {DRMTAP_WAKE_MARKER.decode()} '
            f'marker in {binaries or "any staged binary"}, so it was built without `drm-wake`; '
            'refusing to package it as the unattended-wayland variant, which is named and '
            'documented as the build that can wake an idle-disabled display. If this fired under '
            '--skip-cargo, the cargo line that produced the bundle is missing the feature: '
            '--features ...,drm,drm-wake')


def build_deb_from_folder(version, binary_folder, want_drm=False):
    os.chdir('flutter')
    system2('mkdir -p tmpdeb/usr/bin/')
    system2('mkdir -p tmpdeb/usr/share/rustdesk')
    system2('mkdir -p tmpdeb/usr/share/rustdesk/files/systemd/')
    system2('mkdir -p tmpdeb/usr/share/icons/hicolor/256x256/apps/')
    system2('mkdir -p tmpdeb/usr/share/icons/hicolor/scalable/apps/')
    system2('mkdir -p tmpdeb/usr/share/applications/')
    system2('mkdir -p tmpdeb/usr/share/polkit-1/actions')
    system2('rm tmpdeb/usr/bin/rustdesk || true')
    system2(
        f'cp -r ../{binary_folder}/* tmpdeb/usr/share/rustdesk/')
    system2(
        'cp ../res/rustdesk.service tmpdeb/usr/share/rustdesk/files/systemd/')
    system2(
        'cp ../res/128x128@2x.png tmpdeb/usr/share/icons/hicolor/256x256/apps/rustdesk.png')
    system2(
        'cp ../res/scalable.svg tmpdeb/usr/share/icons/hicolor/scalable/apps/rustdesk.svg')
    system2(
        'cp ../res/rustdesk.desktop tmpdeb/usr/share/applications/rustdesk.desktop')
    system2(
        'cp ../res/rustdesk-link.desktop tmpdeb/usr/share/applications/rustdesk-link.desktop')
    system2(
        "echo \"#!/bin/sh\" >> tmpdeb/usr/share/rustdesk/files/polkit && chmod a+x tmpdeb/usr/share/rustdesk/files/polkit")
    # Where the capture library comes from for a `--package <folder> --drm` build. Two shapes are
    # supported, because two exist in practice: a bundle that already carries libdrmtap.so.0.*
    # (someone staged it, e.g. a CI artifact), and a plain bundle, which is what every build path
    # here actually produces -- the flutter deb builds the library straight into the staged deb, so
    # nothing ever puts it inside the bundle folder. Demanding it in the bundle made this flag
    # combination impossible to satisfy.
    bundled_glob = glob.glob('tmpdeb/usr/share/rustdesk/libdrmtap.so.0.*')
    bundle_carries_so = any(os.path.isfile(p) and not os.path.islink(p) for p in bundled_glob)
    # The variant must be decided by the EXPLICIT --drm request, not merely by what happens to be
    # staged: a bundle that carries the .so must NOT be shipped as the consent-bypass variant when
    # --drm was never passed.
    if bundle_carries_so and not want_drm:
        raise Exception(
            'the staged bundle carries libdrmtap.so.0.* but --drm was not passed; refusing '
            'to silently ship the consent-bypass unattended-wayland variant (pass --drm to '
            'build it deliberately)')
    if want_drm:
        # Whichever shape we are in, the staged BINARY must really be a drm build. This is the
        # property the old presence-of-the-.so test stood in for, badly: a stock binary packaged as
        # the unattended-wayland variant would carry the consent-bypass name, conflict with and
        # replace the stock package, and never be able to capture. The marker is the absolute
        # dlopen path from drmtap_dl.rs, present only when the feature is compiled in -- the same
        # kind of artifact assertion as _assert_so_has_egl, and for the same reason: assert what
        # was produced, not what was asked for.
        assert_staged_binary_is_drm()
        if bundle_carries_so:
            so = _single_real_so(bundled_glob, 'the staged --drm bundle')
            # The THIRD artifact source, and the last one that was missing the check: --package
            # takes the .so straight out of a bundle somebody else produced, so it has the same
            # exposure as DRMTAP_PREBUILT_DIR (see the comment on that branch). A CPU-only stub
            # would ship, the loader would accept it, and capture would degrade to PipeWire
            # without a word. A bundle produced before the helper was disabled is caught here too,
            # which is the point: it must not be repackaged into a deb.
            _assert_so_has_egl(so)
            _assert_so_has_no_helper(so)
            stage_libdrmtap_into_deb(so)
            system2(f'rm -f "{so}"')
            system2('rm -f tmpdeb/usr/share/rustdesk/libdrmtap.so tmpdeb/usr/share/rustdesk/libdrmtap.so.0')
        else:
            # Build it here, exactly as the flutter deb path does (build_libdrmtap_so asserts the
            # EGL backend itself). The library is independent of the staged binary.
            stage_libdrmtap_into_deb(build_libdrmtap_so())

    system2('mkdir -p tmpdeb/DEBIAN')
    generate_control_file(version)
    # Keyed on the EXPLICIT request, not on what happened to be staged: by here a --drm build has
    # its library in tmpdeb whichever of the two shapes it came from.
    if want_drm:
        retarget_control_to_drm_variant()
    system2('cp -a ../res/DEBIAN/* tmpdeb/DEBIAN/')
    md5_file_folder("tmpdeb/")
    system2('dpkg-deb -b tmpdeb rustdesk.deb;')

    system2('/bin/rm -rf tmpdeb/')
    system2('/bin/rm -rf ../res/DEBIAN/control')
    os.rename('rustdesk.deb', '../rustdesk-%s.deb' % version)
    if want_drm:
        os.rename('../rustdesk-%s.deb' % version, f'../{DRM_PACKAGE_NAME}-{version}.deb')
    os.chdir("..")


def build_flutter_dmg(version, features):
    if not skip_cargo:
        # Keep the Rust library aligned with the macOS 26 application minimum.
        system2(
            f'MACOSX_DEPLOYMENT_TARGET=26.0 cargo build --locked --features {features} --release')
    # copy dylib
    system2(
        "cp target/release/liblibrustdesk.dylib target/release/librustdesk.dylib")
    os.chdir('flutter')
    # cargo builds a single-arch dylib for the host; restrict Xcode to the same arch
    # so the universal-by-default ARCHS_STANDARD doesn't try to link a missing slice.
    # FLUTTER_XCODE_* env vars are forwarded to xcodebuild as build settings.
    system2(
        'MACOSX_DEPLOYMENT_TARGET=26.0 FLUTTER_XCODE_ARCHS=arm64 FLUTTER_XCODE_ONLY_ACTIVE_ARCH=YES flutter build macos --release')
    system2('cp -rf ../target/release/service ./build/macos/Build/Products/Release/RustDesk.app/Contents/MacOS/')
    '''
    system2(
        "create-dmg --volname \"RustDesk Installer\" --window-pos 200 120 --window-size 800 400 --icon-size 100 --app-drop-link 600 185 --icon RustDesk.app 200 190 --hide-extension RustDesk.app rustdesk.dmg ./build/macos/Build/Products/Release/RustDesk.app")
    os.rename("rustdesk.dmg", f"../rustdesk-{version}.dmg")
    '''
    os.chdir("..")


def build_flutter_arch_manjaro(version, features):
    if not skip_cargo:
        system2(f'cargo build --locked --features {features} --lib --release')
    ffi_bindgen_function_refactor()
    os.chdir('flutter')
    system2('flutter build linux --release')
    system2(f'strip {flutter_build_dir}/lib/librustdesk.so')
    os.chdir('../res')
    system2('HBB=`pwd`/.. FLUTTER=1 makepkg -f')


def build_flutter_windows(version, features, skip_portable_pack):
    if not skip_cargo:
        system2(f'cargo build --locked --features {features} --lib --release')
        if not os.path.exists("target/release/librustdesk.dll"):
            print("cargo build failed, please check rust source code.")
            exit(-1)
    os.chdir('flutter')
    system2('flutter build windows --release')
    os.chdir('..')
    if skip_portable_pack:
        return
    os.chdir('libs/portable')
    system2('pip3 install -r requirements.txt')
    system2(
        f'python3 ./generate.py -f ../../{flutter_build_dir_2} -o . -e ../../{flutter_build_dir_2}/rustdesk.exe')
    os.chdir('../..')
    if os.path.exists('./rustdesk_portable.exe'):
        os.replace('./target/release/rustdesk-portable-packer.exe',
                   './rustdesk_portable.exe')
    else:
        os.rename('./target/release/rustdesk-portable-packer.exe',
                  './rustdesk_portable.exe')
    print(
        f'output location: {os.path.abspath(os.curdir)}/rustdesk_portable.exe')
    os.rename('./rustdesk_portable.exe', f'./rustdesk-{version}-install.exe')
    print(
        f'output location: {os.path.abspath(os.curdir)}/rustdesk-{version}-install.exe')


def main():
    host_arch = platform.machine().lower()
    supported_arch = ('arm64', 'aarch64') if osx else ('x86_64', 'amd64')
    if host_arch not in supported_arch:
        raise SystemExit('Desktop builds support only Windows/Linux x86_64 and macOS Apple Silicon')
    global skip_cargo
    parser = make_parser()
    args = parser.parse_args()

    # Before anything with a side effect: this is a query, and a caller uses it to build the very
    # binary it will then package. `get_features` stays the single definition of what a flag
    # combination means; a caller that hardcodes the list instead is one edit away from compiling
    # something other than what it ships.
    if args.print_features:
        # stdout carries the list and nothing else, so a caller can use it directly in a command
        # substitution. `get_features` prints a human-readable line of its own; send that to stderr
        # for this call rather than silencing it, which would change what every other path prints.
        with contextlib.redirect_stdout(sys.stderr):
            feats = ','.join(get_features(args))
        print(feats)
        return

    if os.path.exists(exe_path):
        os.unlink(exe_path)
    version = get_version()
    features = ','.join(get_features(args))
    print(args.skip_cargo)
    if args.skip_cargo:
        skip_cargo = True
    package = args.package
    if package:
        build_deb_from_folder(version, package, args.drm)
        return
    res_dir = 'resources'
    external_resources(True, args, res_dir)
    if windows:
        build_flutter_windows(version, features, args.skip_portable_pack)
    elif os.path.isfile('/usr/bin/pacman'):
        system2("sed -i 's/pkgver=.*/pkgver=%s/g' res/PKGBUILD" % version)
        build_flutter_arch_manjaro(version, features)
        system2('mv rustdesk-%s-0-x86_64.pkg.tar.zst rustdesk-%s-manjaro-arch.pkg.tar.zst' % (
            version, version))
    elif os.path.isfile('/usr/bin/yum') or os.path.isfile('/usr/bin/zypper'):
        raise SystemExit('Use the Flutter RPM packaging workflow for this distribution.')
    elif osx:
        build_flutter_dmg(version, features)
    else:
        build_flutter_deb(version, features)


def md5_file(fn):
    md5 = hashlib.md5(open('tmpdeb/' + fn, 'rb').read()).hexdigest()
    system2('echo "%s  /%s" >> tmpdeb/DEBIAN/md5sums' % (md5, fn))

def md5_file_folder(base_dir):
    base_path = Path(base_dir)
    for file in base_path.rglob('*'):
        if file.is_file() and 'DEBIAN' not in file.parts:
            relative_path = file.relative_to(base_path)
            md5_file(str(relative_path))


if __name__ == "__main__":
    main()
