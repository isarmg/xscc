import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

check_source = runpy.run_path(str(Path(__file__).with_name('check-gpui-workflows.py')))['check_hwcodec_source']


class SourceInventoryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.metadata = {'local_changes': ['patched.rs'], 'source_inventory': 'SOURCE_FILES.sha256', 'source_file_count': 2}
        (self.root / 'upstream.json').write_text(json.dumps(self.metadata))
        (self.root / 'original.rs').write_bytes(b'original')
        (self.root / 'patched.rs').write_bytes(b'local patch')
        digest = hashlib.sha256(b'original').hexdigest()
        (self.root / 'SOURCE_FILES.sha256').write_text(f'{digest}  original.rs\n{digest}  patched.rs\n')

    def test_recorded_patches_preserve_original_inventory(self):
        check_source(self.root)

    def test_unrecorded_modification_is_rejected(self):
        (self.root / 'original.rs').write_bytes(b'unrecorded change')
        with self.assertRaisesRegex(ValueError, 'Unrecorded'):
            check_source(self.root)

    def test_incomplete_inventory_is_rejected(self):
        self.metadata['source_file_count'] = 3
        (self.root / 'upstream.json').write_text(json.dumps(self.metadata))
        with self.assertRaisesRegex(ValueError, 'count differs'):
            check_source(self.root)


if __name__ == '__main__':
    unittest.main()
