"""Phase 4 X0: the recorded scenario inventory is well formed, current and complete."""
import copy
import gzip
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_scenarios as scenarios  # noqa: E402

ORPHAN = "tsc/commandLine/adds-color-when-FORCE_COLOR-is-set.js"
CHTIMES = "tsbuild/sample/when-input-file-text-does-not-change-but-its-modified-time-changes.js"


def redigest(document):
    document["provenance"]["scenario_digests"] = {
        scenario["id"]: scenarios.digest(scenarios.canonical(scenario)) for scenario in document["scenarios"]}
    document["provenance"]["scenario_count"] = len(document["scenarios"])
    return document


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.data = scenarios.INVENTORY.read_bytes()
        cls.document = scenarios.read_inventory(cls.data)

    def check(self, document):
        return scenarios.check_document(document)

    def assertRejected(self, document, message):
        with self.assertRaisesRegex(ValueError, message):
            self.check(document)

    def scenario(self, document, sid):
        return next(scenario for scenario in document["scenarios"] if scenario["id"] == sid)

    def test_committed_inventory_passes_check(self):
        counts = self.check(copy.deepcopy(self.document))
        self.assertEqual(counts["scenarios"], 516)
        self.assertEqual(counts["families"], {"tsc": 217, "tsbuild": 192, "tscWatch": 42, "tsbuildWatch": 65})
        self.assertEqual(counts["edits"], 1255)

    def test_container_is_deterministic(self):
        self.assertEqual(scenarios.render(self.document), self.data)
        payload = gzip.decompress(self.data)
        with self.assertRaisesRegex(ValueError, "mtime 0"):
            scenarios.read_inventory(gzip.compress(payload, mtime=1))
        pretty = json.dumps(self.document, indent=1, sort_keys=True).encode() + b"\n"
        with self.assertRaisesRegex(ValueError, "canonical"):
            scenarios.read_inventory(gzip.compress(pretty, mtime=0))

    def test_recorded_facts(self):
        document = self.document
        self.assertEqual(document["orphan_references"], [ORPHAN])
        edits = [edit for scenario in document["scenarios"] for edit in scenario["edits"]]
        operations = [op for edit in edits for op in edit["operations"]]
        self.assertEqual(sum(1 for scenario in document["scenarios"] if scenario["edits"]), 292)
        self.assertEqual({kind: sum(op["op"] == kind for op in operations) for kind in ("write", "remove", "chtimes")},
                         {"write": 601, "remove": 52, "chtimes": 1})
        self.assertEqual([op["op"] for op in self.scenario(document, CHTIMES)["edits"][0]["operations"]], ["chtimes"])
        self.assertEqual(sum("shadow_operations" in edit for edit in edits), 3)
        self.assertEqual(sum(bool(edit["expected_diff"]) for edit in edits), 7)
        built = [scenario["id"] for scenario in document["scenarios"] if scenario["files_from_build"]]
        self.assertEqual(built, ["tsbuildWatch/moduleResolution/build-mode-watches-package-json-lookups-from-existing-buildinfo.js"])

    def test_tampered_scenario_fails(self):
        document = copy.deepcopy(self.document)
        files = self.scenario(document, CHTIMES)["files"]
        path = sorted(path for path, entry in files.items() if "text" in entry)[0]
        files[path]["text"] += " "
        self.assertRejected(document, "scenario digests")

    def test_dropped_operation_fails(self):
        document = copy.deepcopy(self.document)
        self.scenario(document, CHTIMES)["edits"][0]["operations"].clear()
        self.assertRejected(document, "scenario digests")

    def test_wrong_count_fails(self):
        document = copy.deepcopy(self.document)
        document["scenarios"].pop()
        self.assertRejected(redigest(document), "516")
        document = copy.deepcopy(self.document)
        document["scenarios"].append(dict(copy.deepcopy(document["scenarios"][-1]), id="tsc/zz/zz.js",
                                          scenario="zz", file="zz.js", sub_scenario="zz", family="tsc"))
        document["scenarios"].sort(key=lambda scenario: scenario["id"])
        self.assertRejected(redigest(document), "517 scenarios")

    def test_stale_patch_digest_fails(self):
        with tempfile.TemporaryDirectory() as scratch:
            copied = Path(scratch) / "recorder"
            shutil.copytree(scenarios.RECORDER, copied)
            with (copied / "sys.go.diff").open("a") as patch:
                patch.write(" \n")
            stale = scenarios.source_digests(copied)
        with self.assertRaisesRegex(ValueError, "stale patch digests"):
            scenarios.check_document(copy.deepcopy(self.document), current_sources=stale)
        document = copy.deepcopy(self.document)
        document["provenance"]["sources"]["recorder.go"] = "0" * 64
        self.assertRejected(document, "stale patch digests")

    def test_other_pin_or_ledger_fails(self):
        with self.assertRaisesRegex(ValueError, "another pin"):
            scenarios.check_document(copy.deepcopy(self.document), current_pin="0" * 40)
        ledger = dict(scenarios.ledger_hashes(), **{"tsc/internal/execute/tsctests/runner.go": "1" * 64})
        with self.assertRaisesRegex(ValueError, "ledger hashes"):
            scenarios.check_document(copy.deepcopy(self.document), current_ledger=ledger)

    def test_format_rules(self):
        def mutated(change):
            document = copy.deepcopy(self.document)
            change(self.scenario(document, CHTIMES))
            return redigest(document)

        def same_shadow(scenario):
            scenario["edits"][0]["shadow_operations"] = copy.deepcopy(scenario["edits"][0]["operations"])

        def hex_text(scenario):
            entry = next(entry for entry in scenario["files"].values() if "text" in entry)
            entry["text_hex"] = entry.pop("text").encode().hex()

        def unknown_op(scenario):
            scenario["edits"][0]["operations"][0]["op"] = "touch"

        def write_without_clock(scenario):
            scenario["edits"][0]["operations"] = [{"op": "write", "path": "/a.ts", "text": "", "clock_readings": 0}]

        def extra_key(scenario):
            scenario["extra"] = True

        self.assertRejected(mutated(same_shadow), "only when it differs")
        self.assertRejected(mutated(hex_text), "stored as text")
        self.assertRejected(mutated(unknown_op), "unknown operation")
        self.assertRejected(mutated(write_without_clock), "at least one clock reading")
        self.assertRejected(mutated(extra_key), "documented keys")


class PatchTests(unittest.TestCase):
    def test_patches_name_the_ledger_hashes(self):
        ledger = scenarios.ledger_hashes()
        for name in scenarios.PATCHED:
            path, base, hunks = scenarios.parse_patch((scenarios.RECORDER / f"{name}.diff").read_text())
            self.assertEqual(path, f"{scenarios.PACKAGE}/{name}")
            self.assertEqual(base, ledger[path])
            self.assertTrue(hunks)

    @unittest.skipUnless((ROOT / "upstream/tsc/internal/execute/tsctests/sys.go").is_file(), "upstream is not checked out")
    def test_patches_apply_to_the_pinned_files(self):
        for name in scenarios.PATCHED:
            pinned = (ROOT / "upstream" / scenarios.PACKAGE / name).read_text()
            patched = scenarios.apply_patch(pinned, (scenarios.RECORDER / f"{name}.diff").read_text())
            self.assertIn("phase4", patched)
            # The patches only add hook lines (sys.go's Now keeps its call in a local).
            removed = set(pinned.splitlines()) - set(patched.splitlines())
            self.assertLessEqual(removed, {"\treturn s.clock.Now()"})

    def test_apply_patch_is_strict(self):
        patch = "--- f.go\tsha256:" + "0" * 64 + "\n+++ f.go\tx\n@@ -1,2 +1,3 @@\n a\n+b\n c\n"
        self.assertEqual(scenarios.apply_patch("a\nc\n", patch), "a\nb\nc\n")
        with self.assertRaisesRegex(ValueError, "does not apply"):
            scenarios.apply_patch("a\nd\n", patch)
        with self.assertRaisesRegex(ValueError, "line counts"):
            scenarios.apply_patch("a\nc\n", patch.replace("+1,3", "+1,4"))


class NativeTreeTests(unittest.TestCase):
    def test_native_runs_discard_modified_and_added_cached_sources(self):
        # The export marker still names the right pin when an unpatched
        # compiler file or an extra Go test in the cached tree was changed.
        source = "tsc/internal/compiler/program.go"
        reference = scenarios.REFERENCES + "/tsc/example.js"
        contents = {source: b"pinned compiler\n", reference: b"pinned reference\n"}
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as stream:
            for name, content in contents.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                stream.addfile(entry, io.BytesIO(content))
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            tree = root / "tree"
            with mock.patch.object(scenarios, "TREE", tree), \
                    mock.patch.object(scenarios, "pin", return_value="a" * 40), \
                    mock.patch.object(scenarios, "command", return_value=archive.getvalue()):
                scenarios.prepare_tree(root)
                (tree / source).write_bytes(b"modified compiler\n")
                (tree / reference).write_bytes(b"modified reference\n")
                extra = tree / "tsc/internal/compiler/extra_test.go"
                extra.write_bytes(b"unrecorded test\n")
                scenarios.prepare_tree(root)
                for name, content in contents.items():
                    self.assertEqual((tree / name).read_bytes(), content)
                self.assertFalse(extra.exists())


if __name__ == "__main__":
    unittest.main()
