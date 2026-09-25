#!/usr/bin/env python3
"""Phase 1 operation tables: specs, row selection, materialization, scratch campaigns.

docs/PHASE1-mutation-witnesses.md section 9 is the contract. The ``table``
mutation oracle witnesses operations through columns: one request row is one
``(column, input)`` pair, the native driver is ``tools/phase1/tables/go`` and
the Rust side is ``tools/phase1/mutation/driver/src/table``. This module owns
everything before the oracle runs:

* ``data/phase1/tables/<group>.json``, one table spec per group, each column
  naming its claimed operations, input kind, projection (with the caller
  evidence behind it), Rust entry point, and row sources (the S06 survey,
  synthetic inputs, or both). ``check`` validates the specs and requires the Go
  column registry and the Rust ``COLUMNS`` lists to name exactly the spec's
  columns, group by group, and checks the committed inventory's rows (child-free);
  it lists the specs changed since the inventory was selected and fails while
  any has (the scope check stales the table witness then too), unless
  ``--allow-stale`` (a group's package mid-phase, before integration re-selects).
* The survey (part of ``select``) runs the Go driver's survey mode over the
  22,343 S06 primary requests: per file and per surveyed column, the column's
  selection classes. Classes only choose inputs; every expected value comes
  from the native run.
* ``select`` covers each surveyed column's classes greedily (largest gain
  first, then the smaller file, then the row id), appends the spec's synthetic
  inputs, and writes the committed inventory ``data/phase1/tables/requests.json``
  (``--write``) or a scratch one (``--out``). ``--check`` reruns the survey and
  the cover and requires the committed inventory byte for byte.
* ``materialize`` expands the inventory into request lines (S06 rows take
  their parse input from the authenticated S06 requests), each checked against
  its ``request_sha256``: ``phase1_mutation_go.py requests --oracle table``.
* ``campaign`` is the scratch table campaign of one or more groups: select,
  materialize, native freeze, plan of the groups' claimed operations, Go reach,
  schemata, trace, kill, results and (``--confirm``) the confirmation replay,
  all under ``--out``; nothing under ``data/`` changes. It prints each column's
  parity and each claimed operation's mutation state.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import heapq
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04_common import strict_json_loads  # noqa: E402
from s06_protocol import canonical  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SPEC_DIRECTORY = "data/phase1/tables"
INVENTORY = f"{SPEC_DIRECTORY}/requests.json"
GO_DIRECTORY = "tools/phase1/tables/go"
RUST_DIRECTORY = "tools/phase1/mutation/driver/src/table"
TARGET = ROOT / "target/phase1-tables"
DEFAULT_WS = ROOT / "target/phase1-mutation/ws-table"
# The groups: the harness's own (runtime) and the nine stage-2 groups, each
# with its spec, its Go column file and its Rust column module.
GROUPS = ("runtime", "class", "modules", "positions", "targets", "containers", "diagnostics", "core",
          "concurrency", "tsoptions")
# The parsed kinds (S06 parse inputs, which may be surveyed): source and bound
# take the plain walk, source_jsdoc and bound_jsdoc the JSDoc-inclusive one.
SURVEYED_KINDS = ("source", "bound", "source_jsdoc", "bound_jsdoc")
INPUT_KINDS = SURVEYED_KINDS + ("config", "values")
COLUMN_FIELDS = ("id", "operations", "input", "projection", "evidence", "rust", "survey", "synthetic")
OPTIONAL_FIELDS = ("panic_contract", "notes")
SOURCE_FIELDS = ("filename", "path", "jsx", "force", "script_kind", "source_hex")
SELECTION_RULE = ("per surveyed column: greedy cover of the union of the S06 files' survey classes, taking at "
                  "each step the file that adds the most uncovered classes (ties: fewer bytes, then the smaller "
                  "row id); then the spec's synthetic inputs in spec order. Rows are ordered by group (GROUPS "
                  "order), column (spec order), survey rows in choice order, then synthetic rows.")
S06_INVENTORY = "data/s06/requests.json"
S06_PROBES = "data/s06/probes.json"
_COLUMN_ID = re.compile(r"[A-Za-z][A-Za-z0-9_.]*")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_sha256(path):
    return sha256(Path(path).read_bytes())


def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def spec_path(group, root=ROOT):
    return Path(root) / SPEC_DIRECTORY / f"{group}.json"


# ---------------------------------------------------------------------------
# specs and registries

def load_specs(groups=None, root=ROOT):
    """{group: spec} for ``groups`` (default: every group), in GROUPS order."""
    selected = list(GROUPS) if groups is None else [group for group in GROUPS if group in set(groups)]
    unknown = sorted(set(groups or ()) - set(GROUPS))
    if unknown:
        raise ValueError(f"unknown table groups {unknown}; expected some of {list(GROUPS)}")
    return {group: strict_json_loads(spec_path(group, root).read_bytes()) for group in selected}


def columns(specs):
    """[(group, column)] in inventory order."""
    return [(group, column) for group, spec in specs.items() for column in spec.get("columns") or []]


def claimed_operations(specs=None):
    """The operations the table columns claim, sorted (the plan's table operations)."""
    specs = load_specs() if specs is None else specs
    return sorted({op for _, column in columns(specs) for op in column.get("operations") or []})


def go_operation_ids(root=ROOT):
    ids = set()
    for line in (Path(root) / "data/go-functions.tsv").read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or line.startswith("file\t"):
            continue
        ids.add(line.split("\t")[-1])
    return ids


def go_registry(root=ROOT):
    """{column id: (group, input, surveyed)} as the Go column files register them.

    A static reading of tools/phase1/tables/go/<group>_columns.go: each file's
    Register("<group>", Column{ID: ..., Input: ..., Survey: ...}, ...) literals,
    and the generic shapes of columns.go: NodePredicate("<id>", "<input>", ...)
    and NodeMap("<id>", "<input>", ...), which always survey, and
    ValuesMap("<id>", ...) and a group's <name>ValuesColumn("<id>", ...) over
    synthetic values, which never do. The driver itself
    refuses a repeated id at startup.
    """
    found, problems = {}, []
    for path in sorted((Path(root) / GO_DIRECTORY).glob("*_columns.go")):
        text = path.read_text(encoding="utf-8")
        groups = re.findall(r'\bRegister\("([a-z]+)"', text)
        if groups != [path.name.removesuffix("_columns.go")]:
            problems.append(f"{path.name} must call Register exactly once, for its own group")
            continue
        for chunk in text.split("Column{")[1:]:
            identity = re.search(r'\bID:\s*"([^"]+)"', chunk)
            kind = re.search(r'\bInput:\s*"([^"]+)"', chunk)
            if identity is None and re.match(r'\s*ID:\s*[a-z]\w*,', chunk):
                continue  # a helper's template (ID: id), registered through its calls
            if identity is None or kind is None:
                problems.append(f"{path.name}: a Column literal without ID or Input")
                continue
            # The literal ends where the next one would start; Survey is a field of this one.
            surveyed = re.search(r"\bSurvey:", chunk) is not None
            if identity.group(1) in found:
                problems.append(f"column {identity.group(1)} is registered twice in Go")
            found[identity.group(1)] = (groups[0], kind.group(1), surveyed)
        for identity, kind in re.findall(r'\b(?:NodePredicate|NodeMap)\(\s*"([^"]+)",\s*"([^"]+)"', text):
            if identity in found:
                problems.append(f"column {identity} is registered twice in Go")
            found[identity] = (groups[0], kind, True)
        for identity in re.findall(r'\b(?:ValuesMap|\w+ValuesColumn)\(\s*"([^"]+)"', text):
            if identity in found:
                problems.append(f"column {identity} is registered twice in Go")
            found[identity] = (groups[0], "values", False)
    return found, problems


def rust_registry(root=ROOT):
    """{column id: group} from the COLUMNS lists of the Rust column modules."""
    found, problems = {}, []
    for group in GROUPS:
        path = Path(root) / RUST_DIRECTORY / f"{group}.rs"
        if not path.is_file():
            problems.append(f"{RUST_DIRECTORY}/{group}.rs is missing")
            continue
        match = re.search(r"pub const COLUMNS: &\[&str\] = &\[(.*?)\];", path.read_text(encoding="utf-8"), re.S)
        if match is None:
            problems.append(f"{RUST_DIRECTORY}/{group}.rs declares no COLUMNS list")
            continue
        for identity in re.findall(r'"([^"]+)"', match.group(1)):
            if identity in found:
                problems.append(f"column {identity} is declared twice in Rust")
            found[identity] = group
    return found, problems


def spec_problems(specs=None, root=ROOT, *, complete=True):
    """Every rule the specs and both registries break (child-free).

    With ``complete`` every group must have its spec; a scratch selection of
    some groups checks those groups only.
    """
    problems = []
    specs = load_specs(root=root) if specs is None else specs
    missing = [group for group in GROUPS if group not in specs]
    if missing and complete:
        problems.append(f"missing table specs: {missing}")
    operations = go_operation_ids(root)
    seen = {}
    for group, spec in specs.items():
        where = f"{SPEC_DIRECTORY}/{group}.json"
        if not isinstance(spec, dict) or spec.get("version") != 1 or spec.get("group") != group \
                or not isinstance(spec.get("columns"), list) or set(spec) - {"version", "group", "columns", "notes"}:
            problems.append(f"{where}: not a version 1 spec of group {group} with a column list")
            continue
        for column in spec["columns"]:
            identity = column.get("id") if isinstance(column, dict) else None
            label = f"{where}: column {identity!r}"
            if not isinstance(column, dict) or not set(COLUMN_FIELDS) <= set(column) \
                    or set(column) - set(COLUMN_FIELDS) - set(OPTIONAL_FIELDS):
                problems.append(f"{label} must carry exactly {', '.join(COLUMN_FIELDS)} "
                                f"(optionally {', '.join(OPTIONAL_FIELDS)})")
                continue
            if not isinstance(identity, str) or not _COLUMN_ID.fullmatch(identity):
                problems.append(f"{label}: malformed column id")
                continue
            if identity in seen:
                problems.append(f"{label}: also a column of {seen[identity]}")
            seen[identity] = group
            ops = column["operations"]
            if not isinstance(ops, list) or not all(isinstance(op, str) for op in ops) or len(set(ops)) != len(ops):
                problems.append(f"{label}: operations must be a list of distinct operation ids")
            else:
                if not ops and group != "runtime":
                    problems.append(f"{label}: only a runtime column may claim no operation")
                if ops and group == "runtime":
                    problems.append(f"{label}: a runtime column claims no operation")
                problems += [f"{label}: {op} is not in data/go-functions.tsv" for op in ops if op not in operations]
            if column["input"] not in INPUT_KINDS:
                problems.append(f"{label}: input must be one of {list(INPUT_KINDS)}")
            for field in ("projection", "rust"):
                if not isinstance(column[field], str) or not column[field].strip():
                    problems.append(f"{label}: {field} must be a nonempty string")
            if not isinstance(column["evidence"], list) or not all(isinstance(item, str) for item in column["evidence"]):
                problems.append(f"{label}: evidence must be a list of caller file:line strings")
            if type(column["survey"]) is not bool or (column["survey"] and column["input"] not in SURVEYED_KINDS):
                problems.append(f"{label}: survey must be a boolean, true only for {list(SURVEYED_KINDS)} inputs")
            synthetic = column["synthetic"]
            names = [row.get("name") for row in synthetic if isinstance(row, dict)] if isinstance(synthetic, list) else []
            if (not isinstance(synthetic, list) or len(names) != len(synthetic)
                    or not all(isinstance(name, str) and name and "@" not in name for name in names)
                    or len(set(names)) != len(names)
                    or not all(isinstance(row.get("input"), dict) and set(row) <= {"name", "input", "notes"}
                               for row in synthetic)):
                problems.append(f"{label}: synthetic must be a list of {{name, input}} rows with distinct names")
            elif not column["survey"] and not synthetic:
                problems.append(f"{label}: a column needs survey rows or synthetic rows")
            contract = column.get("panic_contract", [])
            if not isinstance(contract, list) or not all(isinstance(item, str) and item for item in contract):
                problems.append(f"{label}: panic_contract must be a list of panic classes")
    registered, go_problems = go_registry(root)
    declared, rust_problems = rust_registry(root)
    problems += go_problems + rust_problems
    for group, column in columns(specs):
        identity = column.get("id") if isinstance(column, dict) else None
        if not isinstance(identity, str):
            continue
        entry = registered.get(identity)
        if entry is None:
            problems.append(f"column {identity} of {group} is not registered by {GO_DIRECTORY}/{group}_columns.go")
        elif entry != (group, column.get("input"), column.get("survey")):
            problems.append(f"column {identity}: the Go registry says group {entry[0]}, input {entry[1]}, "
                            f"surveyed {entry[2]}; the spec says {group}, {column.get('input')}, {column.get('survey')}")
        if declared.get(identity) != group:
            problems.append(f"column {identity} of {group} is not in the COLUMNS of {RUST_DIRECTORY}/{group}.rs")
    for identity, (group, _, _) in registered.items():
        if seen.get(identity) != group and group in specs:
            problems.append(f"Go registers column {identity} in {group}, which no {group} spec column names")
    for identity, group in declared.items():
        if seen.get(identity) != group and group in specs:
            problems.append(f"Rust declares column {identity} in {group}, which no {group} spec column names")
    return problems


def inventory_problems(document=None, specs=None, root=ROOT):
    """What the committed inventory breaks, child-free: its shape, the column and
    group of every row, the synthetic rows' request digests, and whether the
    specs it was selected from are the current ones (``stale``: rerun
    ``select --write``; ``select --check`` reruns the survey too)."""
    document = strict_json_loads((Path(root) / INVENTORY).read_bytes()) if document is None else document
    specs = load_specs(root=root) if specs is None else specs
    problems, stale = [], []
    if (document.get("version") != 1 or document.get("kind") != "phase1-table-inventory"
            or document.get("pin") != strict_json_loads((Path(root) / "data/upstream.json").read_bytes())["pin"]):
        problems.append(f"{INVENTORY} is not a version 1 table inventory at the current pin")
    known = {column["id"]: group for group, column in columns(specs) if isinstance(column, dict)}
    seen = set()
    for row in document.get("requests") or []:
        identity = row.get("id")
        if identity in seen:
            problems.append(f"{INVENTORY}: row {identity} occurs twice")
        seen.add(identity)
        if known.get(row.get("column")) != row.get("group"):
            problems.append(f"{INVENTORY}: row {identity} names column {row.get('column')!r} of group "
                            f"{row.get('group')!r}, which no spec declares")
        if ("s06" in row) == ("input" in row):
            problems.append(f"{INVENTORY}: row {identity} needs exactly one of s06 and input")
        elif "input" in row and sha256(canonical(request_for(row["column"], identity, row["input"]))) \
                != row.get("request_sha256"):
            problems.append(f"{INVENTORY}: row {identity} differs from its request_sha256")
    for relative, digest in sorted((document.get("specs") or {}).items()):
        path = Path(root) / relative
        if not path.is_file() or file_sha256(path) != digest:
            stale.append(relative)
    stale += sorted(f"{SPEC_DIRECTORY}/{group}.json" for group in specs
                    if f"{SPEC_DIRECTORY}/{group}.json" not in (document.get("specs") or {}))
    return problems, stale


# ---------------------------------------------------------------------------
# the Go driver

DRIVER_FLAGS = ("-trimpath", "-mod=readonly")


def driver_key(files, go_version):
    """What the plain table driver is built from: its sources (and bridges and
    cover stub), the pin it is exported from, the Go toolchain and the flags."""
    return sha256(canonical({"pin": pin(), "go": go_version, "flags": list(DRIVER_FLAGS),
                             "sources": {path: sha256(content) for path, content in sorted(files.items())}}))


def build_go_driver():
    """The plain table driver (no coverage), cached by ``driver_key``: a pin
    bump or another toolchain builds a new binary, never reuses the old one."""
    import phase1_mutation_go as go
    from s04 import go_environment
    files = go.driver_sources("table", cover=False)
    digest = driver_key(files, go.go_version(go_environment()))
    binary = TARGET / "driver" / f"phase1table-{digest[:16]}"
    if not binary.is_file():
        # Built beside its final name and renamed: concurrent callers never see
        # a partial binary.
        partial = binary.with_name(f"{binary.name}.{os.getpid()}.partial")
        go.build_driver("table", files, list(DRIVER_FLAGS), partial)
        partial.replace(binary)
    return binary, digest


def s06_requests():
    """The authenticated S06 primary requests (target/phase1-mutation/requests-e1.ndjson), by id, in order."""
    import phase1_mutation_go as go
    if not go.requests_path("e1").exists():
        go.materialize("e1")
    return {row: request for row, _, request in go.load_requests("e1")}


def s06_binding():
    probes = strict_json_loads((ROOT / S06_PROBES).read_bytes())
    return {"inventory": S06_INVENTORY, "inventory_file_sha256": file_sha256(ROOT / S06_INVENTORY),
            "sequence_sha256": probes["request_sha256"]}


# ---------------------------------------------------------------------------
# survey and selection

def survey(group_columns, requests_file, *, binary=None, out=None):
    """{column: [(row, frozenset(classes), bytes)]} and the survey's digest.

    One Go run over every S06 request for the given surveyed columns.
    """
    if not group_columns:
        return {}, None
    if binary is None:
        binary, _ = build_go_driver()
    TARGET.mkdir(parents=True, exist_ok=True)
    out = Path(out) if out else TARGET / f"survey-{os.getpid()}.ndjson"
    env = dict(os.environ, PHASE1_TABLE_COLUMNS=",".join(group_columns))
    completed = subprocess.run([str(binary), "survey", str(requests_file), str(out)], env=env, cwd=ROOT,
                               stdin=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
    if completed.returncode:
        raise RuntimeError(f"table survey failed: {completed.stderr.decode(errors='replace')[:2000]}")
    digest = hashlib.sha256()
    found = {column: [] for column in group_columns}
    interned = {}
    with out.open("rb") as stream:
        for line in stream:
            digest.update(line)
            record = strict_json_loads(line)
            for column, classes in record["classes"].items():
                found[column].append((record["row"], frozenset(interned.setdefault(item, item) for item in classes),
                                      record["bytes"]))
    out.unlink()
    return found, digest.hexdigest()


def greedy(rows):
    """Row ids covering every class of ``rows`` [(row, classes, bytes)], in choice order."""
    universe = frozenset().union(*(classes for _, classes, _ in rows)) if rows else frozenset()
    covered, chosen = set(), []
    heap = [(-len(classes), size, row, index) for index, (row, classes, size) in enumerate(rows) if classes]
    heapq.heapify(heap)
    while heap and len(covered) < len(universe):
        _, size, row, index = heapq.heappop(heap)
        gain = len(rows[index][1] - covered)
        if gain == 0:
            continue
        if heap and (-gain, size, row) > heap[0][:3]:
            heapq.heappush(heap, (-gain, size, row, index))
            continue
        chosen.append(row)
        covered |= rows[index][1]
    return chosen, len(universe)


def source_input(value):
    """A synthetic source/bound input: `source` text becomes source_hex; path defaults to filename."""
    value = dict(value)
    if "source" in value:
        if "source_hex" in value:
            raise ValueError("a synthetic source input names both source and source_hex")
        value["source_hex"] = value.pop("source").encode("utf-8").hex()
    value.setdefault("path", value.get("filename"))
    value.setdefault("jsx", False)
    value.setdefault("force", False)
    if set(value) != set(SOURCE_FIELDS):
        raise ValueError(f"a synthetic source input needs exactly {', '.join(SOURCE_FIELDS)} (source for source_hex)")
    return value


def request_for(column, identity, input_value):
    return {"id": identity, "column": column, "op": "table", "input": input_value}


def select(groups=None, *, specs=None, s06=None, binary=None):
    """The inventory document for ``groups`` (default: every group)."""
    specs = load_specs(groups) if specs is None else specs
    problems = spec_problems(specs, complete=groups is None)
    if problems:
        raise ValueError(f"table specs are invalid: {problems[:5]}")
    surveyed = {group: [column["id"] for column in spec["columns"] if column["survey"]] for group, spec in specs.items()}
    s06 = s06_requests() if s06 is None and any(surveyed.values()) else (s06 or {})
    requests_file = None
    if any(surveyed.values()):
        import phase1_mutation_go as go
        requests_file = go.requests_path("e1")
        if binary is None:
            binary, _ = build_go_driver()
    rows, record = [], {}
    for group, spec in specs.items():
        found, digest = survey(surveyed[group], requests_file, binary=binary)
        record[group] = {"spec": f"{SPEC_DIRECTORY}/{group}.json", "spec_sha256": file_sha256(spec_path(group)),
                         "survey_sha256": digest, "columns": {}}
        for column in spec["columns"]:
            identity, chosen, universe = column["id"], [], 0
            if column["survey"]:
                chosen, universe = greedy(found[identity])
            for s06_id in chosen:
                request = s06[s06_id]
                value = {field: request[field] for field in SOURCE_FIELDS}
                full = request_for(identity, f"{identity}@s06:{s06_id}", value)
                rows.append({"id": full["id"], "column": identity, "group": group, "s06": s06_id,
                             "request_sha256": sha256(canonical(full))})
            for synthetic in column["synthetic"]:
                value = synthetic["input"]
                if column["input"] in SURVEYED_KINDS:
                    value = source_input(value)
                full = request_for(identity, f"{identity}@{synthetic['name']}", value)
                rows.append({"id": full["id"], "column": identity, "group": group, "input": value,
                             "request_sha256": sha256(canonical(full))})
            record[group]["columns"][identity] = {"classes": universe, "survey_rows": len(chosen),
                                                  "synthetic_rows": len(column["synthetic"])}
    identities = [row["id"] for row in rows]
    if len(set(identities)) != len(identities):
        raise ValueError("two table rows share an id")
    selection = {"rule": SELECTION_RULE, "corpus": s06_binding(), "groups": record}
    return {"version": 1, "kind": "phase1-table-inventory", "pin": pin(),
            "specs": {entry["spec"]: entry["spec_sha256"] for entry in record.values()},
            "selection": selection,
            "selection_sha256": sha256(canonical({"selection": selection,
                                                  "rows": [[row["id"], row["request_sha256"]] for row in rows]})),
            "requests": rows}


def inventory_bytes(document):
    return json.dumps(document, indent=1, sort_keys=True, ensure_ascii=True).encode() + b"\n"


def materialize_requests(document, s06=None):
    """The full request of every inventory row, each checked against its request_sha256."""
    needs = any("s06" in row for row in document["requests"])
    s06 = s06_requests() if s06 is None and needs else (s06 or {})
    requests, mismatched = [], []
    for row in document["requests"]:
        if "s06" in row:
            request = s06.get(row["s06"])
            if request is None:
                raise ValueError(f"{row['id']}: S06 request {row['s06']} is not in the S06 inventory")
            value = {field: request[field] for field in SOURCE_FIELDS}
        else:
            value = row["input"]
        full = request_for(row["column"], row["id"], value)
        if sha256(canonical(full)) != row["request_sha256"]:
            mismatched.append(row["id"])
        requests.append(full)
    if mismatched:
        raise ValueError(f"{len(mismatched)} table requests differ from their request_sha256, first {mismatched[:3]}")
    return requests


# ---------------------------------------------------------------------------
# the scratch campaign

@contextlib.contextmanager
def campaign_lock():
    """One scratch campaign at a time on this checkout: the Go freezes and reach
    runs work in fixed directories under target/phase1-mutation, and campaigns
    share the spliced workspace."""
    lock = TARGET / "campaign.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("w") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def campaign(groups, out, *, ws=None, inventory=None, jobs=None, confirm=False, keep=False):
    """The scratch table campaign of ``groups``; returns its summary."""
    out = Path(out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    ws = Path(ws).resolve() if ws else DEFAULT_WS
    specs = load_specs(groups)
    with campaign_lock():
        return _campaign(groups, specs, out, ws, inventory, jobs, confirm, keep)


def _campaign(groups, specs, out, ws, inventory, jobs, confirm, keep):
    import phase1_mutation_go as go
    import phase1_mutation_plan as plan_module
    import phase1_mutation_run as run
    started = time.monotonic()
    timings = {}

    def mark(step):
        timings[step] = round(time.monotonic() - started - sum(timings.values()), 1)

    inventory_path = out / "requests.json"
    if inventory:
        shutil.copyfile(inventory, inventory_path)
    else:
        inventory_path.write_bytes(inventory_bytes(select(groups, specs=specs)))
    mark("select")
    requests_file = out / "requests-table.ndjson"
    native_file, reach_file, plan_file = out / "native-table.json.gz", out / "go-reach-table.json.gz", out / "plan.json"
    receipt = None
    with go.table_inventory(inventory_path):
        go.materialize("table", out=requests_file)
        mark("materialize")
        native = go.native("table", out=native_file, requests=requests_file, jobs=jobs or 8)
        mark("native")
        ops = claimed_operations(specs)
        if not ops:
            raise ValueError(f"groups {groups} claim no operation; there is nothing to plan")
        plan_document = plan_module.plan(ROOT, ops, plan_file)
        mark("plan")
        reach = go.reach("table", out=reach_file, native=native_file, requests=requests_file, manifest=plan_file,
                         stats=out / "go-reach-table.stats.json")
        mark("reach")
        with run._stdout_to_stderr():
            plan_module.schemata(ROOT, plan_file, ws)
            mark("schemata")
            driver = run.build_driver(ws)
            mark("build")
        trace = run.trace("table", ws, requests_file, native_file, out, driver=driver, verify_frames=True)
        mark("trace")
        kill = run.kill("table", ws, plan_file, out / "trace-table.json.gz", reach_file, jobs, out,
                        requests=requests_file, native=native_file, driver=driver)
        mark("kill")
        results = run.results(plan_file, [out / "kill-table.json.gz"], out / "results.json.gz")
        mark("results")
        if confirm:
            receipt = run.confirm(out / "results.json.gz", plan_file, ws=ws, drivers={run.DRIVER: driver},
                                  requests={"table": requests_file}, natives={"table": native_file},
                                  jobs=jobs, out=out / "confirm")
            mark("confirm")
    trace_document = go.read_document(out / "trace-table.json.gz")
    parity = run.column_parity(run.row_columns(requests_file), trace_document["rows"])
    results_document = run.read_gzip(out / "results.json.gz")
    operations = {op: results_document["operations"].get(op, {}).get("state", "absent") for op in ops}
    summary = {
        "groups": list(specs), "out": str(out), "ws": str(ws), "rows": native["rows"],
        "columns": parity, "columns_at_parity": sorted(column for column, entry in parity.items()
                                                      if entry["rows"] == entry["base_match"]),
        "columns_diverging": sorted(column for column, entry in parity.items() if entry["rows"] != entry["base_match"]),
        "operations": operations, "mutants": results["mutants"], "plan": plan_document["counts"],
        "reach": {"rows_checked": reach["rows_checked"], "operations_entered": reach["summary"]["operations_entered"]},
        "trace": {key: trace[key] for key in ("rows", "base_match", "eligible", "mutants_reached")},
        "kill": {key: value for key, value in kill.items() if key not in ("path", "file_sha256", "oracle")},
        "confirm": None if receipt is None else {key: receipt[key] for key in
                                                 ("result", "pairs_total", "confirmed", "base_rows", "base_native",
                                                  "excused_mutants", "failures")},
        "seconds": timings,
    }
    (out / "summary.json").write_bytes(canonical(summary) + b"\n")
    if not keep:
        for name in ("requests-table.ndjson", "base-table.ndjson"):
            (out / name).unlink(missing_ok=True)
        for name in ("dumps-table", "dumps-table-retry", "logs-table"):
            shutil.rmtree(out / name, ignore_errors=True)
    return summary


# ---------------------------------------------------------------------------
# command line

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check", help="validate the specs against the Go and Rust column registries")
    check.add_argument("--allow-stale", action="store_true",
                       help="pass although specs changed since the committed inventory was selected")
    for name in ("select", "campaign"):
        sub = commands.add_parser(name)
        sub.add_argument("--groups", help="comma-separated groups (default: every group)")
        if name == "select":
            mode = sub.add_mutually_exclusive_group()
            mode.add_argument("--write", action="store_true", help=f"write {INVENTORY}")
            mode.add_argument("--out", help="write a scratch inventory here")
            mode.add_argument("--check", action="store_true", help=f"rerun the selection and require {INVENTORY}")
        else:
            sub.add_argument("--out", required=True, help="scratch directory for every campaign artifact")
            sub.add_argument("--ws", help=f"spliced scratch workspace (default {DEFAULT_WS.relative_to(ROOT)})")
            sub.add_argument("--inventory", help="reuse this inventory instead of selecting")
            sub.add_argument("--jobs", type=int)
            sub.add_argument("--confirm", action="store_true", help="also replay every kill pair (phase1_mutation_run confirm)")
            sub.add_argument("--keep", action="store_true", help="keep request lines, dumps and logs")
    materialize = commands.add_parser("materialize", help="expand an inventory into request lines")
    materialize.add_argument("--inventory", help=f"default {INVENTORY}")
    materialize.add_argument("--out", help="default target/phase1-mutation/requests-table.ndjson")
    args = parser.parse_args(argv)
    try:
        if args.command == "check":
            problems = spec_problems()
            inventory, stale = inventory_problems()
            print(json.dumps({"problems": problems + inventory, "columns": len(columns(load_specs())),
                              "operations": len(claimed_operations()), "inventory_stale_specs": stale},
                             sort_keys=True))
            return 1 if problems or inventory or (stale and not args.allow_stale) else 0
        groups = [group for group in args.groups.split(",") if group] if getattr(args, "groups", None) else None
        if args.command == "select":
            document = select(groups)
            data = inventory_bytes(document)
            target = ROOT / INVENTORY if args.write or args.check else Path(args.out) if args.out else None
            result = {"rows": len(document["requests"]), "selection_sha256": document["selection_sha256"],
                      "columns": {column: entry for record in document["selection"]["groups"].values()
                                  for column, entry in record["columns"].items()}}
            if args.check:
                result["identical"] = target.read_bytes() == data
                print(json.dumps(result, sort_keys=True))
                return 0 if result["identical"] else 1
            if target is not None:
                target.write_bytes(data)
                result["path"] = str(target)
            print(json.dumps(result, sort_keys=True))
            return 0
        if args.command == "materialize":
            import phase1_mutation_go as go
            inventory = Path(args.inventory) if args.inventory else ROOT / INVENTORY
            with go.table_inventory(inventory):
                result = go.materialize("table", out=args.out)
            print(json.dumps(result, sort_keys=True))
            return 0
        summary = campaign(groups, args.out, ws=args.ws, inventory=args.inventory, jobs=args.jobs,
                           confirm=args.confirm, keep=args.keep)
        print(json.dumps(summary, sort_keys=True))
        failed = summary["columns_diverging"] or (summary["confirm"] and summary["confirm"]["result"] != "pass")
        return 1 if failed else 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"phase1 tables: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
