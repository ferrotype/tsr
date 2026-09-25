"""Phase 2 C0.1: the frozen inventory rebuilds byte for byte and states the plan's counts."""
import copy
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_inventory as inventory


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = inventory.build()
        cls.rendered = inventory.render(cls.document)
        cls.rows = cls.document["rows"]
        cls.executed = [row for row in cls.rows if row["tier"] == "executed"]

    def test_committed_inventory_is_the_rebuild(self):
        self.assertEqual(inventory.INVENTORY.read_bytes(), self.rendered)
        self.assertEqual(inventory.read()["counts"], self.document["counts"])

    def test_inputs_are_bound_by_digest(self):
        self.assertEqual(set(self.document["inputs"]), set(inventory.INPUTS))
        for name, value in self.document["inputs"].items():
            self.assertEqual(value, inventory.digest((ROOT / name).read_bytes()))

    def test_section_three_counts(self):
        counts = self.document["counts"]
        self.assertEqual(counts["variants"], 15206)
        self.assertEqual(counts["executed"], 13434)
        self.assertEqual(counts["executed_by_s08_tier"], {"acceptance": 9369, "excluded": 4065})
        self.assertEqual(counts["executed_newly_included"], 4050)
        self.assertEqual(counts["executed_content_mapper"], 15)
        self.assertEqual(counts["informational"], {"filename_skip": 52, "option_guard_skip": 1720})
        self.assertEqual(counts["informational_options_rejected"], 39)
        self.assertEqual(counts["references"],
                         {".errors.txt": 7301, ".types": 12753, ".symbols": 12753, ".trace.json": 148})
        self.assertEqual(counts["without_checked_reference"], 464)
        self.assertEqual(counts["emitted_only"], 413)
        self.assertEqual(counts["types_disabled"], 679)
        # The pinned GetEmitDeclarations is declaration || composite. The plan's
        # 1,756 also counted emitDeclarationOnly keys; C0.2 records the native requests.
        self.assertEqual(counts["emit_declarations_by_options"], 1754)
        self.assertEqual(counts["checkpoint"], {"regression": 9369, "C2": 2954, "C3": 185, "C4": 926})

    def test_informational_rows_never_carry_an_owner_or_sample(self):
        for row in self.rows:
            if row["tier"] == "informational":
                self.assertIsNone(row["checkpoint"])
                self.assertFalse(row["sample"])
                self.assertEqual(row["informational_reason"], row["native_selection"])
                self.assertEqual(row["references"], {}, row["id"])
            else:
                self.assertEqual(row["native_selection"], "runs")
                self.assertNotEqual(row["boundary"], "options_rejected")

    def test_checkpoint_follows_the_latest_family(self):
        for row in self.executed:
            families = set(row["families"])
            if row["s08_tier"] == "acceptance":
                self.assertEqual(row["checkpoint"], "regression")
                self.assertEqual(families, set())
            elif families & {"jsx", "decorators"}:
                self.assertEqual(row["checkpoint"], "C4")
            elif families & set(inventory.TYPE_FAMILIES):
                self.assertEqual(row["checkpoint"], "C2")
            else:
                self.assertEqual(row["checkpoint"], "C3")
                self.assertTrue(row["emitted_only"] or row["content_mapper"], row["id"])

    def test_emitted_only_rows_still_assert_zero_diagnostics(self):
        for row in self.executed:
            if row["emitted_only"]:
                self.assertTrue(row["harness"]["NoTypesAndSymbols"])
                self.assertNotIn(".errors.txt", row["references"])

    def test_sample_is_recorded_and_covers_every_owner_and_family(self):
        sample = [row for row in self.rows if row["sample"]]
        self.assertEqual(len(sample), inventory.SAMPLE_SIZE)
        self.assertEqual({row["checkpoint"] for row in sample}, {"regression", "C2", "C3", "C4"})
        families = {family for row in self.executed for family in row["families"]}
        self.assertEqual({family for row in sample for family in row["families"]}, families)
        self.assertEqual(self.document["counts"]["sample_by_checkpoint"], inventory.SAMPLE_TARGETS)
        again = inventory.select_sample(copy.deepcopy(self.rows))
        self.assertEqual(again, {row["id"] for row in sample})

    def test_input_disagreement_is_rejected(self):
        subset = json.loads((ROOT / "data/s07/subset.json").read_bytes())
        schedule = json.loads((ROOT / "data/phase1/syntax-schedule.json").read_bytes())
        acceptance = json.loads((ROOT / "data/s07/e2-acceptance.json").read_bytes())
        changed = copy.deepcopy(schedule)
        changed["rows"][0]["loading_request_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "different request"):
            inventory.build_rows(subset, changed, acceptance)
        changed = copy.deepcopy(schedule)
        changed["rows"].pop()
        with self.assertRaisesRegex(ValueError, "missing from the syntax schedule|same variants"):
            inventory.build_rows(subset, changed, acceptance)
        changed = copy.deepcopy(acceptance)
        changed["variants"].pop(0)
        with self.assertRaisesRegex(ValueError, "S08 partition"):
            inventory.build_rows(subset, schedule, changed)


if __name__ == "__main__":
    unittest.main()
