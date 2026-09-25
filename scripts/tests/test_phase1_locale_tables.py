"""Generated locale data is authenticated independently of leaf success."""
import hashlib
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import generate_locale_tables as generator


class LocaleTableTests(unittest.TestCase):
    def test_generated_outputs_and_local_inputs_match_the_manifest(self):
        manifest = json.loads((ROOT / "data/phase1/locale-tables-manifest.json").read_text())
        self.assertEqual(manifest["pin"], json.loads((ROOT / "data/upstream.json").read_text())["pin"])
        for path, expected in (manifest["inputs"] | manifest["outputs"]).items():
            if path.startswith("golang.org/"):
                continue  # Exact module source hashes are checked by the exporter.
            with self.subTest(path=path):
                self.assertEqual(hashlib.sha256((ROOT / path).read_bytes()).hexdigest(), expected)

    def test_manifest_inputs_cover_the_generator_and_its_module_imports(self):
        """Every script the generator loads at import time is a manifest input (s05_tables was not)."""
        import ast

        manifest = json.loads((ROOT / "data/phase1/locale-tables-manifest.json").read_text())
        pending, loaded = ["scripts/generate_locale_tables.py"], set()
        while pending:
            relative = pending.pop()
            if relative in loaded:
                continue
            loaded.add(relative)
            for node in ast.parse((ROOT / relative).read_text()).body:  # module level only
                names = ([alias.name for alias in node.names] if isinstance(node, ast.Import)
                         else [node.module] if isinstance(node, ast.ImportFrom) and node.module else [])
                pending += [f"scripts/{name}.py" for name in names if (ROOT / f"scripts/{name}.py").is_file()]
        self.assertIn("scripts/s05_tables.py", loaded)
        self.assertLessEqual(loaded | {"scripts/tracking-bootstrap.py"}, set(manifest["inputs"]))

    def test_a_new_matcher_continuation_requires_an_implementation(self):
        with self.assertRaisesRegex(ValueError, "matcher continuation changed"):
            generator.render({}, {"candidates": {"1": [[0] * 8 + [1]]}})
