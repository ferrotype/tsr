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
        self.assertTrue(all(row["owner"] == coverage.OWNERS[row["family"]] for row in inferred))

    def test_compiler_destinations_are_reviewed_without_claiming_coverage(self):
        unresolved = [row for row in self.report["gaps"] if row["root_cause"] == "compiler_destination_unreviewed"]
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        self.assertEqual({row["id"] for row in unresolved}, set(review["unresolved_compiler_destinations"]))
        self.assertEqual(len(unresolved), 23)
        self.assertTrue(all(not row["links"] for row in unresolved))
        row = next(row for row in self.report["operations"] if row["id"] == "tsc/internal/compiler/program.go:Program.ResolveModuleName")
        self.assertEqual(row["roster_state"], "exempt:unused_at_pin")
        self.assertEqual(row["links"], [])

    def test_pilot_missing_is_visible_but_not_a_production_gap_claim(self):
        pilot = [row for row in self.report["case_gaps"] if row["family"] == "pilot"]
        self.assertEqual(sum(row["historical_result"] == "not_implemented" for row in pilot), 4)
        self.assertTrue(all(not row["acceptance"] for row in pilot))
        routes = [row for row in self.report["cases"] if row["family"] == "pilot"]
        self.assertTrue(all(row["producer_metric"] is None for row in routes))

    def test_approvals_classify_without_rewriting_raw_difference(self):
        approved = [row for row in self.report["case_gaps"] if row["approved_difference"]]
        self.assertEqual(len(approved), 5)
        for row in approved:
            self.assertEqual(row["historical_result"], "different")
            self.assertIn(row["recorded_result"], ("different", "not_run"))
            self.assertNotEqual(row["approved_native"], row["approved_rust"])
        self.assertEqual(sum(row["family"] == "filesystem" for row in approved), 3)

    def test_transfer_to_later_preparation_step_does_not_drop_phase1_work(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        expected = {row["id"] for row in document["operations"]
                    if row["roster"].get("state") == "exempt:later_step"
                    and row["roster"].get("step") in ("leaves", "config")}
        actual = {row["id"] for row in self.report["gaps"] if row["root_cause"] == "later_step_unresolved"}
        self.assertEqual(actual, expected)
        self.assertGreaterEqual(len(actual), 89)

    def test_matchfiles_contributes_to_filesystem_and_config_metrics(self):
        rows = [row for row in self.report["cases"] if row["id"].startswith("filesystem/matchfiles/")]
        self.assertEqual(len(rows), 142)
        for row in rows:
            self.assertEqual(set(row["producer_metrics"]), {"run.config.parity", "run.foundations.filesystem_complete"})

    def test_metric_routes_cannot_be_empty(self):
        self.assertTrue(self.report["metric_contributors"])
        cases = copy.deepcopy(self.report["cases"])
        for row in cases:
            row["producer_metrics"] = [metric for metric in row["producer_metrics"] if metric != "run.config.parity"]
        with self.assertRaisesRegex(ValueError, "run.config.parity"):
            coverage.metric_contributors(cases, self.report["witnesses"], self.report["external_inventories"])

    def test_external_program_and_integration_metrics_have_separate_validated_denominators(self):
        rows = {row["id"]: row for row in self.report["external_inventories"]}
        self.assertEqual(rows["full-program-syntax"]["contributor_count"],
                         len(json.loads((ROOT / "data/phase1/syntax-cases.json").read_text())))
        self.assertEqual(rows["integration"]["contributor_count"], len(rows["integration"]["observations"]))
        for metric in ("run.syntax.parity", "run.foundations.integration_complete"):
            self.assertTrue(self.report["metric_contributors"][metric])
        for row in self.report["external_inventories"]:
            changed = copy.deepcopy(self.report["external_inventories"])
            next(item for item in changed if item["id"] == row["id"])["contributor_count"] = 0
            with self.assertRaisesRegex(ValueError, "no contributing"):
                coverage.metric_contributors(self.report["cases"], self.report["witnesses"], changed)

    def test_every_operation_rust_witness_has_executable_producer_route(self):
        manifest = json.loads((ROOT / "data/phase1/cases.json").read_text())
        expected = {row["id"] for row in manifest["witnesses"] if row["kind"] == "rust_gated" and row.get("operations")}
        self.assertEqual({row["id"] for row in self.report["witnesses"]}, expected)
        self.assertTrue(all(row["producer_metrics"] for row in self.report["witnesses"]))

    def test_approval_cannot_follow_a_changed_request(self):
        report = self.mutated("data/phase1/approved-differences.json", lambda d: d["differences"][0].update(request_sha256="0" * 64))
        self.assertTrue(any("approved difference request changed" in p for p in report["problems"]))

    def test_compiler_review_cannot_drop_a_reviewed_identity(self):
        report = self.mutated("data/phase1/coverage-review.json", lambda d: d["reviewed_operation_destinations"].pop())
        self.assertTrue(any("every later-step exemption" in p for p in report["problems"]))

    def test_unknown_and_duplicate_unused_reviews_fail(self):
        for mutate in (
            lambda d: d["reviewed_unused_compiler_operations"].append(d["reviewed_unused_compiler_operations"][0]),
            lambda d: d["reviewed_unused_compiler_operations"][0].update(operation="tsc/internal/compiler/program.go:Invented"),
            lambda d: d["reviewed_unused_compiler_operations"][0].update(go_source_sha256="0" * 64),
        ):
            with self.subTest(mutate=mutate):
                report = self.mutated("data/phase1/coverage-review.json", mutate)
                self.assertFalse(report["healthy"])


class ReviewedDestinationTests(unittest.TestCase):
    def test_reference_loading_is_pending_under_the_accepted_build_boundary(self):
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        decisions = scope.reviewed_destinations()
        unresolved = set(review["unresolved_compiler_destinations"])
        evidence = {row["operation"]: row for row in review["unresolved_compiler_evidence"]}
        self.assertEqual(len(unresolved), 23)
        self.assertEqual(set(evidence), unresolved)
        self.assertFalse(unresolved & decisions.keys())
        for identity in unresolved:
            self.assertTrue(evidence[identity]["reason"])
            self.assertIn("Caller:", evidence[identity]["evidence"])
            self.assertIn("Authority:", evidence[identity]["evidence"])
        self.assertIn("tsc/internal/compiler/fileloader.go:fileLoader.addProjectReferenceTasks", unresolved)
        self.assertIn("tsc/internal/compiler/program.go:Program.verifyProjectReferences", unresolved)
        self.assertEqual(decisions["tsc/internal/compiler/projectreferencedtsfakinghost.go:newProjectReferenceDtsFakingHost"]["destination_phase"], 5)
        self.assertEqual(decisions["tsc/internal/compiler/program.go:Program.GetParseFileRedirect"]["destination_phase"], 2)

    def test_unused_reference_helper_retains_its_closed_wrapper_chain(self):
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        roster = json.loads((ROOT / "data/phase1/syntax-roster.json").read_text())
        helper = "tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getResolvedReferenceFor"
        wrapper = "tsc/internal/compiler/program.go:Program.GetResolvedProjectReferenceFor"
        unused = {row["operation"]: row for row in review["reviewed_unused_compiler_operations"]}
        exemptions = {row["operation"]: row for row in roster["exemptions"]}
        self.assertIn("only call", unused[helper]["evidence"])
        self.assertIn("GetResolvedProjectReferenceFor", unused[helper]["evidence"])
        self.assertEqual(exemptions[helper]["category"], "unused_at_pin")
        self.assertEqual(exemptions[wrapper]["category"], "unused_at_pin")
        self.assertNotIn(helper, scope.reviewed_destinations())

    def test_unmapped_is_derived_without_the_status_view(self):
        expected = scope.unmapped_ids()
        self.assertNotIn("tsc/internal/nativepath/eintr_unix.go:ignoringEINTR", expected)
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
