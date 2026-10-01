#!/usr/bin/env python3
"""Phase 3 T0: the emit blocker register, derived only from execution evidence.

One entry per bucket of rows that cannot pass yet: the rows of a full,
harness-valid comparison (`scripts/phase3_compare.py`) grouped by category
(`unsupported`, `failed`, `different`, `unexecuted`) and cause (the named
refusal, the failure reason, or the differing domain and kind). Each entry
carries its row count, a digest of its sorted row identities, the domains it
withholds with counts, examples with their capture case, and the owning
checkpoint of docs/PHASE3-plan.md section 4:

* a refusal that names its checkpoint (`... Phase 3 T8`) is that checkpoint's;
* a refusal whose reason is the checker's unsupported-operation text is a
  resolver query Phase 2 lacks: a cross-phase entry owned by Phase 2
  maintenance (plan section 3);
* any other cause is owned by its domains' checkpoints: `reprint` T1, the
  source-map sub-tests T2, `declaration` T7, `output` and `emit_diagnostics`
  T8 (the driver, until the comparison attributes a difference to a
  transform).

The plan's two cross-phase dependencies are reported from the evidence: the
resolver queries as above, and the bounded work group from the production
source that implements it and its contract test. A blocker explains a
difference; it never hides or reclassifies one.

    build --native DIR --rust DIR [--record]   # writes data/phase3/blockers.json with --record
    check --native DIR --rust DIR              # the committed register equals the rebuilt one
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import s08_p4 as p4  # noqa: E402
import phase3_compare as compare  # noqa: E402
import phase3_corpus  # noqa: E402

REGISTER = ROOT / "data/phase3/blockers.json"
NATIVE = ROOT / "target/phase3/native-single"
RUST = ROOT / "target/phase3/rust-single"
CHECKPOINTS = tuple(f"T{number}" for number in range(1, 9))
DOMAIN_OWNERS = {"reprint": "T1", "sourcemap": "T2", "sourcemap_record": "T2", "output": "T8",
                 "emit_diagnostics": "T8", "declaration": "T7"}
NAMED_CHECKPOINT = re.compile(r"\bPhase 3 (T[1-8])\b")
# tsr_checker::Error::Unsupported's Display: a resolver query the checker lacks.
CHECKER_UNSUPPORTED = "unsupported checker operation: "
RESOLVER_OWNER = "Phase 2 maintenance (emit resolver)"
WORK_GROUP = ROOT / "crates/tsr_core/src/workgroup.rs"
WORK_GROUP_EVIDENCE = ("pub fn worker_bound()", "fn a_large_parallel_group_runs_on_bounded_workers()")
EXAMPLES = 5


def cause_of(domain, detail):
    category = detail["category"]
    if category == "unsupported":
        return detail["operation"]
    if category in ("failed", "unexecuted"):
        return detail["reason"]
    return f"{domain} differs" + (f": {detail['kind']}" if "kind" in detail else "")


def owner_of(category, cause, domains):
    """(owner, cross_phase) of one bucket."""
    if category == "unsupported" and cause.startswith(CHECKER_UNSUPPORTED):
        return RESOLVER_OWNER, True
    named = NAMED_CHECKPOINT.search(cause)
    if named:
        return named.group(1), False
    return "/".join(sorted({DOMAIN_OWNERS[domain] for domain in domains})), False


def buckets(comparison):
    """{(category, cause): {"variants": [ids], "domains": Counter}} over every
    domain of every row that is neither matched nor disabled."""
    groups = defaultdict(lambda: {"variants": [], "domains": Counter()})
    for row in comparison["rows"]:
        seen = set()
        for domain, category in row["outcomes"].items():
            if category in compare.MATCHED:
                continue
            key = (category, cause_of(domain, row["details"][domain]))
            groups[key]["domains"][domain] += 1
            if key not in seen:
                groups[key]["variants"].append(row["id"])
                seen.add(key)
    return groups


def work_group_state():
    """The bounded work group (plan section 3): landed when the production
    source carries the bound and its contract test."""
    text = WORK_GROUP.read_text() if WORK_GROUP.is_file() else ""
    landed = all(marker in text for marker in WORK_GROUP_EVIDENCE)
    return {"dependency": "bounded work group", "owner": "Phase 2 (C6, amended by the #73 review)",
            "consumer": "T8 (Program.Emit queues one task per emitted file)",
            "state": "landed" if landed else "open",
            "evidence": str(WORK_GROUP.relative_to(ROOT)) + ": " + " and ".join(WORK_GROUP_EVIDENCE)}


def build(native_dir=NATIVE, rust_dir=RUST, record=False, *, comparison=None, capture=None):
    capture = capture or phase3_corpus.load_capture(rust_dir)
    comparison = comparison or compare.report(native_dir, rust_dir, capture=capture)
    summary = comparison["summary"]
    if summary["partial"]:
        raise ValueError("the blocker register requires a full comparison, not an informational sample")
    if not summary["valid"]:
        raise ValueError("the blocker register requires a harness-valid comparison")
    if comparison["rust"]["capture_sha256"] != digest(p4.canonical(capture[0]) + b"\n"):
        raise ValueError("comparison was made against a different capture")
    case_dir = {request["id"]: f"cases/{index:05d}" for index, request in enumerate(capture[1])}
    entries = []
    groups = buckets(comparison)
    for (category, cause), group in sorted(groups.items(), key=lambda kv: (-len(kv[1]["variants"]), kv[0])):
        owner, cross_phase = owner_of(category, cause, group["domains"])
        variants = group["variants"]
        entries.append({
            "id": f"B{len(entries) + 1:02d}", "kind": category, "cause": cause, "owner": owner,
            "cross_phase": cross_phase, "variants": len(variants),
            "variants_sha256": digest(canonical(sorted(variants))),
            "domains": dict(sorted(group["domains"].items())),
            "evidence": [{"variant": vid, "case": case_dir[vid],
                          "observation": f"{category}: {cause}"} for vid in variants[:EXAMPLES]],
        })
    resolver_rows = sum(entry["variants"] for entry in entries if entry["owner"] == RESOLVER_OWNER)
    register = {
        "version": 1, "pin": comparison["pin"],
        "comparison": {"native_observation_sha256": comparison["native"]["observation_sha256"],
                       "rust_capture_sha256": comparison["rust"]["capture_sha256"],
                       "rows": summary["rows"], "all_domains_met": summary["all_domains_met"]},
        "scope": ("Buckets of rows that cannot pass yet in the Phase 3 emit comparison, each with the execution "
                  "evidence that establishes it and its owning checkpoint (docs/PHASE3-plan.md section 4). A "
                  "blocker explains a difference and never reclassifies it."),
        "entries": entries,
        "by_owner": dict(sorted(Counter(entry["owner"] for entry in entries).items())),
        "cross_phase": [
            work_group_state(),
            {"dependency": "emit resolver queries the transforms need", "owner": RESOLVER_OWNER,
             "consumer": "T3 to T8 (every resolver call of the script and declaration transforms)",
             "state": "observed" if resolver_rows else "not observed", "variants": resolver_rows,
             "rule": "a refusal whose reason is the checker's unsupported-operation text becomes a cross-phase entry"},
        ],
    }
    if record:
        REGISTER.write_bytes(json.dumps(register, indent=1, sort_keys=True).encode() + b"\n")
    return register


def complete(register, comparison):
    """Every row that cannot pass is named, once per cause, with its exact rows
    and domain counts, and every entry has an owning checkpoint or a
    cross-phase owner and evidence."""
    if comparison["summary"]["partial"] or not comparison["summary"]["valid"]:
        return False
    if register["comparison"]["rust_capture_sha256"] != comparison["rust"]["capture_sha256"]:
        return False
    needed = {key: (len(group["variants"]), digest(canonical(sorted(group["variants"]))), dict(group["domains"]))
              for key, group in buckets(comparison).items()}
    named = {(entry["kind"], entry["cause"]): (entry["variants"], entry["variants_sha256"], entry["domains"])
             for entry in register["entries"]}
    owners_known = all(entry["evidence"] and (entry["cross_phase"] or set(entry["owner"].split("/")) <= set(CHECKPOINTS))
                       for entry in register["entries"])
    return needed == named and len(named) == len(register["entries"]) and owners_known


def check(native_dir=NATIVE, rust_dir=RUST):
    rebuilt = build(native_dir, rust_dir)
    committed = json.loads(REGISTER.read_bytes()) if REGISTER.exists() else None
    return rebuilt == committed, rebuilt


def brief(register):
    return {"entries": [(e["id"], e["kind"], e["cause"][:60], e["owner"], e["variants"]) for e in register["entries"]],
            "cross_phase": [(item["dependency"], item["state"]) for item in register["cross_phase"]]}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("build", "check"):
        sub = commands.add_parser(name)
        sub.add_argument("--native", type=Path, default=NATIVE)
        sub.add_argument("--rust", type=Path, default=RUST)
        if name == "build":
            sub.add_argument("--record", action="store_true")
    args = parser.parse_args()
    if args.command == "build":
        print(json.dumps(brief(build(args.native, args.rust, args.record)), indent=1))
        return
    same, rebuilt = check(args.native, args.rust)
    print(json.dumps(dict(brief(rebuilt), committed_equal=same), indent=1))
    if not same:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 blockers failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
