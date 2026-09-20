"""Phase 1 F0 runner, manifest and failure-path tests.

These exercise the dispatcher's contracts without capturing a corpus: every
case here builds a small synthetic capture directory on disk. The real native
and Rust smoke lives in `python3 scripts/phase1.py capture --family pilot`.
"""

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import phase1  # noqa: E402
import phase1_baselines as baselines  # noqa: E402
import phase1_capture as capture  # noqa: E402
import phase1_scope as scope  # noqa: E402
from s08_oracle import canonical  # noqa: E402


def sha(data: bytes) -> str:
    import hashlib

    return hashlib.sha256(data).hexdigest()


class ManifestTests(unittest.TestCase):
    def test_committed_scope_has_no_unclassified_operation(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        self.assertEqual(scope.verify(document), [])
        self.assertEqual(
            sum(document["counts"].values()),
            document["total_operations"],
            "every operation must carry exactly one disposition",
        )

    def test_scope_rebuilds_to_the_committed_bytes(self):
        committed = json.loads((ROOT / "data/phase1/scope.json").read_text())
        self.assertEqual(scope.build(), committed, "scope.json is stale against its inputs")

    def test_equivalent_rust_cannot_be_rule_derived(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        forged = json.loads(json.dumps(document))
        forged["operations"][0]["disposition"] = "equivalent_rust"
        problems = scope.verify(forged)
        self.assertTrue(
            any("reviewed behavioral witness" in p for p in problems),
            "equivalent_rust must require a reviewed witness, not a rule",
        )

    def test_counts_cannot_disagree_with_rows(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        forged = json.loads(json.dumps(document))
        forged["counts"]["covered"] += 1
        self.assertTrue(any("counts disagree" in p for p in scope.verify(forged)))


class BaselineIndexTests(unittest.TestCase):
    def setUp(self):
        self.index = json.loads((ROOT / "data/phase1/config-baselines.json").read_text())

    def test_exactly_309_outputs_in_the_declared_groups(self):
        self.assertEqual(self.index["total_outputs"], 309)
        self.assertEqual(
            {g: len(v["outputs"]) for g, v in self.index["groups"].items()},
            {"matchFiles": 142, "tsconfigParsing": 87, "parseCommandLine": 53, "parseBuildOptions": 27},
        )

    def test_nested_matching_outputs_are_indexed(self):
        # Two matchFiles outputs live under a subdirectory because the test
        # title contains a slash. A maxdepth-1 glob would index 140.
        names = [row["name"] for row in self.index["groups"]["matchFiles"]["outputs"]]
        nested = [name for name in names if "/" in name]
        self.assertEqual(len(nested), 2, f"expected two nested outputs, indexed {nested}")

    def test_index_matches_the_pin(self):
        self.assertEqual(baselines.verify(self.index), [])

    def test_only_two_subfolders_are_written_by_pinned_tests(self):
        self.assertEqual(baselines.verify_written_subfolders(), [])

    def test_a_changed_hash_is_rejected(self):
        forged = json.loads(json.dumps(self.index))
        forged["groups"]["tsconfigParsing"]["outputs"][0]["sha256"] = "0" * 64
        self.assertTrue(any("hash differs" in p for p in baselines.verify(forged)))

    def test_a_dropped_output_is_rejected(self):
        forged = json.loads(json.dumps(self.index))
        dropped = forged["groups"]["parseCommandLine"]["outputs"].pop()
        problems = baselines.verify(forged)
        self.assertTrue(any(dropped["name"] in p for p in problems))

    def test_matching_group_authority_is_recorded_as_blocked(self):
        authority = self.index["groups"]["matchFiles"]["authority"]
        self.assertEqual(authority["status"], "blocked")
        self.assertIsNone(authority["renderer"])
        self.assertTrue(authority["blocker"])

    def test_established_groups_name_a_renderer_and_invocation(self):
        for group in ("tsconfigParsing", "parseCommandLine", "parseBuildOptions"):
            authority = self.index["groups"][group]["authority"]
            self.assertEqual(authority["status"], "established", group)
            self.assertTrue(authority["renderer"], group)
            self.assertTrue(authority["invocation"], group)


class SyntheticCapture:
    """A minimal on-disk capture the comparison layer will accept."""

    def __init__(self, directory: Path, rows, native_rows=None, partial=False, selected=None):
        self.directory = directory
        (directory / "native").mkdir(parents=True)
        requests = {"version": 1, "family": "pilot",
                    "requests": [{"case": r["case"], "operation": "vfsmatch.readDirectory"} for r in rows]}
        request_bytes = canonical(requests) + b"\n"
        (directory / "requests.json").write_bytes(request_bytes)
        native = {"version": 1, "observations": native_rows if native_rows is not None else [
            {"case": r["case"], "operation": "vfsmatch.readDirectory", "result": "observed",
             "observation": r["native"]} for r in rows]}
        native_bytes = canonical(native) + b"\n"
        (directory / "native/observations.json").write_bytes(native_bytes)
        rust = {"version": 1, "observations": [
            {"case": r["case"], "operation": "vfsmatch.readDirectory", **r["rust"]} for r in rows]}
        rust_bytes = canonical(rust) + b"\n"
        (directory / "rust-observations.json").write_bytes(rust_bytes)
        provenance = {
            "version": 1, "family": "pilot", "pin": "x" * 40, "partial": partial,
            "selected_cases": selected if selected is not None else [r["case"] for r in rows],
            "requests_sha256": sha(request_bytes),
            "native_observations_sha256": sha(native_bytes),
            "rust_observations_sha256": sha(rust_bytes),
            "rust_binary_sha256": "0" * 64,
            "source_closure": {"data/phase1/requests/pilot.json": sha(
                (ROOT / "data/phase1/requests/pilot.json").read_bytes())},
            "host": {}, "go": "go1.27.1", "goos": "darwin", "goarch": "arm64",
        }
        (directory / "provenance.json").write_bytes(canonical(provenance) + b"\n")


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.temporary, ignore_errors=True)
        self.real_families = dict(capture.FAMILIES)
        # Point the family at a synthetic request file so the comparison's
        # full-inventory check has a known denominator.
        self.requests = Path(self.temporary) / "requests-inventory.json"

    def build(self, name, rows, **kwargs):
        directory = Path(self.temporary) / name
        SyntheticCapture(directory, rows, **kwargs)
        return directory

    def with_inventory(self, cases):
        document = {"version": 1, "family": "pilot",
                    "requests": [{"case": c, "operation": "vfsmatch.readDirectory"} for c in cases]}
        self.requests.write_text(json.dumps(document))
        capture.FAMILIES = dict(capture.FAMILIES)
        capture.FAMILIES["pilot"] = dict(capture.FAMILIES["pilot"])
        capture.FAMILIES["pilot"]["requests"] = str(self.requests.relative_to(ROOT)) \
            if str(self.requests).startswith(str(ROOT)) else str(self.requests)
        self.addCleanup(setattr, capture, "FAMILIES", self.real_families)

    def test_agreeing_observation_is_a_match(self):
        rows = [{"case": "a", "native": {"files": ["/x.ts"]},
                 "rust": {"result": "observed", "observation": {"files": ["/x.ts"]}}}]
        self.with_inventory(["a"])
        report = capture.compare(self.build("ok", rows))
        self.assertEqual(report["counts"]["match"], 1)
        self.assertEqual(report["parity"], 1.0)

    def test_reordered_list_is_different_not_a_match(self):
        rows = [{"case": "a", "native": {"files": ["/x.ts", "/y.ts"]},
                 "rust": {"result": "observed", "observation": {"files": ["/y.ts", "/x.ts"]}}}]
        self.with_inventory(["a"])
        report = capture.compare(self.build("order", rows))
        self.assertEqual(report["counts"]["different"], 1)
        self.assertEqual(report["counts"]["match"], 0)

    def test_absent_rust_api_is_not_implemented_not_a_match(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "not_implemented",
                          "missing_operation": {"operation": "tsoptions.parseCommandLine"}}}]
        self.with_inventory(["a"])
        report = capture.compare(self.build("missing", rows))
        self.assertEqual(report["counts"]["not_implemented"], 1)
        self.assertEqual(report["required_non_match"], 1)

    def test_harness_failure_invalidates_the_capture(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "harness_failed", "error": "driver panicked"}}]
        self.with_inventory(["a"])
        with self.assertRaisesRegex(ValueError, "failed in the harness"):
            capture.compare(self.build("harness", rows))

    def test_partial_capture_leaves_unselected_cases_not_run(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a", "b"])
        report = capture.compare(self.build("partial", rows, partial=True))
        self.assertEqual(report["counts"]["not_run"], 1)
        self.assertLess(report["parity"], 1.0, "an unrun case cannot be counted as parity")

    def test_require_parity_rejects_a_partial_capture(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a", "b"])
        directory = self.build("partial-strict", rows, partial=True)
        with self.assertRaisesRegex(ValueError, "unrun"):
            capture.compare(directory, require_parity=True)

    def test_require_parity_rejects_a_missing_operation(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "not_implemented", "missing_operation": {}}}]
        self.with_inventory(["a"])
        with self.assertRaisesRegex(ValueError, "non-matching"):
            capture.compare(self.build("strict-missing", rows), require_parity=True)

    def test_tampered_observation_bytes_are_rejected(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a"])
        directory = self.build("tampered", rows)
        (directory / "rust-observations.json").write_bytes(b'{"version":1,"observations":[]}\n')
        with self.assertRaisesRegex(ValueError, "does not match its recorded hash"):
            capture.compare(directory)

    def test_source_change_after_capture_is_rejected(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a"])
        directory = self.build("moved-source", rows)
        provenance = json.loads((directory / "provenance.json").read_text())
        provenance["source_closure"]["data/phase1/requests/pilot.json"] = "0" * 64
        (directory / "provenance.json").write_bytes(canonical(provenance) + b"\n")
        with self.assertRaisesRegex(ValueError, "changed after the capture"):
            capture.compare(directory)

    def test_missing_rust_row_for_a_selected_case_is_a_harness_failure(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a"])
        directory = self.build("dropped-row", rows)
        # Rebuild the rust output without the row, then re-hash so the failure
        # is attributed to the missing row rather than to tampering.
        body = canonical({"version": 1, "observations": []}) + b"\n"
        (directory / "rust-observations.json").write_bytes(body)
        provenance = json.loads((directory / "provenance.json").read_text())
        provenance["rust_observations_sha256"] = sha(body)
        (directory / "provenance.json").write_bytes(canonical(provenance) + b"\n")
        with self.assertRaisesRegex(ValueError, "failed in the harness"):
            capture.compare(directory)

    def test_empty_inventory_does_not_yield_parity_one(self):
        self.with_inventory([])
        report = capture.compare(self.build("empty", []))
        self.assertEqual(report["parity"], 0.0)
        self.assertEqual(report["counts"]["match"], 0)

    def test_comparison_runs_no_child_process(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a"])
        directory = self.build("replay", rows)
        import subprocess

        original = subprocess.run

        def refuse(*args, **kwargs):
            raise AssertionError("compare must not run a child process")

        subprocess.run = refuse
        try:
            capture.compare(directory)
        finally:
            subprocess.run = original


class JoinTests(unittest.TestCase):
    def report(self, rows, family="pilot", schedule="s1"):
        counts = {result: 0 for result in capture.RESULTS}
        for row in rows:
            counts[row["result"]] += 1
        return {"version": 1, "family": family, "requests_sha256": schedule,
                "counts": counts, "rows": rows, "partial": False}

    def test_duplicate_conflicting_results_are_rejected(self):
        a = self.report([{"case": "x", "result": "match"}])
        b = self.report([{"case": "x", "result": "different"}])
        with self.assertRaisesRegex(ValueError, "reported twice"):
            capture.join([a, b])

    def test_incompatible_request_schedules_are_rejected(self):
        a = self.report([{"case": "x", "result": "match"}], schedule="s1")
        b = self.report([{"case": "y", "result": "match"}], schedule="s2")
        with self.assertRaisesRegex(ValueError, "different request schedules"):
            capture.join([a, b])

    def test_not_run_is_superseded_by_a_real_result(self):
        a = self.report([{"case": "x", "result": "not_run"}])
        b = self.report([{"case": "x", "result": "match"}])
        joined = capture.join([a, b])
        self.assertEqual(joined["counts"]["match"], 1)
        self.assertEqual(joined["counts"]["not_run"], 0)

    def test_full_acceptance_rejects_an_incomplete_report(self):
        a = self.report([{"case": "x", "result": "match"}, {"case": "y", "result": "not_run"}])
        with self.assertRaisesRegex(ValueError, "rejects 1 unrun"):
            capture.join([a], selection_required=True)


class DispatcherTests(unittest.TestCase):
    def test_inventory_check_passes_on_the_committed_manifests(self):
        result = phase1.inventory_check()
        self.assertEqual(result["problems"], [])
        self.assertTrue(result["ok"])

    def test_inventory_check_reports_the_blocked_baseline_group(self):
        result = phase1.inventory_check()
        self.assertIn("matchFiles", result["baseline_groups_blocked"])

    def test_unprepared_family_is_refused_with_the_declared_list(self):
        with self.assertRaisesRegex(ValueError, "has no adapter yet"):
            capture.capture("config", Path(tempfile.mkdtemp()) / "out")


if __name__ == "__main__":
    unittest.main()
