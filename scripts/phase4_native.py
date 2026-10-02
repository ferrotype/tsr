#!/usr/bin/env python3
"""Build pinned native CLIs, then run Phase 4 smoke and build-state witnesses.

`build --output DIR` exports the Go pin and builds release tsrust. `run --build
DIR --output DIR` verifies both images and their source closure before running.
Every subprocess has a deadline and leaves its raw streams and invocation.
The smoke timings are one bounded observation, not a performance gate.
"""
from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase4_corpus as corpus
import phase4_live as live
from s04 import go_environment, verified_upstream
from s08_oracle import ROOT, canonical, digest

FAMILIES = ("incremental", "composite", "projectReferences", "noEmit", "noEmitOnError", "noCheck")
SOURCE_HELPERS = ("scripts/phase4_native.py", "scripts/phase4_live.py", "scripts/phase4_corpus.py",
                  "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                  "scripts/s08_oracle.py", "scripts/tracking-bootstrap.py", "data/upstream.json",
                  "data/s04/toolchains.toml", "data/phase4/scenarios.json.gz")


def sources():
    return {**corpus.sources(), **{name: digest((ROOT / name).read_bytes()) for name in SOURCE_HELPERS}}


def write(path, value):
    path.write_bytes(canonical(value) + b"\n")


def invoke(command, cwd, directory, *, env=None, timeout=600):
    directory.mkdir(parents=True)
    write(directory / "invocation.json", {"command": list(map(str, command)), "cwd": str(cwd),
                                         "timeout_seconds": timeout})
    started = time.monotonic()
    with (directory / "stdout").open("wb") as out, (directory / "stderr").open("wb") as err:
        try:
            child = subprocess.run(list(map(str, command)), cwd=cwd, env=env, stdout=out,
                                   stderr=err, timeout=timeout, check=False)
            status, timed_out = child.returncode, False
        except subprocess.TimeoutExpired:
            status, timed_out = None, True
    result = {"status": status, "timeout": timed_out, "seconds": time.monotonic() - started}
    write(directory / "result.json", result)
    return result


def artifacts(directory):
    return {p.relative_to(directory).as_posix(): digest(p.read_bytes())
            for p in sorted(directory.rglob("*")) if p.is_file() and p.name != "report.json"}


def build(directory):
    directory.mkdir(parents=True, exist_ok=False)
    before = sources()
    upstream = verified_upstream()
    pin = json.loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    selected = ["tsc/go.mod", "tsc/go.sum", "tsc/internal", "tsc/cmd/tsc",
                "tsc/testdata/fixtures/compiler", "tsc/testdata/fixtures/tsconfig-base.json"]
    raw = subprocess.check_output(["git", "archive", pin, *selected], cwd=upstream)
    export = directory / "export"
    export.mkdir()
    with tarfile.open(fileobj=io.BytesIO(raw)) as stream:
        stream.extractall(export, filter="data")
    env = go_environment()
    env["CGO_ENABLED"] = "1"  # The pinned FSEvents implementation uses cgo.
    go_image = directory / "go-executable"
    go_command = ["go", "build", "-mod=readonly", "-o", go_image, "./cmd/tsc"]
    if invoke(go_command, export / "tsc", directory / "go-build", env=env)["status"] != 0:
        raise ValueError("native Go build failed; see go-build/stderr")
    command = ["cargo", "build", "--locked", "--release", "-p", "tsr", "--bin", "tsrust",
               "--message-format=json"]
    if invoke(command, ROOT, directory / "rust-build")["status"] != 0:
        raise ValueError("native Rust build failed; see rust-build/stderr")
    binaries = set()
    for line in (directory / "rust-build/stdout").read_bytes().splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "tsrust" and event.get("executable"):
            binaries.add(event["executable"])
    if len(binaries) != 1 or sources() != before:
        raise ValueError("ambiguous binary or sources changed during native build")
    shutil.copy2(next(iter(binaries)), directory / "rust-executable")
    # The staging name is deliberately separate from the cargo-install target.
    (directory / "lib").mkdir()
    shutil.copy2(directory / "rust-executable", directory / "lib/tsc")
    report = {"version": 1, "pin": pin, "sources": before, "go_export_sha256": digest(raw),
              "go_environment": {k: env[k] for k in ("CGO_ENABLED", "GOTOOLCHAIN", "GOWORK", "GOFLAGS")},
              "rust_environment": {k: v for k, v in os.environ.items() if k.startswith("CARGO_PROFILE_")
                                   or k in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET")},
              "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
              "go": subprocess.check_output(["go", "version"], env=env, text=True),
              "artifacts": artifacts(directory)}
    write(directory / "report.json", report)
    return report


def verify_build(directory):
    report = json.loads((directory / "report.json").read_bytes())
    if report.get("version") != 1 or report.get("sources") != sources():
        raise ValueError("native binary build is stale")
    if report.get("pin") != json.loads((ROOT / "data/upstream.json").read_bytes())["pin"]:
        raise ValueError("native build uses another pin")
    if report.get("artifacts") != artifacts(directory):
        raise ValueError("native build artifacts changed")
    return report


def result(command, root, directory):
    outcome = invoke(command, root, directory)
    return {"status": outcome["status"], "timeout": outcome["timeout"],
            "stdout": live.normalized((directory / "stdout").read_bytes(), root).hex(),
            "stderr": live.normalized((directory / "stderr").read_bytes(), root).hex(),
            "files": live.outputs(root)}


def interop_project(root, family):
    root.mkdir(parents=True)
    live.put(root, "lib.d.ts", corpus.read_document()["library_text"])
    live.put(root, "src/a.ts", "export const value: number = 1;\n")
    live.put(root, "src/main.ts", 'import { value } from "./a"; export const answer: number = value;\n')
    options = {"noLib": True, "strict": True, "module": "commonjs", "incremental": True,
               "declaration": True, "outDir": "out", "tsBuildInfoFile": "out/state.tsbuildinfo"}
    if family == "composite" or family == "projectReferences":
        options["composite"] = True
    elif family in ("noEmit", "noEmitOnError", "noCheck"):
        options[family] = True
    config = {"compilerOptions": options, "files": ["lib.d.ts", "src/a.ts", "src/main.ts"]}
    if family == "projectReferences":
        live.put(root, "dep/index.ts", "export interface Shape { value: number }\n")
        live.put(root, "dep/tsconfig.json", json.dumps({"compilerOptions": {
            "noLib": True, "composite": True, "outDir": "../dep-out"}, "files": ["../lib.d.ts", "index.ts"]}))
        config["references"] = [{"path": "dep"}]
        live.put(root, "src/main.ts", 'import { value } from "./a"; import { Shape } from "../dep";\nexport const answer: Shape = { value };\n')
    live.put(root, "tsconfig.json", json.dumps(config))


def interop_edit(root, index, family):
    if index == 1:
        live.put(root, "src/a.ts", 'export const value: number = "bad";\n')
    elif index == 2:
        live.put(root, "src/a.ts", "export const value: number = 3;\n")
    elif index == 3:
        config = json.loads((root / "tsconfig.json").read_text())
        if family == "noCheck":
            config["compilerOptions"]["noCheck"] = False
        elif family == "noEmit":
            config["compilerOptions"]["noEmit"] = False
        else:
            config["compilerOptions"]["sourceMap"] = True
        live.put(root, "tsconfig.json", json.dumps(config))


def interoperability(binaries, directory):
    rows = {}
    for family in FAMILIES:
        observations = {}
        for mode, order in (("go", ("go",) * 5), ("rust", ("rust",) * 5),
                            ("go-rust", ("go", "go", "rust", "rust", "rust")),
                            ("rust-go", ("rust", "rust", "go", "go", "go"))):
            project = directory / family / mode / "project"
            interop_project(project, family)
            commands = ["-b"] if family == "projectReferences" else ["-p", "."]
            observations[mode] = []
            for i, runtime in enumerate(order):
                interop_edit(project, i, family)
                observation = result([binaries[runtime], *commands, "--pretty", "false"], project,
                                     directory / family / mode / f"step-{i}")
                observations[mode].append(observation)
        good = (all(v == observations["go"] for v in observations.values())
                and all(not v["timeout"] for v in observations["go"])
                and any(name.endswith(".tsbuildinfo") for name in observations["go"][0]["files"]))
        rows[family] = {"pass": good, "observations": observations}
        print(f"build-state {family}: {'pass' if good else 'different'}", flush=True)
    return rows


def run(build_dir, directory, selected):
    record = verify_build(build_dir)
    directory.mkdir(parents=True, exist_ok=False)
    before = sources()
    binaries = {runtime: build_dir / (runtime + "-executable") for runtime in ("go", "rust")}
    report = {"version": 1, "build_sha256": digest((build_dir / "report.json").read_bytes()),
              "sources": before, "images": {k: digest(v.read_bytes()) for k, v in binaries.items()}, "witnesses": {}}
    if "interop" in selected:
        report["witnesses"]["interop"] = interoperability(binaries, directory / "interop")
        write(directory / "report.json", report)
    if "smoke" in selected:
        fixture = build_dir / "export/tsc/testdata/fixtures/compiler"
        smoke = {}
        for mode, flags in (("single", ["--singleThreaded"]), ("parallel", [])):
            observations = {}
            for runtime, image in binaries.items():
                out = directory / "smoke" / mode / runtime
                command = [image, "-p", fixture, "--noEmit", *flags]
                execution = invoke(command, directory, out)
                observations[runtime] = {**execution, "stdout": (out / "stdout").read_bytes().hex(),
                                         "stderr": (out / "stderr").read_bytes().hex()}
            go, rust = observations["go"], observations["rust"]
            smoke[mode] = {"pass": go["status"] == rust["status"] == 0 and all(
                go[k] == rust[k] for k in ("stdout", "stderr", "timeout")) and not go["timeout"],
                "observations": observations}
            print(f"smoke {mode}: {'pass' if smoke[mode]['pass'] else 'different'}", flush=True)
        report["witnesses"]["smoke"] = smoke
    report["source_stable"] = sources() == before
    report["pass"] = report["source_stable"] and all(row["pass"] for group in report["witnesses"].values() for row in group.values())
    report["artifacts"] = artifacts(directory)
    write(directory / "report.json", report)
    return 0 if report["pass"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "run"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build", type=Path)
    parser.add_argument("--only", action="append", choices=("smoke", "interop"))
    args = parser.parse_args()
    if args.command == "build":
        build(args.output.resolve())
        return 0
    if not args.build:
        parser.error("run requires --build")
    return run(args.build.resolve(), args.output.resolve(), args.only or ("smoke", "interop"))


if __name__ == "__main__":
    raise SystemExit(main())
