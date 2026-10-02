"""Phase 4 X0: the `tsc` producer's harness validity and metrics, the recorded
comparison and the blocker register, over full captures written from the
committed references with `phase4_corpus.write_capture` (the source
fingerprint is patched, as the Phase 3 producer tests patch theirs; the
inventory verification, which runs Go, is stubbed). A tampered, partial or
stale capture leaves `harness_valid` false and withholds the acceptance
metrics; a register that misses a bucket is not complete."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_blockers as blockers  # noqa: E402
import phase4_compare as compare  # noqa: E402
import phase4_corpus as corpus  # noqa: E402
import phase4_producers as producers  # noqa: E402

OPERATION = "execute.CommandLine is Phase 4 X1"
WITH_EDITS = "tsc/incremental/change-to-modifier-of-class-expression-field.js"
BUILD = "tsbuild/sample/always-builds-under-with-force-option.js"
ACCEPTANCE = ("unsupported_required", "baseline_parity", "incremental_correctness")


def verified():
    return {"reproduce": True, "identical": True, "replay": True}


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = corpus.read_document()
        cls.ids = [item["id"] for item in cls.document["scenarios"]]
        cls.by_id = {item["id"]: item for item in cls.document["scenarios"]}
        cls.references = {key: value[0] for key, value in compare.read_references(cls.ids).items()}

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        patches = [mock.patch.object(corpus, "sources", lambda: {}),
                   mock.patch.object(compare, "RECORD", self.root / "first-comparison.json"),
                   mock.patch.object(blockers, "REGISTER", self.root / "blockers.json")]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def refused_rows(self):
        """The real first run's rows: each scenario refused at its first command,
        its transcript the reference up to the command line."""
        texts, rows = {}, []
        for identity in self.ids:
            reference = self.references[identity]
            cut = reference.index(b"\ntsgo ") + 1
            texts[identity] = reference[:cut] + reference[cut:].split(b"\n", 1)[0] + b"\n"
            row = corpus.completed_row(self.by_id[identity], self.document["provenance"]["scenario_digests"][identity],
                                       texts[identity])
            for key in ("sha256", "bytes", "unexpected_diff"):
                del row[key]
            row.update(state="unsupported", operation=OPERATION,
                       progress=dict(row["progress"], stage="initial", commands=0, edits_completed=0))
            rows.append(row)
        return texts, rows

    def capture(self, name="rust", texts=None, rows=None, selectors=("all",)):
        return corpus.write_capture(self.root / name, self.document, list(selectors), texts or self.references,
                                    rows=rows)

    def metrics(self, rust, verify=verified):
        return producers.tsc(rust, verify=verify)["metrics"]

    def record(self, rust):
        comparison = compare.report(rust)
        compare.record(comparison)
        blockers.build(rust, record=True, comparison=comparison)
        return comparison


class Producer(Fixture):
    def test_the_first_run_is_valid_unsupported_and_recorded(self):
        texts, rows = self.refused_rows()
        rust = self.capture(texts=texts, rows=rows)
        metrics = self.metrics(rust)
        self.assertTrue(metrics["harness_valid"])
        self.assertFalse(metrics["result_recorded"])
        self.assertFalse(metrics["blockers_named"])
        self.record(rust)
        metrics = self.metrics(rust)
        for name in ("inventory_frozen", "inventory_verified", "inventory_reproduced", "inventory_identical",
                     "inventory_replayed", "harness_valid", "result_recorded", "blockers_named"):
            self.assertIs(metrics[name], True, name)
        self.assertEqual((metrics["unsupported_required"], metrics["baseline_parity"],
                          metrics["incremental_correctness"]), (516, 0.0, 0.0))
        register = json.loads(blockers.REGISTER.read_bytes())
        self.assertEqual([(entry["kind"], entry["cause"], entry["owner"], entry["scenarios"])
                          for entry in register["entries"]], [("unsupported", OPERATION, "X1", 516)])
        self.assertEqual(register["cross_phase"][0]["state"], "landed")

    def test_matching_references_give_full_parity(self):
        rust = self.capture()
        self.record(rust)
        metrics = self.metrics(rust)
        self.assertEqual((metrics["unsupported_required"], metrics["baseline_parity"],
                          metrics["incremental_correctness"]), (0, 1.0, 1.0))
        self.assertTrue(metrics["blockers_named"])
        self.assertEqual(json.loads(blockers.REGISTER.read_bytes())["entries"], [])

    def test_a_failed_verification_check_is_reported(self):
        rust = self.capture()
        metrics = self.metrics(rust, verify=lambda: dict(verified(), replay=False))
        self.assertFalse(metrics["inventory_verified"])
        self.assertFalse(metrics["inventory_replayed"])
        self.assertTrue(metrics["inventory_reproduced"])

    def test_a_partial_tampered_stale_or_missing_capture_withholds_the_metrics(self):
        partial = self.capture("partial", selectors=("tscWatch",))
        tampered = self.capture("tampered")
        (tampered / "baselines" / WITH_EDITS).write_bytes(b"later")
        stale = self.capture("stale")
        for rust in (partial, tampered, stale, self.root / "missing"):
            with mock.patch.object(corpus, "sources", lambda: {"crates/x.rs": "0" * 64} if rust == stale else {}):
                metrics = self.metrics(rust)
            self.assertFalse(metrics["harness_valid"], rust)
            self.assertFalse(metrics["result_recorded"])
            self.assertFalse(any(name in metrics for name in ACCEPTANCE), rust)
            self.assertTrue(metrics["inventory_verified"])

    def test_a_harness_error_invalidates_the_run(self):
        texts, rows = self.refused_rows()
        rows[0].pop("operation")
        rows[0].update(state="failed", **{"class": "harness"}, reason="the scenario does not replay", location=None)
        metrics = self.metrics(self.capture(texts=texts, rows=rows))
        self.assertFalse(metrics["harness_valid"])

    def test_the_recorded_result_must_be_this_runs(self):
        first = self.capture("first")
        self.record(first)
        texts = dict(self.references)
        texts[WITH_EDITS] = texts[WITH_EDITS].replace(b"Success", b"Failure", 1) + b"\n"
        second = self.capture("second", texts=texts)
        metrics = self.metrics(second)
        self.assertTrue(metrics["harness_valid"])
        self.assertFalse(metrics["result_recorded"])
        self.assertFalse(metrics["blockers_named"])


class Register(Fixture):
    def test_a_new_bucket_makes_the_committed_register_incomplete(self):
        texts, rows = self.refused_rows()
        rust = self.capture(texts=texts, rows=rows)
        comparison = self.record(rust)
        register = json.loads(blockers.REGISTER.read_bytes())
        self.assertTrue(blockers.complete(register, comparison))
        self.assertEqual(blockers.check(rust), (True, register))
        rows[0]["operation"] = "tsr_incremental::Snapshot::semantic_diagnostics_per_file is crate-private (Phase 4 X2)"
        other = self.capture("other", texts=texts, rows=rows)
        rebuilt = blockers.build(other)
        self.assertEqual([(entry["owner"], entry["scenarios"]) for entry in rebuilt["entries"]], [("X1", 515), ("X2", 1)])
        self.assertFalse(blockers.complete(register, compare.report(other)))

    def test_owners_follow_the_named_checkpoint_the_emit_residual_and_the_families(self):
        self.assertEqual(blockers.owner_of("unsupported", OPERATION, {"tsc"}, set()), ("X1", False))
        self.assertEqual(blockers.owner_of("different", compare.cause_of(
            {"category": "different", "emitted_file": True, "first": {"step": "initial", "section": "files"}}),
            {"tsbuild"}, {"files"}), (blockers.EMIT_RESIDUAL, True))
        self.assertEqual(blockers.owner_of("different", "output differs (edit step)", {"tsc", "tsbuildWatch"},
                                           {"output"}), ("X1/X5", False))
        self.assertEqual(blockers.owner_of("different", "buildinfo differs (initial step)", {"tsc"}, {"buildinfo"}),
                         ("X2", False))
        self.assertEqual(blockers.owner_of("failed", "panic: boom at crates/x.rs:1", {"tsbuild"}, set()), ("X3", False))

    def test_differences_are_bucketed_by_section_and_step(self):
        texts = dict(self.references)
        texts[BUILD] = texts[BUILD].replace(b"ExitStatus:: Success", b"ExitStatus:: NotImplemented", 1)
        rust = self.capture(texts=texts)
        register = blockers.build(rust)
        self.assertEqual([(entry["kind"], entry["cause"], entry["owner"], entry["families"])
                          for entry in register["entries"]],
                         [("different", "command differs (initial step)", "X3", {"tsbuild": 1})])
        self.assertEqual(register["entries"][0]["evidence"][0]["transcript"], f"baselines/{BUILD}")

    def test_a_partial_comparison_has_no_register(self):
        rust = self.capture("partial", selectors=("tsc",))
        with self.assertRaisesRegex(ValueError, "full comparison"):
            blockers.build(rust)

    def test_the_emit_dependency_is_read_from_the_evidence(self):
        state = blockers.emit_state()
        self.assertEqual(state["state"], "landed")
        self.assertIn("program_emit.rs", state["evidence"])
        with mock.patch.object(blockers, "PHASE3_COMPARISON", self.root / "absent.json"):
            self.assertEqual(blockers.emit_state()["state"], "open")


if __name__ == "__main__":
    unittest.main()
