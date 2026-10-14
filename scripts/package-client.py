#!/usr/bin/env python3
"""Build a source-bound Client archive on its native OS; never package the Server."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
COMMON_FILES = {"LICENSE-APACHE": "LICENSE", "README.md": "README.md", "docs/platform-setup.md": "platform-setup.md", "config/bootstrap.example.json": "bootstrap.example.json"}


def run(*args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, **kwargs).strip()


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def package_files(directory):
    # WindowsPath sorts case-insensitively; the wire format uses exact ASCII names.
    return sorted((path for path in directory.rglob('*') if path.is_file()),
                  key=lambda path: path.relative_to(directory).as_posix())


def inventory_module():
    spec = importlib.util.spec_from_file_location('client_release_inventory', ROOT / 'scripts/release-inventory.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_protocol_source(manifest, lock):
    """Candidate path inputs must never become a standalone release package."""
    dependency = manifest.get("dependencies", {}).get("xscs-protocol", {})
    source = "https://github.com/isarmg/xscs.git"
    revision = dependency.get("rev") if isinstance(dependency, dict) else None
    if not (
        isinstance(dependency, dict)
        and dependency.get("git") == source
        and isinstance(revision, str)
        and re.fullmatch(r"[0-9a-f]{40}", revision)
        and dependency.get("version") == "=1.0.0"
        and "path" not in dependency
    ):
        raise ValueError("release requires protocol 1.0.0 from an immutable official Git revision; local candidate paths cannot be packaged")
    packages = [package for package in lock.get("package", []) if package["name"] == "xscs-protocol"]
    if len(packages) != 1 or packages[0].get("version") != "1.0.0" or packages[0].get("source") != f"git+{source}?rev={revision}#{revision}":
        raise ValueError("protocol lock entry does not match the exact declared official source")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--require-tag", action="store_true")
    args = parser.parse_args()
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    try:
        verify_protocol_source(manifest, tomllib.loads((ROOT / "Cargo.lock").read_text()))
    except ValueError as error:
        parser.error(str(error))
    for source in COMMON_FILES:
        if not (ROOT / source).is_file():
            parser.error(f"required package input missing: {source}")
    if not args.output.is_absolute() or args.output.exists():
        parser.error("output must be a new absolute directory (no overwrites)")
    if run("git", "status", "--porcelain", "--untracked-files=all"):
        parser.error("packaging requires a clean committed source tree")
    sha = run("git", "rev-parse", "HEAD")
    version = manifest["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?", version):
        parser.error("unsafe Client version")
    if args.require_tag:
        tag = f"v{version}"
        if run("git", "cat-file", "-t", tag) != "tag" or run("git", "rev-parse", tag + "^{commit}") != sha:
            parser.error("annotated Client tag must resolve to this exact source")
    machine = platform.machine().lower()
    windows = platform.system() == "Windows"
    macos = platform.system() == "Darwin"
    if macos and machine in ("arm64", "aarch64"):
        target = "aarch64-apple-darwin"
    elif machine in ("x86_64", "amd64") and (windows or (platform.system() == "Linux" and platform.libc_ver()[0] == "glibc")):
        target = "x86_64-pc-windows-msvc" if windows else "x86_64-unknown-linux-gnu"
    else:
        parser.error("unsupported native OS/architecture")
    executable = "xscc.exe" if windows else "xscc"
    env = dict(os.environ, XSCC_BUILD_SHA=sha, CARGO_INCREMENTAL="0")
    if windows:
        env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -C target-feature=+crt-static"
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "xscc", "--target", target], cwd=ROOT, env=env, check=True)
    metadata = json.loads(run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
    binary = Path(metadata["target_directory"]) / target / "release" / executable
    expected = f"xscc {version} (git {sha}; xscs-management/1)"
    if run(str(binary), "--version") != expected:
        raise RuntimeError("binary source identity mismatch")
    name = f"xscc-{version}-{target}"
    with tempfile.TemporaryDirectory(prefix="client-package-") as tmp:
        stage = Path(tmp) / name
        stage.mkdir()
        shutil.copy2(binary, stage / executable)
        for source, destination in COMMON_FILES.items():
            shutil.copy2(ROOT / source, stage / destination)
        inventory_module().generate(stage, product='xscc', version=version,
            revision=sha, target=target, root_name='xscc',
            runtime_files=[{'path': executable, 'archive_entry': None}])
        subprocess.run([sys.executable,str(ROOT/'scripts/verify-release-inventory.py'),
                        '--stage',str(stage),'--target',target],check=True,
                       env=inventory_module().clean_environment())
        args.output.mkdir(parents=True, exist_ok=False)
        archive = args.output / (name + (".zip" if windows else ".tar.gz"))
        if windows:
            with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as out:
                for p in package_files(stage):
                    out.write(p, f"{name}/{p.relative_to(stage).as_posix()}")
        else:
            with tarfile.open(archive, "x:gz") as out:
                out.add(stage, arcname=name)
        (args.output / (archive.name + ".sha256")).write_text(f"{digest(archive)}  {archive.name}\n", encoding="utf-8")
    if windows:
        # MSI's ProductVersion is numeric; the source version remains in its manifest.
        subprocess.run(["powershell.exe", "-NoProfile", "-File", str(ROOT / "scripts/build-windows-installer.ps1"),
                        "-ClientExe", str(binary), "-Output", str(args.output), "-Version", version.split("-")[0], "-SourceVersion", version,
                        "-SourceRevision", sha, "-Target", target],
                       check=True, env={key: value for key, value in os.environ.items() if key.lower() != "psmodulepath"})
        numeric_version = version.split("-")[0]
        (args.output / f"xscc-{numeric_version}-windows-x64.msi").rename(
            args.output / f"xscc-{version}-windows-x64.msi")
        (args.output / f"xscc-{numeric_version}-windows-x64.msi.inner-sbom.cdx.json").rename(
            args.output / f"xscc-{version}-windows-x64.msi.inner-sbom.cdx.json")
    elif macos:
        subprocess.run(
            [
                "sh",
                str(ROOT / "packaging/macos/build-pkg.sh"),
                str(binary),
                version,
                str(args.output / f"xscc-{version}-macos-arm64-unsigned.pkg"),
                sha, target,
            ],
            check=True,
        )
    else:
        subprocess.run(["python3", str(ROOT / "scripts/build-linux-installer.py"), "--binary", str(binary),
                        "--output", str(args.output), "--version", version,
                        "--source-revision", sha, "--target", target], check=True)
    # WiX debug databases are build intermediates, not release assets.
    for debug_database in args.output.glob("*.wixpdb"):
        debug_database.unlink()
    for installer in (
        sorted(args.output.glob("*.msi"))
        + sorted(args.output.glob("*.deb"))
        + sorted(args.output.glob("*.pkg"))
    ):
        installer.with_name(installer.name + ".sha256").write_text(f"{digest(installer)}  {installer.name}\n")
        installer.with_name(installer.name + ".manifest.json").write_text(json.dumps({
            "product": "xscc", "version": version, "source_commit": sha,
            "authenticode_signed": False, "native_acceptance": "required", "notarized": False, "sha256": digest(installer), "target": target,
        }, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix='xscc-final-native-') as temporary:
        installers=sorted([*args.output.glob('*.msi'),*args.output.glob('*.deb'),*args.output.glob('*.pkg')])
        arguments=[sys.executable,str(ROOT/'scripts/package-native-delivery.py'),
                   '--stage',str(Path(temporary)/'delivery'),'--target',target,
                   '--output',str(args.output/(name+'-native-delivery.tar.gz'))]
        for installer in installers:
            arguments.extend(['--artifact',str(installer),'--inner-bom',str(installer.with_name(installer.name+'.inner-sbom.cdx.json'))])
        subprocess.run(arguments,check=True,env=inventory_module().clean_environment())
    if run("git", "rev-parse", "HEAD") != sha or run("git", "status", "--porcelain", "--untracked-files=all"):
        raise RuntimeError("source changed while packaging; do not publish output")
    print(f"Built {archive.name} at {sha}; installation and real Sunshine acceptance are separate gates.")


if __name__ == "__main__":
    main()
