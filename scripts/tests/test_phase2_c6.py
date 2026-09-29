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


if __name__ == "__main__":
    unittest.main()
