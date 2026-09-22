"""Phase 1 F4a: the corpus side of syntax, binder and utility preparation.

F4a is organized around REQUESTS, not around an operation roster: the plan's
completion rule is that "every primary/expanded request is accounted for", and
it warns that mapping percentages are not substitutes for that report. This
module owns the corpus half of that accounting:

* ``inventory`` -- plan task 1. Every physical source and bundled library, every
  extracted unit, every option variant and every parser request, joined across
  the manifests that already own them, with the reason for any unit that does
  not become a parser request and the ACTUAL freshness of the observations that
  cover them. It points at those manifests rather than copying them: the S06
  corpus already records units, configurations and options per primary, and a
  second copy would only be a second thing to drift.
* ``capture`` -- plan tasks 2 and 3. The syntax-only program schedule over all
  15,206 declared compiler variants, rebuilt from native preprocessing, and the
  pinned ``Program.GetSyntacticDiagnostics`` observed once per loadable row
  (data/phase1/syntax-schedule.json, data/phase1/syntax-native.json).
* ``smoke`` / ``replay`` -- plan task 9, corpus half. A bounded run of the Rust
  production ``Program::syntactic_diagnostics`` over a rule-selected subset,
  and a child-free recomputation of its report (data/phase1/syntax-smoke.json).

The inventory re-derives physical membership from the pin itself through the
existing ``s06_corpus.membership`` and holds every count to ``s06_corpus.EXPECTED``,
so a moved pin or an edited manifest fails here instead of passing on stale
numbers.

A note on the plan's "start from" list: it names ``scripts/s07_inventory.py`` as
the starting point for this validation, but that script inventories binder
source functions and the VS Code benchmark workload, not the compiler corpus.
The corpus membership and expansion it means are owned by ``scripts/s06.py
freeze`` (``s06_corpus.membership``) and re-derived on every binder run by
``s07_binder_corpus``; this module validates their committed outputs.
"""

from __future__ import annotations

import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "data/phase1/syntax-inventory.json"

SOURCES = {
    "corpus": "data/s06/corpus.json",
    "requests": "data/s06/requests.json",
    "fixtures": "data/s06/fixtures.json",
    "binder_cases": "data/s07/binder-cases.json",
    "binder_requests": "data/s07/binder-requests.json",
    "subset": "data/s07/subset.json",
}

# The two corpus producers whose observations cover every primary request.
# Their freshness is read from the committed status view, never assumed: a
# historical record is linked AS historical.
COVERING_PRODUCERS = {
    "e1": "parse, node_index_before, encode_source_file and node_index_after over every primary "
          "request: exact encoded AST bytes, the node table, SourceFile state and all parse-time "
          "diagnostics. It does not bind.",
    "binder": "parse, bind and repeat_bind over every primary request, comparing the parsed, bound "
              "and repeated graphs record by record.",
}

# Units the corpus extracts but does not send to the parser, and the boundary
# each one belongs to. Every such unit must carry exactly one of these reasons;
# an unrecognized reason is a problem, not a silent exclusion.
NON_PARSER_UNITS = {
    "mapper_native_asset": "content-mapper input: the external transformation that turns it into "
                           "TypeScript is the named Phase 5 content-mapper boundary",
    "ancillary_asset": "a non-source file (for example a .tsbuildinfo) whose upstream script kind is "
                       "Unknown; the harness retains it without coercing it into a parse",
}


def _load(relative: str) -> object:
    return json.loads((ROOT / relative).read_text())


def _rows(document: object, key: str) -> list:
    if isinstance(document, dict) and key in document:
        return document[key]
    if isinstance(document, list):
        return document
    raise ValueError(f"expected a list or a {key!r} member")


def _digest(relative: str) -> str:
    return hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()


def _eligibility_kind(value: str) -> str:
    return value.split(":", 1)[0].strip()


def _evidence(producer: str) -> dict:
    status = json.loads((ROOT / "status/status.json").read_text())
    artifact = status.get("evidence_artifacts", {}).get(producer)
    state = status.get("evidence_states", {}).get(producer)
    record = {"producer": producer, "observes": COVERING_PRODUCERS[producer],
              "artifact": artifact, "state": state}
    if artifact and (ROOT / artifact).is_file():
        evidence = json.loads((ROOT / artifact).read_text())
        context = evidence.get("context", {})
        record.update({
            "recorded_at": evidence.get("recorded_at"),
            "revision": context.get("revision"),
            "exit_code": evidence.get("exit_code"),
        })
    # Linked as it is. `current` is true only when the committed status view
    # itself says the record matches the present sources.
    record["current"] = state == "current"
    return record


def load_sources() -> dict[str, list]:
    """The owning manifests' rows, loaded once. Tests pass a perturbed copy."""
    return {
        "corpus": _rows(_load(SOURCES["corpus"]), "cases"),
        "requests": _rows(_load(SOURCES["requests"]), "requests"),
        "fixtures": _rows(_load(SOURCES["fixtures"]), "fixtures"),
        "binder_requests": _rows(_load(SOURCES["binder_requests"]), "requests"),
        "binder_cases": _rows(_load(SOURCES["binder_cases"]), "cases"),
        "subset": _rows(_load(SOURCES["subset"]), "cases"),
    }


def build(sources: dict[str, list] | None = None) -> dict:
    import s06_corpus

    sources = sources or load_sources()
    corpus = sources["corpus"]
    requests = sources["requests"]
    fixtures = sources["fixtures"]
    binder_requests = sources["binder_requests"]
    binder_cases = sources["binder_cases"]
    subset = sources["subset"]

    kinds = Counter(case["kind"] for case in corpus)
    units = Counter()
    script_kinds = Counter()
    non_parser = Counter()
    for case in corpus:
        for unit in case.get("units", []):
            script_kinds[str(unit["script_kind"])] += 1
            kind = _eligibility_kind(unit["eligibility"])
            units[kind] += 1
            if kind != "direct_parser":
                non_parser[kind] += 1
    variants = [(case["id"], variant) for case in corpus if case["kind"] == "case"
                for variant in case["variants"]]
    inputs = Counter(item.get("route") for _, variant in variants for item in variant["inputs"])
    subset_variants = {variant["id"]: variant for case in subset for variant in case["variants"]}
    dispositions = Counter(variant["disposition"] for variant in subset_variants.values())
    outcomes = Counter(variant["option_outcome"] for variant in subset_variants.values())
    supplemental = Counter(fixture.get("group") for fixture in fixtures)

    return {
        "version": 1,
        "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
        "purpose": (
            "Plan task 1: every primary and expanded request of the parser/binder corpus, accounted "
            "for against the manifests that already own it. This file points at those manifests by "
            "digest instead of copying them; `phase1.py inventory --check` re-derives every count "
            "from them and from the pin."
        ),
        "sources": {name: {"path": path, "sha256": _digest(path)} for name, path in SOURCES.items()},
        "counts": {
            "physical_cases": kinds["case"],
            "libraries": kinds["library"],
            "primary_rows": len(corpus),
            "extracted_units": sum(units.values()),
            "units_by_eligibility": dict(sorted(units.items())),
            "script_kinds": dict(sorted(script_kinds.items())),
            "option_variants": len(variants),
            "variants_by_subset_disposition": dict(sorted(dispositions.items())),
            "variants_by_option_outcome": dict(sorted(outcomes.items())),
            "variant_inputs_by_route": dict(sorted(inputs.items())),
            "primary_requests": len(requests),
            "binder_requests": len(binder_requests),
            "binder_cases": len(binder_cases),
            "supplemental_requests": len(fixtures),
            "supplemental_by_group": dict(sorted(supplemental.items())),
        },
        "non_parser_units": [
            {"kind": kind, "units": non_parser[kind], "boundary": NON_PARSER_UNITS[kind]}
            for kind in sorted(non_parser)
        ],
        "eligibility_filters": {
            "e2_subset": (
                f"data/s07/subset.json excludes {dispositions.get('excluded', 0)} of "
                f"{len(subset_variants)} variants from the E2 checker subset by checker-syntax "
                "features. That filter decides which variants the E2 CHECKER capture loads; it does "
                "not reach a parser or binder request, which is checked here: the E1 and binder "
                "request sets are identical and include every variant input of every excluded "
                "variant."
            ),
        },
        "evidence": [_evidence(producer) for producer in COVERING_PRODUCERS],
        "expected": s06_corpus.EXPECTED,
    }


def problems(document: dict | None = None, *, sources: dict[str, list] | None = None,
             check_pin: bool = True) -> list[str]:
    """Why the committed inventory does not account for every request."""
    import s06_corpus

    found: list[str] = []
    if document is None:
        if not INVENTORY.is_file():
            return [f"{INVENTORY.relative_to(ROOT)} is absent; run `phase1.py syntax inventory --write`"]
        document = json.loads(INVENTORY.read_text())

    for name, source in document.get("sources", {}).items() if sources is None else ():
        path = ROOT / source["path"]
        if not path.is_file():
            found.append(f"syntax inventory source {source['path']} is missing")
        elif _digest(source["path"]) != source["sha256"]:
            found.append(f"syntax inventory source {source['path']} changed; rebuild the inventory")

    injected = sources is not None
    sources = sources or load_sources()
    corpus = sources["corpus"]
    requests = sources["requests"]
    binder_requests = sources["binder_requests"]
    binder_cases = sources["binder_cases"]
    subset = sources["subset"]
    fixtures = sources["fixtures"]
    expected = s06_corpus.EXPECTED

    # Membership from the pin itself, not from the manifest's own claim.
    if check_pin:
        try:
            upstream = ROOT / "upstream"
            pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
            physical, libraries = s06_corpus.membership(upstream, pin)
            primary_ids = {s06_corpus.primary_id(path) for path in [*physical, *libraries]}
            corpus_ids = {case["id"] for case in corpus}
            if primary_ids != corpus_ids:
                found.append(
                    f"corpus primaries differ from membership at the pin: "
                    f"{len(primary_ids - corpus_ids)} missing, {len(corpus_ids - primary_ids)} extra"
                )
        except Exception as error:  # noqa: BLE001 - reported, not swallowed
            found.append(f"cannot re-derive physical membership at the pin: {error}")

    kinds = Counter(case["kind"] for case in corpus)
    for key, actual in (("physical_cases", kinds["case"]), ("libraries", kinds["library"]),
                        ("primary_rows", len(corpus))):
        if actual != expected[key]:
            found.append(f"{key} is {actual}, the reviewed denominator is {expected[key]}")
    unit_count = sum(len(case.get("units", [])) for case in corpus)
    if unit_count != expected["extracted_units"]:
        found.append(f"extracted_units is {unit_count}, expected {expected['extracted_units']}")

    # Every unit is either a parser input or carries a named boundary.
    for case in corpus:
        for index, unit in enumerate(case.get("units", [])):
            kind = _eligibility_kind(unit["eligibility"])
            if kind != "direct_parser" and kind not in NON_PARSER_UNITS:
                found.append(f"{case['id']} unit {index} is excluded for an unnamed reason: {unit['eligibility']}")

    # Every primary has requests, and every case's requests are exactly its
    # declared (unit, configuration) inputs -- no dropped and no invented one.
    by_primary: dict[str, set] = {}
    for request in requests:
        by_primary.setdefault(request["primary"], set()).add((request["unit"], request["configuration"]))
    for case in corpus:
        mine = by_primary.get(case["id"])
        if not mine:
            found.append(f"primary {case['id']} has no parser request")
            continue
        if case["kind"] != "case":
            continue
        declared = {(item.get("unit"), variant["configuration"])
                    for variant in case["variants"] for item in variant["inputs"]}
        if declared != mine:
            found.append(
                f"{case['id']}: requests differ from its declared variant inputs "
                f"({len(declared - mine)} undispatched, {len(mine - declared)} undeclared)"
            )
    ids = [request["id"] for request in requests]
    if len(ids) != len(set(ids)):
        found.append("duplicate primary request ids")

    # No eligibility filter may remove a parser or binder request.
    binder_ids = {request["id"] for request in binder_requests}
    if set(ids) != binder_ids:
        found.append(
            f"the binder request set differs from the parser request set: "
            f"{len(set(ids) - binder_ids)} parser requests unbound, {len(binder_ids - set(ids))} extra"
        )
    if {row if isinstance(row, str) else row.get("id") for row in binder_cases} != {c["id"] for c in corpus}:
        found.append("binder cases differ from the corpus primaries")
    corpus_variants = {f"{case['id']}#configuration={variant['configuration']}"
                       for case in corpus if case["kind"] == "case" for variant in case["variants"]}
    subset_variants = {variant["id"]: variant for case in subset for variant in case["variants"]}
    if corpus_variants != set(subset_variants):
        found.append(
            f"subset variants differ from corpus variants: {len(corpus_variants - set(subset_variants))} "
            f"undeclared, {len(set(subset_variants) - corpus_variants)} extra"
        )
    for variant_id, variant in subset_variants.items():
        if variant["disposition"] != "excluded":
            continue
        primary, configuration = variant_id.rsplit("#configuration=", 1)
        declared = by_primary.get(primary, set())
        if not any(config == int(configuration) for _unit, config in declared):
            found.append(f"E2-excluded variant {variant_id} lost its parser requests")

    # Supplemental requests are separate identities and never primary rows.
    supplemental_ids = [fixture["request"]["id"] for fixture in fixtures]
    if len(supplemental_ids) != len(set(supplemental_ids)):
        found.append("duplicate supplemental request ids")
    overlap = set(supplemental_ids) & (set(ids) | {case["id"] for case in corpus})
    if overlap:
        found.append(f"{len(overlap)} supplemental ids collide with primary identities")

    if not injected and document.get("counts") != build(sources)["counts"]:
        found.append("syntax inventory counts are stale; rebuild the inventory")
    return found


def write() -> dict:
    document = build()
    INVENTORY.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    return document


# ---------------------------------------------------------------------------
# Plan tasks 2 and 3: the syntax-only program schedule and its native capture.
# The producer lives in scripts/phase1_syntax_schedule.py, which alone is a
# recorded capture input; the checks below read only committed documents.
# ---------------------------------------------------------------------------

from phase1_syntax_schedule import (  # noqa: E402
    BOUNDARIES, NATIVE, SCHEDULE, SELECTIONS, _json_canonical, _write_rows, capture,
    schedule_inputs, schedule_requests,
)


def _schedule_problems(schedule: dict, native: dict, subset_variants: dict[str, dict],
                       policy: dict | None) -> list[str]:
    """Why a schedule and native document do not account for every variant.

    Checks the documents against data/s07/subset.json and against each other,
    without rerunning Go. Freshness against the producer inputs is checked by
    the caller.
    """
    found: list[str] = []
    rows = schedule.get("rows", [])
    ids = [row["id"] for row in rows]
    if ids != list(subset_variants):
        missing = set(subset_variants) - set(ids)
        extra = set(ids) - set(subset_variants)
        found.append(f"syntax schedule rows differ from the S07 variants: {len(missing)} missing, "
                     f"{len(extra)} extra, or reordered")
    native_rows = native.get("rows", [])
    native_by_id = {}
    for row in native_rows:
        if row["id"] in native_by_id:
            found.append(f"native syntax row {row['id']} is duplicated")
        native_by_id[row["id"]] = row
    expected_native = []
    for row in rows:
        variant = subset_variants.get(row["id"])
        if variant is None:
            continue
        rid = row["id"]
        if row["loading_request_sha256"] != variant["loading_request_sha256"]:
            found.append(f"{rid}: loading request differs from the S07 frozen request")
        rejected = variant["option_outcome"] == "rejected"
        mapped = any(reason["rule"] == "content_mapper_execution" for reason in variant["reasons"])
        boundary = "options_rejected" if rejected else "content_mapper" if mapped else None
        if row["boundary"] != boundary:
            found.append(f"{rid}: boundary is {row['boundary']!r}, the S07 variant says {boundary!r}")
        if row["boundary"] is not None and row["boundary"] not in BOUNDARIES:
            found.append(f"{rid}: unnamed boundary {row['boundary']!r}")
        if rejected and (not row["option_diagnostics"] or row["option_diagnostics"] != variant["option_diagnostics"]):
            found.append(f"{rid}: a rejected variant must keep its native option diagnostics")
        if not rejected and row["option_diagnostics"]:
            found.append(f"{rid}: an accepted variant carries option diagnostics")
        if row["e2_disposition"] != variant["disposition"]:
            found.append(f"{rid}: E2 disposition differs from the S07 subset")
        expected_selection = ("filename_skip" if row["filename_skip"] else
                              {"allowed": "runs", "skipped": "option_guard_skip",
                               "not_reached": "options_rejected"}.get(row["option_guard"]))
        if row["native_selection"] != expected_selection or row["native_selection"] not in SELECTIONS:
            found.append(f"{rid}: native selection {row['native_selection']!r} does not follow from its guard")
        if (row["option_guard"] == "not_reached") != rejected:
            found.append(f"{rid}: only a rejected variant may skip the native option guard")
        # A boundary row never loads; every other row loaded or panicked, and
        # its syntactic status says which. None becomes an empty success.
        want = {"loaded": "observed", "panic": "load_panicked", "not_loaded": "boundary"}.get(row["load"])
        if want is None or row["syntactic"] != want:
            found.append(f"{rid}: load {row['load']!r} and syntactic {row['syntactic']!r} disagree")
        if (row["load"] == "not_loaded") != (boundary is not None):
            found.append(f"{rid}: only a named boundary may leave a variant unloaded")
        if row["load"] in ("loaded", "panic"):
            expected_native.append(rid)
            observed = native_by_id.get(rid)
            if observed is None:
                found.append(f"{rid}: {row['syntactic']} without a native row")
            elif row["load"] == "loaded" and not {"syntactic", "plain_hex", "pretty_hex"} <= set(observed):
                found.append(f"{rid}: native row lacks its structured or rendered diagnostics")
            elif row["load"] == "panic" and not observed.get("panic"):
                found.append(f"{rid}: a panicked load lacks its panic text")
        elif rid in native_by_id:
            found.append(f"{rid}: a boundary row carries a native observation")
    if [row["id"] for row in native_rows] != expected_native:
        found.append("native syntax rows are not exactly the loaded schedule rows, in order")
    # The E2 policy's own native observation of the same guard, over the rows
    # it covers. It ran the guard on the accepted subset of a rejected
    # variant's settings; the harness never reaches the guard for those.
    if policy is not None:
        by_id = {row["id"]: row for row in rows}
        for observed in policy["rows"]:
            row = by_id.get(observed["id"])
            if row is None:
                found.append(f"{observed['id']}: E2 policy row without a schedule row")
                continue
            if observed["filename_skip"] != row["filename_skip"]:
                found.append(f"{observed['id']}: filename skip differs from the E2 policy observation")
            if row["option_guard"] != "not_reached" and observed["option_guard"] != row["option_guard"]:
                found.append(f"{observed['id']}: option guard differs from the E2 policy observation")
    if schedule.get("phase") != "Program.GetSyntacticDiagnostics only; not the whole .errors.txt baseline":
        found.append("the syntax schedule no longer names its phase")
    return found


def schedule_problems(*, check_inputs: bool = True) -> list[str]:
    """The committed schedule and native observation, checked without Go."""
    if not SCHEDULE.is_file() or not NATIVE.is_file():
        return ["the syntax schedule is absent; run `phase1_syntax.py capture --write`"]
    schedule = json.loads(SCHEDULE.read_text())
    native = json.loads(NATIVE.read_text())
    subset = {variant["id"]: variant for case in _rows(_load(SOURCES["subset"]), "cases")
              for variant in case["variants"]}
    policy = json.loads((ROOT / "data/s07/e2-policy-observations.json").read_text())
    found = _schedule_problems(schedule, native, subset, policy)
    if schedule.get("provenance") != native.get("provenance"):
        found.append("the schedule and native documents come from different captures")
    if check_inputs:
        recorded = schedule.get("provenance", {}).get("inputs", {})
        current = schedule_inputs()
        changed = sorted(name for name in set(recorded) | set(current) if recorded.get(name) != current.get(name))
        if changed:
            found.append("the syntax schedule is stale against " + ", ".join(changed))
        pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
        if schedule.get("provenance", {}).get("pin") != pin:
            found.append("the syntax schedule was captured at a different pin")
    return found


# ---------------------------------------------------------------------------
# Plan task 9, corpus half: a bounded Rust smoke over the schedule, replayed
# from its stored outputs. The selection is every row with at least one native
# syntactic diagnostic -- the rows whose output a wrong parser, walk, sort or
# renderer is likeliest to change -- plus the first row of every stratum of
# file extensions in the request, JS/decorator options and native selection,
# so each loader and option path runs at least once. Every other loaded row is
# a prepared, unexecuted case: F5b performs the full correctness capture.
# ---------------------------------------------------------------------------

SMOKE = ROOT / "data/phase1/syntax-smoke.json"
SMOKE_RULE = ("every schedule row with at least one native syntactic diagnostic, plus the first row "
              "(schedule order) of each stratum of (extensions present in the request, checkJs, allowJs, "
              "experimentalDecorators, native selection)")
RUST_EXTRA = ("tools/s07/program/rust_observation.rs", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml")
COMPARED = ("files", "file_names_sha256", "syntactic", "plain_hex", "pretty_hex")


def select(schedule: dict, native: dict) -> list[str]:
    diagnosed = {row["id"] for row in native["rows"] if row.get("syntactic")}
    chosen, strata = set(diagnosed), set()
    for row in schedule["rows"]:
        if row["load"] != "loaded":
            continue
        stratum = json.dumps([row["request_shape"], row["native_selection"]])
        if stratum not in strata:
            strata.add(stratum)
            chosen.add(row["id"])
    return [row["id"] for row in schedule["rows"] if row["id"] in chosen]


def rust_closure() -> dict[str, str]:
    import phase1_capture

    paths = set(RUST_EXTRA)
    for directory in phase1_capture.workspace_closure("phase1_syntax"):
        for path in directory.rglob("*"):
            relative = path.relative_to(ROOT)
            if path.is_file() and not any(part in (".git", "target", "__pycache__") for part in relative.parts):
                paths.add(str(relative))
    return {name: _digest(name) for name in sorted(paths) if (ROOT / name).is_file()}


def build_rust() -> Path:
    from s04_common import command

    messages = command(["cargo", "build", "--locked", "--offline", "--release", "-p", "phase1_syntax",
                        "--bin", "phase1_syntax", "--message-format=json"], cwd=ROOT)
    executables = [record["executable"] for record in map(json.loads, filter(str.strip, messages.decode().splitlines()))
                   if record.get("reason") == "compiler-artifact"
                   and record.get("target", {}).get("name") == "phase1_syntax" and record.get("executable")]
    if len(executables) != 1:
        raise ValueError("cargo reported no single phase1_syntax executable")
    return Path(executables[0])


def compare_row(native_row: dict, rust_row: dict) -> dict:
    """One smoke row's result in the Phase 1 vocabulary."""
    state = rust_row.get("state")
    if state == "not_implemented":
        return {"id": native_row["id"], "result": "not_implemented", "operation": rust_row.get("operation")}
    if state != "observed":
        # A Rust load error or panic on a program Go loaded is a finding, not a
        # harness failure: the harness ran and the production path failed.
        return {"id": native_row["id"], "result": "different", "rust_state": state,
                "detail": rust_row.get("error") or rust_row.get("panic")}
    differs = [field for field in COMPARED if native_row.get(field) != rust_row.get(field)]
    return {"id": native_row["id"], "result": "different" if differs else "match",
            **({"differs": differs} if differs else {})}


def _smoke_report(directory: Path, selected: list[str], native: dict, schedule: dict) -> dict:
    from s04_common import strict_json_loads

    rust_rows = [strict_json_loads(line) for line in (directory / "rust-rows.jsonl").read_bytes().splitlines() if line.strip()]
    if [row.get("id") for row in rust_rows] != selected:
        raise ValueError("Rust smoke rows are missing, extra, duplicated or reordered")
    native_by_id = {row["id"]: row for row in native["rows"]}
    rows = [compare_row(native_by_id[rid], rust) for rid, rust in zip(selected, rust_rows, strict=True)]
    loaded = [row["id"] for row in schedule["rows"] if row["load"] == "loaded"]
    counts = dict(sorted(Counter(row["result"] for row in rows).items()))
    counts["not_run"] = len(loaded) - len(selected)
    return {"counts": counts, "rows": rows,
            "not_implemented_operations": dict(sorted(Counter(row["operation"] for row in rows
                                                              if row["result"] == "not_implemented").items()))}


def smoke(directory: Path, *, write_committed: bool = False) -> dict:
    """Run the bounded Rust smoke once and store every input it read."""
    import s07_subset

    directory = Path(directory).resolve()
    problems_found = schedule_problems()
    if problems_found:
        raise ValueError("the committed syntax schedule is not current: " + problems_found[0])
    schedule = json.loads(SCHEDULE.read_text())
    native = json.loads(NATIVE.read_text())
    observations = directory / "source-observations.ndjson"
    if not observations.is_file():
        s07_subset.export_observations(observations)
    rows, probes = schedule_requests(observations)
    if [row["loading_request_sha256"] for row in rows] != [row["loading_request_sha256"] for row in schedule["rows"]]:
        raise ValueError("fresh native preprocessing differs from the committed schedule")
    selected = select(schedule, native)
    wanted = set(selected)
    smoke_dir = directory / "smoke"
    smoke_dir.mkdir(parents=True, exist_ok=False)
    request_bytes = _json_canonical([probe for probe in probes if probe["id"] in wanted])
    (smoke_dir / "requests.json").write_bytes(request_bytes)
    before = rust_closure()
    executable = build_rust()
    from s04_common import command
    command([str(executable), "--schedule", str(smoke_dir / "requests.json"), str(smoke_dir / "rust-rows.jsonl")], cwd=ROOT)
    if rust_closure() != before:
        raise ValueError("a Rust source input changed during the smoke; the capture is invalid")
    provenance = {"schedule_sha256": _digest(str(SCHEDULE.relative_to(ROOT))),
                  "native_rows_sha256": native_rows_digest(native),
                  "requests_sha256": hashlib.sha256(request_bytes).hexdigest(),
                  "rust_rows_sha256": hashlib.sha256((smoke_dir / "rust-rows.jsonl").read_bytes()).hexdigest(),
                  "rust_binary_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                  "rust_closure": before, "selected": selected}
    (smoke_dir / "provenance.json").write_bytes(_json_canonical(provenance))
    report = replay(smoke_dir)
    if write_committed:
        _write_rows(SMOKE, report)
    return report["counts"]


def replay(smoke_dir: Path) -> dict:
    """Recompute the smoke report from a stored capture. Runs no child."""
    from s04_common import strict_json_loads

    smoke_dir = Path(smoke_dir).resolve()
    provenance = strict_json_loads((smoke_dir / "provenance.json").read_bytes())
    for name, key in (("requests.json", "requests_sha256"), ("rust-rows.jsonl", "rust_rows_sha256")):
        if hashlib.sha256((smoke_dir / name).read_bytes()).hexdigest() != provenance[key]:
            raise ValueError(f"stored smoke {name} changed since capture")
    if provenance["native_rows_sha256"] != native_rows_digest(json.loads(NATIVE.read_text())):
        raise ValueError("the committed native syntax observation changed since the smoke")
    schedule = json.loads(SCHEDULE.read_text())
    native = json.loads(NATIVE.read_text())
    requests = strict_json_loads((smoke_dir / "requests.json").read_bytes())
    if [request["id"] for request in requests] != provenance["selected"]:
        raise ValueError("stored smoke requests are not the recorded selection")
    report = _smoke_report(smoke_dir, provenance["selected"], native, schedule)
    stale = sorted(name for name, sha in provenance["rust_closure"].items()
                   if not (ROOT / name).is_file() or _digest(name) != sha)
    return {"version": 1, "rule": SMOKE_RULE, "selected": provenance["selected"],
            "native_rows_sha256": provenance["native_rows_sha256"], "rust_binary_sha256": provenance["rust_binary_sha256"],
            "rust_closure": provenance["rust_closure"],
            "rust_sources_current": not stale, **report}


def native_rows_digest(native: dict) -> str:
    """The native observation's content, independent of the provenance that
    records which script version captured it."""
    return hashlib.sha256(_json_canonical(native["rows"])).hexdigest()


def smoke_problems() -> list[str]:
    """The committed smoke report checked against the committed schedule, without a child."""
    if not SMOKE.is_file():
        return ["the syntax smoke report is absent; run `phase1_syntax.py smoke --write`"]
    smoke_report = json.loads(SMOKE.read_text())
    schedule = json.loads(SCHEDULE.read_text())
    native = json.loads(NATIVE.read_text())
    found = []
    if smoke_report.get("native_rows_sha256") != native_rows_digest(native):
        found.append("the syntax smoke compared against a different native observation")
    if smoke_report.get("rule") != SMOKE_RULE or smoke_report.get("selected") != select(schedule, native):
        found.append("the syntax smoke selection is not the committed rule's selection")
    rows = smoke_report.get("rows", [])
    if [row.get("id") for row in rows] != smoke_report.get("selected"):
        found.append("the syntax smoke rows are not exactly its selection, in order")
    results = Counter(row.get("result") for row in rows)
    if set(results) - {"match", "different", "not_implemented"}:
        found.append(f"the syntax smoke has unknown results {sorted(set(results) - {'match', 'different', 'not_implemented'})}")
    loaded = sum(1 for row in schedule["rows"] if row["load"] == "loaded")
    expected = dict(sorted(results.items()))
    expected["not_run"] = loaded - len(rows)
    if smoke_report.get("counts") != expected:
        found.append("the syntax smoke counts do not follow from its rows")
    return found


def smoke_freshness() -> dict:
    """Whether the Rust sources the committed smoke ran are still the current ones.

    Informational: a later production change is expected to move them, and F4b
    reruns the smoke when it does. It is reported, never hidden.
    """
    if not SMOKE.is_file():
        return {"rust_sources_current": False, "changed": []}
    recorded = json.loads(SMOKE.read_text()).get("rust_closure", {})
    current = rust_closure()
    changed = sorted(name for name in set(recorded) | set(current) if recorded.get(name) != current.get(name))
    return {"rust_sources_current": not changed, "changed": changed[:20], "changed_count": len(changed)}


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="Phase 1 F4a corpus syntax schedule, native capture and Rust smoke.")
    commands = parser.add_subparsers(dest="command", required=True)
    for name, text in (("capture", "native preprocessing and the native syntax probe, once"),
                       ("smoke", "the bounded Rust smoke over the committed schedule")):
        sub = commands.add_parser(name, help=text)
        sub.add_argument("--output", type=Path, required=True, help="a scratch directory, never an owner capture")
        sub.add_argument("--write", action="store_true", help="also rewrite the committed documents")
    sub = commands.add_parser("replay", help="recompute a stored smoke report without running anything")
    sub.add_argument("directory", type=Path)
    commands.add_parser("check", help="check the committed inventory, schedule and native observation")
    args = parser.parse_args(argv)
    if args.command == "capture":
        print(json.dumps(capture(args.output, write_committed=args.write), indent=2))
    elif args.command == "smoke":
        print(json.dumps(smoke(args.output, write_committed=args.write), indent=2))
    elif args.command == "replay":
        report = replay(args.directory)
        print(json.dumps({"counts": report["counts"], "rust_sources_current": report["rust_sources_current"]},
                         indent=2))
    else:
        found = problems() + schedule_problems() + smoke_problems()
        print(json.dumps({"problems": found, "smoke": smoke_freshness()}, indent=2))
        return 1 if found else 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
