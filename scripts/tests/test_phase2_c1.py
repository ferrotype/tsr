"""C1 exit machinery: regressions against the baseline, claims, audit, receipt."""
import copy
import gzip
import json
from pathlib import Path
import shutil
import subprocess
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
from phase2_fixtures import build_capture, load as load_fixture, CONTROL  # noqa: E402

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
    def setUp(self):
        self.addCleanup(patch.stopall)
        patch.object(compare.phase2_inventory, "executed", return_value=[{"id": "a"}, {"id": "b"}]).start()

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
        with patch("sys.stdout"), patch.object(compare, "report", return_value=dict(full, rust_source_stable=False)) as replay:
            document = compare.baseline(directory, output)
        replay.assert_called_once_with(ROOT / "target/phase2/native", directory, write=False)
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

    def test_baseline_requires_capture_replay_and_rejects_invented_outcomes(self):
        with tempfile.TemporaryDirectory(prefix="phase2-c1-") as name:
            directory = Path(name)
            full = comparison([row("a"), row("b", errors="different")])
            (directory / "comparison.json").write_text(json.dumps(full))
            with patch.object(compare, "report", side_effect=ValueError("raw case artifact changed")):
                with self.assertRaisesRegex(ValueError, "raw case artifact changed"):
                    compare.baseline(directory, directory / "baseline.json.gz")
            with patch.object(compare, "report", return_value=comparison([row("a", errors="failed"), row("b")])):
                with self.assertRaisesRegex(ValueError, "authenticated Rust capture replay"):
                    compare.baseline(directory, directory / "baseline.json.gz")

    def test_baseline_replay_rejects_missing_duplicate_extra_rows_and_invalid_domains(self):
        baseline = comparison([row("a"), row("b")])
        mutations = [comparison([row("a")]), comparison([row("a"), row("a")]),
                     comparison([row("a"), row("b"), row("extra")]), comparison([row("b"), row("a")])]
        bad = copy.deepcopy(baseline)
        del bad["rows"][0]["outcomes"]["types"]
        mutations.append(bad)
        bad = copy.deepcopy(baseline)
        bad["rows"][0]["outcomes"]["types"] = "almost"
        mutations.append(bad)
        bad = copy.deepcopy(baseline)
        bad["executed"] = 1
        mutations.append(bad)
        bad = copy.deepcopy(baseline)
        bad["all_domains_match"] = 1
        mutations.append(bad)
        for bad in mutations:
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    compare.regressions_against(bad, baseline)
                with self.assertRaises(ValueError):
                    compare.regressions_against(baseline, bad)

    def test_recorded_acceptance_ignores_history_but_not_result_or_binding_changes(self):
        current = comparison([row("a"), row("b")], previous=None, changed_observations=[], regressions=[])
        recorded = dict(current, previous="a" * 64, changed_observations=[{"id": "b", "domain": "types"}],
                        regressions=[{"id": "a", "domain": "errors", "before": "match", "after": "different"}])
        self.assertEqual(compare.acceptance_summary(recorded), compare.acceptance_summary(current))
        for key, value in (("rust_capture_sha256", "x" * 64), ("all_domains_match", 1), ("rust_source_stable", False)):
            self.assertNotEqual(compare.acceptance_summary(dict(recorded, **{key: value})),
                                compare.acceptance_summary(current))


class HistoricalBaseline(unittest.TestCase):
    def test_committed_c1_start_baseline_retains_its_complete_inventory(self):
        raw = compare.BASELINE.read_bytes()
        document = compare.load_baseline()
        self.assertEqual(document["executed"], len(compare.phase2_inventory.executed()))
        self.assertEqual(compare.BASELINE.read_bytes(), raw)


class AuthenticatedCapture(unittest.TestCase):
    """Exercise real artifact replay on one committed fixture, never a corpus run."""

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase2-baseline-fixture-"))
        self.addCleanup(shutil.rmtree, self.directory)
        self.addCleanup(patch.stopall)
        self.metadata = build_capture(self.directory, [CONTROL], selection=compare.phase2_corpus.selection())
        fixture = load_fixture()
        native_report = {"pin": fixture["provenance"]["pin"],
                         "observation_sha256": fixture["provenance"]["native_observation_sha256"]}
        inventory_row = next(r for r in compare.phase2_inventory.executed() if r["id"] == CONTROL)
        patch.object(compare.phase2_inventory, "executed", return_value=[inventory_row]).start()
        patch.object(compare.phase2_native, "load_capture", return_value=(self.directory, native_report,
                                                                           [fixture["records"][CONTROL]["native"]])).start()
        patch.object(compare.phase2_native, "current").start()
        self.sources = patch.object(compare.phase2_corpus, "sources", return_value=self.metadata["build"]["sources"]).start()
        patch.object(compare, "RECORD", self.directory / "recorded.json").start()
        patch("sys.stdout").start()
        patch.object(producers.subprocess, "run", side_effect=AssertionError("focused tests must not execute cargo")).start()

    def test_record_with_previous_replays_without_losing_history(self):
        previous = self.directory / "previous.json"
        previous.write_text(json.dumps({"rows": []}))
        original = compare.report(self.directory, self.directory, previous=previous, record=True)
        raw = (self.directory / "comparison.json").read_bytes()
        replayed = compare.report(self.directory, self.directory, write=False)
        self.assertEqual((self.directory / "comparison.json").read_bytes(), raw)
        self.assertIsNotNone(original["previous"])
        self.assertIsNone(replayed["previous"])
        recorded = json.loads((self.directory / "recorded.json").read_bytes())
        self.assertEqual(compare.acceptance_summary(recorded), compare.acceptance_summary(replayed))

    def test_historical_sources_can_stale_without_invalidating_baseline_capture(self):
        compare.report(self.directory, self.directory)
        self.sources.return_value = {"current.rs": "different"}
        baseline = compare.baseline(self.directory, self.directory / "baseline.json.gz", native_dir=self.directory)
        self.assertEqual([r["id"] for r in baseline["rows"]], [CONTROL])

    def test_changed_raw_observation_cannot_be_accepted_via_old_comparison(self):
        compare.report(self.directory, self.directory)
        (self.directory / "cases/00000/stdout").write_bytes(b"changed raw observation")
        with self.assertRaisesRegex(ValueError, "raw case artifact changed"):
            compare.baseline(self.directory, self.directory / "baseline.json.gz", native_dir=self.directory)

    def test_valid_looking_comparison_cannot_replace_captured_results(self):
        original = compare.report(self.directory, self.directory)
        original["rows"][0]["outcomes"]["errors"] = "different"
        original["all_domains_match"] = 0
        (self.directory / "comparison.json").write_text(json.dumps(original))
        with self.assertRaisesRegex(ValueError, "authenticated Rust capture replay"):
            compare.baseline(self.directory, self.directory / "baseline.json.gz", native_dir=self.directory)


class ExitMetrics(unittest.TestCase):
    def setUp(self):
        self.baseline = comparison([row("s08", checkpoint="regression", s08="acceptance"), row("plain"),
                                    row("claimed", errors="different"), row("panicked", errors="failed")])
        self.addCleanup(patch.stopall)
        self.inventory = [{"id": r["id"]} for r in self.baseline["rows"]]
        patch.object(compare.phase2_inventory, "executed", return_value=self.inventory).start()
        self.claims = {"version": 1, "foundation_modules": ["relater_tuples"],
                       "rows": [{"id": "claimed", "status": "claimed"}]}

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
        found = self.metrics([row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"),
                              panicked])
        self.assertEqual((found["c1_failures"], found["c1_complete"]), (1, False))

    def test_unavailable_inputs_keep_completion_false(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), row("panicked")]
        for missing in ("claims", "audit_ok", "baseline", "contracts_ok"):
            found = self.metrics(rows, **{missing: None})
            self.assertFalse(found["c1_complete"], missing)
        self.assertFalse(self.metrics(rows, regression_parity=0.999)["c1_complete"])

    def test_a_blocked_claim_names_a_registered_blocker_and_carries_no_weight(self):
        self.claims["rows"].append({"id": "plain", "status": "blocked", "blocker": "B09", "evidence": "traced emit order"})
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain", errors="different"),
                row("claimed"), row("panicked")]
        self.baseline["rows"][1] = row("plain", errors="different")
        self.baseline["all_domains_match"] -= 1
        found = self.metrics(rows, blockers={"entries": [{"id": "B09", "variants": ["plain"]}]})
        self.assertEqual((found["c1_open"], found["c1_regressions"]), (0, 0))
        with self.assertRaisesRegex(ValueError, "no registered blocker"):
            self.metrics(rows, blockers={"entries": []})
        with self.assertRaisesRegex(ValueError, "covering the variant"):
            self.metrics(rows, blockers={"entries": [{"id": "B09", "variants": ["other"]}]})

    def test_candidate_still_prevents_completion_even_after_matching(self):
        self.claims["rows"].append({"id": "plain", "status": "candidate"})
        found = self.metrics([row("s08", checkpoint="regression", s08="acceptance"), row("plain"),
                              row("claimed"), row("panicked")])
        self.assertEqual((found["c1_open"], found["c1_complete"]), (1, False))

    def test_all_claim_statuses_validate_identity_duplicates_and_disposition(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), row("panicked")]
        for status in ("claimed", "candidate", "returned", "blocked", "typo"):
            with self.subTest(status=status):
                claims = copy.deepcopy(self.claims)
                claims["rows"].append({"id": "ghost", "status": status})
                with self.assertRaisesRegex(ValueError, "unknown executed variant"):
                    self.metrics(rows, claims=claims)
        for entry, message in (({"id": "claimed", "status": "candidate"}, "duplicate claim"),
                               ({"id": "plain", "status": "typo"}, "unknown status"),
                               ({"id": "plain", "status": "returned", "owner": "C2"}, "trace evidence"),
                               ({"id": "plain", "status": "returned", "owner": "C1", "evidence": "trace"}, "another checkpoint")):
            claims = copy.deepcopy(self.claims)
            claims["rows"].append(entry)
            with self.assertRaisesRegex(ValueError, message):
                self.metrics(rows, claims=claims)

    def test_missing_claim_or_foundation_inventory_cannot_certify_completion(self):
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), row("panicked")]
        for claims in ({}, dict(self.claims, rows=[]), dict(self.claims, foundation_modules=[]),
                       dict(self.claims, foundation_modules=["relater_tuples", "relater_tuples"])):
            with self.assertRaisesRegex(ValueError, "claims authority"):
                self.metrics(rows, claims=claims)

    def test_unattributed_failures_require_a_traced_return_or_covering_blocker(self):
        failed = row("panicked", errors="failed")
        failed["bucket"] = "failed: walker_error"
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), failed]
        self.assertEqual(self.metrics(rows)["c1_failures"], 1)
        self.claims["rows"].append({"id": "panicked", "status": "returned", "owner": "C3",
                                   "bucket": "failed: walker_error", "evidence": "traced iteration path"})
        self.assertEqual(self.metrics(rows)["c1_failures"], 0)
        self.claims["rows"][-1]["bucket"] = "failed: previous cause"
        self.assertEqual(self.metrics(rows)["c1_failures"], 1)
        self.claims["rows"].pop()
        blocker = {"id": "B19", "kind": "failed", "owner": "C3", "evidence": ["iteration path"], "variants": ["panicked"]}
        self.assertEqual(self.metrics(rows, blockers={"entries": [blocker]})["c1_failures"], 0)
        self.assertEqual(self.metrics(rows, blockers={"entries": [dict(blocker, owner="someone")]})["c1_failures"], 1)
        blocker["variants"] = ["other"]
        self.assertEqual(self.metrics(rows, blockers={"entries": [blocker]})["c1_failures"], 1)

    def test_unrelated_blocker_and_return_cannot_excuse_foundation_failure(self):
        failed = row("panicked", errors="failed")
        failed["bucket"] = "panic: tsr_checker::relater_tuples"
        rows = [row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"), failed]
        self.claims["rows"].append({"id": "panicked", "status": "returned", "owner": "C3",
                                   "bucket": failed["bucket"], "evidence": "claimed other cause"})
        self.assertEqual(self.metrics(rows)["c1_failures"], 1)
        self.claims["rows"][-1] = {"id": "panicked", "status": "blocked", "blocker": "B19", "evidence": "trace"}
        blocker = {"id": "B19", "kind": "unsupported", "owner": "C3", "evidence": ["trace"], "variants": ["panicked"]}
        failed["bucket"] = "failed: walker_error"
        self.assertEqual(self.metrics(rows, blockers={"entries": [blocker]})["c1_failures"], 1)

    def test_a_claim_must_name_an_executed_variant(self):
        self.claims["rows"].append({"id": "ghost", "status": "claimed"})
        with self.assertRaisesRegex(ValueError, "unknown executed variant"):
            self.metrics([row("s08", checkpoint="regression", s08="acceptance"), row("plain"), row("claimed"),
                          row("panicked")])

    def test_the_four_authorities_are_producer_inputs(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in ("data/phase2/c1-claims.json", "data/phase2/c1-audit.json", "data/phase2/c1-baseline.json.gz",
                     "data/phase2/receipts/c1-contracts.json", "data/go-functions.tsv"):
            self.assertIn(path, spec["inputs"], path)
            self.assertTrue((ROOT / path).is_file(), path)


class Receipt(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase2-receipt-"))
        self.addCleanup(shutil.rmtree, self.directory)
        self.addCleanup(patch.stopall)
        source = self.directory / "src/lib.rs"
        source.parent.mkdir(parents=True)
        source.write_text("#[test]\nfn a() {}\n#[test]\nfn b() {}\n")
        spec = {"commands": [["cargo", "test"], ["cargo", "test", "--release"]],
                "test_source": str(source), "sources": [str(source)]}
        patch.dict(producers.WITNESSES, {"probe": spec}).start()
        patch.object(producers, "ROOT", Path("/")).start()
        self.spec = spec
        self.source = source

    def receipt(self, exit_code=0):
        stdout = ("\nrunning 2 tests\ntest a ... ok\ntest b ... ok\n\n"
                  "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n")
        record = {"version": 2, "witness": "probe", "state": "observed", "tests": ["a", "b"],
                  "runs": [{"command": command, "exit_code": exit_code, "stdout": stdout,
                            "stdout_sha256": producers.digest(stdout.encode())} for command in self.spec["commands"]],
                  "source_inputs": producers.source_inputs(self.spec["sources"])}
        path = self.directory / "probe.json"
        path.write_text(json.dumps(record))
        return path

    def test_receipt_is_current_only_with_success_and_unchanged_sources(self):
        self.assertTrue(producers.receipt_current("probe", self.receipt()))
        self.assertFalse(producers.receipt_current("probe", self.receipt(exit_code=101)))
        path = self.receipt()
        self.source.write_text(self.source.read_text().replace("fn a() {}", "fn a() { let _ = 1; }"))
        self.assertFalse(producers.receipt_current("probe", path))
        self.assertIsNone(producers.receipt_current("probe", self.directory / "absent.json"))
        (self.directory / "probe.json").write_text(json.dumps({"version": 1, "witness": "probe", "state": "unobserved"}))
        self.assertFalse(producers.receipt_current("probe", self.directory / "probe.json"))

    def test_receipt_requires_exact_debug_and_release_commands(self):
        for commands in ([], [["cargo", "test"]], [["true"], ["true"]],
                         [["cargo", "test"], ["cargo", "test"]],
                         [["cargo", "test", "--release"], ["cargo", "test"]]):
            path = self.receipt()
            record = json.loads(path.read_text())
            record["runs"] = [dict(record["runs"][0], command=command) for command in commands]
            path.write_text(json.dumps(record))
            self.assertFalse(producers.receipt_current("probe", path), commands)

    def test_receipt_requires_each_test_to_run_and_pass(self):
        replacements = [("test b ... ok\n", ""), ("test b ... ok", "test a ... ok"),
                        ("test b ... ok", "test b ... ignored"), ("2 passed", "0 passed"),
                        ("0 filtered out", "1 filtered out"), ("test b ... ok", "test other ... ok")]
        for old, new in replacements:
            path = self.receipt()
            record = json.loads(path.read_text())
            record["runs"][1]["stdout"] = record["runs"][1]["stdout"].replace(old, new)
            record["runs"][1]["stdout_sha256"] = producers.digest(record["runs"][1]["stdout"].encode())
            path.write_text(json.dumps(record))
            self.assertFalse(producers.receipt_current("probe", path), (old, new))

    def test_changed_added_or_removed_sources_stale_receipt(self):
        self.spec["sources"] = [str(self.source.parent)]
        path = self.receipt()
        added = self.source.parent / "dep.rs"
        added.write_text("fn dependency() {}\n")
        self.assertFalse(producers.receipt_current("probe", path))
        path = self.receipt()
        added.unlink()
        self.assertFalse(producers.receipt_current("probe", path))

    def test_contract_sources_cover_loader_dependencies_workspace_and_assets(self):
        # Inspect the real closure without running cargo or a producer.
        with patch.object(producers, "ROOT", ROOT):
            inputs = producers.source_inputs(producers.WITNESSES["c1-contracts"]["sources"])
        for path in ("Cargo.toml", ".cargo/config.toml", "crates/tsr_parser/src/lib.rs",
                     "crates/tsr_module/src/lib.rs", "crates/tsr_vfs/src/lib.rs",
                     "crates/tsr_bundled/bundled/libs/lib.d.ts", "xtask/src/gen/diagnostics.rs",
                     "tools/s08/relater-prototype/Cargo.toml", "tools/phase1/syntax/Cargo.toml",
                     "tools/s10/corpus-adapter/Cargo.toml"):
            self.assertIn(path, inputs)

    def test_editor_and_python_caches_do_not_change_contract_sources(self):
        self.spec["sources"] = [str(self.source.parent)]
        before = producers.source_inputs(self.spec["sources"])
        (self.source.parent / ".DS_Store").write_bytes(b"editor metadata")
        cache = self.source.parent / "__pycache__"
        cache.mkdir()
        (cache / "generator.cpython.pyc").write_bytes(b"compiled python")
        self.assertEqual(producers.source_inputs(self.spec["sources"]), before)

    def test_tracker_expansion_covers_every_contract_receipt_source(self):
        # Match xtask's actual Git glob expansion, including nonignored added
        # files, instead of approximating its ** semantics with fnmatch.
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        command = ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--"]
        command.extend(":(top,glob)" + pattern for pattern in spec["sources"])
        tracked = set(subprocess.check_output(command, cwd=ROOT).decode().split("\0")) | set(spec["inputs"])
        with patch.object(producers, "ROOT", ROOT):
            required = set(producers.source_inputs(producers.WITNESSES["c1-contracts"]["sources"]))
        self.assertEqual(required - tracked, set(), "receipt inputs must invalidate tracker evidence too")


class Audit(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="phase2-audit-"))
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "crates/x/src").mkdir(parents=True)
        (self.root / "crates/x/src/lib.rs").write_text("// port: tsc/internal/checker/relater.go:Relater.isRelatedTo\nfn a() {}\nfn b() {}\n")
        self.known = {"tsc/internal/checker/relater.go:Relater.isRelatedTo", "tsc/internal/checker/relater.go:asRecursionId",
                      "tsc/internal/checker/types.go:Type.Id", "tsc/internal/checker/checker.go:Checker.getGlobalSymbol"}
        self.reviewed = {name: (len(members), audit.digest(audit.canonical(sorted(members))))
                         for name, members in self.document({})["groups"].items()}

    def document(self, dispositions):
        return {"version": 1, "pin": "fixture", "groups": {"relations": ["tsc/internal/checker/relater.go:Relater.isRelatedTo",
                                                        "tsc/internal/checker/relater.go:asRecursionId"],
                                         "types": ["tsc/internal/checker/types.go:Type.Id"]},
                "dispositions": dispositions}

    def problems(self, dispositions, **kwargs):
        return audit.problems(self.document(dispositions), root=self.root, known=self.known,
                              reviewed=self.reviewed, pin="fixture", required_handoffs={}, executed_ids={"case"}, **kwargs)

    def test_markers_dispose_and_every_member_needs_a_disposition(self):
        found = self.problems({})
        self.assertEqual(found, ["tsc/internal/checker/relater.go:asRecursionId: no disposition and no port marker",
                                 "tsc/internal/checker/types.go:Type.Id: no disposition and no port marker"])
        complete = {"tsc/internal/checker/relater.go:asRecursionId": {"disposition": "later", "owner": "C2", "reason": "r"},
                    "tsc/internal/checker/types.go:Type.Id": {"disposition": "equivalent", "rust": "crates/x/src/lib.rs:2", "reason": "r"}}
        self.assertEqual(self.problems(complete), [])
        self.assertTrue(audit.complete(self.document(complete), root=self.root, known=self.known,
                                       reviewed=self.reviewed, pin="fixture", required_handoffs={}, executed_ids={"case"}))

    def test_dispositions_need_evidence(self):
        cases = [
            ({"tsc/internal/checker/relater.go:asRecursionId": {"disposition": "mapped"}}, "no port marker names it"),
            ({"tsc/internal/checker/relater.go:Relater.isRelatedTo": {"disposition": "gap", "item": "C1.6", "reason": "r"}},
             "must be mapped"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "equivalent", "rust": "crates/x/src/lib.rs:99", "reason": "r"}},
             "does not exist"),
            ({"tsc/internal/checker/types.go:Type.Id": {"disposition": "equivalent", "rust": "crates/x/src/lib.rs:0", "reason": "r"}},
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

    def test_equivalent_site_must_be_rust_inside_production_crates(self):
        for path in ("elsewhere.rs", "crates/x/src/notes.txt"):
            (self.root / path).write_text("an existing file\n")
            found = self.problems({"tsc/internal/checker/types.go:Type.Id": {
                "disposition": "equivalent", "rust": path + ":1", "reason": "an asserted equivalent"}})
            self.assertTrue(any("does not exist" in problem for problem in found), path)

    def test_unknown_functions_and_duplicates_are_rejected(self):
        document = self.document({})
        document["groups"]["types"].append("tsc/internal/checker/types.go:Nope")
        document["groups"]["types"].append("tsc/internal/checker/relater.go:asRecursionId")
        found = audit.problems(document, root=self.root, known=self.known, allow_open=True,
                               reviewed=self.reviewed, pin="fixture", required_handoffs={}, executed_ids={"case"})
        self.assertTrue(any("not in the pinned inventory" in p for p in found))
        self.assertTrue(any("two groups" in p for p in found))

    def test_empty_removed_or_changed_group_and_wrong_pin_cannot_complete(self):
        original = self.document({})
        mutations = [dict(original, groups={}), dict(original, groups={"types": original["groups"]["types"]}),
                     dict(original, pin="other")]
        missing = copy.deepcopy(original)
        missing["groups"]["relations"].pop()
        mutations.append(missing)
        for document in mutations:
            # Even mapped functions cannot excuse an incomplete audit scope.
            self.assertFalse(audit.complete(document, root=self.root, known=self.known, mapped=self.known,
                                            reviewed=self.reviewed, pin="fixture", required_handoffs={}, executed_ids={"case"}))

    def test_handoff_requires_owner_reason_and_known_function_and_case_inventories(self):
        document = self.document({})
        valid = {"id": "c2-variance-measurement", "owner": "C2", "reason": "remaining measurement",
                 "functions": ["tsc/internal/checker/types.go:Type.Id"], "cases": ["case"]}
        kwargs = {"root": self.root, "known": self.known, "mapped": self.known,
                  "reviewed": self.reviewed, "pin": "fixture", "executed_ids": {"case"}}
        self.assertFalse(audit.complete(document, **kwargs))
        document["handoffs"] = [valid]
        self.assertTrue(audit.complete(document, **kwargs))
        for bad in (dict(valid, owner="C1"), dict(valid, reason=""), dict(valid, functions=[]),
                    dict(valid, functions=["unknown"]), dict(valid, cases=[]), dict(valid, cases=["ghost"]),
                    dict(valid, cases=["case", "case"])):
            self.assertFalse(audit.complete(dict(document, handoffs=[bad]), **kwargs))
        self.assertFalse(audit.complete(dict(document, handoffs=[valid, valid]), **kwargs))

    def test_committed_scope_removal_and_pin_mutations_are_rejected(self):
        original = audit.load()
        mutations = [dict(original, groups={}), dict(original, pin="other")]
        for group in original["groups"]:
            removed = copy.deepcopy(original)
            removed["groups"].pop(group)
            mutations.append(removed)
        changed = copy.deepcopy(original)
        changed["groups"]["C1.6 relations (relater.go)"].pop()
        mutations.append(changed)
        for document in mutations:
            found = audit.problems(document, allow_open=True, mapped=set())
            self.assertTrue(any("reviewed" in problem or "complete pinned file" in problem for problem in found))

    def test_committed_audit_names_only_pinned_functions(self):
        document = audit.load()
        found = [p for p in audit.problems(document, allow_open=True) if "no disposition" not in p]
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
