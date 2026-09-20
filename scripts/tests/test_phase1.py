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
    """A minimal on-disk capture the comparison layer will accept.

    Built in the v2 shape: one directory per native probe, the real pin and
    gitlink, and the closure `source_closure` currently computes, so the
    authenticator's completeness check is exercised rather than bypassed.
    """

    def __init__(self, directory: Path, rows, native_rows=None, partial=False, selected=None,
                 closure=None):
        self.directory = directory
        probe_directory = directory / "native" / "vfsmatch"
        probe_directory.mkdir(parents=True)
        requests = {"version": 1, "family": "pilot",
                    "requests": [{"case": r["case"], "operation": "vfsmatch.readDirectory"} for r in rows]}
        request_bytes = canonical(requests) + b"\n"
        (directory / "requests.json").write_bytes(request_bytes)

        native = {"version": 1, "observations": native_rows if native_rows is not None else [
            {"case": r["case"], "operation": "vfsmatch.readDirectory", "result": "observed",
             "observation": r["native"]} for r in rows]}
        native_bytes = canonical(native) + b"\n"
        (probe_directory / "observations.json").write_bytes(native_bytes)
        # Each probe carries its own provenance in a real capture; freeze
        # installs both files per probe.
        (probe_directory / "provenance.json").write_bytes(canonical({
            "pin": capture.pin(), "package": "vfs/vfsmatch",
            "test": "TestPhase1PilotReadDirectory", "trimpath": True,
            "output_sha256": sha(native_bytes),
            "go": "go1.27.1", "goos": "darwin", "goarch": "arm64",
        }) + b"\n")

        rust = {"version": 1, "observations": [
            {"case": r["case"], "operation": "vfsmatch.readDirectory", **r["rust"]} for r in rows]}
        rust_bytes = canonical(rust) + b"\n"
        (directory / "rust-observations.json").write_bytes(rust_bytes)

        recorded = capture.source_closure("pilot") if closure is None else closure
        provenance = {
            "version": 3, "family": "pilot", "pin": capture.pin(),
            "upstream_gitlink": capture.gitlink(), "partial": partial,
            "selected_cases": selected if selected is not None else [r["case"] for r in rows],
            "requests_sha256": sha(request_bytes),
            "native_probes": {
                "vfsmatch": {
                    "package": "vfs/vfsmatch",
                    "directory": "native/vfsmatch",
                    "observations_sha256": sha(native_bytes),
                    "go": "go1.27.1", "goos": "darwin", "goarch": "arm64", "trimpath": True,
                }
            },
            "rust_observations_sha256": sha(rust_bytes),
            "rust_binary_sha256": "0" * 64,
            "source_closure": recorded,
            "source_closure_size": len(recorded),
            "host": {},
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
                          "missing_operation": {
                              "operation": "tsoptions.parseCommandLine",
                              "go_authority": "commandlineparser.go:ParseCommandLine",
                              "intended_signature": "pub fn parse_command_line(...)",
                              "production_home": "crates/tsr_tsoptions/src/command_line.rs"}}}]
        self.with_inventory(["a"])
        report = capture.compare(self.build("missing", rows))
        self.assertEqual(report["counts"]["not_implemented"], 1)
        self.assertEqual(report["required_non_match"], 1)

    def test_harness_failure_invalidates_the_capture(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "harness_failed", "error": "driver panicked"}}]
        self.with_inventory(["a"])
        # Raised by validate_capture before any comparison happens.
        with self.assertRaisesRegex(ValueError, "Rust harness failure"):
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
                 "rust": {"result": "not_implemented",
                          "missing_operation": {
                              "operation": "tsoptions.parseCommandLine",
                              "go_authority": "commandlineparser.go:ParseCommandLine",
                              "intended_signature": "pub fn parse_command_line(...)",
                              "production_home": "crates/tsr_tsoptions/src/command_line.rs"}}}]
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
        target = "crates/tsr_tsoptions/src/glob.rs"
        self.assertIn(target, provenance["source_closure"],
                      "the production matcher must be part of the closure")
        provenance["source_closure"][target] = "0" * 64
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
        # The sequence check rejects a dropped row before it can be mistaken
        # for a merely absent case.
        with self.assertRaisesRegex(ValueError, "rows for 1 requests"):
            capture.compare(directory)

    def test_empty_inventory_does_not_yield_parity_one(self):
        self.with_inventory([])
        report = capture.compare(self.build("empty", []))
        self.assertEqual(report["parity"], 0.0)
        self.assertEqual(report["counts"]["match"], 0)

    def test_comparison_spawns_no_build_or_observation_child(self):
        """Replay must not rebuild, re-run Go, or re-run the Rust driver.

        A read-only `git ls-files` is still allowed: the gitlink is what proves
        the capture was taken against this pin.
        """
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        self.with_inventory(["a"])
        directory = self.build("replay", rows)
        import subprocess

        original = subprocess.run
        spawned = []

        def record(args, *rest, **kwargs):
            spawned.append(list(args) if isinstance(args, (list, tuple)) else [args])
            return original(args, *rest, **kwargs)

        subprocess.run = record
        try:
            capture.compare(directory)
        finally:
            subprocess.run = original

        for argv in spawned:
            program = Path(str(argv[0])).name
            self.assertNotIn(program, ("go", "cargo"),
                             f"compare spawned a build or observation child: {argv}")
            self.assertNotIn("phase1_pilot", str(argv[0]),
                             f"compare re-ran the Rust driver: {argv}")


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

    def test_f0_is_not_reported_complete_while_outputs_are_unmapped(self):
        result = phase1.inventory_check()
        self.assertFalse(result["f0_complete"],
                         "F0 cannot claim completion while 142 outputs have no mapping")
        self.assertTrue(result["f0_outstanding"])
        self.assertEqual(result["baseline_outputs_verified"], 167)

    def test_manifest_health_is_separate_from_f0_completion(self):
        # The manifests are internally consistent; that is a different claim
        # from the checkpoint being finished.
        result = phase1.inventory_check()
        self.assertTrue(result["ok"])
        self.assertNotEqual(result["ok"], result["f0_complete"])

    def test_unprepared_family_is_refused_with_the_declared_list(self):
        with self.assertRaisesRegex(ValueError, "has no adapter yet"):
            capture.capture("config", Path(tempfile.mkdtemp()) / "out")


class SourceClosureTests(unittest.TestCase):
    """A capture must stale when production code the driver links changes."""

    def test_closure_contains_the_production_matcher_and_vfs(self):
        closure = capture.source_closure("pilot")
        self.assertIn("crates/tsr_tsoptions/src/glob.rs", closure,
                      "the configuration matcher must invalidate a matching capture")
        self.assertTrue(any(k.startswith("crates/tsr_vfs/src/") for k in closure),
                        "the filesystem the driver builds on must be in the closure")

    def test_closure_contains_the_example_target_and_build_inputs(self):
        closure = capture.source_closure("pilot")
        for required in ("crates/tsr_tsoptions/examples/phase1_pilot.rs",
                         "Cargo.lock", "Cargo.toml", "rust-toolchain.toml",
                         "data/upstream.json", "scripts/s08_oracle.py",
                         "scripts/s04_common.py"):
            self.assertIn(required, closure, required)

    def test_closure_contains_every_native_probe(self):
        closure = capture.source_closure("pilot")
        for probe in capture.FAMILIES["pilot"]["native_probes"]:
            self.assertIn(probe["probe"], closure, probe["probe"])

    def test_closure_is_derived_from_the_dependency_graph(self):
        # tsr_tsoptions links these transitively; a hand-listed closure would
        # drift from the manifest the moment a dependency is added.
        packages = {p.name for p in capture.workspace_closure("tsr_tsoptions")}
        for name in ("tsr_tsoptions", "tsr_vfs", "tsr_jsstring", "tsr_tspath"):
            self.assertIn(name, packages, name)


class ResponseValidationTests(unittest.TestCase):
    """Malformed responses must be rejected before anything is indexed."""

    def requests(self, *cases):
        return [{"case": c, "operation": "vfsmatch.readDirectory"} for c in cases]

    def document(self, rows):
        return {"version": 1, "observations": rows}

    def ok(self, case):
        return {"case": case, "operation": "vfsmatch.readDirectory",
                "result": "observed", "observation": {"files": []}}

    def test_a_well_formed_response_is_accepted(self):
        requests = self.requests("a", "b")
        rows = capture.validate_response(
            self.document([self.ok("a"), self.ok("b")]), requests, "rust")
        self.assertEqual([r["case"] for r in rows], ["a", "b"])

    def test_duplicate_rows_are_rejected(self):
        requests = self.requests("a", "b")
        with self.assertRaisesRegex(ValueError, "reordered or substituted|duplicates"):
            capture.validate_response(
                self.document([self.ok("a"), self.ok("a")]), requests, "rust")

    def test_extra_rows_are_rejected(self):
        requests = self.requests("a")
        with self.assertRaisesRegex(ValueError, "rows for 1 requests"):
            capture.validate_response(
                self.document([self.ok("a"), self.ok("b")]), requests, "rust")

    def test_missing_rows_are_rejected(self):
        requests = self.requests("a", "b")
        with self.assertRaisesRegex(ValueError, "rows for 2 requests"):
            capture.validate_response(self.document([self.ok("a")]), requests, "rust")

    def test_reordered_response_is_rejected(self):
        requests = self.requests("a", "b")
        with self.assertRaisesRegex(ValueError, "reordered or substituted"):
            capture.validate_response(
                self.document([self.ok("b"), self.ok("a")]), requests, "rust")

    def test_unknown_status_is_rejected(self):
        requests = self.requests("a")
        row = dict(self.ok("a"), result="totally_fine")
        with self.assertRaisesRegex(ValueError, "unknown rust status"):
            capture.validate_response(self.document([row]), requests, "rust")

    def test_a_side_cannot_report_the_other_side_status(self):
        requests = self.requests("a")
        row = dict(self.ok("a"), result="native_unavailable", reason="x")
        with self.assertRaisesRegex(ValueError, "unknown rust status"):
            capture.validate_response(self.document([row]), requests, "rust")
        row = dict(self.ok("a"), result="not_implemented")
        with self.assertRaisesRegex(ValueError, "unknown native status"):
            capture.validate_response(self.document([row]), requests, "native")

    def test_operation_substitution_is_rejected(self):
        requests = self.requests("a")
        row = dict(self.ok("a"), operation="locale.selectTranslation")
        with self.assertRaisesRegex(ValueError, "request asked for"):
            capture.validate_response(self.document([row]), requests, "rust")

    def test_observed_without_a_payload_is_rejected(self):
        requests = self.requests("a")
        row = {"case": "a", "operation": "vfsmatch.readDirectory", "result": "observed"}
        with self.assertRaisesRegex(ValueError, "no observation payload"):
            capture.validate_response(self.document([row]), requests, "rust")

    def test_not_implemented_without_a_complete_record_is_rejected(self):
        requests = self.requests("a")
        row = {"case": "a", "operation": "vfsmatch.readDirectory",
               "result": "not_implemented", "missing_operation": {"operation": "x"}}
        with self.assertRaisesRegex(ValueError, "complete missing_operation"):
            capture.validate_response(self.document([row]), requests, "rust")

    def test_truncated_document_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "no observations array"):
            capture.validate_response({"version": 1}, self.requests("a"), "rust")


class MembershipTests(unittest.TestCase):
    def setUp(self):
        self.scope = json.loads((ROOT / "data/phase1/scope.json").read_text())

    def test_every_pinned_package_has_a_membership_decision(self):
        packages = {
            line.split("\t")[1]
            for line in (ROOT / "data/go-functions.tsv").read_text().splitlines()[2:]
            if line.strip()
        }
        self.assertEqual(sorted(packages - set(scope.MEMBERSHIP)), [])

    def test_syntax_packages_are_in_scope_despite_their_ledger_phase(self):
        present = {row["go_package"] for row in self.scope["operations"]}
        for package in ("internal/ast", "internal/scanner", "internal/parser",
                        "internal/binder", "internal/compiler", "internal/astnav"):
            self.assertIn(package, present, package)

    def test_later_phase_helpers_are_excluded_with_a_destination(self):
        present = {row["go_package"] for row in self.scope["operations"]}
        for package in ("internal/ls", "internal/lsp", "internal/api", "internal/checker",
                        "internal/testutil/projecttestutil", "internal/testutil/lsptestutil"):
            self.assertNotIn(package, present, package)
            self.assertIn(package, self.scope["excluded_packages"], package)
            self.assertTrue(self.scope["excluded_packages"][package]["reason"])

    def test_compiler_membership_is_recorded_as_partial(self):
        rows = [r for r in self.scope["operations"] if r["go_package"] == "internal/compiler"]
        self.assertTrue(rows)
        self.assertTrue(all(r["membership"] == "partial" for r in rows))

    def test_every_row_carries_case_and_dependency_links(self):
        for row in self.scope["operations"]:
            self.assertIn("cases", row, row["id"])
            self.assertIn("depends_on", row, row["id"])

    def test_prepared_cases_resolve_to_scope_operations(self):
        linked = {row["id"] for row in self.scope["operations"] if row["cases"]}
        self.assertTrue(linked, "no scope row links a prepared case")


class InvocationMappingTests(unittest.TestCase):
    def setUp(self):
        self.index = json.loads((ROOT / "data/phase1/config-baselines.json").read_text())

    def test_every_output_carries_a_per_output_mapping_decision(self):
        for group, value in self.index["groups"].items():
            for row in value["outputs"]:
                self.assertIn("rendering_verified", row, f"{group}/{row['name']}")
                self.assertIn("invocation", row, f"{group}/{row['name']}")

    def test_established_groups_name_a_concrete_test_per_output(self):
        for group in ("tsconfigParsing", "parseCommandLine", "parseBuildOptions"):
            for row in self.index["groups"][group]["outputs"]:
                self.assertTrue(row["rendering_verified"], f"{group}/{row['name']}")
                self.assertTrue(row["invocation"]["test"], f"{group}/{row['name']}")

    def test_the_blocked_group_has_no_invocation(self):
        for row in self.index["groups"]["matchFiles"]["outputs"]:
            self.assertIsNone(row["invocation"])
            self.assertFalse(row["rendering_verified"])

    def test_mapping_totals_are_consistent(self):
        mapping = self.index["invocation_mapping"]
        self.assertEqual(mapping["verified_outputs"], 167)
        self.assertEqual(mapping["unrendered_outputs"], 142)
        self.assertEqual(mapping["problem_outputs"], 0)
        self.assertEqual(mapping["outputs_rendered_but_not_indexed"], [])

    def test_a_forged_verification_claim_is_rejected(self):
        forged = json.loads(json.dumps(self.index))
        row = forged["groups"]["matchFiles"]["outputs"][0]
        row["rendering_verified"] = True
        problems = baselines.verify(forged)
        self.assertTrue(any("verified rendering with no invocation" in p for p in problems))

    def test_an_index_without_a_mapping_is_rejected(self):
        forged = json.loads(json.dumps(self.index))
        del forged["invocation_mapping"]
        self.assertTrue(any("no invocation mapping" in p for p in baselines.verify(forged)))

    def test_the_instrumentation_anchor_still_applies_to_the_pin(self):
        # If the pin moves and baseline.Run changes, the recorder must fail
        # loudly rather than silently recording nothing.
        import phase1_invocations as invocations

        text, _ = invocations.instrumented_source()
        self.assertIn("phase1RecordInvocation(t, subfolder, fileName, actual)", text)
        self.assertIn("writeComparison(t, actual, localPath, referencePath)", text)


class NativeMergeTests(unittest.TestCase):
    """A native harness failure must never be merged away."""

    def setUp(self):
        self.temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.temporary, ignore_errors=True)

    def capture_with(self, name, probe_rows):
        """Build a capture whose probes report the given rows for one case."""
        directory = Path(self.temporary) / name
        requests = [{"case": "a", "operation": "vfsmatch.readDirectory"}]
        provenance = {"native_probes": {}}
        for package, row in probe_rows.items():
            probe_directory = directory / "native" / package
            probe_directory.mkdir(parents=True)
            body = canonical({"version": 1, "observations": [row]}) + b"\n"
            (probe_directory / "observations.json").write_bytes(body)
            provenance["native_probes"][package] = {
                "package": "vfs/vfsmatch",
                "directory": f"native/{package}",
                "observations_sha256": sha(body),
            }
        return directory, provenance, requests

    def test_an_earlier_unavailable_cannot_hide_a_later_failure(self):
        # "aaa" sorts before "zzz", so the unavailable row is merged first and
        # previously won the setdefault.
        directory, provenance, requests = self.capture_with("hidden-by-unavailable", {
            "aaa": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "native_unavailable", "reason": "not mine"},
            "zzz": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "harness_failed", "error": "probe crashed"},
        })
        with self.assertRaisesRegex(ValueError, "native harness failure"):
            capture._merge_native(directory, provenance, requests)

    def test_an_observed_row_cannot_hide_a_failure(self):
        directory, provenance, requests = self.capture_with("hidden-by-observed", {
            "aaa": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "harness_failed", "error": "probe crashed"},
            "zzz": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "observed", "observation": {"files": []}},
        })
        with self.assertRaisesRegex(ValueError, "native harness failure"):
            capture._merge_native(directory, provenance, requests)

    def test_the_failure_cause_is_reported(self):
        directory, provenance, requests = self.capture_with("cause", {
            "aaa": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "harness_failed", "error": "probe crashed"},
        })
        with self.assertRaisesRegex(ValueError, "probe crashed"):
            capture._merge_native(directory, provenance, requests)

    def test_two_observing_probes_are_still_ambiguous(self):
        directory, provenance, requests = self.capture_with("ambiguous", {
            "aaa": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "observed", "observation": {"files": []}},
            "zzz": {"case": "a", "operation": "vfsmatch.readDirectory",
                    "result": "observed", "observation": {"files": []}},
        })
        with self.assertRaisesRegex(ValueError, "authority is ambiguous"):
            capture._merge_native(directory, provenance, requests)


class NativeDependencyTests(unittest.TestCase):
    """The closure must cover what the native side actually executes."""

    def test_go_toolchain_and_upstream_helpers_are_in_the_closure(self):
        closure = capture.source_closure("pilot")
        for required in (
            "data/s04/toolchains.toml",      # selects the required Go version
            "scripts/s04.py",                # go_environment and verified_upstream
            "scripts/s04_runtime.py",        # toolchain pin validation
            "scripts/tracking-bootstrap.py", # loaded by verified_upstream
            "scripts/phase1_invocations.py", # the baseline instrumentation
        ):
            self.assertIn(required, closure, required)

    def test_changing_the_required_go_version_invalidates_a_capture(self):
        temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, temporary, ignore_errors=True)
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        directory = Path(temporary) / "toolchain"
        SyntheticCapture(directory, rows)
        provenance = json.loads((directory / "provenance.json").read_text())
        target = "data/s04/toolchains.toml"
        self.assertIn(target, provenance["source_closure"])
        provenance["source_closure"][target] = "0" * 64
        (directory / "provenance.json").write_bytes(canonical(provenance) + b"\n")
        with self.assertRaisesRegex(ValueError, f"{target} changed after the capture"):
            capture.compare(directory)


class CoverageLinkTests(unittest.TestCase):
    """`covered` requires exact operation-level links, not file-level metrics."""

    def setUp(self):
        self.scope = json.loads((ROOT / "data/phase1/scope.json").read_text())

    def test_every_covered_row_has_exact_case_links(self):
        for row in self.scope["operations"]:
            if row["disposition"] == "covered":
                self.assertTrue(row["cases"], f"{row['id']} is covered with no case links")

    def test_file_level_metrics_alone_do_not_make_an_operation_covered(self):
        # A mapped operation whose file carries producer metrics but which has
        # no case link is untested, not covered. (An *unmapped* operation in
        # such a file stays `missing`; the metrics say nothing about it either.)
        rows = [
            r for r in self.scope["operations"]
            if r.get("ledger_verification") and not r["cases"] and r["mapped_in_ledger"]
        ]
        self.assertTrue(rows, "expected mapped operations carrying only file-level metrics")
        for row in rows:
            self.assertEqual(row["disposition"], "implemented_untested", row["id"])
        self.assertGreater(len(rows), 1000, "the overclaim affected thousands of rows")

    def test_file_level_metrics_are_retained_for_context(self):
        rows = [r for r in self.scope["operations"] if r.get("ledger_verification")]
        self.assertTrue(rows)
        self.assertTrue(any("run." in v for r in rows for v in r["ledger_verification"]))

    def test_a_covered_row_without_links_is_rejected(self):
        forged = json.loads(json.dumps(self.scope))
        row = next(r for r in forged["operations"] if r["disposition"] == "implemented_untested")
        row["disposition"] = "covered"
        forged["counts"]["covered"] += 1
        forged["counts"]["implemented_untested"] -= 1
        problems = scope.verify(forged)
        self.assertTrue(any("covered requires exact case links" in p for p in problems))

    def test_unlinked_coverage_is_named_as_outstanding_f0_work(self):
        result = phase1.inventory_check()
        self.assertTrue(
            any("operation-level coverage link" in item for item in result["f0_outstanding"]),
            "connecting existing evidence to operation ids must be named, not assumed",
        )


class FreezeTests(unittest.TestCase):
    """Freezing must handle the multi-probe layout and authenticate first."""

    def setUp(self):
        self.temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.temporary, ignore_errors=True)

    def test_freeze_authenticates_before_installing(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        directory = Path(self.temporary) / "unauthentic"
        SyntheticCapture(directory, rows)
        (directory / "rust-observations.json").write_bytes(b'{"version":1,"observations":[]}\n')
        with self.assertRaisesRegex(ValueError, "does not match its recorded hash"):
            phase1.freeze(directory)

    def test_freeze_refuses_a_partial_capture(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        directory = Path(self.temporary) / "partial"
        SyntheticCapture(directory, rows, partial=True)
        with self.assertRaisesRegex(ValueError, "partial capture cannot be frozen"):
            phase1.freeze(directory)

    def snapshot(self, directory):
        """Every file under a directory, by relative path and bytes."""
        if not directory.is_dir():
            return None
        return {
            str(p.relative_to(directory)): p.read_bytes()
            for p in sorted(directory.rglob("*"))
            if p.is_file()
        }

    def build_capture(self, name, rows, **kwargs):
        directory = Path(self.temporary) / name
        SyntheticCapture(directory, rows, **kwargs)
        return directory

    def test_freeze_rejects_a_native_harness_failure(self):
        """A correctly hashed capture can still record a failed observation."""
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        directory = self.build_capture("native-failed", rows, native_rows=[
            {"case": "a", "operation": "vfsmatch.readDirectory",
             "result": "harness_failed", "error": "probe crashed"}])
        with self.assertRaisesRegex(ValueError, "native harness failure"):
            phase1.freeze(directory)

    def test_freeze_rejects_a_rust_harness_failure(self):
        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "harness_failed", "error": "driver panicked"}}]
        directory = self.build_capture("rust-failed", rows)
        with self.assertRaisesRegex(ValueError, "Rust harness failure"):
            phase1.freeze(directory)

    def test_freeze_accepts_not_implemented(self):
        """Preparation legitimately freezes native truth for absent Rust APIs."""
        rows = [{"case": "a", "native": {"files": ["/x.ts"]},
                 "rust": {"result": "not_implemented",
                          "missing_operation": {
                              "operation": "tsoptions.parseCommandLine",
                              "go_authority": "commandlineparser.go:ParseCommandLine",
                              "intended_signature": "pub fn parse_command_line(...)",
                              "production_home": "crates/tsr_tsoptions/src/command_line.rs"}}}]
        directory = self.build_capture("not-implemented", rows)
        original = self.snapshot(ROOT / "data/phase1/native/pilot")
        self.addCleanup(self.restore, ROOT / "data/phase1/native/pilot", original)
        result = phase1.freeze(directory)
        self.assertEqual(result["frozen"], "pilot")

    def restore(self, directory, original):
        if original is None:
            shutil.rmtree(directory, ignore_errors=True)
            return
        shutil.rmtree(directory, ignore_errors=True)
        for relative, body in original.items():
            target = directory / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(body)

    def test_a_rejected_freeze_preserves_the_existing_frozen_inventory(self):
        installed = ROOT / "data/phase1/native/pilot"
        before = self.snapshot(installed)
        self.assertIsNotNone(before, "expected committed frozen observations to protect")

        rows = [{"case": "a", "native": {"files": []},
                 "rust": {"result": "observed", "observation": {"files": []}}}]
        directory = self.build_capture("rejected", rows, native_rows=[
            {"case": "a", "operation": "vfsmatch.readDirectory",
             "result": "harness_failed", "error": "probe crashed"}])
        with self.assertRaises(ValueError):
            phase1.freeze(directory)

        self.assertEqual(self.snapshot(installed), before,
                         "a rejected freeze must not touch the frozen inventory")
        for leftover in ("pilot.incoming", "pilot.outgoing"):
            self.assertFalse((ROOT / "data/phase1/native" / leftover).exists(), leftover)

    def test_the_committed_frozen_layout_is_per_probe(self):
        installed = ROOT / "data/phase1/native/pilot"
        if not installed.is_dir():
            self.skipTest("no frozen pilot observations are committed")
        self.assertTrue((installed / "capture-provenance.json").is_file())
        provenance = json.loads((installed / "capture-provenance.json").read_text())
        for probe in provenance["native_probes"].values():
            directory = installed / probe["directory"].removeprefix("native/")
            self.assertTrue((directory / "observations.json").is_file(), str(directory))
            self.assertTrue((directory / "provenance.json").is_file(), str(directory))


class OrderSensitiveTests(unittest.TestCase):
    """Order-sensitive cases must not be compared through a sorting canonicaliser."""

    def test_canonical_alone_is_order_blind(self):
        # The premise: this is why the extra rules exist.
        self.assertEqual(canonical({"b": 1, "a": 2}), canonical({"a": 2, "b": 1}))

    def test_entry_arrays_survive_canonicalisation(self):
        self.assertNotEqual(canonical([["b", 1], ["a", 2]]), canonical([["a", 2], ["b", 1]]))

    def test_order_safe_problems_flags_a_nested_ordered_map(self):
        # A multi-key object *inside* an element is an ordered map whose order
        # canonicalisation would erase.
        self.assertTrue(capture.order_safe_problems(
            {"ordered": [{"op": "entries", "result": {"a": 1, "b": 2}}]}))
        self.assertTrue(capture.order_safe_problems(
            {"ordered": [{"op": "entries", "result": [{"a": 1, "b": 2}]}]}))

    def test_order_safe_problems_accepts_entry_arrays_and_named_fields(self):
        # An element's own named fields are fine: their order carries no
        # information and canonicalisation sorts them the same on both sides.
        self.assertEqual(capture.order_safe_problems(
            {"ordered": [{"op": "set", "panic": ""}, {"op": "entries",
                                                      "result": [["a", 1], ["b", 2]]}]}), [])
        self.assertEqual(capture.order_safe_problems(
            {"ordered": [{"bytes": "{\"b\":1,\"a\":2}"}]}), [])

    def test_an_order_sensitive_observation_must_declare_its_ordered_payload(self):
        self.assertTrue(capture.order_safe_problems({"entries": [["a", 1]]}))
        self.assertTrue(capture.order_safe_problems({"ordered": {"a": 1}}))

    def test_an_order_sensitive_case_rejects_an_order_erasing_observation(self):
        requests = [{"case": "m", "operation": "collections.orderedMap",
                     "order_sensitive": True}]
        document = {"version": 1, "observations": [
            {"case": "m", "operation": "collections.orderedMap", "result": "observed",
             "observation": {"ordered": [{"op": "entries", "result": {"a": 1, "b": 2}}]}}]}
        with self.assertRaisesRegex(ValueError, "order-erasing representation"):
            capture.validate_response(document, requests, "rust")

    def test_an_order_sensitive_case_accepts_an_entry_array(self):
        requests = [{"case": "m", "operation": "collections.orderedMap",
                     "order_sensitive": True}]
        document = {"version": 1, "observations": [
            {"case": "m", "operation": "collections.orderedMap", "result": "observed",
             "observation": {"ordered": [{"op": "entries", "result": [["a", 1], ["b", 2]]}]}}]}
        self.assertEqual(len(capture.validate_response(document, requests, "rust")), 1)

    def test_a_pure_member_order_difference_is_reported_as_different(self):
        temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, temporary, ignore_errors=True)
        # Override the family inventory *before* building the capture, so the
        # recorded closure covers the same inputs the replay recomputes.
        real = dict(capture.FAMILIES)
        self.addCleanup(setattr, capture, "FAMILIES", real)
        inventory = Path(temporary) / "inv.json"
        inventory.write_text(json.dumps({"version": 1, "family": "pilot", "requests": [
            {"case": "a", "operation": "vfsmatch.readDirectory", "order_sensitive": True}]}))
        capture.FAMILIES = dict(capture.FAMILIES)
        capture.FAMILIES["pilot"] = dict(capture.FAMILIES["pilot"])
        capture.FAMILIES["pilot"]["requests"] = str(inventory)

        rows = [{"case": "a",
                 "native": {"ordered": [{"result": [["b", 1], ["a", 2]]}]},
                 "rust": {"result": "observed",
                          "observation": {"ordered": [{"result": [["a", 2], ["b", 1]]}]}}}]
        directory = Path(temporary) / "ordered"
        SyntheticCapture(directory, rows)
        report = capture.compare(directory)
        self.assertEqual(report["counts"]["different"], 1,
                         "a member-order difference must not canonicalise away")


class ProbeRegistryTests(unittest.TestCase):
    """A Go package may host more than one probe."""

    def test_every_declared_probe_has_a_unique_name(self):
        for family, spec in capture.FAMILIES.items():
            names = [p["name"] for p in spec["native_probes"]]
            self.assertEqual(len(names), len(set(names)), family)
            for probe in spec["native_probes"]:
                self.assertIn("package", probe, family)

    def test_probe_directories_are_named_per_probe_not_per_package(self):
        # Two probes in one package must not collide; the pilot's four probes
        # already exercise the keying.
        spec = capture.FAMILIES["pilot"]
        directories = [p["name"] for p in spec["native_probes"]]
        self.assertEqual(len(directories), len(set(directories)))

    def test_the_committed_capture_is_keyed_by_probe_name(self):
        installed = ROOT / "data/phase1/native/pilot"
        if not installed.is_dir():
            self.skipTest("no frozen pilot observations are committed")
        provenance = json.loads((installed / "capture-provenance.json").read_text())
        for name, probe in provenance["native_probes"].items():
            self.assertEqual(probe["directory"], f"native/{name}")
            self.assertIn("package", probe)

    def test_unknown_rust_target_kind_is_refused(self):
        real = dict(capture.FAMILIES)
        self.addCleanup(setattr, capture, "FAMILIES", real)
        capture.FAMILIES = dict(capture.FAMILIES)
        capture.FAMILIES["pilot"] = dict(capture.FAMILIES["pilot"], rust_target_kind="lib")
        with self.assertRaisesRegex(ValueError, "unknown rust_target_kind"):
            capture.build_rust("pilot")


class WitnessTests(unittest.TestCase):
    """An existing artifact only confers coverage when it actually runs Rust."""

    def setUp(self):
        self.cases = json.loads((ROOT / "data/phase1/cases.json").read_text())
        self.scope = json.loads((ROOT / "data/phase1/scope.json").read_text())

    def test_committed_witnesses_validate(self):
        self.assertEqual(scope.witness_problems(), [])

    def test_only_rust_gated_witnesses_confer_coverage(self):
        self.assertEqual(scope.COVERING_WITNESS_KINDS, ("rust_gated",))
        linked = scope.cases_by_operation()
        for witness in self.cases.get("witnesses", []):
            if witness["kind"] == "rust_gated":
                continue
            for operation in witness["operations"]:
                self.assertNotIn(
                    witness["id"], linked.get(operation, []),
                    f"{witness['kind']} witness {witness['id']} must not confer coverage",
                )

    def test_go_only_producers_are_recorded_as_native_authorities(self):
        """s07_path_helpers and s07_semver run `go test` and never execute Rust."""
        by_id = {w["id"]: w for w in self.cases.get("witnesses", [])}
        for identity in ("witness/s07-path-observations", "witness/s07-semver-observations"):
            self.assertIn(identity, by_id)
            self.assertEqual(by_id[identity]["kind"], "native_authority", identity)

    def test_a_rust_gated_witness_must_name_its_gate(self):
        forged = json.loads(json.dumps(self.cases))
        forged["witnesses"].append({
            "id": "witness/forged", "kind": "rust_gated",
            "artifact": "data/phase1/cases.json", "operations": [],
            "witnesses": "claims coverage with no gate",
        })
        path = ROOT / "data/phase1/cases.json"
        original = path.read_bytes()
        try:
            path.write_text(json.dumps(forged))
            problems = scope.witness_problems()
        finally:
            path.write_bytes(original)
        self.assertTrue(any("must name the producer command" in p for p in problems))

    def test_a_witness_pointing_at_a_missing_artifact_is_rejected(self):
        forged = json.loads(json.dumps(self.cases))
        forged["witnesses"].append({
            "id": "witness/absent", "kind": "native_authority",
            "artifact": "data/phase1/does-not-exist.json", "operations": [],
            "witnesses": "points nowhere",
        })
        path = ROOT / "data/phase1/cases.json"
        original = path.read_bytes()
        try:
            path.write_text(json.dumps(forged))
            problems = scope.witness_problems()
        finally:
            path.write_bytes(original)
        self.assertTrue(any("does not exist" in p for p in problems))

    def test_every_covered_operation_names_a_real_link(self):
        # A covered row lists every case touching the operation, which can
        # include a case that witnesses a different gap on the same symbol, so
        # the covering links are a subset rather than the whole list.
        linked = scope.cases_by_operation()
        for row in self.scope["operations"]:
            if row["disposition"] != "covered":
                continue
            self.assertTrue(row["cases"], row["id"])
            self.assertTrue(set(linked[row["id"]]).issubset(set(row["cases"])), row["id"])

    def test_a_not_implemented_case_never_confers_coverage(self):
        cases = json.loads((ROOT / "data/phase1/cases.json").read_text())["cases"]
        gap_only = {
            c["id"] for c in cases if c.get("last_result") == "not_implemented"
        }
        self.assertTrue(gap_only, "expected prepared cases reporting a missing Rust entry point")
        covering = set()
        for ids in scope.cases_by_operation().values():
            covering |= set(ids)
        self.assertEqual(covering & gap_only, set(),
                         "a case that reports not_implemented must not cover its operation")

    def test_a_case_with_no_recorded_result_does_not_confer_coverage(self):
        cases = json.loads((ROOT / "data/phase1/cases.json").read_text())["cases"]
        self.assertTrue(all("last_result" in c for c in cases),
                        "every committed case should carry its last comparison result")


class RequestFragmentTests(unittest.TestCase):
    """A family may be split into per-group fragments."""

    def test_a_string_requests_field_still_works(self):
        self.assertEqual(capture.request_files({"requests": "a.json"}), ["a.json"])

    def test_a_list_requests_field_is_returned_in_order(self):
        self.assertEqual(capture.request_files({"requests": ["a.json", "b.json"]}),
                         ["a.json", "b.json"])

    def test_duplicate_case_ids_across_fragments_are_refused(self):
        temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, temporary, ignore_errors=True)
        first, second = Path(temporary) / "one.json", Path(temporary) / "two.json"
        body = {"version": 1, "requests": [{"case": "x", "operation": "o"}]}
        first.write_text(json.dumps(body))
        second.write_text(json.dumps(body))
        with self.assertRaisesRegex(ValueError, "duplicate case id"):
            capture.load_requests({"requests": [str(first), str(second)]})

    def test_the_committed_leaves_fragments_have_no_duplicate_cases(self):
        merged = capture.load_requests(capture.FAMILIES["leaves"])
        cases = [r["case"] for r in merged["requests"]]
        self.assertEqual(len(cases), len(set(cases)))


class NegativeControlTests(unittest.TestCase):
    """Plan task 6: the comparator must reject each deliberate corruption.

    Each control builds a capture whose Rust side differs from the native side
    in exactly one way, and asserts the comparison reports `different` rather
    than `match`. A comparator that passes these cannot quietly accept the
    corresponding real regression.
    """

    def setUp(self):
        self.temporary = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.temporary, ignore_errors=True)
        self.real = dict(capture.FAMILIES)
        self.addCleanup(setattr, capture, "FAMILIES", self.real)

    def control(self, name, native, rust, order_sensitive=True):
        inventory = Path(self.temporary) / f"{name}-inv.json"
        inventory.write_text(json.dumps({"version": 1, "family": "pilot", "requests": [
            {"case": "a", "operation": "vfsmatch.readDirectory",
             "order_sensitive": order_sensitive}]}))
        capture.FAMILIES = dict(capture.FAMILIES)
        capture.FAMILIES["pilot"] = dict(capture.FAMILIES["pilot"])
        capture.FAMILIES["pilot"]["requests"] = str(inventory)
        directory = Path(self.temporary) / name
        SyntheticCapture(directory, [{"case": "a", "native": native,
                                      "rust": {"result": "observed", "observation": rust}}])
        return capture.compare(directory)

    def assert_different(self, report, what):
        self.assertEqual(report["counts"]["different"], 1, what)
        self.assertEqual(report["counts"]["match"], 0, what)

    def test_reordered_collection_output_is_rejected(self):
        report = self.control(
            "collection-order",
            {"ordered": [{"op": "entries", "result": [["a", "1"], ["b", "2"]]}]},
            {"ordered": [{"op": "entries", "result": [["b", "2"], ["a", "1"]]}]})
        self.assert_different(report, "a reordered ordered-map must not match")

    def test_reordered_json_members_are_rejected(self):
        report = self.control(
            "json-order",
            {"ordered": [{"op": "marshal", "bytes": '{"b":1,"a":2}'}]},
            {"ordered": [{"op": "marshal", "bytes": '{"a":2,"b":1}'}]})
        self.assert_different(report, "reordered JSON members must not match")

    def test_a_wrong_fallback_locale_is_rejected(self):
        report = self.control(
            "locale-fallback",
            {"ordered": [{"op": "select", "requested": "pt-BR", "selected": "pt-br"}]},
            {"ordered": [{"op": "select", "requested": "pt-BR", "selected": "pt"}]})
        self.assert_different(report, "a different fallback locale must not match")

    def test_byte_replacement_is_rejected(self):
        # A replacement-decoded string must never compare equal to the raw
        # bytes: repairing malformed input early is the regression this catches.
        report = self.control(
            "byte-replacement",
            {"ordered": [{"op": "read", "bytes_hex": "eda0bd"}]},
            {"ordered": [{"op": "read", "bytes_hex": "efbfbd"}]})
        self.assert_different(report, "replacement bytes must not match the original")

    def test_a_missing_translation_key_is_rejected(self):
        report = self.control(
            "missing-translation",
            {"ordered": [{"op": "localize", "key": "Cannot_find_name_0", "text": "Nome non trovato"}]},
            {"ordered": [{"op": "localize", "key": "Cannot_find_name_0", "text": "Cannot find name"}]})
        self.assert_different(report, "an untranslated fallback must not match a translation")

    def test_an_identical_observation_still_matches(self):
        # The controls above would be vacuous if the comparator reported
        # `different` for everything.
        payload = {"ordered": [{"op": "entries", "result": [["a", "1"]]}]}
        report = self.control("identical", payload, json.loads(json.dumps(payload)))
        self.assertEqual(report["counts"]["match"], 1)


if __name__ == "__main__":
    unittest.main()
