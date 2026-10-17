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
    def test_playback_jni_requires_every_registered_entry(self):
        bridge = "Lorg/xscc/gpui/AndroidPlaybackAudio;"
        members = {
            (bridge, "<init>", "(Landroid/content/Context;J)V"),
            (bridge, "activate", "()Z"),
            (bridge, "close", "()V"),
            (bridge, "nativeStopped", "(JI)V"),
        }
        privacy.check_playback_jni({bridge}, members)
        with self.assertRaisesRegex(ValueError, "playback JNI"):
            privacy.check_playback_jni(set(), members)
        for missing in members:
            with self.subTest(missing=missing), self.assertRaisesRegex(ValueError, "playback JNI"):
                privacy.check_playback_jni({bridge}, members - {missing})

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


    def test_normalizes_only_verified_java8_access_synthetics(self):
        marker = privacy.JAVA8_ACCESS_MARKER
        self.assertEqual(privacy.normalize_java8_access(marker, 0x1020, 52, []),
                         (None, set()))
        ctor = privacy.JAVA8_ACCESS_CTOR
        real = (ctor[0], "<init>",
                "(Lcom/google/androidgamesdk/gametextinput/InputConnection;)V")
        owner, members = privacy.normalize_java8_access(
            ctor[0], 0x20, 52, [(ctor, 0x1000), (real, 0x2)])
        self.assertEqual(members, {real})
        # A same-named nonsynthetic declaration must not be silently exempted.
        self.assertEqual(privacy.normalize_java8_access(
            ctor[0], 0x20, 52, [(ctor, 0x1)])[1], {ctor})
        self.assertEqual(privacy.normalize_java8_access(marker, 0x20, 52, [])[0], marker)

    def test_missing_real_jni_entry_still_fails(self):
        owner = privacy.JAVA8_ACCESS_CTOR[0]
        real = (owner, "<init>", "(Lcom/google/androidgamesdk/gametextinput/InputConnection;)V")
        with self.assertRaisesRegex(ValueError, "members removed"):
            privacy.check_game_abi({owner}, {real}, {owner}, set())
        bridge = "Lorg/xscc/gpui/AndroidImeBridge;"
        classes = {bridge, bridge[:-1] + "$ImeSurfaceView;", bridge[:-1] + "$SessionConnection;"}
        members = {
            (bridge, "configureIme", "(JIIILjava/lang/String;IIIIZ)V"),
            (bridge, "hideIme", "(J)V"),
            (bridge, "nativeTextChanged", "(JLjava/lang/String;IIII)V"),
            (bridge, "nativeAction", "(JI)V"),
        }
        members.update(("Landroidx/core/view/WindowInsetsCompat$Type;", name, "()I")
                       for name in ("systemBars", "displayCutout", "ime"))
        members.update(("Landroidx/core/graphics/Insets;", name, "I")
                       for name in ("left", "top", "right", "bottom"))
        privacy.check_jni(classes, members)
        members.remove((bridge, "nativeTextChanged", "(JLjava/lang/String;IIII)V"))
        with self.assertRaisesRegex(ValueError, "JNI declarations missing"):
            privacy.check_jni(classes, members)


if __name__ == "__main__":
    unittest.main()
