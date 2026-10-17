"""Source-contract gates; these do not replace device login/streaming tests."""
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class MobileManagedLoginContract(unittest.TestCase):
    def test_managed_mobile_login_returns_before_legacy_password_handling(self):
        source = (ROOT / 'src/client.rs').read_text()
        function = source.split('pub async fn handle_hash(', 1)[1].split(
            'fn try_get_password_from_personal_ab(', 1)[0]
        guard = '''#[cfg(any(
        feature = "gpui-session",
        all(feature = "managed", any(target_os = "android", target_os = "ios")),
    ))]'''
        managed = function.split(guard, 1)[1].split(
            'lc.write().unwrap().hash = hash.clone();', 1)[0]
        self.assertIn('if lc.read().unwrap().id.starts_with("managed:")', managed)
        self.assertIn('send_login(lc.clone(), String::new(), String::new(), Vec::new(), peer).await;', managed)
        self.assertIn('return true;', managed)
        self.assertNotIn('input-password', managed)
        self.assertNotIn('password.clone()', managed)
        self.assertIn('interface.msgbox("input-password"', function)

    def test_android_candidate_keeps_managed_without_lan_test_feature(self):
        cargo = (ROOT / 'Cargo.toml').read_text()
        defaults = next(line for line in cargo.splitlines() if line.startswith('default ='))
        self.assertIn('"managed"', defaults)
        script = (ROOT / 'flutter/ndk_arm64.sh').read_text()
        self.assertNotIn('--no-default-features', script)
        self.assertNotIn('test-lan-direct', script)
        self.assertIn('--platform 29 --target aarch64-linux-android build', script)


if __name__ == '__main__':
    unittest.main()
