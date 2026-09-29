"""C6 starts from the concurrent native capture, the moved checker-pool ledger entry, the bound audit scope and
the C6-start baseline; the two native modes differ only where the pin's own union-ordering walk counts checkers."""
import copy
import json
from pathlib import Path
import sys
import tomllib
import unittest
import unittest.mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_assignments as assignments  # noqa: E402
import phase2_audit as audit  # noqa: E402
import phase2_blockers as blockers  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import phase2_native_concurrent as concurrent  # noqa: E402
import phase2_producers as producers  # noqa: E402


class Wiring(unittest.TestCase):
    def test_c6_authorities_are_checker_inputs_and_the_audit_scope_is_bound(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C6"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        self.assertIn("data/phase2/native-provenance-concurrent.json", spec["inputs"])
        self.assertIn("C6", blockers.CHECKPOINT_CLAIMS)
        self.assertEqual(producers.CHECKPOINTS[-1], "C6")
        document = audit.load(ROOT / "data/phase2/c6-audit.json")
        self.assertEqual(document["checkpoint"], "C6")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        for group, (count, _) in audit.C6_REVIEWED_GROUPS.items():
            self.assertEqual(len(document["groups"][group]), count)
        self.assertEqual(sum(count for count, _ in audit.C6_REVIEWED_GROUPS.values()), 89)
        self.assertEqual(len(audit.C6_COMPLETE_FILES), 3)
        altered = copy.deepcopy(document)
        altered["groups"]["C6.3 compiler checker pool (checkerpool.go)"].pop()
        self.assertTrue(audit.problems(altered, allow_open=True))

    def test_the_checker_pool_ledger_entry_is_phase_2(self):
        ledger = tomllib.loads((ROOT / "PORTS.toml").read_text())
        phases = {entry["go"]: entry["phase"] for entry in ledger["file"]}
        self.assertEqual(phases["tsc/internal/compiler/checkerpool.go"], 2)
        self.assertEqual(phases["tsc/internal/compiler/program.go"], 4)
        self.assertEqual(phases["tsc/internal/project/checkerpool.go"], 5)


class NativeModes(unittest.TestCase):
    def test_the_concurrent_capture_is_bound_to_its_mode_and_the_mode_differences_are_recorded(self):
        provenance = json.loads((ROOT / "data/phase2/native-provenance-concurrent.json").read_text())
        self.assertEqual((provenance["mode"], provenance["single_threaded"]), ("concurrent", False))
        self.assertEqual(provenance["reference_disagreements"], [])
        single = json.loads((ROOT / "data/phase2/native-provenance.json").read_text())
        self.assertIs(single["single_threaded"], True)
        self.assertEqual((provenance["go"], provenance["goos"], provenance["goarch"]),
                         (single["go"], single["goos"], single["goarch"]))
        self.assertEqual(provenance["oracle"], single["oracle"])
        claims = json.loads((ROOT / "data/phase2/c6-claims.json").read_text())
        modes = claims["native_modes"]
        self.assertEqual(modes["differences"], [])
        self.assertEqual(modes["concurrent"]["capture_observation_sha256"], provenance["observation_sha256"])
        self.assertEqual(modes["single"]["capture_observation_sha256"], single["observation_sha256"])
        self.assertEqual(claims["rows"], [])

    def test_a_single_threaded_report_is_not_a_concurrent_capture(self):
        report = {"mode": "single", "single_threaded": True, "partial": False}
        with self.assertRaisesRegex(ValueError, "not a concurrent-mode native capture"):
            with unittest.mock.patch.object(concurrent.native, "load_capture",
                                            return_value=(ROOT, report, [])):
                concurrent.load_capture(ROOT)


class Assignments(unittest.TestCase):
    """C6.7: the recorded assignment witnesses and the arm64 fusion evidence."""

    def setUp(self):
        self.record = json.loads(assignments.RECORD.read_text())

    def test_the_record_is_the_pins_on_the_capture_host_and_covers_every_feature(self):
        pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
        self.assertEqual(self.record["pin"], pin)
        self.assertEqual((self.record["goos"], self.record["goarch"], self.record["go"]),
                         ("darwin", "arm64", "go1.27.1"))
        self.assertEqual(self.record["mode"], "concurrent")
        self.assertIs(self.record["step_check"]["equal"], True)
        self.assertGreater(self.record["step_check"]["distinct_multi_checker_records"], 0)
        decided = {(case["checker_count"], feature) for case in self.record["synthetic"] for feature in case["features"]}
        for count in (2, 4, 8):
            for feature in ("tie", "fallback", "slack", "source_dominated", "imports", "import_unit_clamp"):
                self.assertIn((count, feature), decided)
        self.assertIn((4, "fusion"), decided)

    def test_the_fusion_cases_are_decided_by_fused_arithmetic(self):
        fusion = [case for case in self.record["synthetic"] if "fusion" in case["features"]]
        self.assertEqual(len(fusion), 2)
        for case in fusion:
            fused, _ = assignments._replica(case, True)
            plain, _ = assignments._replica(case, False)
            self.assertNotEqual(fused, plain)
            self.assertEqual(case["native"]["associations"], fused)

    def test_the_synthetic_set_is_the_generators(self):
        generated = assignments.synthetic_cases()
        self.assertEqual([assignments.step_input(case) for case in generated],
                         [assignments.step_input(case) for case in self.record["synthetic"]])

    def test_the_fusion_evidence_names_both_fused_sites_on_arm64_only(self):
        text = assignments.FUSION.read_text()
        arm64, amd64 = text.split("## arm64\n", 1)[1].split("## amd64 GOAMD64=v1\n", 1)
        self.assertEqual(sum("FMSUBD" in line for line in arm64.splitlines()), 2)
        self.assertIn("checkerpool.go:210) FMSUBD", arm64)
        self.assertIn("checkerpool.go:211) FMSUBD", arm64)
        self.assertNotIn("FMSUB", amd64)
        self.assertNotIn("VFMADD", amd64)
        self.assertNotIn("VFNMADD", amd64)


class Modes(unittest.TestCase):
    """C6.4: a request names its test-program mode and a pooled row records
    its mode and checker count; union ordering covers every checker."""

    def row(self, mode, count, checkers):
        row = {"load": {"state": "executed"}, "phase2": {
            "trace": {"state": "disabled"},
            "union_ordering": {"state": "executed", "checkers": checkers, "unions": 7 * checkers, "inconsistent": 0},
            "parent_pointers": {"state": "executed", "files": 1, "nodes": 1, "failure": None}}}
        if mode is not None:
            row["mode"] = mode
        if count is not None:
            row["checker_count"] = count
        return row

    def validate(self, request, row):
        with unittest.mock.patch.object(corpus.p5, "validate_row"), \
                unittest.mock.patch.object(corpus, "pre_emit_view", side_effect=lambda _, base: base):
            return corpus.validate_row(dict(request, loading={"options": {}}), row)

    def test_a_pooled_row_records_its_mode_and_checker_count(self):
        self.validate({"mode": "concurrent"}, self.row("concurrent", 4, 4))
        self.validate({"mode": "single"}, self.row("single", 1, 1))
        self.validate({}, self.row(None, None, 1))
        for request, row, message in (
                ({"mode": "concurrent"}, self.row(None, None, 4), "mode differs"),
                ({"mode": "concurrent"}, self.row("single", 4, 4), "mode differs"),
                ({"mode": "single"}, self.row("single", 4, 4), "checker count"),
                ({"mode": "concurrent"}, self.row("concurrent", 0, 0), "checker count"),
                ({"mode": "concurrent"}, self.row("concurrent", 4, 1), "every checker"),
                ({}, self.row("single", 1, 1), "without a mode"),
                ({}, self.row(None, None, 2), "exactly one checker")):
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.validate(request, row)

    def test_the_single_mode_refuses_a_concurrent_capture(self):
        report = {"mode": "concurrent", "single_threaded": False}
        with unittest.mock.patch.object(corpus.phase2_native, "load_capture", return_value=(ROOT, report, [])), \
                unittest.mock.patch.object(corpus.phase2_native, "current"):
            with self.assertRaisesRegex(ValueError, "single-threaded native capture"):
                corpus.load_native(ROOT, "single")
        with self.assertRaisesRegex(ValueError, "unknown mode"):
            corpus.requests(ROOT, mode="parallel")


class Producer(unittest.TestCase):
    """C6.10: the concurrent mode's evidence, the modes' parity and the
    assignments decide C6's completion."""

    EXECUTED = 3

    def passing(self, stack, **changes):
        """Patch every loader of `concurrent_metrics` to a passing state."""
        facts = {"source_stable": True, "harness_errors": 0, "outcome_differences": 0, "mode": "concurrent",
                 "failed": [], **changes}
        report = {"observation_sha256": "concurrent", "mode": "concurrent", "single_threaded": False}
        review = {"reference_disagreements": [], "input_mismatches": [], "states": {"executed": self.EXECUTED}}
        replayed = {"summary": {"partial": False, "harness_errors": facts["harness_errors"], "observed": self.EXECUTED},
                    "source_stable": facts["source_stable"]}
        context = unittest.mock.Mock(replayed=replayed, metadata={
            "mode": facts["mode"], "native": {"observation_sha256": "concurrent"},
            "requests_sha256": producers.digest(producers.phase2_corpus.p4.canonical([]) + b"\n")})
        modes = {"outcome_differences": facts["outcome_differences"],
                 "single": {"harness_errors": 0, "rust_capture_sha256": "single-rust"}}
        concurrent = {"rows": [{"id": vid, "outcomes": {"errors": "failed"}} for vid in facts["failed"]]}
        patch = unittest.mock.patch.object
        stack.enter_context(patch(producers.phase2_native_concurrent, "load_capture", return_value=(ROOT, report, [])))
        stack.enter_context(patch(producers.phase2_native_concurrent, "current"))
        stack.enter_context(patch(producers.phase2_native_concurrent, "review", return_value=review))
        stack.enter_context(patch(producers, "strict_json_loads", side_effect=lambda raw: json.loads(raw)))
        stack.enter_context(patch(producers.phase2_compare, "load_context", return_value=context))
        stack.enter_context(patch(producers.phase2_corpus, "requests", return_value=(None, [], None)))
        stack.enter_context(patch(producers.phase2_compare, "modes", return_value=modes))
        stack.enter_context(patch(producers.phase2_compare, "report", return_value=concurrent))
        stack.enter_context(patch(producers, "assignments_current", return_value=facts.get("assignments", True)))
        stack.enter_context(patch(Path, "read_bytes", lambda path: json.dumps(
            {"observation_sha256": "concurrent"} if path.name == "verified.json" else review).encode()))

    def metrics(self, **changes):
        import contextlib
        with contextlib.ExitStack() as stack:
            self.passing(stack, **changes)
            claims = {"native_modes": {"differences": [], "single": {"capture_observation_sha256": "single"},
                                       "concurrent": {"capture_observation_sha256": "concurrent"}}}
            comparison = {"rust_capture_sha256": "single-rust", "native_observation_sha256": "single", "rows": []}
            authorities = producers.CHECKPOINT_AUTHORITIES["C6"]
            return producers.concurrent_metrics(ROOT, ROOT, ROOT, ROOT, {"counts": {"executed": self.EXECUTED}},
                                                comparison, claims, authorities)

    def test_the_passing_state_passes(self):
        self.assertEqual(self.metrics(), {"native_verified_concurrent": True, "harness_valid_concurrent": True,
                                          "c6_mode_parity": True, "c6_assignments": True,
                                          "c6_failures_concurrent": 0})

    def test_one_outcome_difference_keeps_mode_parity_false(self):
        self.assertIs(self.metrics(outcome_differences=1)["c6_mode_parity"], False)

    def test_a_stale_or_single_mode_concurrent_capture_is_not_a_valid_harness(self):
        for changes in ({"source_stable": False}, {"harness_errors": 1}, {"mode": "single"}):
            with self.subTest(changes=changes):
                metrics = self.metrics(**changes)
                self.assertIs(metrics["harness_valid_concurrent"], False)
                self.assertIs(metrics["c6_mode_parity"], False)

    def test_an_assignment_difference_keeps_assignments_false(self):
        self.assertIs(self.metrics(assignments=False)["c6_assignments"], False)

    def test_a_concurrent_only_failure_counts(self):
        self.assertEqual(self.metrics(failed=["a", "b"])["c6_failures_concurrent"], 2)

    def test_each_c6_boolean_gates_completion_and_failures_add(self):
        base = {"native_verified_concurrent": True, "harness_valid_concurrent": True, "c6_mode_parity": True,
                "c6_assignments": True, "c6_failures_concurrent": 0}
        comparison = {"rows": []}
        with unittest.mock.patch.object(producers.phase2_compare, "validate_complete_rows"):
            def complete(extra):
                return producers.checkpoint_metrics("C6", comparison, None, True, None, True, 1, None,
                                                    prerequisites={name: True for name in (
                                                        "inventory_frozen", "native_verified", "harness_valid",
                                                        "result_recorded", "blockers_named")}, extra=extra)
            self.assertFalse(complete(base)["c6_complete"], "counters missing without claims")
            for name in ("native_verified_concurrent", "harness_valid_concurrent", "c6_mode_parity", "c6_assignments"):
                metrics = complete(dict(base, **{name: False}))
                self.assertIs(metrics[name], False)
                self.assertFalse(metrics["c6_complete"])
            self.assertNotIn("c6_failures_concurrent", complete(base))


class AssignmentsCurrent(unittest.TestCase):
    """The recorded comparison binds the record, the Rust sources and the host."""

    def setUp(self):
        import tempfile
        import phase2_assignments
        self.assignments = phase2_assignments
        self.temp = Path(tempfile.mkdtemp())
        pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
        self.record = {"pin": pin, "inputs": {"a": "1"}, "step_check": {"equal": True}, "goarch": "arm64",
                       "synthetic": [{}, {}]}
        self.write()

    def write(self, **changes):
        (self.temp / "record.json").write_text(json.dumps(self.record))
        comparison = {"record_sha256": producers.digest((self.temp / "record.json").read_bytes()),
                      "rust_sources_sha256": "sources", "machine": "arm64", "equal": True, "programs": 5,
                      "programs_equal": 4, "unsupported": 1, "synthetic": 2, "synthetic_equal": 2, **changes}
        (self.temp / "comparison.json").write_text(json.dumps(comparison))

    def current(self):
        with unittest.mock.patch.object(self.assignments, "input_digests", return_value={"a": "1"}), \
                unittest.mock.patch.object(self.assignments, "rust_sources_sha256", return_value="sources"):
            return producers.assignments_current(self.temp / "record.json", self.temp / "comparison.json", 5)

    def test_every_binding_is_required(self):
        self.assertTrue(self.current())
        for changes in ({"equal": False}, {"rust_sources_sha256": "stale"}, {"machine": "x86_64"},
                        {"programs_equal": 3}, {"synthetic_equal": 1}, {"record_sha256": "other"}):
            with self.subTest(changes=changes):
                self.write(**changes)
                self.assertFalse(self.current())
        self.write()
        self.record["inputs"] = {"a": "2"}
        self.write()
        self.assertFalse(self.current(), "a changed witness input invalidates the record")


if __name__ == "__main__":
    unittest.main()
