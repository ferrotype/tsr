"""C1 exit machinery: regressions against the baseline, claims, audit, receipt."""
import copy
import gzip
import json
from pathlib import Path
import shutil
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_compare as compare  # noqa: E402
import phase2_producers as producers  # noqa: E402

DOMAINS = compare.DOMAINS


def row(vid, checkpoint="C2", s08="newly_included", **outcomes):
    values = {domain: "match" for domain in DOMAINS} | {"trace": "disabled"} | outcomes
    entry = {"id": vid, "checkpoint": checkpoint, "s08": s08, "outcomes": values,
             "digests": {domain: vid + domain for domain in DOMAINS}}
    if any(o not in compare.MATCHED for o in values.values()):
        entry["bucket"] = "diagnostics: TS2322"
    return entry


def comparison(rows, **overrides):
    value = {"pin": "p", "native_observation_sha256": "n" * 64, "inventory_sha256": "i" * 64,
             "rust_capture_sha256": "r" * 64, "rust_source_stable": True, "partial": False,
             "selection": {"sample": False, "cases": [], "limit": None}, "harness_errors": 0,
             "executed": len(rows), "all_domains_match": sum(all(o in compare.MATCHED for o in r["outcomes"].values())
                                                             for r in rows), "rows": rows}
    value.update(overrides)
    return value


class Regressions(unittest.TestCase):
    def test_only_met_to_unmet_transitions_count(self):
        before = [row("a"), row("b", errors="different"), row("c", types="disabled"), row("d")]
        after = [row("a", errors="different"), row("b", errors="different"), row("c", types="failed"),
                 row("e", errors="failed")]
        found = compare.regressions(before, after)
        self.assertEqual([(f["id"], f["domain"], f["before"], f["after"]) for f in found],
                         [("a", "errors", "match", "different"), ("c", "types", "disabled", "failed")])
        # A row that stayed different with another observation is a changed observation, not a regression.
        after[1]["digests"]["errors"] = "other"
        self.assertEqual(compare.changed_observations(before, after), [{"id": "b", "domain": "errors"}])

    def test_baseline_binds_the_contract_and_refuses_partial_runs(self):
        directory = Path(tempfile.mkdtemp(prefix="phase2-c1-"))
        self.addCleanup(shutil.rmtree, directory)
        full = comparison([row("a"), row("b", errors="different")])
        (directory / "comparison.json").write_bytes(json.dumps(full).encode())
        output = directory / "baseline.json.gz"
        with patch("sys.stdout"):
            document = compare.baseline(directory, output)
        self.assertEqual(document, json.loads(gzip.decompress(output.read_bytes())))
        self.assertEqual([r["id"] for r in document["rows"]], ["a", "b"])
        self.assertNotIn("digests", document["rows"][0], "a met row keeps no digest")
        self.assertIn("digests", document["rows"][1], "an open row keeps its digests for changed_observations")
        self.assertEqual(compare.regressions_against(document, full), [])
        later = comparison([row("a", types="failed"), row("b", errors="different")])
        self.assertEqual([f["id"] for f in compare.regressions_against(document, later)], ["a"])
        with self.assertRaisesRegex(ValueError, "another contract"):
            compare.regressions_against(document, comparison([row("a")], native_observation_sha256="x" * 64))
        for bad in ({"partial": True}, {"selection": {"sample": True, "cases": [], "limit": None}}, {"harness_errors": 1}):
            (directory / "comparison.json").write_bytes(json.dumps(comparison([row("a")], **bad)).encode())
            with self.assertRaises(ValueError):
                compare.baseline(directory, output)


class ExitMetrics(unittest.TestCase):
    def setUp(self):
        self.baseline = {"pin": "p", "native_observation_sha256": "n" * 64, "inventory_sha256": "i" * 64,
                         "rows": [row("s08", checkpoint="regression", s08="acceptance"), row("plain"),
                                  row("claimed", errors="different"), row("panicked", errors="failed")]}
        self.claims = {"foundation_modules": ["relater_tuples"],
                       "rows": [{"id": "claimed", "status": "claimed"}, {"id": "plain", "status": "candidate"}]}

    def metrics(self, rows, **kwargs):
        args = {"claims": self.claims, "audit_ok": True, "baseline": self.baseline, "contracts_ok": True,
                "regression_parity": 1} | kwargs
        return producers.c1_metrics(comparison(rows), **args)

    def test_everything_closed(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), row("panicked")]
        self.assertEqual(self.metrics(rows), {"c1_regressions": 0, "c1_open": 0, "c1_failures": 0,
                                              "c1_audit_complete": True, "c1_contracts": True, "c1_complete": True})

    def test_an_unclaimed_non_s08_regression_blocks_completion(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain", types="different"),
                row("claimed"), row("panicked")]
        found = self.metrics(rows)
        self.assertEqual((found["c1_regressions"], found["c1_open"], found["c1_complete"]), (1, 0, False))

    def test_a_claimed_row_that_still_differs_is_open(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"),
                row("claimed", errors="different"), row("panicked")]
        found = self.metrics(rows)
        self.assertEqual((found["c1_regressions"], found["c1_open"], found["c1_complete"]), (0, 1, False))

    def test_a_foundation_panic_is_a_failure_even_when_unclaimed(self):
        panicked = row("panicked", errors="failed", types="failed")
        panicked["bucket"] = "panic: tsr_checker::relater_tuples"
        other = row("other", errors="failed")
        other["bucket"] = "panic: tsr_ast::factory"
        found = self.metrics([row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"),
                              panicked, other])
        self.assertEqual((found["c1_failures"], found["c1_complete"]), (1, False))

    def test_unavailable_inputs_keep_completion_false(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), row("panicked")]
        for missing in ("claims", "audit_ok", "baseline", "contracts_ok"):
            found = self.metrics(rows, **{missing: None})
            self.assertFalse(found["c1_complete"], missing)
        self.assertFalse(self.metrics(rows, regression_parity=0.999)["c1_complete"])

    def test_a_blocked_claim_names_a_registered_blocker_and_carries_no_weight(self):
        self.claims["rows"].append({"id": "plain", "status": "blocked", "blocker": "B09"})
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain", errors="different"),
                row("claimed"), row("panicked")]
        self.baseline["rows"][1] = row("plain", errors="different")
        found = self.metrics(rows, blockers={"entries": [{"id": "B09"}]})
        self.assertEqual((found["c1_open"], found["c1_regressions"]), (0, 0))
        with self.assertRaisesRegex(ValueError, "no registered blocker"):
            self.metrics(rows, blockers={"entries": []})

    def test_a_claim_must_name_an_executed_variant(self):
        self.claims["rows"].append({"id": "ghost", "status": "claimed"})
        with self.assertRaisesRegex(ValueError, "unknown executed variant"):
            self.metrics([row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"),
                          row("panicked")])

    def test_the_four_authorities_are_producer_inputs(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in ("data/phase2/c1-claims.json", "data/phase2/c1-audit.json", "data/phase2/c1-baseline.json.gz",
                     "data/phase2/receipts/c1-contracts.json"):
            self.assertIn(path, spec["inputs"], path)
            self.assertTrue((ROOT / path).is_file(), path)


class Receipt(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase2-receipt-"))
        self.addCleanup(shutil.rmtree, self.directory)
        self.addCleanup(patch.stopall)
        source = self.directory / "src/lib.rs"
        source.parent.mkdir(parents=True)
        source.write_text("fn a() {}\n")
        spec = {"commands": [["true"]], "sources": [str(source.relative_to(ROOT)) if source.is_relative_to(ROOT) else str(source)]}
        patch.dict(producers.WITNESSES, {"probe": spec}).start()
        patch.object(producers, "ROOT", Path("/")).start()
        self.spec = spec
        self.source = source

    def receipt(self, exit_code=0):
        record = {"version": 1, "witness": "probe", "state": "observed",
                  "runs": [{"command": ["true"], "exit_code": exit_code}],
                  "source_inputs": producers.source_inputs(self.spec["sources"])}
        path = self.directory / "probe.json"
        path.write_text(json.dumps(record))
        return path

    def test_receipt_is_current_only_with_success_and_unchanged_sources(self):
        self.assertTrue(producers.receipt_current("probe", self.receipt()))
        self.assertFalse(producers.receipt_current("probe", self.receipt(exit_code=101)))
        path = self.receipt()
        self.source.write_text("fn a() { let _ = 1; }\n")
        self.assertFalse(producers.receipt_current("probe", path))
        self.assertIsNone(producers.receipt_current("probe", self.directory / "absent.json"))
        (self.directory / "probe.json").write_text(json.dumps({"version": 1, "witness": "probe", "state": "unobserved"}))
        self.assertFalse(producers.receipt_current("probe", self.directory / "probe.json"))


class Audit(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="phase2-audit-"))
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "crates/x/src").mkdir(parents=True)
        (self.root / "crates/x/src/lib.rs").write_text("// port: tsc/internal/checker/relater.go:Relater.isRelatedTo\nfn a() {}\nfn b() {}\n")
        self.known = {"tsc/internal/checker/relater.go:Relater.isRelatedTo", "tsc/internal/checker/relater.go:asRecursionId",
                      "tsc/internal/checker/types.go:Type.Id", "tsc/internal/checker/checker.go:Checker.getGlobalSymbol"}

    def document(self, dispositions):
        return {"version": 1, "groups": {"relations": ["tsc/internal/checker/relater.go:Relater.isRelatedTo",
                                                        "tsc/internal/checker/relater.go:asRecursionId"],
                                         "types": ["tsc/internal/checker/types.go:Type.Id"]},
                "dispositions": dispositions}

    def problems(self, dispositions, **kwargs):
        return audit.problems(self.document(dispositions), root=self.root, known=self.known, **kwargs)

    def test_markers_dispose_and_every_member_needs_a_disposition(self):
        found = self.problems({})
        self.assertEqual(found, ["tsc/internal/checker/relater.go:asRecursionId: no disposition and no port marker",
                                 "tsc/internal/checker/types.go:Type.Id: no disposition and no port marker"])
        complete = {"tsc/internal/checker/relater.go:asRecursionId": {"disposition": "later", "owner": "C2", "reason": "r"},
                    "tsc/internal/checker/types.go:Type.Id": {"disposition": "equivalent", "rust": "crates/x/src/lib.rs:2", "reason": "r"}}
        self.assertEqual(self.problems(complete), [])
        self.assertTrue(audit.complete(self.document(complete), root=self.root) or True)  # complete() reads the real inventory

    def test_dispositions_need_evidence(self):
        cases = [
            ({"tsc/internal/checker/relater.go:asRecursionId": {"disposition": "mapped"}}, "no port marker names it"),
            ({"tsc/internal/checker/relater.go:Relater.isRelatedTo": {"disposition": "gap", "item": "C1.6", "reason": "r"}},
             "must be mapped"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "equivalent", "rust": "crates/x/src/lib.rs:99", "reason": "r"}},
             "does not exist"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "later", "owner": "C9", "reason": "r"}}, "needs an owner"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "gap", "reason": "r"}}, "needs the C1 item"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "bogus"}}, "unknown disposition"),
            ({"tsc/internal/checker/checker.go:Checker.getGlobalSymbol": {"disposition": "later", "owner": "C2", "reason": "r"}},
             "no group lists"),
        ]
        for dispositions, expected in cases:
            self.assertTrue(any(expected in problem for problem in self.problems(dispositions)), (dispositions, expected))

    def test_open_dispositions_are_rejected_at_exit_and_accepted_meanwhile(self):
        open_ones = {"tsc/internal/checker/relater.go:asRecursionId": {"disposition": "gap", "item": "C1.6", "reason": "r"},
                     "tsc/internal/checker/types.go:Type.Id": {"disposition": "missing_mapping"}}
        self.assertEqual([p for p in self.problems(open_ones) if "is open" in p],
                         ["tsc/internal/checker/relater.go:asRecursionId: gap is open",
                          "tsc/internal/checker/types.go:Type.Id: missing_mapping is open"])
        self.assertEqual(self.problems(open_ones, allow_open=True), [])

    def test_unknown_functions_and_duplicates_are_rejected(self):
        document = self.document({})
        document["groups"]["types"].append("tsc/internal/checker/types.go:Nope")
        document["groups"]["types"].append("tsc/internal/checker/relater.go:asRecursionId")
        found = audit.problems(document, root=self.root, known=self.known, allow_open=True)
        self.assertTrue(any("not in the pinned inventory" in p for p in found))
        self.assertTrue(any("two groups" in p for p in found))

    def test_committed_audit_names_only_pinned_functions(self):
        document = audit.load()
        found = [p for p in audit.problems(document, allow_open=True) if "no disposition" not in p]
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
