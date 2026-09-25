#!/usr/bin/env python3
"""Phase 2 C0.1: the frozen checker acceptance inventory.

One row per effective compiler/conformance variant at the pin (15,206), joined
from three committed inputs and never from a native or Rust run:

* data/s07/subset.json -- variant identity, effective options, harness
  options, exclusion reasons (family tags), reference baselines by kind with
  git blobs, loading-request digests;
* data/phase1/syntax-schedule.json -- the native runner's selection outcome
  (`runs`, `option_guard_skip`, `filename_skip`) and the named boundaries
  (`content_mapper`, `options_rejected`);
* data/s07/e2-acceptance.json -- the S08 acceptance partition, kept as the
  9,369-variant regression subset.

`executed` rows form the Phase 2 denominator: the 13,432 variants the pinned
runner enumerates (`compilerBaselineRegex`, `\.tsx?$`) and runs. The runner's
option-guard and filename skips, and the two stray `.js` files the syntax
schedule counted as `runs` but `EnumerateTestFiles` never lists
(`not_enumerated`), are `informational` with their native reason and never
enter a ratio. The checkpoint column is a completion owner derived from syntax
families, not an execution outcome. The recorded sample is the owner-approved
(2026-09-25) intermediate-checkpoint selection: about 300 executed variants
covering the main feature and option combinations; it never claims parity.

    python3 scripts/phase2_inventory.py freeze   # writes data/phase2/inventory.json
    python3 scripts/phase2_inventory.py check    # rebuilds and requires byte identity
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "data/phase2/inventory.json"
INPUTS = ("data/s07/subset.json", "data/phase1/syntax-schedule.json", "data/s07/e2-acceptance.json")
PRODUCER = "scripts/phase2_inventory.py"

TYPE_FAMILIES = ("explicit_type_parameters", "nonempty_type_arguments", "conditional_types", "mapped_types",
                 "indexed_access_types", "infer_types", "template_literal_types", "import_types")
C4_FAMILIES = ("jsx", "decorators")
OTHER_RULES = ("emitted_output_only", "content_mapper_execution", "project_references")
KNOWN_RULES = frozenset(TYPE_FAMILIES + C4_FAMILIES + OTHER_RULES)
CHECKED_KINDS = (".errors.txt", ".types", ".symbols")
SELECTIONS = ("runs", "option_guard_skip", "filename_skip")
# compiler_runner.go: compilerBaselineRegex. EnumerateTestFiles lists only
# these, so any other physical file in the case directories is never a test,
# whatever the syntax schedule's guard-based selection says.
RUNNER_TEST_FILE = re.compile(r"\.tsx?$")
CHECKPOINT_RULE = ("regression: the S08 acceptance variant; C4: uses JSX or decorators, whatever else it uses; "
                   "C2: uses a type-level family (explicit type parameters, type arguments, conditional, mapped, "
                   "indexed-access, infer, template-literal or import types); C3: executed with no such family "
                   "(the emitted-output-only and content-mapper rows), i.e. ordinary program semantics. "
                   "An owner of completion, never an execution outcome.")
SAMPLE_SIZE = 300
SAMPLE_TARGETS = {"regression": 100, "C2": 100, "C4": 70, "C3": 30}
SAMPLE_RULE = ("Greedy deterministic cover over executed rows: each feature value below needs its quota of "
               "representatives (checkpoint 8, family 3, other features 2, capped by availability); at each step "
               "take the row covering the most unmet quota, ties by fewest source bytes then id. Then fill each "
               "checkpoint group in sha256(id) order up to regression 100, C2 100, C4 70, C3 30, and top up from "
               "regression to exactly 300. Features: checkpoint, family, "
               "suite, module, target, moduleResolution, jsx, strict, types/symbols disabled, declaration emit, "
               "allowJs, checkJs, suggestions, pretty, traceResolution, tsconfig, multiple roots, errors reference, "
               "emitted-only, content mapper.")

# Pinned core enum names (tsc/internal/core/compileroptions.go), for buckets only.
MODULE = {0: "none", 1: "commonjs", 2: "amd", 3: "umd", 4: "system", 5: "es2015", 6: "es2020", 7: "es2022",
          99: "esnext", 100: "node16", 101: "node18", 102: "node20", 199: "nodenext", 200: "preserve"}
TARGET = {0: "none", 1: "es5", 2: "es2015", 3: "es2016", 4: "es2017", 5: "es2018", 6: "es2019", 7: "es2020",
          8: "es2021", 9: "es2022", 10: "es2023", 11: "es2024", 12: "es2025", 99: "esnext", 100: "json"}
RESOLUTION = {0: "none", 1: "classic", 2: "node10", 3: "node16", 99: "nodenext", 100: "bundler"}
JSX = {0: "none", 1: "preserve", 2: "react-native", 3: "react", 4: "react-jsx", 5: "react-jsxdev"}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encode(value) -> str:
    """Canonical JSON that keeps the semantic ordered maps of S07 options."""
    sys.path.insert(0, str(ROOT / "scripts"))
    from s07_subset import json_bytes

    return json_bytes(value)[:-1].decode()


def load(relative: str):
    return json.loads((ROOT / relative).read_bytes())


def _enum(table, value):
    if value is None:
        return "unset"
    return table.get(value, str(value))


def _flag(options, name):
    value = options.get(name)
    if value is not None and type(value) is not bool:
        raise ValueError(f"non-boolean option {name}: {value!r}")
    return value is True


def checkpoint(families, s08_tier):
    if s08_tier == "acceptance":
        if families:
            raise ValueError("an S08 acceptance variant carries an exclusion family")
        return "regression"
    if set(families) & set(C4_FAMILIES):
        return "C4"
    if set(families) & set(TYPE_FAMILIES):
        return "C2"
    return "C3"


def build_rows(subset, schedule, acceptance):
    selections = {row["id"]: row for row in schedule["rows"]}
    tiers = {row["id"]: row for row in acceptance["variants"]}
    rows = []
    seen = set()
    stray = {}
    for case in subset["cases"]:
        source = case["source"]
        suite = case["id"].split("/", 1)[0]
        if suite not in ("compiler", "conformance"):
            raise ValueError("variant outside the compiler/conformance suites: " + case["id"])
        for variant in case["variants"]:
            vid = variant["id"]
            if vid in seen:
                raise ValueError("duplicate variant " + vid)
            seen.add(vid)
            native = selections.get(vid)
            if native is None:
                raise ValueError("variant missing from the syntax schedule: " + vid)
            if native["loading_request_sha256"] != variant["loading_request_sha256"]:
                raise ValueError("syntax schedule loads a different request: " + vid)
            if native["configured_name"] != variant["configured_name"]:
                raise ValueError("configured name differs between inputs: " + vid)
            selection = native["native_selection"]
            if selection not in SELECTIONS:
                raise ValueError("unknown native selection: " + vid)
            enumerated = RUNNER_TEST_FILE.search(source["path"]) is not None
            if not enumerated:
                selection = "not_enumerated"
            boundary = native["boundary"]
            if boundary not in (None, "content_mapper", "options_rejected"):
                raise ValueError("unknown native boundary: " + vid)
            families = sorted({reason["rule"] for reason in variant.get("reasons") or []})
            if set(families) - KNOWN_RULES:
                raise ValueError("unknown exclusion rule: " + vid)
            if (variant["disposition"] == "eligible") == bool(families):
                raise ValueError("S07 disposition disagrees with its reasons: " + vid)
            s08 = tiers.get(vid)
            if (s08 is None) != (variant["disposition"] != "eligible"):
                raise ValueError("S08 partition disagrees with S07 eligibility: " + vid)
            s08_tier = s08["tier"] if s08 else None
            references = {}
            for baseline in variant["baselines"]:
                if baseline["kind"] in references:
                    raise ValueError("duplicate reference kind: " + vid)
                references[baseline["kind"]] = baseline["git_blob"]
            if not enumerated:
                # S07 indexed baselines by stem; they belong to the enumerated
                # sibling test of the same name, checked after the loop.
                stray[(suite, variant["configured_name"].rsplit(".", 1)[0])] = (vid, references)
                references = {}
            harness = variant["harness_options"]
            options = variant["options"]
            content_mapper = "content_mapper_execution" in families
            if content_mapper != (boundary == "content_mapper"):
                raise ValueError("content-mapper rule and boundary disagree: " + vid)
            executed = selection == "runs"
            if executed and boundary == "options_rejected":
                raise ValueError("an executed variant was rejected by the option parser: " + vid)
            rows.append({
                "id": vid,
                "suite": suite,
                "primary": native["primary"],
                "path": source["path"],
                "configuration": variant["configuration"],
                "configured_name": variant["configured_name"],
                "source_bytes": source["loaded_bytes"],
                "options": options,
                "config_root_keys": variant["config_root_keys"],
                "config_option_keys": variant["config_option_keys"],
                "roots": variant["roots"],
                "loading_request_sha256": variant["loading_request_sha256"],
                "native_selection": selection,
                "boundary": boundary,
                "tier": "executed" if executed else "informational",
                "informational_reason": None if executed else selection,
                "harness": {"NoTypesAndSymbols": harness["NoTypesAndSymbols"],
                            "CaptureSuggestions": harness["CaptureSuggestions"],
                            "pretty": _flag(options, "pretty")},
                "references": dict(sorted(references.items())),
                "emitted_only": "emitted_output_only" in families,
                "content_mapper": content_mapper,
                "emit_declarations": _flag(options, "declaration") or _flag(options, "composite"),
                "families": families,
                "checkpoint": checkpoint(families, s08_tier) if executed else None,
                "s08_tier": s08_tier,
                "sample": False,
            })
    if len(rows) != len(selections) or {row["id"] for row in rows} != set(selections):
        raise ValueError("inputs do not describe the same variants")
    for (suite, stem), (vid, references) in stray.items():
        siblings = [row for row in rows if row["suite"] == suite and row["tier"] == "executed"
                    and row["configured_name"].rsplit(".", 1)[0] == stem]
        if len(siblings) != 1 or any(siblings[0]["references"].get(kind) != blob for kind, blob in references.items()
                                     if kind in CHECKED_KINDS):
            raise ValueError("a non-enumerated file's baselines do not belong to one enumerated sibling: " + vid)
    return rows


def features(row):
    options = row["options"]
    values = [("checkpoint", row["checkpoint"]), ("suite", row["suite"]),
              ("module", _enum(MODULE, options.get("module"))),
              ("target", _enum(TARGET, options.get("target"))),
              ("moduleResolution", _enum(RESOLUTION, options.get("moduleResolution"))),
              ("jsx", _enum(JSX, options.get("jsx"))),
              ("strict", _flag(options, "strict")),
              ("types_disabled", row["harness"]["NoTypesAndSymbols"]),
              ("emit_declarations", row["emit_declarations"]),
              ("allowJs", _flag(options, "allowJs")), ("checkJs", _flag(options, "checkJs")),
              ("suggestions", row["harness"]["CaptureSuggestions"]), ("pretty", row["harness"]["pretty"]),
              ("traceResolution", _flag(options, "traceResolution")),
              ("tsconfig", "configFilePath" in options), ("multiple_roots", len(row["roots"]) > 1),
              ("errors_reference", ".errors.txt" in row["references"]),
              ("emitted_only", row["emitted_only"]), ("content_mapper", row["content_mapper"])]
    values += [("family", family) for family in row["families"]]
    return values


def quota(feature):
    return 8 if feature[0] == "checkpoint" else 3 if feature[0] == "family" else 2


def select_sample(rows):
    executed = [row for row in rows if row["tier"] == "executed"]
    order = sorted(executed, key=lambda row: (row["source_bytes"], row["id"]))
    covering = {row["id"]: features(row) for row in executed}
    available = Counter(feature for values in covering.values() for feature in values)
    need = Counter({feature: min(quota(feature), count) for feature, count in available.items()})
    chosen, chosen_set = [], set()
    while any(value > 0 for value in need.values()) and len(chosen) < SAMPLE_SIZE:
        best, best_gain = None, 0
        for row in order:
            if row["id"] in chosen_set:
                continue
            gain = sum(1 for feature in covering[row["id"]] if need[feature] > 0)
            if gain > best_gain:
                best, best_gain = row, gain
        if best is None:
            break
        chosen.append(best["id"])
        chosen_set.add(best["id"])
        for feature in covering[best["id"]]:
            if need[feature] > 0:
                need[feature] -= 1
    if any(value > 0 for value in need.values()):
        raise ValueError("the sample cannot meet its feature quotas within 300 rows")
    groups = {}
    for row in sorted(executed, key=lambda row: digest(row["id"].encode())):
        groups.setdefault(row["checkpoint"], []).append(row["id"])
    owner = {row["id"]: row["checkpoint"] for row in executed}
    by_group = Counter(owner[rid] for rid in chosen)

    def fill(name, limit):
        for rid in groups.get(name, []):
            if by_group[name] >= limit or len(chosen) >= SAMPLE_SIZE:
                return
            if rid not in chosen_set:
                chosen.append(rid)
                chosen_set.add(rid)
                by_group[name] += 1

    for name, target in SAMPLE_TARGETS.items():
        fill(name, target)
    fill("regression", SAMPLE_SIZE)
    if len(chosen) != SAMPLE_SIZE:
        raise ValueError("the sample does not reach its recorded size")
    return chosen_set


def counts(rows):
    executed = [row for row in rows if row["tier"] == "executed"]
    return {
        "variants": len(rows),
        "executed": len(executed),
        "informational": dict(sorted(Counter(row["informational_reason"] for row in rows
                                              if row["tier"] == "informational").items())),
        "informational_options_rejected": sum(row["boundary"] == "options_rejected" for row in rows),
        "executed_by_s08_tier": dict(sorted(Counter(row["s08_tier"] or "excluded" for row in executed).items())),
        "executed_content_mapper": sum(row["content_mapper"] for row in executed),
        "executed_newly_included": sum(row["s08_tier"] is None and not row["content_mapper"] for row in executed),
        "checkpoint": dict(sorted(Counter(row["checkpoint"] for row in executed).items())),
        "references": {kind: sum(kind in row["references"] for row in executed) for kind in
                       (".errors.txt", ".types", ".symbols", ".trace.json")},
        "without_checked_reference": sum(not set(CHECKED_KINDS) & set(row["references"]) for row in executed),
        "emitted_only": sum(row["emitted_only"] for row in executed),
        "types_disabled": sum(row["harness"]["NoTypesAndSymbols"] for row in executed),
        "emit_declarations_by_options": sum(row["emit_declarations"] for row in executed),
        "families": dict(sorted(Counter(family for row in executed for family in row["families"]).items())),
        "sample": sum(row["sample"] for row in rows),
        "sample_by_checkpoint": dict(sorted(Counter(row["checkpoint"] for row in rows if row["sample"]).items())),
    }


def build():
    raw = {name: (ROOT / name).read_bytes() for name in INPUTS}
    subset, schedule, acceptance = (json.loads(raw[name]) for name in INPUTS)
    pins = {subset["pin"], schedule["provenance"]["pin"] if "pin" in schedule.get("provenance", {}) else subset["pin"],
            acceptance["pin"], load("data/upstream.json")["pin"]}
    if len(pins) != 1:
        raise ValueError("inputs name different upstream pins")
    rows = build_rows(subset, schedule, acceptance)
    sample = select_sample(rows)
    for row in rows:
        row["sample"] = row["id"] in sample
    document = {
        "version": 1,
        "pin": subset["pin"],
        "scope": ("Phase 2 C0.1 inventory: every effective compiler/conformance variant at the pin with its native "
                  "selection. Executed rows are the Phase 2 denominator; informational rows keep their native "
                  "reason and never enter a ratio. No native or Rust execution is inferred from this document."),
        "inputs": {name: digest(raw[name]) for name in INPUTS},
        "checkpoint_rule": CHECKPOINT_RULE,
        "sample_rule": SAMPLE_RULE,
        "sample_decision": ("owner decision 2026-09-25: intermediate checkpoints run this recorded sample plus targeted "
                            "cases for the change; full runs only for C0's gap map and C7's acceptance"),
        "counts": counts(rows),
        "rows": rows,
    }
    return document


def render(document) -> bytes:
    head = encode({key: value for key, value in document.items() if key != "rows"})
    body = ",\n".join(encode(row) for row in document["rows"])
    return (head[:-1] + ',"rows":[\n' + body + "\n]}\n").encode()


def read():
    """The committed inventory, verified against its current inputs."""
    raw = INVENTORY.read_bytes()
    document = json.loads(raw)
    for name, expected in document["inputs"].items():
        if digest((ROOT / name).read_bytes()) != expected:
            raise ValueError(f"inventory input changed since freeze: {name}; rerun freeze and review")
    return document


def executed(document=None):
    document = document or read()
    return [row for row in document["rows"] if row["tier"] == "executed"]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("freeze", "check"))
    parser.add_argument("--output", type=Path, default=INVENTORY)
    args = parser.parse_args()
    rendered = render(build())
    if args.command == "freeze":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(rendered)
    elif not args.output.exists() or args.output.read_bytes() != rendered:
        print("phase2 inventory is stale or was edited; rerun freeze and review the diff", file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps(json.loads(rendered)["counts"], sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 inventory failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
