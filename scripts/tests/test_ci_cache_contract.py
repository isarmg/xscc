"""Keep cache optimizations separate from release and native acceptance gates."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/ci.yml'


class CiCacheContractTests(unittest.TestCase):
    def setUp(self):
        self.workflow = WORKFLOW.read_text(encoding='utf-8')
        self.jobs = dict(re.findall(r'^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)',
                                    self.workflow.split('jobs:\n', 1)[1], re.M | re.S))

    def test_existing_required_jobs_and_publish_dependencies_remain(self):
        self.assertEqual(set(self.jobs), {'scope', 'client-linux-package',
                         'client-windows-package', 'client-macos-arm64', 'publish'})
        self.assertIn('needs: [client-linux-package, client-windows-package, client-macos-arm64]',
                      self.jobs['publish'])
        self.assertNotIn('continue-on-error:', self.workflow)

    def test_cache_keys_keep_platform_toolchain_job_and_lock_boundaries(self):
        for name in ('client-linux-package', 'client-windows-package', 'client-macos-arm64'):
            with self.subTest(job=name):
                job = self.jobs[name]
                self.assertEqual(job.count('uses: actions/cache@0400d5f644dc74513175e3cd8d07132dd4860809'), 2)
                self.assertIn("xscc-cargo-v1-${{ runner.os }}-${{ runner.arch }}-${{ github.job }}-rust-1.99.0-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml') }}-${{ github.sha }}", job)
                tools = job[job.index('      - name: Cache exact source-built inventory tools'):]
                tools = tools[:tools.index('      - name: Install or verify')]
                self.assertNotIn('restore-keys:', tools)
                self.assertIn("hashFiles('scripts/ci-inventory-tools.py')", tools)
                self.assertIn('python scripts/ci-inventory-tools.py --root', job)
                self.assertIn('python scripts/check-xcsc-source.py', job)
                self.assertIn('scripts/package-client.py', job)
                self.assertIn('scripts/check-client-package.py', job)

    def test_native_lifecycle_coverage_stays_present(self):
        for name in ('client-linux-package', 'client-windows-package'):
            self.assertIn('--install --installer', self.jobs[name])
            self.assertIn('cargo clippy --locked', self.jobs[name])
            self.assertIn('cargo test --locked', self.jobs[name])
        mac = self.jobs['client-macos-arm64']
        self.assertIn('scripts/native-service-acceptance.py', mac)
        self.assertIn('sudo installer -verboseR', mac)
        self.assertIn('cargo build --locked --release --target aarch64-apple-darwin', mac)
        self.assertNotIn('$PWD/target/release/xscc', mac)

    def test_release_inventory_pin_matches_current_release_notes(self):
        identity = json.loads((ROOT / 'build-tools-source.json').read_text(encoding='utf-8'))
        self.assertEqual(identity['repository'], 'https://github.com/isarmg/xssi.git')
        self.assertEqual(identity['revision'], '79d13d30a5941b422042a6027e3c4225325e9da8')
        self.assertIn(identity['revision'], (ROOT / 'docs/releases/1.0.0.md').read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
