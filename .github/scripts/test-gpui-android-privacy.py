#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "privacy", Path(__file__).with_name("verify-gpui-android-privacy.py")
)
privacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(privacy)


class PrivacyTests(unittest.TestCase):
    def test_rejects_packaged_debug_logging(self):
        for name in ("d", "v"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Log"):
                privacy.check_privacy([], [("Landroid/util/Log;", name, "()I")])

    def test_rejects_remaining_plaintext_format_construction(self):
        with self.assertRaisesRegex(ValueError, "log-format string"):
            privacy.check_privacy(["processKeyEvent(key=%d) text=%s"], [])

    def test_preserves_warning_and_error_diagnostics(self):
        privacy.check_privacy(["invalid length"], [
            ("Landroid/util/Log;", name, "()I") for name in ("i", "w", "e")
        ])


if __name__ == "__main__":
    unittest.main()
