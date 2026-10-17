#!/usr/bin/env python3
import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
import warnings
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "prepare", Path(__file__).with_name("prepare-gpui-game-activity.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class SourceReplacementTests(unittest.TestCase):
    def setUp(self):
        self.original_sha = prepare.AAR_SHA256
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(setattr, prepare, "AAR_SHA256", self.original_sha)
        self.source = Path(self.directory.name) / "original.aar"
        self.output = Path(self.directory.name) / "filtered.aar"

    def fixture(self, duplicate=False):
        jar = io.BytesIO()
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(jar, "w") as archive:
                for name in prepare.REMOVED:
                    archive.writestr(name, b"original class")
                archive.writestr("GameActivity.class", b"unchanged class")
                if duplicate:
                    archive.writestr("GameActivity.class", b"duplicate class")
        with zipfile.ZipFile(self.source, "w") as archive:
            archive.writestr("classes.jar", jar.getvalue())
            archive.writestr("proguard.txt", b"keep JNI")
            archive.writestr("AndroidManifest.xml", b"unchanged manifest")
        prepare.AAR_SHA256 = hashlib.sha256(self.source.read_bytes()).hexdigest()

    def test_removes_only_replaced_classes_and_source_has_no_debug_formats(self):
        self.fixture()
        prepare.prepare(self.source, self.output)
        with zipfile.ZipFile(self.output) as output, zipfile.ZipFile(self.source) as original:
            self.assertEqual(set(output.namelist()), set(original.namelist()))
            for name in original.namelist():
                if name != "classes.jar":
                    self.assertEqual(output.read(name), original.read(name))
            with zipfile.ZipFile(io.BytesIO(output.read("classes.jar"))) as jar:
                self.assertEqual(jar.namelist(), ["GameActivity.class"])
                self.assertEqual(jar.read("GameActivity.class"), b"unchanged class")
        text = (ROOT / "packaging/gpui-android/vendor/game-text-input/src/main/java/com/google/androidgamesdk/gametextinput/InputConnection.java").read_text()
        self.assertNotRegex(text, r"Log\.[dv]\s*\(")
        self.assertNotIn("String.format", text)
        self.assertEqual(text.count("Log.i("), 2)

    def test_rejects_changed_upstream_hash(self):
        self.fixture()
        prepare.AAR_SHA256 = "0" * 64
        with self.assertRaisesRegex(ValueError, "Unexpected GameActivity AAR"):
            prepare.prepare(self.source, self.output)

    def test_rejects_duplicate_classes(self):
        self.fixture(duplicate=True)
        with self.assertRaisesRegex(ValueError, "Duplicate class"):
            prepare.prepare(self.source, self.output)


if __name__ == "__main__":
    unittest.main()
