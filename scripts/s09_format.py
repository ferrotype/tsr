#!/usr/bin/env python3
"""Native formatter observations over the frozen parser inventory (S09-3, step F0).

The Go oracle in tools/s09/format_oracle is built inside a fresh export of the
pinned tree and answers navigation, indentation and document-formatting probes
for every distinct parser input of the S06 inventory. Observations are row
counts and digests; `detail` prints the rows of one request for diagnosis.
See docs/S09-3-formatter-plan.md.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

from s04 import go_environment, verified_upstream
from s04_common import command, strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
ORACLE = ROOT / "tools/s09/format_oracle/main.go"
PROBES = ROOT / "data/s09/format-probes.json"
EXPORT_PATHS = ("tsc/go.mod", "tsc/go.sum", "tsc/internal")
OPS = ("nav", "indent", "format", "position", "insert")
INDENT_VARIANTS = ("default", "tabs", "two")
INSERT_VARIANTS = ("default", "tabs")
FORMAT_VARIANTS = ("default", "tabs", "two", "dense", "terse")
REQUEST_FIELDS = ("source_hex", "filename", "path", "script_kind", "jsx", "force")
VERSION = 1


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def build(directory):
    """Compile the oracle from a fresh export; the submodule is never touched."""
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    upstream = verified_upstream()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    env = go_environment()
    destination = directory / "format-oracle"
    with tempfile.TemporaryDirectory(prefix="export-", dir=directory) as temporary:
        checkout = Path(temporary)
        archive = command(["git", "archive", pin, *EXPORT_PATHS], cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(checkout, filter="data")
        package = checkout / "tsc/internal/s09format"
        package.mkdir()
        shutil.copyfile(ORACLE, package / "main.go")
        command(["go", "vet", "-mod=readonly", "./internal/s09format"], cwd=checkout / "tsc", env=env)
        command(["go", "build", "-trimpath", "-mod=readonly", "-o", str(destination), "./internal/s09format"],
                cwd=checkout / "tsc", env=env)
    verified_upstream()
    version = command(["go", "version"], cwd=ROOT, env=env).decode().strip()
    report = {"version": VERSION, "pin": pin, "go": version, "toolchain_local": env["GOTOOLCHAIN"] == "local",
              "oracle_sha256": digest(ORACLE.read_bytes()), "binary": str(destination),
              "binary_sha256": digest(destination.read_bytes())}
    (directory / "build.json").write_bytes(canonical(report) + b"\n")
    return report


def inventory():
    """Distinct parser inputs of the frozen S06 inventory, in inventory order."""
    from s06 import freeze
    _, requests, changes = freeze()
    if changes:
        raise ValueError("the S06 manifests differ from a fresh reconstruction: " + ", ".join(changes))
    distinct, seen = [], set()
    for request in requests:
        if request["op"] != "parse":
            raise ValueError("unexpected primary S06 operation " + request["op"])
        identity = digest(canonical({field: request[field] for field in REQUEST_FIELDS}))
        if identity in seen:
            continue
        seen.add(identity)
        distinct.append({"version": VERSION, "id": request["id"], **{field: request[field] for field in REQUEST_FIELDS}})
    return distinct, len(requests)


def validate_stream(name, value):
    if set(value) == {"panic"} and isinstance(value["panic"], str):
        return
    fields = set(value) - {"detail"}
    if name == "format":
        # Exactly one: the applied text, or the native failure to apply the edits.
        if len(fields & {"text_sha256", "text_panic"}) != 1:
            raise ValueError("malformed format observation")
        fields -= {"text_sha256", "text_panic"}
    if fields != {"rows", "failures", "sha256"}:
        raise ValueError(f"malformed {name} observation")
    if (type(value["rows"]) is not int or type(value["failures"]) is not int
            or not 0 <= value["failures"] <= value["rows"] or len(value["sha256"]) != 64):
        raise ValueError(f"malformed {name} observation")


def validate_observation(request, observation, ops):
    if observation.get("id") != request["id"]:
        raise ValueError("observation answers another request: " + str(observation.get("id")))
    if "error" in observation:
        raise ValueError(f"{request['id']}: oracle rejected the request: {observation['error']}")
    expected = {"id", "parse", *ops}
    if set(observation) != expected:
        raise ValueError(f"{request['id']}: observation fields {sorted(observation)} != {sorted(expected)}")
    for op in ("nav", "position"):
        if op in ops:
            validate_stream(op, observation[op])
    for op, names in (("indent", INDENT_VARIANTS), ("format", FORMAT_VARIANTS), ("insert", INSERT_VARIANTS)):
        if op in ops:
            if set(observation[op]) != set(names):
                raise ValueError(f"{request['id']}: {op} variants differ from the contract")
            for value in observation[op].values():
                validate_stream(op, value)


def run(binary, requests, ops, output, detail=False):
    """One long-lived process; a request is answered before the next is sent."""
    process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    stream = hashlib.sha256()
    totals = {"requests": 0, "rows": 0, "failure_rows": 0, "panics": 0, "unappliable_edit_lists": 0}
    panics = []
    started = time.monotonic()
    try:
        with Path(output).open("wb") as sink:
            for index, request in enumerate(requests):
                line = canonical({**request, "ops": list(ops), "detail": detail}) + b"\n"
                process.stdin.write(line)
                process.stdin.flush()
                answer = process.stdout.readline()
                if not answer.endswith(b"\n"):
                    raise RuntimeError(f"{request['id']}: oracle ended: " + process.stderr.read().decode(errors="replace")[-2000:])
                observation = strict_json_loads(answer)
                validate_observation(request, observation, ops)
                sink.write(answer)
                stream.update(answer)
                totals["requests"] += 1
                for value in walk_streams(observation, ops):
                    if "panic" in value:
                        totals["panics"] += 1
                        panics.append(request["id"])
                    else:
                        totals["rows"] += value["rows"]
                        totals["failure_rows"] += value["failures"]
                        totals["unappliable_edit_lists"] += "text_panic" in value
                if index % 2000 == 0:
                    print(f"S09 format oracle {index + 1}/{len(requests)}", file=sys.stderr)
        process.stdin.close()
        if process.wait(timeout=60) != 0:
            raise RuntimeError("oracle failed: " + process.stderr.read().decode(errors="replace")[-2000:])
    finally:
        if process.poll() is None:
            process.kill()
    return {**totals, "stream_sha256": stream.hexdigest(), "seconds": round(time.monotonic() - started, 1),
            "panic_ids": sorted(set(panics))}


def walk_streams(observation, ops):
    if "parse" in observation and "panic" in observation["parse"]:
        yield observation["parse"]
    for op in ("nav", "position"):
        if op in ops:
            yield observation[op]
    for op in ("indent", "format", "insert"):
        if op in ops:
            yield from observation[op].values()


def observe(directory, ops, prefix=None, limit=None):
    directory = Path(directory).resolve()
    report = build(directory)
    requests, total = inventory()
    selected = [r for r in requests if not prefix or r["id"].startswith(prefix)][:limit]
    if not selected:
        raise ValueError("the filter selected no request")
    inventory_sha256 = digest(b"".join(canonical(r) + b"\n" for r in requests))
    result = run(report["binary"], selected, ops, directory / "native.ndjson")
    summary = {"version": VERSION, "pin": report["pin"], "go": report["go"], "toolchain_local": report["toolchain_local"],
               "oracle_sha256": report["oracle_sha256"], "script_sha256": digest(Path(__file__).read_bytes()),
               "ops": list(ops), "indent_variants": list(INDENT_VARIANTS), "format_variants": list(FORMAT_VARIANTS),
               "insert_variants": list(INSERT_VARIANTS),
               "inventory": {"s06_requests": total, "distinct_inputs": len(requests), "sha256": inventory_sha256},
               "diagnostic_subset": len(selected) != len(requests), "native": result}
    (directory / "summary.json").write_bytes(canonical(summary) + b"\n")
    return summary


def frozen_form(summary):
    """What a later run must reproduce exactly: no elapsed time, no subset flag."""
    native = {key: value for key, value in summary["native"].items() if key != "seconds"}
    return {**{key: value for key, value in summary.items() if key != "diagnostic_subset"}, "native": native}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "observe", "detail", "freeze", "verify"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ops", default=",".join(OPS))
    parser.add_argument("--prefix")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--id")
    args = parser.parse_args()
    ops = tuple(args.ops.split(","))
    if not ops or any(op not in OPS for op in ops):
        raise ValueError("unknown operation")
    if args.command == "build":
        print(json.dumps(build(args.output), sort_keys=True))
    elif args.command == "detail":
        report = build(args.output)
        requests, _ = inventory()
        selected = [r for r in requests if r["id"] == args.id]
        if len(selected) != 1:
            raise ValueError("--id must name exactly one distinct input")
        run(report["binary"], selected, ops, Path(args.output) / "detail.ndjson", detail=True)
        print((Path(args.output) / "detail.ndjson").read_text())
    else:
        summary = observe(args.output, ops, args.prefix, args.limit)
        if args.command in ("freeze", "verify"):
            if summary["diagnostic_subset"] or set(ops) != set(OPS):
                raise ValueError("only a complete observation can be frozen or verified")
            content = canonical(frozen_form(summary)) + b"\n"
            if args.command == "freeze":
                PROBES.write_bytes(content)
            elif not PROBES.exists() or PROBES.read_bytes() != content:
                raise ValueError("native observations differ from data/s09/format-probes.json")
        print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as error:
        print(f"S09 format observation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
