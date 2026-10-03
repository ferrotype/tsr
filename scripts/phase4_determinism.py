#!/usr/bin/env python3
"""Phase 4 X7: five complete runs of one current command-line harness build.

`run --output DIR [--capture DIR] [--jobs N]` builds once, or authenticates a
complete corpus capture as its build seed, then starts five separate harness
processes. All run images, build proof, invocations, raw captures and failures
are retained. `replay --output DIR` validates that evidence and recomputes the
metric; the producer uses `verify_witnesses` and never executes the harness.
A current, well-formed measured difference, refusal or failed full execution
is false. Missing, malformed, partial or stale evidence raises instead.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tomllib
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase4_corpus as corpus  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

DEFAULT_OUTPUT = ROOT / "target/phase4/determinism"
RUN_COUNT = 5
BUILD_COMMAND = ["cargo", "build", "--locked", "-p", corpus.PACKAGE, "--bin",
                 corpus.PACKAGE, "--message-format=json"]
BUILD_FILES = ("build.json", "build.stdout", "build.stderr", "executable")
HELPERS = ("scripts/phase4_determinism.py", "scripts/phase4_corpus.py",
           "scripts/phase4_scenarios.py", "scripts/s04.py", "scripts/s04_runtime.py",
           "scripts/s04_common.py", "scripts/s08_oracle.py")


def write_json(path, value):
    path.write_bytes(canonical(value) + b"\n")


def read_json(path):
    return strict_json_loads(path.read_bytes())


def sources():
    """Bind the executable closure and the code deciding this witness."""
    return dict(sorted({**corpus.sources(), **{name: digest((ROOT / name).read_bytes())
                                               for name in HELPERS}}.items()))


def toolchain():
    result = {"host": platform.platform(), "system": platform.system(), "machine": platform.machine()}
    for name, command in (("cargo", ["cargo", "--version"]),
                          ("rustc", ["rustc", "--version", "--verbose"])):
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, check=True)
        result[name] = {"command": command, "exit_status": completed.returncode,
                        "stdout": completed.stdout.decode("utf-8"), "stderr": completed.stderr.decode("utf-8")}
    return result


def validate_toolchain(record):
    """Replay recorded version output against the pin without starting children."""
    pin = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if (not isinstance(record, dict)
            or any(not isinstance(record.get(key), str) or not record[key]
                   for key in ("host", "system", "machine"))):
        raise ValueError("malformed recorded determinism toolchain host")
    for name, command in (("cargo", ["cargo", "--version"]),
                          ("rustc", ["rustc", "--version", "--verbose"])):
        observed = record.get(name)
        if (not isinstance(observed, dict) or observed.get("command") != command
                or type(observed.get("exit_status")) is not int or observed["exit_status"] != 0
                or not isinstance(observed.get("stderr"), str)
                or not isinstance(observed.get("stdout"), str)
                or not observed["stdout"].startswith(f"{name} {pin} ")):
            raise ValueError("stale or malformed recorded determinism toolchain version")
    fields = dict(line.split(": ", 1) for line in record["rustc"]["stdout"].splitlines() if ": " in line)
    host = fields.get("host", "")
    systems = {"Darwin": "apple-darwin", "Linux": "linux"}
    machine = {"arm64": "aarch64", "amd64": "x86_64"}.get(record["machine"].lower(), record["machine"].lower())
    if (fields.get("release") != pin or record["system"] not in systems
            or systems[record["system"]] not in host or not host.startswith(machine + "-")):
        raise ValueError("recorded determinism compiler does not match its pin or host")


def environment():
    """The exact child environment, with no unrecorded inherited switches."""
    names = {"PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "TMP", "TEMP", "SYSTEMROOT",
             "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"}
    prefixes = ("CARGO_", "RUST", "TSR_", "TS_TEST_", "RAYON_", "LLVM_")
    selected = {key: value for key, value in os.environ.items()
                if key in names or key.startswith(prefixes)}
    selected.update(LANG="C", LC_ALL="C", TZ="UTC")
    return dict(sorted(selected.items()))


def artifact_hashes(directory):
    """Nothing outside this directory, including symlinked runs, is evidence."""
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("symlink in determinism evidence: " + str(path))
        if path.is_file():
            name = path.relative_to(directory).as_posix()
            if name not in ("report.json", "replayed.json"):
                result[name] = digest(path.read_bytes())
    return result


def authenticate_build(directory, current_sources=None):
    """Check the corpus build binding and Cargo's concrete binary artifact."""
    record = read_json(directory / "build.json")
    if (not isinstance(record, dict) or record.get("version") != 1
            or record.get("command") != BUILD_COMMAND
            or record.get("sources") != (corpus.sources() if current_sources is None else current_sources)):
        raise ValueError("determinism build is synthetic, malformed or stale")
    if not isinstance(record.get("environment"), dict) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in record["environment"].items()):
        raise ValueError("malformed build environment")
    image = (directory / "executable").read_bytes()
    # A write_capture fixture is useful for unit tests, but is never execution
    # evidence. Rebinding its build.json must not turn its marker into a binary.
    if image == corpus.SYNTHETIC or image[:4] not in (
            b"\x7fELF", b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xce", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
        raise ValueError("determinism image is not a native harness executable")
    if digest(image) != record.get("binary_sha256"):
        raise ValueError("determinism executable differs from its build")
    if not isinstance(record.get("binary"), str) or not Path(record["binary"]).is_absolute():
        raise ValueError("malformed build binary path")
    events = [strict_json_loads(line) for line in (directory / "build.stdout").read_bytes().splitlines()]
    if (not events or any(not isinstance(event, dict) for event in events)
            or events[-1] != {"reason": "build-finished", "success": True}):
        raise ValueError("missing successful Cargo build proof")
    artifacts = [event for event in events if event.get("reason") == "compiler-artifact"
                 and event.get("target", {}).get("name") == corpus.PACKAGE
                 and event.get("target", {}).get("kind") == ["bin"]]
    if len(artifacts) != 1:
        raise ValueError("missing or ambiguous harness build artifact")
    event = artifacts[0]
    manifest = event.get("manifest_path")
    suffix = "/tools/phase4/tsctests/Cargo.toml"
    if not isinstance(manifest, str) or not manifest.endswith(suffix):
        raise ValueError("Cargo proof names another harness manifest")
    repository = Path(manifest[:-len(suffix)])
    if not repository.is_absolute() or ".." in repository.parts:
        raise ValueError("malformed captured build repository")
    harness = repository / "tools/phase4/tsctests"
    if (event.get("executable") != record["binary"]
            or event.get("manifest_path") != str(harness / "Cargo.toml")
            or event["target"].get("src_path") != str(harness / "src/main.rs")
            or event.get("filenames") != [record["binary"]]
            or not isinstance(event.get("profile"), dict)
            or event["profile"].get("test") is not False
            or not isinstance(event.get("features"), list)):
        raise ValueError("Cargo proof does not identify the harness build")
    # Read both logs even when stderr is empty; the report binds both later.
    (directory / "build.stderr").read_bytes()
    return {"repository": str(repository), "binary_sha256": record["binary_sha256"], "profile": event["profile"],
            "features": event["features"], "command": record["command"],
            "environment": record["environment"]}


def invocation_command(directory, jobs, selectors, repository=ROOT):
    return [str(directory / "executable"), "--output", str(directory), "--scenarios",
            str(repository / corpus.INVENTORY.relative_to(ROOT)), "--jobs", str(jobs), *selectors]


def copy_build(source, destination):
    for name in BUILD_FILES:
        shutil.copy2(source / name, destination / name)


def run(output, *, capture=None, jobs=None, timeout=600):
    """Retain five independent executions, building only once when no seed is given."""
    jobs = jobs if jobs is not None else max(2, min(8, (os.cpu_count() or 2) - 2))
    if type(jobs) is not int or jobs < 1:
        raise ValueError("--jobs must be positive")
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("--timeout must be positive and finite")
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    before, versions, child_env = sources(), toolchain(), environment()
    validate_toolchain(versions)
    write_json(output / "toolchain.json", versions)
    document = corpus.read_document()
    ids = corpus.selection(document, ["all"])
    build_dir = output / "build"
    build_dir.mkdir()
    if capture is None:
        record = corpus.build(build_dir)
        write_json(build_dir / "build.json", record)
        shutil.copy2(record["binary"], build_dir / "executable")
    else:
        seed = corpus.load_capture(capture)
        replayed = corpus.replay(capture, write=False, capture=seed)
        if seed.metadata["partial"] or not replayed["source_stable"]:
            raise ValueError("determinism build seed must be a current full corpus capture")
        copy_build(seed.directory, build_dir)
    proof = authenticate_build(build_dir)
    report = {"version": 1, "sources": before, "toolchain": versions,
              "roots": {"repository": str(ROOT), "output": str(output)}, "timeout_seconds": timeout,
              "inventory": corpus.inventory_binding(document), "build": proof,
              "environment": child_env, "jobs": jobs, "runs": []}
    for number in range(1, RUN_COUNT + 1):
        relative = f"runs/{number}"
        directory = output / relative
        directory.mkdir(parents=True)
        copy_build(build_dir, directory)
        invocation = {"version": 1, "id": uuid.uuid4().hex,
                      "command": invocation_command(directory, jobs, ["all"]),
                      "cwd": str(ROOT), "environment": child_env, "jobs": jobs,
                      "exit_status": None, "error": None, "timeout_seconds": timeout, "timed_out": False}
        write_json(directory / "invocation.json", invocation)
        with (directory / "stdout").open("wb") as stdout, (directory / "stderr").open("wb") as stderr:
            try:
                completed = subprocess.run(invocation["command"], cwd=ROOT, env=child_env,
                                           stdout=stdout, stderr=stderr, check=False, timeout=timeout)
                invocation["exit_status"] = completed.returncode
            except subprocess.TimeoutExpired:
                invocation.update(error="harness exceeded its timeout", timed_out=True)
            except OSError as error:
                invocation["error"] = str(error)
        write_json(directory / "invocation.json", invocation)
        entry = {"path": relative, "capture_sha256": None}
        if invocation["exit_status"] == 0:
            # A zero-status malformed output is invalid evidence, not a measured
            # mismatch. Keep it, finish the remaining processes, then replay.
            try:
                corpus.bind(directory, document, ["all"], ids, 0)
                entry["capture_sha256"] = digest((directory / "capture.json").read_bytes())
            except (OSError, ValueError, KeyError, TypeError) as error:
                entry["error"] = str(error)
        report["runs"].append(entry)
        report["artifacts"] = artifact_hashes(output)
        write_json(output / "report.json", report)
    if sources() != before or toolchain() != versions:
        raise ValueError("determinism sources or toolchain changed during execution")
    result = verify_witnesses(output)
    write_json(output / "replayed.json", result)
    return result


def _verify_witnesses(directory):
    """Replay raw rows/bytes; invalid evidence raises so the producer withholds."""
    directory = Path(directory).resolve()
    report_bytes = (directory / "report.json").read_bytes()
    report = strict_json_loads(report_bytes)
    if not isinstance(report, dict) or report.get("version") != 1:
        raise ValueError("not a determinism witness")
    if report.get("sources") != sources():
        raise ValueError("stale determinism sources")
    versions = read_json(directory / "toolchain.json")
    validate_toolchain(versions)
    if report.get("toolchain") != versions:
        raise ValueError("stale or changed determinism toolchain binding")
    roots = report.get("roots")
    if (not isinstance(roots, dict) or set(roots) != {"repository", "output"}
            or any(not isinstance(value, str) or not Path(value).is_absolute() or ".." in Path(value).parts
                   for value in roots.values())):
        raise ValueError("malformed recorded determinism roots")
    repository, recorded_output = Path(roots["repository"]), Path(roots["output"])
    timeout = report.get("timeout_seconds")
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("malformed determinism timeout")
    document = corpus.read_document()
    if report.get("inventory") != corpus.inventory_binding(document):
        raise ValueError("stale determinism inventory")
    if report.get("artifacts") != artifact_hashes(directory):
        raise ValueError("missing, extra or changed determinism artifact")
    build_dir = directory / "build"
    proof = authenticate_build(build_dir)
    if report.get("build") != proof:
        raise ValueError("determinism build proof changed")
    jobs = report.get("jobs")
    env = report.get("environment")
    if (type(jobs) is not int or jobs < 1 or not isinstance(env, dict)
            or any(not isinstance(key, str) or not isinstance(value, str) for key, value in env.items())):
        raise ValueError("malformed determinism execution environment or jobs")
    runs = report.get("runs")
    if (not isinstance(runs, list) or len(runs) != RUN_COUNT
            or any(not isinstance(row, dict) for row in runs)
            or [row.get("path") for row in runs] != [f"runs/{i}" for i in range(1, RUN_COUNT + 1)]):
        raise ValueError("determinism requires exactly five distinct, ordered run paths")
    captures, invocations, details = [], set(), []
    valid = True
    for entry in runs:
        run_dir = directory / entry["path"]
        recorded_run = recorded_output / entry["path"]
        for name in BUILD_FILES:
            if (run_dir / name).read_bytes() != (build_dir / name).read_bytes():
                raise ValueError("determinism runs do not share one harness build")
        invocation = read_json(run_dir / "invocation.json")
        identity = invocation.get("id") if isinstance(invocation, dict) else None
        if (not isinstance(identity, str) or len(identity) != 32
                or any(letter not in "0123456789abcdef" for letter in identity) or identity in invocations):
            raise ValueError("missing or duplicate independent run invocation")
        invocations.add(identity)
        if (invocation.get("version") != 1 or invocation.get("cwd") != str(repository)
                or invocation.get("environment") != env or invocation.get("jobs") != jobs
                or invocation.get("timeout_seconds") != timeout or type(invocation.get("timed_out")) is not bool):
            raise ValueError("run invocation environment or jobs differs from its witness")
        status, error = invocation.get("exit_status"), invocation.get("error")
        if ((status is not None and type(status) is not int)
                or (error is not None and (not isinstance(error, str) or not error))
                or (status is None) != (error is not None)
                or (invocation["timed_out"] and status is not None)):
            raise ValueError("malformed determinism process outcome")
        if status != 0:
            if (entry.get("capture_sha256") is not None
                    or invocation.get("command") != invocation_command(recorded_run, jobs, ["all"], repository)):
                raise ValueError("failed process claims a successful capture or another invocation")
            # Missing logs are missing evidence, even for a measured failure.
            (run_dir / "stdout").read_bytes()
            (run_dir / "stderr").read_bytes()
            valid = False
            details.append({"run": entry["path"], "exit_status": status, "error": error,
                            "timed_out": invocation["timed_out"]})
            continue
        if entry.get("capture_sha256") != digest((run_dir / "capture.json").read_bytes()):
            raise ValueError("determinism capture binding changed")
        capture = corpus.load_capture(run_dir)
        replayed = corpus.replay(run_dir, write=False, capture=capture)
        if not replayed["source_stable"]:
            raise ValueError("stale determinism run build")
        if invocation.get("command") != invocation_command(recorded_run, jobs, capture.metadata["selectors"], repository):
            raise ValueError("run invocation does not identify its actual capture and selection")
        if capture.metadata["partial"] or not corpus.full(document, [row["id"] for row in capture.rows]):
            raise ValueError("partial determinism capture: the full inventory denominator is missing")
        complete = all(row["state"] == "completed" for row in capture.rows)
        valid = valid and complete
        captures.append(capture)
        details.append({"run": entry["path"], "complete": complete,
                        "states": replayed["summary"]["states"],
                        "harness_errors": replayed["summary"]["harness_errors"]})
    differences = []
    if len(captures) == RUN_COUNT:
        baseline = captures[0]
        for number, capture in enumerate(captures[1:], 2):
            if capture.rows != baseline.rows or capture.transcripts != baseline.transcripts:
                valid = False
                differences.append(number)
    else:
        valid = False
    return {"metrics": {"determinism": bool(valid)},
            "identities": {"determinism": digest(report_bytes)},
            "details": {"runs": details, "different_runs": differences}}


def verify_witnesses(directory):
    """Expose malformed nested corpus/build evidence as a withholding error."""
    try:
        return _verify_witnesses(directory)
    except (AttributeError, KeyError, TypeError, IndexError) as error:
        raise ValueError("malformed determinism evidence: " + str(error)) from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    execute = commands.add_parser("run")
    execute.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    execute.add_argument("--timeout", type=float, default=600, help="maximum seconds for each retained process")
    execute.add_argument("--capture", type=Path, help="reuse the authenticated build of a full current corpus capture")
    execute.add_argument("--jobs", type=int, default=max(2, min(8, (os.cpu_count() or 2) - 2)))
    replay = commands.add_parser("replay")
    replay.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    result = (run(args.output, capture=args.capture, jobs=args.jobs, timeout=args.timeout) if args.command == "run"
              else verify_witnesses(args.output))
    print(json.dumps(result, sort_keys=True))
    return 0 if result["metrics"]["determinism"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("phase4 determinism failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
