"""Counterexamples for CP0 accounting; no benchmark acceptance is tested here."""

import copy
import itertools
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout_model as model


def report(workers=1):
    return {"workers": workers,
            "pre_pipeline": {"live_requested_bytes": 20, "total_requested_bytes": 70},
            "retained_endpoint": {"live_requested_bytes": 120, "total_requested_bytes": 210},
            "pipeline_allocated_bytes": 140, "pipeline_live_growth_bytes": 100,
            "pipeline_superseded_or_freed_requested_bytes": 40,
            "elapsed_worker_totals": {
                "parse_allocated_bytes": 80, "parse_live_growth_bytes": 60,
                "bind_allocated_bytes": 50, "bind_live_growth_bytes": 35,
                "publish_allocated_bytes": 8, "publish_live_growth_bytes": 3}}


class WeightedLayoutTests(unittest.TestCase):
    def test_overlay_bounds_match_independent_exhaustive_assignment(self):
        counts, sizes = {"a": 2, "b": 3, "token": 4}, {"a": 12, "b": 4, "token": 0}
        for removed in range(10):
            allowed = [sum((counts[name] - value) * sizes[name] for name, value in zip(counts, combination))
                       for combination in itertools.product(*(range(count + 1) for count in counts.values()))
                       if sum(combination) == removed]
            actual = model.weighted_bounds(counts, sizes, removed)
            self.assertEqual((actual["lower"], actual["upper"]), (min(allowed), max(allowed)))

    def test_overlay_count_must_fit_inventory(self):
        for invalid in [-1, 3]:
            with self.assertRaisesRegex(ValueError, "overlay count"):
                model.weighted_bounds({"token": 2}, {"token": 0}, invalid)

    def test_actual_generated_shape_inventory_includes_empty_and_rare_types(self):
        source = (model.ROOT / "crates/tsr_ast/src/data_generated.rs").read_text()
        schema = json.loads((model.ROOT / "upstream/tools/scripts/tsc/ast.json").read_text())
        shapes = model.schema_shapes(source, schema)
        self.assertEqual(set(shapes), set(schema["nodes"]["definitions"]))
        self.assertEqual(shapes["Token"]["fields"], [])
        self.assertEqual(shapes["Identifier"]["fields"], [("text", "JsString")])
        self.assertEqual(shapes["Identifier"]["binding"], ["FlowNode"])
        self.assertEqual(shapes["SyntheticExpression"]["unknown"], [{"field": "Type", "type": "any"}])

    def test_unknown_type_and_missing_shape_cannot_silently_cost_zero(self):
        schema = {"nodes": {"definitions": {"Token": {}}}}
        with self.assertRaisesRegex(ValueError, "unsupported generated type"):
            model.schema_shapes("pub struct TokenData {\n    pub value: Vec<u8>,\n}", schema)
        with self.assertRaisesRegex(ValueError, "shape mismatch"):
            model.schema_shapes("", schema)


class TrafficTests(unittest.TestCase):
    def test_starting_live_is_not_pipeline_allocation(self):
        actual = model.traffic_model(report())
        self.assertEqual(actual["pipeline_requests"], 140)
        self.assertEqual(actual["freed_or_superseded_requests"], 40)
        self.assertEqual(actual["maximum_traffic_at_live_and_request_ceilings"], 200_000_020)
        self.assertEqual(actual["outside_phase_signed_remainder"], {"requests": 2, "live_growth": 2})

    def test_eight_worker_phase_zeroes_are_unavailable_not_measurements(self):
        value = report(8)
        del value["elapsed_worker_totals"]
        self.assertIsNone(model.traffic_model(value)["phase_observations"])

    def test_changed_start_recomputes_independent_request_constraint(self):
        value = report()
        value["pre_pipeline"]["live_requested_bytes"] = 30
        value["pipeline_live_growth_bytes"] = 90
        value["pipeline_superseded_or_freed_requested_bytes"] = 50
        actual = model.traffic_model(value)
        self.assertEqual(actual["maximum_traffic_at_live_and_request_ceilings"], 200_000_030)
        self.assertEqual(actual["pipeline_requests_at_1700mb_live_350mb_traffic"], 2_049_999_970)

    def test_corrupt_identity_and_boolean_counter_rejected(self):
        broken = report()
        broken["pipeline_allocated_bytes"] += 1
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            model.traffic_model(broken)
        broken = report()
        broken["pre_pipeline"]["live_requested_bytes"] = True
        with self.assertRaisesRegex(ValueError, "invalid native counter"):
            model.traffic_model(broken)

    def test_duplicate_and_nonfinite_json_rejected(self):
        for data in [b'{"x": 1, "x": 2}', b'{"x": NaN}']:
            with self.assertRaises(ValueError):
                model.strict_json(data)


if __name__ == "__main__":
    unittest.main()
