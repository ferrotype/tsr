#!/usr/bin/env python3
"""Freeze, capture and replay the bounded synthetic list-distribution pilot."""
import argparse
from collections import Counter
from contextlib import contextmanager
import fcntl
import gzip
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
from s07_benchmark import cargo_configuration_paths, cargo_executable, native_environment, release_configuration, rust_native_toolchain
from s07_benchmark_measure import host_info, reject_concurrent_builds

SPEC = importlib.util.spec_from_file_location("list_owner_validation", HERE.parent / "owner-census/validate.py")
owner_validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(owner_validation)

POLICIES = ("legacy", "page64", "page256", "page1024")
REPETITIONS = 7
MASK = 2**64 - 1
SCOPE = "physical backing-length/order replay with synthetic edge values; excludes parser, AST nodes, list headers, imports and binding"
BUILD_KIND = "s07_bis_list_distribution_build"
CAPTURE_KIND = "s07_bis_list_distribution_capture"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=0, maximum=MASK):
    require(type(value) is int and minimum <= value <= maximum, "invalid bounded integer")
    return value


def tool_files():
    files = [Path(__file__), HERE / "test_run_lists.py", HERE.parent / "runner.py", HERE.parent / "owner-census/validate.py"]
    files += list((ROOT / "scripts").glob("s07_benchmark*.py"))
    files += [ROOT / "scripts" / name for name in ("s04.py", "s04_common.py", "s04_runtime.py", "s04_ownership.py", "s06_process.py", "s06_protocol.py", "s07_binder.py", "s07_inventory.py")]
    return {str(path.relative_to(ROOT)): runner.digest(path) for path in sorted(set(files))}


def source_files():
    files = [*HERE.glob("*.rs"), HERE / "Cargo.toml", HERE / "Cargo.lock"]
    return {path.name: runner.digest(path) for path in sorted(files)}


def configuration(env, directory):
    return {str(path): runner.digest(path) if path.is_file() else None for path in cargo_configuration_paths(env, directory)}


def preflight(value):
    require(type(value) is dict and set(value) == {"version", "preflight", "expected_bytes", "requested_bytes", "live_before", "live_after"}, "invalid allocation preflight envelope")
    require(integer(value["version"]) == 1 and value["preflight"] is True, "invalid allocation preflight identity")
    for key in ("expected_bytes", "requested_bytes", "live_before", "live_after"):
        integer(value[key])
    require(value["expected_bytes"] == value["requested_bytes"] == 1_200_050, "allocation wrapper did not count original requests exactly")
    require(value["live_before"] == value["live_after"], "allocation preflight did not balance deallocation")
    return value


def sealed(directory, expected_sha, kind):
    require(type(expected_sha) is str and len(expected_sha) == 64, "an externally recorded manifest SHA is required")
    require(runner.digest(directory / "manifest.json") == expected_sha, "manifest identity changed")
    value = strict_json_loads((directory / "manifest.json").read_bytes())
    require(type(value) is dict and type(value.get("version")) is int and value["version"] == 1 and value.get("kind") == kind and value.get("diagnostic_only") is True, "wrong diagnostic manifest")
    require(not any(path.is_symlink() or path.stat().st_mode & 0o222 for path in (directory, *directory.rglob("*"))), "bundle is not immutable")
    require(runner.inventory(directory) == value["inventory"], "immutable inventory changed")
    return value


def validate_build(directory, expected_sha):
    value = sealed(directory, expected_sha, BUILD_KIND)
    require(set(value["artifacts"]) == {"normal", "allocation"}, "missing binary mode")
    for role, record in value["artifacts"].items():
        path = directory / record["path"]
        require(path.resolve().is_relative_to(directory) and runner.digest(path) == record["sha256"] and os.access(path, os.X_OK), "binary changed or escaped its bundle")
        require(record["features"] == (["allocation"] if role == "allocation" else []), "binary feature identity changed")
    require(value["artifacts"]["normal"]["sha256"] != value["artifacts"]["allocation"]["sha256"], "binary modes are identical")
    observed = strict_json_loads(gzip.decompress((directory / "allocation-preflight.stdout.gz").read_bytes()))
    require(preflight(observed) == value["allocation_preflight"], "allocation preflight replay differs")
    return value


def compressed(path, raw):
    path.write_bytes(gzip.compress(raw, mtime=0))


def build(output):
    output.mkdir(parents=True, exist_ok=False)
    (output / "artifacts").mkdir()
    source = output / "source"
    source.mkdir()
    before, tools = source_files(), tool_files()
    for name, expected in before.items():
        runner.copy_file(HERE / name, source / name)
        require(runner.digest(source / name) == expected, "source changed while snapshotting")
    for name, expected in tools.items():
        runner.copy_file(ROOT / name, output / "tool-snapshot" / name)
        require(runner.digest(output / "tool-snapshot" / name) == expected, "tool changed while snapshotting")
    runner.copy_file(ROOT / "rust-toolchain.toml", output / "rust-toolchain.toml")
    env = native_environment()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target/s07-bis-list-pilot-native")
    stable, host = rust_native_toolchain(env)
    config = configuration(env, source)
    profile = release_configuration(env, source)
    artifacts, commands = {}, {}
    for role in ("normal", "allocation"):
        features = ["allocation"] if role == "allocation" else []
        argv = ["cargo", "+" + stable, "build", "--offline", "--locked", "--release", "--manifest-path", str(source / "Cargo.toml"), "--bin", "list-pilot", "--target", host, "--message-format=json-render-diagnostics", *profile]
        if features:
            argv += ["--features", "allocation"]
        completed = subprocess.run(argv, cwd=source, env=env, capture_output=True)
        compressed(output / (role + "-build.stdout.gz"), completed.stdout)
        compressed(output / (role + "-build.stderr.gz"), completed.stderr)
        require(completed.returncode == 0, role + " build failed; raw diagnostics retained")
        binary = cargo_executable(completed.stdout, source / "Cargo.toml", "list-pilot", "bin", features)
        destination = output / "artifacts" / ("list-pilot-" + role)
        shutil.copyfile(binary, destination)
        destination.chmod(0o755)
        runner.runtime_libraries(destination, output / (role + "-runtime-libraries.txt"))
        artifacts[role] = {"path": str(destination.relative_to(output)), "sha256": runner.digest(destination), "features": features}
        commands[role] = argv
    completed = subprocess.run([str(output / artifacts["allocation"]["path"]), "--allocation-preflight"], cwd=source, env=env, capture_output=True)
    compressed(output / "allocation-preflight.stdout.gz", completed.stdout)
    compressed(output / "allocation-preflight.stderr.gz", completed.stderr)
    require(completed.returncode == 0, "allocator preflight failed; raw diagnostics retained")
    allocation = preflight(strict_json_loads(completed.stdout))
    require(source_files() == before and tool_files() == tools and configuration(env, source) == config, "source/tool/configuration changed during build")
    require({name: runner.digest(source / name) for name in before} == before, "copied build source changed")
    manifest = {"version": 1, "kind": BUILD_KIND, "diagnostic_only": True, "source_sha256": before, "tool_sha256": tools,
        "rustc": command(["rustc", "+" + stable, "-vV"], cwd=source, env=env).decode(), "host_target": host,
        "cargo_configuration": config, "release_configuration": profile, "build_commands": commands,
        "artifacts": artifacts, "allocation_preflight": allocation, "scope": SCOPE}
    return runner.seal(output, manifest)


def affine_for_length(length):
    """Independent modular affine composition, rather than the child's edge loop."""
    quotient, remainder = divmod(length, 17)
    multiplier, addition = 1, 0
    block_multiplier, block_addition = 1, 0
    for word in range(17):
        block_multiplier = block_multiplier * 31 & MASK
        block_addition = (block_addition * 31 + word) & MASK
    while quotient:
        if quotient & 1:
            multiplier = block_multiplier * multiplier & MASK
            addition = (block_multiplier * addition + block_addition) & MASK
        block_addition = (block_multiplier * block_addition + block_addition) & MASK
        block_multiplier = block_multiplier * block_multiplier & MASK
        quotient >>= 1
    for word in range(remainder):
        multiplier = multiplier * 31 & MASK
        addition = (addition * 31 + word) & MASK
    return multiplier, addition


def expected_from_raw(raw, *, validate_owners=False):
    require(type(raw) is bytes and raw and raw.endswith(b"\n"), "census must be complete nonempty NDJSON")
    files, backings, edges, checksum, wide, scratch_minimum = 0, 0, 0, 0, 0, 0
    page_counts = {policy: 0 for policy in POLICIES[1:]}
    transforms = {}
    for index, line in enumerate(raw.splitlines()):
        row = strict_json_loads(line)
        require(type(row) is dict and type(row.get("index")) is int and row["index"] == index, "census file order/type changed")
        if validate_owners:
            owner_validation.validate_record(row, index)
        record = row.get("node_backings")
        require(type(record) is dict and type(record.get("physical_lengths_in_aux_order")) is list, "missing physical backing lengths")
        lengths = record["physical_lengths_in_aux_order"]
        histogram, total = Counter(), 0
        for length in lengths:
            integer(length, 0, 2**32 - 1)
            histogram[str(length)] += 1
            wide += total >= 2**32 - 1
            total += length
            if length not in transforms:
                transforms[length] = affine_for_length(length)
            multiplier, addition = transforms[length]
            checksum = (checksum * multiplier + addition) & MASK
        if "physical_length_histogram" in record:
            require(record["physical_length_histogram"] == dict(histogram), "backing order/histogram disagree")
        files += 1
        backings += len(lengths)
        edges += total
        scratch_minimum += max(lengths, default=0)
        for policy in page_counts:
            width = int(policy.removeprefix("page"))
            page_counts[policy] += (total + width - 1) // width
    return {"input_sha256": runner.sha(raw), "files": files, "backings": backings, "edges": edges, "checksum": checksum,
        "wide_backings": wide, "scratch_minimum": scratch_minimum, "page_counts": page_counts}


def validate_census(directory, expected_sha):
    manifest = sealed(directory, expected_sha, "s07_bis_untimed_owner_census")
    raw = (directory / "owners.ndjson").read_bytes()
    require(runner.sha(raw) == manifest["raw_sha256"] and len(raw) == manifest["raw_bytes"], "census bytes changed")
    require(strict_json_loads((directory / "child.stdout").read_bytes()) == manifest["child"], "census child/provenance changed")
    expected = expected_from_raw(raw, validate_owners=True)
    require(type(manifest["child"]["files"]) is int and expected["files"] == manifest["child"]["files"], "census file count changed")
    return manifest, raw, expected


def schedule():
    rows = []
    for warmup in (True, False):
        for allocation in (False, True):
            for index in range(1 if warmup else REPETITIONS):
                policies = POLICIES if (index + int(allocation)) % 2 == 0 else tuple(reversed(POLICIES))
                rows.extend({"warmup": warmup, "allocation": allocation, "index": index, "mode": mode} for mode in policies)
    return rows


def validate_child(value, expected, identity, sweeps):
    require(type(value) is dict and set(value) == {"version", "diagnostic_only", "mode", "scope", "input_sha256", "files", "backings", "edges", "checksum", "allocation", "timing", "storage"}, "unknown/missing child fields")
    require(integer(value["version"]) == 1 and value["diagnostic_only"] is True and value["scope"] == SCOPE and value["mode"] == identity["mode"], "child identity/scope changed")
    for key in ("files", "backings", "edges", "checksum"):
        require(integer(value[key]) == expected[key], "child work/checksum mismatch: " + key)
    require(value["input_sha256"] == expected["input_sha256"], "child loaded different input bytes")
    if identity["allocation"]:
        require(value["timing"] is None and type(value["allocation"]) is dict and set(value["allocation"]) == {"requested_bytes", "retained_requested_bytes", "freed_or_superseded_requests", "drop_returns_to_start"}, "wrong allocation metric presence")
        allocation = value["allocation"]
        for key in ("requested_bytes", "retained_requested_bytes", "freed_or_superseded_requests"):
            integer(allocation[key], 0 if key == "freed_or_superseded_requests" else 1)
        require(allocation["drop_returns_to_start"] is True and allocation["requested_bytes"] - allocation["retained_requested_bytes"] == allocation["freed_or_superseded_requests"], "allocation/drop accounting mismatch")
        element_bytes = 8 if identity["mode"] == "legacy" else 4
        require(allocation["retained_requested_bytes"] >= expected["edges"] * element_bytes, "retained counter is below stored edge bytes")
    else:
        require(value["allocation"] is None and type(value["timing"]) is dict and set(value["timing"]) == {"construction_ns", "traversal_ns", "sweeps"}, "wrong normal metric presence")
        for key in ("construction_ns", "traversal_ns"):
            integer(value["timing"][key], 1, 600_000_000_000)
        require(integer(value["timing"]["sweeps"], 1, 100) == sweeps, "traversal sweep count changed")
    if identity["mode"] == "legacy":
        require(value["storage"] is None, "legacy reported paged storage")
    else:
        storage = value["storage"]
        require(type(storage) is dict and set(storage) == {"edge_pages", "spare_edge_words", "retained_scratch_words", "wide_backings"}, "unknown/missing storage fields")
        for count in storage.values():
            integer(count)
        pages = expected["page_counts"][identity["mode"]]
        width = int(identity["mode"].removeprefix("page"))
        require(storage["edge_pages"] == pages and storage["spare_edge_words"] == pages * width - expected["edges"] and storage["wide_backings"] == expected["wide_backings"], "page/overflow counts differ from physical distribution")
        require(storage["retained_scratch_words"] >= expected["scratch_minimum"], "scratch capacity cannot hold the largest lists")
    return value


def summarize(observations):
    result = {}
    for mode in POLICIES:
        selected = [row for row in observations if not row["identity"]["warmup"] and row["identity"]["mode"] == mode]
        metrics = {}
        for group, names in (("timing", ("construction_ns", "traversal_ns")), ("allocation", ("requested_bytes", "retained_requested_bytes", "freed_or_superseded_requests"))):
            values = [row["child"][group] for row in selected if row["child"][group] is not None]
            require(len(values) == REPETITIONS, "missing measured repetition")
            for name in names:
                samples = [value[name] for value in values]
                center = median(samples)
                metrics[name] = {"samples": samples, "median": center, "relative_mad": median(abs(n - center) for n in samples) / center if center else 0.0}
        result[mode] = metrics
    return result


def quiet_host():
    reject_concurrent_builds()
    names = command(["ps", "-axo", "comm="], cwd=ROOT).decode().splitlines()
    extra = {"list-pilot", "list-pilot-normal", "list-pilot-allocation", "owner-census", "tsr_s07_bis_owner_census", "phase-probe", "s07-phase-probe"}
    active = sorted({Path(name.strip()).name for name in names} & extra)
    require(not active, "measurement host has another diagnostic child: " + ", ".join(active))


@contextmanager
def measurement_lock():
    path = runner.CACHE / "s07-benchmark/measurement.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("another performance capture owns this host") from error
        yield


def raw_name(index, stream):
    return f"raw/{index:03d}.{stream}.gz"


def capture(build_dir, build_sha, census_dir, census_sha, output, sweeps):
    integer(sweeps, 1, 100)
    bundle = validate_build(build_dir, build_sha)
    census, raw, expected = validate_census(census_dir, census_sha)
    require(tool_files() == bundle["tool_sha256"], "capture helpers differ from the frozen build")
    output.mkdir(parents=True, exist_ok=False)
    (output / "raw").mkdir()
    (output / "input.ndjson").write_bytes(raw)
    (output / "input.ndjson").chmod(0o444)
    runner.copy_file(census_dir / "manifest.json", output / "census-manifest.json")
    env = native_environment()
    config = configuration(env, HERE)
    metadata = {"version": 1, "kind": CAPTURE_KIND, "diagnostic_only": True, "scope": SCOPE,
        "build_manifest_sha256": build_sha, "build_directory": str(build_dir), "capture_directory": str(output), "artifacts": bundle["artifacts"],
        "census_manifest_sha256": census_sha, "census_build_manifest_sha256": census["build_manifest_sha256"],
        "expected": expected, "sweeps": sweeps, "host": host_info(), "tool_sha256": tool_files(), "cargo_configuration": config,
        "schedule": schedule(), "allocation_preflight": bundle["allocation_preflight"], "policy": "one warmup and seven measured children per policy/build; alternating forward/reverse policy order; no extension or selection",
        "limitations": ["Synthetic edges and auxiliary completion order do not reproduce parser construction or nested list starts.", "Timing excludes input parsing, checksum setup and root disposal; traversal is separately timed.", "No AST, list headers, real node retention, imports, binding, whole-owner allocation, RSS or gate claim.", "Scratch remains retained; native requested bytes count original allocation and realloc sizes."]}
    runner.write_json(output / "report.json", {**metadata, "status": "in_progress"})
    observations = []
    try:
        for index, identity in enumerate(metadata["schedule"]):
            quiet_host()
            require(runner.digest(output / "input.ndjson") == expected["input_sha256"], "frozen child input changed")
            role = "allocation" if identity["allocation"] else "normal"
            binary = build_dir / bundle["artifacts"][role]["path"]
            require(runner.digest(binary) == bundle["artifacts"][role]["sha256"], "binary changed before child")
            argv = [str(binary), str(output / "input.ndjson"), identity["mode"], str(sweeps)]
            try:
                completed = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, timeout=600)
                stdout, stderr, returncode = completed.stdout, completed.stderr, completed.returncode
            except subprocess.TimeoutExpired as error:
                stdout, stderr, returncode = error.stdout or b"", error.stderr or b"", None
            compressed(output / raw_name(index, "stdout"), stdout)
            compressed(output / raw_name(index, "stderr"), stderr)
            require(returncode == 0, f"child {index} failed or timed out; raw output retained")
            child = validate_child(strict_json_loads(stdout), expected, identity, sweeps)
            observations.append({"identity": identity, "returncode": returncode, "command": argv, "stdout_sha256": runner.sha(stdout), "stderr_sha256": runner.sha(stderr), "child": child})
        validate_build(build_dir, build_sha)
        require(runner.digest(output / "input.ndjson") == expected["input_sha256"] and runner.digest(output / "census-manifest.json") == census_sha, "captured input/provenance changed")
        require(tool_files() == metadata["tool_sha256"] and configuration(env, HERE) == config, "capture tools/configuration changed")
        report = {**metadata, "status": "complete", "observations": observations, "summary": summarize(observations)}
        validate_observations(report, output, expected)
        runner.write_json(output / "report.json", report)
        return runner.seal(output, report)
    except BaseException as error:
        runner.write_json(output / "report.json", {**metadata, "status": "failed", "error": str(error), "observations": observations})
        raise


def validate_observations(report, directory, expected):
    require(report["schedule"] == schedule() and len(report["observations"]) == len(schedule()), "missing/extra/reordered capture schedule")
    for identity in report["schedule"]:
        require(type(identity["warmup"]) is bool and type(identity["allocation"]) is bool and type(identity["index"]) is int, "schedule integer/boolean types changed")
    storage_seen = {}
    raw_files = set()
    for index, (identity, observation) in enumerate(zip(schedule(), report["observations"], strict=True)):
        require(type(observation) is dict and set(observation) == {"identity", "returncode", "command", "stdout_sha256", "stderr_sha256", "child"}, "unknown/missing observation fields")
        require(observation["identity"] == identity and type(observation["identity"]["index"]) is int and type(observation["identity"]["warmup"]) is bool and type(observation["identity"]["allocation"]) is bool, "observation identity/type/order changed")
        require(type(observation["returncode"]) is int and observation["returncode"] == 0, "failed child cannot become a sample")
        role = "allocation" if identity["allocation"] else "normal"
        argv = [str(Path(report["build_directory"]) / report["artifacts"][role]["path"]), str(Path(report["capture_directory"]) / "input.ndjson"), identity["mode"], str(report["sweeps"])]
        require(observation["command"] == argv, "child command/input/build mode changed")
        for stream in ("stdout", "stderr"):
            name = raw_name(index, stream)
            raw_files.add(name)
            raw = gzip.decompress((directory / name).read_bytes())
            require(runner.sha(raw) == observation[stream + "_sha256"], "raw child output changed")
            if stream == "stdout":
                child = validate_child(strict_json_loads(raw), expected, identity, report["sweeps"])
                require(child == observation["child"], "raw/parsed child observations differ")
        mode = identity["mode"]
        if mode in storage_seen:
            require(child["storage"] == storage_seen[mode], "deterministic storage counts changed across children")
        storage_seen[mode] = child["storage"]
    require({str(path.relative_to(directory)) for path in (directory / "raw").iterdir()} == raw_files, "missing/extra raw child files")
    require(summarize(report["observations"]) == report["summary"], "summary does not replay from all measured samples")


def verify(directory, capture_sha, build_dir, build_sha):
    bundle = validate_build(build_dir, build_sha)
    report = sealed(directory, capture_sha, CAPTURE_KIND)
    require(report["status"] == "complete" and report["scope"] == SCOPE and report["build_manifest_sha256"] == build_sha and report["tool_sha256"] == bundle["tool_sha256"] == tool_files() and report["artifacts"] == bundle["artifacts"], "capture provenance changed")
    require(runner.digest(directory / "census-manifest.json") == report["census_manifest_sha256"], "census manifest changed")
    census = strict_json_loads((directory / "census-manifest.json").read_bytes())
    expected = expected_from_raw((directory / "input.ndjson").read_bytes(), validate_owners=True)
    require(expected == report["expected"] and expected["input_sha256"] == census["raw_sha256"] and expected["files"] == census["child"]["files"], "input replay differs")
    require(preflight(report["allocation_preflight"]) == bundle["allocation_preflight"], "preflight provenance changed")
    integer(report["sweeps"], 1, 100)
    validate_observations(report, directory, expected)
    require(strict_json_loads((directory / "report.json").read_bytes()) == {key: value for key, value in report.items() if key != "inventory"}, "report/manifest differ")
    return {"diagnostic_only": True, "scope": SCOPE, "children": len(report["observations"]), "summary": report["summary"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "capture", "verify"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build", type=Path)
    parser.add_argument("--build-sha")
    parser.add_argument("--census", type=Path)
    parser.add_argument("--census-sha")
    parser.add_argument("--capture-sha")
    parser.add_argument("--sweeps", type=int, default=8)
    args = parser.parse_args()
    if args.command != "build":
        require(args.build is not None and args.build_sha is not None, "build directory and external manifest SHA are required")
    if args.command == "verify":
        result = verify(args.output.resolve(), args.capture_sha, args.build.resolve(), args.build_sha)
    else:
        with measurement_lock():
            if args.command == "build":
                result = build(args.output.resolve())
            else:
                require(args.census is not None and args.census_sha is not None, "sealed owner census and external manifest SHA are required")
                result = capture(args.build.resolve(), args.build_sha, args.census.resolve(), args.census_sha, args.output.resolve(), args.sweeps)
        result = {"manifest_sha256": result}
    print(json.dumps(result, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
