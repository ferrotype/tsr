#!/usr/bin/env python3
"""Phase 4: the `tsc` producer (status/runs.toml `[tsc]`).

Checks the recorded scenario inventory, re-runs its verification and replays
the recorded Rust capture at target/phase4/rust; it never runs the harness.
Metrics:

* inventory_frozen -- data/phase4/scenarios.json.gz is well formed and current
  (`phase4_scenarios.py check`: the pin, the recorder's patch digests, the
  ledger hashes of the patched pinned files, 516 scenarios with their digests);
* inventory_reproduced, inventory_identical, inventory_replayed -- the three
  checks of `phase4_scenarios.py verify`, which the producer runs (seconds; it
  needs the Go toolchain of data/s04/toolchains.toml on PATH): a fresh patched
  run of the pinned suite renders every committed reference it produces byte
  for byte, a second recording is byte-identical to the inventory, and the
  recorded scenarios replay through the pin's unpatched runner;
* inventory_verified -- all three;
* harness_valid -- the Rust capture (scripts/phase4_corpus.py) is complete (not
  partial, one row per scenario), taken over the current inventory, built from
  the current sources, every raw artifact bound, and without a harness error
  in its replay or its comparison;
* result_recorded -- the comparison's acceptance summary equals the committed
  data/phase4/first-comparison.json;
* blockers_named -- data/phase4/blockers.json equals the register rebuilt from
  the comparison (scripts/phase4_blockers.py) and names every scenario that
  cannot pass with an owner;
* unsupported_required -- scenarios whose row is a named refusal;
* baseline_parity -- scenarios whose rendered transcript equals the committed
  reference whole, over the inventory's scenarios;
* incremental_correctness -- edit steps whose incremental difference agrees
  with the reference's (none, or the same explained difference), over every
  edit step of the inventory;
* unit_tests -- data/phase4/unit-tests.json is current and valid
  (`phase4_unit_tests.py check`);
* audit -- data/phase4/x-audit.json is current and valid (`phase4_audit.py check`);
* smoke, live_watch_parity, buildinfo_interop, determinism, thread_sanitizer
  -- independently replayed X7 witnesses selected by --witnesses (default
  target/phase4/acceptance.json). Native metrics are host facts; smoke_all_hosts
  and live_watch_parity_all_hosts additionally require macOS and Linux records.

The parity ratios and the count are emitted only over a harness-valid capture;
a missing, partial or stale capture leaves harness_valid false and withholds
them. A scenario capture does not gate the independent X7 witnesses. No
threshold is introduced. Normal contract receipts supply buildinfo_codec,
unit_rosters and watcher_tests; the full comparison supplies residuals.
Checkpoint completions join these current facts, never source presence alone.
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
from s08_oracle import canonical  # noqa: E402
import phase4_audit  # noqa: E402
import phase4_acceptance  # noqa: E402
import phase4_blockers  # noqa: E402
import phase4_compare  # noqa: E402
import phase4_corpus  # noqa: E402
import phase4_contracts  # noqa: E402
import phase4_scenarios  # noqa: E402
import phase4_unit_tests  # noqa: E402

RUST = phase4_corpus.DEFAULT_OUTPUT
VERIFY_METRICS = {"reproduce": "inventory_reproduced", "identical": "inventory_identical",
                  "replay": "inventory_replayed"}


def quietly(function, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return function(*args, **kwargs)


def verification(inventory=phase4_scenarios.INVENTORY):
    """The three checks of `phase4_scenarios.py verify`: {check: passed}."""
    report, timings = quietly(phase4_scenarios.verify, inventory, set(phase4_scenarios.CHECKS))
    print("inventory verify: " + canonical({
        "reproduce": {key: report["reproduce"][key] for key in ("matched", "produced", "references", "orphans")},
        "identical": {key: report["identical"][key] for key in ("json_identical", "gzip_identical")},
        "replay": {key: report["replay"][key] for key in ("scenarios", "subtests_passed", "differing")},
        "seconds": {key: round(value, 1) for key, value in timings.items() if isinstance(value, float)},
    }).decode(), file=sys.stderr)
    return {name: report[name]["passed"] is True for name in phase4_scenarios.CHECKS}


def capture_valid(capture, replayed, comparison, scenarios):
    """Whether the Rust capture can stand for the denominator: complete, built
    from the current sources, and free of harness errors."""
    return (not capture.metadata["partial"] and capture.metadata["rows"] == scenarios
            and len(capture.rows) == scenarios
            and replayed["summary"]["harness_errors"] == 0 and replayed["source_stable"] is True
            and comparison["rust"]["source_stable"] is True
            and comparison["rust"]["capture_sha256"] == capture.sha256
            and comparison["summary"]["valid"] and not comparison["summary"]["partial"]
            and comparison["summary"]["rows"] == scenarios and comparison["summary"]["selected"] == scenarios)


def harness(rust):
    """(valid, capture, comparison) of the capture at `rust`."""
    capture = phase4_corpus.load_capture(rust)
    replayed = phase4_corpus.replay(rust, write=False, capture=capture)
    comparison = phase4_compare.report(rust, capture=capture)
    return capture_valid(capture, replayed, comparison, len(capture.document["scenarios"])), capture, comparison


def ratio(numerator, denominator):
    return numerator / denominator if denominator else 0.0


def checkpoint_metrics(metrics, comparison=None):
    """A conjunction of measured checkpoint exits; missing inputs stay false.

    X7 also requires the separate P2/P3 final-source gates in the sprint. Those
    expensive producers are never run implicitly here.
    """
    out = {f"x{i}_complete": False for i in range(1, 8)}
    prerequisites = all(metrics.get(n) is True for n in (
        "inventory_frozen", "inventory_verified", "harness_valid", "result_recorded", "blockers_named", "audit"))
    if not prerequisites or comparison is None:
        return out
    rows = comparison["rows"]
    def matched(selected):
        return bool(selected) and all(r["category"] == "match" or
            (r["category"] == "different" and r.get("approved_difference")) for r in selected)
    families = {family: [r for r in rows if r["family"] == family] for family in phase4_corpus.FAMILIES}
    compiler = matched(families["tsc"])
    # The tsc family includes X1 and X2; requiring the whole family prevents a
    # filename-based partition from silently dropping incremental cases.
    out["x1_complete"] = compiler and metrics.get("x1_contracts") is True
    out["x2_complete"] = (compiler and metrics.get("x2_contracts") is True
                           and metrics.get("buildinfo_codec") == 1 and metrics.get("incremental_correctness") == 1)
    out["x3_complete"] = matched(families["tsbuild"]) and metrics.get("x3_contracts") is True
    out["x4_complete"] = metrics.get("x4_contracts") is True and metrics.get("watcher_tests") is True
    out["x5_complete"] = (matched(families["tscWatch"]) and matched(families["tsbuildWatch"])
                           and metrics.get("x5_contracts") is True and metrics.get("incremental_correctness") == 1)
    out["x6_complete"] = (matched([r for r in rows if r["id"].startswith("tsc/generateTrace/")])
                           and metrics.get("x6_contracts") is True)
    out["x7_complete"] = (all(out[f"x{i}_complete"] for i in range(1, 7))
                           and metrics.get("unit_rosters") == 1 and metrics.get("residuals") == 0
                           and all(metrics.get(n) is True for n in ("smoke", "buildinfo_interop", "live_watch_parity",
                                                                   "determinism", "thread_sanitizer")))
    return out


def tsc(rust=None, *, verify=verification, witnesses=None, contracts=None):
    rust = Path(rust or RUST)
    metrics = {"inventory_frozen": False, "inventory_verified": False,
               **dict.fromkeys(VERIFY_METRICS.values(), False),
               "harness_valid": False, "result_recorded": False, "blockers_named": False,
               "unit_tests": False, "audit": False, **checkpoint_metrics({})}
    witnesses = quietly(phase4_acceptance.collect, witnesses)
    metrics.update(witnesses["metrics"])
    # xtask's evidence record retains and hashes stderr too. Keep exact capture
    # identities here, not in numeric/boolean metrics or an uncommitted sidecar.
    print("tsc witnesses: " + canonical(witnesses).decode(), file=sys.stderr)
    try:
        receipt = phase4_contracts.replay(contracts or phase4_contracts.DEFAULT_OUTPUT,
                                           expected_host=phase4_acceptance.current_host())
        metrics.update(receipt["metrics"])
        print("tsc contracts: " + canonical(receipt).decode(), file=sys.stderr)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("contract receipt unavailable: " + str(error), file=sys.stderr)
    for name, module in (("unit_tests", phase4_unit_tests), ("audit", phase4_audit)):
        try:
            problems = module.check()
            metrics[name] = not problems
            for problem in problems:
                print(f"phase4 {name}: {problem}", file=sys.stderr)
        except (OSError, ValueError, KeyError) as error:
            print(f"phase4 {name} unavailable: " + str(error), file=sys.stderr)
    try:
        phase4_corpus.read_document()
        metrics["inventory_frozen"] = True
    except (OSError, ValueError, KeyError) as error:
        print("inventory unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    try:
        checks = verify()
        for check, name in VERIFY_METRICS.items():
            metrics[name] = checks.get(check) is True
        metrics["inventory_verified"] = all(metrics[name] for name in VERIFY_METRICS.values())
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print("inventory verification unavailable: " + str(error), file=sys.stderr)
    try:
        valid, capture, comparison = harness(rust)
        metrics["harness_valid"] = valid
    except (OSError, ValueError, KeyError) as error:
        print(f"Rust capture at {rust} unavailable: " + str(error), file=sys.stderr)
        valid = False
    if not valid:
        print("the Rust capture is missing, partial, stale or invalid; acceptance metrics withheld", file=sys.stderr)
        return {"metrics": metrics}
    summary = comparison["summary"]
    try:
        recorded = (strict_json_loads(phase4_compare.RECORD.read_bytes())
                    if phase4_compare.RECORD.exists() else None)
        metrics["result_recorded"] = recorded == phase4_compare.acceptance_summary(comparison)
    except (OSError, ValueError) as error:
        print("recorded comparison unavailable: " + str(error), file=sys.stderr)
    metrics["unsupported_required"] = summary["unsupported_rows"]
    metrics["baseline_parity"] = ratio(summary["matched"], summary["rows"])
    metrics["baseline_accepted"] = ratio(summary["accepted"], summary["rows"])
    metrics["incremental_correctness"] = ratio(summary["edit_steps_agreeing"], summary["edit_steps"])
    try:
        register = phase4_blockers.build(rust, comparison=comparison, capture=capture)
        committed = (strict_json_loads(phase4_blockers.REGISTER.read_bytes())
                     if phase4_blockers.REGISTER.exists() else None)
        metrics["blockers_named"] = register == committed and phase4_blockers.complete(register, comparison)
        metrics["residuals"] = sum(entry["scenarios"] for entry in register["entries"])
    except (OSError, ValueError, KeyError) as error:
        print("blocker register unavailable: " + str(error), file=sys.stderr)
    print("tsc evidence: " + canonical({"inventory": capture.metadata["inventory"]["sha256"],
                                        "rust": capture.sha256,
                                        "references": comparison["references"]["git_blobs_sha256"]}).decode(),
          file=sys.stderr)
    metrics.update(checkpoint_metrics(metrics, comparison))
    return {"metrics": metrics}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("tsc",))
    parser.add_argument("--rust", type=Path, default=RUST)
    parser.add_argument("--witnesses", type=Path, default=phase4_acceptance.DEFAULT_INDEX)
    parser.add_argument("--contracts", type=Path, default=phase4_contracts.DEFAULT_OUTPUT)
    args = parser.parse_args()
    print(json.dumps(tsc(args.rust, witnesses=args.witnesses, contracts=args.contracts), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
