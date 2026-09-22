"""Complete, child-free Phase A join and conservative destination contracts."""
import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_coverage as coverage
import phase1_scope as scope


class CoverageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.report = coverage.build()

    def mutated(self, relative, mutate):
        original = Path.read_bytes
        target = ROOT / relative
        value = json.loads(original(target))
        mutate(value)
        replacement = json.dumps(value).encode()
        with patch.object(Path, "read_bytes", lambda path: replacement if path == target else original(path)):
            return coverage.build()

    def test_real_join_is_healthy_but_does_not_close_unwitnessed_operations(self):
        self.assertTrue(self.report["healthy"], self.report["problems"])
        self.assertFalse(self.report["preparation_complete"])
        self.assertGreater(self.report["counts"]["pending_operations"], 0)
        self.assertEqual(self.report["counts"]["config_outputs"], 309)
        self.assertEqual(self.report["counts"]["operations"], 4795)

    def test_no_subprocess_is_started(self):
        with patch("subprocess.run", side_effect=AssertionError("child")), patch("subprocess.Popen", side_effect=AssertionError("child")):
            self.assertEqual(coverage.build(), self.report)

    def test_removed_case_and_duplicate_case_fail(self):
        for mutate in (lambda d: d["cases"].pop(), lambda d: d["cases"].append(d["cases"][0])):
            with self.subTest(mutate=mutate):
                report = self.mutated("data/phase1/cases.json", mutate)
                self.assertFalse(report["healthy"])
                self.assertTrue(any("case" in problem for problem in report["problems"]))

    def test_removed_operation_cannot_shrink_denominator(self):
        report = self.mutated("data/phase1/scope.json", lambda d: d["operations"].pop())
        self.assertFalse(report["healthy"])
        self.assertIn("scope operation identity set differs from the complete pinned inventory", report["problems"])

    def test_unknown_operation_and_forged_case_link_fail(self):
        def mutate(document):
            document["cases"][0]["operations"].append("tsc/internal/core/core.go:Invented")
        self.assertTrue(any("orphan operation" in p for p in self.mutated("data/phase1/cases.json", mutate)["problems"]))
        report = self.mutated("data/phase1/scope.json", lambda d: d["operations"][0]["cases"].append("invented-case"))
        self.assertTrue(any("does not link" in p for p in report["problems"]))

    def test_duplicate_baseline_ownership_and_removed_output_fail(self):
        def mutate(document):
            cases = [row for row in document["cases"] if row.get("baseline")]
            cases[1]["baseline"] = cases[0]["baseline"]
        report = self.mutated("data/phase1/cases.json", mutate)
        self.assertTrue(any("duplicate case ownership" in p for p in report["problems"]))
        self.assertTrue(any("309 frozen paths" in p for p in report["problems"]))

    def test_partial_report_and_changed_request_fail_replay(self):
        partial = copy.deepcopy(self.report)
        partial["cases"].pop()
        self.assertIn("coverage report differs from current complete inputs", coverage.verify(partial))
        changed = self.mutated("data/phase1/requests/syntax-debug.json", lambda d: d["requests"][0].update(discriminates="changed"))
        self.assertNotEqual(changed, self.report)
        self.assertNotEqual(changed["input_sha256"], self.report["input_sha256"])

    def test_arbitrary_documentation_is_not_a_capture_input(self):
        self.assertFalse(any(path.startswith("docs/") for path in self.report["input_sha256"]))
        original = Path.read_bytes
        with patch.object(Path, "read_bytes", lambda path: b"different" if str(path).endswith("docs/unrelated.md") else original(path)):
            self.assertEqual(coverage.build(), self.report)

    def test_name_inference_does_not_claim_confirmed_absence(self):
        inferred = [row for row in self.report["gaps"] if row["root_cause"] == "implementation_unverified"]
        self.assertGreater(len(inferred), 0)
        self.assertTrue(all(row["owner"] in ("F2b", "F4b") for row in inferred))

    def test_ambiguous_compiler_destinations_stay_in_scope(self):
        unresolved = [row for row in self.report["gaps"] if row["root_cause"] == "compiler_destination_unreviewed"]
        self.assertEqual(len(unresolved), 44)
        self.assertTrue(all(row["destination_phase"] == 1 for row in unresolved))
        self.assertTrue(all(row["owner"] == "F5a" for row in unresolved))

    def test_pilot_missing_is_visible_but_not_a_production_gap_claim(self):
        pilot = [row for row in self.report["case_gaps"] if row["family"] == "pilot"]
        self.assertEqual(len(pilot), 4)
        self.assertTrue(all(not row["acceptance"] for row in pilot))
        routes = [row for row in self.report["cases"] if row["family"] == "pilot"]
        self.assertTrue(all(row["producer_metric"] is None for row in routes))

    def test_approvals_classify_without_rewriting_raw_difference(self):
        approved = [row for row in self.report["case_gaps"] if row["approved_difference"]]
        self.assertEqual(len(approved), 3)
        for row in approved:
            self.assertEqual(row["recorded_result"], "different")
            self.assertNotEqual(row["approved_native"], row["approved_rust"])
        self.assertEqual(self.report["families"]["filesystem"]["different"], 3)

    def test_approval_cannot_follow_a_changed_request(self):
        report = self.mutated("data/phase1/approved-differences.json", lambda d: d["differences"][0].update(request_sha256="0" * 64))
        self.assertTrue(any("approved difference request changed" in p for p in report["problems"]))

    def test_compiler_review_cannot_drop_an_unresolved_identity(self):
        report = self.mutated("data/phase1/coverage-review.json", lambda d: d["unresolved_compiler_destinations"].pop())
        self.assertTrue(any("every later-step exemption" in p for p in report["problems"]))


class ReviewedDestinationTests(unittest.TestCase):
    def test_unmapped_is_derived_without_the_status_view(self):
        expected = {operation for rows in scope.unmapped().values() for operation in rows}
        original = Path.read_text
        target = ROOT / "status/unmapped-functions.json"
        def read(path, *args, **kwargs):
            if path == target:
                raise AssertionError("generated status read")
            return original(path, *args, **kwargs)
        with patch.object(Path, "read_text", read):
            self.assertEqual(scope.unmapped_ids(), expected)

    def test_explicit_later_phase_accepts_only_real_destinations(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        row = next(row for row in document["operations"] if row["basis_kind"] == "review" and row["disposition"] == "later_phase")
        self.assertEqual(scope.verify(document), [])
        for destination in (None, 1, True, "2", 8):
            with self.subTest(destination=destination):
                row["destination_phase"] = destination
                self.assertTrue(any("destination outside phase 1" in p for p in scope.verify(document)))

    def test_forged_review_cannot_mint_an_operation(self):
        original = Path.read_text
        target = ROOT / "data/phase1/coverage-review.json"
        review = json.loads(original(target))
        review["reviewed_operation_destinations"][0]["operation"] = "tsc/internal/compiler/program.go:Invented"
        with patch.object(Path, "read_text", lambda path, *a, **kw: json.dumps(review) if path == target else original(path, *a, **kw)):
            with self.assertRaisesRegex(ValueError, "unknown operation"):
                scope.reviewed_destinations()

    def test_review_rejects_changed_pinned_source(self):
        original = Path.read_bytes
        target = ROOT / "upstream/tsc/internal/compiler/checkerpool.go"
        with patch.object(Path, "read_bytes", lambda path: b"changed" if path == target else original(path)):
            with self.assertRaisesRegex(ValueError, "reviewed pinned source changed"):
                scope.reviewed_destinations()


if __name__ == "__main__":
    unittest.main()
