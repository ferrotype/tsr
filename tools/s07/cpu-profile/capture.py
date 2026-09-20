#!/usr/bin/env python3
"""Capture diagnostic CPU profiles without changing or certifying E5/E6."""
import argparse
import fcntl
import hashlib
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
from s07_benchmark import CACHE, native_environment, go_native_environment, source_fingerprint
from s07_benchmark_measure import reject_concurrent_builds, host_info
from s07_benchmark_report import read_capture
import build as profile_build


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def tool_inputs():
    folder = Path(__file__).resolve().parent
    return {str(path.relative_to(ROOT)): sha(path) for path in sorted(folder.rglob("*"))
            if path.is_file() and path.suffix in {".py", ".rs", ".go", ".toml", ".lock"}}


def validate_work(report, expected, workers):
    if type(report) is not dict or type(report.get("workers")) is not int or report["workers"] != workers:
        raise ValueError("profile worker count mismatch")
    for key, value in expected.items():
        if type(report.get(key)) is not type(value) or report[key] != value:
            raise ValueError("profile did not execute frozen work: " + key)
    return report


def run(argv, env, prefix):
    reject_concurrent_builds()
    processes = subprocess.check_output(["ps", "-axo", "comm="], text=True).splitlines()
    active = {Path(name.strip()).name for name in processes} & {"tsr_cpu_profile", "go-cpu-profile", "xctrace"}
    if active:
        raise ValueError("another diagnostic capture is running: " + ", ".join(sorted(active)))
    started = time.monotonic_ns()
    with prefix.with_suffix(".stdout").open("w") as out, prefix.with_suffix(".stderr").open("w") as err:
        process = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=out, stderr=err, start_new_session=True)
        try:
            returncode = process.wait(timeout=300)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
    if returncode:
        raise ValueError(f"diagnostic command failed ({returncode}); see {prefix}.stderr")
    return time.monotonic_ns() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/s07-cpu-profiles/capture")
    parser.add_argument("--repetitions", type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.repetitions <= 10:
        raise ValueError("repetitions must be between 1 and 10")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    lock = (ROOT / "target/s07-cpu-profiles/capture.lock").open("a")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    baseline, _ = read_capture()
    before = source_fingerprint()
    tools_before = tool_inputs()
    folder = ROOT / "target/s07-cpu-profiles"
    build = json.loads((folder / "build.json").read_bytes())
    if build.get("source_fingerprint") != before or build.get("tool_inputs") != tools_before:
        raise ValueError("profiling sources changed since adapter build; run build.py again")
    if (build.get("dependency_inputs") != profile_build.dependency_inputs()
            or build.get("cargo_configuration") != profile_build.cargo_config(native_environment())
            or build.get("registry_lock") != profile_build.registry_lock()):
        raise ValueError("profiling build configuration changed; run build.py again")
    artifact = json.loads((folder / "rust-artifact.json").read_bytes())
    if artifact != build.get("rust_artifact"):
        raise ValueError("Rust artifact differs from build provenance")
    rust = Path(artifact["executable"])
    go = folder / "go-cpu-profile"
    binary_paths = {"rust": rust, "go": go,
                    "rust-baseline": CACHE / "s07-benchmark/rust-benchmark",
                    "go-baseline": CACHE / "s07-benchmark/go-benchmark"}
    binaries = {name: sha(path) for name, path in binary_paths.items()}
    for runtime in ("rust", "go"):
        if build["binaries"][runtime] != {"path": str(binary_paths[runtime]), "sha256": binaries[runtime]}:
            raise ValueError("diagnostic binary differs from build provenance: " + runtime)
    symbols = Path(str(rust) + ".dSYM")
    symbol_paths = [path for path in sorted(symbols.rglob("*")) if path.is_file()]
    if not symbol_paths:
        raise ValueError("Rust profiling build has no packed debug symbols")
    symbol_hashes = {str(path.relative_to(symbols)): sha(path) for path in symbol_paths}
    if symbol_hashes != build["symbols"]["files"]:
        raise ValueError("Rust debug symbols differ from build provenance")
    expected = baseline["expected_work"]
    inputs = CACHE / "s07-benchmark/inputs.json"
    input_hash = sha(inputs)
    rust_env = native_environment()
    go_env = go_native_environment()
    trace_env = {**rust_env, "DEVELOPER_DIR": "/Applications/Xcode.app/Contents/Developer"}
    rows = []
    metadata = {"schema": 1, "diagnostic_only": True,
                "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "source_fingerprint": before, "tool_inputs": tools_before,
                "binaries": binaries, "expected_work": expected, "transport_sha256": input_hash,
                "host": host_info(), "rust_debug_symbols": symbol_hashes,
                "build": build, "build_sha256": sha(folder / "build.json"),
                "baseline_samples_sha256": baseline["samples_sha256"],
                "repetitions": args.repetitions,
                "rust_artifact": artifact,
                "rustc": subprocess.check_output(["rustc", "-Vv"], cwd=ROOT, text=True),
                "go_version": subprocess.check_output(["go", "version"], env=go_env, text=True),
                "xctrace": subprocess.check_output(["/usr/bin/xctrace", "version"], env=trace_env, text=True)}
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    with (args.output / "runs.ndjson").open("w") as records:
        for workers in (1, 8):
            for index in range(args.repetitions):
                # Alternate runtime order; every invocation is a fresh process.
                for runtime in (("go", "rust") if index % 2 == 0 else ("rust", "go")):
                    prefix = args.output / f"{runtime}-{workers}-{index}"
                    if runtime == "go":
                        elapsed = run([str(go), str(inputs), str(workers), str(prefix)], go_env, prefix)
                        report = json.loads(prefix.with_suffix(".stdout").read_bytes())
                        if report != json.loads(prefix.with_suffix(".report.json").read_bytes()):
                            raise ValueError("Go profile report and stdout disagree")
                        if prefix.with_suffix(".pprof").stat().st_size == 0:
                            raise ValueError("empty Go CPU profile")
                        raw = {"profile": sha(prefix.with_suffix(".pprof"))}
                    else:
                        command = ["/usr/bin/xctrace", "record", "--template", "Time Profiler",
                                   "--output", str(prefix.with_suffix(".trace")), "--target-stdout",
                                   str(prefix.with_suffix(".report.json")), "--launch", "--",
                                   str(rust), str(inputs), str(workers)]
                        elapsed = run(command, trace_env, prefix)
                        report = json.loads(prefix.with_suffix(".report.json").read_bytes())
                        raw = {"trace_files": {str(p.relative_to(prefix.with_suffix(".trace"))): sha(p)
                               for p in sorted(prefix.with_suffix(".trace").rglob("*")) if p.is_file()}}
                        if not raw["trace_files"]:
                            raise ValueError("empty Rust CPU trace")
                    validate_work(report, expected, workers)
                    if report.get("diagnostic_only") is not True:
                        raise ValueError("adapter did not identify diagnostic output")
                    row = {"runtime": runtime, "workers": workers, "index": index,
                           "profiled": True, "report": report, "artifacts": raw,
                           "command_elapsed_ns": elapsed}
                    records.write(json.dumps(row, sort_keys=True) + "\n"); records.flush(); rows.append(row)
                    print(f"Captured {runtime} CPU profile: workers={workers}, repetition={index}", flush=True)
                # Controls identify adapter/profiler perturbation; they never replace E6.
                for runtime, binary, env in (("go-baseline", CACHE / "s07-benchmark/go-benchmark", go_env),
                                             ("rust-baseline", CACHE / "s07-benchmark/rust-benchmark", rust_env),
                                             ("rust-adapter", rust, rust_env)):
                    prefix = args.output / f"{runtime}-{workers}-{index}"
                    elapsed = run([str(binary), str(inputs), str(workers)], env, prefix)
                    report = validate_work(json.loads(prefix.with_suffix(".stdout").read_bytes()), expected, workers)
                    row = {"runtime": runtime, "workers": workers, "index": index,
                           "profiled": False, "report": report, "command_elapsed_ns": elapsed}
                    records.write(json.dumps(row, sort_keys=True) + "\n"); records.flush(); rows.append(row)
        if (before != source_fingerprint() or tools_before != tool_inputs()
                or binaries != {name: sha(path) for name, path in binary_paths.items()}
                or symbol_hashes != {str(path.relative_to(symbols)): sha(path) for path in symbol_paths}
                or input_hash != sha(inputs)):
            raise ValueError("profiling inputs changed; preserve raw records without a complete report")
    (args.output / "report.json").write_text(json.dumps({**metadata, "capture_complete": True,
        "cpu_sample_validation": "pending separate profile analysis", "runs": rows}, indent=2) + "\n")
    print(f"Complete diagnostic capture: {len(rows)} fresh processes; {args.output}")


if __name__ == "__main__":
    main()
