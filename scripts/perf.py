#!/usr/bin/env python3
"""Performance runs against Go on fixed workloads (docs/EVIDENCE-plan.md, sections 3 and 6).

    perf.py record <workload> --capture DIR [--label TEXT] [--out DIR] [--graph-report FILE]
    perf.py check  <workload> [--run FILE]

`record` reads a finished capture of the workload's harness, through the
harness's own reader, and writes status/perf/<workload>/<recorded_at>-<rev8>.json:
the workload, the pin, the revision, the host, the Rust/Go ratios and the raw
per-sample numbers behind them. A run file is committed by hand (or from the
dispatch workflow's artifact) and never regenerated; `record` refuses to
overwrite one.

`check` compares the newest run of a workload (or --run FILE) with
status/perf/thresholds.toml, prints each ratio with its threshold, writes the
same table to $GITHUB_STEP_SUMMARY when set, and exits 1 on a miss. It is not a
pull-request gate: the dispatch workflow (.github/workflows/perf.yml) calls it
for the job summary.

Workloads, and the harness commands that make their captures:

  parse-bind  parse and bind of the pinned VS Code tree, one and eight workers
              (E5/E6, ADR 0021): scripts/s07_benchmark_graph.py capture, then
              scripts/s07_benchmark.py capture (target/s07-benchmark)
  checker     the frozen checker query workload and per-type footprint
              (ADR 0022): scripts/s08_checkerbench.py build, then capture
              (target/s08/checkerbench)
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
PERF = ROOT / "status/perf"
THRESHOLDS = PERF / "thresholds.toml"
UPSTREAM = ROOT / "data/upstream.json"

# The host class the thresholds are authorized for (ADR 0021): macOS arm64 with
# at least eight schedulable CPUs. A run elsewhere is recorded and compared, and
# `check` says that its misses are informational.
THRESHOLD_HOST = {"os": "macos", "arch": "aarch64", "cpus": 8}

OS_NAMES = {"darwin": "macos"}
ARCH_NAMES = {"arm64": "aarch64", "amd64": "x86_64"}


@dataclass(frozen=True)
class Workload:
    capture: str
    read: Callable[[Path, argparse.Namespace], dict]


# ---------------------------------------------------------------- parse-bind

# The capture's metrics, renamed: Rust/Go medians, peak RSS and allocated bytes
# the larger of the two worker modes (s07_benchmark_measure.metrics_from_summaries).
PARSE_BIND_RATIOS = {
    "one_thread_wall_time_ratio": "one_thread_wall_time",
    "eight_threads_wall_time_ratio": "eight_threads_wall_time",
    "peak_rss_ratio": "peak_rss",
    "allocated_bytes_ratio": "allocated_bytes",
}
# Per worker mode: each summary field, and whether its samples come from the
# allocation-instrumented pass (s07_benchmark_measure.aggregate).
PARSE_BIND_FIELDS = (("wall_time_ns", False), ("peak_rss_bytes", False), ("allocated_bytes", True))


def parse_bind_measurement(report, rows):
    """Ratios and raw samples of an S07 capture: its `report.json` and the rows of
    `samples.ndjson`. Sample lists are keyed by the `workers_<n>_<field>` ratio
    they feed, in capture order."""
    ratios = {name: report["metrics"][key] for key, name in PARSE_BIND_RATIOS.items()}
    samples = {"rust": {}, "go": {}}
    for workers in ("1", "8"):
        for field, allocation in PARSE_BIND_FIELDS:
            name = f"workers_{workers}_{field}"
            ratios[name] = report["summaries"][workers][field]["ratio"]
            for runtime in ("rust", "go"):
                selected = [row["sample"] for row in rows if str(row["workers"]) == workers
                            and row["allocation"] is allocation and row["runtime"] == runtime]
                samples[runtime][name] = [sample[field] if field == "peak_rss_bytes" else sample["report"][field]
                                          for sample in selected]
    return {"ratios": ratios, "samples": samples, "host": report["host"], "revision": report["revision"]}


def read_parse_bind(capture, args):
    from s07_benchmark_report import read_capture
    report, rows = read_capture(capture, args.graph_report)
    return parse_bind_measurement(report, rows)


# ---------------------------------------------------------------- checker

CHECKER_RATIOS = {
    "throughput_ratio": "throughput",
    "elapsed_ratio": "elapsed",
    "allocated_bytes_ratio": "allocated_bytes",
    "retained_bytes_ratio": "retained_bytes",
    "type_footprint_ratio": "type_footprint",
}


def checker_measurement(result):
    """Ratios and raw samples of a checkerbench report (s08_checkerbench.report).
    Samples: the normal mode's checker interval (throughput, elapsed), the
    allocation mode's requested and retained bytes (allocated_bytes,
    retained_bytes) and its type census (type_footprint), one value per sample."""
    ratios = {name: result["metrics"][key] for key, name in CHECKER_RATIOS.items() if key in result["metrics"]}
    modes = result["modes"]
    samples = {}
    for runtime in ("rust", "go"):
        values = {"interval_ns": modes["normal"]["interval_ns"][runtime]["samples"]}
        allocation = modes["alloc"].get("allocation", {}).get(runtime, {})
        census = modes["alloc"].get("census", {}).get(runtime, {})
        for key, source in (("requested_bytes", allocation), ("retained_bytes", allocation),
                            ("type_storage_bytes", census), ("types_reachable", census)):
            if key in source:
                values[key] = source[key]
        samples[runtime] = values
    return {"ratios": ratios, "samples": samples, "host": result["host"]}


def read_checker(capture, args):
    import s08_checkerbench as bench
    capture = capture.resolve()
    finished = json.loads((capture / "capture.json").read_text()).get("finished")
    result = bench.report(capture)
    if result.get("smoke"):
        raise ValueError("a smoke capture measures a prefix of the workload; it is not a run")
    for name, reason in sorted(result["unavailable"].items()):
        print(f"checker: {name} unavailable: {reason}", file=sys.stderr)
    return {**checker_measurement(result), "recorded_at": finished}


WORKLOADS = {
    "parse-bind": Workload("target/s07-benchmark", read_parse_bind),
    "checker": Workload("target/s08/checkerbench", read_checker),
}


# ---------------------------------------------------------------- run files

def pin():
    return json.loads(UPSTREAM.read_text())["pin"]


def head():
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, check=True,
                          capture_output=True, text=True).stdout.strip()


def timestamp(value=None):
    """ISO 8601 UTC to the second; `value` is Unix seconds, or None for now."""
    moment = datetime.now(timezone.utc) if value is None else datetime.fromtimestamp(value, timezone.utc)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def host_record(host, label=None):
    """The run file's host from a harness host record (s07_benchmark_measure.host_info)."""
    name = OS_NAMES.get(host.get("os", sys.platform), host.get("os", sys.platform))
    arch = ARCH_NAMES.get(host.get("architecture", ""), host.get("architecture", ""))
    cpus = host.get("cpu_capacity") or os.cpu_count()
    return {"os": name, "arch": arch, "cpus": cpus, "label": label or f"{name} {arch}, {cpus} CPUs"}


def finite(value):
    return type(value) in (int, float) and math.isfinite(value)


def run_document(workload, measurement, label=None, revision=None, recorded_at=None):
    """The status/perf run file of one measurement (docs/EVIDENCE-plan.md, section 3)."""
    ratios = measurement["ratios"]
    if not ratios:
        raise ValueError(f"{workload}: the capture yields no ratios")
    bad = sorted(name for name, value in ratios.items() if not finite(value))
    if bad:
        raise ValueError(f"{workload}: ratios that are not finite numbers: {', '.join(bad)}")
    when = recorded_at or measurement.get("recorded_at")
    return {
        "workload": workload,
        "pin": pin(),
        "revision": revision or measurement.get("revision") or head(),
        "host": host_record(measurement["host"], label),
        "recorded_at": when if isinstance(when, str) else timestamp(when),
        "ratios": {name: ratios[name] for name in sorted(ratios)},
        "samples": measurement["samples"],
    }


def dumps(value, depth=0):
    """Indented JSON with each sample list on one line."""
    if isinstance(value, dict) and value:
        pad = "  " * (depth + 1)
        items = [f"{pad}{json.dumps(key)}: {dumps(item, depth + 1)}" for key, item in value.items()]
        return "{\n" + ",\n".join(items) + "\n" + "  " * depth + "}"
    return json.dumps(value)


def run_path(out, document):
    stamp = document["recorded_at"].replace(":", "-")
    return Path(out) / document["workload"] / f"{stamp}-{document['revision'][:8]}.json"


def write_run(document, out=None):
    path = run_path(out or PERF, document)
    if path.exists():
        raise ValueError(f"{path} exists; a run is never regenerated")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(dumps(document) + "\n")
    return path


def record(args):
    workload = WORKLOADS[args.workload]
    capture = Path(args.capture or ROOT / workload.capture)
    measurement = workload.read(capture, args)
    path = write_run(run_document(args.workload, measurement, args.label), args.out).resolve()
    print(path.relative_to(ROOT) if path.is_relative_to(ROOT) else path)
    print(json.dumps(json.loads(path.read_text())["ratios"], indent=2))


# ---------------------------------------------------------------- check

def read_thresholds(path=None):
    path = Path(path or THRESHOLDS)
    if not path.exists():
        return {}
    tables = tomllib.loads(path.read_text())
    for workload, table in tables.items():
        if not isinstance(table, dict):
            raise ValueError(f"{path}: [{workload}] must be a table of <ratio> = <threshold>")
        for name, value in table.items():
            if not finite(value) or value <= 0:
                raise ValueError(f"{path}: [{workload}] {name} must be a positive finite ratio")
    return tables


def runs(workload, perf=None):
    """The workload's run files, oldest first (by recorded_at, then file name)."""
    documents = []
    for path in sorted((Path(perf or PERF) / workload).glob("*.json")):
        documents.append((json.loads(path.read_text()), path))
    documents.sort(key=lambda item: item[0]["recorded_at"])
    return documents


def in_threshold_class(host):
    return (host.get("os") == THRESHOLD_HOST["os"] and host.get("arch") == THRESHOLD_HOST["arch"]
            and (host.get("cpus") or 0) >= THRESHOLD_HOST["cpus"])


def compare(run, thresholds):
    """(ratio, value, threshold, status) rows; status is pass, miss, missing (a
    threshold the run has no ratio for) or empty (no threshold)."""
    rows = []
    for name in sorted(set(run["ratios"]) | set(thresholds)):
        value, limit = run["ratios"].get(name), thresholds.get(name)
        if limit is None:
            status = ""
        elif value is None:
            status = "missing"
        else:
            status = "pass" if value <= limit else "miss"
        rows.append((name, value, limit, status))
    return rows


def report_lines(workload, run, path, rows):
    host = run["host"]
    lines = [f"perf: {workload}, run {Path(path).name}",
             f"revision {run['revision'][:8]}, recorded {run['recorded_at']}, "
             f"host \"{host['label']}\": {host['os']} {host['arch']}, {host['cpus']} CPUs"]
    if not in_threshold_class(host):
        lines.append("note: the thresholds are authorized for macOS arm64 with at least 8 CPUs (ADR 0021); "
                     "on this host a miss is informational")
    width = max(len(name) for name, *_ in rows)
    for name, value, limit, status in rows:
        shown = "—" if value is None else f"{value:.4f}"
        bound = "" if limit is None else f"<= {limit}"
        lines.append(f"  {name:<{width}}  {shown:>8}  {bound:<8}  {status}".rstrip())
    misses = [row for row in rows if row[3] in ("miss", "missing")]
    lines.append(f"{len(misses)} of {sum(1 for row in rows if row[2] is not None)} thresholds missed")
    return lines


def summary_markdown(lines, rows):
    head, tail = lines[:-len(rows) - 1], lines[-1]
    table = ["| Ratio (Rust / Go) | Value | Threshold | |", "|---|---:|---:|---|"]
    for name, value, limit, status in rows:
        table.append(f"| `{name}` | {'—' if value is None else f'{value:.4f}'} | "
                     f"{'' if limit is None else f'≤ {limit}'} | {status} |")
    return "### " + head[0] + "\n\n" + "\n\n".join(head[1:]) + "\n\n" + "\n".join(table) + f"\n\n{tail}\n"


def check(args):
    if args.run:
        path = Path(args.run)
        run = json.loads(path.read_text())
        if run.get("workload") != args.workload:
            sys.exit(f"{path} is a {run.get('workload')} run, not {args.workload}")
    else:
        found = runs(args.workload)
        if not found:
            sys.exit(f"no runs under {PERF / args.workload}")
        run, path = found[-1]
    rows = compare(run, read_thresholds().get(args.workload, {}))
    lines = report_lines(args.workload, run, path, rows)
    print("\n".join(lines))
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as handle:
            handle.write(summary_markdown(lines, rows))
    if any(row[3] in ("miss", "missing") for row in rows):
        sys.exit(1)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("record", help="write a run file from a finished capture")
    command.add_argument("workload", choices=sorted(WORKLOADS))
    command.add_argument("--capture", help="the harness's capture directory (default: its own)")
    command.add_argument("--label", help="the host label shown on the status page")
    command.add_argument("--out", help="the status/perf directory to write under (default: status/perf)")
    command.add_argument("--graph-report", type=Path, default=ROOT / "target/s07-bindworkload/report.json",
                         help="parse-bind: the graph capture's report the benchmark capture is bound to")
    command.set_defaults(function=record)
    command = commands.add_parser("check", help="compare a run with status/perf/thresholds.toml")
    command.add_argument("workload", choices=sorted(WORKLOADS))
    command.add_argument("--run", help="a run file (default: the workload's newest)")
    command.set_defaults(function=check)
    args = parser.parse_args(argv)
    try:
        args.function(args)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        sys.exit(f"perf.py {args.command}: {error}")


if __name__ == "__main__":
    main()
