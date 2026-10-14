"""Internal dependency injection for native layout/lifecycle tests, never shipped."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def make_linux_package(binary, output, version, *, umask=0o022):
    spec = importlib.util.spec_from_file_location('linux_native_test', ROOT / 'scripts/build-linux-installer.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    def record(stage, runtime_path, actual_version, revision, target):
        assert actual_version == version and target == 'x86_64-unknown-linux-gnu'
        assert (stage / runtime_path).read_bytes() == binary.read_bytes()
        share = stage / 'usr/share/doc/xscc'; share.mkdir(mode=0o755,parents=True,exist_ok=True); share.chmod(0o755)
        checksum = hashlib.sha256(binary.read_bytes()).hexdigest()
        for name, contents in {
            'runtime-sbom.cdx.json': json.dumps({'test_fixture': True, 'version': version}),
            'release-sbom.cdx.json': json.dumps({'test_fixture': True, 'path': runtime_path, 'sha256': checksum}),
            'THIRD_PARTY_RUNTIME_LICENSES.txt': 'Native layout dependency fixture; not a release runtime graph.\n',
        }.items():
            path = share / name; path.write_text(contents); path.chmod(0o644)
    arguments = ['build-linux-installer.py', '--binary', str(binary), '--output', str(output),
                 '--version', version,
                 '--source-revision', 'a' * 40, '--target', 'x86_64-unknown-linux-gnu']
    previous = os.umask(umask)
    try:
        with patch.object(sys, 'argv', arguments), patch.object(module, 'inventory_module', return_value=SimpleNamespace(native_inventory=record)):
            module.main()
    finally:
        os.umask(previous)
