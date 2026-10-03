"""Completion requires actual applicable test observations, not port markers."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase4_contracts as contracts
import phase4_producers as producers


class ContractCoverage(unittest.TestCase):
    def row(self, *, host="any", status="ported", checkpoint="X1"):
        return {"id": "native:test", "host": host, "status": status, "checkpoint": checkpoint,
                "rust": [{"test": "crates/tsr_tsc/src/tests.rs::witness"}]}

    def test_roster_requires_the_named_executed_test(self):
        document = {"tests": [self.row()]}
        for passed, names in ((False, ["tests::witness"]), (True, ["tests::other"])):
            runs = {"tsr_tsc:lib:tsr_tsc": {"passed": passed, "tests": names}}
            result = contracts.coverage(runs, "macos", document)["X1"]
            self.assertEqual((result["observed"], result["required"]), (0, 1))
        runs = {"tsr_tsc:lib:tsr_tsc": {"passed": True, "tests": ["tests::witness"]}}
        self.assertEqual(contracts.coverage(runs, "macos", document)["X1"]["observed"], 1)

    def test_same_name_in_another_package_cannot_witness_a_port(self):
        result = contracts.coverage({"tsr_build:lib:tsr_build": {"passed": True, "tests": ["tests::witness"]}},
                                    "macos", {"tests": [self.row()]})
        self.assertEqual(result["X1"]["observed"], 0)

    def test_host_applicability_and_pending_are_separate(self):
        document = {"tests": [self.row(host="linux"), self.row(status="pending"),
                              self.row(status="not_applicable")]}
        result = contracts.coverage({}, "macos", document)["X1"]
        self.assertEqual((result["observed"], result["required"]), (0, 1))
        self.assertEqual(result["missing"], ["native:test"])

    def test_every_checkpoint_suite_is_a_real_cargo_target(self):
        targets = {contracts.tests.target_id(t) for t in contracts.targets()}
        targets.add(contracts.FSWATCH_DOC)
        self.assertTrue(all(set(suites) <= targets for suites in contracts.REQUIRED.values()))

    def test_callback_rejection_requires_its_compile_fail_observation(self):
        row = self.row(checkpoint="X4")
        row["rust"] = [{"test": contracts.CALLBACK_WITNESS}]
        runs = {contracts.FSWATCH: {"passed": True, "tests": ["tests::watch_directory"]}}
        for passed in (None, False, True):
            if passed is not None:
                runs[contracts.FSWATCH_DOC] = {"passed": passed, "tests": contracts.doc_inventory()}
            result = contracts.coverage(runs, "macos", {"tests": [row]})["X4"]
            self.assertEqual(result["observed"], int(passed is True))


class TestOutput(unittest.TestCase):
    def output(self, name, *, status="ok"):
        return (f"test {name} ... {status}\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n").encode()

    def test_expected_panic_decoration_is_not_a_different_test(self):
        name = "help::tests::init_rejects_untyped_numeric_enum_values"
        result = contracts.tests.validate_test_run(self.output(name + " - should panic"), [name])
        self.assertEqual(result["tests"], [name])
        for output in (self.output("other - should panic"), self.output(name + " - should panic", status="FAILED")):
            with self.assertRaises(contracts.tests.MeasuredFailure):
                contracts.tests.validate_test_run(output, [name])

    def test_compile_fail_requires_explicit_rustdoc_policy(self):
        names = contracts.doc_inventory()
        output = self.output(names[0] + " - compile fail")
        with self.assertRaises(contracts.tests.MeasuredFailure):
            contracts.tests.validate_test_run(output, names)
        result = contracts.tests.validate_test_run(output, names, name_suffixes=(" - compile fail",))
        self.assertEqual(result["passed"], 1)


class Completion(unittest.TestCase):
    def evidence(self):
        metrics = {name: True for name in ("inventory_frozen", "inventory_verified", "harness_valid", "result_recorded",
                   "blockers_named", "audit", "watcher_tests", "smoke", "buildinfo_interop", "live_watch_parity",
                   "determinism", "thread_sanitizer", *[f"x{i}_contracts" for i in range(1, 7)])}
        metrics.update(buildinfo_codec=1, incremental_correctness=1, unit_rosters=1, residuals=0)
        rows = [{"family": f, "id": f + "/case.js", "category": "match"} for f in producers.phase4_corpus.FAMILIES]
        rows.append({"family": "tsc", "id": "tsc/generateTrace/case.js", "category": "match"})
        return metrics, {"rows": rows}

    def test_missing_receipts_cannot_complete_checkpoints(self):
        self.assertFalse(any(producers.checkpoint_metrics({}).values()))
        metrics, comparison = self.evidence()
        del metrics["x3_contracts"]
        result = producers.checkpoint_metrics(metrics, comparison)
        self.assertTrue(result["x2_complete"])
        self.assertFalse(result["x3_complete"])
        self.assertFalse(result["x7_complete"])

    def test_B01_blocks_build_even_when_every_unit_test_passes(self):
        metrics, comparison = self.evidence()
        next(r for r in comparison["rows"] if r["family"] == "tsbuild")["category"] = "different"
        result = producers.checkpoint_metrics(metrics, comparison)
        self.assertFalse(result["x3_complete"])
        self.assertFalse(result["x7_complete"])

    def test_exact_approved_trace_is_accepted_but_never_a_failure(self):
        metrics, comparison = self.evidence()
        comparison["rows"][-1].update(category="different", approved_difference="exact-pair")
        self.assertTrue(producers.checkpoint_metrics(metrics, comparison)["x6_complete"])
        comparison["rows"][-1]["category"] = "failed"
        self.assertFalse(producers.checkpoint_metrics(metrics, comparison)["x6_complete"])

    def test_stale_audit_or_missing_X7_witness_cannot_close(self):
        for missing in ("audit", "smoke", "buildinfo_interop", "live_watch_parity", "determinism", "thread_sanitizer"):
            metrics, comparison = self.evidence()
            del metrics[missing]
            self.assertFalse(producers.checkpoint_metrics(metrics, comparison)["x7_complete"], missing)

    def test_an_empty_family_is_not_vacuous_success(self):
        metrics, comparison = self.evidence()
        comparison["rows"] = [r for r in comparison["rows"] if r["family"] != "tsbuild"]
        self.assertFalse(producers.checkpoint_metrics(metrics, comparison)["x3_complete"])


if __name__ == "__main__":
    unittest.main()
