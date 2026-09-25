#!/usr/bin/env python3
"""Phase 2 C0.3: run the Rust corpus over every executed inventory variant.

Each row is the S08 P5 protocol (one process per variant, deadline, raw
stdout/stderr/observation, atomic immutable completion record, resume and
replay against the captured executable) running `phase2_checker`, which adds
the three runner sub-tests after the baseline walk. Requests carry only native
*inputs* (loading request, ordered error/walker inputs, the observed
declaration request), never expected outputs.

Harness health is separate from compiler outcomes. A malformed response, a
protocol violation or a panic located in the adapter is a harness error: it
invalidates the run instead of becoming a compiler failure. A production panic,
a deadline or a named refusal (`Error::Unsupported`) is a measured gap.

    run    --native DIR --output DIR [--jobs N] [--timeout S] [--resume] [--limit N]
    replay --output DIR        # recompute categories from the raw outputs
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
import math
import os
from pathlib import Path
import shutil
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
import s08_p4 as p4  # noqa: E402
import s08_p5_corpus as p5  # noqa: E402
from s08_oracle import ROOT, digest  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402

EXAMPLE = "phase2_checker"
SOURCE_OBSERVATIONS = ROOT / "target/s07-subset/source-observations.ndjson"
PHASES = ("config", "program", "syntactic", "semantic", "global")
SUBTESTS = ("trace", "union_ordering", "parent_pointers")
# What decides a Rust row: the production crates and the S08 adapters
# (p4.sources), the P5 adapter and the Phase 2 sub-tests. Requests and the
# native capture are bound by digest in capture.json, and validation reruns
# with the current code at every replay, so scripts cannot stale a capture.
SOURCE_PATTERNS = ("tools/phase2/**/*.rs", "tools/s08/p5/**/*")


def sources():
    result = p4.sources()
    for pattern in SOURCE_PATTERNS:
        for path in ROOT.glob(pattern):
            if path.is_file():
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return dict(sorted(result.items()))


def loading_requests():
    """Loading requests for every variant, rebuilt from the native preprocessing
    S07 froze; each hashes to the inventory's loading_request_sha256."""
    import phase1_syntax_schedule as schedule

    _rows, probes = schedule.schedule_requests(SOURCE_OBSERVATIONS)
    return {probe["id"]: probe["request"] for probe in probes}


def requests(native_dir, limit=None):
    directory, report, observed = phase2_native.load_capture(native_dir)
    phase2_native.current(report)
    if not (directory / "verified.json").exists():
        raise ValueError("the Rust run requires a verified native capture")
    document = phase2_inventory.read()
    rows = phase2_inventory.executed(document)
    if [row["id"] for row in rows] != [row["id"] for row in observed]:
        raise ValueError("native capture does not follow the executed inventory")
    loading = loading_requests()
    result = []
    for row, native in zip(rows, observed, strict=True):
        request = loading[row["id"]]
        if digest(p4.canonical(request) + b"\n") != row["loading_request_sha256"]:
            raise ValueError("loading request differs from the inventory: " + row["id"])
        types = not row["harness"]["NoTypesAndSymbols"]
        phases = list(PHASES)
        if native["state"] == "executed" and native["emit_declarations"]:
            phases.append("declaration")
        if row["harness"]["CaptureSuggestions"]:
            phases.append("suggestion")
        entry = {"id": row["id"], "acceptance_tier": "executed", "diagnostic_phases": phases,
                 "type_baseline_requested": types, "loading": request, "error_baseline_requested": True,
                 "public_type_strings": True}
        if native["state"] == "executed":
            entry["error_inputs"] = native["error_inputs"]
            if types:
                entry["baseline_inputs"] = native["baseline_inputs"]
                entry["baseline_header"] = native["baseline_header"]
        result.append(entry)
    if limit is not None:
        result, observed = result[:limit], observed[:limit]
    return report, result, observed


def validate_row(request, row):
    """The S08 P5 row contract plus the sub-test observations."""
    if not isinstance(row, dict):
        raise ValueError("observation is not an object")
    extra = {key: row[key] for key in ("phase2", "panic_location") if key in row}
    base = {key: value for key, value in row.items() if key not in extra}
    p5.validate_row(request, base)
    if "fatal" in base:
        # Only the example's own panic handler records a location; the runner's
        # deadline, exit and protocol rows (s08_p4.fatal) never do.
        if ("panic_location" in extra) != (base["fatal"]["class"] == "panic"):
            raise ValueError("fatal observation has a missing or spurious panic location")
        return row
    if "panic_location" in extra:
        raise ValueError("completed observation carries a panic location")
    subtests = extra.get("phase2")
    if not isinstance(subtests, dict):
        raise ValueError("sub-test observations missing")
    if subtests == {"state": "not_reached"}:
        if base["load"]["state"] == "executed":
            raise ValueError("sub-tests silently skipped after a loaded program")
        return row
    if set(subtests) != set(SUBTESTS):
        raise ValueError("missing or extra sub-test observation")
    for name, value in subtests.items():
        state = value.get("state")
        if state == "failed":
            if not isinstance(value.get("class"), str) or not isinstance(value.get("reason"), str):
                raise ValueError("sub-test failure lacks class and reason: " + name)
        elif name == "trace":
            if state not in ("content", "no_content", "disabled"):
                raise ValueError("unknown trace outcome")
            if state == "content":
                p5.hex_bytes(value["text_hex"])
        elif state != "executed":
            raise ValueError("unknown sub-test outcome: " + name)
    enabled = request["loading"]["options"].get("traceResolution") is True
    if (subtests["trace"]["state"] == "disabled") != (not enabled):
        raise ValueError("trace enablement differs from traceResolution")
    return row


def adapter_panic(row):
    location = row.get("panic_location") or ""
    return location.startswith(("tools/", "crates/tsr_compiler/examples/")) or "/tools/" in location


STACK_OVERFLOW = b"has overflowed its stack"
ADAPTER_SOURCES = ("tools/s08/p5", "tools/s08/p4", "tools/phase2", "crates/tsr_compiler/examples/phase2_checker.rs")
_ADAPTER_TEXT = None


def adapter_literal(reason):
    """Whether an error text is a string literal of the adapter (test code), as
    opposed to a production error's Display text."""
    global _ADAPTER_TEXT
    if _ADAPTER_TEXT is None:
        paths = [p for name in ADAPTER_SOURCES for p in ([ROOT / name] if name.endswith(".rs") else
                                                          (ROOT / name).rglob("*.rs"))]
        _ADAPTER_TEXT = "\n".join(path.read_text() for path in paths)
    return bool(reason) and f'"{reason}"' in _ADAPTER_TEXT


def completed_problems(row):
    """Harness defects hidden inside a completed row."""
    problems = []
    walk = row["type_symbol_baselines"]
    if walk.get("state") == "failed" and walk.get("class") == "walker_error" and adapter_literal(walk.get("reason")):
        problems.append("adapter walker error: " + walk["reason"])
    errors = row.get("error_baseline", {})
    if errors.get("state") == "failed" and errors.get("class") == "error_baseline" and adapter_literal(errors.get("reason")):
        problems.append("adapter error rendering: " + errors["reason"])
    for name, value in (row.get("phase2") or {}).items():
        if isinstance(value, dict) and value.get("state") == "failed" and value.get("class") == "panic":
            location = value.get("location") or ""
            if not location.startswith("crates/") or "/examples/" in location:
                problems.append(f"sub-test {name} panic at {location or 'unknown location'}")
    return problems


def attribute(row, stderr):
    """Who owns a fatal row: `production` gaps are measured; anything else is a
    harness defect that invalidates preparation until it is attributed."""
    fatal = row["fatal"]
    if fatal["class"] == "harness_protocol":
        return "harness", "protocol: " + fatal["reason"]
    if fatal["class"] == "panic":
        if adapter_panic(row):
            return "harness", "adapter panic at " + row["panic_location"]
        if (row.get("panic_location") or "").startswith("crates/"):
            return "production", "panic at " + row["panic_location"]
        return "harness", "panic without a production location"
    if fatal["class"] == "timeout":
        return "production", "deadline"
    if fatal["class"] == "process_exit" and STACK_OVERFLOW in stderr:
        return "production", "stack overflow"
    return "harness", f"unattributed {fatal['class']}: {fatal['reason']}"


def run(native_dir, output, jobs, timeout, resume=False, limit=None):
    output = Path(output).resolve()
    report, request_rows, _ = requests(native_dir, limit)
    native_meta = {"directory": str(Path(native_dir).resolve()), "report_sha256":
                   digest((Path(native_dir) / "report.json").read_bytes()),
                   "observation_sha256": report["observation_sha256"]}
    if output.exists():
        if not resume:
            raise ValueError("existing capture requires --resume")
        metadata = p4.read(output / "capture.json")
        if (metadata["timeout_seconds"] != timeout or metadata["native"] != native_meta
                or metadata["requests_sha256"] != digest(p4.canonical(request_rows) + b"\n")):
            raise ValueError("resume requires the identical native capture, requests and timeout")
    else:
        output.mkdir(parents=True)
        (output / "cases").mkdir()
        record = p4.build(output / "build", example=EXAMPLE, source_fn=sources, optimize=True)
        Path(record["source_snapshot"]).rename(output / "source-snapshot")
        record["source_snapshot"] = str(output / "source-snapshot")
        p4.atomic(output / "build/build.json", record)
        metadata = {"version": 1, "requests_sha256": digest(p4.canonical(request_rows) + b"\n"), "build": record,
                    "timeout_seconds": timeout, "native": native_meta, "partial": limit is not None,
                    "inventory_sha256": digest(phase2_inventory.INVENTORY.read_bytes())}
        shutil.copy2(record["binary"], output / "executable")
        p4.write_new(output / "requests.json", request_rows)
        p4.write_new(output / "capture.json", metadata)
    binary = output / "executable"
    if digest(binary.read_bytes()) != metadata["build"]["binary_sha256"]:
        raise ValueError("captured executable changed")
    pending = [index for index in range(len(request_rows))
               if not (output / "cases" / f"{index:05d}" / "result.json").exists()]
    started = time.monotonic()
    done = 0

    def execute(index):
        request = request_rows[index]
        case_dir = output / "cases" / f"{index:05d}"
        row = p4.execute_case(binary, case_dir, request, timeout, validator=validate_row)
        p4.complete_case(case_dir, request, metadata, row)

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        for _ in pool.map(execute, pending):
            done += 1
            if done % 500 == 0 or done == len(pending):
                print(f"phase2 corpus {done}/{len(pending)} in {time.monotonic() - started:.0f}s",
                      file=sys.stderr, flush=True)
    elapsed = time.monotonic() - started
    timing = p4.read(output / "timing.json") if (output / "timing.json").exists() else {"segments": []}
    timing["segments"].append({"rows": len(pending), "jobs": jobs, "seconds": round(elapsed, 3)})
    p4.atomic(output / "timing.json", timing)
    result = replay(output)
    print(json.dumps(result["summary"], sort_keys=True))
    return result


def replay(output):
    output = Path(output).resolve()
    metadata = p4.read(output / "capture.json")
    p4.verify_source_snapshot(output / "source-snapshot", metadata["build"]["sources"])
    raw = (output / "requests.json").read_bytes()
    if digest(raw) != metadata["requests_sha256"]:
        raise ValueError("capture request inventory changed")
    request_rows = strict_json_loads(raw)
    if len({row["id"] for row in request_rows}) != len(request_rows):
        raise ValueError("duplicate request identity")
    if digest((output / "executable").read_bytes()) != metadata["build"]["binary_sha256"]:
        raise ValueError("captured executable changed")
    case_names = {entry.name for entry in (output / "cases").iterdir() if entry.name.isdigit()}
    if case_names - {f"{i:05d}" for i in range(len(request_rows))}:
        raise ValueError("completion record outside the request inventory")
    rows, harness, attributed = [], [], []
    for index, request in enumerate(request_rows):
        path = output / "cases" / f"{index:05d}" / "result.json"
        if not path.exists():
            raise ValueError("capture incomplete at " + request["id"])
        envelope = p4.read(path)
        if (envelope.get("request_sha256") != digest(p4.canonical(request) + b"\n")
                or envelope.get("capture_sha256") != digest(p4.canonical(metadata) + b"\n")):
            raise ValueError("completion belongs to a different request or capture: " + request["id"])
        for name, expected in envelope["artifacts"].items():
            if name not in ("stdout", "stderr", "observation.json") or digest((path.parent / name).read_bytes()) != expected:
                raise ValueError("raw case artifact changed: " + request["id"])
        row = envelope["row"]
        if ("observation.json" in envelope["artifacts"] and "fatal" not in row
                and p4.read(path.parent / "observation.json") != row):
            raise ValueError("completion differs from its raw observation: " + request["id"])
        if "fatal" not in row or row["fatal"]["class"] != "harness_protocol":
            validate_row(request, row)
        if "fatal" in row:
            owner, reason = attribute(row, (path.parent / "stderr").read_bytes())
            if owner == "harness":
                harness.append({"id": request["id"], "problem": reason})
            else:
                attributed.append({"id": request["id"], "class": row["fatal"]["class"], "attribution": reason})
        else:
            harness.extend({"id": request["id"], "problem": problem} for problem in completed_problems(row))
        rows.append(row)
    states = Counter("fatal:" + row["fatal"]["class"] if "fatal" in row else "completed" for row in rows)
    summary = {"requested": len(request_rows), "observed": len(rows), "states": dict(sorted(states.items())),
               "harness_errors": len(harness), "partial": metadata.get("partial", False)}
    result = {"version": 1, "summary": summary, "harness_errors": harness, "production_failures": attributed,
              "capture_sha256": digest(p4.canonical(metadata) + b"\n"),
              "source_stable": sources() == metadata["build"]["sources"]}
    p4.atomic(output / "replayed.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("run")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    sub.add_argument("--timeout", type=float, default=60)
    sub.add_argument("--resume", action="store_true")
    sub.add_argument("--limit", type=int, help="development smoke over the first N rows; never recorded")
    sub = commands.add_parser("replay")
    sub.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "run":
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            parser.error("--timeout must be positive and finite")
        run(args.native, args.output, args.jobs, args.timeout, args.resume, args.limit)
    else:
        print(json.dumps(replay(args.output)["summary"], sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 corpus failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
