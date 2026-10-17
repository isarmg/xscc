#!/usr/bin/env python3
"""Prepare an unsigned, relocatable Apple Silicon GPUI application on macOS."""
import argparse
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def bundle_info(role, version):
    if role not in ("controller", "host"):
        raise ValueError("Expected controller or host")
    match = re.fullmatch(r"(\d+\.\d+\.\d+)-beta\.(\d+)", version)
    if not match:
        raise ValueError("Expected a version such as 0.1.0-beta.1")
    name = f"XSCC {role.title()}"
    info = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": f"xscc-{role}",
        "CFBundleIdentifier": f"com.xscc.gpui.{role}",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": name,
        "CFBundleDisplayName": name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": match[1],
        "CFBundleVersion": str(int(match[2]) + 1),
        "LSMinimumSystemVersion": "26.0",
        "LSArchitecturePriority": ["arm64"],
        "NSHighResolutionCapable": True,
        "NSLocalNetworkUsageDescription": f"{name} connects to authorized computers on your local network.",
    }
    if role == "host":
        info["NSScreenCaptureUsageDescription"] = "Share your screen and system audio with an authorized remote controller."
        info["NSMicrophoneUsageDescription"] = "Share audio from your selected input device with an authorized remote controller."
        info["NSAudioCaptureUsageDescription"] = "Share system audio with an authorized remote controller."
    return info


def validate_native_output(architectures, header, load_commands, dependencies):
    if not re.search(r"\bEXECUTE\b", header):
        raise ValueError("Expected a Mach-O executable")
    if architectures.strip() != "arm64":
        raise ValueError("Expected an arm64-only executable")
    # otool also prints dylib versions; only inspect deployment-version commands.
    deployment_blocks = re.findall(r"cmd LC_(?:BUILD_VERSION|VERSION_MIN_MACOSX)\n(.*?)(?=Load command|\Z)", load_commands, re.S)
    if len(deployment_blocks) != 1:
        raise ValueError("Expected exactly one macOS deployment version")
    block = deployment_blocks[0]
    if "platform" in block and not re.search(r"platform\s+(?:1|macos|MACOS)\s", block):
        raise ValueError("Expected a macOS executable")
    minimums = re.findall(r"^\s*(?:minos|version)\s+(\d+)\.(\d+)(?:\.\d+)?\s*$", block, re.M)
    if len(minimums) != 1 or tuple(map(int, minimums[0])) != (26, 0):
        raise ValueError("Build with MACOSX_DEPLOYMENT_TARGET=26.0")
    for line in dependencies.splitlines()[1:]:
        path = line.strip().split(" (", 1)[0]
        if path and not path.startswith(("/System/Library/", "/usr/lib/")):
            raise ValueError(f"Unbundled native dependency: {path}; use static codec dependencies")


def package(binary, output, role, version):
    info = bundle_info(role, version)
    if sys.platform != "darwin":
        raise ValueError("Packaging requires macOS and Xcode command-line tools")
    if not binary.is_file() or binary.is_symlink():
        raise ValueError("Expected a regular native executable")
    if output.exists() or output.is_symlink() or output.suffix != ".app":
        raise ValueError("Output must be a new .app path")
    def run(*args):
        return subprocess.check_output(args, text=True)
    validate_native_output(run("lipo", "-archs", str(binary)),
                           run("otool", "-hv", str(binary)),
                           run("otool", "-l", str(binary)),
                           run("otool", "-L", str(binary)))
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="xscc-bundle-", dir=output.parent) as staging:
        app = Path(staging) / output.name
        contents = app / "Contents"
        executable = contents / "MacOS" / info["CFBundleExecutable"]
        executable.parent.mkdir(parents=True)
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        resources = contents / "Resources"
        resources.mkdir()
        for source in (ROOT / "LICENCE", ROOT / "packaging/gpui-windows/GPUI-LICENSE-APACHE.txt",
                       ROOT / "packaging/gpui-windows/THIRD_PARTY_NOTICES.txt",
                       ROOT / "packaging/gpui-macos/README.txt"):
            shutil.copyfile(source, resources / source.name)
        with (contents / "Info.plist").open("wb") as stream:
            plistlib.dump(info, stream)
        (contents / "PkgInfo").write_bytes(b"APPL????")
        app.rename(output)
    print(f"Prepared unsigned {output}. Native runtime acceptance and distribution signing remain required.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--role", choices=("controller", "host"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        package(args.binary, args.output, args.role, args.version)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Packaging failed: {error}\n")
