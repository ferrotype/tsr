#!/usr/bin/env python3
"""Phase 2 C0.6: the `checker` producer (status/runs.toml `[checker]`).

Replays the recorded captures and never runs a corpus: the native capture at
target/phase2/native and the Rust capture at target/phase2/rust. Metrics:

* inventory_frozen -- data/phase2/inventory.json rebuilds byte for byte from
  current inputs;
* native_verified -- the native capture is current, verified from a second
  sharding, agrees with every committed reference, and its review equals the
  committed data/phase2/native-provenance.json;
* harness_valid -- the Rust capture is complete, current, built from the
  current sources, bound to the current native capture and requests, and has
  no harness error;
* result_recorded -- that run replays and its comparison equals the committed
  data/phase2/first-comparison.json;
* errors_parity, types_parity, symbols_parity, display_parity -- ratios over
  the executed denominator (native-disabled domains count as met; failures and
  unsupported rows count against); trace_parity over the variants that trace;
  ordering and parent_pointers over the executed denominator;
* unsupported_required -- executed variants with an unsupported domain;
* blockers_named -- data/phase2/blockers.json equals the register rebuilt from
  the evidence and names every withheld observation;
* regression_parity -- the S08 acceptance variants the runner executes
  (9,367) with every domain met.

No threshold is introduced; PLAN's Phase 2 gate binds the parity metrics at C7.
A missing or stale capture leaves the correctness metrics unavailable.
"""
from __future__ import annotations

import argparse
import contextlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_blockers  # noqa: E402
import phase2_compare  # noqa: E402
import phase2_corpus  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402

NATIVE = ROOT / "target/phase2/native"
RUST = ROOT / "target/phase2/rust"
MATCHED = ("match", "disabled")


def quietly(function, *args, **kwargs):
    """Library calls print human summaries; stdout carries only the metrics."""
    with contextlib.redirect_stdout(sys.stderr):
        return function(*args, **kwargs)


def ratio(rows, domain, *, exclude_disabled=False):
    selected = [row for row in rows if not (exclude_disabled and row["outcomes"][domain] == "disabled")]
    if not selected:
        return None
    return sum(row["outcomes"][domain] in MATCHED for row in selected) / len(selected)


def checker(native=NATIVE, rust=RUST):
    metrics = {"inventory_frozen": False, "native_verified": False, "harness_valid": False,
               "result_recorded": False, "blockers_named": False}
    try:
        document = phase2_inventory.read()
        metrics["inventory_frozen"] = (phase2_inventory.INVENTORY.read_bytes()
                                       == phase2_inventory.render(phase2_inventory.build())
                                       and document["counts"]["executed"] > 0)
    except (OSError, ValueError, KeyError) as error:
        print("inventory unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    try:
        _, report, _ = phase2_native.load_capture(native)
        phase2_native.current(report)
        verified = strict_json_loads((Path(native) / "verified.json").read_bytes())
        review = quietly(phase2_native.review, native, False)
        committed = strict_json_loads(phase2_native.PROVENANCE.read_bytes())
        metrics["native_verified"] = (verified["observation_sha256"] == report["observation_sha256"]
                                      and review == committed and not review["reference_disagreements"]
                                      and not review["input_mismatches"]
                                      and review["states"] == {"executed": document["counts"]["executed"]})
    except (OSError, ValueError, KeyError) as error:
        print("native capture unavailable: " + str(error), file=sys.stderr)
    if not metrics["native_verified"]:
        return {"metrics": metrics}
    try:
        replayed = quietly(phase2_corpus.replay, rust)
        capture = strict_json_loads((Path(rust) / "capture.json").read_bytes())
        _, current_requests, _ = quietly(phase2_corpus.requests, native)
        metrics["harness_valid"] = (not replayed["summary"]["partial"] and replayed["summary"]["harness_errors"] == 0
                                    and replayed["source_stable"]
                                    and replayed["summary"]["observed"] == document["counts"]["executed"]
                                    and capture["native"]["observation_sha256"] == report["observation_sha256"]
                                    and capture["requests_sha256"] == digest(
                                        phase2_corpus.p4.canonical(current_requests) + b"\n"))
    except (OSError, ValueError, KeyError) as error:
        print("Rust capture unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    if not metrics["harness_valid"]:
        print("Rust capture is partial, stale or invalid; acceptance metrics withheld", file=sys.stderr)
        return {"metrics": metrics}
    try:
        comparison = quietly(phase2_compare.report, native, rust)
    except (OSError, ValueError, KeyError) as error:
        print("comparison unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    summary = {key: value for key, value in comparison.items() if key != "rows"}
    recorded = json.loads(phase2_compare.RECORD.read_bytes()) if phase2_compare.RECORD.exists() else None
    metrics["result_recorded"] = metrics["harness_valid"] and recorded == summary
    rows = [row for row in comparison["rows"] if "outcomes" in row]
    for metric, domain in (("errors_parity", "errors"), ("types_parity", "types"), ("symbols_parity", "symbols"),
                           ("display_parity", "display"), ("ordering", "union_ordering"),
                           ("parent_pointers", "parent_pointers")):
        metrics[metric] = ratio(rows, domain)
    metrics["trace_parity"] = ratio(rows, "trace", exclude_disabled=True)
    metrics["unsupported_required"] = sum(any(o == "unsupported" for o in row["outcomes"].values()) for row in rows)
    regression = [row for row in rows if row["s08"] == "acceptance"]
    metrics["regression_parity"] = (sum(all(o in MATCHED for o in row["outcomes"].values()) for row in regression)
                                    / len(regression)) if regression else None
    try:
        register = quietly(phase2_blockers.build, native, rust)
        committed = json.loads(phase2_blockers.REGISTER.read_bytes()) if phase2_blockers.REGISTER.exists() else None
        metrics["blockers_named"] = register == committed and phase2_blockers.complete(register, comparison)
    except (OSError, ValueError, KeyError) as error:
        print("blocker register unavailable: " + str(error), file=sys.stderr)
    print("checker evidence: " + canonical({"native": report["observation_sha256"],
                                             "rust": comparison["rust_capture_sha256"]}).decode(), file=sys.stderr)
    return {"metrics": {k: v for k, v in metrics.items() if v is not None}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("checker",))
    parser.add_argument("--native", type=Path, default=NATIVE)
    parser.add_argument("--rust", type=Path, default=RUST)
    args = parser.parse_args()
    print(json.dumps(checker(args.native, args.rust), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
