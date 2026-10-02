"""Phase 4 X0: the Rust command-line harness's row contract and its captures.

Rows are built as the binary writes them over real scenarios of
data/phase4/scenarios.json.gz; captures are written with
`phase4_corpus.write_capture` into a temporary directory, so no `target/`
capture is needed. A tampered, partial, reordered, wrong-count or stale
capture is rejected or flagged."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_corpus as corpus  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402

WITH_EDITS = "tsc/incremental/change-to-modifier-of-class-expression-field.js"
WATCH = "tscWatch/commandLineWatch/watch-detects-imported-directory-removed.js"
HELP = "tsc/commandLine/help.js"
CHOSEN = (HELP, WITH_EDITS, WATCH)
OPERATION = "execute.CommandLine is Phase 4 X1"


def rebind(directory):
    """Recompute capture.json's artifact digests after a deliberate edit."""
    metadata = json.loads((directory / "capture.json").read_bytes())
    metadata["artifacts"] = {name: digest((directory / name).read_bytes()) for name in corpus.ARTIFACTS}
    (directory / "capture.json").write_bytes(canonical(metadata) + b"\n")


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = corpus.read_document()
        cls.by_id = {item["id"]: item for item in cls.document["scenarios"]}
        cls.digests = cls.document["provenance"]["scenario_digests"]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def row(self, identity, text=b"transcript\n"):
        return corpus.completed_row(self.by_id[identity], self.digests[identity], text)

    def refused(self, identity, text=b"currentDirectory::/\n"):
        row = self.row(identity, text)
        for key in ("sha256", "bytes", "unexpected_diff"):
            del row[key]
        row.update(state="unsupported", operation=OPERATION,
                   progress=dict(row["progress"], stage="initial", commands=0, edits_completed=0))
        return row

    def capture(self, name="capture", ids=CHOSEN, rows=None, texts=None):
        texts = texts or {identity: f"transcript of {identity}\n".encode() for identity in ids}
        return corpus.write_capture(self.root / name, self.document, list(ids), texts, rows=rows)


class RowContract(Fixture):
    def validate(self, row):
        return corpus.validate_row(self.by_id[row["id"]], self.digests[row["id"]], row)

    def test_completed_unsupported_and_failed_rows_validate(self):
        for identity in CHOSEN:
            self.validate(self.row(identity))
            self.validate(self.refused(identity))
        failed = self.refused(WITH_EDITS)
        del failed["operation"]
        failed.update(state="failed", **{"class": "panic"}, reason="boom",
                      location="crates/tsr_compiler/src/program.rs:1:1")
        self.validate(failed)
        failed.update(**{"class": "harness"}, location=None)
        self.validate(failed)

    def test_a_row_is_in_exactly_one_state_with_exactly_its_fields(self):
        for mutate, message in (
                (lambda row: row.update(operation=OPERATION), "missing or extra row field"),
                (lambda row: row.pop("unexpected_diff"), "missing or extra row field"),
                (lambda row: row.update(state="skipped"), "exactly one state"),
                (lambda row: row.update(extra=1), "missing or extra row field"),
                (lambda row: row.update(version=2), "another row version"),
                (lambda row: row.update(id=HELP), "reordered row")):
            row = self.row(WITH_EDITS)
            mutate(row)
            with self.assertRaisesRegex(ValueError, message):
                corpus.validate_row(self.by_id[WITH_EDITS], self.digests[WITH_EDITS], row)

    def test_a_row_names_its_scenario_and_recording(self):
        for key, value, message in (("family", "tsbuild", "another family"), ("baseline", HELP, "another family"),
                                    ("scenario_sha256", "0" * 64, "another recording")):
            row = self.row(WITH_EDITS)
            row[key] = value
            with self.assertRaisesRegex(ValueError, message):
                self.validate(row)

    def test_progress_fits_the_scenarios_edits(self):
        edits = len(self.by_id[WITH_EDITS]["edits"])
        self.assertGreater(edits, 1)
        for change, message in (({"edits": edits + 1}, "does not fit"), ({"edits_completed": edits + 1}, "does not fit"),
                                ({"commands": edits + 2}, "does not fit"), ({"stage": "running"}, "malformed progress"),
                                ({"commands": -1}, "malformed count")):
            row = self.row(WITH_EDITS)
            row["progress"].update(change)
            with self.assertRaisesRegex(ValueError, message):
                self.validate(row)
        # A completed row ran every step; a refusal stopped before the last.
        row = self.row(WITH_EDITS)
        row["progress"].update(edits_completed=edits - 1, commands=edits)
        with self.assertRaisesRegex(ValueError, "did not run every step"):
            self.validate(row)
        row = self.refused(WITH_EDITS)
        row["progress"].update(stage="done")
        with self.assertRaisesRegex(ValueError, "after the last step"):
            self.validate(row)

    def test_a_completed_row_is_its_transcript(self):
        row = self.row(WITH_EDITS)
        row["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "is not its transcript"):
            self.validate(row)
        row = self.row(WITH_EDITS)
        row["unexpected_diff"] = ""
        with self.assertRaisesRegex(ValueError, "null or a text"):
            self.validate(row)
        row["unexpected_diff"] = "Edit [0]:: no change\n!!! Unexpected diff"
        self.validate(row)
        row = self.row(WITH_EDITS)
        row["transcript"]["sha256"] = "XYZ"
        with self.assertRaisesRegex(ValueError, "malformed digest"):
            self.validate(row)

    def test_refusals_and_failures_carry_their_reason(self):
        row = self.refused(WITH_EDITS)
        row["operation"] = ""
        with self.assertRaisesRegex(ValueError, "names its operation"):
            self.validate(row)
        failed = self.refused(WITH_EDITS)
        del failed["operation"]
        for change in ({"class": "timeout", "reason": "x", "location": None},
                       {"class": "panic", "reason": "", "location": None},
                       {"class": "panic", "reason": "x", "location": 3}):
            failed.update(state="failed", **change)
            with self.assertRaisesRegex(ValueError, "malformed failure"):
                self.validate(failed)

    def test_harness_defects_are_separate_from_production_outcomes(self):
        failed = self.refused(WITH_EDITS)
        del failed["operation"]
        failed.update(state="failed", **{"class": "harness"}, reason="the runner named another baseline",
                      location=None)
        self.assertEqual(corpus.harness_problem(failed), "harness failure: the runner named another baseline")
        failed.update(**{"class": "panic"}, reason="boom", location="tools/phase4/tsctests/src/sys.rs:10:5")
        self.assertEqual(corpus.harness_problem(failed), "panic at tools/phase4/tsctests/src/sys.rs:10:5: boom")
        failed["location"] = None
        self.assertEqual(corpus.harness_problem(failed), "panic at unknown location: boom")
        failed["location"] = "crates/tsr_compiler/src/program.rs:3:9"
        self.assertIsNone(corpus.harness_problem(failed))
        failed.update(**{"class": "error"}, reason="Host(Io)", location=None)
        self.validate(failed)
        self.assertIsNone(corpus.harness_problem(failed))
        self.assertIsNone(corpus.harness_problem(self.refused(WITH_EDITS)))


class Selection(Fixture):
    def test_selectors_are_the_binarys(self):
        everything = [item["id"] for item in self.document["scenarios"]]
        self.assertEqual(len(everything), 516)
        self.assertEqual(corpus.selection(self.document, ["all"]), everything)
        self.assertEqual(len(corpus.selection(self.document, ["tscWatch"])), 42)
        self.assertEqual(corpus.selection(self.document, [HELP.removesuffix(".js")]), [HELP])
        # Inventory order, whatever the selector order.
        self.assertEqual(corpus.selection(self.document, [WATCH, HELP]), [HELP, WATCH])
        with self.assertRaisesRegex(ValueError, "no scenario matches"):
            corpus.selection(self.document, ["tsc/commandLine/adds-color-when-FORCE_COLOR-is-set"])
        with self.assertRaisesRegex(ValueError, "at least one selector"):
            corpus.selection(self.document, [])
        self.assertTrue(corpus.full(self.document, everything))
        self.assertFalse(corpus.full(self.document, everything[:-1]))

    def test_the_source_closure_is_the_harness_and_the_production_crates(self):
        sources = corpus.sources()
        self.assertIn("tools/phase4/tsctests/src/runner.rs", sources)
        self.assertIn("tools/phase3/harness/patience.rs", sources)
        self.assertIn("crates/tsr_compiler/src/program_emit.rs", sources)
        self.assertIn("Cargo.lock", sources)
        self.assertFalse([path for path in sources if corpus.test_only(path)])
        self.assertFalse([path for path in sources if "/target/" in path or path.startswith("target/")])


class Captures(Fixture):
    def test_a_written_capture_loads_and_replays(self):
        directory = self.capture()
        capture = corpus.load_capture(directory)
        self.assertEqual([row["id"] for row in capture.rows], list(CHOSEN))
        self.assertTrue(capture.metadata["partial"])
        self.assertEqual(capture.transcripts[HELP], f"transcript of {HELP}\n".encode())
        replayed = corpus.replay(directory, write=False, capture=capture)
        self.assertEqual(replayed["summary"]["states"], {"completed": 3})
        self.assertEqual(replayed["summary"]["harness_errors"], 0)
        # A synthetic capture names no sources, so it is never current.
        self.assertFalse(replayed["source_stable"])
        with mock.patch.object(corpus, "sources", lambda: {}):
            self.assertTrue(corpus.replay(directory, write=False)["source_stable"])

    def test_a_tampered_artifact_is_rejected(self):
        for name, message in (("rows.jsonl", "raw capture artifact changed: rows.jsonl"),
                              ("executable", "raw capture artifact changed: executable"),
                              ("summary.json", "raw capture artifact changed: summary.json")):
            directory = self.capture(name)
            with (directory / name).open("ab") as stream:
                stream.write(b" ")
            with self.assertRaisesRegex(ValueError, message):
                corpus.load_capture(directory)

    def test_a_changed_or_extra_transcript_is_rejected(self):
        directory = self.capture("changed")
        (directory / "baselines" / HELP).write_bytes(b"another transcript\n")
        with self.assertRaisesRegex(ValueError, "not the one its row describes"):
            corpus.load_capture(directory)
        directory = self.capture("extra")
        (directory / "baselines/tsc/extra.js").write_bytes(b"x")
        with self.assertRaisesRegex(ValueError, "no row describes"):
            corpus.load_capture(directory)

    def test_reordered_missing_and_extra_rows_are_rejected(self):
        texts = {identity: identity.encode() for identity in CHOSEN}
        rows = [self.row(identity, texts[identity]) for identity in CHOSEN]
        for name, wrong in (("reordered", [rows[1], rows[0], rows[2]]), ("missing", rows[:2]),
                            ("extra", rows + [copy.deepcopy(rows[0])])):
            directory = self.capture(name, rows=wrong, texts=texts)
            with self.assertRaisesRegex(ValueError, "missing, extra or reordered rows"):
                corpus.load_capture(directory)

    def test_a_capture_over_another_inventory_or_selection_is_rejected(self):
        directory = self.capture()
        metadata = json.loads((directory / "capture.json").read_bytes())
        metadata["inventory"]["sha256"] = "0" * 64
        (directory / "capture.json").write_bytes(canonical(metadata) + b"\n")
        with self.assertRaisesRegex(ValueError, "another inventory"):
            corpus.load_capture(directory)
        directory = self.capture("selection")
        metadata = json.loads((directory / "capture.json").read_bytes())
        metadata["partial"] = False
        (directory / "capture.json").write_bytes(canonical(metadata) + b"\n")
        with self.assertRaisesRegex(ValueError, "selection disagrees"):
            corpus.load_capture(directory)

    def test_the_summary_and_standard_output_agree_with_the_rows(self):
        directory = self.capture()
        summary = json.loads((directory / "summary.json").read_bytes())
        summary["states"] = {"unsupported": {"tsc": 3}}
        text = json.dumps(summary).encode() + b"\n"
        (directory / "summary.json").write_bytes(text)
        (directory / "stdout").write_bytes(text)
        rebind(directory)
        with self.assertRaisesRegex(ValueError, "summary disagrees with the rows"):
            corpus.load_capture(directory)
        directory = self.capture("stdout")
        (directory / "stdout").write_bytes(b"{}\n")
        rebind(directory)
        with self.assertRaisesRegex(ValueError, "standard output is not its summary"):
            corpus.load_capture(directory)

    def test_harness_errors_are_counted_apart(self):
        rows = [self.row(HELP, b"a"), self.refused(WITH_EDITS, b"b"), self.refused(WATCH, b"c")]
        del rows[2]["operation"]
        rows[2].update(state="failed", **{"class": "harness"}, reason="the scenario does not replay", location=None)
        directory = self.capture(rows=rows, texts={HELP: b"a", WITH_EDITS: b"b", WATCH: b"c"})
        replayed = corpus.replay(directory, write=False)
        self.assertEqual(replayed["summary"]["harness_errors"], 1)
        self.assertEqual(replayed["harness_errors"], [{"id": WATCH, "problem": "harness failure: the scenario does "
                                                                              "not replay"}])
        self.assertEqual(replayed["summary"]["operations"], {OPERATION: 1})

    def test_replace_refuses_a_directory_that_is_not_a_capture(self):
        other = self.root / "other"
        other.mkdir()
        (other / "notes.txt").write_text("keep")
        with self.assertRaisesRegex(ValueError, "not a capture directory"):
            corpus.prepare(other, True)
        directory = self.capture()
        with self.assertRaisesRegex(ValueError, "pass --replace"):
            corpus.prepare(directory, False)
        self.assertEqual(list(corpus.prepare(directory, True).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
