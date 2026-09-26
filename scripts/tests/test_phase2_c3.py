"""C3 exits require the content-mapper rows to stay blocked, no measurement, and rebound handoffs."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_blockers as blockers  # noqa: E402
import phase2_claims as claims_module  # noqa: E402
import phase2_compare as compare  # noqa: E402
import phase2_producers as producers  # noqa: E402
from test_phase2_c1 import row, comparison  # noqa: E402


class MetricsFixture:
    def setUp(self):
        self.inventory = [{"id": "owned", "checkpoint": "C3"}, {"id": "mapper", "checkpoint": "C3"},
                          {"id": "other", "checkpoint": "C4"}]
        self.addCleanup(patch.stopall)
        patch.object(compare.phase2_inventory, "executed", return_value=self.inventory).start()
        self.baseline = comparison([row("owned", checkpoint="C3"), row("mapper", checkpoint="C3", errors="unsupported"),
                                    row("other", checkpoint="C4")])
        self.current = copy.deepcopy(self.baseline)
        self.handoff = {"owner": "Phase 5", "go": "tsc/internal/contentmapper/transform.go:TransformAndParse",
                        "domains": ["errors"], "blocker": {"kind": "unsupported", "operation": "content-mapper execution"},
                        "reproduce": ["python3", "scripts/phase2_corpus.py", "run", "--case", "mapper"],
                        "trace": {"path": "data/phase2/c3-content-mapper/trace.json", "sha256": "a" * 64},
                        "capture_sha256": "r" * 64, "request_sha256": "q" * 64, "raw_observation_sha256": "o" * 64}
        self.claims = {"version": 1, "pin": "p", "inventory_sha256": "i" * 64, "rust_capture_sha256": "r" * 64,
                       "baseline_sha256": "f" * 64,
                       "rows": [{"id": "mapper", "status": "blocked", "handoff": self.handoff}]}
        self.blockers = {"version": 2, "entries": [{"id": "B04", "kind": "unsupported", "operation": "content-mapper execution",
                                                    "missing_operation": "content-mapper execution", "owner": "Phase 5",
                                                    "variants": ["mapper"], "domains": {"errors": 1},
                                                    "ownership": [{"variant": "mapper", "domain": "errors",
                                                                   "inventory_owner": "C3", "effective_owner": "Phase 5"}]}]}
        self.prerequisites = {name: True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                     "result_recorded", "blockers_named")}

    def metrics(self, **kwargs):
        transfers = kwargs.pop("handoffs", {"mapper": dict(self.handoff)})
        arguments = dict(checkpoint="C3", comparison=self.current, claims=self.claims, audit_ok=True,
                         baseline=self.baseline, contracts_ok=True, regression_parity=1,
                         blockers=self.blockers, baseline_sha256="f" * 64, prerequisites=self.prerequisites,
                         inventory=self.inventory, handoffs=transfers)
        return producers.checkpoint_metrics(**(arguments | kwargs))


class ExitMetrics(MetricsFixture, unittest.TestCase):
    def test_blocked_content_mapper_rows_do_not_count_and_no_measurement_is_required(self):
        metrics = self.metrics()
        self.assertEqual(metrics["c3_open"], 0)
        self.assertEqual(metrics["c3_blockers_open"], 0)
        self.assertNotIn("c3_measured", metrics)
        self.assertTrue(metrics["c3_complete"])
        self.assertNotIn("measurement", producers.CHECKPOINT_AUTHORITIES["C3"])

    def test_blocked_row_needs_a_validated_transfer_and_a_registered_blocker(self):
        self.assertEqual(self.metrics(handoffs={})["c3_open"], 1)
        self.assertFalse(self.metrics(handoffs={})["c3_complete"])
        unregistered = {"version": 2, "entries": []}
        self.assertEqual(self.metrics(blockers=unregistered)["c3_open"], 1)

    def test_a_c3_owned_row_that_differs_counts_whatever_its_label(self):
        self.current = comparison([row("owned", checkpoint="C3", types="different"),
                                   row("mapper", checkpoint="C3", errors="unsupported"), row("other", checkpoint="C4")])
        self.claims["rows"].append({"id": "owned", "status": "closed", "commit": "abcdef1"})
        self.assertEqual(self.metrics()["c3_open"], 1)
        self.assertFalse(self.metrics()["c3_complete"])

    def test_incoming_row_counts_until_it_matches(self):
        self.inventory.append({"id": "given", "checkpoint": "C2"})
        base = [row("owned", checkpoint="C3"), row("mapper", checkpoint="C3", errors="unsupported"),
                row("other", checkpoint="C4")]
        self.baseline = comparison(base + [row("given", checkpoint="C2", errors="different")])
        self.current = comparison(base + [row("given", checkpoint="C2", errors="different")])
        incoming = {"given": {"owner": "C3", "from": "C2", "go": "f", "domains": ["errors"]}}
        self.claims["rows"].append({"id": "given", "status": "open", "incoming": incoming["given"]})
        self.assertEqual(self.metrics(incoming=incoming)["c3_open"], 1)
        self.current = comparison(base + [row("given", checkpoint="C2")])
        self.assertEqual(self.metrics(incoming=incoming)["c3_open"], 0)

    def test_every_prerequisite_and_authority_is_required(self):
        for key in self.prerequisites:
            with self.subTest(key=key):
                self.assertFalse(self.metrics(prerequisites=self.prerequisites | {key: False})["c3_complete"])
        for key in ("claims", "audit_ok", "baseline", "contracts_ok", "blockers"):
            with self.subTest(key=key):
                self.assertFalse(self.metrics(**{key: None})["c3_complete"])


class Rebind(unittest.TestCase):
    def test_rebind_rewrites_current_bindings_and_reports_uncovered_rows(self):
        claims = {"version": 1, "rust_capture_sha256": "r" * 64, "rows": [
            {"id": "a", "status": "blocked", "handoff": {"domains": ["errors"], "trace": {"path": "t", "sha256": None},
                                                         "capture_sha256": "r" * 64, "request_sha256": "x", "raw_observation_sha256": "y"}},
            {"id": "b", "status": "handed", "handoff": {"domains": ["types"], "trace": {"path": "t", "sha256": None},
                                                        "capture_sha256": "r" * 64, "request_sha256": "x", "raw_observation_sha256": "y"}},
            {"id": "c", "status": "closed", "commit": "abcdef1"}]}
        current = comparison([row("a", errors="unsupported"), row("b", errors="different"), row("c")],
                             rust_capture_sha256="n" * 64)
        digests = {vid: {"request_sha256": vid + "req", "raw_observation_sha256": vid + "raw"} for vid in "abc"}
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / "t"
            trace.write_bytes(b"trace")
            for entry in claims["rows"][:2]:
                entry["handoff"]["trace"]["sha256"] = blockers.digest(b"trace")
            stale = claims_module.rebind(claims, current, "n" * 64, digests, root=Path(directory))
        self.assertEqual(stale, ["b"])
        self.assertEqual(claims["rows"][0]["handoff"]["capture_sha256"], "n" * 64)
        self.assertEqual(claims["rows"][0]["handoff"]["request_sha256"], "areq")
        self.assertEqual(claims["rows"][1]["handoff"]["capture_sha256"], "r" * 64)
        # The file's own binding names the checkpoint's start capture and stays.
        self.assertEqual(claims["rust_capture_sha256"], "r" * 64)
        with self.assertRaises(ValueError):
            claims_module.rebind(claims, current, "m" * 64, digests, root=Path("/"))


class RecordedCompletion(unittest.TestCase):
    def test_only_the_newest_checkpoint_computes_completion(self):
        self.assertEqual(producers.newest_checkpoint({"C2": None, "C3": None}), "C3")
        self.assertEqual(producers.newest_checkpoint({"C2": None}), "C2")
        self.assertIsNone(producers.newest_checkpoint({}))
        metrics = {"c2_open": 0, "c2_handoffs": 3, "c2_measured": False, "c2_complete": False, "c3_complete": True}
        producers.drop_historical_completion(metrics, "C2")
        self.assertEqual(metrics, {"c2_open": 0, "c2_handoffs": 3, "c3_complete": True})


class Wiring(unittest.TestCase):
    def test_c3_authorities_are_checker_inputs_and_the_audit_scope_is_bound(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C3"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        for extra in ("data/phase2/receipts/c3-contracts.json", "data/phase2/c3-content-mapper/trace.json"):
            self.assertIn(extra, spec["inputs"])
        self.assertIn("C3", blockers.CHECKPOINT_CLAIMS)
        document = audit.load(ROOT / "data/phase2/c3-audit.json")
        self.assertEqual(document["checkpoint"], "C3")
        problems = audit.problems(document, allow_open=True)
        self.assertEqual(problems, [])
        for group, (count, _) in audit.C3_REVIEWED_GROUPS.items():
            self.assertEqual(len(document["groups"][group]), count)
        altered = copy.deepcopy(document)
        altered["groups"]["C3.2 narrowing and flow (flow.go)"].pop()
        self.assertTrue(audit.problems(altered, allow_open=True))
        later = copy.deepcopy(document)
        name = later["groups"]["C3.3 iteration, async and generators"][0]
        later["dispositions"][name] = {"disposition": "later", "owner": "C2", "reason": "backwards"}
        self.assertTrue(any("later needs an owner" in p for p in audit.problems(later, allow_open=True)))

    def test_committed_c3_claims_hold_every_baseline_open_row_as_blocked(self):
        claims = json.loads((ROOT / "data/phase2/c3-claims.json").read_bytes())
        self.assertEqual({entry["status"] for entry in claims["rows"]}, {"blocked"})
        self.assertEqual(len(claims["rows"]), 15)
        for entry in claims["rows"]:
            handoff = entry["handoff"]
            self.assertEqual(handoff["owner"], "Phase 5")
            self.assertEqual(handoff["blocker"], {"kind": "unsupported", "operation": "content-mapper execution"})
            self.assertEqual(handoff["reproduce"][-1], entry["id"])
            trace = ROOT / handoff["trace"]["path"]
            self.assertEqual(blockers.digest(trace.read_bytes()), handoff["trace"]["sha256"])


if __name__ == "__main__":
    unittest.main()
