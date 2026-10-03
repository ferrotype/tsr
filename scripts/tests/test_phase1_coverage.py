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
        # The closure leaves no unresolved transfer in the committed rosters,
        # so the routes are proved on a synthetic one: an exempt, unlinked leaf
        # operation whose exemption the patched roster reports as later_step.
        original = scope.roster_exemptions
        cls.operation = next(operation for operation, entry in sorted(original("leaves").items())
                             if entry["category"] != "later_step" and operation not in linked
                             and rows[operation]["basis_kind"] != "review"
                             and rows[operation]["disposition"] != "later_phase")

        def transferred(step):
            exemptions = copy.deepcopy(original(step))
            if step in (None, "leaves"):
                exemptions[cls.operation] = {**exemptions[cls.operation], "category": "later_step",
                                             "owner": "a later step, for this test"}
            return exemptions

        patcher = patch.object(scope, "roster_exemptions", transferred)
        patcher.start()
        cls.addClassCleanup(patcher.stop)
        cls.case = next(case["id"] for case in cls.cases["cases"]
                        if case["family"] == "config" and results[case["id"]] == "match")

    def pending(self, document, cases):
        report = scope.leaf_preparation(document, cases, "leaves")
        self.assertEqual(report["total_operations"], report["accounted_operations"] + len(report["pending"]))
        self.assertEqual(report["later_step_unresolved"],
                         sum(row.get("reason") == "later_step_unresolved" for row in report["pending"]))
        return {row["operation"]: row for row in report["pending"]}, report

    def gaps(self, cases):
        # The join reads the transfer from the committed scope's roster state,
        # so the synthetic transfer is written into that document as well.
        original = Path.read_bytes
        document = copy.deepcopy(self.scope)
        row = next(row for row in document["operations"] if row["id"] == self.operation)
        row["roster"] = {**row["roster"], "state": "exempt:later_step", "owner": "a later step, for this test"}
        replacements = {ROOT / "data/phase1/cases.json": json.dumps(cases).encode(),
                        ROOT / "data/phase1/scope.json": json.dumps(document).encode()}
        with patch.object(Path, "read_bytes", lambda path: replacements.get(path) or original(path)):
            report = coverage.build()
        return {row["id"]: row["root_cause"] for row in report["gaps"]}

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
        self.assertEqual((approval["date"], approval["rows"]), ("2026-09-25", 209))
        self.assertTrue(approval["statement"].startswith("yes approved"))
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
        # The 23 reference-loading operations, plus the symlinked-output helpers
        # the js-import case witnesses on the same load path.
        helpers = {"tsc/internal/ast/utilities.go:NewHasFileName",
                   "tsc/internal/ast/utilities.go:hasFileNameImpl.FileName",
                   "tsc/internal/ast/utilities.go:hasFileNameImpl.Path"}
        self.assertLessEqual(helpers, claimed)
        self.assertEqual(len(claimed - helpers), 23)
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

    def test_a_row_outside_the_compiler_moves_only_by_owner_decision(self):
        original = Path.read_text
        target = ROOT / "data/phase1/coverage-review.json"
        review = json.loads(original(target))
        rows = review["reviewed_operation_destinations"]
        decided = [row for row in rows if row["authority_basis"] == "owner_decision"]
        self.assertEqual({row["operation"].split(":")[0] for row in decided}, {"tsc/internal/ast/ast_generated.go"})
        self.assertEqual(len(decided), 4)
        self.assertEqual({row["destination_phase"] for row in decided}, {2})
        self.assertEqual(scope.reviewed_destinations()[decided[0]["operation"]]["destination_phase"], 2)
        decided[0]["authority_basis"] = "accepted_plan_scope_interpretation"
        review["approval"]["rows_sha256"] = scope.review_approval_digest(rows)
        with patch.object(Path, "read_text", lambda path, *a, **kw: json.dumps(review) if path == target else original(path, *a, **kw)):
            with self.assertRaisesRegex(ValueError, "only the reviewed partial compiler scope"):
                scope.reviewed_destinations()

    def test_review_rejects_changed_pinned_source(self):
        original = Path.read_bytes
        target = ROOT / "upstream/tsc/internal/compiler/checkerpool.go"
        with patch.object(Path, "read_bytes", lambda path: b"changed" if path == target else original(path)):
            with self.assertRaisesRegex(ValueError, "reviewed pinned source changed"):
                scope.reviewed_destinations()


if __name__ == "__main__":
    unittest.main()
