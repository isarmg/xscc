import importlib.util
from pathlib import Path
import plistlib
import unittest

SPEC = importlib.util.spec_from_file_location("package_gpui_macos", Path(__file__).with_name("package-gpui-macos.py"))
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)

LOAD = """Load command 1
      cmd LC_BUILD_VERSION
  cmdsize 32
 platform 1
    minos 26.0
      sdk 26.0
   ntools 1
Load command 2
      cmd LC_LOAD_DYLIB
"""
DEPS = "xscc:\n\t/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit (compatibility version 45.0.0, current version 1.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)\n"


class MacOSPackageTests(unittest.TestCase):
    def test_separate_native_roles_and_host_privacy_descriptions(self):
        controller = PACKAGE.bundle_info("controller", "0.1.0-beta.1")
        host = PACKAGE.bundle_info("host", "0.1.0-beta.1")
        self.assertNotEqual(controller["CFBundleIdentifier"], host["CFBundleIdentifier"])
        self.assertEqual(host["LSMinimumSystemVersion"], "26.0")
        self.assertEqual(host["LSArchitecturePriority"], ["arm64"])
        self.assertNotIn("NSMicrophoneUsageDescription", controller)
        self.assertIn("NSAudioCaptureUsageDescription", host)
        self.assertEqual(plistlib.loads(plistlib.dumps(host)), host)
        for role, version in (("other", "0.1.0-beta.1"), ("host", "../bad")):
            with self.assertRaises(ValueError):
                PACKAGE.bundle_info(role, version)

    def test_only_native_arm64_macos26_executables_are_packaged(self):
        PACKAGE.validate_native_output("arm64\n", "ARM64 EXECUTE", LOAD, DEPS)
        for arch, header, load in (("x86_64", "EXECUTE", LOAD), ("arm64", "DYLIB", LOAD),
                                  ("arm64", "EXECUTE", LOAD.replace("minos 26.0", "minos 14.0")),
                                  ("arm64", "EXECUTE", LOAD.replace("platform 1", "platform 2"))):
            with self.assertRaises(ValueError):
                PACKAGE.validate_native_output(arch, header, load, DEPS)

    def test_unbundled_native_libraries_fail_before_packaging(self):
        for path in ("/opt/homebrew/lib/libvpx.dylib", "@rpath/libavcodec.dylib", "@executable_path/libopus.dylib"):
            with self.assertRaisesRegex(ValueError, "Unbundled native dependency"):
                PACKAGE.validate_native_output("arm64", "EXECUTE", LOAD, DEPS + f"\t{path} (compatibility version 1.0.0)\n")


if __name__ == "__main__":
    unittest.main()
