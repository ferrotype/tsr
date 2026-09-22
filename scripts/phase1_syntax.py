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
