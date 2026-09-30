#!/usr/bin/env python3
"""Phase 2 C6.0: the native checker contract in upstream's concurrent mode.

The C0 capture (`phase2_native.py`) runs upstream's default single-threaded
test programs. This capture runs the same oracle, with every request, the same
sharding and the same validation, in the mode `TS_TEST_PROGRAM_SINGLE_THREADED=false`
selects: each program checks with the compiler checker pool (four checkers
unless a test sets `checkers`, clamped to the file count). Only that variable
differs, and every shard's summary must report the concurrent mode.

It lives beside `phase2_native.py` rather than in it: that script is an input
of the recorded single-threaded capture, so editing it would make that
capture stale.

    capture --output DIR [--oracle-from DIR] [--shards N] [--scheme contiguous|interleaved] [--jobs J]
    verify  --capture DIR              # second sharding, identical row digests
    review  --capture DIR [--record]   # native outputs equal the committed references

`--oracle-from` reuses a C0 capture's oracle binary (its digest is checked), so
the two captures come from one binary. `--record` writes
data/phase2/native-provenance-concurrent.json.
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase2_native as native  # noqa: E402
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_inventory  # noqa: E402

PROVENANCE = ROOT / "data/phase2/native-provenance-concurrent.json"
SCRIPT_INPUTS = (*native.SCRIPT_INPUTS, "scripts/phase2_native_concurrent.py")
MODE = "concurrent"
CHECKERS = ("4 unless the test sets the internal `checkers` option, clamped to at least 1 and at most "
            "the smaller of the program's file count and 256 (compiler/checkerpool.go:newCheckerPool)")


def input_digests():
    return {name: digest((ROOT / name).read_bytes()) for name in SCRIPT_INPUTS}


def run_shard(binary, directory, requests, env, upstream, timeout_minutes):
    directory.mkdir(parents=True)
    raw = canonical(requests) + b"\n"
    (directory / "requests.json").write_bytes(raw)
    shard_env = dict(env, PHASE2_REQUESTS=str(directory / "requests.json"),
                     PHASE2_OUTPUT=str(directory / "observations.ndjson"),
                     PHASE2_SUMMARY=str(directory / "go-summary.json"),
                     TS_TEST_PROGRAM_SINGLE_THREADED="false")
    command = [str(binary), "-test.run", f"^{native.TEST}$", "-test.count=1", f"-test.timeout={timeout_minutes}m"]
    with (directory / "go.stdout").open("wb") as out, (directory / "go.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc/internal/testrunner", env=shard_env,
                                   stdout=out, stderr=err, check=False)
    if not (directory / "go-summary.json").exists():
        raise ValueError(f"native shard did not complete (exit {completed.returncode}); see {directory}")
    summary = strict_json_loads((directory / "go-summary.json").read_bytes())
    if summary["request_sha256"] != digest(raw) or summary["rows"] != len(requests):
        raise ValueError("native shard observed a different request inventory: " + str(directory))
    if summary.get("single_threaded") is not False:
        raise ValueError("native shard did not run upstream's concurrent test programs")
    observed = [strict_json_loads(line) for line in (directory / "observations.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in observed] != [row["id"] for row in requests]:
        raise ValueError("native shard rows are missing, extra or reordered: " + str(directory))
    return completed.returncode, summary, observed


def oracle_binary(output, upstream, env, oracle_from):
    """Build the oracle, or reuse a capture's oracle binary after checking it
    against its recorded digest and the current overlay."""
    if oracle_from is None:
        return native.build_oracle(output / "oracle", upstream, env), output / "oracle/oracle.test"
    source = Path(oracle_from).resolve()
    oracle = strict_json_loads((source / "report.json").read_bytes())["oracle"]
    binary = source / "oracle/oracle.test"
    if digest(binary.read_bytes()) != oracle["binary_sha256"]:
        raise ValueError("the reused oracle binary changed")
    overlay = {name: digest(text.encode()) for name, text in sorted(native.overlay_sources(upstream).items())}
    if oracle["overlay_sha256"] != overlay:
        raise ValueError("the reused oracle was built from another overlay")
    # The capture keeps its own copy, so its verification reruns this binary.
    copy = output / "oracle/oracle.test"
    if copy != binary:
        copy.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, copy)
    return oracle, copy


def capture(output, shards, scheme, jobs, timeout_minutes, *, oracle_from=None, limit=None):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    inputs = input_digests()
    document, subset, pairs, request_rows = native.requests()
    if limit is not None:
        pairs, request_rows = pairs[:limit], request_rows[:limit]
    raw = canonical(request_rows) + b"\n"
    (output / "requests.json").write_bytes(raw)
    oracle, binary = oracle_binary(output, upstream, env, oracle_from)
    groups = native.shard_indices(len(request_rows), shards, scheme)
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(run_shard, binary, output / "shards" / f"{number:03d}",
                               [request_rows[i] for i in group], env, upstream, timeout_minutes)
                   for number, group in enumerate(groups)]
        results = [future.result() for future in futures]
    observed = [None] * len(request_rows)
    summaries = []
    for group, (code, summary, rows) in zip(groups, results, strict=True):
        if code not in (0, 1) or (code == 1 and not any(r["state"] == "upstream_failed" for r in rows)):
            raise ValueError("unexplained native test failure")
        summaries.append(summary)
        for index, row in zip(group, rows, strict=True):
            observed[index] = row
    with (output / "observations.ndjson").open("wb") as stream:
        for row in observed:
            stream.write(canonical(row) + b"\n")
    mismatches = native.validate(pairs, request_rows, observed, subset["file_observations"])
    verified_upstream()
    if input_digests() != inputs:
        raise ValueError("capture inputs changed during the native run")
    hosts = {(s["go"], s["goos"], s["goarch"]) for s in summaries}
    if len(hosts) != 1:
        raise ValueError("native shards ran on different toolchains")
    go, goos, goarch = hosts.pop()
    report = {
        "version": 1, "pin": document["pin"], "inputs": inputs, "oracle": oracle,
        "inventory_sha256": digest(phase2_inventory.INVENTORY.read_bytes()),
        "requests": len(request_rows), "request_sha256": digest(raw), "partial": limit is not None,
        "observation_sha256": digest((output / "observations.ndjson").read_bytes()),
        "row_sha256": [native.contract_digest(row) for row in observed],
        "row_digest": "sha256 of the canonical row without numeric walker type_id values",
        "sharding": {"shards": len(groups), "scheme": scheme, "jobs": jobs},
        "states": dict(Counter(row["state"] for row in observed)),
        "failure_stages": dict(Counter(row["failure"]["stage"] for row in observed if row["state"] == "upstream_failed")),
        "mismatches": mismatches,
        "go": go, "goos": goos, "goarch": goarch, "host": platform.platform(),
        "single_threaded": False, "mode": MODE, "checkers": CHECKERS,
        "scope": ("Pinned native checker observations for the Phase 2 denominator in upstream's concurrent "
                  "test-program mode (TS_TEST_PROGRAM_SINGLE_THREADED=false); no Rust comparison"),
    }
    (output / "report.json").write_bytes(canonical(report) + b"\n")
    print(json.dumps({k: report[k] for k in ("requests", "states", "failure_stages", "sharding")}, sort_keys=True))
    return report


def load_capture(directory, *, partial=False):
    directory, report, observed = native.load_capture(directory, partial=partial)
    if report.get("mode") != MODE or report["single_threaded"] is not False:
        raise ValueError("not a concurrent-mode native capture: " + str(directory))
    return directory, report, observed


def current(report):
    if report["inputs"] != input_digests():
        stale = sorted(name for name, value in report["inputs"].items() if input_digests().get(name) != value)
        raise ValueError("concurrent native capture is stale; changed inputs: " + ", ".join(stale))
    if report["oracle"]["overlay_sha256"] != {name: digest(text.encode()) for name, text
                                              in sorted(native.overlay_sources(verified_upstream()).items())}:
        raise ValueError("native oracle overlay changed since the capture")


def verify(directory, shards, scheme, jobs, timeout_minutes, *, partial=False):
    directory, report, observed = load_capture(directory, partial=partial)
    current(report)
    other = directory / f"verify-{scheme}-{shards}"
    if other.exists():
        shutil.rmtree(other)
    if (shards, scheme) == (report["sharding"]["shards"], report["sharding"]["scheme"]):
        raise ValueError("verification needs a different sharding than the capture")
    second = capture(other, shards, scheme, jobs, timeout_minutes, oracle_from=directory,
                     limit=report["requests"] if report["partial"] else None)
    differing = [row["id"] for row, left, right in zip(observed, report["row_sha256"], second["row_sha256"], strict=True)
                 if left != right]
    if differing:
        raise ValueError(f"second sharding differs on {len(differing)} rows, first {differing[:5]}")
    raw_only = sum(left != right for left, right in zip(
        (directory / "observations.ndjson").read_bytes().splitlines(),
        (other / "observations.ndjson").read_bytes().splitlines(), strict=True))
    result = {"verified": True, "rows": len(observed), "capture_sharding": report["sharding"],
              "verify_sharding": second["sharding"], "observation_sha256": report["observation_sha256"],
              "rows_differing_only_in_type_ids": raw_only}
    (directory / "verified.json").write_bytes(canonical(result) + b"\n")
    print(json.dumps(result, sort_keys=True))
    return result


def review(directory, record):
    directory, report, observed = load_capture(directory)
    current(report)
    verified = directory / "verified.json"
    if not verified.exists() or strict_json_loads(verified.read_bytes())["observation_sha256"] != report["observation_sha256"]:
        raise ValueError("review requires a verified capture (run verify first)")
    document, _subset, _pairs, request_rows = native.requests()
    if digest(canonical(request_rows) + b"\n") != report["request_sha256"]:
        raise ValueError("inventory requests changed since the capture")
    disagreements, pre_post = [], []
    references = Counter()
    for row, result in zip(phase2_inventory.executed(document), observed, strict=True):
        if result["state"] != "executed":
            continue
        for kind, key in ((".errors.txt", "errors"), (".types", "types"), (".symbols", "symbols"), (".trace.json", "trace")):
            expected = native.reference(row, kind)
            actual = result[key]
            if actual["state"] in ("disabled", "no_content"):
                ok = expected is None
            else:
                ok = expected is not None and bytes.fromhex(actual["text_hex"]) == expected
            references[(kind, actual["state"], ok)] += 1
            if not ok:
                disagreements.append({"id": row["id"], "kind": kind, "native_state": actual["state"],
                                      "reference_present": expected is not None})
        if result["error_pre_diagnostics"] != result["error_post_diagnostics"]:
            pre_post.append(row["id"])
    executed_rows = [r for r in observed if r["state"] == "executed"]
    summary = {
        "version": 1, "pin": report["pin"], "capture_report_sha256": digest((directory / "report.json").read_bytes()),
        "request_sha256": report["request_sha256"], "observation_sha256": report["observation_sha256"],
        "inventory_sha256": report["inventory_sha256"], "inputs": report["inputs"], "oracle": report["oracle"],
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"], "host": report["host"],
        "single_threaded": False, "mode": MODE, "checkers": report["checkers"],
        "requests": report["requests"], "states": report["states"], "failure_stages": report["failure_stages"],
        "input_mismatches": report["mismatches"],
        "sharding": {"capture": report["sharding"], "verify": strict_json_loads(verified.read_bytes())["verify_sharding"]},
        "reference_outcomes": [{"kind": k, "native_state": s, "agrees": ok, "variants": n}
                               for (k, s, ok), n in sorted(references.items())],
        "reference_disagreements": disagreements,
        "pre_post_different": pre_post,
        "declaration_requests": sum(r["emit_declarations"] for r in executed_rows),
        "trace_rows": sum(r["trace"]["state"] != "disabled" for r in executed_rows),
        "union_ordering_inconsistent": [r["id"] for r in executed_rows if r["union_ordering"]["inconsistent"]],
        "parent_pointer_failures": [r["id"] for r in executed_rows if r["parent_pointers"]["failure"] is not None],
        "row_sha256": report["row_sha256"],
    }
    (directory / "review.json").write_bytes(canonical(summary) + b"\n")
    if record:
        PROVENANCE.parent.mkdir(parents=True, exist_ok=True)
        PROVENANCE.write_bytes(canonical(summary) + b"\n")
    brief = {k: summary[k] for k in ("requests", "states", "declaration_requests", "trace_rows")}
    brief.update(reference_disagreements=len(disagreements), pre_post_different=len(pre_post),
                 union_ordering_inconsistent=len(summary["union_ordering_inconsistent"]),
                 parent_pointer_failures=len(summary["parent_pointer_failures"]),
                 input_mismatches=len(report["mismatches"]))
    print(json.dumps(brief, sort_keys=True))
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("capture", "verify"):
        sub = commands.add_parser(name)
        sub.add_argument("--shards", type=int, default=10 if name == "capture" else 7)
        sub.add_argument("--scheme", choices=("contiguous", "interleaved"),
                         default="contiguous" if name == "capture" else "interleaved")
        sub.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
        sub.add_argument("--timeout-minutes", type=int, default=240)
        if name == "capture":
            sub.add_argument("--output", type=Path, required=True)
            sub.add_argument("--oracle-from", type=Path, help="reuse this capture's oracle binary")
            sub.add_argument("--limit", type=int, help="development smoke over the first N rows; never recorded")
        else:
            sub.add_argument("--capture", type=Path, required=True)
            sub.add_argument("--smoke", action="store_true", help="verify a --limit capture's sharding only")
    sub = commands.add_parser("review")
    sub.add_argument("--capture", type=Path, required=True)
    sub.add_argument("--record", action="store_true")
    args = parser.parse_args()
    if args.command == "capture":
        capture(args.output, args.shards, args.scheme, args.jobs, args.timeout_minutes,
                oracle_from=args.oracle_from, limit=args.limit)
    elif args.command == "verify":
        verify(args.capture, args.shards, args.scheme, args.jobs, args.timeout_minutes, partial=args.smoke)
    else:
        review(args.capture, args.record)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("phase2 native (concurrent) failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
