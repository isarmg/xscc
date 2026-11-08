#!/usr/bin/env python3
"""Set up the CI-pinned vcpkg source and verified official host tool."""
import argparse
import os
from pathlib import Path
import platform
import re
import subprocess

from pinned_asset_download import download_asset

COMMIT = "9e593bb18ea69cc5095e012465dcd675a822ed0d"
REPOSITORY = "https://github.com/microsoft/vcpkg.git"
RELEASE = "2026-07-27"
WINDOWS_SHA256 = "13b8175e99a884c5ad34249218754b45541a1a63f216e92603aee57a285ac741"
WINDOWS_BYTES = 6745912


def git(root, *arguments):
    return subprocess.run(
        ["git", "-C", str(root), *arguments], check=True,
        stdout=subprocess.PIPE, text=True,
    ).stdout.strip()


def checkout(root):
    root.mkdir(parents=True, exist_ok=True)
    if (root / ".git").exists():
        if Path(git(root, "rev-parse", "--show-toplevel")).resolve() != root.resolve():
            raise RuntimeError("Expected the vcpkg repository root")
        if git(root, "remote", "get-url", "origin") not in (
            REPOSITORY, REPOSITORY.removesuffix(".git"),
        ):
            raise RuntimeError("Expected the official vcpkg origin")
        git(root, "diff", "--quiet")
        git(root, "diff", "--cached", "--quiet")
        current = git(root, "rev-parse", "HEAD")
    else:
        if any(root.iterdir()):
            raise RuntimeError("Refusing to initialize a nonempty vcpkg directory")
        git(root, "init")
        git(root, "remote", "add", "origin", REPOSITORY)
        current = None
    if current is not None and git(root, "rev-parse", "--is-shallow-repository") == "true":
        git(root, "fetch", "--unshallow", "--no-tags", "origin", COMMIT)
    elif current != COMMIT:
        git(root, "fetch", "--no-tags", "origin", COMMIT)
    git(root, "checkout", "--detach", COMMIT)
    if git(root, "rev-parse", "HEAD") != COMMIT:
        raise RuntimeError("vcpkg checkout does not match the source pin")


def tool_spec(metadata, system, machine):
    if metadata.get("VCPKG_TOOL_RELEASE_TAG") != RELEASE:
        raise RuntimeError("The pinned source has an unexpected tool release")
    machine = machine.lower()
    if system == "Windows" and machine in ("amd64", "x86_64"):
        return (
            "https://api.github.com/repos/microsoft/vcpkg-tool/releases/assets/492070117",
            WINDOWS_SHA256, "sha256", WINDOWS_BYTES,
        )
    if system == "Darwin" and machine in ("arm64", "aarch64", "x86_64"):
        name, key = "vcpkg-macos", "VCPKG_MACOS_SHA"
    elif system == "Linux" and machine in ("amd64", "x86_64"):
        name, key = "vcpkg-glibc", "VCPKG_GLIBC_SHA"
    elif system == "Linux" and machine in ("arm64", "aarch64"):
        name, key = "vcpkg-glibc-arm64", "VCPKG_GLIBC_ARM64_SHA"
    else:
        raise RuntimeError("No pinned vcpkg host tool for this CI platform")
    digest = metadata[key]
    if not re.fullmatch(r"[0-9a-f]{128}", digest):
        raise RuntimeError("The pinned source has an invalid tool SHA512")
    return (
        f"https://github.com/microsoft/vcpkg-tool/releases/download/{RELEASE}/{name}",
        digest, "sha512", None,
    )


def check_version(output):
    if not re.search(r"\bversion " + re.escape(RELEASE) + r"(?:[-\s]|$)", output):
        raise RuntimeError("The verified vcpkg tool reported an unexpected version")


def export_environment(root, env_file, path_file, output_file=None):
    value = str(root)
    if "\n" in value or "\r" in value:
        raise RuntimeError("The vcpkg root contains a line break")
    with Path(env_file).open("a", encoding="utf-8") as output:
        output.write(
            f"VCPKG_ROOT={value}\nRUNVCPKG_VCPKG_ROOT={value}\n"
            "VCPKG_DISABLE_METRICS=1\n"
        )
    with Path(path_file).open("a", encoding="utf-8") as output:
        output.write(value + "\n")
    if output_file:
        with Path(output_file).open("a", encoding="utf-8") as output:
            output.write(f"RUNVCPKG_VCPKG_ROOT_OUT={value}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    env_file, path_file = os.environ.get("GITHUB_ENV"), os.environ.get("GITHUB_PATH")
    if not env_file or not path_file:
        raise RuntimeError("This setup command requires GitHub Actions environment files")
    root = args.root.absolute()
    if any(char in str(root) for char in "\r\n"):
        raise RuntimeError("The vcpkg root contains a line break")
    checkout(root)
    metadata = dict(
        line.split("=", 1)
        for line in (root / "scripts/vcpkg-tool-metadata.txt").read_text().splitlines()
        if line and not line.startswith("#")
    )
    system = platform.system()
    url, digest, algorithm, size = tool_spec(metadata, system, platform.machine())
    executable = root / ("vcpkg.exe" if system == "Windows" else "vcpkg")
    download_asset(url, executable, digest, algorithm=algorithm, expected_size=size)
    if system != "Windows":
        executable.chmod(0o755)
    (root / "vcpkg.disable-metrics").touch()
    version = subprocess.run(
        [str(executable), "version", "--disable-metrics"], cwd=root,
        env=dict(os.environ, VCPKG_ROOT=str(root), VCPKG_DISABLE_METRICS="1"),
        check=True, stdout=subprocess.PIPE, text=True,
    ).stdout
    check_version(version)
    export_environment(root, env_file, path_file, os.environ.get("GITHUB_OUTPUT"))
    print(version.strip())
    print(f"Pinned vcpkg source {COMMIT}; host tool {RELEASE}; root {root}")


if __name__ == "__main__":
    main()
