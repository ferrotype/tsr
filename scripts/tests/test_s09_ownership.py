import copy
from contextlib import redirect_stderr
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s09_ownership import CRITERIA, MODES, OWNERSHIP_SUITES, SUITES, measure, publish_metrics, validate_manifest
import s09_printing


def inventory():
    return {"version": 2, "suites": {
        name: {"package": package, "filter": prefix,
               "cases": [prefix + "commit_before_retirement", prefix + "retirement_before_commit"]}
        for name, (package, prefix) in SUITES.items()
    }}


def suite_output(cases):
    rows = "\n".join(f"test {name} ... ok" for name in cases)
    return (f"running {len(cases)} tests\n{rows}\n"
            f"test result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n").encode()


class CheckerOwnershipProducer(unittest.TestCase):
    def setUp(self):
        self.manifest = inventory()
        self.modes = {mode: {suite: True for suite in SUITES} for mode in MODES}
        self.arena = {mode: True for mode in MODES}
        # These tests invent tiny libtest inventories. Fixture hashes and
        # captured wire rows are checked independently by test_s09_printing.
        verifier = patch.object(s09_printing, "verify_frozen", return_value=None)
        self.verify_frozen = verifier.start()
        self.addCleanup(verifier.stop)

    def test_stale_or_missing_printing_fixture_blocks_scratch_before_cargo_only(self):
        for error in (ValueError("stale request hash"), OSError("missing native observation")):
            calls = []
            self.verify_frozen.reset_mock()
            self.verify_frozen.side_effect = error

            def invoke(root, args, env):
                package = args[args.index("--package") + 1]
                suite = next(suite for suite in self.manifest["suites"].values()
                             if suite["package"] == package and suite["filter"] in args)
                calls.append((package, suite["filter"]))
                return suite_output(suite["cases"])

            with self.subTest(error=type(error).__name__), redirect_stderr(io.StringIO()):
                outcomes = measure(Path("synthetic-root"), invoke, ["cargo"], [], {}, self.manifest, "debug")
            self.verify_frozen.assert_called_once_with(root=Path("synthetic-root"))
            self.assertEqual(outcomes, {"generation": True, "pool": True, "registry": True, "scratch": False})
            self.assertEqual(calls, [SUITES[name] for name in OWNERSHIP_SUITES])
            modes = {**self.modes, "debug": outcomes}
            report = {"metrics": {}}
            publish_metrics(report, modes, self.arena, self.manifest)
            self.assertTrue(all(report["metrics"][criterion] for criterion in CRITERIA))
            self.assertFalse(report["metrics"]["api_print_scratch_disposal"])
            self.assertEqual(report["metrics"]["api_print_scratch_tests"], 0)

    def test_manifest_rejects_missing_retargeted_or_ignored_suites(self):
        for name in SUITES:
            missing = copy.deepcopy(self.manifest)
            del missing["suites"][name]
            with self.assertRaises(ValueError):
                validate_manifest(missing)
            for key, value in (("package", "ts_checker"), ("filter", "unrelated::"),
                               ("skip", ["retirement_before_commit"]), ("exact", True)):
                changed = copy.deepcopy(self.manifest)
                changed["suites"][name][key] = value
                with self.assertRaisesRegex(ValueError, "changed scope"):
                    validate_manifest(changed)
        for version in (True, 0, 1, "2"):
            with self.assertRaises(ValueError):
                validate_manifest({**self.manifest, "version": version})

    def test_empty_duplicate_unsorted_or_foreign_cases_are_not_a_denominator(self):
        for cases in ([], None, [False], ["not_a_test"], ["tests::other"],
                      ["lease::tests::a", "lease::tests::a"],
                      ["lease::tests::z", "lease::tests::a"]):
            changed = copy.deepcopy(self.manifest)
            changed["suites"]["generation"]["cases"] = cases
            with self.subTest(cases=cases), self.assertRaises(ValueError):
                validate_manifest(changed)

    def test_zero_exit_requires_exact_complete_passing_output_and_does_not_skip_other_suites(self):
        for defect in ("missing", "extra", "duplicate", "failed", "ignored", "no_summary", "exit_failure"):
            calls = []

            def invoke(root, args, env):
                package = args[args.index("--package") + 1]
                suite = next(suite for suite in self.manifest["suites"].values()
                             if suite["package"] == package and suite["filter"] in args)
                calls.append(package)
                cases = suite["cases"]
                if package != "ts_project":
                    return suite_output(cases)
                if defect == "exit_failure":
                    raise RuntimeError("project test assertion")
                if defect == "missing":
                    return suite_output(cases[1:])
                if defect == "extra":
                    return suite_output(cases + ["tests::new_unreviewed_boundary"])
                if defect == "duplicate":
                    return suite_output(cases + cases[:1])
                if defect == "no_summary":
                    return suite_output(cases).split(b"test result:")[0]
                return suite_output(cases).replace(b" ... ok", f" ... {defect}".encode(), 1)

            with self.subTest(defect=defect), redirect_stderr(io.StringIO()):
                result = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
            self.assertEqual(result, {"generation": True, "pool": False, "registry": True, "scratch": True})
            self.assertEqual(calls, ["ts_arena", "ts_project", "ts_api", "ts_api"])

    def test_every_suite_runs_with_each_modes_actual_instrumentation(self):
        variants = (("debug", ["cargo"], []), ("release", ["cargo"], ["--release"]),
                    ("miri", ["cargo", "+nightly-test", "miri"], ["--target", "native"]),
                    ("address_sanitizer", ["cargo", "+nightly-test"], ["-Zbuild-std", "--target", "native"]))
        calls = []
        for mode, prefix, options in variants:
            def invoke(root, args, env):
                package = args[args.index("--package") + 1]
                suite = next(suite for suite in self.manifest["suites"].values()
                             if suite["package"] == package and suite["filter"] in args)
                self.assertEqual(args, [*prefix, "test", "--package", package, "--lib", "--locked",
                                        *options, suite["filter"], "--", "--test-threads=1", "--nocapture"])
                self.assertEqual(env, {"mode": mode})
                calls.append((mode, package))
                return suite_output(suite["cases"])

            with redirect_stderr(io.StringIO()):
                result = measure(Path("."), invoke, prefix, options, {"mode": mode}, self.manifest, mode)
            self.assertEqual(result, self.modes[mode])
        self.assertEqual(len(calls), 16)
        with self.assertRaises(ValueError):
            measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "unknown")

    def test_every_ownership_suite_and_mode_is_required_for_both_criteria(self):
        for mode in MODES:
            for suite in OWNERSHIP_SUITES:
                changed = copy.deepcopy(self.modes)
                changed[mode][suite] = False
                report = {"metrics": {}}
                publish_metrics(report, changed, self.arena, self.manifest)
                for criterion in CRITERIA:
                    self.assertFalse(report["metrics"][criterion])
                    self.assertFalse(report["metrics"][f"{criterion}_{mode}"])
                self.assertEqual(report["metrics"]["checker_ownership_tests"], 4)

    def test_scratch_failure_is_informational_without_changing_s09_4_or_completing_s09_3(self):
        for mode in MODES:
            changed = copy.deepcopy(self.modes)
            changed[mode]["scratch"] = False
            report = {"metrics": {}}
            publish_metrics(report, changed, self.arena, self.manifest)
            self.assertTrue(all(report["metrics"][criterion] for criterion in CRITERIA))
            self.assertFalse(report["metrics"]["api_print_scratch_disposal"])
            self.assertFalse(report["metrics"][f"api_print_scratch_disposal_{mode}"])
            self.assertEqual(report["metrics"]["api_print_scratch_tests"], 0)
            self.assertEqual(report["metrics"]["checker_ownership_tests"], 6)
            self.assertNotIn("api_scratch_disposal", report["metrics"])
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.arena, self.manifest)
        self.assertTrue(report["metrics"]["api_print_scratch_disposal"])
        self.assertEqual(report["metrics"]["api_print_scratch_tests"], 2)
        self.assertNotIn("api_scratch_disposal", report["metrics"])

    def test_registry_and_scratch_filters_execute_disjoint_cases_in_the_same_package(self):
        cases_by_package = {}
        for suite in self.manifest["suites"].values():
            cases_by_package.setdefault(suite["package"], []).extend(suite["cases"])

        def invoke(root, args, env):
            package = args[args.index("--package") + 1]
            selected = args[args.index("--") - 1]
            return suite_output(sorted(case for case in cases_by_package[package] if selected in case))

        with redirect_stderr(io.StringIO()):
            result = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
        self.assertTrue(all(result.values()))
        cases_by_package["ts_api"].append("printing::tests::new_unreviewed_case")
        with redirect_stderr(io.StringIO()):
            result = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
        self.assertFalse(result["registry"])
        self.assertTrue(result["scratch"])

    def test_arena_boundary_observation_is_required_even_if_checker_suites_pass(self):
        for mode in MODES:
            changed = {**self.arena, mode: False}
            report = {"metrics": {}}
            publish_metrics(report, self.modes, changed, self.manifest)
            self.assertFalse(report["metrics"]["release_boundaries"])
            self.assertFalse(report["metrics"][f"release_boundaries_{mode}"])
            self.assertTrue(report["metrics"]["shared_pool_panic_retirement"])
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.arena, self.manifest)
        self.assertTrue(all(report["metrics"][criterion] for criterion in CRITERIA))
        self.assertEqual(report["metrics"]["checker_ownership_tests"], 6)
        for unavailable in ("independent_checker_merges", "checker_ast_retention", "api_scratch_disposal",
                            "live_owner_delta", "live_allocation_delta", "miri", "address_sanitizer"):
            self.assertNotIn(unavailable, report["metrics"])

    def test_missing_or_untyped_measurements_cannot_claim_success(self):
        for mode in MODES:
            changed = copy.deepcopy(self.modes)
            del changed[mode]
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, changed, self.arena, self.manifest)
            for value in (None, 1, "true", []):
                changed = copy.deepcopy(self.modes)
                changed[mode]["pool"] = value
                with self.assertRaises(ValueError):
                    publish_metrics({"metrics": {}}, changed, self.arena, self.manifest)
                with self.assertRaises(ValueError):
                    publish_metrics({"metrics": {}}, self.modes, {**self.arena, mode: value}, self.manifest)
            changed = copy.deepcopy(self.modes)
            del changed[mode]["registry"]
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, changed, self.arena, self.manifest)
            changed = copy.deepcopy(self.modes)
            del changed[mode]["scratch"]
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, changed, self.arena, self.manifest)
            changed_arena = self.arena.copy()
            del changed_arena[mode]
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, self.modes, changed_arena, self.manifest)


if __name__ == "__main__":
    unittest.main()
