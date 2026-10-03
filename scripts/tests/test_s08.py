"""S08 contracts must preserve explicit obligations and reject incomplete observations."""
import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s08_manifest import eligible, make_manifest, optional_bool, phase_requests, validate_policy


class S08Contracts(unittest.TestCase):
    def acceptance(self, subset):
        return {"amendment":"approved", "variants":[{"id":v["id"],"tier":"acceptance","reasons":[]} for _,v in eligible(subset)]}

    def fixture(self):
        variant = {
            "id": "case/default", "disposition": "eligible", "configuration": [],
            "configured_name": "case", "options": {"declaration": False, "composite": True},
            "harness_options": {"NoTypesAndSymbols": False, "CaptureSuggestions": True},
            "baselines": [], "dependency_closure": [0], "loading_request_sha256": "loading",
        }
        return {"pin": "pin", "file_observations": [{"path": "lib.d.ts", "sha256": "lib"}],
                "cases": [{"id": "case", "source": {"raw_sha256": "source"}, "variants": [variant]}]}

    def test_option_presence_is_not_truth_and_missing_remains_distinct(self):
        self.assertIsNone(optional_bool({}, "declaration"))
        self.assertIs(optional_bool({"declaration": False}, "declaration"), False)
        for value in (0, 1, "false", [], {}):
            with self.assertRaises(ValueError):
                optional_bool({"declaration": value}, "declaration")
        self.assertEqual(phase_requests(eligible(self.fixture())), [
            {"id": "case/default", "declaration": False, "composite": True}])

    def test_missing_duplicate_extra_and_reordered_observations_fail(self):
        requests = [{"id": "a"}, {"id": "b"}]
        rows = [{"id": name, "declaration_requested": True} for name in ("a", "b")]
        self.assertEqual(validate_policy(requests, {"rows": rows}), rows)
        for altered in (rows[:1], rows[::-1], rows + rows[:1], [rows[0], rows[0]],
                        [dict(rows[0], declaration_requested=1), rows[1]],
                        [dict(rows[0], panic="failure"), rows[1]]):
            with self.assertRaises(ValueError):
                validate_policy(requests, {"rows": altered})

    def test_empty_or_duplicate_eligible_inventory_fails(self):
        subset = self.fixture()
        subset["cases"][0]["variants"] *= 2
        with self.assertRaises(ValueError):
            eligible(subset)
        subset["cases"].clear()
        with self.assertRaises(ValueError):
            eligible(subset)

    def test_absent_reference_never_suppresses_a_requested_phase(self):
        subset = self.fixture()
        policy = [{"id": "case/default", "declaration_requested": True}]
        result = make_manifest(subset, eligible(subset), policy, {}, self.acceptance(subset))
        row = result["requests"][0]
        self.assertEqual(row["reference_baselines"], [])
        self.assertIn("declaration", row["diagnostic_phases"])
        self.assertIn("suggestion", row["diagnostic_phases"])
        self.assertTrue(row["type_baseline_requested"])
        self.assertFalse(result["result_contract"]["reference_absence_proves_no_content"])
        self.assertFalse(result["result_contract"]["not_implemented_passes"])
        subset["cases"][0]["variants"][0]["harness_options"]["NoTypesAndSymbols"] = True
        disabled = make_manifest(subset, eligible(subset), policy, {}, self.acceptance(subset))
        self.assertFalse(disabled["requests"][0]["type_baseline_requested"])
        self.assertEqual(disabled["counts"]["type_baseline_requested"], 0)

    def test_dependencies_and_options_are_bound_and_wrong_join_fails(self):
        subset = self.fixture()
        policy = [{"id": "case/default", "declaration_requested": True}]
        before = make_manifest(subset, eligible(subset), policy, {}, self.acceptance(subset))["requests"][0]
        changed = copy.deepcopy(subset)
        changed["file_observations"][0]["sha256"] = "different"
        after = make_manifest(changed, eligible(changed), policy, {}, self.acceptance(changed))["requests"][0]
        self.assertNotEqual(before["dependency_closure_sha256"], after["dependency_closure_sha256"])
        with self.assertRaises(ValueError):
            make_manifest(subset, eligible(subset), [dict(policy[0], id="wrong")], {}, self.acceptance(subset))

    def test_informational_phase_requests_do_not_enter_acceptance_counts(self):
        subset=self.fixture()
        variant=copy.deepcopy(subset["cases"][0]["variants"][0])
        variant["id"]="case/informational"
        subset["cases"][0]["variants"].append(variant)
        acceptance=self.acceptance(subset)
        acceptance["variants"][1].update(tier="informational",reasons=["native_option_guard"])
        policy=[{"id":v["id"],"declaration_requested":True} for _,v in eligible(subset)]
        manifest=make_manifest(subset,eligible(subset),policy,{},acceptance)
        self.assertEqual(manifest["counts"]["variants"],2)
        self.assertEqual(manifest["counts"]["acceptance"],1)
        self.assertEqual(manifest["counts"]["informational"],1)
        self.assertEqual(manifest["counts"]["acceptance_diagnostic_phases"]["declaration"],1)
        self.assertFalse(manifest["result_contract"]["informational_outcomes_affect_e2"])

