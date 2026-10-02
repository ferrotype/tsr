"""Phase 4 X0: the command-line comparison over the committed references.

The step and section split is checked on every committed reference (lossless,
every step and section found). The comparison is mutation-checked for real:
synthetic completed rows built from the committed references are written as
captures and reported, and a changed byte, a dropped edit step, two swapped
outputs and two swapped transcripts must each read `different` while the
untouched references read `match`."""
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_compare as compare  # noqa: E402
import phase4_corpus as corpus  # noqa: E402

WITH_EDITS = "tsc/incremental/change-to-modifier-of-class-expression-field.js"
EXPLAINED = "tscWatch/commandLineWatch/watch-detects-imported-directory-removed.js"
BUILD = "tsbuild/sample/always-builds-under-with-force-option.js"
BUILD_WATCH = "tsbuildWatch/sample/reportErrors-when-stopBuildOnErrors-is-passed-on-command-line.js"
HELP = "tsc/commandLine/help.js"
CHOSEN = (HELP, WITH_EDITS, BUILD, EXPLAINED, BUILD_WATCH)
ORPHAN = "tsc/commandLine/adds-color-when-FORCE_COLOR-is-set.js"
OPERATION = "execute.CommandLine is Phase 4 X1"


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = corpus.read_document()
        cls.by_id = {item["id"]: item for item in cls.document["scenarios"]}
        identities = [item["id"] for item in cls.document["scenarios"]]
        cls.references = {key: value[0] for key, value in compare.read_references(identities).items()}

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def report(self, texts, rows=None, name="capture", selectors=CHOSEN):
        directory = corpus.write_capture(self.root / name, self.document, list(selectors), texts, rows=rows)
        return compare.report(directory)

    def rows_of(self, comparison):
        return {row["id"]: row for row in comparison["rows"]}


class Split(Fixture):
    def test_every_reference_splits_losslessly_into_its_steps_and_sections(self):
        explained = 0
        for scenario in self.document["scenarios"]:
            text = self.references[scenario["id"]]
            steps = compare.split(text, scenario)
            self.assertEqual(b"".join(value for _, parts in steps for _, value in parts), text, scenario["id"])
            self.assertEqual([name for name, _ in steps],
                             ["initial"] + [f"edit {index}" for index in range(len(scenario["edits"]))])
            watch = scenario["family"].endswith("Watch")
            for index, (name, parts) in enumerate(steps):
                found = dict(parts)
                self.assertIn("output", found, (scenario["id"], name))
                self.assertIn("input" if index == 0 else "edit", found)
                # A watch scenario's edits run a watch cycle, every other step a command.
                self.assertEqual("command" in found, index == 0 or not watch, (scenario["id"], name))
                if "command" in found:
                    self.assertRegex(found["command"], rb"\Atsgo [^\n]*\nExitStatus:: \w+\Z")
                self.assertFalse(compare.ENTRY.search(found["output"]), (scenario["id"], name))
                fs = found["fs"]
                self.assertTrue(fs == b"\n" or compare.entry_starts(fs)[0].start() == 0, (scenario["id"], name))
                explained += "incremental" in found
        # The seven explained differences of the plan's section 5.
        self.assertEqual(explained, 7)

    def test_entries_name_their_paths_and_labels(self):
        steps = compare.split(self.references[WITH_EDITS], self.by_id[WITH_EDITS])
        files = compare.entries(dict(steps[0][1])["fs"])
        self.assertEqual([(path, label) for path, label, _ in files][:2],
                         [("/home/src/tslibs/TS/Lib/lib.es2025.full.d.ts", "*Lib*"),
                          ("/home/src/workspaces/project/MessageablePerson.js", "*new*")])
        self.assertTrue(compare.is_build_info("/home/src/workspaces/project/tsconfig.tsbuildinfo"))
        self.assertTrue(compare.is_build_info("/p/tsconfig.tsbuildinfo.readable.baseline.txt"))
        head = compare.entries(dict(steps[0][1])["input"])
        self.assertEqual(head[0][:2], ("", "header"))
        self.assertTrue(head[0][2].startswith(b"currentDirectory::"))


class Comparison(Fixture):
    def test_untouched_references_match_and_the_orphan_is_apart(self):
        comparison = self.report({identity: self.references[identity] for identity in CHOSEN})
        summary = comparison["summary"]
        self.assertEqual(summary["rows"], 516)
        self.assertEqual(summary["categories"], {"match": 5, "different": 0, "failed": 0, "unsupported": 0,
                                                 "unexecuted": 511})
        self.assertTrue(summary["partial"])
        self.assertTrue(summary["valid"])
        self.assertEqual(sum(summary["categories"].values()), 516)
        self.assertEqual(summary["edit_steps"], sum(len(item["edits"]) for item in self.document["scenarios"]))
        self.assertEqual(summary["edit_steps_agreeing"], sum(len(self.by_id[identity]["edits"]) for identity in CHOSEN))
        self.assertEqual([item["reference"] for item in comparison["orphan_references"]], [ORPHAN])
        self.assertNotIn(ORPHAN, self.rows_of(comparison))
        self.assertEqual(comparison["buckets"]["unexecuted"][0]["cause"], "not selected by the run")

    def test_a_refused_row_is_unsupported_and_its_transcript_a_prefix(self):
        texts, rows = {}, []
        for identity in corpus.selection(self.document, list(CHOSEN)):
            reference = self.references[identity]
            texts[identity] = reference[:reference.index(b"\ntsgo ") + 1]
            texts[identity] += reference[len(texts[identity]):].split(b"\n", 1)[0] + b"\n"
            row = corpus.completed_row(self.by_id[identity], self.document["provenance"]["scenario_digests"][identity],
                                       texts[identity])
            for key in ("sha256", "bytes", "unexpected_diff"):
                del row[key]
            row.update(state="unsupported", operation=OPERATION,
                       progress=dict(row["progress"], stage="initial", commands=0, edits_completed=0))
            rows.append(row)
        comparison = self.report(texts, rows)
        self.assertEqual(comparison["summary"]["categories"]["unsupported"], 5)
        self.assertEqual(comparison["summary"]["stopped_rows_prefix_of_reference"], 5)
        self.assertEqual(comparison["summary"]["edit_steps_agreeing"], 0)
        self.assertEqual(comparison["buckets"]["unsupported"],
                         [{"cause": OPERATION, "rows": 5, "families": {"tsbuild": 1, "tsbuildWatch": 1, "tsc": 2,
                                                                       "tscWatch": 1},
                           "examples": [BUILD, BUILD_WATCH, HELP]}])

    def test_a_difference_names_its_step_section_path_and_line(self):
        reference = self.references[WITH_EDITS]
        # The incremental build info of the initial build, one byte changed.
        at = reference.index(b'"version":"FakeTSVersion"') + 2
        texts = {WITH_EDITS: reference[:at] + b"V" + reference[at + 1:]}
        row = self.rows_of(self.report(texts, selectors=(WITH_EDITS,)))[WITH_EDITS]
        self.assertEqual(row["category"], "different")
        first = row["first"]
        self.assertEqual((first["step"], first["section"], first["path"], first["difference"],
                          first["reference_label"], first["rust_label"]),
                         ("initial", "buildinfo", "/home/src/workspaces/project/tsconfig.tsbuildinfo", "text", "*new*",
                          "*new*"))
        self.assertEqual(row["sections"], ["buildinfo"])
        self.assertEqual(row["incremental_agreeing"], len(self.by_id[WITH_EDITS]["edits"]))
        # A changed exit status is the command's; a missing file is named.
        texts = {WITH_EDITS: reference.replace(b"ExitStatus:: DiagnosticsPresent_OutputsGenerated",
                                               b"ExitStatus:: Success", 1)}
        first = self.rows_of(self.report(texts, name="status", selectors=(WITH_EDITS,)))[WITH_EDITS]["first"]
        self.assertEqual((first["step"], first["section"], first["line"]), ("initial", "command", 2))
        start = reference.index(b"//// [/home/src/workspaces/project/main.js] *new* ")
        end = reference.index(b"//// [", start + 1)
        texts = {WITH_EDITS: reference[:start] + reference[end:]}
        first = self.rows_of(self.report(texts, name="missing", selectors=(WITH_EDITS,)))[WITH_EDITS]["first"]
        self.assertEqual((first["section"], first["path"], first["difference"], first["rust_label"]),
                         ("files", "/home/src/workspaces/project/main.js", "missing", None))

    def test_an_emitted_file_difference_is_flagged(self):
        reference = self.references[WITH_EDITS]
        at = reference.index(b"function logMessage(person) {")
        texts = {WITH_EDITS: reference[:at] + b"async " + reference[at:]}
        comparison = self.report(texts, selectors=(WITH_EDITS,))
        row = self.rows_of(comparison)[WITH_EDITS]
        self.assertEqual((row["first"]["section"], row["first"]["path"], row["emitted_file"]),
                         ("files", "/home/src/workspaces/project/main.js", True))
        self.assertEqual(comparison["buckets"]["different"][0]["cause"], "an emitted file differs")

    def test_an_incremental_difference_is_counted_per_edit_step(self):
        reference = self.references[EXPLAINED]
        texts = {EXPLAINED: reference.replace(b"\n\nDiff:: ", b"\n\nDiff:: (changed) ", 1)}
        row = self.rows_of(self.report(texts, selectors=(EXPLAINED,)))[EXPLAINED]
        self.assertEqual(row["first"]["section"], "incremental")
        self.assertEqual(row["incremental_agreeing"], len(self.by_id[EXPLAINED]["edits"]) - 1)

    def test_harness_defects_invalidate_the_report(self):
        texts = {identity: self.references[identity] for identity in CHOSEN}
        order = corpus.selection(self.document, list(CHOSEN))
        rows = [corpus.completed_row(self.by_id[identity], self.document["provenance"]["scenario_digests"][identity],
                                     texts[identity]) for identity in order]
        at = {identity: index for index, identity in enumerate(order)}
        rows[at[HELP]]["unexpected_diff"] = "Edit [0]:: no change\n!!! Unexpected diff"
        comparison = self.report(texts, rows)
        self.assertFalse(comparison["summary"]["valid"])
        self.assertEqual(self.rows_of(comparison)[HELP]["category"], "failed")
        self.assertEqual(comparison["harness_errors"][0]["id"], HELP)
        rows[at[HELP]]["unexpected_diff"] = None
        failed = rows[at[WITH_EDITS]]
        del failed["sha256"], failed["bytes"], failed["unexpected_diff"]
        failed.update(state="failed", **{"class": "panic"}, reason="boom",
                      location="tools/phase4/tsctests/src/runner.rs:1:1")
        comparison = self.report(texts, rows, name="panic")
        self.assertEqual(comparison["harness_errors"], [
            {"id": WITH_EDITS, "problem": "panic at tools/phase4/tsctests/src/runner.rs:1:1: boom"}])
        failed["location"] = "crates/tsr_compiler/src/program.rs:1:1"
        comparison = self.report(texts, rows, name="production")
        self.assertTrue(comparison["summary"]["valid"])
        self.assertEqual(self.rows_of(comparison)[WITH_EDITS]["reason"],
                         "panic: boom at crates/tsr_compiler/src/program.rs:1:1")

    def test_only_a_full_valid_current_run_is_recorded(self):
        texts = {identity: self.references[identity] for identity in CHOSEN}
        target = self.root / "first-comparison.json"
        with self.assertRaisesRegex(ValueError, "partial"):
            compare.record(self.report(texts), target)
        full = corpus.write_capture(self.root / "full", self.document, ["all"], self.references)
        comparison = compare.report(full)
        self.assertEqual(comparison["summary"]["matched"], 516)
        with self.assertRaisesRegex(ValueError, "current sources"):
            compare.record(comparison, target)
        with mock.patch.object(corpus, "sources", lambda: {}):
            comparison = compare.report(full)
        compare.record(comparison, target)
        recorded = json.loads(target.read_bytes())
        self.assertEqual(recorded, json.loads(json.dumps(compare.acceptance_summary(comparison))))
        self.assertNotIn("rows", recorded)


class Mutation(Fixture):
    """The plan's mutation check, run through captures on disk."""

    def test_every_mutation_reads_different_and_untouched_rows_match(self):
        summary = compare.mutation(self.root / "mutation", list(CHOSEN))
        self.assertTrue(summary["passed"], json.dumps(summary, indent=1))
        variants = summary["variants"]
        self.assertEqual((variants["untouched"]["controls"], variants["untouched"]["controls_matched"]), (5, 5))
        for name, mutated in (("changed_byte", 5), ("dropped_edit", 4), ("swapped_outputs", 2), ("swapped_rows", 2)):
            self.assertEqual(variants[name]["mutated"], mutated, name)
            self.assertEqual(variants[name]["mutated_different"], mutated, name)
        for name in ("changed_byte", "dropped_edit", "swapped_outputs"):
            self.assertEqual(variants[name]["attributed_to_the_mutated_section"], variants[name]["mutated"], name)
        self.assertEqual(variants["dropped_edit"]["first_sections"], {"step": 4})
        self.assertEqual(variants["swapped_outputs"]["first_sections"], {"output": 2})
        self.assertTrue((self.root / "mutation/mutation.json").is_file())

    def test_each_mutation_changes_the_text(self):
        for identity in CHOSEN:
            reference, scenario = self.references[identity], self.by_id[identity]
            for index in range(4):
                mutated, (step, section) = compare.changed_byte(reference, scenario, index)
                self.assertNotEqual(mutated, reference)
                self.assertEqual(len(mutated), len(reference))
                self.assertIn(section, compare.SECTIONS)
            if scenario["edits"]:
                text, expected = compare.dropped_edit(reference, scenario)
                self.assertTrue(reference.startswith(text) and len(text) < len(reference))
                self.assertEqual(expected, (f"edit {len(scenario['edits']) - 1}", "step"))
        self.assertIsNone(compare.dropped_edit(self.references[HELP], self.by_id[HELP]))
        self.assertIsNone(compare.swapped_outputs(self.references[HELP], self.by_id[HELP]))
        # Every step of WITH_EDITS reports the same two errors: nothing to swap.
        self.assertIsNone(compare.swapped_outputs(self.references[WITH_EDITS], self.by_id[WITH_EDITS]))
        text, expected = compare.swapped_outputs(self.references[EXPLAINED], self.by_id[EXPLAINED])
        self.assertEqual(sorted(text), sorted(self.references[EXPLAINED]))
        self.assertNotEqual(text, self.references[EXPLAINED])
        self.assertEqual(expected, ("initial", "output"))

    def test_the_mutated_markers_are_never_rewritten(self):
        for line, column in ((b"tsgo --b tests", 5), (b"ExitStatus:: Success", 13), (b"Output::", None),
                             (b"//// [/a.ts] *new* ", None), (b"export {};", 0)):
            self.assertEqual(compare.mutable_column(line), column)
        self.assertTrue(re.fullmatch(r"edit \d+", compare.dropped_edit(self.references[WITH_EDITS],
                                                                       self.by_id[WITH_EDITS])[1][0]))


if __name__ == "__main__":
    unittest.main()


class ApprovedDifferences(unittest.TestCase):
    def test_only_the_exact_paired_observation_is_approved(self):
        ledger = json.loads(compare.APPROVED.read_bytes())
        row = ledger["exceptions"][0]["observations"][0]
        # Use small bytes while preserving the reviewed scenario identity.
        row["native_sha256"] = compare.digest(b"native")
        row["rust_sha256"] = compare.digest(b"rust")
        self.assertEqual(compare.approved_difference(row["scenario"], b"native", b"rust", ledger),
                         "P4-eager-bind-trace-order")
        self.assertIsNone(compare.approved_difference(row["scenario"], b"native", b"rust changed", ledger))
        self.assertIsNone(compare.approved_difference(row["scenario"], b"native changed", b"rust", ledger))
        self.assertIsNone(compare.approved_difference("unreviewed-scenario", b"native", b"rust", ledger))
        ledger["exceptions"][0]["approved"] = False
        self.assertIsNone(compare.approved_difference(row["scenario"], b"native", b"rust", ledger))

    def test_approval_does_not_survive_a_pin_change(self):
        ledger = json.loads(compare.APPROVED.read_bytes())
        ledger["pin"] = "unreviewed"
        with self.assertRaisesRegex(ValueError, "another version or pin"):
            compare.approved_difference("scenario", b"a", b"b", ledger)
