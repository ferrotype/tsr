"""A native-unavailable platform row needs its own authenticated execution."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_producers as p
import phase1_coverage as coverage

CASE = "filesystem/osvfs/nativepath-realpath-linux-procfs"


class PlatformCaptureTests(unittest.TestCase):
    def setUp(self):
        self.provenance = {"family": "filesystem", "source_closure": {"input": "digest"},
                           "selected_cases": [CASE], "host": {"platform": "Linux-6.8-x86_64"}}
        self.supplemental = {"rows": [{"case": CASE, "result": "match"},
                                      {"case": "another", "result": "not_run"}]}
        self.report = {"rows": [{"case": CASE, "result": "native_unavailable"},
                                {"case": "another", "result": "match"}]}

    def attach(self, provenance=None, supplemental=None, directories=1):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps(provenance or self.provenance))
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={"input": "digest"}), \
                 patch.object(p.capture, "compare", return_value=supplemental or self.supplemental) as compare:
                p.attach_platform_captures("filesystem", self.report, [root] * directories)
                self.assertEqual(compare.call_count, directories)

    def test_separate_linux_witness_keeps_mac_raw_row(self):
        raw = copy.deepcopy(self.report["rows"])
        self.attach()
        self.assertEqual(self.report["rows"], raw)
        self.assertTrue(p.platform_matched("filesystem", raw[0], self.report))
        self.assertFalse(p.platform_matched("filesystem", raw[1], self.report))
        self.assertFalse(p.platform_matched("syntax", raw[0], self.report))

    def test_rejects_wrong_host_changed_input_or_missing_selection(self):
        for field, value in (("host", {"platform": "macOS-26-arm64"}),
                             ("source_closure", {"input": "old"}),
                             ("selected_cases", []), ("selected_cases", [CASE, CASE]),
                             ("selected_cases", ["unknown"]), ("family", "syntax")):
            doc = copy.deepcopy(self.provenance)
            doc[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.attach(provenance=doc)

    def test_failed_or_unavailable_linux_cannot_pass(self):
        for outcome in ("different", "native_unavailable", "not_implemented", "not_run"):
            rows = {"rows": [{"case": CASE, "result": outcome}]}
            with self.subTest(outcome=outcome), self.assertRaises(ValueError):
                self.attach(supplemental=rows)

    def test_does_not_override_executed_rows_or_duplicate_witnesses(self):
        for result in ("different", "not_implemented", "match"):
            self.report["rows"][0]["result"] = result
            with self.subTest(result=result), self.assertRaisesRegex(ValueError, "executed result"):
                self.attach()
        self.report["rows"][0]["result"] = "native_unavailable"
        with self.assertRaisesRegex(ValueError, "duplicate platform witness"):
            self.attach(directories=2)

    def test_unscoped_case_cannot_be_replaced(self):
        self.provenance["selected_cases"] = ["another"]
        self.supplemental["rows"] = [{"case": "another", "result": "match"}]
        with self.assertRaisesRegex(ValueError, "no platform supplementation policy"):
            self.attach()

    def test_comparator_rejection_propagates(self):
        with patch.object(p.capture, "compare", side_effect=ValueError("tampered artifact")):
            # Exercise the real attach call without the convenience helper's
            # comparator patch: authentication failure must never be waived.
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / "provenance.json").write_text(json.dumps(self.provenance))
                with patch.object(p, "rust_packages", return_value=[]), \
                     patch.object(p.capture, "source_closure", return_value={"input": "digest"}), \
                     self.assertRaisesRegex(ValueError, "tampered artifact"):
                    p.attach_platform_captures("filesystem", self.report, [root])

    def test_platform_preparation_closes_only_its_linked_gap(self):
        self.attach()
        document = p.read(p.ROOT / "data/phase1/scope.json")
        cases = p.read(p.ROOT / "data/phase1/cases.json")
        health = {"healthy": True, "coverage": coverage.build(), "preparations": {
            family: p.scope.leaf_preparation(document, cases, family)
            for family in p.scope.STEP_PACKAGES}}
        before = copy.deepcopy(health)
        reports = {"filesystem": self.report}
        raw_reports = copy.deepcopy(reports)
        frozen = {path: (p.ROOT / path).read_bytes() for path in (
            "data/phase1/scope.json", "data/phase1/cases.json", "data/phase1/coverage-report.json.gz")}
        self.assertTrue(before["coverage"]["healthy"], before["coverage"]["problems"])
        self.assertFalse(before["preparations"]["filesystem"]["complete"])

        p.apply_platform_preparation(reports, health)

        self.assertTrue(health["preparations"]["filesystem"]["complete"])
        self.assertEqual(health["preparations"]["filesystem"]["pending"], [])
        for family in p.scope.STEP_PACKAGES:
            if family != "filesystem":
                self.assertEqual(health["preparations"][family], before["preparations"][family])
        removed = {row["id"] for row in before["coverage"]["gaps"]} - {
            row["id"] for row in health["coverage"]["gaps"]}
        self.assertEqual(removed, {"tsc/internal/nativepath/realpath_linux.go:Realpath"})
        self.assertTrue(health["coverage"]["healthy"], health["coverage"]["problems"])
        self.assertFalse(health["coverage"]["preparation_complete"])
        self.assertEqual(health["coverage"]["supplemental_prepared_cases"], [CASE])
        for key in ("cases", "case_gaps", "families"):
            self.assertEqual(health["coverage"][key], before["coverage"][key])
        self.assertEqual(reports, raw_reports)
        self.assertEqual({path: (p.ROOT / path).read_bytes() for path in frozen}, frozen)
        full_report = {"rows": [{"case": row["case"],
                                 "result": "native_unavailable" if row["case"] == CASE else "match"}
                                for row in p.capture.load_requests(p.capture.FAMILIES["filesystem"])["requests"]],
                       "platform_witnesses": self.report["platform_witnesses"]}
        health["integration"] = {"prepared": False}
        metrics = p.aggregate("foundations", {"filesystem": full_report}, health)["metrics"]
        self.assertTrue(metrics["filesystem_prepared"])
        self.assertTrue(metrics["filesystem_complete"])
        self.assertFalse(metrics["utilities_complete"])

    def test_preparation_without_authenticated_witness_is_unchanged(self):
        health = {"preparations": {"filesystem": {"complete": False}}, "coverage": {"healthy": True}}
        before = copy.deepcopy(health)
        p.apply_platform_preparation({"filesystem": self.report}, health)
        self.assertEqual(health, before)

    def test_invalid_supplemental_preparation_case_is_rejected(self):
        cases = p.read(p.ROOT / "data/phase1/cases.json")["cases"]
        executed = next(row["id"] for row in cases if row["family"] == "filesystem"
                        and row["last_result"] == "match")
        pilot = next(row["id"] for row in cases if row["family"] == "pilot")
        for identity in ("unknown", executed, pilot):
            with self.subTest(identity=identity), self.assertRaisesRegex(ValueError, "native-unavailable acceptance"):
                coverage.build(supplemental_prepared_cases={identity})


if __name__ == "__main__":
    unittest.main()
