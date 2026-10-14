#!/usr/bin/env python3
"""Independently unpack/verify an Client package; native service tests only on disposable CI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import tarfile
import tempfile
import zipfile


def run(*args):
    env = powershell_environment() if args[0] == "powershell.exe" else None
    return subprocess.check_output(args, text=True, env=env).strip()


def powershell_environment():
    # A pwsh-hosted runner must not inject PowerShell 7 modules into Windows
    # PowerShell 5.1. Let the child initialize its own standard module paths.
    return {key: value for key, value in os.environ.items() if key.lower() != "psmodulepath"}


def powershell(script):
    return run("powershell.exe", "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; " + script)


def machine_path_entry_count(path):
    expected = os.path.normcase(os.path.normpath(str(path)))
    machine_path = powershell("[Environment]::GetEnvironmentVariable('Path','Machine')")
    return sum(
        os.path.normcase(os.path.normpath(entry.strip())) == expected
        for entry in machine_path.split(";")
        if entry.strip()
    )


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify_inventory(root, manifest, target, sha):
    bom = json.loads((root / 'share/release-sbom.cdx.json').read_text())
    runtime = json.loads((root / 'share/runtime-sbom.cdx.json').read_text())
    for document in (bom, runtime):
        if document.get('bomFormat') != 'CycloneDX' or document.get('specVersion') != '1.5':
            raise ValueError('release runtime inventory is not CycloneDX 1.5')
    component = bom['metadata']['component']
    properties = bom['metadata']['properties']
    values = {value['name']: value['value'] for value in properties}
    if len(values) != len(properties) or component['name'] != 'xscc' or component['version'] != manifest['version'] or values.get('source_revision') != sha or values.get('target') != target:
        raise ValueError('release inventory differs from the compiled source identity')
    components = bom['components']
    refs = {value['bom-ref']: value for value in components}
    if len(refs) != len(components):
        raise ValueError('release inventory contains repeated component identities')
    actual = {value['name']: value['hashes'] for value in components if value['type'] == 'file'}
    expected = {path: [{'alg': 'SHA-256', 'content': checksum}]
                for path, checksum in manifest['files'].items()
                if path != 'share/release-sbom.cdx.json'}
    if actual != expected:
        raise ValueError('release inventory does not cover the actual payload files')
    native = runtime['metadata']['component']
    if native['name'] != 'xscc' or native['version'] != manifest['version'] or native['bom-ref'] not in refs or not runtime.get('components'):
        raise ValueError('resolved Client runtime graph is absent from the final inventory')
    executable = 'xscc.exe' if target.endswith('windows-msvc') else 'xscc'
    native_properties = {value['name']: value['value'] for value in refs[native['bom-ref']].get('properties', [])}
    bindings = json.loads(native_properties.get('release-file-bindings', '[]'))
    if bindings != [{'path': executable, 'sha256': digest(root / executable), 'archive_entry': None}]:
        raise ValueError('resolved runtime graph is not bound to the actual Client executable')


def verify(archive, destination, sha):
    windows = archive.suffix == ".zip"
    suffix = ".zip" if windows else ".tar.gz"
    name = archive.name.removesuffix(suffix)
    match = re.fullmatch(r"xscc-[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?-(?P<target>x86_64-pc-windows-msvc|x86_64-unknown-linux-gnu|aarch64-apple-darwin)", name)
    if match is None:
        raise ValueError("unexpected archive name")
    target = match["target"]
    if windows != (target == "x86_64-pc-windows-msvc"):
        raise ValueError("archive format does not match target")
    checksum = archive.with_name(archive.name + ".sha256").read_text().strip()
    if checksum != f"{digest(archive)}  {archive.name}":
        raise ValueError("archive digest mismatch")
    required={"xscc.exe" if windows else "xscc", "README.md", "platform-setup.md", "LICENSE", "bootstrap.example.json",
              'share/runtime-sbom.cdx.json','share/release-sbom.cdx.json','share/THIRD_PARTY_RUNTIME_LICENSES.txt',
              'share/package.json','share/package.sha256'}
    root = destination / name
    root.mkdir()
    seen = set()
    budget = 128 * 1024 * 1024

    def write(path, size, data, mode):
        nonlocal budget
        parts = path.split("/")
        relative = '/'.join(parts[1:])
        canonical=Path(relative).as_posix()
        if not parts or parts[0] != name or not relative or canonical!=relative or any(value in ('','..','.') for value in parts[1:]) or "\\" in relative or relative.startswith('/') or (relative not in required and not relative.startswith('share/')) or relative in seen:
            raise ValueError("unexpected or duplicate archive entry")
        budget -= size
        if size < 0 or budget < 0:
            raise ValueError("archive budget exceeded")
        payload = data.read(size + 1)
        if len(payload) != size:
            raise ValueError("archive entry size mismatch")
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open("xb") as output:
            output.write(payload)
        destination.chmod(mode & 0o777)
        seen.add(relative)

    if windows:
        with zipfile.ZipFile(archive) as source:
            for entry in source.infolist():
                if entry.is_dir() or (entry.external_attr >> 16) & 0o170000 not in (0, 0o100000):
                    raise ValueError("non-file ZIP entry")
                with source.open(entry) as data:
                    write(entry.filename, entry.file_size, data, (entry.external_attr >> 16) & 0o777 or 0o644)
    else:
        with tarfile.open(archive) as source:
            for entry in source:
                if entry.isdir() and (entry.name==name or entry.name.startswith(name+'/share/')) and '..' not in entry.name.split('/'):
                    continue
                if not entry.isfile():
                    raise ValueError("non-file TAR entry")
                with source.extractfile(entry) as data:
                    write(entry.name, entry.size, data, entry.mode)
    if not required<=seen:
        raise ValueError("incomplete package")
    raw=(root/'share/package.json').read_bytes()
    if (root/'share/package.sha256').read_bytes()!=(hashlib.sha256(raw).hexdigest()+'\n').encode():
        raise ValueError('release manifest checksum mismatch')
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result:raise ValueError('release manifest has duplicate fields')
            result[key]=value
        return result
    authority=json.loads(raw,object_pairs_hook=unique)
    if set(authority)!= {'manifest_version','kind','identity','files'} or type(authority['manifest_version']) is not int or authority['manifest_version']!=1 or authority['kind']!='release-inventory':
        raise ValueError('unsupported release manifest')
    identity=authority['identity']
    if set(identity)!= {'product','version','source_revision','target'} or identity['source_revision']!=sha or identity['target']!=target or identity['product']!='xscc' or name!=f"xscc-{identity['version']}-{target}":
        raise ValueError('package identity mismatch')
    payload=seen-{'share/package.json','share/package.sha256'}
    expected=[]
    for relative in sorted(payload):
        path=root/relative
        expected.append({'path':relative,'sha256':digest(path),'size':path.stat().st_size,'mode':stat.S_IMODE(path.stat().st_mode)})
    if windows and os.name=='nt':
        recorded=authority['files']
        if [dict(value,mode=0) for value in recorded]!=[dict(value,mode=0) for value in expected]:
            raise ValueError('manifest file inventory mismatch')
    elif authority['files']!=expected:
        raise ValueError('manifest file inventory mismatch')
    manifest={'version':identity['version'],'files':{value['path']:value['sha256'] for value in expected if value['path']!='share/release-sbom.cdx.json'}}
    verify_inventory(root, manifest, target, sha)
    binary = root / ("xscc.exe" if windows else "xscc")
    binary.chmod(0o755)
    if run(str(binary), "--version") != f"xscc {manifest['version']} (git {sha}; xscs-management/1)":
        raise ValueError("executable identity mismatch")
    return root, binary


def install_test(root, binary, temporary, installer=None, seed=None):
    # This test intentionally creates a real service/account, but never on a user's host.
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise ValueError("--install is restricted to disposable GitHub-hosted runners")
    if installer is None:
        raise ValueError("--install requires the native platform installer")
    windows = platform.system() == "Windows"
    ca = temporary / "ca.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(temporary / "key.pem"), "-out", str(ca), "-days", "1", "-subj", "/CN=Client installation test", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "subjectAltName=IP:127.0.0.1"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    fixture = temporary / "private-bootstrap"
    fixture.mkdir(mode=0o700)
    bootstrap = fixture / "bootstrap.json"
    bootstrap.write_text(json.dumps({"manager_endpoint": "wss://127.0.0.1:9/xscc/v1/connect", "enrollment_token": "a" * 36, "sunshine_endpoint": "https://127.0.0.1:47990/", "sunshine_username": "test", "sunshine_password": "installation-fixture-only"}), encoding="utf-8")
    bootstrap.chmod(0o600)
    if windows:
        # Protect only the secret fixture, not extracted executable/scripts. Set explicit
        # file ACEs as well as directory inheritance; recursive icacls can remove access
        # from archive files when it disables inherited ACEs at every descendant.
        escaped = str(fixture).replace("'", "''")
        powershell(f"""
$current=[Security.Principal.WindowsIdentity]::GetCurrent().User
foreach($path in @('{escaped}', '{escaped}\\bootstrap.json')) {{
  $directory=(Get-Item -LiteralPath $path).PSIsContainer
  $acl=if($directory){{New-Object System.Security.AccessControl.DirectorySecurity}}else{{New-Object System.Security.AccessControl.FileSecurity}}
  $acl.SetAccessRuleProtection($true,$false)
  $acl.SetOwner($current)
  foreach($sid in @($current.Value,'S-1-5-18','S-1-5-32-544')) {{
    $identity=New-Object System.Security.Principal.SecurityIdentifier $sid
    $inherit=if($directory){{[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'}}else{{[System.Security.AccessControl.InheritanceFlags]::None}}
    $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($identity,'FullControl',$inherit,'None','Allow')
    $acl.AddAccessRule($rule)
  }}
  Set-Acl -LiteralPath $path -AclObject $acl
}}
""")
        subprocess.run(['msiexec.exe', '/i', str(installer), '/qn', '/norestart'], check=True)
        installed = Path(os.environ['ProgramFiles']) / 'Xscc'
        if machine_path_entry_count(installed) != 1:
            raise ValueError('MSI did not add exactly one machine PATH entry')
        if (installed / 'xscc-tray.exe').exists():
            raise ValueError('CLI installer contains a tray executable')
        subprocess.run([str(installed / 'xscc.exe'), 'init', '--config', str(Path(os.environ['ProgramData']) / 'Xscc'), '--bootstrap', str(bootstrap)], check=True, timeout=60)
        if powershell("(Get-Service Xscc).Status") != 'Stopped':
            raise ValueError('unpaired service was started by installer')
        if powershell("(Get-Service Xscc).StartType") != 'Automatic':
            raise ValueError('new Windows service is not registered for automatic system startup')
        state_file = Path(os.environ['ProgramData']) / 'Xscc/provisioning/state.sqlite3'
        before = state_file.read_bytes()
        subprocess.run(['msiexec.exe', '/i', str(installer), '/qn', '/norestart'], check=True)
        if machine_path_entry_count(installed) != 1:
            raise ValueError('MSI repair duplicated or removed its machine PATH entry')
        if state_file.read_bytes() != before:
            raise ValueError("repair changed existing protected state")

        if seed:
            exercise_native_service(Path(os.environ['ProgramFiles']) / 'Xscc/xscc.exe', seed, Path(os.environ['ProgramData']) / 'Xscc')
        if installer:
            subprocess.run(['msiexec.exe', '/x', str(installer), '/qn', '/norestart'], check=True)
            if (installed / 'xscc-tray.exe').exists():
                raise ValueError('MSI uninstall retained tray executable')
            if machine_path_entry_count(installed) != 0:
                raise ValueError('MSI uninstall retained its machine PATH entry')
        if not (Path(os.environ["ProgramData"]) / "Xscc/provisioning/state.sqlite3").is_file():
            raise ValueError("uninstall removed protected state")
    else:
        subprocess.run(['dpkg', '--install', str(installer)], check=True)
        # Offline installation is separate from successful online pairing.
        subprocess.run([str(binary), 'init', '--config', '/var/lib/xscc', '--bootstrap', str(bootstrap)], check=True)
        subprocess.run(['chown', '-R', 'xscc:xscc', '/var/lib/xscc'], check=True)
        state_file = Path('/var/lib/xscc/provisioning/bootstrap.json')
        before = state_file.read_bytes()
        subprocess.run(['dpkg', '--install', str(installer)], check=True)
        if state_file.read_bytes() != before:
            raise ValueError("repair changed existing protected state")
        if subprocess.run(["systemctl", "is-active", "--quiet", "xscc.service"]).returncode == 0:
            raise ValueError("unpaired service was started by installer")
        if subprocess.run(["systemctl", "is-enabled", "--quiet", "xscc.service"]).returncode != 0:
            raise ValueError("fresh Linux installation did not enable systemd startup")
        pending_run = subprocess.run([str(binary), "run", "--config", "/var/lib/xscc", "--format", "json"], capture_output=True, timeout=60)
        if pending_run.returncode != 4:
            raise ValueError("unpaired run did not return awaiting_pairing")
        state = Path("/var/lib/xscc/provisioning")
        if (state.stat().st_mode & 0o077) != 0 or not (state / "bootstrap.json").is_file():
            raise ValueError("protected pending configuration missing")
        if seed:
            exercise_native_service(Path('/opt/xscc/xscc'), seed, Path('/var/lib/xscc'), 'xscc:xscc')
        subprocess.run(['dpkg', '--remove', 'xscc'], check=True)
        if not (state / "bootstrap.json").is_file():
            raise ValueError("uninstall removed protected state")
    print("Native offline install, default boot-start policy, program replacement and state-preserving uninstall passed; online service behavior requires enrolled-device acceptance.")


def exercise_native_service(binary, seed, state, service_user=None):
    import importlib.util
    spec = importlib.util.spec_from_file_location("native_service_acceptance", Path(__file__).with_name("native-service-acceptance.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.exercise(binary, seed, state, service_user)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--installer", type=Path, help="MSI/DEB to exercise instead of script installation")
    parser.add_argument("--seed", type=Path, help="native CI fixture executable for real service/IPC acceptance")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
        parser.error("full source SHA required")
    with tempfile.TemporaryDirectory(prefix="client-check-") as tmp:
        temporary = Path(tmp)
        root, binary = verify(args.archive.resolve(), temporary, args.sha)
        print("Independent archive, manifest, checksums and executable identity verified.")
        if args.install:
            if args.installer:
                expected = args.installer.with_name(args.installer.name + '.manifest.json')
                manifest = json.loads(expected.read_text())
                if manifest['source_commit'] != args.sha or manifest['sha256'] != digest(args.installer):
                    raise ValueError('installer source or checksum mismatch')
            install_test(root, binary, temporary, args.installer.resolve() if args.installer else None, args.seed.resolve() if args.seed else None)


if __name__ == "__main__":
    main()
