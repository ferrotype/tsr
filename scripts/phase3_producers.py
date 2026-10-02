#!/usr/bin/env python3
"""Phase 3 T0: the `emit` producer (status/runs.toml `[emit]`).

Replays recorded captures and never runs a corpus: the native captures at
target/phase3/native-single and target/phase3/native-concurrent, and the
committed transpile observation. Metrics:

* inventory_frozen -- data/phase3/inventory.json rebuilds byte for byte from
  its current inputs;
* native_verified, native_verified_concurrent -- that mode's native capture is
  complete and current, was reproduced from a second sharding, has no native
  failure or failed sub-test, composes every committed reference baseline,
  owes exactly what the inventory says, and its review equals the committed
  data/phase3/native-provenance-<mode>.json;
* transpile_native_verified -- data/phase3/transpile-native.json was observed
  by the current oracle and scripts, every configuration executed, and its
  runs compose exactly the inventory's transpile reference baselines;
* harness_valid, harness_valid_concurrent -- that mode's Rust capture
  (target/phase3/rust-<mode>, scripts/phase3_corpus.py) is complete (not a
  sample, every executed variant observed), current with the sources it was
  built from and with the requests the inventory and native capture give now,
  bound to that mode's verified native capture, and has no harness error in
  its replay or its comparison;
* result_recorded -- the single-mode comparison's acceptance summary equals the
  committed data/phase3/first-comparison.json;
* reprint_parity -- rows whose reprint domain matches, over the executed
  denominator;
* output_parity, sourcemap_parity, sourcemap_record_parity,
  emit_diagnostics_parity -- rows whose domain matches (or is disabled with
  the pin's reason), over the executed denominator;
* declaration_parity -- rows whose declaration domain matches, over the
  rows that require it (the pin emitted a declaration file, or the Rust row
  differs from it);
* unsupported_required -- executed variants with an unsupported domain;
* mode_parity -- both modes' native captures verified and Rust captures
  harness-valid, and zero outcome differences between the two runs, each
  compared with its own mode's native capture
  (`scripts/phase3_compare.py modes`);
* blockers_named -- data/phase3/blockers.json equals the register rebuilt from
  the single-mode comparison (scripts/phase3_blockers.py) and names every row
  that cannot pass with an owning checkpoint.

The ratios and counts are emitted only over a harness-valid single-mode run;
`transpile_parity` and the closure metrics of docs/PHASE3-plan.md section 5
come with T8's closure.
No threshold is introduced. A missing or stale capture leaves its metric false.
"""
from __future__ import annotations

import argparse
import contextlib
import io
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import s08_p4 as p4  # noqa: E402
import phase3_blockers  # noqa: E402
import phase3_compare  # noqa: E402
import phase3_corpus  # noqa: E402
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402

NATIVE = {mode: ROOT / f"target/phase3/native-{mode}" for mode in phase3_native.MODES}
RUST = {mode: ROOT / f"target/phase3/rust-{mode}" for mode in phase3_native.MODES}
TRANSPILE = phase3_native.DATA / "transpile-native.json"
METRIC = {"single": "native_verified", "concurrent": "native_verified_concurrent"}
HARNESS_METRIC = {"single": "harness_valid", "concurrent": "harness_valid_concurrent"}


def quietly(function, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return function(*args, **kwargs)


def native_verified(mode, directory, document):
    _, report, _ = phase3_native.load_capture(directory)
    phase3_native.current(report)
    if report["mode"] != mode or report["request_sha256"] != digest(canonical(phase3_inventory.requests(document)) + b"\n"):
        return False
    review = quietly(phase3_native.review, directory, False)
    committed = strict_json_loads(phase3_native.provenance(mode).read_bytes())
    return (review == committed and review["states"] == {"executed": document["counts"]["executed"]}
            and not review["reference_disagreements"] and not review["inventory_disagreements"]
            and not review["failed_domains"])


def transpile_verified(document):
    observed = strict_json_loads(TRANSPILE.read_bytes())
    if observed["inputs"] != phase3_native.input_digests():
        return False
    overlay = phase3_native.overlay_sources(phase3_native.pinned_upstream())
    if observed["oracle"]["overlay_sha256"] != {name: digest(text.encode()) for name, text in sorted(overlay.items())}:
        return False
    owed = {entry["name"]: entry["git_blob"] for entry in document["transpile"]["references"]}
    composed = {}
    for row in observed["rows"]:
        if row["state"] != "executed":
            return False
        for run in row["runs"]:
            item = run["baseline"]
            if item["state"] != "content" or item["name"] in composed:
                return False
            composed[item["name"]] = phase3_inventory.git_blob(bytes.fromhex(item["text_hex"]))
    cases = {entry["path"].rsplit("/", 1)[1]: entry["sha256"] for entry in document["transpile"]["cases"]}
    sources = {row["file"].split("/", 1)[1]: row["source_sha256"] for row in observed["rows"]}
    return composed == owed and sources == cases and not observed["reference_disagreements"]


def capture_valid(mode, metadata, replayed, comparison, native_report, native_report_sha256, requests_sha256,
                  executed):
    """Whether one mode's Rust capture can stand for the denominator: complete,
    current with its sources and requests, bound to that mode's native capture,
    and free of harness errors."""
    return (metadata["mode"] == mode and native_report["mode"] == mode
            and not metadata["partial"] and not replayed["summary"]["partial"]
            and replayed["summary"]["observed"] == executed and replayed["summary"]["harness_errors"] == 0
            and replayed["source_stable"] is True and comparison["rust"]["source_stable"] is True
            and metadata["native"]["observation_sha256"] == native_report["observation_sha256"]
            and metadata["native"]["report_sha256"] == native_report_sha256
            and metadata["requests_sha256"] == requests_sha256
            and comparison["summary"]["valid"] and comparison["summary"]["rows"] == executed)


def harness(mode, native_dir, rust_dir, executed):
    """One mode's Rust capture: (valid, native, capture, comparison), where
    `native` and `capture` are the loaded captures and `comparison` the report
    of the capture against its mode's native capture."""
    native = phase3_compare.load_native(native_dir)
    capture = phase3_corpus.load_capture(rust_dir)
    replayed = quietly(phase3_corpus.replay, rust_dir, write=False, capture=capture)
    _, current_requests = quietly(phase3_corpus.requests, native_dir, mode=mode)
    comparison = phase3_compare.report(native_dir, rust_dir, native=native, capture=capture)
    valid = capture_valid(mode, capture[0], replayed, comparison, native[1],
                          digest((native[0] / "report.json").read_bytes()),
                          digest(p4.canonical(current_requests) + b"\n"), executed)
    return valid, native, capture, comparison


def matched_ratio(rows, domain):
    return sum(row["outcomes"][domain] in phase3_compare.MATCHED for row in rows) / len(rows) if rows else None


PARITY = {"output_parity": "output", "sourcemap_parity": "sourcemap", "sourcemap_record_parity": "sourcemap_record",
          "emit_diagnostics_parity": "emit_diagnostics"}


def parity(rows):
    """The emit parity metrics over the comparison's rows."""
    metrics = {name: matched_ratio(rows, domain) for name, domain in PARITY.items()}
    required = [row for row in rows if row["declaration_required"]]
    metrics["declaration_parity"] = matched_ratio(required, "declaration")
    return metrics


def emit(native=None, rust=None):
    native = native or NATIVE
    rust = rust or RUST
    metrics = {"inventory_frozen": False, "native_verified": False, "native_verified_concurrent": False,
               "transpile_native_verified": False, "harness_valid": False, "harness_valid_concurrent": False,
               "result_recorded": False, "blockers_named": False, "mode_parity": False}
    try:
        document = phase3_inventory.read()
        metrics["inventory_frozen"] = (phase3_inventory.INVENTORY.read_bytes()
                                       == phase3_inventory.render(phase3_inventory.build())
                                       and document["counts"]["executed"] > 0)
    except (OSError, ValueError, KeyError) as error:
        print("inventory unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    for mode, name in METRIC.items():
        try:
            metrics[name] = native_verified(mode, native[mode], document)
        except (OSError, ValueError, KeyError) as error:
            print(f"native {mode} capture unavailable: " + str(error), file=sys.stderr)
    try:
        metrics["transpile_native_verified"] = transpile_verified(document)
    except (OSError, ValueError, KeyError) as error:
        print("transpile observation unavailable: " + str(error), file=sys.stderr)
    executed = document["counts"]["executed"]
    states = {}
    for mode, name in HARNESS_METRIC.items():
        if not metrics[METRIC[mode]]:
            print(f"{mode} Rust capture not assessed: its native capture is not verified", file=sys.stderr)
            continue
        try:
            states[mode] = harness(mode, native[mode], rust[mode], executed)
            metrics[name] = states[mode][0]
        except (OSError, ValueError, KeyError) as error:
            print(f"{mode} Rust capture unavailable: " + str(error), file=sys.stderr)
    if not metrics["harness_valid"]:
        print("the single-mode Rust capture is partial, stale or invalid; acceptance metrics withheld", file=sys.stderr)
        return {"metrics": metrics}
    _, _, capture, comparison = states["single"]
    summary = phase3_compare.acceptance_summary(comparison)
    try:
        recorded = strict_json_loads(phase3_compare.RECORD.read_bytes()) if phase3_compare.RECORD.exists() else None
        metrics["result_recorded"] = recorded == summary
    except (OSError, ValueError) as error:
        print("recorded comparison unavailable: " + str(error), file=sys.stderr)
    rows = comparison["rows"]
    metrics["reprint_parity"] = matched_ratio(rows, "reprint")
    metrics.update(parity(rows))
    metrics["unsupported_required"] = sum(any(o == "unsupported" for o in row["outcomes"].values()) for row in rows)
    try:
        register = phase3_blockers.build(native["single"], rust["single"], comparison=comparison, capture=capture)
        committed = (strict_json_loads(phase3_blockers.REGISTER.read_bytes())
                     if phase3_blockers.REGISTER.exists() else None)
        metrics["blockers_named"] = register == committed and phase3_blockers.complete(register, comparison)
    except (OSError, ValueError, KeyError) as error:
        print("blocker register unavailable: " + str(error), file=sys.stderr)
    if metrics["harness_valid_concurrent"]:
        try:
            loaded = {mode: (states[mode][1], states[mode][2]) for mode in HARNESS_METRIC}
            modes = phase3_compare.modes(native["single"], rust["single"], native["concurrent"], rust["concurrent"],
                                         write=False, loaded=loaded)
            metrics["mode_parity"] = (metrics["native_verified"] and metrics["native_verified_concurrent"]
                                      and modes["outcome_differences"] == 0
                                      and modes["single"]["rust_capture_sha256"] == comparison["rust"]["capture_sha256"])
        except (OSError, ValueError, KeyError) as error:
            print("mode comparison unavailable: " + str(error), file=sys.stderr)
    print("emit evidence: " + canonical({"native": comparison["native"]["observation_sha256"],
                                         "rust": comparison["rust"]["capture_sha256"]}).decode(), file=sys.stderr)
    return {"metrics": metrics}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("emit",))
    parser.add_argument("--native-single", type=Path, default=NATIVE["single"])
    parser.add_argument("--native-concurrent", type=Path, default=NATIVE["concurrent"])
    parser.add_argument("--rust-single", type=Path, default=RUST["single"])
    parser.add_argument("--rust-concurrent", type=Path, default=RUST["concurrent"])
    args = parser.parse_args()
    print(json.dumps(emit({"single": args.native_single, "concurrent": args.native_concurrent},
                          {"single": args.rust_single, "concurrent": args.rust_concurrent}), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
