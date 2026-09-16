"""The bounded census check must reject incomparable or incomplete observations."""
import copy
import unittest
from unittest.mock import patch

import s08_census as census
import s08_measurement as measurement


class CensusValidationTests(unittest.TestCase):
    def rows(self):
        row = {"id": "case", "outcome": "executed", "output_sha256": "a" * 64,
               "actions": {"GetTypeAtLocation": 1}, "interval_ns": 1,
               "allocation": {"requested_bytes": 16, "live_before_interval": 0,
                              "live_at_checkpoint": 16, "live_after_release": 0},
               "checkpoint": {"census": {"type_storage_bytes": 16, "checker_bytes": 16,
                                         "types": {"reachable": 1, "created": 1}, "unavailable": [],
                                         "families": {"type_records": {"count": 1, "bytes": 16}}}}}
        result = {r: [copy.deepcopy(row)] for r in ("rust", "go")}
        self.totals(result)
        return result

    def totals(self, rows):
        for runtime in ("rust", "go"):
            rows[runtime + "_totals"] = measurement.checker_rows(rows[runtime], ["case"], "alloc")

    def test_comparable_rows(self):
        census.validate_workload(self.rows(), ["case"])

    def test_different_outputs_or_action_schedules_are_rejected(self):
        for field, value in (("output_sha256", "b" * 64), ("actions", {"GetTypeAtLocation": 2})):
            with self.subTest(field=field):
                rows = self.rows()
                rows["go"][0][field] = value
                self.totals(rows)
                with self.assertRaisesRegex(ValueError, "cross-runtime"):
                    census.validate_workload(rows, ["case"])

    def test_inventory_and_totals_are_checked(self):
        rows = self.rows()
        with self.assertRaisesRegex(ValueError, "inventory"):
            census.validate_workload(rows, ["case", "omitted"])
        rows["go_totals"]["census"]["type_storage_bytes"] += 1
        with self.assertRaisesRegex(ValueError, "raw observations"):
            census.validate_workload(rows, ["case"])

    def test_non_type_family_omission_is_rejected(self):
        rows = self.rows()
        rows["rust"][0]["checkpoint"]["census"]["families"]["display_ast"] = {"count": 0, "bytes": 0}
        with self.assertRaisesRegex(ValueError, "family inventory"):
            census.validate_workload(rows, ["case"])

    def test_failed_validation_returns_nonzero(self):
        report = {"pass": False, "problems": ["unavailable"], "family_inventory": {}, "cases": []}
        with patch("sys.argv", ["s08_census.py", "fixtures"]), patch.object(census, "fixtures", return_value=report), patch("builtins.print"):
            self.assertEqual(census.main(), 1)


if __name__ == "__main__":
    unittest.main()
