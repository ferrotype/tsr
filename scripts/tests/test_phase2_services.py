"""C5.5: the services record groups and digests tests, the manifest excludes timing-dependent tests, the replay
classifies operations against the owner's approvals, and the reference is current only with every input."""
import json
from collections import Counter
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_services as services  # noqa: E402


def approvals(operations=(), exclusions=(), unsupported=()):
    return {"operations": dict.fromkeys(operations, {"approved_by": "owner"}),
            "exclusions": dict.fromkeys(exclusions, {"approved_by": "owner"}),
            "unsupported": dict.fromkeys(unsupported, {"approved_by": "owner"})}


class Record(unittest.TestCase):
    def test_tests_are_grouped_without_sequence_numbers_and_empty_tests_record_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            raw = Path(directory) / "raw.ndjson"
            raw.write_text("\n".join(json.dumps(event) for event in [
                {"e": "end", "n": 1},
                {"e": "test", "name": "TestA", "files": {}, "symlinks": {}, "n": 2},
                {"args": [], "checker": "c1", "e": "call", "op": "Checker.WasCanceled", "results": [False], "n": 3},
                {"e": "end", "n": 4},
                {"e": "end", "n": 5},
            ]) + "\n")
            tests = list(services.recorded_tests(raw))
        self.assertEqual([name for name, _ in tests], ["TestA"])
        events = tests[0][1]
        self.assertTrue(all(b'"n"' not in event for event in events))
        self.assertEqual(services.operation_counts(events), Counter({"Checker.WasCanceled": 1}))
        self.assertEqual(services.test_digest(events), services.test_digest(list(events)))

    def test_the_manifest_keeps_only_reproduced_tests_and_names_the_rest(self):
        oracle = {"command": ["go"], "overlay_sha256": {"a": "b"}, "entry_points": 3}
        outcomes = {"TestA": "PASS", "TestB": "PASS", "TestC": "SKIP"}
        runs = (outcomes, outcomes, dict(outcomes))
        digests = ({"TestA": "1", "TestB": "2"}, {"TestA": "1", "TestB": "3"})
        manifest = services.manifest_of(oracle, runs, digests, "pin", "go1", Counter({"op": 2}), "r", "x")
        self.assertTrue(manifest["neutral"])
        self.assertEqual(manifest["excluded_nondeterministic"], ["TestB"])
        self.assertEqual(manifest["test_digests"], {"TestA": "1"})
        self.assertEqual(manifest["skipped"], ["TestC"])
        changed = services.manifest_of(oracle, (outcomes, {**outcomes, "TestA": "FAIL"}, outcomes), digests,
                                       "pin", "go1", Counter(), "r", "x")
        self.assertFalse(changed["neutral"])


class Classification(unittest.TestCase):
    recorded = {"Checker.A": 2, "Checker.B": 3, "Checker.C": 1}

    def classify(self, operations, unsupported=None, exclusions=None, approved=None, whole=True):
        return services.classify(self.recorded, operations, unsupported or {}, exclusions or {},
                                 approved or approvals(), whole=whole)

    def test_matched_and_excluded_calls_replay_an_operation(self):
        states, excluded, complete = self.classify({
            "Checker.A": Counter(match=2), "Checker.B": Counter(match=1, excluded=2), "Checker.C": Counter(match=1)},
            exclusions={"Phase 5: content mappers": Counter(calls=2, programs=1)},
            approved=approvals(exclusions=["Phase 5: content mappers"]))
        self.assertEqual({op: entry["state"] for op, entry in states.items()},
                         dict.fromkeys(self.recorded, "replayed"))
        self.assertTrue(excluded["Phase 5: content mappers"]["approved"])
        self.assertTrue(complete)

    def test_an_unapproved_exclusion_or_open_operation_keeps_the_replay_incomplete(self):
        base = {"Checker.A": Counter(match=2), "Checker.B": Counter(match=3), "Checker.C": Counter(match=1)}
        _, _, complete = self.classify(base, exclusions={"Phase 5: content mappers": Counter(calls=1)})
        self.assertFalse(complete)
        states, _, complete = self.classify({**base, "Checker.C": Counter(mismatch=1)})
        self.assertEqual(states["Checker.C"]["state"], "open")
        self.assertFalse(complete)

    def test_unsupported_calls_need_an_approved_reason_for_every_call(self):
        operations = {"Checker.A": Counter(match=1, unsupported=1), "Checker.B": Counter(match=3),
                      "Checker.C": Counter(unsupported=1)}
        unsupported = {"Checker.A": Counter({"synthetic node argument": 1}),
                       "Checker.C": Counter({"something else": 1})}
        states, _, complete = self.classify(operations, unsupported,
                                            approved=approvals(unsupported=["synthetic node argument"]))
        self.assertEqual(states["Checker.A"]["state"], "approved")
        self.assertEqual(states["Checker.C"]["state"], "open")
        self.assertFalse(complete)
        states, _, complete = self.classify(operations, unsupported,
                                            approved=approvals(operations=["Checker.C"],
                                                               unsupported=["synthetic node argument"]))
        self.assertEqual(states["Checker.C"]["state"], "approved")
        self.assertTrue(complete)

    def test_a_short_whole_replay_is_incomplete_and_a_subset_is_never_complete(self):
        operations = {"Checker.A": Counter(match=1), "Checker.B": Counter(match=3), "Checker.C": Counter(match=1)}
        states, _, complete = self.classify(operations)
        self.assertEqual(states["Checker.A"]["state"], "incomplete")
        self.assertFalse(complete)
        _, _, complete = self.classify(operations, whole=False)
        self.assertFalse(complete)


class Currency(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.record = self.root / "record.xz"
        self.record.write_bytes(b"record")
        self.manifest = self.root / "manifest.json"
        self.manifest.write_text(json.dumps({"record_xz_sha256": services.digest(b"record"), "pin": "pin"}))
        self.approvals = self.root / "approvals.json"
        self.approvals.write_text(json.dumps({"operations": {}, "exclusions": {}, "unsupported": {}}))
        self.receipt = self.root / "receipt.json"
        for name, value in (("RECORD", self.record), ("RECEIPT", self.receipt), ("APPROVALS", self.approvals)):
            patcher = patch.object(services, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        for target, value in (("pin", "pin"),):
            patcher = patch.object(services, target, return_value=value)
            patcher.start()
            self.addCleanup(patcher.stop)
        import phase2_producers
        patcher = patch.object(phase2_producers, "source_inputs", return_value={"crates/x.rs": "s"})
        self.sources = patcher.start()
        self.addCleanup(patcher.stop)

    def write_receipt(self, **overrides):
        receipt = {"version": 1, "state": "observed", "complete": True,
                   "manifest_sha256": services.digest(self.manifest.read_bytes()),
                   "approvals_sha256": services.digest(self.approvals.read_bytes()),
                   "source_inputs": {"crates/x.rs": "s"}, **overrides}
        self.receipt.write_text(json.dumps(receipt))

    def test_a_complete_receipt_over_this_manifest_approvals_and_sources_is_current(self):
        self.write_receipt()
        self.assertTrue(services.current(self.manifest))

    def test_a_missing_or_incomplete_receipt_is_not_current(self):
        self.assertFalse(services.current(self.manifest))
        self.write_receipt(complete=False)
        self.assertFalse(services.current(self.manifest))

    def test_a_changed_manifest_approval_record_or_source_invalidates_the_receipt(self):
        self.write_receipt()
        self.approvals.write_text(json.dumps({"operations": {"Checker.A": {"approved_by": "owner"}}}))
        self.assertFalse(services.current(self.manifest))
        self.write_receipt()
        self.sources.return_value = {"crates/x.rs": "t"}
        self.assertFalse(services.current(self.manifest))
        self.sources.return_value = {"crates/x.rs": "s"}
        self.record.write_bytes(b"other")
        self.assertFalse(services.current(self.manifest))


if __name__ == "__main__":
    unittest.main()
