"""scripts/perf.py: run files from fake harness captures, the threshold check,
the thresholds file and the committed runs."""
from __future__ import annotations

import contextlib
import copy
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import perf  # noqa: E402

RUN_KEYS = {"workload", "pin", "revision", "host", "recorded_at", "ratios", "samples"}
REVISION = "0123456789abcdef0123456789abcdef01234567"
HOST = {"os": "darwin", "architecture": "arm64", "release": "25.6.0", "cpu_capacity": 18,
        "physical_cpus": 18, "memory_bytes": 68719476736, "initial_load_average": [1.0, 1.0, 1.0]}


def s07_rows():
    """samples.ndjson rows in the capture's order: every worker mode and
    instrumentation pass, seven alternating pairs, Go at 100 and Rust at 70
    (wall time at eight workers 140), so every ratio is known."""
    from s07_benchmark_measure import COUNTERS
    rows = []
    for workers in (1, 8):
        for allocation in (False, True):
            for index in range(7):
                for runtime in (("go", "rust") if index % 2 == 0 else ("rust", "go")):
                    rust = runtime == "rust"
                    report = dict.fromkeys(COUNTERS, 1)
                    report.update(version=1, loaded_input_sha256="0" * 64, workers=workers,
                                  wall_time_ns=(140 if workers == 8 else 70) + index if rust else 100 + index,
                                  allocated_bytes=(70 + index if rust else 100 + index) if allocation or not rust else None,
                                  startup_ns=0, preload_ns=0, worker_setup_ns=0, cpu_capacity=18,
                                  goroutines_ready=None if rust else workers + 1)
                    sample = {"report": report, "peak_rss_bytes": (70 if rust else 100) * 1000 + index,
                              "process_time_ns": 1000, "user_time_ns": 1, "system_time_ns": 1, "stderr": ""}
                    rows.append({"workers": workers, "allocation": allocation, "runtime": runtime,
                                 "index": index, "sample": sample})
    return rows


def s07_capture(directory):
    """A capture directory as `s07_benchmark.py capture` leaves it: report.json
    with the real aggregation of the rows, and samples.ndjson."""
    from s07_benchmark_measure import aggregate, metrics_from_summaries
    rows = s07_rows()
    summaries = aggregate(rows)
    report = {"version": 1, "host": HOST, "revision": REVISION, "summaries": summaries,
              "metrics": metrics_from_summaries(summaries), "samples": len(rows)}
    directory.mkdir(parents=True)
    (directory / "report.json").write_text(json.dumps(report))
    (directory / "samples.ndjson").write_text("".join(json.dumps(row) + "\n" for row in rows))
    return report, rows


def fake_read_capture(directory, graph_report):
    """Stands in for s07_benchmark_report.read_capture, whose validation needs
    the real binaries, sources and graph report: the same two values back."""
    directory = Path(directory)
    report = json.loads((directory / "report.json").read_text())
    rows = [json.loads(line) for line in (directory / "samples.ndjson").read_text().splitlines()]
    return report, rows


def checker_report(smoke=None, footprint=True):
    """The shape `s08_checkerbench.report` returns, three samples per runtime."""
    def stability(values):
        return {"samples": values, "median": sorted(values)[1], "unstable": False}
    census = {"go": {"type_storage_bytes": [300, 300, 300], "types_reachable": [10, 10, 10], "unavailable": [[], [], []]},
              "rust": {"type_storage_bytes": [240, 240, 240], "types_reachable": [10, 10, 10], "unavailable": [[], [], []]}}
    metrics = {"throughput_ratio": 0.5, "elapsed_ratio": 2.0, "allocated_bytes_ratio": 0.6,
               "retained_bytes_ratio": 1.4, "stable": True, "variants": 3, "digest_agreement": 3,
               "action_mismatches": 0, "go_interval_ns_median": 5, "rust_interval_ns_median": 10}
    unavailable = {}
    if footprint:
        metrics["type_footprint_ratio"] = 0.8
    else:
        unavailable["type_footprint_ratio"] = "a census failed on at least one variant"
    return {"version": 1, "smoke": smoke, "host": HOST, "metrics": metrics, "unavailable": unavailable,
            "modes": {"normal": {"interval_ns": {"go": stability([4, 5, 6]), "rust": stability([9, 10, 11])}},
                      "phase": {"interval_ns": {"go": stability([4, 5, 6]), "rust": stability([9, 10, 11])}},
                      "alloc": {"allocation": {"go": {"requested_bytes": [100, 100, 100], "retained_bytes": [50, 50, 50],
                                                      "allocation_calls": [1, 1, 1]},
                                               "rust": {"requested_bytes": [60, 60, 60], "retained_bytes": [70, 70, 70],
                                                        "allocation_calls": [None, None, None]}},
                                "census": census}}}


def run_file(workload="parse-bind", recorded_at="2026-10-01T00:00:00Z", host=None, **ratios):
    return {"workload": workload, "pin": "p" * 40, "revision": REVISION,
            "host": host or {"os": "macos", "arch": "aarch64", "cpus": 18, "label": "owner quiet host"},
            "recorded_at": recorded_at, "ratios": ratios, "samples": {}}


class TemporaryPerf(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.perf = self.directory / "status/perf"
        self.perf.mkdir(parents=True)
        for name, value in (("PERF", self.perf), ("THRESHOLDS", self.perf / "thresholds.toml")):
            patcher = patch.object(perf, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def main(self, *argv):
        """perf.main's exit code (0 when it returns), its stdout and its stderr."""
        out, err = io.StringIO(), io.StringIO()
        code = 0
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                perf.main(list(argv))
            except SystemExit as exit:
                code = exit.code if isinstance(exit.code, int) else 1
                err.write(str(exit.code))
        return code, out.getvalue(), err.getvalue()

    def written(self, workload):
        paths = sorted((self.perf / workload).glob("*.json"))
        self.assertEqual(len(paths), 1)
        return paths[0], json.loads(paths[0].read_text())


class RecordTests(TemporaryPerf):
    def test_parse_bind_capture_becomes_a_run_file(self):
        capture = self.directory / "s07-benchmark"
        report, rows = s07_capture(capture)
        with patch("s07_benchmark_report.read_capture", side_effect=fake_read_capture) as reader:
            code, out, _ = self.main("record", "parse-bind", "--capture", str(capture), "--label", "github ubuntu-latest")
        self.assertEqual(code, 0)
        reader.assert_called_once()
        path, run = self.written("parse-bind")
        self.assertEqual(set(run), RUN_KEYS)
        self.assertEqual(run["workload"], "parse-bind")
        self.assertEqual(run["pin"], perf.pin())
        self.assertEqual(run["revision"], REVISION)  # the capture's own revision
        self.assertEqual(run["host"], {"os": "macos", "arch": "aarch64", "cpus": 18, "label": "github ubuntu-latest"})
        self.assertRegex(run["recorded_at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual(path.name, run["recorded_at"].replace(":", "-") + "-01234567.json")
        self.assertIn(str(path.name), out)
        self.assertEqual(set(run["ratios"]), {
            "one_thread_wall_time", "eight_threads_wall_time", "peak_rss", "allocated_bytes",
            *(f"workers_{w}_{f}" for w in (1, 8) for f in ("wall_time_ns", "peak_rss_bytes", "allocated_bytes"))})
        self.assertEqual(run["ratios"]["one_thread_wall_time"], report["metrics"]["one_thread_wall_time_ratio"])
        self.assertEqual(run["ratios"]["one_thread_wall_time"], 73 / 103)
        self.assertEqual(run["ratios"]["eight_threads_wall_time"], 143 / 103)
        self.assertEqual(run["ratios"]["workers_8_wall_time_ns"], 143 / 103)
        self.assertEqual(run["ratios"]["peak_rss"], report["metrics"]["peak_rss_ratio"])
        # Samples: what each ratio is the median ratio of, in capture order.
        self.assertEqual(run["samples"]["rust"]["workers_1_wall_time_ns"], [70, 71, 72, 73, 74, 75, 76])
        self.assertEqual(run["samples"]["go"]["workers_8_wall_time_ns"], [100, 101, 102, 103, 104, 105, 106])
        self.assertEqual(run["samples"]["rust"]["workers_8_peak_rss_bytes"], [70000 + i for i in range(7)])
        # Allocated bytes come from the instrumented pass; the timing pass has none for Rust.
        self.assertEqual(run["samples"]["rust"]["workers_1_allocated_bytes"], [70 + i for i in range(7)])
        for runtime in ("rust", "go"):
            self.assertEqual(set(run["samples"][runtime]), {f"workers_{w}_{f}" for w in (1, 8)
                                                            for f in ("wall_time_ns", "peak_rss_bytes", "allocated_bytes")})
            self.assertTrue(all(len(values) == 7 for values in run["samples"][runtime].values()))

    def test_parse_bind_measurement_from_real_aggregation(self):
        report, rows = s07_capture(self.directory / "capture")
        measurement = perf.parse_bind_measurement(report, rows)
        for workers in ("1", "8"):
            for field in ("wall_time_ns", "peak_rss_bytes", "allocated_bytes"):
                self.assertEqual(measurement["ratios"][f"workers_{workers}_{field}"],
                                 report["summaries"][workers][field]["ratio"])

    def test_a_run_is_never_regenerated(self):
        capture = self.directory / "s07-benchmark"
        s07_capture(capture)
        with patch("s07_benchmark_report.read_capture", side_effect=fake_read_capture), \
                patch.object(perf, "timestamp", return_value="2026-10-03T00:00:00Z"):
            self.assertEqual(self.main("record", "parse-bind", "--capture", str(capture))[0], 0)
            code, _, err = self.main("record", "parse-bind", "--capture", str(capture))
        self.assertEqual(code, 1)
        self.assertIn("never regenerated", err)
        _, run = self.written("parse-bind")
        self.assertEqual(run["host"]["label"], "macos aarch64, 18 CPUs")

    def test_a_capture_the_harness_rejects_writes_nothing(self):
        with patch("s07_benchmark_report.read_capture", side_effect=ValueError("benchmark capture binaries changed")):
            code, _, err = self.main("record", "parse-bind", "--capture", str(self.directory))
        self.assertEqual(code, 1)
        self.assertIn("benchmark capture binaries changed", err)
        self.assertFalse((self.perf / "parse-bind").exists())

    def checker_capture(self):
        capture = self.directory / "checkerbench"
        capture.mkdir()
        (capture / "capture.json").write_text(json.dumps({"version": 2, "finished": 1789790586.25}))
        return capture

    def test_checker_capture_becomes_a_run_file(self):
        import s08_checkerbench
        capture = self.checker_capture()
        with patch.object(s08_checkerbench, "report", return_value=checker_report()) as report:
            code, _, _ = self.main("record", "checker", "--capture", str(capture), "--label", "owner quiet host")
        self.assertEqual(code, 0)
        report.assert_called_once_with(capture.resolve())
        path, run = self.written("checker")
        self.assertEqual(set(run), RUN_KEYS)
        self.assertEqual(run["recorded_at"], "2026-09-19T04:03:06Z")  # the capture's finish time
        self.assertEqual(path.name, "2026-09-19T04-03-06Z-" + perf.head()[:8] + ".json")
        self.assertEqual(run["ratios"], {"throughput": 0.5, "elapsed": 2.0, "allocated_bytes": 0.6,
                                         "retained_bytes": 1.4, "type_footprint": 0.8})
        self.assertEqual(run["samples"]["rust"], {"interval_ns": [9, 10, 11], "requested_bytes": [60, 60, 60],
                                                  "retained_bytes": [70, 70, 70], "type_storage_bytes": [240, 240, 240],
                                                  "types_reachable": [10, 10, 10]})
        self.assertEqual(run["samples"]["go"]["interval_ns"], [4, 5, 6])

    def test_checker_without_a_footprint_records_the_rest_and_says_why(self):
        import s08_checkerbench
        capture = self.checker_capture()
        with patch.object(s08_checkerbench, "report", return_value=checker_report(footprint=False)):
            code, _, err = self.main("record", "checker", "--capture", str(capture))
        self.assertEqual(code, 0)
        self.assertIn("type_footprint_ratio unavailable: a census failed", err)
        _, run = self.written("checker")
        self.assertNotIn("type_footprint", run["ratios"])

    def test_checker_smoke_capture_is_not_a_run(self):
        import s08_checkerbench
        capture = self.checker_capture()
        with patch.object(s08_checkerbench, "report", return_value=checker_report(smoke=3)):
            code, _, err = self.main("record", "checker", "--capture", str(capture))
        self.assertEqual(code, 1)
        self.assertIn("smoke", err)
        self.assertFalse((self.perf / "checker").exists())

    def test_ratios_must_be_finite_numbers(self):
        measurement = {"ratios": {"elapsed": float("nan")}, "samples": {}, "host": HOST}
        for ratios in ({}, {"elapsed": float("nan")}, {"elapsed": True}, {"elapsed": "2.0"}):
            with self.subTest(ratios=ratios), self.assertRaises(ValueError):
                perf.run_document("checker", {**measurement, "ratios": ratios}, revision=REVISION)

    def test_run_file_is_json_with_one_line_per_sample_list(self):
        document = perf.run_document("checker", perf.checker_measurement(checker_report()), "owner quiet host",
                                     revision=REVISION, recorded_at="2026-09-19T04:03:06Z")
        text = perf.dumps(document)
        self.assertEqual(json.loads(text), document)
        self.assertIn('"interval_ns": [9, 10, 11]', text)
        self.assertEqual(list(document["ratios"]), sorted(document["ratios"]))


class CheckTests(TemporaryPerf):
    def write(self, document, name=None):
        directory = self.perf / document["workload"]
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / (name or document["recorded_at"].replace(":", "-") + ".json")
        path.write_text(json.dumps(document))
        return path

    def thresholds(self, text):
        (self.perf / "thresholds.toml").write_text(text)

    def test_newest_run_passes(self):
        self.thresholds("[parse-bind]\none_thread_wall_time = 1.25\npeak_rss = 0.85\n")
        self.write(run_file(recorded_at="2026-09-01T00:00:00Z", one_thread_wall_time=1.4, peak_rss=0.7))
        self.write(run_file(recorded_at="2026-10-01T00:00:00Z", one_thread_wall_time=1.25, peak_rss=0.7,
                            workers_1_wall_time_ns=1.25), name="0-sorts-first-by-name.json")
        code, out, _ = self.main("check", "parse-bind")
        self.assertEqual(code, 0, out)
        self.assertIn("0-sorts-first-by-name.json", out)
        self.assertRegex(out, r"one_thread_wall_time\s+1\.2500\s+<= 1\.25\s+pass")
        self.assertRegex(out, r"workers_1_wall_time_ns\s+1\.2500\n")
        self.assertIn("0 of 2 thresholds missed", out)
        self.assertNotIn("informational", out)

    def test_miss_exits_1_and_writes_the_summary(self):
        self.thresholds("[parse-bind]\neight_threads_wall_time = 1.45\n")
        self.write(run_file(eight_threads_wall_time=1.5, peak_rss=0.7))
        summary = self.directory / "summary.md"
        with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}):
            code, out, _ = self.main("check", "parse-bind")
        self.assertEqual(code, 1)
        self.assertRegex(out, r"eight_threads_wall_time\s+1\.5000\s+<= 1\.45\s+miss")
        self.assertIn("1 of 1 thresholds missed", out)
        text = summary.read_text()
        self.assertTrue(text.startswith("### perf: parse-bind"))
        self.assertIn("| `eight_threads_wall_time` | 1.5000 | ≤ 1.45 | miss |", text)
        self.assertIn("| `peak_rss` | 0.7000 |  |  |", text)

    def test_unmeasured_threshold_is_a_miss(self):
        self.thresholds("[checker]\ntype_footprint = 0.85\n")
        self.write(run_file("checker", throughput=0.47))
        code, out, _ = self.main("check", "checker")
        self.assertEqual(code, 1)
        self.assertRegex(out, r"type_footprint\s+—\s+<= 0\.85\s+missing")

    def test_explicit_run_and_host_outside_the_threshold_class(self):
        self.thresholds("[parse-bind]\none_thread_wall_time = 1.25\n")
        linux = {"os": "linux", "arch": "x86_64", "cpus": 4, "label": "github ubuntu-latest"}
        path = self.write(run_file(host=linux, one_thread_wall_time=1.6))
        self.write(run_file(recorded_at="2026-12-01T00:00:00Z", one_thread_wall_time=1.0))
        code, out, _ = self.main("check", "parse-bind", "--run", str(path))
        self.assertEqual(code, 1)
        self.assertIn("ADR 0021", out)
        self.assertIn("informational", out)
        for host in ({"os": "macos", "arch": "aarch64", "cpus": 4}, {"os": "macos", "arch": "x86_64", "cpus": 18}):
            self.assertFalse(perf.in_threshold_class(host))
        self.assertTrue(perf.in_threshold_class({"os": "macos", "arch": "aarch64", "cpus": 8}))

    def test_run_of_another_workload_and_no_runs(self):
        path = self.write(run_file("checker", throughput=0.47))
        code, _, err = self.main("check", "parse-bind", "--run", str(path))
        self.assertEqual(code, 1)
        self.assertIn("not parse-bind", err)
        code, _, err = self.main("check", "parse-bind")
        self.assertEqual(code, 1)
        self.assertIn("no runs under", err)

    def test_invalid_thresholds_are_rejected(self):
        self.write(run_file(peak_rss=0.7))
        for text in ("[parse-bind]\npeak_rss = true\n", "[parse-bind]\npeak_rss = 0\n",
                     "[parse-bind]\npeak_rss = \"0.85\"\n", "parse-bind = 0.85\n", "[parse-bind]\npeak_rss = nan\n"):
            with self.subTest(text=text):
                self.thresholds(text)
                code, _, err = self.main("check", "parse-bind")
                self.assertEqual(code, 1)
                self.assertIn("thresholds.toml", err)


class LspTests(TemporaryPerf):
    def measurement(self, pairs=20, ratio=0.8):
        return dict(samples={runtime: {name: [100 * (ratio if runtime == 'rust' else 1)] * pairs
                                       for name in perf.LSP_SCENARIOS} for runtime in ('go', 'rust')},
                    smoke=False, correctness_matched=True, host=HOST, revision=REVISION,
                    recorded_at='2026-10-06T00:00:00Z', metadata={'fixture': 'pending-example'})

    def test_matched_pairs_recompute_and_record_uncertainty(self):
        measurement = perf.lsp_measurement(self.measurement())
        document = perf.run_document('lsp', measurement)
        self.assertEqual(set(document['ratios']), set(perf.LSP_SCENARIOS))
        self.assertTrue(all(ratio == 0.8 for ratio in document['ratios'].values()))
        self.assertEqual(document['metadata']['fixture'], 'pending-example')
        self.assertEqual(document['summaries']['hover']['bootstrap']['lower'], 0.8)
        self.assertFalse(document['summaries']['hover']['needs_more'])
        self.assertTrue(all(row[3] == 'pass' for row in perf.compare(document, dict.fromkeys(perf.LSP_SCENARIOS, 1.0))))

    def test_inconclusive_requires_extension_then_owner_review(self):
        for pairs, action in ((20, 'extend once to 40 pairs'), (40, 'requires owner review')):
            with self.subTest(pairs=pairs):
                document = perf.run_document('lsp', perf.lsp_measurement(self.measurement(pairs, ratio=1.0)))
                rows = perf.compare(document, dict.fromkeys(perf.LSP_SCENARIOS, 1.0))
                self.assertTrue(all(row[3] == 'inconclusive' for row in rows))
                lines = perf.report_lines('lsp', document, 'example.json', rows)
                self.assertIn(action, '\n'.join(lines))
                self.assertIn('95% interval', perf.summary_markdown(lines, rows, document))
                self.assertEqual(document['summaries']['hover']['needs_more'], pairs == 20)
        path = perf.write_run(document, self.perf)
        self.thresholds_text = '[lsp]\n' + ''.join(f'{name}=1.0\n' for name in perf.LSP_SCENARIOS)
        (self.perf / 'thresholds.toml').write_text(self.thresholds_text)
        code, output, _ = self.main('check', 'lsp', '--run', str(path))
        self.assertEqual(code, 1)
        self.assertIn('inconclusive', output)

    def test_smoke_mismatch_and_incomplete_pairs_are_not_runs(self):
        for field, value in (('smoke', True), ('correctness_matched', False)):
            measurement = self.measurement()
            measurement[field] = value
            with self.assertRaisesRegex(ValueError, 'correctness-matched'):
                perf.lsp_measurement(measurement)
        for pairs in (3, 19, 21, 39):
            with self.assertRaisesRegex(ValueError, '20 or 40'):
                perf.lsp_measurement(self.measurement(pairs))
        measurement = self.measurement()
        measurement['samples']['rust']['hover'].pop()
        with self.assertRaisesRegex(ValueError, 'complete matched pairs'):
            perf.lsp_measurement(measurement)

    def test_absent_interval_or_unconfigured_threshold_cannot_pass(self):
        document = perf.run_document('lsp', perf.lsp_measurement(self.measurement()))
        del document['summaries']['hover']
        rows = perf.compare(document, dict.fromkeys(perf.LSP_SCENARIOS, 1.0))
        self.assertIn(('hover', 0.8, 1.0, 'missing'), rows)
        path = perf.write_run(document, self.perf)
        code, _, error = self.main('check', 'lsp', '--run', str(path))
        self.assertEqual(code, 1)
        self.assertIn('explicitly name all five scenarios', error)

    def test_sample_count_override_preserves_original_defaults(self):
        from s07_benchmark_stats import ratio_summary
        with self.assertRaises(ValueError):
            ratio_summary([100] * 20, [80] * 20)
        self.assertEqual(ratio_summary([100] * 20, [80] * 20, accepted_counts=(20, 40))['ratio'], 0.8)
        self.assertEqual(ratio_summary([100] * 7, [80] * 7)['ratio'], 0.8)


class CommittedFiles(unittest.TestCase):
    def test_thresholds_file(self):
        tables = perf.read_thresholds()
        self.assertEqual(tables, {
            "parse-bind": {"one_thread_wall_time": 1.25, "eight_threads_wall_time": 1.45,
                           "peak_rss": 0.85, "allocated_bytes": 0.85},
            "checker": {"type_footprint": 0.85},
            "lsp": dict.fromkeys(perf.LSP_SCENARIOS, 1.0),
        })
        self.assertEqual(tomllib.loads(perf.THRESHOLDS.read_text()), tables)
        for workload in tables:
            self.assertIn(workload, perf.WORKLOADS)
        from s07_benchmark_measure import e6_thresholds
        self.assertEqual(e6_thresholds(), {"1": 1.25, "8": 1.45})

    def test_committed_runs_have_the_rendered_format_and_pass(self):
        found = 0
        thresholds = perf.read_thresholds()
        for workload in perf.WORKLOADS:
            for run, path in perf.runs(workload):
                found += 1
                with self.subTest(path=path.name):
                    self.assertEqual(set(run), RUN_KEYS | ({"summaries", "metadata"} & set(run) if workload == "lsp" else set()))
                    self.assertEqual(run["workload"], workload)
                    self.assertEqual(perf.run_path(perf.PERF, run), path)
                    self.assertRegex(run["revision"], r"^[0-9a-f]{40}$")
                    self.assertRegex(run["pin"], r"^[0-9a-f]{40}$")
                    self.assertRegex(run["recorded_at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
                    self.assertEqual(set(run["host"]), {"os", "arch", "cpus", "label"})
                    self.assertTrue(run["ratios"] and all(perf.finite(v) for v in run["ratios"].values()))
                    self.assertEqual(path.read_text(), perf.dumps(run) + "\n")
                    for runtime, lists in run["samples"].items():
                        self.assertIn(runtime, ("rust", "go"))
                        self.assertEqual(set(lists), set(run["samples"]["rust"]))
                        self.assertTrue(all(len(values) in ((20, 40) if workload == "lsp" else (7,)) for values in lists.values()))
                    rows = perf.compare(run, thresholds.get(workload, {}))
                    self.assertFalse([row for row in rows if row[3] in ("miss", "missing")])
        self.assertGreaterEqual(found, 2)

    def test_carried_over_parse_bind_samples_reproduce_the_ratios(self):
        from statistics import median
        for run, _ in perf.runs("parse-bind"):
            samples = run["samples"]
            for name in samples["rust"]:
                with self.subTest(name=name):
                    self.assertAlmostEqual(median(samples["rust"][name]) / median(samples["go"][name]),
                                           run["ratios"][name], places=12)


if __name__ == "__main__":
    unittest.main()
