"""The reference relater's own source and compiler options bind its evidence."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import s08_relater as relater


class RelaterSourceFingerprint(unittest.TestCase):
    def test_reference_sources_and_nested_build_config_are_digested(self):
        paths = (
            '.cargo/config.toml',
            'tools/s08/relater-prototype/Cargo.toml',
            'tools/s08/relater-prototype/src/lib.rs',
            'tools/s08/relater-prototype/src/bound.rs',
            'tools/s08/relater-prototype/src/bound/generic_source.rs',
            'tools/s08/relater-prototype/src/bound_input.rs',
            'tools/s08/relater-prototype/src/diagnostics.rs',
            'tools/s08/relater-prototype/src/template.rs',
            'crates/tsr_compiler/examples/p7_relater/reference.rs',
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path in paths:
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(path.encode())
            with patch.object(relater, 'ROOT', root):
                before = relater.sources()
                self.assertEqual(set(before), set(paths))
                for path in paths:
                    with self.subTest(path=path):
                        target = root / path
                        original = target.read_bytes()
                        target.write_bytes(original + b'\nchanged')
                        after = relater.sources()
                        self.assertNotEqual(after[path], before[path])
                        self.assertNotEqual(relater.fingerprint(before), relater.fingerprint(after))
                        target.write_bytes(original)
                self.assertEqual(relater.sources(), before)


if __name__ == '__main__':
    unittest.main()
