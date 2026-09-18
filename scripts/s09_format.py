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
# Copied into the export's internal/format: access to unexported internals.
BRIDGE = ROOT / "tools/s09/format_oracle/format_bridge.go"
PROBES = ROOT / "data/s09/format-probes.json"
EXPORT_PATHS = ("tsc/go.mod", "tsc/go.sum", "tsc/internal")
OPS = ("nav", "indent", "format", "position", "insert", "scan", "rules")
# Facts about the implementation rather than about an input. They are asked once,
# with the first input of the inventory as the carrier.
GLOBAL_OPS = ("rulesmap",)
INDENT_VARIANTS = ("default", "tabs", "two")
INSERT_VARIANTS = ("default", "tabs")
FORMAT_VARIANTS = ("default", "tabs", "two", "dense", "terse")
# Operations that answer with one stream, and those that answer per settings variant.
SINGLE = ("nav", "position", "scan", "rulesmap")
VARIANTS = {"indent": INDENT_VARIANTS, "format": FORMAT_VARIANTS, "insert": INSERT_VARIANTS, "rules": FORMAT_VARIANTS}
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
        bridge = checkout / "tsc/internal/format/s09_bridge.go"
        if bridge.exists():
            raise ValueError("the format bridge would replace a pinned source file")
        shutil.copyfile(BRIDGE, bridge)
        command(["go", "vet", "-mod=readonly", "./internal/s09format"], cwd=checkout / "tsc", env=env)
        command(["go", "build", "-trimpath", "-mod=readonly", "-o", str(destination), "./internal/s09format"],
                cwd=checkout / "tsc", env=env)
    verified_upstream()
    version = command(["go", "version"], cwd=ROOT, env=env).decode().strip()
    report = {"version": VERSION, "pin": pin, "go": version, "toolchain_local": env["GOTOOLCHAIN"] == "local",
              "oracle_sha256": digest(ORACLE.read_bytes() + BRIDGE.read_bytes()), "binary": str(destination),
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
    for op in SINGLE:
        if op in ops:
            validate_stream(op, observation[op])
    for op, names in VARIANTS.items():
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


def rust_binary():
    """The harness in tools/s09/format-harness, built in release mode."""
    output = command(["cargo", "build", "--package", "s09_format_harness", "--release", "--locked",
                      "--message-format=json"], cwd=ROOT)
    binaries = [item["executable"] for item in map(strict_json_loads, filter(bytes.strip, output.splitlines()))
                if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "s09_format_harness"
                and item.get("executable")]
    if len(binaries) != 1:
        raise ValueError("Cargo did not identify exactly one format harness")
    return binaries[0]


class Side:
    """One long-lived implementation answering one request at a time."""

    def __init__(self, name, binary):
        self.name = name
        self.process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE)

    def ask(self, line, request):
        self.process.stdin.write(line)
        self.process.stdin.flush()
        answer = self.process.stdout.readline()
        if not answer.endswith(b"\n"):
            raise RuntimeError(f"{request['id']}: {self.name} ended: "
                               + self.process.stderr.read().decode(errors="replace")[-2000:])
        return strict_json_loads(answer)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.process.kill()


def differences(native, rust, ops):
    """Where two observations of one request disagree, by operation and variant."""
    out = []
    if native.get("parse") != rust.get("parse"):
        out.append("parse")
    for op in ops:
        left, right = native.get(op), rust.get(op)
        if op in SINGLE:
            if left != right:
                out.append(op)
        else:
            out.extend(f"{op}/{name}" for name in sorted(set(left or {}) | set(rust.get(op) or {}))
                       if (left or {}).get(name) != (right or {}).get(name))
    return out


def compare(directory, ops, prefix=None, limit=None):
    """Live differential: both implementations answer every request."""
    directory = Path(directory).resolve()
    report = build(directory)
    rust = rust_binary()
    requests, total = inventory()
    selected = [r for r in requests if not prefix or r["id"].startswith(prefix)][:limit]
    if not selected:
        raise ValueError("the filter selected no request")
    sides = [Side("oracle", report["binary"]), Side("rust", rust)]
    matched, failures, by_part = 0, [], {}
    started = time.monotonic()
    global_matches = {}
    try:
        carrier = global_request(requests)
        native, ported = (side.ask(canonical(carrier) + b"\n", carrier) for side in sides)
        for op in GLOBAL_OPS:
            global_matches[op] = "error" not in ported and native.get(op) == ported.get(op)
        with (directory / "failures.ndjson").open("w") as sink:
            for index, request in enumerate(selected):
                line = canonical({**request, "ops": list(ops), "detail": False}) + b"\n"
                native, ported = (side.ask(line, request) for side in sides)
                validate_observation(request, native, ops)
                if "error" in ported:
                    parts = ["error: " + str(ported["error"])]
                else:
                    parts = differences(native, ported, ops)
                if parts:
                    failures.append(request["id"])
                    for part in parts:
                        by_part[part.split(":")[0]] = by_part.get(part.split(":")[0], 0) + 1
                    sink.write(json.dumps({"id": request["id"], "parts": parts, "native": native, "rust": ported},
                                          sort_keys=True) + "\n")
                else:
                    matched += 1
                if index % 2000 == 0:
                    print(f"S09 format compare {index + 1}/{len(selected)}: {matched} match", file=sys.stderr)
    finally:
        for side in sides:
            side.close()
    summary = {"version": VERSION, "pin": report["pin"], "ops": list(ops),
               "inventory": {"s06_requests": total, "distinct_inputs": len(requests)},
               "diagnostic_subset": len(selected) != len(requests), "requests": len(selected), "matched": matched,
               "parity": matched / len(selected), "mismatched_by_part": dict(sorted(by_part.items())),
               "global": global_matches,
               "first_failures": failures[:20], "seconds": round(time.monotonic() - started, 1)}
    (directory / "compare.json").write_bytes(canonical(summary) + b"\n")
    return summary


def global_request(requests):
    return {**requests[0], "id": "global", "ops": list(GLOBAL_OPS), "detail": False}


def walk_streams(observation, ops):
    if "parse" in observation and "panic" in observation["parse"]:
        yield observation["parse"]
    for op in SINGLE:
        if op in ops:
            yield observation[op]
    for op in VARIANTS:
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
    carrier = global_request(requests)
    answered = run(report["binary"], [{k: v for k, v in carrier.items() if k not in ("ops", "detail")}],
                   GLOBAL_OPS, directory / "native-global.ndjson")
    if answered["panics"]:
        raise ValueError("the oracle failed on a global operation")
    result["global"] = {op: {k: v for k, v in strict_json_loads((directory / "native-global.ndjson").read_bytes())[op].items()
                             if k in ("rows", "failures", "sha256")} for op in GLOBAL_OPS}
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
    parser.add_argument("command", choices=("build", "observe", "detail", "freeze", "verify", "compare"))
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
    elif args.command == "compare":
        print(json.dumps(compare(args.output, ops, args.prefix, args.limit), sort_keys=True))
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
