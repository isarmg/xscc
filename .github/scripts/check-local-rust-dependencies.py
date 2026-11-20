#!/usr/bin/env python3
"""Verify that all checked-in Cargo lockfiles can use the local registry."""
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
REGISTRY = ROOT / "vendor/cargo-registry"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"


def index_file(name):
    name = name.lower()
    if len(name) < 3:
        parent = str(len(name))
    elif len(name) == 3:
        parent = f"3/{name[0]}"
    else:
        parent = f"{name[:2]}/{name[2:4]}"
    return REGISTRY / "index" / parent / name


def check():
    locks = subprocess.check_output(
        ["git", "ls-files", "-z", "*Cargo.lock"], cwd=ROOT
    ).decode().split("\0")
    packages = {}
    for lock in filter(None, locks):
        for package in tomllib.loads((ROOT / lock).read_text("utf-8"))["package"]:
            source = package.get("source")
            if source is None:
                continue
            if source != CRATES_IO:
                raise ValueError(f"{lock}: unexpected external source {source}")
            key = package["name"], package["version"]
            checksum = package["checksum"]
            if packages.setdefault(key, checksum) != checksum:
                raise ValueError(f"{lock}: inconsistent checksum for {key}")

    for (name, version), checksum in sorted(packages.items()):
        archive = REGISTRY / f"{name}-{version}.crate"
        if hashlib.sha256(archive.read_bytes()).hexdigest() != checksum:
            raise ValueError(f"{archive.name}: archive checksum differs")
        entries = map(json.loads, index_file(name).read_text("utf-8").splitlines())
        if not any(
            record["name"] == name and record["vers"] == version and record["cksum"] == checksum
            for record in entries
        ):
            raise ValueError(f"{archive.name}: package version missing from the local index")
    print(f"Verified {len(packages)} Cargo archives and their local index entries.")


if __name__ == "__main__":
    check()
