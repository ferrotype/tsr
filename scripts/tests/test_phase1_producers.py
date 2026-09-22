"""The grouped tracker adapters must not turn incomplete captures into parity."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_producers as p


class AggregationTests(unittest.TestCase):
    def setUp(self):
        self.qualifier = patch.object(p, "qualifications", return_value={})
        self.qualifier.start()
        self.addCleanup(self.qualifier.stop)
        self.health = {"healthy": True, "preparations": {family: {"complete": True}
                       for family in ("leaves", "filesystem", "syntax", "config")},
                       "integration": {"prepared": True, "complete": False}, "coverage": {"preparation_complete": False}}

    def test_comparator_controls_are_executed_not_assumed(self):
        self.assertTrue(all(p.comparator_controls().values()))
        with patch.object(p, "require_rows", return_value={"one": {}, "two": {}}):
            controls = p.comparator_controls()
        self.assertFalse(controls["removed"])
        self.assertFalse(controls["duplicate"])

    def test_partial_duplicate_extra_and_old_order_never_pass(self):
        good = [{"case": name, "result": "match"} for name in ("one", "two")]
        for rows in (good[:1], [good[0], good[0]], good + [good[0]], good[::-1]):
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, "exactly once"):
                p.require_rows(rows, ["one", "two"])
        for result in ("not_run", "harness_failed", "approved", "invented"):
            with self.subTest(result=result), self.assertRaises(ValueError):
                p.require_rows([{"case": "one", "result": result}], ["one"])

    def test_unavailable_and_missing_behavior_cannot_be_green(self):
        for result in ("native_unavailable", "not_implemented", "different"):
            report = {"rows": [{"case": "one", "result": result}]}
            with patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
                out = p.aggregate("foundations", {"leaves": report}, self.health)
            self.assertFalse(out["metrics"]["leaves_complete"])
            self.assertTrue(out["metrics"]["leaves_prepared"])
            self.assertFalse(out["metrics"]["filesystem_complete"])
            self.assertFalse(out["metrics"]["integration_complete"])

    def test_empty_report_is_not_vacuous_success(self):
        out = p.aggregate("foundations", {}, self.health)
        for metric in ("leaves_complete", "filesystem_complete", "utilities_complete", "leaves_prepared"):
            self.assertFalse(out["metrics"][metric])
        with self.assertRaises(ValueError):
            p.require_rows([], [])

    def test_config_output_owners_are_exactly_309_with_matchfiles_once(self):
        owners = p.baseline_owners()
        self.assertEqual(len(owners), 309)
        self.assertEqual(sum(family == "filesystem" for family, _ in owners.values()), 142)
        self.assertEqual(len(set(owners.values())), 309)
        self.assertEqual(p.case_manifest_problems(), [])

    def test_removed_required_case_is_detected_before_aggregation(self):
        original = p.capture.load_requests
        def removed(spec):
            value = copy.deepcopy(original(spec))
            if spec == p.capture.FAMILIES["filesystem"]:
                value["requests"] = [r for r in value["requests"] if not r.get("baseline")][1:]
            return value
        with patch.object(p.capture, "load_requests", side_effect=removed), self.assertRaises(ValueError):
            p.baseline_owners()

    def test_config_baselines_do_not_close_supplementary_direct_cases(self):
        with patch.object(p, "baseline_owners", return_value={"out": ("config", "base")}), \
             patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "base"}, {"case": "direct"}]}):
            result = p.aggregate("config", {"config": {"rows": [
                {"case": "base", "result": "match"}, {"case": "direct", "result": "different"}]}}, self.health)
        self.assertEqual(result["tests"], {"out": "pass"})
        self.assertFalse(result["metrics"]["direct_complete"])
        self.assertNotIn("parity", result["metrics"])  # xtask derives this.

    def test_manifest_failure_blocks_every_completion_metric(self):
        self.health["healthy"] = False
        with patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
            result = p.aggregate("foundations", {"leaves": {"rows": [{"case": "one", "result": "match"}]}}, self.health)
        self.assertFalse(any(result["metrics"].values()))

    def test_authenticated_replay_rejects_tampering_and_partial_capture(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "source_closure": {"source.rs": "old"}}))
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={"source.rs": "new"}), \
                 patch.object(p.capture, "compare", return_value={"family": "leaves", "partial": False}) as compare:
                with self.assertRaisesRegex(ValueError, "inputs changed"):
                    p.replay_family("leaves", root)
                compare.assert_called_once()  # Artifact integrity precedes staleness.
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "source_closure": {}}))
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={}), \
                 patch.object(p.capture, "compare", side_effect=ValueError("capture artifact changed")):
                with self.assertRaisesRegex(ValueError, "artifact changed"):
                    p.replay_family("leaves", root)
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={}), \
                 patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}), \
                 patch.object(p.capture, "compare", return_value={"family": "leaves", "partial": True}):
                with self.assertRaisesRegex(ValueError, "complete capture"):
                    p.replay_family("leaves", root)

    def test_wrong_family_is_invalid_even_when_its_sources_are_stale(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps({"family": "config", "source_closure": {"old": "old"}}))
            with patch.object(p.capture, "compare", side_effect=p.capture.StaleCapture("changed source")) as compare:
                with self.assertRaisesRegex(ValueError, "another family") as error:
                    p.replay_family("leaves", root)
                self.assertNotIsInstance(error.exception, p.capture.StaleCapture)
                compare.assert_not_called()
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "partial": True,
                                                              "source_closure": {"old": "old"}}))
            with patch.object(p.capture, "compare", side_effect=p.capture.StaleCapture("changed source")) as compare:
                with self.assertRaisesRegex(ValueError, "complete capture") as error:
                    p.replay_family("leaves", root)
                self.assertNotIsInstance(error.exception, p.capture.StaleCapture)
                compare.assert_not_called()

    def test_program_smoke_or_stale_full_never_passes(self):
        for mode, current in (("smoke", True), ("full", False)):
            with patch.object(p.syntax, "schedule_problems", return_value=[]), \
                 patch.object(p.syntax, "replay", return_value={"selection": mode, "rust_sources_current": current}), \
                 self.assertRaisesRegex(ValueError, "current full"):
                p.replay_program(Path("unused"))

    def test_stale_family_is_unavailable_without_discarding_other_family(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            captures = {family: directory / family for family in ("config", "filesystem")}
            for path in captures.values():
                path.mkdir()
            def replay(family, _):
                if family == "config":
                    raise p.capture.StaleCapture("config production input changed")
                return {"capture_identity": "f" * 64, "rows": [{"case": "one", "result": "match"}]}
            with patch.object(p, "source_closure", return_value={"input": "a" * 64}), \
                 patch.object(p, "harness_check", return_value=self.health), \
                 patch.object(p, "replay_family", side_effect=replay), \
                 patch.object(p, "baseline_owners", return_value={"fs-output": ("filesystem", "one")}), \
                 patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
                result = p.produce("config", captures, directory / "reports", platform_captures=[])
            self.assertEqual(result["tests"], {"fs-output": "pass"})
            self.assertFalse(result["metrics"]["direct_complete"])
            detail = p.read(next((directory / "reports").glob("config-*.json")))
            self.assertEqual(detail["unavailable"], {"config": "config production input changed"})
            self.assertEqual(set(detail["reports"]), {"filesystem"})

    def test_invalid_capture_is_not_downgraded_to_stale(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(p, "source_closure", return_value={"input": "a" * 64}), \
                 patch.object(p, "harness_check", return_value=self.health), \
                 patch.object(p, "replay_family", side_effect=ValueError("changed artifact")), \
                 self.assertRaisesRegex(ValueError, "changed artifact"):
                p.produce("config", {"config": Path(tmp)}, Path(tmp) / "reports")

    def test_scoped_health_does_not_consume_integration_or_live_source_classification(self):
        import phase1_integration
        for producer in ("config", "syntax"):
            with self.subTest(producer=producer), \
                 patch.object(phase1_integration, "check", side_effect=AssertionError("unfingerprinted integration")), \
                 patch.object(p.scope, "build", side_effect=AssertionError("unrelated Rust source audit")):
                health = p.harness_check(producer)
                self.assertEqual(health["integration"], {"prepared": False, "complete": False, "problems": []})


class QualificationTests(unittest.TestCase):
    def test_approved_differences_keep_raw_result_and_reject_additional_drift(self):
        approved = p.qualifications()
        self.assertEqual(len(approved), 5)
        for identity, item in approved.items():
            row = {"case": identity, "result": "different", "native": item["native"], "rust": item["rust"]}
            with self.subTest(case=identity):
                self.assertTrue(p.accepted(row, approved))
                self.assertEqual(row["result"], "different")
                changed = copy.deepcopy(row)
                changed["rust"]["unapproved_extra_field"] = True
                self.assertFalse(p.accepted(changed, approved))
                self.assertFalse(p.accepted({**row, "result": "not_implemented"}, approved))
        package = next(k for k in approved if k.startswith("config/"))
        changed = copy.deepcopy(approved[package]["rust"])
        # The exact-pair policy cannot waive even a harmless-looking addition,
        # let alone losing the separately observed shared contents identity.
        changed["new"] = False
        self.assertFalse(p.accepted({"case": package, "result": "different",
                                    "native": approved[package]["native"], "rust": changed}, approved))

    def test_changed_approval_request_fails_closed(self):
        original = p.capture.load_requests
        def changed(spec):
            value = copy.deepcopy(original(spec))
            for row in value["requests"]:
                row["unreviewed_action"] = True
            return value
        with patch.object(p.capture, "load_requests", side_effect=changed), self.assertRaisesRegex(ValueError, "request changed"):
            p.qualifications()


if __name__ == '__main__':
    unittest.main()
