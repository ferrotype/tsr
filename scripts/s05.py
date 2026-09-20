#!/usr/bin/env python3
"""S05 scanner producer: pinned oracle, frozen requests and bounded stream comparison."""

import argparse
from contextlib import contextmanager
import fcntl
import json
from pathlib import Path
import subprocess
import sys

from s04_common import command, strict_json_loads
from s05_build import ROOT, build_oracle, verified_upstream
from s05_cases import freeze, requests
from s05_protocol import Process, compare_case, report
from s05_tables import update as update_tables


@contextmanager
def producer_lock():
    """Serialize owned build/report paths without removing another process's lock."""
    path = ROOT / "target/s05-producer.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError("another S05 producer owns target/s05-producer.lock; wait for it to finish") from error
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def scanner(executable, env, upstream, tables, prefix=None):
    update_tables(tables)
    corpus, supplemental, probes = freeze(upstream, tables)
    # Cargo identifies its freshly built executable even with configured target directories.
    build = command(["cargo", "build", "--package", "tsr_scanner", "--example", "s05", "--release", "--locked", "--message-format=json"], cwd=ROOT)
    artifacts = [strict_json_loads(line) for line in build.splitlines() if line.strip()]
    binaries = [item["executable"] for item in artifacts if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "s05" and item.get("executable")]
    if len(binaries) != 1:
        raise ValueError("Cargo did not identify exactly one S05 example executable")
    logs = ROOT / "target/s05-reports"
    logs.mkdir(parents=True, exist_ok=True)
    oracle = Process([str(executable)], logs / "oracle.stderr", env=env)
    rust = Process([binaries[0]], logs / "rust.stderr")
    items, results = [], []
    try:
        with (logs / "failures.ndjson").open("w") as failures:
            for index, item in enumerate(requests(upstream, corpus, supplemental)):
                request = item["request"]
                if prefix and not request["id"].startswith(prefix):
                    continue
                try:
                    oracle.send(request); rust.send(request)
                    result = compare_case(item, oracle.observations(request, "oracle"), rust.observations(request, "rust"))
                except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
                    raise RuntimeError(f"case {request['id']} protocol failed: {error}") from error
                if result["failure"] is not None:
                    failure = {"id": request["id"], "request": request, "failure": result["failure"]}
                    failures.write(json.dumps(failure, ensure_ascii=True) + "\n")
                    print("S05 mismatch " + request["id"] + ": " + json.dumps(result["failure"], ensure_ascii=True)[:1500], file=sys.stderr)
                result.pop("failure")
                # Keep only aggregation metadata, not all corpus source/token bytes.
                items.append({"request": {"id": request["id"]}, "groups": item["groups"], "witness": item["witness"]})
                results.append(result)
                if index % 2000 == 0:
                    print(f"S05 compared {len(items)} cases", file=sys.stderr)
        oracle.finish(); rust.finish()
        print(f"S05 oracle stream sha256={oracle.digest.hexdigest()}; Rust stream sha256={rust.digest.hexdigest()}", file=sys.stderr)
    finally:
        oracle.close(); rust.close()
    verified_upstream()
    if prefix:
        if not results:
            raise ValueError("diagnostic filter selected no frozen cases")
        print(json.dumps({"selected_cases": len(results), "failed_cases": sum(not result["pass"] for result in results)}))
        return None
    output = report(items, results)
    output["metrics"].update(table_current=True, source_files=probes["source_files"], source_bytes=probes["source_bytes"])
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("scanner", "tables", "freeze", "oracle"))
    parser.add_argument("--write-manifest", action="store_true")
    parser.add_argument("--filter", help="Diagnostic subset only; never emits a tracker report")
    args = parser.parse_args()
    try:
        with producer_lock():
            return run(args)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"S05 capture failed: {error}", file=sys.stderr)
        return 1


def run(args):
    upstream = verified_upstream()
    executable, env = build_oracle()
    tables = strict_json_loads(command([str(executable), "--tables"], cwd=ROOT, env=env))
    if args.mode == "tables":
        update_tables(tables, write=args.write_manifest)
    elif args.mode == "freeze":
        freeze(upstream, tables, write=args.write_manifest)
    elif args.mode == "oracle":
        print(executable)
    else:
        if args.write_manifest:
            raise ValueError("scanner capture cannot rewrite frozen manifests")
        result = scanner(executable, env, upstream, tables, args.filter)
        if result is not None:
            print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
