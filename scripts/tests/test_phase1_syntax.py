"""Phase 1 F4a: the syntax request inventory, schedule and case-layer contracts.

The inventory reports zero problems against the committed manifests, which by
itself means nothing. Each test below perturbs one owning manifest the way a
real defect would and requires the validator to name it.
"""

import copy
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import phase1_syntax as syntax  # noqa: E402


class SyntaxInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sources = syntax.load_sources()
        cls.document = syntax.build(cls.sources)

    def perturbed(self):
        return copy.deepcopy(self.sources)

    def check(self, sources):
        return syntax.problems(self.document, sources=sources, check_pin=False)

    def test_the_committed_manifests_account_for_every_request(self):
        self.assertEqual(syntax.problems(), [])

    def test_the_counts_are_the_reviewed_denominators(self):
        counts = self.document["counts"]
        self.assertEqual(counts["physical_cases"], 12721)
        self.assertEqual(counts["libraries"], 108)
        self.assertEqual(counts["primary_requests"], 22343)
        self.assertEqual(counts["option_variants"], 15206)
        self.assertEqual(counts["supplemental_requests"], 135)

    def test_a_dropped_parser_request_is_named(self):
        sources = self.perturbed()
        dropped = sources["requests"].pop(1234)
        found = self.check(sources)
        self.assertTrue(any(dropped["primary"] in problem for problem in found), found)

    def test_a_request_removed_only_from_the_binder_set_is_named(self):
        # The plan: "No E2 eligibility filter may remove a parser request." The
        # binder set must be the parser set, so a filter applied on one side
        # shows up as a difference between them.
        sources = self.perturbed()
        sources["binder_requests"].pop(0)
        found = self.check(sources)
        self.assertTrue(any("binder request set differs" in problem for problem in found), found)

    def test_an_excluded_variant_that_lost_its_requests_is_named(self):
        sources = self.perturbed()
        excluded = next(variant for case in sources["subset"] for variant in case["variants"]
                        if variant["disposition"] == "excluded")
        primary, configuration = excluded["id"].rsplit("#configuration=", 1)
        sources["requests"] = [request for request in sources["requests"]
                               if not (request["primary"] == primary
                                       and request["configuration"] == int(configuration))]
        found = self.check(sources)
        self.assertTrue(any(excluded["id"] in problem or primary in problem for problem in found), found)

    def test_a_unit_excluded_for_an_unnamed_reason_is_named(self):
        sources = self.perturbed()
        case = next(case for case in sources["corpus"] if case.get("units"))
        case["units"][0]["eligibility"] = "quietly_skipped: no reason"
        found = self.check(sources)
        self.assertTrue(any("unnamed reason" in problem for problem in found), found)

    def test_an_invented_request_is_named(self):
        sources = self.perturbed()
        extra = copy.deepcopy(sources["requests"][0])
        extra["id"] = extra["id"] + "-invented"
        extra["configuration"] = 999
        sources["requests"].append(extra)
        sources["binder_requests"].append(copy.deepcopy(extra))
        found = self.check(sources)
        self.assertTrue(any("undeclared" in problem for problem in found), found)

    def test_a_missing_subset_variant_is_named(self):
        sources = self.perturbed()
        sources["subset"][0]["variants"].pop(0)
        found = self.check(sources)
        self.assertTrue(any("subset variants differ" in problem for problem in found), found)

    def test_a_supplemental_id_colliding_with_a_primary_is_named(self):
        sources = self.perturbed()
        sources["fixtures"][0]["request"]["id"] = sources["requests"][0]["id"]
        found = self.check(sources)
        self.assertTrue(any("collide" in problem for problem in found), found)

    def test_historical_evidence_is_linked_as_historical(self):
        for record in self.document["evidence"]:
            if record["state"] != "current":
                self.assertFalse(record["current"], record["producer"])

    def test_a_changed_source_manifest_invalidates_the_inventory(self):
        committed = json.loads(syntax.INVENTORY.read_text())
        committed = copy.deepcopy(committed)
        committed["sources"]["corpus"]["sha256"] = "0" * 64
        found = syntax.problems(committed, check_pin=False)
        self.assertTrue(any("changed" in problem for problem in found), found)


if __name__ == "__main__":
    unittest.main()
