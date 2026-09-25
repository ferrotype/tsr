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

C1 (docs/PHASE2-C1-plan.md, C1.10) adds, from four committed authorities that
are `[checker]` inputs by name:

* c1_regressions -- domains met in data/phase2/c1-baseline.json.gz (the
  authenticated C1-start row report, bound to the same native observation and
  inventory) and not met now, over the whole denominator;
* c1_open -- rows claimed in data/phase2/c1-claims.json (status `claimed`)
  with a domain not met;
* c1_failures -- failed rows whose panic module is a foundation module named
  by the claims file, plus failed claimed rows;
* c1_audit_complete -- data/phase2/c1-audit.json checks with no open
  disposition (scripts/phase2_audit.py);
* c1_contracts -- data/phase2/receipts/c1-contracts.json records exit code 0
  for the contract tests and its source inputs are unchanged;
* c1_complete -- regression_parity == 1, c1_open == 0, c1_regressions == 0,
  c1_failures == 0, c1_audit_complete and c1_contracts; false while any of
  them is unavailable.

`observe --witness c1-contracts` runs the contract tests and writes that
receipt. No threshold is introduced; PLAN's Phase 2 gate binds the parity
metrics at C7. A missing or stale capture leaves the correctness metrics
unavailable.
"""
from __future__ import annotations

import argparse
import contextlib
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_audit  # noqa: E402
import phase2_blockers  # noqa: E402
import phase2_compare  # noqa: E402
import phase2_corpus  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402

NATIVE = ROOT / "target/phase2/native"
RUST = ROOT / "target/phase2/rust"
MATCHED = ("match", "disabled")
CLAIMS = ROOT / "data/phase2/c1-claims.json"
AUDIT = phase2_audit.AUDIT
BASELINE = phase2_compare.BASELINE
RECEIPTS = ROOT / "data/phase2/receipts"
WITNESSES = {
    "c1-contracts": {
        "commands": [["cargo", "test", "-p", "tsr_checker", "--test", "c1_contracts", "--locked"],
                     ["cargo", "test", "-p", "tsr_checker", "--test", "c1_contracts", "--locked", "--release"]],
        # Everything whose change can alter the contract tests' outcome.
        "sources": ["crates/tsr_checker", "crates/tsr_arena", "crates/tsr_ast", "crates/tsr_binder",
                    "crates/tsr_compiler", "crates/tsr_core", "crates/tsr_diagnostics", "Cargo.lock",
                    "rust-toolchain.toml"],
    },
}


def source_inputs(paths):
    """Digest of every file under the listed paths, keyed by repository path."""
    found = {}
    for entry in paths:
        base = ROOT / entry
        files = [base] if base.is_file() else sorted(p for p in base.rglob("*") if p.is_file() and "target" not in p.parts)
        for path in files:
            found[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return found


def observe(identity, output=RECEIPTS):
    """Run one contract witness and retain its outcome and source binding."""
    spec = WITNESSES.get(identity)
    if spec is None:
        raise ValueError(f"unknown witness {identity!r}")
    inputs = source_inputs(spec["sources"])
    runs = []
    for command in spec["commands"]:
        process = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        runs.append({"command": command, "exit_code": process.returncode,
                     "stdout_sha256": digest(process.stdout), "stderr_sha256": digest(process.stderr),
                     "stdout_tail": process.stdout.decode(errors="replace")[-2000:],
                     "stderr_tail": process.stderr.decode(errors="replace")[-2000:]})
    if inputs != source_inputs(spec["sources"]):
        raise ValueError("sources changed while the witness ran")
    record = {"version": 1, "witness": identity, "state": "observed", "runs": runs, "source_inputs": inputs}
    Path(output).mkdir(parents=True, exist_ok=True)
    path = Path(output) / f"{identity}.json"
    path.write_bytes(json.dumps(record, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({"receipt": str(path), "exit_codes": [run["exit_code"] for run in runs]}))
    return record


def receipt_current(identity, path=None):
    """True when the receipt records success and none of its source inputs changed."""
    path = Path(path) if path else RECEIPTS / f"{identity}.json"
    if not path.is_file():
        return None
    record = strict_json_loads(path.read_bytes())
    if record.get("state") != "observed" or record.get("witness") != identity:
        return False
    if any(run["exit_code"] != 0 for run in record["runs"]):
        return False
    return record["source_inputs"] == source_inputs(WITNESSES[identity]["sources"])


def c1_metrics(comparison, claims, audit_ok, baseline, contracts_ok, regression_parity):
    """The C1 exit metrics from a full comparison and the four authorities."""
    rows = {row["id"]: row for row in comparison["rows"] if "outcomes" in row}
    metrics = {}
    if baseline is not None:
        metrics["c1_regressions"] = len(phase2_compare.regressions_against(baseline, comparison))
    if claims is not None:
        claimed = [entry["id"] for entry in claims.get("rows", []) if entry.get("status") == "claimed"]
        unknown = [vid for vid in claimed if vid not in rows]
        if unknown:
            raise ValueError(f"claim names an unknown executed variant: {unknown[0]}")
        metrics["c1_open"] = sum(any(o not in MATCHED for o in rows[vid]["outcomes"].values()) for vid in claimed)
        modules = tuple(f"panic: tsr_checker::{name}" for name in claims.get("foundation_modules", []))
        failed = [row for row in rows.values() if "failed" in row["outcomes"].values()]
        metrics["c1_failures"] = sum(row.get("bucket", "").startswith(modules) or row["id"] in claimed
                                     for row in failed if modules or claimed)
    if audit_ok is not None:
        metrics["c1_audit_complete"] = audit_ok
    if contracts_ok is not None:
        metrics["c1_contracts"] = contracts_ok
    metrics["c1_complete"] = (regression_parity == 1 and metrics.get("c1_open") == 0
                              and metrics.get("c1_regressions") == 0 and metrics.get("c1_failures") == 0
                              and metrics.get("c1_audit_complete") is True and metrics.get("c1_contracts") is True)
    return metrics


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
        claims = strict_json_loads(CLAIMS.read_bytes()) if CLAIMS.is_file() else None
        audit_ok = phase2_audit.complete(phase2_audit.load(AUDIT)) if AUDIT.is_file() else None
        baseline = phase2_compare.load_baseline(BASELINE) if BASELINE.is_file() else None
        metrics.update(c1_metrics(comparison, claims, audit_ok, baseline, receipt_current("c1-contracts"),
                                  metrics["regression_parity"]))
    except (OSError, ValueError, KeyError) as error:
        print("C1 metrics unavailable: " + str(error), file=sys.stderr)
        metrics["c1_complete"] = False
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
    parser.add_argument("command", choices=("checker", "observe"))
    parser.add_argument("--native", type=Path, default=NATIVE)
    parser.add_argument("--rust", type=Path, default=RUST)
    parser.add_argument("--witness", help="contract witness identity for observe")
    args = parser.parse_args()
    if args.command == "observe":
        if not args.witness:
            raise ValueError("observe requires --witness")
        observe(args.witness)
        return
    print(json.dumps(checker(args.native, args.rust), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
