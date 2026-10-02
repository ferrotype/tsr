#!/usr/bin/env python3
"""Phase 3 T8: one bounded emit timing capture, recorded beside Go's, with no
threshold (docs/PHASE3-plan.md decision 10; Phase 7 owns the budgets).

The bounded set is the inventory's 300-variant sample. Go's side is a
single-shard, one-job native capture of those variants with an already built
oracle (`--oracle-from`, a native capture directory, so the build is not
timed); Rust's side is the corpus harness over the same variants with one job,
against that capture. Both run the pinned runner's whole emit sub-test per
variant (load, check, emit, the baselines), each in a process of its own, so
the numbers include the harnesses' per-variant process cost and are a
disclosure, not a benchmark. Run it on a quiet host.

    capture --output DIR --oracle-from DIR   # writes DIR/timing.json
    record  DIR                              # writes data/phase3/timing.json
"""
from __future__ import annotations

import argparse
import json
import os
import platform
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase3_corpus  # noqa: E402
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402

RECORD = ROOT / "data/phase3/timing.json"
TIMEOUT_MINUTES = 120
ROW_TIMEOUT_SECONDS = 300


def sample():
    return [row["id"] for row in phase3_inventory.read()["rows"] if row["sample"]]


def host():
    return {"os": sys.platform, "architecture": platform.machine(), "release": platform.release(),
            "cpus": os.cpu_count(), "load_average": os.getloadavg()}


def capture(output, oracle_from):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    cases = sample()
    started = time.monotonic()
    phase3_native.capture(output / "native", "single", 1, "contiguous", 1, TIMEOUT_MINUTES,
                          oracle_from=oracle_from, cases=cases)
    go_seconds = time.monotonic() - started
    started = time.monotonic()
    phase3_corpus.run(output / "native", output / "rust", 1, ROW_TIMEOUT_SECONDS, cases=cases, mode="single")
    rust_seconds = time.monotonic() - started
    document = {
        "version": 1,
        "pin": strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"],
        "sources_sha256": digest(canonical(phase3_corpus.sources())),
        "host": host(),
        "rows": len(cases),
        "go_seconds": round(go_seconds, 3),
        "rust_seconds": round(rust_seconds, 3),
        "rust_over_go": round(rust_seconds / go_seconds, 4),
        "scope": "the 300-variant sample's emit sub-tests, one job each side; harness process cost included",
    }
    (output / "timing.json").write_text(json.dumps(document, indent=1, sort_keys=True) + "\n")
    print(json.dumps(document, sort_keys=True))
    return document


def record(directory):
    document = strict_json_loads((Path(directory) / "timing.json").read_bytes())
    if document["sources_sha256"] != digest(canonical(phase3_corpus.sources())):
        raise ValueError("the timing capture was made from other sources")
    RECORD.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n")
    print(json.dumps(document, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    capture_parser = sub.add_parser("capture")
    capture_parser.add_argument("--output", type=Path, required=True)
    capture_parser.add_argument("--oracle-from", type=Path, required=True)
    sub.add_parser("record").add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "capture":
        capture(args.output, args.oracle_from)
    else:
        record(args.directory)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 timing failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
