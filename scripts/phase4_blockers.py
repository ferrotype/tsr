#!/usr/bin/env python3
"""Phase 4 X0: the command-line blocker register, derived only from the comparison.

One entry per bucket of scenarios that cannot pass yet: the rows of a full,
harness-valid comparison (`scripts/phase4_compare.py`) grouped by category
(`unsupported`, `failed`, `different`) and cause (the named refusal, the
failure reason, or the first differing section with the kind of step it is
in). Each entry carries its scenario count, a digest of its sorted scenario
ids, the count per family, examples with their capture transcript, and the
owning checkpoint of docs/PHASE4-plan.md section 4:

* a cause that names its checkpoint (`... Phase 4 X1`) is that checkpoint's;
* a content difference inside an emitted output file is Phase 3's residual
  (plan section 7, "Phase 3 slips, or its emit differs"), a cross-phase entry;
* any other cause is owned by its scenarios' families: `tsc` X1 (X2 when the
  first difference is in the build information, the program data or the
  incremental difference), `tsbuild` X3, `tscWatch` and `tsbuildWatch` X5.

The plan's cross-phase dependency is reported from the evidence: Phase 3's
emit (plan sections 2 and 3: 420 of the 517 baselines contain emitted files),
landed when the production source carries the pinned emit entry points and
Phase 3's recorded comparison matches every executed variant with no
unsupported row. A blocker explains a difference; it never hides or
reclassifies one.

    build [--rust DIR] [--record]   # writes data/phase4/blockers.json with --record
    check [--rust DIR]              # the committed register equals the rebuilt one
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase4_compare as compare  # noqa: E402
import phase4_corpus as corpus  # noqa: E402

REGISTER = ROOT / "data/phase4/blockers.json"
RUST = corpus.DEFAULT_OUTPUT
CHECKPOINTS = tuple(f"X{number}" for number in range(8))
NAMED_CHECKPOINT = re.compile(r"\bPhase 4 (X[0-7])\b")
FAMILY_OWNERS = {"tsc": "X1", "tsbuild": "X3", "tscWatch": "X5", "tsbuildWatch": "X5"}
BUILD_STATE_SECTIONS = ("buildinfo", "program", "incremental")
EMIT_RESIDUAL = "Phase 3 (emit residual)"
EMIT_CAUSE = "an emitted file differs"
EMIT_SOURCE = ROOT / "crates/tsr_compiler/src/program_emit.rs"
EMIT_MARKERS = tuple(f"// port: tsc/internal/compiler/program.go:{name}"
                     for name in ("Program.Emit", "CombineEmitResults", "HandleNoEmitOptions"))
PHASE3_COMPARISON = ROOT / "data/phase3/first-comparison.json"
EXAMPLES = 5


def owner_of(category, cause, families, first_sections):
    """(owner, cross_phase) of one bucket."""
    named = NAMED_CHECKPOINT.search(cause)
    if named:
        return named.group(1), False
    if category == "different" and cause == EMIT_CAUSE:
        return EMIT_RESIDUAL, True
    owners = set()
    for family in families:
        owner = FAMILY_OWNERS[family]
        if family == "tsc" and first_sections & set(BUILD_STATE_SECTIONS):
            owner = "X2"
        owners.add(owner)
    return "/".join(sorted(owners)), False


def buckets(comparison):
    """{(category, cause): {"scenarios": [ids], "families": Counter, "sections": set}}
    over every row that is not a match."""
    groups = defaultdict(lambda: {"scenarios": [], "families": Counter(), "sections": set()})
    for row in comparison["rows"]:
        if row["category"] == "match":
            continue
        group = groups[(row["category"], compare.cause_of(row))]
        group["scenarios"].append(row["id"])
        group["families"][row["family"]] += 1
        if row["category"] == "different":
            group["sections"].add(row["first"]["section"])
    return groups


def shown(path):
    return str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)


def emit_state():
    """Phase 3's emit (plan sections 2 and 3): landed when the production source
    carries the pinned entry points and Phase 3's recorded comparison matches
    every executed variant with no unsupported row."""
    text = EMIT_SOURCE.read_text() if EMIT_SOURCE.is_file() else ""
    marked = all(marker in text for marker in EMIT_MARKERS)
    recorded = strict_json_loads(PHASE3_COMPARISON.read_bytes()) if PHASE3_COMPARISON.is_file() else {}
    summary = recorded.get("summary", {})
    complete = (summary.get("rows", 0) > 0 and summary.get("all_domains_met") == summary.get("rows")
                and summary.get("unsupported_rows") == 0 and summary.get("valid") is True)
    return {"dependency": "Phase 3's emit: Program.Emit, HandleNoEmitOptions, CombineEmitResults, the EmitOnly and "
                          "ForceEmit modes and the write seam",
            "owner": "Phase 3 (T8)",
            "consumer": "X1, X2, X3 and X5: the scenarios whose baselines contain emitted files (plan section 2: "
                        "420 of the 517 baselines)",
            "state": "landed" if marked and complete else "open",
            "evidence": (f"{shown(EMIT_SOURCE)}: " + ", ".join(marker.rsplit(":", 1)[1]
                                                                          for marker in EMIT_MARKERS)
                         + f" marked; {shown(PHASE3_COMPARISON)}: all_domains_met "
                         + f"{summary.get('all_domains_met')} of {summary.get('rows')} rows, unsupported_rows "
                         + f"{summary.get('unsupported_rows')}"),
            "rule": f"a content difference inside an emitted output file is a cross-phase entry owned by "
                    f"{EMIT_RESIDUAL}"}


def build(rust_dir=RUST, record=False, *, comparison=None, capture=None):
    capture = capture or corpus.load_capture(rust_dir)
    comparison = comparison or compare.report(rust_dir, capture=capture)
    summary = comparison["summary"]
    if summary["partial"]:
        raise ValueError("the blocker register requires a full comparison, not an informational partial run")
    if not summary["valid"]:
        raise ValueError("the blocker register requires a harness-valid comparison")
    if comparison["rust"]["capture_sha256"] != capture.sha256:
        raise ValueError("comparison was made against a different capture")
    entries = []
    for (category, cause), group in sorted(buckets(comparison).items(),
                                           key=lambda item: (-len(item[1]["scenarios"]), item[0])):
        owner, cross_phase = owner_of(category, cause, set(group["families"]), group["sections"])
        scenarios = group["scenarios"]
        entries.append({
            "id": f"B{len(entries) + 1:02d}", "kind": category, "cause": cause, "owner": owner,
            "cross_phase": cross_phase, "scenarios": len(scenarios),
            "scenarios_sha256": digest(canonical(sorted(scenarios))),
            "families": dict(sorted(group["families"].items())),
            "evidence": [{"scenario": identity, "transcript": f"baselines/{identity}",
                          "observation": f"{category}: {cause}"} for identity in scenarios[:EXAMPLES]],
        })
    register = {
        "version": 1, "pin": comparison["pin"],
        "comparison": {"rust_capture_sha256": comparison["rust"]["capture_sha256"], "rows": summary["rows"],
                       "matched": summary["matched"],
                       "orphan_references": [item["reference"] for item in comparison["orphan_references"]]},
        "scope": ("Buckets of scenarios that cannot pass yet in the Phase 4 command-line comparison, each with the "
                  "execution evidence that establishes it and its owning checkpoint (docs/PHASE4-plan.md section "
                  "4). A blocker explains a difference and never reclassifies it. The orphan reference renders "
                  "from no scenario and is not a blocker."),
        "entries": entries,
        "by_owner": dict(sorted(Counter(entry["owner"] for entry in entries).items())),
        "cross_phase": [emit_state()],
    }
    if record:
        REGISTER.write_bytes(json.dumps(register, indent=1, sort_keys=True).encode() + b"\n")
    return register


def complete(register, comparison):
    """Every scenario that cannot pass is named, once per cause, with its exact
    scenarios, and every entry has an owning checkpoint or a cross-phase owner
    and evidence."""
    if comparison["summary"]["partial"] or not comparison["summary"]["valid"]:
        return False
    if register["comparison"]["rust_capture_sha256"] != comparison["rust"]["capture_sha256"]:
        return False
    needed = {key: (len(group["scenarios"]), digest(canonical(sorted(group["scenarios"]))),
                    dict(sorted(group["families"].items())))
              for key, group in buckets(comparison).items()}
    named = {(entry["kind"], entry["cause"]): (entry["scenarios"], entry["scenarios_sha256"], entry["families"])
             for entry in register["entries"]}
    owners_known = all(entry["evidence"] and (entry["cross_phase"] or set(entry["owner"].split("/")) <= set(CHECKPOINTS))
                       for entry in register["entries"])
    return needed == named and len(named) == len(register["entries"]) and owners_known


def check(rust_dir=RUST):
    rebuilt = build(rust_dir)
    committed = strict_json_loads(REGISTER.read_bytes()) if REGISTER.exists() else None
    return rebuilt == committed, rebuilt


def brief(register):
    return {"entries": [(e["id"], e["kind"], e["cause"][:60], e["owner"], e["scenarios"]) for e in register["entries"]],
            "cross_phase": [(item["dependency"][:40], item["state"]) for item in register["cross_phase"]]}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("build", "check"):
        sub = commands.add_parser(name)
        sub.add_argument("--rust", type=Path, default=RUST)
        if name == "build":
            sub.add_argument("--record", action="store_true")
    args = parser.parse_args()
    if args.command == "build":
        print(json.dumps(brief(build(args.rust, args.record)), indent=1))
        return
    same, rebuilt = check(args.rust)
    print(json.dumps(dict(brief(rebuilt), committed_equal=same), indent=1))
    if not same:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 blockers failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
