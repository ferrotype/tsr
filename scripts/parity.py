#!/usr/bin/env python3
"""Suite parity against the pin, as expectation files (docs/EVIDENCE-plan.md).

    parity.py list   <suite> [--runner PATH | --no-build]
    parity.py run    <suite> --output DIR [--shard i/n] [--jobs J] [--timeout S] [--id ID ...] [--runner PATH | --no-build]
    parity.py check  <suite> --results DIR [DIR ...]
    parity.py accept <suite> --results DIR [DIR ...]

A suite is a runner binary that enumerates the pin's tests (`list`) and runs
one variant per process (`run --id`), printing one JSON line per sub-test:
{"id": "<variant>/<subtest>", "state": "pass"|"fail"|"skip", "reason"?: str,
"detail"?: str}. A process that panics, aborts or exceeds the deadline fails
as the whole variant, under the variant id alone.

`run` writes DIR/results.ndjson and DIR/meta.json (and the runner's local
baseline output under DIR/local). `check` merges the result directories,
compares the failing set with status/parity/<suite>.json and exits non-zero
on any difference; `accept` rewrites that file, keeping every reason and
approval still in force.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
PARITY = ROOT / "status/parity"
UPSTREAM = ROOT / "data/upstream.json"
STATES = ("pass", "fail", "skip")


@dataclass(frozen=True)
class Suite:
    package: str
    binary: str
    arguments: tuple[str, ...]
    timeout: int


# `arguments` precede `list` / `run --id ID --local DIR` on the runner's command line.
SUITES = {
    "compiler": Suite("tsr_testrunner", "tsr-testrunner", ("--suite", "compiler", "--mode", "single"), 120),
    "compiler-concurrent": Suite("tsr_testrunner", "tsr-testrunner", ("--suite", "compiler", "--mode", "concurrent"), 120),
    "transpile": Suite("tsr_testrunner", "tsr-testrunner", ("--suite", "transpile"), 60),
    "tsc": Suite("phase4_tsctests", "phase4_tsctests", ("--suite", "tsc"), 120),
}


def pin():
    return json.loads(UPSTREAM.read_text())["pin"]


def expectation_path(suite):
    return PARITY / f"{suite}.json"


def read_expectation(suite):
    path = expectation_path(suite)
    if not path.exists():
        return {"suite": suite, "pin": pin(), "total": 0, "failing": {}}
    return json.loads(path.read_text())


def write_expectation(suite, document):
    document = {"suite": suite, "pin": document["pin"], "total": document["total"],
                "failing": {key: document["failing"][key] for key in sorted(document["failing"])}}
    expectation_path(suite).parent.mkdir(parents=True, exist_ok=True)
    expectation_path(suite).write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n")


def runner_path(suite, build, explicit=None):
    """The suite's runner binary: --runner PATH as given, else target/release,
    built in release unless --no-build."""
    if explicit:
        return Path(explicit).resolve()
    binary = ROOT / "target/release" / SUITES[suite].binary
    if build:
        subprocess.run(["cargo", "build", "--release", "--locked", "-p", SUITES[suite].package,
                        "--bin", SUITES[suite].binary], cwd=ROOT, check=True)
    if not binary.exists():
        sys.exit(f"runner binary missing: {binary} (build it, or drop --no-build)")
    return binary


def runner_command(suite, runner, *rest):
    return [str(runner), *SUITES[suite].arguments, *rest]


def list_variants(suite, runner):
    completed = subprocess.run(runner_command(suite, runner, "list"), cwd=ROOT, check=True,
                               capture_output=True, text=True)
    variants = sorted(set(line for line in completed.stdout.splitlines() if line.strip()))
    if not variants:
        sys.exit(f"{suite}: the runner enumerated no variants")
    return variants


def shard(variants, spec):
    if spec is None:
        return variants, (1, 1)
    index, count = (int(part) for part in spec.split("/"))
    if not 1 <= index <= count:
        sys.exit(f"--shard {spec}: the index must be between 1 and the shard count")
    return variants[index - 1::count], (index, count)


def run_variant(suite, runner, variant, local, timeout):
    """The result lines of one variant; a crash or a deadline fails the whole variant."""
    command = runner_command(suite, runner, "run", "--id", variant, "--local", str(local))
    started = time.monotonic()
    try:
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return [{"id": variant, "state": "fail", "reason": f"deadline: {timeout} s"}], time.monotonic() - started
    elapsed = time.monotonic() - started
    rows = []
    for line in completed.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            return [{"id": variant, "state": "fail", "reason": "malformed result line", "detail": line[:2000]}], elapsed
        if not (isinstance(row, dict) and isinstance(row.get("id"), str) and row.get("state") in STATES
                and row["id"].startswith(variant + "/")):
            return [{"id": variant, "state": "fail", "reason": "malformed result line", "detail": line[:2000]}], elapsed
        rows.append(row)
    if completed.returncode != 0:
        tail = completed.stderr.strip().splitlines()[-30:]
        reason = f"exit {completed.returncode}" if completed.returncode > 0 else f"signal {-completed.returncode}"
        return [{"id": variant, "state": "fail", "reason": reason, "detail": "\n".join(tail)[:4000]}], elapsed
    if not rows:
        return [{"id": variant, "state": "fail", "reason": "no result lines"}], elapsed
    return rows, elapsed


def run(args):
    suite = args.suite
    runner = runner_path(suite, not args.no_build, args.runner)
    variants = args.id or list_variants(suite, runner)
    selected, (index, count) = shard(variants, args.shard)
    output = Path(args.output)
    local = output / "local"
    local.mkdir(parents=True, exist_ok=True)
    timeout = args.timeout or SUITES[suite].timeout
    jobs = args.jobs or os.cpu_count() or 1
    meta = {"suite": suite, "pin": pin(), "shard": [index, count], "total": len(variants),
            "selected": len(selected), "partial": bool(args.id), "jobs": jobs, "timeout": timeout,
            "host": {"system": platform.system(), "machine": platform.machine(), "cpus": os.cpu_count()},
            "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    counts = dict.fromkeys(STATES, 0)
    slowest = []
    with (output / "results.ndjson").open("w") as results, ThreadPoolExecutor(jobs) as pool:
        futures = {pool.submit(run_variant, suite, runner, variant, local, timeout): variant for variant in selected}
        for done, future in enumerate(as_completed(futures), 1):
            rows, elapsed = future.result()
            slowest = sorted(slowest + [(elapsed, futures[future])], reverse=True)[:5]
            for row in rows:
                counts[row["state"]] += 1
                results.write(json.dumps(row, ensure_ascii=False) + "\n")
            if done % 500 == 0 or done == len(selected):
                print(f"{done}/{len(selected)} variants; sub-tests pass {counts['pass']} fail {counts['fail']} "
                      f"skip {counts['skip']}", file=sys.stderr)
    meta["finished"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    meta["counts"] = counts
    meta["slowest"] = [{"variant": variant, "seconds": round(seconds, 1)} for seconds, variant in slowest]
    (output / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    print(json.dumps({"suite": suite, **meta["counts"], "variants": len(selected)}))


def merge(suite, directories):
    """(meta, rows) of a complete set of result directories: every shard once."""
    metas, rows = [], []
    for directory in map(Path, directories):
        meta = json.loads((directory / "meta.json").read_text())
        if meta["suite"] != suite:
            sys.exit(f"{directory}: results of suite {meta['suite']}, not {suite}")
        metas.append(meta)
        with (directory / "results.ndjson").open() as results:
            rows.extend(json.loads(line) for line in results if line.strip())
    counts = {meta["shard"][1] for meta in metas}
    pins = {meta["pin"] for meta in metas}
    totals = {meta["total"] for meta in metas}
    if len(counts) != 1 or len(pins) != 1 or len(totals) != 1:
        sys.exit("the result directories disagree on shard count, pin or total")
    seen = sorted(meta["shard"][0] for meta in metas)
    if seen != list(range(1, counts.pop() + 1)):
        sys.exit(f"shards present: {seen}; every shard must appear exactly once")
    if any(meta.get("partial") for meta in metas):
        sys.exit("a `run --id` result is a development check and cannot be checked or accepted")
    return {"pin": pins.pop(), "total": totals.pop()}, rows


def failing_ids(rows):
    return {row["id"]: row for row in rows if row["state"] == "fail"}


def summary(expectation, meta, rows, new_failures, now_passing):
    failing = failing_ids(rows)
    approved = sum(1 for key in failing if expectation["failing"].get(key, {}).get("approved"))
    counts = {state: sum(1 for row in rows if row["state"] == state) for state in STATES}
    lines = [f"suite {expectation['suite']}: {meta['total']} variants; sub-tests pass {counts['pass']}, "
             f"skip {counts['skip']}, fail {counts['fail']} ({approved} approved)"]
    if new_failures:
        lines.append(f"{len(new_failures)} new failure(s) not named in status/parity/{expectation['suite']}.json:")
        lines.extend(f"  {key}: {failing[key].get('reason', '')}".rstrip(": ") for key in new_failures)
    if now_passing:
        lines.append(f"{len(now_passing)} named failure(s) now pass; remove them with `parity.py accept`:")
        lines.extend(f"  {key}" for key in now_passing)
    return lines


def check(args):
    suite = args.suite
    expectation = read_expectation(suite)
    meta, rows = merge(suite, args.results)
    problems = []
    if meta["pin"] != expectation["pin"]:
        problems.append(f"pin {meta['pin']} differs from the expectation file's {expectation['pin']}")
    if meta["total"] != expectation["total"]:
        problems.append(f"{meta['total']} variants enumerated; the expectation file records {expectation['total']}")
    failing = failing_ids(rows)
    new_failures = sorted(set(failing) - set(expectation["failing"]))
    now_passing = sorted(set(expectation["failing"]) - set(failing))
    lines = problems + summary(expectation, meta, rows, new_failures, now_passing)
    print("\n".join(lines))
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as handle:
            handle.write(f"### parity: {suite}\n\n```\n" + "\n".join(lines) + "\n```\n")
    if problems or new_failures or now_passing:
        sys.exit(1)


def accept(args):
    suite = args.suite
    expectation = read_expectation(suite)
    meta, rows = merge(suite, args.results)
    failing = {}
    for key, row in sorted(failing_ids(rows).items()):
        entry = dict(expectation["failing"].get(key) or {})
        if "reason" not in entry:
            entry["reason"] = row.get("reason") or "unexplained"
            if row.get("detail"):
                entry["detail"] = row["detail"].splitlines()[0][:200]
        failing[key] = entry
    write_expectation(suite, {"pin": meta["pin"], "total": meta["total"], "failing": failing})
    print(f"status/parity/{suite}.json: {meta['total']} variants, {len(failing)} failing sub-tests")


def list_command(args):
    runner = runner_path(args.suite, not args.no_build, args.runner)
    print("\n".join(list_variants(args.suite, runner)))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name, function in (("list", list_command), ("run", run), ("check", check), ("accept", accept)):
        command = commands.add_parser(name)
        command.add_argument("suite", choices=sorted(SUITES))
        command.set_defaults(function=function)
        if name in ("list", "run"):
            command.add_argument("--no-build", action="store_true", help="use the built runner as is")
            command.add_argument("--runner", help="the runner binary to use instead of target/release")
        if name == "run":
            command.add_argument("--output", required=True)
            command.add_argument("--shard", help="i/n: this shard of the sorted variant list")
            command.add_argument("--jobs", type=int)
            command.add_argument("--timeout", type=int, help="per-variant deadline in seconds")
            command.add_argument("--id", action="append", help="run only these variants")
        if name in ("check", "accept"):
            command.add_argument("--results", nargs="+", required=True, help="result directories, every shard")
    args = parser.parse_args(argv)
    args.function(args)


if __name__ == "__main__":
    main()
