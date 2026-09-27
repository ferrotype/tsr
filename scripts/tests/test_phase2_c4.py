"""C4 exits require every C4-owned row to match or be handed with a trace, no C4-owned blocker, no measurement."""
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
        self.inventory = [{"id": "jsx", "checkpoint": "C4"}, {"id": "decorated", "checkpoint": "C4"},
                          {"id": "other", "checkpoint": "C5"}]
        self.addCleanup(patch.stopall)
        patch.object(compare.phase2_inventory, "executed", return_value=self.inventory).start()
        self.baseline = comparison([row("jsx", checkpoint="C4", errors="unsupported"),
                                    row("decorated", checkpoint="C4", errors="unsupported"),
                                    row("other", checkpoint="C5")])
        self.current = comparison([row("jsx", checkpoint="C4"), row("decorated", checkpoint="C4"),
                                   row("other", checkpoint="C5")])
        self.claims = {"version": 1, "pin": "p", "inventory_sha256": "i" * 64, "rust_capture_sha256": "r" * 64,
                       "baseline_sha256": "f" * 64,
                       "rows": [{"id": "jsx", "status": "open"}, {"id": "decorated", "status": "open"}]}
        self.blockers = {"version": 2, "entries": []}
        self.prerequisites = {name: True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                     "result_recorded", "blockers_named")}

    def metrics(self, **kwargs):
        arguments = dict(checkpoint="C4", comparison=self.current, claims=self.claims, audit_ok=True,
                         baseline=self.baseline, contracts_ok=True, regression_parity=1,
                         blockers=self.blockers, baseline_sha256="f" * 64, prerequisites=self.prerequisites,
                         inventory=self.inventory, handoffs=kwargs.pop("handoffs", {}))
        return producers.checkpoint_metrics(**(arguments | kwargs))


class ExitMetrics(MetricsFixture, unittest.TestCase):
    def test_matching_rows_complete_without_a_measurement(self):
        metrics = self.metrics()
        self.assertEqual(metrics["c4_open"], 0)
        self.assertEqual(metrics["c4_blockers_open"], 0)
        self.assertNotIn("c4_measured", metrics)
        self.assertTrue(metrics["c4_complete"])
        self.assertNotIn("measurement", producers.CHECKPOINT_AUTHORITIES["C4"])

    def test_an_open_row_keeps_c4_incomplete_whatever_its_label(self):
        self.current = comparison([row("jsx", checkpoint="C4", errors="unsupported"),
                                   row("decorated", checkpoint="C4"), row("other", checkpoint="C5")])
        self.assertEqual(self.metrics()["c4_open"], 1)
        self.assertFalse(self.metrics()["c4_complete"])
        self.claims["rows"][0] = {"id": "jsx", "status": "closed", "commit": "abcdef1"}
        self.assertEqual(self.metrics()["c4_open"], 1)

    def test_a_handed_row_needs_its_validated_transfer(self):
        self.current = comparison([row("jsx", checkpoint="C4", types="different"),
                                   row("decorated", checkpoint="C4"), row("other", checkpoint="C5")])
        self.claims["rows"][0] = {"id": "jsx", "status": "handed", "handoff": {"owner": "C5", "domains": ["types"]}}
        self.assertEqual(self.metrics(handoffs={})["c4_open"], 1)
        self.assertFalse(self.metrics(handoffs={})["c4_complete"])

    def test_a_c4_owned_blocker_left_in_the_register_keeps_it_open(self):
        self.blockers = {"version": 2, "entries": [{"id": "B01", "kind": "unsupported", "operation": "checkExpressionWorker",
                                                    "missing_operation": "checkExpressionWorker", "owner": "C4",
                                                    "variants": ["jsx"], "domains": {"errors": 1},
                                                    "ownership": [{"variant": "jsx", "domain": "errors",
                                                                   "inventory_owner": "C4", "effective_owner": "C4"}]}]}
        self.current = comparison([row("jsx", checkpoint="C4", errors="unsupported"),
                                   row("decorated", checkpoint="C4"), row("other", checkpoint="C5")])
        metrics = self.metrics()
        self.assertGreater(metrics["c4_blockers_open"], 0)
        self.assertFalse(metrics["c4_complete"])

    def test_every_prerequisite_and_authority_is_required(self):
        for key in self.prerequisites:
            with self.subTest(key=key):
                self.assertFalse(self.metrics(prerequisites=self.prerequisites | {key: False})["c4_complete"])
        for key in ("claims", "audit_ok", "baseline", "contracts_ok", "blockers"):
            with self.subTest(key=key):
                self.assertFalse(self.metrics(**{key: None})["c4_complete"])


class Wiring(unittest.TestCase):
    def test_c4_authorities_are_checker_inputs_and_the_audit_scope_is_bound(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C4"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        self.assertIn("data/phase2/receipts/c4-contracts.json", spec["inputs"])
        self.assertIn("C4", blockers.CHECKPOINT_CLAIMS)
        self.assertIn("C4", producers.CHECKPOINTS)
        self.assertIn("c4-contracts", producers.WITNESSES)
        document = audit.load(ROOT / "data/phase2/c4-audit.json")
        self.assertEqual(document["checkpoint"], "C4")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        for group, (count, _) in audit.C4_REVIEWED_GROUPS.items():
            self.assertEqual(len(document["groups"][group]), count)
        self.assertEqual(len(document["groups"]["C4.2-C4.5 JSX (jsx.go)"]), 59)
        altered = copy.deepcopy(document)
        altered["groups"]["C4.2-C4.5 JSX (jsx.go)"].pop()
        self.assertTrue(audit.problems(altered, allow_open=True))

    def test_the_committed_claims_name_every_open_c4_row_once(self):
        claims = json.loads((ROOT / "data/phase2/c4-claims.json").read_text())
        ids = [entry["id"] for entry in claims["rows"]]
        self.assertEqual(len(ids), len(set(ids)))
        baseline = compare.load_baseline(ROOT / "data/phase2/c4-baseline.json.gz")
        owners = {entry["id"]: entry["checkpoint"] for entry in compare.phase2_inventory.executed()}
        open_rows = {row["id"] for row in baseline["rows"] if owners.get(row["id"]) == "C4"
                     and any(outcome not in compare.MATCHED for outcome in row["outcomes"].values())}
        self.assertEqual(set(ids), open_rows)


if __name__ == "__main__":
    unittest.main()
