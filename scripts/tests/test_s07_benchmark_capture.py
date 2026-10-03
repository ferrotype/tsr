"""Exact graph artifacts are reused; stale prerequisites fail before sampling.

All builders, runtime processes and measurements are mocked. The real graph
prerequisite validator and artifact hashing run against temporary receipts.
"""
import copy
from contextlib import ExitStack
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s07_benchmark_graph as graph
import s07_benchmark_graph_tests as fixtures
import s07_benchmark_measure as measure
import s07_benchmark_report as consumer


class CaptureArtifacts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixtures.PrerequisiteTests.setUpClass()

    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.directory = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        cache = self.directory / "s07-benchmark"
        cache.mkdir()
        self.go = cache / "go-benchmark"
        self.rust = cache / "rust-benchmark"
        self.allocation = cache / "rust-benchmark-allocation"
        for path in (self.go, self.rust, self.allocation):
            path.write_bytes(path.name.encode())
        self.inputs = cache / "inputs.json"
        self.inputs.write_text("[]\n")
        self.source = copy.deepcopy(fixtures.PrerequisiteTests.source)
        self.configuration = copy.deepcopy(fixtures.PrerequisiteTests.configuration)
        self.report = copy.deepcopy(fixtures.PrerequisiteTests.report)
        identities = {"oracle": measure.sha(self.go.read_bytes()), "rust": measure.sha(self.rust.read_bytes())}
        self.report.update(binary_sha256=identities, binary_sha256_after=identities)
        self.graph_path = self.directory / "target/s07-bindworkload/report.json"
        self.graph_path.parent.mkdir(parents=True)
        self.destination = self.directory / "capture"
        self.write_graph()
        self.stack.enter_context(patch.object(measure, "CACHE", self.directory))
        self.stack.enter_context(patch.object(measure, "source_fingerprint", side_effect=lambda: copy.deepcopy(self.source)))
        self.stack.enter_context(patch.object(measure, "cargo_configuration", side_effect=lambda: copy.deepcopy(self.configuration)))
        self.preflight = self.stack.enter_context(patch.object(measure, "allocation_preflight", return_value={"calibrated": True}))
        self.builder = self.stack.enter_context(patch.object(measure, "build_rust", return_value=(self.allocation, {})))
        # Any accidental Go rebuild must fail instead of running a compiler.
        self.go_builder = self.stack.enter_context(patch("s07_benchmark.build_go", side_effect=AssertionError("unexpected Go rebuild")))
        self.stack.enter_context(patch.object(measure, "go_native_environment", return_value={}))
        self.stack.enter_context(patch.object(measure, "native_environment", return_value={}))
        self.stack.enter_context(patch.object(measure, "provision_inputs", return_value=(self.inputs, {})))
        self.stack.enter_context(patch.object(graph, "requests_from_frozen", return_value=([], fixtures.PrerequisiteTests.frozen["requests"])))
        self.stack.enter_context(patch.object(measure, "host_info", return_value={"os": "darwin", "architecture": "arm64"}))
        self.stack.enter_context(patch.object(measure, "reject_concurrent_builds"))
        self.stack.enter_context(patch.object(measure, "rust_native_toolchain", return_value=("fixture", "fixture")))
        self.stack.enter_context(patch.object(measure, "command", return_value=b"fixture\n"))
        self.sampler = self.stack.enter_context(patch.object(measure, "sample", return_value={"report": {"wall_time_ns": 100}}))
        self.stack.enter_context(patch.object(measure, "ratio_summary", return_value={"needs_more": False}))
        self.stack.enter_context(patch.object(measure, "aggregate", return_value={}))
        self.stack.enter_context(patch.object(measure, "metrics_from_summaries", return_value={}))
        self.stack.enter_context(patch.object(graph, "CACHE", self.directory))
        self.stack.enter_context(patch.object(graph, "source_fingerprint", side_effect=lambda: copy.deepcopy(self.source)))
        self.stack.enter_context(patch.object(graph, "cargo_configuration", side_effect=lambda: copy.deepcopy(self.configuration)))

    def write_graph(self):
        self.graph_path.write_text(json.dumps(self.report))

    def capture(self):
        return measure.capture(self.graph_path, self.destination)

    def test_reuses_graph_go_and_normal_builds_only_allocation(self):
        report = self.capture()
        self.preflight.assert_called_once_with()
        self.builder.assert_called_once_with(True)
        self.go_builder.assert_not_called()
        self.assertEqual(report["samples"], 56)
        self.assertEqual(self.sampler.call_count, 64)  # 56 samples + 8 warmups.
        self.assertEqual({call.args[0] for call in self.sampler.call_args_list}, {self.go, self.rust, self.allocation})
        self.assertEqual(report["binaries"]["go"], self.report["binary_sha256"]["oracle"])
        self.assertEqual(report["binaries"]["rust"], self.report["binary_sha256"]["rust"])
        self.assertEqual(report["graph_report_sha256"], measure.sha(self.graph_path.read_bytes()))
        self.assertEqual(report["allocation_preflight"], {"calibrated": True})

    def test_hosts_outside_the_threshold_class_are_measured(self):
        # ADR 0021's host class is reported by `perf.py check`, not enforced here.
        linux = {"os": "linux", "architecture": "x86_64"}
        with patch.object(measure, "host_info", return_value=linux):
            report = self.capture()
        self.assertEqual(report["host"], linux)
        self.assertEqual(report["samples"], 56)

    def test_graph_reuse_rejects_source_changes_during_validation(self):
        with patch.object(graph, 'source_fingerprint', side_effect=[self.source, {}]):
            with self.assertRaisesRegex(ValueError, 'changed during reuse validation'):
                graph.current_capture(self.graph_path)

    def test_stale_source_configuration_or_artifact_rejects_before_builds(self):
        for name in ("source", "configuration", "missing_configuration", "normal", "go", "missing_normal"):
            with self.subTest(name=name):
                old_source = copy.deepcopy(self.source)
                old_configuration = copy.deepcopy(self.configuration)
                old_report = copy.deepcopy(self.report)
                old_rust, old_go = self.rust.read_bytes(), self.go.read_bytes()
                if name == "source": self.source["sha256"] = "0" * 64
                elif name == "configuration": self.configuration["/external/.cargo/config.toml"] = None
                elif name == "missing_configuration":
                    self.report.pop("cargo_configuration")
                    self.write_graph()
                elif name == "normal": self.rust.write_bytes(b"different normal build")
                elif name == "go": self.go.write_bytes(b"different Go build")
                else: self.rust.unlink()
                with self.assertRaises((ValueError, FileNotFoundError)):
                    self.capture()
                self.preflight.assert_not_called()
                self.builder.assert_not_called()
                self.sampler.assert_not_called()
                self.assertEqual(json.loads((self.destination / "report.json").read_text())["status"], "capture_in_progress")
                self.source, self.configuration, self.report = old_source, old_configuration, old_report
                self.rust.write_bytes(old_rust); self.go.write_bytes(old_go); self.write_graph()

    def test_preparation_changes_reject_before_first_sample(self):
        for name in ("source", "configuration", "graph", "normal", "go"):
            with self.subTest(name=name):
                old_source, old_configuration = copy.deepcopy(self.source), copy.deepcopy(self.configuration)
                old_rust, old_go = self.rust.read_bytes(), self.go.read_bytes()
                def build(_):
                    if name == "source": self.source["sha256"] = "0" * 64
                    elif name == "configuration": self.configuration.clear()
                    elif name == "graph": self.graph_path.write_bytes(self.graph_path.read_bytes() + b"\n")
                    elif name == "normal": self.rust.write_bytes(b"overwritten normal")
                    else: self.go.write_bytes(b"overwritten Go")
                    return self.allocation, {}
                self.builder.side_effect = build
                with self.assertRaises(ValueError): self.capture()
                self.sampler.assert_not_called()
                self.source, self.configuration = old_source, old_configuration
                self.rust.write_bytes(old_rust); self.go.write_bytes(old_go); self.write_graph()

    def test_changes_during_samples_leave_only_failed_receipt(self):
        def sample(*args):
            self.rust.write_bytes(b"changed during measurement")
            return {"report": {"wall_time_ns": 100}}
        self.sampler.side_effect = sample
        with self.assertRaisesRegex(ValueError, "changed during measurement"):
            self.capture()
        self.assertEqual(json.loads((self.destination / "report.json").read_text())["status"], "capture_in_progress")
        self.assertTrue((self.destination / "samples.ndjson").is_file())

    def test_native_consumer_requires_graph_configuration_binding(self):
        capture = self.capture()
        with patch.object(consumer, "CACHE", self.directory), \
                patch.object(consumer, "source_fingerprint", return_value=self.source), \
                patch.object(consumer, "cargo_configuration", return_value=self.configuration), \
                patch.object(consumer, "validate_allocation_preflight"), \
                patch.object(consumer, "validate_metadata"), \
                patch.object(consumer, "validate_rows", return_value={}), \
                patch.object(consumer, "metrics_from_summaries", return_value={}):
            consumer.read_capture(self.destination, self.graph_path)
            self.report.pop("cargo_configuration")
            self.write_graph()
            # Updating the outer hash cannot make a graph with no build
            # configuration receipt qualify as current native evidence.
            capture["graph_report_sha256"] = measure.sha(self.graph_path.read_bytes())
            (self.destination / "report.json").write_text(json.dumps(capture))
            with self.assertRaisesRegex(ValueError, "Cargo configuration"):
                consumer.read_capture(self.destination, self.graph_path)


class HostCapacity(unittest.TestCase):
    """parse-bind's eight-worker mode needs eight CPUs; checkerbench runs one process at a time."""

    def cpus(self, count):
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(patch.object(measure.os, "sched_getaffinity", return_value=set(range(count)), create=True))
        stack.enter_context(patch.object(measure.os, "cpu_count", return_value=count))

    def test_parse_bind_needs_eight_available_cpus(self):
        self.cpus(4)
        with self.assertRaisesRegex(ValueError, "has 4 available CPUs; this workload needs at least 8"):
            measure.host_info()
        self.cpus(8)
        self.assertEqual(measure.host_info()["cpu_capacity"], 8)

    def test_checkerbench_measures_on_fewer_cpus(self):
        import s08_checkerbench as checker
        self.cpus(4)
        self.assertEqual(checker.measurement_host()["cpu_capacity"], 4)


if __name__ == "__main__":
    unittest.main()
