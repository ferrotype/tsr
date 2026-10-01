#!/usr/bin/env python3
"""Phase 3 T0: run the Rust emit harness over the executed inventory.

Each row is the S08 P5 protocol (one process per variant, deadline, raw
stdout/stderr/observation, atomic immutable completion record, resume and
replay against the captured executable) running `phase3_emit`, which loads
the variant's program as the pin's harness loads its post-emit program and
records the reprint witness (decision 7) and the emit domains. Requests carry
only native *inputs*: the loading request, the native capture's baseline
inputs and what the inventory says the `output` sub-test is (it runs, or the
pin disables it with a reason), never expected outputs.

Harness health is separate from compiler outcomes. A malformed response, a
protocol violation or a panic located in the adapter is a harness error: it
invalidates the run instead of becoming a compiler failure. A production panic,
a deadline or a named refusal (`unsupported`) is a measured gap.

    run    --native DIR --output DIR [--mode single|concurrent] [--sample] [--case ID ...] [--limit N] [--resume]
    replay --output DIR        # recompute categories from the raw outputs

The native capture is `scripts/phase3_native.py`'s, verified, of the same
mode (target/phase3/native-<mode>); captures go under target/phase3/rust-<name>.
`--limit` is a development smoke over the first N rows and is never recorded.
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
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_corpus  # noqa: E402
import phase2_inventory  # noqa: E402
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402

EXAMPLE = "phase3_emit"
MODES = ("single", "concurrent")
EMIT_DOMAINS = ("emit", "output", "sourcemap", "sourcemap_record")
ROW_FIELDS = {"version", "id", "acceptance_tier", "mode", "load", "reprint", *EMIT_DOMAINS}
PRINT_STATES = ("printed", "refused", "failed")
# What decides a Rust row: the production crates and the S08 executor
# (p4.sources), the Phase 3 harness modules and the build configuration.
# Requests and the native capture are bound by digest in capture.json.
SOURCE_PATTERNS = ("tools/phase3/harness/**/*", "rust-toolchain.toml", ".cargo/**/*", "crates/**/*",
                   "tools/**/Cargo.toml", "tools/s08/relater-prototype/**/*", "xtask/**/*", "tools/s03/**/*",
                   "scripts/generate_locale_tables.py")
ADAPTER_SOURCES = ("tools/s08/p4", "tools/phase3/harness", "crates/tsr_compiler/examples/phase3_emit.rs")
_ADAPTER_TEXT = None


def sources():
    result = p4.sources()
    for pattern in SOURCE_PATTERNS:
        for path in ROOT.glob(pattern):
            if path.is_file() and not ({"target", "__pycache__"} & set(path.relative_to(ROOT).parts)) and path.name != ".DS_Store":
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return dict(sorted((path, value) for path, value in result.items() if not phase2_corpus.test_only(path)))


def load_native(native_dir, mode):
    """The verified, complete native capture of `mode`, current with its inputs
    and taken over the inventory's requests."""
    directory, report, observed = phase3_native.load_capture(native_dir)
    phase3_native.current(report)
    if report.get("mode") != mode:
        raise ValueError(f"the {mode} mode compares with a {mode}-mode native capture")
    verified = directory / "verified.json"
    if not verified.exists():
        raise ValueError("the Rust run requires a verified native capture")
    check = strict_json_loads(verified.read_bytes())
    if not check.get("verified") or check.get("observation_sha256") != report["observation_sha256"]:
        raise ValueError("the native capture's verification is for another observation")
    return directory, report, observed


def native_binding(native_dir, report):
    return {"directory": str(Path(native_dir).resolve()),
            "report_sha256": digest((Path(native_dir) / "report.json").read_bytes()),
            "observation_sha256": report["observation_sha256"], "mode": report["mode"]}


INPUT_GROUPS = ("ts_config_files", "to_be_compiled", "other_files")


def build_request(row, native, loading, mode):
    """One Rust request: native inputs only. `error_inputs` are the runner's
    three input groups in its order, which the executor's config parse reads
    (the configuration's content mappers come from it); they equal the Phase 2
    requests' `error_inputs`."""
    inputs = native["baseline_inputs"]
    return {"id": row["id"], "acceptance_tier": "executed", "loading": loading, "mode": mode,
            "reprint": True, "emit": True, "output": row["output"], "baseline_inputs": inputs,
            "error_inputs": [item for group in INPUT_GROUPS for item in inputs[group]]}


def requests(native_dir, limit=None, *, sample=False, cases=(), mode="single"):
    """The Rust requests of the selected rows, in inventory order."""
    if mode not in MODES:
        raise ValueError("unknown mode: " + str(mode))
    _, report, observed = load_native(native_dir, mode)
    document = phase3_inventory.read()
    if report["request_sha256"] != digest(canonical(phase3_inventory.requests(document)) + b"\n"):
        raise ValueError("native capture was taken over other requests than the inventory's")
    rows = document["rows"]
    if [row["id"] for row in rows] != [row["id"] for row in observed]:
        raise ValueError("native capture does not follow the inventory")
    selected = {row["id"] for row in phase2_corpus.select_rows(rows, sample=sample, cases=cases, limit=limit)}
    loading = phase2_corpus.loading_requests()
    # The Phase 2 rows are bound to the Phase 3 inventory by their digest
    # (phase3_inventory.read), not by phase2_inventory's own input check.
    phase2_rows = json.loads(phase2_inventory.INVENTORY.read_bytes())["rows"]
    loading_digests = {row["id"]: row["loading_request_sha256"] for row in phase2_rows if row["tier"] == "executed"}
    result = []
    for row, native in zip(rows, observed, strict=True):
        if row["id"] not in selected:
            continue
        if native["state"] != "executed":
            raise ValueError("native row did not execute: " + row["id"])
        request = loading[row["id"]]
        if digest(p4.canonical(request) + b"\n") != loading_digests[row["id"]]:
            raise ValueError("loading request differs from the inventory: " + row["id"])
        result.append(build_request(row, native, request, mode))
    return report, result


def _failure(value, name):
    if (not isinstance(value, dict) or set(value) != {"state", "class", "reason"} or value["state"] != "failed"
            or not isinstance(value["class"], str) or not isinstance(value["reason"], str) or not value["reason"]):
        raise ValueError("malformed failure: " + name)


def _digest_text(value, name):
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        raise ValueError("malformed digest: " + name)


def validate_print(value):
    if not isinstance(value, dict):
        raise ValueError("reprint result is not an object")
    state = value.get("state")
    if state == "printed":
        if set(value) != {"state", "sha256", "bytes"} or type(value["bytes"]) is not int or value["bytes"] < 0:
            raise ValueError("malformed printed reprint")
        _digest_text(value["sha256"], "reprint")
    elif state == "refused":
        if set(value) != {"state", "reason"} or not isinstance(value["reason"], str) or not value["reason"]:
            raise ValueError("malformed refused reprint")
    elif state == "failed":
        if (set(value) != {"state", "class", "reason", "location"} or value["class"] != "panic"
                or not isinstance(value["reason"], str)
                or value["location"] is not None and not isinstance(value["location"], str)):
            raise ValueError("malformed failed reprint")
    else:
        raise ValueError("unknown reprint state")


def validate_reprint(request, row):
    value = row["reprint"]
    if not isinstance(value, dict):
        raise ValueError("reprint observation is not an object")
    if request.get("reprint") is not True:
        if value != {"state": "not_requested"}:
            raise ValueError("unrequested reprint was executed")
        return
    if row["load"]["state"] != "executed":
        if value != {"state": "not_reached", "reason": "the program did not load"}:
            raise ValueError("reprint observed without a loaded program")
        return
    if value.get("state") == "failed":
        _failure(value, "reprint")
        return
    if value.get("state") != "executed" or set(value) != {"state", "files"} or not isinstance(value["files"], list):
        raise ValueError("requested reprint silently dropped or malformed")
    if len(value["files"]) > row["load"]["files"] - row["load"]["libraries"]:
        raise ValueError("reprint lists more files than the program's non-library files")
    names = set()
    for item in value["files"]:
        if not isinstance(item, dict) or set(item) != {"name_hex", "source_sha256", "comments", "no_comments"}:
            raise ValueError("malformed reprint file")
        bytes.fromhex(item["name_hex"])
        if item["name_hex"] in names:
            raise ValueError("reprint file listed twice")
        names.add(item["name_hex"])
        _digest_text(item["source_sha256"], "source")
        validate_print(item["comments"])
        validate_print(item["no_comments"])


def validate_emit(request, row):
    if request.get("emit") is not True:
        if any(row[name] != {"state": "not_requested"} for name in EMIT_DOMAINS):
            raise ValueError("unrequested emit domain was observed")
        return
    output = request["output"]
    for name in EMIT_DOMAINS:
        value = row[name]
        if name == "output" and output["state"] == "disabled":
            if value != {"state": "disabled", "reason": output["reason"]}:
                raise ValueError("a disabled output sub-test ran or lost its reason")
            continue
        if not isinstance(value, dict):
            raise ValueError("emit domain is not an object: " + name)
        if value.get("state") == "disabled":
            raise ValueError("Rust disabled a sub-test the runner runs: " + name)
        # Until T8 every emit domain is a named failure; T8 adds the executed shapes.
        _failure(value, name)


def validate_row(request, row):
    """The phase3_emit row contract; raises on any harness defect."""
    if not isinstance(row, dict) or row.get("version") != 1 or row.get("id") != request["id"] \
            or row.get("acceptance_tier") != request["acceptance_tier"]:
        raise ValueError("missing/extra/reordered observation or changed acceptance tier")
    if "fatal" in row:
        expected = {"version", "id", "acceptance_tier", "fatal"}
        if row["fatal"].get("class") == "panic":
            expected.add("panic_location")
        if set(row) != expected:
            raise ValueError("fatal observation carries a fabricated partial success or misses its panic location")
        p4.state(row["fatal"])
        if row["fatal"]["state"] != "failed":
            raise ValueError("fatal observation is not a failure")
        if row.get("panic_location") is not None and not isinstance(row["panic_location"], str):
            raise ValueError("invalid panic location")
        return row
    if set(row) != ROW_FIELDS:
        raise ValueError("missing or extra row field")
    if row["mode"] != request.get("mode"):
        raise ValueError("the observation's mode differs from the request's")
    load = row["load"]
    if not isinstance(load, dict):
        raise ValueError("load observation is not an object")
    if load.get("state") == "executed":
        if (set(load) != {"state", "files", "libraries"} or type(load["files"]) is not int
                or type(load["libraries"]) is not int or not 0 <= load["libraries"] <= load["files"]):
            raise ValueError("malformed executed load")
    else:
        _failure(load, "load")
    validate_reprint(request, row)
    validate_emit(request, row)
    return row


def adapter_literal(reason):
    """Whether an error text is a string literal of the adapter, as opposed to
    a production error's Display text."""
    global _ADAPTER_TEXT
    if _ADAPTER_TEXT is None:
        paths = [p for name in ADAPTER_SOURCES for p in ([ROOT / name] if name.endswith(".rs") else
                                                          (ROOT / name).rglob("*.rs"))]
        _ADAPTER_TEXT = "\n".join(path.read_text() for path in paths)
    return bool(reason) and f'"{reason}"' in _ADAPTER_TEXT


def completed_problems(row):
    """Harness defects hidden inside a completed row."""
    problems = []
    reprint = row["reprint"]
    if reprint.get("state") == "failed" and adapter_literal(reprint.get("reason")):
        problems.append("adapter reprint failure: " + reprint["reason"])
    for item in reprint.get("files", []):
        for key in ("comments", "no_comments"):
            value = item[key]
            if value["state"] == "failed":
                location = value.get("location") or ""
                if not location.startswith("crates/") or "/examples/" in location:
                    problems.append(f"reprint panic at {location or 'unknown location'}")
    return problems


def attribute(row, stderr):
    """Who owns a fatal row (`phase2_corpus.attribute`'s rules)."""
    return phase2_corpus.attribute(row, stderr)


def run(native_dir, output, jobs, timeout, resume=False, limit=None, *, sample=False, cases=(), mode="single"):
    output = Path(output).resolve()
    selected = phase2_corpus.selection(sample, cases, limit)
    report, request_rows = requests(native_dir, limit, sample=sample, cases=cases, mode=mode)
    native_meta = native_binding(native_dir, report)
    requests_sha256 = digest(p4.canonical(request_rows) + b"\n")
    if output.exists():
        if not resume:
            raise ValueError("existing capture requires --resume")
        metadata = p4.read(output / "capture.json")
        if (metadata["timeout_seconds"] != timeout or metadata["native"] != native_meta
                or phase2_corpus.capture_selection(metadata) != selected or metadata["mode"] != mode
                or metadata["requests_sha256"] != requests_sha256):
            raise ValueError("resume requires the identical native capture, requests, mode and timeout")
    else:
        output.mkdir(parents=True)
        (output / "cases").mkdir()
        record = p4.build(output / "build", example=EXAMPLE, source_fn=sources, optimize=True)
        Path(record["source_snapshot"]).rename(output / "source-snapshot")
        record["source_snapshot"] = str(output / "source-snapshot")
        p4.atomic(output / "build/build.json", record)
        metadata = {"version": 1, "requests_sha256": requests_sha256, "build": record, "timeout_seconds": timeout,
                    "native": native_meta, "partial": selected != phase2_corpus.selection(), "selection": selected,
                    "mode": mode, "inventory_sha256": digest(phase3_inventory.INVENTORY.read_bytes())}
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
                print(f"phase3 corpus {done}/{len(pending)} in {time.monotonic() - started:.0f}s",
                      file=sys.stderr, flush=True)
    elapsed = time.monotonic() - started
    timing = p4.read(output / "timing.json") if (output / "timing.json").exists() else {"segments": []}
    timing["segments"].append({"rows": len(pending), "jobs": jobs, "seconds": round(elapsed, 3)})
    p4.atomic(output / "timing.json", timing)
    result = replay(output)
    print(json.dumps(result["summary"], sort_keys=True))
    return result


def load_capture(output):
    """The capture's metadata, requests and validated rows, with the raw
    artifacts checked against their completion records. Every request must be
    complete: a partial capture is never replayed or compared."""
    output = Path(output).resolve()
    metadata = p4.read(output / "capture.json")
    phase2_corpus.capture_selection(metadata)
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
    rows, stderrs = [], []
    capture_sha256 = digest(p4.canonical(metadata) + b"\n")
    for index, request in enumerate(request_rows):
        path = output / "cases" / f"{index:05d}" / "result.json"
        if not path.exists():
            raise ValueError("capture incomplete at " + request["id"])
        envelope = p4.read(path)
        if (envelope.get("request_sha256") != digest(p4.canonical(request) + b"\n")
                or envelope.get("capture_sha256") != capture_sha256):
            raise ValueError("completion belongs to a different request or capture: " + request["id"])
        artifacts = envelope.get("artifacts")
        row = envelope["row"]
        expected_names = {"stdout", "stderr"}
        child_record = "fatal" not in row or row["fatal"]["class"] == "panic"
        if child_record or (path.parent / "observation.json").exists():
            expected_names.add("observation.json")
        if not isinstance(artifacts, dict) or set(artifacts) != expected_names:
            raise ValueError("missing or extra raw case artifact: " + request["id"])
        for name, expected in artifacts.items():
            if digest((path.parent / name).read_bytes()) != expected:
                raise ValueError("raw case artifact changed: " + request["id"])
        if child_record and p4.read(path.parent / "observation.json") != row:
            raise ValueError("completion differs from its raw observation: " + request["id"])
        if "fatal" not in row or row["fatal"]["class"] != "harness_protocol":
            validate_row(request, row)
        rows.append(row)
        stderrs.append(path.parent / "stderr")
    return metadata, request_rows, rows, stderrs


def replay(output):
    output = Path(output).resolve()
    metadata, request_rows, rows, stderrs = load_capture(output)
    harness, attributed = [], []
    reprint_states = Counter()
    for request, row, stderr in zip(request_rows, rows, stderrs, strict=True):
        if "fatal" in row:
            owner, reason = attribute(row, stderr.read_bytes())
            if owner == "harness":
                harness.append({"id": request["id"], "problem": reason})
            else:
                attributed.append({"id": request["id"], "class": row["fatal"]["class"], "attribution": reason})
            continue
        harness.extend({"id": request["id"], "problem": problem} for problem in completed_problems(row))
        for item in row["reprint"].get("files", []):
            for key in ("comments", "no_comments"):
                reprint_states[f"{key}:{item[key]['state']}"] += 1
    states = Counter("fatal:" + row["fatal"]["class"] if "fatal" in row else "completed" for row in rows)
    loads = Counter(row["load"]["state"] for row in rows if "fatal" not in row)
    selected = phase2_corpus.capture_selection(metadata)
    summary = {"requested": len(request_rows), "observed": len(rows), "states": dict(sorted(states.items())),
               "loads": dict(sorted(loads.items())), "reprint_files": dict(sorted(reprint_states.items())),
               "harness_errors": len(harness), "production_failures": len(attributed),
               "partial": selected != phase2_corpus.selection(), "mode": metadata["mode"]}
    result = {"version": 1, "summary": summary, "harness_errors": harness, "production_failures": attributed,
              "selection": selected, "capture_sha256": digest(p4.canonical(metadata) + b"\n"),
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
    sub.add_argument("--sample", action="store_true", help="the inventory's 300-variant sample; informational")
    sub.add_argument("--case", action="append", default=[], help="executed variant ID; repeatable, additive to --sample")
    sub.add_argument("--limit", type=int, help="development smoke over the first N rows; never recorded")
    sub.add_argument("--mode", choices=MODES, default="single",
                     help="the test-program mode; compares with that mode's native capture")
    sub = commands.add_parser("replay")
    sub.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "run":
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            parser.error("--timeout must be positive and finite")
        run(args.native, args.output, args.jobs, args.timeout, args.resume, args.limit, sample=args.sample,
            cases=args.case, mode=args.mode)
    else:
        print(json.dumps(replay(args.output)["summary"], sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 corpus failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
