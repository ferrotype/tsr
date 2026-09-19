#!/usr/bin/env python3
"""Build or run bounded one-worker A0 attribution; no acceptance metrics."""

import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import shutil
from statistics import median
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE.parent))
import runner
from s04_common import command, strict_json_loads
from s07_benchmark import (CACHE, cargo_configuration_paths, cargo_executable,
                           native_environment, release_configuration,
                           rust_native_toolchain, source_fingerprint)
from s07_benchmark_measure import host_info, reject_concurrent_builds

BACKENDS = ("published", "consuming")
COUNTERS = ("files", "loaded_bytes", "nodes", "symbols", "parse_diagnostics", "bind_diagnostics")
TIMERS = ("parse_ns", "bind_and_publication_ns")


def tools_fingerprint():
    files = {str(path.relative_to(ROOT)): runner.digest(path)
             for path in HERE.rglob("*") if path.is_file() and path.suffix in (".py", ".rs", ".toml", ".lock")}
    files[str(HERE.parent.relative_to(ROOT) / "runner.py")] = runner.digest(HERE.parent / "runner.py")
    files["tools/s07/cpu-profile/build.py"] = runner.digest(ROOT / "tools/s07/cpu-profile/build.py")
    return files


def registry_lock():
    # Reuse the checked standalone/workspace registry-closure comparison, with
    # a private helper module whose manifest location is this diagnostic crate.
    specification = importlib.util.spec_from_file_location("phase_registry_helpers", ROOT / "tools/s07/cpu-profile/build.py")
    helper = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(helper)
    helper.RUST = HERE
    return helper.registry_lock()


def configuration(env):
    return {str(path): runner.digest(path) if path.is_file() else None
            for path in cargo_configuration_paths(env, HERE)}


def build(output, control, control_sha):
    # A semantic graph capture may run alongside this build. Timing cannot:
    # hold the same exclusion lock for the complete build, not just a ps check.
    with (CACHE / "s07-benchmark/measurement.lock").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return build_locked(output, control, control_sha)


def build_locked(output, control, control_sha):
    reference = runner.validate_bundle(control, control_sha)
    runner.validate_inputs(control, reference["expected_work"])
    output.mkdir(parents=True, exist_ok=False)
    (output / "artifacts").mkdir()
    env = native_environment()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target/s07-bis-phase-build")
    stable, host = rust_native_toolchain(env)
    before = {"source_fingerprint": source_fingerprint(), "tool_inputs": tools_fingerprint(),
              "cargo_configuration": configuration(env), "registry_lock": registry_lock()}
    argv = ["cargo", "+" + stable, "build", "--release", "--offline", "--locked",
            "--manifest-path", str(HERE / "Cargo.toml"), "--bin", "tsr_s07_bis_phases",
            "--target", host, "--message-format=json-render-diagnostics", *release_configuration(env, HERE)]
    messages = command(argv, cwd=HERE, env=env)
    (output / "cargo-messages.ndjson").write_bytes(messages)
    binary = cargo_executable(messages, HERE / "Cargo.toml", "tsr_s07_bis_phases", "bin", [])
    frozen = output / "artifacts/tsr_s07_bis_phases"
    shutil.copyfile(binary, frozen)
    runner.runtime_libraries(frozen, output / "runtime-libraries.txt")
    runner.snapshot_sources(output, before["source_fingerprint"])
    shutil.copyfile(control / "inputs.json", output / "inputs.json")
    after = {"source_fingerprint": source_fingerprint(), "tool_inputs": tools_fingerprint(),
             "cargo_configuration": configuration(env), "registry_lock": registry_lock()}
    if before != after:
        raise ValueError("phase build sources/configuration changed")
    manifest = {"version": 1, "kind": "s07_bis_same_revision_phases", "diagnostic_only": True,
        **before, "expected_work": reference["expected_work"], "control_manifest_sha256": control_sha,
        "backend_policy": "one binary, one source revision, published and consuming selection at runtime",
        "profile": "release; panic=unwind; fat LTO; one codegen unit; tsr_ast/layout-profile enabled in both modes",
        "artifact": {"path": "artifacts/tsr_s07_bis_phases", "sha256": runner.digest(frozen)},
        "rustc": command(["rustc", "+" + stable, "-vV"], cwd=ROOT, env=env).decode(),
        "command": argv, "revision": command(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip()}
    return runner.seal(output, manifest)


def validate_build(directory, expected_sha):
    if runner.digest(directory / "manifest.json") != expected_sha:
        raise ValueError("phase build manifest changed")
    manifest = strict_json_loads((directory / "manifest.json").read_bytes())
    if manifest.get("kind") != "s07_bis_same_revision_phases" or manifest.get("diagnostic_only") is not True:
        raise ValueError("not a phase diagnostic build")
    if runner.inventory(directory) != manifest["inventory"]:
        raise ValueError("phase build inventory changed")
    for path in (directory, *directory.rglob("*")):
        if path.is_symlink() or path.stat().st_mode & 0o222:
            raise ValueError("phase build is not immutable")
    binary = directory / manifest["artifact"]["path"]
    if not binary.resolve().is_relative_to(directory) or runner.digest(binary) != manifest["artifact"]["sha256"]:
        raise ValueError("phase artifact changed")
    if not os.access(binary, os.X_OK):
        raise ValueError("phase artifact is not executable")
    return manifest


def validate_observation(value, expected, backend, shapes):
    fields = {"version", "runtime", "diagnostic_only", "backend", "workers", *COUNTERS,
              "loaded_input_sha256", "bound_in_place_files", "published_files", "shapes_emitted",
              "pipeline_wall_ns", "elapsed_worker_totals", "timer_domain"}
    if type(value) is not dict or set(value) != fields:
        raise ValueError("invalid phase report fields")
    if (type(value["version"]) is not int or value["version"] != 1
            or type(value["workers"]) is not int or value["workers"] != 1
            or value["runtime"] != "rust" or value["diagnostic_only"] is not True
            or value["backend"] != backend or value["shapes_emitted"] is not shapes):
        raise ValueError("wrong diagnostic backend/domain")
    for key in COUNTERS:
        if type(value[key]) is not int or value[key] != expected[key]:
            raise ValueError("phase workload mismatch: " + key)
    if value["loaded_input_sha256"] != expected["loaded_input_sha256"]:
        raise ValueError("phase child loaded different bytes/options")
    for key in ["bound_in_place_files", "published_files", "pipeline_wall_ns"]:
        if type(value[key]) is not int or value[key] < 0:
            raise ValueError("invalid phase counter: " + key)
    if value["bound_in_place_files"] + value["published_files"] != value["files"]:
        raise ValueError("binding paths lost files")
    if backend == "published" and value["bound_in_place_files"]:
        raise ValueError("published backend cannot bind in place")
    totals = value["elapsed_worker_totals"]
    if type(totals) is not dict or set(totals) != set(TIMERS):
        raise ValueError("phase labels must group binding and publication")
    if any(type(totals[key]) is not int or totals[key] < 0 for key in TIMERS):
        raise ValueError("invalid elapsed timer")
    if not 0 < sum(totals.values()) <= value["pipeline_wall_ns"] <= 600_000_000_000:
        raise ValueError("phase intervals exceed whole pipeline")
    return value


def validate_shapes(path, expected):
    rows = [strict_json_loads(line) for line in path.read_bytes().splitlines()]
    if len(rows) != expected["files"]:
        raise ValueError("shape census lost files")
    total, exclusive = 0, 0
    for index, row in enumerate(rows):
        if (set(row) != {"index", "core_shapes", "bound_in_place"}
                or type(row["index"]) is not int or row["index"] != index
                or type(row["bound_in_place"]) is not bool or type(row["core_shapes"]) is not dict):
            raise ValueError("invalid per-file shape record")
        for name, count in row["core_shapes"].items():
            if type(name) is not str or type(count) is not int or count < 1:
                raise ValueError("invalid core shape count")
            total += count
        exclusive += row["bound_in_place"]
    if total != expected["nodes"]:
        raise ValueError("owned core shapes do not match allocated node obligations")
    return {"files": len(rows), "core_nodes": total, "bound_in_place_files": exclusive}


def order():
    yield from ((True, 0, backend) for backend in BACKENDS)
    for index in range(7):
        yield from ((False, index, backend) for backend in (BACKENDS if index % 2 == 0 else BACKENDS[::-1]))


def capture(directory, build_sha, output, shapes_only):
    manifest = validate_build(directory, build_sha)
    runner.validate_inputs(directory, manifest["expected_work"])
    output.mkdir(parents=True, exist_ok=False)
    binary = directory / manifest["artifact"]["path"]
    with (CACHE / "s07-benchmark/measurement.lock").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        before = {"current_source": source_fingerprint(), "tool_inputs": tools_fingerprint()}
        metadata = {"version": 1, "diagnostic_only": True, "build_manifest_sha256": build_sha,
                    "build_directory": str(directory), "host": host_info(), **before}
        runner.write_json(output / "report.json", {**metadata, "status": "in_progress"})
        rows = []
        try:
            schedule = [(False, 0, "consuming")] if shapes_only else list(order())
            for warmup, index, backend in schedule:
                # The shape census is untimed and may coexist with semantic
                # graph/test work. Keep the measurement lock and all identity
                # checks so it cannot overlap an actual timing screen.
                if not shapes_only:
                    reject_concurrent_builds()
                validate_build(directory, build_sha)
                stem = f"{'warmup' if warmup else 'sample'}-{index}-{backend}"
                argv = [str(binary), str(directory / "inputs.json"), backend]
                if shapes_only:
                    argv.extend(["--shapes", str(output / "core-shapes.ndjson")])
                child = subprocess.run(argv, cwd=ROOT, env=native_environment(), capture_output=True, check=False, timeout=600)
                (output / (stem + ".stdout")).write_bytes(child.stdout)
                (output / (stem + ".stderr")).write_bytes(child.stderr)
                if child.returncode:
                    raise ValueError("phase child failed; raw output retained: " + stem)
                report = validate_observation(strict_json_loads(child.stdout), manifest["expected_work"], backend, shapes_only)
                rows.append({"warmup": warmup, "index": index, "backend": backend, "report": report})
                runner.write_json(output / "observations.json", rows)
            validate_build(directory, build_sha)
            runner.validate_inputs(directory, manifest["expected_work"])
            if before != {"current_source": source_fingerprint(), "tool_inputs": tools_fingerprint()}:
                raise ValueError("source or phase helper changed during capture")
            summary = {}
            if shapes_only:
                summary = {"core_shapes": validate_shapes(output / "core-shapes.ndjson", manifest["expected_work"]),
                           "core_shapes_sha256": runner.digest(output / "core-shapes.ndjson")}
            else:
                for timer in (*TIMERS, "pipeline_wall_ns"):
                    values = {backend: [row["report"][timer] if timer == "pipeline_wall_ns" else row["report"]["elapsed_worker_totals"][timer]
                                        for row in rows if row["backend"] == backend and not row["warmup"]]
                              for backend in BACKENDS}
                    medians = {backend: median(values[backend]) for backend in BACKENDS}
                    summary[timer] = {"values": values, "medians": medians,
                                      "consuming_over_published": medians["consuming"] / medians["published"]}
            raw = {path.name: runner.digest(path) for path in output.iterdir() if path.suffix in (".stdout", ".stderr")}
            result = {**metadata, "status": "complete", "kind": "owned_core_shapes" if shapes_only else "phase_attribution",
                      "summary": summary, "raw_sha256": raw, "observations_sha256": runner.digest(output / "observations.json"),
                      "limitations": ["No acceptance metrics or performance promotion; compare final normal binaries separately.",
                          "Both backends use the current revision, not the frozen original control implementation.",
                          "Per-file timers include descheduling and explicit timer overhead; binding/publication are inseparable here.",
                          "No allocation, lifetime RSS, sampled CPU or per-shape capacity measurement is produced."]}
            runner.write_json(output / "report.json", result)
            return result
        except BaseException as error:
            runner.write_json(output / "report.json", {**metadata, "status": "failed", "error": str(error)})
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    builder = commands.add_parser("build")
    builder.add_argument("--control", type=Path, required=True)
    builder.add_argument("--control-sha", required=True)
    builder.add_argument("--output", type=Path, required=True)
    for name in ["capture", "shapes"]:
        child = commands.add_parser(name)
        child.add_argument("--build", type=Path, required=True)
        child.add_argument("--build-sha", required=True)
        child.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    for key, value in vars(args).items():
        if isinstance(value, Path):
            setattr(args, key, value.resolve())
    if args.operation == "build":
        result = {"build_manifest_sha256": build(args.output, args.control, args.control_sha)}
    else:
        result = capture(args.build, args.build_sha, args.output, args.operation == "shapes")
    print(json.dumps(result, sort_keys=True, allow_nan=False))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print("S07-bis phase probe failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
