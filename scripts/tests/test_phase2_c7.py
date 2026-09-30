"""C7 starts from the content-mapper ledger move with its bound audit scope, and from fingerprints that leave the
crates' test-only suites to the contract receipts (docs/PHASE2-C7-plan.md, C7.8.0 and decision 8)."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
import unittest.mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import phase2_informational as informational  # noqa: E402
import phase2_inventory as inventory  # noqa: E402
import phase2_order_trace as order_trace  # noqa: E402
import phase2_producers as producers  # noqa: E402

_spec = importlib.util.spec_from_file_location("ledger_init", ROOT / "scripts/ledger-init.py")
ledger_init = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ledger_init)


class LedgerMove(unittest.TestCase):
    def test_the_content_mapper_files_the_rows_execute_are_phase_2(self):
        phases = {entry["go"]: entry["phase"] for entry in tomllib.loads((ROOT / "PORTS.toml").read_text())["file"]}
        self.assertEqual(len(ledger_init.CONTENT_MAPPER_FILES), 22)
        for path in ledger_init.CONTENT_MAPPER_FILES:
            self.assertEqual(phases[path], 2, path)
        # Reached by no row: the process transports, the synchronous conn, the
        # other mappers and the content-mapper baseline the oracle never takes.
        for path, phase in (("tsc/internal/ipc/transport.go", 6), ("tsc/internal/ipc/transport_unix.go", 6),
                            ("tsc/internal/ipc/conn_sync.go", 6),
                            ("tsc/internal/testutil/contentmappertest/manifest.go", 1),
                            ("tsc/internal/testutil/contentmappertest/verbatim.go", 1),
                            ("tsc/internal/testutil/tsbaseline/contentmapper_baseline.go", 1)):
            self.assertEqual(phases[path], phase, path)

    def test_the_retaken_native_captures_stay_current(self):
        # Every script, data file and oracle source the native captures bind:
        # a C7 change to one of them stales both captures until they are
        # retaken, so C7's tools live beside them instead.
        import phase2_native as native
        import phase2_native_concurrent as concurrent
        for module, path in ((native, "data/phase2/native-provenance.json"),
                             (concurrent, "data/phase2/native-provenance-concurrent.json")):
            recorded = json.loads((ROOT / path).read_text())["inputs"]
            self.assertEqual(module.input_digests(), recorded, path)

    def test_the_c7_audit_scope_is_bound_over_complete_files_and_the_integration(self):
        document = audit.load(ROOT / "data/phase2/c7-audit.json")
        self.assertEqual(document["checkpoint"], "C7")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        self.assertEqual(sum(count for count, _ in audit.C7_REVIEWED_GROUPS.values()), 237)
        known = audit.inventory()
        moved = {path + ":" for path in ledger_init.CONTENT_MAPPER_FILES}
        covered = {identity for group in audit.C7_COMPLETE_FILES for identity in document["groups"][group]}
        self.assertEqual(covered, {identity for identity in known if identity.startswith(tuple(moved))})
        altered = copy.deepcopy(document)
        altered["groups"]["C7.8.1 IPC connection and protocol (ipc)"].pop()
        self.assertTrue(any("complete pinned file" in problem or "reviewed function inventory" in problem
                            for problem in audit.problems(altered, allow_open=True)))
        # C7.8.3 closed the last gaps and review issues.
        self.assertTrue(audit.complete(document))
        self.assertEqual(document["open_issues"], [])


class Fingerprints(unittest.TestCase):
    def test_test_only_suites_leave_the_capture_and_checker_fingerprints(self):
        self.assertTrue(corpus.test_only("crates/tsr_compiler/tests/fixtures/c2/contextual_audit.json"))
        self.assertFalse(corpus.test_only("crates/tsr_compiler/src/program.rs"))
        self.assertFalse(corpus.test_only("crates/tsr_compiler/examples/phase2_checker.rs"))
        sources = corpus.sources()
        self.assertIn("crates/tsr_compiler/src/lib.rs", sources)
        self.assertFalse([path for path in sources if corpus.test_only(path)])
        self.assertFalse([path for path in producers.production_inputs() if corpus.test_only(path)])
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        self.assertIn("crates/**", spec["sources"])
        self.assertEqual(spec["exclude"], ["crates/*/tests/**"])
        # The contract receipts still bind the suites they run.
        receipt = producers.source_inputs(producers.WITNESSES["c2-contracts"]["sources"])
        self.assertIn("crates/tsr_compiler/tests/fixtures/c2/contextual_audit.json", receipt)

    def test_the_order_trace_example_keeps_the_test_support_it_includes(self):
        program = "crates/tsr_compiler/tests/support/c2_order_program.rs"
        self.assertIn(f'#[path = "../tests/support/{Path(program).name}"]',
                      (ROOT / "crates/tsr_compiler/examples/c2_order_trace.rs").read_text())
        self.assertIn(program, order_trace.rust_inputs())


class Observation(unittest.TestCase):
    def test_a_failing_suite_fails_the_observation_but_keeps_its_receipt(self):
        tests = producers.witness_tests(producers.WITNESSES["c1-contracts"])
        passing = "".join(f"test {name} ... ok\n" for name in tests) + (
            f"test result: ok. {len(tests)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n")
        failing = passing.replace(f"test {tests[0]} ... ok", f"test {tests[0]} ... FAILED")
        for stdout, code in ((failing, 101), (passing, 101), (failing, 0)):
            completed = subprocess.CompletedProcess([], code, stdout.encode(), b"")
            with tempfile.TemporaryDirectory() as directory, \
                    unittest.mock.patch.object(producers.subprocess, "run", return_value=completed):
                with self.assertRaisesRegex(ValueError, "c1-contracts failed"):
                    producers.observe("c1-contracts", directory)
                self.assertTrue((Path(directory) / "c1-contracts.json").is_file())
        completed = subprocess.CompletedProcess([], 0, passing.encode(), b"")
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(producers.subprocess, "run", return_value=completed):
            self.assertEqual(producers.observe("c1-contracts", directory)["witness"], "c1-contracts")


class Wiring(unittest.TestCase):
    def test_c7_is_the_current_checkpoint_and_its_authorities_are_checker_inputs(self):
        self.assertEqual(producers.CHECKPOINTS[-1], "C7")
        self.assertEqual(producers.newest_checkpoint({name: None for name in producers.CHECKPOINTS}), "C7")
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in ("data/phase2/c7-audit.json", "data/phase2/informational.json", "data/phase2/residuals.json",
                     "data/phase2/native-provenance-concurrent.json", "data/divergences.toml"):
            self.assertIn(path, spec["inputs"])
        # Every contract witness's receipt, C7.8.5's among them, binds the run.
        self.assertIn("c7-contracts", producers.WITNESSES)
        for witness in producers.WITNESSES:
            self.assertIn(f"data/phase2/receipts/{witness}.json", spec["inputs"])
        declared = set(tomllib.loads((ROOT / "status/runs.toml").read_text()))
        self.assertEqual(declared, {*producers.PREREQUISITE_RUNS, "checker"})

    def test_the_sprint_items_close_on_the_runs_the_producer_reads(self):
        sprint = tomllib.loads((ROOT / "sprints/P2B.toml").read_text())
        items = {item["id"]: item["done_when"] for item in sprint["item"]}
        for checkpoint, (evidence_id, metric) in producers.RECORDED_COMPLETIONS.items():
            self.assertEqual(items["P2B-" + checkpoint], [f"recorded.checker.{evidence_id}.{metric} == true"])
            self.assertIs(producers.recorded_metric(evidence_id, metric), True)
        for checkpoint in ("C3", "C4"):
            self.assertEqual(items["P2B-" + checkpoint], sprint["exit"] + ["run.checker.c7_residuals == 0"])
        self.assertEqual([f"run.checker.{name} == {str(value).lower()}" for name, value in producers.P2B_EXIT],
                         [check for check in sprint["exit"] if check.startswith("run.checker.")])

    def test_a_recorded_run_without_the_metric_or_a_missing_id_stays_open(self):
        c1, _ = producers.RECORDED_COMPLETIONS["C1"]
        self.assertIsNone(producers.recorded_metric(c1, "c6_complete"))
        self.assertIsNone(producers.recorded_metric("c" * 64, "c1_complete"))
        self.assertIsNone(producers.recorded_metric(c1, "c1_complete", run="relater"))
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory) / "status/evidence"
            evidence.mkdir(parents=True)
            raw = (ROOT / f"status/evidence/{c1}.json").read_bytes()
            (evidence / f"{c1}.json").write_bytes(raw + b" ")
            self.assertIsNone(producers.recorded_metric(c1, "c1_complete", root=directory))


class RunLevel(unittest.TestCase):
    EXECUTED = [{"id": "a", "content_mapper": False}, {"id": "mapped", "content_mapper": True}]

    def metrics(self, *, differences=0, outcomes=None, receipt=True, verified=True):
        outcomes = outcomes or {"errors": "match"}
        rows = [{"id": row["id"], "outcomes": dict(outcomes)} for row in self.EXECUTED]
        state = {"native_verified_concurrent": verified, "harness_valid_concurrent": True, "report": {},
                 "modes": {"outcome_differences": differences,
                           "single": {"harness_errors": 0, "rust_capture_sha256": "rust"}},
                 "concurrent": {"rows": rows}}
        comparison = {"rust_capture_sha256": "rust", "rows": rows}
        patch = unittest.mock.patch.object
        with patch(producers, "assignments_current", return_value=True), \
                patch(producers, "services_current", return_value=True), \
                patch(producers, "receipt_current", return_value=receipt):
            return producers.run_level_metrics(state, comparison, self.EXECUTED)

    def test_the_run_level_two_mode_and_services_metrics_are_computed(self):
        self.assertEqual(self.metrics(), dict.fromkeys(producers.RUN_LEVEL, True))

    def test_one_outcome_difference_keeps_mode_parity_false(self):
        self.assertIs(self.metrics(differences=1)["mode_parity"], False)
        self.assertIs(self.metrics(verified=False)["mode_parity"], False)

    def test_content_mappers_need_their_rows_matched_in_both_modes_and_a_current_receipt(self):
        self.assertIs(self.metrics(outcomes={"errors": "unsupported"})["content_mappers"], False)
        self.assertIs(self.metrics(receipt=None)["content_mappers"], False)


class Completion(unittest.TestCase):
    PASSING = {**{name: True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                          "result_recorded", "blockers_named")},
               **{name: value for name, value in producers.P2B_EXIT}, **dict.fromkeys(producers.RUN_LEVEL, True)}
    STATES = {**dict.fromkeys(producers.PREREQUISITE_RUNS, "current"), "checker": "incomplete attempt"}

    def complete(self, metrics=None, states=None, **authorities):
        checks = {"informational": True, "residuals": 0, "dispositions": True, "divergences": True, "report": True}
        checks.update(authorities)
        modules = {name: unittest.mock.Mock(**{function: unittest.mock.Mock(return_value=checks[key])})
                   for name, function, key in (("phase2_residuals", "verified_count", "residuals"),
                                               ("phase2_dispositions", "complete", "dispositions"),
                                               ("phase2_divergences", "valid", "divergences"),
                                               ("phase2_report", "current", "report"))}
        with unittest.mock.patch.dict(sys.modules, modules), \
                unittest.mock.patch.object(informational, "current",
                                           return_value=checks["informational"]):
            return producers.c7_metrics(self.PASSING if metrics is None else metrics, True,
                                        self.STATES if states is None else states, {"concurrent": None}, {})

    def test_the_passing_state_completes_c7(self):
        self.assertIs(self.complete()["c7_complete"], True)

    def test_one_residual_keeps_c7_open(self):
        metrics = self.complete(residuals=1)
        self.assertEqual(metrics["c7_residuals"], 1)
        self.assertIs(metrics["c7_complete"], False)

    def test_each_c7_authority_gates_completion(self):
        for name in ("informational", "dispositions", "divergences", "report"):
            with self.subTest(name=name):
                self.assertIs(self.complete(**{name: False})["c7_complete"], False)

    def test_a_stale_prerequisite_fails_evidence_while_the_checker_run_is_excluded(self):
        self.assertIs(self.complete()["c7_evidence_current"], True)
        stale = dict(self.STATES, e5="stale: source, pin, command or inputs changed")
        metrics = self.complete(states=stale)
        self.assertIs(metrics["c7_evidence_current"], False)
        self.assertIs(metrics["c7_complete"], False)
        undeclared = dict(self.STATES, extra="current")
        self.assertIs(self.complete(states=undeclared)["c7_evidence_current"], False)

    def test_the_p2b_exit_and_the_run_level_metrics_gate_completion(self):
        for name in ("errors_parity", "unsupported_required", "mode_parity", "content_mappers", "blockers_named"):
            with self.subTest(name=name):
                metrics = dict(self.PASSING, **{name: 1 if name == "unsupported_required" else False})
                self.assertIs(self.complete(metrics)["c7_complete"], False)

    def test_an_unbuilt_authority_reads_false(self):
        with unittest.mock.patch.object(informational, "current",
                                        side_effect=ValueError("the skip list differs")):
            metrics = producers.c7_metrics(self.PASSING, True, self.STATES, {"concurrent": None}, {})
        self.assertIs(metrics["c7_informational_listed"], False)
        self.assertNotIn("c7_residuals", metrics)
        self.assertIs(metrics["c7_complete"], False)


class SkipList(unittest.TestCase):
    def test_the_skip_list_is_current_and_agrees_with_the_pinned_rules(self):
        self.assertIs(informational.current(), True)
        document = informational.informational()
        self.assertEqual(document["counts"]["informational"],
                         {"filename_skip": 52, "not_enumerated": 2, "option_guard_skip": 1720})
        self.assertEqual(document["counts"]["executed"], 13432)

    def test_a_classification_the_pinned_rules_disagree_with_fails(self):
        base = inventory.read()
        executed = next(row for row in base["rows"] if row["tier"] == "executed")
        guarded = next(row for row in base["rows"] if row["informational_reason"] == "option_guard_skip")
        for row, change in ((executed, {"target": 1}), (guarded, None)):
            document = copy.deepcopy(base)
            target = next(entry for entry in document["rows"] if entry["id"] == row["id"])
            target["options"] = dict(target["options"], **change) if change else {}
            with self.subTest(row=row["id"]), self.assertRaisesRegex(ValueError, "rule"):
                informational.informational(document)



class Residuals(unittest.TestCase):
    """C7.1: the residual list over both modes' comparisons."""

    def setUp(self):
        import phase2_residuals
        from test_phase2_c1 import comparison, row
        self.residuals, self.row, self.comparison = phase2_residuals, row, comparison
        self.single = comparison([row("a"), row("b", errors="different"), row("c", types="unsupported"),
                                  row("d")])
        self.concurrent = comparison([row("a"), row("b", errors="different", union_ordering="different"),
                                      row("c"), row("d", symbols="failed")], rust_capture_sha256="k" * 64)

    def test_open_rows_join_both_modes_and_drop_approved_pairs(self):
        self.assertEqual(self.residuals.open_rows(self.single, self.concurrent), {
            "b": {"single": ["errors"], "concurrent": ["errors", "union_ordering"]},
            "c": {"single": ["types"], "concurrent": []},
            "d": {"single": [], "concurrent": ["symbols"]}})
        covered = {("b", "errors"), ("b", "union_ordering"), ("c", "types")}
        self.assertEqual(self.residuals.open_rows(self.single, self.concurrent, covered),
                         {"d": {"single": [], "concurrent": ["symbols"]}})

    def test_each_row_names_its_owner_attribution_blocker_and_resolution(self):
        handoff = {"owner": "Phase 5", "go": "tsc/internal/x.go:F", "cause": "c",
                   "trace": {"path": "t", "sha256": "s" * 64}}
        register = {"entries": [{"id": "B01", "ownership": [{"variant": "c", "domain": "types"}]}]}
        rows = self.residuals.residual_rows(
            self.single, self.concurrent, owners={"b": "C2", "c": "C3", "d": "C4"}, handoffs={"c": handoff},
            register=register, proposed={"d"})
        self.assertEqual([(row["id"], row["owner"], row["blocker"], row["resolution"]) for row in rows],
                         [("b", "C2", None, "checkpoint"), ("c", "Phase 5", "B01", "joint"),
                          ("d", "C4", None, "divergence")])
        self.assertEqual(rows[1]["attribution"], {"from": None, "go": "tsc/internal/x.go:F", "cause": "c",
                                                  "trace": {"path": "t", "sha256": "s" * 64}})
        self.assertIsNone(rows[0]["attribution"])

    def test_the_count_holds_only_for_the_comparisons_the_list_names(self):
        rows = self.residuals.residual_rows(self.single, self.concurrent, owners={"b": "C2", "c": "C3", "d": "C4"},
                                            handoffs={}, register=None)
        value = self.residuals.document(self.single, self.concurrent, rows)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "residuals.json"
            path.write_text(self.residuals.render(value))
            with unittest.mock.patch.object(self.residuals, "divergence_coverage", return_value=(set(), set())):
                self.assertEqual(self.residuals.verified_count(self.single, self.concurrent, path), 3)
                for single, concurrent in ((self.concurrent, self.single), (self.single, None),
                                           (self.single, self.comparison(self.concurrent["rows"][:1] + [
                                               self.row("b", errors="different")], rust_capture_sha256="k" * 64))):
                    with self.subTest(concurrent=concurrent and concurrent["rust_capture_sha256"]):
                        with self.assertRaises(ValueError):
                            self.residuals.verified_count(single, concurrent, path)
            empty = self.comparison([self.row("a")])
            path.write_text(self.residuals.render(self.residuals.document(empty, empty, [])))
            self.assertEqual(self.residuals.verified_count(empty, empty, path), 0)

    def test_a_malformed_list_is_rejected(self):
        value = self.residuals.document(self.single, self.concurrent, [])
        for broken in ({**value, "version": 2}, {**value, "captures": {"single": {}}},
                       {**value, "rows": [{"id": "b", "domains": {"single": [], "concurrent": []}, "owner": "C2",
                                           "attribution": None, "blocker": None, "resolution": "checkpoint"}]},
                       {**value, "rows": [{"id": "b", "domains": {"single": ["errors"], "concurrent": []},
                                           "owner": "C2", "attribution": None, "blocker": None,
                                           "resolution": "later"}]}):
            with self.assertRaises(ValueError):
                self.residuals.validate(broken)


if __name__ == "__main__":
    unittest.main()
