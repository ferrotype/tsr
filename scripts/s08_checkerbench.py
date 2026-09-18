#!/usr/bin/env python3
"""S08 checkerbench: the frozen checker workload (data/s08/checker-workload.json) on the
pinned Go harness and the Rust port, one fresh process per sample, plus the per-type
footprint census (data/s08/type-footprint.json) at the retained checkpoint.

  build     compile the three Rust executables (normal, phase, alloc) and the Go test binary
  capture   run warmups and the alternating seven-sample batches for every executable kind
  report    aggregate the raw samples into ratios, stability flags and the footprint statistic
  producer  emit the run.checkerbench.* metrics for the ledger when a current report exists
"""
import argparse
import hashlib
import json
import math
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path
from statistics import median, mean, pstdev

from s04 import go_environment, verified_upstream
from s04_common import command, strict_json_loads
from s07_benchmark import cargo_executable, native_environment
from s07_benchmark_measure import host_info, reject_concurrent_builds
from s07_benchmark_stats import ratio_summary
from s08_oracle import ROOT, canonical, digest
from s08_p4 import canonical as request_canonical
import s08_baselines
import s08_measurement as measurement
from s08_census_runtime import OBSERVER_SOURCES, runtime_overlay
from s08_e2_contract import inventory

DEFAULT = ROOT / "target/s08/checkerbench"
CORPUS = ROOT / "target/s08/e2/corpus"
METHOD = ROOT / "data/s08/checker-workload.json"
FOOTPRINT = ROOT / "data/s08/type-footprint.json"
BENCH = ROOT / "tools/s08/oracle/checkerbench"
FAMILIES = ROOT / "tools/s08/oracle/families/bridge.go"
MODES = {"normal": [], "phase": ["s08-phase-timer"], "alloc": ["s08-allocation"]}
RUNTIMES = ("go", "rust")
SAMPLE_TIMEOUT = 4 * 3600


def method():
    return strict_json_loads(METHOD.read_bytes())


def sources():
    """Every input the measurement depends on, so a report can be tied to exact source bytes."""
    result = {}
    patterns = ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.*", "rust-toolchain*", ".cargo/**/*",
                "tools/s08/p4/**/*", "tools/s08/p5/**/*", "tools/s08/p7/**/*", "tools/s08/oracle/**/*",
                "tools/s07/program/*.rs", "tools/s07/config/host.rs",
                "scripts/s08_checkerbench.py", "scripts/s08_census_runtime.py", "scripts/s08_measurement.py", "scripts/s08_e2_contract.py", "scripts/s08_p4.py", "scripts/s08_p5_corpus.py", "scripts/s08_manifest.py", "scripts/s07_acceptance.py", "scripts/s08_baselines.py", "scripts/s08_oracle.py",
                "scripts/s07_benchmark.py", "scripts/s07_benchmark_stats.py", "scripts/s07_benchmark_measure.py", "scripts/s07_subset.py",
                "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                "data/s08/checker-workload.json", "data/s08/type-footprint.json", "data/s08/baseline-requests.json",
                "data/s07/subset.json", "data/upstream.json", ".gitmodules", "data/s04/toolchains.toml")
    for pattern in patterns:
        for path in ROOT.glob(pattern):
            if path.is_file() and "__pycache__" not in path.parts:
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return result


def fingerprint(files):
    return digest(canonical(files))


def frozen_ids():
    manifest = strict_json_loads((ROOT / "data/s08/baseline-requests.json").read_bytes())
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    if manifest["pin"] != pin or method()["pin"] != pin:
        raise ValueError("checkerbench pin drift")
    return [r["id"] for r in manifest["requests"] if r["acceptance_tier"] == "acceptance"]


def prepare_requests(directory, smoke=None):
    """Rust requests come from the current E2 corpus capture (identical bound inputs to the
    verified parity run); Go requests are the frozen harness identities of the same variants."""
    if not (CORPUS / "requests.json").exists():
        raise ValueError("no E2 corpus capture at target/s08/e2/corpus; capture E2 first (docs/S08-E2.md)")
    rust = [r for r in strict_json_loads((CORPUS / "requests.json").read_bytes()) if r["acceptance_tier"] == "acceptance"]
    ids = frozen_ids()
    if [r["id"] for r in rust] != ids or len(ids) != method()["acceptance_variants"]:
        raise ValueError("E2 corpus requests do not match the frozen acceptance inventory")
    subset = strict_json_loads((ROOT / "data/s07/subset.json").read_bytes())
    _, go_all = s08_baselines.requests_from_subset(subset)
    by_id = {r["id"]: r for r in go_all}
    go = [by_id[i] for i in ids]
    inventory(rust, strict_json_loads((ROOT / "data/s08/baseline-requests.json").read_bytes())["requests"], partial=True)
    if smoke is not None and (type(smoke) is not int or not 1 <= smoke <= len(ids)):
        raise ValueError("smoke count must select a nonempty acceptance prefix")
    if smoke:
        rust, go = rust[:smoke], go[:smoke]
    # Paths and raw config contain ordered semantic maps, as in the frozen E2
    # loading fingerprint. Metadata canonicalization would reorder their keys.
    (directory / "rust-requests.json").write_bytes(request_canonical(rust) + b"\n")
    (directory / "go-requests.json").write_bytes(request_canonical(go) + b"\n")
    return {"variants": len(rust), "smoke": smoke, "ids_sha256": digest(canonical([r["id"] for r in rust])),
            "rust_requests_sha256": digest((directory / "rust-requests.json").read_bytes()),
            "go_requests_sha256": digest((directory / "go-requests.json").read_bytes())}


def overlay_sources(upstream):
    """The E2 baseline overlay plus the checkerbench clocks, compile hook and count-only walker."""
    replace = s08_baselines.replace_exact
    sources = s08_baselines.overlay_sources(upstream)
    harness = sources["testutil/harnessutil/harnessutil.go"]
    harness = replace(harness, "\tctx := context.Background()\n\n\tvar preErrors []*ast.Diagnostic\n",
                      "\tif S08CheckerbenchCompile != nil {\n\t\treturn S08CheckerbenchCompile(host, config, harnessOptions)\n\t}\n"
                      "\tctx := context.Background()\n\n\tvar preErrors []*ast.Diagnostic\n")
    sources["testutil/harnessutil/harnessutil.go"] = harness
    walker = sources["testutil/tsbaseline/type_symbol_baseline.go"]
    walker = replace(walker, "\t\t\tbuilder := checker.NewNodeBuilder(fileChecker, ctx)\n",
                     "\t\t\ts08DisplayBegin()\n\t\t\tbuilder := checker.NewNodeBuilder(fileChecker, ctx)\n")
    walker = replace(walker, "\t\t\ttypeString = writer.String()\n", "\t\t\ttypeString = writer.String()\n\t\t\ts08DisplayEnd()\n")
    symbol = "\tsymbolString.WriteString(ast.EscapeAllInternalSymbolNames(fileChecker.SymbolToStringEx(symbol, node.Parent, ast.SymbolFlagsNone, checker.SymbolFormatFlagsAllowAnyNodeKind)))\n"
    walker = replace(walker, symbol,
                     "\ts08DisplayBegin()\n\ts08SymbolText := fileChecker.SymbolToStringEx(symbol, node.Parent, ast.SymbolFlagsNone, checker.SymbolFormatFlagsAllowAnyNodeKind)\n"
                     "\ts08DisplayEnd()\n\tsymbolString.WriteString(ast.EscapeAllInternalSymbolNames(s08SymbolText))\n")
    # Baseline assembly is outside the interval; the native walk itself resumes it.
    walker = replace(walker, "\tvar result strings.Builder\n", "\trestoreClock := core.S08Bench.Pause()\n\tdefer restoreClock()\n\tvar result strings.Builder\n")
    walker = replace(walker, "\t\tvar results []*typeWriterResult\n", "\t\tvar results []*typeWriterResult\n\t\tcore.S08Bench.Start()\n")
    walker = replace(walker, "\t\tlastIndexWritten := -1\n", "\t\tcore.S08Bench.Stop()\n\t\tlastIndexWritten := -1\n")
    sources["testutil/tsbaseline/type_symbol_baseline.go"] = walker
    bridge = sources["testutil/tsbaseline/s08_baselines_bridge.go"]
    bridge = replace(bridge, '\t"encoding/hex"\n',
                     '\t"encoding/hex"\n\t"github.com/microsoft/TypeScript/tsc/internal/core"\n')
    bridge = replace(bridge, "\tS08Queries = append(S08Queries, q)\n", "\ts08Record(q)\n", 3)
    bridge = replace(bridge, "\tresult := c.GetTypeAtLocation(node)\n",
                     "\tresult := c.GetTypeAtLocation(node)\n\ts08RestoreRoots := core.S08Bench.Pause()\n\tif S08CollectRoots && result != nil {\n\t\tS08Roots = append(S08Roots, result)\n\t}\n\ts08RestoreRoots()\n")
    bridge = replace(bridge, '\t\t\treturn map[string]any{"operation":stamp.Operation,',
                     '\t\t\trestoreClock := core.S08Bench.Pause()\n\t\t\tdefer restoreClock()\n\t\t\tcore.S08Bench.Start()\n\t\t\ts08DisplayBegin()\n\t\t\ts08Text := c.TypeToString(result)\n\t\t\ts08DisplayEnd()\n\t\t\tcore.S08Bench.Stop()\n\t\t\treturn map[string]any{"operation":stamp.Operation,')
    bridge = replace(bridge, 'hex.EncodeToString([]byte(c.TypeToString(result)))', 'hex.EncodeToString([]byte(s08Text))')
    sources["testutil/tsbaseline/s08_baselines_bridge.go"] = bridge
    pool = (upstream / "tsc/internal/compiler/checkerpool.go").read_text()
    pool = replace(pool, "\t\t\t\tp.checkers[i], p.locks[i] = checker.NewChecker(p.program, tracer)\n",
                   "\t\t\t\ts08CheckerInitBegin()\n\t\t\t\tp.checkers[i], p.locks[i] = checker.NewChecker(p.program, tracer)\n\t\t\t\ts08CheckerInitEnd()\n")
    sources["compiler/checkerpool.go"] = pool
    sources["core/s08_checkerbench_clock.go"] = (BENCH / "core_clock.go").read_text()
    sources["compiler/s08_checkerbench_hooks.go"] = (BENCH / "compiler_hooks.go").read_text()
    sources["testutil/harnessutil/s08_checkerbench.go"] = (BENCH / "harness_hooks.go").read_text()
    sources["testutil/tsbaseline/s08_checkerbench_walker.go"] = (BENCH / "walker_hooks.go").read_text()
    sources["testrunner/s08_checkerbench_test.go"] = (BENCH / "driver_test.go").read_text()
    sources["checker/s08_families_bridge.go"] = FAMILIES.read_text()
    sources["checker/s08_families_census_v2.go"] = (FAMILIES.parent / "census_v2.go").read_text()
    sources["checker/s08_census_allocations.go"] = (FAMILIES.parent / "census_allocations.go").read_text()
    return sources


def write_overlay(directory, upstream):
    replacements = {}
    for name, source in overlay_sources(upstream).items():
        path = directory / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        virtual = upstream / "tsc/internal" / name
        if virtual.exists() and virtual.read_text() == source:
            raise ValueError(f"overlay entry is not a modification: {name}")
        replacements[str(virtual)] = str(path)
    (directory / "overlay.json").write_bytes(canonical({"Replace": replacements}) + b"\n")
    return directory / "overlay.json"


def build(directory, modes=None):
    """Release executables, with a separate Go allocation observer binary.

    Bounded census checks request only alloc; acceptance captures build all modes.
    """
    selected_modes = tuple(MODES) if modes is None else tuple(modes)
    if not selected_modes or not set(selected_modes) <= set(MODES):
        raise ValueError("invalid checkerbench build modes")
    reject_concurrent_builds()
    initial_sources = sources()
    directory.mkdir(parents=True, exist_ok=True)
    bin_dir = directory / "bin"
    bin_dir.mkdir(exist_ok=True)
    env = native_environment()
    manifest = ROOT / "crates/ts_compiler/Cargo.toml"
    binaries = {}
    for mode in selected_modes:
        features = MODES[mode]
        args = ["cargo", "build", "--release", "--locked", "--example", "p7_checkerbench", "--message-format=json",
                "--manifest-path", str(manifest)]
        if features:
            args += ["--features", ",".join(features)]
        messages = command(args, cwd=ROOT, env=env).decode()
        executable = cargo_executable(messages, manifest, "p7_checkerbench", "example", features)
        target = bin_dir / f"rust-{mode}"
        shutil.copy2(executable, target)
        binaries[f"rust-{mode}"] = {"path": str(target), "sha256": digest(target.read_bytes()), "features": features, "command": args}
    upstream = verified_upstream()
    overlay = write_overlay(directory, upstream)
    go_env = go_environment()
    repo_flag = "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=" + str(directory / "unmatched-prefix")
    target = bin_dir / "go-checkerbench.test"
    args = ["go", "test", "-c", "-o", str(target), "-trimpath", "-mod=readonly", repo_flag, "-overlay", str(overlay), "./internal/testrunner"]
    command(args, cwd=upstream / "tsc", env=go_env)
    binaries["go"] = {"path": str(target), "sha256": digest(target.read_bytes()), "command": args,
                      "go": command(["go", "version"], cwd=ROOT, env=go_env).decode().strip()}
    allocation_overlay = strict_json_loads(overlay.read_bytes())["Replace"]
    observer_hashes = runtime_overlay(directory, allocation_overlay, upstream, go_env)
    allocation_overlay_path = directory / "allocation-overlay.json"
    allocation_overlay_path.write_bytes(canonical({"Replace": allocation_overlay}) + b"\n")
    target = bin_dir / "go-checkerbench-alloc.test"
    args = ["go", "test", "-c", "-o", str(target), "-trimpath", "-mod=readonly", repo_flag, "-overlay", str(allocation_overlay_path), "./internal/testrunner"]
    command(args, cwd=upstream / "tsc", env=go_env)
    binaries["go-alloc"] = {"path": str(target), "sha256": digest(target.read_bytes()), "command": args,
                            "runtime_observer": "requested-allocation-provenance-v1",
                            "observer_sources_sha256": observer_hashes,
                            "sdk_sha256": digest((directory / "runtime-overlay/sdk.json").read_bytes())}
    verify_runtime_observer(directory, binaries["go-alloc"])
    verified_upstream()
    if sources() != initial_sources:
        raise ValueError("measurement sources changed during build")
    report = {"version": 1, "binaries": binaries, "sources": initial_sources, "rust_toolchain": command(["rustc", "--version"], cwd=ROOT, env=env).decode().strip()}
    report["sources_sha256"] = fingerprint(report["sources"])
    (directory / "build.json").write_bytes(canonical(report) + b"\n")
    return report


def run_child(argv, env, cwd, stdout_path, timeout):
    """One sample process with its own resource accounting; never a reused process."""
    with open(stdout_path, "wb") as output, open(str(stdout_path) + ".stderr", "wb") as error:
        started = time.monotonic_ns()
        child = subprocess.Popen(argv, stdout=output, stderr=error, env=env, cwd=cwd)
        deadline = time.monotonic() + timeout
        try:
            while True:
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    child.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.monotonic() >= deadline:
                    raise ValueError("checkerbench child exceeded the fixed process deadline")
                time.sleep(0.05)
        finally:
            if child.returncode is None:
                child.kill()
                _, status, _ = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
        process_ns = time.monotonic_ns() - started
    rss = usage.ru_maxrss * (1024 if sys.platform == "linux" else 1)
    return {"returncode": child.returncode, "process_ns": process_ns, "peak_rss_bytes": rss,
            "user_time_ns": round(usage.ru_utime * 1e9), "system_time_ns": round(usage.ru_stime * 1e9)}


def sample(directory, build_report, runtime, mode, label):
    samples = directory / "samples" / mode
    samples.mkdir(parents=True, exist_ok=True)
    rows = samples / f"{runtime}-{label}.rows.ndjson"
    stdout = samples / f"{runtime}-{label}.stdout"
    env = native_environment()
    if runtime == "rust":
        binary = build_report["binaries"][f"rust-{mode}"]["path"]
        result = run_child([binary, str(directory / "rust-requests.json"), str(rows)], env, ROOT, stdout, SAMPLE_TIMEOUT)
        if result["returncode"] != 0:
            raise ValueError(f"rust {mode} sample failed: see {stdout}.stderr")
        totals = strict_json_loads(stdout.read_bytes())
    else:
        summary = samples / f"{runtime}-{label}.summary.json"
        env.update(go_environment())
        env.update(S08_MODE=mode, S08_REQUESTS=str(directory / "go-requests.json"), S08_OUTPUT=str(rows),
                   S08_SUMMARY=str(summary), TS_TEST_PROGRAM_SINGLE_THREADED="1")
        for key in ("GOMEMLIMIT", "GODEBUG", "GOMAXPROCS"):
            env.pop(key, None)
        env["GOGC"] = "100"
        binary = build_report["binaries"]["go-alloc" if mode == "alloc" else "go"]["path"]
        result = run_child([binary, "-test.run", "^TestS08Checkerbench$", "-test.count=1", "-test.timeout=0"],
                           env, verified_upstream() / "tsc", stdout, SAMPLE_TIMEOUT)
        if result["returncode"] != 0 or not summary.exists():
            raise ValueError(f"go {mode} sample failed: see {stdout}")
        totals = strict_json_loads(summary.read_bytes())
    if totals["mode"] != mode:
        raise ValueError("sample executable mode mismatch")
    if totals["failed"] or totals["executed"] != totals["variants"]:
        raise ValueError(f"{runtime} {mode} sample did not complete the fixed work: {totals['failures'][:5]}")
    artifacts = {p.name: measurement.file_digest(p) for p in (rows, stdout, Path(str(stdout) + ".stderr"))}
    if runtime == "go":
        artifacts[summary.name] = measurement.file_digest(summary)
    return {"artifacts": artifacts, "runtime": runtime, "mode": mode, "label": label, "totals": totals, "process": result,
            "rows_sha256": digest(rows.read_bytes()), "load_average": os.getloadavg()}


def capture(directory, samples_per_runtime=7, smoke=None):
    if any((directory / name).exists() for name in ("capture.json", "capture-failure.json")):
        raise ValueError("capture already exists; select a new output directory")
    directory.mkdir(parents=True, exist_ok=True)
    plan = method()
    if type(samples_per_runtime) is not int or not 1 <= samples_per_runtime <= len(plan["sampling"]["measured_order"]):
        raise ValueError("invalid measurement sample count")
    if samples_per_runtime != plan["sampling"]["measured_samples_per_runtime"] and smoke is None:
        raise ValueError("full captures use the frozen sample count")
    requests = prepare_requests(directory, smoke)
    # Check the bytes the children will read, not just the pre-write objects.
    # Share the replay checks so a writer regression fails before compilation.
    ids = verify_requests(directory, requests, smoke)
    build_report = build(directory)
    host = host_info() if not smoke else {"os": sys.platform, "architecture": platform.machine(), "smoke": True}
    runs = []
    capture_report = {"version": 2, "pin": plan["pin"], "host": host, "smoke": smoke, "requests": requests,
                      "build_sha256": digest((directory / "build.json").read_bytes()), "sources_sha256": build_report["sources_sha256"],
                      "method_sha256": digest(METHOD.read_bytes()), "footprint_sha256": digest(FOOTPRINT.read_bytes()),
                      "samples_per_runtime": samples_per_runtime, "runs": runs}
    identity = None

    def take_sample(runtime, mode, label, warmup):
        nonlocal identity
        try:
            run = {**sample(directory, build_report, runtime, mode, label), "warmup": warmup}
            runs.append(run)
            identity = verify_sample(directory, run, requests, ids, identity)
        except (OSError, ValueError, RuntimeError, KeyError, TypeError, subprocess.SubprocessError) as error:
            # Keep the failed sample and completed prefix for diagnosis, without
            # publishing an incomplete batch as capture.json.
            failure = {**capture_report, "failure": {"runtime": runtime, "mode": mode, "label": label,
                                                    "reason": str(error)}, "finished": time.time()}
            (directory / "capture-failure.json").write_bytes(canonical(failure) + b"\n")
            raise

    order = [[r.lower() for r in pair] for pair in plan["sampling"]["measured_order"]][:samples_per_runtime]
    for mode in MODES:
        for warmup in [r.lower() for r in plan["sampling"]["warmup_order"]]:
            take_sample(warmup, mode, "warmup", True)
        for index, pair in enumerate(order):
            for runtime in pair:
                take_sample(runtime, mode, f"sample-{index}", False)
    capture_report["finished"] = time.time()
    (directory / "capture.json").write_bytes(canonical(capture_report) + b"\n")
    return capture_report


def rows_of(directory, run):
    path = directory / "samples" / run["mode"] / f"{run['runtime']}-{run['label']}.rows.ndjson"
    return [strict_json_loads(line) for line in measurement.authenticated(path, run["rows_sha256"]).splitlines()]


def verify_requests(directory, inputs, smoke):
    """Authenticate serialized requests against the frozen semantic inventory."""
    rust = strict_json_loads(measurement.authenticated(directory / 'rust-requests.json', inputs['rust_requests_sha256']))
    go = strict_json_loads(measurement.authenticated(directory / 'go-requests.json', inputs['go_requests_sha256']))
    ids = frozen_ids()
    if smoke is not None:
        if type(smoke) is not int or not 1 <= smoke <= len(ids):
            raise ValueError('invalid smoke inventory')
        ids = ids[:smoke]
    if inputs['smoke'] != smoke or inputs['variants'] != len(ids) or inputs['ids_sha256'] != digest(canonical(ids)):
        raise ValueError('measurement request count/identity differs')
    if [r['id'] for r in rust] != ids or [r['id'] for r in go] != ids:
        raise ValueError('measurement requests differ from frozen inventory')
    manifest = strict_json_loads((ROOT / 'data/s08/baseline-requests.json').read_bytes())
    inventory(rust, manifest['requests'], partial=True)
    _, native = s08_baselines.requests_from_subset(strict_json_loads((ROOT / 'data/s07/subset.json').read_bytes()))
    selected = {r['id']: r for r in native}
    if go != [selected[i] for i in ids]:
        raise ValueError('native measurement requests changed')
    return ids


def verify_sample(directory, run, inputs, ids, identity=None):
    """Reconstruct a sample from raw rows before accepting its work identity."""
    prefix = f"{run['runtime']}-{run['label']}"
    names = {prefix + suffix for suffix in ('.rows.ndjson', '.stdout', '.stdout.stderr')}
    if run['runtime'] == 'go':
        names.add(prefix + '.summary.json')
    if set(run['artifacts']) != names or run['process']['returncode'] != 0:
        raise ValueError('missing sample artifacts or failed process')
    sample_dir = directory / 'samples' / run['mode']
    for name, sha in run['artifacts'].items():
        measurement.authenticated(sample_dir / name, sha)
    totals_path = sample_dir / (prefix + ('.stdout' if run['runtime'] == 'rust' else '.summary.json'))
    totals = strict_json_loads(totals_path.read_bytes())
    if totals != run['totals'] or totals['version'] != 2 or totals['mode'] != run['mode']:
        raise ValueError('sample totals or executable mode changed')
    request_hash = inputs[run['runtime'] + '_requests_sha256']
    if totals['request_sha256'] != request_hash:
        raise ValueError('child loaded different requests')
    rows = rows_of(directory, run)
    measurement.check_totals(totals, measurement.checker_rows(rows, ids, run['mode']))
    observed = [(r['id'], r['actions'], r['output_sha256']) for r in rows]
    if identity is not None and identity != observed:
        different = next(row[0] for previous, row in zip(identity, observed, strict=True) if previous != row)
        raise ValueError('measurement output or action schedule differs across samples/runtimes: '
                         f"{run['runtime']} {run['mode']} {run['label']}, variant {different}")
    return observed


def verify_runtime_observer(directory, observer):
    if observer.get('runtime_observer') != 'requested-allocation-provenance-v1':
        raise ValueError('allocation provenance observer missing')
    hashes = observer.get('observer_sources_sha256')
    if not isinstance(hashes, dict) or set(hashes) != set(OBSERVER_SOURCES):
        raise ValueError('allocation observer source inventory differs')
    for name, expected in hashes.items():
        # Authenticate both the bytes compiled through the overlay and their
        # current source, independently of the broader capture fingerprint.
        measurement.authenticated(directory / 'runtime-overlay' / name, expected)
        measurement.authenticated(ROOT / 'tools/s08/oracle/families' / name, expected)
    measurement.authenticated(directory / 'runtime-overlay/sdk.json', observer['sdk_sha256'])


def verify_capture(directory, capture_report):
    plan = method()
    build = measurement.build_record(directory, capture_report, sources(), METHOD)
    if set(build['binaries']) != {'go', 'go-alloc', *(f'rust-{mode}' for mode in MODES)}:
        raise ValueError('measurement executable inventory differs')
    verify_runtime_observer(directory, build['binaries']['go-alloc'])
    if capture_report['pin'] != plan['pin'] or capture_report['footprint_sha256'] != measurement.file_digest(FOOTPRINT):
        raise ValueError('measurement pin or footprint contract differs')
    measurement.roster(capture_report, plan, MODES, 'runtime')
    inputs = capture_report['requests']
    ids = verify_requests(directory, inputs, capture_report['smoke'])
    identity = None
    for run in capture_report['runs']:
        identity = verify_sample(directory, run, inputs, ids, identity)
    return build



# One-minute load average above which the host was doing other work while a sample
# ran: a serial child on a quiet host contributes about 1 itself. Disclosed, never a
# reason to drop or repeat samples (the method's stability rule).
HOST_BUSY_LOAD = 2.0


def host_load(runs):
    """The one-minute load average recorded after each sample, and whether any exceeded
    HOST_BUSY_LOAD."""
    values = [r["load_average"][0] for r in runs if isinstance(r.get("load_average"), list) and r["load_average"]]
    return {"one_minute": values, "max": max(values) if values else None, "threshold": HOST_BUSY_LOAD,
            "busy": any(v > HOST_BUSY_LOAD for v in values)}


def census_diagnosis(rows):
    """Why a census sample is unavailable, by family name and variant count, and the Go
    allocation observer's log use over the sample (headroom before it overflows)."""
    reasons = {}
    observer = {"variants": 0, "overflow_variants": 0, "max_recorded_over_capacity": None}
    for row in rows:
        census = row["checkpoint"]["census"]
        if census.get("state") == "failed":
            continue
        for name in census["unavailable"]:
            reasons[name] = reasons.get(name, 0) + 1
        status = census.get("observer")
        if status and status.get("present"):
            observer["variants"] += 1
            observer["overflow_variants"] += bool(status.get("overflow"))
            use = status["recorded"] / status["capacity"] if status.get("capacity") else None
            if use is not None and (observer["max_recorded_over_capacity"] is None or use > observer["max_recorded_over_capacity"]):
                observer["max_recorded_over_capacity"] = use
    return {"unavailable_reasons": dict(sorted(reasons.items())), "observer": observer}


def stability(values):
    return {"samples": values, "median": median(values), "max_over_min": max(values) / min(values),
            "coefficient_of_variation": pstdev(values) / mean(values) if len(values) > 1 else 0.0,
            "unstable": max(values) / min(values) > 1.10}


def report(directory):
    capture_report = strict_json_loads((directory / "capture.json").read_bytes())
    verify_capture(directory, capture_report)
    measured = [r for r in capture_report["runs"] if not r["warmup"]]
    result = {"version": 1, "pin": capture_report["pin"], "smoke": capture_report["smoke"], "variants": capture_report["requests"]["variants"],
              "host": capture_report["host"], "sources_sha256": capture_report["sources_sha256"],
              "source_stable": fingerprint(sources()) == capture_report["sources_sha256"],
              "capture_sha256": digest((directory / "capture.json").read_bytes()), "modes": {}, "metrics": {}, "unavailable": {}}
    # Work identity: every runtime repeats its own outputs and action schedule exactly; the two
    # runtimes execute the same action counts per variant. Output digests are compared per variant.
    identity = {}
    for runtime in RUNTIMES:
        runs = [r for r in measured if r["runtime"] == runtime]
        identity[runtime] = {"outputs_sha256": sorted({r["totals"]["outputs_sha256"] for r in runs}),
                             "actions_sha256": sorted({r["totals"]["actions_sha256"] for r in runs})}
        if len(identity[runtime]["outputs_sha256"]) != 1 or len(identity[runtime]["actions_sha256"]) != 1:
            raise ValueError(f"{runtime} samples did not repeat identical fixed work")
    go_rows = {row["id"]: row for row in rows_of(directory, next(r for r in measured if r["runtime"] == "go" and r["mode"] == "normal"))}
    rust_rows = {row["id"]: row for row in rows_of(directory, next(r for r in measured if r["runtime"] == "rust" and r["mode"] == "normal"))}
    if list(go_rows) != list(rust_rows):
        raise ValueError("runtimes observed different variant inventories")
    action_mismatches = [i for i in go_rows if go_rows[i]["actions"] != rust_rows[i]["actions"]]
    digest_mismatches = [i for i in go_rows if go_rows[i]["output_sha256"] != rust_rows[i]["output_sha256"]]
    result["work"] = {"identity": identity, "action_mismatches": action_mismatches, "digest_mismatches": digest_mismatches,
                      "digest_agreement": len(go_rows) - len(digest_mismatches)}
    valid = not action_mismatches and not digest_mismatches
    if not valid:
        result["unavailable"]["actions"] = f"{len(action_mismatches)} variants executed different action counts; samples invalid"
    for mode in MODES:
        runs = {runtime: [r for r in measured if r["runtime"] == runtime and r["mode"] == mode] for runtime in RUNTIMES}
        summary = {"interval_ns": {runtime: stability([r["totals"]["interval_ns"] for r in runs[runtime]]) for runtime in RUNTIMES},
                   "process_ns": {runtime: [r["process"]["process_ns"] for r in runs[runtime]] for runtime in RUNTIMES},
                   "peak_rss_bytes": {runtime: [r["process"]["peak_rss_bytes"] for r in runs[runtime]] for runtime in RUNTIMES},
                   "host_load": host_load([r for runtime in RUNTIMES for r in runs[runtime]])}
        result["host_busy"] = result.get("host_busy", False) or summary["host_load"]["busy"]
        go_ns = [r["totals"]["interval_ns"] for r in runs["go"]]
        rust_ns = [r["totals"]["interval_ns"] for r in runs["rust"]]
        try:
            summary["elapsed"] = ratio_summary(go_ns, rust_ns, timing=True, threshold=1.0)
            summary["throughput_ratio"] = summary["elapsed"]["go_median"] / summary["elapsed"]["rust_median"]
        except ValueError as error:
            summary["elapsed"] = {"unavailable": str(error)}
        if mode == "phase":
            phases = {}
            for runtime in RUNTIMES:
                totals = [r["totals"]["phases_ns"] for r in runs[runtime]]
                phases[runtime] = {name: median([t[name] for t in totals]) for name in ("init", "check", "display")}
                interval = median([r["totals"]["interval_ns"] for r in runs[runtime]])
                phases[runtime]["sum_over_interval"] = sum(phases[runtime].values()) / interval if interval else None
            summary["phases_ns"] = phases
        if mode == "alloc":
            allocation = {}
            for runtime in RUNTIMES:
                allocation[runtime] = {key: [r["totals"]["allocation"][key] for r in runs[runtime]] for key in ("requested_bytes", "retained_bytes")}
                allocation[runtime]["allocation_calls"] = [r["totals"]["allocation"].get("allocation_calls") for r in runs[runtime]]
            summary["allocation"] = allocation
            for key, metric in (("requested_bytes", "allocated_bytes_ratio"), ("retained_bytes", "retained_bytes_ratio")):
                go_values, rust_values = allocation["go"][key], allocation["rust"][key]
                if min(go_values) <= 0:
                    result["unavailable"][metric] = f"non-positive Go {key} denominator"
                    continue
                if key == "requested_bytes" and min(rust_values) <= 0:
                    # A zero request total means the counting allocator was not active; it
                    # is never a valid ratio of zero.
                    result["unavailable"][metric] = "Rust requested bytes are zero: allocation instrumentation inactive"
                    continue
                if key == "retained_bytes" and min(rust_values) <= 0:
                    # Retained deltas are signed (live before minus live at the checkpoint
                    # can go either way); the raw values and the ratio of medians are
                    # reported without the positive-sample bootstrap.
                    summary[metric] = {"samples_per_runtime": len(go_values), "go_median": median(go_values),
                                       "rust_median": median(rust_values), "ratio": median(rust_values) / median(go_values),
                                       "signed": True}
                    continue
                try:
                    summary[metric] = ratio_summary(go_values, rust_values)
                    summary[metric]["ratio"] = median(rust_values) / median(go_values)
                except ValueError as error:
                    result["unavailable"][metric] = str(error)
            # The census is a structural sum per sample; hash-table capacities can differ
            # between processes, so the statistic is the median over samples with its spread.
            census = {}
            for runtime in RUNTIMES:
                totals = [r["totals"]["census"] for r in runs[runtime]]
                means = [t["type_storage_bytes"] / t["types_reachable"] for t in totals if t["types_reachable"]]
                census[runtime] = {key: [t[key] for t in totals] for key in ("type_storage_bytes", "checker_bytes", "types_reachable", "types_created", "unavailable")}
                census[runtime]["failed"] = sum(t.get("failed", 0) for t in totals)
                census[runtime]["mean_bytes_per_reachable_type"] = median(means) if len(means) == len(totals) else None
                census[runtime]["mean_bytes_max_over_min"] = (max(means) / min(means)) if means and min(means) > 0 else None
                census[runtime].update(census_diagnosis(rows_of(directory, runs[runtime][0])))
            summary["census"] = census
            if census["go"]["failed"] or census["rust"]["failed"]:
                result["unavailable"]["type_footprint_ratio"] = "a census failed on at least one variant"
            elif any(any(census[r]["unavailable"]) for r in RUNTIMES):
                reasons = {r: census[r]["unavailable_reasons"] for r in RUNTIMES if census[r]["unavailable_reasons"]}
                result["unavailable"]["type_footprint_ratio"] = f"required census families or semantic roots are unavailable: {reasons}"
            elif any(not census[r]["mean_bytes_per_reachable_type"] for r in RUNTIMES):
                result["unavailable"]["type_footprint_ratio"] = "census bytes per reachable type are not positive"
            else:
                summary["type_footprint_ratio"] = census["rust"]["mean_bytes_per_reachable_type"] / census["go"]["mean_bytes_per_reachable_type"]
        result["modes"][mode] = summary
    normal = result["modes"]["normal"]
    if valid and "throughput_ratio" in normal:
        result["metrics"]["throughput_ratio"] = normal["throughput_ratio"]
        result["metrics"]["elapsed_ratio"] = normal["elapsed"]["ratio"]
        result["metrics"]["stable"] = not any(normal["interval_ns"][r]["unstable"] for r in RUNTIMES)
        for runtime in RUNTIMES:
            result["metrics"][f"{runtime}_interval_ns_median"] = normal["interval_ns"][runtime]["median"]
            result["metrics"][f"{runtime}_max_over_min"] = normal["interval_ns"][runtime]["max_over_min"]
    alloc = result["modes"]["alloc"]
    for metric in ("allocated_bytes_ratio", "retained_bytes_ratio"):
        if valid and metric in alloc:
            result["metrics"][metric] = alloc[metric]["ratio"]
    if valid and "type_footprint_ratio" in alloc:
        result["metrics"]["type_footprint_ratio"] = alloc["type_footprint_ratio"]
    result["metrics"]["variants"] = result["variants"]
    result["metrics"]["digest_agreement"] = result["work"]["digest_agreement"]
    result["metrics"]["action_mismatches"] = len(action_mismatches)
    (directory / "report.json").write_bytes(canonical(result) + b"\n")
    return result


def current_report(directory=DEFAULT):
    path = directory / "capture.json"
    if not path.exists():
        return None
    try:
        result = report(directory)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"checkerbench capture invalid/unavailable: {error}", file=sys.stderr)
        return None
    if result.get("smoke"):
        print("checkerbench report is a smoke capture; no acceptance metrics", file=sys.stderr)
        return None
    return result


def footprint_metric():
    """The E5 per-type footprint statistic, when a current full capture measured it."""
    result = current_report()
    if result is None or "type_footprint_ratio" not in result["metrics"]:
        return None
    return result["metrics"]["type_footprint_ratio"]


def producer():
    result = current_report()
    if result is None:
        print("run.checkerbench metrics unavailable: no current full capture at target/s08/checkerbench (see docs/S08-P7.md)", file=sys.stderr)
        return {"metrics": {}}
    print(json.dumps({"report": result}, sort_keys=True), file=sys.stderr)
    metrics = {k: v for k, v in result["metrics"].items() if k != "type_footprint_ratio"}
    for name, reason in result["unavailable"].items():
        print(f"run.checkerbench.{name} unavailable: {reason}", file=sys.stderr)
    return {"metrics": metrics}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "capture", "report", "producer"))
    parser.add_argument("--output", type=Path, default=DEFAULT)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--smoke", type=int, help="bounded variant count; never produces acceptance metrics")
    args = parser.parse_args()
    directory = args.output.resolve()
    if args.command == "build":
        print(json.dumps({k: v for k, v in build(directory).items() if k != "sources"}, sort_keys=True))
    elif args.command == "capture":
        capture(directory, args.samples, args.smoke)
        print(json.dumps(report(directory)["metrics"], sort_keys=True))
    elif args.command == "report":
        print(json.dumps(report(directory)["metrics"], sort_keys=True))
    else:
        print(canonical(producer()).decode())


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
