"""Phase 3 C1: the baseline writers' witness (`scripts/phase3_baselines.py`):
its row contract, buckets, summary and mutants, over the writers' real fixture
rows."""
import copy
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_baselines as witness  # noqa: E402


def matched_row(fixture):
    native = fixture["native"]
    row = {"id": fixture["id"], "state": "executed", "order": {"state": "match"},
           "source_maps": {"state": "reconstructed", "count": 0}, "declaration": fixture["declaration"]}
    for domain in witness.DOMAINS:
        row[domain] = {"outcome": "match", "state": native[domain]["state"]}
    return row


class Fixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = json.loads(witness.FIXTURES.read_bytes())
        cls.fixtures = cls.document["fixtures"]
        cls.native = [dict(fixture["native"], state="executed") for fixture in cls.fixtures]

    def test_the_fixtures_are_the_declared_cases_in_order(self):
        self.assertEqual([(f["kind"], f["id"]) for f in self.fixtures], list(witness.FIXTURE_CASES))
        for fixture in self.fixtures:
            self.assertEqual(set(fixture["native"]), set(witness.NATIVE_FIXTURE_FIELDS), fixture["id"])
            self.assertIn("source_files", fixture["facts"])
            for item in (fixture["native"]["outputs"][group] for group in ("js", "dts", "maps")):
                for output in item:
                    self.assertIn("text_hex", output, "fixtures need a capture taken with --texts")

    def test_library_texts_are_not_carried(self):
        for fixture in self.fixtures:
            for item in fixture["facts"]["source_files"]:
                name = bytes.fromhex(item["file_name_hex"])
                library = name.startswith(b"bundled:///libs/") or name.startswith(b"/.lib/")
                self.assertEqual(item.get("library", False), library, name)
                self.assertEqual("text_hex" in item, not library, name)

    def test_a_witness_line_carries_the_runner_input_groups_in_order(self):
        fixture = self.fixtures[0]
        request = {"configured_name": "2dArrays.ts", "path": "tests/cases/compiler/2dArrays.ts"}
        row = dict(fixture["native"], state="executed")
        inputs = row["baseline_inputs"]
        line = witness.witness_request(request, row, {"files": {}}, "single")
        self.assertEqual(line["error_inputs"], inputs["ts_config_files"] + inputs["to_be_compiled"]
                         + inputs["other_files"])
        self.assertEqual((line["subfolder"], line["mode"]), ("compiler", "single"))
        self.assertNotIn("dump_facts", line)
        self.assertTrue(witness.witness_request(request, row, {}, "single", dump_facts=True)["dump_facts"])
        self.assertEqual(witness.subfolder({"path": "x/tests/cases/conformance/a.ts"}), "conformance")

    def test_selection(self):
        observed = [{"id": "a"}, {"id": "b"}, {"id": "c"}]
        self.assertEqual(witness.select(observed), [0, 1, 2])
        self.assertEqual(witness.select(observed, limit=2), [0, 1])
        self.assertEqual(witness.select(observed, cases=["c", "a"]), [0, 2])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            witness.select(observed, cases=["a", "a"])
        with self.assertRaisesRegex(ValueError, "unknown"):
            witness.select(observed, cases=["z"])

    def test_the_row_contract(self):
        rows = [matched_row(fixture) for fixture in self.fixtures]
        for row in rows:
            witness.validate_row(row)
        broken = copy.deepcopy(rows[0])
        broken["output"]["outcome"] = "unsupported"
        with self.assertRaisesRegex(ValueError, "malformed output"):
            witness.validate_row(broken)
        broken = copy.deepcopy(rows[0])
        broken["sourcemap"] = {"outcome": "failed"}
        with self.assertRaisesRegex(ValueError, "without a reason"):
            witness.validate_row(broken)
        broken = copy.deepcopy(rows[0])
        del broken["order"]
        with self.assertRaisesRegex(ValueError, "order not compared"):
            witness.validate_row(broken)
        with self.assertRaisesRegex(ValueError, "class and reason"):
            witness.validate_row({"id": "x", "state": "failed", "reason": "r"})
        witness.validate_row({"id": "x", "state": "failed", "class": "panic", "reason": "r"})

    def test_buckets(self):
        self.assertIsNone(witness.bucket("output", {"outcome": "match"}))
        self.assertEqual(witness.bucket("output", {"outcome": "different", "native_state": "content",
                                                   "rust_state": "no_content"}),
                         "output: different state content -> no_content")
        self.assertEqual(witness.bucket("sourcemap", {"outcome": "different", "native_state": "content",
                                                      "rust_state": "content", "native_name": "a",
                                                      "rust_name": "b"}),
                         "sourcemap: different baseline name")
        self.assertEqual(witness.bucket("output", {"outcome": "different", "native_state": "content",
                                                   "rust_state": "content", "native_name": "a", "rust_name": "a"}),
                         "output: different text")
        self.assertEqual(witness.bucket("output", {"outcome": "unverifiable", "without_repeat_matches": True}),
                         "output: unverifiable (the composition without the repeat blocks matches)")
        self.assertEqual(witness.bucket("sourcemap_record", {"outcome": "failed", "reason": "runtime: x"}),
                         "sourcemap_record: failed: runtime: x")

    def test_the_summary_counts_outcomes_by_native_state_and_dts_file_errors(self):
        rows = [matched_row(fixture) for fixture in self.fixtures]
        different = rows[1]
        different["output"] = {"outcome": "different", "native_state": "content", "rust_state": "content",
                               "native_name": "n", "rust_name": "n"}
        summary = witness.summarize(rows, self.native)
        self.assertEqual(summary["rows"], len(self.fixtures))
        output = summary["domains"]["output"]
        self.assertEqual(output["match"], {"content": 9, "no_content": 1})
        self.assertEqual(output["different"], {"content": 1})
        self.assertEqual(summary["domains"]["sourcemap"]["match"], {"content": 4, "not_baselined": 7})
        self.assertEqual(summary["buckets"][0]["bucket"], "output: different text")
        self.assertEqual(summary["buckets"][0]["examples"], [self.fixtures[1]["id"]])
        errors = summary["dts_file_errors"]
        self.assertEqual((errors["native"], errors["composed"]), (1, 1))
        self.assertEqual(errors["native_only"], [])
        self.assertEqual(summary["declaration"], {"compiled": 3, "none": 8})


class Mutants(unittest.TestCase):
    def test_each_mutant_anchor_occurs_once(self):
        self.assertGreaterEqual(len(witness.MUTANTS), 3)
        for name, path, before, after in witness.MUTANTS:
            text = (ROOT / path).read_text()
            self.assertEqual(text.count(before), 1, name)
            self.assertNotEqual(before, after, name)

    def test_the_mutation_subset_covers_maps_and_dts_file_errors(self):
        observed = []
        for fixture in json.loads(witness.FIXTURES.read_bytes())["fixtures"]:
            observed.append(dict(fixture["native"], state="executed"))
        chosen = witness.mutation_cases(observed)
        self.assertEqual(chosen, [row["id"] for row in observed])


if __name__ == "__main__":
    unittest.main()
