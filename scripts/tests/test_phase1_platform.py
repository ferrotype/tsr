"""Every applicable (request, GOOS) needs its own authenticated observation."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_producers as p

REQUESTS = [{"case": name, "hosts": targets} for name, targets in (
    ("any", ["any"]), ("linux", ["linux"]), ("darwin", ["darwin"]), ("posix", ["posix"]))]


def comparison(goos, selected=None):
    selected = [r["case"] for r in REQUESTS] if selected is None else selected
    return {"family": "filesystem", "host": {"goos": goos}, "rows": [
        {"case": r["case"], "hosts": r["hosts"],
         "result": "not_run" if r["case"] not in selected else
                   "match" if p.hosts.applies(r, goos) else "not_applicable"}
        for r in REQUESTS]}


class HostCaptureTests(unittest.TestCase):
    def setUp(self):
        for mock in (patch.object(p, "filesystem_requests", return_value=REQUESTS),
                     patch.object(p, "qualifications", return_value={}),
                     patch.object(p, "rust_packages", return_value=[]),
                     patch.object(p.capture, "source_closure", return_value={"input": "digest"})):
            mock.start()
            self.addCleanup(mock.stop)
        self.report = {**comparison("darwin"), "capture_identity": "darwin-archive"}
        self.provenance = {"family": "filesystem", "source_closure": {"input": "digest"},
                           "selected_cases": p.host_inventory("linux"), "host": {"goos": "linux"}}
        self.supplemental = comparison("linux", self.provenance["selected_cases"])

    def attach(self, provenance=None, supplemental=None, directories=1):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps(self.provenance if provenance is None else provenance))
            with patch.object(p.capture, "compare", return_value=self.supplemental if supplemental is None else supplemental) as compare:
                unavailable = p.attach_platform_captures("filesystem", self.report, [root] * directories)
                self.assertEqual(compare.call_count, directories)
                return unavailable

    def test_any_and_posix_explicitly_require_both_supported_ci_hosts(self):
        self.assertEqual(p.host_inventory("linux"), ["any", "linux", "posix"])
        self.assertEqual(p.host_inventory("darwin"), ["any", "darwin", "posix"])
        for request in (REQUESTS[0], REQUESTS[3]):
            self.assertEqual(set(p.hosts.required_goos(request)), {"linux", "darwin"})
        with self.assertRaisesRegex(ValueError, "unsupported"):
            p.host_inventory("freebsd")

    def test_separate_archives_preserve_raw_rows_and_exclude_other_host_rows(self):
        raw = copy.deepcopy(self.report)
        linux_raw = copy.deepcopy(self.supplemental)
        self.assertEqual(self.attach(), {})
        self.assertEqual(self.report["rows"], raw["rows"])
        self.assertEqual(self.report["capture_identity"], raw["capture_identity"])
        self.assertEqual(self.report["host_captures"]["linux"]["report"], linux_raw)
        self.assertEqual(self.supplemental, linux_raw)
        coverage = p.host_coverage(self.report)
        self.assertTrue(coverage["complete"], coverage)
        self.assertEqual(coverage["required_hosts"], ["darwin", "linux"])
        self.assertEqual(set(coverage["captures"]), {"darwin", "linux"})

    def test_exact_applicable_partial_capture_can_be_the_base(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            provenance = {**self.provenance, "partial": True}
            (directory / "provenance.json").write_text(json.dumps(provenance))
            with patch.object(p.capture, "compare", return_value=copy.deepcopy(self.supplemental)):
                base = p.replay_family("filesystem", directory)
        self.assertEqual(base["rows"], self.supplemental["rows"])
        self.assertEqual(base["rows"][2]["result"], "not_run")
        self.assertFalse(p.host_coverage(base)["complete"])
        base["host_captures"] = {"darwin": {"capture_identity": "darwin-archive",
            "report": comparison("darwin", p.host_inventory("darwin"))}}
        health = {"healthy": True, "preparations": {f: {"complete": True} for f in p.scope.STEP_PACKAGES},
                  "coverage": {"preparation_complete": False}, "integration": {"prepared": False}}
        with patch.object(p.capture, "load_requests", return_value={"requests": REQUESTS}):
            metrics = p.aggregate("foundations", {"filesystem": base}, health)["metrics"]
        self.assertTrue(metrics["filesystem_prepared"])
        self.assertTrue(metrics["filesystem_complete"])

    def test_missing_host_cannot_be_prepared_despite_all_local_rows_matching(self):
        coverage = p.host_coverage(self.report)
        self.assertFalse(coverage["complete"])
        self.assertEqual(coverage["missing_hosts"], ["linux"])
        self.assertEqual(coverage["missing"], [{"case": c, "goos": "linux"} for c in ("any", "linux", "posix")])
        self.assertEqual(coverage["complete_cases"], ["darwin"])

    def test_wrong_host_missing_or_duplicate_inventory_is_rejected(self):
        for field, value in (("host", {"goos": "darwin"}), ("host", {}),
                             ("selected_cases", []), ("selected_cases", ["linux", "linux"]),
                             ("selected_cases", ["linux"]), ("family", "syntax")):
            provenance = copy.deepcopy(self.provenance)
            provenance[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.attach(provenance=provenance)

    def test_sorted_selection_does_not_change_non_alphabetical_request_order(self):
        requests = [{"case": identity, "hosts": ["any"]} for identity in ("z", "a", "b")]
        provenance = {**self.provenance, "selected_cases": ["a", "b", "z"]}
        report = {"host": {"goos": "linux"}, "rows": [
            {"case": request["case"], "result": "match"} for request in requests]}
        with patch.object(p, "filesystem_requests", return_value=requests):
            self.attach(provenance=provenance, supplemental=report)
            report["rows"].reverse()
            with self.assertRaisesRegex(ValueError, "exactly once"):
                self.attach(provenance=provenance, supplemental=report)

    def test_stale_host_stays_unavailable_without_replacing_base(self):
        provenance = {**self.provenance, "source_closure": {"input": "old"}}
        raw = copy.deepcopy(self.report["rows"])
        unavailable = self.attach(provenance=provenance)
        self.assertEqual(len(unavailable), 1)
        self.assertIn("linux host capture current source closure differs", next(iter(unavailable.values())))
        self.assertEqual(self.report["rows"], raw)
        self.assertEqual(self.report["host_captures"], {})
        self.assertFalse(p.host_coverage(self.report)["complete"])

    def test_applicable_failure_remains_failure_on_either_host(self):
        for target in ("darwin", "linux"):
            for result in ("native_unavailable", "different", "not_implemented", "not_applicable"):
                with self.subTest(target=target, result=result):
                    self.report = {**comparison("darwin"), "capture_identity": "darwin-archive"}
                    supplemental = copy.deepcopy(self.supplemental)
                    rows = self.report["rows"] if target == "darwin" else supplemental["rows"]
                    rows[0]["result"] = result
                    self.attach(supplemental=supplemental)
                    coverage = p.host_coverage(self.report)
                    self.assertFalse(coverage["complete"])
                    self.assertIn({"case": "any", "goos": target, "result": result}, coverage["failures"])

    def test_one_hosts_match_cannot_override_other_hosts_executed_difference(self):
        self.report["rows"][0]["result"] = "different"
        self.attach()
        self.assertEqual(self.report["rows"][0]["result"], "different")
        self.assertFalse(p.host_coverage(self.report)["complete"])

    def test_duplicate_host_captures_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate filesystem host"):
            self.attach(directories=2)
        provenance = {**self.provenance, "host": {"goos": "darwin"}, "selected_cases": p.host_inventory("darwin")}
        with self.assertRaisesRegex(ValueError, "duplicate filesystem host"):
            self.attach(provenance=provenance, supplemental=comparison("darwin", provenance["selected_cases"]))

    def test_exact_approved_difference_is_consumed_for_each_required_host(self):
        row = self.supplemental["rows"][0]
        row.update(result="different", native={"value": "native"}, rust={"value": "rust"})
        self.attach()
        approval = {"any": {"native": {"value": "native"}, "rust": {"value": "rust"}}}
        self.assertTrue(p.host_coverage(self.report, approval)["complete"])
        row = self.report["host_captures"]["linux"]["report"]["rows"][0]
        row["rust"]["extra"] = True
        self.assertFalse(p.host_coverage(self.report, approval)["complete"])

    def test_comparator_rejection_is_not_downgraded_to_staleness(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps({**self.provenance, "source_closure": {}}))
            with patch.object(p.capture, "compare", side_effect=ValueError("tampered artifact")), \
                 self.assertRaisesRegex(ValueError, "tampered artifact"):
                p.attach_platform_captures("filesystem", self.report, [root])

    def test_host_summary_preserves_raw_report_and_records_applicable_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps(self.provenance))
            for observed, state in (("match", "match"), ("different", "different"),
                                    ("native_unavailable", "unavailable")):
                report = copy.deepcopy(self.supplemental)
                report["rows"][0]["result"] = observed
                with self.subTest(observed=observed), patch.object(p.capture, "compare", return_value=report):
                    summary = p.platform_summary(root, root / "records")
                    self.assertEqual(summary["state"], state)
                    raw = Path(summary["artifact"]).read_bytes()
                    self.assertEqual(p.sha(raw), summary["sha256"])
                    artifact = json.loads(raw)
                    self.assertEqual(artifact["capture_identity"], p.sha((root / "provenance.json").read_bytes()))
                    self.assertEqual(artifact["report"], report)
                    self.assertEqual(artifact["host"]["goos"], "linux")

    def test_preparation_requires_every_host_and_leaves_raw_reports_intact(self):
        health = {"healthy": True, "preparations": {f: {"complete": True} for f in p.scope.STEP_PACKAGES},
                  "coverage": {"preparation_complete": False}, "integration": {"prepared": False}}
        requests = {"requests": REQUESTS}
        with patch.object(p.capture, "load_requests", return_value=requests):
            self.assertFalse(p.aggregate("foundations", {"filesystem": self.report}, health)["metrics"]["filesystem_prepared"])
            self.attach()
            self.assertTrue(p.aggregate("foundations", {"filesystem": self.report}, health)["metrics"]["filesystem_prepared"])
            self.report["rows"][0]["result"] = "native_unavailable"
            self.assertFalse(p.aggregate("foundations", {"filesystem": self.report}, health)["metrics"]["filesystem_prepared"])

    def test_supplemental_preparation_uses_only_complete_historical_unavailable_rows(self):
        self.attach()
        health = {"preparations": {}, "coverage": {}}
        cases = {"cases": []}
        with patch.object(p, "read", return_value=cases), \
             patch.object(p.scope, "recorded_results", return_value={"linux": "not_applicable", "any": "match"}), \
             patch.object(p.scope, "leaf_preparation", return_value={"complete": True}) as prepare, \
             patch("phase1_coverage.build", return_value={}) as coverage:
            p.apply_platform_preparation({"filesystem": self.report}, health)
        self.assertEqual(coverage.call_args.kwargs["supplemental_prepared_cases"], {"linux"})
        self.assertTrue(all(c.kwargs["supplemental_prepared_cases"] == {"linux"} for c in prepare.call_args_list))


if __name__ == "__main__":
    unittest.main()
