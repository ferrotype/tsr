import copy
from contextlib import redirect_stderr
import io
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s09_ownership import CRITERIA, MODES, OWNERSHIP_SUITES, RETENTION, SUITES, measure, publish_metrics, validate_manifest
import s09_format
import s09_printing

ROOT = Path(__file__).resolve().parents[2]


def inventory():
    return {"version": 4, "suites": {
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
        insertion = patch.object(s09_format, "verify_insertion_frozen", return_value=None)
        self.verify_insertion = insertion.start()
        self.addCleanup(insertion.stop)

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
            self.assertEqual(outcomes, {**{name: True for name in SUITES}, "scratch": False})
            self.assertEqual(calls, [SUITES[name] for name in SUITES if name != "scratch"])
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
            # A package no suite names, so the retarget is a change for every one.
            for key, value in (("package", "tsr_compiler"), ("filter", "unrelated::"),
                               ("skip", ["retirement_before_commit"]), ("exact", True)):
                changed = copy.deepcopy(self.manifest)
                changed["suites"][name][key] = value
                with self.assertRaisesRegex(ValueError, "changed scope"):
                    validate_manifest(changed)
        for version in (True, 0, 1, 2, "3"):
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
                # One defective suite; its package also hosts the retention suites.
                if (package, suite["filter"]) != SUITES["pool"]:
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
            self.assertEqual(result, {**{name: True for name in SUITES}, "pool": False})
            self.assertEqual(calls, [package for package, _ in SUITES.values()])

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
        self.assertEqual(len(calls), len(SUITES) * len(variants))
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
            # Printing is half of S09-3, so the criterion fails with it.
            self.assertFalse(report["metrics"]["api_scratch_disposal"])
            self.assertFalse(report["metrics"][f"api_scratch_disposal_{mode}"])
            self.assertEqual(report["metrics"]["api_scratch_tests"], 0)
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.arena, self.manifest)
        self.assertTrue(report["metrics"]["api_print_scratch_disposal"])
        self.assertEqual(report["metrics"]["api_print_scratch_tests"], 2)
        self.assertTrue(report["metrics"]["api_scratch_disposal"])
        self.assertEqual(report["metrics"]["api_scratch_tests"], 4)

    def test_s09_3_needs_insertion_formatting_in_every_mode_and_leaves_printing_and_s09_4_alone(self):
        for mode in MODES:
            changed = copy.deepcopy(self.modes)
            changed[mode]["insertion"] = False
            report = {"metrics": {}}
            publish_metrics(report, changed, self.arena, self.manifest)
            self.assertFalse(report["metrics"]["api_scratch_disposal"])
            self.assertFalse(report["metrics"][f"api_scratch_disposal_{mode}"])
            self.assertEqual(report["metrics"]["api_scratch_tests"], 0)
            self.assertTrue(report["metrics"]["api_print_scratch_disposal"])
            self.assertTrue(all(report["metrics"][criterion] for criterion in CRITERIA))

    def test_stale_or_missing_insertion_fixture_blocks_that_suite_before_cargo_only(self):
        for error in (ValueError("stale insertion rows"), OSError("missing native rows")):
            calls = []
            self.verify_insertion.reset_mock()
            self.verify_insertion.side_effect = error

            def invoke(root, args, env):
                package = args[args.index("--package") + 1]
                suite = next(suite for suite in self.manifest["suites"].values()
                             if suite["package"] == package and suite["filter"] in args)
                calls.append((package, suite["filter"]))
                return suite_output(suite["cases"])

            with self.subTest(error=type(error).__name__), redirect_stderr(io.StringIO()):
                outcomes = measure(Path("synthetic-root"), invoke, ["cargo"], [], {}, self.manifest, "debug")
            self.verify_insertion.assert_called_once_with(root=Path("synthetic-root"))
            self.assertEqual(outcomes, {**{name: True for name in SUITES}, "insertion": False})
            self.assertEqual(calls, [SUITES[name] for name in SUITES if name != "insertion"])
        self.verify_insertion.side_effect = None

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
        cases_by_package["tsr_api"].append("printing::tests::new_unreviewed_case")
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
        for unavailable in ("independent_checker_merges",
                            "live_owner_delta", "live_allocation_delta", "miri", "address_sanitizer"):
            self.assertNotIn(unavailable, report["metrics"])

    def test_each_retention_criterion_is_one_suite_in_every_mode_and_scored_apart_from_s09_4(self):
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.arena, self.manifest)
        self.assertEqual(set(RETENTION), {"checker_result_retention", "checker_ast_retention",
                                          "builder_cache_retention"})
        self.assertTrue(all(report["metrics"][criterion] for criterion in RETENTION))
        self.assertEqual(report["metrics"]["checker_retention_tests"], 6)
        for criterion, suite in RETENTION.items():
            others = [name for name in RETENTION if name != criterion]
            for mode in MODES:
                changed = copy.deepcopy(self.modes)
                changed[mode][suite] = False
                report = {"metrics": {}}
                publish_metrics(report, changed, self.arena, self.manifest)
                with self.subTest(criterion=criterion, mode=mode):
                    self.assertFalse(report["metrics"][criterion])
                    self.assertFalse(report["metrics"][f"{criterion}_{mode}"])
                    self.assertTrue(all(report["metrics"][name] for name in others),
                                    "the sibling criteria keep their own outcomes")
                    self.assertTrue(all(report["metrics"][name] for name in CRITERIA), "and so does S09-4")
                    self.assertEqual(report["metrics"]["checker_retention_tests"], 4)
                    self.assertEqual(report["metrics"]["checker_ownership_tests"], 6)
        # A pool or registry failure is S09-4's to report; the retention suites
        # ran and passed, so their criteria are not falsified by association.
        for suite in OWNERSHIP_SUITES:
            changed = copy.deepcopy(self.modes)
            changed["release"][suite] = False
            report = {"metrics": {}}
            publish_metrics(report, changed, self.arena, self.manifest)
            self.assertFalse(report["metrics"]["shared_pool_panic_retirement"])
            self.assertTrue(all(report["metrics"][criterion] for criterion in RETENTION))

    def test_a_missing_retention_suite_cannot_claim_success(self):
        for suite in RETENTION.values():
            for mode in MODES:
                changed = copy.deepcopy(self.modes)
                del changed[mode][suite]
                with self.assertRaises(ValueError):
                    publish_metrics({"metrics": {}}, changed, self.arena, self.manifest)
            manifest = copy.deepcopy(self.manifest)
            del manifest["suites"][suite]
            with self.assertRaises(ValueError):
                validate_manifest(manifest)

    def test_ci_rejects_each_s09_suite_failure_in_every_mode(self):
        workflow = (ROOT / ".github/workflows/status.yml").read_text()
        required = re.findall(r"'run\.e3\.(\w+) == true'", workflow)
        for suite in SUITES:
            for mode in MODES:
                modes = copy.deepcopy(self.modes)
                modes[mode][suite] = False
                report = {"metrics": {}}
                publish_metrics(report, modes, self.arena, self.manifest)
                with self.subTest(suite=suite, mode=mode):
                    self.assertTrue(any(report["metrics"].get(name) is False for name in required),
                                    "CI must reject the producer's valid failing result")

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


class OwnershipInputsAndCI(unittest.TestCase):
    def test_e3_fingerprints_the_test_packages_dependency_closure_and_insertion_inputs(self):
        runs = tomllib.loads((ROOT / "status/runs.toml").read_text())

        def tracked(*patterns):
            return set(subprocess.check_output(
                ["git", "ls-files", "-z", "--", *patterns], cwd=ROOT
            ).decode().rstrip("\0").split("\0")) - {""}

        covered = tracked(*(f":(top,glob){pattern}" for pattern in runs["e3"]["sources"]))
        suite_manifests = {(ROOT / "crates" / package / "Cargo.toml").resolve()
                           for package, _ in SUITES.values()}
        pending = list(suite_manifests)
        visited = set()
        required = set()
        while pending:
            manifest = pending.pop().resolve()
            if manifest in visited:
                continue
            visited.add(manifest)
            required |= tracked(str(manifest.parent.relative_to(ROOT)))
            package = tomllib.loads(manifest.read_text())
            scopes = [package, *package.get("target", {}).values()]
            # Cargo builds dev dependencies only for the selected test
            # packages, not for every library in their dependency graph.
            sections = ["dependencies", "build-dependencies"]
            if manifest in suite_manifests:
                sections.append("dev-dependencies")
            for scope in scopes:
                for section in sections:
                    for dependency in scope.get(section, {}).values():
                        if isinstance(dependency, dict) and "path" in dependency:
                            pending.append(manifest.parent / dependency["path"] / "Cargo.toml")
        required |= tracked("tools/s09/format_oracle")
        required |= {"scripts/s09_format.py", "data/s09/insertion-cases.json",
                     "data/s09/insertion-observations.json"}
        self.assertFalse(required - covered, f"unfingerprinted E3 inputs: {sorted(required - covered)}")

    def test_ci_runs_navigation_and_formatter_regressions_in_both_profiles(self):
        workflow = (ROOT / ".github/workflows/status.yml").read_text()
        commands = [line.strip() for line in workflow.splitlines() if "cargo test " in line]
        for package in ("tsr_astnav", "tsr_format"):
            for release in (False, True):
                with self.subTest(package=package, release=release):
                    self.assertTrue(any(f"-p {package} " in line and
                                        ("--release" in line) == release for line in commands))


if __name__ == "__main__":
    unittest.main()
