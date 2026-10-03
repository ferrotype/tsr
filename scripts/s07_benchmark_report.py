"""Validate and consume one complete S07 capture for both memory and CPU gates."""
from pathlib import Path
import math
import re
import tomllib

from s04_common import strict_json_loads
from s06_protocol import canonical
from s07_benchmark import ROOT, CACHE, sha, source_fingerprint, cargo_configuration
from s07_benchmark_graph import validate_measurement_prerequisite
from s07_benchmark_measure import aggregate, metrics_from_summaries, validate_sample, validate_allocation_preflight, RUST_PROFILE, MEASUREMENT_DOMAINS, e6_thresholds
from s07_benchmark_stats import ratio_summary
from s04_runtime import load_toolchains


def validate_metadata(report):
    if (report["allocator"] != "mimalloc 0.1.48" or report["allocation_wrapper"] != "cap 0.1.2/stats"
            or report["rust_profile"] != RUST_PROFILE
            or canonical(report["measurement_domains"]) != canonical(MEASUREMENT_DOMAINS)
            or canonical(report["go_gc"]) != canonical({"GOGC": 100, "GOMEMLIMIT": "unset", "GOMAXPROCS": "worker count"})):
        raise ValueError("benchmark allocator/build/runtime contract changed")
    for name, length in (("revision", 40), ("transport_sha256", 64), ("graph_report_sha256", 64), ("samples_sha256", 64)):
        value = report[name]
        if type(value) is not str or re.fullmatch("[0-9a-f]{"+str(length)+"}", value) is None:
            raise ValueError("invalid benchmark provenance hash: " + name)
    host = report["host"]
    if (type(host) is not dict or set(host) != {"os", "architecture", "release", "cpu_capacity", "physical_cpus", "memory_bytes", "initial_load_average"}
            or type(host["os"]) is not str or host["os"] not in {"darwin", "linux"}
            or type(host["architecture"]) is not str or not host["architecture"]
            or type(host["release"]) is not str or not host["release"]
            or type(host["cpu_capacity"]) is not int or host["cpu_capacity"] < 8
            or type(host["memory_bytes"]) is not int or host["memory_bytes"] <= 0
            or host["physical_cpus"] is not None and (type(host["physical_cpus"]) is not int or host["physical_cpus"] <= 0)
            or type(host["initial_load_average"]) is not list or len(host["initial_load_average"]) != 3
            or any(type(value) not in {int, float} or not 0 <= value <= 2**31 or not math.isfinite(value) for value in host["initial_load_average"])):
        raise ValueError("benchmark lacks a capacity-qualified host record")
    stable = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    rustc = report["rustc"]
    if type(rustc) is not str:
        raise ValueError("invalid Rust compiler provenance")
    lines = rustc.splitlines()
    fields = dict(line.split(": ", 1) for line in lines[1:] if ": " in line)
    if (len(lines) != 7 or len(fields) != 6 or set(fields) != {"binary", "commit-hash", "commit-date", "host", "release", "LLVM version"}
            or fields["binary"] != "rustc" or fields["release"] != stable
            or re.fullmatch(r"[0-9a-f]{40}", fields["commit-hash"]) is None
            or re.fullmatch(r"\d{4}-\d{2}-\d{2}", fields["commit-date"]) is None
            or re.fullmatch(r"\d+\.\d+\.\d+", fields["LLVM version"]) is None
            or lines[0] != f"rustc {stable} ({fields['commit-hash'][:9]} {fields['commit-date']})"):
        raise ValueError("Rust compiler does not match pinned native provenance")
    architecture = {"arm64": "aarch64", "amd64": "x86_64"}.get(host["architecture"], host["architecture"])
    if not fields["host"].startswith(architecture+"-") or ("-apple-darwin" if host["os"] == "darwin" else "-linux-") not in fields["host"]:
        raise ValueError("Rust compiler host differs from the measurement host")
    go_architecture = {"aarch64": "arm64", "x86_64": "amd64"}.get(architecture, architecture)
    if report["go_version"] != f"go version {load_toolchains(ROOT)['go']} {host['os']}/{go_architecture}\n":
        raise ValueError("Go compiler does not match pinned native provenance")


def validate_rows(rows, expected):
    """Reject omitted, duplicated, reordered and selectively extended samples."""
    cursor = 0
    thresholds = e6_thresholds()
    for workers in (1, 8):
        for allocation in (False, True):
            selected = []
            while cursor < len(rows) and rows[cursor].get("workers") == workers and rows[cursor].get("allocation") is allocation:
                row = rows[cursor]
                index, side = divmod(len(selected), 2)
                runtime = (("go", "rust") if index % 2 == 0 else ("rust", "go"))[side]
                if (set(row) != {"workers", "allocation", "runtime", "index", "sample"}
                        or type(row["workers"]) is not int or type(row["allocation"]) is not bool
                        or type(row["index"]) is not int or row["index"] != index or row["runtime"] != runtime):
                    raise ValueError("benchmark sample identity/order changed")
                validate_sample(row["sample"], expected, workers, runtime, allocation)
                selected.append(row)
                cursor += 1
            count = len(selected) // 2
            if len(selected) % 2 or count not in ({7} if allocation else {7, 14, 21}):
                raise ValueError("incomplete benchmark sample batch")
            if not allocation:
                values = {runtime: [row["sample"]["report"]["wall_time_ns"] for row in selected if row["runtime"] == runtime] for runtime in ("go", "rust")}
                # Every extension and stopping decision follows the frozen rule.
                for end in range(7, count + 1, 7):
                    summary = ratio_summary(values["go"][:end], values["rust"][:end], timing=True, threshold=thresholds[str(workers)])
                    if summary["needs_more"] != (end < count):
                        raise ValueError("benchmark stopping/extension policy changed")
    if cursor != len(rows):
        raise ValueError("unknown, reordered or extra benchmark sample mode")
    return aggregate(rows)


def read_capture(directory=ROOT / "target/s07-benchmark", graph_report=ROOT / "target/s07-bindworkload/report.json"):
    directory, graph_report = Path(directory), Path(graph_report)
    report = strict_json_loads((directory / "report.json").read_bytes())
    required = {"version", "source_fingerprint", "binaries", "host", "cargo_configuration", "graph_report_sha256", "transport_sha256", "revision", "rust_profile", "measurement_domains", "allocator", "allocation_wrapper", "go_gc", "allocation_preflight", "expected_work", "rustc", "go_version", "source_fingerprint_after", "binaries_after", "summaries", "metrics", "samples_sha256", "samples"}
    if type(report) is not dict or set(report) != required:
        raise ValueError("benchmark capture has missing or unknown metadata")
    validate_allocation_preflight(report["allocation_preflight"])
    validate_metadata(report)
    if report["cargo_configuration"] != cargo_configuration():
        raise ValueError("caller Cargo configuration changed since measurement")
    before = source_fingerprint()
    if (type(report.get("version")) is not int or report["version"] != 1
            or report.get("source_fingerprint") != before or report.get("source_fingerprint_after") != before):
        raise ValueError("benchmark capture is incomplete or stale for the current sources")
    binaries = {runtime: sha((CACHE / "s07-benchmark" / name).read_bytes()) for runtime, name in
                (("go", "go-benchmark"), ("rust", "rust-benchmark"), ("rust_allocation", "rust-benchmark-allocation"))}
    if report.get("binaries") != binaries or report.get("binaries_after") != binaries:
        raise ValueError("benchmark capture binaries changed")
    if report.get("graph_report_sha256") != sha(graph_report.read_bytes()):
        raise ValueError("benchmark graph prerequisite changed")
    expected = validate_measurement_prerequisite(graph_report, before, binaries, report["cargo_configuration"])
    if canonical(report.get("expected_work")) != canonical(expected):
        raise ValueError("benchmark scalar obligations changed")
    raw = (directory / "samples.ndjson").read_bytes()
    rows = [strict_json_loads(line) for line in raw.splitlines()]
    if (sha(raw) != report.get("samples_sha256") or type(report.get("samples")) is not int
            or report["samples"] != len(rows) or any(type(row) is not dict for row in rows)):
        raise ValueError("benchmark raw sample inventory changed")
    summaries = validate_rows(rows, expected)
    if canonical(report.get("summaries")) != canonical(summaries) or canonical(report.get("metrics")) != canonical(metrics_from_summaries(summaries)):
        raise ValueError("benchmark published aggregates differ from all raw samples")
    return report, rows
