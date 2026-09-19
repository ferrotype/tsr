"""Counterexamples for assignment, timing and frozen-patch integrity."""
import copy
import hashlib
import tempfile
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

import patches
import probe


class DiagnosticContracts(unittest.TestCase):
    def fixture(self):
        expected = {"files": 8, "loaded_bytes": 16, "nodes": 30, "symbols": 9,
                    "parse_diagnostics": 0, "bind_diagnostics": 0, "loaded_input_sha256": "d" * 64}
        assignments = [{"worker": worker, "files": 1, "loaded_bytes": 2,
                        "assignment_sha256": hashlib.sha256(b"S07-worker-assignment-v1\0" + worker.to_bytes(8, "big")).hexdigest()}
                       for worker in range(8)]
        rows = [{**assignment, "work_ns": 70, "receive_wait_ns": 20, "start_offset_ns": 0,
                 "completion_offset_ns": 95, "tail_ns": 5} for assignment in assignments]
        timing = {**probe.DOMAINS, "runtime": "rust", "sender_send_ns": 70,
                  "sender_completion_offset_ns": 85, "rows": rows}
        report = {**expected, "version": 1, "workers": 8, "wall_time_ns": 100, "allocated_bytes": None,
                  "startup_ns": 1, "preload_ns": 1, "worker_setup_ns": 1, "cpu_capacity": 18,
                  "goroutines_ready": None, "worker_timing": timing}
        envelope = {"report": report, "peak_rss_bytes": 1000, "process_time_ns": 110,
                    "user_time_ns": 400, "system_time_ns": 10, "stderr": ""}
        return envelope, expected, assignments

    def test_valid_worker_observation(self):
        value, expected, assignments = self.fixture()
        probe.validate_observation(value, expected, assignments, "rust")

    def test_duplicated_or_reordered_worker_is_rejected(self):
        for order in ([0, 0, 2, 3, 4, 5, 6, 7], [1, 0, 2, 3, 4, 5, 6, 7]):
            value, expected, assignments = self.fixture()
            original = value["report"]["worker_timing"]["rows"]
            value["report"]["worker_timing"]["rows"] = [original[i] for i in order]
            with self.assertRaises(ValueError):
                probe.validate_observation(value, expected, assignments, "rust")

    def test_same_counts_wrong_assignment_is_rejected(self):
        value, expected, assignments = self.fixture()
        value["report"]["worker_timing"]["rows"][0]["assignment_sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            probe.validate_observation(value, expected, assignments, "rust")

    def test_same_counts_wrong_loaded_digest_is_rejected(self):
        value, expected, assignments = self.fixture()
        value["report"]["loaded_input_sha256"] = "e" * 64
        with self.assertRaises(ValueError):
            probe.validate_observation(value, expected, assignments, "rust")

    def test_missing_worker_is_rejected(self):
        value, expected, assignments = self.fixture()
        value["report"]["worker_timing"]["rows"].pop()
        with self.assertRaises(ValueError):
            probe.validate_observation(value, expected, assignments, "rust")

    def test_timers_cannot_overlap_or_escape_endpoint(self):
        for changes in ({"work_ns": 76}, {"tail_ns": 4}, {"start_offset_ns": 99}, {"receive_wait_ns": -1}):
            value, expected, assignments = self.fixture()
            value["report"]["worker_timing"]["rows"][0].update(changes)
            with self.assertRaises(ValueError):
                probe.validate_observation(value, expected, assignments, "rust")

    def test_instrumented_allocator_or_changed_policy_is_rejected(self):
        for key, field in (("allocated_bytes", 10), ("workers", 1)):
            value, expected, assignments = self.fixture()
            value["report"][key] = field
            with self.assertRaises(ValueError):
                probe.validate_observation(value, expected, assignments, "rust")
        value, expected, assignments = self.fixture()
        value["report"]["worker_timing"]["queue_capacity"] = 16
        with self.assertRaises(ValueError):
            probe.validate_observation(value, expected, assignments, "rust")

    def test_domain_type_and_sender_conservation(self):
        for key, changed in (("version", True), ("sender_send_ns", 86)):
            value, expected, assignments = self.fixture()
            value["report"]["worker_timing"][key] = changed
            with self.assertRaises(ValueError):
                probe.validate_observation(value, expected, assignments, "rust")

    def test_assignment_oracle_preserves_original_order(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            requests = []
            for index in range(17):
                path = root / str(index)
                path.write_bytes(bytes(index))
                requests.append({"local": str(path)})
            probe.runner.write_json(root / "inputs.json", requests)
            rows = probe.assignment_rows(root / "inputs.json")
            self.assertEqual(rows[0]["files"], 3)
            self.assertEqual(rows[0]["loaded_bytes"], 24)
            self.assertEqual(rows[1]["loaded_bytes"], 10)
            ordered = b"S07-worker-assignment-v1\0" + b"".join(index.to_bytes(8, "big") for index in (0, 8, 16))
            self.assertEqual(rows[0]["assignment_sha256"], hashlib.sha256(ordered).hexdigest())

    def test_timeout_preserves_intent_and_partial_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            error = subprocess.TimeoutExpired(["fake"], 1, output=b"partial stdout", stderr=b"partial stderr")
            with patch.object(probe.subprocess, "run", side_effect=error):
                with self.assertRaises(subprocess.TimeoutExpired):
                    probe.run(["fake"], root, {}, root, "attempt")
            receipt = probe.strict_json_loads((root / "attempt.command.json").read_bytes())
            self.assertEqual(receipt["argv"], ["fake"])
            self.assertEqual(receipt["status"], "timeout")
            self.assertEqual((root / "attempt.stdout").read_bytes(), b"partial stdout")
            self.assertEqual((root / "attempt.stderr").read_bytes(), b"partial stderr")

    def test_launch_failure_preserves_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(probe.subprocess, "run", side_effect=OSError("no executable")):
                with self.assertRaises(OSError):
                    probe.run(["absent"], root, {}, root, "attempt")
            receipt = probe.strict_json_loads((root / "attempt.command.json").read_bytes())
            self.assertEqual(receipt["status"], "launch_failed")
            self.assertEqual((root / "attempt.stdout").read_bytes(), b"")

    def test_patch_rejects_missing_or_duplicate_anchor(self):
        for text in ("none", "anchor anchor"):
            with self.assertRaises(ValueError):
                patches.replace_once(text, "anchor", "addition")

    def test_real_frozen_adapters_keep_core_work_and_dispatch(self):
        # Source-level guard complements executed whole-work/assignment checks.
        frozen = probe.ROOT / "target/s07-bis/current-candidate-acceptance-2026-09-10/frozen/source"
        if not frozen.is_dir():
            self.skipTest("local frozen source is unavailable")
        rust = patches.rust((frozen / "crates/tsr_bench/src/main.rs").read_text())
        go = patches.go((frozen / "tools/s07/benchmark/main.go").read_text())
        for marker in ("mpsc::sync_channel::<usize>(workers)", "senders[index % workers].send(index)?;", "tsr_binder::bind_parsed_file(parsed)", "release.wait();"):
            self.assertIn(marker, rust)
        for marker in ("make(chan int, workers)", "channels[index%workers] <- index", "binder.BindSourceFile(file)", "runtime.KeepAlive(results)"):
            self.assertIn(marker, go)
        with self.assertRaises(ValueError):
            patches.rust(rust)
        with self.assertRaises(ValueError):
            patches.go(go)


if __name__ == "__main__":
    unittest.main()
