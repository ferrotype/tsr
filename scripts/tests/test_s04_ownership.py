import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[1] / "s04_ownership.py"
sys.path.insert(0, str(SOURCE.parent))
import s04_common
import s04_runtime

SPEC = importlib.util.spec_from_file_location("s04_ownership_tests_module", SOURCE)
ownership = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ownership)


def measured_report():
    return {"metrics": {**{name: True for name in ownership.SCENARIOS[:5]},
                        "live_owner_delta": 0, "live_allocation_delta": 0},
            "tests": [{"id": name, "result": "pass"} for name in ownership.SCENARIOS]}


def suite_output():
    tests = "\n".join(f"test tests::e3_{name} ... ok" for name in ownership.SCENARIOS)
    return (tests + "\ntest result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n").encode()


def named_suite_output(cases):
    rows = "\n".join(f"test {name} ... ok" for name in cases)
    return (f"running {len(cases)} tests\n" + rows +
            f"\ntest result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 0.00s\n").encode()


def ast_suite_output(root):
    return named_suite_output(ownership.s06_ownership.load_cases(root))


def program_manifest():
    # Minimal independent S07 suites keep these legacy producer tests focused on
    # their S04/S06 failures without bypassing S07 inventory/output validation.
    return {
        "version": 3,
        "common": {
            name: {"package": package, "filter": prefix, "exact": False,
                   "skip": ownership.s07_ownership.SKIPS.get(name, []),
                   "cases": [prefix + "publication"]}
            for name, (package, prefix) in ownership.s07_ownership.COMMON.items()
        },
        "groups": {
            name: {"package": "ts_compiler", "filter": f"ownership_tests::{name}",
                   "exact": True, "skip": [], "cases": [f"ownership_tests::{name}"]}
            for name in ownership.s07_ownership.GROUPS
        },
    }


def checker_manifest():
    return {"version": 1, "suites": {
        name: {"package": package, "filter": prefix, "cases": [prefix + "generation_boundary"]}
        for name, (package, prefix) in ownership.s09_ownership.SUITES.items()
    }}


def fixture(root):
    (root / "data/s04").mkdir(parents=True)
    (root / "data/s06").mkdir(parents=True)
    (root / "data/s07").mkdir(parents=True)
    (root / "data/s09").mkdir(parents=True)
    inventory = SOURCE.parent.parent / "data/s06/ownership-cases.json"
    (root / "data/s06/ownership-cases.json").write_bytes(inventory.read_bytes())
    (root / "data/s07/ownership-cases.json").write_text(json.dumps(program_manifest()))
    (root / "data/s09/ownership-cases.json").write_text(json.dumps(checker_manifest()))
    (root / "data/s04/e3-cases.json").write_text(json.dumps(sorted(ownership.SCENARIOS)))
    (root / "data/s04/toolchains.toml").write_text(
        'nightly = "nightly-2026-09-05"\ngo = "go1.27.1"\nmsrv = "1.96.0"\n')
    (root / "Cargo.toml").write_text('[workspace.package]\nrust-version = "1.96"\n')


def successful_invoke(root, args, env=None):
    if "-Vv" in args:
        return b"rustc test\nhost: aarch64-apple-darwin\n"
    if "component" in args:
        return b"miri-aarch64-apple-darwin\nrust-src\n"
    if "run" in args and "e3" in args:
        return json.dumps(measured_report()).encode()
    if "setup" in args:
        return b""
    manifest = ownership.s07_ownership.load_cases(root)
    for suite in [*manifest["common"].values(), *manifest["groups"].values()]:
        if suite["package"] in args and suite["filter"] in args:
            return named_suite_output(suite["cases"])
    if "ts_ast" in args and "storage_tests::" in args:
        return ast_suite_output(root)
    for suite in ownership.s09_ownership.load_cases(root)["suites"].values():
        if suite["package"] in args and suite["filter"] in args:
            return named_suite_output(suite["cases"])
    return suite_output()


class OwnershipProducerTests(unittest.TestCase):
    def test_missing_registry_case_blocks_s09_and_instrumentation_without_hiding_other_modes(self):
        for mode in ("debug", "release", "miri", "address_sanitizer"):
            def invoke(root, args, env=None):
                actual_mode = ("miri" if "miri" in args else "address_sanitizer" if "-Zbuild-std" in args
                               else "release" if "--release" in args else "debug")
                if "ts_api" in args and actual_mode == mode:
                    return named_suite_output([])
                return successful_invoke(root, args, env)

            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                fixture(root)
                with patch.object(ownership, "invoke", invoke):
                    report = ownership.run(root)
                self.assertFalse(report["metrics"]["shared_pool_panic_retirement"])
                self.assertFalse(report["metrics"]["release_boundaries"])
                self.assertFalse(report["metrics"][f"checker_ownership_registry_{mode}"])
                self.assertTrue(report["metrics"][f"checker_ownership_pool_{mode}"])
                self.assertEqual(report["metrics"]["checker_ownership_tests"], 2)
                self.assertEqual(report["metrics"]["miri"], mode != "miri")
                self.assertEqual(report["metrics"]["address_sanitizer"], mode != "address_sanitizer")

    def test_arena_boundary_failure_cannot_be_hidden_by_passing_pool_tests(self):
        def invoke(root, args, env=None):
            if "miri" in args and "ts_arena" in args and "lease::" not in args:
                raise RuntimeError("arena owner boundary failed")
            return successful_invoke(root, args, env)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke):
                report = ownership.run(root)
        self.assertTrue(report["metrics"]["shared_pool_panic_retirement"])
        self.assertFalse(report["metrics"]["release_boundaries"])
        self.assertFalse(report["metrics"]["release_boundaries_miri"])
        self.assertTrue(report["metrics"]["release_boundaries_release"])

    def test_missing_exclusive_suite_cannot_leave_shared_e3_metrics_passing(self):
        def invoke(root, args, env=None):
            if "miri" in args and "exclusive_tests::" in args:
                return b"running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n"
            return successful_invoke(root, args, env)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke):
                report = ownership.run(root)
        self.assertFalse(report["metrics"]["miri"])
        self.assertFalse(report["metrics"]["shared_bound_file"])
        self.assertFalse(report["metrics"]["retained_snapshot_edit"])
        self.assertFalse(report["metrics"]["shared_bound_file_miri"])
        self.assertTrue(report["metrics"]["shared_bound_file_address_sanitizer"])
        self.assertEqual(report["metrics"]["program_ownership_tests"],
                         len(ownership.s07_ownership.COMMON) + len(ownership.s07_ownership.GROUPS) - 1)

    def test_failed_validation_proof_suite_cannot_leave_sanitizer_metrics_passing(self):
        def invoke(root, args, env=None):
            if "-Zbuild-std" in args and "storage::validation_proof_tests::" in args:
                raise RuntimeError("validation proof invariant failed under ASan")
            return successful_invoke(root, args, env)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke):
                report = ownership.run(root)
        self.assertFalse(report["metrics"]["address_sanitizer"])
        self.assertFalse(report["metrics"]["shared_bound_file"])
        self.assertFalse(report["metrics"]["retained_snapshot_edit"])
        self.assertTrue(report["metrics"]["miri"])
        self.assertEqual(report["metrics"]["program_ownership_tests"],
                         len(ownership.s07_ownership.COMMON) + len(ownership.s07_ownership.GROUPS) - 1)

    def test_duplicate_and_nonfinite_json_cannot_hide_failed_measurements(self):
        for text in ('{"live_owner_delta":2,"live_owner_delta":0}',
                     '{"live_owner_delta":NaN}', '{"live_owner_delta":Infinity}',
                     '{"live_owner_delta":1e999}'):
            with self.assertRaises(ValueError):
                ownership.strict_json_loads(text)

    def test_native_example_cannot_claim_unimplemented_criteria(self):
        report = measured_report()
        report["metrics"]["shared_pool_panic_retirement"] = True
        with self.assertRaises(ValueError):
            ownership.validate_measurements(report)

    def test_requires_actual_nonnegative_numeric_counters(self):
        for value in (None, False, "0", -1, 0.0):
            with self.subTest(value=value):
                report = measured_report()
                report["metrics"]["live_owner_delta"] = value
                with self.assertRaises(ValueError):
                    ownership.validate_measurements(report)

    def test_missing_duplicate_skipped_or_failed_scenario_cannot_pass(self):
        for mutate in (
            lambda rows: rows.pop(),
            lambda rows: rows.append(rows[0].copy()),
            lambda rows: rows[0].update(result="skip"),
            lambda rows: rows[0].update(result="fail"),
        ):
            report = measured_report()
            mutate(report["tests"])
            with self.assertRaises(ValueError):
                ownership.validate_measurements(report)

    def test_native_example_cannot_claim_instrumentation(self):
        for metric in ("miri", "address_sanitizer"):
            report = measured_report()
            report["metrics"][metric] = True
            with self.assertRaises(ValueError):
                ownership.validate_measurements(report)

    def test_zero_filtered_ignored_and_missing_named_tests_are_rejected(self):
        output = suite_output()
        for changed in (
            b"test result: ok. 0 passed; 0 failed; 0 ignored; 7 filtered out;\n",
            output.replace(b"0 ignored", b"1 ignored"),
            output.replace(b"e3_id_exhaustion", b"unrelated_test"),
            output.replace(b"e3_id_exhaustion ... ok", b"e3_id_exhaustion ... FAILED"),
        ):
            with self.assertRaises(ValueError):
                ownership.validate_test_output(changed)

    def test_measured_nonzero_delta_remains_a_failing_metric(self):
        report = measured_report()
        report["metrics"]["live_owner_delta"] = 2
        self.assertEqual(ownership.validate_measurements(report)["metrics"]["live_owner_delta"], 2)

    def test_sanitizer_flags_cannot_be_overridden_by_encoded_rustflags(self):
        calls = []

        def invoke(root, args, env=None):
            calls.append((args, copy.deepcopy(env)))
            return successful_invoke(root, args, env)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke), patch.dict(ownership.os.environ, {"CARGO_ENCODED_RUSTFLAGS": ""}):
                report = ownership.run(root)
        asan = [(args, env) for args, env in calls
                if "-Zbuild-std" in args and "ts_arena" in args and "lease::" not in args]
        self.assertEqual(len(asan), 1)
        args, env = asan[0]
        self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", env)
        self.assertEqual(env["RUSTFLAGS"], "-Zsanitizer=address")
        self.assertIn("--target", args)
        self.assertEqual(report["metrics"]["miri"], True)
        self.assertEqual(report["metrics"]["address_sanitizer"], True)
        self.assertEqual(report["tests"], {name: "pass" for name in ownership.SCENARIOS})

    def test_failed_instrumentation_never_returns_passing_report(self):
        # invoke is the shared failure boundary used for every child process.
        failed = subprocess.CompletedProcess([], 1, stdout=b"partial", stderr=b"instrumentation error")
        with patch.object(s04_common.subprocess, "run", return_value=failed):
            with self.assertRaises(RuntimeError):
                ownership.invoke(Path("."), ["cargo", "miri", "test"])

    def test_instrumentation_scrubs_flags_wrappers_sysroots_and_runners(self):
        injected = {
            "PATH": "/kept", "RUSTC": "/wrong/compiler", "RUSTC_WRAPPER": "/wrong/wrapper",
            "RUSTC_WORKSPACE_WRAPPER": "/wrong/workspace-wrapper", "RUSTUP_TOOLCHAIN": "nightly",
            "RUSTFLAGS": "--cfg bypass", "CARGO_ENCODED_RUSTFLAGS": "",
            "CARGO_BUILD_RUSTFLAGS": "--cfg bypass", "CARGO_BUILD_RUSTC": "/wrong/compiler",
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS": "--cfg bypass",
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER": "/wrong/runner",
            "MIRIFLAGS": "-Zmiri-disable-validation", "MIRI_SYSROOT": "/wrong/sysroot",
            "MIRI_BE_RUSTC": "1", "ASAN_OPTIONS": "start_deactivated=1",
            "LSAN_OPTIONS": "suppressions=/wrong/suppressions",
            "CARGO_HOME": "/configured/cargo", "RUSTUP_HOME": "/configured/rustup",
            "CARGO_NET_OFFLINE": "true", "CARGO_REGISTRIES_CRATES_IO_INDEX": "https://mirror.invalid/index",
        }
        env = ownership.instrumentation_environment(injected, Path("/workspace"))
        self.assertEqual(env, {"PATH": "/kept", "RUSTUP_HOME": "/configured/rustup",
                               "CARGO_HOME": "/configured/cargo", "CARGO_NET_OFFLINE": "true",
                               "CARGO_REGISTRIES_CRATES_IO_INDEX": "https://mirror.invalid/index"})

    def test_explicit_rustup_override_leaves_cargo_config_in_place(self):
        env = ownership.instrumentation_environment({"CARGO_HOME": "/mirror/cargo",
            "RUSTUP_HOME": "/user/rustup", "S04_RUSTUP_HOME": "/isolated/rustup"}, Path("/workspace"))
        self.assertEqual(env["RUSTUP_HOME"], "/isolated/rustup")
        self.assertEqual(env["CARGO_HOME"], "/mirror/cargo")

    def test_default_sysroot_cache_survives_target_pruning(self):
        cache = s04_runtime.cache_home(Path("/workspace"), {"HOME": "/user"})
        self.assertTrue(str(cache).startswith("/user/"))
        self.assertNotIn("target", cache.parts)
        self.assertEqual(s04_runtime.cache_home(base={"S04_CACHE_HOME": "/cache/s04"}), Path("/cache/s04"))

    def test_msrv_pin_must_match_workspace_declaration(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            self.assertEqual(s04_runtime.load_toolchains(root)["msrv"], "1.96.0")
            (root / "Cargo.toml").write_text('[workspace.package]\nrust-version = "1.97"\n')
            with self.assertRaisesRegex(ValueError, "workspace rust-version"):
                s04_runtime.load_toolchains(root)

    def test_native_failure_remains_named_even_when_later_tools_are_unavailable(self):
        failed = measured_report()
        failed["tests"][0].update(result="fail", error="expected exhaustion")
        failed["metrics"]["id_exhaustion"] = False
        del failed["metrics"]["live_owner_delta"]
        del failed["metrics"]["live_allocation_delta"]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", side_effect=[json.dumps(failed).encode(), FileNotFoundError()]) as invoke:
                report = ownership.run(root)
        self.assertEqual(invoke.call_count, 1)
        self.assertEqual(report["tests"]["id_exhaustion"], "fail")
        self.assertIs(report["metrics"]["id_exhaustion"], False)
        self.assertNotIn("miri", report["metrics"])
        self.assertNotIn("live_owner_delta", report["metrics"])

    def test_failed_scenario_cannot_claim_true_metric(self):
        report = measured_report()
        report["tests"][0].update(result="fail", error="wrong owner accepted")
        del report["metrics"]["live_owner_delta"]
        del report["metrics"]["live_allocation_delta"]
        with self.assertRaisesRegex(ValueError, "disagrees"):
            ownership.validate_measurements(report)

    def test_failed_scenario_cannot_claim_aggregate_counters(self):
        report = measured_report()
        report["tests"][0].update(result="fail", error="")
        report["metrics"]["id_exhaustion"] = False
        with self.assertRaises(ValueError):
            ownership.validate_measurements(report)
        del report["metrics"]["live_owner_delta"]
        del report["metrics"]["live_allocation_delta"]
        self.assertIs(ownership.validate_measurements(report)["metrics"]["id_exhaustion"], False)

    def test_instrumented_failure_is_measured_and_does_not_skip_other_mode(self):
        for failed_mode in ("miri", "address_sanitizer"):
            with self.subTest(mode=failed_mode):
                def invoke(root, args, env=None):
                    if ((failed_mode == "miri" and "miri" in args and "test" in args)
                            or (failed_mode == "address_sanitizer" and "-Zbuild-std" in args)):
                        raise RuntimeError("scenario assertion failed")
                    return successful_invoke(root, args, env)
                with tempfile.TemporaryDirectory() as temp:
                    root = Path(temp)
                    fixture(root)
                    with patch.object(ownership, "invoke", invoke):
                        report = ownership.run(root)
                self.assertIs(report["metrics"][failed_mode], False)
                other = "miri" if failed_mode == "address_sanitizer" else "address_sanitizer"
                self.assertIs(report["metrics"][other], True)

    def test_missing_instrumentation_component_is_a_failed_run(self):
        def invoke(root, args, env=None):
            if "component" in args:
                return b"rust-src\n"
            return successful_invoke(root, args, env)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke):
                with self.assertRaisesRegex(RuntimeError, "requires installed miri"):
                    ownership.run(root)

    def test_required_scenarios_run_under_strict_provenance(self):
        calls = []

        def invoke(root, args, env=None):
            calls.append((args, copy.deepcopy(env)))
            return successful_invoke(root, args, env)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(ownership, "invoke", invoke):
                ownership.run(root)
        runs = [(args, env) for args, env in calls
                if "miri" in args and "test" in args and "ts_arena" in args and "lease::" not in args]
        docs = [args for args, _ in calls if "--doc" in args]
        self.assertEqual(docs, [["cargo", "test", "--package", "ts_arena", "--doc", "--locked"]])
        self.assertEqual(len(runs), 1)
        args, env = runs[0]
        self.assertEqual(env["MIRIFLAGS"], "-Zmiri-strict-provenance")
        self.assertIn("--lib", args)
        self.assertIn("+nightly-2026-09-05", args)


if __name__ == "__main__":
    unittest.main()
