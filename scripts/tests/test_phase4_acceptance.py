"""Independent witness joins, host attribution and refusal to guess missing facts."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_acceptance as acceptance  # noqa: E402
import phase4_producers as producers  # noqa: E402


def result(**metrics):
    return {"metrics": metrics, "identities": dict.fromkeys(metrics, "a" * 64)}


class WitnessJoin(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.index = self.root / "acceptance.json"
        self.document = {"version": 1,
                         "native": [{"host": "macos", "build": "build", "capture": "native"},
                                    {"host": "linux", "build": "linux-build", "capture": "linux-native"}],
                         "live": [{"host": "macos", "build": "build", "capture": "live"}],
                         "determinism": [{"capture": "five-runs"}],
                         "thread_sanitizer": [{"host": "macos", "capture": "tsan"}]}
        self.replay = {"native": mock.Mock(return_value=result(smoke=True, buildinfo_interop=True)),
                       "live": mock.Mock(return_value=result(live_watch_parity=True)),
                       "determinism": mock.Mock(return_value=result(determinism=True)),
                       "thread_sanitizer": mock.Mock(return_value=result(thread_sanitizer=True))}

    def collect(self, **kwargs):
        self.index.write_text(json.dumps(self.document))
        return acceptance.collect(self.index, host="macos", replay=self.replay, **kwargs)

    def test_host_facts_and_both_host_coverage_are_distinct(self):
        report = self.collect()
        metrics = report["metrics"]
        for name in ("smoke", "smoke_macos", "smoke_linux", "smoke_all_hosts",
                     "buildinfo_interop", "live_watch_parity", "determinism", "thread_sanitizer"):
            self.assertIs(metrics[name], True)
        self.assertNotIn("live_watch_parity_all_hosts", metrics)
        self.assertEqual(report["identities"]["smoke"], dict.fromkeys(acceptance.HOSTS, "a" * 64))
        self.assertEqual(len(report["index_sha256"]), 64)
        self.replay["native"].assert_any_call(self.root / "build", self.root / "native", expected_host="macos")

    def test_a_foreign_host_cannot_stand_in_for_this_host(self):
        self.document["native"] = self.document["native"][1:]
        report = self.collect()
        self.assertNotIn("smoke", report["metrics"])
        self.assertIs(report["metrics"]["smoke_linux"], True)
        self.assertNotIn("smoke_all_hosts", report["metrics"])
        self.assertIn("smoke", report["unavailable"])

    def test_complete_failure_is_false_but_absent_or_partial_is_withheld(self):
        self.replay["native"].side_effect = [result(smoke=False), result(smoke=True, buildinfo_interop=True)]
        self.replay["determinism"].return_value = result()
        report = self.collect()
        self.assertIs(report["metrics"]["smoke"], False)
        self.assertIs(report["metrics"]["smoke_all_hosts"], False)
        self.assertNotIn("buildinfo_interop", report["metrics"])
        self.assertNotIn("determinism", report["metrics"])
        self.assertIn("native/macos/buildinfo_interop", report["unavailable"])

    def test_stale_captures_and_bad_groups_do_not_discard_other_groups(self):
        self.document["live"] *= 2
        self.replay["native"].side_effect = ValueError("source closure changed")
        report = self.collect()
        self.assertNotIn("smoke", report["metrics"])
        self.assertNotIn("live_watch_parity", report["metrics"])
        self.assertIs(report["metrics"]["thread_sanitizer"], True)
        self.assertIs(report["metrics"]["determinism"], True)
        self.assertIn("live", report["unavailable"])
        self.assertIn("native/macos", report["unavailable"])

    def test_a_boolean_without_a_capture_identity_is_not_evidence(self):
        self.replay["live"].return_value = {"metrics": {"live_watch_parity": True}, "identities": {}}
        self.assertNotIn("live_watch_parity", self.collect()["metrics"])

    def test_missing_and_duplicate_key_indexes_do_not_run_verifiers(self):
        self.assertIn("index", acceptance.collect(self.index, replay=self.replay)["unavailable"])
        self.index.write_text('{"version":1,"native":[],"native":[]}')
        self.assertIn("duplicate", acceptance.collect(self.index, replay=self.replay)["unavailable"]["index"])
        for verifier in self.replay.values():
            verifier.assert_not_called()

    def test_unrequested_or_invalid_metric_is_rejected(self):
        for response in (result(smoke=True), result(live_watch_parity=1),
                         {"metrics": {"live_watch_parity": True}, "identities": {"live_watch_parity": "bad"}}):
            self.replay["live"].return_value = response
            self.assertNotIn("live_watch_parity", self.collect()["metrics"])

    def test_producer_keeps_witnesses_when_scenario_inventory_is_unavailable(self):
        stderr, stdout = io.StringIO(), io.StringIO()
        expected = self.collect()
        with (mock.patch.object(acceptance, "collect", return_value=expected),
              mock.patch.object(producers.phase4_audit, "check", return_value=[]),
              mock.patch.object(producers.phase4_unit_tests, "check", return_value=[]),
              mock.patch.object(producers.phase4_corpus, "read_document", side_effect=ValueError("missing inventory")),
              contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(stdout)):
            metrics = producers.tsc(witnesses=self.index)["metrics"]
        self.assertIs(metrics["smoke"], True)
        self.assertFalse(metrics["harness_valid"])
        self.assertNotIn("baseline_parity", metrics)
        self.assertIn('"smoke":{"linux":"' + "a" * 64, stderr.getvalue())
        self.assertIn(expected["index_sha256"], stderr.getvalue())
        self.assertEqual(stdout.getvalue(), "")

    def test_runner_and_proof_helpers_are_in_the_ledger_source_closure(self):
        run = tomllib.loads((ROOT / "status/runs.toml").read_text())["tsc"]
        declared = {path.relative_to(ROOT).as_posix()
                    for pattern in run["sources"] for path in ROOT.glob(pattern) if path.is_file()}
        required = {"scripts/phase4_acceptance.py", "scripts/phase4_native.py",
                    "scripts/phase4_live.py", "scripts/phase4_determinism.py",
                    "scripts/phase4_sanitizer.py", "scripts/phase4_producers.py",
                    "scripts/phase4_corpus.py", "scripts/phase4_scenarios.py",
                    "scripts/s04_runtime.py", "scripts/s04_common.py", "scripts/s04.py",
                    "scripts/s08_oracle.py", "scripts/tracking-bootstrap.py"}
        self.assertFalse(required - declared, required - declared)


if __name__ == "__main__":
    unittest.main()
