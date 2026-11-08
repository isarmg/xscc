#!/usr/bin/env python3
"""CPU-only contracts for the pinned CI vcpkg setup."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "setup_pinned_vcpkg", Path(__file__).with_name("setup-pinned-vcpkg.py"),
)
SETUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SETUP)


class SetupTests(unittest.TestCase):
    def test_host_assets_keep_the_pinned_release_and_metadata_hashes(self):
        metadata = {
            "VCPKG_TOOL_RELEASE_TAG": "2026-07-27",
            "VCPKG_MACOS_SHA": "a" * 128,
            "VCPKG_GLIBC_SHA": "b" * 128,
            "VCPKG_GLIBC_ARM64_SHA": "c" * 128,
        }
        windows = SETUP.tool_spec(metadata, "Windows", "AMD64")
        self.assertEqual(windows, (
            "https://api.github.com/repos/microsoft/vcpkg-tool/releases/assets/492070117",
            "13b8175e99a884c5ad34249218754b45541a1a63f216e92603aee57a285ac741",
            "sha256", 6745912,
        ))
        for system, machine, name, digest in [
            ("Darwin", "arm64", "vcpkg-macos", "a" * 128),
            ("Linux", "x86_64", "vcpkg-glibc", "b" * 128),
            ("Linux", "aarch64", "vcpkg-glibc-arm64", "c" * 128),
        ]:
            self.assertEqual(SETUP.tool_spec(metadata, system, machine), (
                "https://github.com/microsoft/vcpkg-tool/releases/download/2026-07-27/" + name,
                digest, "sha512", None,
            ))
        with self.assertRaises(RuntimeError):
            SETUP.tool_spec(dict(metadata, VCPKG_TOOL_RELEASE_TAG="2026-07-28"), "Linux", "x86_64")
        with self.assertRaises(RuntimeError):
            SETUP.tool_spec(dict(metadata, VCPKG_GLIBC_SHA="b" * 64), "Linux", "x86_64")

    def test_an_existing_non_repository_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sentinel = root / "preserve.txt"
            sentinel.write_text("existing contents")
            with self.assertRaisesRegex(RuntimeError, "nonempty"):
                SETUP.checkout(root)
            self.assertEqual(sentinel.read_text(), "existing contents")
            self.assertEqual(list(root.iterdir()), [sentinel])

    def test_exports_preserve_existing_environment_and_require_the_tool_version(self):
        SETUP.check_version(
            "vcpkg package management program version 2026-07-27-98d7cb0cf1f4686a3e43aa5672b6230c1d56bce8"
        )
        with self.assertRaises(RuntimeError):
            SETUP.check_version("vcpkg package management program version 2026-07-24-other")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "chosen vcpkg"
            env_file = Path(directory) / "env"
            path_file = Path(directory) / "path"
            output_file = Path(directory) / "output"
            original = "VCPKG_DEFAULT_TRIPLET=arm64-ios\nVCPKG_BINARY_SOURCES=preserved\n"
            env_file.write_text(original)
            path_file.write_text("existing-path\n")
            SETUP.export_environment(root, env_file, path_file, output_file)
            self.assertEqual(env_file.read_text(), original + (
                f"VCPKG_ROOT={root}\nRUNVCPKG_VCPKG_ROOT={root}\nVCPKG_DISABLE_METRICS=1\n"
            ))
            self.assertEqual(path_file.read_text(), f"existing-path\n{root}\n")
            self.assertEqual(output_file.read_text(), f"RUNVCPKG_VCPKG_ROOT_OUT={root}\n")
            with self.assertRaises(RuntimeError):
                SETUP.export_environment(Path("invalid\nroot"), env_file, path_file)
            self.assertTrue(env_file.read_text().startswith(original))


if __name__ == "__main__":
    unittest.main()
