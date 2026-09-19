"""Run the same arena scenarios in release, debug, Miri and AddressSanitizer.

The seven S04 counter scenarios, S06 AST tests, S07 binder/program groups and
S09 generation/pool/registry suites become separately scoped evidence. Instrumented
runs use a separately pinned nightly, native target and reusable user caches.
No synthetic success or skipped-test count can satisfy an instrumentation gate.
"""

import json
import os
from pathlib import Path
import re
import sys

from s04_common import command, strict_json_loads
from s04_runtime import cache_home, load_toolchains
import s06_ownership
import s07_ownership
import s09_ownership


SCENARIOS = (
    "id_exhaustion", "wrong_owner_rejected", "stale_and_recycled_ids_rejected",
    "concurrent_lazy_storage", "mapper_bundle_disposal", "owners_return_to_baseline",
    "allocations_return_to_baseline",
)


def invoke(root, args, env=None):
    return command(args, cwd=root, env=env)


def validate_test_output(output):
    text = output.decode()
    sys.stderr.write(text)
    summary = re.findall(r"test result: ok\. (\d+) passed; 0 failed; (\d+) ignored;", text)
    if not summary or any(int(passed) < len(SCENARIOS) or int(ignored) for passed, ignored in summary):
        raise ValueError("ownership suite must run every required scenario without ignores")
    for scenario in SCENARIOS:
        if not re.search(rf"^test [\w:]*e3_{scenario} \.\.\. ok$", text, re.MULTILINE):
            raise ValueError(f"ownership suite did not pass {scenario}")
    return True


def validate_measurements(report):
    if not isinstance(report, dict) or set(report) != {"metrics", "tests"}:
        raise ValueError("invalid ownership report envelope")
    tests = report["tests"]
    if (not isinstance(tests, list)
            or any(not isinstance(test, dict) or not isinstance(test.get("id"), str) for test in tests)
            or sorted(test["id"] for test in tests) != sorted(SCENARIOS)):
        raise ValueError("ownership report must contain exactly the frozen scenarios")
    for test in tests:
        if test.get("result") == "pass" and set(test) == {"id", "result"}:
            continue
        if (test.get("result") != "fail" or set(test) != {"id", "result", "error"}
                or not isinstance(test["error"], str)):
            raise ValueError("ownership scenario must report pass or fail with its panic diagnostic")
        print(f"ownership scenario {test['id']} failed: {test['error']}", file=sys.stderr)
    outcomes = {test["id"]: test["result"] for test in tests}
    failed = "fail" in outcomes.values()
    metrics = report["metrics"]
    expected_metrics = set(SCENARIOS[:5])
    if not failed:
        expected_metrics |= {"live_owner_delta", "live_allocation_delta"}
    if not isinstance(metrics, dict) or set(metrics) != expected_metrics:
        raise ValueError("native example may emit only the measured S04 metrics")
    for scenario in SCENARIOS[:5]:
        if metrics.get(scenario) is not (outcomes[scenario] == "pass"):
            raise ValueError(f"ownership measurement disagrees with scenario {scenario}")
    for metric in ("live_owner_delta", "live_allocation_delta"):
        if metric not in metrics and failed:
            continue
        value = metrics.get(metric)
        if type(value) is not int or value < 0:
            raise ValueError(f"missing or invalid measured counter {metric}")
    return report


def instrumentation_environment(base, root):
    """Keep caller flags, wrappers and alternate sysroots out of measured runs."""
    env = base.copy()
    compiler_overrides = {
        "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTDOC",
        "RUSTFLAGS", "RUSTDOCFLAGS", "RUSTUP_TOOLCHAIN", "MIRIFLAGS",
        "CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS",
        "CARGO_BUILD_RUSTC", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_RUSTDOC", "CARGO_BUILD_RUSTDOCFLAGS",
        "ASAN_OPTIONS", "LSAN_OPTIONS", "UBSAN_OPTIONS",
    }
    for key in list(env):
        if key in compiler_overrides or key.startswith("MIRI_") or re.fullmatch(
            r"CARGO_TARGET_.+_(RUSTFLAGS|RUNNER)", key
        ):
            env.pop(key)
    # Cargo registry mirrors/offline configuration and installed Rustup toolchains
    # belong to the caller. An explicit override supports isolated installations.
    if env.get("S04_RUSTUP_HOME"):
        env["RUSTUP_HOME"] = str(Path(env["S04_RUSTUP_HOME"]).expanduser().resolve())
    return env


def run(root):
    root = Path(root)
    manifest = json.loads((root / "data/s04/e3-cases.json").read_text())
    if manifest != sorted(SCENARIOS):
        raise ValueError("ownership case manifest drift")
    ast_cases = s06_ownership.load_cases(root)
    program_cases = s07_ownership.load_cases(root)
    checker_cases = s09_ownership.load_cases(root)
    base = os.environ.copy()
    base["CARGO_TERM_COLOR"] = "never"
    report = validate_measurements(strict_json_loads(invoke(root, [
        "cargo", "run", "--quiet", "--package", "ts_arena", "--example", "e3",
        "--features", "harness", "--release", "--locked",
    ], base)))
    # Publish named native failures even when later tools would fail too. There
    # is no valid instrumentation claim until every required native scenario ran.
    if any(test["result"] == "fail" for test in report["tests"]):
        report["tests"] = {test["id"]: test["result"] for test in report["tests"]}
        return report
    tests = ["test", "--package", "ts_arena", "--lib", "--locked"]
    tail = ["--", "--test-threads=1"]
    arena_modes = {
        "debug": validate_test_output(invoke(root, ["cargo", *tests, *tail], base)),
        "release": validate_test_output(invoke(root, ["cargo", *tests, "--release", *tail], base)),
    }
    ast_modes = {}
    program_modes = {}
    checker_modes = {}
    for mode, options in (("debug", []), ("release", ["--release"])):
        ast_modes[mode] = s06_ownership.measure(root, invoke, ["cargo"], options, base, ast_cases, mode)
        report["metrics"][f"ast_runtime_{mode}"] = ast_modes[mode]
        program_modes[mode] = s07_ownership.measure(root, invoke, ["cargo"], options, base, program_cases, mode)
        checker_modes[mode] = s09_ownership.measure(root, invoke, ["cargo"], options, base, checker_cases, mode)
    report["metrics"]["ast_runtime_tests"] = len(ast_cases)
    sys.stderr.buffer.write(invoke(root, ["cargo", "test", "--package", "ts_arena", "--doc", "--locked"], base))
    nightly = load_toolchains(root)["nightly"]
    native = invoke(root, ["rustc", "-Vv"], base).decode()
    print(native, file=sys.stderr)
    host = next(line.removeprefix("host: ") for line in native.splitlines() if line.startswith("host: "))
    instrument = instrumentation_environment(base, root)
    print(invoke(root, ["rustup", "run", nightly, "rustc", "-Vv"], instrument).decode(), file=sys.stderr)
    components = invoke(root, ["rustup", "component", "list", "--toolchain", nightly, "--installed"], instrument).decode()
    if not any(line.startswith("miri-") for line in components.splitlines()) or not any(
            line.startswith("rust-src") for line in components.splitlines()):
        raise RuntimeError(f"{nightly} requires installed miri and rust-src components; "
                           f"run rustup component add --toolchain {nightly} miri rust-src")
    # The production std::sync locks support Miri's strict provenance checks.
    miri = {**instrument, "MIRI_SYSROOT": str(cache_home(root, instrument) / "miri" / nightly / host),
            "CARGO_TARGET_DIR": str(root / "target/s04-miri"),
            "MIRIFLAGS": "-Zmiri-strict-provenance", "RUSTFLAGS": ""}
    try:
        invoke(root, ["cargo", f"+{nightly}", "miri", "setup", "--target", host], miri)
        validate_test_output(invoke(root, ["cargo", f"+{nightly}", "miri", *tests, "--target", host, *tail], miri))
    except (RuntimeError, ValueError) as error:
        print(f"Miri ownership suite failed: {error}", file=sys.stderr)
        report["metrics"]["miri"] = False
    else:
        report["metrics"]["miri"] = True
    arena_modes["miri"] = report["metrics"]["miri"]
    ast_modes["miri"] = s06_ownership.measure(
        root, invoke, ["cargo", f"+{nightly}", "miri"], ["--target", host],
        miri, ast_cases, "miri")
    report["metrics"]["ast_runtime_miri"] = ast_modes["miri"]
    report["metrics"]["miri"] = report["metrics"]["miri"] and ast_modes["miri"]
    program_modes["miri"] = s07_ownership.measure(root, invoke, ["cargo", f"+{nightly}", "miri"], ["--target", host], miri, program_cases, "miri")
    report["metrics"]["miri"] = report["metrics"]["miri"] and all(program_modes["miri"].values())
    checker_modes["miri"] = s09_ownership.measure(
        root, invoke, ["cargo", f"+{nightly}", "miri"], ["--target", host],
        miri, checker_cases, "miri")
    report["metrics"]["miri"] = report["metrics"]["miri"] and all(checker_modes["miri"].values())
    asan = {**instrument, "CARGO_TARGET_DIR": str(root / "target/s04-asan"),
            "RUSTFLAGS": "-Zsanitizer=address",
            # macOS ASan does not support LeakSanitizer. Miri and measured
            # ownership counters still require all roots/allocations to drop.
            "ASAN_OPTIONS": "detect_leaks=0" if "apple" in host else "detect_leaks=1"}
    try:
        validate_test_output(invoke(root, ["cargo", f"+{nightly}", *tests, "-Zbuild-std", "--target", host, *tail], asan))
    except (RuntimeError, ValueError) as error:
        print(f"AddressSanitizer ownership suite failed: {error}", file=sys.stderr)
        report["metrics"]["address_sanitizer"] = False
    else:
        report["metrics"]["address_sanitizer"] = True
    arena_modes["address_sanitizer"] = report["metrics"]["address_sanitizer"]
    ast_modes["address_sanitizer"] = s06_ownership.measure(
        root, invoke, ["cargo", f"+{nightly}"], ["-Zbuild-std", "--target", host],
        asan, ast_cases, "address_sanitizer")
    report["metrics"]["ast_runtime_address_sanitizer"] = ast_modes["address_sanitizer"]
    report["metrics"]["address_sanitizer"] = report["metrics"]["address_sanitizer"] and ast_modes["address_sanitizer"]
    report["metrics"]["ast_runtime"] = all(ast_modes.values())
    program_modes["address_sanitizer"] = s07_ownership.measure(root, invoke, ["cargo", f"+{nightly}"], ["-Zbuild-std", "--target", host], asan, program_cases, "address_sanitizer")
    report["metrics"]["address_sanitizer"] = report["metrics"]["address_sanitizer"] and all(program_modes["address_sanitizer"].values())
    checker_modes["address_sanitizer"] = s09_ownership.measure(
        root, invoke, ["cargo", f"+{nightly}"], ["-Zbuild-std", "--target", host],
        asan, checker_cases, "address_sanitizer")
    report["metrics"]["address_sanitizer"] = report["metrics"]["address_sanitizer"] and all(checker_modes["address_sanitizer"].values())
    s07_ownership.publish_metrics(report, program_modes, program_cases)
    s09_ownership.publish_metrics(report, checker_modes, arena_modes, checker_cases)
    # The native example's row inventory has already been checked for duplicate
    # IDs. The evidence runner requires its public id -> outcome map schema.
    report["tests"] = {test["id"]: test["result"] for test in report["tests"]}
    return report
