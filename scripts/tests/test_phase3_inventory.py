"""Phase 3 T0: the emit inventory rebuilds byte for byte and states the plan's counts."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_inventory as inventory  # noqa: E402


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = inventory.build()
        cls.rows = cls.document["rows"]

    def test_committed_inventory_is_the_rebuild(self):
        self.assertEqual(inventory.INVENTORY.read_bytes(), inventory.render(self.document))
        self.assertEqual(inventory.read()["counts"], self.document["counts"])

    def test_plan_counts(self):
        counts = self.document["counts"]
        self.assertEqual(counts["executed"], 13432)
        self.assertEqual(counts["references"], {".js": 12174, ".js.map": 150, ".sourcemap.txt": 157})
        self.assertEqual(counts["without_js_reference"], 1258)
        # The plan's "42 others" are the 8 runner-skipped rows and these 34.
        self.assertEqual(counts["output"], {"disabled": 69, "no_content": 1189, "reference": 12174})
        self.assertEqual(counts["no_content_class"], {"noEmit": 1155, "other": 34})
        self.assertEqual(sum(counts["output_disabled"].values()), 69)
        self.assertEqual(counts["output_disabled"][inventory.DECLARATION_ONLY], 61)
        self.assertEqual(counts["sample"], 300)
        self.assertEqual((counts["transpile_cases"], counts["transpile_references"]), (25, 41))

    def test_skipped_emit_tests_are_the_pins(self):
        skipped = self.document["skipped_emit_tests"]
        self.assertEqual(len(skipped), 8)
        self.assertEqual(skipped["jsFileCompilationWithoutJsExtensions.ts"], "No files are emitted.")
        rows = [row for row in self.rows if Path(row["path"]).name in skipped]
        self.assertEqual(len(rows), 8)
        for row in rows:
            self.assertEqual(row["output"], {"state": "disabled", "reason": skipped[Path(row["path"]).name]})

    def test_skipped_table_parser_rejects_an_unrecognised_entry(self):
        source = 'var skippedEmitTests = map[string]string{\n\t"a.ts": "reason",\n\tname: "x",\n}\n'
        with self.assertRaisesRegex(ValueError, "unrecognised"):
            inventory.skipped_emit_tests(source)
        with self.assertRaisesRegex(ValueError, "empty"):
            inventory.skipped_emit_tests("var skippedEmitTests = map[string]string{\n}\n")

    def test_a_disabled_output_never_has_a_reference(self):
        for row in self.rows:
            output = row["output"]
            self.assertEqual(output["state"] == "runs" and output["owes"] == "reference", ".js" in row["references"], row["id"])
            self.assertEqual(row["sourcemap"] == "reference", ".js.map" in row["references"])
            self.assertEqual(row["sourcemap_record"] == "reference", ".sourcemap.txt" in row["references"])
            if not row["has_non_dts_files"]:
                self.assertEqual(output["reason"], inventory.DECLARATION_ONLY)

    def test_requests_are_the_s08_shape_in_inventory_order(self):
        requests = inventory.requests(self.document)
        self.assertEqual([row["id"] for row in requests], [row["id"] for row in self.rows])
        self.assertEqual(set(requests[0]), {*inventory.REQUEST_FIELDS, "acceptance_tier"})
        self.assertEqual({row["acceptance_tier"] for row in requests}, {"executed"})

    def test_rows_are_bound_to_the_phase2_rows_not_the_file(self):
        """Re-freezing the Phase 2 inventory for an input digest alone keeps this one current."""
        phase2 = json.loads((ROOT / inventory.PHASE2).read_bytes())
        executed = [row for row in phase2["rows"] if row["tier"] == "executed"]
        self.assertEqual(self.document["inputs"]["phase2_executed_rows_sha256"],
                         inventory.digest(inventory.encode(executed).encode()))
        self.assertNotIn(inventory.PHASE2, self.document["inputs"])

    def test_transpile_references_are_git_blobs(self):
        for entry in self.document["transpile"]["references"]:
            path = ROOT / "upstream/tsc/testdata/baselines/reference" / entry["name"]
            self.assertEqual(entry["git_blob"], inventory.git_blob(path.read_bytes()))


if __name__ == "__main__":
    unittest.main()
