"""C5 exits require every incoming row to match, no C5-owned blocker, a current services replay, no measurement."""
import copy
import json
from pathlib import Path
import sys
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_blockers as blockers  # noqa: E402
import phase2_compare as compare  # noqa: E402
import phase2_producers as producers  # noqa: E402
from test_phase2_c1 import row, comparison  # noqa: E402


class MetricsFixture:
    def setUp(self):
        self.inventory = [{"id": "emitted", "checkpoint": "C2"}, {"id": "other", "checkpoint": "C4"}]
        self.addCleanup(patch.stopall)
        patch.object(compare.phase2_inventory, "executed", return_value=self.inventory).start()
        self.baseline = comparison([row("emitted", checkpoint="C2", errors="different"), row("other", checkpoint="C4")])
        self.current = comparison([row("emitted", checkpoint="C2"), row("other", checkpoint="C4")])
        self.incoming = {"emitted": {"owner": "C5", "from": "C2", "domains": ["errors"],
                                     "go": "tsc/internal/checker/emitresolver.go:EmitResolver.GetConstantValue"}}
        self.claims = {"version": 1, "pin": "p", "inventory_sha256": "i" * 64, "rust_capture_sha256": "r" * 64,
                       "baseline_sha256": "f" * 64,
                       "rows": [{"id": "emitted", "status": "open", "incoming": self.incoming["emitted"]}]}
        self.blockers = {"version": 2, "entries": []}
        self.prerequisites = {name: True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                     "result_recorded", "blockers_named")}

    def metrics(self, **kwargs):
        arguments = dict(checkpoint="C5", comparison=self.current, claims=self.claims, audit_ok=True,
                         baseline=self.baseline, contracts_ok=True, regression_parity=1,
                         blockers=self.blockers, baseline_sha256="f" * 64, prerequisites=self.prerequisites,
                         inventory=self.inventory, handoffs={}, incoming=self.incoming, services_ok=True)
        return producers.checkpoint_metrics(**(arguments | kwargs))


class ExitMetrics(MetricsFixture, unittest.TestCase):
    def test_matching_incoming_rows_complete_with_a_current_replay_and_no_measurement(self):
        metrics = self.metrics()
        self.assertEqual(metrics["c5_open"], 0)
        self.assertEqual(metrics["c5_blockers_open"], 0)
        self.assertTrue(metrics["c5_services"])
        self.assertNotIn("c5_measured", metrics)
        self.assertTrue(metrics["c5_complete"])
        self.assertNotIn("measurement", producers.CHECKPOINT_AUTHORITIES["C5"])
        self.assertIn("services", producers.CHECKPOINT_AUTHORITIES["C5"])

    def test_a_differing_incoming_row_keeps_c5_incomplete_whatever_its_label(self):
        self.current = comparison([row("emitted", checkpoint="C2", errors="different"), row("other", checkpoint="C4")])
        self.assertEqual(self.metrics()["c5_open"], 1)
        self.assertFalse(self.metrics()["c5_complete"])
        self.claims["rows"][0]["status"] = "closed"
        self.claims["rows"][0]["commit"] = "abcdef1"
        self.assertEqual(self.metrics()["c5_open"], 1)

    def test_an_incoming_row_needs_its_validated_transfer(self):
        with self.assertRaises(ValueError):
            self.metrics(incoming={})

    def test_a_stale_or_missing_replay_keeps_c5_services_false(self):
        for value in (False, None):
            with self.subTest(services_ok=value):
                metrics = self.metrics(services_ok=value)
                self.assertFalse(metrics["c5_services"])
                self.assertFalse(metrics["c5_complete"])

    def test_a_c5_owned_blocker_left_in_the_register_keeps_it_open(self):
        self.blockers = {"version": 2, "entries": [{"id": "B02", "kind": "emit_order",
                                                    "operation": "post-emit diagnostic order",
                                                    "owner": "C5 emit resolver, with Phase 3 emit (joint)",
                                                    "variants": ["emitted"], "domains": {"errors": 1},
                                                    "ownership": [{"variant": "emitted", "domain": "errors",
                                                                   "inventory_owner": "C2", "effective_owner": "C5"}]}]}
        metrics = self.metrics()
        self.assertEqual(metrics["c5_blockers_open"], 1)
        self.assertFalse(metrics["c5_complete"])

    def test_every_prerequisite_and_authority_is_required(self):
        for key in self.prerequisites:
            with self.subTest(key=key):
                self.assertFalse(self.metrics(prerequisites=self.prerequisites | {key: False})["c5_complete"])
        for key in ("claims", "audit_ok", "baseline", "contracts_ok", "blockers"):
            with self.subTest(key=key):
                self.assertFalse(self.metrics(**{key: None})["c5_complete"])


class RecordedCompletion(unittest.TestCase):
    def test_a_later_run_drops_the_historical_services_metric(self):
        metrics = {"c5_open": 0, "c5_services": True, "c5_complete": True}
        producers.drop_historical_completion(metrics, "C5")
        self.assertEqual(metrics, {"c5_open": 0})

    def test_a_missing_manifest_is_not_a_current_replay(self):
        self.assertFalse(producers.services_current(ROOT / "data/phase2/no-such-manifest.json", {}))


class Wiring(unittest.TestCase):
    def test_c5_authorities_are_checker_inputs_and_the_audit_scope_is_bound(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C5"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        for extra in ("data/phase2/receipts/c5-contracts.json", "data/phase2/receipts/c5-services.json",
                      "data/phase2/services-replay.json.xz"):
            self.assertIn(extra, spec["inputs"])
        self.assertIn("C5", blockers.CHECKPOINT_CLAIMS)
        self.assertEqual(producers.CHECKPOINTS[-1], "C5")
        self.assertIn("c5-contracts", producers.WITNESSES)
        document = audit.load(ROOT / "data/phase2/c5-audit.json")
        self.assertEqual(document["checkpoint"], "C5")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        for group, (count, _) in audit.C5_REVIEWED_GROUPS.items():
            self.assertEqual(len(document["groups"][group]), count)
        self.assertEqual(len(audit.C5_COMPLETE_FILES), 12)
        altered = copy.deepcopy(document)
        altered["groups"]["C5.5 services (services.go)"].pop()
        self.assertTrue(audit.problems(altered, allow_open=True))

    def test_the_committed_claims_hold_the_three_incoming_emit_order_rows(self):
        claims = json.loads((ROOT / "data/phase2/c5-claims.json").read_text())
        self.assertEqual(len(claims["rows"]), 3)
        owned = blockers.audit_owned_functions(audit.load(ROOT / "data/phase2/c5-audit.json"), "C5")
        for entry in claims["rows"]:
            incoming = entry["incoming"]
            self.assertEqual((incoming["owner"], incoming["from"]), ("C5", "C2"))
            self.assertIn(incoming["go"], owned)
            self.assertEqual(incoming["reproduce"][-1], entry["id"])
            self.assertEqual(incoming["blocker"], {"kind": "emit_order", "operation": "post-emit diagnostic order"})
            trace = ROOT / incoming["trace"]["path"]
            self.assertEqual(blockers.digest(trace.read_bytes()), incoming["trace"]["sha256"])


if __name__ == "__main__":
    unittest.main()
