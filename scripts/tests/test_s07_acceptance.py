"""An informational failure cannot certify or fail E2 acceptance."""
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from s07_acceptance import classify, counts, policy_observation, requests_for, select_acceptance
from s08_oracle import canonical,digest


class AcceptanceBoundaryTests(unittest.TestCase):
    def test_frozen_policy_excludes_host_provenance(self):
        _,facts=self.fixture()
        for goos,goarch in (("darwin","arm64"),("linux","amd64"),("linux","arm64"),("darwin","amd64")):
            report={**facts,"go":"go1.27.1","goos":goos,"goarch":goarch}
            self.assertEqual(policy_observation(report),facts)
            self.assertEqual(report["goos"],goos)

    def fixture(self):
        rows=[({"source":{"path":f"compiler/{i}.ts"}},
               {"id":str(i),"options":{},"option_outcome":"accepted","option_diagnostics":[]}) for i in range(4)]
        rows[3][1].update(option_outcome="rejected",option_diagnostics=[{"code":6046}])
        observed=[{"id":str(i),"filename_skip":i in (2,3),"option_guard":"skipped" if i in (1,3) else "allowed"} for i in range(4)]
        return rows,{"request_sha256":digest(canonical(requests_for(rows))+b"\n"),"rows":observed}

    def test_union_preserves_overlapping_reasons_without_double_counting(self):
        variants=classify(*self.fixture())
        self.assertEqual(counts(variants),{"source_variants":4,"acceptance":1,"informational":3,
            "reason_memberships":{"rejected_options":1,"native_option_guard":2,"native_filename_skip":2}})
        self.assertEqual(variants[3]["reasons"],["rejected_options","native_option_guard","native_filename_skip"])

    def test_unknown_fatal_partial_reordered_and_malformed_policy_fail(self):
        for change in ("digest","missing","reordered","fatal","truthiness","diagnostics"):
            rows,report=self.fixture()
            if change=="digest":report["request_sha256"]="changed"
            if change=="missing":report["rows"].pop()
            if change=="reordered":report["rows"].reverse()
            if change=="fatal":report["rows"][0]["option_guard"]="failed"
            if change=="truthiness":report["rows"][0]["filename_skip"]=1
            if change=="diagnostics":rows[3][1]["option_diagnostics"]=[]
            with self.subTest(change=change),self.assertRaises(ValueError):classify(rows,report)

    def test_informational_failure_or_absence_never_affects_acceptance(self):
        variants=classify(*self.fixture())
        accepted={"id":"0","state":"content","text":"observed"}
        for state in ("content","mismatch","panic","not_executed","failed"):
            rows=[accepted]+[{"id":str(i),"state":state} for i in range(1,4)]
            self.assertEqual(select_acceptance(rows,variants),[accepted])
        self.assertEqual(select_acceptance([accepted],variants),[accepted])
        with self.assertRaises(ValueError):select_acceptance([{"id":"1","state":"pass"}],variants)

    def test_informational_rows_cannot_replace_missing_or_reordered_acceptance(self):
        variants=classify(*self.fixture())
        variants[1].update(tier="acceptance",reasons=[])
        for results in ([{"id":"0"},{"id":"2"}], [{"id":"1"},{"id":"0"}], [{"id":"0"}]*2, [{"id":"unknown"}]):
            with self.assertRaises(ValueError):select_acceptance(results,variants)
        with self.assertRaises(ValueError):select_acceptance([], [dict(v,tier="informational") for v in variants])
        variants[0]["tier"]="future_tier"
        with self.assertRaises(ValueError):select_acceptance([],variants)
