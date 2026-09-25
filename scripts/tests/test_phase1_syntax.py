"""Phase 1 F4a: the syntax request inventory, schedule and case-layer contracts.

The inventory reports zero problems against the committed manifests, which by
itself means nothing. Each test below perturbs one owning manifest the way a
real defect would and requires the validator to name it.
"""

import copy
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import phase1_syntax as syntax  # noqa: E402


class SyntaxInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sources = syntax.load_sources()
        cls.document = syntax.build(cls.sources)

    def perturbed(self):
        return copy.deepcopy(self.sources)

    def check(self, sources):
        return syntax.problems(self.document, sources=sources, check_pin=False)

    def test_the_committed_manifests_account_for_every_request(self):
        self.assertEqual(syntax.problems(), [])

    def test_the_counts_are_the_reviewed_denominators(self):
        counts = self.document["counts"]
        self.assertEqual(counts["physical_cases"], 12721)
        self.assertEqual(counts["libraries"], 108)
        self.assertEqual(counts["primary_requests"], 22343)
        self.assertEqual(counts["option_variants"], 15206)
        self.assertEqual(counts["supplemental_requests"], 135)

    def test_a_dropped_parser_request_is_named(self):
        sources = self.perturbed()
        dropped = sources["requests"].pop(1234)
        found = self.check(sources)
        self.assertTrue(any(dropped["primary"] in problem for problem in found), found)

    def test_a_request_removed_only_from_the_binder_set_is_named(self):
        # The plan: "No E2 eligibility filter may remove a parser request." The
        # binder set must be the parser set, so a filter applied on one side
        # shows up as a difference between them.
        sources = self.perturbed()
        sources["binder_requests"].pop(0)
        found = self.check(sources)
        self.assertTrue(any("binder request set differs" in problem for problem in found), found)

    def test_an_excluded_variant_that_lost_its_requests_is_named(self):
        sources = self.perturbed()
        excluded = next(variant for case in sources["subset"] for variant in case["variants"]
                        if variant["disposition"] == "excluded")
        primary, configuration = excluded["id"].rsplit("#configuration=", 1)
        sources["requests"] = [request for request in sources["requests"]
                               if not (request["primary"] == primary
                                       and request["configuration"] == int(configuration))]
        found = self.check(sources)
        self.assertTrue(any(excluded["id"] in problem or primary in problem for problem in found), found)

    def test_a_unit_excluded_for_an_unnamed_reason_is_named(self):
        sources = self.perturbed()
        case = next(case for case in sources["corpus"] if case.get("units"))
        case["units"][0]["eligibility"] = "quietly_skipped: no reason"
        found = self.check(sources)
        self.assertTrue(any("unnamed reason" in problem for problem in found), found)

    def test_an_invented_request_is_named(self):
        sources = self.perturbed()
        extra = copy.deepcopy(sources["requests"][0])
        extra["id"] = extra["id"] + "-invented"
        extra["configuration"] = 999
        sources["requests"].append(extra)
        sources["binder_requests"].append(copy.deepcopy(extra))
        found = self.check(sources)
        self.assertTrue(any("undeclared" in problem for problem in found), found)

    def test_a_missing_subset_variant_is_named(self):
        sources = self.perturbed()
        sources["subset"][0]["variants"].pop(0)
        found = self.check(sources)
        self.assertTrue(any("subset variants differ" in problem for problem in found), found)

    def test_a_supplemental_id_colliding_with_a_primary_is_named(self):
        sources = self.perturbed()
        sources["fixtures"][0]["request"]["id"] = sources["requests"][0]["id"]
        found = self.check(sources)
        self.assertTrue(any("collide" in problem for problem in found), found)

    def test_historical_evidence_is_linked_as_historical(self):
        for record in self.document["evidence"]:
            if record["state"] != "current":
                self.assertFalse(record["current"], record["producer"])

    def test_a_changed_source_manifest_invalidates_the_inventory(self):
        committed = json.loads(syntax.INVENTORY.read_text())
        committed = copy.deepcopy(committed)
        committed["sources"]["corpus"]["sha256"] = "0" * 64
        found = syntax.problems(committed, check_pin=False)
        self.assertTrue(any("changed" in problem for problem in found), found)


def _native_row(predicate):
    native = json.loads(syntax.NATIVE.read_text())
    return copy.deepcopy(next(row for row in native["rows"] if predicate(row)))


class SyntaxScheduleTests(unittest.TestCase):
    """Plan tasks 2 and 3: the committed schedule accounts for every variant and
    no row that could not run becomes an empty success."""

    @classmethod
    def setUpClass(cls):
        cls.schedule = json.loads(syntax.SCHEDULE.read_text())
        cls.native = json.loads(syntax.NATIVE.read_text())
        subset = json.loads((ROOT / "data/s07/subset.json").read_text())
        cls.subset = {variant["id"]: variant for case in subset["cases"] for variant in case["variants"]}
        cls.policy = json.loads((ROOT / "data/s07/e2-policy-observations.json").read_text())

    def check(self, schedule=None, native=None):
        return syntax._schedule_problems(schedule or self.schedule, native or self.native, self.subset, self.policy)

    def row(self, schedule, predicate):
        return next(row for row in schedule["rows"] if predicate(row))

    def test_the_committed_schedule_is_current_and_complete(self):
        self.assertEqual(syntax.schedule_problems(), [])
        counts = self.schedule["counts"]
        self.assertEqual(counts["variants"], 15206)
        self.assertEqual(counts["by_boundary"], {"content_mapper": 15, "options_rejected": 39})
        # The E2 filter excluded 4,478 of these variants; all of them are scheduled.
        self.assertEqual(counts["by_e2_disposition"], {"eligible": 10728, "excluded": 4478})

    def test_a_dropped_variant_is_named(self):
        schedule = copy.deepcopy(self.schedule)
        schedule["rows"].pop(100)
        self.assertTrue(any("differ from the S07 variants" in p for p in self.check(schedule)))

    def test_a_boundary_row_cannot_become_an_empty_success(self):
        schedule = copy.deepcopy(self.schedule)
        row = self.row(schedule, lambda r: r["boundary"] == "content_mapper")
        row.update(load="loaded", syntactic="observed")
        found = self.check(schedule)
        self.assertTrue(any(row["id"] in p and "without a native row" in p for p in found), found)
        self.assertTrue(any(row["id"] in p and "only a named boundary" in p for p in found), found)

    def test_a_rejected_variant_keeps_its_native_option_diagnostics(self):
        schedule = copy.deepcopy(self.schedule)
        row = self.row(schedule, lambda r: r["boundary"] == "options_rejected")
        row["option_diagnostics"] = []
        self.assertTrue(any(row["id"] in p and "option diagnostics" in p for p in self.check(schedule)))

    def test_a_rejected_variant_cannot_claim_to_have_passed_the_guard(self):
        schedule = copy.deepcopy(self.schedule)
        # Every rejected variant is also a filename skip: testrunner.skippedTests
        # lists exactly the sources whose settings the pin no longer accepts.
        self.assertTrue(all(r["filename_skip"] for r in schedule["rows"] if r["boundary"] == "options_rejected"))
        row = self.row(schedule, lambda r: r["boundary"] == "options_rejected")
        row["option_guard"] = "allowed"
        self.assertTrue(any(row["id"] in p and "only a rejected variant" in p for p in self.check(schedule)))

    def test_a_missing_native_observation_is_named(self):
        native = copy.deepcopy(self.native)
        dropped = native["rows"].pop(7)
        found = self.check(native=native)
        self.assertTrue(any(dropped["id"] in p for p in found), found)

    def test_a_guard_outcome_that_contradicts_the_e2_policy_is_named(self):
        schedule = copy.deepcopy(self.schedule)
        row = self.row(schedule, lambda r: r["e2_disposition"] == "eligible" and r["option_guard"] == "skipped")
        row.update(option_guard="allowed", native_selection="runs")
        self.assertTrue(any(row["id"] in p and "E2 policy" in p for p in self.check(schedule)))

    def test_the_phase_is_named_as_syntactic_only(self):
        schedule = copy.deepcopy(self.schedule)
        schedule["phase"] = "whole error baseline"
        self.assertTrue(any("names its phase" in p for p in self.check(schedule)))


class SyntaxScheduleRequestControls(unittest.TestCase):
    def test_native_closure_includes_preprocessing_and_executed_helpers(self):
        import s07_subset

        required = {"scripts/tracking-bootstrap.py", "scripts/s04_runtime.py", "scripts/s06_build.py",
                    "scripts/s06_oracle/export_boundaries_test.go", "scripts/s06_oracle/fixture_export_test.go",
                    "tools/s07/subset/export_test.go", "tools/s07/subset/options_bridge.go", ".gitmodules"}
        self.assertLessEqual(required, set(s07_subset.PRODUCER_INPUTS))
        required.update(s07_subset.PRODUCER_INPUTS)
        required.update({"scripts/s06_corpus.py", "scripts/s06_protocol.py", "data/s06/corpus.json"})
        self.assertLessEqual(required, set(syntax.schedule_inputs()))

    def test_wire_requests_preserve_the_frozen_paths_order(self):
        from s07_subset import json_bytes, sha256

        request = {"id": "ordered", "options": {"paths": {"foo": ["first"], "bar": ["second"]}}}
        probes = [{"id": "ordered", "request": request, "guard": True, "load": True}]
        decoded = json.loads(syntax.schedule_request_bytes(probes))[0]["request"]
        self.assertEqual(list(decoded["options"]["paths"]), ["foo", "bar"])
        self.assertEqual(sha256(json_bytes(decoded)), sha256(json_bytes(request)))

    def test_unexpected_native_panic_invalidates_the_capture(self):
        import phase1_syntax_schedule

        with self.assertRaisesRegex(ValueError, "unexpected native syntax panic"):
            phase1_syntax_schedule.join(
                [{"id": "panic"}], [{"id": "panic", "guard": True, "load": True}],
                {"rows": [{"id": "panic", "option_guard": "allowed", "load": "panic", "panic": "boom"}]})


class SyntaxComparatorControls(unittest.TestCase):
    """Plan task 8: each control perturbs a real observation the way a defect
    would and requires the comparator to fail."""

    def setUp(self):
        self.native = _native_row(lambda r: len({d["File"] for d in r.get("syntactic") or []}) > 1)

    def rust(self, **changes):
        row = {key: copy.deepcopy(self.native[key]) for key in syntax.COMPARED}
        row.update(state="observed", id=self.native["id"], **changes)
        return row

    def result(self, rust):
        return syntax.compare_row(self.native, rust)

    def test_the_unperturbed_row_matches(self):
        self.assertEqual(self.result(self.rust())["result"], "match")

    def test_reordered_diagnostics_fail(self):
        diagnostics = copy.deepcopy(self.native["syntactic"])
        diagnostics[0], diagnostics[-1] = diagnostics[-1], diagnostics[0]
        self.assertEqual(self.result(self.rust(syntactic=diagnostics))["result"], "different")

    def test_a_syntactic_adapter_appending_a_semantic_diagnostic_fails(self):
        diagnostics = copy.deepcopy(self.native["syntactic"])
        semantic = dict(diagnostics[0], Code=2304, Key="Cannot_find_name_0_2304", Args=["x"],
                        Text="Cannot find name 'x'.")
        self.assertEqual(self.result(self.rust(syntactic=[*diagnostics, semantic]))["result"], "different")

    def test_a_diagnostic_on_the_wrong_owner_fails(self):
        diagnostics = copy.deepcopy(self.native["syntactic"])
        others = sorted({d["File"] for d in diagnostics} - {diagnostics[0]["File"]})
        diagnostics[0]["File"] = others[0]
        self.assertEqual(self.result(self.rust(syntactic=diagnostics))["result"], "different")

    def test_a_changed_range_or_argument_fails(self):
        for field, value in (("Pos", self.native["syntactic"][0]["Pos"] + 1), ("Args", ["changed"])):
            diagnostics = copy.deepcopy(self.native["syntactic"])
            diagnostics[0][field] = value
            self.assertEqual(self.result(self.rust(syntactic=diagnostics))["result"], "different", field)

    def test_a_rendering_change_alone_fails(self):
        pretty = self.native["pretty_hex"].replace("0d0a", "0a")
        self.assertNotEqual(pretty, self.native["pretty_hex"])
        self.assertEqual(self.result(self.rust(pretty_hex=pretty))["differs"], ["pretty_hex"])

    def test_a_different_program_fails(self):
        self.assertEqual(self.result(self.rust(file_names_sha256="0" * 64))["differs"], ["file_names_sha256"])

    def test_a_rust_load_error_is_a_difference_not_a_skip(self):
        outcome = self.result({"id": self.native["id"], "state": "load_error", "error": "boom"})
        self.assertEqual(outcome["result"], "different")

    def test_unexpected_failures_invalidate_the_smoke(self):
        for state in ("panic", "error", "unknown", None):
            with self.subTest(state=state), self.assertRaisesRegex(ValueError, "harness failed"):
                self.result({"id": self.native["id"], "state": state, "panic": "boom", "error": "boom"})

    def test_malformed_observed_rows_cannot_be_semantic_differences(self):
        for field, value in (("files", None), ("syntactic", None), ("pretty_hex", "invalid")):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "malformed"):
                self.result(self.rust(**{field: value}))

    def test_a_missing_entry_point_must_be_named(self):
        with self.assertRaisesRegex(ValueError, "lacks its operation"):
            self.result({"id": self.native["id"], "state": "not_implemented"})

    def test_a_named_unsupported_branch_is_not_implemented(self):
        outcome = self.result({"id": self.native["id"], "state": "not_implemented", "operation": "x"})
        self.assertEqual(outcome, {"id": self.native["id"], "result": "not_implemented", "operation": "x"})


class SyntaxReportFreshnessTests(unittest.TestCase):
    """A committed report is present when its smoke says it exists, and a moved
    Rust closure is an outstanding item, not a silently current report."""

    def with_reports(self, smoke, full=None):
        directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory, True)
        smoke_path, full_path = directory / "syntax-smoke.json", directory / "syntax-full.json"
        smoke_path.write_text(json.dumps(smoke))
        if full is not None:
            full_path.write_text(json.dumps(full))
        for name, value in (("SMOKE", smoke_path), ("FULL", full_path)):
            saver = patch.object(syntax, name, value)
            saver.start()
            self.addCleanup(saver.stop)

    def test_a_smoke_derived_from_a_full_capture_requires_the_full_report(self):
        self.with_reports({"capture_selection": "full", "selection": "smoke"})
        self.assertTrue(any("is absent" in problem for problem in syntax.full_problems()))

    def test_a_bounded_smoke_does_not_invent_a_full_report(self):
        self.with_reports({"capture_selection": "smoke", "selection": "smoke"})
        self.assertEqual(syntax.full_problems(), [])

    def test_a_stale_recorded_rust_closure_is_an_outstanding_item(self):
        current = {"crates/tsr_parser/src/lib.rs": "1" * 64, "Cargo.lock": "2" * 64}
        stale = {**current, "Cargo.lock": "3" * 64}
        self.with_reports({"capture_selection": "full", "rust_closure": current, "rust_sources_current": True},
                          {"selection": "full", "rust_closure": stale, "rust_sources_current": True})
        with patch.object(syntax, "rust_closure", return_value=current):
            items = syntax.freshness_items()
            self.assertTrue(syntax.smoke_freshness()["rust_sources_current"])
            self.assertEqual(syntax.full_freshness()["changed"], ["Cargo.lock"])
        self.assertEqual(len(items), 1)
        self.assertIn("syntax-full.json", items[0])
        self.assertIn("Cargo.lock", items[0])


class SyntaxSmokeTests(unittest.TestCase):
    """Plan task 9, corpus half: the committed smoke is its rule's selection and
    its counts follow from its rows."""

    def setUp(self):
        self.report = json.loads(syntax.SMOKE.read_text())

    def problems_with(self, report):
        path = ROOT / "target/phase1/test-syntax-smoke.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(report))
        saved = syntax.SMOKE
        syntax.SMOKE = path
        try:
            return syntax.smoke_problems()
        finally:
            syntax.SMOKE = saved
            path.unlink()

    def test_the_committed_smoke_is_consistent(self):
        self.assertEqual(syntax.smoke_problems(), [])
        self.assertEqual(self.report["counts"]["not_run"] + len(self.report["selected"]),
                         sum(1 for row in json.loads(syntax.SCHEDULE.read_text())["rows"] if row["load"] == "loaded"))

    def test_the_selection_includes_every_diagnosed_row(self):
        native = json.loads(syntax.NATIVE.read_text())
        diagnosed = {row["id"] for row in native["rows"] if row.get("syntactic")}
        self.assertLessEqual(diagnosed, set(self.report["selected"]))

    def test_a_trimmed_selection_is_named(self):
        report = copy.deepcopy(self.report)
        report["selected"].pop()
        report["rows"].pop()
        self.assertTrue(any("selection" in p for p in self.problems_with(report)))

    def test_counts_that_do_not_follow_from_rows_are_named(self):
        report = copy.deepcopy(self.report)
        report["rows"][0]["result"] = "different"
        self.assertTrue(any("counts" in p for p in self.problems_with(report)))

    def test_a_smoke_against_another_native_observation_is_named(self):
        report = copy.deepcopy(self.report)
        report["native_rows_sha256"] = "0" * 64
        self.assertTrue(any("different native" in p for p in self.problems_with(report)))

    def test_replay_refuses_reordered_rust_rows(self):
        import tempfile

        native = json.loads(syntax.NATIVE.read_text())
        schedule = json.loads(syntax.SCHEDULE.read_text())
        selected = self.report["selected"][:3]
        by_id = {row["id"]: row for row in native["rows"]}
        rows = [{"id": rid, "state": "observed", **{k: by_id[rid][k] for k in syntax.COMPARED}} for rid in selected]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rust-rows.jsonl"
            path.write_text("".join(json.dumps(row) + "\n" for row in [rows[1], rows[0], rows[2]]))
            with self.assertRaisesRegex(ValueError, "reordered"):
                syntax._smoke_report(Path(directory), selected, native, schedule)
            path.write_text("".join(json.dumps(row) + "\n" for row in rows))
            report = syntax._smoke_report(Path(directory), selected, native, schedule)
            self.assertEqual({k: v for k, v in report["counts"].items() if k != "not_run"}, {"match": 3})


class SyntaxReplayControls(unittest.TestCase):
    def setUp(self):
        from s07_subset import json_bytes, sha256

        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.request = {"id": "case", "options": {}, "files": {}, "roots": []}
        self.schedule = {"rows": [{"id": "case", "load": "loaded", "request_shape": [],
                                   "native_selection": "runs",
                                   "loading_request_sha256": sha256(json_bytes(self.request))}]}
        self.native = {"rows": [{"id": "case", "files": 0, "file_names_sha256": "0" * 64,
                                 "syntactic": [], "plain_hex": "", "pretty_hex": ""}]}
        schedule_path = self.directory / "schedule.json"
        native_path = self.directory / "native.json"
        schedule_path.write_bytes(syntax._json_canonical(self.schedule))
        native_path.write_bytes(syntax._json_canonical(self.native))
        for name, value in (("SCHEDULE", schedule_path), ("NATIVE", native_path)):
            replacement = patch.object(syntax, name, value)
            replacement.start()
            self.addCleanup(replacement.stop)
        replacement = patch.object(syntax, "rust_closure", return_value={".cargo/config.toml": "current"})
        replacement.start()
        self.addCleanup(replacement.stop)
        self.probes = [{"id": "case", "request": self.request, "load": True, "guard": True}]
        self.rust = [{**self.native["rows"][0], "state": "observed"}]
        self.provenance = {"schedule_sha256": sha256(schedule_path.read_bytes()),
                           "native_rows_sha256": syntax.native_rows_digest(self.native),
                           "rust_binary_sha256": "unused", "selected": ["case"],
                           "rust_closure": {".cargo/config.toml": "current"}}
        self.write_capture()

    def write_capture(self):
        from s07_subset import sha256

        requests = syntax.schedule_request_bytes(self.probes)
        rows = b"".join(syntax._json_canonical(row) for row in self.rust)
        (self.directory / "requests.json").write_bytes(requests)
        (self.directory / "rust-rows.jsonl").write_bytes(rows)
        self.provenance.update(requests_sha256=sha256(requests), rust_rows_sha256=sha256(rows))
        (self.directory / "provenance.json").write_bytes(syntax._json_canonical(self.provenance))

    def test_valid_capture_replays_without_children(self):
        with patch("s04_common.command", side_effect=AssertionError("replay ran a child")):
            self.assertEqual(syntax.replay(self.directory)["counts"], {"match": 1, "not_run": 0})

    def test_a_changed_schedule_is_refused(self):
        syntax.SCHEDULE.write_text(syntax.SCHEDULE.read_text() + "\n")
        with self.assertRaisesRegex(ValueError, "schedule changed"):
            syntax.replay(self.directory)

    def test_a_self_consistent_but_partial_capture_is_refused(self):
        self.provenance["selected"] = []
        self.probes, self.rust = [], []
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "rule's selection"):
            syntax.replay(self.directory)

    def test_request_payload_must_match_the_frozen_loading_request(self):
        self.request["options"]["checkJs"] = True
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "frozen loading request"):
            syntax.replay(self.directory)

    def test_an_omitted_source_input_is_not_current(self):
        self.provenance["rust_closure"] = {}
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "syntax capture sources changed"):
            syntax.replay(self.directory)

    def test_omitting_a_whole_parser_package_is_not_current(self):
        current = {"Cargo.toml": "workspace", "tools/phase1/syntax/Cargo.toml": "driver",
                   "crates/tsr_parser/Cargo.toml": "parser", "crates/tsr_parser/src/lib.rs": "code"}
        self.provenance["rust_closure"] = {
            name: digest for name, digest in current.items() if not name.startswith("crates/tsr_parser/")}
        self.write_capture()
        with patch.object(syntax, "rust_closure", return_value=current):
            with self.assertRaisesRegex(ValueError, "syntax capture sources changed"):
                syntax.replay(self.directory)

    def test_rust_panic_cannot_produce_a_report(self):
        self.rust = [{"id": "case", "state": "panic", "panic": "boom"}]
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "harness failed"):
            syntax.replay(self.directory)

    def full_capture(self):
        from s07_subset import json_bytes, sha256

        second = {**self.request, "id": "case-two"}
        self.schedule["rows"].append({**self.schedule["rows"][0], "id": second["id"],
                                      "loading_request_sha256": sha256(json_bytes(second))})
        self.native["rows"].append({**self.native["rows"][0], "id": second["id"]})
        self.probes.append({"id": second["id"], "request": second, "load": True, "guard": True})
        self.rust.append({**self.native["rows"][-1], "state": "observed"})
        syntax.SCHEDULE.write_bytes(syntax._json_canonical(self.schedule))
        syntax.NATIVE.write_bytes(syntax._json_canonical(self.native))
        self.provenance.update(selection="full", rule=syntax.FULL_RULE, selected=["case", "case-two"],
                               schedule_sha256=sha256(syntax.SCHEDULE.read_bytes()),
                               native_rows_sha256=syntax.native_rows_digest(self.native))
        self.write_capture()

    def test_full_capture_requires_every_loaded_row(self):
        self.full_capture()
        report = syntax.replay(self.directory)
        self.assertEqual(report["counts"], {"match": 2, "not_run": 0})
        self.assertEqual(report["selection"], "full")
        self.assertEqual(report["rule"], syntax.FULL_RULE)
        self.provenance["selected"].pop()
        self.probes.pop()
        self.rust.pop()
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "rule's selection"):
            syntax.replay(self.directory)

    def test_smoke_can_be_derived_from_full_outputs_without_children(self):
        self.full_capture()
        with patch("s04_common.command", side_effect=AssertionError("child")):
            report = syntax.smoke_from_full(self.directory)
        self.assertEqual(report["selected"], ["case"])
        self.assertEqual(report["counts"], {"match": 1, "not_run": 1})
        self.assertEqual((report["selection"], report["capture_selection"]), ("smoke", "full"))
        self.assertEqual(report["rule"], syntax.SMOKE_RULE)

    def test_deriving_smoke_does_not_hide_a_failure_outside_the_smoke(self):
        self.full_capture()
        self.rust[-1] = {"id": "case-two", "state": "panic", "panic": "outside smoke"}
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "harness failed"):
            syntax.smoke_from_full(self.directory)

    def test_full_replay_checks_unselected_smoke_request_payloads(self):
        self.full_capture()
        self.probes[-1]["request"] = {**self.probes[-1]["request"], "options": {"checkJs": True}}
        self.write_capture()
        with self.assertRaisesRegex(ValueError, "frozen loading request"):
            syntax.smoke_from_full(self.directory)

    def test_a_smoke_capture_cannot_pose_as_the_full_capture(self):
        with self.assertRaisesRegex(ValueError, "requires a full"):
            syntax.smoke_from_full(self.directory)

    def test_full_and_smoke_reports_have_separate_validation(self):
        self.full_capture()
        path = self.directory / "full-report.json"
        path.write_bytes(syntax._json_canonical(syntax.replay(self.directory)))
        self.assertEqual(syntax._report_problems(path, full=True), [])
        self.assertTrue(any("selection" in problem for problem in syntax._report_problems(path)))

    def test_full_cli_uses_the_same_capture_function(self):
        with patch.object(syntax, "smoke", return_value={"match": 2}) as run, patch("builtins.print"):
            self.assertEqual(syntax.main(["smoke", "--full", "--write", "--output", str(self.directory)]), 0)
        run.assert_called_once_with(self.directory, write_committed=True, full=True)


class SyntaxSourceClosureControls(unittest.TestCase):
    def test_cargo_config_and_new_package_files_are_included_without_cargo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text("[workspace]\n")
            (root / ".cargo").mkdir()
            (root / ".cargo/config.toml").write_text("[build]\n")
            (root / "tools/phase1/syntax").mkdir(parents=True)
            (root / "tools/phase1/syntax/Cargo.toml").write_text("[package]\n")
            with patch.object(syntax, "ROOT", root), patch("s04_common.command", side_effect=AssertionError("child")):
                before = syntax.rust_closure()
                self.assertIn(".cargo/config.toml", before)
                (root / "tools/phase1/syntax/new.rs").write_text("// new build input\n")
                self.assertEqual(set(syntax.rust_closure()) - set(before), {"tools/phase1/syntax/new.rs"})

    def test_finder_metadata_is_not_a_rust_source_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text("[workspace]\n")
            (root / ".cargo").mkdir()
            (root / ".cargo/config.toml").write_text("[build]\n")
            source = root / "tools/phase1/syntax/src/main.rs"
            source.parent.mkdir(parents=True)
            (root / "tools/phase1/syntax/Cargo.toml").write_text("[package]\n")
            source.write_text("fn main() {}\n")
            with patch.object(syntax, "ROOT", root), patch("s04_common.command", side_effect=AssertionError("child")):
                before = syntax.rust_closure()
                for parent in (root / ".cargo", root / "tools/phase1/syntax", source.parent):
                    (parent / ".DS_Store").write_bytes(b"Finder view settings")
                self.assertEqual(before, syntax.rust_closure())
                source.write_text("fn main() { panic!() }\n")
                self.assertNotEqual(before, syntax.rust_closure())

    def test_parser_dependency_is_found_without_any_recorded_package_list(self):
        with patch("s04_common.command", side_effect=AssertionError("child")):
            paths = syntax.rust_closure()
        self.assertIn("crates/tsr_parser/Cargo.toml", paths)
        self.assertIn("crates/tsr_parser/src/lib.rs", paths)

    def test_manifest_graph_includes_target_build_and_inherited_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace.dependencies]\ninherited = { path = "inherited" }\n')
            manifests = {
                "tools/phase1/syntax": ('[dependencies]\ninherited.workspace = true\n'
                                        '[build-dependencies]\nhelper = { path = "../../../helper" }\n'
                                        '[target.\'cfg(unix)\'.dev-dependencies]\n'
                                        'target = { path = "../../../target-dependency" }\n'),
                "inherited": '[package]\nname = "inherited"\n',
                "helper": '[package]\nname = "helper"\n',
                "target-dependency": '[package]\nname = "target"\n',
            }
            for name, content in manifests.items():
                (root / name).mkdir(parents=True)
                (root / name / "Cargo.toml").write_text(content)
            with patch.object(syntax, "ROOT", root):
                self.assertEqual({str(path.relative_to(root.resolve()))
                                  for path in syntax.rust_dependency_directories()}, set(manifests))


class SyntaxOperationCoverageControls(unittest.TestCase):
    def setUp(self):
        import phase1_capture

        self.capture = phase1_capture
        self.request = {"case": "syntax/control", "operation": "navigator", "file": {"source": "a"},
                        "actions": [{"op": "token_at"}, {"op": "next_in_file"}]}
        self.case = {"id": self.request["case"], "family": "syntax", "last_result": "match",
                     "operations": ["GetTokenAtPosition", "FindNextToken"],
                     "operation_actions": {"token_at": ["GetTokenAtPosition"], "next_in_file": ["FindNextToken"]}}
        self.cases = {"cases": [self.case]}
        self.review_signature()

    def review_signature(self):
        self.case["request_sha256"] = self.capture.digest(self.capture.request_bytes(self.request))

    def problems(self):
        return self.capture.operation_coverage_problems([self.request], self.cases)

    def test_reviewed_links_match_the_request(self):
        self.assertEqual(self.problems(), [])

    def test_committed_syntax_operation_links_match_their_requests(self):
        requests = self.capture.load_requests(self.capture.FAMILIES["syntax"])["requests"]
        self.assertEqual(self.capture.operation_coverage_problems(requests), [])

    def test_removing_an_action_cannot_retain_its_operation_credit(self):
        self.request["actions"].pop()
        self.review_signature()
        self.assertTrue(any("requested actions" in problem for problem in self.problems()))

    def test_removing_a_link_cannot_retain_its_operation_credit(self):
        self.case["operation_actions"]["next_in_file"] = []
        self.assertTrue(any("claimed operations" in problem for problem in self.problems()))

    def test_changed_source_requires_review_of_the_discriminator(self):
        self.request["file"]["source"] = ""
        self.assertTrue(any("request changed" in problem for problem in self.problems()))

    def test_missing_review_signature_is_rejected(self):
        del self.case["request_sha256"]
        self.assertTrue(any("request changed" in problem for problem in self.problems()))

    def test_uncredited_actions_are_explicit_and_allowed(self):
        self.case["operation_actions"]["next_in_file"] = []
        self.case["operations"].remove("FindNextToken")
        self.assertEqual(self.problems(), [])

    def test_actionless_requests_bind_their_mode_and_payload(self):
        del self.request["actions"]
        self.request.update(operation="evaluator", mode="literal")
        self.case["operation_actions"] = {"evaluator": self.case["operations"]}
        self.review_signature()
        self.assertEqual(self.problems(), [])
        self.request["mode"] = "callback"
        self.assertTrue(any("request changed" in problem for problem in self.problems()))

    def test_capture_rejects_stale_coverage_before_running_children(self):
        self.request["actions"].pop()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data/phase1").mkdir(parents=True)
            (root / "data/phase1/cases.json").write_text(json.dumps(self.cases))
            with (patch.object(self.capture, "ROOT", root),
                  patch.object(self.capture, "load_requests", return_value={"requests": [self.request]}),
                  patch.object(self.capture, "source_closure", side_effect=AssertionError("child preflight"))):
                with self.assertRaisesRegex(ValueError, "invalid syntax operation coverage"):
                    self.capture.capture("syntax", root / "capture")

    def test_replay_rejects_stale_coverage_before_accepting_observations(self):
        self.request["actions"].pop()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data/phase1").mkdir(parents=True)
            (root / "data/phase1/cases.json").write_text(json.dumps(self.cases))
            (root / "requests.json").write_text(json.dumps({"requests": [self.request]}))
            with (patch.object(self.capture, "ROOT", root),
                  patch.object(self.capture, "_authenticate", return_value={"family": "syntax"}),
                  patch.object(self.capture, "_merge_native", side_effect=AssertionError("accepted rows"))):
                with self.assertRaisesRegex(ValueError, "invalid syntax operation coverage"):
                    self.capture.validate_capture(root)


class _Replay:
    """A binder child replayed from recorded frames, through the real stream validator."""

    def __init__(self, records):
        self.records = records

    def observations(self, request):
        import s07_binder

        state = s07_binder.Stream(request)
        for record in self.records:
            accepted = state.accept(copy.deepcopy(record))
            if accepted is not None:
                yield accepted


class BinderComparatorControls(unittest.TestCase):
    """Plan task 8 for the identity-aware parser/binder comparator.

    The fixture is one real stream both runtimes emitted identically. Each
    control makes the SAME change in every graph stage it belongs to -- a
    bound-graph change alone would be caught earlier, by the repeat-bind digest,
    and would not reach the comparator at all.
    """

    @classmethod
    def setUpClass(cls):
        cls.fixture = json.loads((ROOT / "data/phase1/syntax-comparator-fixture.json").read_text())
        cls.request = cls.fixture["request"]
        cls.source = bytes.fromhex(cls.request["source_hex"]) if "source_hex" in cls.request else None

    def records(self):
        return copy.deepcopy(self.fixture["records"])

    def compare(self, rust_records):
        import s07_binder

        return s07_binder.compare(self.request, _Replay(self.fixture["records"]), _Replay(rust_records))

    def edit(self, records, kind, predicate, change, stages=("parsed_graph", "bound_graph", "repeated_graph")):
        touched = 0
        for record in records:
            if record.get("kind") == kind and record.get("stage") in stages and predicate(record["value"]):
                change(record["value"])
                touched += 1
        self.assertGreater(touched, 0)
        return records

    def assert_fails_at(self, records, suffix):
        result = self.compare(records)
        self.assertFalse(result["equal"])
        self.assertTrue(result["first_difference"]["path"].endswith(suffix), result["first_difference"])

    def test_the_recorded_stream_compares_equal(self):
        self.assertTrue(self.fixture["captured_equal"])
        self.assertTrue(self.compare(self.records())["equal"])

    def test_a_changed_parent_edge_fails(self):
        records = self.edit(self.records(), "node", lambda v: v["id"] == 5, lambda v: v.update(parent=1))
        self.assert_fails_at(records, ".parent")

    def test_a_changed_flow_edge_fails(self):
        records = self.records()
        flows = sorted({r["value"]["id"] for r in records if r.get("kind") == "flow" and r["stage"] == "bound_graph"})
        target = next(r["value"] for r in records if r.get("kind") == "flow" and r["stage"] == "bound_graph"
                      and r["value"]["antecedent"])
        other = next(flow for flow in flows if flow not in (target["id"], target["antecedent"]))
        records = self.edit(records, "flow", lambda v: v["id"] == target["id"], lambda v: v.update(antecedent=other),
                            stages=("bound_graph", "repeated_graph"))
        self.assert_fails_at(records, ".antecedent")

    def test_a_lost_trailing_comma_fails(self):
        # `function f(a, b,)`: the parameter list is list 2 and ends after the
        # comma, past its last node. NodeList.HasTrailingComma is exactly
        # `last.End() < list.End()` (ast/ast.go:139-145), so moving the list end
        # onto the last parameter's end is the lost comma.
        records = self.records()
        parameters = next(r["value"] for r in records if r.get("kind") == "list" and r["stage"] == "parsed_graph"
                          and r["value"]["id"] == 2)
        last_parameter_end = parameters["end"] - 1
        self.assertEqual(self.request["filename"], "/phase1/controls.ts")
        records = self.edit(records, "list", lambda v: v["id"] == 2, lambda v: v.update(end=last_parameter_end))
        self.assert_fails_at(records, ".end")

    def test_a_wrong_symbol_identity_fails(self):
        records = self.records()
        node = next(r["value"] for r in records if r.get("kind") == "node" and r["stage"] == "bound_graph"
                    and r["value"]["symbol"])
        symbols = sorted({r["value"]["id"] for r in records if r.get("kind") == "symbol" and r["stage"] == "bound_graph"})
        other = next(symbol for symbol in symbols if symbol != node["symbol"])
        records = self.edit(records, "node", lambda v: v["id"] == node["id"], lambda v: v.update(symbol=other),
                            stages=("bound_graph", "repeated_graph"))
        self.assert_fails_at(records, ".symbol")

    def test_reordered_diagnostics_fail(self):
        # Swap the two parse diagnostics' payloads under their original
        # [collection, index] labels: a relabelled reorder is refused earlier,
        # by the stream validator (next test), so this is the reorder that
        # reaches the comparator.
        def swap(value):
            first, second = value["diagnostics"][0], value["diagnostics"][1]
            self.assertEqual((first[0], second[0]), ("parse", "parse"))
            first[2], second[2] = second[2], first[2]
        records = self.edit(self.records(), "source", lambda v: len(v["diagnostics"]) >= 2, swap)
        result = self.compare(records)
        self.assertFalse(result["equal"])
        self.assertIn("diagnostics", result["first_difference"]["path"])

    def test_a_relabelled_diagnostic_reorder_is_refused_by_the_stream(self):
        def swap(value):
            value["diagnostics"][0], value["diagnostics"][1] = value["diagnostics"][1], value["diagnostics"][0]
        records = self.edit(self.records(), "source", lambda v: len(v["diagnostics"]) >= 2, swap)
        with self.assertRaisesRegex(ValueError, "reordered or duplicate diagnostic"):
            self.compare(records)

    def test_an_appended_diagnostic_fails(self):
        def append(value):
            extra = copy.deepcopy(value["diagnostics"][-1])
            extra[1] += 1
            extra[2].update(code=2304, pos=0, end=1)
            value["diagnostics"].append(extra)
        records = self.edit(self.records(), "source", lambda v: bool(v["diagnostics"]), append)
        result = self.compare(records)
        self.assertFalse(result["equal"])
        self.assertIn("diagnostics", result["first_difference"]["path"])


if __name__ == "__main__":
    unittest.main()
