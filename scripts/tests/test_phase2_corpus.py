"""C0 replay and schema regressions using committed, small real observations."""
import copy
import fnmatch
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare as compare  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import s08_p4 as p4  # noqa: E402
from s08_oracle import digest  # noqa: E402

from phase2_fixtures import build_capture, load, CONTROL, PANIC, MAPPER, S08_CONTROLS  # noqa: E402


class HarnessValidation(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase2-corpus-"))
        self.addCleanup(shutil.rmtree, self.directory)
        self.build([CONTROL, PANIC, MAPPER])

    def build(self, ids):
        if (self.directory / "cases").exists():
            shutil.rmtree(self.directory / "cases")
        # Duplicate identities are deliberately constructed by one rejection test.
        self.metadata = build_capture(self.directory, ids, selection=corpus.selection())

    def envelope(self, position):
        return p4.read(self.directory / "cases" / f"{position:05d}" / "result.json")

    def write_envelope(self, position, envelope):
        (self.directory / "cases" / f"{position:05d}" / "result.json").write_bytes(p4.canonical(envelope) + b"\n")

    def test_bound_capture_replays_with_production_failures_attributed(self):
        result = corpus.replay(self.directory)
        self.assertEqual(result["summary"]["observed"], 3)
        self.assertEqual(result["harness_errors"], [])
        self.assertEqual([entry["id"] for entry in result["production_failures"]], [PANIC])
        self.assertTrue(result["production_failures"][0]["attribution"].startswith("panic at crates/"))

    def test_missing_row_is_rejected(self):
        (self.directory / "cases/00001/result.json").unlink()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            corpus.replay(self.directory)

    def test_extra_row_is_rejected(self):
        shutil.copytree(self.directory / "cases/00000", self.directory / "cases/00003")
        with self.assertRaisesRegex(ValueError, "outside the request inventory"):
            corpus.replay(self.directory)

    def test_reordered_rows_are_rejected(self):
        cases = self.directory / "cases"
        (cases / "00000").rename(cases / "swap")
        (cases / "00001").rename(cases / "00000")
        (cases / "swap").rename(cases / "00001")
        with self.assertRaisesRegex(ValueError, "different request or capture"):
            corpus.replay(self.directory)

    def test_duplicate_request_identity_is_rejected(self):
        self.build([CONTROL, CONTROL])
        with self.assertRaisesRegex(ValueError, "duplicate request identity"):
            corpus.replay(self.directory)

    def test_forged_request_digest_is_rejected(self):
        envelope = self.envelope(0)
        envelope["request_sha256"] = "0" * 64
        self.write_envelope(0, envelope)
        with self.assertRaisesRegex(ValueError, "different request or capture"):
            corpus.replay(self.directory)

    def test_stale_capture_metadata_is_rejected(self):
        metadata = dict(self.metadata, timeout_seconds=self.metadata["timeout_seconds"] + 1)
        (self.directory / "capture.json").write_bytes(p4.canonical(metadata) + b"\n")
        with self.assertRaisesRegex(ValueError, "different request or capture"):
            corpus.replay(self.directory)

    def test_changed_raw_output_is_rejected(self):
        with (self.directory / "cases/00000/stderr").open("ab") as stream:
            stream.write(b"tampered")
        with self.assertRaisesRegex(ValueError, "raw case artifact changed"):
            corpus.replay(self.directory)

    def test_unknown_status_is_rejected(self):
        envelope = self.envelope(0)
        envelope["row"]["phases"]["config"]["state"] = "mostly"
        raw = p4.canonical(envelope["row"])
        (self.directory / "cases/00000/observation.json").write_bytes(raw)
        envelope["artifacts"]["observation.json"] = digest(raw)
        self.write_envelope(0, envelope)
        with self.assertRaisesRegex(ValueError, "unclassified operation result"):
            corpus.replay(self.directory)

    def test_injected_adapter_panic_is_a_harness_error_not_a_gap(self):
        envelope = self.envelope(1)
        envelope["row"]["panic_location"] = "tools/phase2/subtests.rs:120"
        self.write_envelope(1, envelope)
        # The raw observation of a panic row is the example's own record; keep it consistent.
        raw = p4.canonical(envelope["row"])
        (self.directory / "cases/00001/observation.json").write_bytes(raw)
        envelope["artifacts"]["observation.json"] = digest(raw)
        self.write_envelope(1, envelope)
        result = corpus.replay(self.directory)
        self.assertEqual([entry["id"] for entry in result["harness_errors"]], [PANIC])
        self.assertEqual(result["production_failures"], [])
        self.assertIsNone(compare.compare_row({"state": "executed"}, envelope["row"], "adapter panic"))

    def test_adapter_literal_walker_error_is_a_harness_error(self):
        row = copy.deepcopy(self.envelope(0)["row"])
        row["type_symbol_baselines"] = {"state": "failed", "class": "walker_error",
                                        "reason": "baseline source absent from program"}
        self.assertEqual(corpus.completed_problems(row), ["adapter walker error: baseline source absent from program"])
        row["type_symbol_baselines"]["reason"] = "unsupported checker operation: someOperation"
        self.assertEqual(corpus.completed_problems(row), [])
        self.assertEqual(compare.failure_outcome(row["type_symbol_baselines"]),
                         {"category": "unsupported", "operation": "someOperation"})

    def test_production_panic_remains_a_measured_gap(self):
        row = self.envelope(1)["row"]
        native = {"state": "executed", "types": {"state": "content"}, "trace": {"state": "disabled"}}
        result = compare.compare_row(native, row, None, "panic at " + row["panic_location"])
        self.assertEqual(result["errors"]["category"], "failed")
        self.assertEqual(result["trace"]["category"], "disabled")

    def test_resume_requires_identical_inputs(self):
        requests = p4.read(self.directory / "requests.json")
        with patch.object(corpus, "requests", return_value=({"observation_sha256": "fixture"}, requests, [])):
            (self.directory / "report.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "identical native capture"):
                corpus.run(self.directory, self.directory, 1, self.metadata["timeout_seconds"] + 5, resume=True)

    def test_panic_completion_must_equal_raw_observation(self):
        envelope = self.envelope(1)
        envelope["row"]["fatal"]["reason"] = "forged reason"
        self.write_envelope(1, envelope)
        with self.assertRaisesRegex(ValueError, "differs from its raw observation"):
            corpus.replay(self.directory)

    def test_raw_artifact_obligations_cannot_be_removed(self):
        for position in (0, 1, 2):
            original = self.envelope(position)
            for missing in ("stdout", "stderr", "observation.json", "all"):
                with self.subTest(position=position, missing=missing):
                    envelope = copy.deepcopy(original)
                    if missing == "all":
                        envelope["artifacts"] = {}
                    else:
                        del envelope["artifacts"][missing]
                    self.write_envelope(position, envelope)
                    with self.assertRaisesRegex(ValueError, "missing or extra raw case artifact"):
                        corpus.replay(self.directory)
                    self.write_envelope(position, original)

    def test_child_completion_requires_an_observation_even_if_file_and_hash_are_deleted(self):
        envelope = self.envelope(1)
        del envelope["artifacts"]["observation.json"]
        (self.directory / "cases/00001/observation.json").unlink()
        self.write_envelope(1, envelope)
        with self.assertRaisesRegex(ValueError, "missing or extra raw case artifact"):
            corpus.replay(self.directory)

    def test_runner_timeout_can_retain_an_incomplete_child_observation(self):
        envelope = self.envelope(1)
        envelope["row"] = p4.fatal(p4.read(self.directory / "requests.json")[1], "timeout", "variant exceeded 60 seconds")
        self.write_envelope(1, envelope)
        result = corpus.replay(self.directory)
        self.assertEqual(result["production_failures"][0]["attribution"], "deadline")


class SubtestSchema(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.record = load()["records"][CONTROL]

    def validate(self, name, value, *, trace=False):
        request = copy.deepcopy(self.record["request"])
        if trace:
            request["loading"]["options"]["traceResolution"] = True
        row = copy.deepcopy(self.record["envelope"]["row"])
        row["phase2"][name] = value
        return corpus.validate_row(request, row)

    def test_bad_counts_cannot_be_classified_as_matches(self):
        for name, fields in (("union_ordering", ("checkers", "unions", "inconsistent")),
                             ("parent_pointers", ("files", "nodes"))):
            original = self.record["envelope"]["row"]["phase2"][name]
            for field in fields:
                for invalid in (-1, False, 0.5, "1", None):
                    with self.subTest(name=name, field=field, invalid=invalid):
                        value = dict(original, **{field: invalid})
                        with self.assertRaisesRegex(ValueError, "nonnegative integers"):
                            self.validate(name, value)
            with self.assertRaises(ValueError):
                self.validate(name, dict(original, extra=0))
            for field in fields:
                value = dict(original)
                del value[field]
                with self.assertRaises(ValueError):
                    self.validate(name, value)
        for checkers in (0, 2):
            with self.assertRaisesRegex(ValueError, "exactly one checker"):
                self.validate("union_ordering", {"state": "executed", "checkers": checkers, "unions": 0,
                                                 "inconsistent": 0})

    def test_parent_failure_and_failure_records_are_typed(self):
        original = self.record["envelope"]["row"]["phase2"]["parent_pointers"]
        for invalid in (False, 0, {}, []):
            with self.assertRaisesRegex(ValueError, "string or null"):
                self.validate("parent_pointers", dict(original, failure=invalid))
        self.validate("parent_pointers", dict(original, failure="wrong parent"))
        for name, kind in (("union_ordering", "checker_error"), ("parent_pointers", "compiler_error")):
            self.validate(name, {"state": "failed", "class": kind, "reason": "production error"})
            self.validate(name, {"state": "failed", "class": "panic", "reason": "boom", "location": None})
            for value in ([], {"state": "failed", "class": "invented", "reason": "x"},
                          {"state": "failed", "class": kind},
                          {"state": "failed", "class": "panic", "reason": "x"},
                          {"state": "failed", "class": kind, "reason": "x", "extra": 0}):
                with self.subTest(name=name, value=value), self.assertRaises(ValueError):
                    self.validate(name, value)

    def test_trace_payload_matches_its_state(self):
        for value in ({"state": "disabled", "text_hex": "61"}, {"state": "no_content", "text_hex": "61"},
                      {"state": "content"}, {"state": "content", "text_hex": ""},
                      {"state": "content", "text_hex": "AA"}, {"state": "content", "text_hex": "61 62"},
                      {"state": "content", "text_hex": False}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.validate("trace", value, trace=value["state"] != "disabled")
        self.validate("trace", {"state": "content", "text_hex": "6162"}, trace=True)
        self.validate("trace", {"state": "no_content"}, trace=True)
        self.validate("trace", {"state": "failed", "class": "panic", "reason": "boom", "location": None}, trace=True)


class BuildInputs(unittest.TestCase):
    def test_build_inputs_invalidate_capture_and_are_in_the_ledger(self):
        inputs = ("rust-toolchain.toml", ".cargo/config.toml", "crates/tsr_bundled/bundled/libs/lib.d.ts")
        before = corpus.sources()
        ledger = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]["sources"]
        original = Path.read_bytes
        for name in inputs:
            with self.subTest(name=name):
                self.assertIn(name, before)
                self.assertTrue(any(fnmatch.fnmatchcase(name, pattern) for pattern in ledger), name)
                def changed(path):
                    raw = original(path)
                    return raw + b"changed" if path == ROOT / name else raw
                with patch.object(Path, "read_bytes", changed):
                    after = corpus.sources()
                self.assertNotEqual(before[name], after[name])
                self.assertEqual({key for key in before if before[key] != after[key]}, {name})

    def test_display_is_a_required_acceptance_exit(self):
        sprint = tomllib.loads((ROOT / "sprints/P2B.toml").read_text())
        self.assertIn("run.checker.display_parity == 1", sprint["exit"])


class S08Controls(unittest.TestCase):
    def test_reused_adapter_matches_its_s08_controls(self):
        records = load()["records"]
        for vid in S08_CONTROLS:
            with self.subTest(vid):
                record = records[vid]
                corpus.validate_row(record["request"], record["envelope"]["row"])
                result = compare.compare_row(record["native"], record["envelope"]["row"], None)
                self.assertTrue(all(o["category"] in ("match", "disabled") for o in result.values()), result)


if __name__ == "__main__":
    unittest.main()
