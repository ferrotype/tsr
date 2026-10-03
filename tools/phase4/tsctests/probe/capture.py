#!/usr/bin/env python3
"""Phase 4 X0: capture the native expectations of the harness's unit tests.

    python3 tools/phase4/tsctests/probe/capture.py            # write testdata/probe.json
    python3 tools/phase4/tsctests/probe/capture.py --check    # rerun, require the same file

`harness_probe_test.go` (beside this script) is added to the pinned package
`internal/execute/tsctests` as `phase4_harness_probe_test.go` with
`go test -overlay` (it replaces no pinned file, and nothing under upstream/
is written; the overlay lives in target/phase4/harness-probe/). The test
drives the pinned harness functions over fixed inputs; this script wraps its
results with the pin, the overlay's SHA-256 and the Go version into
`tools/phase4/tsctests/testdata/probe.json`, which the Rust tests read.
Needs go on PATH (see CLAUDE.md).
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment, verified_upstream  # noqa: E402

PROBE = Path(__file__).resolve().parent / "harness_probe_test.go"
FIXTURE = ROOT / "tools/phase4/tsctests/testdata/probe.json"
SCRATCH = ROOT / "target/phase4/harness-probe"
TARGET = "tsc/internal/execute/tsctests/phase4_harness_probe_test.go"
TEST = "TestPhase4HarnessProbe"


def capture():
    upstream = verified_upstream().resolve()
    env = go_environment()
    if (upstream / TARGET).exists():
        raise ValueError("the overlay would replace a pinned file: " + TARGET)
    source = PROBE.read_bytes()
    SCRATCH.mkdir(parents=True, exist_ok=True)
    overlay_source = SCRATCH / "phase4_harness_probe_test.go"
    overlay_source.write_bytes(source)
    overlay = SCRATCH / "overlay.json"
    overlay.write_text(json.dumps({"Replace": {str(upstream / TARGET): str(overlay_source)}}))
    output = SCRATCH / "probe-output.json"
    if output.exists():
        output.unlink()
    command = ["go", "test", "-mod=readonly", "-count=1", "-overlay", str(overlay), "-run", f"^{TEST}$",
               "./internal/execute/tsctests"]
    completed = subprocess.run(command, cwd=upstream / "tsc", env=dict(env, PHASE4_PROBE_OUTPUT=str(output)),
                               capture_output=True, check=False)
    if completed.returncode or not output.exists():
        raise ValueError("the probe failed:\n" + (completed.stdout + completed.stderr).decode(errors="replace")[-6000:])
    results = json.loads(output.read_bytes())
    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    document = {"version": 1, "pin": pin, "overlay_sha256": hashlib.sha256(source).hexdigest(),
                "go": results.pop("go"), "results": results}
    return json.dumps(document, indent=1, sort_keys=True, ensure_ascii=False).encode() + b"\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    rendered = capture()
    if args.check:
        if FIXTURE.read_bytes() != rendered:
            raise ValueError("the probe fixture is stale or was edited; rerun capture and review the diff")
        print(json.dumps({"fixture": str(FIXTURE.relative_to(ROOT)), "current": True}))
        return
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_bytes(rendered)
    print(json.dumps({"fixture": str(FIXTURE.relative_to(ROOT)), "bytes": len(rendered)}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print("phase4 harness probe failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
