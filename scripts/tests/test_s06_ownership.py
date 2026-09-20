import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s04_ownership
import s06_ownership
from test_s04_ownership import fixture, ast_suite_output, successful_invoke


class AstOwnershipProducerTests(unittest.TestCase):
    def test_inventory_names_the_storage_module_without_a_function_prefix_convention(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            cases = ["storage_tests::borrowed_node_slice_rejection", "storage_tests::storage_owner"]
            (root / s06_ownership.CASE_MANIFEST).write_text(json.dumps(cases))
            self.assertEqual(s06_ownership.load_cases(root), cases)

    def test_inventory_requires_sorted_unique_nonempty_exact_names(self):
        for cases in ([], ["other::test"], ["not_storage_tests::owner"], ["storage_tests::"],
                      ["storage_tests::nested::owner"], [False], ["storage_tests::storage_b", "storage_tests::storage_a"],
                      ["storage_tests::storage_a"] * 2):
            with self.subTest(cases=cases), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                fixture(root)
                (root / s06_ownership.CASE_MANIFEST).write_text(json.dumps(cases))
                with self.assertRaisesRegex(ValueError, "inventory"):
                    s06_ownership.load_cases(root)

    def test_missing_failed_ignored_duplicate_extra_and_zero_tests_are_rejected_by_name(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            cases = s06_ownership.load_cases(root)
            output = ast_suite_output(root)
            first = cases[0].encode()
            row = b"test " + first + b" ... ok\n"
            mutations = (
                (output.replace(row, b""), cases[0]),
                (output.replace(first + b" ... ok", first + b" ... FAILED"), cases[0]),
                (output.replace(first + b" ... ok", first + b" ... ignored"), cases[0]),
                (output + row, cases[0]),
                (output + b"test storage_tests::unexpected ... ok\n", "unexpected"),
                (output.replace(b"0 ignored", b"1 ignored"), "summary"),
                (f"running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; {len(cases)} filtered out;\n".encode(), cases[0]),
                (output + output, "duplicate"),
                (b"\xff", None),
            )
            for changed, diagnostic in mutations:
                with self.subTest(diagnostic=diagnostic), contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(ValueError) as error:
                        s06_ownership.validate_output(changed, cases, "release")
                    if diagnostic:
                        self.assertIn(diagnostic, str(error.exception))

    def test_all_four_modes_execute_the_frozen_named_subset_and_gate_the_aggregate(self):
        calls = []
        def invoke(root, args, env=None):
            calls.append((args, env))
            return successful_invoke(root, args, env)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixture(root)
            with patch.object(s04_ownership, "invoke", invoke):
                report = s04_ownership.run(root)
            self.assertEqual(report["metrics"]["ast_runtime_tests"], len(s06_ownership.load_cases(root)))
        runs = [(args, env) for args, env in calls
                if "tsr_ast" in args and "storage_tests::" in args]
        self.assertEqual(len(runs), 4)
        for args, _ in runs:
            self.assertIn("storage_tests::", args)
            self.assertEqual(args[-3:], ["--", "--test-threads=1", "--nocapture"])
        self.assertIn("--release", runs[1][0])
        self.assertEqual(runs[2][1]["MIRIFLAGS"], "-Zmiri-strict-provenance")
        self.assertEqual(runs[3][1]["RUSTFLAGS"], "-Zsanitizer=address")
        self.assertTrue(report["metrics"]["ast_runtime"])
        self.assertEqual(set(report["tests"]), set(s04_ownership.SCENARIOS))
        for mode in ("debug", "release", "miri", "address_sanitizer"):
            self.assertTrue(report["metrics"]["ast_runtime_" + mode])
            for group in s04_ownership.s07_ownership.GROUPS:
                self.assertTrue(report["metrics"][group + "_" + mode])

    def test_each_mode_failure_is_false_and_does_not_skip_independent_modes(self):
        for failed_mode in ("debug", "release", "miri", "address_sanitizer"):
            for failure in ("missing", "failed", "ignored", "command"):
                calls = []
                def invoke(root, args, env=None):
                    if "tsr_ast" not in args or "storage_tests::" not in args:
                        return successful_invoke(root, args, env)
                    mode = ("miri" if "miri" in args else "address_sanitizer" if "-Zbuild-std" in args
                            else "release" if "--release" in args else "debug")
                    calls.append(mode)
                    if mode != failed_mode:
                        return ast_suite_output(root)
                    if failure == "command":
                        raise RuntimeError("instrumented storage scenario assertion failed")
                    output = ast_suite_output(root)
                    name = s06_ownership.load_cases(root)[0].encode()
                    if failure == "missing":
                        return output.replace(b"test " + name + b" ... ok\n", b"")
                    return output.replace(name + b" ... ok", name + b" ... " + failure.encode())
                with self.subTest(mode=failed_mode, failure=failure), tempfile.TemporaryDirectory() as temp:
                    root = Path(temp)
                    fixture(root)
                    with patch.object(s04_ownership, "invoke", invoke):
                        report = s04_ownership.run(root)
                    self.assertEqual(calls, ["debug", "release", "miri", "address_sanitizer"])
                    self.assertFalse(report["metrics"]["ast_runtime"])
                    for mode in calls:
                        self.assertIs(report["metrics"]["ast_runtime_" + mode], mode != failed_mode)
                        for group in s04_ownership.s07_ownership.GROUPS:
                            self.assertTrue(report["metrics"][group + "_" + mode])
                    if failed_mode in ("miri", "address_sanitizer"):
                        self.assertFalse(report["metrics"][failed_mode])
                    # Measured leaf outcomes remain separately identifiable.
                    self.assertEqual(set(report["tests"].values()), {"pass"})


if __name__ == "__main__":
    unittest.main()
