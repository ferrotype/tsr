#!/usr/bin/env python3
"""Phase 2 C3.9: rebind a checkpoint's handoffs to a fresh Rust capture.

Every handoff is bound to one Rust capture: the blocker builder drops an entry
whose capture, request or raw-observation digests differ from the current
comparison's. `rebind --checkpoint C3 --rust DIR` re-validates each `handed`
and `blocked` row and each `incoming` entry of that checkpoint's claims file
against the capture in DIR: it recomputes the request and raw-observation
digests from the authenticated capture, checks that the row's currently unmet
domains are still covered by the handoff's domains and that the trace artifact
still has its recorded digest, and rewrites the capture bindings. A handoff
that no longer holds is reported and left unchanged, so its row counts as open
for its owner. The claims file's `rust_capture_sha256` follows the capture; the
baseline binding does not move (the baseline is the checkpoint's start).
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare  # noqa: E402
from phase2_blockers import CHECKPOINT_CLAIMS, canonical, digest, p4  # noqa: E402

MATCHED = phase2_compare.MATCHED


def capture_digests(rust_dir):
    """Request and raw-observation digests of an authenticated capture."""
    replayed, requests, rows, _, _ = phase2_compare.load_rust(Path(rust_dir))
    if replayed["summary"]["partial"] or replayed["summary"]["harness_errors"]:
        raise ValueError("rebind requires a complete Rust capture without harness errors")
    return replayed["capture_sha256"], {request["id"]: {
        "request_sha256": digest(p4.canonical(request) + b"\n"),
        "raw_observation_sha256": digest(canonical(row)),
    } for request, row in zip(requests, rows, strict=True)}


def rebind(claims, comparison, capture_sha256, digests, *, root=ROOT):
    """Rewrite current handoffs; return the variants whose handoff no longer holds."""
    rows = {row["id"]: row for row in comparison["rows"] if "outcomes" in row}
    if comparison["rust_capture_sha256"] != capture_sha256:
        raise ValueError("the comparison was made against another capture")
    stale = []
    for entry in claims.get("rows", []):
        vid = entry["id"]
        for key in ("handoff", "incoming"):
            handoff = entry.get(key)
            if not isinstance(handoff, dict):
                continue
            if key == "handoff" and entry.get("status") not in ("handed", "blocked"):
                continue
            row, current = rows.get(vid), digests.get(vid)
            unmet = {domain for domain, outcome in row["outcomes"].items() if outcome not in MATCHED} if row else set()
            trace = handoff.get("trace", {})
            path = root / str(trace.get("path", ""))
            holds = (row is not None and current is not None and unmet <= set(handoff.get("domains", []))
                     and path.is_file() and digest(path.read_bytes()) == trace.get("sha256"))
            if not holds:
                stale.append(vid)
                continue
            handoff["capture_sha256"] = capture_sha256
            handoff["request_sha256"] = current["request_sha256"]
            handoff["raw_observation_sha256"] = current["raw_observation_sha256"]
    claims["rust_capture_sha256"] = capture_sha256
    return sorted(set(stale))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("rebind",))
    parser.add_argument("--checkpoint", required=True, choices=sorted(CHECKPOINT_CLAIMS))
    parser.add_argument("--rust", type=Path, required=True, help="the fresh, complete Rust capture")
    parser.add_argument("--native", type=Path, default=ROOT / "target/phase2/native")
    parser.add_argument("--claims", type=Path, help="override the checkpoint's claims file")
    args = parser.parse_args()
    claims_path = args.claims or CHECKPOINT_CLAIMS[args.checkpoint][0]
    claims = json.loads(claims_path.read_bytes())
    comparison = phase2_compare.report(args.native, args.rust, write=False)
    capture_sha256, digests = capture_digests(args.rust)
    stale = rebind(claims, comparison, capture_sha256, digests)
    claims_path.write_text(json.dumps(claims, indent=1, sort_keys=True) + "\n")
    print(json.dumps({"checkpoint": args.checkpoint, "capture_sha256": capture_sha256,
                      "rebound": sum(1 for e in claims["rows"] if e.get("status") in ("handed", "blocked")
                                     or "incoming" in e) - len(stale),
                      "stale": stale}, indent=1))
    if stale:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 claims failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
