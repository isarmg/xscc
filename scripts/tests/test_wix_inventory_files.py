import importlib.util
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

SPEC=importlib.util.spec_from_file_location('wix_inventory_files',Path(__file__).resolve().parents[1]/'wix-inventory-files.py')
module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)
NS={'w':module.NAMESPACE}


class WixInventoryFilesTests(unittest.TestCase):
    def test_every_actual_nested_build_input_is_authored_and_identity_is_stable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary).resolve(strict=True);payload=root/'share';payload.mkdir()
            for name in ('release-sbom.cdx.json','build-inputs/sqlite/sqlite3.c','licenses/native/TLS/COPYRIGHT.txt'):
                file=payload/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(name)
            first=root/'first.wxs';second=root/'second.wxs'
            module.generate(payload,first,'xscc');module.generate(payload,second,'xscc')
            self.assertEqual(first.read_bytes(),second.read_bytes())
            tree=ET.parse(first)
            files=tree.findall('.//w:File',NS)
            self.assertEqual({Path(item.attrib['Source']).relative_to(payload).as_posix() for item in files},
                {'release-sbom.cdx.json','build-inputs/sqlite/sqlite3.c','licenses/native/TLS/COPYRIGHT.txt'})
            references=tree.findall('.//w:ComponentGroup/w:ComponentRef',NS)
            self.assertEqual({item.attrib['Id'] for item in references},
                {item.attrib['Id'] for item in tree.findall('.//w:Component',NS)})

    def test_linked_evidence_is_rejected_before_output_authority(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary).resolve(strict=True);payload=root/'share';payload.mkdir()
            (root/'external').write_text('outside input')
            (payload/'input').symlink_to(root/'external')
            output=root/'inventory.wxs'
            with self.assertRaises(ValueError):module.generate(payload,output,'xscc')
            self.assertFalse(output.exists())


if __name__=='__main__':unittest.main()
