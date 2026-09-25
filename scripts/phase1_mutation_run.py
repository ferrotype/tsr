#!/usr/bin/env python3
"""Phase 1 mutation witnesses, Rust side: base trace, kill campaign, results, confirmation.

docs/PHASE1-mutation-witnesses.md is the contract. This module runs the Rust
side of every oracle and owns the Rust half of the evidence. The ``e1``,
``binder``, ``facts`` and ``table`` oracles are ``phase1_mutation_driver``
(tools/phase1/mutation/driver); the ``syntax`` oracle is the ``phase1_syntax``
harness in its mutation mode (``PHASE1_MUTATION=trace|kill``). Both speak one
protocol (tools/phase1/mutation/driver/src/jobs.rs).

* ``trace`` builds the oracle binary in a workspace (the repository, or a
  spliced scratch copy from ``phase1_mutation_plan.schemata``), runs every
  request row once with no active mutant (sharded over processes when asked),
  and checks each row's outcomes, messages and stage digests against the frozen
  native file of ``phase1_mutation_go``. It writes ``trace-<oracle>.json.gz``:
  per row ``base_match``, ``eligible`` (base match with every stage ``ok``), the
  base digests, the mutant ids the row reached in production (``hits``; E1 also
  counts parser sites reached while observing, for lazy JSDoc), the ids of every
  other site executed while observing (``observe_hits``: observation reach,
  never activating), the row time and the source size.
* ``kill`` selects, per planned mutant, the candidate rows (eligible, reached
  by the mutant in Rust, and on which Go entered one of the mutant's
  operations; unstable Go operations are never candidates), smallest source
  first, and feeds them to parallel oracle processes. A per-row deadline of
  max(2 s, 20 x the row's base time), enforced from heartbeats, and a
  resident-memory limit catch hanging mutants. A timed-out or dead row is
  skipped: the worker restarts and the same mutant continues with its next
  candidate rows, up to three abnormal rows per mutant (``timeouts``,
  ``deaths``). A row is credited as a kill only when every stage completed
  ``ok``, a compared stage differs from native, the same row run afterwards in
  the same process with no mutant still equals the base, the dumped frames
  (binder, syntax, facts) digest to the reported values, and, for a mutant
  whose operator allocates, its paired control mutant (the replacement computed
  and discarded) completes like native and, in a compared stage where the
  mutant differs from native, differs from the mutant too. Control digests are
  recorded with the kill. A syntax row is a whole program, so Go enters nearly
  every operation on it: a syntax kill of a site that carries markers for
  several operations is never credited and is recorded apart
  (``not_credited``, state ``not_credited_multi_op``). ``--skip-killed`` records
  its source's digest, the skipped count and the skipped keys.
* ``results`` merges the per-oracle kill files into ``results.json.gz``. An
  operation's homes are every marker site of the plan (``homes``); a home with
  no production and no observation reach on any row of every traced oracle is
  excused, and the operation is ``killed`` only when every other home is
  killed. A mutant that is not killed and that some traced oracle did not run
  (``not_run``; skipped mutants carry ``skipped_by``) or did not finish
  (``budget``) makes the results ``partial``: those states are not final.
* The ``table`` oracle adds two rules (docs section 9). **Column parity**: a
  kill on a row of column C credits only while every row of C matches native
  in the base trace (``kill`` records each column's parity, ``results`` keeps
  it, ``confirm`` re-traces it). **One home per table operation**: an
  operation a table column claims (``data/phase1/tables/<group>.json``) has no
  excusable home on any oracle, so an extra marked copy must be reached and
  killed or lose its marker. ``results`` writes that set as
  ``one_home_operations`` and the scope check applies the same set to every
  mutation witness; an operation a column credits only through its callee
  keeps the ordinary excusal rule.
* ``confirm`` splices the full manifest (the campaign's mutant ids), replays
  every recorded kill pair and its control plus the pairs' base rows, requires
  each pair to reproduce its recorded stages and digests, re-traces every
  oracle and fails if an excused home is reached. It prints a receipt with the
  per-pair list.

Digests follow ``phase1_mutation_go``'s ``digest_rule`` per oracle. The Rust
side computes them; ``trace --verify-frames`` recomputes every row's digests in
Python (``phase1_mutation_go.row_digests``) from the frames the oracle emits.
"""

from __future__ import annotations

import argparse
from collections import defaultdict, deque
import contextlib
import gzip
import json
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import canonical  # noqa: E402
import phase1_mutation_go as go  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target/phase1-mutation"
DEFAULT_WS = TARGET / "ws"
DEFAULT_OUT = TARGET / "run"
DRIVER = "phase1_mutation_driver"
SYNTAX = "phase1_syntax"
# The Rust binary of each oracle.
PACKAGES = {"e1": DRIVER, "binder": DRIVER, "facts": DRIVER, "syntax": SYNTAX, "table": DRIVER}
# Oracles whose kill rows are re-run with their frames dumped.
DUMPING = ("binder", "facts", "syntax", "table")
# Oracles whose rows belong to columns: a kill credits only on a column whose
# every row matches native (column parity), and an operation a column claims
# has one home, never an excused one.
COLUMN_ORACLES = ("table",)
VERSION = 1
# Release semantics without fat LTO: one schemata build serves every mutant.
BUILD_ENV = {"CARGO_PROFILE_RELEASE_LTO": "false", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16"}
DEADLINE_FLOOR = 2.0
DEADLINE_FACTOR = 20
READY_TIMEOUT = 300.0
MEMORY_LIMIT_MB = 4096
MAX_KILLS = 3
# Timed-out or dead rows a mutant may skip before its job ends.
MAX_ABNORMAL = 3
# Mutant states, the merged state being the first that any oracle reports.
# `killed` holds on any credited kill. `budget`: candidate rows remained when
# the row budget (--max-rows) ran out. `not_run`: a traced oracle did not run
# the mutant (a --mutants subset, or a --skip-killed skip that the merged kills
# do not justify). Neither is evidence either way, unlike `not_reached`, which a
# campaign measured; both are non-final and make the results partial.
# `not_credited_multi_op`: every difference came from an oracle whose rows are
# whole programs, on a site of several operations (see WHOLE_PROGRAM_ORACLES).
# `control`: a control mutant, never run on its own and never credited.
STATE_ORDER = ("killed", "budget", "not_run", "not_credited_multi_op", "timeout", "crash", "survived", "not_reached",
               "control")
NON_FINAL_STATES = ("budget", "not_run")
# Oracles whose rows are whole programs (a syntax row loads its program and
# about sixty bundled library files), on which Go enters nearly every parser
# operation. A kill there cannot tell which operation of a site that carries
# markers for several operations made the difference, so such a site is never
# credited there.
WHOLE_PROGRAM_ORACLES = ("syntax",)
ONE_HOME_REASON = ("one home per table operation: a table column claims this operation, so a marked copy "
                   "that no traced oracle reaches is never excused; it must be reached and killed or lose "
                   "its marker")


def file_sha256(path):
    return go.file_sha256(path)


def write_gzip(path, document):
    """Deterministic gzip of one canonical JSON document."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".partial")
    temporary.write_bytes(gzip.compress(canonical(document) + b"\n", compresslevel=9, mtime=0))
    temporary.replace(path)


def read_gzip(path):
    return strict_json_loads(gzip.decompress(Path(path).read_bytes()))


def row_deadline(micros):
    return max(DEADLINE_FLOOR, DEADLINE_FACTOR * micros / 1e6)


def package(oracle):
    if oracle not in PACKAGES:
        raise ValueError(f"unknown oracle {oracle!r}; expected one of {sorted(PACKAGES)}")
    return PACKAGES[oracle]


def oracle_command(oracle, binary, mode, **options):
    """(argv, extra environment) of one oracle process in ``mode``.

    ``options`` become ``--name value`` pairs (underscores as hyphens).
    """
    if package(oracle) == SYNTAX:
        argv, env = [str(binary)], {"PHASE1_MUTATION": mode}
    else:
        argv, env = [str(binary), mode, "--oracle", oracle], {}
    for name, value in options.items():
        if value is not None:
            argv += [f"--{name.replace('_', '-')}", str(value)]
    return argv, env


def requests_file(oracle, requests=None):
    return Path(requests) if requests else go.requests_path(oracle)


def is_control(mutant):
    return mutant.get("operator") == "control" or "control_of" in mutant


def mutant_ops(mutant):
    return list(mutant.get("ops") or [mutant["op"]])


# ---------------------------------------------------------------------------
# builds and the workspace identity

def build_driver(ws, name=DRIVER, *, locked=False):
    """Builds one oracle binary in ``ws`` (release, no fat LTO) and returns it."""
    ws = Path(ws)
    args = ["cargo", "build", "--offline", "--release", "-p", name, "--bin", name, "--message-format=json"]
    if locked:
        args.insert(2, "--locked")
    completed = subprocess.run(args, cwd=ws, env={**os.environ, **BUILD_ENV}, stdout=subprocess.PIPE, check=True)
    binaries = []
    for line in completed.stdout.splitlines():
        if line.strip():
            item = json.loads(line)
            if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == name \
                    and item.get("executable"):
                binaries.append(item["executable"])
    if len(binaries) != 1:
        raise RuntimeError(f"cargo did not name exactly one {name} binary in {ws}")
    return Path(binaries[0])


def ws_identity(ws):
    """What a binary was built from: a spliced schemata copy (its marker names
    the plan and splice report) or a checkout (its source tree id)."""
    ws = Path(ws).resolve()
    import phase1_mutation_plan as plan_module
    marker = ws / plan_module.WS_MARKER
    if marker.is_file():
        document = strict_json_loads(marker.read_bytes())
        return {"kind": "schemata", "path": str(ws), "marker_sha256": file_sha256(marker),
                "plan_sha256": document.get("plan_sha256"), "root_tree": document.get("root_tree"),
                "splice_report_sha256": document.get("splice_report_sha256")}
    if (ws / ".git").exists():
        return {"kind": "checkout", "path": str(ws), "source_tree": plan_module.source_tree_sha(ws)}
    return {"kind": "unmarked", "path": str(ws)}


# ---------------------------------------------------------------------------
# digests recomputed from the frames an oracle emits

def frame_digests(oracle, frames):
    """One row's outcomes and compared digests, by ``phase1_mutation_go.row_digests``.

    E1 and binder frames are the example protocol's; a syntax row's frame is its
    schedule row; facts and table frames are driver session frames, turned into
    the row shape of the Go facts or table driver (outcomes plus the node list
    or the column value).
    """
    if oracle == "syntax":
        if len(frames) != 1:
            raise ValueError("a syntax row has exactly one frame")
        return go.row_digests("syntax", frames[0])
    if oracle == "table":
        spec = go.oracle_spec("table")
        outcomes, messages, record = {}, {}, {"row": frames[0].get("id")}
        for frame in frames:
            if frame["tag"] == "stage":
                outcomes[frame["stage"]] = frame["outcome"]
                if frame["outcome"] != "ok":
                    messages[frame["stage"]] = frame["message_hex"]
            elif frame["tag"] == "observation" and frame["stage"] == "column":
                if "value" in record:
                    raise ValueError("a table row observes one column value")
                record["value"] = frame["value"]
        record["outcomes"] = {stage: outcomes.get(stage, "not_run") for stage in spec.operations}
        if messages:
            record["messages"] = messages
        return go.row_digests("table", record)
    if oracle == "facts":
        spec = go.oracle_spec("facts")
        outcomes, messages, pairs = {}, {}, None
        for frame in frames:
            if frame["tag"] == "stage":
                outcomes[frame["stage"]] = frame["outcome"]
                if frame["outcome"] != "ok":
                    messages[frame["stage"]] = frame["message_hex"]
            elif frame["tag"] == "observation" and frame["stage"] == "subtree_facts":
                pairs = frame["value"]
        record = {"row": frames[0].get("id"), "outcomes": {stage: outcomes.get(stage, "not_run")
                                                           for stage in spec.operations}}
        if pairs is not None:
            record["list"] = pairs
        if messages:
            record["messages"] = messages
        return go.row_digests("facts", record)
    return go.row_digests(oracle, frames)


def read_frames(path):
    return [json.loads(line) for line in Path(path).read_bytes().splitlines() if line.strip()]


# ---------------------------------------------------------------------------
# table columns

def row_columns(requests):
    """{row id: column} of a materialized table request file."""
    columns = {}
    for line in Path(requests).read_bytes().splitlines():
        if line.strip():
            request = strict_json_loads(line)
            columns[request["id"]] = request["column"]
    return columns


def column_parity(column_of, rows, natives=None):
    """{column: {rows, base_match, mismatched}} over trace rows (``base_match``)
    or, with ``natives`` ({row: native row}), over rows compared here."""
    parity = {}
    for row in rows:
        column = column_of[row["row"]]
        entry = parity.setdefault(column, {"rows": 0, "base_match": 0, "mismatched": []})
        entry["rows"] += 1
        if natives is None:
            matched = row["base_match"]
        else:
            native = natives[row["row"]]
            matched = (not row.get("error") and row["outcomes"] == native["outcomes"]
                       and row["digests"] == native["digests"] and row.get("messages") == native.get("messages"))
        entry["base_match"] += bool(matched)
        if not matched and len(entry["mismatched"]) < 20:
            entry["mismatched"].append(row["row"])
    return dict(sorted(parity.items()))


def parity_gate(column_of, parity):
    """The column-parity rule as a kill gate: the reason a row cannot be credited, or None."""
    def gate(row):
        column = column_of[row]
        entry = parity[column]
        if entry["base_match"] != entry["rows"]:
            return (f"column {column} differs from native on {entry['rows'] - entry['base_match']} of its "
                    f"{entry['rows']} rows (first {entry['mismatched'][:3]}): column parity fails")
        return None
    return gate


def table_operations(columns):
    """The operations the table specs claim for these columns: they have one home each."""
    import phase1_tables
    claims = {column["id"]: column["operations"] for _, column in phase1_tables.columns(phase1_tables.load_specs())}
    return sorted({op for column in columns for op in claims.get(column, ())})


# ---------------------------------------------------------------------------
# trace

def _consume_frames(stream, oracle, verified, failures):
    """Digest an oracle's frame stream row by row with the Go side's rules."""
    try:
        frames = []
        for line in stream:
            record = json.loads(line)
            if oracle == "syntax":
                verified[record["id"]] = frame_digests(oracle, [record])
                continue
            frames.append(record)
            if record["tag"] == "end":
                verified[record["id"]] = frame_digests(oracle, frames)
                frames = []
        if frames:
            failures.append("frame stream ended inside a row")
    except Exception as error:  # reported by the caller  # noqa: BLE001
        failures.append(f"{type(error).__name__}: {error}")
    finally:
        for _ in stream:  # drain, so the oracle never blocks on a full pipe
            pass


def _split_requests(requests, shards, directory):
    """Contiguous shards of a request file, in order."""
    lines = [line for line in Path(requests).read_bytes().splitlines(keepends=True) if line.strip()]
    size = -(-len(lines) // shards)
    paths = []
    for index in range(shards):
        chunk = lines[index * size:(index + 1) * size]
        if chunk:
            path = Path(directory) / f"requests-shard-{index}.ndjson"
            path.write_bytes(b"".join(chunk))
            paths.append(path)
    return paths


def run_trace(binary, oracle, requests, out, *, verify_frames=False, shards=1):
    """Runs the oracle's trace and returns (rows, frame-verified row digests or None)."""
    out = Path(out)
    shard_requests = [Path(requests)] if shards <= 1 else _split_requests(requests, shards, out.parent)
    outputs = [out if len(shard_requests) == 1 else out.with_name(f"{out.name}.shard-{index}")
               for index in range(len(shard_requests))]
    verified = {} if verify_frames else None
    failures, running = [], []
    for shard, shard_out in zip(shard_requests, outputs):
        argv, env = oracle_command(oracle, binary, "trace", requests=shard, out=shard_out)
        environment = {**os.environ, **env}
        if not verify_frames:
            running.append((subprocess.Popen(argv, env=environment), None, argv))
            continue
        read_end, write_end = os.pipe()
        stream = os.fdopen(read_end, "rb", buffering=1 << 20)
        try:
            process = subprocess.Popen([*argv, "--frames", f"/dev/fd/{write_end}"], pass_fds=(write_end,),
                                       env=environment)
        finally:
            os.close(write_end)
        reader = threading.Thread(target=_consume_frames, args=(stream, oracle, verified, failures), daemon=True)
        reader.start()
        running.append((process, (reader, stream), argv))
    codes = []
    for process, reading, argv in running:
        codes.append((process.wait(), argv))
        if reading:
            reading[0].join()
            reading[1].close()
    for code, argv in codes:
        if code != 0:
            raise subprocess.CalledProcessError(code, argv)
    if failures:
        raise ValueError(f"frame verification failed: {failures[0]}")
    rows = []
    for shard_out in outputs:
        rows += [strict_json_loads(line) for line in shard_out.read_bytes().splitlines() if line.strip()]
        if shard_out != out:
            shard_out.unlink()
    for shard in shard_requests:
        if shard != Path(requests):
            shard.unlink()
    return rows, verified


def native_view(row):
    return {"outcomes": row["outcomes"], "digests": row["digests"], "messages": row.get("messages")}


def default_shards(oracle):
    return max(1, (os.cpu_count() or 4) - 2) if oracle == "syntax" else 1


def trace(oracle, ws=None, requests=None, native=None, out=None, *, driver=None, verify_frames=False, shards=None):
    """Base trace of every row with no mutant, checked against the native freeze."""
    spec = go.oracle_spec(oracle)
    ws = Path(ws) if ws else DEFAULT_WS
    requests = requests_file(oracle, requests)
    native = Path(native) if native else go.native_path(oracle)
    out = Path(out) if out else DEFAULT_OUT
    out.mkdir(parents=True, exist_ok=True)
    native_document = go.load_native(oracle, native)
    driver = Path(driver) if driver else build_driver(ws, package(oracle))
    started = time.monotonic()
    raw = out / f"trace-{oracle}.ndjson"
    rows, verified = run_trace(driver, oracle, requests, raw, verify_frames=verify_frames,
                               shards=default_shards(oracle) if shards is None else shards)
    seconds = round(time.monotonic() - started, 1)
    natives = native_document["rows"]
    if [(row["row"], row["request_sha256"]) for row in rows] != [(row["row"], row["request_sha256"]) for row in natives]:
        raise ValueError(f"{oracle}: traced rows differ from the native file's rows")
    frame_mismatches = []
    documents = []
    mismatched = []
    for row, expected in zip(rows, natives):
        observed = native_view(row)
        if verified is not None and not row.get("error") and (
                row["row"] not in verified or native_view(verified[row["row"]]) != observed):
            frame_mismatches.append(row["row"])
        base_match = observed == native_view(expected) and not row.get("error")
        eligible = base_match and all(value == "ok" for value in row["outcomes"].values())
        if not base_match:
            mismatched.append(row["row"])
        document = {"row": row["row"], "request_sha256": row["request_sha256"], "base_match": base_match,
                    "eligible": eligible, "outcomes": row["outcomes"], "digests": row["digests"],
                    "hits": row["hits"], "micros": row["micros"], "source_bytes": row["source_bytes"]}
        for field in ("messages", "error", "observe_hits"):
            if field in row:
                document[field] = row[field]
        documents.append(document)
    if frame_mismatches:
        raise ValueError(f"{oracle}: Rust digests differ from its own frames on {len(frame_mismatches)} rows, "
                         f"first {frame_mismatches[:3]}")
    hits = sorted({hit for row in documents if row["eligible"] for hit in row["hits"]})
    produced = {hit for row in documents for hit in row["hits"]}
    observed = {hit for row in documents for hit in row.get("observe_hits", ())}
    observe_only = sorted(observed - produced)
    document = {
        "version": VERSION, "oracle": oracle, "operations": list(spec.operations), "stages": list(spec.compared),
        "digest_rule": spec.digest_rule,
        "inputs": {"requests_sha256": native_document["requests_sha256"], "native_sha256": file_sha256(native),
                   "driver_sha256": file_sha256(driver), "driver_package": package(oracle), "ws": ws_identity(ws)},
        "summary": {"rows": len(documents), "base_match": len(documents) - len(mismatched),
                    "eligible": sum(row["eligible"] for row in documents), "mismatched_first": mismatched[:20],
                    "mutants_reached": len(hits), "mutants_observed": len(observed),
                    "observe_only_mutants": observe_only[:50], "observe_only_count": len(observe_only),
                    "seconds": seconds,
                    "frames_verified": None if verified is None else len(verified)},
        "rows": documents,
    }
    if mismatched:
        # Rows the base Rust build does not reproduce are never candidates;
        # each is a real defect, reported with the smallest examples first.
        native_by_row = {row["row"]: row for row in natives}
        smallest = sorted((row for row in documents if not row["base_match"]),
                          key=lambda row: (row["source_bytes"], row["row"]))[:20]
        document["summary"]["mismatched_smallest"] = [
            {"row": row["row"], "source_bytes": row["source_bytes"], "outcomes": row["outcomes"],
             "stages": [stage for stage in spec.compared
                        if row["digests"].get(stage) != native_by_row[row["row"]]["digests"].get(stage)]}
            for row in smallest]
    path = out / f"trace-{oracle}.json.gz"
    go.write_rows_document(path, document)
    raw.unlink(missing_ok=True)
    return {"oracle": oracle, "path": str(path), "file_sha256": file_sha256(path), **document["summary"]}


def compare_traces(left, right):
    """Rows whose outcomes, messages or digests differ between two traces."""
    left, right = read_gzip(left), read_gzip(right)
    if [row["row"] for row in left["rows"]] != [row["row"] for row in right["rows"]]:
        raise ValueError("traces cover different rows")
    differing = [a["row"] for a, b in zip(left["rows"], right["rows"])
                 if (a["outcomes"], a["digests"], a.get("messages")) != (b["outcomes"], b["digests"], b.get("messages"))]
    return {"rows": len(left["rows"]), "differing": len(differing), "first": differing[:10]}


# ---------------------------------------------------------------------------
# Go reach and candidate rows

def load_go_reach(path, trace_document):
    """({op: set(row index)}, unstable ops) from a go-reach file bound to the
    trace's native file and decoded under the current stage rule. Unstable
    operations are dropped: they are never candidates and never credited."""
    document = go.read_document(path)
    oracle = trace_document["oracle"]
    if document.get("version") != go.VERSION or document.get("oracle") != oracle:
        raise ValueError(f"{path}: wrong version or oracle")
    if document["native_sha256"] != trace_document["inputs"]["native_sha256"]:
        raise ValueError(f"{path}: bound to another native file than the trace")
    if document.get("row_encoding") != go.ROW_ENCODING:
        raise ValueError(f"{path}: unknown row encoding")
    if document.get("stage_rule") != go.stage_rule(oracle) or \
            document.get("stage_rule_sha256") != go.stage_rule_digest(oracle):
        raise ValueError(f"{path}: decoded under another stage rule than the current one; rerun reach")
    count = len(trace_document["rows"])
    if document["rows_checked"] != count:
        raise ValueError(f"{path}: checked {document['rows_checked']} rows, the trace has {count}")
    unstable = sorted(document.get("unstable_ops") or ())
    reach = {}
    for op, gaps in document["op_row_gaps"].items():
        indices = go.decode_gaps(gaps)
        if indices and indices[-1] >= count:
            raise ValueError(f"{path}: {op} names a row outside the native file")
        if op not in unstable:
            reach[op] = set(indices)
    return reach, unstable


def candidates(plan_document, trace_document, reach):
    """Per mutant id: (Rust-reached eligible rows, candidate rows smallest first)."""
    rows = trace_document["rows"]
    reached = defaultdict(list)
    for index, row in enumerate(rows):
        if row["eligible"]:
            for hit in row["hits"]:
                reached[hit].append(index)
    result = {}
    for mutant in plan_document["mutants"]:
        rust = reached.get(mutant["id"], [])
        go_rows = set().union(*(reach.get(op, set()) for op in mutant_ops(mutant)))
        chosen = sorted((index for index in rust if index in go_rows), key=lambda index: (rows[index]["source_bytes"], index))
        result[mutant["id"]] = (rust, chosen)
    return result


def reach_table(plan_document, trace_document):
    """Every planned mutant's Rust reach on this oracle: [eligible rows, all
    rows, observing rows], the first two counting rows whose production reached
    the site, the last rows on which the site executed while observing."""
    eligible, every, observing = defaultdict(int), defaultdict(int), defaultdict(int)
    for row in trace_document["rows"]:
        for hit in row["hits"]:
            every[hit] += 1
            eligible[hit] += row["eligible"]
        for hit in row.get("observe_hits", ()):
            observing[hit] += 1
    return {mutant["key"]: [eligible[mutant["id"]], every[mutant["id"]], observing[mutant["id"]]]
            for mutant in plan_document["mutants"]}


def mutant_jobs(mutant, candidate_rows, rows, reach, max_kills, max_rows=0):
    """The kill jobs of one mutant: one over its candidate rows, or, for a site
    that carries several operations, one per operation over the rows where Go
    entered that operation. A kill credits only the operations Go entered on its
    row, so stopping after ``max_kills`` differing rows of the union could leave
    an operation with candidate rows unprobed. ``max_rows`` (0: no limit) caps
    each job's rows."""
    ops = mutant_ops(mutant)
    if len(ops) == 1:
        groups = [candidate_rows] if candidate_rows else []
    else:
        groups = [[index for index in candidate_rows if index in reach.get(op, ())] for op in ops]
        # Operations Go entered on the same rows share one job: two identical
        # jobs would dump the same (mutant, row) files, and crediting the first
        # removes the second's.
        groups = [group for position, group in enumerate(groups) if group and group not in groups[:position]]
    jobs = []
    for group in groups:
        job = {"mutant": mutant["id"], "rows": [rows[index]["row"] for index in group[:max_rows or None]],
               "max_kills": max_kills}
        if mutant.get("control") is not None:
            job["control"] = mutant["control"]
        if max_rows and len(group) > max_rows:
            job["truncated"] = len(group) - max_rows
        jobs.append(job)
    return jobs


def kept_kills(kills, max_kills):
    """The recorded kills: up to ``max_kills`` per credited operation, in order."""
    if not max_kills:
        return kills
    kept, credited = [], defaultdict(int)
    for kill_row in kills:
        if any(credited[op] < max_kills for op in kill_row["ops"]):
            kept.append(kill_row)
            for op in kill_row["ops"]:
                credited[op] += 1
    return kept


# ---------------------------------------------------------------------------
# the supervisor

JOB_FIELDS = ("mutant", "rows", "max_kills", "control", "report_all", "dump_all", "recheck")


class Worker:
    """One oracle ``kill`` process fed one job at a time."""

    def __init__(self, index, argv, log_dir, selector, env=None):
        self.index, self.argv, self.selector = index, argv, selector
        self.env = {**os.environ, **env} if env else None
        self.log = open(Path(log_dir) / f"worker-{index}.stderr", "ab")
        self.process = None
        self.starts = 0

    def start(self):
        self.process = subprocess.Popen(self.argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                        env=self.env)
        os.set_blocking(self.process.stdout.fileno(), False)
        self.selector.register(self.process.stdout, selectors.EVENT_READ, self)
        self.buffer = b""
        self.ready = False
        self.job = None
        self.record = None
        self.row = None
        self.phase = None
        self.deadline = time.monotonic() + READY_TIMEOUT
        self.starts += 1

    def stop(self):
        if self.process is None:
            return
        self.selector.unregister(self.process.stdout)
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait()
        for stream in (self.process.stdin, self.process.stdout):
            try:
                stream.close()
            except OSError:
                pass
        self.process = None

    def close(self):
        self.stop()
        self.log.close()

    def restart(self):
        self.stop()
        self.start()

    def assign(self, index, job, record):
        self.job = (index, job)
        self.record = record
        self.row = None
        self.phase = None
        self.deadline = time.monotonic() + READY_TIMEOUT
        try:
            self.process.stdin.write(canonical({key: job[key] for key in JOB_FIELDS if key in job}) + b"\n")
            self.process.stdin.flush()
        except (BrokenPipeError, OSError):
            pass  # the process died; its EOF is handled as a crash

    def lines(self, chunk):
        self.buffer += chunk
        *complete, self.buffer = self.buffer.split(b"\n")
        return [line for line in complete if line.strip()]


def _returncode_reason(code):
    return f"signal {-code}" if code is not None and code < 0 else f"exit {code}"


def _memory(processes):
    """Resident KiB per pid, from one ps call."""
    pids = [str(process.pid) for process in processes if process is not None]
    if not pids:
        return {}
    completed = subprocess.run(["ps", "-o", "pid=,rss=", "-p", ",".join(pids)], stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, text=True)
    usage = {}
    for line in completed.stdout.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[0].isdigit() and fields[1].isdigit():
            usage[int(fields[0])] = int(fields[1])
    return usage


def _new_record(job):
    return {"status": None, "results": [], "rechecks": {}, "ran": 0, "done": None, "row": None, "reason": None,
            "worker": None, "timeouts": [], "deaths": [], "abnormal": [], "segments": 0,
            "max_kills": job.get("max_kills", MAX_KILLS)}


def run_jobs(argv, jobs, *, workers, deadline, log_dir, memory_limit_mb=MEMORY_LIMIT_MB, fresh=False, progress=None,
             env=None, max_abnormal=MAX_ABNORMAL):
    """Feeds ``jobs`` to ``workers`` oracle processes and returns one record per job.

    A row that passes its deadline (or the process's memory limit) is a
    timeout, a process that dies on a row is a death; either way the worker is
    restarted and the job continues with the rows after that one, until the
    mutant has had ``max_abnormal`` abnormal rows. A record's status is
    ``done`` when its row list was worked through (``timeouts``/``deaths`` list
    the skipped rows), else the abnormal status that ended it (``timeout`` or
    ``crash``). A worker is also restarted after any job with crashing rows or a
    failed base recheck, and after every job when ``fresh``.
    """
    Path(log_dir).mkdir(parents=True, exist_ok=True)
    queue = deque((index, job, _new_record(job)) for index, job in enumerate(jobs))
    records = [None] * len(jobs)
    attempts = defaultdict(int)
    abnormal_rows = defaultdict(int)
    selector = selectors.DefaultSelector()
    pool = [Worker(index, argv, log_dir, selector, env) for index in range(max(1, min(workers, len(jobs))))]
    finished = 0
    last_memory = 0.0
    try:
        for worker in pool:
            worker.start()

        def complete(index, job, record, status, row=None, reason=None):
            nonlocal finished
            record["status"], record["row"], record["reason"] = status, row, reason
            records[index] = record
            finished += 1
            if progress:
                progress(finished, len(jobs), jobs[index], record)

        def release(worker):
            job = worker.job
            worker.job = worker.record = worker.row = worker.phase = None
            worker.deadline = None
            return job

        def settle(worker, status, reason=None):
            """One job segment ended: normally (``done``) or on a row's timeout or death."""
            record, row, phase = worker.record, worker.row, worker.phase
            index, job = release(worker)
            if status == "done":
                complete(index, job, record, "done")
                tainted = any(result.get("crash") for result in record["results"]) or \
                    not all(record["rechecks"].values())
                if fresh or tainted:
                    worker.restart()
                return
            worker.restart()
            record["abnormal"].append({"row": row, "status": status, "reason": reason, "phase": phase})
            (record["timeouts"] if status == "timeout" else record["deaths"]).append(row)
            abnormal_rows[job["mutant"]] += 1
            rows = job["rows"]
            rest = rows[rows.index(row) + 1:] if phase != "recheck" and row in rows else []
            kills = sum(1 for result in record["results"] if result.get("kill"))
            left = 0 if not record["max_kills"] else record["max_kills"] - kills
            if row is None or phase == "recheck" or abnormal_rows[job["mutant"]] >= max_abnormal:
                complete(index, job, record, status, row, reason)
            elif not rest or (record["max_kills"] and left <= 0):
                complete(index, job, record, "done")
            else:
                queue.appendleft((index, {**job, "rows": rest, "max_kills": left}, record))

        def died(worker):
            code = worker.process.poll()
            if code is None:
                worker.process.wait()
                code = worker.process.returncode
            if worker.job is None:
                if not worker.ready:
                    raise RuntimeError(f"oracle worker {worker.index} exited during startup ({_returncode_reason(code)}); "
                                       f"see {log_dir}/worker-{worker.index}.stderr")
                worker.restart()
                return
            index, job = worker.job
            if worker.row is None and attempts[index] < 2:
                # No row of this segment started: the segment goes back to the queue.
                attempts[index] += 1
                record = worker.record
                release(worker)
                queue.appendleft((index, job, record))
                worker.restart()
                return
            settle(worker, "crash", _returncode_reason(code))

        while queue or any(worker.job is not None for worker in pool):
            for worker in pool:
                if worker.ready and worker.job is None and queue:
                    index, job, record = queue.popleft()
                    record["worker"] = worker.index
                    record["segments"] += 1
                    worker.assign(index, job, record)
            now = time.monotonic()
            waits = [worker.deadline for worker in pool if worker.deadline is not None]
            timeout = min(1.0, max(0.0, min(waits) - now)) if waits else 1.0
            for key, _ in selector.select(timeout=timeout):
                worker = key.data
                if worker.process is None or key.fd != worker.process.stdout.fileno():
                    continue
                try:
                    chunk = os.read(key.fd, 1 << 16)
                except BlockingIOError:
                    continue
                if not chunk:
                    died(worker)
                    continue
                for line in worker.lines(chunk):
                    message = json.loads(line)
                    now = time.monotonic()
                    if message.get("ready"):
                        worker.ready = True
                        worker.deadline = None
                    elif worker.job is None:
                        raise RuntimeError(f"oracle worker {worker.index} spoke with no job: {line[:200]!r}")
                    elif "heartbeat" in message:
                        worker.row = message["heartbeat"]
                        worker.phase = ("recheck" if message.get("recheck") else "control" if "control" in message
                                        else "dump" if message.get("dump") else "row")
                        worker.deadline = now + deadline(worker.row)
                        if worker.phase == "row":
                            worker.record["ran"] += 1
                    elif "recheck" in message:
                        worker.record["rechecks"][message["recheck"]] = message["base_ok"]
                    elif message.get("done"):
                        worker.record["done"] = message
                        settle(worker, "done")
                        break
                    elif "error" in message and "differs" not in message:
                        raise RuntimeError(f"oracle rejected a job row: {message}")
                    else:
                        worker.record["results"].append(message)
            now = time.monotonic()
            for worker in pool:
                if worker.deadline is not None and now > worker.deadline:
                    if worker.job is None:
                        raise RuntimeError(f"oracle worker {worker.index} did not become ready")
                    settle(worker, "timeout", "deadline")
            if memory_limit_mb and now - last_memory >= 1.0:
                last_memory = now
                usage = _memory([worker.process for worker in pool if worker.job is not None])
                for worker in pool:
                    if worker.job is not None and usage.get(worker.process.pid, 0) > memory_limit_mb * 1024:
                        settle(worker, "timeout", "memory_limit")
    finally:
        for worker in pool:
            worker.close()
        selector.close()
    return records


# ---------------------------------------------------------------------------
# kill

def confirm_dump(oracle, dump, native_row, reported):
    """Digest a dumped row's frames with the Go side's rules and compare with native."""
    observed = frame_digests(oracle, read_frames(dump))
    all_ok = all(value == "ok" for value in observed["outcomes"].values())
    stages = [stage for stage in go.oracle_spec(oracle).compared
              if observed["digests"].get(stage) != native_row["digests"].get(stage)]
    consistent = observed["outcomes"] == reported["outcomes"] and observed["digests"] == reported["digests"]
    return {"ok": all_ok and observed["outcomes"] == native_row["outcomes"] and bool(stages) and consistent,
            "stages": stages, "consistent": consistent, "digests": observed["digests"],
            "outcomes": observed["outcomes"]}


def write_base(path, rows):
    """An oracle's --base file: row, request_sha256, outcomes, digests."""
    with Path(path).open("wb") as stream:
        for row in rows:
            stream.write(canonical({"row": row["row"], "request_sha256": row["request_sha256"],
                                    "outcomes": row["outcomes"], "digests": row["digests"]}) + b"\n")


def _dumps(result):
    return [path for path in (result.get("dump"), (result.get("control") or {}).get("dump")) if path]


def _kill_entry(mutant, record, rows_by_id, natives, reach, oracle, keep_dumps, gate=None):
    """The credited kills and notes of one job record."""
    kills, notes, crash_rows = [], [], []
    for result in record["results"]:
        try:
            kill_row, note = _credit(mutant, record, result, rows_by_id, natives, reach, oracle, gate)
        finally:
            if not keep_dumps:
                for path in _dumps(result):
                    Path(path).unlink(missing_ok=True)
        if result["crash"] and len(crash_rows) < 3:
            crash_rows.append({key: result[key] for key in ("row", "outcomes", "messages", "error") if key in result})
        if note:
            notes.append({"row": result["row"], "note": note})
        if kill_row:
            kills.append(kill_row)
    return kills, notes, crash_rows


def _credit(mutant, record, result, rows_by_id, natives, reach, oracle, gate=None):
    """(kill, None) when one reported row is a creditable kill, else (None, note or None).

    ``gate`` (the table oracle's column parity) names why a row cannot be
    credited at all.
    """
    row = result["row"]
    if result["crash"] or not result["differs"]:
        return None, None
    if gate is not None and (reason := gate(row)):
        return None, reason
    control = result.get("control")
    if mutant.get("control") is not None and control is None:
        return None, "no control run for an allocating mutant"
    if not result.get("kill"):
        if control is not None:
            return None, "the control crashed" if control["crash"] else "the control reproduces the difference"
        return None, "not a kill"
    if result.get("nondeterministic"):
        return None, "mutant row differed between two runs"
    recheck = record["rechecks"].get(row)
    if recheck is not True:
        return None, "base recheck failed" if recheck is False else "no base recheck"
    index, trace_row = rows_by_id[row]
    native_row = natives[index]
    stages = [stage for stage in result["differs"] if result["digests"].get(stage) != native_row["digests"].get(stage)]
    mutant_digests = result["digests"]
    if oracle in DUMPING:
        if not result.get("dump"):
            return None, "kill without frames"
        confirmation = confirm_dump(oracle, result["dump"], native_row, result)
        if not confirmation["ok"]:
            return None, ("normalized frames equal native" if confirmation["consistent"]
                          else "dumped frames disagree with the reported digests")
        # The recorded mutant digests are the evidence comparator's, from the frames.
        stages, mutant_digests = confirmation["stages"], confirmation["digests"]
    if not stages:
        return None, "no stage differs from native"
    compared = go.oracle_spec(oracle).compared
    control_digests, control_stages = None, None
    if control is not None:
        control_digests = control["digests"]
        if oracle in DUMPING:
            if not control.get("dump"):
                return None, "control without frames"
            observed = frame_digests(oracle, read_frames(control["dump"]))
            if observed["outcomes"] != control["outcomes"] or observed["digests"] != control["digests"]:
                return None, "dumped control frames disagree with the reported digests"
            control_digests = observed["digests"]
        if control["crash"] or control["outcomes"] != native_row["outcomes"]:
            return None, "the control crashed"
        # Only a stage where the mutant differs from native counts: a control
        # that differs from the mutant elsewhere shows the replacement's side
        # effects, not its value, moving a digest.
        control_stages = [stage for stage in stages if mutant_digests.get(stage) != control_digests.get(stage)]
        if not control_stages:
            return None, "the control reproduces the difference"
    credited = [op for op in mutant_ops(mutant) if index in reach.get(op, set())]
    kill = {"row": row, "request_sha256": trace_row["request_sha256"], "stages": stages,
            "native": {stage: native_row["digests"].get(stage) for stage in compared},
            "base": {stage: trace_row["digests"].get(stage) for stage in compared},
            "mutant": {stage: mutant_digests.get(stage) for stage in compared},
            "control": None if control_digests is None else {stage: control_digests.get(stage) for stage in compared},
            "ops": credited, "reach": {"rust": mutant["id"] in trace_row["hits"], "go": credited}}
    if control is not None:
        kill["control_id"] = control["id"]
        kill["control_stages"] = control_stages
    return kill, None


def _state(kills, records, candidates_count, truncated=False, not_credited=()):
    """A mutant's state on one oracle. A truncated mutant is ``budget`` unless
    it was killed or ended abnormally (its abnormal-row limit), because
    candidate rows it never ran remain."""
    if kills:
        return "killed"
    if not candidates_count:
        return "not_reached"
    if not_credited:
        return "not_credited_multi_op"
    if truncated and not any(record["status"] in ("timeout", "crash") for record in records):
        return "budget"
    if any(record["timeouts"] or record["status"] == "timeout" for record in records):
        return "timeout"
    if any(record["deaths"] or record["status"] == "crash" for record in records) or \
            any(result["crash"] for record in records for result in record["results"]):
        return "crash"
    return "survived"


def skip_source(path):
    """(killed keys, settings) of ``--skip-killed``: the killed mutants of a
    results or kill file, and what identifies it."""
    path = Path(path)
    document = read_gzip(path)
    keys = {mutant["key"] for mutant in document["mutants"] if mutant["state"] == "killed"}
    return keys, {"source": str(path), "sha256": file_sha256(path), "killed": len(keys)}


def kill(oracle, ws=None, plan=None, trace=None, go_reach=None, jobs=None, out=None, *, requests=None, native=None,
         driver=None, mutants=None, skip_killed=None, max_kills=MAX_KILLS, max_rows=0,
         memory_limit_mb=MEMORY_LIMIT_MB, keep_dumps=False, progress=None):
    """The kill campaign of one oracle over the planned mutants.

    ``mutants`` restricts it to those ids; ``skip_killed`` (a results or kill
    file) skips the mutants that file records as killed. The kill file records
    both (``settings``) and lists the skipped keys (``skipped``), so ``results``
    can tell a justified skip from a mutant this oracle never ran.
    """
    ws = Path(ws) if ws else DEFAULT_WS
    out = Path(out) if out else DEFAULT_OUT
    plan = Path(plan) if plan else TARGET / "plan.json"
    trace = Path(trace) if trace else out / f"trace-{oracle}.json.gz"
    go_reach = Path(go_reach) if go_reach else go.reach_path(oracle)
    requests = requests_file(oracle, requests)
    native = Path(native) if native else go.native_path(oracle)
    jobs = jobs or max(1, (os.cpu_count() or 4) - 2)
    plan_document = strict_json_loads(plan.read_bytes())
    trace_document = go.read_document(trace)
    if trace_document["oracle"] != oracle:
        raise ValueError(f"{trace} is a {trace_document['oracle']} trace")
    if trace_document["inputs"]["native_sha256"] != file_sha256(native):
        raise ValueError(f"{trace} was checked against another native file than {native}")
    native_document = go.load_native(oracle, native)
    driver = Path(driver) if driver else build_driver(ws, package(oracle))
    driver_sha256 = file_sha256(driver)
    if trace_document["inputs"]["driver_sha256"] != driver_sha256:
        raise ValueError("the trace was recorded with another oracle build; re-run trace on this workspace")
    traced_plan = trace_document["inputs"]["ws"].get("plan_sha256")
    if traced_plan is not None and traced_plan != file_sha256(plan):
        raise ValueError("the traced workspace was spliced from another plan")
    reach, unstable = load_go_reach(go_reach, trace_document)
    rows = trace_document["rows"]
    rows_by_id = {row["row"]: (index, row) for index, row in enumerate(rows)}
    natives = native_document["rows"]
    parity, gate, one_home = None, None, None
    if oracle in COLUMN_ORACLES:
        column_of = row_columns(requests)
        parity = column_parity(column_of, rows)
        gate = parity_gate(column_of, parity)
        one_home = table_operations(parity)
    selected = [mutant for mutant in plan_document["mutants"]
                if not is_control(mutant) and (mutants is None or mutant["id"] in mutants)]
    skip_keys, skip_settings = skip_source(skip_killed) if skip_killed else (set(), None)
    skipped = sorted(mutant["key"] for mutant in selected if mutant["key"] in skip_keys)
    selected = [mutant for mutant in selected if mutant["key"] not in skip_keys]
    chosen = candidates({"mutants": selected}, trace_document, reach)
    work = [job for mutant in selected
            for job in mutant_jobs(mutant, chosen[mutant["id"]][1], rows, reach, max_kills, max_rows)]
    truncated = {job["mutant"] for job in work if job.get("truncated")}
    out.mkdir(parents=True, exist_ok=True)
    base = out / f"base-{oracle}.ndjson"
    write_base(base, [row for row in rows if row["eligible"]])
    dumps = out / f"dumps-{oracle}"
    dumps.mkdir(exist_ok=True)
    argv, env = oracle_command(oracle, driver, "kill", requests=requests, base=base, dump_dir=dumps)
    micros = {row["row"]: row["micros"] for row in rows}

    def deadline(row):
        return row_deadline(micros.get(row, 0))

    started = time.monotonic()
    common = {"workers": jobs, "deadline": deadline, "log_dir": out / f"logs-{oracle}", "memory_limit_mb": memory_limit_mb,
              "progress": progress, "env": env}
    records = run_jobs(argv, work, **common)
    by_mutant = defaultdict(list)
    retry = []
    for job, record in zip(work, records):
        by_mutant[job["mutant"]].append(record)
        failed_recheck = not all(record["rechecks"].values())
        unverified = [result["row"] for result in record["results"]
                      if result.get("kill") and result["row"] not in record["rechecks"]]
        if failed_recheck:
            retry.append({key: value for key, value in job.items() if key != "truncated"})
        elif unverified:
            retry.append({**{key: value for key, value in job.items() if key != "truncated"},
                          "rows": unverified, "max_kills": len(unverified)})
    if retry:
        # Kills whose base recheck failed or never ran are redone in fresh processes.
        # A retry dumps the same (mutant, row) under the same file name, so it
        # gets its own directory: crediting the first attempt removes its dumps.
        retry_dumps = out / f"dumps-{oracle}-retry"
        retry_dumps.mkdir(exist_ok=True)
        retry_argv, _ = oracle_command(oracle, driver, "kill", requests=requests, base=base, dump_dir=retry_dumps)
        for job, record in zip(retry, run_jobs(retry_argv, retry, **common, fresh=True)):
            record["retry"] = True
            by_mutant[job["mutant"]].append(record)
    entries = []
    counts = defaultdict(int)
    for mutant in selected:
        rust, candidate_rows = chosen[mutant["id"]]
        mutant_records = by_mutant.get(mutant["id"], [])
        kills, notes, crash_rows = [], [], []
        for record in mutant_records:
            found, record_notes, record_crashes = _kill_entry(mutant, record, rows_by_id, natives, reach, oracle,
                                                              keep_dumps, gate)
            kills += [kill_row for kill_row in found if kill_row["row"] not in {row["row"] for row in kills}]
            notes += record_notes
            crash_rows += record_crashes
        not_credited = []
        if kills and oracle in WHOLE_PROGRAM_ORACLES and len(mutant_ops(mutant)) > 1:
            # Kept visible, never credited: see WHOLE_PROGRAM_ORACLES.
            not_credited, kills = kills, []
            notes.append({"row": None, "note": f"{oracle} rows are whole programs; a site of several "
                                               "operations is never credited there"})
        state = _state(kills, mutant_records, len(candidate_rows), mutant["id"] in truncated, not_credited)
        counts[state] += 1
        abnormal = [event for record in mutant_records for event in record["abnormal"]]
        entry = {"id": mutant["id"], "key": mutant["key"], "op": mutant["op"], "ops": mutant_ops(mutant),
                 "file": mutant["file"], "function": mutant["function"], "state": state, "rust_rows": len(rust),
                 "candidates": len(candidate_rows), "ran": sum(record["ran"] for record in mutant_records),
                 "crashes": sum(result["crash"] for record in mutant_records for result in record["results"]),
                 "timeouts": sorted({event["row"] for event in abnormal if event["status"] == "timeout"}),
                 "deaths": sorted({event["row"] for event in abnormal if event["status"] == "crash"}),
                 "control": mutant.get("control"), "kills": kept_kills(kills, max_kills)}
        if not_credited:
            entry["not_credited"] = [{**kill_row, "reason": "not_credited_multi_op"}
                                     for kill_row in kept_kills(not_credited, max_kills)]
        if not candidate_rows:
            entry["reason"] = "no Rust reach on an eligible row" if not rust else "Go never entered the operation on a reached row"
        if mutant["id"] in truncated:
            entry["truncated"] = True
        if abnormal:
            entry["failure"] = {key: abnormal[0][key] for key in ("status", "row", "reason")}
        if crash_rows:
            entry["crash_rows"] = crash_rows[:3]
        if notes:
            entry["notes"] = notes
        entries.append(entry)
    document = {
        "version": VERSION, "oracle": oracle,
        "inputs": {"plan_sha256": file_sha256(plan), "trace_sha256": file_sha256(trace),
                   "native_sha256": file_sha256(native), "go_reach_sha256": file_sha256(go_reach),
                   "driver_sha256": driver_sha256, "driver_package": package(oracle),
                   "requests_sha256": trace_document["inputs"]["requests_sha256"], "ws": trace_document["inputs"]["ws"]},
        "settings": {"max_kills": max_kills, "max_rows": max_rows,
                     "deadline": {"floor_seconds": DEADLINE_FLOOR, "base_factor": DEADLINE_FACTOR},
                     "max_abnormal_rows": MAX_ABNORMAL, "memory_limit_mb": memory_limit_mb, "workers": jobs,
                     "retried_jobs": len(retry), "mutants": None if mutants is None else sorted(mutants),
                     "skip_killed": None if skip_settings is None else {**skip_settings, "skipped": len(skipped)}},
        "unstable_ops": unstable,
        # Planned mutants --skip-killed left out; results require each to be killed elsewhere.
        "skipped": skipped,
        # Every planned mutant's Rust reach on this oracle, [eligible rows, all rows,
        # observing rows], so results can tell a measured-unexecuted home from an
        # unmeasured one.
        "reach": reach_table(plan_document, trace_document),
        "summary": {"mutants": len(entries), **dict(sorted(counts.items())), "skipped": len(skipped),
                    "seconds": round(time.monotonic() - started, 1)},
        "mutants": entries,
    }
    if parity is not None:
        # Column parity of the base trace (every kill on a column that is not
        # at parity was refused) and the operations with one home each.
        document["columns"] = parity
        document["one_home_operations"] = one_home
        document["summary"]["columns_diverging"] = sorted(column for column, entry in parity.items()
                                                          if entry["base_match"] != entry["rows"])
    path = out / f"kill-{oracle}.json.gz"
    write_gzip(path, document)
    return {"oracle": oracle, "path": str(path), "file_sha256": file_sha256(path), **document["summary"]}


# ---------------------------------------------------------------------------
# results

def plan_homes(plan_document):
    """{op: [home]}: the plan's ``homes`` (every marker site), or, for a plan
    without them, one home per function of the op's mutants and unsupported
    entries."""
    if "homes" in plan_document:
        return {op: [dict(home) for home in homes] for op, homes in plan_document["homes"].items()}
    homes = defaultdict(dict)
    for mutant in plan_document["mutants"]:
        if is_control(mutant):
            continue
        for op in mutant_ops(mutant):
            home = homes[op].setdefault((mutant["file"], mutant["function"]), {
                "file": mutant["file"], "function": mutant["function"], "site_kind": None, "site_line": None,
                "span_sha256": None, "mutants": []})
            home["mutants"].append(mutant["key"])
    for entry in plan_document.get("unsupported", ()):
        if entry.get("file"):
            homes[entry["op"]].setdefault((entry["file"], entry.get("function")), {
                "file": entry["file"], "function": entry.get("function"), "site_kind": None, "site_line": None,
                "span_sha256": None, "mutants": []})
    return {op: list(found.values()) for op, found in homes.items()}


def home_name(home):
    site = f"@{home['site_kind']}:{home['site_line']}" if home.get("site_line") is not None else ""
    return f"{home['file']}::{home['function']}{site}"


def _op_state(homes, states):
    """An operation's state from its evaluated homes (``states``: key -> mutant state)."""
    if not homes:
        return "unsupported"
    pending = [home for home in homes if not home["mutants"]]
    killed = [home for home in homes if home["killed"]]
    open_homes = [home for home in homes if home["mutants"] and not home["killed"] and not home["excused"]]
    if killed and not pending and not open_homes:
        return "killed"
    if killed:
        return "partial"
    reached = [home for home in homes if home["reached"]]
    if reached:
        # A mutant killed only on rows where Go never entered this operation
        # credits another operation of its function, not this one.
        found = {"survived" if states[key] == "killed" else states[key] for home in reached for key in home["mutants"]}
        return next((state for state in STATE_ORDER[1:] if state in found), "not_reached")
    probed = [home for home in homes if home["mutants"]]
    if probed and all(states[key] == "not_run" for home in probed for key in home["mutants"]):
        return "not_run"
    if probed:
        return "not_reached"
    return "unsupported"


def results(plan, kill_files, out=None):
    """Merge per-oracle kill files into results.json.gz (mutants, homes and operations)."""
    plan = Path(plan)
    plan_document = strict_json_loads(plan.read_bytes())
    plan_sha256 = file_sha256(plan)
    campaigns = {}
    for path in kill_files:
        document = read_gzip(path)
        if document["inputs"]["plan_sha256"] != plan_sha256:
            raise ValueError(f"{path} ran another plan")
        if document["oracle"] in campaigns:
            raise ValueError(f"two kill files for {document['oracle']}")
        if "reach" not in document:
            raise ValueError(f"{path} has no reach table; re-run kill")
        campaigns[document["oracle"]] = (Path(path), document)
    traced = sorted(campaigns)
    # Column parity per column oracle, and the operations that have one home
    # each (a table column claims them): no home of theirs is ever excused.
    columns = {oracle: document["columns"] for oracle, (_, document) in sorted(campaigns.items())
               if "columns" in document}
    one_home = sorted({op for _, document in campaigns.values() for op in document.get("one_home_operations") or ()})
    per_mutant = defaultdict(dict)
    skipped_by = defaultdict(dict)
    for oracle, (_, document) in campaigns.items():
        for entry in document["mutants"]:
            per_mutant[entry["key"]][oracle] = entry
        skip = (document.get("settings") or {}).get("skip_killed")
        for key in document.get("skipped") or ():
            skipped_by[key][oracle] = skip["sha256"] if skip else None
    mutants = []
    for mutant in plan_document["mutants"]:
        runs = per_mutant.get(mutant["key"], {})
        kills = [{"oracle": oracle, **kill_row} for oracle, entry in sorted(runs.items()) for kill_row in entry["kills"]]
        # A traced oracle that did not run the mutant (skipped or not selected)
        # measured nothing; only a kill elsewhere makes that final.
        states = [runs[oracle]["state"] if oracle in runs else "not_run" for oracle in traced]
        if is_control(mutant):
            state = "control"
        else:
            state = "killed" if kills else next((state for state in STATE_ORDER if state in states), "not_run")
        record = {"id": mutant["id"], "key": mutant["key"], "op": mutant["op"], "ops": mutant_ops(mutant),
                  "file": mutant["file"], "function": mutant["function"], "span_sha256": mutant["span_sha256"],
                  "operator": mutant["operator"], "state": state,
                  "candidates": sum(entry["candidates"] for entry in runs.values()),
                  "ran": sum(entry["ran"] for entry in runs.values()), "kills": kills,
                  "reach": {oracle: document["reach"].get(mutant["key"])
                            for oracle, (_, document) in sorted(campaigns.items())},
                  "oracles": {oracle: {key: entry[key] for key in ("state", "rust_rows", "candidates", "ran", "crashes",
                                                                   "timeouts", "deaths") if key in entry}
                              for oracle, entry in sorted(runs.items())}}
        not_credited = [{"oracle": oracle, **kill_row} for oracle, entry in sorted(runs.items())
                        for kill_row in entry.get("not_credited") or ()]
        if not_credited:
            record["not_credited"] = not_credited
        if skipped_by.get(mutant["key"]):
            record["skipped_by"] = dict(sorted(skipped_by[mutant["key"]].items()))
        for field in ("control", "control_of"):
            if mutant.get(field) is not None:
                record[field] = mutant[field]
        mutants.append(record)
    by_key = {record["key"]: record for record in mutants}
    states = {key: record["state"] for key, record in by_key.items()}
    homes_by_op = plan_homes(plan_document)
    all_ops = set(homes_by_op) | {op for record in mutants if record["state"] != "control" for op in record["ops"]} \
        | {entry["op"] for entry in plan_document.get("unsupported", ())}
    operations, summary = {}, defaultdict(int)
    reason = f"no production or observation reach on {', '.join(traced)}"
    for op in sorted(all_ops):
        evaluated = []
        for home in homes_by_op.get(op, []):
            keys = [key for key in home["mutants"] if key in by_key and by_key[key]["state"] != "control"]
            reaches = {oracle: [by_key[key]["reach"].get(oracle) for key in keys] for oracle in traced}
            # Reach is measured by a full reach table entry, [eligible, all, observing] rows.
            measured = [oracle for oracle in traced
                        if keys and all(isinstance(entry, list) and len(entry) == 3 for entry in reaches[oracle])]
            reached = any(entry and entry[1] > 0 for oracle in traced for entry in reaches[oracle])
            observed = any(isinstance(entry, list) and len(entry) == 3 and entry[2] > 0
                           for oracle in traced for entry in reaches[oracle])
            killed = any(op in kill_row["ops"] for key in keys for kill_row in by_key[key]["kills"])
            excusable = (bool(keys) and bool(traced) and measured == traced and not reached and not observed
                         and not killed)
            excused = excusable and op not in one_home
            why = {"reason": reason} if excused else {"reason": ONE_HOME_REASON} if excusable else {}
            evaluated.append({**{field: home.get(field) for field in ("file", "function", "site_kind", "site_line",
                                                                      "span_sha256")},
                              "mutants": keys, "reached": reached, "observed": observed, "killed": killed,
                              "measured": measured, "excused": excused, **why})
        state = _op_state(evaluated, states)
        summary[state] += 1
        operations[op] = {
            "state": state, "mutants": sorted(key for home in evaluated for key in home["mutants"]),
            "homes": evaluated,
            "homes_reached": sorted(home_name(home) for home in evaluated if home["reached"]),
            "homes_observed": sorted(home_name(home) for home in evaluated if home["observed"]),
            "homes_killed": sorted(home_name(home) for home in evaluated if home["killed"]),
            "homes_unprobed": sorted(home_name(home) for home in evaluated if not home["mutants"]),
            "homes_excused": sorted(home_name(home) for home in evaluated if home["excused"]),
        }
    document = {
        "version": VERSION,
        "partial": any(mutant["state"] in NON_FINAL_STATES for mutant in mutants),
        "inputs": {"plan_sha256": plan_sha256, "plan_root_tree": plan_document.get("root_tree"),
                   "traced_oracles": traced,
                   "oracles": {oracle: {"kill_sha256": file_sha256(path), **document["inputs"],
                                        "skip_killed": (document.get("settings") or {}).get("skip_killed")}
                               for oracle, (path, document) in sorted(campaigns.items())}},
        "summary": {"mutants": {state: sum(mutant["state"] == state for mutant in mutants) for state in STATE_ORDER},
                    "operations": dict(sorted(summary.items()))},
        "mutants": mutants,
        "unsupported": plan_document["unsupported"],
        "operations": operations,
    }
    if columns:
        document["columns"] = columns
        document["one_home_operations"] = one_home
    out = Path(out) if out else DEFAULT_OUT / "results.json.gz"
    write_gzip(out, document)
    return {"path": str(out), "file_sha256": file_sha256(out), **document["summary"]}


# ---------------------------------------------------------------------------
# confirm

@contextlib.contextmanager
def _stdout_to_stderr():
    """Children (cargo, the splicer) must not write into a receipt on stdout."""
    sys.stdout.flush()
    saved = os.dup(1)
    os.dup2(2, 1)
    try:
        yield
    finally:
        sys.stdout.flush()
        os.dup2(saved, 1)
        os.close(saved)


def _replayed(oracle, result, native_row):
    """(outcomes ok and native-like, digests) of one replayed row, from its
    frames for dumping oracles."""
    if result is None or result.get("crash"):
        return False, None
    digests = result["digests"]
    if oracle in DUMPING:
        if not result.get("dump"):
            return False, None
        observed = frame_digests(oracle, read_frames(result["dump"]))
        if observed["outcomes"] != result["outcomes"] or observed["digests"] != digests:
            return False, None
        digests = observed["digests"]
    return result["outcomes"] == native_row["outcomes"], digests


def _lost_state(record, row):
    """Why a replayed row has no usable result."""
    if row in record["timeouts"]:
        return "timeout"
    if row in record["deaths"]:
        return "crash"
    return record["status"] if record["status"] in ("timeout", "crash") else "crash"


def confirm(results_path, plan=None, *, root=ROOT, ws=None, drivers=None, requests=None, natives=None, jobs=None,
            out=None, retrace=True, splice_ws=None):
    """Replay every recorded kill pair, its control and its base row on a fresh
    splice of the full manifest, and re-trace every oracle.

    Each pair must reproduce exactly its recorded stages and mutant digests
    (and its control's digests, with a stage where the mutant differs from
    native and from its control); each base row must equal native; no mutant of
    an excused home may be executed, while producing or while observing, on any
    row of any traced oracle; and on a column oracle every row of a column with
    a recorded kill must still equal native (column parity). A mutant whose span changed since
    planning cannot be spliced: its pairs are ``stale`` failures, and a stale
    excused home fails the receipt too. Returns the receipt, with the per-pair
    list ``pairs``.
    """
    results_path = Path(results_path)
    plan = Path(plan) if plan else TARGET / "plan.json"
    out = Path(out) if out else DEFAULT_OUT / "confirm"
    out.mkdir(parents=True, exist_ok=True)
    document = read_gzip(results_path)
    plan_document = strict_json_loads(plan.read_bytes())
    if document["inputs"]["plan_sha256"] != file_sha256(plan):
        raise ValueError("the results were produced from another plan")
    planned = {mutant["key"]: mutant for mutant in plan_document["mutants"]}
    killed = [mutant for mutant in document["mutants"] if mutant["state"] == "killed"]
    for mutant in killed:
        if planned.get(mutant["key"], {}).get("id") != mutant["id"]:
            raise ValueError(f"mutant {mutant['key']} is not planned under id {mutant['id']}")
    oracles = sorted(document["inputs"].get("traced_oracles") or document["inputs"]["oracles"])
    excused = sorted({key for op in document["operations"].values() for home in op.get("homes", ())
                      if home.get("excused") for key in home["mutants"]})
    stale = set()
    drivers = dict(drivers or {})
    with _stdout_to_stderr():
        if ws is None:
            import phase1_mutation_plan as plan_module
            stale = {key for key, mutant in planned.items() if not plan_module.span_intact(root, mutant)}
            splice = plan
            if stale:
                # Every other mutant keeps its campaign id; stale ones cannot be spliced.
                splice = out / "plan-confirm.json"
                splice.write_bytes(canonical({**plan_document, "mutants": [
                    mutant for mutant in plan_document["mutants"] if mutant["key"] not in stale]}) + b"\n")
            ws = plan_module.schemata(root, splice, Path(splice_ws) if splice_ws else TARGET / "ws-confirm")
        ws = Path(ws)
        for name in sorted({package(oracle) for oracle in oracles}):
            if name not in drivers:
                drivers[name] = build_driver(ws, name)
    pairs, failures = [], []
    base_rows = base_native = 0
    trace_summary = {}
    excused_hits = []
    workers = jobs or max(1, (os.cpu_count() or 4) - 2)
    for oracle in oracles:
        binary = Path(drivers[package(oracle)])
        native_path = Path((natives or {}).get(oracle) or go.native_path(oracle))
        native_document = go.load_native(oracle, native_path)
        if document["inputs"]["oracles"][oracle]["native_sha256"] != file_sha256(native_path):
            raise ValueError(f"{oracle}: the results were checked against another native file")
        native_rows = {row["row"]: row for row in native_document["rows"]}
        request_file = requests_file(oracle, (requests or {}).get(oracle))
        compared = go.oracle_spec(oracle).compared
        recorded = [(mutant, kill_row) for mutant in killed for kill_row in mutant["kills"] if kill_row["oracle"] == oracle]
        rows = sorted({kill_row["row"] for _, kill_row in recorded})
        dump_all = oracle in DUMPING
        base = out / f"native-base-{oracle}.ndjson"
        write_base(base, [native_rows[row] for row in rows])
        dumps = out / f"dumps-{oracle}"
        dumps.mkdir(exist_ok=True)
        options = {"max_kills": 0, "report_all": True, "recheck": False, "dump_all": dump_all}
        work = [{"mutant": 0, "rows": rows, **options}] if rows else []
        by_mutant = defaultdict(list)
        for mutant, kill_row in recorded:
            by_mutant[mutant["key"]].append(kill_row)
        for key, kill_rows in sorted(by_mutant.items()):
            if key in stale:
                continue
            ids = [planned[key]["id"]]
            if planned[key].get("control") is not None:
                ids.append(planned[key]["control"])
            work += [{"mutant": id_, "rows": [kill_row["row"] for kill_row in kill_rows], **options} for id_ in ids]
        records = []
        if work:
            argv, env = oracle_command(oracle, binary, "kill", requests=request_file, base=base, dump_dir=dumps)
            micros = {row: native_rows[row].get("micros", 0) for row in rows}
            records = run_jobs(argv, work, workers=workers, fresh=True, env=env,
                               deadline=lambda row: max(30.0, row_deadline(micros.get(row, 0))),
                               log_dir=out / f"logs-{oracle}")
        replays = {}
        for job, record in zip(work, records):
            for result in record["results"]:
                replays[(job["mutant"], result["row"])] = (result, record)
            for row in job["rows"]:
                replays.setdefault((job["mutant"], row), (None, record))
        for row in rows:
            base_rows += 1
            result, record = replays[(0, row)]
            good, digests = _replayed(oracle, result, native_rows[row])
            good = good and digests is not None and all(digests.get(stage) == native_rows[row]["digests"].get(stage)
                                                        for stage in compared)
            base_native += good
            if not good:
                failures.append({"oracle": oracle, "row": row, "mutant": 0, "status": record["status"],
                                 "failure": "base row no longer equals native"})
        for mutant, kill_row in recorded:
            planned_mutant = planned[mutant["key"]]
            entry = {"key": mutant["key"], "oracle": oracle, "row": kill_row["row"],
                     "request_sha256": kill_row["request_sha256"], "stages": None, "mutant": None, "control": None}
            if mutant["key"] in stale:
                entry["state"] = "stale"
                pairs.append(entry)
                continue
            native_row = native_rows[kill_row["row"]]
            result, record = replays[(planned_mutant["id"], kill_row["row"])]
            good, digests = _replayed(oracle, result, native_row)
            if digests is None:
                entry["state"] = _lost_state(record, kill_row["row"])
                pairs.append(entry)
                continue
            entry["mutant"] = {stage: digests.get(stage) for stage in compared}
            entry["stages"] = [stage for stage in compared if digests.get(stage) != native_row["digests"].get(stage)]
            state = "reproduced"
            if not good or not entry["stages"]:
                state = "not_a_kill"
            elif entry["mutant"] != kill_row["mutant"] or entry["stages"] != kill_row["stages"]:
                state = "changed"
            if kill_row.get("control") is not None:
                control_id = planned_mutant.get("control")
                control_result, _ = replays.get((control_id, kill_row["row"]), (None, None))
                control_good, control_digests = _replayed(oracle, control_result, native_row)
                if control_digests is None or not control_good:
                    state = "control_failed" if state == "reproduced" else state
                else:
                    entry["control"] = {stage: control_digests.get(stage) for stage in compared}
                    if state == "reproduced" and entry["control"] != kill_row["control"]:
                        state = "changed"
                    if state == "reproduced" and all(entry["control"][stage] == entry["mutant"][stage]
                                                     for stage in entry["stages"]):
                        # The mutant must differ from native and from its
                        # control in one stage, as when it was credited.
                        state = "not_a_kill"
            elif planned_mutant.get("control") is not None:
                state = "control_failed" if state == "reproduced" else state
            entry["state"] = state
            pairs.append(entry)
        shutil.rmtree(dumps, ignore_errors=True)
        if retrace:
            # Hits only: no excused home may execute on any row, while
            # producing or while observing.
            traced_rows, _ = run_trace(binary, oracle, request_file, out / f"retrace-{oracle}.ndjson",
                                       shards=default_shards(oracle))
            (out / f"retrace-{oracle}.ndjson").unlink(missing_ok=True)
            hit, observed, executed = defaultdict(int), defaultdict(int), defaultdict(int)
            for row in traced_rows:
                for id_ in row["hits"]:
                    hit[id_] += 1
                for id_ in row.get("observe_hits", ()):
                    observed[id_] += 1
                for id_ in set(row["hits"]) | set(row.get("observe_hits", ())):
                    executed[id_] += 1
            for key in excused:
                id_ = planned[key]["id"]
                if key in stale:
                    excused_hits.append({"oracle": oracle, "key": key, "rows": None,
                                         "failure": "the excused home's span changed since the campaign"})
                elif executed.get(id_):
                    excused_hits.append({"oracle": oracle, "key": key, "rows": executed[id_],
                                         "production_rows": hit.get(id_, 0), "observe_rows": observed.get(id_, 0),
                                         "failure": "an excused home is reached" if hit.get(id_)
                                         else "an excused home executes while observing"})
            trace_summary[oracle] = {"rows": len(traced_rows), "mutants_reached": len(hit),
                                     "mutants_observed": len(observed)}
            if oracle in COLUMN_ORACLES:
                # Column parity holds on the replay too: every row of a column
                # with a recorded kill still equals native.
                column_of = row_columns(request_file)
                parity = column_parity(column_of, traced_rows, native_rows)
                for column in sorted({column_of[kill_row["row"]] for _, kill_row in recorded}):
                    entry = parity[column]
                    if entry["base_match"] != entry["rows"]:
                        failures.append({"oracle": oracle, "column": column, "rows": entry["rows"],
                                         "base_match": entry["base_match"], "first": entry["mismatched"][:5],
                                         "failure": "a column with a recorded kill no longer equals native on "
                                                    "every row (column parity)"})
                trace_summary[oracle]["columns_diverging"] = sorted(
                    column for column, entry in parity.items() if entry["base_match"] != entry["rows"])
    reproduced = sum(pair["state"] == "reproduced" for pair in pairs)
    for pair in pairs:
        if pair["state"] != "reproduced":
            failures.append({"oracle": pair["oracle"], "row": pair["row"], "key": pair["key"], "status": pair["state"],
                             "failure": "the recorded kill pair does not reproduce"})
    failures += excused_hits
    # Stale killed mutants and excused homes are reported once each.
    excused_stale = sorted({hit["key"] for hit in excused_hits if hit["rows"] is None})
    receipt = {
        "version": VERSION, "kind": "mutation-witnesses", "results_sha256": file_sha256(results_path),
        "plan_sha256": file_sha256(plan),
        "drivers": {name: file_sha256(path) for name, path in sorted(drivers.items())}, "ws": ws_identity(ws),
        "native_sha256": {oracle: document["inputs"]["oracles"][oracle]["native_sha256"] for oracle in oracles},
        "oracles": oracles, "killed_mutants": len(killed), "stale": sorted(stale & {m["key"] for m in killed}),
        "pairs_total": len(pairs), "confirmed": reproduced, "pairs": pairs,
        "base_rows": base_rows, "base_native": base_native,
        "excused_mutants": len(excused), "excused_stale": excused_stale,
        "retraced": trace_summary if retrace else None, "failures": failures,
        "result": "pass" if (not failures and reproduced == len(pairs) and base_native == base_rows and retrace)
        else "fail",
    }
    path = out / "receipt.json"
    path.write_bytes(canonical(receipt) + b"\n")
    return receipt


# ---------------------------------------------------------------------------

def _progress(done, total, job, record):
    if done == total or done % 25 == 0 or record["status"] != "done":
        print(f"[{done}/{total}] mutant {job['mutant']}: {record['status']}"
              f"{' ' + str(record['reason']) if record['reason'] else ''}", file=sys.stderr, flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    oracles = sorted(PACKAGES)
    build = commands.add_parser("build", help="build the oracle binaries in a workspace")
    build.add_argument("--ws", default=str(DEFAULT_WS))
    build.add_argument("--oracle", choices=oracles, action="append")
    run = commands.add_parser("trace", help="base trace of every row, checked against native")
    run.add_argument("--oracle", required=True, choices=oracles)
    run.add_argument("--ws", default=str(DEFAULT_WS), help="workspace to build in (the repository for an unspliced trace)")
    run.add_argument("--requests")
    run.add_argument("--native")
    run.add_argument("--out", default=str(DEFAULT_OUT))
    run.add_argument("--driver", help="use this oracle binary instead of building one")
    run.add_argument("--shards", type=int, help="parallel trace processes (default: cpu count - 2 for syntax, else 1)")
    run.add_argument("--verify-frames", action="store_true", help="recompute every row's digests from its frames in Python")
    campaign = commands.add_parser("kill", help="the kill campaign of one oracle")
    campaign.add_argument("--oracle", required=True, choices=oracles)
    campaign.add_argument("--ws", default=str(DEFAULT_WS))
    campaign.add_argument("--plan", default=str(TARGET / "plan.json"))
    campaign.add_argument("--trace")
    campaign.add_argument("--go-reach")
    campaign.add_argument("--requests")
    campaign.add_argument("--native")
    campaign.add_argument("--out", default=str(DEFAULT_OUT))
    campaign.add_argument("--driver")
    campaign.add_argument("--jobs", type=int)
    campaign.add_argument("--mutants", help="comma-separated mutant ids (default: every planned mutant)")
    campaign.add_argument("--skip-killed", help="results or kill file whose killed mutants are skipped")
    campaign.add_argument("--max-kills", type=int, default=MAX_KILLS)
    campaign.add_argument("--max-rows", type=int, default=0, help="row budget per job (0: every candidate row)")
    campaign.add_argument("--memory-limit-mb", type=int, default=MEMORY_LIMIT_MB)
    campaign.add_argument("--keep-dumps", action="store_true")
    merge = commands.add_parser("results", help="merge kill files into results.json.gz")
    merge.add_argument("--plan", default=str(TARGET / "plan.json"))
    merge.add_argument("--kills", nargs="+", required=True)
    merge.add_argument("--out")
    replay = commands.add_parser("confirm", help="replay every recorded kill pair on a fresh splice of the manifest")
    replay.add_argument("--results", required=True)
    replay.add_argument("--plan", default=str(TARGET / "plan.json"))
    replay.add_argument("--ws", help="an existing spliced workspace instead of a fresh schemata build")
    replay.add_argument("--splice-into", help="where the fresh schemata build goes (default target/phase1-mutation/ws-confirm)")
    replay.add_argument("--driver", help=f"a {DRIVER} binary to use")
    replay.add_argument("--syntax-driver", help=f"a {SYNTAX} binary to use")
    replay.add_argument("--requests", action="append", default=[], metavar="ORACLE=FILE")
    replay.add_argument("--native", action="append", default=[], metavar="ORACLE=FILE")
    replay.add_argument("--jobs", type=int)
    replay.add_argument("--out")
    compare = commands.add_parser("compare-traces", help="rows whose digests differ between two traces")
    compare.add_argument("left")
    compare.add_argument("right")
    args = parser.parse_args(argv)
    if args.command == "build":
        result = {name: str(build_driver(args.ws, name))
                  for name in sorted({package(oracle) for oracle in (args.oracle or oracles)})}
    elif args.command == "trace":
        result = trace(args.oracle, args.ws, args.requests, args.native, args.out, driver=args.driver,
                       verify_frames=args.verify_frames, shards=args.shards)
    elif args.command == "kill":
        mutants = {int(value) for value in args.mutants.split(",")} if args.mutants else None
        result = kill(args.oracle, args.ws, args.plan, args.trace, args.go_reach, args.jobs, args.out,
                      requests=args.requests, native=args.native, driver=args.driver, mutants=mutants,
                      skip_killed=args.skip_killed,
                      max_kills=args.max_kills, max_rows=args.max_rows, memory_limit_mb=args.memory_limit_mb,
                      keep_dumps=args.keep_dumps, progress=_progress)
    elif args.command == "results":
        result = results(args.plan, args.kills, args.out)
    elif args.command == "confirm":
        pairs = lambda values: dict(value.split("=", 1) for value in values)  # noqa: E731
        drivers = {name: path for name, path in ((DRIVER, args.driver), (SYNTAX, args.syntax_driver)) if path}
        result = confirm(args.results, args.plan, ws=args.ws, drivers=drivers, requests=pairs(args.requests),
                         natives=pairs(args.native), jobs=args.jobs, out=args.out, splice_ws=args.splice_into)
    else:
        result = compare_traces(args.left, args.right)
    print(json.dumps(result, sort_keys=True))
    return 0 if result.get("result", "pass") == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
