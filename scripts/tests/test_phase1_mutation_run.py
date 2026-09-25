"""Phase 1 mutation witnesses, Rust side: candidates, supervision, kill crediting, results, confirmation."""
import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_mutation_go as go  # noqa: E402
import phase1_mutation_run as run  # noqa: E402
from s06_protocol import canonical  # noqa: E402

E1 = list(go.ORACLES["e1"].operations)

# A stand-in for the oracle binaries that speaks their trace and kill protocols.
# Mutant behaviour: 1 and 5 differ on every row (5's base recheck fails),
# 2 hangs, 3 exits, 4 panics on every row, 11 hangs on r2 only and differs
# elsewhere, 12 and 14 differ on every row in `parse` (12's control 13
# reproduces the difference, 14's control 15 does not; with FAKE_CONTROL_SHIFT
# set, 15 equals 14 in `parse` and differs from it only in node_index_before),
# anything else survives. FAKE_HITS and FAKE_OBSERVE give each row's trace
# hits and observe_hits.
FAKE_DRIVER = r'''#!/usr/bin/env python3
import hashlib, json, os, sys, time
sys.path.insert(0, %(scripts)r)
from s06_protocol import canonical
STAGES = %(stages)r
HITS = json.loads(os.environ.get("FAKE_HITS", "{}"))
OBSERVE = json.loads(os.environ.get("FAKE_OBSERVE", "{}"))
SHIFT = bool(os.environ.get("FAKE_CONTROL_SHIFT"))
DIFFERING = (1, 5, 11, 12, 14)

def emit(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def frames(row, salt=""):
    yield {"tag": "begin", "op": "parse", "id": row, "version": 1}
    for stage in STAGES:
        yield {"tag": "observation", "seq": 0, "stage": stage, "kind": "n",
               "value": {"row": row, "stage": stage, "salt": salt}, "id": row, "version": 1}
        yield {"tag": "stage", "stage": stage, "outcome": "ok", "message_hex": "", "id": row, "version": 1}
    yield {"tag": "end", "observations": 4, "stages": 4, "id": row, "version": 1}

def digests(row, salt=""):
    return {stage: hashlib.sha256(canonical({"kind": "n", "value": {"row": row, "stage": stage, "salt": salt}})
                                  + b"\n").hexdigest() for stage in STAGES}

def run(mutant, row):
    """Digests of a row with `mutant` active."""
    if mutant in DIFFERING or mutant == 13:
        return {**digests(row), "parse": digests(row, "mutant")["parse"]}
    if mutant == 15 and SHIFT:
        return {**digests(row), "parse": digests(row, "mutant")["parse"],
                "node_index_before": digests(row, "shift")["node_index_before"]}
    return digests(row)

OK = {stage: "ok" for stage in STAGES}
args = dict(zip(sys.argv[2::2], sys.argv[3::2]))
if sys.argv[1] == "trace":
    requests = [json.loads(line) for line in open(args["--requests"])]
    stream = open(args["--frames"], "wb") if "--frames" in args else None
    with open(args["--out"], "w") as out:
        for request in requests:
            row = request["id"]
            if stream:
                for frame in frames(row):
                    stream.write(json.dumps(frame).encode() + b"\n")
            line = {"row": row, "request_sha256": request["request_sha256"], "outcomes": OK,
                    "digests": digests(row), "hits": HITS.get(row, []), "micros": 100,
                    "source_bytes": len(request["source_hex"]) // 2}
            if OBSERVE.get(row):
                line["observe_hits"] = OBSERVE[row]
            out.write(json.dumps(line) + "\n")
    if stream:
        stream.close()
    sys.exit(0)
emit({"ready": True})
for line in sys.stdin:
    job = json.loads(line)
    mutant, kills, ran, crashes = job["mutant"], [], 0, 0
    limit = job.get("max_kills", 3)
    for row in job["rows"]:
        emit({"heartbeat": row, "mutant": mutant})
        ran += 1
        if mutant == 2 or (mutant == 11 and row == "r2"):
            time.sleep(60)
        if mutant == 3:
            os._exit(3)
        if mutant in DIFFERING:
            result = {"mutant": mutant, "row": row, "outcomes": OK, "differs": ["parse"], "crash": False, "dump": None,
                      "digests": run(mutant, row), "micros": 5, "kill": True}
            control = job.get("control")
            if control is not None:
                emit({"heartbeat": row, "mutant": mutant, "control": control})
                controlled = run(control, row)
                differs = [stage for stage in STAGES if controlled[stage] != result["digests"][stage]]
                result["control"] = {"id": control, "outcomes": OK, "digests": controlled, "differs": differs,
                                     "crash": False, "dump": None}
                result["kill"] = any(stage in result["differs"] for stage in differs)
            if result["kill"]:
                kills.append(row)
            emit(result)
        elif mutant == 4:
            crashes += 1
            emit({"mutant": mutant, "row": row, "outcomes": {**{stage: "not_run" for stage in STAGES}, "parse": "panic"},
                  "messages": {"parse": "6d"}, "differs": STAGES, "crash": True, "kill": False, "dump": None,
                  "digests": {}, "micros": 5})
        elif job.get("report_all"):
            emit({"mutant": mutant, "row": row, "outcomes": OK, "differs": [], "crash": False, "kill": False,
                  "dump": None, "digests": run(mutant, row), "micros": 5})
        if limit and len(kills) >= limit:
            break
    if job.get("recheck", True):
        for row in kills:
            emit({"heartbeat": row, "mutant": mutant, "recheck": True})
            emit({"mutant": mutant, "recheck": row, "base_ok": mutant != 5})
    emit({"mutant": mutant, "done": True, "ran": ran, "differing": len(kills), "crashes": crashes})
'''


def fake_digests(row, salt=""):
    return {stage: hashlib.sha256(canonical({"kind": "n", "value": {"row": row, "stage": stage, "salt": salt}})
                                  + b"\n").hexdigest() for stage in E1}


class Fixture(unittest.TestCase):
    ROWS = [("r1", 30), ("r2", 10), ("r3", 20), ("r4", 5)]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.dir = Path(self.temporary.name)
        self.driver = self.dir / "fake-driver"
        self.driver.write_text(FAKE_DRIVER % {"scripts": str(ROOT / "scripts"), "stages": E1})
        self.driver.chmod(self.driver.stat().st_mode | stat.S_IEXEC)
        self.requests = self.dir / "requests-e1.ndjson"
        with self.requests.open("w") as stream:
            for row, size in self.ROWS:
                stream.write(json.dumps({"id": row, "request_sha256": "a" * 64, "source_hex": "00" * size}) + "\n")
        self.native = self.dir / "native-e1.json.gz"
        self.native_document = {
            "version": 1, "oracle": "e1", "requests_sha256": "b" * 64, "stages": E1, "operations": E1,
            "rows": [{"row": row, "request_sha256": "a" * 64, "outcomes": {stage: "ok" for stage in E1},
                      "digests": fake_digests(row)} for row, _ in self.ROWS]}
        go.write_rows_document(self.native, self.native_document)

    def tearDown(self):
        self.temporary.cleanup()

    def trace_document(self, hits, eligible=("r1", "r2", "r3"), observe=None):
        rows = []
        for row, size in self.ROWS:
            rows.append({"row": row, "request_sha256": "a" * 64, "base_match": row in eligible,
                         "eligible": row in eligible, "outcomes": {stage: "ok" for stage in E1},
                         "digests": fake_digests(row), "hits": hits.get(row, []), "micros": 100, "source_bytes": size})
            if (observe or {}).get(row):
                rows[-1]["observe_hits"] = observe[row]
        return {"version": 1, "oracle": "e1", "inputs": {
            "requests_sha256": "b" * 64, "native_sha256": go.file_sha256(self.native),
            "driver_sha256": go.file_sha256(self.driver), "ws": {"kind": "unmarked", "path": str(self.dir)}},
            "rows": rows}

    def reach_document(self, ops, unstable=()):
        return {"version": go.VERSION, "oracle": "e1", "native_sha256": go.file_sha256(self.native),
                "row_encoding": go.ROW_ENCODING, "rows_checked": len(self.ROWS),
                "stage_rule": go.stage_rule("e1"), "stage_rule_sha256": go.stage_rule_digest("e1"),
                "unstable_ops": sorted(unstable),
                "op_row_gaps": {op: go.encode_gaps(indices) for op, indices in sorted(ops.items())}}


def mutant(id_, ops, home, **fields):
    file, function = home.split("::")
    return {"id": id_, "key": f"{id_:016x}", "op": ops[0], "ops": ops, "file": file, "function": function,
            "span_sha256": "c" * 64, "operator": "test", **fields}


def control(id_, of, ops, home):
    return mutant(id_, ops, home, operator="control", control_of=of)


class CandidateTests(Fixture):
    def test_deadline_has_a_floor_and_scales_with_the_base_time(self):
        self.assertEqual(run.row_deadline(1000), run.DEADLINE_FLOOR)
        self.assertEqual(run.row_deadline(1_000_000), run.DEADLINE_FACTOR)

    def test_candidates_are_eligible_rust_and_go_reached_rows_smallest_source_first(self):
        trace = self.trace_document({"r1": [1, 2], "r2": [1], "r3": [1, 2], "r4": [1]},
                                    observe={"r2": [3], "r3": [2, 3], "r4": [3]})
        plan = {"mutants": [mutant(1, ["op/a", "op/b"], "f::a"), mutant(2, ["op/c"], "f::c"), mutant(3, ["op/a"], "f::d")]}
        reach = {"op/a": {0, 3}, "op/b": {1}, "op/c": set()}
        chosen = run.candidates(plan, trace, reach)
        self.assertEqual(chosen[1], ([0, 1, 2], [1, 0]), "r4 is ineligible; r3 is outside Go reach; r2 is smaller")
        self.assertEqual(chosen[2], ([0, 2], []))
        self.assertEqual(chosen[3], ([], []), "observation reach never makes a candidate")
        self.assertEqual(run.reach_table(plan, trace), {"0000000000000001": [3, 4, 0], "0000000000000002": [2, 2, 1],
                                                        "0000000000000003": [0, 0, 3]},
                         "reach counts eligible rows, every row and rows that executed the site while observing")

    def test_go_reach_binds_the_native_file_and_stage_rule_and_drops_unstable_operations(self):
        trace = self.trace_document({})
        path = self.dir / "reach.json.gz"
        go.write_ops_document(path, self.reach_document({"op/a": [0, 2], "op/pool": [1]}, unstable=["op/pool"]))
        self.assertEqual(run.load_go_reach(path, trace), ({"op/a": {0, 2}}, ["op/pool"]),
                         "an unstable operation is never a candidate even when a file indexes it")
        go.write_ops_document(path, {**self.reach_document({}), "stage_rule": {"segments": {}}})
        with self.assertRaisesRegex(ValueError, "another stage rule"):
            run.load_go_reach(path, trace)
        go.write_ops_document(path, {**self.reach_document({}), "stage_rule_sha256": "0" * 64})
        with self.assertRaisesRegex(ValueError, "another stage rule"):
            run.load_go_reach(path, trace)
        go.write_ops_document(path, self.reach_document({"op/a": [0, 2]}))
        trace["inputs"]["native_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "another native file"):
            run.load_go_reach(path, trace)
        go.write_ops_document(path, {**self.reach_document({}), "rows_checked": 3})
        trace["inputs"]["native_sha256"] = go.file_sha256(self.native)
        with self.assertRaisesRegex(ValueError, "checked 3 rows"):
            run.load_go_reach(path, trace)

    def test_oracle_commands_select_the_binary_protocol(self):
        argv, env = run.oracle_command("syntax", "/bin/syn", "kill", requests="r", base="b", dump_dir="d")
        self.assertEqual((argv, env), (["/bin/syn", "--requests", "r", "--base", "b", "--dump-dir", "d"],
                                       {"PHASE1_MUTATION": "kill"}))
        argv, env = run.oracle_command("facts", "/bin/drv", "trace", requests="r", out="o")
        self.assertEqual((argv, env), (["/bin/drv", "trace", "--oracle", "facts", "--requests", "r", "--out", "o"], {}))
        with self.assertRaisesRegex(ValueError, "unknown oracle"):
            run.package("other")


class SupervisorTests(Fixture):
    def argv(self):
        base = self.dir / "base.ndjson"
        base.write_text("")
        return [str(self.driver), "kill", "--oracle", "e1", "--requests", str(self.requests), "--base", str(base),
                "--dump-dir", str(self.dir)]

    def test_timed_out_and_dead_rows_are_skipped_and_the_run_continues(self):
        jobs = [{"mutant": 2, "rows": ["r1", "r2"]}, {"mutant": 1, "rows": ["r1", "r2"], "max_kills": 1},
                {"mutant": 3, "rows": ["r1"]}, {"mutant": 4, "rows": ["r1", "r2"]}, {"mutant": 6, "rows": ["r1", "r2"]},
                {"mutant": 11, "rows": ["r2", "r3", "r1"], "max_kills": 1}]
        started = time.monotonic()
        records = run.run_jobs(self.argv(), jobs, workers=2, deadline=lambda row: 0.5, log_dir=self.dir / "logs")
        self.assertLess(time.monotonic() - started, 20, "a hanging mutant must not stall the run")
        self.assertEqual((records[0]["status"], records[0]["timeouts"]), ("done", ["r1", "r2"]),
                         "every row of the hanging mutant was tried")
        self.assertEqual(records[0]["segments"], 2)
        self.assertEqual(records[1]["status"], "done")
        self.assertEqual((records[2]["status"], records[2]["deaths"], records[2]["abnormal"][0]["reason"]),
                         ("done", ["r1"], "exit 3"))
        self.assertEqual(records[3]["status"], "done")
        self.assertEqual(records[4]["status"], "done")
        self.assertEqual([result["row"] for result in records[1]["results"]], ["r1"], "max_kills stops the job")
        self.assertEqual(records[1]["rechecks"], {"r1": True})
        self.assertEqual(sum(result["crash"] for result in records[3]["results"]), 2)
        self.assertEqual(records[4]["ran"], 2)
        eleven = records[5]
        self.assertEqual(eleven["timeouts"], ["r2"])
        self.assertEqual([result["row"] for result in eleven["results"]], ["r3"],
                         "after the hanging row the mutant continues and kills; max_kills counts across segments")
        self.assertEqual(eleven["rechecks"], {"r3": True})

    def test_kills_before_a_skipped_row_count_against_max_kills(self):
        records = run.run_jobs(self.argv(), [{"mutant": 11, "rows": ["r1", "r2", "r3", "r4"], "max_kills": 2}],
                               workers=1, deadline=lambda row: 0.5, log_dir=self.dir / "logs")
        self.assertEqual([result["row"] for result in records[0]["results"]], ["r1", "r3"])
        self.assertEqual(records[0]["timeouts"], ["r2"])

    def test_a_mutant_stops_after_three_abnormal_rows(self):
        records = run.run_jobs(self.argv(), [{"mutant": 2, "rows": ["r1", "r2", "r3", "r4"]}], workers=1,
                               deadline=lambda row: 0.3, log_dir=self.dir / "logs")
        self.assertEqual((records[0]["status"], records[0]["row"], records[0]["timeouts"]),
                         ("timeout", "r3", ["r1", "r2", "r3"]))
        self.assertEqual(records[0]["ran"], 3, "r4 never ran")

    def test_memory_limit_turns_a_runaway_worker_into_a_timeout(self):
        def fake_memory(processes):
            return {process.pid: 10 ** 9 for process in processes}

        with patch.object(run, "_memory", fake_memory):
            records = run.run_jobs(self.argv(), [{"mutant": 2, "rows": ["r1"]}], workers=1, deadline=lambda row: 30,
                                   log_dir=self.dir / "logs", memory_limit_mb=1)
        self.assertEqual(records[0]["abnormal"][0]["reason"], "memory_limit")


class CampaignTests(Fixture):
    def campaign(self, extra_mutants=(), max_rows=0, skip_killed=None, env=None, out_name="run"):
        hits = {row: [1, 2, 3, 4, 5, 6, 8, 11, 12, 13, 14, 15] for row in ("r1", "r2", "r3")}
        hits["r4"] = [7]
        trace = self.dir / "trace-e1.json.gz"
        go.write_rows_document(trace, self.trace_document(hits))
        reach = self.dir / "reach-e1.json.gz"
        go.write_ops_document(reach, self.reach_document({"op/a": [0, 1, 2], "op/b": [2], "op/c": [0, 1, 2],
                                                           "op/d": [0, 1, 2, 3], "op/g": [0, 1, 2],
                                                           "op/pool": [0, 1, 2]}, unstable=["op/pool"]))
        plan = {"version": 1, "root_tree": "t", "unsupported": [
            {"op": "op/a", "file": "g", "function": "unprobed", "reason": "no operator"},
            {"op": "op/f", "file": "g", "function": "f", "reason": "no operator"}],
            "mutants": [mutant(1, ["op/a", "op/b"], "f1::a"), *[mutant(id_, ["op/c"], "f2::c") for id_ in (2, 3, 4, 5, 6)],
                        mutant(7, ["op/d"], "f3::d"), mutant(8, ["op/e"], "f4::e"),
                        mutant(12, ["op/g"], "f5::g", control=13), control(13, 12, ["op/g"], "f5::g"),
                        mutant(14, ["op/g"], "f6::g", control=15), control(15, 14, ["op/g"], "f6::g"),
                        mutant(16, ["op/pool"], "f7::pool"), *extra_mutants]}
        plan_path = self.dir / "plan.json"
        plan_path.write_bytes(canonical(plan) + b"\n")
        out = self.dir / out_name
        with patch.object(go, "load_native", return_value=self.native_document), \
                patch.object(run, "DEADLINE_FLOOR", 0.5), patch.dict(os.environ, env or {}):
            summary = run.kill("e1", self.dir, plan_path, trace, reach, 3, out, requests=self.requests,
                               native=self.native, driver=self.driver, max_rows=max_rows, skip_killed=skip_killed)
        return summary, run.read_gzip(out / "kill-e1.json.gz"), plan_path, out

    def test_kill_credits_only_rechecked_differences_and_records_every_state(self):
        summary, document, _, _ = self.campaign()
        entries = {entry["id"]: entry for entry in document["mutants"]}
        states = {id_: entry["state"] for id_, entry in entries.items()}
        self.assertEqual(states, {1: "killed", 2: "timeout", 3: "crash", 4: "crash", 5: "survived", 6: "survived",
                                  7: "not_reached", 8: "not_reached", 12: "survived", 14: "killed",
                                  16: "not_reached"})
        self.assertNotIn(13, entries, "controls never run as their own jobs")
        kills = entries[1]["kills"]
        self.assertEqual([kill_row["row"] for kill_row in kills], ["r2", "r3", "r1"], "smallest source first")
        self.assertEqual([kill_row["ops"] for kill_row in kills], [["op/a"], ["op/a", "op/b"], ["op/a"]],
                         "a kill credits only operations Go entered on its row")
        self.assertEqual(kills[0]["stages"], ["parse"])
        self.assertEqual(kills[0]["native"], fake_digests("r2"))
        self.assertIsNone(kills[0]["control"])
        self.assertNotEqual(kills[0]["mutant"]["parse"], kills[0]["native"]["parse"])
        self.assertEqual(entries[5]["kills"], [])
        self.assertIn("base recheck failed", {note["note"] for note in entries[5]["notes"]})
        self.assertEqual(document["settings"]["retried_jobs"], 1, "a failed recheck is redone in a fresh process")
        self.assertEqual(entries[2]["failure"], {"status": "timeout", "row": "r2", "reason": "deadline"})
        self.assertEqual(entries[2]["timeouts"], ["r1", "r2", "r3"])
        self.assertEqual(entries[3]["failure"]["reason"], "exit 3")
        self.assertEqual(entries[3]["deaths"], ["r1", "r2", "r3"], "each dead row is skipped in turn")
        self.assertEqual(entries[4]["crashes"], 3)
        self.assertEqual(entries[7]["reason"], "no Rust reach on an eligible row")
        self.assertEqual(entries[8]["reason"], "Go never entered the operation on a reached row")
        self.assertEqual(entries[16]["candidates"], 0, "an unstable Go operation is never a candidate")
        self.assertEqual(document["unstable_ops"], ["op/pool"])
        # Controls: 12's control reproduces its difference (a counter-only
        # allocation); 14's does not, and its kills carry the control digests.
        self.assertEqual({note["note"] for note in entries[12]["notes"]}, {"the control reproduces the difference"})
        fourteen = entries[14]["kills"][0]
        self.assertEqual(fourteen["control_id"], 15)
        self.assertEqual(fourteen["control"], fake_digests(fourteen["row"]))
        self.assertEqual(fourteen["control_stages"], ["parse"])
        self.assertEqual(document["reach"]["000000000000000d"], [3, 3, 0], "controls have measured reach too")
        self.assertEqual(summary["killed"], 2)
        self.assertEqual((document["settings"]["skip_killed"], document["skipped"], summary["skipped"]), (None, [], 0))

    def test_the_supervisor_rechecks_the_control_behind_the_oracle(self):
        # An oracle that calls a row a kill although its control reproduces the
        # mutant's digests is overruled.
        native = self.native_document["rows"][1]
        trace_row = self.trace_document({"r2": [14]})["rows"][1]
        mutated = {**fake_digests("r2"), "parse": fake_digests("r2", "mutant")["parse"]}
        result = {"row": "r2", "crash": False, "differs": ["parse"], "kill": True, "digests": mutated,
                  "outcomes": native["outcomes"],
                  "control": {"id": 15, "crash": False, "differs": [], "outcomes": native["outcomes"],
                              "digests": dict(mutated)}}
        record = {"rechecks": {"r2": True}}
        subject = mutant(14, ["op/g"], "f6::g", control=15)
        reach = {"op/g": {1}}
        kill, note = run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                 reach, "e1")
        self.assertEqual((kill, note), (None, "the control reproduces the difference"))
        # The control differs from the mutant only in node_index_before, where
        # the mutant equals native: the replaced value moved nothing.
        result["control"]["digests"] = {**mutated, "node_index_before": "0" * 64}
        kill, note = run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                 reach, "e1")
        self.assertEqual((kill, note), (None, "the control reproduces the difference"),
                         "the mutant must differ from native and from its control in one stage")
        result["control"]["digests"] = {**fake_digests("r2"), "node_index_before": "0" * 64}
        kill, note = run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                 reach, "e1")
        self.assertEqual((kill["control_stages"], kill["ops"], note), (["parse"], ["op/g"], None),
                         "control stages are only stages where the mutant differs from native")
        result["control"]["digests"] = fake_digests("r2")
        kill, note = run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                 reach, "e1")
        self.assertEqual((kill["control_stages"], kill["ops"], note), (["parse"], ["op/g"], None))
        result["control"]["crash"] = True
        self.assertEqual(run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                     reach, "e1"), (None, "the control crashed"))
        del result["control"]
        self.assertEqual(run._credit(subject, record, result, {"r2": (1, trace_row)}, self.native_document["rows"],
                                     reach, "e1"), (None, "no control run for an allocating mutant"))

    def test_a_row_budget_is_its_own_state_and_makes_the_results_partial(self):
        _, document, plan_path, out = self.campaign(max_rows=1)
        entries = {entry["id"]: entry for entry in document["mutants"]}
        self.assertEqual((entries[6]["state"], entries[6]["ran"], entries[6]["truncated"]), ("budget", 1, True))
        self.assertEqual(entries[1]["state"], "killed")
        self.assertEqual((entries[2]["state"], entries[2]["timeouts"]), ("budget", ["r2"]),
                         "a skipped hanging row does not make the unprobed rows final")
        run.results(plan_path, [out / "kill-e1.json.gz"], out / "results.json.gz")
        merged = run.read_gzip(out / "results.json.gz")
        states = {entry["id"]: entry["state"] for entry in merged["mutants"]}
        self.assertEqual((states[1], states[6]), ("killed", "budget"))
        self.assertTrue(merged["partial"], "budget is not a final state")

    def test_whole_program_oracles_never_credit_a_site_of_several_operations(self):
        self.assertEqual(run.WHOLE_PROGRAM_ORACLES, ("syntax",))
        with patch.object(run, "WHOLE_PROGRAM_ORACLES", ("e1",)):
            _, document, plan_path, out = self.campaign()
        entries = {entry["id"]: entry for entry in document["mutants"]}
        one = entries[1]
        self.assertEqual((one["state"], one["kills"]), ("not_credited_multi_op", []),
                         "mutant 1 carries op/a and op/b")
        self.assertEqual([(row["row"], row["reason"]) for row in one["not_credited"]],
                         [("r2", "not_credited_multi_op"), ("r3", "not_credited_multi_op"),
                          ("r1", "not_credited_multi_op")])
        self.assertEqual(entries[14]["state"], "killed", "a single-operation site is still credited")
        run.results(plan_path, [out / "kill-e1.json.gz"], out / "results.json.gz")
        merged = run.read_gzip(out / "results.json.gz")
        record = next(entry for entry in merged["mutants"] if entry["id"] == 1)
        self.assertEqual((record["state"], record["kills"], len(record["not_credited"])),
                         ("not_credited_multi_op", [], 3))
        self.assertEqual(merged["operations"]["op/b"]["state"], "not_credited_multi_op")
        self.assertFalse(merged["partial"])

    def test_skipped_mutants_are_recorded_and_must_be_killed_elsewhere(self):
        _, full, plan_path, out = self.campaign()
        keys = {entry["id"]: entry["key"] for entry in full["mutants"]}
        # A skip source that claims 1 (killed in the full run) and 6 (survived there).
        source = self.dir / "previous-results.json.gz"
        run.write_gzip(source, {"mutants": [{"key": keys[1], "state": "killed"}, {"key": keys[6], "state": "killed"},
                                            {"key": keys[5], "state": "survived"}]})
        summary, skipping, _, skip_out = self.campaign(skip_killed=source, out_name="run-skip")
        self.assertEqual(skipping["settings"]["skip_killed"],
                         {"source": str(source), "sha256": go.file_sha256(source), "killed": 2, "skipped": 2})
        self.assertEqual((skipping["skipped"], summary["skipped"]), (sorted([keys[1], keys[6]]), 2))
        self.assertNotIn(1, {entry["id"] for entry in skipping["mutants"]})
        # The skipping campaign stands for a second oracle.
        binder = self.dir / "kill-binder.json.gz"
        run.write_gzip(binder, {**skipping, "oracle": "binder"})
        run.results(plan_path, [out / "kill-e1.json.gz", binder], self.dir / "results.json.gz")
        merged = run.read_gzip(self.dir / "results.json.gz")
        records = {entry["id"]: entry for entry in merged["mutants"]}
        skip_sha256 = go.file_sha256(source)
        self.assertEqual((records[1]["state"], records[1]["skipped_by"]), ("killed", {"binder": skip_sha256}))
        self.assertEqual((records[6]["state"], records[6]["skipped_by"]), ("not_run", {"binder": skip_sha256}),
                         "a skip the merged kills do not justify leaves the mutant unmeasured")
        self.assertNotIn("skipped_by", records[5])
        self.assertTrue(merged["partial"])
        self.assertEqual(merged["inputs"]["oracles"]["binder"]["skip_killed"]["skipped"], 2)

    def test_results_merge_homes_and_operation_states(self):
        _, _, plan_path, out = self.campaign()
        summary = run.results(plan_path, [out / "kill-e1.json.gz"], out / "results.json.gz")
        document = run.read_gzip(out / "results.json.gz")
        operations = document["operations"]
        self.assertEqual(operations["op/a"]["state"], "partial", "a home without a mutant blocks a full kill")
        self.assertEqual(operations["op/a"]["homes_unprobed"], ["g::unprobed"])
        self.assertEqual(operations["op/b"]["state"], "killed")
        self.assertEqual(operations["op/c"]["state"], "timeout")
        self.assertEqual(operations["op/d"]["state"], "not_reached")
        self.assertEqual(operations["op/e"]["state"], "not_reached")
        self.assertEqual(operations["op/f"]["state"], "unsupported")
        self.assertEqual(operations["op/g"]["state"], "partial",
                         "f5::g is reached and unkilled (its control reproduced the difference)")
        self.assertEqual(summary["mutants"]["killed"], 2)
        self.assertEqual(summary["mutants"]["control"], 2)
        self.assertFalse(document["partial"], "controls are not un-run mutants")
        states = {entry["id"]: entry["state"] for entry in document["mutants"]}
        self.assertEqual((states[13], states[15]), ("control", "control"))
        killed = [entry for entry in document["mutants"] if entry["state"] == "killed"]
        self.assertEqual([kill_row["oracle"] for kill_row in killed[0]["kills"]], ["e1"] * 3)
        self.assertEqual(document["inputs"]["traced_oracles"], ["e1"])
        with self.assertRaisesRegex(ValueError, "another plan"):
            other = self.dir / "other-plan.json"
            other.write_text("{}")
            run.results(other, [out / "kill-e1.json.gz"])

    def test_retries_dump_into_their_own_directory(self):
        # Crediting the first attempt deletes its dumps; a retry of the same
        # (mutant, row) must not share the file names it would delete.
        with patch.object(run, "run_jobs", wraps=run.run_jobs) as jobs:
            self.campaign()
        dump_dirs = [call.args[0][call.args[0].index("--dump-dir") + 1] for call in jobs.call_args_list]
        self.assertEqual(len(dump_dirs), 2, "mutant 5's failed recheck is retried")
        self.assertNotEqual(dump_dirs[0], dump_dirs[1])

    def test_merged_budget_is_final_only_when_killed_elsewhere(self):
        _, document, plan_path, out = self.campaign(max_rows=1)
        entries = {entry["id"]: entry for entry in document["mutants"]}
        self.assertEqual((entries[12]["state"], entries[14]["state"]), ("budget", "killed"))
        killed_12 = {**entries[12], "state": "killed", "kills": [{"row": "r1", "ops": ["op/g"]}]}
        other = self.dir / "kill-binder.json.gz"
        run.write_gzip(other, {**document, "oracle": "binder", "mutants": [
            killed_12 if entry["id"] == 12 else {**entry, "state": "survived", "kills": []}
            for entry in document["mutants"]]})
        run.results(plan_path, [out / "kill-e1.json.gz", other], self.dir / "results.json.gz")
        merged = run.read_gzip(self.dir / "results.json.gz")
        states = {entry["id"]: entry["state"] for entry in merged["mutants"]}
        self.assertEqual(states[12], "killed", "a kill on another oracle is final")
        self.assertEqual(states[6], "budget", "budget on one oracle outranks survived on another")
        self.assertTrue(merged["partial"])

    def test_results_mark_planned_mutants_no_campaign_ran_as_not_run(self):
        _, document, plan_path, out = self.campaign()
        subset = out / "kill-e1-subset.json.gz"
        run.write_gzip(subset, {**document, "mutants": [entry for entry in document["mutants"] if entry["id"] < 7]})
        run.results(plan_path, [subset], out / "results-subset.json.gz")
        merged = run.read_gzip(out / "results-subset.json.gz")
        states = {entry["id"]: entry["state"] for entry in merged["mutants"]}
        self.assertEqual((states[7], states[8]), ("not_run", "not_run"),
                         "a mutant no kill file ran is not evidence of non-reach")
        self.assertEqual(merged["mutants"][6]["oracles"], {})
        self.assertEqual((merged["operations"]["op/d"]["state"], merged["operations"]["op/e"]["state"]),
                         ("not_run", "not_run"), "a reached home whose mutants never ran is no evidence")
        self.assertEqual(merged["operations"]["op/b"]["state"], "killed")
        self.assertTrue(merged["partial"])
        self.assertEqual(merged["summary"]["mutants"]["not_run"], 5)

    def test_kill_refuses_a_trace_from_another_driver_build(self):
        trace = self.dir / "trace-e1.json.gz"
        document = self.trace_document({})
        document["inputs"]["driver_sha256"] = "0" * 64
        go.write_rows_document(trace, document)
        plan_path = self.dir / "plan.json"
        plan_path.write_bytes(canonical({"mutants": [], "unsupported": []}))
        with patch.object(go, "load_native", return_value=self.native_document), \
                self.assertRaisesRegex(ValueError, "another oracle build"):
            run.kill("e1", self.dir, plan_path, trace, self.dir / "missing", 1, self.dir / "run",
                     requests=self.requests, native=self.native, driver=self.driver)


def kill_file(oracle, plan_sha256, entries, reach):
    return {"version": 1, "oracle": oracle, "inputs": {"plan_sha256": plan_sha256}, "reach": reach,
            "mutants": entries}


def entry(id_, key, ops, state, kills=(), rust_rows=0):
    return {"id": id_, "key": key, "op": ops[0], "ops": ops, "state": state, "candidates": len(kills),
            "ran": len(kills), "crashes": 0, "rust_rows": rust_rows, "kills": list(kills)}


class HomeTests(unittest.TestCase):
    """Every marker site is a home; excusing one needs reach measured on every traced oracle."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.dir = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def merge(self, homes, kill_files):
        plan = {"version": 1, "unsupported": [], "homes": homes, "mutants": [
            mutant(1, ["op/a"], "a.rs::f"), mutant(2, ["op/a"], "a.rs::g"), mutant(3, ["op/b"], "b.rs::h"),
            mutant(4, ["op/b"], "b.rs::stmt")]}
        plan_path = self.dir / "plan.json"
        plan_path.write_bytes(canonical(plan) + b"\n")
        paths = []
        for oracle, (entries, reach) in kill_files.items():
            path = self.dir / f"kill-{oracle}.json.gz"
            run.write_gzip(path, kill_file(oracle, go.file_sha256(plan_path), entries, reach))
            paths.append(path)
        run.results(plan_path, paths, self.dir / "results.json.gz")
        return run.read_gzip(self.dir / "results.json.gz")["operations"]

    def test_unreached_homes_are_excused_only_when_measured_on_every_traced_oracle(self):
        key = {id_: f"{id_:016x}" for id_ in range(1, 5)}
        kill_row = {"row": "r", "ops": ["op/a"]}
        homes = {"op/a": [{"file": "a.rs", "function": "f", "site_kind": "fn", "site_line": 1, "span_sha256": "s",
                           "mutants": [key[1]]},
                          {"file": "a.rs", "function": "g", "site_kind": "arm", "site_line": 9, "span_sha256": "t",
                           "mutants": [key[2]]}],
                 "op/b": [{"file": "b.rs", "function": "h", "site_kind": "fn", "site_line": 1, "span_sha256": "u",
                           "mutants": [key[3]]},
                          {"file": "b.rs", "function": "stmt", "site_kind": "stmt", "site_line": 5, "span_sha256": "v",
                           "mutants": []}]}
        entries = [entry(1, key[1], ["op/a"], "killed", [kill_row], 1), entry(2, key[2], ["op/a"], "not_reached"),
                   entry(3, key[3], ["op/b"], "killed", [{"row": "r", "ops": ["op/b"]}], 1),
                   entry(4, key[4], ["op/b"], "not_reached")]
        e1 = (entries, {key[1]: [1, 1, 0], key[2]: [0, 0, 0], key[3]: [1, 1, 0], key[4]: [0, 0, 0]})
        operations = self.merge(homes, {"e1": e1})
        self.assertEqual(operations["op/a"]["state"], "killed", "the unreached arm home is excused")
        excused = [home for home in operations["op/a"]["homes"] if home["excused"]]
        self.assertEqual([(home["site_kind"], home["reason"]) for home in excused],
                         [("arm", "no production or observation reach on e1")])
        self.assertEqual(operations["op/b"]["state"], "partial", "a statement home with no mutant keeps it pending")
        self.assertEqual(operations["op/b"]["homes_unprobed"], ["b.rs::stmt@stmt:5"])

        # A second traced oracle that did not measure the arm: no excuse.
        binder = ([entry(1, key[1], ["op/a"], "not_reached")], {key[1]: [0, 0, 0]})
        operations = self.merge(homes, {"e1": e1, "binder": binder})
        self.assertEqual(operations["op/a"]["state"], "partial")
        arm = next(home for home in operations["op/a"]["homes"] if home["site_kind"] == "arm")
        self.assertEqual((arm["measured"], arm["excused"]), (["e1"], False))

        # Reach on an ineligible row of any oracle is reach: no excuse.
        binder = ([entry(2, key[2], ["op/a"], "not_reached")], {key[1]: [0, 0, 0], key[2]: [0, 3, 0]})
        operations = self.merge(homes, {"e1": e1, "binder": binder})
        arm = next(home for home in operations["op/a"]["homes"] if home["site_kind"] == "arm")
        self.assertEqual((arm["reached"], arm["excused"], operations["op/a"]["state"]), (True, False, "partial"))

    def test_observation_reach_or_an_unmeasured_observation_count_blocks_an_excuse(self):
        key = {id_: f"{id_:016x}" for id_ in range(1, 5)}
        homes = {"op/a": [{"file": "a.rs", "function": "f", "site_kind": "fn", "site_line": 1, "span_sha256": "s",
                           "mutants": [key[1]]},
                          {"file": "a.rs", "function": "g", "site_kind": "arm", "site_line": 9, "span_sha256": "t",
                           "mutants": [key[2]]}]}
        entries = [entry(1, key[1], ["op/a"], "killed", [{"row": "r", "ops": ["op/a"]}], 1),
                   entry(2, key[2], ["op/a"], "not_reached"), entry(3, key[3], ["op/b"], "not_reached"),
                   entry(4, key[4], ["op/b"], "not_reached")]
        rest = {key[3]: [0, 0, 0], key[4]: [0, 0, 0]}
        operations = self.merge(homes, {"e1": (entries, {key[1]: [1, 1, 0], key[2]: [0, 0, 5], **rest})})
        arm = next(home for home in operations["op/a"]["homes"] if home["site_kind"] == "arm")
        self.assertEqual((arm["reached"], arm["observed"], arm["excused"]), (False, True, False),
                         "a home the observers execute is not a home no oracle executes")
        self.assertEqual((operations["op/a"]["state"], operations["op/a"]["homes_observed"]),
                         ("partial", ["a.rs::g@arm:9"]))
        operations = self.merge(homes, {"e1": (entries, {key[1]: [1, 1, 0], key[2]: [0, 0], **rest})})
        arm = next(home for home in operations["op/a"]["homes"] if home["site_kind"] == "arm")
        self.assertEqual((arm["measured"], arm["excused"]), ([], False),
                         "a reach entry without an observation count measured nothing")


class TableRuleTests(unittest.TestCase):
    """The table oracle's column parity and one-home rules (docs section 9)."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.dir = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def requests(self, columns):
        path = self.dir / "requests-table.ndjson"
        path.write_bytes(b"".join(canonical({"id": row, "column": column, "op": "table", "input": {}}) + b"\n"
                                  for row, column in columns.items()))
        return path

    def test_column_parity_is_every_row_of_the_column_matching_native(self):
        column_of = run.row_columns(self.requests({"a1": "A", "a2": "A", "b1": "B"}))
        self.assertEqual(column_of, {"a1": "A", "a2": "A", "b1": "B"})
        rows = [{"row": "a1", "base_match": True}, {"row": "a2", "base_match": False},
                {"row": "b1", "base_match": True}]
        parity = run.column_parity(column_of, rows)
        self.assertEqual(parity, {"A": {"rows": 2, "base_match": 1, "mismatched": ["a2"]},
                                  "B": {"rows": 1, "base_match": 1, "mismatched": []}})
        gate = run.parity_gate(column_of, parity)
        self.assertIsNone(gate("b1"))
        self.assertIn("column A differs from native on 1 of its 2 rows", gate("a1"))
        # Against native rows directly (the confirmation replay): outcomes, digests and messages.
        ok = {"setup": "ok", "column": "ok"}
        natives = {"a1": {"outcomes": ok, "digests": {"column": "x"}}, "a2": {"outcomes": ok, "digests": {"column": "y"}},
                   "b1": {"outcomes": ok, "digests": {"column": "z"}}}
        replay = [{"row": "a1", "outcomes": ok, "digests": {"column": "x"}},
                  {"row": "a2", "outcomes": ok, "digests": {"column": "y"}, "error": "session defect"},
                  {"row": "b1", "outcomes": ok, "digests": {"column": "z"}}]
        self.assertEqual(run.column_parity(column_of, replay, natives)["A"]["base_match"], 1)
        self.assertEqual(run.column_parity(column_of, replay, natives)["B"]["base_match"], 1)

    def test_a_kill_on_a_column_without_parity_is_never_credited(self):
        column_of = {"a1": "A", "a2": "A", "b1": "B"}
        parity = {"A": {"rows": 2, "base_match": 1, "mismatched": ["a2"]}, "B": {"rows": 1, "base_match": 1,
                                                                               "mismatched": []}}
        gate = run.parity_gate(column_of, parity)
        mutant_entry = mutant(1, ["op/a"], "a.rs::f")
        native = {"row": "a1", "request_sha256": "q", "outcomes": {"setup": "ok", "column": "ok"},
                  "digests": {"column": "n"}}
        rows_by_id = {"a1": (0, {**native, "hits": [1], "base_match": True}),
                      "b1": (1, {**native, "row": "b1", "hits": [1], "base_match": True})}
        record = {"rechecks": {"a1": True, "b1": True}}
        result = {"row": "a1", "crash": False, "differs": ["column"], "kill": True, "digests": {"column": "m"},
                  "outcomes": {"setup": "ok", "column": "ok"}, "dump": None}
        reach = {"op/a": {0, 1}}
        # The table oracle dumps kills; skip the dump check to isolate the gate.
        with patch.object(run, "DUMPING", ()):
            kill, note = run._credit(mutant_entry, record, result, rows_by_id, [native, native], reach, "table", gate)
            self.assertIsNone(kill)
            self.assertIn("column parity fails", note)
            kill, note = run._credit(mutant_entry, record, {**result, "row": "b1"}, rows_by_id, [native, native],
                                     reach, "table", gate)
            self.assertIsNone(note)
            self.assertEqual((kill["row"], kill["ops"], kill["stages"]), ("b1", ["op/a"], ["column"]))

    def test_table_frames_digest_by_the_go_rule(self):
        value = [[0, [1]], {"x": "\u00e9"}]
        frames = [{"tag": "begin", "id": "r"}, {"tag": "stage", "stage": "setup", "outcome": "ok", "message_hex": ""},
                  {"tag": "stage", "stage": "column", "outcome": "ok", "message_hex": ""},
                  {"tag": "observation", "stage": "column", "kind": "value", "value": value}, {"tag": "end"}]
        row = run.frame_digests("table", frames)
        self.assertEqual(row, {"outcomes": {"setup": "ok", "column": "ok"},
                               "digests": {"column": hashlib.sha256(canonical(value)).hexdigest()}})
        panicked = run.frame_digests("table", [frames[0], frames[1], {**frames[2], "outcome": "panic", "message_hex": "6d"},
                                               frames[4]])
        self.assertEqual((panicked["outcomes"], panicked["digests"], panicked["messages"]),
                         ({"setup": "ok", "column": "panic"}, {}, {"column": "6d"}))
        with self.assertRaisesRegex(ValueError, "one column value"):
            run.frame_digests("table", frames[:4] + [frames[3], frames[4]])
        self.assertIn("table", run.DUMPING)
        self.assertEqual(run.PACKAGES["table"], run.DRIVER)

    def test_a_table_operation_never_has_an_excused_home(self):
        key = {id_: f"{id_:016x}" for id_ in range(1, 5)}
        homes = {"op/a": [{"file": "a.rs", "function": "f", "site_kind": "fn", "site_line": 1, "span_sha256": "s",
                           "mutants": [key[1]]},
                          {"file": "a.rs", "function": "g", "site_kind": "fn", "site_line": 9, "span_sha256": "t",
                           "mutants": [key[2]]}]}
        plan = {"version": 1, "unsupported": [], "homes": homes, "mutants": [
            mutant(1, ["op/a"], "a.rs::f"), mutant(2, ["op/a"], "a.rs::g")]}
        plan_path = self.dir / "plan.json"
        plan_path.write_bytes(canonical(plan) + b"\n")
        entries = [entry(1, key[1], ["op/a"], "killed", [{"row": "r", "ops": ["op/a"]}], 1),
                   entry(2, key[2], ["op/a"], "not_reached")]
        reach = {key[1]: [1, 1, 0], key[2]: [0, 0, 0]}

        def merge(one_home):
            document = kill_file("table", go.file_sha256(plan_path), entries, reach)
            document["columns"] = {"C": {"rows": 1, "base_match": 1, "mismatched": []}}
            document["one_home_operations"] = one_home
            run.write_gzip(self.dir / "kill-table.json.gz", document)
            run.results(plan_path, [self.dir / "kill-table.json.gz"], self.dir / "results.json.gz")
            return run.read_gzip(self.dir / "results.json.gz")

        excused = merge([])
        self.assertEqual(excused["operations"]["op/a"]["state"], "killed", "the unreached copy is excused")
        self.assertEqual(excused["columns"], {"table": {"C": {"rows": 1, "base_match": 1, "mismatched": []}}})
        results = merge(["op/a"])
        self.assertEqual(results["operations"]["op/a"]["state"], "partial")
        copy = next(home for home in results["operations"]["op/a"]["homes"] if home["function"] == "g")
        self.assertEqual((copy["excused"], copy["reason"]), (False, run.ONE_HOME_REASON))
        self.assertEqual(results["one_home_operations"], ["op/a"])

    def test_table_operations_are_the_spec_claims_of_the_traced_columns(self):
        import phase1_tables
        specs = phase1_tables.load_specs()
        column = next(column for _, column in phase1_tables.columns(specs) if column["operations"])
        self.assertEqual(run.table_operations([column["id"], "no.such.column"]), sorted(column["operations"]))
        self.assertEqual(run.table_operations(["runtime.walk"]), [])


class TraceTests(Fixture):
    def test_trace_checks_rows_against_native_and_the_frames_in_shards(self):
        self.native_document["rows"][1]["digests"]["parse"] = "0" * 64
        out = self.dir / "run"
        for shards in (1, 3):
            with patch.object(go, "load_native", return_value=self.native_document), \
                    patch.dict(os.environ, {"FAKE_HITS": json.dumps({"r1": [3, 1], "r4": [9]}),
                                            "FAKE_OBSERVE": json.dumps({"r2": [1, 4], "r3": [4]})}):
                summary = run.trace("e1", self.dir, self.requests, self.native, out, driver=self.driver,
                                    verify_frames=True, shards=shards)
            self.assertEqual((summary["rows"], summary["base_match"], summary["eligible"]), (4, 3, 3))
            self.assertEqual(summary["frames_verified"], 4)
            self.assertEqual(summary["mismatched_first"], ["r2"])
            self.assertEqual(summary["mismatched_smallest"], [{"row": "r2", "source_bytes": 10,
                                                               "outcomes": {stage: "ok" for stage in E1},
                                                               "stages": ["parse"]}])
            self.assertEqual(summary["mutants_reached"], 3)
            self.assertEqual((summary["mutants_observed"], summary["observe_only_count"]), (2, 1))
            document = run.read_gzip(out / "trace-e1.json.gz")
            self.assertEqual([row.get("observe_hits") for row in document["rows"]], [None, [1, 4], [4], None],
                             "observation reach is carried per row")
            self.assertEqual([row["row"] for row in document["rows"]], ["r1", "r2", "r3", "r4"], "shards keep order")
            self.assertEqual([row["eligible"] for row in document["rows"]], [True, False, True, True])
            self.assertEqual(document["inputs"]["driver_sha256"], go.file_sha256(self.driver))
            self.assertEqual(run.compare_traces(out / "trace-e1.json.gz", out / "trace-e1.json.gz")["differing"], 0)
        self.assertEqual(sorted(path.name for path in out.iterdir()), ["trace-e1.json.gz"], "shard files are removed")


class FrameTests(unittest.TestCase):
    def test_facts_and_syntax_frames_digest_by_the_go_rules(self):
        pairs = [[308, 4194304], [80, 0]]
        frames = [{"tag": "begin", "id": "r"}, {"tag": "stage", "stage": "parse", "outcome": "ok", "message_hex": ""},
                  {"tag": "stage", "stage": "subtree_facts", "outcome": "ok", "message_hex": ""},
                  {"tag": "observation", "stage": "subtree_facts", "kind": "facts", "value": pairs},
                  {"tag": "end"}]
        facts = run.frame_digests("facts", frames)
        self.assertEqual(facts["digests"], {"subtree_facts": hashlib.sha256(canonical(pairs)).hexdigest()})
        self.assertEqual(facts["outcomes"], {"parse": "ok", "subtree_facts": "ok"})
        parse_failed = run.frame_digests("facts", [frames[0], {**frames[1], "outcome": "panic", "message_hex": "6d"}])
        self.assertEqual((parse_failed["outcomes"], parse_failed["digests"]),
                         ({"parse": "panic", "subtree_facts": "not_run"}, {}))
        row = {"id": "s", "state": "observed", "files": 20, "file_names_sha256": "f" * 64, "syntactic": [],
               "plain_hex": "", "pretty_hex": ""}
        syntax = run.frame_digests("syntax", [row])
        self.assertEqual(syntax["outcomes"], {"program": "ok"})
        self.assertEqual(syntax["digests"]["files"], hashlib.sha256(canonical(20)).hexdigest())
        failed = run.frame_digests("syntax", [{"id": "s", "state": "panic", "panic": "boom"}])
        self.assertEqual((failed["outcomes"], failed["messages"]), ({"program": "panic"}, {"program": b"boom".hex()}))


class DumpTests(unittest.TestCase):
    @staticmethod
    def frames(value):
        stages = go.ORACLES["binder"].operations
        records = [{"tag": "begin"}]
        for stage in stages:
            if stage.endswith("_graph"):
                records.append({"tag": "observation", "stage": stage, "kind": "symbol", "value": value})
            records.append({"tag": "stage", "stage": stage, "outcome": "ok", "message_hex": ""})
        return records + [{"tag": "end"}]

    def confirm(self, native_value, mutant_value, *, reported=None):
        native = go.row_digests("binder", self.frames(native_value))
        mutant = go.row_digests("binder", self.frames(mutant_value))
        with tempfile.TemporaryDirectory() as directory:
            dump = Path(directory) / "dump.ndjson"
            dump.write_text("".join(json.dumps(frame) + "\n" for frame in self.frames(mutant_value)))
            return run.confirm_dump("binder", dump, native, reported or mutant)

    def test_binder_kills_are_confirmed_on_normalized_frames(self):
        name = {"raw_hex": "fe31", "identity": {"kind": "node", "ref": 1, "prefix_hex": "", "suffix_hex": ""}}
        renamed = {**name, "raw_hex": "fe32"}
        self.assertFalse(self.confirm({"name": name}, {"name": renamed})["ok"],
                         "an identity-number-only difference is no kill")
        result = self.confirm({"name": name, "flags": 1}, {"name": name, "flags": 2})
        self.assertTrue(result["ok"])
        self.assertEqual(result["stages"], ["parsed_graph", "bound_graph", "repeated_graph"])
        wrong = self.confirm({"flags": 1}, {"flags": 2}, reported={"outcomes": {}, "digests": {}})
        self.assertFalse(wrong["ok"])
        self.assertFalse(wrong["consistent"])


class SharedSiteTests(unittest.TestCase):
    def test_a_site_of_several_operations_is_probed_per_operation(self):
        rows = [{"row": f"r{index}"} for index in range(5)]
        reach = {"op/a": {0, 1, 2, 3}, "op/b": {4}}
        shared = mutant(9, ["op/a", "op/b"], "f::shared")
        jobs = run.mutant_jobs(shared, [0, 1, 2, 3, 4], rows, reach, 3)
        self.assertEqual([job["rows"] for job in jobs], [["r0", "r1", "r2", "r3"], ["r4"]],
                         "op/b's only row is not left behind op/a's first three kills")
        single = mutant(10, ["op/a"], "f::single", control=11)
        self.assertEqual(run.mutant_jobs(single, [2, 0], rows, reach, 3),
                         [{"mutant": 10, "rows": ["r2", "r0"], "max_kills": 3, "control": 11}])
        self.assertEqual(run.mutant_jobs(single, [], rows, reach, 3), [])
        self.assertEqual(run.mutant_jobs(single, [2, 0, 1], rows, reach, 3, max_rows=2)[0]["truncated"], 1)

    def test_recorded_kills_keep_up_to_the_limit_per_credited_operation(self):
        kills = [{"row": f"r{index}", "ops": ops} for index, ops in
                 enumerate([["op/a"], ["op/a"], ["op/a"], ["op/a"], ["op/b"], ["op/a", "op/b"]])]
        self.assertEqual([kill["row"] for kill in run.kept_kills(kills, 3)], ["r0", "r1", "r2", "r4", "r5"])
        self.assertEqual(run.kept_kills(kills, 0), kills)


class ConfirmTests(Fixture):
    """The receipt replays every recorded pair and control, and re-traces for excused homes."""

    def results(self):
        _, _, plan_path, out = CampaignTests.campaign(self)
        run.results(plan_path, [out / "kill-e1.json.gz"], out / "results.json.gz")
        return plan_path, out / "results.json.gz"

    def confirm(self, plan_path, results_path, fake_hits, observe=None, env=None):
        with patch.object(go, "load_native", return_value=self.native_document), \
                patch.dict(os.environ, {"FAKE_HITS": json.dumps(fake_hits), "FAKE_OBSERVE": json.dumps(observe or {}),
                                        **(env or {})}):
            return run.confirm(results_path, plan_path, ws=self.dir, drivers={run.DRIVER: self.driver},
                               requests={"e1": self.requests}, natives={"e1": self.native}, jobs=2,
                               out=self.dir / "confirm")

    def test_every_pair_reproduces_with_its_control_and_excused_homes_stay_unreached(self):
        plan_path, results_path = self.results()
        receipt = self.confirm(plan_path, results_path, {"r1": [1, 14]})
        self.assertEqual(receipt["result"], "pass", receipt["failures"])
        self.assertEqual((receipt["pairs_total"], receipt["confirmed"]), (6, 6))
        pair = next(pair for pair in receipt["pairs"] if pair["key"] == "000000000000000e")
        self.assertEqual(pair["state"], "reproduced")
        self.assertEqual(pair["stages"], ["parse"])
        self.assertEqual(pair["control"], fake_digests(pair["row"]), "the control's replayed digests are listed")
        self.assertEqual(receipt["base_rows"], receipt["base_native"])
        self.assertEqual(receipt["retraced"], {"e1": {"rows": 4, "mutants_reached": 2, "mutants_observed": 0}})

    def test_a_changed_pair_or_a_reached_excused_home_fails(self):
        plan_path, results_path = self.results()
        document = run.read_gzip(results_path)
        one = next(entry for entry in document["mutants"] if entry["id"] == 1)
        one["kills"][0]["mutant"]["parse"] = "0" * 64
        document["operations"]["op/d"]["homes"][0]["excused"] = True
        run.write_gzip(results_path, document)
        receipt = self.confirm(plan_path, results_path, {"r4": [7]})
        self.assertEqual(receipt["result"], "fail")
        states = sorted({pair["state"] for pair in receipt["pairs"]})
        self.assertEqual(states, ["changed", "reproduced"])
        self.assertIn({"oracle": "e1", "key": "0000000000000007", "rows": 1, "production_rows": 1, "observe_rows": 0,
                       "failure": "an excused home is reached"}, receipt["failures"])

    def test_an_excused_home_executed_while_observing_fails(self):
        plan_path, results_path = self.results()
        document = run.read_gzip(results_path)
        document["operations"]["op/d"]["homes"][0]["excused"] = True
        run.write_gzip(results_path, document)
        receipt = self.confirm(plan_path, results_path, {}, observe={"r3": [7], "r4": [7]})
        self.assertEqual(receipt["result"], "fail")
        self.assertEqual(receipt["failures"], [{"oracle": "e1", "key": "0000000000000007", "rows": 2,
                                                "production_rows": 0, "observe_rows": 2,
                                                "failure": "an excused home executes while observing"}])
        self.assertEqual(receipt["retraced"]["e1"]["mutants_observed"], 1)

    def test_a_pair_whose_control_differs_only_where_the_mutant_equals_native_is_not_a_kill(self):
        # The recorded kill of 14 is rewritten as if credited under the old
        # rule: its control equals the mutant in `parse`, the only stage where
        # the mutant differs from native, and differs from it in
        # node_index_before. The replay reproduces those digests exactly.
        plan_path, results_path = self.results()
        document = run.read_gzip(results_path)
        fourteen = next(entry for entry in document["mutants"] if entry["id"] == 14)
        for kill_row in fourteen["kills"]:
            row = kill_row["row"]
            kill_row["control"] = {**fake_digests(row), "parse": fake_digests(row, "mutant")["parse"],
                                   "node_index_before": fake_digests(row, "shift")["node_index_before"]}
        run.write_gzip(results_path, document)
        receipt = self.confirm(plan_path, results_path, {}, env={"FAKE_CONTROL_SHIFT": "1"})
        states = {(pair["key"], pair["row"]): pair["state"] for pair in receipt["pairs"]}
        self.assertEqual({state for (key, _), state in states.items() if key == fourteen["key"]}, {"not_a_kill"})
        self.assertEqual({state for (key, _), state in states.items() if key != fourteen["key"]}, {"reproduced"})
        self.assertEqual(receipt["result"], "fail")


if __name__ == "__main__":
    unittest.main()
