#!/usr/bin/env python3
"""S06: frozen direct parser corpus and independent runtime/codec comparisons."""

import argparse
from contextlib import ExitStack, contextmanager
import fcntl
import json
import subprocess
import sys

from s04 import verified_upstream
from s04_common import command, strict_json_loads
from s06_build import ROOT, build_oracle, export_cases
from s06_compare import panic_identity
from s06_corpus import freeze_records, manifest_changes, membership, read_export
from s06_fixtures import fixture_documents
from s06_process import Process
from s06_results import compare_case, report
from s06_watchdog import run_watchdogs
from s06_utilities import measure as measure_utilities


@contextmanager
def producer_lock():
    path = ROOT / "target/s06-producer.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError("another S06 producer owns target/s06-producer.lock") from error
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def freeze(write=False):
    upstream = verified_upstream()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    physical, libraries = membership(upstream, pin)
    output = ROOT / "target/s06-oracle/export.ndjson"
    output.parent.mkdir(parents=True, exist_ok=True)
    export_cases(physical, output)
    records = read_export(output, physical, libraries)
    documents, requests = freeze_records(records, upstream, pin)
    native = strict_json_loads(output.with_name("native-fixtures.json").read_bytes())
    factory = strict_json_loads((ROOT / "data/s06/factory-fixtures.json").read_bytes())
    supplemental, inventory = fixture_documents(native, pin, factory)
    documents.update(supplemental)
    documents["probes.json"]["supplemental"] = inventory
    changes = manifest_changes(ROOT / "data/s06", documents, write=write)
    return documents, requests, changes


def parser_binary():
    output = command(["cargo", "build", "--package", "tsr_parser", "--example", "s06", "--release", "--locked", "--message-format=json"], cwd=ROOT)
    artifacts = [strict_json_loads(line) for line in output.splitlines() if line.strip()]
    binaries = [item["executable"] for item in artifacts if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "s06" and item.get("executable")]
    if len(binaries) != 1:
        raise ValueError("Cargo did not identify exactly one S06 parser example")
    return binaries[0]


def capture(documents, primary, prefix=None):
    command([sys.executable, "tools/s06/node-index-sort/generate.py", "--check"], cwd=ROOT)
    executable = build_oracle()
    binary = parser_binary()
    reports = ROOT / "target/s06-reports"
    reports.mkdir(parents=True, exist_ok=True)
    # The digest/order is checked at aggregation as well as during freeze. Retain
    # source-free result metadata; neither node tables nor encoded buffers grow
    # with the corpus in this process.
    items = [{"request": request, "group": "primary"} for request in primary]
    items.extend(documents["fixtures.json"]["fixtures"])
    selected = [item for item in items if not prefix or item["request"]["id"].startswith(prefix)]
    if not selected:
        raise ValueError("diagnostic filter selected no frozen S06 request")
    watchdog = None
    utilities = None
    if not prefix:
        utilities = measure_utilities(ROOT, reports / "utilities")
        watchdog = run_watchdogs({"oracle": [str(executable)], "rust": [binary]}, reports / "watchdogs")
        (reports / "watchdogs.json").write_text(json.dumps(watchdog, sort_keys=True)+"\n")
    results = []
    nil_wires = {"oracle": set(), "rust": set()}
    with ExitStack() as cleanup:
        oracle = Process([str(executable)], reports / "oracle.stderr", deadline=120)
        cleanup.callback(oracle.close)
        rust = Process([binary], reports / "rust.stderr", deadline=120)
        cleanup.callback(rust.close)
        with (reports / "failures.ndjson").open("w") as failures, (reports / "requests.ndjson").open("w") as outcomes:
            for index, item in enumerate(selected):
                request = item["request"]
                try:
                    oracle.send(request)
                    rust.send(request)
                    result = compare_case(request, oracle.observations(request), rust.observations(request), panic_identity, nil_wires)
                except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
                    raise RuntimeError(f"request {request['id']}: invalid capture: {error}") from error
                if result["failure"] is not None:
                    failure = {"id": request["id"], "request": request, "failure": result["failure"]}
                    failures.write(json.dumps(failure, ensure_ascii=True)+"\n")
                    failures.flush()
                    print("S06 mismatch " + request["id"] + ": " + json.dumps(result["failure"], ensure_ascii=True)[:1500], file=sys.stderr)
                result.pop("failure")
                outcomes.write(json.dumps({"id": request["id"], **result}, sort_keys=True)+"\n")
                results.append(result)
                if index % 500 == 0:
                    outcomes.flush()
                    print(f"S06 compared {index+1}/{len(selected)} requests", file=sys.stderr)
        oracle.finish()
        rust.finish()
        print(f"S06 oracle stream sha256={oracle.digest.hexdigest()}; Rust stream sha256={rust.digest.hexdigest()}", file=sys.stderr)
    verified_upstream()
    if prefix:
        return {"diagnostic_subset": True, "requests": len(results), "failed_requests": sum(not result["pass"] for result in results)}
    output = report(items, results, documents["cases.json"], documents["probes.json"])
    output["metrics"].update({"decoder_watchdog": watchdog["metric"],
                              "decoder_watchdog_requests": watchdog["requests"],
                              "decoder_watchdog_runtimes": watchdog["runtimes"],
                              "ast_utilities": utilities["metric"],
                              "ast_utility_tests": utilities["tests"],
                              "ast_utility_groups": utilities["groups"]})
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("freeze", "e1"))
    parser.add_argument("--write-manifest", action="store_true")
    parser.add_argument("--filter", help="Diagnostic subset only; never emits tracker metrics")
    args = parser.parse_args()
    try:
        with producer_lock():
            if args.mode == "e1" and args.write_manifest:
                raise ValueError("parity capture cannot rewrite its frozen obligations")
            if args.mode == "freeze" and args.filter:
                raise ValueError("freeze cannot select a subset of the corpus")
            documents, requests, changes = freeze(args.write_manifest)
            if args.mode == "freeze":
                output = {"operation": "freeze" if args.write_manifest else "verify", "changed": changes, "inventory": documents["probes.json"]}
            else:
                output = capture(documents, requests, args.filter)
            print(json.dumps(output, sort_keys=True))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"S06 capture failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
