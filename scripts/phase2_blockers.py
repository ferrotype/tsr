#!/usr/bin/env python3
"""Phase 2 C0.5: the blocker register, derived only from execution evidence.

One entry per dependency that withholds observations: each named production
refusal (`Error::Unsupported`) observed in the Rust run, the native
emit-order dependency (pre- and post-emit diagnostic sets differ, so the
baseline needs the post-emit program's order), and content-mapper execution.
Every entry lists its variants, the domains it withholds, its owner and the
raw observation that establishes it. A blocker explains a difference; it never
hides or reclassifies one: the comparison keeps every category as observed.

The declaration audit compares the Rust declaration phase with the exact
post-emit native declaration diagnostics for every native declaration request.

    build --native DIR --rust DIR [--record]
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s08_p4 as p4  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_compare  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402

REGISTER = ROOT / "data/phase2/blockers.json"
OWNERS = (
    ("content-mapper execution", "Phase 5 (content-mapper execution)"),
)
PHASE1_CONTRACTS = [
    {"contract": "program loading including project references", "state": "no open item",
     "closed_by": ["#55 (ported reference loading)", "#56 (last Phase 1 operations)"]},
    {"contract": "options, configuration and module resolution", "state": "no open item",
     "closed_by": ["#49", "#50 (F3b completion)", "#55"]},
    {"contract": "binder and bound-file ownership", "state": "no open item",
     "closed_by": ["#51 (F4a syntax schedule)", "#53 (F5b integration)", "#55"]},
]
PHASE1_PENDING_NOTE = ("Phase 1 coverage keeps four pending entries after #56: the Linux nativepath.Realpath host "
                       "capture and fresh mutation evidence for WithFileNames, ThrottleGroup.Go and ThrottleGroup.Wait. "
                       "C0 consumes none of them.")
TRACE_DECISION = ("owner decision 2026-09-25: Phase 2 owns the module-resolution sub-test and its gate "
                  "(run.checker.trace_parity); C7 requires the 148 .trace.json baselines to pass subject to approved "
                  "divergences. Phase 1 remains responsible for the resolver implementation: a trace difference is a "
                  "foundation defect to fix, not a Phase 2 checker gap.")


def owner_of(operation, rows):
    for prefix, owner in OWNERS:
        if operation.startswith(prefix):
            return owner
    checkpoints = Counter(row["checkpoint"] for row in rows)
    return "/".join(sorted(checkpoints)) if len(checkpoints) > 1 else next(iter(checkpoints))


def sort_key(diagnostic):
    return (diagnostic["file_hex"] or "", diagnostic["pos"], diagnostic["end"], diagnostic["code"],
            diagnostic["text_hex"], json.dumps(diagnostic, sort_keys=True))


def rust_declarations(row):
    phase = row["phases"].get("declaration")
    if phase is None:
        return None, None
    values, failure = [], None
    for file in phase.get("files", []):
        result = file["result"]
        if result["state"] != "executed":
            failure = failure or result
            continue
        values.extend(result.get("diagnostics", []))
    return values, failure


def build(native_dir, rust_dir, record=False):
    comparison = json.loads((Path(rust_dir) / "comparison.json").read_bytes())
    if comparison.get("partial", False):
        raise ValueError("the acceptance blocker register requires a full comparison, not an informational sample")
    _, native_report, native_rows = phase2_native.load_capture(native_dir)
    phase2_native.current(native_report)
    if comparison["native_observation_sha256"] != native_report["observation_sha256"]:
        raise ValueError("comparison was made against a different native capture")
    inventory = {row["id"]: row for row in phase2_inventory.executed()}
    native = {row["id"]: row for row in native_rows}
    rust_requests = json.loads((Path(rust_dir) / "requests.json").read_bytes())
    case_dir = {request["id"]: f"cases/{index:05d}" for index, request in enumerate(rust_requests)}
    rust = {request["id"]: p4.read(Path(rust_dir) / case_dir[request["id"]] / "result.json")["row"]
            for request in rust_requests}
    groups = defaultdict(lambda: {"variants": [], "domains": Counter(), "evidence": []})
    for row in comparison["rows"]:
        if "outcomes" not in row:
            continue
        withheld = {}
        for domain, category in row["outcomes"].items():
            if category == "unsupported":
                detail = row["details"][domain]
                withheld.setdefault(detail["operation"], []).append(domain)
        for operation, domains in withheld.items():
            group = groups[("unsupported", operation)]
            group["variants"].append(row["id"])
            group["domains"].update(domains)
            if len(group["evidence"]) < 5:
                group["evidence"].append({"variant": row["id"], "case": case_dir[row["id"]],
                                          "observation": f"unsupported: {operation}"})
        if native[row["id"]]["error_pre_diagnostics"] != native[row["id"]]["error_post_diagnostics"]:
            group = groups[("emit_order", "post-emit diagnostic order")]
            group["variants"].append(row["id"])
            group["domains"].update(["errors"])
            group["evidence"].append({"variant": row["id"], "observation":
                                      "native pre-emit and post-emit structured diagnostic sets differ; the "
                                      "baseline is the post-emit set"})
    entries = []
    for (kind, operation), group in sorted(groups.items(), key=lambda kv: (-len(kv[1]["variants"]), kv[0])):
        rows = [inventory[vid] for vid in group["variants"]]
        if kind == "emit_order":
            owner = "C5 emit resolver, with Phase 3 emit (joint)"
            missing = ("checking in the post-emit program order: the native harness emits before collecting "
                       "diagnostics, and these variants' diagnostics depend on that order")
        else:
            owner = owner_of(operation, rows)
            missing = operation
        entries.append({"id": f"B{len(entries) + 1:02d}", "kind": kind, "missing_operation": missing,
                        "owner": owner, "variants": sorted(group["variants"]),
                        "domains": dict(sorted(group["domains"].items())),
                        "checkpoints": dict(sorted(Counter(row["checkpoint"] for row in rows).items())),
                        "evidence": group["evidence"]})
    audit = Counter()
    audit_examples = defaultdict(list)
    for vid, observed in native.items():
        if observed["state"] != "executed" or not observed["emit_declarations"]:
            continue
        row = rust[vid]
        if "fatal" in row:
            state = "failed: " + row["fatal"]["class"]
        elif row["load"]["state"] != "executed":
            state = "not loaded: " + row["load"].get("class", row["load"]["state"])
        else:
            values, failure = rust_declarations(row)
            if values is None:
                raise ValueError("a native declaration request is missing from the Rust request: " + vid)
            if failure is not None:
                state = ("unsupported: " + failure["reason"] if failure.get("class") == "unsupported"
                         else "failed: " + failure.get("class", failure["state"]))
            elif sorted(values, key=sort_key) == sorted(observed["declaration_diagnostics"], key=sort_key):
                state = "working"
            else:
                state = "different"
        audit[state] += 1
        if state != "working" and len(audit_examples[state]) < 5:
            audit_examples[state].append(vid)
    register = {
        "version": 1, "pin": native_report["pin"],
        "comparison": {"native_observation_sha256": comparison["native_observation_sha256"],
                       "rust_capture_sha256": comparison["rust_capture_sha256"]},
        "scope": ("Dependencies that withhold Phase 2 observations, each with the execution evidence that "
                  "establishes it. A blocker explains a difference and never reclassifies it."),
        "entries": entries,
        "declaration_audit": {"native_requests": sum(audit.values()), "outcomes": dict(sorted(audit.items())),
                              "examples": {k: v for k, v in sorted(audit_examples.items())},
                              "rule": ("working paths keep their results; only a missing transform or emit-resolver "
                                       "operation becomes a blocker, under its Phase 3 or Phase 2 owner")},
        "module_resolution": TRACE_DECISION,
        "phase1_contracts": PHASE1_CONTRACTS, "phase1_pending": PHASE1_PENDING_NOTE,
    }
    if record:
        REGISTER.write_bytes(json.dumps(register, indent=1, sort_keys=True).encode() + b"\n")
    summary = {"entries": [(e["id"], e["kind"], e["missing_operation"][:60], e["owner"], len(e["variants"]))
                           for e in entries], "declaration_audit": register["declaration_audit"]["outcomes"]}
    print(json.dumps(summary, indent=1))
    return register


def withheld(comparison):
    """(operation, variant) pairs for every unsupported observation."""
    return {(row["details"][domain]["operation"], row["id"]) for row in comparison["rows"] if "outcomes" in row
            for domain, category in row["outcomes"].items() if category == "unsupported"}


def complete(register, comparison):
    """Every withheld observation is explained by an entry naming its operation."""
    if comparison.get("partial", False):
        return False
    named = {(entry["missing_operation"], vid) for entry in register["entries"] if entry["kind"] == "unsupported"
             for vid in entry["variants"]}
    return withheld(comparison) <= named and all(entry["evidence"] for entry in register["entries"])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("build")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--rust", type=Path, required=True)
    sub.add_argument("--record", action="store_true")
    args = parser.parse_args()
    build(args.native, args.rust, args.record)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 blockers failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
