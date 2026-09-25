"""Phase 2 C0.3 harness validation over a small capture cut from the recorded run.

The mini capture keeps three real completed cases (an S08 control that matches,
a production panic and a content-mapper refusal) and rebinds their completion
records to its own capture metadata, so every identity check can be exercised
without rerunning the corpus. Skipped when target/phase2/rust is absent.
"""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare as compare  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import s08_p4 as p4  # noqa: E402
from s08_oracle import digest  # noqa: E402

RUST = ROOT / "target/phase2/rust"
NATIVE = ROOT / "target/phase2/native"
CONTROL = "compiler/2dArrays.ts#configuration=0"
PANIC = "compiler/sliceTupleTypeOutOfBounds.ts#configuration=0"
MAPPER = "compiler/contentMapperInvalidExtension.ts#configuration=0"
# S08 acceptance variants that matched in S08 and exercise errors, declaration
# emit, suggestions, JS and multi-file programs.
S08_CONTROLS = (
    "compiler/2dArrays.ts#configuration=0",
    "compiler/ClassDeclaration14.ts#configuration=0",
    "compiler/accessorDeclarationEmitVisibilityErrors.ts#configuration=0",
    "compiler/unusedLocalsAndParameters.ts#configuration=0",
    "conformance/salsa/moduleExportAlias.ts#configuration=0",
)


@unittest.skipUnless((RUST / "capture.json").exists(), "no recorded Rust capture")
class HarnessValidation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.requests = json.loads((RUST / "requests.json").read_bytes())
        cls.index = {request["id"]: i for i, request in enumerate(cls.requests)}

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase2-corpus-"))
        self.addCleanup(shutil.rmtree, self.directory)
        self.build([CONTROL, PANIC, MAPPER])

    def build(self, ids):
        if (self.directory / "cases").exists():
            shutil.rmtree(self.directory / "cases")
        metadata = p4.read(RUST / "capture.json")
        requests = [self.requests[self.index[vid]] for vid in ids]
        metadata = dict(metadata, requests_sha256=digest(p4.canonical(requests) + b"\n"))
        for name in ("requests.json", "capture.json"):
            (self.directory / name).unlink(missing_ok=True)
        p4.write_new(self.directory / "requests.json", requests)
        p4.write_new(self.directory / "capture.json", metadata)
        for link in ("source-snapshot", "executable"):
            target = self.directory / link
            if not target.exists():
                target.symlink_to(RUST / link)
        (self.directory / "cases").mkdir()
        for position, vid in enumerate(ids):
            source = RUST / "cases" / f"{self.index[vid]:05d}"
            destination = self.directory / "cases" / f"{position:05d}"
            shutil.copytree(source, destination)
            envelope = p4.read(destination / "result.json")
            envelope["capture_sha256"] = digest(p4.canonical(metadata) + b"\n")
            (destination / "result.json").write_bytes(p4.canonical(envelope) + b"\n")
        self.metadata = metadata

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
        with self.assertRaisesRegex(ValueError, "identical native capture"):
            corpus.run(NATIVE, self.directory, 1, self.metadata["timeout_seconds"] + 5, resume=True, limit=3)


@unittest.skipUnless((RUST / "comparison.json").exists(), "no comparison of the recorded run")
class S08Controls(unittest.TestCase):
    def test_reused_adapter_matches_its_s08_controls(self):
        rows = {row["id"]: row for row in json.loads((RUST / "comparison.json").read_bytes())["rows"]}
        for vid in S08_CONTROLS:
            with self.subTest(vid):
                self.assertEqual(rows[vid]["s08"], "acceptance")
                self.assertTrue(all(o in ("match", "disabled") for o in rows[vid]["outcomes"].values()), rows[vid])


if __name__ == "__main__":
    unittest.main()
