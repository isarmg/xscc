import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC=importlib.util.spec_from_file_location('client_release_inventory',Path(__file__).resolve().parents[1]/'release-inventory.py')
inventory=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(inventory)


class ReleaseInventoryTests(unittest.TestCase):
    def test_unbound_or_mutable_tool_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            for value in ({'repository':inventory.TOOLS_REPOSITORY,'revision':None},
                          {'repository':inventory.TOOLS_REPOSITORY,'revision':'main'},
                          {'repository':'https://github.com/another-owner/xssi.git','revision':'a'*40}):
                with self.subTest(value=value):
                    (root/'build-tools-source.json').write_text(json.dumps(value))
                    with patch.object(inventory,'ROOT',root), self.assertRaises(ValueError):
                        inventory.tools_identity()

    def test_source_export_is_exact_and_refuses_changed_or_uncommitted_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);source=root/'git';source.mkdir()
            def git(*args):
                return subprocess.check_output(['git',*args],cwd=source,text=True).strip()
            git('init','--quiet');(source/'current.c').write_bytes(b'committed build source\n')
            git('add','current.c')
            git('-c','user.name=Release fixture','-c','user.email=fixture@example.invalid','-c','commit.gpgsign=false','commit','--quiet','-m','actual source fixture')
            revision=git('rev-parse','HEAD')
            with patch.object(inventory,'ROOT',source):
                exported=inventory.exported_source(root/'export',revision,inventory.clean_environment())
                self.assertEqual((exported/'current.c').read_bytes(),(source/'current.c').read_bytes())
                self.assertFalse((exported/'.git').exists())
                (source/'current.c').write_bytes(b'changed build source\n')
                with self.assertRaisesRegex(ValueError,'exact clean compiled source'):
                    inventory.exported_source(root/'changed-export',revision,inventory.clean_environment())
                self.assertFalse((root/'changed-export').exists())

    def test_actual_binary_and_archive_bindings_are_hashed_and_cannot_escape_stage(self):
        with tempfile.TemporaryDirectory() as temporary:
            stage=Path(temporary);binary=b'actual fixture payload bytes'
            (stage/'client').write_bytes(binary)
            with zipfile.ZipFile(stage/'client.apk','w') as archive:
                archive.writestr('lib/arm64-v8a/libclient.so',binary)
            expected=hashlib.sha256(binary).hexdigest()
            self.assertEqual(inventory.file_binding(stage,{'path':'client','archive_entry':None})['sha256'],expected)
            self.assertEqual(inventory.file_binding(stage,{'path':'client.apk','archive_entry':'lib/arm64-v8a/libclient.so'})['sha256'],expected)
            for value in ({'path':'../client','archive_entry':None},
                          {'path':'client.apk','archive_entry':'../libclient.so'},
                          {'path':'/tmp/client','archive_entry':None}):
                with self.subTest(value=value),self.assertRaises(ValueError):
                    inventory.file_binding(stage,value)

    def test_build_credentials_are_absent_but_proxy_and_trust_remain(self):
        environment={'GH_TOKEN':'credential','GITHUB_TOKEN':'credential','SERVICE_PASSWORD':'credential',
                     'BUILD_SIGNING_TEST_CREDENTIAL':'credential','SERVICE_TOKEN':'credential','PRIVATE_SECRET':'credential',
                     'HTTPS_PROXY':'https://proxy.example.invalid:8443','CARGO_HTTP_CAINFO':'/tmp/public-ca.pem',
                     'SOURCE_REVISION':'a'*40}
        with patch.dict(os.environ,environment,clear=True):
            self.assertEqual(inventory.clean_environment(),{name:environment[name] for name in ('HTTPS_PROXY','CARGO_HTTP_CAINFO','SOURCE_REVISION')})


if __name__=='__main__':unittest.main()
