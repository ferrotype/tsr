import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import unittest

from s07_benchmark_stats import ratio_summary, sample_values


class FixedStatistics(unittest.TestCase):
    def test_clear_improvement_and_clear_regression_keep_all_samples(self):
        faster = ratio_summary([100] * 7, [70] * 7, timing=True)
        self.assertEqual(faster["ratio"], 0.7)
        self.assertTrue(faster["stable"])
        self.assertFalse(faster["needs_more"])
        slower = ratio_summary([100] * 7, [130] * 7, timing=True)
        self.assertEqual(slower["ratio"], 1.3)
        self.assertFalse(slower["stable"])
        self.assertFalse(slower["needs_more"])

    def test_timing_threshold_is_the_re_based_criterion_not_parity(self):
        # ADR 0021: 1.2x is stable under a 1.25 criterion and unstable under parity.
        rebased = ratio_summary([100] * 7, [120] * 7, timing=True, threshold=1.25)
        self.assertTrue(rebased["stable"])
        self.assertFalse(rebased["needs_more"])
        self.assertEqual(rebased["bootstrap"]["threshold"], 1.25)
        parity = ratio_summary([100] * 7, [120] * 7, timing=True)
        self.assertFalse(parity["stable"])
        for bad in (True, False, 0, -1.0, float("nan"), float("inf"), "1.25"):
            with self.assertRaises(ValueError):
                ratio_summary([100] * 7, [120] * 7, timing=True, threshold=bad)

    def test_extension_uses_the_criterion_boundary(self):
        rust = [120, 120, 120, 125, 125, 130, 130]
        parity = ratio_summary([100] * 7, rust, timing=True)
        rebased = ratio_summary([100] * 7, rust, timing=True, threshold=1.25)
        self.assertFalse(parity["needs_more"])
        self.assertTrue(rebased["needs_more"])
        boundary = ratio_summary([100] * 7, [125] * 7, timing=True, threshold=1.25)
        self.assertTrue(boundary["stable"])
        self.assertFalse(boundary["needs_more"])


class Thresholds(unittest.TestCase):
    def test_reads_both_modes_and_rejects_invalid_thresholds(self):
        import tempfile
        from unittest.mock import patch
        import s07_benchmark_measure as measure
        committed = (measure.ROOT / measure.THRESHOLDS).read_text()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / measure.THRESHOLDS
            path.parent.mkdir(parents=True)
            path.write_text(committed)
            with patch.object(measure, "ROOT", root):
                self.assertEqual(measure.e6_thresholds(), {"1": 1.25, "8": 1.45})
                for bad in ("true", "0", "-1", "nan", "inf", '"1.25"'):
                    path.write_text(committed.replace("one_thread_wall_time = 1.25", "one_thread_wall_time = " + bad))
                    with self.subTest(bad=bad), self.assertRaises(ValueError):
                        measure.e6_thresholds()
                path.write_text(committed.replace("eight_threads_wall_time = 1.45", ""))
                with self.assertRaisesRegex(ValueError, "eight_threads_wall_time"):
                    measure.e6_thresholds()
                path.write_text(committed.replace("[parse-bind]", "[parse-and-bind]"))
                with self.assertRaisesRegex(ValueError, r"\[parse-bind\]"):
                    measure.e6_thresholds()
                path.write_text(committed + "\n[parse-bind]\none_thread_wall_time = 1.0\n")
                with self.assertRaises(ValueError):
                    measure.e6_thresholds()

    def test_thresholds_file_is_a_capture_input(self):
        from s07_benchmark import source_fingerprint
        self.assertIn("status/perf/thresholds.toml", source_fingerprint()["files"])

    def test_noisy_capture_is_extended_then_remains_uncertain(self):
        values = [60, 70, 80, 100, 110, 120, 130]
        initial = ratio_summary([100] * 7, values, timing=True)
        self.assertTrue(initial["needs_more"])
        self.assertFalse(initial["stable"])
        final = ratio_summary([100] * 21, values * 3, timing=True)
        self.assertFalse(final["needs_more"])
        self.assertFalse(final["stable"])
        self.assertEqual(final["samples_per_runtime"], 21)

    def test_protocol_rejects_missing_extra_invalid_or_incomplete_batches(self):
        for values in ([], [True], [0], [-1], [float("nan")], [float("inf")], [10**500]):
            with self.assertRaises(ValueError):
                sample_values(values)
        for left, right in (([1] * 6, [1] * 6), ([1] * 7, [1] * 14), ([1] * 28, [1] * 28)):
            with self.assertRaises(ValueError):
                ratio_summary(left, right)

    def test_reproducible_bootstrap_and_outlier_inclusion(self):
        rust = [70, 72, 68, 71, 69, 70, 200]
        first = ratio_summary([100] * 7, rust, timing=True)
        second = ratio_summary([100] * 7, rust, timing=True)
        self.assertEqual(first, second)
        self.assertEqual(first["samples_per_runtime"], 7)
        self.assertGreater(first["bootstrap"]["upper"], 0.7)



class NativeSamples(unittest.TestCase):
    def sample(self, runtime="rust", workers=1, allocation=False):
        from s07_benchmark_measure import COUNTERS
        report = dict.fromkeys(COUNTERS, 1)
        report.update(version=1, loaded_input_sha256="0"*64, workers=workers, wall_time_ns=70 if runtime == "rust" else 100,
                      allocated_bytes=70 if allocation or runtime == "go" else None,
                      startup_ns=0, preload_ns=0, worker_setup_ns=0, cpu_capacity=8,
                      goroutines_ready=workers + 1 if runtime == "go" else None)
        if runtime == "go":
            report.update(gomaxprocs=workers, gogc=100)
        return {"report": report, "peak_rss_bytes": 100, "process_time_ns": 1000,
                "user_time_ns": 100, "system_time_ns": 1, "stderr": ""}

    def test_native_sample_rejects_wrong_instrumentation_and_false_numeric_values(self):
        from copy import deepcopy
        from s07_benchmark_measure import COUNTERS, validate_sample
        expected = {**dict.fromkeys(COUNTERS, 1), "loaded_input_sha256":"0"*64}
        valid = self.sample()
        validate_sample(valid, expected, 1, "rust", False)
        for field, value in (("version", True), ("workers", True), ("files", True),
                             ("cpu_capacity", 0), ("allocated_bytes", 500), ("goroutines_ready", 2),
                             ("wall_time_ns", -1), ("loaded_input_sha256", "1"*64), ("unknown", 0)):
            with self.subTest(field=field), self.assertRaises(ValueError):
                changed = deepcopy(valid)
                changed["report"][field] = value
                validate_sample(changed, expected, 1, "rust", False)
        for field, value in (("peak_rss_bytes", True), ("process_time_ns", float("nan")),
                             ("user_time_ns", -1), ("stderr", None)):
            with self.subTest(field=field), self.assertRaises(ValueError):
                changed = deepcopy(valid)
                changed[field] = value
                validate_sample(changed, expected, 1, "rust", False)

    def rows(self):
        return [{"workers": workers, "allocation": allocation, "runtime": runtime, "index": index,
                 "sample": self.sample(runtime, workers, allocation)}
                for workers in (1, 8) for allocation in (False, True) for index in range(7)
                for runtime in (("go", "rust") if index % 2 == 0 else ("rust", "go"))]

    def test_all_modes_pairs_and_outliers_are_required(self):
        from copy import deepcopy
        from s07_benchmark_measure import COUNTERS
        from s07_benchmark_report import validate_rows
        expected = {**dict.fromkeys(COUNTERS, 1), "loaded_input_sha256":"0"*64}
        valid = self.rows()
        summaries = validate_rows(valid, expected)
        self.assertEqual(summaries["1"]["wall_time_ns"]["ratio"], 0.7)
        bad_order = deepcopy(valid)
        bad_order[0], bad_order[1] = bad_order[1], bad_order[0]
        missing_mode = [row for row in valid if row["workers"] == 1]
        duplicate = valid[:1] + valid
        wrong_identity = deepcopy(valid)
        wrong_identity[-1]["index"] = 1
        manufactured_count = deepcopy(valid)
        manufactured_count[0]["sample"]["report"]["files"] = 2
        for rows in (valid[:-1], bad_order, missing_mode, duplicate, wrong_identity, manufactured_count):
            with self.assertRaises(ValueError):
                validate_rows(rows, expected)
        high_memory = deepcopy(valid)
        high_memory[0]["sample"]["peak_rss_bytes"] = 10000
        self.assertEqual(validate_rows(high_memory, expected)["1"]["peak_rss_bytes"]["samples_per_runtime"], 7)

    def test_replay_uses_each_modes_threshold_and_rejects_omitted_extension(self):
        from s07_benchmark_measure import COUNTERS
        from s07_benchmark_report import validate_rows
        expected = {**dict.fromkeys(COUNTERS, 1), "loaded_input_sha256": "0"*64}
        rows = self.rows()
        for row in rows:
            if row["runtime"] == "rust":
                row["sample"]["report"]["wall_time_ns"] = 120 if row["workers"] == 1 else 140
        summaries = validate_rows(rows, expected)
        for workers, threshold in (("1", 1.25), ("8", 1.45)):
            timing = summaries[workers]["wall_time_ns"]
            self.assertTrue(timing["stable"])
            self.assertEqual(timing["bootstrap"]["threshold"], threshold)
        for row in rows:
            if row["runtime"] == "rust" and row["workers"] == 1 and not row["allocation"]:
                row["sample"]["report"]["wall_time_ns"] = [120, 120, 120, 125, 125, 130, 130][row["index"]]
        with self.assertRaisesRegex(ValueError, "stopping/extension"):
            validate_rows(rows, expected)


class AllocatorPreflight(unittest.TestCase):
    def test_requires_exact_both_modes_and_measured_counts(self):
        from copy import deepcopy
        from s07_benchmark_measure import validate_allocation_preflight, ROOT
        from s04_runtime import load_toolchains
        rows = [dict(mode=mode, toolchain=load_toolchains(ROOT)["msrv"], version=1, workers=8, requested_bytes=9600400, expected_bytes=9600400, live_before=400, live_after=400) for mode in ("debug", "release")]
        validate_allocation_preflight(rows)
        changed = deepcopy(rows)
        changed[0]["version"] = True
        wrong_count = deepcopy(rows)
        wrong_count[0].update(requested_bytes=100, expected_bytes=100)
        leaked = deepcopy(rows)
        leaked[1]["live_after"] = 500
        for bad in ([], rows[:1], rows[::-1], changed, wrong_count, leaked):
            with self.assertRaises(ValueError):
                validate_allocation_preflight(bad)

if __name__ == "__main__":
    unittest.main()
