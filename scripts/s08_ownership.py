#!/usr/bin/env python3
"""Compose E3 with native-observed, independently owned checker merges.

The existing P2 executable supplies the production operations; no second Rust
harness or checker implementation is introduced. Its frozen native fixture is
projected to one independent primitive program and all four merge schedules.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

import s04_ownership as ownership
from s04_common import strict_json_loads
from s04_runtime import cache_home, load_toolchains
from s08_oracle import ROOT, canonical, digest
import s08_p2 as p2

ARCHIVE = "data/s08/p2/review-fixes.tar.xz"
RECORD = "data/s08/p2/review-fixes.json"
PROGRAM = "primitive-and-literal-aliases"
MODES = ("debug", "release", "miri", "address_sanitizer")


def fixture(root):
    root = Path(root)
    record = strict_json_loads((root / RECORD).read_bytes())
    if (record["archive"]["path"] != ARCHIVE
            or digest((root / ARCHIVE).read_bytes()) != record["archive"]["sha256"]
            or record["upstream_pin"] != strict_json_loads((root / "data/upstream.json").read_bytes())["pin"]):
        raise ValueError("checker merge native archive or pin differs")
    with tarfile.open(root / ARCHIVE) as archive:
        def read(name):
            return archive.extractfile("native/" + name).read()
        report = strict_json_loads(read("report.json"))
        raw, observations = read("requests.json"), read("observations.json")
        if (digest(raw) != report["request_sha256"]
                or digest(observations) != report["observation_sha256"]
                or report["pin"] != record["upstream_pin"]):
            raise ValueError("checker merge native observation identity differs")
        for name, sha in report["sources"].items():
            if digest(read("source-snapshot/" + name)) != sha:
                raise ValueError("checker merge native source snapshot differs: " + name)
    spec, native = strict_json_loads(raw), strict_json_loads(observations)
    p2.validate(spec, native)
    # Each program and each merge schedule constructs its own checker. The
    # projection drops independent programs, never actions within a merge.
    spec["programs"] = [p for p in spec["programs"] if p["id"] == PROGRAM]
    native["programs"] = [p for p in native["programs"] if p["id"] == PROGRAM]
    if len(spec["programs"]) != 1 or len(native["programs"]) != 1:
        raise ValueError("checker merge control program missing")
    native["request_sha256"] = digest(canonical(spec) + b"\n")
    p2.validate(spec, native)
    return spec, native


def validate(spec, native, actual):
    p2.validate(spec, actual, require_rust_ownership=True)
    for section, key in (("programs", "id"), ("merges", "mode")):
        for expected, observed in zip(native[section], actual[section], strict=True):
            if not p2.same_json_value(expected, observed):
                raise ValueError("checker merge native mismatch: " + section + "/" + expected[key])


def measure(root, invoke, prefix, options, env, spec, native, directory, mode):
    if mode not in MODES:
        raise ValueError("unknown checker merge instrumentation mode")
    output = directory / (mode + ".json")
    # Exclusive creation prevents a previous output from satisfying a failed or
    # silent child. The request and output paths belong to this invocation only.
    if output.exists():
        raise ValueError("checker merge output already exists")
    args = [*prefix, "run", "--package", "ts_compiler", "--example", "p2_checker",
            "--locked", *options, "--", str(directory / "requests.json"), str(output)]
    try:
        stdout = invoke(root, args, env)
        (directory / (mode + ".stdout")).write_bytes(stdout)
        actual = strict_json_loads(output.read_bytes())
        # Preserve raw observations in the evidence runner's authenticated stderr
        # as well as the local replay directory, including failed comparisons.
        print("S08 checker merges " + mode + ": " + canonical(actual).decode(), file=sys.stderr)
        validate(spec, native, actual)
    except (OSError, RuntimeError, ValueError, KeyError, TypeError) as error:
        print(f"S08 checker merges {mode} failed: {error}", file=sys.stderr)
        return False
    return True


def publish_metrics(report, modes):
    if set(modes) != set(MODES) or any(type(value) is not bool for value in modes.values()):
        raise ValueError("checker merges require all four instrumentation outcomes")
    for mode, passed in modes.items():
        report["metrics"]["independent_checker_merges_" + mode] = passed
    report["metrics"]["independent_checker_merges"] = all(modes.values())
    # Extend the instrumentation aggregate only with this executed scenario;
    # unrelated future E3 criteria remain absent.
    for mode in ("miri", "address_sanitizer"):
        if mode in report["metrics"]:
            report["metrics"][mode] = report["metrics"][mode] and modes[mode]


def merges(root, invoke=ownership.invoke):
    root = Path(root)
    spec, native = fixture(root)
    parent = root / "target/s08"
    parent.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="e3-merges-", dir=parent))
    (directory / "requests.json").write_bytes(canonical(spec) + b"\n")
    (directory / "native.json").write_bytes(canonical(native) + b"\n")
    print(f"S08 checker merge replay: {directory}; request sha256={native['request_sha256']}", file=sys.stderr)
    base = {**os.environ, "CARGO_TERM_COLOR": "never"}
    modes = {}
    for mode, options in (("debug", []), ("release", ["--release"])):
        modes[mode] = measure(root, invoke, ["cargo"], options, base, spec, native, directory, mode)
    instrument = ownership.instrumentation_environment(base, root)
    nightly = load_toolchains(root)["nightly"]
    host = next(line.removeprefix("host: ") for line in
                invoke(root, ["rustc", "-Vv"], base).decode().splitlines() if line.startswith("host: "))
    miri = {**instrument, "MIRI_SYSROOT": str(cache_home(root, instrument) / "miri" / nightly / host),
            "CARGO_TARGET_DIR": str(root / "target/s04-miri"), "RUSTFLAGS": "",
            # The example reads requests and writes observations. Host I/O needs
            # isolation disabled; strict provenance and memory checks stay on.
            "MIRIFLAGS": "-Zmiri-strict-provenance -Zmiri-disable-isolation"}
    invoke(root, ["cargo", f"+{nightly}", "miri", "setup", "--target", host], miri)
    modes["miri"] = measure(root, invoke, ["cargo", f"+{nightly}", "miri"], ["--target", host],
                            miri, spec, native, directory, "miri")
    asan = {**instrument, "CARGO_TARGET_DIR": str(root / "target/s04-asan"),
            "RUSTFLAGS": "-Zsanitizer=address",
            "ASAN_OPTIONS": "detect_leaks=0" if "apple" in host else "detect_leaks=1"}
    modes["address_sanitizer"] = measure(root, invoke, ["cargo", f"+{nightly}"],
        ["-Zbuild-std", "--target", host], asan, spec, native, directory, "address_sanitizer")
    (directory / "outcomes.json").write_bytes(canonical(modes) + b"\n")
    return modes


def run(root):
    report = ownership.run(root)
    # An early native failure must retain its existing diagnostic; don't claim
    # new coverage when the underlying ownership harness did not finish.
    if "miri" not in report["metrics"] or "address_sanitizer" not in report["metrics"]:
        return report
    publish_metrics(report, merges(root))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--merges-only", action="store_true", help="development check; does not emit a complete E3 record")
    args = parser.parse_args()
    try:
        if args.merges_only:
            outcomes = merges(ROOT)
            print(json.dumps(outcomes, sort_keys=True))
            return 0 if all(outcomes.values()) else 1
        print(json.dumps(run(ROOT), sort_keys=True))
        return 0
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"S08 ownership producer failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
