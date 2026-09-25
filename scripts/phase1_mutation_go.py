#!/usr/bin/env python3
"""Phase 1 mutation witnesses, Go side: request materialization, native freeze, Go reach.

docs/PHASE1-mutation-witnesses.md is the contract. A mutation kill needs, on one
frozen request row, a frozen native observation that the unmutated Rust build
reproduces and that the mutated build does not, and proof that the pinned Go
function was entered on that row. This module owns the two Go-side artifacts
for four oracles:

* ``e1`` and ``binder`` run the existing S06 and S07 long-running oracles
  (``scripts/s06_oracle``, ``scripts/s07_oracle``) over the S06 primary
  requests and the S07 bind requests.
* ``syntax`` runs a main-package copy of the Phase 1 syntax-schedule probe
  (``tools/phase1/mutation/go/syntax``) over the 15,152 loaded rows of
  ``data/phase1/syntax-schedule.json``; its native digests are those of the
  committed ``data/phase1/syntax-native.json`` values, which every freeze
  reproduces with the driver and with the committed probe itself.
* ``facts`` runs ``tools/phase1/mutation/go/facts`` over the S06 primary
  requests: after the S06 parse, the ``[kind, SubtreeFacts()]`` pair of every
  node in document order.

Commands:

* ``requests`` materializes ``target/phase1-mutation/requests-<oracle>.ndjson``
  in committed inventory order. e1, binder and facts come from the existing
  S06 freeze path (the pinned Go preprocessing export, run when no export is
  present); syntax comes from the committed F5b full capture (or a fresh
  TestS07SubsetExport). Every row must reproduce its committed request digest
  and, where one is committed, the whole sequence its sequence hash, or nothing
  is written.
* ``native`` runs the plain pinned oracle twice, from two shardings, and writes
  ``data/phase1/mutation/native-<oracle>.json.gz`` only when both runs agree
  on every row (syntax: agree with the committed native values, and the
  committed probe agrees too). ``verify-native`` repeats a plain run against
  the frozen file.
* ``reach`` builds instrumented copies from a git-archive export of the pin
  (``-cover -covermode=atomic``, the main package included in ``-coverpkg``;
  upstream/ is never edited), brackets every pinned stage with a counter clear
  and a counter snapshot, runs every row twice from two shardings, checks that
  both instrumented runs equal the frozen native digests on every row, and
  decodes the snapshots into ``data/phase1/mutation/go-reach-<oracle>.json.gz``.
  The file holds only what a rerun reproduces: every statistic in it counts
  stable operations only, and its summary is taken over the operations of the
  plan's ``homes`` (``data/phase1/mutation/manifest.json``). Run statistics
  that depend on pool or scheduling state (row counts of the unstable
  operations, every entered function) go to an unbound log,
  ``target/phase1-mutation/go/reach-<oracle>.stats.json``.
  ``verify-reach`` repeats the instrumented runs with the recorded process
  counts and requires a byte-identical file.

Digests (``digest_rule`` in the native file):

* e1: per stage, sha256 over ``canonical({"kind": kind, "value": value}) + "\\n"``
  for each observation frame of that stage, in order: exactly the per-stage
  sha256 ``s06_results.compare_case`` computes.
* binder: per graph stage, sha256 over ``canonical([kind, comparable(value)]) +
  "\\n"`` for each logical graph record (fragments reassembled), with
  ``s07_binder.comparable`` the evidence comparator's only normalization.
* syntax: per compared field of ``phase1_syntax.COMPARED``, sha256 over
  ``canonical(value)`` of the row's field.
* facts: sha256 over ``canonical([[kind, facts], ...])`` of the node list.

Go reach rule (``stage_rule`` in the reach file, section 3 of the contract): a
function is entered on a row when the entry block of its FuncDecl ran (the
first-positioned coverage unit, so a closure folded into its parent never counts
as the parent's entry). A snapshot of a production segment counts every
instrumented package; an observation segment counts only the operation-id
prefixes the oracle names (e1: ``tsc/internal/parser/``, for lazy JSDoc
parsing; every other oracle: nothing). Functions map to operation ids by file
and line containment against data/go-functions.tsv, never by name. The rule
also records the environment of the instrumented runs and the decoder:
``DECODER_VERSION`` and the sha256 of the source of every decoder function
(``DECODER_FUNCTIONS``), so a decoder edit stales every reach file decoded by
the old code.

Operations whose reach depends on process state (the sync.Pool operations of
POOL_OPERATIONS, and any operation whose row set differs between the two
instrumented runs) are ``unstable_ops``: listed, never indexed, so they are
never candidates and never credited. The syntax reach run drops the bundled
library memo before every row, so each row's reach is what a fresh process
enters on it.
"""

from __future__ import annotations

import argparse
import ast
from dataclasses import dataclass
import functools
import gzip
import hashlib
import io
import json
import multiprocessing
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04_common import command, strict_json_loads  # noqa: E402
from s06_protocol import canonical  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target/phase1-mutation"
DATA = ROOT / "data/phase1/mutation"
TOOL = ROOT / "tools/phase1/mutation/go"
MODULE = "github.com/microsoft/TypeScript/tsc"
MODULE_PREFIX = "github.com/microsoft/TypeScript/"
COVER_PACKAGES = ("internal/ast", "internal/parser", "internal/binder", "internal/scanner", "internal/compiler")
VERSION = 1
SYNTAX_SCHEDULE = "data/phase1/syntax-schedule.json"
SYNTAX_NATIVE = "data/phase1/syntax-native.json"
SYNTAX_PROBE = "tools/phase1/syntax/program_probe_test.go"
SYNTAX_ARCHIVE = "data/phase1/captures/f5b-reviewed-syntax-full.tar.gz"
SYNTAX_ARCHIVE_DIR = "phase1-f5b-review-full-20260923/full"
# scripts/phase1_syntax.py COMPARED (a test pins the equality).
SYNTAX_COMPARED = ("files", "file_names_sha256", "syntactic", "plain_hex", "pretty_hex")
PROBE_FIELDS = ("guard", "id", "load", "path", "request")
BATCH_DEADLINE = 4 * 3600
ENTRY_RULE = ("a FuncDecl is entered when its first-positioned coverage unit counted; "
              "mapping by file and line containment against data/go-functions.tsv")
# The plan whose `homes` operations the reach summary is taken over.
MANIFEST = "data/phase1/mutation/manifest.json"
# Bump on any change to the decoder: the module-level names and functions below
# turn the instrumented runs into the index. stage_rule records the version and
# the sha256 of their source text (test_phase1_mutation_go pins the pair).
DECODER_VERSION = 2
DECODER_CONSTANTS = ("MODULE_PREFIX", "UNIT")
DECODER_FUNCTIONS = ("debugdump", "parse_dump", "load_inventory", "attach_ids", "uleb", "parse_counters",
                     "meta_file", "read_stream", "entered", "_decode_shard", "merge_shards", "decode_run",
                     "op_index", "unstable_operations", "stable_index", "encode_gaps")


@dataclass(frozen=True)
class Oracle:
    name: str
    # The committed request inventory and, when one exists, its sequence hash.
    inventory: str
    probes: str | None
    # Protocol stages with outcomes, and the stages whose digests are compared.
    operations: tuple
    compared: tuple
    # "process": an existing NDJSON protocol oracle, instrumented by patches.
    # "batch": a Phase 1 main-package driver reading and writing NDJSON files.
    kind: str
    package: str
    source_dir: str
    sources: tuple
    bridges: tuple
    segments: dict
    # Operation-id prefixes an observation segment counts (empty: nothing).
    observe_counts: tuple
    digest_rule: str


ORACLES = {
    "e1": Oracle(
        name="e1", inventory="data/s06/requests.json", probes="data/s06/probes.json",
        operations=("parse", "node_index_before", "encode_source_file", "node_index_after"),
        compared=("parse", "node_index_before", "encode_source_file", "node_index_after"),
        kind="process", package="internal/s06oracle", source_dir="scripts/s06_oracle",
        sources=("json.go", "protocol.go", "behavior.go", "factory.go", "codec.go"),
        bridges=(("codec_bridge.go", "internal/ast/s06_codec_bridge.go"),),
        segments={"parse": "production", "parse:observe": "observe", "node_index_before": "observe",
                  "encode_source_file": "observe", "node_index_after": "observe"},
        observe_counts=("tsc/internal/parser/",),
        digest_rule="sha256 over canonical({kind,value})+LF per observation frame of the stage, in order "
                    "(s06_results.compare_case sha256); framing fields excluded"),
    "binder": Oracle(
        name="binder", inventory="data/s07/binder-requests.json", probes="data/s07/binder-probes.json",
        operations=("parse", "parsed_graph", "bind", "bound_graph", "repeat_bind", "repeated_graph"),
        compared=("parsed_graph", "bound_graph", "repeated_graph"),
        kind="process", package="internal/s07binder", source_dir="scripts/s07_oracle",
        sources=("json.go", "protocol.go", "binder.go", "graph.go", "graph_stream.go"),
        bridges=(("syntax_bridge.go", "internal/ast/s07_syntax_bridge.go"),
                 ("access_bridge.go", "internal/ast/s07_access_bridge.go"),
                 ("parser_bridge.go", "internal/parser/s07_bridge.go")),
        segments={"parse": "production", "bind": "production", "repeat_bind": "production",
                  "parsed_graph": "observe", "bound_graph": "observe", "repeated_graph": "observe"},
        observe_counts=(),
        digest_rule="sha256 over canonical([kind, s07_binder.comparable(value)])+LF per logical graph record "
                    "of the stage (fragments reassembled), in order"),
    "syntax": Oracle(
        name="syntax", inventory=SYNTAX_SCHEDULE, probes=None,
        operations=("program",), compared=SYNTAX_COMPARED,
        kind="batch", package="internal/phase1syntax", source_dir="tools/phase1/mutation/go/syntax",
        sources=("main.go",), bridges=(),
        segments={"load": "production", "files": "observe", "syntactic": "production", "render": "observe"},
        observe_counts=(),
        digest_rule="per compared field of phase1_syntax.COMPARED: sha256 over canonical(value) of the row's "
                    "field; the program outcome is ok when the row loaded and was observed"),
    "facts": Oracle(
        name="facts", inventory="data/s06/requests.json", probes="data/s06/probes.json",
        operations=("parse", "subtree_facts"), compared=("subtree_facts",),
        kind="batch", package="internal/phase1facts", source_dir="tools/phase1/mutation/go/facts",
        sources=("main.go",), bridges=(),
        segments={"parse": "production", "facts:walk": "observe", "subtree_facts": "production"},
        observe_counts=(),
        digest_rule="sha256 over canonical([[kind, facts], ...]): for every node of a pre-order "
                    "ForEachChild walk from the SourceFile, its Kind and (*ast.Node).SubtreeFacts(); "
                    "a panicking stage digests the pairs computed before the panic"),
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        while chunk := stream.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def oracle_spec(name):
    if name not in ORACLES:
        raise ValueError(f"unknown oracle {name!r}; expected one of {sorted(ORACLES)}")
    return ORACLES[name]


def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def requests_path(oracle):
    return TARGET / f"requests-{oracle}.ndjson"


def native_path(oracle):
    return DATA / f"native-{oracle}.json.gz"


def reach_path(oracle):
    return DATA / f"go-reach-{oracle}.json.gz"


def requests_digest(pairs):
    """Digest of the ordered (id, request_sha256) list every artifact binds."""
    return sha256(canonical([[row, digest] for row, digest in pairs]))


# ---------------------------------------------------------------------------
# rules the reach file records and the harness recomputes

def stage_rule(oracle):
    """The segment map, counting rule, run environment and decoder a reach index was decoded under."""
    spec = oracle_spec(oracle)
    return {"segments": dict(sorted(spec.segments.items())),
            "production": {"counts": "every operation of the instrumented packages",
                           "packages": list(COVER_PACKAGES)},
            "observe": {"counts": "operations whose id starts with one of these prefixes",
                        "prefixes": list(spec.observe_counts)},
            "entry_rule": ENTRY_RULE,
            "environment": reach_environment(oracle),
            "decoder": {"version": DECODER_VERSION, "definitions": list(DECODER_CONSTANTS + DECODER_FUNCTIONS),
                        "source_sha256": decoder_digest()}}


def stage_rule_digest(oracle):
    return sha256(canonical(stage_rule(oracle)))


def decoder_source(text=None):
    """The source text of the decoder: each of DECODER_CONSTANTS and
    DECODER_FUNCTIONS as written at module level (decorators included), in
    that order. ``text`` defaults to this module's file; the lookup is by
    name, so an edit anywhere else in the module leaves the result unchanged.
    """
    if text is None:
        text = Path(__file__).read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    found = {}
    for node in ast.parse(text).body:
        if isinstance(node, ast.FunctionDef):
            name, start = node.name, min([node.lineno] + [item.lineno for item in node.decorator_list])
        elif (isinstance(node, ast.Assign) and len(node.targets) == 1
              and isinstance(node.targets[0], ast.Name)):
            name, start = node.targets[0].id, node.lineno
        else:
            continue
        if name not in DECODER_CONSTANTS + DECODER_FUNCTIONS:
            continue
        if name in found:
            raise ValueError(f"decoder definition {name} occurs twice")
        found[name] = "".join(lines[start - 1:node.end_lineno])
    missing = [name for name in DECODER_CONSTANTS + DECODER_FUNCTIONS if name not in found]
    if missing:
        raise ValueError(f"decoder definitions missing from the module: {missing}")
    return "".join(found[name] for name in DECODER_CONSTANTS + DECODER_FUNCTIONS)


@functools.lru_cache(maxsize=8)
def _decoder_digest(text):
    return sha256(decoder_source(text).encode())


def decoder_digest(text=None):
    """sha256 of decoder_source; the module file is re-read on every call."""
    return _decoder_digest(Path(__file__).read_text(encoding="utf-8") if text is None else text)


# Entered or skipped depending on sync.Pool state (parser.go parserPool,
# ast/utilities.go setParentInChildrenPool): a pool miss enters the first three
# and the last, a hit enters Scanner.Reset and cleared.
POOL_OPERATIONS = (
    "tsc/internal/parser/parser.go:newParser", "tsc/internal/parser/parser.go:Parser.initializeClosures",
    "tsc/internal/scanner/scanner.go:NewScanner", "tsc/internal/scanner/scanner.go:Scanner.Reset",
    "tsc/internal/scanner/scanner.go:cleared", "tsc/internal/ast/utilities.go:newParentInChildrenSetter",
)
UNSTABLE_RULE = ("unstable_ops are POOL_OPERATIONS plus every operation whose row set differs between the "
                 "two instrumented runs (different process counts, so different pool and memo histories); "
                 "they are never indexed in op_row_gaps, so they are never candidates and never credited")


# ---------------------------------------------------------------------------
# requests

def committed_inventory(oracle):
    """(inventory rows, sequence document) for an S06/S07 inventory oracle."""
    spec = oracle_spec(oracle)
    if spec.probes is None:
        raise ValueError(f"{oracle} has no sequence-hashed request inventory")
    inventory = strict_json_loads((ROOT / spec.inventory).read_bytes())
    probes = strict_json_loads((ROOT / spec.probes).read_bytes())
    if inventory.get("pin") != pin() or probes.get("pin") != pin():
        raise ValueError(f"{spec.inventory} or {spec.probes} is not frozen at the current pin")
    return inventory["requests"], probes


def syntax_inventory():
    """The loaded rows of the committed syntax schedule, in schedule order."""
    schedule = strict_json_loads((ROOT / SYNTAX_SCHEDULE).read_bytes())
    if schedule.get("provenance", {}).get("pin") != pin():
        raise ValueError(f"{SYNTAX_SCHEDULE} is not frozen at the current pin")
    return [row for row in schedule["rows"] if row["load"] == "loaded"]


def inventory_rows(oracle):
    """The (id, committed request digest) list a native freeze covers, in order."""
    if oracle == "syntax":
        return [(row["id"], row["loading_request_sha256"]) for row in syntax_inventory()]
    recipes, _ = committed_inventory(oracle)
    return [(recipe["id"], recipe["request_sha256"]) for recipe in recipes]


def request_line(oracle, request, digest):
    """One materialized request line (syntax keeps its ordered option maps)."""
    if oracle == "syntax":
        from s07_subset import json_bytes
        return json_bytes({**request, "request_sha256": digest})
    return canonical({**request, "request_sha256": digest}) + b"\n"


def verify_syntax_requests(probes):
    from s06_corpus import primary_id
    from s07_subset import json_bytes
    rows = syntax_inventory()
    if len(probes) != len(rows):
        raise ValueError(f"syntax: {len(probes)} requests, the schedule loads {len(rows)}")
    mismatched = []
    for probe, row in zip(probes, rows):
        if (type(probe) is not dict or tuple(sorted(probe)) != PROBE_FIELDS or probe["id"] != row["id"]
                or type(probe["request"]) is not dict or probe["request"].get("id") != row["id"]
                or probe["guard"] is not True or probe["load"] is not True
                or primary_id(probe["path"]) != row["primary"]
                or sha256(json_bytes(probe["request"])) != row["loading_request_sha256"]):
            mismatched.append(row["id"])
    if mismatched:
        raise ValueError(f"syntax: {len(mismatched)} requests differ from the schedule's loading_request_sha256, "
                         f"first {mismatched[:3]}")
    return [row["loading_request_sha256"] for row in rows]


def verify_requests(oracle, requests):
    """Refuse unless every row and the whole sequence match the committed freeze."""
    if oracle == "syntax":
        return verify_syntax_requests(requests)
    recipes, probes = committed_inventory(oracle)
    if len(requests) != len(recipes):
        raise ValueError(f"{oracle}: {len(requests)} requests, committed inventory has {len(recipes)}")
    mismatched = []
    for request, recipe in zip(requests, recipes):
        if request["id"] != recipe["id"] or sha256(canonical(request)) != recipe["request_sha256"]:
            mismatched.append(recipe["id"])
    if mismatched:
        raise ValueError(f"{oracle}: {len(mismatched)} requests differ from committed request_sha256, first {mismatched[:3]}")
    sequence = sha256(canonical(requests))
    if sequence != probes["request_sha256"]:
        raise ValueError(f"{oracle}: request sequence sha256 {sequence} differs from {oracle_spec(oracle).probes}")
    if oracle == "binder":
        parser_probes = strict_json_loads((ROOT / "data/s06/probes.json").read_bytes())
        if probes["parser_expansion_sha256"] != parser_probes["request_sha256"]:
            raise ValueError("binder inventory was frozen from a different S06 expansion")
    return [recipe["request_sha256"] for recipe in recipes]


def parser_requests(export):
    """The S06 primary requests rebuilt from a pinned Go preprocessing export."""
    from s04 import verified_upstream
    from s06_corpus import freeze_records, membership, read_export
    upstream = verified_upstream()
    physical, libraries = membership(upstream, pin())
    if export is None:
        export = TARGET / "s06-export.ndjson"
        if not export.exists():
            from s06_build import export_cases
            export.parent.mkdir(parents=True, exist_ok=True)
            export_cases(physical, export)
    records = read_export(Path(export), physical, libraries)
    _, requests = freeze_records(records, upstream, pin())
    return requests


def syntax_requests(export=None):
    """The loaded syntax probe requests and where they came from.

    By default they come from the committed F5b full capture, whose own
    provenance binds the file; with ``export`` (a TestS07SubsetExport
    observation file) they are rebuilt through the schedule producer, which
    authenticates them against data/s07/subset.json.
    """
    if export:
        from phase1_syntax_schedule import schedule_requests
        _, probes = schedule_requests(Path(export))
        probes = [probe for probe in probes if probe["load"]]
        return probes, {"export": str(export), "export_sha256": file_sha256(export)}
    with tarfile.open(ROOT / SYNTAX_ARCHIVE) as archive:
        raw = archive.extractfile(f"{SYNTAX_ARCHIVE_DIR}/requests.json").read()
        provenance = strict_json_loads(archive.extractfile(f"{SYNTAX_ARCHIVE_DIR}/provenance.json").read())
    if sha256(raw) != provenance["requests_sha256"] or provenance.get("selection") != "full":
        raise ValueError(f"{SYNTAX_ARCHIVE}: requests.json does not match its full-capture provenance")
    probes = strict_json_loads(raw)
    if [probe["id"] for probe in probes] != provenance["selected"]:
        raise ValueError(f"{SYNTAX_ARCHIVE}: requests are not the capture's selection")
    return probes, {"archive": SYNTAX_ARCHIVE, "archive_sha256": file_sha256(ROOT / SYNTAX_ARCHIVE),
                    "member": f"{SYNTAX_ARCHIVE_DIR}/requests.json", "requests_sha256": sha256(raw)}


def materialize(oracle, *, export=None, out=None):
    oracle_spec(oracle)
    started = time.monotonic()
    source = None
    if oracle == "syntax":
        requests, source = syntax_requests(export)
    else:
        requests = parser_requests(Path(export) if export else None)
        if oracle == "binder":
            from s07_binder import convert_request
            requests = [convert_request(request) for request in requests]
    digests = verify_requests(oracle, requests)
    out = Path(out) if out else requests_path(oracle)
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_name(out.name + ".partial")
    with temporary.open("wb") as stream:
        for request, digest in zip(requests, digests):
            stream.write(request_line(oracle, request, digest))
    temporary.replace(out)
    result = {"oracle": oracle, "requests": len(requests),
              "path": str(out.relative_to(ROOT) if out.is_relative_to(ROOT) else out),
              "file_sha256": file_sha256(out),
              "requests_sha256": requests_digest(zip((r["id"] for r in requests), digests)),
              "seconds": round(time.monotonic() - started, 1)}
    if source:
        result["source"] = source
    return result


def load_requests(oracle, path=None):
    """Read the materialized rows and re-authenticate each against the inventory."""
    path = Path(path) if path else requests_path(oracle)
    if not path.exists():
        raise ValueError(f"{path} is missing; run `phase1_mutation_go.py requests --oracle {oracle}` first")
    rows = []
    with path.open("rb") as stream:
        for line in stream:
            line = strict_json_loads(line)
            digest = line.pop("request_sha256")
            rows.append((line["id"], digest, line))
    committed = verify_requests(oracle, [request for _, _, request in rows])
    for (row, digest, _), expected in zip(rows, committed, strict=True):
        if digest != expected:
            raise ValueError(f"{path}: {row} carries a request_sha256 that is not its committed digest")
    return rows


# ---------------------------------------------------------------------------
# digests

def e1_row(records, spec=ORACLES["e1"]):
    """Outcomes and per-stage digests of one validated S06 request stream."""
    digests, outcomes, messages = {}, {}, {}
    running = {}
    for record in records:
        tag = record["tag"]
        if tag == "observation":
            running.setdefault(record["stage"], hashlib.sha256()).update(
                canonical({"kind": record["kind"], "value": record["value"]}) + b"\n")
        elif tag == "stage":
            stage = record["stage"]
            outcomes[stage] = record["outcome"]
            if record["outcome"] != "ok":
                messages[stage] = record["message_hex"]
            if stage in spec.compared:
                digests[stage] = running.pop(stage, hashlib.sha256()).hexdigest()
    if running:
        raise ValueError("observations without a closing stage frame")
    return finish_row(spec, outcomes, messages, digests)


class GraphDigest:
    """Normalized graph digests over raw frames; reassembles 128 KiB fragments."""

    def __init__(self):
        self.running = {}
        self.fragment = None

    def observation(self, stage, kind, value):
        if kind == "fragment":
            key = (value["record_kind"], value["parts"])
            if self.fragment is None:
                if value["part"] != 0:
                    raise ValueError("graph fragment does not start at part 0")
                self.fragment = {"key": key, "chunks": []}
            if self.fragment["key"] != key or value["part"] != len(self.fragment["chunks"]):
                raise ValueError("interleaved or reordered graph fragment")
            self.fragment["chunks"].append(bytes.fromhex(value["payload_hex"]))
            if len(self.fragment["chunks"]) < value["parts"]:
                return
            kind = value["record_kind"]
            value = strict_json_loads(b"".join(self.fragment["chunks"]))
            self.fragment = None
        elif self.fragment is not None:
            raise ValueError("incomplete graph fragment")
        from s07_binder import comparable
        self.running.setdefault(stage, hashlib.sha256()).update(canonical([kind, comparable(value)]) + b"\n")

    def close(self, stage):
        if self.fragment is not None:
            raise ValueError("stage closes an incomplete graph fragment")
        return self.running.pop(stage, hashlib.sha256()).hexdigest()


def binder_row(records, spec=ORACLES["binder"]):
    """Outcomes and normalized graph digests of one binder request stream.

    Accepts either raw frames (fragments present) or the reassembled records
    s07_binder.Process yields; both produce the same digests.
    """
    graph = GraphDigest()
    digests, outcomes, messages = {}, {}, {}
    for record in records:
        tag = record["tag"]
        if tag == "observation":
            graph.observation(record["stage"], record["kind"], record["value"])
        elif tag == "stage":
            stage = record["stage"]
            outcomes[stage] = record["outcome"]
            if record["outcome"] != "ok":
                messages[stage] = record["message_hex"]
            if stage in spec.compared:
                digests[stage] = graph.close(stage)
    if graph.running:
        raise ValueError("graph observations without a closing stage frame")
    return finish_row(spec, outcomes, messages, digests)


def syntax_row(record, spec=ORACLES["syntax"]):
    """Outcome and per-field digests of one syntax row.

    Accepts the Go driver's row (``outcome`` and ``values``), a native row
    of data/phase1/syntax-native.json (the fields at top level, no outcome) or
    the Rust harness's schedule row (``state``, the fields at top level); a
    Rust row that is not ``observed`` has its state as the outcome.
    """
    if "values" in record or "outcome" in record:
        outcome, values = record["outcome"], record.get("values")
    elif "state" in record:
        outcome = "ok" if record["state"] == "observed" else record["state"]
        values = record
    else:
        outcome, values = "ok", record
    row = {"outcomes": {"program": outcome}, "digests": {}}
    if outcome == "ok":
        if not isinstance(values, dict) or any(field not in values for field in spec.compared):
            raise ValueError(f"syntax row {record.get('row', record.get('id'))!r} lacks a compared field")
        row["digests"] = {field: sha256(canonical(values[field])) for field in spec.compared}
    else:
        message = record.get("message") or record.get("panic") or record.get("error") or record.get("operation") or ""
        row["messages"] = {"program": str(message).encode().hex()}
    return row


def facts_digest(pairs):
    return sha256(canonical(pairs))


def facts_row(record, spec=ORACLES["facts"]):
    """Outcomes and the subtree-facts digest of one facts row.

    ``digest`` is the driver's; with ``list`` present (raw mode) the digest is
    recomputed from it and must agree.
    """
    outcomes = dict(record["outcomes"])
    if set(outcomes) != set(spec.operations) or any(not isinstance(value, str) for value in outcomes.values()):
        raise ValueError(f"facts row {record.get('row')!r} has malformed outcomes")
    digest = record.get("digest")
    if "list" in record:
        recomputed = facts_digest(record["list"])
        if digest is not None and digest != recomputed:
            raise ValueError(f"facts row {record.get('row')!r}: digest differs from its own node list")
        digest = recomputed
    ran = outcomes["subtree_facts"] != "not_run"
    if ran != (digest is not None):
        raise ValueError(f"facts row {record.get('row')!r}: a digest exactly when subtree_facts ran")
    row = {"outcomes": {stage: outcomes[stage] for stage in spec.operations},
           "digests": {"subtree_facts": digest} if ran else {}}
    if record.get("messages"):
        row["messages"] = dict(record["messages"])
    return row


def finish_row(spec, outcomes, messages, digests):
    ran = [stage for stage in spec.operations if stage in outcomes]
    if ran != list(outcomes) or ran != list(spec.operations[:len(ran)]):
        raise ValueError("stage frames out of protocol order")
    for stage in spec.operations[len(ran):]:
        outcomes[stage] = "not_run"
    row = {"outcomes": {stage: outcomes[stage] for stage in spec.operations},
           "digests": {stage: digests[stage] for stage in spec.compared if stage in digests}}
    if messages:
        row["messages"] = messages
    return row


def row_digests(oracle, records):
    """One row's outcomes and compared digests: frames (e1, binder) or a row (syntax, facts)."""
    return {"e1": e1_row, "binder": binder_row, "syntax": syntax_row, "facts": facts_row}[oracle](records)


# ---------------------------------------------------------------------------
# running an oracle over sharded rows

def _process(oracle, binary, stderr, env):
    if oracle == "e1":
        from s06_process import Process
    else:
        from s07_binder import Process
    return Process([str(binary)], stderr, env=env, deadline=300)


def _run_shard(job):
    oracle, binary, rows, stderr, env = job
    if oracle_spec(oracle).kind == "batch":
        return _run_batch_shard(job)
    results = []
    process = _process(oracle, binary, stderr, env)
    try:
        for index, row, digest, request in rows:
            process.send(request)
            started = time.monotonic()
            observed = row_digests(oracle, process.observations(request))
            results.append((index, {"row": row, "request_sha256": digest, **observed,
                                    "micros": int((time.monotonic() - started) * 1e6)}))
        process.finish()
        stream = process.digest.hexdigest()
    finally:
        process.close()
    return results, stream


def _run_batch_shard(job):
    """One Phase 1 driver process over a shard's request file."""
    oracle, binary, rows, stderr, env = job
    base = Path(stderr).with_suffix("")
    requests_file, output = base.with_suffix(".requests.ndjson"), base.with_suffix(".rows.ndjson")
    with requests_file.open("wb") as stream:
        for _, _, digest, request in rows:
            stream.write(request_line(oracle, request, digest))
    with Path(stderr).open("wb") as log:
        completed = subprocess.run([str(binary), str(requests_file), str(output)], env=env, cwd=ROOT,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                   timeout=BATCH_DEADLINE, check=False)
    if completed.returncode:
        raise RuntimeError(f"{oracle} driver exited {completed.returncode}; see {stderr}")
    data = output.read_bytes()
    lines = data.splitlines()
    if len(lines) != len(rows):
        raise RuntimeError(f"{oracle} driver wrote {len(lines)} rows for {len(rows)} requests")
    results = []
    for (index, row, digest, _), line in zip(rows, lines):
        record = strict_json_loads(line)
        if record.get("row") != row:
            raise RuntimeError(f"{oracle} driver answered {record.get('row')!r} for {row!r}")
        results.append((index, {"row": row, "request_sha256": digest, **row_digests(oracle, record),
                                "micros": int(record["micros"])}))
    requests_file.unlink()
    return results, sha256(data)


def run_oracle(oracle, binary, rows, *, jobs, workdir, env=None, cover=False):
    """Run every row once; shard i gets rows i, i+jobs, ... (balanced by size)."""
    workdir = Path(workdir)
    if workdir.exists():
        shutil.rmtree(workdir)
    workdir.mkdir(parents=True)
    jobs = max(1, min(jobs, len(rows)))
    work = []
    for shard in range(jobs):
        shard_env = dict(env or os.environ)
        if cover:
            # GOCOVERDIR must exist at startup: the runtime emits meta-data in init.
            (workdir / f"shard-{shard}.meta").mkdir()
            shard_env.update(PHASE1_COVER_STREAM=str(workdir / f"shard-{shard}.cover"),
                             PHASE1_COVER_META=str(workdir / f"shard-{shard}.meta"),
                             GOCOVERDIR=str(workdir / f"shard-{shard}.meta"))
        selected = [(index, *rows[index]) for index in range(shard, len(rows), jobs)]
        work.append((oracle, str(binary), selected, str(workdir / f"shard-{shard}.stderr"), shard_env))
    started = time.monotonic()
    if jobs == 1:
        outputs = [_run_shard(work[0])]
    else:
        with multiprocessing.get_context("spawn").Pool(jobs) as pool:
            outputs = pool.map(_run_shard, work)
    merged = [None] * len(rows)
    for results, _ in outputs:
        for index, result in results:
            merged[index] = result
    if any(result is None for result in merged):
        raise RuntimeError("a shard lost rows")
    print(f"{oracle}: {len(rows)} rows in {time.monotonic() - started:.1f}s over {jobs} processes", file=sys.stderr)
    return merged


def compare_rows(expected, actual):
    """Rows whose outcomes, messages or digests differ, by row id."""
    differences = []
    for left, right in zip(expected, actual, strict=True):
        if left["row"] != right["row"] or left["request_sha256"] != right["request_sha256"]:
            raise ValueError(f"row order drift at {left['row']} / {right['row']}")
        for field in ("outcomes", "digests", "messages"):
            if left.get(field) != right.get(field):
                differences.append({"row": left["row"], "field": field})
                break
    return differences


# ---------------------------------------------------------------------------
# builds

def go_version(env):
    return command(["go", "version"], cwd=ROOT, env=env).decode().split()[2]


def driver_sources(oracle, *, cover):
    """The files a Phase 1 driver build adds to the export: its sources and the hook (or its stub)."""
    spec = oracle_spec(oracle)
    files = {f"tsc/{spec.package}/{name}": (ROOT / spec.source_dir / name).read_bytes() for name in spec.sources}
    hook = "phase1_cover.go" if cover else "phase1_cover_off.go"
    files[f"tsc/{spec.package}/{hook}"] = (TOOL / hook).read_bytes()
    return files


def build_driver(oracle, files, flags, destination):
    """Build a Phase 1 driver package from a git-archive export of the pin."""
    from s04 import verified_upstream
    from s06_build import oracle_export
    spec = oracle_spec(oracle)
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with oracle_export() as (checkout, env, _):
        (checkout / "tsc" / spec.package).mkdir()
        for relative, content in files.items():
            path = checkout / relative
            if path.exists():
                raise ValueError(f"the build would overwrite pinned source {relative}")
            path.write_bytes(content)
        command(["go", "build", *flags, "-o", str(destination), f"./{spec.package}"], cwd=checkout / "tsc", env=env)
        verified_upstream()
        version = go_version(env)
    return destination, version, env


def build_plain(oracle):
    from s04 import go_environment
    spec = oracle_spec(oracle)
    if oracle == "e1":
        from s06_build import build_oracle
        binary = build_oracle()
    elif oracle == "binder":
        from s07_binder import build_oracle
        binary, _ = build_oracle()
    else:
        binary, version, _ = build_driver(oracle, driver_sources(oracle, cover=False), ["-trimpath", "-mod=readonly"],
                                          TARGET / "go" / f"{oracle}-plain")
        return binary, version
    assert spec.kind == "process"
    return Path(binary), go_version(go_environment())


def oracle_sources(oracle):
    """Digests of the sources a native freeze depends on (syntax: also the probe and its frozen values)."""
    spec = oracle_spec(oracle)
    names = list(spec.sources) + [source for source, _ in spec.bridges]
    result = {f"{spec.source_dir}/{name}": file_sha256(ROOT / spec.source_dir / name) for name in sorted(names)}
    if spec.kind == "batch":
        result["tools/phase1/mutation/go/phase1_cover_off.go"] = file_sha256(TOOL / "phase1_cover_off.go")
    if oracle == "syntax":
        for name in (SYNTAX_PROBE, SYNTAX_NATIVE):
            result[name] = file_sha256(ROOT / name)
    return dict(sorted(result.items()))


def inventory_binding(oracle, rows):
    spec = oracle_spec(oracle)
    binding = {"inventory": spec.inventory, "inventory_file_sha256": file_sha256(ROOT / spec.inventory),
               "rows": len(rows)}
    if spec.probes:
        _, probes = committed_inventory(oracle)
        binding.update(probes=spec.probes, sequence_sha256=probes["request_sha256"])
    else:
        binding["selection"] = "schedule rows whose native load is loaded, in schedule order"
    return binding


def strip_timing(results):
    return [{key: value for key, value in result.items() if key != "micros"} for result in results]


# ---------------------------------------------------------------------------
# native freeze

def syntax_native_rows(rows):
    """The committed native syntax values as native-file rows, in request order."""
    document = strict_json_loads((ROOT / SYNTAX_NATIVE).read_bytes())
    if document.get("provenance", {}).get("pin") != pin():
        raise ValueError(f"{SYNTAX_NATIVE} is not frozen at the current pin")
    native = document["rows"]
    if [item["id"] for item in native] != [row for row, _, _ in rows]:
        raise ValueError(f"{SYNTAX_NATIVE} rows are not the schedule's loaded rows")
    result = []
    for item, (row, digest, _) in zip(native, rows):
        if set(item) != {"id", *SYNTAX_COMPARED}:
            raise ValueError(f"{SYNTAX_NATIVE}: {row} is not an observed native row")
        result.append({"row": row, "request_sha256": digest, **syntax_row({"outcome": "ok", "values": item})})
    return result, document


def syntax_probe_check(rows, expected):
    """The committed probe itself (go test overlay, phase1_syntax_schedule.run_probe) over the rows."""
    from phase1_syntax_schedule import run_probe
    directory = TARGET / "native-syntax-probe"
    if directory.exists():
        shutil.rmtree(directory)
    report = run_probe(directory, [request for _, _, request in rows])
    observed = report["rows"]
    if [item["id"] for item in observed] != [row for row, _, _ in rows]:
        raise ValueError("the syntax probe answered other rows")
    actual = []
    for item, (row, digest, _) in zip(observed, rows):
        if item["load"] == "loaded":
            values = {field: item[field] for field in SYNTAX_COMPARED}
            actual.append({"row": row, "request_sha256": digest, **syntax_row({"outcome": "ok", "values": values})})
        else:
            actual.append({"row": row, "request_sha256": digest,
                           **syntax_row({"outcome": item["load"], "message": item.get("panic", "")})})
    differences = compare_rows(expected, actual)
    result = {"producer": "phase1_syntax_schedule.run_probe", "test": "TestPhase1SyntaxSchedule",
              "rows": len(actual), "go": report["go"], "request_sha256": report["request_sha256"],
              "equal": not differences}
    shutil.rmtree(directory)
    if differences:
        raise RuntimeError(f"syntax: the committed probe differs from {SYNTAX_NATIVE} on {len(differences)} rows, "
                           f"first {differences[:3]}")
    return result


def facts_digest_check(binary, rows, frozen, *, count=160):
    """Recompute the facts digest in Python from raw node lists on a spread of rows."""
    step = max(1, len(rows) // count)
    picked = sorted(set(range(0, len(rows), step)) | {max(range(len(rows)), key=lambda index: len(rows[index][2]["source_hex"]))})
    env = dict(os.environ, PHASE1_FACTS_RAW="1")
    workdir = TARGET / "native-facts-raw"
    if workdir.exists():
        shutil.rmtree(workdir)
    workdir.mkdir(parents=True)
    job = ("facts", str(binary), [(index, *rows[index]) for index in picked], str(workdir / "shard-0.stderr"), env)
    results, _ = _run_batch_shard(job)
    nodes = 0
    for index, result in results:
        if strip_timing([result])[0] != frozen[index]:
            raise RuntimeError(f"facts: raw-mode row {result['row']} differs from the frozen digest")
    for line in (workdir / "shard-0.rows.ndjson").read_bytes().splitlines():
        record = strict_json_loads(line)
        if "list" in record:
            nodes += len(record["list"])
            if len(record["list"]) != record["nodes"]:
                raise RuntimeError(f"facts: {record['row']} node count differs from its list")
    shutil.rmtree(workdir)
    return {"rows": len(picked), "nodes": nodes, "equal": True,
            "rule": "PHASE1_FACTS_RAW=1 rows: facts_digest(list) equals the driver digest and the frozen digest"}


def native(oracle, *, jobs=8, second_jobs=5, out=None, binary=None, probe_check=True):
    spec = oracle_spec(oracle)
    rows = load_requests(oracle)
    if binary is None:
        binary, version = build_plain(oracle)
    else:
        from s04 import go_environment
        binary, version = Path(binary), go_version(go_environment())
    first = strip_timing(run_oracle(oracle, binary, rows, jobs=jobs, workdir=TARGET / f"native-{oracle}-1"))
    second = strip_timing(run_oracle(oracle, binary, rows, jobs=second_jobs, workdir=TARGET / f"native-{oracle}-2"))
    differences = compare_rows(first, second)
    if differences:
        raise RuntimeError(f"{oracle}: two plain native runs disagree on {len(differences)} rows, first {differences[:3]}")
    fresh = {"runs": 2, "processes": [jobs, second_jobs], "equal": True}
    extra = {}
    if oracle == "syntax":
        frozen, source = syntax_native_rows(rows)
        differences = compare_rows(frozen, first)
        if differences:
            raise RuntimeError(f"syntax: the driver differs from {SYNTAX_NATIVE} on {len(differences)} rows, "
                               f"first {differences[:3]}")
        if source["provenance"]["go"] != version:
            raise RuntimeError(f"syntax: {SYNTAX_NATIVE} was captured with {source['provenance']['go']}, not {version}")
        fresh["equal_to_committed_values"] = True
        extra["values_source"] = {
            "native": SYNTAX_NATIVE, "file_sha256": file_sha256(ROOT / SYNTAX_NATIVE),
            "native_rows_sha256": sha256(canonical(source["rows"]) + b"\n"),
            "capture_request_sha256": source["provenance"]["request_sha256"],
            "driver": f"{spec.source_dir}/main.go: the probe's load path, memo on, as a main package"}
        if probe_check:
            extra["probe_check"] = syntax_probe_check(rows, frozen)
        first = frozen
    if oracle == "facts":
        extra["digest_check"] = facts_digest_check(binary, rows, first)
        e1 = read_document(native_path("e1")) if native_path("e1").exists() else None
        if e1 is not None:
            same = [row["outcomes"]["parse"] for row in e1["rows"]] == [row["outcomes"]["parse"] for row in first]
            extra["parse_outcomes_equal_e1"] = same
            if not same:
                raise RuntimeError("facts: parse outcomes differ from the frozen e1 parse")
    document = {
        "version": VERSION, "oracle": oracle, "pin": pin(), "go_version": version,
        "oracle_binary_sha256": file_sha256(binary), "oracle_sources": oracle_sources(oracle),
        "requests_sha256": requests_digest((row, digest) for row, digest, _ in rows),
        "request_inventory": inventory_binding(oracle, rows),
        "operations": list(spec.operations), "stages": list(spec.compared), "digest_rule": spec.digest_rule,
        "fresh_run": fresh, **extra,
        "rows": first,
    }
    out = Path(out) if out else native_path(oracle)
    write_rows_document(out, document)
    for index in (1, 2):
        shutil.rmtree(TARGET / f"native-{oracle}-{index}", ignore_errors=True)
    return summarize_native(document, out)


def summarize_native(document, path):
    rows = document["rows"]
    outcomes = {}
    for row in rows:
        shape = ",".join(f"{stage}={value}" for stage, value in row["outcomes"].items() if value != "ok")
        outcomes[shape or "all_ok"] = outcomes.get(shape or "all_ok", 0) + 1
    path = Path(path)
    return {"oracle": document["oracle"], "path": str(path.relative_to(ROOT) if path.is_relative_to(ROOT) else path),
            "rows": len(rows), "bytes": path.stat().st_size, "file_sha256": file_sha256(path),
            "oracle_binary_sha256": document["oracle_binary_sha256"], "outcome_shapes": outcomes}


def write_rows_document(path, document):
    """Deterministic gzip: canonical header, then one canonical row per line."""
    header = {key: value for key, value in document.items() if key != "rows"}
    raw = io.BytesIO()
    raw.write(canonical(header)[:-1] + b',"rows":[\n')
    raw.write(b",\n".join(canonical(row) for row in document["rows"]))
    raw.write(b"\n]}\n")
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".partial")
    with temporary.open("wb") as stream:
        with gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0, compresslevel=9) as compressed:
            compressed.write(raw.getvalue())
    temporary.replace(path)


def read_document(path):
    return strict_json_loads(gzip.decompress(Path(path).read_bytes()))


def load_native(oracle, path=None):
    """The frozen native file, checked against the current request inventory."""
    path = Path(path) if path else native_path(oracle)
    document = read_document(path)
    spec = oracle_spec(oracle)
    if document.get("version") != VERSION or document.get("oracle") != oracle or document.get("pin") != pin():
        raise ValueError(f"{path}: wrong version, oracle or pin")
    if document["stages"] != list(spec.compared) or document["operations"] != list(spec.operations):
        raise ValueError(f"{path}: stage list differs from the oracle definition")
    pairs = [(row["row"], row["request_sha256"]) for row in document["rows"]]
    if pairs != inventory_rows(oracle):
        raise ValueError(f"{path}: rows differ from the committed inventory")
    if document["requests_sha256"] != requests_digest(pairs):
        raise ValueError(f"{path}: request binding is stale")
    if spec.probes:
        _, probes = committed_inventory(oracle)
        if document["request_inventory"]["sequence_sha256"] != probes["request_sha256"]:
            raise ValueError(f"{path}: request binding is stale")
    return document


def verify_native(oracle, *, jobs=8, binary=None):
    document = load_native(oracle)
    rows = load_requests(oracle)
    if binary is None:
        binary, version = build_plain(oracle)
    else:
        from s04 import go_environment
        binary, version = Path(binary), go_version(go_environment())
    fresh = strip_timing(run_oracle(oracle, binary, rows, jobs=jobs, workdir=TARGET / f"native-{oracle}-verify"))
    differences = compare_rows(document["rows"], fresh)
    shutil.rmtree(TARGET / f"native-{oracle}-verify", ignore_errors=True)
    return {"oracle": oracle, "rows": len(fresh), "differences": len(differences), "first": differences[:5],
            "go_version": version, "binary_sha256": file_sha256(binary),
            "binary_equal": file_sha256(binary) == document["oracle_binary_sha256"]}


# ---------------------------------------------------------------------------
# instrumented build

def load_patches():
    return strict_json_loads((TOOL / "patches.json").read_bytes())


def splice(text, patch, context):
    count = text.count(patch["old"])
    if count != 1:
        raise ValueError(f"{context}: instrumentation anchor occurs {count} times (drifted oracle source?)")
    return text.replace(patch["old"], patch["new"])


def instrumented_sources(oracle):
    """Every Go file the instrumented build adds to the export, after splicing."""
    spec = oracle_spec(oracle)
    if spec.kind == "batch":
        return driver_sources(oracle, cover=True)
    patches = load_patches()["oracles"][oracle]
    files = {}
    package = f"tsc/{spec.package}"
    for name in spec.sources:
        text = (ROOT / spec.source_dir / name).read_text(encoding="utf-8")
        for patch in patches:
            if patch["file"] == name:
                text = splice(text, patch, f"{spec.source_dir}/{name}")
        files[f"{package}/{name}"] = text.encode()
    unknown = {patch["file"] for patch in patches} - set(spec.sources)
    if unknown:
        raise ValueError(f"patches name files outside the oracle: {sorted(unknown)}")
    files[f"{package}/phase1_cover.go"] = (TOOL / "phase1_cover.go").read_bytes()
    for source, target in spec.bridges:
        files[f"tsc/{target}"] = (ROOT / spec.source_dir / source).read_bytes()
    return files


def cover_flags(oracle):
    spec = oracle_spec(oracle)
    packages = [f"{MODULE}/{spec.package}"] + [f"{MODULE}/{package}" for package in COVER_PACKAGES]
    return ["-trimpath", "-mod=readonly", "-cover", "-covermode=atomic", "-coverpkg=" + ",".join(packages)]


def instrumentation_digest(oracle):
    files = instrumented_sources(oracle)
    document = {"files": {path: sha256(content) for path, content in sorted(files.items())},
                "flags": cover_flags(oracle)}
    if oracle_spec(oracle).kind == "process":
        document["patches_sha256"] = file_sha256(TOOL / "patches.json")
    return sha256(canonical(document))


def reach_environment(oracle):
    """Environment the instrumented runs add: syntax parses its libraries on every row."""
    return {"PHASE1_SYNTAX_MEMO": "0"} if oracle == "syntax" else {}


def build_instrumented(oracle):
    if oracle == "binder":
        command([sys.executable, "tools/s07/binder/generate_syntax.py", "--check"], cwd=ROOT)
    return build_driver(oracle, instrumented_sources(oracle), cover_flags(oracle), TARGET / "go" / f"{oracle}-cover")


# ---------------------------------------------------------------------------
# coverage decoding (port of the go-coverage prototype covmap.py)

UNIT = re.compile(r"^(\d+): L(\d+):C(\d+) -- L(\d+):C(\d+) NS=(\d+) = (\d+)$")


def parse_dump(text):
    """`go tool covdata debugdump` text -> [(package path, [function])] in meta order."""
    packages, current, function = [], None, None
    for line in text.splitlines():
        if line.startswith("Package path: "):
            current = []
            packages.append((line[len("Package path: "):], current))
            function = None
        elif line.startswith("Func: "):
            if current is None:
                raise ValueError("debugdump function before its package")
            function = {"name": line[len("Func: "):], "units": [], "counts": [], "file": None, "literal": False}
            current.append(function)
        elif line.startswith("Srcfile: ") and function is not None:
            function["file"] = line[len("Srcfile: "):].removeprefix(MODULE_PREFIX)
        elif line.startswith("Literal: ") and function is not None:
            function["literal"] = line.endswith("true")
        elif function is not None and (match := UNIT.match(line)):
            if int(match.group(1)) != len(function["units"]):
                raise ValueError(f"debugdump unit order broken in {function['name']}")
            function["units"].append(tuple(int(group) for group in match.groups()[1:6]))
            function["counts"].append(int(match.group(7)))
    return packages


def load_inventory():
    by_file = {}
    for line in (ROOT / "data/go-functions.tsv").read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or line.startswith("file\t"):
            continue
        file, _package, _receiver, _name, start, end, identifier = line.split("\t")
        by_file.setdefault(file, []).append((int(start), int(end), identifier))
    return by_file


def attach_ids(packages, inventory):
    """Give each instrumented FuncDecl its operation id by file and line containment.

    Returns the mapping problems; a nonempty list refuses the decode. Package
    literals (``func.L<line>.C<col>``) are not FuncDecls and keep no id. Files
    outside the inventory (oracle main package, bridges) keep no id.
    """
    problems = []
    for _path, functions in packages:
        for function in functions:
            function["id"] = None
            function["entry"] = None
            if not function["units"] or function["file"] not in inventory or function["literal"]:
                continue
            first = min(unit[0] for unit in function["units"])
            last = max(unit[2] for unit in function["units"])
            hits = [entry for entry in inventory[function["file"]] if entry[0] <= first and last <= entry[1]]
            if len(hits) != 1:
                problems.append({"function": function["name"], "file": function["file"], "hits": [hit[2] for hit in hits]})
                continue
            function["id"] = hits[0][2]
            order = sorted(range(len(function["units"])), key=lambda index: function["units"][index][:2])
            function["entry"] = order[0]
    return problems


def uleb(data, at):
    value = shift = 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, at
        shift += 7


def parse_counters(data, meta_hash=None):
    """Yield (package index, function index, counters) from one counter-data file."""
    if data[:4] != b"\x00cwm":
        raise ValueError("not a coverage counter-data file")
    if int.from_bytes(data[4:8], "little") != 1:
        raise ValueError("unsupported counter-data file version")
    if meta_hash is not None and data[8:24] != meta_hash:
        raise ValueError("counter data belongs to a different meta-data file")
    if data[24] != 2 or data[25] != 0:
        raise ValueError("expected little-endian ULEB128 counter flavor")
    if data[-16:-12] != b"\x00cwm" or int.from_bytes(data[-8:-4], "little") != 1:
        raise ValueError("expected exactly one counter segment")
    at = 32
    entries = int.from_bytes(data[at:at + 8], "little")
    strings = int.from_bytes(data[at + 8:at + 12], "little")
    arguments = int.from_bytes(data[at + 12:at + 16], "little")
    at = (at + 16 + strings + arguments + 3) & ~3
    for _ in range(entries):
        count, at = uleb(data, at)
        package, at = uleb(data, at)
        function, at = uleb(data, at)
        counters = []
        for _ in range(count):
            value, at = uleb(data, at)
            counters.append(value)
        yield package, function, counters
    if at > len(data) - 16:
        raise ValueError("counter payload overruns its footer")


def meta_file(directory):
    files = sorted(Path(directory).glob("covmeta.*"))
    if len(files) != 1:
        raise ValueError(f"{directory}: expected one covmeta file, found {len(files)}")
    data = files[0].read_bytes()
    if data[:4] != b"\x00cvm":
        raise ValueError("not a coverage meta-data file")
    return files[0], data[24:40]


def read_stream(path):
    """Yield (row id, segment, counter payload) records of a PHASE1_COVER_STREAM file."""
    data = Path(path).read_bytes()
    at = 0
    while at < len(data):
        size = int.from_bytes(data[at:at + 4], "little")
        at += 4
        row = data[at:at + size].decode()
        at += size
        size = int.from_bytes(data[at:at + 2], "little")
        at += 2
        segment = data[at:at + size].decode()
        at += size
        size = int.from_bytes(data[at:at + 4], "little")
        at += 4
        if at + size > len(data):
            raise ValueError(f"{path}: truncated coverage record")
        yield row, segment, data[at:at + size]
        at += size


def entered(packages, payload, meta_hash, rule, prefixes=()):
    """Operation ids whose entry block ran in one snapshot, under the stage rule.

    ``rule`` is the segment's rule: ``production`` counts every mapped
    function, ``observe`` only ids starting with one of ``prefixes``.
    """
    if rule not in ("production", "observe"):
        raise ValueError(f"unknown segment rule {rule!r}")
    if rule == "observe" and not prefixes:
        return set()
    result = set()
    for package, function, counters in parse_counters(payload, meta_hash):
        item = packages[package][1][function]
        if len(counters) != len(item["units"]):
            raise ValueError(f"counter/unit length mismatch for {item['name']}")
        identifier = item["id"]
        if identifier is None or counters[item["entry"]] == 0:
            continue
        if rule == "observe" and not identifier.startswith(tuple(prefixes)):
            continue
        result.add(identifier)
    return result


def _decode_shard(job):
    oracle, stream, dump, meta_hash = job
    spec = oracle_spec(oracle)
    packages = parse_dump(dump)
    problems = attach_ids(packages, load_inventory())
    if problems:
        raise ValueError(f"coverage functions without a unique inventory home: {problems[:5]}")
    rows, segments = {}, {}
    for row, segment, payload in read_stream(stream):
        rule = spec.segments.get(segment)
        if rule is None:
            raise ValueError(f"unknown coverage segment {segment!r}")
        ids = entered(packages, payload, meta_hash, rule, spec.observe_counts)
        rows.setdefault(row, set()).update(ids)
        # Per segment: snapshots, and per operation the snapshots entering it,
        # so the document can count stable operations only.
        record = segments.setdefault(segment, [0, {}])
        record[0] += 1
        for identifier in ids:
            record[1][identifier] = record[1].get(identifier, 0) + 1
    return rows, segments


def merge_shards(decoded):
    """Merge per-shard (rows, segments) decodes; a row decoded twice refuses."""
    row_ops, segments = {}, {}
    for shard_rows, shard_segments in decoded:
        for row, ids in shard_rows.items():
            if row in row_ops:
                raise ValueError(f"row {row} was run by two processes")
            row_ops[row] = ids
        for segment, (snapshots, counts) in shard_segments.items():
            total = segments.setdefault(segment, [0, {}])
            total[0] += snapshots
            for identifier, count in counts.items():
                total[1][identifier] = total[1].get(identifier, 0) + count
    return row_ops, segments


def debugdump(directory, env, *, live=False):
    args = ["go", "tool", "covdata", "debugdump"] + (["-live"] if live else []) + ["-i=" + str(directory)]
    result = subprocess.run(args, cwd=ROOT, env=env, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"covdata debugdump failed: {result.stderr.decode(errors='replace')[:500]}")
    return result.stdout.decode()


def covdata_check(meta_directory, stream, env, samples, packages, spec):
    """Cross-check the in-process decoder with `go tool covdata debugdump -live`.

    For each sampled snapshot, every function with a nonzero counter must
    appear in covdata's live dump with the same package, name, source file and
    counter vector, and nothing else may. ``packages`` is the mapped meta-data
    dump; each checked sample also carries the operations the snapshot enters
    under the stage rule (``ids``), so the document can count the stable ones.
    """
    meta, meta_hash = meta_file(meta_directory)
    checked = []
    for index, (row, segment, payload) in enumerate(read_stream(stream)):
        if index not in samples:
            continue
        with tempfile.TemporaryDirectory(dir=TARGET) as scratch:
            shutil.copyfile(meta, Path(scratch) / meta.name)
            (Path(scratch) / f"covcounters.{meta_hash.hex()}.1.1").write_bytes(payload)
            live = parse_dump(debugdump(scratch, env, live=True))
        theirs = {(path, function["name"], function["file"]): tuple(function["counts"])
                  for path, functions in live for function in functions}
        mine = {}
        for package, function, counters in parse_counters(payload, meta_hash):
            if any(counters):
                path, functions = packages[package]
                item = functions[function]
                mine[(path, item["name"], item["file"])] = tuple(counters)
        if mine != theirs:
            missing = sorted(set(theirs) - set(mine))[:3]
            extra = sorted(set(mine) - set(theirs))[:3]
            raise ValueError(f"decoder disagrees with covdata on {row} {segment}: missing {missing}, extra {extra}")
        if segment not in spec.segments:
            raise ValueError(f"unknown coverage segment {segment!r}")
        ids = entered(packages, payload, meta_hash, spec.segments[segment], spec.observe_counts)
        checked.append({"row": row, "segment": segment, "live_functions": len(mine), "ids": ids})
    if len(checked) != len(samples):
        raise ValueError("coverage stream has fewer records than the covdata sample")
    return checked


def select_rows(rows, only):
    if not only:
        return list(range(len(rows)))
    positions = {row: index for index, (row, _, _) in enumerate(rows)}
    missing = [row for row in only if row not in positions]
    if missing:
        raise ValueError(f"rows not in the inventory: {missing}")
    return [positions[row] for row in only]


def decode_run(oracle, workdir, env, *, covdata_samples=0):
    """Decode one instrumented run: {row: set(op)}, per-segment counts, the covdata check."""
    spec = oracle_spec(oracle)
    shards = sorted(Path(workdir).glob("shard-*.cover"), key=lambda path: int(path.name.split("-")[1].split(".")[0]))
    metas = [meta_file(Path(workdir) / path.name.replace(".cover", ".meta")) for path in shards]
    if len({meta.read_bytes() for meta, _ in metas}) != 1:
        raise ValueError("instrumented processes wrote different coverage meta-data")
    meta_directory, meta_hash = metas[0][0].parent, metas[0][1]
    dump = debugdump(meta_directory, env)
    packages = parse_dump(dump)
    problems = attach_ids(packages, load_inventory())
    if problems:
        raise ValueError(f"coverage functions without a unique inventory home: {problems[:5]}")
    instrumented = [path.removeprefix(MODULE + "/") for path, _ in packages]
    covdata = (covdata_check(meta_directory, shards[0], env, set(range(covdata_samples)), packages, spec)
               if covdata_samples else [])
    work = [(oracle, str(path), dump, meta_hash) for path in shards]
    if len(work) == 1:
        decoded = [_decode_shard(work[0])]
    else:
        with multiprocessing.get_context("spawn").Pool(len(work)) as pool:
            decoded = pool.map(_decode_shard, work)
    row_ops, segments = merge_shards(decoded)
    return {"row_ops": row_ops, "segments": segments, "covdata": covdata, "instrumented": instrumented,
            "processes": len(shards)}


def op_index(rows, row_ops):
    """{op: [row index]} in native row order."""
    ops = {}
    for index, (row, _, _) in enumerate(rows):
        for op in row_ops[row]:
            ops.setdefault(op, []).append(index)
    return {op: ops[op] for op in sorted(ops)}


def unstable_operations(first, second):
    """POOL_OPERATIONS plus every operation whose row set differs between two runs.

    The row counts are run statistics: they go to the unbound log, never into
    the reach file, which keeps only each operation's reason.
    """
    detail = {}
    for op in sorted(set(first) | set(second) | set(POOL_OPERATIONS)):
        left, right = first.get(op, []), second.get(op, [])
        if op in POOL_OPERATIONS or left != right:
            detail[op] = {"reason": "pool" if op in POOL_OPERATIONS else "run_dependent",
                          "rows": [len(left), len(right)],
                          "rows_differing": len(set(left) ^ set(right))}
    return detail


def stable_index(rows, runs):
    """(stable {op: [row index]}, unstable detail) of two decoded runs over the same rows."""
    if len(runs) != 2:
        raise ValueError("a reach index needs exactly two instrumented runs")
    first, second = (op_index(rows, run["row_ops"]) for run in runs)
    unstable = unstable_operations(first, second)
    return {op: indices for op, indices in first.items() if op not in unstable}, unstable


def planned_operations(path=None):
    """The operations of the plan's ``homes``: the set the reach summary is taken over."""
    path = Path(path) if path else ROOT / MANIFEST
    try:
        manifest = strict_json_loads(path.read_bytes())
    except OSError as error:
        raise ValueError(f"{path} is unreadable ({error}); run phase1_mutation_plan.py plan first") from error
    homes = manifest.get("homes") if isinstance(manifest, dict) else None
    if not isinstance(homes, dict) or not homes:
        raise ValueError(f"{path} records no plan homes; the reach summary is taken over them")
    return frozenset(homes)


def reach_summary(ops, unstable, planned):
    """Counts over the stable index; the planned figures over the plan's homes operations."""
    entered = set(ops)
    return {"operations_entered": len(ops), "unstable_operations": len(unstable),
            "index_entries": sum(map(len, ops.values())),
            "planned": {"source": f"homes of {MANIFEST}", "operations": len(planned),
                        "operations_sha256": sha256(canonical(sorted(planned))),
                        "entered": len(entered & planned), "unstable": sorted(set(unstable) & planned)}}


def reach_document(oracle, rows, runs, *, native_document, native_sha256, go_version, binary_sha256, planned):
    """The reach file and, separately, the run statistics it must not contain.

    Everything in the document is reproduced by a rerun with the same process
    counts: the index and every count in it are over stable operations only,
    ``unstable_detail`` keeps reasons only, and the summary depends on the
    plan's homes set, not on the coverage report. Row counts of the unstable
    operations and counts over every entered function go to the statistics.
    """
    spec = oracle_spec(oracle)
    ops, unstable = stable_index(rows, runs)
    files = instrumented_sources(oracle)

    def stable_count(counts):
        return sum(count for op, count in counts.items() if op not in unstable)

    document = {
        "version": VERSION, "oracle": oracle, "pin": pin(), "go_version": go_version,
        "instrumentation_sha256": instrumentation_digest(oracle),
        "instrumentation": {"files": {path: sha256(content) for path, content in sorted(files.items())},
                            "flags": cover_flags(oracle), "instrumented_packages": runs[0]["instrumented"],
                            "environment": reach_environment(oracle),
                            **({"patches_sha256": file_sha256(TOOL / "patches.json")} if spec.kind == "process" else {})},
        "binary_sha256": binary_sha256, "native_sha256": native_sha256,
        "requests_sha256": native_document["requests_sha256"], "rows_checked": runs[0]["rows_checked"],
        "processes": runs[0]["processes"],
        "stage_rule": stage_rule(oracle), "stage_rule_sha256": stage_rule_digest(oracle),
        "go_functions_sha256": file_sha256(ROOT / "data/go-functions.tsv"),
        "covdata_check": [{"row": item["row"], "segment": item["segment"],
                           "stable_entered": len(set(item["ids"]) - set(unstable))} for item in runs[0]["covdata"]],
        "segments": {segment: {"snapshots": snapshots, "stable_entered": stable_count(counts)}
                     for segment, (snapshots, counts) in sorted(runs[0]["segments"].items())},
        "runs": [{"processes": run["processes"], "rows_checked": run["rows_checked"], "equal_to_native": True}
                 for run in runs],
        "pool_operations": list(POOL_OPERATIONS), "unstable_rule": UNSTABLE_RULE,
        "unstable_ops": sorted(unstable),
        "unstable_detail": {op: {"reason": item["reason"]} for op, item in unstable.items()},
        "summary": reach_summary(ops, unstable, planned),
        "row_encoding": ROW_ENCODING,
        "op_row_gaps": {op: encode_gaps(indices) for op, indices in ops.items()},
    }
    first, second = (op_index(rows, run["row_ops"]) for run in runs)
    statistics = {
        "oracle": oracle, "unbound": "run statistics; nothing binds this file",
        "runs": [{"processes": run["processes"], "operations_entered": len(index),
                  "segments": {segment: {"snapshots": snapshots, "entered": sum(counts.values()),
                                         "stable_entered": stable_count(counts)}
                               for segment, (snapshots, counts) in sorted(run["segments"].items())}}
                 for run, index in zip(runs, (first, second))],
        "unstable_detail": unstable,
        "covdata_check": [{"row": item["row"], "segment": item["segment"], "live_functions": item["live_functions"],
                           "entered": len(item["ids"])} for item in runs[0]["covdata"]],
    }
    statistics["stable_segments_equal_between_runs"] = (
        statistics["runs"][0]["segments"].keys() == statistics["runs"][1]["segments"].keys()
        and all(statistics["runs"][0]["segments"][segment]["stable_entered"]
                == statistics["runs"][1]["segments"][segment]["stable_entered"]
                for segment in statistics["runs"][0]["segments"]))
    return document, statistics


def stats_path(oracle, *, verify=False):
    return TARGET / "go" / f"{'verify-' if verify else ''}reach-{oracle}.stats.json"


def reach(oracle, *, jobs=8, second_jobs=5, only=None, out=None, covdata_samples=6, stats=None):
    """Two instrumented runs + decode. With ``only`` it is a diagnostic sample, never written."""
    started = time.monotonic()
    native_document = load_native(oracle)
    rows = load_requests(oracle)
    selected = select_rows(rows, only)
    # A diagnostic sample may run before any plan exists; a written index may not.
    planned = planned_operations() if not only or (ROOT / MANIFEST).exists() else None
    binary, version, env = build_instrumented(oracle)
    run_env = dict(env, **reach_environment(oracle))
    shardings = [1] if only else [jobs, second_jobs]
    runs = []
    for number, processes in enumerate(shardings, 1):
        workdir = TARGET / (f"reach-{oracle}-sample" if only else f"reach-{oracle}-{number}")
        results = run_oracle(oracle, binary, [rows[index] for index in selected], jobs=processes,
                             workdir=workdir, env=run_env, cover=True)
        expected = [native_document["rows"][index] for index in selected]
        differences = compare_rows(expected, strip_timing(results))
        if differences:
            raise RuntimeError(f"{oracle}: instrumented run {number} differs from the frozen native digests on "
                               f"{len(differences)} rows, first {differences[:3]}")
        decoded = decode_run(oracle, workdir, env, covdata_samples=covdata_samples if number == 1 else 0)
        for index in selected:
            if rows[index][0] not in decoded["row_ops"]:
                raise ValueError(f"row {rows[index][0]} has no coverage snapshot in run {number}")
        decoded["rows_checked"] = len(results)
        runs.append(decoded)
        if not only:
            shutil.rmtree(workdir)
    if only:
        report = []
        for index in selected:
            row = rows[index][0]
            ids = runs[0]["row_ops"][row]
            item = {"row": row, "entered": len(ids)}
            if planned is not None:
                item["planned_entered"] = len(ids & planned)
            report.append(item)
        segments = {segment: {"snapshots": snapshots, "entered": sum(counts.values())}
                    for segment, (snapshots, counts) in sorted(runs[0]["segments"].items())}
        return {"oracle": oracle, "sample": report, "segments": segments,
                "covdata_check": [{key: value for key, value in item.items() if key != "ids"}
                                  for item in runs[0]["covdata"]],
                "instrumented_packages": runs[0]["instrumented"]}
    document, statistics = reach_document(
        oracle, rows, runs, native_document=native_document, native_sha256=file_sha256(native_path(oracle)),
        go_version=version, binary_sha256=file_sha256(binary), planned=planned)
    out = Path(out) if out else reach_path(oracle)
    write_ops_document(out, document)
    statistics.update(file=str(out.relative_to(ROOT) if out.is_relative_to(ROOT) else out),
                      file_sha256=file_sha256(out), seconds=round(time.monotonic() - started, 1))
    stats = Path(stats) if stats else stats_path(oracle)
    stats.parent.mkdir(parents=True, exist_ok=True)
    stats.write_bytes(canonical(statistics) + b"\n")
    return {"oracle": oracle, "path": statistics["file"], "bytes": out.stat().st_size,
            "file_sha256": statistics["file_sha256"], "rows_checked": document["rows_checked"],
            "summary": document["summary"], "unstable_ops": document["unstable_detail"],
            "runs": document["runs"], "segments": document["segments"],
            "covdata_check": len(document["covdata_check"]),
            "stats": str(stats.relative_to(ROOT) if stats.is_relative_to(ROOT) else stats),
            "seconds": statistics["seconds"]}


def reach_differences(committed, fresh):
    """Where two reach documents differ: header fields, then index operations."""
    header = sorted(key for key in set(committed) | set(fresh)
                    if key != "op_row_gaps" and committed.get(key) != fresh.get(key))
    left, right = committed.get("op_row_gaps") or {}, fresh.get("op_row_gaps") or {}
    return {"header": header,
            "only_committed": sorted(set(left) - set(right))[:20], "only_fresh": sorted(set(right) - set(left))[:20],
            "rows_differ": sorted(op for op in set(left) & set(right) if left[op] != right[op])[:20]}


def verify_reach(oracle, *, jobs=None, second_jobs=None, covdata_samples=None):
    """Rerun the instrumented oracle with the recorded process counts; require a byte-identical file."""
    committed = reach_path(oracle)
    document = read_document(committed)
    recorded = [run.get("processes") for run in document.get("runs") or []]
    if len(recorded) != 2 or not all(type(count) is int and count > 0 for count in recorded):
        raise ValueError(f"{committed} does not record the process counts of two runs")
    jobs, second_jobs = jobs or recorded[0], second_jobs or recorded[1]
    if covdata_samples is None:
        covdata_samples = len(document.get("covdata_check") or [])
    fresh = TARGET / f"verify-go-reach-{oracle}.json.gz"
    result = reach(oracle, jobs=jobs, second_jobs=second_jobs, out=fresh, covdata_samples=covdata_samples,
                   stats=stats_path(oracle, verify=True))
    identical = fresh.read_bytes() == committed.read_bytes()
    report = {"oracle": oracle,
              "committed": str(committed.relative_to(ROOT) if committed.is_relative_to(ROOT) else committed),
              "committed_sha256": file_sha256(committed), "fresh_sha256": result["file_sha256"],
              "identical": identical, "processes": [jobs, second_jobs],
              "problems": reach_problems(oracle, document, file_sha256(native_path(oracle))),
              "stats": result["stats"], "seconds": result["seconds"]}
    if identical:
        fresh.unlink()
    else:
        report["fresh"] = str(fresh.relative_to(ROOT) if fresh.is_relative_to(ROOT) else fresh)
        report["differences"] = reach_differences(document, read_document(fresh))
    return report


ROW_ENCODING = ("op_row_gaps[op] lists the rows on which Go entered op as gaps: the first entry is a row "
                "index, each later entry is added to the previous index; indices are positions in the rows "
                "of the native file named by native_sha256. Read it with phase1_mutation_go.load_reach.")


def encode_gaps(indices):
    if any(later <= earlier for earlier, later in zip(indices, indices[1:])) or (indices and indices[0] < 0):
        raise ValueError("row indices must be strictly increasing and nonnegative")
    return [index - previous for previous, index in zip([0] + indices[:-1], indices)] if indices else []


def decode_gaps(gaps):
    indices, current = [], 0
    for position, gap in enumerate(gaps):
        if type(gap) is not int or gap < (0 if position == 0 else 1):
            raise ValueError("invalid row gap")
        current += gap
        indices.append(current)
    return indices


def write_ops_document(path, document):
    """Deterministic gzip: canonical header, then one operation per line."""
    field = "op_row_gaps"
    header = {key: value for key, value in document.items() if key != field}
    lines = [canonical(op) + b":" + canonical(rows) for op, rows in document[field].items()]
    raw = canonical(header)[:-1] + b',"' + field.encode() + b'":{\n' + b",\n".join(lines) + b"\n}}\n"
    if strict_json_loads(raw) != document:
        raise AssertionError("reach document does not round-trip")
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".partial")
    with temporary.open("wb") as stream:
        with gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0, compresslevel=9) as compressed:
            compressed.write(raw)
    temporary.replace(path)


def reach_problems(oracle, document, native_sha256):
    """Why a reach document no longer binds the current code and native file (child-free)."""
    problems = []
    if document.get("version") != VERSION or document.get("oracle") != oracle or document.get("pin") != pin():
        problems.append("wrong version, oracle or pin")
    if document.get("native_sha256") != native_sha256:
        problems.append("bound to a different native file")
    if document.get("row_encoding") != ROW_ENCODING:
        problems.append("unknown row encoding")
    if document.get("stage_rule") != stage_rule(oracle) or document.get("stage_rule_sha256") != stage_rule_digest(oracle):
        problems.append("decoded under a different stage rule")
    if document.get("instrumentation_sha256") != instrumentation_digest(oracle):
        problems.append("built from different instrumentation")
    if document.get("go_functions_sha256") != file_sha256(ROOT / "data/go-functions.tsv"):
        problems.append("mapped against a different data/go-functions.tsv")
    unstable = document.get("unstable_ops")
    index = document.get("op_row_gaps")
    if not isinstance(unstable, list) or not set(POOL_OPERATIONS) <= set(unstable):
        problems.append("does not list every pool operation as unstable")
    elif not isinstance(index, dict) or set(unstable) & set(index):
        problems.append("indexes an unstable operation")
    return problems


def load_reach(oracle, path=None):
    """The Go-reach index as {op: frozenset(row ids)}, bound to the current native file and rules.

    Unstable operations are absent: they are never candidates.
    """
    path = Path(path) if path else reach_path(oracle)
    document = read_document(path)
    native_file = native_path(oracle)
    problems = reach_problems(oracle, document, file_sha256(native_file))
    if problems:
        raise ValueError(f"{path}: {'; '.join(problems)}")
    rows = [row["row"] for row in read_document(native_file)["rows"]]
    if document["rows_checked"] != len(rows):
        raise ValueError(f"{path}: checked {document['rows_checked']} rows, native file has {len(rows)}")
    result = {}
    for op, gaps in document["op_row_gaps"].items():
        indices = decode_gaps(gaps)
        if indices and indices[-1] >= len(rows):
            raise ValueError(f"{path}: {op} names a row outside the native file")
        result[op] = frozenset(rows[index] for index in indices)
    return result


# ---------------------------------------------------------------------------
# command line

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("requests", "native", "verify-native", "reach", "verify-reach"):
        sub = commands.add_parser(name)
        sub.add_argument("--oracle", choices=sorted(ORACLES), action="append",
                         help="repeatable; default: every oracle")
        if name == "requests":
            sub.add_argument("--export", help="e1/binder/facts: reuse an existing S06 preprocessing export; "
                                              "syntax: a TestS07SubsetExport observation file instead of the "
                                              "committed capture (rows are re-authenticated either way)")
        if name in ("native", "verify-native", "reach"):
            sub.add_argument("--jobs", type=int, default=8)
        if name in ("native", "reach"):
            sub.add_argument("--second-jobs", type=int, default=5, help="process count of the second run")
        if name in ("native", "verify-native"):
            sub.add_argument("--binary", help="use this plain oracle binary instead of rebuilding")
        if name == "native":
            sub.add_argument("--no-probe-check", action="store_true",
                             help="syntax: skip the rerun of the committed go test probe")
        if name == "reach":
            sub.add_argument("--rows", help="comma-separated row ids: a fresh single-process diagnostic sample, never written")
        if name == "verify-reach":
            sub.add_argument("--jobs", type=int, help="process count of the first run (default: the recorded one)")
            sub.add_argument("--second-jobs", type=int,
                             help="process count of the second run (default: the recorded one)")
    args = parser.parse_args(argv)
    oracles = args.oracle or sorted(ORACLES)
    failed = False
    try:
        for oracle in oracles:
            if args.command == "requests":
                result = materialize(oracle, export=args.export)
            elif args.command == "native":
                result = native(oracle, jobs=args.jobs, second_jobs=args.second_jobs, binary=args.binary,
                                probe_check=not args.no_probe_check)
            elif args.command == "verify-native":
                result = verify_native(oracle, jobs=args.jobs, binary=args.binary)
            elif args.command == "verify-reach":
                result = verify_reach(oracle, jobs=args.jobs, second_jobs=args.second_jobs)
                failed |= not result["identical"]
            else:
                only = [row for row in args.rows.split(",") if row] if args.rows else None
                result = reach(oracle, jobs=args.jobs, second_jobs=args.second_jobs, only=only)
            print(json.dumps(result, sort_keys=True))
            if args.command == "verify-native" and result["differences"]:
                return 1
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"phase1 mutation go: {error}", file=sys.stderr)
        return 1
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
