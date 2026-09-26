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
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s08_p4 as p4  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_compare  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402
import phase2_audit  # noqa: E402

REGISTER = ROOT / "data/phase2/blockers.json"
CLAIMS = ROOT / "data/phase2/c2-claims.json"
AUDIT = ROOT / "data/phase2/c2-audit.json"
TARGETS = {*(f"C{i}" for i in range(1, 8)), "Phase 3", "Phase 4", "Phase 5"}
EMIT_OPERATION = "post-emit diagnostic order"
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


def owner_of(operation, rows, ownership=None):
    if ownership is not None:
        return "/".join(sorted({item["effective_owner"] for item in ownership}))
    for prefix, owner in OWNERS:
        if operation.startswith(prefix):
            return owner
    checkpoints = Counter(row["checkpoint"] for row in rows)
    return "/".join(sorted(checkpoints)) if len(checkpoints) > 1 else next(iter(checkpoints))


def capture_context(directory, comparison):
    """Authenticate raw observations before using them to exempt any difference."""
    replayed, requests, rows, _, _ = phase2_compare.load_rust(Path(directory))
    if (replayed["summary"]["partial"] or replayed["summary"]["harness_errors"]
            or not replayed["source_stable"]
            or replayed["capture_sha256"] != comparison["rust_capture_sha256"]):
        raise ValueError("handoffs require the current authenticated full Rust capture")
    return {request["id"]: {
        "request_sha256": digest(p4.canonical(request) + b"\n"),
        # Hash the entire observation, including panic_location, not the
        # comparator's fatal-domain digest (which deliberately omits location).
        "raw_observation_sha256": digest(canonical(row)),
    } for request, row in zip(requests, rows, strict=True)}


def cause_domains(row, kind, operation):
    if kind == "emit_order" and operation == EMIT_OPERATION:
        detail = row.get("details", {}).get("errors", {})
        return {"errors"} if "native_pre_post" in detail.get("differences", []) else set()
    if kind not in ("unsupported", "failed"):
        return set()
    key = "operation" if kind == "unsupported" else "reason"
    return {domain for domain, outcome in row["outcomes"].items()
            if outcome == kind and row.get("details", {}).get(domain, {}).get(key) == operation}


def validated_handoffs(checkpoint, claims, comparison, context, *, root=ROOT, known=None,
                       owned_functions=(), incoming=False, inventory=None):
    """Validate schema; return only current, fully covered domain transfers.

    Malformed authorities raise. Stale observations, artifacts or uncovered
    domains invalidate the exemption and leave the row with its original owner.
    """
    known = phase2_audit.inventory() if known is None else known
    owners = {row["id"]: row["checkpoint"] for row in
              (phase2_inventory.executed() if inventory is None else inventory)} if incoming else {}
    rows = {row["id"]: row for row in comparison["rows"]}
    transfers, seen = {}, set()
    for entry in (claims or {}).get("rows", []):
        vid = entry.get("id")
        if vid not in rows or vid in seen:
            raise ValueError("handoff has unknown or duplicate variant: " + str(vid))
        seen.add(vid)
        if (incoming and "incoming" not in entry) or (not incoming and entry.get("status") not in ("handed", "blocked")):
            continue
        handoff = entry.get("incoming" if incoming else "handoff")
        if not isinstance(handoff, dict):
            raise ValueError(f"handoff {vid} requires a trace-bound handoff object")
        owner, function = handoff.get("owner"), handoff.get("go")
        valid_owner = (owner == checkpoint and handoff.get("from") == owners.get(vid)
                       and handoff.get("from") in TARGETS - {checkpoint}) if incoming else owner in TARGETS - {checkpoint}
        valid_function = function in owned_functions if incoming else function not in owned_functions
        if not valid_owner or function not in known or not valid_function:
            direction = "inside" if incoming else "outside"
            raise ValueError(f"handoff {vid} needs a known function {direction} {checkpoint} and matching owner")
        argv, domains, trace = handoff.get("reproduce"), handoff.get("domains"), handoff.get("trace")
        if (not isinstance(argv, list) or not all(isinstance(v, str) and v for v in argv)
                or argv.count("--case") != 1 or argv.index("--case") + 1 >= len(argv)
                or argv[argv.index("--case") + 1] != vid):
            raise ValueError(f"handoff {vid} reproduction must name its exact --case")
        if (not isinstance(domains, list) or not domains or not all(isinstance(d, str) for d in domains)
                or len(set(domains)) != len(domains)
                or not set(domains) <= set(phase2_compare.DOMAINS)
                or not isinstance(handoff.get("cause"), str) or not handoff["cause"].strip()):
            raise ValueError(f"handoff {vid} needs valid domains and traced cause")
        if not isinstance(trace, dict) or not isinstance(trace.get("path"), str):
            raise ValueError(f"handoff {vid} requires a trace artifact")
        path = Path(trace["path"])
        if path.is_absolute() or ".." in path.parts or not path.parts or not (root / path).resolve().is_relative_to(root.resolve()):
            raise ValueError(f"handoff {vid} trace must be a repository-relative artifact")
        hashes = [handoff.get(key) for key in ("capture_sha256", "request_sha256", "raw_observation_sha256")]
        hashes.append(trace.get("sha256"))
        if any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in hashes):
            raise ValueError(f"handoff {vid} needs complete SHA-256 bindings")
        blocker = handoff.get("blocker")
        if not incoming and entry["status"] == "blocked" and not isinstance(blocker, dict):
            raise ValueError(f"blocked handoff {vid} requires kind/operation identity")
        if blocker is not None and (not isinstance(blocker, dict) or set(blocker) != {"kind", "operation"}
                                    or blocker["kind"] not in ("unsupported", "failed", "emit_order")
                                    or not isinstance(blocker["operation"], str) or not blocker["operation"]):
            raise ValueError(f"handoff {vid} has invalid blocker identity")
        unmet = {domain for domain, outcome in rows[vid]["outcomes"].items() if outcome not in phase2_compare.MATCHED}
        current = context.get(vid, {})
        if (handoff["capture_sha256"] != comparison["rust_capture_sha256"]
                or any(handoff[key] != current.get(key) for key in ("request_sha256", "raw_observation_sha256"))
                or not unmet <= set(domains) or not (root / path).is_file()
                or digest((root / path).read_bytes()) != trace["sha256"]):
            continue
        if blocker is not None and not unmet <= cause_domains(rows[vid], blocker["kind"], blocker["operation"]):
            continue
        transfers[vid] = handoff
    return transfers


def audit_owned_functions(document, checkpoint):
    dispositions = document.get("dispositions", {})
    names = {name for members in document.get("groups", {}).values() for name in members}
    names.update(document.get("ownership", {}).get("functions", []))
    return {name for name in names
            if not (dispositions.get(name, {}).get("disposition") == "later"
                    and dispositions[name].get("owner") != checkpoint)}


def stable_blocker(entry):
    """Resolve legacy C1 emit-order references by cause, never their B number."""
    identity = entry.get("blocker_identity")
    if identity is not None:
        return identity
    if entry.get("bucket") == "emit order: native pre/post-emit sets differ":
        return {"kind": "emit_order", "operation": EMIT_OPERATION, "domains": ["errors"]}
    return None


def covering_blocker(register, vid, identity):
    for entry in (register or {}).get("entries", []):
        if (entry.get("kind") != identity.get("kind")
                or entry.get("operation", EMIT_OPERATION if entry.get("kind") == "emit_order"
                             else entry.get("missing_operation")) != identity.get("operation")
                or vid not in entry.get("variants", [])):
            continue
        domains = {item["domain"] for item in entry.get("ownership", []) if item.get("variant") == vid}
        if "ownership" not in entry and entry.get("kind") == "emit_order":
            domains = {"errors"}  # the legacy C1 cause has exactly this domain
        if set(identity.get("domains", [])) <= domains:
            return entry
    return None


def effective_ownership(kind, operation, vid, domains, inventory_owner, handoffs, incoming=None):
    handoff = handoffs.get(vid)
    default_owner = "C5" if kind == "emit_order" else inventory_owner
    if operation.startswith("content-mapper execution"):
        default_owner = "Phase 5"
    result = []
    for domain in sorted(domains):
        received = (incoming or {}).get(vid)
        owner = received["owner"] if received and domain in received["domains"] else default_owner
        transferred = (handoff is not None and domain in handoff["domains"]
                       and handoff.get("blocker") == {"kind": kind, "operation": operation})
        result.append({"variant": vid, "domain": domain, "inventory_owner": inventory_owner,
                       "effective_owner": handoff["owner"] if transferred else owner})
    return result


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


def build(native_dir, rust_dir, record=False, *, handoffs=None, incoming=None):
    comparison = phase2_compare.report(native_dir, rust_dir, write=False)
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
    if handoffs is None:
        claims = p4.read(CLAIMS) if CLAIMS.is_file() else None
        audit = p4.read(AUDIT) if AUDIT.is_file() else {}
        has_transfers = claims and any(entry.get("status") in ("handed", "blocked") or "incoming" in entry
                                      for entry in claims.get("rows", []))
        if has_transfers:
            if not audit or phase2_audit.problems(audit, allow_open=True):
                raise ValueError("handoff ownership requires a valid reviewed C2 audit scope")
        context = capture_context(rust_dir, comparison) if has_transfers else {}
        handoffs = validated_handoffs("C2", claims, comparison, context,
                                     owned_functions=audit_owned_functions(audit, "C2")) if has_transfers else {}
        incoming = validated_handoffs("C2", claims, comparison, context, incoming=True,
                                     owned_functions=audit_owned_functions(audit, "C2")) if has_transfers else {}
    groups = defaultdict(lambda: {"variants": [], "domains": Counter(), "evidence": [], "ownership": []})
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
            group["ownership"].extend(effective_ownership("unsupported", operation, row["id"], domains,
                                                           inventory[row["id"]]["checkpoint"], handoffs, incoming))
            if len(group["evidence"]) < 5:
                group["evidence"].append({"variant": row["id"], "case": case_dir[row["id"]],
                                          "observation": f"unsupported: {operation}"})
        if native[row["id"]]["error_pre_diagnostics"] != native[row["id"]]["error_post_diagnostics"]:
            group = groups[("emit_order", EMIT_OPERATION)]
            group["variants"].append(row["id"])
            group["domains"].update(["errors"])
            group["ownership"].extend(effective_ownership("emit_order", EMIT_OPERATION, row["id"], ["errors"],
                                                           inventory[row["id"]]["checkpoint"], handoffs, incoming))
            group["evidence"].append({"variant": row["id"], "observation":
                                      "native pre-emit and post-emit structured diagnostic sets differ; the "
                                      "baseline is the post-emit set"})
        failures = defaultdict(list)
        for domain, category in row["outcomes"].items():
            if category == "failed":
                failures[row["details"][domain]["reason"]].append(domain)
        for operation, domains in failures.items():
            group = groups[("failed", operation)]
            group["variants"].append(row["id"])
            group["domains"].update(domains)
            group["ownership"].extend(effective_ownership("failed", operation, row["id"], domains,
                                                           inventory[row["id"]]["checkpoint"], handoffs, incoming))
            group["evidence"].append({"variant": row["id"], "case": case_dir[row["id"]],
                                      "observation": operation, "raw_observation_sha256": digest(canonical(rust[row["id"]]))})
    entries = []
    for (kind, operation), group in sorted(groups.items(), key=lambda kv: (-len(kv[1]["variants"]), kv[0])):
        rows = [inventory[vid] for vid in group["variants"]]
        if kind == "emit_order":
            owner = "C5 emit resolver, with Phase 3 emit (joint)"
            missing = ("checking in the post-emit program order: the native harness emits before collecting "
                       "diagnostics, and these variants' diagnostics depend on that order")
        else:
            owner = owner_of(operation, rows, group["ownership"])
            missing = operation
        entries.append({"id": f"B{len(entries) + 1:02d}", "kind": kind, "operation": operation, "missing_operation": missing,
                        "owner": owner, "variants": sorted(group["variants"]),
                        "domains": dict(sorted(group["domains"].items())),
                        "checkpoints": dict(sorted(Counter(row["checkpoint"] for row in rows).items())),
                        "evidence": group["evidence"], "ownership": group["ownership"]})
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
        "version": 2, "pin": native_report["pin"],
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
    """Every refusal or failure has its exact cause, variant and domain named."""
    if comparison.get("partial", False):
        return False
    named = {(entry["kind"], entry.get("operation", entry.get("missing_operation")), item["variant"], item["domain"])
             for entry in register["entries"] if entry["kind"] in ("unsupported", "failed")
             for item in entry.get("ownership", [])}
    needed = {(category, row["details"][domain]["operation" if category == "unsupported" else "reason"], row["id"], domain)
              for row in comparison["rows"] if "outcomes" in row
              for domain, category in row["outcomes"].items() if category in ("unsupported", "failed")}
    return needed <= named and all(entry["evidence"] for entry in register["entries"])


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
