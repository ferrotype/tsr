#!/usr/bin/env python3
"""Build pinned native CLIs, then run Phase 4 smoke and build-state witnesses.

`build --output DIR` exports the Go pin and builds release tsrust. `run --build
DIR --output DIR` verifies both images and their source closure before running.
Every subprocess has a deadline and leaves its raw streams and invocation.
The smoke timings are one bounded observation, not a performance gate.
"""
from __future__ import annotations

import argparse
import functools
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase4_corpus as corpus
import phase4_live as live
from s04 import go_environment, verified_upstream
from s04_common import strict_json_loads
from s08_oracle import ROOT, canonical, digest

FAMILIES = ("incremental", "composite", "projectReferences", "noEmit", "noEmitOnError", "noCheck")
SOURCE_HELPERS = ("scripts/phase4_native.py", "scripts/phase4_live.py", "scripts/phase4_corpus.py",
                  "scripts/phase4_scenarios.py",
                  "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                  "scripts/s08_oracle.py", "scripts/tracking-bootstrap.py", "data/upstream.json",
                  "data/s04/toolchains.toml", "data/phase4/scenarios.json.gz")
EXPORT_PATHS = ("tsc/go.mod", "tsc/go.sum", "tsc/internal", "tsc/cmd/tsc",
                "tsc/testdata/fixtures/compiler", "tsc/testdata/fixtures/tsconfig-base.json")
ORDERS = {"go": ("go",) * 5, "rust": ("rust",) * 5,
          "go-rust": ("go", "go", "rust", "rust", "rust"),
          "rust-go": ("rust", "rust", "go", "go", "go")}


def rust_command(repo_root=ROOT):
    return ["cargo", "build", "--locked", "--release", "-p", "tsr", "--bin", "tsrust",
            "--message-format=json", "--target-dir", str(repo_root / "target")]


def malformed_as_value_error(function):
    @functools.wraps(function)
    def wrapped(*args, **kwargs):
        try:
            return function(*args, **kwargs)
        except (OSError, KeyError, TypeError, AttributeError, IndexError, UnicodeError,
                tarfile.TarError, subprocess.SubprocessError) as error:
            raise ValueError("malformed or incomplete native artifacts") from error
    return wrapped


def recorded_root(value):
    if (not isinstance(value, str) or not value or not Path(value).is_absolute()
            or value == "/" or str(Path(value)) != value or ".." in Path(value).parts or "\\" in value
            or value.startswith("//")):
        raise ValueError("malformed recorded artifact root")
    return Path(value)


def host():
    return {"os": {"Darwin": "macos", "Linux": "linux"}.get(platform.system(), platform.system()),
            "arch": platform.machine()}


def check_host(value, expected=None):
    if (not isinstance(value, dict) or set(value) != {"os", "arch"}
            or value["os"] not in ("macos", "linux") or not isinstance(value["arch"], str)
            or not value["arch"]):
        raise ValueError("malformed native host")
    if expected is not None and (value != expected if isinstance(expected, dict) else value["os"] != expected):
        raise ValueError("native evidence belongs to another host")


def read_json(path):
    try:
        if Path(path).is_symlink():
            raise ValueError("symlink in captured JSON artifact")
        value = strict_json_loads(Path(path).read_bytes())
    except (OSError, UnicodeError) as error:
        raise ValueError(f"missing or unreadable artifact: {path}") from error
    return value


def safe_relative(name):
    if (not isinstance(name, str) or not name or Path(name).is_absolute()
            or "\\" in name or any(p in ("", ".", "..") for p in name.split("/"))):
        raise ValueError("unsafe artifact path")
    return name


def files(directory):
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("symlink in captured artifacts")
        if path.is_file():
            result[path.relative_to(directory).as_posix()] = path.read_bytes()
    return result


def verify_artifacts(directory, recorded):
    if not isinstance(recorded, dict):
        raise ValueError("missing artifact inventory")
    for name, checksum in recorded.items():
        safe_relative(name)
        if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
            raise ValueError("malformed artifact digest")
    if recorded != artifacts(directory):
        raise ValueError("native artifacts changed or are incomplete")


def check_layout(directory, fixed, trees=()):
    for path in directory.rglob("*"):
        if path.is_file():
            name = path.relative_to(directory).as_posix()
            if name != "report.json" and name not in fixed and not any(name.startswith(tree + "/") for tree in trees):
                raise ValueError("unexpected artifact outside the executed witness inventory: " + name)


def execution(directory, command, cwd):
    invocation = read_json(directory / "invocation.json")
    if invocation != {"command": list(map(str, command)), "cwd": str(cwd), "timeout_seconds": 600}:
        raise ValueError("unexpected native invocation")
    outcome = read_json(directory / "result.json")
    if (not isinstance(outcome, dict) or set(outcome) != {"status", "timeout", "seconds"}
            or type(outcome["timeout"]) is not bool
            or type(outcome["seconds"]) not in (int, float) or outcome["seconds"] < 0
            or (outcome["status"] is not None and type(outcome["status"]) is not int)
            or (outcome["status"] is None) != outcome["timeout"]):
        raise ValueError("malformed native process outcome")
    for name in ("stdout", "stderr"):
        if not (directory / name).is_file():
            raise ValueError("missing native process stream")
    return outcome


def pinned_export():
    pin = read_json(ROOT / "data/upstream.json")["pin"]
    return subprocess.check_output(["git", "archive", pin, *EXPORT_PATHS], cwd=verified_upstream())


def archive_files(raw):
    result = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as stream:
        for member in stream:
            safe_relative(member.name.rstrip("/"))
            if member.isfile():
                if member.name in result:
                    raise ValueError("duplicate pinned archive path")
                result[member.name] = stream.extractfile(member).read()
            elif not member.isdir():
                raise ValueError("unsupported pinned archive entry")
    return result


def cargo_binary(directory, repo_root=ROOT):
    binaries = []
    finished = []
    for line in (directory / "rust-build/stdout").read_bytes().splitlines():
        event = strict_json_loads(line)
        if not isinstance(event, dict):
            raise ValueError("malformed cargo event")
        if event.get("reason") == "build-finished":
            finished.append(event.get("success"))
        if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "tsrust":
            target, profile = event["target"], event.get("profile", {})
            if (target.get("kind") != ["bin"] or target.get("src_path") != str(repo_root / "crates/tsr/src/main.rs")
                    or profile.get("opt_level") != "3" or profile.get("debug_assertions") is not False
                    or profile.get("test") is not False
                    or event.get("executable") != str(repo_root / "target/release/tsrust")):
                raise ValueError("unexpected release executable provenance")
            binaries.append(event["executable"])
    if len(finished) != 1 or finished[0] is not True or len(binaries) != 1:
        raise ValueError("incomplete or ambiguous release build")
    return Path(binaries[0])


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
    return {name: digest(raw) for name, raw in files(directory).items() if name != "report.json"}


def rust_environment():
    return {k: v for k, v in os.environ.items() if k.startswith("CARGO_PROFILE_")
            or k in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "RUSTC",
                     "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")}


def verify_tools(directory, report):
    recorded_host = report["host"]
    versions = {}
    for name, command in (("go", ["go", "version"]), ("rustc", ["rustc", "-Vv"]),
                          ("cargo", ["cargo", "--version"])):
        outcome = execution(directory / (name + "-version"), command, recorded_root(report["repo_root"]))
        if outcome["status"] != 0 or outcome["timeout"] or (directory / (name + "-version/stderr")).read_bytes():
            raise ValueError("unsuccessful toolchain observation")
        versions[name] = (directory / (name + "-version/stdout")).read_text()
    if report.get("tools") != versions:
        raise ValueError("toolchain versions disagree with raw output")
    rust_pin = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    go_pin = tomllib.loads((ROOT / "data/s04/toolchains.toml").read_text())["go"]
    rust_arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(recorded_host["arch"])
    go_arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64"}.get(recorded_host["arch"])
    rust_os = {"macos": "apple-darwin", "linux": "unknown-linux-gnu"}[recorded_host["os"]]
    go_os = {"macos": "darwin", "linux": "linux"}[recorded_host["os"]]
    if (not rust_arch or not go_arch or not versions["rustc"].startswith("rustc " + rust_pin + " ")
            or f"host: {rust_arch}-{rust_os}\n" not in versions["rustc"]
            or f"release: {rust_pin}\n" not in versions["rustc"]
            or not versions["cargo"].startswith("cargo " + rust_pin + " ")
            or versions["go"] != f"go version {go_pin} {go_os}/{go_arch}\n"):
        raise ValueError("unrecognized toolchain pin or build host")


def build(directory):
    directory.mkdir(parents=True, exist_ok=False)
    before = sources()
    if rust_environment():
        raise ValueError("native acceptance build requires the pinned default release configuration")
    pin = read_json(ROOT / "data/upstream.json")["pin"]
    raw = pinned_export()
    archive_files(raw)
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
    if invoke(rust_command(), ROOT, directory / "rust-build")["status"] != 0:
        raise ValueError("native Rust build failed; see rust-build/stderr")
    binary = cargo_binary(directory)
    if sources() != before:
        raise ValueError("sources changed during native build")
    shutil.copy2(binary, directory / "rust-executable")
    # The staging name is deliberately separate from the cargo-install target.
    (directory / "lib").mkdir()
    shutil.copy2(directory / "rust-executable", directory / "lib/tsc")
    versions = {}
    for name, command in (("go", ["go", "version"]), ("rustc", ["rustc", "-Vv"]),
                          ("cargo", ["cargo", "--version"])):
        if invoke(command, ROOT, directory / (name + "-version"), env=env if name == "go" else None)["status"] != 0:
            raise ValueError("cannot observe native build toolchain")
        versions[name] = (directory / (name + "-version/stdout")).read_text()
    if invoke(["go", "version", "-m", go_image], ROOT, directory / "go-image", env=env)["status"] != 0:
        raise ValueError("cannot observe Go executable build metadata")
    report = {"version": 2, "pin": pin, "host": host(), "sources": before, "go_export_sha256": digest(raw),
              "repo_root": str(ROOT), "directory": str(directory),
              "go_environment": {k: env[k] for k in ("CGO_ENABLED", "GOTOOLCHAIN", "GOWORK", "GOFLAGS")},
              "rust_environment": {}, "tools": versions,
              "images": {runtime: digest((directory / (runtime + "-executable")).read_bytes())
                         for runtime in ("go", "rust")},
              "rust_binary": str(binary),
              "artifacts": artifacts(directory)}
    verify_tools(directory, report)
    if sources() != before:
        raise ValueError("sources changed during native build")
    write(directory / "report.json", report)
    return report


@malformed_as_value_error
def verify_build(directory, *, expected_host=None):
    directory = Path(directory).resolve()
    report = read_json(directory / "report.json")
    if (not isinstance(report, dict) or type(report.get("version")) is not int
            or report["version"] != 2 or report.get("sources") != sources()):
        raise ValueError("native binary build is stale")
    check_host(report.get("host"), expected_host)
    repo_root = recorded_root(report.get("repo_root"))
    build_root = recorded_root(report.get("directory"))
    if report.get("pin") != read_json(ROOT / "data/upstream.json")["pin"]:
        raise ValueError("native build uses another pin")
    verify_artifacts(directory, report.get("artifacts"))
    check_layout(directory, {"go-executable", "rust-executable", "lib/tsc"} |
                 {f"{group}/{name}" for group in ("go-build", "rust-build", "go-version", "rustc-version", "cargo-version", "go-image")
                  for name in ("invocation.json", "result.json", "stdout", "stderr")}, ("export",))
    raw = pinned_export()
    if report.get("go_export_sha256") != digest(raw) or files(directory / "export") != archive_files(raw):
        raise ValueError("native build export differs from the authenticated pin")
    for name, command, cwd in (("go", ["go", "build", "-mod=readonly", "-o", build_root / "go-executable", "./cmd/tsc"],
                               build_root / "export/tsc"), ("rust", rust_command(repo_root), repo_root)):
        outcome = execution(directory / (name + "-build"), command, cwd)
        if outcome["status"] != 0 or outcome["timeout"]:
            raise ValueError("unsuccessful native build")
    if (report.get("rust_environment") != {} or report.get("go_environment") !=
            {"CGO_ENABLED": "1", "GOTOOLCHAIN": "local", "GOWORK": "off", "GOFLAGS": ""}):
        raise ValueError("unexpected native build environment")
    verify_tools(directory, report)
    if report.get("rust_binary") != str(cargo_binary(directory, repo_root)):
        raise ValueError("release executable disagrees with cargo")
    images = {runtime: digest((directory / (runtime + "-executable")).read_bytes()) for runtime in ("go", "rust")}
    if (report.get("images") != images or digest((directory / "lib/tsc").read_bytes()) != images["rust"]
            or any(not os.access(directory / (runtime + "-executable"), os.X_OK) for runtime in ("go", "rust"))):
        raise ValueError("native executable or staging image differs")
    outcome = execution(directory / "go-image", ["go", "version", "-m", build_root / "go-executable"], repo_root)
    metadata = (directory / "go-image/stdout").read_text()
    go_pin = report["tools"]["go"].split()[2]
    go_os, go_arch = report["tools"]["go"].split()[3].split("/")
    if (outcome["status"] != 0 or outcome["timeout"] or (directory / "go-image/stderr").read_bytes()
            or not metadata.startswith(str(build_root / "go-executable") + ": " + go_pin + "\n")
            or "\tpath\tgithub.com/microsoft/TypeScript/tsc/cmd/tsc\n" not in metadata
            or f"\tbuild\tGOOS={go_os}\n" not in metadata
            or f"\tbuild\tGOARCH={go_arch}\n" not in metadata
            or "\tbuild\tCGO_ENABLED=1\n" not in metadata):
        raise ValueError("Go executable metadata does not match its build")
    return report


def result(command, root, directory):
    outcome = invoke(command, root, directory)
    shutil.copytree(root, directory / "project")
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
        config = strict_json_loads((root / "tsconfig.json").read_bytes())
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
        for mode, order in ORDERS.items():
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
    check_host(record["host"], host())
    report = {"version": 2, "host": host(), "build_sha256": digest((build_dir / "report.json").read_bytes()),
              "repo_root": record["repo_root"], "build_root": str(build_dir), "directory": str(directory),
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


def capture_report(build_dir, directory, *, expected_host=None):
    build_record = verify_build(build_dir, expected_host=expected_host)
    report = read_json(directory / "report.json")
    if (not isinstance(report, dict) or type(report.get("version")) is not int or report["version"] != 2
            or report.get("sources") != sources() or report.get("source_stable") is not True
            or report.get("build_sha256") != digest((build_dir / "report.json").read_bytes())
            or report.get("images") != build_record["images"]):
        raise ValueError("missing, stale or unauthenticated native capture")
    check_host(report.get("host"), build_record["host"])
    recorded_root(report.get("directory"))
    recorded_root(report.get("build_root"))
    if report.get("repo_root") != build_record["repo_root"]:
        raise ValueError("capture uses another recorded source root")
    verify_artifacts(directory, report.get("artifacts"))
    if not isinstance(report.get("witnesses"), dict):
        raise ValueError("missing native witness inventory")
    return report


def check_inputs(snapshot, expected):
    observed = files(snapshot)
    outputs = set(live.outputs(snapshot))
    expected_files = files(expected)
    if {k: v for k, v in observed.items() if k not in outputs} != expected_files:
        raise ValueError("captured project inputs do not match the scripted edit")


@malformed_as_value_error
def verify_witnesses(build_dir, capture_dir, *, expected_host=None):
    """Replay complete selected groups; an absent group supplies no metric."""
    import tempfile
    build_dir, directory = Path(build_dir).resolve(), Path(capture_dir).resolve()
    report = capture_report(build_dir, directory, expected_host=expected_host)
    witnesses = report["witnesses"]
    if set(witnesses) - {"smoke", "interop"}:
        raise ValueError("unknown native witness group")
    metrics = {}
    fixed, trees = set(), set()
    capture_root = recorded_root(report["directory"])
    build_root = recorded_root(report["build_root"])
    binaries = {runtime: build_root / (runtime + "-executable") for runtime in ("go", "rust")}
    if "smoke" in witnesses:
        if not isinstance(witnesses["smoke"], dict) or set(witnesses["smoke"]) != {"single", "parallel"}:
            raise ValueError("incomplete smoke mode inventory")
        passed = True
        for mode, flags in (("single", ["--singleThreaded"]), ("parallel", [])):
            observations = {}
            recorded = witnesses["smoke"][mode]
            if not isinstance(recorded, dict) or set(recorded.get("observations", {})) != {"go", "rust"}:
                raise ValueError("incomplete paired smoke observation")
            for runtime, image in binaries.items():
                out = directory / "smoke" / mode / runtime
                fixed.update(f"smoke/{mode}/{runtime}/{name}" for name in ("invocation.json", "result.json", "stdout", "stderr"))
                command = [image, "-p", build_root / "export/tsc/testdata/fixtures/compiler", "--noEmit", *flags]
                outcome = execution(out, command, capture_root)
                observations[runtime] = {**outcome, "stdout": (out / "stdout").read_bytes().hex(),
                                         "stderr": (out / "stderr").read_bytes().hex()}
            if recorded["observations"] != observations:
                raise ValueError("smoke summary disagrees with raw outcomes")
            go, rust = observations["go"], observations["rust"]
            passed &= (go["status"] == rust["status"] == 0 and not go["timeout"]
                       and all(go[key] == rust[key] for key in ("stdout", "stderr", "timeout")))
        metrics["smoke"] = bool(passed)
    if "interop" in witnesses:
        group = witnesses["interop"]
        if not isinstance(group, dict) or set(group) != set(FAMILIES):
            raise ValueError("incomplete build-info family inventory")
        passed = True
        for family in FAMILIES:
            row = group[family]
            if not isinstance(row, dict) or set(row.get("observations", {})) != set(ORDERS):
                raise ValueError("incomplete build-info mode inventory")
            observations = {}
            for mode, order in ORDERS.items():
                project = directory / "interop" / family / mode / "project"
                original_project = capture_root / "interop" / family / mode / "project"
                trees.add(f"interop/{family}/{mode}/project")
                recorded = row["observations"][mode]
                if not isinstance(recorded, list) or len(recorded) != 5:
                    raise ValueError("incomplete build-info step inventory")
                observations[mode] = []
                with tempfile.TemporaryDirectory(prefix="phase4-interop-replay-") as temporary:
                    expected = Path(temporary) / "project"
                    interop_project(expected, family)
                    for index, runtime in enumerate(order):
                        interop_edit(expected, index, family)
                        step = project.parent / f"step-{index}"
                        prefix = f"interop/{family}/{mode}/step-{index}"
                        fixed.update(prefix + "/" + name for name in ("invocation.json", "result.json", "stdout", "stderr"))
                        trees.add(prefix + "/project")
                        args = ["-b"] if family == "projectReferences" else ["-p", "."]
                        outcome = execution(step, [binaries[runtime], *args, "--pretty", "false"], original_project)
                        check_inputs(step / "project", expected)
                        observed = {"status": outcome["status"], "timeout": outcome["timeout"],
                                    "stdout": live.normalized((step / "stdout").read_bytes(), original_project,
                                                              roots=[original_project]).hex(),
                                    "stderr": live.normalized((step / "stderr").read_bytes(), original_project,
                                                              roots=[original_project]).hex(),
                                    "files": live.outputs(step / "project", normalization_roots=[original_project])}
                        if observed != recorded[index]:
                            raise ValueError("build-info summary disagrees with raw artifacts")
                        observations[mode].append(observed)
                    if files(project) != files(project.parent / "step-4/project"):
                        raise ValueError("build-info final project disagrees with its last observation")
            go = observations["go"]
            passed &= (all(value == go for value in observations.values())
                       and all(not value["timeout"] and value["status"] in (0, 1, 2) for value in go)
                       and any(name.endswith(".tsbuildinfo") for name in go[0]["files"]))
        metrics["buildinfo_interop"] = bool(passed)
    check_layout(directory, fixed, trees)
    identity = digest((directory / "report.json").read_bytes())
    return {"metrics": metrics, "identities": dict.fromkeys(metrics, identity), "details": {"host": report["host"]}}


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
