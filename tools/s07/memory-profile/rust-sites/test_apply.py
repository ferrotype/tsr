#!/usr/bin/env python3
"""Cheap applier contracts; no compiler or workload execution."""
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("sites_apply", Path(__file__).with_name("apply.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ApplyTests(unittest.TestCase):
    def test_source_inventory_and_slices_are_concrete(self):
        changes, sites = MODULE.patches(MODULE.ROOT)
        self.assertEqual(len(sites), 58)
        self.assertEqual(sum(s["variant"].startswith("Payload") for s in sites), 37)
        nodes = next(s for s in sites if s["variant"] == "NodeSliceBacking")
        self.assertEqual(len(nodes["source_occurrences"]), 2)
        self.assertNotIn("/* SITE_VARIANTS */", changes["crates/tsr_jsstring/src/memory_sites.rs"])
        self.assertNotIn("unsafe ", changes["crates/tsr_jsstring/src/memory_sites.rs"])
        for site in sites:
            self.assertGreater(site["source_line"], 0)
            self.assertEqual(len(site["source_sha256"]), 64)

    def make_stage(self, root):
        changes, _ = MODULE.patches(MODULE.ROOT)
        for relative in changes:
            source = MODULE.ROOT / relative
            if source.is_file():
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
        (root / "census-manifest.json").write_text("{}\n")

    def test_applies_once_and_records_every_mutation(self):
        with tempfile.TemporaryDirectory(prefix="s07-sites-test-") as temp:
            root = Path(temp)
            self.make_stage(root)
            result = MODULE.apply(root)
            self.assertEqual(len(result["sites"]), 58)
            for entry in result["files"]:
                self.assertEqual(MODULE.sha((root / entry["path"]).read_bytes()), entry["after_sha256"])
            with self.assertRaisesRegex(ValueError, "already instrumented"):
                MODULE.apply(root)

    def test_rejects_source_drift_before_writes(self):
        with tempfile.TemporaryDirectory(prefix="s07-sites-test-") as temp:
            root = Path(temp)
            self.make_stage(root)
            target = root / "crates/tsr_arena/src/arena.rs"
            target.write_text(target.read_text().replace("fn push", "fn changed_push"))
            before = {p: p.read_bytes() for p in root.rglob("*") if p.is_file()}
            with self.assertRaisesRegex(ValueError, "source prefix"):
                MODULE.apply(root)
            self.assertEqual(before, {p: p.read_bytes() for p in root.rglob("*") if p.is_file()})

    def test_rejects_production_alias(self):
        with self.assertRaisesRegex(ValueError, "disposable census stage"):
            MODULE.apply(MODULE.ROOT)


if __name__ == "__main__":
    unittest.main()
