import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PureWindowsPath
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

spec = importlib.util.spec_from_file_location("client_package", Path(__file__).resolve().parents[1] / "check-client-package.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)
SHA = "a" * 40
VERSION = "0.1.0-rc.1"


class PackageTests(unittest.TestCase):
    def test_packager_requires_immutable_protocol_input_and_matching_lock(self):
        builder_spec = importlib.util.spec_from_file_location("client_builder", Path(__file__).resolve().parents[1] / "package-client.py")
        builder = importlib.util.module_from_spec(builder_spec)
        builder_spec.loader.exec_module(builder)
        source = "https://github.com/isarmg/xscs.git"
        dependency = {"git": source, "rev": SHA, "version": "=1.0.0"}
        lock = {"package": [{"name": "xscs-protocol", "version": "1.0.0", "source": f"git+{source}?rev={SHA}#{SHA}"}]}
        builder.verify_protocol_source({"dependencies": {"xscs-protocol": dependency}}, lock)
        for invalid in [
            {"path": "../xscs/crates/protocol", "version": "=1.0.0"},
            dict(dependency, rev="main"),
            dict(dependency, git="https://github.com/another-owner/xscs.git"),
            dict(dependency, version="1.0.0"),
        ]:
            with self.subTest(dependency=invalid), self.assertRaisesRegex(ValueError, "release requires protocol"):
                builder.verify_protocol_source({"dependencies": {"xscs-protocol": invalid}}, lock)
        for invalid_lock in [
            {"package": []},
            {"package": [{"name": "xscs-protocol", "version": "1.0.0"}]},
            {"package": [{"name": "xscs-protocol", "version": "1.0.0", "source": f"git+{source}?rev={SHA}#{'b' * 40}"}]},
        ]:
            with self.subTest(lock=invalid_lock), self.assertRaisesRegex(ValueError, "protocol lock entry"):
                builder.verify_protocol_source({"dependencies": {"xscs-protocol": dependency}}, invalid_lock)

    def test_windows_powershell_does_not_inherit_pwsh_modules(self):
        with patch.dict(os.environ, {"PSModulePath": "incompatible-pwsh-modules", "KEEP_TEST_VALUE": "kept"}, clear=True):
            self.assertEqual(checker.powershell_environment(), {"KEEP_TEST_VALUE": "kept"})

    @unittest.skipUnless(os.name == "nt", "Windows PowerShell native check")
    def test_windows_security_module_loads(self):
        self.assertEqual(checker.powershell("Import-Module Microsoft.PowerShell.Security; (Get-Command Set-Acl).Name"), "Set-Acl")

    def test_packager_inputs_exist_in_standalone_checkout(self):
        builder_spec = importlib.util.spec_from_file_location("client_builder", Path(__file__).resolve().parents[1] / "package-client.py")
        builder = importlib.util.module_from_spec(builder_spec)
        builder_spec.loader.exec_module(builder)
        for source in builder.COMMON_FILES:
            self.assertTrue((builder.ROOT / source).is_file(), source)
        with tempfile.TemporaryDirectory() as temporary:
            directory=Path(temporary)
            (directory/'share').mkdir()
            for name in ['bootstrap.example.json','README.md','LICENSE','share/runtime-sbom.cdx.json']:
                (directory/name).write_bytes(b'input')
            self.assertEqual([p.relative_to(directory).as_posix() for p in builder.package_files(directory)],
                             ['LICENSE','README.md','bootstrap.example.json','share/runtime-sbom.cdx.json'])

    def fixture(self, temporary, windows=False, extra=None, wrong_sha=False, mac_target=None,
                inventory_target=None, unbound=False):
        target = mac_target or ("x86_64-pc-windows-msvc" if windows else "x86_64-unknown-linux-gnu")
        name = f"xscc-{VERSION}-{target}"
        names = ["xscc.exe" if windows else "xscc", "README.md", "platform-setup.md", "LICENSE", "bootstrap.example.json"]
        files = {n: b"fixture" for n in names}
        runtime_ref='pkg:cargo/xscc@'+VERSION+'?source_revision='+SHA
        runtime={'bomFormat':'CycloneDX','specVersion':'1.5',
                 'metadata':{'component':{'type':'application','name':'xscc','version':VERSION,'bom-ref':runtime_ref}},
                 'components':[{'type':'library','name':'fixture-dependency','version':'1.0.0','bom-ref':'fixture-dependency'}],
                 'dependencies':[{'ref':runtime_ref,'dependsOn':['fixture-dependency']},
                                 {'ref':'fixture-dependency','dependsOn':[]}]}
        files['share/runtime-sbom.cdx.json']=json.dumps(runtime).encode()
        files['share/THIRD_PARTY_RUNTIME_LICENSES.txt']=b'Test-only actual inventory fixture.\n'
        file_components=[{'type':'file','bom-ref':'urn:release-file:'+n,'name':n,
                          'hashes':[{'alg':'SHA-256','content':hashlib.sha256(b).hexdigest()}]}
                         for n,b in files.items()]
        root_component=dict(runtime['metadata']['component'])
        binding=[] if unbound else [{'path':names[0],'sha256':hashlib.sha256(files[names[0]]).hexdigest(),'archive_entry':None}]
        root_component['properties']=[{'name':'release-file-bindings','value':json.dumps(binding)}]
        sbom={'bomFormat':'CycloneDX','specVersion':'1.5',
              'metadata':{'component':{'type':'application','name':'xscc','version':VERSION,'bom-ref':'release'},
                          'properties':[{'name':'source_revision','value':SHA},
                                        {'name':'target','value':inventory_target or target}]},
              'components':[root_component,*runtime['components'],*file_components]}
        files['share/release-sbom.cdx.json']=json.dumps(sbom).encode()
        identity={'product':'xscc','version':VERSION,'source_revision':'b'*40 if wrong_sha else SHA,'target':target}
        manifest={'manifest_version':1,'kind':'release-inventory','identity':identity,
                  'files':[{'path':n,'sha256':hashlib.sha256(b).hexdigest(),'size':len(b),'mode':0o600 if windows else 0o644}
                           for n,b in sorted(files.items())]}
        raw=json.dumps(manifest).encode();files['share/package.json']=raw
        files['share/package.sha256']=(hashlib.sha256(raw).hexdigest()+'\n').encode()
        entries = [(name + "/" + n, b) for n, b in files.items()]
        if extra is not None:
            entries.append((extra.replace("ROOT", name), b"bad"))
        archive = temporary / (name + (".zip" if windows else ".tar.gz"))
        if windows:
            with zipfile.ZipFile(archive, "w") as out:
                for n, b in entries:
                    out.writestr(n, b)
        else:
            with tarfile.open(archive, "w:gz") as out:
                for n, b in entries:
                    entry = tarfile.TarInfo(n)
                    entry.size = len(b)
                    entry.mode = 0o644
                    out.addfile(entry, io.BytesIO(b))
        archive.with_name(archive.name + ".sha256").write_text(f"{checker.digest(archive)}  {archive.name}\n")
        return archive

    def test_both_platform_packages(self):
        for windows in [False, True]:
            with self.subTest(windows=windows), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = self.fixture(root, windows=windows)
                with patch.object(checker, "run", return_value=f"xscc {VERSION} (git {SHA}; xscs-management/1)"):
                    checker.verify(archive, root, SHA)

    def test_macos_archives_keep_their_actual_architecture(self):
        for target in ["aarch64-apple-darwin"]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = self.fixture(root, mac_target=target)
                with patch.object(checker, "run", return_value=f"xscc {VERSION} (git {SHA}; xscs-management/1)"):
                    checker.verify(archive, root, SHA)

    def test_intel_macos_archive_is_not_a_supported_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root, mac_target="x86_64-apple-darwin")
            with patch.object(checker, "run") as executable, self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)
            executable.assert_not_called()

    def test_traversal_extra_and_duplicate_entries_rejected(self):
        for windows in [False, True]:
            for extra in ["ROOT/../escape", "ROOT/unknown", "ROOT/README.md", "/absolute",
                          "ROOT/share/unknown", "ROOT/share/../escape", "ROOT/share/runtime-sbom.cdx.json"]:
                with self.subTest(windows=windows, extra=extra), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    archive = self.fixture(root, windows=windows, extra=extra)
                    with self.assertRaises(ValueError):
                        checker.verify(archive, root, SHA)

    def test_source_mismatch_rejected_before_execution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root, wrong_sha=True)
            with patch.object(checker, "run") as executable, self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)
            executable.assert_not_called()

    def test_resolved_inventory_cannot_claim_another_target_or_drop_binary_binding(self):
        for arguments in ({'inventory_target':'aarch64-apple-darwin'},{'unbound':True}):
            with self.subTest(arguments=arguments), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp)
                archive=self.fixture(root,**arguments)
                with patch.object(checker,'run') as executable, self.assertRaisesRegex(ValueError,'inventory|runtime graph'):
                    checker.verify(archive,root,SHA)
                executable.assert_not_called()

    def test_tampered_archive_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root)
            with archive.open("ab") as file:
                file.write(b"modified")
            with self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)

    def test_install_test_refuses_user_host(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), self.assertRaises(ValueError):
            checker.install_test(None, None, None)


if __name__ == "__main__":
    unittest.main()
