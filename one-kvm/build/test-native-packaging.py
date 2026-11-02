#!/usr/bin/env python3
"""Exercise Debian packaging without installing or starting a KVM host."""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix="one-kvm-package-test-") as directory:
        fixture = Path(directory)
        (fixture / "build").mkdir()
        for relative in ("build/package-deb.sh", "build/one-kvm.service", "build/debian/copyright", "LICENSE", "SOURCE.json", "docs/managed-kvm-operations.md"):
            target = fixture / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, target)
        shutil.copytree(ROOT / "licenses", fixture / "licenses")
        (fixture / "Cargo.toml").write_text('[package]\nversion = "0.0.0-test"\n')
        binary = fixture / "target/x86_64-unknown-linux-gnu/release/one-kvm"
        binary.parent.mkdir(parents=True)
        shutil.copy2("/bin/true", binary)
        subprocess.run(["bash", str(fixture / "build/package-deb.sh"), "amd64"], check=True)
        package = next((fixture / "target/debian").glob("*.deb"))
        extracted, control = fixture / "extracted", fixture / "control"
        subprocess.run(["dpkg-deb", "--extract", str(package), str(extracted)], check=True)
        subprocess.run(["dpkg-deb", "--control", str(package), str(control)], check=True)
        assert (extracted / "usr/bin/one-kvm").read_bytes() == binary.read_bytes()
        assert (extracted / "usr/share/doc/one-kvm/copyright").read_bytes() == (ROOT / "build/debian/copyright").read_bytes()
        assert (extracted / "usr/share/doc/one-kvm/LICENSE").read_bytes() == (ROOT / "LICENSE").read_bytes()
        assert (extracted / "usr/share/doc/one-kvm/SOURCE.json").is_file()
        service = (extracted / "lib/systemd/system/one-kvm.service").read_text()
        assert "--data-dir /etc/one-kvm" in service and "UMask=0077" in service
        postinst = (control / "postinst").read_text()
        assert "systemctl start" not in postinst
        assert "install -d -m 0700 /etc/one-kvm" in postinst
        rejected = subprocess.run(["bash", str(fixture / "build/package-deb.sh"), "invalid-arch"], capture_output=True)
        assert rejected.returncode != 0
    print("Native Debian package, persistent private data and explicit startup verified.")


if __name__ == "__main__":
    main()
