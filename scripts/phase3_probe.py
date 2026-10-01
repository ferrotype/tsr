#!/usr/bin/env python3
"""Phase 3: native transform probes.

Only two of the pinned transformers have Go unit tests; the others are
witnessed by corpus emit, which needs the whole pipeline. A probe gives one
transformer (or any chain of them) a native expectation of its own: the driver
(`tools/phase3/probe/`, an access-only overlay) compiles a small compiler-test
source as the compiler runner does, runs the named chain of pinned
transformers over each source file with the options the emitter would build,
and prints the result with the emitter's printer options.

A requests file is authored by hand:

    {"cases": [{"id": "enum-1", "name": "probe.ts", "source": "enum E { A }",
                "chains": [["typeeraser", "runtimesyntax"], ["script"]]}]}

`source` uses the compiler-test format (`// @target: es2015`, `// @filename:`),
with one configuration per case. `["script"]` is the pin's own chain for the
file. `names` lists the transformer names.

    capture --requests FILE --output FILE    # run the oracle, write the native fixture
    check   --requests FILE --native FILE    # run it again and require the same fixture
    names

The fixture binds the pin, the overlay sources and the requests by digest. The
oracle binary is cached under target/phase3/probe-oracle by overlay digest.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

BRIDGES = ROOT / "tools/phase3/probe"
CACHE = ROOT / "target/phase3/probe-oracle"
TEST = "TestPhase3Probe"
OVERLAY = {"compiler/phase3_probe.go": "compiler_probe.go",
           "transformers/estransforms/phase3_probe.go": "estransforms_probe.go",
           "testrunner/phase3_probe_test.go": "probe_test.go"}


def overlay_sources():
    return {name: (BRIDGES / source).read_text() for name, source in sorted(OVERLAY.items())}


def oracle(upstream, env):
    """The probe binary for the current overlay, built once per overlay digest."""
    sources = overlay_sources()
    identity = {name: digest(text.encode()) for name, text in sources.items()}
    directory = CACHE / digest(canonical({"pin": pin(), "overlay": identity}))[:16]
    binary = directory / "probe.test"
    if not binary.exists():
        if directory.exists():
            shutil.rmtree(directory)
        directory.mkdir(parents=True)
        replacements = {}
        for name, source in sources.items():
            if (upstream / "tsc/internal" / name).exists():
                raise ValueError("overlay would replace a pinned source file: " + name)
            path = directory / "overlay" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
            replacements[str(upstream / "tsc/internal" / name)] = str(path)
        (directory / "overlay.json").write_bytes(canonical({"Replace": replacements}))
        staged = directory / "probe.test.building"
        # The repo package finds the checkout from its own source path.
        repo_flag = ("-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath="
                     + str(directory / "unmatched-prefix"))
        command = ["go", "test", "-c", "-o", str(staged), "-trimpath", "-mod=readonly", repo_flag,
                   "-overlay", str(directory / "overlay.json"), "./internal/testrunner"]
        with (directory / "build.stdout").open("wb") as out, (directory / "build.stderr").open("wb") as err:
            completed = subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=out, stderr=err, check=False)
        if completed.returncode or not staged.exists():
            raise ValueError("probe oracle build failed; see " + str(directory / "build.stderr"))
        staged.rename(binary)
    return binary, identity


def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def read_requests(path):
    document = strict_json_loads(Path(path).read_bytes())
    if set(document) != {"cases"} or not isinstance(document["cases"], list) or not document["cases"]:
        raise ValueError("a requests file is {\"cases\": [...]} with at least one case")
    seen = set()
    for case in document["cases"]:
        if set(case) != {"id", "name", "source", "chains"}:
            raise ValueError("a case has exactly id, name, source and chains: " + str(case.get("id")))
        if not isinstance(case["id"], str) or not case["id"] or case["id"] in seen:
            raise ValueError("empty or duplicate case id: " + str(case["id"]))
        seen.add(case["id"])
        if (not isinstance(case["name"], str) or "/" in case["name"] or not isinstance(case["source"], str)
                or not isinstance(case["chains"], list) or not case["chains"]
                or any(not isinstance(chain, list) or not chain or any(not isinstance(name, str) for name in chain)
                       for chain in case["chains"])):
            raise ValueError("malformed case: " + case["id"])
    return document["cases"]


def text(value):
    """Bytes as a JSON string where they are UTF-8, else as hex."""
    raw = bytes.fromhex(value)
    try:
        return {"text": raw.decode("utf-8")}
    except UnicodeDecodeError:
        return {"text_hex": value}


def run(cases):
    upstream = verified_upstream().resolve()
    env = go_environment()
    binary, identity = oracle(upstream, env)
    requests = [{"id": case["id"], "name": case["name"], "source_hex": case["source"].encode().hex(),
                 "chains": case["chains"]} for case in cases]
    with tempfile.TemporaryDirectory(prefix="phase3-probe-") as scratch:
        scratch = Path(scratch)
        (scratch / "requests.json").write_bytes(canonical(requests) + b"\n")
        run_env = dict(env, PHASE3_REQUESTS=str(scratch / "requests.json"), PHASE3_OUTPUT=str(scratch / "rows.ndjson"),
                       PHASE3_SUMMARY=str(scratch / "summary.json"))
        completed = subprocess.run([str(binary), "-test.run", f"^{TEST}$", "-test.count=1", "-test.timeout=10m"],
                                   cwd=upstream / "tsc/internal/testrunner", env=run_env, capture_output=True, check=False)
        if not (scratch / "summary.json").exists():
            raise ValueError("probe run did not complete:\n" + (completed.stdout + completed.stderr).decode(errors="replace")[-4000:])
        summary = strict_json_loads((scratch / "summary.json").read_bytes())
        rows = [strict_json_loads(line) for line in (scratch / "rows.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in rows] != [case["id"] for case in cases]:
        raise ValueError("probe rows are missing, extra or reordered")
    results = []
    for case, row in zip(cases, rows, strict=True):
        if row["state"] != "executed":
            raise ValueError(f"probe {case['id']} failed natively: {row.get('message')}\n"
                             + completed.stdout.decode(errors="replace")[-4000:])
        files = []
        for file in row["files"]:
            chains = []
            for chain in file["chains"]:
                entry = {"chain": chain["chain"]}
                if chain["state"] == "printed":
                    entry.update(text(chain["text_hex"]))
                else:
                    entry["panic"] = chain["message"]
                chains.append(entry)
            files.append({"name": bytes.fromhex(file["name_hex"]).decode(), "chains": chains})
        results.append({"id": case["id"], "name": case["name"], "source": case["source"],
                        "diagnostics": row["diagnostics"], "files": files})
    return {"version": 1, "pin": pin(), "overlay_sha256": identity,
            "go": summary["go"], "transformers": summary["transformers"], "cases": results}


def render(document):
    return json.dumps(document, indent=1, sort_keys=True, ensure_ascii=False).encode() + b"\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("capture")
    sub.add_argument("--requests", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    sub = commands.add_parser("check")
    sub.add_argument("--requests", type=Path, required=True)
    sub.add_argument("--native", type=Path, required=True)
    commands.add_parser("names")
    args = parser.parse_args()
    if args.command == "names":
        document = run([{"id": "names", "name": "probe.ts", "source": "var x;", "chains": [["script"]]}])
        print("\n".join(document["transformers"]))
        return
    document = run(read_requests(args.requests))
    if args.command == "capture":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(render(document))
        panics = sum("panic" in chain for case in document["cases"] for file in case["files"] for chain in file["chains"])
        print(json.dumps({"cases": len(document["cases"]), "panics": panics}))
    elif args.native.read_bytes() != render(document):
        raise ValueError("the native fixture is stale or was edited; rerun capture and review the diff")
    else:
        print(json.dumps({"cases": len(document["cases"]), "current": True}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 probe failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
