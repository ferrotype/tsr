#!/usr/bin/env python3
"""Capture and replay Phase 4 ThreadSanitizer evidence without installing tools.

Run:    phase4_sanitizer.py run --output DIR
Replay: phase4_sanitizer.py replay --output DIR

Only run starts children. It requires the installed pinned nightly and rust-src,
uses a fresh build directory, and preserves Cargo/Rustup homes and offline/mirror
configuration. Failures and unavailable backends remain in the receipt. Replay
checks the actual compiler audit, Cargo artifacts, copied images, complete test
inventories and raw outputs; a report's claimed result is never trusted.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time
import tomllib
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ("phase4_tsctests", "tsr", "tsr_execute", "tsr_tsc", "tsr_build",
            "tsr_incremental", "tsr_fswatch", "tsr_tracing")
PACKAGE_PATHS = {name: ("tools/phase4/tsctests" if name == PACKAGES[0] else "crates/" + name)
                 for name in PACKAGES}
FLAGS = "-Zsanitizer=thread"
TSAN_OPTIONS = "halt_on_error=1:exitcode=66"
JOBS = 4
TEST_FILTERS = ("PHASE3_INCREMENTAL_CASE",)
SYSTEM_ALLOCATOR_FEATURE = "system-allocator"
COUNTING_ALLOCATOR_SHA256 = "4fd661085728fb4f2773cc176bf9b73c6f1dbe5a219cf134fdf018b324377282"
SOURCE_PATTERNS = ("crates/**/*", "tools/phase4/**/*", "tools/**/*.rs",
                   "tools/**/Cargo.toml", "xtask/Cargo.toml", "Cargo.toml", "Cargo.lock",
                   "rust-toolchain*", ".cargo/**/*", "scripts/phase4_sanitizer.py",
                   "scripts/phase4_corpus.py", "scripts/phase4_scenarios.py", "scripts/s08_oracle.py",
                   "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                   "scripts/tracking-bootstrap.py",
                   "scripts/tests/test_phase4_sanitizer.py", "data/s04/toolchains.toml",
                   "data/upstream.json", "data/phase4/scenarios.json.gz",
                   "PORTS.toml")
DIAGNOSTIC = re.compile(rb"ThreadSanitizer|Sanitizer CHECK failed|Sanitizer:DEADLYSIGNAL|"
                        rb"(?:WARNING|SUMMARY|FATAL):[^\n]*[Ss]anitizer", re.I)
SKIP = re.compile(rb"(?:^|\n)\s*(?:skip(?:ped)?\s*:|.*not available|.*not supported:)", re.I)
LIST_LINE = re.compile(r"^(.+): test$", re.M)
TEST_LINE = re.compile(r"^test (.+) \.\.\. (ok|FAILED|ignored(?:, .*)?)$", re.M)
SUMMARY = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
                     r"(\d+) measured; (\d+) filtered out;.*$", re.M)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return strict_json_loads(Path(path).read_bytes())


def write(path, value):
    Path(path).write_bytes(canonical(value) + b"\n")


def require(condition, message):
    if not condition:
        raise ValueError(message)


class MeasuredFailure(ValueError):
    """An authenticated completed schedule observed a failing measurement."""


def measured(condition, message):
    if not condition:
        raise MeasuredFailure(message)


def sources():
    """Include integration tests, fixtures, runner helpers, pin and build config."""
    paths = {p for pattern in SOURCE_PATTERNS for p in ROOT.glob(pattern)
             if p.is_file() and not {"target", "__pycache__", ".git"}.intersection(p.relative_to(ROOT).parts)
             and p.name != ".DS_Store"}
    return {p.relative_to(ROOT).as_posix(): digest(p.read_bytes()) for p in sorted(paths)}


def nightly():
    return tomllib.loads((ROOT / "data/s04/toolchains.toml").read_text())["nightly"]


def instrumentation_environment(base):
    """Keep registry/offline settings and homes; remove compiler/runtime overrides."""
    env = base.copy()
    for key in list(env):
        if (key in TEST_FILTERS or key in {"RUSTC", "RUSTDOC", "RUSTFLAGS", "RUSTDOCFLAGS", "RUSTUP_TOOLCHAIN",
                    "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_ENCODED_RUSTFLAGS",
                    "CARGO_ENCODED_RUSTDOCFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR",
                    "TSAN_OPTIONS", "ASAN_OPTIONS", "LSAN_OPTIONS", "UBSAN_OPTIONS", "MIRIFLAGS",
                    "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "RUST_TEST_THREADS", "RUST_TEST_NOCAPTURE"}
                or key.startswith(("CARGO_BUILD_", "CARGO_PROFILE_", "MIRI_", "PHASE4_TSAN_"))
                or re.fullmatch(r"CARGO_TARGET_.+_(RUSTFLAGS|RUNNER|LINKER)", key)):
            env.pop(key)
    if env.get("S04_RUSTUP_HOME"):
        env["RUSTUP_HOME"] = str(Path(env["S04_RUSTUP_HOME"]).expanduser().resolve())
    env.update(RUSTFLAGS=FLAGS, RUSTC_WORKSPACE_WRAPPER="", CARGO_INCREMENTAL="0",
               CARGO_TERM_COLOR="never", TSAN_OPTIONS=TSAN_OPTIONS, RUST_BACKTRACE="1")
    return env


def host_from_target(target):
    os_name = "macos" if "apple-darwin" in target else "linux" if "linux" in target else None
    require(os_name is not None, "ThreadSanitizer receipt supports macOS and Linux hosts")
    return {"os": os_name, "arch": target.split("-", 1)[0]}


def test_targets(*, package_paths=PACKAGE_PATHS):
    """Independently derive required lib/bin/integration targets from manifests."""
    targets = []
    for package, directory in package_paths.items():
        base = ROOT / directory
        manifest = tomllib.loads((base / "Cargo.toml").read_text())
        entries = []
        lib = manifest.get("lib", {})
        if lib or (base / "src/lib.rs").is_file():
            entries.append(("lib", {"name": package, "path": "src/lib.rs", **lib}))
        bins = manifest.get("bin", [])
        entries.extend(("bin", entry) for entry in bins)
        if ((base / "src/main.rs").is_file() and manifest["package"].get("autobins", True)
                and not any(entry.get("path") == "src/main.rs" for entry in bins)):
            entries.append(("bin", {"name": package, "path": "src/main.rs"}))
        explicit = manifest.get("test", [])
        entries.extend(("test", entry) for entry in explicit)
        if manifest["package"].get("autotests", True):
            for file in sorted([*base.glob("tests/*.rs"), *base.glob("tests/*/main.rs")]):
                name = file.parent.name if file.name == "main.rs" else file.stem
                if not any(entry["name"] == name for entry in explicit):
                    entries.append(("test", {"name": name, "path": file.relative_to(base).as_posix()}))
        for kind, entry in entries:
            require(entry.get("test", True) and entry.get("harness", True),
                    "all Phase 4 test targets must use the test harness")
            name = entry.get("name", package)
            default = f"tests/{name}.rs" if kind == "test" else "src/main.rs" if kind == "bin" else "src/lib.rs"
            targets.append({"package": package, "kind": kind, "name": name,
                            "source": directory + "/" + entry.get("path", default)})
    return sorted(targets, key=target_id)


def required_test_names(target, host):
    """Conservative source floor, independent of a recorded libtest inventory.

    Phase 4 uses ordinary #[test] functions. Platform modules are selected here;
    unexpected future cfg arrangements fail closed until this policy is updated.
    The compiled --list inventory remains authoritative for full module names.
    """
    base = ROOT / PACKAGE_PATHS[target["package"]]
    if target["kind"] == "test":
        paths = [ROOT / target["source"]]
    elif target["package"] == PACKAGES[0] and target["kind"] == "bin":
        paths = [base / "src/main.rs"]
    else:
        paths = list(base.glob("src/**/*.rs"))
        if target["kind"] == "lib":
            paths = [path for path in paths if path != base / "src/main.rs"]
    pattern = re.compile(r"#\[test\]\s*(?:(?://[^\n]*\n|#\[[^\n]*\]\s*)\s*)*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
    names = []
    for path in paths:
        relative = path.relative_to(base).parts
        if target["package"] == "tsr_fswatch":
            excluded = "linux" if host["os"] == "macos" else "macos"
            if excluded in relative or excluded + ".rs" in relative:
                continue
        names.extend(pattern.findall(path.read_text()))
    return Counter(names)


def target_id(target):
    return ":".join(target[k] for k in ("package", "kind", "name"))


def artifacts(directory):
    result = {}
    for path in sorted(directory.rglob("*")):
        relative = path.relative_to(directory)
        if relative.parts[0] == "build-target" or relative.as_posix() == "report.json":
            continue
        require(not path.is_symlink(), "symlink in sanitizer receipt: " + str(relative))
        if path.is_file():
            result[relative.as_posix()] = digest(path.read_bytes())
    return result


def wrapper_text(python, source_root):
    return (f"#!{python}\nimport runpy, sys\n"
            f"module = runpy.run_path({str(Path(source_root) / 'scripts/phase4_sanitizer.py')!r})\n"
            "raise SystemExit(module['compiler_wrapper'](sys.argv[1:]))\n")


def compiler_wrapper(arguments):
    """Cargo's actual rustc invocations, including instrumented std/core builds."""
    audit = Path(os.environ["PHASE4_TSAN_AUDIT"])
    destination = audit / (uuid.uuid4().hex + ".json")
    compiler = Path(arguments[0]).resolve()
    record = {"compiler": str(compiler), "compiler_sha256": digest(compiler.read_bytes()),
              "arguments": arguments[1:], "cwd": os.getcwd(), "status": None, "outputs": {}}
    write(destination, record)
    require(str(compiler) == os.environ["PHASE4_TSAN_RUSTC"], "Cargo substituted another compiler")
    require(record["compiler_sha256"] == os.environ["PHASE4_TSAN_RUSTC_SHA256"], "compiler image changed")
    result = subprocess.run([str(compiler), *arguments[1:]], check=False)
    record["status"] = result.returncode
    args = arguments[1:]
    if result.returncode == 0 and "--out-dir" in args and "--crate-name" in args:
        out_dir = Path(args[args.index("--out-dir") + 1])
        crate = args[args.index("--crate-name") + 1]
        extra = next((arg.split("=", 1)[1] for arg in args if arg.startswith("extra-filename=")), "")
        image = out_dir / (crate + extra)
        if image.is_file():
            record["outputs"][str(image.resolve())] = digest(image.read_bytes())
    require(digest(compiler.read_bytes()) == record["compiler_sha256"], "compiler changed during compilation")
    write(destination, record)
    return result.returncode


def environment_record(env):
    keys = {"RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_TARGET_DIR",
            "CARGO_INCREMENTAL", "TSAN_OPTIONS", "CARGO_TERM_COLOR", "RUST_BACKTRACE",
            "CARGO_HOME", "RUSTUP_HOME", "CARGO_NET_OFFLINE", "PHASE4_TSAN_AUDIT",
            "PHASE4_TSAN_RUSTC", "PHASE4_TSAN_RUSTC_SHA256"}
    return {key: env[key] for key in sorted(keys) if key in env}


def invoke(directory, name, command, env, timeout):
    work = directory / "processes" / name
    work.mkdir(parents=True)
    write(work / "invocation.json", {"command": list(map(str, command)), "cwd": str(ROOT),
                                     "environment": environment_record(env), "unset_environment": list(TEST_FILTERS),
                                     "timeout_seconds": timeout})
    started = time.monotonic()
    status, timed_out, error = None, False, None
    with (work / "stdout").open("wb") as out, (work / "stderr").open("wb") as err:
        try:
            child = subprocess.run(command, cwd=ROOT, env=env, stdout=out, stderr=err,
                                   timeout=timeout, check=False)
            status = child.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
        except OSError as problem:
            error = str(problem)
    write(work / "result.json", {"status": status, "timeout": timed_out, "error": error,
                                "seconds": time.monotonic() - started})
    require(status == 0 and not timed_out and error is None, f"{name} failed; raw streams retained")
    return (work / "stdout").read_bytes()


def cargo_command(report, test):
    prefix = ["rustup", "run", report["nightly"], "cargo", "test" if test else "build", "--locked",
              "-Zbuild-std", "--target", report["target"], "--message-format=json",
              "--config", "build.rustc=" + json.dumps(report["compiler"]["path"]),
              "--config", "build.rustc-wrapper=" + json.dumps(report["capture_root"] + "/rustc-wrapper"),
              "--config", 'build.rustc-workspace-wrapper=""']
    if test:
        prefix += ["--no-run", "--lib", "--bins", "--tests",
                   "--features", "tsr/" + SYSTEM_ALLOCATOR_FEATURE]
        for package in PACKAGES:
            prefix += ["--package", package]
    else:
        prefix += ["--package", PACKAGES[0], "--bin", PACKAGES[0]]
    return prefix


def read_events(path):
    # Cargo JSON is line oriented; duplicate fields are rejected just as in receipts.
    return [strict_json_loads(line) for line in path.read_bytes().splitlines()]


def built_images(directory, report, test):
    expected = test_targets() if test else [{"package": PACKAGES[0], "name": PACKAGES[0],
                                           "kind": "bin", "source": "tools/phase4/tsctests/src/main.rs"}]
    events = read_events(directory / "processes" / ("tests-build" if test else "harness-build") / "stdout")
    finished = [row for row in events if row.get("reason") == "build-finished"]
    require(len(finished) == 1 and finished[0].get("success") is True, "missing successful Cargo build finish")
    rows = {}
    for event in events:
        if event.get("reason") != "compiler-artifact" or not event.get("executable"):
            continue
        if event.get("profile", {}).get("test") is not test:
            continue
        target = event["target"]
        matches = [item for item in expected if target.get("name") == item["name"]
                   and target.get("kind") == [item["kind"]]
                   and target.get("src_path") == str(Path(report["source_root"]) / item["source"])]
        require(len(matches) == 1, "unexpected Cargo executable target")
        item = matches[0]
        if item["package"] == "tsr":
            require(event.get("features") == [SYSTEM_ALLOCATOR_FEATURE],
                    "CLI Cargo artifact did not select the system allocator")
        key = target_id(item)
        require(key not in rows, "duplicate Cargo executable target")
        image = Path(event["executable"])
        require(image.is_relative_to(Path(report["capture_root"]) / "build-target" / report["target"]),
                "Cargo executable is outside the instrumented target directory")
        rows[key] = {**item, "built_path": str(image)}
    require(set(rows) == {target_id(item) for item in expected}, "missing required Cargo test or harness target")
    return dict(sorted(rows.items()))


def validate_inventory(raw):
    text = raw.decode("utf-8")
    names = LIST_LINE.findall(text)
    counts = re.findall(r"^(\d+) tests?, (\d+) benchmarks?$", text, re.M)
    require(len(counts) == 1 and tuple(map(int, counts[0])) == (len(names), 0)
            and len(names) == len(set(names)), "invalid or incomplete --list inventory")
    return sorted(names)


def validate_test_run(raw, names, *, name_suffixes=(" - should panic",)):
    text = raw.decode("utf-8")
    lines = []
    for name, status in TEST_LINE.findall(text):
        # libtest decorates expected-panic results, but not its --list names.
        # Rustdoc similarly decorates compile-fail tests; callers must opt in.
        for suffix in name_suffixes:
            name = name.removesuffix(suffix)
        lines.append((name, status))
    counts = SUMMARY.findall(text)
    measured(len(counts) == 1 and counts[0][0] == "ok"
            and tuple(map(int, counts[0][1:])) == (len(names), 0, 0, 0, 0),
            "test summary omitted, failed, ignored or filtered required tests")
    measured(sorted(name for name, status in lines) == names and all(status == "ok" for _, status in lines),
            "executed tests differ from --list inventory")
    return {"tests": names, "passed": len(names)}


def process(directory, report, name, command, *, build=False, outcomes=True):
    location = directory / "processes" / name
    invocation = load(location / "invocation.json")
    result = load(location / "result.json")
    require(invocation.get("unset_environment") == list(TEST_FILTERS), "test-filter environment policy changed")
    require(invocation.get("command") == command and invocation.get("cwd") == report["source_root"],
            "changed invocation schedule: " + name)
    require(type(invocation.get("timeout_seconds")) is int and invocation["timeout_seconds"] > 0,
            "missing child deadline")
    require(set(result) == {"status", "timeout", "error", "seconds"}
            and (type(result["status"]) is int or result["status"] is None)
            and type(result["timeout"]) is bool and isinstance(result["seconds"], (int, float))
            and result["seconds"] >= 0 and (result["error"] is None or isinstance(result["error"], str)),
            "malformed process result: " + name)
    env = invocation.get("environment", {})
    require(not set(TEST_FILTERS).intersection(env), "test-filter environment can narrow coverage")
    for key, value in {"RUSTFLAGS": FLAGS, "TSAN_OPTIONS": TSAN_OPTIONS, "CARGO_INCREMENTAL": "0",
                       "RUSTC_WORKSPACE_WRAPPER": ""}.items():
        require(env.get(key) == value, "changed instrumentation environment: " + key)
    if build:
        for key, value in {"RUSTC": report["compiler"]["path"],
                           "RUSTC_WRAPPER": report["capture_root"] + "/rustc-wrapper",
                           "CARGO_TARGET_DIR": report["capture_root"] + "/build-target",
                           "PHASE4_TSAN_AUDIT": report["capture_root"] + "/compiler-audit",
                           "PHASE4_TSAN_RUSTC": report["compiler"]["path"],
                           "PHASE4_TSAN_RUSTC_SHA256": report["compiler"]["sha256"]}.items():
            require(env.get(key) == value, "changed compiler audit environment: " + key)
    stdout, stderr = (location / "stdout").read_bytes(), (location / "stderr").read_bytes()
    if outcomes:
        measured(result["status"] == 0 and not result["timeout"] and result["error"] is None,
                 name + " did not finish successfully")
        measured(not DIAGNOSTIC.search(stdout + b"\n" + stderr), "sanitizer diagnostic in " + name)
    return stdout, stderr


def audit_images(directory, report, images):
    audits = [load(path) for path in sorted((directory / "compiler-audit").glob("*.json"))]
    require(audits, "missing actual rustc invocation audit")
    std = set()
    linked = {}
    for row in audits:
        require(row.get("compiler") == report["compiler"]["path"]
                and row.get("compiler_sha256") == report["compiler"]["sha256"]
                and row.get("status") == 0, "another compiler or failed compiler invocation")
        args = row["arguments"]
        if "--target" not in args:
            continue  # Host proc macros/build scripts are deliberately uninstrumented.
        require(args[args.index("--target") + 1] == report["target"] and FLAGS in args,
                "target compilation lacks ThreadSanitizer instrumentation")
        require(not any(a.startswith(("-Zsanitizer=", "sanitizer=")) and a != FLAGS for a in args),
                "conflicting sanitizer compiler flags")
        if "--crate-name" in args:
            std.add(args[args.index("--crate-name") + 1])
        for path, checksum in row.get("outputs", {}).items():
            linked.setdefault(path, []).append((row, checksum))
    require({"std", "core"} <= std, "standard library was not rebuilt with ThreadSanitizer")
    for key, image in images.items():
        actual = digest((directory / image["image"]).read_bytes())
        # Cargo promotes a linked deps/<bin>-<hash> to debug/<bin>. Its JSON
        # executable may therefore name that copy; both must have identical bytes.
        candidates = [(row, checksum) for records in linked.values() for row, checksum in records
                      if checksum == actual and "--crate-name" in row["arguments"]
                      and row["arguments"][row["arguments"].index("--crate-name") + 1] == image["name"].replace("-", "_")
                      and any(str(Path(arg) if Path(arg).is_absolute() else Path(row["cwd"]) / arg)
                              == str(Path(report["source_root"]) / image["source"])
                              for arg in row["arguments"] if arg.endswith(".rs"))]
        require(candidates, "executed image has no audited compiler output: " + key)
        require(actual == image["sha256"], "copied executable digest changed")
        require(any(checksum == actual and ("--test" in row["arguments"]) is image["test"]
                    and FLAGS in row["arguments"] and "--target" in row["arguments"]
                    for row, checksum in candidates), "copied image is not the audited instrumented artifact")
        if image["package"] == "tsr":
            feature = 'feature="' + SYSTEM_ALLOCATOR_FEATURE + '"'
            require(all(("--cfg", feature) in zip(row["arguments"], row["arguments"][1:])
                        for row, _ in candidates),
                    "CLI compiler invocation did not select the system allocator")


def allocator_policy():
    # Rust defaults to System. The CLI explicitly selects its System forwarding
    # branch; bind the reviewed selector and forwarding implementation here, and
    # separately verify the feature on Cargo artifacts and actual compiler calls.
    paths = []
    for pattern in ("crates/*/src/**/*.rs", "tools/phase4/tsctests/**/*.rs"):
        paths += [p.relative_to(ROOT).as_posix() for p in ROOT.glob(pattern)
                  if "#[global_allocator]" in p.read_text()]
    require(sorted(paths) == ["crates/tsr/src/main.rs", "crates/tsr_bench/src/main.rs"],
            "review changed global allocator declarations before sanitizer capture")
    allocation = ROOT / "crates/tsr/src/allocation.rs"
    text = allocation.read_text()
    require(digest(allocation.read_bytes()) == COUNTING_ALLOCATOR_SHA256,
            "system allocator implementation needs policy review")
    require("unsafe impl GlobalAlloc for CountingAllocator" in text
            and '#[cfg(feature = "system-allocator")]\nuse std::alloc::System as InnerAllocator;' in text
            and all("InnerAllocator." + name + "(" in text for name in ("alloc", "alloc_zeroed", "dealloc", "realloc"))
            and "CountingAllocator" in (ROOT / "crates/tsr/src/main.rs").read_text(),
            "CLI allocator must forward to the system allocator")
    return {"default": "std::alloc::System", "tsr": "CountingAllocator forwarding to System",
            "tsr_feature": SYSTEM_ALLOCATOR_FEATURE,
            "source_sha256": digest(allocation.read_bytes())}


def backend_coverage(host, runs):
    key = "tsr_fswatch:lib:tsr_fswatch"
    require(key in runs, "missing filesystem watcher tests")
    names = runs[key]["tests"]
    require(any(name.startswith("watcher::roster::") for name in names), "missing native backend roster")
    if host["os"] == "linux":
        require(all("linux::tests::" + name in names for name in (
            "fanotify_backend_selection", "fanotify_cross_watcher_same_filesystem", "fanotify_handle_key_round_trip")),
                "missing native fanotify availability tests")
        backends = ("inotify", "fanotify", "fanotify-no-rename")
    else:
        require(any(name.startswith("macos::tests::") for name in names), "missing native FSEvents tests")
        backends = ("fsevents",)
    return {name: {"available": True, "tested": True, "evidence": key} for name in backends}


def scenario_results(directory):
    # These validators only read files. This is deliberately not corpus.replay,
    # whose build contract describes the ordinary, uninstrumented producer.
    import phase4_corpus as corpus
    document = corpus.read_document()
    output = directory / "scenarios"
    rows = read_events(output / "rows.jsonl")
    require(len(rows) == len(document["scenarios"]), "missing full scenario-harness inventory")
    for scenario, row in zip(document["scenarios"], rows):
        corpus.validate_row(scenario, document["provenance"]["scenario_digests"][scenario["id"]], row)
        text = (output / "baselines" / scenario["id"]).read_bytes()
        require(row["transcript"] == {"sha256": digest(text), "bytes": len(text)}, "scenario transcript changed")
        measured(row["state"] == "completed", "unsupported or failed scenario under ThreadSanitizer")
    corpus.validate_summary(load(output / "summary.json"), document, ["all"], rows)
    return {"scenarios": len(rows), "ids_sha256": digest(canonical([row["id"] for row in rows]))}


def scenario_command(report, image):
    return [report["capture_root"] + "/" + image, "--output", report["capture_root"] + "/scenarios",
            "--scenarios", report["source_root"] + "/data/phase4/scenarios.json.gz", "--jobs", str(JOBS), "all"]


def replay(directory, *, expected_host=None):
    directory = Path(directory)
    report = load(directory / "report.json")
    require(isinstance(report, dict) and type(report.get("version")) is int
            and report["version"] == 1 and report.get("sources") == sources(), "stale sanitizer source closure")
    require(report.get("source_stable") is True, "sources changed during sanitizer capture")
    require(report.get("artifacts") == artifacts(directory), "sanitizer receipt artifacts changed")
    require(report.get("nightly") == nightly(), "sanitizer toolchain is not the pin")
    require(report.get("host") == host_from_target(report["target"]), "host differs from compiler target")
    if expected_host is not None:
        require(report["host"]["os"] == expected_host if isinstance(expected_host, str)
                else report["host"] == expected_host, "sanitizer receipt is for another host")
    require(report.get("allocator") == allocator_policy(), "system allocator policy changed")
    require((directory / "rustc-wrapper").read_text() == wrapper_text(report["python"], report["source_root"]),
            "compiler audit wrapper changed")
    schedule = {
        "rustc-path": ["rustup", "which", "--toolchain", nightly(), "rustc"],
        "rustc-version": ["rustup", "run", nightly(), "rustc", "-Vv"],
        "components": ["rustup", "component", "list", "--toolchain", nightly(), "--installed"],
        "tests-build": cargo_command(report, True), "harness-build": cargo_command(report, False),
    }
    expected_images = {("test:" + target_id(row)): row for row in test_targets()}
    expected_images["bin:phase4_tsctests:bin:phase4_tsctests"] = {
        "package": "phase4_tsctests", "kind": "bin", "name": "phase4_tsctests",
        "source": "tools/phase4/tsctests/src/main.rs"}
    require(set(report["images"]) == set(expected_images), "unexpected or missing executed images")
    for identity in expected_images:
        image = "images/" + identity.replace(":", "-")
        if identity.startswith("test:"):
            name = identity.replace(":", "-")
            executable = report["capture_root"] + "/" + image
            schedule[name + "-list"] = [executable, "--list", "--format=pretty"]
            schedule[name + "-run"] = [executable, "--test-threads=1", "--show-output", "--format=pretty"]
        else:
            schedule["scenarios"] = scenario_command(report, image)
    require({p.name for p in (directory / "processes").iterdir()} == set(schedule), "changed process schedule")
    for name, command in schedule.items():
        process(directory, report, name, command, build=name in ("tests-build", "harness-build"), outcomes=False)
    compiler_path, _ = process(directory, report, "rustc-path", ["rustup", "which", "--toolchain", nightly(), "rustc"], outcomes=False)
    require(compiler_path.decode().strip() == report["compiler"]["path"], "compiler path disagrees with rustup")
    version, _ = process(directory, report, "rustc-version", ["rustup", "run", nightly(), "rustc", "-Vv"], outcomes=False)
    require(f"host: {report['target']}\n" in version.decode() and "-nightly" in version.decode(),
            "compiler version does not describe the pinned nightly host")
    components, _ = process(directory, report, "components", ["rustup", "component", "list", "--toolchain", nightly(), "--installed"], outcomes=False)
    require(any(line.startswith(b"rust-src") for line in components.splitlines()), "rust-src was unavailable")
    images = {}
    for test, name in ((True, "tests-build"), (False, "harness-build")):
        process(directory, report, name, cargo_command(report, test), build=True, outcomes=False)
        for key, row in built_images(directory, report, test).items():
            identity = ("test:" if test else "bin:") + key
            recorded = report["images"].get(identity, {})
            expected = {**row, "test": test, "image": "images/" + identity.replace(":", "-"),
                        "sha256": recorded.get("sha256")}
            require(recorded == expected, "Cargo artifact/image binding changed")
            images[identity] = expected
    require(set(report["images"]) == set(images), "unexpected or missing executed images")
    audit_images(directory, report, images)
    for name, command in schedule.items():
        process(directory, report, name, command, build=name in ("tests-build", "harness-build"))
    runs = {}
    process_names = {"rustc-path", "rustc-version", "components", "tests-build", "harness-build", "scenarios"}
    for identity, row in images.items():
        if not row["test"]:
            continue
        key = target_id(row)
        image = report["capture_root"] + "/" + row["image"]
        name = identity.replace(":", "-")
        listed, _ = process(directory, report, name + "-list", [image, "--list", "--format=pretty"])
        names = validate_inventory(listed)
        declared = required_test_names(row, report["host"])
        observed = Counter(name.rsplit("::", 1)[-1] for name in names)
        require(not (declared - observed), "--list omitted source-declared required tests: " + key)
        require(names or (row["package"] == PACKAGES[0] and row["kind"] == "bin"), "empty required test suite")
        stdout, stderr = process(directory, report, name + "-run", [image, "--test-threads=1", "--show-output", "--format=pretty"])
        measured(not SKIP.search(stdout + b"\n" + stderr), "unavailable test/backend coverage: " + key)
        runs[key] = validate_test_run(stdout, names)
        process_names.update((name + "-list", name + "-run"))
    harness = images["bin:phase4_tsctests:bin:phase4_tsctests"]
    process(directory, report, "scenarios", scenario_command(report, harness["image"]))
    require({p.name for p in (directory / "processes").iterdir()} == process_names, "changed process schedule")
    coverage = scenario_results(directory)
    return {"host": report["host"], "backends": backend_coverage(report["host"], runs),
            "suites": runs, **coverage}


def verify_witnesses(directory, *, expected_host=None):
    """Invalid/stale/unavailable evidence raises; authenticated failures are false."""
    directory = Path(directory)
    identity = digest((directory / "report.json").read_bytes())
    try:
        details = replay(directory, expected_host=expected_host)
    except (AttributeError, IndexError, KeyError, TypeError, UnicodeError) as error:
        raise ValueError("malformed sanitizer receipt: " + str(error)) from error
    except MeasuredFailure as error:
        report = load(directory / "report.json")
        native = ("fsevents",) if report["host"]["os"] == "macos" else ("inotify", "fanotify", "fanotify-no-rename")
        details = {"error": str(error), "host": report["host"],
                   "backends": {name: {"available": None, "tested": False} for name in native}}
        fswatch = directory / "processes/test-tsr_fswatch-lib-tsr_fswatch-run"
        raw = b"\n".join(path.read_bytes() for path in (fswatch / "stdout", fswatch / "stderr") if path.is_file())
        if b"skip: fanotify not available" in raw:
            for name in ("fanotify", "fanotify-no-rename"):
                if name in details["backends"]:
                    details["backends"][name]["available"] = False
        return {"metrics": {"thread_sanitizer": False}, "identities": {"thread_sanitizer": identity}, "details": details}
    return {"metrics": {"thread_sanitizer": True}, "identities": {"thread_sanitizer": identity}, "details": details}


def run(directory, *, timeout=7200):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    before = sources()
    report = {"version": 1, "nightly": nightly(), "sources": before, "source_root": str(ROOT),
              "capture_root": str(directory), "python": sys.executable, "images": {}, "error": None}
    env = instrumentation_environment(os.environ)
    try:
        report["allocator"] = allocator_policy()
        path = invoke(directory, "rustc-path", ["rustup", "which", "--toolchain", nightly(), "rustc"], env, 60).decode().strip()
        require(Path(path).is_absolute() and Path(path).resolve() == Path(path), "rustup compiler must be an absolute canonical path")
        report["compiler"] = {"path": path, "sha256": digest(Path(path).read_bytes())}
        version = invoke(directory, "rustc-version", ["rustup", "run", nightly(), "rustc", "-Vv"], env, 60).decode()
        report["target"] = next(line[6:] for line in version.splitlines() if line.startswith("host: "))
        report["host"] = host_from_target(report["target"])
        require(report["host"]["os"] == {"Darwin": "macos", "Linux": "linux"}.get(platform.system()), "compiler is not native to this host")
        components = invoke(directory, "components", ["rustup", "component", "list", "--toolchain", nightly(), "--installed"], env, 60)
        require(any(line.startswith(b"rust-src") for line in components.splitlines()), "pinned nightly rust-src is not installed")
        (directory / "compiler-audit").mkdir()
        (directory / "images").mkdir()
        wrapper = directory / "rustc-wrapper"
        wrapper.write_text(wrapper_text(sys.executable, str(ROOT)))
        wrapper.chmod(0o755)
        env.update(RUSTC=path, RUSTC_WRAPPER=str(wrapper), CARGO_TARGET_DIR=str(directory / "build-target"),
                   PHASE4_TSAN_AUDIT=str(directory / "compiler-audit"), PHASE4_TSAN_RUSTC=path,
                   PHASE4_TSAN_RUSTC_SHA256=report["compiler"]["sha256"])
        for test, name in ((True, "tests-build"), (False, "harness-build")):
            invoke(directory, name, cargo_command(report, test), env, timeout)
            for key, row in built_images(directory, report, test).items():
                identity = ("test:" if test else "bin:") + key
                image = "images/" + identity.replace(":", "-")
                shutil.copy2(row["built_path"], directory / image)
                report["images"][identity] = {**row, "test": test, "image": image,
                                              "sha256": digest((directory / image).read_bytes())}
        audit_images(directory, report, report["images"])
        # Do not stop at a failing suite: retain independent failures and coverage.
        failures = []
        for identity, row in report["images"].items():
            if not row["test"]:
                continue
            image = str(directory / row["image"])
            name = identity.replace(":", "-")
            for suffix, args in (("-list", ["--list", "--format=pretty"]),
                                 ("-run", ["--test-threads=1", "--show-output", "--format=pretty"])):
                try:
                    invoke(directory, name + suffix, [image, *args], env, timeout)
                except ValueError as error:
                    failures.append(str(error))
        try:
            invoke(directory, "scenarios", scenario_command(report, report["images"]["bin:phase4_tsctests:bin:phase4_tsctests"]["image"]), env, timeout)
        except ValueError as error:
            failures.append(str(error))
        if failures:
            report["error"] = "; ".join(failures)
    except (OSError, ValueError, KeyError, StopIteration) as error:
        report["error"] = str(error)
    finally:
        report["source_stable"] = sources() == before
        report["artifacts"] = artifacts(directory)
        write(directory / "report.json", report)
    try:
        return verify_witnesses(directory)
    except (OSError, ValueError, KeyError, TypeError) as error:
        return {"metrics": {"thread_sanitizer": False},
                "identities": {"thread_sanitizer": digest((directory / "report.json").read_bytes())},
                "details": {"unavailable": str(error), "capture_error": report["error"]}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("run", "replay"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=7200)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    try:
        result = run(args.output, timeout=args.timeout) if args.command == "run" else verify_witnesses(args.output)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"invalid or unavailable sanitizer evidence: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0 if result["metrics"]["thread_sanitizer"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
