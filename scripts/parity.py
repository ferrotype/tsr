#!/usr/bin/env python3
"""Suite parity against the pin, as expectation files (docs/EVIDENCE-plan.md).

    parity.py list   <suite> [--runner PATH]
    parity.py run    <suite> --output DIR [--shard i/n] [--jobs J] [--timeout S] [--id ID ...] [--runner PATH]
    parity.py check  <suite> --results DIR [DIR ...]
    parity.py accept <suite> --results DIR [DIR ...]

A suite is a runner binary that enumerates the pin's tests (`list`) and runs
one variant per process (`run --id`), printing one JSON line per sub-test:
{"id": "<variant>/<subtest>", "state": "pass"|"fail"|"skip", "reason"?: str,
"detail"?: str}. A process that panics, aborts or exceeds the deadline fails
as the whole variant, under the variant id alone.

`run` writes DIR/results.ndjson and DIR/meta.json (and the runner's local
baseline output under DIR/local). Crashes and deadlines also retain their
command and complete stdout/stderr under DIR/crashes. `check` merges the result directories,
compares the failing set with status/parity/<suite>.json and exits non-zero
on any difference; `accept` rewrites that file, keeping every reason and
approval still in force.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
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
# The deadline guards against a hang, so it is generous: the slowest compiler
# variants (the union/intersection stress tests) take about a minute alone on a
# fast core and several on a shared 4-vCPU runner.
SUITES = {
    "compiler": Suite("tsr_testrunner", "tsr-testrunner", ("--suite", "compiler", "--mode", "single"), 600),
    "compiler-concurrent": Suite("tsr_testrunner", "tsr-testrunner", ("--suite", "compiler", "--mode", "concurrent"), 600),
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


def runner_path(suite, explicit=None):
    """The suite's runner binary: --runner PATH as given, else the executable
    Cargo reports for the release build, which honors CARGO_TARGET_DIR and
    .cargo/config (a fresh build is a no-op that still reports the path)."""
    if explicit:
        return Path(explicit).resolve()
    command = ["cargo", "build", "--release", "--locked", "-p", SUITES[suite].package,
               "--bin", SUITES[suite].binary, "--message-format=json-render-diagnostics"]
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        sys.exit(f"cargo build failed for {SUITES[suite].package}")
    executables = [json.loads(line).get("executable") for line in completed.stdout.splitlines()
                   if line.startswith("{") and '"compiler-artifact"' in line]
    executables = [path for path in executables if path and Path(path).name == SUITES[suite].binary]
    if not executables:
        sys.exit(f"cargo reported no executable for {SUITES[suite].binary}")
    return Path(executables[-1])


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


def save_process_failure(command, variant, local, reason, stdout, stderr):
    """Keep the full crash trace separately from the result row's bounded tail."""
    directory = local.parent / "crashes" / hashlib.sha256(variant.encode()).hexdigest()
    directory.mkdir(parents=True, exist_ok=True)
    for name, content in (("stdout", stdout), ("stderr", stderr)):
        # TimeoutExpired may carry bytes even with subprocess text mode.
        data = content.encode() if isinstance(content, str) else content or b""
        (directory / name).write_bytes(data)
    (directory / "process.json").write_text(json.dumps({
        "variant": variant, "command": command, "cwd": str(ROOT), "reason": reason,
    }, indent=2) + "\n")


def run_variant(suite, runner, variant, local, timeout):
    """The result lines of one variant; a crash or a deadline fails the whole variant."""
    command = runner_command(suite, runner, "run", "--id", variant, "--local", str(local))
    started = time.monotonic()
    try:
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        reason = f"deadline: {timeout} s"
        save_process_failure(command, variant, local, reason, error.stdout, error.stderr)
        return [{"id": variant, "state": "fail", "reason": reason}], time.monotonic() - started
    elapsed = time.monotonic() - started
    if completed.returncode != 0:
        reason = f"exit {completed.returncode}" if completed.returncode > 0 else f"signal {-completed.returncode}"
        save_process_failure(command, variant, local, reason, completed.stdout, completed.stderr)
        tail = completed.stderr.strip().splitlines()[-30:]
        return [{"id": variant, "state": "fail", "reason": reason, "detail": "\n".join(tail)[-4000:]}], elapsed
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
    if not rows:
        return [{"id": variant, "state": "fail", "reason": "no result lines"}], elapsed
    return rows, elapsed


def run(args):
    suite = args.suite
    runner = runner_path(suite, args.runner)
    variants = args.id or list_variants(suite, runner)
    selected, (index, count) = shard(variants, args.shard)
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    # A restarted run starts clean: no metadata of an earlier run can vouch
    # for these results, and no earlier local output can outlive its failure
    # (the pin's runner cleans its local directory the same way).
    (output / "meta.json").unlink(missing_ok=True)
    shutil.rmtree(output / "crashes", ignore_errors=True)
    local = output / "local"
    shutil.rmtree(local, ignore_errors=True)
    local.mkdir(parents=True, exist_ok=True)
    timeout = args.timeout or SUITES[suite].timeout
    jobs = args.jobs or os.cpu_count() or 1
    meta = {"suite": suite, "pin": pin(), "shard": [index, count], "total": len(variants),
            "selected": len(selected), "variants": selected, "partial": bool(args.id), "jobs": jobs,
            "timeout": timeout,
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


def variant_of(row_id, selected):
    """The selected variant a result id belongs to: the id itself (a crash or a
    deadline fails the whole variant) or the id without its sub-test suffix.
    Variant ids themselves contain slashes (`tsc/commandLine/help.js`), so
    membership decides, not the number of segments."""
    if row_id in selected:
        return row_id
    variant = row_id.rsplit("/", 1)[0]
    return variant if variant in selected else row_id


def complete(directory, meta, rows):
    """Every variant the shard selected answered, with no stranger and no
    duplicate sub-test; otherwise the run is incomplete and cannot be checked."""
    selected = set(meta["variants"])
    answered = {}
    for row in rows:
        answered.setdefault(variant_of(row["id"], selected), []).append(row["id"])
    missing = sorted(selected - set(answered))
    strangers = sorted(set(answered) - selected)
    duplicates = sorted(key for ids in answered.values() for key in set(ids) if ids.count(key) > 1)
    if missing or strangers or duplicates:
        sys.exit(f"{directory}: incomplete results ({len(missing)} selected variants without a result, "
                 f"{len(strangers)} results of unselected variants, {len(duplicates)} duplicate sub-tests); "
                 f"first: {(missing or strangers or duplicates)[0]}")


def merge(suite, directories):
    """(meta, rows) of a complete set of result directories: every shard once,
    every shard complete, the shards' variants partitioning the total."""
    metas, rows, variants = [], [], []
    for directory in map(Path, directories):
        meta = json.loads((directory / "meta.json").read_text())
        if meta["suite"] != suite:
            sys.exit(f"{directory}: results of suite {meta['suite']}, not {suite}")
        with (directory / "results.ndjson").open() as results:
            shard_rows = [json.loads(line) for line in results if line.strip()]
        complete(directory, meta, shard_rows)
        metas.append(meta)
        rows.extend(shard_rows)
        variants.extend(meta["variants"])
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
    total = totals.pop()
    if len(variants) != total or len(set(variants)) != total:
        sys.exit(f"the shards answer {len(set(variants))} distinct variants of the {total} enumerated")
    return {"pin": pins.pop(), "total": total}, rows


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
    runner = runner_path(args.suite, args.runner)
    print("\n".join(list_variants(args.suite, runner)))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name, function in (("list", list_command), ("run", run), ("check", check), ("accept", accept)):
        command = commands.add_parser(name)
        command.add_argument("suite", choices=sorted(SUITES))
        command.set_defaults(function=function)
        if name in ("list", "run"):
            command.add_argument("--runner", help="a prebuilt runner binary to use instead of building one")
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
