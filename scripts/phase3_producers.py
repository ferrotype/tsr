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
* harness_valid, result_recorded, blockers_named -- false until the Rust emit
  harness, its first recorded run and the blocker register exist (T0's
  remaining work); the parity metrics of docs/PHASE3-plan.md section 5 follow
  them and are not emitted before then.

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
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402

NATIVE = {mode: ROOT / f"target/phase3/native-{mode}" for mode in phase3_native.MODES}
TRANSPILE = phase3_native.DATA / "transpile-native.json"
METRIC = {"single": "native_verified", "concurrent": "native_verified_concurrent"}


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


def emit(native=None):
    native = native or NATIVE
    metrics = {"inventory_frozen": False, "native_verified": False, "native_verified_concurrent": False,
               "transpile_native_verified": False, "harness_valid": False, "result_recorded": False,
               "blockers_named": False}
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
    return {"metrics": metrics}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("emit",))
    parser.add_argument("--native-single", type=Path, default=NATIVE["single"])
    parser.add_argument("--native-concurrent", type=Path, default=NATIVE["concurrent"])
    args = parser.parse_args()
    print(json.dumps(emit({"single": args.native_single, "concurrent": args.native_concurrent}), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
