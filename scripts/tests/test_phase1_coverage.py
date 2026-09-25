"""Complete, child-free Phase A join and conservative destination contracts."""
from collections import Counter
import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_capture as capture
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

    def test_documentation_is_not_a_capture_input(self):
        """Real prose, including the decisions the review and approvals cite, never enters the join."""
        self.assertFalse(any(path.endswith(".md") for path in self.report["input_sha256"]))
        prose = [ROOT / "docs/PHASE1-F5b-destinations.md", ROOT / "docs/PHASE1-aliasing-audit.md",
                 ROOT / "docs/PHASE1-progress.md", ROOT / "crates/tsr_jsnum/SLICE.md"]
        self.assertTrue(all(path.is_file() for path in prose))
        original_bytes, original_text = Path.read_bytes, Path.read_text
        with patch.object(Path, "read_bytes", lambda path: b"edited prose" if path in prose else original_bytes(path)), \
             patch.object(Path, "read_text", lambda path, *a, **k: "edited prose" if path in prose
                          else original_text(path, *a, **k)):
            self.assertEqual(coverage.build(), self.report)

    def test_name_inference_does_not_claim_confirmed_absence(self):
        inferred = [row for row in self.report["gaps"] if row["root_cause"] == "implementation_unverified"]
        self.assertGreater(len(inferred), 0)
        self.assertTrue(all(row["owner"] == coverage.OWNERS[row["family"]] for row in inferred))

    def test_compiler_destinations_are_reviewed_without_claiming_coverage(self):
        unresolved = [row for row in self.report["gaps"] if row["root_cause"] == "compiler_destination_unreviewed"]
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        self.assertEqual({row["id"] for row in unresolved}, set(review["unresolved_compiler_destinations"]))
        self.assertEqual(unresolved, [])
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

    def test_transfer_to_later_preparation_step_is_a_gap_until_answered(self):
        """Report-level agreement; LaterStepResolutionTests prove each resolution route."""
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        unresolved = {row["id"] for row in self.report["gaps"] if row["root_cause"] == "later_step_unresolved"}
        self.assertTrue(unresolved)
        for step in ("leaves", "config"):
            pending = {row["operation"] for row in scope.leaf_preparation(document, json.loads(
                (ROOT / "data/phase1/cases.json").read_text()), step)["pending"]
                if row.get("reason") == "later_step_unresolved"}
            with self.subTest(step=step):
                self.assertEqual(pending, {row["id"] for row in self.report["gaps"]
                                           if row["root_cause"] == "later_step_unresolved" and row["family"] == step})

    def test_every_root_cause_is_counted_with_a_native_and_rust_example(self):
        expected = Counter(row["root_cause"] for row in self.report["gaps"] + self.report["case_gaps"])
        causes = {row["cause"]: row for row in self.report["root_causes"]}
        self.assertEqual({cause: row["count"] for cause, row in causes.items()}, dict(expected))
        self.assertTrue(any(row["level"] == "case" for row in causes.values()))
        for cause, row in causes.items():
            with self.subTest(cause=cause):
                self.assertEqual(row["example"]["root_cause"], cause)
                self.assertEqual(row["level"], "operation" if cause in {gap["root_cause"] for gap in self.report["gaps"]}
                                 else "case")
                self.assertTrue(coverage.observed_pair(row["example"]) or row.get("example_reason"))
                if any(coverage.observed_pair(item) for item in self.report["gaps"] + self.report["case_gaps"]
                       if item["root_cause"] == cause):
                    self.assertTrue(coverage.observed_pair(row["example"]), "a two-sided example existed")
        one_sided = [{"root_cause": "native_platform_unavailable", "native": [], "rust": {"driver": "d"},
                      "recorded_result": "native_unavailable", "host_note": "linux only"}]
        entry = coverage.root_cause_entry("native_platform_unavailable", "case", one_sided)
        self.assertIn("linux only", entry["example_reason"])
        for cause, row in causes.items():
            if row["level"] == "case" and "example_reason" not in row:
                with self.subTest(pair=cause):
                    example = row["example"]
                    self.assertTrue(example["native"])
                    self.assertTrue(example["recorded_result"] in coverage.RUST_OBSERVED_RESULTS
                                    or example.get("approved_rust"), "a case pair needs a Rust observation")

    def test_a_case_whose_rust_side_produced_no_observation_is_not_a_pair(self):
        # The case row's `rust` always names the driver and recorded result;
        # only a compared (or approved) Rust observation makes the row two-sided.
        native = [{"host": "darwin", "observation": {"value": 1}}]
        for result in ("harness_failed", "not_implemented", "not_run"):
            with self.subTest(result=result):
                row = {"id": "c", "root_cause": "harness_failure", "native": native,
                       "rust": {"driver": "d", "recorded_result": result}, "recorded_result": result}
                self.assertFalse(coverage.observed_pair(row))
                entry = coverage.root_cause_entry("harness_failure", "case", [row])
                self.assertIn("no Rust observation", entry["example_reason"])
                self.assertNotIn("no native", entry["example_reason"])
        different = {"id": "d", "root_cause": "observation_difference", "native": native,
                     "rust": {"driver": "d", "recorded_result": "different"}, "recorded_result": "different"}
        self.assertTrue(coverage.observed_pair(different))
        failed = {**different, "id": "f", "recorded_result": "harness_failed",
                  "rust": {"driver": "d", "recorded_result": "harness_failed"}}
        entry = coverage.root_cause_entry("observation_difference", "case", [failed, different])
        self.assertEqual(entry["example"]["id"], "d")
        self.assertNotIn("example_reason", entry)
        approved = {**failed, "id": "a", "approved_rust": {"value": 2}}
        self.assertTrue(coverage.observed_pair(approved))

    def test_operation_gaps_reproduce_through_their_family(self):
        families = {row["id"]: row["family"] for row in self.report["cases"]}
        linked = 0
        for gap in self.report["gaps"]:
            cases = sorted(identity for identity in gap["links"] if identity in families)
            with self.subTest(operation=gap["id"]):
                if cases:
                    linked += 1
                    family = families[cases[0]]
                    self.assertEqual(gap["reproduce"], gap["reproduce_by_family"][family])
                    self.assertTrue(gap["reproduce"].startswith(f"python3 scripts/phase1.py capture --family {family} "))
                    self.assertIn(f"--case {cases[0]}", gap["reproduce"])
                else:
                    self.assertEqual(gap["reproduce"], gap["explain"])
                if gap["family"] in capture.FAMILIES:
                    self.assertTrue(gap["family_capture"].startswith(f"python3 scripts/phase1.py capture --family {gap['family']} "))
        self.assertGreater(linked, 0, "no linked operation gap exercised the family route")

    def test_integration_cases_route_to_the_metric_the_evaluator_consumes(self):
        import phase1_integration as integration
        routed = {row["id"] for row in self.report["cases"] if coverage.INTEGRATION_METRIC in row["producer_metrics"]}
        prepared = integration.check()
        rows = [{"case": row["id"], "result": "match"} for row in self.report["cases"]]
        result = integration.evaluate(prepared, [{"rows": rows}], source_inputs={})
        consumed = {identity for row in result["witnesses"] for identity in row.get("observations", {})}
        self.assertTrue(consumed)
        self.assertEqual(consumed, routed)
        self.assertLessEqual(routed, set(self.report["metric_contributors"][coverage.INTEGRATION_METRIC]))
        # Consumption is real, not a listing: one routed case that differs moves its witness.
        changed = sorted(routed)[0]
        result = integration.evaluate(prepared, [{"rows": [{**row, "result": "different"} if row["case"] == changed
                                                           else row for row in rows]}], source_inputs={})
        self.assertTrue(any(row.get("observations", {}).get(changed) == "different" and row["state"] == "different"
                            for row in result["witnesses"]))

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
        mutations = {row["id"] for row in manifest["witnesses"] if row["kind"] == "mutation_kill"}
        self.assertEqual({row["id"] for row in self.report["witnesses"]}, expected | mutations)
        for row in self.report["witnesses"]:
            if row.get("kind") == "mutation_kill":
                # Routed only while bound; see test_phase1_mutation_witness.py.
                self.assertEqual(row["producer_metrics"],
                                 [scope.MUTATION_METRIC] if row["state"] == "bound" and row["operations"] else [])
            else:
                self.assertTrue(row["producer_metrics"])

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


class LaterStepResolutionTests(unittest.TestCase):
    """A later-step transfer stays pending until another step's case, a covering witness or a reviewed destination answers it."""

    @classmethod
    def setUpClass(cls):
        cls.scope = json.loads((ROOT / "data/phase1/scope.json").read_text())
        cls.cases = json.loads((ROOT / "data/phase1/cases.json").read_text())
        results = scope.recorded_results(cls.cases)
        linked = {operation for case in cls.cases["cases"] for operation in case.get("operations", [])}
        linked |= {operation for _, operations in scope.covering_witness_operations(cls.cases, committed_scope=cls.scope)
                   for operation in operations}
        rows = {row["id"]: row for row in cls.scope["operations"]}
        cls.operation = next(operation for operation, entry in sorted(scope.roster_exemptions("leaves").items())
                             if entry["category"] == "later_step" and operation not in linked
                             and rows[operation]["basis_kind"] != "review")
        cls.case = next(case["id"] for case in cls.cases["cases"]
                        if case["family"] == "config" and results[case["id"]] == "match")

    def pending(self, document, cases):
        report = scope.leaf_preparation(document, cases, "leaves")
        self.assertEqual(report["total_operations"], report["accounted_operations"] + len(report["pending"]))
        self.assertEqual(report["later_step_unresolved"],
                         sum(row.get("reason") == "later_step_unresolved" for row in report["pending"]))
        return {row["operation"]: row for row in report["pending"]}, report

    def gaps(self, cases):
        original = Path.read_bytes
        target = ROOT / "data/phase1/cases.json"
        replacement = json.dumps(cases).encode()
        with patch.object(Path, "read_bytes", lambda path: replacement if path == target else original(path)):
            report = coverage.build()
        return {row["id"]: row["root_cause"] for row in report["gaps"]}

    def test_an_unlinked_transfer_is_pending_and_a_gap(self):
        pending, report = self.pending(self.scope, self.cases)
        self.assertEqual(pending[self.operation].get("reason"), "later_step_unresolved")
        self.assertFalse(report["complete"])
        self.assertNotIn("later_step", report["exempt_by_category"])
        self.assertEqual(self.gaps(self.cases).get(self.operation), "later_step_unresolved")

    def test_a_preparing_case_of_the_owning_step_answers_it(self):
        cases = copy.deepcopy(self.cases)
        case = next(row for row in cases["cases"] if row["id"] == self.case)
        case["operations"].append(self.operation)
        case["result_evidence"]["claims_sha256"] = scope.case_claims_digest(case)
        self.assertNotIn(self.operation, self.pending(self.scope, cases)[0])
        self.assertNotIn(self.operation, self.gaps(cases))

    def test_a_covering_rust_witness_answers_it(self):
        cases = copy.deepcopy(self.cases)
        witness = next(row for row in cases["witnesses"] if row["kind"] == "rust_gated" and row.get("operations"))
        witness["operations"].append(self.operation)
        self.assertNotIn(self.operation, self.pending(self.scope, cases)[0])
        self.assertNotIn(self.operation, self.gaps(cases))

    def test_only_a_reviewed_destination_answers_it(self):
        document = copy.deepcopy(self.scope)
        row = next(row for row in document["operations"] if row["id"] == self.operation)
        row.update(disposition="later_phase", basis_kind="review", destination_phase=2)
        self.assertNotIn(self.operation, self.pending(document, self.cases)[0])
        row.update(basis_kind="rule", destination_phase=None)
        self.assertIn(self.operation, self.pending(document, self.cases)[0])


class ReviewedDestinationTests(unittest.TestCase):
    def test_the_owner_approval_covers_exactly_the_reviewed_rows(self):
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        approval = review["approval"]
        self.assertEqual(scope.review_approval_problems(review), [])
        self.assertEqual((approval["date"], approval["statement"], approval["rows"]), ("2026-09-25", "yes approved", 205))
        self.assertEqual(approval["rows_sha256"], scope.review_approval_digest(review["reviewed_operation_destinations"]))
        for name, mutate in (
                ("absent", lambda d: d.pop("approval")),
                ("edited row", lambda d: d["reviewed_operation_destinations"][0].update(destination_phase=3)),
                ("added row", lambda d: d["reviewed_operation_destinations"].append(
                    {**d["reviewed_operation_destinations"][0], "operation": "tsc/internal/compiler/x.go:Y"})),
                ("removed row", lambda d: d["reviewed_operation_destinations"].pop()),
                ("forged digest", lambda d: d["approval"].update(rows_sha256="0" * 64)),
                ("no approver", lambda d: d["approval"].update(approved_by="")),
                ("bad date", lambda d: d["approval"].update(date="25/09/2026")),
                ("no decision", lambda d: d["approval"].update(decision="docs/absent.md#x")),
                ("other scope", lambda d: d["approval"].update(scope="reviewed_unused_compiler_operations")),
                # The pre-approval authority text contradicted the approval block.
                ("authority denies approval", lambda d: d.update(authority=(
                    "Accepted docs/PHASE1-implementation-plan.md section 2 exclusions, interpreted per pinned "
                    "compiler operation; not new owner-approved deferrals or behavioral waivers."))),
                ("authority cites and denies", lambda d: d.update(authority=(
                    "See `approval`; these rows are not new owner-approved deferrals."))),
                ("authority omits approval", lambda d: d.update(authority="Accepted plan exclusions."))):
            with self.subTest(mutation=name):
                changed = copy.deepcopy(review)
                mutate(changed)
                self.assertTrue(scope.review_approval_problems(changed))

    def test_consumers_refuse_an_edited_approved_row(self):
        original_bytes, original_text = Path.read_bytes, Path.read_text
        target = ROOT / "data/phase1/coverage-review.json"
        review = json.loads(original_text(target))
        review["reviewed_operation_destinations"][0]["reason"] += " Edited after approval."
        replacement = json.dumps(review)
        with patch.object(Path, "read_bytes", lambda path: replacement.encode() if path == target else original_bytes(path)):
            report = coverage.build()
        self.assertTrue(any("needs a new owner approval" in problem for problem in report["problems"]))
        with patch.object(Path, "read_text", lambda path, *a, **k: replacement if path == target
                          else original_text(path, *a, **k)):
            with self.assertRaisesRegex(ValueError, "owner approval"):
                scope.reviewed_destinations()

    def test_reference_loading_is_phase1_work_under_the_accepted_build_boundary(self):
        review = json.loads((ROOT / "data/phase1/coverage-review.json").read_text())
        roster = json.loads((ROOT / "data/phase1/syntax-roster.json").read_text())
        cases = json.loads((ROOT / "data/phase1/cases.json").read_text())
        decisions = scope.reviewed_destinations()
        loading = {"tsc/internal/compiler/fileloader.go:fileLoader.addProjectReferenceTasks",
                   "tsc/internal/compiler/program.go:Program.verifyProjectReferences",
                   "tsc/internal/compiler/filesparser.go:parseTask.redirect"}
        # Ported and witnessed: neither unresolved, exempt nor a later-phase destination.
        self.assertEqual(review["unresolved_compiler_destinations"], [])
        self.assertEqual(review["unresolved_compiler_evidence"], [])
        self.assertFalse(loading & {row["operation"] for row in roster["exemptions"]})
        self.assertFalse(loading & decisions.keys())
        claimed = {operation for case in cases["cases"]
                   if case["id"].startswith("syntax/project-references/")
                   for operation in case["operations"]}
        self.assertLessEqual(loading, claimed)
        self.assertEqual(len(claimed), 23)
        # The editor host and the checker accessors keep their destinations.
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
