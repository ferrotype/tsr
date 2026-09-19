#!/usr/bin/env python3
"""S08 relater comparison (data/s08/relater-fixtures.json): the production ID relater and the
isolated reference prototype (tools/s08/relater-prototype) over the 21 frozen fixtures, five
modes each, compared against the pinned Go observations and then measured in alternating
fresh-process samples.

  build     compile the normal and allocation relater executables
  parity    run both implementations once and compare every group with the Go observations
  capture   parity, then warmups and seven alternating samples per implementation and executable
  report    aggregate a capture into the run.relater.* metrics
  producer  emit the metrics for the ledger when a current report exists
"""
import argparse
import json
import lzma
import os
import shutil
import subprocess
import sys
import tarfile
import time
from pathlib import Path
from statistics import median, mean, pstdev

from s04 import same_json_value
from s04_common import strict_json_loads, command
from s07_benchmark import cargo_executable, native_environment
from s07_benchmark_measure import reject_concurrent_builds
from s07_benchmark_stats import ratio_summary
from s08_oracle import ROOT, canonical, digest
import s08_measurement as measurement

DEFAULT = ROOT / "target/s08/relater"
METHOD = ROOT / "data/s08/relater-fixtures.json"
OBSERVATIONS = ROOT / "data/s08/supplemental-observations.json.xz"
ARCHIVE = ROOT / "tools/s08/results/p0-contracts/native.tar.xz"
ARCHIVE_MEMBER = "p0-final-verification/supplemental/requests.json"
MODES = {"normal": ["relation-probe"], "alloc": ["relation-probe", "s08-allocation"]}
IMPLEMENTATIONS = ("id", "reference")
SAMPLE_TIMEOUT = 3600
RELATION_MODES = ("identity", "assignable", "subtype", "strict_subtype", "comparable")


def method():
    return strict_json_loads(METHOD.read_bytes())


def sources():
    result = {}
    for pattern in ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.*", "rust-toolchain*", ".cargo/**/*",
                    "tools/s08/relater-prototype/**/*", "crates/tsr_compiler/examples/p7_relater.rs", "crates/tsr_compiler/examples/p3/mod.rs",
                    "scripts/s08_relater.py", "scripts/s08_measurement.py", "scripts/s07_benchmark.py", "scripts/s07_benchmark_stats.py", "scripts/s07_benchmark_measure.py",
                    "scripts/s04.py", "scripts/s04_common.py", "scripts/s08_oracle.py",
                    "data/s08/relater-fixtures.json", "data/s08/supplemental-observations.json.xz", "tools/s08/contracts/relations.json",
                    "tools/s08/results/p0-contracts/native.tar.xz", "data/upstream.json"):
        for path in ROOT.glob(pattern):
            if path.is_file() and "__pycache__" not in path.parts:
                result[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return result


def fingerprint(files):
    return digest(canonical(files))


def frozen():
    """The frozen request inventory and its Go observations, checked against each other."""
    observations = strict_json_loads(lzma.decompress(OBSERVATIONS.read_bytes()))
    with tarfile.open(ARCHIVE) as archive:
        requests_raw = archive.extractfile(ARCHIVE_MEMBER).read()
    if digest(requests_raw) != observations["request_sha256"]:
        raise ValueError("frozen relater requests do not match the Go observations")
    requests = strict_json_loads(requests_raw)
    contract = strict_json_loads((ROOT / "tools/s08/contracts/relations.json").read_bytes())
    if [r["id"] for r in requests] != [c["id"] for c in contract["cases"]] or [r["id"] for r in observations["rows"]] != [r["id"] for r in requests]:
        raise ValueError("relater fixture inventory drift")
    plan = method()
    if plan["pin"] != strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]:
        raise ValueError("relater pin drift")
    return requests_raw, requests, observations, contract


def build(directory):
    reject_concurrent_builds()
    initial_sources = sources()
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "bin").mkdir(exist_ok=True)
    env = native_environment()
    manifest = ROOT / "crates/tsr_compiler/Cargo.toml"
    binaries = {}
    for mode, features in MODES.items():
        args = ["cargo", "build", "--release", "--locked", "--example", "p7_relater", "--features", ",".join(features),
                "--message-format=json", "--manifest-path", str(manifest)]
        messages = command(args, cwd=ROOT, env=env).decode()
        executable = cargo_executable(messages, manifest, "p7_relater", "example", features)
        target = directory / "bin" / f"relater-{mode}"
        shutil.copy2(executable, target)
        binaries[mode] = {"path": str(target), "sha256": digest(target.read_bytes()), "features": features, "command": args}
    if sources() != initial_sources:
        raise ValueError("measurement sources changed during build")
    report = {"version": 1, "binaries": binaries, "sources": initial_sources, "rust_toolchain": command(["rustc", "--version"], cwd=ROOT, env=env).decode().strip()}
    report["sources_sha256"] = fingerprint(report["sources"])
    (directory / "build.json").write_bytes(canonical(report) + b"\n")
    return report


def run_child(binary, requests, output, implementation, timeout=SAMPLE_TIMEOUT):
    env = native_environment()
    with open(str(output) + ".stdout", "wb") as stdout, open(str(output) + ".stderr", "wb") as stderr:
        started = time.monotonic_ns()
        child = subprocess.Popen([str(binary), str(requests), str(output), "--implementation", implementation], stdout=stdout, stderr=stderr, env=env, cwd=ROOT)
        deadline = time.monotonic() + timeout
        try:
            while True:
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    child.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.monotonic() >= deadline:
                    raise ValueError("relater child exceeded the fixed process deadline")
                time.sleep(0.02)
        finally:
            if child.returncode is None:
                child.kill()
                _, status, _ = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
        process_ns = time.monotonic_ns() - started
    if child.returncode != 0:
        raise ValueError(f"relater child ({implementation}) exited {child.returncode}: see {output}.stderr")
    observed = strict_json_loads(Path(output).read_bytes())
    if observed["implementation"] != implementation:
        raise ValueError("relater child reported the wrong implementation")
    return observed, {"returncode": 0, "process_ns": process_ns, "peak_rss_bytes": usage.ru_maxrss * (1024 if sys.platform == "linux" else 1),
                      "user_time_ns": round(usage.ru_utime * 1e9), "system_time_ns": round(usage.ru_stime * 1e9)}


def cache_view(state):
    return {mode: state["caches"][mode] for mode in RELATION_MODES}


def cache_delta(before, after):
    """Entries and result flags an action added to each relation cache (multiset difference)."""
    delta = {}
    for mode in RELATION_MODES:
        added = list(after["caches"][mode]["result_flags"])
        for flag in before["caches"][mode]["result_flags"]:
            if flag in added:
                added.remove(flag)
        delta[mode] = {"entries": after["caches"][mode]["entries"] - before["caches"][mode]["entries"], "result_flags": sorted(added)}
    return delta


def compare_group(implementation, go_group, actions, found, *, behavior_only=False):
    """Exact protocol parity; a separately named projection reports partial behavior.

    Bound-program construction is necessary but not sufficient: native state,
    lazy creation counts, cache transitions and diagnostic payloads must agree.
    """
    differences = []
    if implementation == "reference" and not behavior_only and found.get("source_mode") != "bound_program":
        differences.append("reference setup pre-resolves the production graph; lazy protocol unavailable")
    if found["state"] != "executed":
        return [f"group not executed: {found.get('reason')}"]
    if [a["action"] for a in go_group["actions"]] != actions or len(found["actions"]) != len(actions):
        return ["relation action drift"]
    if not behavior_only and not same_json_value(found["before_lookup"], go_group["before_lookup"]):
        differences.append("before_lookup")
    if not behavior_only and not same_json_value(found["after_lookup"], go_group["actions"][0]["before"]):
        differences.append("after_lookup")
    for index, (wanted, observed) in enumerate(zip(go_group["actions"], found["actions"], strict=True)):
        if not same_json_value(observed["action"], wanted["action"]):
            differences.append(f"action {index}: reordered")
            continue
        for key in ("result", "ternary_calls"):
            if not same_json_value(observed[key], wanted[key]):
                differences.append(f"action {index}: {key} {observed[key]!r} != {wanted[key]!r}")
        if not behavior_only:
            for key in ("before", "after", "diagnostics"):
                if not same_json_value(observed[key], wanted[key]):
                    differences.append(f"action {index}: {key}")
        else:
            observed_delta = cache_delta(observed["before"], observed["after"])
            wanted_delta = cache_delta(wanted["before"], wanted["after"])
            if not same_json_value(observed_delta, wanted_delta):
                differences.append(f"action {index}: cache delta {observed_delta!r} != {wanted_delta!r}")
            if len(observed["diagnostics"]) != len(wanted["diagnostics"]):
                differences.append(f"action {index}: diagnostic count")
    return differences


def transitions(go_group, found):
    """Actual semantic creation deltas, without surrogate allocation counters."""
    rows = []
    for wanted, observed in zip(go_group["actions"], found.get("actions", []), strict=False):
        counters = ("types_created", "signatures_created", "instantiations")
        go_created = {k: wanted["after"][k] - wanted["before"][k] for k in counters}
        ref_created = {k: observed["after"][k] - observed["before"][k] for k in counters}
        rows.append({"go_created": go_created, "reference_created": ref_created, "agree": same_json_value(go_created, ref_created)})
    return rows


def compare(observations, requests, contract, observed, implementation):
    result = {"implementation": implementation, "groups": 0, "matched": 0, "behavior_matched": 0, "unsupported": 0, "differences": {}, "cases": {}}
    by_id = {r["id"]: r for r in observations["rows"]}
    starting = {c["id"]: c["starting_cache_entries"] for c in contract["cases"]}
    for request, row in zip(requests, observed["rows"], strict=True):
        if row["id"] != request["id"]:
            raise ValueError("relater inventory drift in the observation")
        go_row = by_id[request["id"]]
        groups = []
        start = 0
        actions = request["actions"]
        while start < len(actions):
            end = start + 1
            while end < len(actions) and actions[end]["mode"] == actions[start]["mode"]:
                end += 1
            groups.append(actions[start:end])
            start = end
        if len(groups) != len(go_row["groups"]) or len(groups) != len(row["groups"]):
            raise ValueError("missing relation mode group")
        case = {"matched": 0, "groups": len(groups), "unsupported": [], "differences": {}, "transitions": {}}
        for group_actions, go_group, found in zip(groups, go_row["groups"], row["groups"], strict=True):
            mode = group_actions[0]["mode"]
            result["groups"] += 1
            if found["state"] != "executed":
                result["unsupported"] += 1
                case["unsupported"].append({"mode": mode, "reason": found.get("reason")})
                continue
            declared = starting[request["id"]][mode]
            differences = compare_group(implementation, go_group, group_actions, found)
            if not compare_group(implementation, go_group, group_actions, found, behavior_only=True):
                result["behavior_matched"] += 1
            if go_group["actions"][0]["before"]["caches"][mode]["entries"] != declared:
                differences.append(f"declared starting cache entries {declared} differ from the observation")
            if differences:
                case["differences"][mode] = differences
            else:
                case["matched"] += 1
                result["matched"] += 1
            if implementation == "reference":
                case["transitions"][mode] = transitions(go_group, found)
        result["cases"][request["id"]] = case
        if case["differences"]:
            result["differences"][request["id"]] = case["differences"]
    result["parity"] = result["matched"] / result["groups"] if result["groups"] else 0.0
    result["behavior_agreement"] = result["behavior_matched"] / result["groups"] if result["groups"] else 0.0
    result["all_cases_match"] = result["matched"] == result["groups"]
    return result


def parity(directory, build_report=None):
    directory.mkdir(parents=True, exist_ok=True)
    build_report = build_report or strict_json_loads((directory / "build.json").read_bytes())
    requests_raw, requests, observations, contract = frozen()
    (directory / "requests.json").write_bytes(requests_raw)
    comparison = {"version": 1, "request_sha256": digest(requests_raw), "implementations": {}, "artifacts": {}}
    for implementation in IMPLEMENTATIONS:
        observed, _ = run_child(build_report["binaries"]["normal"]["path"], directory / "requests.json", directory / f"parity-{implementation}.json", implementation)
        if observed["request_sha256"] != comparison["request_sha256"]:
            raise ValueError("relater child observed a different request inventory")
        for suffix in (".json", ".json.stdout", ".json.stderr"):
            name = f"parity-{implementation}{suffix}"
            comparison["artifacts"][name] = measurement.file_digest(directory / name)
        comparison["implementations"][implementation] = compare(observations, requests, contract, observed, implementation)
    both = all(comparison["implementations"][i]["all_cases_match"] for i in IMPLEMENTATIONS)
    comparison["parity"] = 1.0 if both else min(comparison["implementations"][i]["parity"] for i in IMPLEMENTATIONS)
    comparison["both_match_every_case"] = both
    (directory / "parity.json").write_bytes(canonical(comparison) + b"\n")
    return comparison


def capture(directory, samples_per_runtime=7, smoke=False):
    if (directory / "capture.json").exists():
        raise ValueError("capture already exists; select a new output directory")
    directory.mkdir(parents=True, exist_ok=True)
    build_report = build(directory)
    comparison = parity(directory, build_report)
    if not comparison["both_match_every_case"] and not smoke:
        raise ValueError("relater parity failed; inspect parity.json before collecting a measurement batch")
    plan = method()
    order = [[r.lower() for r in pair] for pair in plan["sampling"]["measured_order"]]
    order = [[{"id": "id", "reference": "reference"}[r] for r in pair] for pair in order][:samples_per_runtime]
    if samples_per_runtime != plan["sampling"]["measured_samples_per_runtime"] and not smoke:
        raise ValueError("full captures use the frozen sample count")
    runs = []
    samples = directory / "samples"
    samples.mkdir(exist_ok=True)
    for mode in MODES:
        for warmup in [r.lower() for r in plan["sampling"]["warmup_order"]]:
            observed, process = run_child(build_report["binaries"][mode]["path"], directory / "requests.json", samples / f"{mode}-{warmup}-warmup.json", warmup)
            runs.append({"mode": mode, "implementation": warmup, "warmup": True, "label": "warmup", "totals": observed["totals"], "process": process,
                         "load_average": os.getloadavg()})
        for index, pair in enumerate(order):
            for implementation in pair:
                observed, process = run_child(build_report["binaries"][mode]["path"], directory / "requests.json", samples / f"{mode}-{implementation}-{index}.json", implementation)
                runs.append({"mode": mode, "implementation": implementation, "warmup": False, "label": f"sample-{index}", "totals": observed["totals"], "process": process,
                             "load_average": os.getloadavg()})
    for run in runs:
        label = 'warmup' if run['warmup'] else run['label'].removeprefix('sample-')
        name = f"{run['mode']}-{run['implementation']}-{label}.json"
        run['artifacts'] = {name + suffix: measurement.file_digest(samples / (name + suffix))
                            for suffix in ('', '.stdout', '.stderr')}
    capture_report = {"version": 2, "pin": plan["pin"], "smoke": smoke, "samples_per_runtime": samples_per_runtime,
                      "build_sha256": digest((directory / "build.json").read_bytes()), "sources_sha256": build_report["sources_sha256"],
                      "parity_sha256": digest((directory / "parity.json").read_bytes()), "method_sha256": digest(METHOD.read_bytes()),
                      "parity": comparison["parity"], "runs": runs, "finished": time.time()}
    (directory / "capture.json").write_bytes(canonical(capture_report) + b"\n")
    return capture_report


def stability(values):
    return {"samples": values, "median": median(values), "max_over_min": (max(values) / min(values)) if min(values) else None,
            "coefficient_of_variation": (pstdev(values) / mean(values)) if len(values) > 1 and mean(values) else 0.0,
            "unstable": bool(min(values)) and max(values) / min(values) > 1.10}


def observed_totals(observed, requests):
    totals = {k: 0 for k in ("setup_ns", "relation_ns", "setup_requested_bytes", "relation_requested_bytes",
                            "retained_bytes", "groups_executed", "groups_unsupported")}
    if [r['id'] for r in observed['rows']] != [r['id'] for r in requests]:
        raise ValueError('relater sample inventory differs')
    for request, row in zip(requests, observed['rows'], strict=True):
        modes = list(dict.fromkeys(a['mode'] for a in request['actions']))
        if row['state'] != 'executed' or [g['mode'] for g in row['groups']] != modes:
            raise ValueError('relater sample mode inventory differs')
        for group in row['groups']:
            state = group['state']
            if state not in ('executed', 'unsupported'):
                raise ValueError('relater sample failed')
            totals['groups_' + state] += 1
            for key in ('setup_ns', 'relation_ns'):
                totals[key] += measurement.number(group[key], key)
            allocation = group['allocation']
            for key in ('setup_requested_bytes', 'relation_requested_bytes'):
                totals[key] += measurement.number(allocation[key], key)
            before = measurement.number(allocation['live_before'], 'live_before')
            after = measurement.number(allocation['live_after'], 'live_after')
            measurement.number(allocation['live_after_release'], 'live_after_release')
            totals['retained_bytes'] += after - before
    measurement.check_totals(observed['totals'], totals)
    return totals


def semantic_rows(observed):
    return [{"id": row['id'], "state": row['state'], "groups": [
        {**{key: group[key] for key in ('mode', 'state', 'reason', 'before_lookup', 'after_lookup', 'actions')},
         "source_mode": group.get("source_mode")}
        for group in row['groups']]} for row in observed['rows']]


def verify_capture(directory, capture_report):
    plan = method()
    build = measurement.build_record(directory, capture_report, sources(), METHOD)
    if set(build['binaries']) != set(MODES) or capture_report['pin'] != plan['pin']:
        raise ValueError('relater executable inventory or pin differs')
    if type(capture_report['smoke']) is not bool:
        raise ValueError('invalid relater smoke marker')
    measurement.roster(capture_report, plan, MODES, 'implementation')
    raw, requests, observations, contract = frozen()
    measurement.authenticated(directory / 'requests.json', digest(raw))
    comparison = strict_json_loads(measurement.authenticated(directory / 'parity.json', capture_report['parity_sha256']))
    if comparison['request_sha256'] != digest(raw):
        raise ValueError('relater parity requests differ')
    names = {f'parity-{impl}.json{suffix}' for impl in IMPLEMENTATIONS for suffix in ('', '.stdout', '.stderr')}
    if set(comparison['artifacts']) != names:
        raise ValueError('missing relater parity artifacts')
    for name, sha in comparison['artifacts'].items():
        measurement.authenticated(directory / name, sha)
    identities = {}
    for implementation in IMPLEMENTATIONS:
        observed = strict_json_loads((directory / f'parity-{implementation}.json').read_bytes())
        if (observed['implementation'], observed['mode'], observed['request_sha256']) != (implementation, 'normal', digest(raw)):
            raise ValueError('relater parity child protocol differs')
        observed_totals(observed, requests)
        recomputed = compare(observations, requests, contract, observed, implementation)
        if not same_json_value(comparison['implementations'][implementation], recomputed):
            raise ValueError('relater parity summary differs from raw observations')
        identities[implementation] = semantic_rows(observed)
    both = all(comparison['implementations'][i]['all_cases_match'] for i in IMPLEMENTATIONS)
    parity_value = min(comparison['implementations'][i]['parity'] for i in IMPLEMENTATIONS)
    if comparison['both_match_every_case'] != both or comparison['parity'] != parity_value or capture_report['parity'] != parity_value:
        raise ValueError('relater parity total differs')
    for run in capture_report['runs']:
        label = 'warmup' if run['warmup'] else run['label'].removeprefix('sample-')
        name = f"{run['mode']}-{run['implementation']}-{label}.json"
        names = {name + suffix for suffix in ('', '.stdout', '.stderr')}
        if set(run['artifacts']) != names or run['process']['returncode'] != 0:
            raise ValueError('missing relater sample artifacts or failed process')
        for path, sha in run['artifacts'].items():
            measurement.authenticated(directory / 'samples' / path, sha)
        observed = strict_json_loads((directory / 'samples' / name).read_bytes())
        if (observed['implementation'], observed['mode'], observed['request_sha256']) != (run['implementation'], run['mode'], digest(raw)):
            raise ValueError('relater sample child protocol differs')
        observed_totals(observed, requests)
        if not same_json_value(run['totals'], observed['totals']):
            raise ValueError('relater sample totals differ from raw observations')
        if not same_json_value(identities[run['implementation']], semantic_rows(observed)):
            raise ValueError('relater semantics changed between parity and measurement')
    return comparison


def report(directory):
    capture_report = strict_json_loads((directory / "capture.json").read_bytes())
    comparison = verify_capture(directory, capture_report)
    measured = [r for r in capture_report["runs"] if not r["warmup"]]
    result = {"version": 1, "pin": capture_report["pin"], "smoke": capture_report["smoke"], "sources_sha256": capture_report["sources_sha256"],
              "source_stable": fingerprint(sources()) == capture_report["sources_sha256"],
              "capture_sha256": digest((directory / "capture.json").read_bytes()),
              "parity": comparison, "modes": {}, "metrics": {}, "unavailable": {}}
    executed = {}
    for implementation in IMPLEMENTATIONS:
        totals = [r["totals"] for r in measured if r["implementation"] == implementation]
        executed[implementation] = sorted({(t["groups_executed"], t["groups_unsupported"]) for t in totals})
        if len(executed[implementation]) != 1:
            raise ValueError(f"{implementation} samples executed different group counts")
    # Timing compares the same fixed work: only groups both implementations execute count,
    # so the child totals are usable only when the executed inventories agree.
    same_inventory = executed["id"] == executed["reference"]
    # Equal inventories alone do not prove comparable endpoints. Strict parity
    # includes bound construction, initial state, every semantic creation count,
    # ordered diagnostics and the first/repeat cache transitions.
    same_work = same_inventory and comparison["both_match_every_case"]
    if not same_work:
        result["unavailable"]["comparison_protocol"] = "reference and ID do not both match the bound-program construction, lazy-work and diagnostic protocol"
    if not same_inventory:
        result["unavailable"]["fixed_work"] = f"executed groups differ: id {executed['id']} reference {executed['reference']}; the reference covers fewer fixtures"
    for mode in MODES:
        runs = {i: [r for r in measured if r["implementation"] == i and r["mode"] == mode] for i in IMPLEMENTATIONS}
        summary = {}
        for key in ("setup_ns", "relation_ns"):
            summary[key] = {i: stability([r["totals"][key] for r in runs[i]]) for i in IMPLEMENTATIONS}
            try:
                summary[f"{key}_ratio"] = ratio_summary([r["totals"][key] for r in runs["id"]], [r["totals"][key] for r in runs["reference"]], timing=True, threshold=1.0)
            except ValueError as error:
                summary[f"{key}_ratio"] = {"unavailable": str(error)}
        if mode == "alloc":
            for key in ("setup_requested_bytes", "relation_requested_bytes", "retained_bytes"):
                summary[key] = {i: [r["totals"][key] for r in runs[i]] for i in IMPLEMENTATIONS}
                id_values, reference_values = summary[key]["id"], summary[key]["reference"]
                if min(id_values) <= 0 or min(reference_values) <= 0:
                    summary[f"{key}_ratio"] = {"unavailable": "non-positive ID or reference bytes; raw signed endpoints remain recorded"}
                    continue
                try:
                    summary[f"{key}_ratio"] = ratio_summary(id_values, reference_values)
                except ValueError as error:
                    summary[f"{key}_ratio"] = {"unavailable": str(error)}
        for key in list(summary):
            if key.endswith("_ratio") and not same_work:
                summary[key] = {"unavailable": result["unavailable"]["comparison_protocol"]}
        result["modes"][mode] = summary
    result["metrics"]["parity"] = comparison["parity"]
    result["metrics"]["parity_id"] = comparison["implementations"]["id"]["parity"]
    result["metrics"]["reference_behavior_agreement"] = comparison["implementations"]["reference"]["behavior_agreement"]
    result["metrics"]["parity_reference"] = comparison["implementations"]["reference"]["parity"]
    result["metrics"]["reference_unsupported_groups"] = comparison["implementations"]["reference"]["unsupported"]
    normal = result["modes"]["normal"]
    if same_work and "ratio" in normal["relation_ns_ratio"]:
        # reference / ID elapsed; throughput is the inverse (>1 means the reference is faster).
        result["metrics"]["elapsed_ratio"] = normal["relation_ns_ratio"]["ratio"]
        result["metrics"]["throughput_ratio"] = 1 / normal["relation_ns_ratio"]["ratio"]
        result["metrics"]["setup_elapsed_ratio"] = normal["setup_ns_ratio"].get("ratio")
        result["metrics"]["stable"] = not any(normal["relation_ns"][i]["unstable"] for i in IMPLEMENTATIONS)
    elif same_work:
        result["unavailable"]["throughput_ratio"] = normal["relation_ns_ratio"].get("unavailable", "no timing ratio")
    alloc = result["modes"]["alloc"]
    for key, metric in (("relation_requested_bytes", "allocated_bytes_ratio"), ("retained_bytes", "retained_bytes_ratio")):
        if same_work and "ratio" in alloc.get(f"{key}_ratio", {}):
            result["metrics"][metric] = alloc[f"{key}_ratio"]["ratio"]
        elif same_work:
            result["unavailable"][metric] = alloc.get(f"{key}_ratio", {}).get("unavailable", "no ratio")
    (directory / "report.json").write_bytes(canonical(result) + b"\n")
    return result


def current_report(directory=DEFAULT):
    if not (directory / "capture.json").exists():
        return None
    try:
        result = report(directory)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"relater capture invalid/unavailable: {error}", file=sys.stderr)
        return None
    if result.get("smoke"):
        print("relater report is a smoke capture; no acceptance metrics", file=sys.stderr)
        return None
    return result


def producer():
    result = current_report()
    if result is None:
        print("run.relater metrics unavailable: no current full capture at target/s08/relater (see docs/S08-P7.md)", file=sys.stderr)
        return {"metrics": {}}
    print(json.dumps({"report": {k: v for k, v in result.items() if k != "parity"}}, sort_keys=True), file=sys.stderr)
    for name, reason in result["unavailable"].items():
        print(f"run.relater.{name} unavailable: {reason}", file=sys.stderr)
    return {"metrics": result["metrics"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "parity", "capture", "report", "producer"))
    parser.add_argument("--output", type=Path, default=DEFAULT)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--smoke", action="store_true", help="bounded sample count; never produces acceptance metrics")
    args = parser.parse_args()
    directory = args.output.resolve()
    if args.command == "build":
        print(json.dumps({k: v for k, v in build(directory).items() if k != "sources"}, sort_keys=True))
    elif args.command == "parity":
        result = parity(directory)
        print(json.dumps({i: {k: v for k, v in r.items() if k != "cases"} for i, r in result["implementations"].items()}, sort_keys=True, indent=1))
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
