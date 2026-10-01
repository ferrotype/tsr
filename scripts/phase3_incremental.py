#!/usr/bin/env python3
"""Phase 3: the native build-info witness of `tsr_incremental`.

The corpus compares the `.tsbuildinfo` file *name* only (`EmittedFiles`);
this witness records the pin's build-info *text*. The driver
(`tools/phase3/incremental/`, an access-only overlay) sets each case up as
the compiler runner sets up a test (`newCompilerTest`), then runs its steps:
each step is one compilation as the pin's harness runs its post-emit
compilation (`createProgram` over a fresh file system, so an `incremental`
program is `incremental.NewProgram` over the build info the harness's test
reader reads), on the previous step's files plus everything that step wrote,
with the step's edits.

A requests file is authored by hand:

    {"cases": [{"id": "two-files", "name": "a.ts", "source": "// @incremental: true\\n...",
                "steps": [{"actions": ["emit"]},
                          {"edits": {"/.src/a.ts": "export const a = 2;"}, "actions": ["diagnostics", "emit"]}]}]}

A case gives `source` (the compiler-test format, one configuration) or
`corpus` (a path under `tsc/testdata/tests/cases`, read at the pin). A step
has `actions` (`emit`: `Emit(ctx, EmitOptions{})`; `diagnostics`: the
harness's post-emit diagnostics) and optionally `edits` (absolute name to
text, `null` deletes), `roots` (the program's new root names) and `options`
(compiler options set with `ParseCompilerOptions`).

    capture --requests FILE --output FILE    # run the driver, write the native fixture
    check   --requests FILE --native FILE    # run it again and require the same fixture

Each step of the fixture carries its `loading` request (the S07 shape) and
each case its `error_inputs` (the runner's units, which the executor's
config parse reads), so the Rust witness
(`crates/tsr_incremental/tests/buildinfo_witness.rs`) loads the same programs
without porting the test-file format. The driver binary is cached under
target/phase3/inc-oracle by overlay digest.
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

BRIDGES = ROOT / "tools/phase3/incremental"
CACHE = ROOT / "target/phase3/inc-oracle"
TEST = "TestPhase3Incremental"
OVERLAY = {"testutil/harnessutil/phase3_incremental_step.go": "harnessutil_step.go",
           "testrunner/phase3_incremental_test.go": "incremental_test.go"}
STEP_KEYS = {"edits", "roots", "options", "actions"}
ACTIONS = {"emit", "diagnostics"}


def overlay_sources():
    return {name: (BRIDGES / source).read_text() for name, source in sorted(OVERLAY.items())}


def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def oracle(upstream, env):
    """The driver binary for the current overlay, built once per overlay digest."""
    sources = overlay_sources()
    identity = {name: digest(text.encode()) for name, text in sources.items()}
    directory = CACHE / digest(canonical({"pin": pin(), "overlay": identity}))[:16]
    binary = directory / "incremental.test"
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
        staged = directory / "incremental.test.building"
        # The repo package finds the checkout from its own source path.
        repo_flag = ("-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath="
                     + str(directory / "unmatched-prefix"))
        command = ["go", "test", "-c", "-o", str(staged), "-trimpath", "-mod=readonly", repo_flag,
                   "-overlay", str(directory / "overlay.json"), "./internal/testrunner"]
        with (directory / "build.stdout").open("wb") as out, (directory / "build.stderr").open("wb") as err:
            completed = subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=out, stderr=err, check=False)
        if completed.returncode or not staged.exists():
            raise ValueError("incremental driver build failed; see " + str(directory / "build.stderr"))
        staged.rename(binary)
    return binary, identity


def read_requests(path, upstream):
    document = strict_json_loads(Path(path).read_bytes())
    if set(document) != {"cases"} or not isinstance(document["cases"], list) or not document["cases"]:
        raise ValueError("a requests file is {\"cases\": [...]} with at least one case")
    seen = set()
    cases = []
    for case in document["cases"]:
        keys = set(case)
        if keys not in ({"id", "name", "source", "steps"}, {"id", "corpus", "steps"}):
            raise ValueError("a case has id, steps and either name and source or corpus: " + str(case.get("id")))
        if not isinstance(case["id"], str) or not case["id"] or case["id"] in seen:
            raise ValueError("empty or duplicate case id: " + str(case["id"]))
        seen.add(case["id"])
        if "corpus" in case:
            source_path = upstream / "tsc/testdata/tests/cases" / case["corpus"]
            name, source = source_path.name, source_path.read_text()
        else:
            name, source = case["name"], case["source"]
        if not isinstance(name, str) or "/" in name or not isinstance(source, str):
            raise ValueError("malformed case: " + case["id"])
        steps = case["steps"]
        if not isinstance(steps, list) or not steps:
            raise ValueError("a case has at least one step: " + case["id"])
        for step in steps:
            if not isinstance(step, dict) or not set(step) <= STEP_KEYS or "actions" not in step:
                raise ValueError("a step has actions and optionally edits, roots and options: " + case["id"])
            if not isinstance(step["actions"], list) or not step["actions"] or not set(step["actions"]) <= ACTIONS:
                raise ValueError("unknown or missing step actions: " + case["id"])
            edits = step.get("edits", {})
            if not isinstance(edits, dict) or any(not name.startswith("/") or not (text is None or isinstance(text, str))
                                                  for name, text in edits.items()):
                raise ValueError("an edit maps an absolute name to text or null: " + case["id"])
            roots = step.get("roots", [])
            if not isinstance(roots, list) or any(not isinstance(root, str) or not root.startswith("/") for root in roots):
                raise ValueError("roots are absolute names: " + case["id"])
            if not isinstance(step.get("options", {}), dict):
                raise ValueError("step options are an object: " + case["id"])
        cases.append({"id": case["id"], "name": name, "source": source, "steps": steps,
                      **({"corpus": case["corpus"]} if "corpus" in case else {})})
    return cases


def text(value):
    """Bytes as a JSON string where they are UTF-8, else as hex."""
    raw = bytes.fromhex(value)
    try:
        return {"text": raw.decode("utf-8")}
    except UnicodeDecodeError:
        return {"text_hex": value}


def diagnostics(values):
    return [{"file": d["file"], "pos": d["pos"], "end": d["end"], "code": d["code"],
             "message": bytes.fromhex(d["message_hex"]).decode()} for d in values]


def run(cases):
    upstream = verified_upstream().resolve()
    env = go_environment()
    env.pop("TS_TEST_PROGRAM_SINGLE_THREADED", None)
    binary, identity = oracle(upstream, env)
    requests = [{"id": case["id"], "name": case["name"], "source_hex": case["source"].encode().hex(),
                 "steps": [{"edits": step.get("edits", {}), "roots": step.get("roots"),
                            "options": step.get("options"), "actions": step["actions"]} for step in case["steps"]]}
                for case in cases]
    with tempfile.TemporaryDirectory(prefix="phase3-incremental-") as scratch:
        scratch = Path(scratch)
        (scratch / "requests.json").write_bytes(canonical(requests) + b"\n")
        run_env = dict(env, PHASE3_REQUESTS=str(scratch / "requests.json"), PHASE3_OUTPUT=str(scratch / "rows.ndjson"),
                       PHASE3_SUMMARY=str(scratch / "summary.json"))
        completed = subprocess.run([str(binary), "-test.run", f"^{TEST}$", "-test.count=1", "-test.timeout=20m"],
                                   cwd=upstream / "tsc/internal/testrunner", env=run_env, capture_output=True, check=False)
        if not (scratch / "summary.json").exists():
            raise ValueError("driver run did not complete:\n" + (completed.stdout + completed.stderr).decode(errors="replace")[-4000:])
        summary = strict_json_loads((scratch / "summary.json").read_bytes())
        rows = [strict_json_loads(line) for line in (scratch / "rows.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in rows] != [case["id"] for case in cases]:
        raise ValueError("driver rows are missing, extra or reordered")
    results = []
    for case, row in zip(cases, rows, strict=True):
        if row["state"] != "executed":
            raise ValueError(f"case {case['id']} failed natively: {row.get('message')}\n"
                             + completed.stdout.decode(errors="replace")[-4000:])
        if len(row["steps"]) != len(case["steps"]):
            raise ValueError("driver steps are missing or extra: " + case["id"])
        steps = []
        for request, step in zip(case["steps"], row["steps"], strict=True):
            # The pin writes an incremental emit's files from a map in random
            # order; the written set is the observation.
            outputs = sorted(({"name": output["name"], **text(output["text_hex"])} for output in step["outputs"]),
                             key=lambda output: output["name"])
            emits = [None if emit is None else {"emit_skipped": emit["emit_skipped"], "emitted_files": emit["emitted_files"],
                                               "diagnostics": diagnostics(emit["diagnostics"])}
                     for emit in step["emits"]]
            steps.append({"request": request, "loading": step["loading"], "actions": step["actions"], "emits": emits,
                          "diagnostics": [diagnostics(values) for values in step["diagnostics"]], "outputs": outputs})
        entry = {"id": case["id"], "name": case["name"], "error_inputs": row["error_inputs"], "steps": steps}
        if "corpus" in case:
            entry["corpus"] = case["corpus"]
        else:
            entry["source"] = case["source"]
        results.append(entry)
    return {"version": 1, "pin": pin(), "overlay_sha256": identity, "go": summary["go"],
            "compiler_version": summary["version"], "affects_build_info": summary["affects_build_info"], "cases": results}


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
    args = parser.parse_args()
    document = run(read_requests(args.requests, verified_upstream().resolve()))
    if args.command == "capture":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(render(document))
        build_infos = sum(output["name"].endswith(".tsbuildinfo") for case in document["cases"]
                          for step in case["steps"] for output in step["outputs"])
        print(json.dumps({"cases": len(document["cases"]), "build_infos": build_infos}))
    elif args.native.read_bytes() != render(document):
        raise ValueError("the native fixture is stale or was edited; rerun capture and review the diff")
    else:
        print(json.dumps({"cases": len(document["cases"]), "current": True}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 incremental failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
