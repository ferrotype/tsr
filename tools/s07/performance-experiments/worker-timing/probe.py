#!/usr/bin/env python3
"""One frozen Rust/Go worker timing observation each; never an acceptance ratio."""
import argparse
import difflib
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE.parent))
import runner
import patches
from s04 import verified_upstream
from s04_common import strict_json_loads
from s07_benchmark import (CACHE, cargo_configuration_paths, cargo_executable,
                           native_environment, go_native_environment,
                           release_configuration, rust_native_toolchain)
from s07_benchmark_measure import host_info, reject_concurrent_builds, validate_sample

WORKERS = 8
KINDS = ("rust", "go")
DOMAINS = {
    "version": 1, "diagnostic_only": True, "queue_capacity": WORKERS,
    "assignment": "round_robin_index_mod_workers",
    "work_domain": "parse_bind_publication_validation_retention_elapsed",
    "wait_domain": "receive_call_elapsed_clipped_to_pipeline",
    "sender_domain": "send_call_elapsed_including_scheduling_and_queue_blocking",
}
ROW_FIELDS = {"worker", "files", "loaded_bytes", "assignment_sha256", "work_ns", "receive_wait_ns",
              "start_offset_ns", "completion_offset_ns", "tail_ns"}


def tool_inputs():
    paths = sorted(HERE.glob("*.py")) + [HERE / "README.md", HERE.parent / "runner.py"]
    paths += [ROOT / "scripts" / name for name in
              ("s04_common.py", "s04.py", "s04_ownership.py", "s04_runtime.py", "s07_benchmark.py",
               "s07_benchmark_inputs.py", "s07_benchmark_child.py", "s07_benchmark_measure.py")]
    return {str(path.relative_to(ROOT)): runner.digest(path) for path in paths}


def run(argv, cwd, env, output, label):
    # Write intent first, then preserve partial output even on timeout/launch errors.
    receipt = {"argv": argv, "cwd": str(cwd), "status": "in_progress", "exit_code": None}
    command_path = output / (label + ".command.json")
    runner.write_json(command_path, receipt)
    stdout, stderr = b"", b""
    try:
        result = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, check=False, timeout=1800)
        stdout, stderr = result.stdout, result.stderr
        receipt.update(status="complete" if result.returncode == 0 else "failed", exit_code=result.returncode)
    except subprocess.TimeoutExpired as error:
        stdout, stderr = error.stdout or b"", error.stderr or b""
        receipt.update(status="timeout", error=str(error))
        raise
    except OSError as error:
        receipt.update(status="launch_failed", error=str(error))
        raise
    finally:
        (output / (label + ".stdout")).write_bytes(stdout)
        (output / (label + ".stderr")).write_bytes(stderr)
        runner.write_json(command_path, receipt)
    if result.returncode:
        raise ValueError(f"{label} exited {result.returncode}; preserved stdout/stderr in {output}")
    return result.stdout


def assignment_rows(inputs):
    rows = []
    requests = strict_json_loads(inputs.read_bytes())
    for worker in range(WORKERS):
        indices = list(range(worker, len(requests), WORKERS))
        digest = hashlib.sha256(b"S07-worker-assignment-v1\0")
        for index in indices:
            digest.update(index.to_bytes(8, "big"))
        rows.append({"worker": worker, "files": len(indices),
                     "loaded_bytes": sum(Path(requests[index]["local"]).stat().st_size for index in indices),
                     "assignment_sha256": digest.hexdigest()})
    return rows


def registry_entries(lock):
    return {(row["name"], row["version"], row["source"]): row["checksum"]
            for row in tomllib.loads(lock.read_text())["package"] if "source" in row}


def configuration(env, workspace):
    return {str(path): runner.digest(path) if path.is_file() else None
            for path in cargo_configuration_paths(env, workspace)}


def patch_file(path, transform, output):
    before = path.read_text()
    after = transform(before)
    path.write_text(after)
    (output / (path.name + ".patch")).write_text("".join(difflib.unified_diff(
        before.splitlines(keepends=True), after.splitlines(keepends=True),
        fromfile="frozen/" + path.name, tofile="instrumented/" + path.name)))


def build(control, control_sha, output):
    reference = runner.validate_bundle(control, control_sha)
    runner.validate_inputs(control, reference["expected_work"])
    output.mkdir(parents=True, exist_ok=False)
    work = output.parent / (output.name + "-work")
    work.mkdir(exist_ok=False)
    (output / "artifacts").mkdir()
    (output / "patches").mkdir()
    shutil.copytree(control / "source", work / "rust")
    workspace = work / "rust"
    for path in (workspace, *workspace.rglob("*")):
        path.chmod(0o755 if path.is_dir() else 0o644)
    before_tools = tool_inputs()
    # Frozen benchmark closure omits unrelated workspace members. Change only
    # membership and prune the lock offline; every registry version/checksum
    # must remain in the frozen lock before compiling anything.
    manifest = workspace / "Cargo.toml"
    original_manifest = manifest.read_text()
    members = sorted(str(path.parent.relative_to(workspace)) for path in (workspace / "crates").glob("*/Cargo.toml"))
    manifest.write_text("\n".join("members = " + json.dumps(members) if line.startswith("members = ") else line
                                 for line in original_manifest.splitlines()) + "\n")
    parser_manifest = workspace / "crates/tsr_parser/Cargo.toml"
    parser_before = parser_manifest.read_text()
    if tomllib.loads(parser_before)["dev-dependencies"]["tsr_encoder"]["path"] != "../tsr_encoder":
        raise ValueError("frozen parser test-only encoder dependency changed")
    parser_manifest.write_text(patches.replace_once(parser_before,
        'tsr_encoder = { path = "../tsr_encoder", version = "0.1.0" }\n', ""))
    (output / "patches/tsr_parser-Cargo.toml.patch").write_text("".join(difflib.unified_diff(
        parser_before.splitlines(keepends=True), parser_manifest.read_text().splitlines(keepends=True),
        fromfile="frozen/tsr_parser/Cargo.toml", tofile="instrumented/tsr_parser/Cargo.toml")))
    patch_file(workspace / "crates/tsr_bench/src/main.rs", patches.rust, output / "patches")
    patch_file(workspace / "tools/s07/benchmark/main.go", patches.go, output / "patches")
    env = native_environment()
    build_directory = work / "cargo-target"
    build_directory.mkdir()
    env["CARGO_TARGET_DIR"] = str(build_directory)
    stable, host = rust_native_toolchain(env, workspace)
    config_before = configuration(env, workspace)
    overrides = release_configuration(env, workspace)
    run(["cargo", "+" + stable, "metadata", "--offline", "--format-version=1"], workspace, env, output, "rust-metadata")
    original_registry = registry_entries(control / "source/Cargo.lock")
    compiled_registry = registry_entries(workspace / "Cargo.lock")
    if any(original_registry.get(key) != value for key, value in compiled_registry.items()):
        raise ValueError("diagnostic resolved a registry version/checksum outside the frozen lock")
    for name in ("Cargo.toml", "Cargo.lock"):
        (output / "patches" / (name + ".patch")).write_text("".join(difflib.unified_diff(
            (control / "source" / name).read_text().splitlines(keepends=True),
            (workspace / name).read_text().splitlines(keepends=True), fromfile="frozen/" + name,
            tofile="instrumented/" + name)))
    argv = ["cargo", "+" + stable, "build", "--release", "--offline", "--locked", "-p", "tsr_bench", "--bin", "ts-bench",
            "--target", host, "--message-format=json-render-diagnostics", *overrides,
            "--target-dir", str(build_directory), "--config", "build.build-dir=" + json.dumps(str(build_directory))]
    messages = run(argv, workspace, env, output, "rust-build")
    binary = cargo_executable(messages, workspace / "crates/tsr_bench/Cargo.toml", "ts-bench", "bin", [])
    if not binary.resolve().is_relative_to(build_directory):
        raise ValueError("Cargo worker diagnostic artifact escaped its isolated build directory")
    shutil.copyfile(binary, output / "artifacts/rust-worker-timing")
    go_env = go_native_environment()
    if go_env["GOTOOLCHAIN"] != "local":
        raise ValueError("Go toolchain selection is not local")
    go_version = run(["go", "version"], ROOT, go_env, output, "go-version").decode()
    if go_version != reference["go_version"]:
        raise ValueError("Go version differs from frozen control")
    pin = strict_json_loads((control / "source/data/upstream.json").read_bytes())["pin"]
    archive = run(["git", "archive", pin, "tsc/go.mod", "tsc/go.sum", "tsc/internal"], verified_upstream(), env, work, "go-export")
    go_source = work / "go"
    go_source.mkdir()
    with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
        stream.extractall(go_source, filter="data")
    bridge = go_source / "tsc/internal/s07benchmark"
    bridge.mkdir()
    for origin, destination in (("tools/s07/benchmark/main.go", bridge / "main.go"),
                                ("tools/s07/benchmark/graph.go", bridge / "graph.go"),
                                ("scripts/s07_oracle/graph.go", bridge / "canonical_graph.go"),
                                ("scripts/s07_oracle/syntax_bridge.go", go_source / "tsc/internal/ast/s07_syntax_bridge.go"),
                                ("scripts/s07_oracle/access_bridge.go", go_source / "tsc/internal/ast/s07_access_bridge.go"),
                                ("scripts/s07_oracle/parser_bridge.go", go_source / "tsc/internal/parser/s07_parser_bridge.go")):
        shutil.copyfile(workspace / origin, destination)
    run(["gofmt", "-w", str(bridge / "main.go")], ROOT, go_env, output, "gofmt")
    # Capture exactly the formatted source that Go actually compiles.
    shutil.copyfile(bridge / "main.go", workspace / "tools/s07/benchmark/main.go")
    go_argv = ["go", "build", "-trimpath", "-mod=readonly", "-o", str(output / "artifacts/go-worker-timing"), "./internal/s07benchmark"]
    run(go_argv, go_source / "tsc", go_env, output, "go-build")
    if configuration(env, workspace) != config_before or tool_inputs() != before_tools:
        raise ValueError("diagnostic tool/configuration changed during build")
    runner.validate_bundle(control, control_sha)
    runner.validate_inputs(control, reference["expected_work"])
    shutil.copytree(workspace, output / "source")
    shutil.copyfile(control / "inputs.json", output / "inputs.json")
    artifacts = {}
    for runtime in KINDS:
        path = output / "artifacts" / (runtime + "-worker-timing")
        path.chmod(0o755)
        runner.runtime_libraries(path, output / (runtime + "-runtime-libraries.txt"))
        artifacts[runtime] = {"path": str(path.relative_to(output)), "sha256": runner.digest(path)}
    result = {"version": 1, "kind": "s07_bis_worker_timing_build", "diagnostic_only": True,
              "control_manifest_sha256": control_sha, "control_source_fingerprint": reference["source_fingerprint"],
              "expected_work": reference["expected_work"], "assignments": assignment_rows(output / "inputs.json"),
              "artifacts": artifacts, "tool_inputs": before_tools, "cargo_configuration": config_before,
              "rust_toolchain": stable, "rust_host": host, "go_version": go_version,
              "go_environment_policy": {key: go_env[key] for key in ("GOTOOLCHAIN", "GOENV", "CGO_ENABLED", "GOWORK", "GOFLAGS", "GOOS", "GOARCH")},
              "rust_command": argv, "isolated_build_directory": str(build_directory), "go_command": go_argv, "go_export_sha256": runner.sha(archive), "go_pin": pin,
              "registry_packages": len(compiled_registry), "invocation_policy": "one Rust then one Go eight-worker observation; no warmup, repeats or acceptance ratios"}
    return runner.seal(output, result)


def validate_build(directory, expected_sha):
    if runner.digest(directory / "manifest.json") != expected_sha:
        raise ValueError("worker timing build manifest changed")
    manifest = strict_json_loads((directory / "manifest.json").read_bytes())
    if manifest.get("kind") != "s07_bis_worker_timing_build" or manifest.get("diagnostic_only") is not True:
        raise ValueError("not a worker timing diagnostic build")
    if runner.inventory(directory) != manifest["inventory"]:
        raise ValueError("worker timing build inventory changed")
    for path in (directory, *directory.rglob("*")):
        if path.is_symlink() or path.stat().st_mode & 0o222:
            raise ValueError("worker timing build is not immutable")
    if set(manifest["artifacts"]) != set(KINDS):
        raise ValueError("missing diagnostic runtime")
    for runtime, artifact in manifest["artifacts"].items():
        binary = directory / artifact["path"]
        if not binary.resolve().is_relative_to(directory) or runner.digest(binary) != artifact["sha256"] or not os.access(binary, os.X_OK):
            raise ValueError("diagnostic executable changed: " + runtime)
    return manifest


def validate_observation(envelope, expected, assignments, runtime):
    # Reuse the native protocol's exact full-work checks, after removing only
    # our declared diagnostic addition. It still checks CPU, GC and allocator mode.
    envelope = dict(envelope)
    base = dict(envelope["report"])
    timing = base.pop("worker_timing")
    envelope["report"] = base
    validate_sample(envelope, expected, WORKERS, runtime, False)
    fields = {*DOMAINS, "runtime", "sender_send_ns", "sender_completion_offset_ns", "rows"}
    if (type(timing) is not dict or set(timing) != fields or timing.get("runtime") != runtime
            or any(type(timing.get(key)) is not type(value) or timing[key] != value for key, value in DOMAINS.items())):
        raise ValueError("worker timing domain changed")
    wall = base["wall_time_ns"]
    for name in ("sender_send_ns", "sender_completion_offset_ns"):
        if type(timing[name]) is not int or not 0 <= timing[name] <= wall:
            raise ValueError("invalid sender timer")
    if timing["sender_send_ns"] > timing["sender_completion_offset_ns"]:
        raise ValueError("sender intervals exceed sender completion")
    if type(timing["rows"]) is not list or len(timing["rows"]) != WORKERS:
        raise ValueError("missing worker")
    for worker, (row, assignment) in enumerate(zip(timing["rows"], assignments, strict=True)):
        if type(row) is not dict or set(row) != ROW_FIELDS:
            raise ValueError("unknown/missing worker fields")
        if any(type(row[key]) is not type(value) or row[key] != value for key, value in assignment.items()):
            raise ValueError("received assignment or byte count differs: " + str(worker))
        for name in ROW_FIELDS - {"assignment_sha256"}:
            if type(row[name]) is not int or row[name] < 0:
                raise ValueError("invalid worker counter/timer")
        start, end = row["start_offset_ns"], row["completion_offset_ns"]
        if not 0 <= start <= end <= wall or end + row["tail_ns"] != wall:
            raise ValueError("worker completion/tail escapes pipeline")
        if row["work_ns"] + row["receive_wait_ns"] > end - start:
            raise ValueError("worker intervals exceed elapsed lifetime")
    if sum(row["files"] for row in timing["rows"]) != expected["files"] or sum(row["loaded_bytes"] for row in timing["rows"]) != expected["loaded_bytes"]:
        raise ValueError("worker assignment totals differ")
    return timing


def capture(directory, build_sha, output):
    manifest = validate_build(directory, build_sha)
    runner.validate_inputs(directory, manifest["expected_work"])
    output.mkdir(parents=True, exist_ok=False)
    report = {"version": 1, "diagnostic_only": True, "build_manifest_sha256": build_sha,
              "expected_work": manifest["expected_work"], "assignments": manifest["assignments"],
              "tool_inputs": tool_inputs(), "host": host_info(), "schedule": list(KINDS),
              "status": "in_progress", "observations": []}
    runner.write_json(output / "report.json", report)
    try:
        for runtime in KINDS:
            reject_concurrent_builds()
            validate_build(directory, build_sha)
            runner.validate_inputs(directory, manifest["expected_work"])
            binary = directory / manifest["artifacts"][runtime]["path"]
            env = go_native_environment() if runtime == "go" else native_environment()
            argv = [sys.executable, str(ROOT / "scripts/s07_benchmark_child.py"), str(binary), str(directory / "inputs.json"), str(WORKERS)]
            raw = run(argv, ROOT, env, output, runtime)
            sample = strict_json_loads(raw)
            timing = validate_observation(sample, manifest["expected_work"], manifest["assignments"], runtime)
            report["observations"].append({"runtime": runtime, "wall_time_ns": sample["report"]["wall_time_ns"], "timing": timing})
            runner.write_json(output / "report.json", report)
        validate_build(directory, build_sha)
        runner.validate_inputs(directory, manifest["expected_work"])
        if tool_inputs() != report["tool_inputs"]:
            raise ValueError("diagnostic tools changed during capture")
        report["status"] = "complete"
        report["final_host"] = host_info()
        report["limits"] = "Single elapsed observations with per-file timer overhead; receive/send elapsed include scheduling. Worker work includes off-CPU descheduling. Not self CPU, pure queue blocking, causal attribution, a scheduler experiment, or E5/E6 evidence."
    except BaseException as error:
        report["status"] = "failed"
        report["failure"] = str(error)
        raise
    finally:
        runner.write_json(output / "report.json", report)
    return runner.digest(output / "report.json")


def verify(directory):
    report = strict_json_loads((directory / "report.json").read_bytes())
    if report["status"] != "complete" or report["schedule"] != list(KINDS):
        raise ValueError("incomplete worker diagnostic schedule")
    observed = []
    for runtime in KINDS:
        command = strict_json_loads((directory / (runtime + ".command.json")).read_bytes())
        if command["exit_code"] != 0 or command["argv"][-1] != str(WORKERS):
            raise ValueError("failed/wrong worker diagnostic invocation")
        sample = strict_json_loads((directory / (runtime + ".stdout")).read_bytes())
        timing = validate_observation(sample, report["expected_work"], report["assignments"], runtime)
        observed.append({"runtime": runtime, "wall_time_ns": sample["report"]["wall_time_ns"], "timing": timing})
    if observed != report["observations"]:
        raise ValueError("worker diagnostic summary differs from raw observations")
    return {"status": "pass", "observations": len(observed), "workers_each": WORKERS,
            "loaded_input_sha256": report["expected_work"]["loaded_input_sha256"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("build")
    prepare.add_argument("--control", type=Path, required=True)
    prepare.add_argument("--control-sha256", required=True)
    prepare.add_argument("--output", type=Path, required=True)
    collect = sub.add_parser("capture")
    collect.add_argument("--build", type=Path, required=True)
    collect.add_argument("--build-sha256", required=True)
    collect.add_argument("--output", type=Path, required=True)
    replay = sub.add_parser("verify")
    replay.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "verify":
        print(json.dumps(verify(args.directory.resolve()), sort_keys=True))
        return
    with (CACHE / "s07-benchmark/measurement.lock").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.command == "build":
            print(build(args.control.resolve(), args.control_sha256, args.output.resolve()))
        else:
            print(capture(args.build.resolve(), args.build_sha256, args.output.resolve()))


if __name__ == "__main__":
    main()
