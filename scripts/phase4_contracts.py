#!/usr/bin/env python3
"""Run/replay the normal Phase 4 contract suites, independently of X7/TSAN.

The receipt binds Cargo's test artifacts, complete libtest inventories, raw
streams, the current unit roster and sources. Replay starts no children. Native
backend skips are failures for that suite, not evidence of watcher coverage.
Artifacts stay under target; the tsc evidence records the receipt's hash.
"""
from __future__ import annotations

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import shutil
import sys

import phase4_acceptance as acceptance
import phase4_sanitizer as tests
import phase4_unit_tests as roster

ROOT = tests.ROOT
DEFAULT_OUTPUT = ROOT / "target/phase4/contracts"
PACKAGES = (*tests.PACKAGES, "tsr_vfs")
LIB = "phase4_tsctests:lib:phase4_tsctests"
OWNERSHIP = "phase4_tsctests:test:ownership"
DIRECT = "phase4_tsctests:test:build_direct"
RACE = "phase4_tsctests:test:watcher_race"
FSWATCH = "tsr_fswatch:lib:tsr_fswatch"
FSWATCH_DOC = "tsr_fswatch:doc:tsr_fswatch"
CALLBACK_WITNESS = "crates/tsr_fswatch/src/watcher.rs::watch_directory"
REQUIRED = {
    "X1": (LIB, DIRECT, OWNERSHIP, "tsr:bin:tsrust", "tsr_execute:lib:tsr_execute", "tsr_tsc:lib:tsr_tsc"),
    "X2": (LIB, OWNERSHIP, "tsr_incremental:lib:tsr_incremental", "tsr_incremental:test:buildinfo_witness"),
    "X3": (LIB, DIRECT, OWNERSHIP, "tsr_build:lib:tsr_build"),
    "X4": (FSWATCH, FSWATCH_DOC),
    "X5": (LIB, DIRECT, RACE, OWNERSHIP, "tsr_execute:lib:tsr_execute"),
    "X6": (LIB, "tsr_tracing:lib:tsr_tracing"),
}


def sources():
    return {**tests.sources(), **{name: tests.digest((ROOT / name).read_bytes()) for name in (
        "scripts/phase4_contracts.py", "scripts/phase4_unit_tests.py", "scripts/phase4_acceptance.py",
        "scripts/phase4_audit.py", "scripts/phase2_audit.py", "scripts/tests/test_phase4_contracts.py")}}


def targets():
    return tests.test_targets(package_paths={**tests.PACKAGE_PATHS, "tsr_vfs": "crates/tsr_vfs"})


def command():
    return ["cargo", "test", "--locked", "--no-run", "--lib", "--bins", "--tests", "--message-format=json",
            "--target-dir", str(ROOT / "target"), *[arg for p in PACKAGES for arg in ("-p", p)]]


def doc_command(args):
    return ["cargo", "test", "--locked", "--doc", "-p", "tsr_fswatch", "--target-dir", str(ROOT / "target"),
            "--", *args]


def doc_inventory():
    # This Go runtime rejection is a Rust type-system contract. Authenticate
    # the actual compile-fail example; a same-named runtime test is no witness.
    path = CALLBACK_WITNESS.split("::")[0]
    text = (ROOT / path).read_text()
    examples = list(re.finditer(r"(?m)^    /// ```compile_fail\n(?:    //[^\n]*\n)+    pub fn watch_directory\(", text))
    tests.require(len(examples) == 1, "missing callback compile-fail witness")
    line = text[:examples[0].start()].count("\n") + 1
    return [f"{path} - watcher::Watcher::watch_directory (line {line})"]


def environment():
    # Reuse the sanitizer's override/filter removal, without instrumentation.
    env = tests.instrumentation_environment(os.environ)
    for name in ("RUSTFLAGS", "TSAN_OPTIONS", "CARGO_INCREMENTAL"):
        env.pop(name, None)
    return env


def built_targets(directory, report):
    events = tests.read_events(directory / "processes/build/stdout")
    tests.require([e.get("success") for e in events if e.get("reason") == "build-finished"] == [True],
                  "missing successful Cargo build")
    images = {}
    for event in events:
        if event.get("reason") != "compiler-artifact" or not event.get("executable"):
            continue
        if event.get("profile", {}).get("test") is not True:
            continue
        target = event["target"]
        found = [t for t in targets() if target.get("name") == t["name"] and target.get("kind") == [t["kind"]]
                 and target.get("src_path") == str(Path(report["source_root"]) / t["source"])]
        tests.require(len(found) == 1, "unexpected test executable")
        item = found[0]
        key = tests.target_id(item)
        tests.require(key not in images, "duplicate Cargo target")
        tests.require(event.get("manifest_path") == str(Path(report["source_root"]) /
                      ("tools/phase4/tsctests" if item["package"] == "phase4_tsctests" else "crates/" + item["package"])
                      / "Cargo.toml"), "test artifact manifest differs")
        images[key] = {**item, "built_path": event["executable"], "image": "images/" + key.replace(":", "-")}
    tests.require(set(images) == {tests.target_id(t) for t in targets()}, "missing required test targets")
    return images


def process(directory, report, name, expected):
    folder = directory / "processes" / name
    invocation = tests.load(folder / "invocation.json")
    tests.require(invocation == {"command": expected, "cwd": report["source_root"],
                                "environment": report["environment"], "unset_environment": list(tests.TEST_FILTERS),
                                "timeout_seconds": 600}, "test invocation changed: " + name)
    result = tests.load(folder / "result.json")
    tests.require(type(result.get("status")) is int and result.get("timeout") is False and result.get("error") is None,
                  "unfinished test invocation: " + name)
    return result["status"], (folder / "stdout").read_bytes(), (folder / "stderr").read_bytes()


def coverage(runs, host, document):
    """Credit each applicable Go test only through its observed Rust witness."""
    checkpoints = {f"X{i}": {"observed": 0, "required": 0, "missing": []} for i in range(1, 7)}
    for row in document["tests"]:
        if row["status"] == "not_applicable" or row["host"] not in ("any", host):
            continue
        checkpoint = checkpoints[row["checkpoint"]]
        checkpoint["required"] += 1
        observed = row["status"] == "ported" and bool(row.get("rust"))
        for home in row.get("rust", []):
            if home["test"] == CALLBACK_WITNESS:
                observed = observed and runs.get(FSWATCH_DOC, {}).get("passed") is True
                continue
            path, name = home["test"].rsplit("::", 1)
            package = "phase4_tsctests" if path.startswith("tools/phase4/tsctests/") else path.split("/")[1]
            candidates = [value for key, value in runs.items() if key.split(":")[0] == package
                          and (key.split(":")[1] != "test" or Path(path).stem == key.split(":")[2])]
            observed = observed and any(value["passed"] and any(test.rsplit("::", 1)[-1] == name
                                                                 for test in value["tests"]) for value in candidates)
        if observed:
            checkpoint["observed"] += 1
        else:
            checkpoint["missing"].append(row["id"])
    return checkpoints


def replay(directory=DEFAULT_OUTPUT, *, expected_host=None):
    directory = Path(directory)
    raw = (directory / "report.json").read_bytes()
    report = tests.load(directory / "report.json")
    tests.require(report.get("version") == 1 and report.get("sources") == sources(), "stale contract receipt")
    host = report.get("host")
    tests.require(host in acceptance.HOSTS and (expected_host is None or host == expected_host), "wrong contract host")
    tests.require(report.get("artifacts") == tests.artifacts(directory), "contract artifacts changed")
    tests.require(not roster.check(), "unit roster is stale or invalid")
    build_command = [str(arg).replace(str(ROOT), report["source_root"]) for arg in command()]
    tests.require(process(directory, report, "build", build_command)[0] == 0, "test build failed")
    images = built_targets(directory, report)
    tests.require(set(report.get("images", {})) == set(images), "image inventory changed")
    runs, schedule = {}, {"build"}
    for key, item in images.items():
        expected = {**item, "sha256": tests.digest((directory / item["image"]).read_bytes())}
        tests.require(report["images"][key] == expected, "test image binding changed")
        image = str(Path(report["capture_root"]) / item["image"])
        name = key.replace(":", "-")
        status, stdout, _ = process(directory, report, name + "-list", [image, "--list", "--format=pretty"])
        tests.require(status == 0, "test listing failed")
        names = tests.validate_inventory(stdout)
        # tsr_vfs contributes the host-specific nativepath witness via the
        # pinned roster below; its other inline cfg tests aren't an X roster.
        if item["package"] != "tsr_vfs":
            required = tests.required_test_names(item, {"os": host})
            tests.require(not (required - Counter(n.rsplit("::", 1)[-1] for n in names)), "test source floor differs")
        status, stdout, stderr = process(directory, report, name + "-run",
                                          [image, "--test-threads=1", "--show-output", "--format=pretty"])
        passed = False
        try:
            tests.validate_test_run(stdout, names)
            passed = status == 0 and not tests.SKIP.search(stdout + b"\n" + stderr)
        except tests.MeasuredFailure:
            pass
        runs[key] = {"tests": names, "passed": bool(passed)}
        schedule.update((name + "-list", name + "-run"))
    name = FSWATCH_DOC.replace(":", "-")
    doc_commands = [(["--list", "--format=pretty"], "list"),
                    (["--test-threads=1", "--show-output", "--format=pretty"], "run")]
    results = [process(directory, report, name + "-" + suffix,
                       [arg.replace(str(ROOT), report["source_root"]) for arg in doc_command(args)])
               for args, suffix in doc_commands]
    status, stdout, _ = results[0]
    tests.require(status == 0 and tests.validate_inventory(stdout) == doc_inventory(),
                  "compile-fail test inventory changed")
    status, stdout, stderr = results[1]
    passed = False
    try:
        tests.validate_test_run(stdout, doc_inventory(), name_suffixes=(" - compile fail",))
        passed = status == 0 and not tests.SKIP.search(stdout + b"\n" + stderr)
    except tests.MeasuredFailure:
        pass
    runs[FSWATCH_DOC] = {"tests": doc_inventory(), "passed": bool(passed)}
    schedule.update(name + "-" + suffix for _, suffix in doc_commands)
    tests.require({p.name for p in (directory / "processes").iterdir()} == schedule, "test process schedule changed")
    checkpoints = coverage(runs, host, tests.load(roster.ROSTER))
    required = sum(c["required"] for c in checkpoints.values())
    observed = sum(c["observed"] for c in checkpoints.values())
    metrics = {"unit_rosters": observed / required if required else 0.0}
    for checkpoint, suites in REQUIRED.items():
        metrics[checkpoint.lower() + "_contracts"] = (all(runs.get(s, {}).get("passed") is True for s in suites)
                                                        and checkpoints[checkpoint]["required"] > 0
                                                        and not checkpoints[checkpoint]["missing"])
    codec = runs.get(LIB, {})
    metrics["buildinfo_codec"] = int(codec.get("passed") is True and
        "tests::readable_build_info_renders_every_committed_rendering" in codec.get("tests", []))
    metrics["watcher_tests"] = metrics["x4_contracts"]
    if metrics["watcher_tests"]:
        tests.backend_coverage({"os": host}, runs)
    return {"metrics": metrics, "identity": tests.digest(raw), "host": host,
            "rosters": checkpoints, "suites": runs}


def run(directory):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "images").mkdir()
    env = environment()
    report = {"version": 1, "host": acceptance.current_host(), "sources": sources(),
              "source_root": str(ROOT), "capture_root": str(directory), "environment": tests.environment_record(env),
              "images": {}}
    try:
        tests.invoke(directory, "build", command(), env, 600)
        for key, item in built_targets(directory, report).items():
            source, destination = Path(item["built_path"]), directory / item["image"]
            # Cargo replaces linked outputs; hard links preserve this image
            # without another copy of every debug executable on the same disk.
            try:
                os.link(source, destination)
            except OSError:
                shutil.copy2(source, destination)
            report["images"][key] = {**item, "sha256": tests.digest(destination.read_bytes())}
            name = key.replace(":", "-")
            for suffix, args in (("list", ["--list", "--format=pretty"]),
                                 ("run", ["--test-threads=1", "--show-output", "--format=pretty"])):
                try:
                    tests.invoke(directory, name + "-" + suffix, [str(destination), *args], env, 600)
                except ValueError:
                    # Preserve a failed suite, and still measure independent ones.
                    pass
        for suffix, args in (("list", ["--list", "--format=pretty"]),
                             ("run", ["--test-threads=1", "--show-output", "--format=pretty"])):
            try:
                tests.invoke(directory, FSWATCH_DOC.replace(":", "-") + "-" + suffix, doc_command(args), env, 600)
            except ValueError:
                pass
    finally:
        report["artifacts"] = tests.artifacts(directory)
        tests.write(directory / "report.json", report)
    return replay(directory, expected_host=acceptance.current_host())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("run", "replay"))
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    result = run(args.output) if args.command == "run" else replay(args.output)
    print(json.dumps(result, sort_keys=True))
    return 0 if all(result["metrics"].values()) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 contracts: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
