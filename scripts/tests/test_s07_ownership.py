import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import copy
from contextlib import redirect_stderr
import io
import unittest

from s07_ownership import COMMON, GROUPS, MODES, load_cases, measure, publish_metrics, validate_manifest


def suite_output(cases):
    tests = "\n".join(f"test {name} ... ok" for name in cases)
    return (f"running {len(cases)} tests\n{tests}\n"
            f"test result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n").encode()


class OwnershipScope(unittest.TestCase):
    def setUp(self):
        self.manifest = load_cases(Path(__file__).resolve().parents[2])
        self.modes = {mode: {suite: True for suite in (*COMMON, *GROUPS)} for mode in MODES}

    def test_inventory_keeps_existing_cases_and_explicitly_includes_local_scope_suites(self):
        self.assertEqual(self.manifest["version"], 3)
        self.assertEqual(len(self.manifest["common"]["binding_publication"]["cases"]), 18)
        self.assertTrue({
            "bind_tests::exclusive_node_reads_observe_mutations_and_reject_unretained_owners",
            "bind_tests::exclusive_node_reads_route_new_lazy_records_and_reject_failed_slots",
            "bind_tests::synthetic_factory_retains_completed_edges_after_caller_drops",
        }.issubset(self.manifest["common"]["binding_publication"]["cases"]))
        self.assertEqual(len(self.manifest["common"]["exclusive_binding"]["cases"]), 10)
        self.assertEqual(len(self.manifest["common"]["core_validation_proof"]["cases"]), 9)
        for name, count in (("local_ast", 10), ("local_ast_core", 21), ("local_binder", 9),
                            ("local_flow_ids", 2), ("local_symbol_ids", 3)):
            self.assertEqual(len(self.manifest["common"][name]["cases"]), count)
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.manifest)
        self.assertEqual(report["metrics"]["program_ownership_tests"], 84)

    def test_missing_or_retargeted_common_inventory_is_rejected(self):
        for name in COMMON:
            missing = copy.deepcopy(self.manifest)
            del missing["common"][name]
            with self.assertRaises(ValueError):
                validate_manifest(missing)
        legacy = copy.deepcopy(self.manifest)
        legacy["version"] = 1
        legacy["common"] = legacy["common"]["binding_publication"]
        with self.assertRaises(ValueError):
            validate_manifest(legacy)
        legacy = copy.deepcopy(self.manifest)
        legacy["version"] = 2
        with self.assertRaises(ValueError):
            validate_manifest(legacy)
        retargeted = copy.deepcopy(self.manifest)
        retargeted["common"]["exclusive_binding"]["package"] = "ts_ast"
        with self.assertRaisesRegex(ValueError, "changed scope"):
            validate_manifest(retargeted)

    def test_malformed_suite_names_types_or_denominators_are_rejected(self):
        for cases in ([], [None], ["bad name"], ["exclusive_tests::a", "exclusive_tests::a"],
                      ["exclusive_tests::b", "exclusive_tests::a"], ["bind_tests::wrong_suite"]):
            manifest = copy.deepcopy(self.manifest)
            manifest["common"]["exclusive_binding"]["cases"] = cases
            with self.assertRaises(ValueError):
                validate_manifest(manifest)
        manifest = copy.deepcopy(self.manifest)
        manifest["version"] = True
        with self.assertRaises(ValueError):
            validate_manifest(manifest)

    def test_skips_must_be_declared_and_cannot_exclude_an_inventoried_case(self):
        for skips in (None, "local_bind_tests::", [False], [""], ["bind_tests::"],
                      ["local_bind_tests::", "local_bind_tests::"], [],
                      ["local_bind_tests::", "unrelated::"]):
            manifest = copy.deepcopy(self.manifest)
            manifest["common"]["binding_publication"]["skip"] = skips
            with self.subTest(skips=skips), self.assertRaises(ValueError):
                validate_manifest(manifest)
        manifest = copy.deepcopy(self.manifest)
        del manifest["common"]["binding_publication"]["skip"]
        with self.assertRaises(ValueError):
            validate_manifest(manifest)

    def test_substring_collision_is_executed_in_its_own_group_without_hiding_new_extras(self):
        suites = {**self.manifest["common"], **self.manifest["groups"]}
        inventories = {}
        for suite in suites.values():
            inventories.setdefault(suite["package"], []).extend(suite["cases"])
        calls = {}

        def invoke(root, args, env):
            package = args[args.index("--package") + 1]
            filter_value = args[args.index("--") - 1]
            skips = [args[index + 1] for index, value in enumerate(args) if value == "--skip"]
            exact = "--exact" in args
            cases = sorted(name for name in inventories[package]
                           if (name == filter_value if exact else filter_value in name)
                           and not any(skip in name for skip in skips))
            calls[filter_value] = cases
            return suite_output(cases)

        with redirect_stderr(io.StringIO()):
            result = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
        self.assertTrue(all(result.values()))
        self.assertEqual(calls["bind_tests::"], self.manifest["common"]["binding_publication"]["cases"])
        self.assertEqual(calls["local_bind_tests::"], self.manifest["common"]["local_ast"]["cases"])
        inventories["ts_ast"].append("unreviewed_bind_tests::unexpected")
        with redirect_stderr(io.StringIO()):
            result = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
        self.assertFalse(result["binding_publication"])
        self.assertTrue(result["local_ast"])

    def test_local_scope_outcomes_reject_missing_duplicate_failed_and_extra_cases(self):
        suites = {**self.manifest["common"], **self.manifest["groups"]}
        affected = self.manifest["common"]["local_ast_core"]
        cases = affected["cases"]
        for change in ("missing", "duplicate", "failed", "extra"):
            def invoke(root, args, env):
                selected = args[args.index("--") - 1]
                suite = next(suite for suite in suites.values() if suite["filter"] == selected)
                if suite != affected:
                    return suite_output(suite["cases"])
                if change == "missing":
                    return suite_output(cases[1:])
                if change == "duplicate":
                    return suite_output(cases + cases[:1])
                if change == "extra":
                    return suite_output(cases + [affected["filter"] + "unreviewed::new_test"])
                return suite_output(cases).replace(b" ... ok", b" ... FAILED", 1)

            with self.subTest(change=change), redirect_stderr(io.StringIO()):
                modes = copy.deepcopy(self.modes)
                modes["miri"] = measure(Path("."), invoke, ["cargo", "+nightly-test", "miri"],
                                        ["--target", "native"], {}, self.manifest, "miri")
                self.assertFalse(modes["miri"]["local_ast_core"])
                report = {"metrics": {}}
                publish_metrics(report, modes, self.manifest)
                self.assertFalse(report["metrics"]["shared_bound_file"])
                self.assertFalse(report["metrics"]["retained_snapshot_edit"])
                self.assertEqual(report["metrics"]["program_ownership_tests"], 63)

    def test_all_suites_execute_in_every_mode_with_unchanged_instrumentation_arguments(self):
        suites = {**self.manifest["common"], **self.manifest["groups"]}
        calls = []

        def invoke(root, args, env):
            suite_name = next(name for name, suite in suites.items() if suite["filter"] in args)
            suite = suites[suite_name]
            self.assertEqual(args[args.index("--package") + 1], suite["package"])
            self.assertIn("--test-threads=1", args)
            self.assertEqual("--exact" in args, suite["exact"])
            skips = [args[index + 1] for index, argument in enumerate(args) if argument == "--skip"]
            self.assertEqual(skips, suite["skip"])
            calls.append((suite_name, args, env))
            return suite_output(suite["cases"])

        modes = {}
        variants = (("debug", ["cargo"], []), ("release", ["cargo"], ["--release"]),
                    ("miri", ["cargo", "+nightly-test", "miri"], ["--target", "native"]),
                    ("address_sanitizer", ["cargo", "+nightly-test"], ["-Zbuild-std", "--target", "native"]))
        with redirect_stderr(io.StringIO()):
            for mode, prefix, options in variants:
                environment = {"mode": mode}
                modes[mode] = measure(Path("."), invoke, prefix, options, environment, self.manifest, mode)
                for _, args, env in calls[-len(suites):]:
                    self.assertEqual(args[:len(prefix)], prefix)
                    self.assertEqual(env, environment)
                    self.assertTrue(all(argument in args for argument in options))
        self.assertEqual(modes, self.modes)
        self.assertEqual(len(calls), len(suites) * 4)
        self.assertTrue(all(sum(name == called for called, _, _ in calls) == 4 for name in suites))

    def test_failed_new_suite_keeps_other_observations_but_blocks_both_groups(self):
        suites = {**self.manifest["common"], **self.manifest["groups"]}

        def invoke(root, args, env):
            suite = next(suite for suite in suites.values() if suite["filter"] in args)
            output = suite_output(suite["cases"])
            if suite["filter"] == "exclusive_tests::":
                return output.replace(b" ... ok", b" ... FAILED", 1)
            return output

        with redirect_stderr(io.StringIO()):
            self.modes["miri"] = measure(Path("."), invoke, ["cargo", "+nightly-test", "miri"],
                                         ["--target", "native"], {}, self.manifest, "miri")
        self.assertFalse(self.modes["miri"]["exclusive_binding"])
        self.assertTrue(self.modes["miri"]["core_validation_proof"])
        self.assertTrue(self.modes["miri"]["shared_bound_file"])
        self.assertFalse(all(self.modes["miri"].values()))
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.manifest)
        self.assertFalse(report["metrics"]["shared_bound_file"])
        self.assertFalse(report["metrics"]["retained_snapshot_edit"])
        self.assertFalse(report["metrics"]["shared_bound_file_miri"])
        self.assertEqual(report["metrics"]["program_ownership_tests"], 74)

    def test_each_common_suite_is_required_in_each_mode(self):
        for mode in MODES:
            for suite in COMMON:
                with self.subTest(mode=mode, suite=suite):
                    modes = copy.deepcopy(self.modes)
                    modes[mode][suite] = False
                    report = {"metrics": {}}
                    publish_metrics(report, modes, self.manifest)
                    self.assertFalse(report["metrics"]["shared_bound_file"])
                    self.assertFalse(report["metrics"]["retained_snapshot_edit"])
                    del modes[mode][suite]
                    with self.assertRaises(ValueError):
                        publish_metrics({"metrics": {}}, modes, self.manifest)

    def test_missing_or_failed_lookup_or_retention_case_cannot_publish_ownership_success(self):
        suites = {**self.manifest["common"], **self.manifest["groups"]}
        cases = self.manifest["common"]["binding_publication"]["cases"]
        new_cases = [case for case in cases if case.startswith("bind_tests::exclusive_node_reads_")]
        new_cases.append("bind_tests::synthetic_factory_retains_completed_edges_after_caller_drops")
        for affected in new_cases:
            for failed in (False, True):
                def invoke(root, args, env):
                    suite = next(suite for suite in suites.values() if suite["filter"] in args)
                    if suite["filter"] != "bind_tests::":
                        return suite_output(suite["cases"])
                    if failed:
                        return suite_output(cases).replace(
                            f"test {affected} ... ok".encode(), f"test {affected} ... FAILED".encode())
                    return suite_output([case for case in cases if case != affected])

                with self.subTest(case=affected, failed=failed), redirect_stderr(io.StringIO()):
                    modes = copy.deepcopy(self.modes)
                    modes["debug"] = measure(Path("."), invoke, ["cargo"], [], {}, self.manifest, "debug")
                    self.assertFalse(modes["debug"]["binding_publication"])
                    report = {"metrics": {}}
                    publish_metrics(report, modes, self.manifest)
                    self.assertFalse(report["metrics"]["shared_bound_file"])
                    self.assertFalse(report["metrics"]["retained_snapshot_edit"])
                    self.assertEqual(report["metrics"]["program_ownership_tests"], 66)

    def test_old_two_group_success_shape_and_nonboolean_observations_fail(self):
        old = {mode: {group: True for group in GROUPS} for mode in MODES}
        with self.assertRaises(ValueError):
            publish_metrics({"metrics": {}}, old, self.manifest)
        for value in (1, 0, None, "true", [], {}):
            changed = copy.deepcopy(self.modes)
            changed["miri"]["exclusive_binding"] = value
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, changed, self.manifest)

    def test_group_failure_does_not_certify_it_or_later_checker_work(self):
        self.modes["miri"]["retained_snapshot_edit"] = False
        report = {"metrics": {}}
        publish_metrics(report, self.modes, self.manifest)
        self.assertTrue(report["metrics"]["shared_bound_file"])
        self.assertFalse(report["metrics"]["retained_snapshot_edit"])
        self.assertNotIn("independent_checker_merges", report["metrics"])
        self.assertNotIn("type_footprint_ratio", report["metrics"])

    def test_missing_instrumentation_or_group_cannot_publish_success(self):
        for missing in self.modes:
            modes = copy.deepcopy(self.modes)
            del modes[missing]
            with self.assertRaises(ValueError):
                publish_metrics({"metrics": {}}, modes, self.manifest)
        del self.modes["miri"]["shared_bound_file"]
        with self.assertRaises(ValueError):
            publish_metrics({"metrics": {}}, self.modes, self.manifest)


if __name__ == "__main__":
    unittest.main()
