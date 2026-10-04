"""Repeated native S07 captures. Raw failures and slow samples stay in the record."""
import fcntl
import json
import math
import os
import platform
from pathlib import Path
import sys
import tomllib
from statistics import median

from s04_common import command, strict_json_loads
from s07_benchmark import ROOT, CACHE, build_rust, build_allocation_probe, native_environment, go_native_environment, provision_inputs, sha, source_fingerprint, cargo_configuration, rust_native_toolchain
from s07_benchmark_stats import ratio_summary
from s04_runtime import load_toolchains

COUNTERS = ("files", "loaded_bytes", "nodes", "symbols", "parse_diagnostics", "bind_diagnostics")
REPORT_FIELDS = {*COUNTERS, "loaded_input_sha256", "version", "workers", "wall_time_ns", "allocated_bytes", "startup_ns", "preload_ns", "worker_setup_ns", "cpu_capacity", "goroutines_ready"}
ENVELOPE_FIELDS = {"report", "peak_rss_bytes", "process_time_ns", "user_time_ns", "system_time_ns", "stderr"}
MAX_PROCESS_NS = 600_000_000_000
RUST_PROFILE = "release; panic=unwind; fat LTO; one codegen unit"
MEASUREMENT_DOMAINS = {"rss": "per-child lifetime OS peak, including preload; uninstrumented Rust",
                       "wall": "precreated-worker barrier through all retained parse/bind roots; uninstrumented Rust",
                       "allocation": "Rust original layout request bytes via cap, including realloc new size; Go MemStats.TotalAlloc delta; preload and worker setup excluded"}


def integer(value, minimum, maximum, name):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError("invalid benchmark integer: " + name)
    return value


def host_info(minimum_cpus=8):
    """The measurement host's record. The default minimum is parse-bind's: its
    eight-worker mode needs eight available CPUs. Workloads without that
    worker requirement may pass a lower minimum."""
    cpus = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else os.cpu_count()
    if cpus is None or cpus < minimum_cpus:
        raise ValueError(f"the measurement host has {cpus} available CPUs; this workload needs at least {minimum_cpus}")
    if sys.platform == "darwin":
        memory = int(command(["sysctl", "-n", "hw.memsize"], cwd=ROOT))
        physical = int(command(["sysctl", "-n", "hw.physicalcpu"], cwd=ROOT))
    else:
        memory = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
        physical = None
    return {"os": sys.platform, "architecture": platform.machine(), "release": platform.release(),
            "cpu_capacity": cpus, "physical_cpus": physical, "memory_bytes": memory,
            "initial_load_average": os.getloadavg()}


def reject_concurrent_builds():
    # Inspect executable names only; do not collect unrelated process arguments.
    processes = command(["ps", "-axo", "comm="], cwd=ROOT).decode().splitlines()
    blockers = {"cargo", "rustc", "rustdoc", "go", "clang", "clang++", "ld", "ts-bench", "rust-benchmark", "rust-benchmark-allocation", "go-benchmark"}
    active = sorted({Path(name.strip()).name for name in processes} & blockers)
    if active:
        raise ValueError("measurement host is running another compiler/benchmark: " + ", ".join(active))


def validate_sample(sample, expected, workers, runtime, allocation):
    if runtime not in {"go", "rust"} or type(allocation) is not bool or type(workers) is not int or workers not in (1, 8):
        raise ValueError("invalid benchmark sample domain")
    if type(sample) is not dict or set(sample) != ENVELOPE_FIELDS:
        raise ValueError("invalid resource-accounting envelope")
    for name in ("peak_rss_bytes", "process_time_ns"):
        integer(sample[name], 1, 2**63-1 if name == "peak_rss_bytes" else MAX_PROCESS_NS + 1_000_000_000, name)
    for name in ("user_time_ns", "system_time_ns"):
        integer(sample[name], 0, MAX_PROCESS_NS * 1024, name)
    if type(sample["stderr"]) is not str or len(sample["stderr"].encode()) > 262144:
        raise ValueError("invalid bounded benchmark stderr")
    report = sample["report"]
    fields = REPORT_FIELDS | ({"gomaxprocs", "gogc"} if runtime == "go" else set())
    if type(report) is not dict or set(report) != fields:
        raise ValueError("unknown or missing native benchmark fields")
    if integer(report["version"], 1, 1, "version") != 1 or integer(report["workers"], 1, 8, "workers") != workers:
        raise ValueError("benchmark version or worker count changed")
    for name in COUNTERS:
        if type(report.get(name)) is not int or report[name] != expected[name]:
            raise ValueError("benchmark did not execute the independently validated workload: " + name)
    if type(report.get("cpu_capacity")) is not int or report["cpu_capacity"] < workers:
        raise ValueError("benchmark child did not observe the declared CPU capacity")
    for name in ("wall_time_ns", "startup_ns", "preload_ns", "worker_setup_ns"):
        if type(report.get(name)) is not int or not 0 <= report[name] <= 600_000_000_000:
            raise ValueError("invalid phase timing: " + name)
    if report["wall_time_ns"] <= 0 or report["wall_time_ns"] > sample["process_time_ns"]:
        raise ValueError("phase timing exceeds complete child lifetime")
    if runtime == "go":
        if integer(report["gomaxprocs"], 1, 8, "gomaxprocs") != workers or integer(report["gogc"], 100, 100, "gogc") != 100:
            raise ValueError("Go processor/GC policy changed")
        integer(report["goroutines_ready"], workers + 1, 2**31-1, "goroutines_ready")
    elif report["goroutines_ready"] is not None:
        raise ValueError("Rust driver cannot report a Go runtime goroutine count")
    if type(report.get("loaded_input_sha256")) is not str or report["loaded_input_sha256"] != expected["loaded_input_sha256"]:
        raise ValueError("native preload bytes/options differ from frozen graph obligations")
    allocated = report.get("allocated_bytes")
    if allocation or runtime == "go":
        integer(allocated, 1, 2**63-1, "allocated_bytes")
    elif allocated is not None:
        raise ValueError("timing/RSS came from an allocation-instrumented Rust driver")
    return sample


def sample(binary, inputs, workers, env, expected, runtime, allocation):
    raw = command([sys.executable, str(ROOT / "scripts/s07_benchmark_child.py"), str(binary), str(inputs), str(workers)], cwd=ROOT, env=env)
    return validate_sample(strict_json_loads(raw), expected, workers, runtime, allocation)



def validate_allocation_preflight(results):
    if type(results) is not list or len(results) != 2:
        raise ValueError("missing debug/release allocator accounting preflight")
    msrv = load_toolchains(ROOT)["msrv"]
    for result, mode in zip(results, ("debug", "release")):
        if (type(result) is not dict or set(result) != {"mode", "toolchain", "version", "workers", "requested_bytes", "expected_bytes", "live_before", "live_after"}
                or result["mode"] != mode or result["toolchain"] != msrv
                or any(type(value) is not int or value < 0 for key, value in result.items() if key not in {"mode", "toolchain"})
                or result["version"] != 1 or result["workers"] != 8
                or result["requested_bytes"] != 9_600_400 or result["expected_bytes"] != 9_600_400
                or result["live_before"] != result["live_after"]):
            raise ValueError("allocator accounting preflight is incomplete or incompatible")
    return results

def allocation_preflight():
    results = []
    for mode in ("debug", "release"):
        msrv = load_toolchains(ROOT)["msrv"]
        binary, env = build_allocation_probe(mode, msrv)
        raw = command([str(binary)], cwd=ROOT, env=env)
        result = strict_json_loads(raw)
        if (set(result) != {"version", "workers", "requested_bytes", "expected_bytes", "live_before", "live_after"}
                or any(type(value) is not int or value < 0 for value in result.values())
                or result["version"] != 1 or result["workers"] != 8
                or result["requested_bytes"] != result["expected_bytes"] or result["requested_bytes"] != 9_600_400
                or result["live_before"] != result["live_after"]):
            raise ValueError("allocator wrapper failed original-request/realloc/zeroed/deallocation accounting")
        results.append({"mode": mode, "toolchain": msrv, **result})
    return validate_allocation_preflight(results)


THRESHOLDS = "status/perf/thresholds.toml"


def e6_thresholds():
    """Per-mode wall-time thresholds (ADR 0021) from `[parse-bind]` of status/perf/thresholds.toml,
    the only source of the thresholds the stability and extension rule judges against."""
    table = tomllib.loads((ROOT / THRESHOLDS).read_text()).get("parse-bind")
    if type(table) is not dict:
        raise ValueError(THRESHOLDS + " has no [parse-bind] table")
    thresholds = {}
    for workers, name in (("1", "one_thread_wall_time"), ("8", "eight_threads_wall_time")):
        if name not in table:
            raise ValueError(f"{THRESHOLDS} [parse-bind] has no {name} threshold")
        value = table[name]
        if type(value) not in {int, float} or not math.isfinite(value) or value <= 0:
            raise ValueError(f"{THRESHOLDS} [parse-bind] {name} must be a positive finite ratio")
        thresholds[workers] = float(value)
    return thresholds


def aggregate(rows):
    thresholds = e6_thresholds()
    result = {}
    for workers in (1, 8):
        selected = [row for row in rows if row["workers"] == workers]
        summary = {}
        for metric, allocation in (("wall_time_ns", False), ("peak_rss_bytes", False), ("allocated_bytes", True)):
            values = {}
            for runtime in ("go", "rust"):
                samples = [row["sample"] for row in selected if row["runtime"] == runtime and row["allocation"] == allocation]
                values[runtime] = [item["peak_rss_bytes"] if metric == "peak_rss_bytes" else item["report"][metric] for item in samples]
            summary[metric] = ratio_summary(values["go"], values["rust"], timing=metric == "wall_time_ns", threshold=thresholds[str(workers)])
        overhead = {}
        for metric in ("wall_time_ns", "peak_rss_bytes"):
            values = {}
            for allocation in (False, True):
                samples = [row["sample"] for row in selected if row["runtime"] == "rust" and row["allocation"] is allocation]
                values[allocation] = [item[metric] if metric == "peak_rss_bytes" else item["report"][metric] for item in samples]
            normal, instrumented = median(values[False]), median(values[True])
            overhead[metric] = {"normal_samples": len(values[False]), "instrumented_samples": len(values[True]),
                                "normal_median": normal, "instrumented_median": instrumented,
                                "instrumented_over_normal": instrumented / normal,
                                "median_delta": instrumented - normal}
        summary["rust_wrapper_overhead"] = {"informational": True, "comparison": "separate-process instrumented/normal Rust medians; no adjustment to performance gates", **overhead}
        result[str(workers)] = summary
    return result


def metrics_from_summaries(summaries):
    return {"peak_rss_ratio": max(mode["peak_rss_bytes"]["ratio"] for mode in summaries.values()),
            "allocated_bytes_ratio": max(mode["allocated_bytes"]["ratio"] for mode in summaries.values()),
            "one_thread_wall_time_ratio": summaries["1"]["wall_time_ns"]["ratio"],
            "eight_threads_wall_time_ratio": summaries["8"]["wall_time_ns"]["ratio"],
            "stable": all(mode["wall_time_ns"]["stable"] for mode in summaries.values())}


def capture(graph_report, destination):
    """The caller supplies the current complete two-mode bindworkload report."""
    # Final graph report validation/binary binding is intentionally mandatory;
    # its schema is shared with the independent workload producer.
    from s07_benchmark_graph import validate_measurement_prerequisite, requests_from_frozen
    from s07_benchmark_inputs import loaded_input_digest
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    lock_path = CACHE / "s07-benchmark/measurement.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("another S07 performance capture owns this host") from error
        # An interrupted or invalid attempt must never leave an older success
        # visible to a separate e5/e6 consumer.
        (destination / "report.json").write_text('{"version":1,"status":"capture_in_progress"}\n')
        before = source_fingerprint()
        thresholds = e6_thresholds()
        # Capacity check before any build. The thresholds' host class (ADR 0021)
        # is not enforced here: another host is measured and recorded, and
        # `scripts/perf.py check` reports it beside any miss.
        host_info()
        cargo_config = cargo_configuration()
        graph_bytes = Path(graph_report).read_bytes()
        prerequisite = strict_json_loads(graph_bytes)
        graph_sha256 = sha(graph_bytes)
        go = CACHE / "s07-benchmark/go-benchmark"
        rust = CACHE / "s07-benchmark/rust-benchmark"
        # Native builds are not byte-reproducible (for example, mimalloc embeds
        # its compilation time). Reuse the exact normal artifacts that passed
        # graph parity, rejecting stale source/configuration or changed bytes.
        graph_binaries = {"go": sha(go.read_bytes()), "rust": sha(rust.read_bytes())}
        expected = validate_measurement_prerequisite(prerequisite, before, graph_binaries, cargo_config)
        preflight = allocation_preflight()
        go_env = go_native_environment()
        rust_env = native_environment()
        instrumented, _ = build_rust(True)
        binaries = {"go": sha(go.read_bytes()), "rust": sha(rust.read_bytes()), "rust_allocation": sha(instrumented.read_bytes())}
        if source_fingerprint() != before or cargo_configuration() != cargo_config or sha(Path(graph_report).read_bytes()) != graph_sha256:
            raise ValueError("source, Cargo configuration or graph prerequisite changed while preparing measurement")
        validate_measurement_prerequisite(prerequisite, before, binaries, cargo_config)
        inputs, _ = provision_inputs(go, go_env)
        _, recipes = requests_from_frozen(inputs)
        if loaded_input_digest(recipes) != expected["loaded_input_sha256"]:
            raise ValueError("benchmark input transport differs from graph prerequisite")
        transport_sha256 = sha(inputs.read_bytes())
        host = host_info()
        reject_concurrent_builds()
        samples = []
        metadata = {"version": 1, "source_fingerprint": before, "binaries": binaries, "host": host,
                    "graph_report_sha256": graph_sha256,
                    "transport_sha256": transport_sha256,
                    "revision": command(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip(),
                    "rust_profile": RUST_PROFILE,
                    "measurement_domains": MEASUREMENT_DOMAINS,
                    "cargo_configuration": cargo_config,
                    "allocator": "mimalloc 0.1.48", "allocation_wrapper": "cap 0.1.2/stats",
                    "go_gc": {"GOGC": 100, "GOMEMLIMIT": "unset", "GOMAXPROCS": "worker count"},
                    "allocation_preflight": preflight, "expected_work": expected,
                    "rustc": command(["rustc", "+"+rust_native_toolchain(rust_env)[0], "-Vv"], cwd=ROOT, env=rust_env).decode(),
                    "go_version": command(["go", "version"], cwd=ROOT, env=go_env).decode()}
        (destination / "capture.json").write_text(json.dumps(metadata, indent=2) + "\n")
        with (destination / "samples.ndjson").open("w") as stream:
            for workers in (1, 8):
                for allocation in (False, True):
                    runtimes = {"go": (go, go_env), "rust": (instrumented if allocation else rust, rust_env)}
                    # One fixed unrecorded warm-up per runtime/mode; validate it
                    # with the same protocol but never select it as a sample.
                    for runtime, (binary, env) in runtimes.items():
                        sample(binary, inputs, workers, env, expected, runtime, allocation)
                    for index in range(7):
                        for runtime in (("go", "rust") if index % 2 == 0 else ("rust", "go")):
                            reject_concurrent_builds()
                            binary, env = runtimes[runtime]
                            value = sample(binary, inputs, workers, env, expected, runtime, allocation)
                            row = {"workers": workers, "allocation": allocation, "runtime": runtime, "index": index, "sample": value}
                            stream.write(json.dumps(row, separators=(",", ":")) + "\n")
                            stream.flush()
                            samples.append(row)
                    if not allocation:
                        while True:
                            values = {runtime: [row["sample"]["report"]["wall_time_ns"] for row in samples if row["workers"] == workers and not row["allocation"] and row["runtime"] == runtime] for runtime in runtimes}
                            statistics = ratio_summary(values["go"], values["rust"], timing=True, threshold=thresholds[str(workers)])
                            if not statistics["needs_more"]:
                                break
                            for index in range(len(values["go"]), len(values["go"]) + 7):
                                for runtime in (("go", "rust") if index % 2 == 0 else ("rust", "go")):
                                    reject_concurrent_builds()
                                    binary, env = runtimes[runtime]
                                    value = sample(binary, inputs, workers, env, expected, runtime, allocation)
                                    row = {"workers": workers, "allocation": allocation, "runtime": runtime, "index": index, "sample": value}
                                    stream.write(json.dumps(row, separators=(",", ":")) + "\n")
                                    stream.flush()
                                    samples.append(row)
        _, after_recipes = requests_from_frozen(inputs)
        if sha(inputs.read_bytes()) != transport_sha256 or loaded_input_digest(after_recipes) != expected["loaded_input_sha256"]:
            raise ValueError("benchmark input transport changed during measurement")
        after = source_fingerprint()
        after_binaries = {"go": sha(go.read_bytes()), "rust": sha(rust.read_bytes()), "rust_allocation": sha(instrumented.read_bytes())}
        if after != before or cargo_configuration() != cargo_config or after_binaries != binaries or sha(Path(graph_report).read_bytes()) != metadata["graph_report_sha256"]:
            raise ValueError("source files changed during measurement; retain raw samples without publishing metrics")
        summaries = aggregate(samples)
        report = {**metadata, "source_fingerprint_after": after, "binaries_after": after_binaries,
                  "summaries": summaries, "metrics": metrics_from_summaries(summaries),
                  "samples_sha256": sha((destination / "samples.ndjson").read_bytes()), "samples": len(samples)}
        (destination / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        return report
