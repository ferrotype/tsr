"""Adversarial checks for the P1 storage-families oracle: the trace is frozen and every
observation class must match between runtimes before a report can claim parity."""
import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s04_common import strict_json_loads
from s08_families import (
    OBSERVATIONS,
    REQUESTS,
    P3_UNPAIRED_FAMILIES,
    P4_UNPAIRED_FAMILIES,
    P5_UNPAIRED_FAMILIES,
    requests,
    validate,
)
from s08_oracle import ROOT


def observation(trace, rust=False):
    """A Rust-shaped observation consistent with the frozen Go one."""
    census = {"families": {"union": {"count": 1, "bytes": 8}}, "types": {"created": 1, "reachable": 1, "unreachable_occupied": 0},
              "unavailable": ["checker_ast"]}
    if rust:
        census["families"].update({name:{"count":0,"bytes":0} for name in P3_UNPAIRED_FAMILIES | P4_UNPAIRED_FAMILIES | P5_UNPAIRED_FAMILIES})
    return {"roots": trace["roots"], "named": trace["named"], "counts": trace["counts"], "prefix_counts": trace["prefix_counts"],
            "census": census, "real_counts": trace["real_counts"]}


class S08Families(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.spec = strict_json_loads((ROOT / REQUESTS).read_bytes())
        cls.frozen = strict_json_loads((ROOT / OBSERVATIONS).read_bytes())

    def test_generator_matches_the_frozen_trace_and_covers_every_family(self):
        self.assertEqual(json.dumps(requests(), sort_keys=True), json.dumps(self.spec, sort_keys=True))
        self.assertEqual([t["options"] for t in self.spec["traces"]], [t["options"] for t in self.frozen["traces"]])
        ops = {action["op"] for trace in self.spec["traces"] for action in trace["actions"]}
        self.assertEqual(ops, {"builtin", "string", "number", "bigint", "fresh", "regular", "union", "union_alias", "symbol", "type_parameter",
                               "tuple_target", "tuple", "reference", "anonymous", "template", "call_signature", "synthetic_expression"})
        for trace in self.frozen["traces"]:
            self.assertEqual(len(trace["named"]), 57)
            self.assertEqual(trace["real_counts"]["types"], trace["prefix_counts"]["types"] + 2)

    def test_matching_observations_validate(self):
        for request, trace in zip(self.spec["traces"], self.frozen["traces"]):
            ours = observation(trace, rust=True)
            validate(request, ours, observation(trace))

    def test_unpaired_families_cannot_disappear_or_become_paired_silently(self):
        request, trace = self.spec["traces"][0], self.frozen["traces"][0]
        go, rust = observation(trace), observation(trace, rust=True)
        for name in P3_UNPAIRED_FAMILIES | P4_UNPAIRED_FAMILIES | P5_UNPAIRED_FAMILIES:
            missing = copy.deepcopy(rust)
            del missing["census"]["families"][name]
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate(request, missing, go)
        with self.assertRaises(ValueError):
            validate(request, rust, rust)

    def test_reordered_missing_or_altered_observations_fail(self):
        request, trace = self.spec["traces"][0], self.frozen["traces"][0]
        go = observation(trace)
        ours = observation(trace, rust=True)
        reordered = copy.deepcopy(ours)
        reordered["roots"][0], reordered["roots"][1] = reordered["roots"][1], reordered["roots"][0]
        with self.assertRaises(ValueError):
            validate(request, reordered, go)
        short = copy.deepcopy(ours)
        short["roots"] = short["roots"][:-1]
        with self.assertRaises(ValueError):
            validate(request, short, short)
        renamed = copy.deepcopy(ours)
        renamed["named"]["stringType"]["id"] += 1
        with self.assertRaises(ValueError):
            validate(request, renamed, go)
        counted = copy.deepcopy(ours)
        counted["prefix_counts"]["types"] -= 1
        with self.assertRaises(ValueError):
            validate(request, counted, go)
        family = copy.deepcopy(ours)
        family["census"]["families"]["extra"] = {"count": 0, "bytes": 0}
        with self.assertRaises(ValueError):
            validate(request, family, go)
        stringly = copy.deepcopy(ours)
        stringly["counts"]["types"] = str(stringly["counts"]["types"])
        with self.assertRaises(ValueError):
            validate(request, stringly, go)


if __name__ == "__main__":
    unittest.main()
