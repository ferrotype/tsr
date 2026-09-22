"""Phase 1 F4a: the syntax request inventory, schedule and case-layer contracts.

The inventory reports zero problems against the committed manifests, which by
itself means nothing. Each test below perturbs one owning manifest the way a
real defect would and requires the validator to name it.
"""

import copy
import json
import sys
import unittest
from pathlib import Path

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

    def test_a_rust_load_failure_is_a_difference_not_a_skip(self):
        for state in ("panic", "load_error", "error"):
            outcome = self.result({"id": self.native["id"], "state": state, "panic": "boom", "error": "boom"})
            self.assertEqual(outcome["result"], "different", state)

    def test_a_named_unsupported_branch_is_not_implemented(self):
        outcome = self.result({"id": self.native["id"], "state": "not_implemented", "operation": "x"})
        self.assertEqual(outcome, {"id": self.native["id"], "result": "not_implemented", "operation": "x"})


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
