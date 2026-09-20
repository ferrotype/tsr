"""Small staging and schema counterexamples; no compiler build or workload."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE))
import apply


class StagingContracts(unittest.TestCase):
    def stage(self, path):
        for relative in apply.patches(apply.ROOT):
            source=apply.ROOT/relative
            if source.exists():
                dest=path/relative;dest.parent.mkdir(parents=True,exist_ok=True)
                dest.write_bytes(source.read_bytes())
        cargo=path/'crates/tsr_ast/Cargo.toml';cargo.write_bytes((apply.ROOT/'crates/tsr_ast/Cargo.toml').read_bytes())

    def test_production_and_alias_destinations_are_rejected_before_mutation(self):
        with self.assertRaisesRegex(ValueError,'production'):
            apply.apply(apply.ROOT)
        with tempfile.TemporaryDirectory() as temp:
            stage=Path(temp);self.stage(stage)
            path=stage/'crates/tsr_ast/src/symbols.rs'; path.unlink()
            path.symlink_to(apply.ROOT/'crates/tsr_ast/src/symbols.rs')
            sentinel=stage/'crates/tsr_ast/src/lib.rs';before=sentinel.read_bytes()
            with self.assertRaisesRegex(ValueError,'aliases production'):
                apply.apply(stage)
            self.assertEqual(sentinel.read_bytes(),before)
            self.assertFalse((stage/'census.patch').exists())

    def test_additive_receipt_binds_every_modified_byte_and_reapplication_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            stage=Path(temp);self.stage(stage)
            before={str(p.relative_to(stage)):p.read_bytes() for p in stage.rglob('*.rs')}
            record=apply.apply(stage)
            self.assertEqual(record['patch_sha256'],apply.sha((stage/'census.patch').read_bytes()))
            self.assertEqual(record,json.loads((stage/'census-manifest.json').read_text()))
            for entry in record['files']:
                raw=(stage/entry['path']).read_bytes()
                self.assertEqual(entry['after_sha256'],apply.sha(raw))
                if entry['path'] in before:
                    self.assertTrue(raw.startswith(before[entry['path']]))
                    self.assertEqual(entry['before_sha256'],apply.sha(before[entry['path']]))
            with self.assertRaisesRegex(ValueError,'already instrumented'):
                apply.apply(stage)

    def test_new_generated_ownership_field_fails_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            stage=Path(temp);self.stage(stage)
            path=stage/'crates/tsr_ast/src/data_generated.rs'
            path.write_text(path.read_text().replace('pub struct TokenData {}','pub struct TokenData {\n    pub hidden: Vec<u8>,\n}'))
            with self.assertRaisesRegex(ValueError,'unclassified generated field'):
                apply.apply(stage)
            self.assertFalse((stage/'census.patch').exists())

    def test_boxed_and_inline_variants_are_exhaustive_generated_observers(self):
        source='enum Data {\n Inline(A),\n Owned(Box<B>),\n}'
        output=apply.enum_walk(source,'Data')
        self.assertIn('Self::Inline(value)',output)
        self.assertIn('Self::Owned(value)',output)
        self.assertNotIn('_ =>',output)
        with self.assertRaisesRegex(ValueError,'unrecognized'):
            apply.enum_walk('enum Data {\n Unknown { hidden: u8 },\n}','Data')


if __name__=='__main__':
    unittest.main()
